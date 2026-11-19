//! One check against a fake venue: what it finds, what it saves, and what it
//! leaves alone when the venue fails.

use std::sync::{Arc, Mutex};

use galata_desk_core::venue::{Info, Transport, VenueError};
use galata_watcher::alert::Kind;
use galata_watcher::check::{self, CheckError, Health};
use galata_watcher::files::{self, Paths};
use galata_watcher::state::State;
use serde_json::{Value, json};

const ME: &str = "0x00000000000000000000000000000000000000aa";
const MIN: u64 = 60_000;
/// 2026-11-19 00:00 UTC.
const DAY: u64 = 1_795_046_400_000;

#[derive(Clone, Default)]
struct Venue(Arc<Mutex<Inner>>);

#[derive(Default)]
struct Inner {
    fills: Vec<Value>,
    orders: Vec<Value>,
    positions: Vec<Value>,
    down: bool,
    asked: Vec<String>,
}

impl Venue {
    fn with(f: impl FnOnce(&mut Inner)) -> Self {
        let v = Venue::default();
        f(&mut v.0.lock().unwrap());
        v
    }
    fn asked(&self, kind: &str) -> usize {
        self.0.lock().unwrap().asked.iter().filter(|k| *k == kind).count()
    }
}

impl Transport for Venue {
    async fn post(&self, body: Value) -> Result<Vec<u8>, VenueError> {
        let mut v = self.0.lock().unwrap();
        let kind = body["type"].as_str().unwrap_or("").to_string();
        v.asked.push(kind.clone());
        if v.down {
            return Err(VenueError::Unreachable("no connection".into()));
        }
        let answer = match kind.as_str() {
            "userFillsByTime" => {
                let start = body["startTime"].as_u64().unwrap();
                Value::Array(v.fills.iter().filter(|f| f["time"].as_u64().unwrap() >= start).cloned().collect())
            }
            "historicalOrders" => Value::Array(v.orders.clone()),
            "perpDexs" => json!([null]),
            "clearinghouseState" => json!({
                "marginSummary": {"accountValue": "1000", "totalNtlPos": "0", "totalMarginUsed": "0"},
                "withdrawable": "0",
                "assetPositions": v.positions.iter().map(|p| json!({"type": "oneWay", "position": p})).collect::<Vec<_>>()
            }),
            "candleSnapshot" => json!([]),
            other => panic!("unexpected query {other}"),
        };
        Ok(serde_json::to_vec(&answer).unwrap())
    }
}

fn stop_fill(oid: u64, tid: u64, time: u64) -> Value {
    json!({"coin": "BTC", "px": "61240", "sz": "0.5", "side": "A", "time": time, "tid": tid, "oid": oid,
           "dir": "Close Long", "crossed": true, "fee": "1", "closedPnl": "-10", "hash": "0x", "startPosition": "0.5"})
}

fn stop_order(oid: u64) -> Value {
    json!({"order": {"coin": "BTC", "side": "A", "limitPx": "61000", "sz": "0", "origSz": "0.5", "oid": oid,
                     "timestamp": 0, "orderType": "Stop Market", "reduceOnly": true, "isTrigger": true,
                     "triggerCondition": "Price below 61300", "triggerPx": "61300", "isPositionTpsl": true},
           "status": "triggered", "statusTimestamp": 0})
}

fn data() -> (tempfile::TempDir, Paths) {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    files::write_json(&paths.config(), &json!({"address": ME, "telegram_chat": "1"})).unwrap();
    (dir, paths)
}

fn state_at(paths: &Paths, last_ok: u64) {
    State {
        cursor_ms: Some(last_ok),
        last_ok_ms: Some(last_ok),
        ..State::default()
    }
    .save(&paths.state())
    .unwrap();
}

async fn run(venue: &Venue, paths: &Paths, now: u64) -> Result<check::Outcome, CheckError> {
    check::run(venue, &Info::new(venue.clone()), paths, now).await
}

