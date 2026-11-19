//! The watcher's configuration and alert history over HTTP.
//!
//! Four routes, every one behind the bearer token, `/health` included:
//!
//! | Route | Does |
//! |---|---|
//! | `GET /config` | the configuration, each level with its state, and an `ETag` of its version |
//! | `PUT /config` | replace it whole; `If-Match` must name the current version |
//! | `GET /alerts?since=<ms>` | alerts detected since then, newest first, with delivery state |
//! | `GET /health` | the last successful check, the last error, the version in use |
//!
//! Nothing here talks to the venue. The only file it writes is `config.json`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::{Query, Request, State as Extract};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use galata_watcher::alert::Alert;
use galata_watcher::check::Health;
use galata_watcher::config::{self, Config, StatedLevel};
use galata_watcher::digest::Delivery;
use galata_watcher::files::{self, Paths};
use galata_watcher::state::State;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct App {
    paths: Arc<Paths>,
    token: Arc<String>,
    /// One write at a time, so a version check and its write are one step.
    writing: Arc<Mutex<()>>,
}

impl App {
    pub fn new(paths: Paths, token: String) -> Self {
        Self {
            paths: Arc::new(paths),
            token: Arc::new(token),
            writing: Arc::new(Mutex::new(())),
        }
    }
}

pub fn router(app: App) -> Router {
    Router::new()
        .route("/config", get(get_config).put(put_config))
        .route("/alerts", get(get_alerts))
        .route("/health", get(get_health))
        .layer(middleware::from_fn_with_state(app.clone(), require_token))
        .with_state(app)
}

/// Compare without stopping at the first difference.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn require_token(Extract(app): Extract<App>, request: Request, next: Next) -> Response {
    let given = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    if given.is_empty() || !same(given.as_bytes(), app.token.as_bytes()) {
        // Nothing about the configuration, the alerts or the account.
        return StatusCode::UNAUTHORIZED.into_response();
    }
    next.run(request).await
}

fn internal(e: impl std::fmt::Display) -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, axum::Json(json!({"error": e.to_string()}))).into_response()
}

fn etag(version: u64) -> HeaderValue {
    HeaderValue::from_str(&format!("\"{version}\"")).expect("digits and quotes")
}

/// The saved configuration, or `None` before the first write.
fn current(paths: &Paths) -> std::io::Result<Option<Config>> {
    files::read_json(&paths.config())
}

/// The configuration as read: each level with its state and when it fired.
fn view(config: &Config, state: &State) -> Value {
    let mut doc = serde_json::to_value(config).expect("a configuration serialises");
    if let Some(levels) = doc["levels"].as_array_mut() {
        for (row, level) in levels.iter_mut().zip(&config.levels) {
            match state.fired_at(level) {
                Some(at) => {
                    row["state"] = json!("fired");
                    row["fired_at"] = json!(at);
                }
                None => row["state"] = json!("armed"),
            }
        }
    }
    doc
}

fn with_etag(version: u64, body: Value) -> Response {
    let mut response = axum::Json(body).into_response();
    response.headers_mut().insert(header::ETAG, etag(version));
    response
}

async fn get_config(Extract(app): Extract<App>) -> Response {
    let state = match State::load(&app.paths.state()) {
        Ok(s) => s,
        Err(e) => return internal(e),
    };
    match current(&app.paths) {
        Ok(Some(config)) => with_etag(config.version, view(&config, &state)),
        Ok(None) => {
            let mut r = (StatusCode::NOT_FOUND, axum::Json(json!({"error": "no configuration yet: PUT one with If-Match: \"0\""}))).into_response();
            r.headers_mut().insert(header::ETAG, etag(0));
            r
        }
        Err(e) => internal(e),
    }
}

/// The version an `If-Match` header names: `"7"`, `W/"7"` or `7`.
fn if_match(headers: &HeaderMap) -> Option<Result<u64, ()>> {
    let raw = headers.get(header::IF_MATCH)?.to_str().ok()?.trim();
    let raw = raw.strip_prefix("W/").unwrap_or(raw).trim_matches('"');
    Some(raw.parse().map_err(|_| ()))
}

