//! The API's routes, through the router itself.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use galata_watcher::alert::{Alert, Kind};
use galata_watcher::digest::Delivery;
use galata_watcher::files::{self, Paths};
use galata_watcher::state::{Fired, State};
use galata_watcher_api::{App, router};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const TOKEN: &str = "api-token";
const ME: &str = "0x00000000000000000000000000000000000000aa";

struct Api {
    _dir: tempfile::TempDir,
    paths: Paths,
}

impl Api {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        Api { _dir: dir, paths }
    }

    fn with_config(version: u64) -> Self {
        let api = Api::new();
        let mut doc = config();
        doc["version"] = json!(version);
        let c = galata_watcher::config::check(&doc).unwrap().config;
        files::write_json(&api.paths.config(), &c).unwrap();
        api
    }

    async fn call(&self, method: &str, uri: &str, token: Option<&str>, if_match: Option<&str>, body: Option<Value>) -> (StatusCode, Option<String>, Vec<u8>) {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(t) = token {
            req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
        }
        if let Some(m) = if_match {
            req = req.header(header::IF_MATCH, m);
        }
        let req = req.body(body.map_or(Body::empty(), |b| Body::from(b.to_string()))).unwrap();
        let res = router(App::new(self.paths.clone(), TOKEN.into())).oneshot(req).await.unwrap();
        let status = res.status();
        let tag = res.headers().get(header::ETAG).map(|v| v.to_str().unwrap().to_string());
        let bytes = res.into_body().collect().await.unwrap().to_bytes().to_vec();
        (status, tag, bytes)
    }

    async fn get(&self, uri: &str) -> (StatusCode, Option<String>, Value) {
        let (s, t, b) = self.call("GET", uri, Some(TOKEN), None, None).await;
        (s, t, serde_json::from_slice(&b).unwrap_or(Value::Null))
    }

    async fn put(&self, if_match: Option<&str>, body: Value) -> (StatusCode, Option<String>, Value) {
        let (s, t, b) = self.call("PUT", "/config", Some(TOKEN), if_match, Some(body)).await;
        (s, t, serde_json::from_slice(&b).unwrap_or(Value::Null))
    }
}

fn config() -> Value {
    json!({
        "address": ME,
        "telegram_chat": "777",
        "levels": [{"id": "btc-58k", "market": "BTC", "price": "58000", "direction": "below", "note": "support"}]
    })
}

// 5.1

#[tokio::test]
async fn without_the_token_every_route_is_401_with_an_empty_body() {
    let api = Api::with_config(1);
    for (method, uri) in [("GET", "/config"), ("GET", "/health"), ("GET", "/alerts"), ("PUT", "/config")] {
        let (status, _, body) = api.call(method, uri, None, Some("\"1\""), Some(config())).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {uri}");
        assert!(body.is_empty(), "{method} {uri}");
        let (status, _, body) = api.call(method, uri, Some("wrong"), Some("\"1\""), Some(config())).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(body.is_empty());
    }
}

