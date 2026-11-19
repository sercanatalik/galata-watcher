//! One check of the account: everything new since the last one.
//!
//! [`run`] asks the venue and decides, but saves nothing. The caller prints
//! the alerts and only then calls [`commit`], so a crash in between sends an
//! alert twice rather than losing it. A check that fails saves nothing but
//! its failure in `health.json`.

use std::collections::HashMap;
use std::io;

use galata_desk_core::venue::{Info, Position, Transport, VenueError};
use serde::{Deserialize, Serialize};

use crate::alert::Alert;
use crate::classify;
use crate::config::{self, Config};
use crate::files::{self, Paths};
use crate::fills;
use crate::levels;
use crate::nearliq;
use crate::state::{OVERLAP_MS, State};

/// Fills come at most this many to an answer.
const PAGE: usize = 2_000;
/// Pages read in one check before giving up on catching up further.
const MAX_PAGES: usize = 5;

#[derive(Debug, thiserror::Error)]
pub enum CheckError {
    /// The configuration is missing or invalid: exit 2.
    #[error("configuration: {}", .0.join("; "))]
    Config(Vec<String>),
    /// The venue did not answer: exit 1.
    #[error("venue: {0}")]
    Venue(#[from] VenueError),
    /// The data volume could not be read or written: exit 1.
    #[error("data: {0}")]
    Io(#[from] io::Error),
}

/// What the latest check found out about itself, for `/health`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Health {
    pub last_ok_ms: Option<u64>,
    pub last_error: Option<String>,
    pub last_error_ms: Option<u64>,
    pub config_version: Option<u64>,
    pub address: Option<String>,
}

/// A check's result, not yet saved.
#[derive(Debug)]
pub struct Outcome {
    pub alerts: Vec<Alert>,
    pub state: State,
    pub config: Config,
}

/// Read and check `config.json`.
pub fn load_config(paths: &Paths) -> Result<Config, CheckError> {
    let doc: serde_json::Value = files::read_json(&paths.config())?
        .ok_or_else(|| CheckError::Config(vec![format!("{} does not exist", paths.config().display())]))?;
    config::check(&doc).map(|c| c.config).map_err(CheckError::Config)
}

/// Every open position, on every perpetuals dex.
pub async fn positions<T: Transport>(info: &Info<T>, user: &str) -> Result<Vec<Position>, VenueError> {
    let mut out = Vec::new();
    for dex in info.perp_dexs().await? {
        let state = info.clearinghouse_state(user, &dex).await?;
        out.extend(state.asset_positions.into_iter().map(|p| p.position).filter(|p| !p.szi.is_zero()));
    }
    Ok(out)
}

/// Check the account once, as of `now_ms`.
pub async fn run<T: Transport>(transport: &T, info: &Info<T>, paths: &Paths, now_ms: u64) -> Result<Outcome, CheckError> {
    let config = load_config(paths)?;
    let mut state = State::load(&paths.state())?;
    let user = config.address.as_str();
    let mut alerts = Vec::new();

    // Fills: from the cursor, minus the overlap. The first check ever starts
    // at its own time and reports nothing older.
    let mut start = match state.cursor_ms {
        Some(cursor) => cursor.saturating_sub(OVERLAP_MS),
        None => now_ms,
    };
    let mut new_fills = Vec::new();
    for _ in 0..MAX_PAGES {
        let page = fills::fills_since(transport, user, start).await?;
        let full = page.len() >= PAGE;
        let last = page.last().map(|f| f.time);
        new_fills.extend(page.into_iter().filter(|f| !state.seen.contains_key(&f.tid) && f.time >= start));
        match (full, last) {
            (true, Some(t)) if t > start => start = t,
            _ => break,
        }
    }
    new_fills.sort_by_key(|f| (f.time, f.tid));
    new_fills.dedup_by_key(|f| f.tid);

    let needs_orders = new_fills.iter().any(|f| f.liquidation.is_none());
    let orders = if needs_orders {
        Some(classify::order_kinds(&info.historical_orders(user).await?))
    } else {
        None
    };
    let classified = classify::classify(&new_fills, orders.as_ref(), &config, now_ms);

    // Positions near liquidation.
    let open = positions(info, user).await?;
    let mut near = state.near.clone();
    let near_alerts = nearliq::check(&open, config.near_liquidation_pct, &mut near, now_ms);

    // Call levels, over the minutes since the previous check.
    let window = levels::window_start(state.last_ok_ms, now_ms);
    let mut candles = HashMap::new();
    for market in levels::markets_to_read(&config.levels, &state) {
        let cs = info.candle_snapshot(&market, "1m", window.saturating_sub(60_000), now_ms).await?;
        candles.insert(market, cs);
    }

    // Every question has been answered: now decide the next state.
    let level_alerts = levels::check(&config.levels, &candles, &mut state, window, now_ms);
    alerts.extend(classified.alerts);
    alerts.extend(near_alerts);
    alerts.extend(level_alerts);

    state.near = near;
    for (tid, time) in classified.handled {
        state.seen.insert(tid, time);
    }
    let next_cursor = classified.held_times.iter().copied().min().unwrap_or(now_ms).min(now_ms);
    state.cursor_ms = Some(next_cursor);
    state.prune_seen(next_cursor);
    state.last_ok_ms = Some(now_ms);

    Ok(Outcome { alerts, state, config })
}

/// Save a check's outcome: alerts first, then state, then health.
pub fn commit(paths: &Paths, outcome: &Outcome) -> io::Result<()> {
    for a in &outcome.alerts {
        files::append_line(&paths.alerts(), a)?;
    }
    outcome.state.save(&paths.state())?;
    files::write_json(
        &paths.health(),
        &Health {
            last_ok_ms: outcome.state.last_ok_ms,
            last_error: None,
            last_error_ms: None,
            config_version: Some(outcome.config.version),
            address: Some(outcome.config.address.clone()),
        },
    )
}

/// Record a failed check in `health.json`, keeping the last success.
pub fn record_failure(paths: &Paths, error: &CheckError, now_ms: u64) -> io::Result<()> {
    let mut health: Health = files::read_json(&paths.health())?.unwrap_or_default();
    health.last_error = Some(error.to_string());
    health.last_error_ms = Some(now_ms);
    files::write_json(&paths.health(), &health)
}