async fn put_config(Extract(app): Extract<App>, headers: HeaderMap, body: Body) -> Response {
    let Some(named) = if_match(&headers) else {
        return (StatusCode::PRECONDITION_REQUIRED, axum::Json(json!({"error": "If-Match must name the version this write replaces"}))).into_response();
    };
    let _one_at_a_time = app.writing.lock().await;
    let saved = match current(&app.paths) {
        Ok(c) => c,
        Err(e) => return internal(e),
    };
    let version_now = saved.as_ref().map_or(0, |c| c.version);
    if named != Ok(version_now) {
        let mut r = (StatusCode::PRECONDITION_FAILED, axum::Json(json!({"error": "the configuration has changed since it was read", "version": version_now}))).into_response();
        r.headers_mut().insert(header::ETAG, etag(version_now));
        return r;
    }

    let bytes = match axum::body::to_bytes(body, 1 << 20).await {
        Ok(b) => b,
        Err(_) => return (StatusCode::PAYLOAD_TOO_LARGE, axum::Json(json!({"error": "the body is too large"}))).into_response(),
    };
    let doc: Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) => return (StatusCode::UNPROCESSABLE_ENTITY, axum::Json(json!({"problems": [format!("not JSON: {e}")]}))).into_response(),
    };
    let checked = match config::check(&doc) {
        Ok(c) => c,
        Err(problems) => return (StatusCode::UNPROCESSABLE_ENTITY, axum::Json(json!({"problems": problems}))).into_response(),
    };
    let state = match State::load(&app.paths.state()) {
        Ok(s) => s,
        Err(e) => return internal(e),
    };

    let (mut config, problems) = settle_levels(checked.config, &checked.stated, saved.as_ref(), &state);
    if !problems.is_empty() {
        return (StatusCode::UNPROCESSABLE_ENTITY, axum::Json(json!({"problems": problems}))).into_response();
    }
    config.version = version_now + 1;
    if let Err(e) = files::write_json(&app.paths.config(), &config) {
        return internal(e);
    }
    with_etag(config.version, view(&config, &state))
}

/// Give each written level the generation it should have. A level already
/// saved keeps its generation, raised by one when the writer marks it armed
/// while it has fired: that is a re-arm. A writer may not mark a level fired
/// that has not.
fn settle_levels(mut config: Config, stated: &BTreeMap<String, StatedLevel>, saved: Option<&Config>, state: &State) -> (Config, Vec<String>) {
    let mut problems = Vec::new();
    for (i, level) in config.levels.iter_mut().enumerate() {
        let before = saved.and_then(|s| s.levels.iter().find(|l| l.id == level.id));
        level.generation = match before {
            Some(b) => b.generation,
            // A new level, or one deleted and added back: start past any
            // firing still on record under its id.
            None => state.fired.get(&level.id).map_or(0, |f| f.generation + 1),
        };
        let fired = state.fired_at(level).is_some();
        match stated.get(&level.id).copied().unwrap_or(StatedLevel::Unstated) {
            StatedLevel::Armed if fired => level.generation += 1,
            StatedLevel::Fired if !fired => problems.push(format!("`levels[{i}].state`: a level cannot be marked fired; it fires when price crosses it")),
            _ => {}
        }
    }
    (config, problems)
}

#[derive(Deserialize)]
struct Since {
    since: Option<u64>,
}

async fn get_alerts(Extract(app): Extract<App>, Query(q): Query<Since>) -> Response {
    let alerts: Vec<Alert> = match files::read_lines(&app.paths.alerts()) {
        Ok(a) => a,
        Err(e) => return internal(e),
    };
    let deliveries: Vec<Delivery> = match files::read_lines(&app.paths.deliveries()) {
        Ok(d) => d,
        Err(e) => return internal(e),
    };
    let delivered: BTreeSet<&str> = deliveries.iter().filter(|d| d.delivered).map(|d| d.alert_id.as_str()).collect();
    let since = q.since.unwrap_or(0);
    let mut seen = BTreeSet::new();
    let mut rows: Vec<Value> = Vec::new();
    // Newest first; an alert logged twice (re-sent after a crash) once.
    for a in alerts.iter().rev().filter(|a| a.detected_ms >= since) {
        if !seen.insert(a.id.as_str()) {
            continue;
        }
        let mut row = serde_json::to_value(a).expect("an alert serialises");
        row["delivered"] = json!(delivered.contains(a.id.as_str()));
        rows.push(row);
    }
    axum::Json(json!({ "alerts": rows })).into_response()
}

async fn get_health(Extract(app): Extract<App>) -> Response {
    match files::read_json::<Health>(&app.paths.health()) {
        Ok(h) => axum::Json(h.unwrap_or_default()).into_response(),
        Err(e) => internal(e),
    }
}