#[tokio::test]
async fn an_unknown_route_without_the_token_says_nothing_either() {
    let api = Api::new();
    let (status, _, body) = api.call("GET", "/anything", None, None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(body.is_empty());
}

// 5.2

#[tokio::test]
async fn reading_gives_every_setting_its_version_and_level_states() {
    let api = Api::with_config(3);
    let (status, tag, body) = api.get("/config").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(tag.as_deref(), Some("\"3\""));
    assert_eq!(body["version"], 3);
    assert_eq!(body["near_liquidation_pct"], "15");
    assert_eq!(body["levels"][0]["state"], "armed");
}

#[tokio::test]
async fn the_first_write_creates_version_1() {
    let api = Api::new();
    assert_eq!(api.get("/config").await.0, StatusCode::NOT_FOUND);
    let (status, tag, body) = api.put(Some("\"0\""), config()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(tag.as_deref(), Some("\"1\""));
    assert_eq!(body["version"], 1);
}

#[tokio::test]
async fn a_write_naming_no_version_is_428_and_changes_nothing() {
    let api = Api::with_config(7);
    let before = std::fs::read(api.paths.config()).unwrap();
    assert_eq!(api.put(None, config()).await.0, StatusCode::PRECONDITION_REQUIRED);
    assert_eq!(std::fs::read(api.paths.config()).unwrap(), before);
}

#[tokio::test]
async fn of_two_editors_of_version_7_the_second_is_refused_and_the_first_stands() {
    let api = Api::with_config(7);
    let mut first = config();
    first["near_liquidation_pct"] = json!("10");
    let mut second = config();
    second["near_liquidation_pct"] = json!("20");

    let (status, tag, _) = api.put(Some("\"7\""), first).await;
    assert_eq!((status, tag.as_deref()), (StatusCode::OK, Some("\"8\"")));
    let (status, tag, _) = api.put(Some("\"7\""), second).await;
    assert_eq!((status, tag.as_deref()), (StatusCode::PRECONDITION_FAILED, Some("\"8\"")));
    let (_, _, now) = api.get("/config").await;
    assert_eq!((now["version"].as_u64(), now["near_liquidation_pct"].as_str()), (Some(8), Some("10")));
}

#[tokio::test]
async fn an_80_percent_distance_is_422_naming_it() {
    let api = Api::with_config(1);
    let mut doc = config();
    doc["near_liquidation_pct"] = json!(80);
    let (status, _, body) = api.put(Some("\"1\""), doc).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body["problems"][0].as_str().unwrap().starts_with("`near_liquidation_pct`"));
    assert_eq!(api.get("/config").await.2["version"], 1);
}

#[tokio::test]
async fn switching_off_liquidation_alerts_is_422_naming_the_field() {
    let api = Api::with_config(1);
    let mut doc = config();
    doc["near_liquidation_alerts"] = json!(false);
    let (status, _, body) = api.put(Some("\"1\""), doc).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body["problems"][0].as_str().unwrap().contains("`near_liquidation_alerts`"));
}

#[tokio::test]
async fn a_config_json_without_the_pnl_summary_setting_reads_it_as_on() {
    let api = Api::new();
    let mut doc = config();
    doc["version"] = json!(1);
    let mut stored = serde_json::to_value(galata_watcher::config::check(&doc).unwrap().config).unwrap();
    stored.as_object_mut().unwrap().remove("pnl_summary");
    files::write_json(&api.paths.config(), &stored).unwrap();
    assert_eq!(api.get("/config").await.2["pnl_summary"], true);
}

#[tokio::test]
async fn the_pnl_summary_can_be_switched_off_and_read_back() {
    let api = Api::with_config(1);
    let mut doc = config();
    doc["pnl_summary"] = json!(false);
    let (status, _, _) = api.put(Some("\"1\""), doc).await;
    assert_eq!(status, StatusCode::OK);
    let (_, _, read) = api.get("/config").await;
    assert_eq!((read["pnl_summary"].clone(), read["version"].clone()), (json!(false), json!(2)));
}

#[tokio::test]
async fn a_pnl_summary_setting_that_is_not_a_switch_is_422_naming_it() {
    let api = Api::with_config(1);
    let mut doc = config();
    doc["pnl_summary"] = json!("sometimes");
    let (status, _, body) = api.put(Some("\"1\""), doc).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body["problems"][0].as_str().unwrap().starts_with("`pnl_summary`"));
    assert_eq!(api.get("/config").await.2["version"], 1);
}

#[tokio::test]
async fn the_watchlist_and_market_update_switch_are_written_and_read_back() {
    let api = Api::with_config(1);
    let (_, _, before) = api.get("/config").await;
    assert_eq!((before["market_update"].clone(), before["watchlist"].clone()), (json!(true), json!([])));
    let mut doc = config();
    doc["market_update"] = json!(false);
    doc["watchlist"] = json!(["BTC", "xyz:NVDA"]);
    let (status, _, _) = api.put(Some("\"1\""), doc.clone()).await;
    assert_eq!(status, StatusCode::OK);
    let (_, _, read) = api.get("/config").await;
    assert_eq!((read["market_update"].clone(), read["watchlist"].clone()), (json!(false), json!(["BTC", "xyz:NVDA"])));

    doc["watchlist"] = json!(["BTC", 7]);
    let (status, _, body) = api.put(Some("\"2\""), doc).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body["problems"][0].as_str().unwrap().starts_with("`watchlist[1]`"));
    assert_eq!(api.get("/config").await.2["version"], 2);
}