#[tokio::test]
async fn a_stop_fill_is_found_once_and_not_again() {
    let (_d, paths) = data();
    let now = DAY + 3 * 60 * MIN;
    state_at(&paths, now - MIN);
    let venue = Venue::with(|v| {
        v.fills.push(stop_fill(9, 1, now - 30_000));
        v.orders.push(stop_order(9));
    });

    let first = run(&venue, &paths, now).await.unwrap();
    assert_eq!(first.alerts.len(), 1);
    assert_eq!(first.alerts[0].kind, Kind::StopLoss);
    assert_eq!(first.alerts[0].text, "BTC stop-loss filled: sold 0.5 at 61,240");
    assert!(!first.alerts[0].late);
    check::commit(&paths, &first).unwrap();

    let again = run(&venue, &paths, now + MIN).await.unwrap();
    assert!(again.alerts.is_empty(), "a re-run prints nothing again");
    let logged: Vec<Value> = files::read_lines(&paths.alerts()).unwrap();
    assert_eq!(logged.len(), 1);
}

#[tokio::test]
async fn a_quiet_minute_finds_nothing_and_asks_for_no_orders() {
    let (_d, paths) = data();
    state_at(&paths, DAY);
    let venue = Venue::default();
    let out = run(&venue, &paths, DAY + MIN).await.unwrap();
    assert!(out.alerts.is_empty());
    assert_eq!(venue.asked("historicalOrders"), 0);
}

#[tokio::test]
async fn a_venue_failure_leaves_state_untouched_and_records_health() {
    let (_d, paths) = data();
    state_at(&paths, DAY);
    let before = std::fs::read(paths.state()).unwrap();
    let venue = Venue::with(|v| v.down = true);
    let err = run(&venue, &paths, DAY + MIN).await.unwrap_err();
    assert!(matches!(err, CheckError::Venue(_)));
    check::record_failure(&paths, &err, DAY + MIN).unwrap();
    assert_eq!(std::fs::read(paths.state()).unwrap(), before);
    let health: Health = files::read_json(&paths.health()).unwrap().unwrap();
    assert!(health.last_error.unwrap().contains("could not be reached"));
}

#[tokio::test]
async fn an_invalid_configuration_is_a_config_error() {
    let (_d, paths) = data();
    files::write_json(&paths.config(), &json!({"address": "nope"})).unwrap();
    let err = run(&Venue::default(), &paths, DAY).await.unwrap_err();
    assert!(matches!(err, CheckError::Config(_)));
}

#[tokio::test]
async fn the_first_check_ever_reports_nothing_older_than_itself() {
    let (_d, paths) = data();
    let now = DAY + 60 * MIN;
    let venue = Venue::with(|v| {
        v.fills.push(stop_fill(9, 1, now - 2 * MIN));
        v.orders.push(stop_order(9));
    });
    let out = run(&venue, &paths, now).await.unwrap();
    assert!(out.alerts.is_empty());
    assert_eq!(out.state.cursor_ms, Some(now));
}

#[tokio::test]
async fn a_stop_that_filled_overnight_while_down_is_caught_up_marked_late() {
    let (_d, paths) = data();
    // Last check at 01:00; down until 06:00; the stop filled at 03:12.
    state_at(&paths, DAY + 60 * MIN);
    let at_0312 = DAY + (3 * 60 + 12) * MIN;
    let venue = Venue::with(|v| {
        v.fills.push(stop_fill(9, 1, at_0312));
        v.orders.push(stop_order(9));
    });
    let out = run(&venue, &paths, DAY + 6 * 60 * MIN).await.unwrap();
    assert_eq!(out.alerts.len(), 1);
    let a = &out.alerts[0];
    assert!(a.late);
    assert_eq!(a.event_ms, at_0312);
    assert_eq!(a.text, "[late: happened 2026-11-19 03:12 UTC] BTC stop-loss filled: sold 0.5 at 61,240");
}

#[tokio::test]
async fn a_position_near_liquidation_is_alerted_with_the_check() {
    let (_d, paths) = data();
    state_at(&paths, DAY);
    let venue = Venue::with(|v| {
        v.positions.push(json!({
            "coin": "ETH", "szi": "2", "leverage": {"type": "cross", "value": 10}, "entryPx": "3100",
            "positionValue": "6000", "unrealizedPnl": "0", "returnOnEquity": "0", "liquidationPx": "2600",
            "marginUsed": "600", "cumFunding": {"allTime": "0", "sinceOpen": "0", "sinceChange": "0"}
        }));
    });
    let out = run(&venue, &paths, DAY + MIN).await.unwrap();
    assert_eq!(out.alerts.len(), 1);
    assert_eq!(out.alerts[0].kind, Kind::NearLiquidation);
}
