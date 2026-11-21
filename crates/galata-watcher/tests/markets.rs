//! One market update against a fake venue: what it says, what it asks, that
//! it is sent once per slot, and what it saves.

use std::sync::{Arc, Mutex};

use galata_desk_core::venue::{Transport, VenueError};
use galata_watcher::alert::{Alert, Kind};
use galata_watcher::files::{self, Paths};
use galata_watcher::markets;
use galata_watcher::pnl::Sent;
use serde_json::{Value, json};

const ME: &str = "0x00000000000000000000000000000000000000aa";
/// 14:17 in London on 2026-11-20: inside the 14:15 slot.
const NOW: u64 = 1_795_184_220_000;

/// Answers `metaAndAssetCtxs` in the venue's shape, and records each dex asked.
#[derive(Clone, Default)]
struct Venue {
    down: bool,
    asked: Arc<Mutex<Vec<String>>>,
}

impl Transport for Venue {
    async fn post(&self, body: Value) -> Result<Vec<u8>, VenueError> {
        if self.down {
            return Err(VenueError::Unreachable("no connection".into()));
        }
        assert_eq!(body["type"], "metaAndAssetCtxs", "nothing else is asked: {body}");
        let dex = body["dex"].as_str().unwrap().to_string();
        let answer = match dex.as_str() {
            "" => json!([
                {"universe": [{"name": "BTC", "szDecimals": 5, "maxLeverage": 40}, {"name": "ETH", "szDecimals": 4, "maxLeverage": 25}]},
                [
                    {"funding": "0.0000053875", "openInterest": "37690.7182", "prevDayPx": "64609.0", "dayNtlVlm": "3862264071.04",
                     "premium": "-0.0004319188", "oraclePx": "64827.0", "markPx": "64796.1", "midPx": "64798.5", "impactPxs": ["64790.0", "64799.0"]},
                    {"funding": "0.0", "openInterest": "1.0", "prevDayPx": "3000.0", "dayNtlVlm": "1.0",
                     "premium": null, "oraclePx": "3100.0", "markPx": "3100.0", "midPx": "3100.5", "impactPxs": null}
                ]
            ]),
            "xyz" => json!([
                {"universe": [{"name": "xyz:NVDA", "szDecimals": 3, "maxLeverage": 10}]},
                // No mid: the mark stands in.
                [{"funding": "0.0000215729", "openInterest": "2673690.002", "prevDayPx": "161.37", "dayNtlVlm": "504176558.87",
                  "premium": null, "oraclePx": "157.52", "markPx": "157.63", "midPx": null}]
            ]),
            other => panic!("unexpected dex {other}"),
        };
        self.asked.lock().unwrap().push(dex);
        Ok(serde_json::to_vec(&answer).unwrap())
    }
}

fn data(extra: Value) -> (tempfile::TempDir, Paths) {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    let mut config = json!({"address": ME, "telegram_chat": "1", "digest": {"time": "07:00", "zone": "Europe/London"},
        "watchlist": ["xyz:NVDA", "BTC", "DELISTED"]});
    for (k, v) in extra.as_object().unwrap() {
        config[k] = v.clone();
    }
    files::write_json(&paths.config(), &config).unwrap();
    (dir, paths)
}

#[tokio::test]
async fn an_update_lists_each_watched_market_once_per_slot() {
    let (_d, paths) = data(json!({}));
    let venue = Venue::default();

    let outcome = markets::run(&venue, &paths, NOW).await.unwrap().expect("the 14:15 slot is due");
    let a = &outcome.alert;
    assert_eq!(a.id, "markets-2026-11-20T14:15");
    assert_eq!(a.kind, Kind::MarketUpdate);
    assert_eq!(a.text, "Markets, 14:15\nxyz:NVDA 157.63 -2.32%\nBTC 64,798.5 +0.29%\nDELISTED n/a n/a");
    let html = a.html.as_deref().expect("sent as HTML");
    assert!(html.starts_with("📈 <b>Markets, 14:15</b>\n<blockquote><code>xyz:NVDA"), "{html}");
    assert_eq!(*venue.asked.lock().unwrap(), ["", "xyz"], "one question per dex");
    assert!(!a.text.to_lowercase().contains(&ME[2..]), "nothing about the account");

    markets::commit(&paths, &outcome).unwrap();
    let saved: Vec<Alert> = files::read_lines(&paths.alerts()).unwrap();
    assert_eq!(saved, vec![outcome.alert.clone()]);
    let sent: Sent = files::read_json(&paths.markets()).unwrap().unwrap();
    assert_eq!(sent.last_slot.as_deref(), Some("2026-11-20T14:15"));
    assert!(files::read_json::<Sent>(&paths.pnl()).unwrap().is_none(), "the PnL summary's record is its own");

    assert!(markets::run(&venue, &paths, NOW + 60_000).await.unwrap().is_none(), "not twice for one slot");
    let next = markets::run(&venue, &paths, NOW + 13 * 60_000).await.unwrap().expect("14:30 is the next slot");
    assert_eq!(next.alert.id, "markets-2026-11-20T14:30");
}

#[tokio::test]
async fn nothing_is_asked_of_the_venue_when_no_update_is_due() {
    // A venue that is down: any question to it would fail the run.
    let down = Venue { down: true, ..Venue::default() };
    let (_d, paths) = data(json!({}));
    let quiet = NOW + 10 * 60 * 60 * 1000; // 00:17 in London
    assert!(markets::run(&down, &paths, quiet).await.unwrap().is_none());

    let (_d, paths) = data(json!({"market_update": false}));
    assert!(markets::run(&down, &paths, NOW).await.unwrap().is_none(), "switched off");

    let (_d, paths) = data(json!({"watchlist": []}));
    assert!(markets::run(&down, &paths, NOW).await.unwrap().is_none(), "nothing watched");
    assert!(files::read_json::<Sent>(&paths.markets()).unwrap().is_none());
}

#[tokio::test]
async fn a_venue_failure_saves_nothing() {
    let (_d, paths) = data(json!({}));
    let down = Venue { down: true, ..Venue::default() };
    assert!(markets::run(&down, &paths, NOW).await.is_err());
    assert!(files::read_json::<Sent>(&paths.markets()).unwrap().is_none());
}