fn fired_at_0312(api: &Api, generation: u64) {
    let mut s = State::default();
    s.fired.insert("btc-58k".into(), Fired { generation, at_ms: 1_795_057_920_000 });
    s.save(&api.paths.state()).unwrap();
}

#[tokio::test]
async fn a_fired_level_written_back_as_armed_is_re_armed() {
    let api = Api::with_config(1);
    fired_at_0312(&api, 0);
    let (_, _, before) = api.get("/config").await;
    assert_eq!(before["levels"][0]["state"], "fired");
    assert_eq!(before["levels"][0]["fired_at"], 1_795_057_920_000u64);

    let mut doc = before.clone();
    doc["levels"][0]["state"] = json!("armed");
    let (status, _, after) = api.put(Some("\"1\""), doc).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(after["levels"][0]["state"], "armed");
    assert!(after["levels"][0].get("fired_at").is_none());
    assert_eq!(after["levels"][0]["generation"], 1);
}

#[tokio::test]
async fn a_fired_level_written_back_unchanged_stays_fired() {
    let api = Api::with_config(1);
    fired_at_0312(&api, 0);
    let (_, _, doc) = api.get("/config").await;
    let (_, _, after) = api.put(Some("\"1\""), doc).await;
    assert_eq!(after["levels"][0]["state"], "fired");
}

#[tokio::test]
async fn an_armed_level_cannot_be_marked_fired() {
    let api = Api::with_config(1);
    let mut doc = config();
    doc["levels"][0]["state"] = json!("fired");
    let (status, _, body) = api.put(Some("\"1\""), doc).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body["problems"][0].as_str().unwrap().contains("cannot be marked fired"));
}

#[tokio::test]
async fn a_writer_cannot_set_a_generation() {
    let api = Api::with_config(1);
    let mut doc = config();
    doc["levels"][0]["generation"] = json!(42);
    let (_, _, after) = api.put(Some("\"1\""), doc).await;
    assert_eq!(after["levels"][0]["generation"], 0);
}

// 5.3

fn alert(id: &str, kind: Kind, at: u64) -> Alert {
    Alert {
        id: id.into(),
        kind,
        market: "BTC".into(),
        side: None,
        size: None,
        price: None,
        event_ms: at,
        detected_ms: at,
        late: id == "late",
        text: format!("text of {id}"),
        html: None,
    }
}

#[tokio::test]
async fn alerts_are_listed_newest_first_with_delivery_state() {
    let api = Api::with_config(1);
    for a in [alert("old", Kind::StopLoss, 10), alert("late", Kind::StopLoss, 20), alert("lvl", Kind::Level, 30), alert("lvl", Kind::Level, 30)] {
        files::append_line(&api.paths.alerts(), &a).unwrap();
    }
    files::append_line(&api.paths.deliveries(), &Delivery { alert_id: "late".into(), delivered: true, at_ms: 21, error: None }).unwrap();
    files::append_line(&api.paths.deliveries(), &Delivery { alert_id: "lvl".into(), delivered: false, at_ms: 31, error: Some("429".into()) }).unwrap();

    let (status, _, body) = api.get("/alerts?since=15").await;
    assert_eq!(status, StatusCode::OK);
    let rows = body["alerts"].as_array().unwrap();
    let ids: Vec<_> = rows.iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["lvl", "late"], "newest first, since 15, a re-logged alert once");
    assert_eq!(rows[0]["delivered"], false);
    assert_eq!(rows[1]["delivered"], true);
    assert_eq!(rows[1]["late"], true);

    let (_, _, all) = api.get("/alerts").await;
    let old = &all["alerts"].as_array().unwrap()[2];
    assert_eq!((old["id"].as_str(), old["delivered"].as_bool()), (Some("old"), Some(false)), "no delivery line reads as not delivered");
}

#[tokio::test]
async fn health_reports_the_last_success_and_error() {
    let api = Api::with_config(1);
    files::write_json(
        &api.paths.health(),
        &json!({"last_ok_ms": 100, "last_error": "venue: no connection", "last_error_ms": 200, "config_version": 1, "address": ME}),
    )
    .unwrap();
    let (status, _, body) = api.get("/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["last_ok_ms"], 100);
    assert_eq!(body["last_error"], "venue: no connection");
    assert_eq!(body["config_version"], 1);
}
