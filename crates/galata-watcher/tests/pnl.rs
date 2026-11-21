//! One PnL summary against a fake venue answering with recorded fixtures:
//! what it says, that it is sent once per slot, and what it saves.

use galata_desk_core::venue::{Info, Transport, VenueError};
use galata_watcher::alert::{Alert, Kind};
use galata_watcher::files::{self, Paths};
use galata_watcher::pnl::{self, Sent};
use serde_json::{Value, json};

const ME: &str = "0x00000000000000000000000000000000000000aa";
const DAY: u64 = 24 * 60 * 60 * 1000;
/// The BTC fixture's 05:00 UTC candle on 2026-11-17, a day before `now`.
const CANDLE_05_00: u64 = 1_794_891_600_000;

fn fixture(name: &str) -> Value {
    let text = std::fs::read_to_string(format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap();
    serde_json::from_str(&text).unwrap()
}

#[derive(Clone)]
struct Venue {
    down: bool,
}

impl Transport for Venue {
    async fn post(&self, body: Value) -> Result<Vec<u8>, VenueError> {
        if self.down {
            return Err(VenueError::Unreachable("no connection".into()));
        }
        let answer = match body["type"].as_str().unwrap_or("") {
            "perpDexs" => json!([null]),
            "clearinghouseState" => fixture("clearinghouseState.json"),
            "portfolio" => fixture("portfolio.json"),
            "userFillsByTime" | "userFunding" => json!([]),
            "userFills" => fixture("userFillsByTime_triggers.json"),
            "candleSnapshot" if body["req"]["coin"] == "BTC" => fixture("candleSnapshot_BTC.json"),
            "candleSnapshot" => json!([]),
            other => panic!("unexpected query {other}"),
        };
        Ok(serde_json::to_vec(&answer).unwrap())
    }
}

/// Tokyo, so the fixture's 05:00 UTC is 14:00 local, outside quiet hours.
fn data() -> (tempfile::TempDir, Paths) {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    files::write_json(
        &paths.config(),
        &json!({"address": ME, "telegram_chat": "1", "digest": {"time": "07:00", "zone": "Asia/Tokyo"}}),
    )
    .unwrap();
    (dir, paths)
}

#[tokio::test]
async fn a_summary_is_made_once_per_slot() {
    let (_d, paths) = data();
    let now = CANDLE_05_00 + DAY + 2 * 60_000; // 14:02 in Tokyo
    let info = Info::new(Venue { down: false });

    let outcome = pnl::run(&info, &paths, now).await.unwrap().expect("the 14:00 slot is due");
    let a = &outcome.alert;
    assert_eq!(a.id, "pnl-2026-11-18T14:00");
    assert_eq!(a.kind, Kind::PnlSummary);
    let lines: Vec<&str> = a.text.lines().collect();
    assert_eq!(
        &lines[..5],
        ["PnL summary, 14:00", "24h -$8,027", "30D +$72,487", "All-time +$4,416,429", "Open positions:"]
    );
    // BTC has a price a day ago in the fixture; the others do not.
    assert!(lines[5].starts_with("BTC short ") && !lines[5].ends_with("n/a"), "{}", a.text);
    assert_eq!(lines[6], "ETH short n/a");
    assert!(!a.text.contains("Closed / other"), "a position without a price drops it");
    // Then the last three trades, one line each.
    let at = lines.iter().position(|l| *l == "Last trades:").expect("a trades section");
    assert_eq!(lines.len(), at + 4, "{}", a.text);
    let html = a.html.as_deref().expect("sent as HTML");
    assert!(html.starts_with("📊 <b>PnL summary, 14:00</b>\n<pre>"), "{html}");
    assert!(html.contains("<b>Last trades</b>\n<pre>"), "{html}");

    pnl::commit(&paths, &outcome).unwrap();
    let saved: Vec<Alert> = files::read_lines(&paths.alerts()).unwrap();
    assert_eq!(saved, vec![outcome.alert.clone()]);
    let sent: Sent = files::read_json(&paths.pnl()).unwrap().unwrap();
    assert_eq!(sent.last_slot.as_deref(), Some("2026-11-18T14:00"));

    assert!(pnl::run(&info, &paths, now + 60_000).await.unwrap().is_none(), "not twice for one slot");
}

#[tokio::test]
async fn nothing_is_asked_of_the_venue_when_no_slot_is_due() {
    let (_d, paths) = data();
    let quiet = CANDLE_05_00 + DAY + 10 * 60 * 60 * 1000; // 00:00 in Tokyo
    let info = Info::new(Venue { down: true });
    assert!(pnl::run(&info, &paths, quiet).await.unwrap().is_none());
}

#[tokio::test]
async fn a_venue_failure_saves_nothing() {
    let (_d, paths) = data();
    let now = CANDLE_05_00 + DAY + 2 * 60_000;
    let info = Info::new(Venue { down: true });
    assert!(pnl::run(&info, &paths, now).await.is_err());
    assert!(files::read_json::<Sent>(&paths.pnl()).unwrap().is_none());
}

#[tokio::test]
async fn switched_off_nothing_is_asked_and_nothing_saved() {
    let (_d, paths) = data();
    let mut config: serde_json::Value = files::read_json(&paths.config()).unwrap().unwrap();
    config["pnl_summary"] = json!(false);
    files::write_json(&paths.config(), &config).unwrap();
    let now = CANDLE_05_00 + DAY + 2 * 60_000; // 14:02 in Tokyo, a due slot
    // A venue that is down: any question to it would fail the run.
    let info = Info::new(Venue { down: true });
    assert!(pnl::run(&info, &paths, now).await.unwrap().is_none());
    assert!(files::read_json::<Sent>(&paths.pnl()).unwrap().is_none());
}
