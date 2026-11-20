//! The PnL summary: every half hour outside the night, the account's PnL
//! over the venue's rolling 24h, 30D and all-time windows, then each open
//! position's 24-hour PnL.
//!
//! The lane asks on the hour and the half hour. A summary is due when the
//! local time (in the digest's zone) is within [`GRACE_MS`] of a slot's
//! start, the slot is outside [`QUIET_FROM`]..[`QUIET_UNTIL`], and
//! `pnl.json` does not already record it. A slot missed is never sent late.
//!
//! It keeps its own `pnl.json`, so it never writes another mode's state.

use std::collections::HashMap;
use std::io;

use galata_desk_core::venue::{Info, PortfolioHistory, Position, Transport};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::alert::{Alert, Kind};
use crate::check::{self, CheckError};
use crate::config::Config;
use crate::daypnl::{day_pnl, usd};
use crate::files::{self, Paths};

/// Quiet hours, local: no slot starting at or after 23:00 and before 07:00.
pub const QUIET_FROM: i8 = 23;
pub const QUIET_UNTIL: i8 = 7;
/// How long after a slot's start it may still be sent.
pub const GRACE_MS: u64 = 10 * 60 * 1000;

/// When the last summary went, by `pnl.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Sent {
    /// The local slot it was for: `2026-11-20T14:30`.
    pub last_slot: Option<String>,
    pub last_ms: Option<u64>,
}

/// A half-hour slot, local.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    /// `2026-11-20T14:30`: the alert's id and `pnl.json`'s record.
    pub id: String,
    /// `14:30`: what the text shows.
    pub time: String,
}

#[derive(Debug)]
pub struct Outcome {
    pub alert: Alert,
    pub sent: Sent,
}

/// The slot a summary is due for at `now_ms`, if one is due. None while
/// the summary is switched off.
pub fn due(config: &Config, sent: &Sent, now_ms: u64) -> Option<Slot> {
    if !config.pnl_summary {
        return None;
    }
    let tz = TimeZone::get(&config.digest.zone).ok()?;
    let local = Timestamp::from_millisecond(now_ms as i64).ok()?.to_zoned_datetime(tz);
    let (h, m) = (local.hour(), local.minute());
    if h >= QUIET_FROM || h < QUIET_UNTIL {
        return None;
    }
    let into = (m % 30) as u64 * 60_000 + local.second() as u64 * 1000 + local.millisecond() as u64;
    if into >= GRACE_MS {
        return None;
    }
    let time = format!("{h:02}:{:02}", m - m % 30);
    let id = format!("{}T{time}", local.date());
    (sent.last_slot.as_deref() != Some(id.as_str())).then_some(Slot { id, time })
}

/// The account's PnL over the venue's rolling windows, perps, spot and
/// vaults together. `None` where the venue has no history.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Windows {
    pub day: Option<Decimal>,
    pub month: Option<Decimal>,
    pub all_time: Option<Decimal>,
}

impl Windows {
    pub fn from_portfolio(p: &PortfolioHistory) -> Self {
        let last = |key: &str| p.period(key).and_then(|h| h.pnl_history.last()).map(|(_, v)| *v);
        Windows {
            day: last("day"),
            month: last("month"),
            all_time: last("allTime"),
        }
    }
}

fn or_na(d: Option<Decimal>) -> String {
    d.map_or("n/a".into(), usd)
}

/// The summary's text. `pnl` is each position's 24-hour PNL by market; a
/// market missing from it reads `n/a`.
pub fn text(time: &str, windows: &Windows, positions: &[Position], pnl: &HashMap<String, Decimal>) -> String {
    let mut lines = vec![
        format!("PnL summary, {time}"),
        format!("24h {}", or_na(windows.day)),
        format!("30D {}", or_na(windows.month)),
        format!("All-time {}", or_na(windows.all_time)),
    ];
    if positions.is_empty() {
        lines.push("No open positions.".into());
        return lines.join("\n");
    }
    lines.push("Open positions:".into());
    for p in positions {
        let side = if p.szi.is_sign_positive() { "long" } else { "short" };
        lines.push(format!("{} {side} {}", p.coin, or_na(pnl.get(&p.coin).copied())));
    }
    let listed: Option<Decimal> = positions.iter().map(|p| pnl.get(&p.coin).copied()).sum();
    if let (Some(day), Some(listed)) = (windows.day, listed) {
        lines.push(format!("Closed / other {}", usd(day - listed)));
    }
    lines.join("\n")
}

/// The summary, if one is due. Saves nothing; see [`commit`].
pub async fn run<T: Transport>(info: &Info<T>, paths: &Paths, now_ms: u64) -> Result<Option<Outcome>, CheckError> {
    let config = check::load_config(paths)?;
    let sent: Sent = files::read_json(&paths.pnl())?.unwrap_or_default();
    let Some(slot) = due(&config, &sent, now_ms) else {
        return Ok(None);
    };
    let positions = check::positions(info, &config.address).await?;
    let windows = Windows::from_portfolio(&info.portfolio(&config.address).await?);
    let pnl = day_pnl(info, &config.address, &positions, now_ms).await?;
    let body = text(&slot.time, &windows, &positions, &pnl);
    Ok(Some(Outcome {
        alert: Alert {
            id: format!("pnl-{}", slot.id),
            kind: Kind::PnlSummary,
            market: String::new(),
            side: None,
            size: None,
            price: None,
            event_ms: now_ms,
            detected_ms: now_ms,
            late: false,
            text: body,
        },
        sent: Sent {
            last_slot: Some(slot.id),
            last_ms: Some(now_ms),
        },
    }))
}

/// Record the summary as sent: the alert first, then `pnl.json`.
pub fn commit(paths: &Paths, outcome: &Outcome) -> io::Result<()> {
    files::append_line(&paths.alerts(), &outcome.alert)?;
    files::write_json(&paths.pnl(), &outcome.sent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;
    use serde_json::json;

    fn cfg(zone: &str) -> Config {
        config::check(&json!({
            "address": "0x00000000000000000000000000000000000000aa",
            "telegram_chat": "1",
            "digest": {"time": "07:00", "zone": zone}
        }))
        .unwrap()
        .config
    }

    fn ms(rfc3339: &str) -> u64 {
        rfc3339.parse::<Timestamp>().unwrap().as_millisecond() as u64
    }

    fn slot(id: &str) -> Option<String> {
        Some(id.to_string())
    }

    fn due_id(c: &Config, sent: &Sent, at: &str) -> Option<String> {
        due(c, sent, ms(at)).map(|s| s.id)
    }

    fn position(coin: &str, szi: &str) -> Position {
        serde_json::from_value(json!({
            "coin": coin, "szi": szi, "leverage": {"type": "cross", "value": 10},
            "entryPx": "100", "positionValue": "100", "unrealizedPnl": "0",
            "returnOnEquity": "0", "liquidationPx": null, "marginUsed": "1",
            "cumFunding": {"allTime": "0", "sinceOpen": "0", "sinceChange": "0"}
        }))
        .unwrap()
    }

    fn d(s: &str) -> Decimal {
        s.parse().unwrap()
    }

    fn windows(day: &str, month: &str, all: &str) -> Windows {
        Windows {
            day: Some(d(day)),
            month: Some(d(month)),
            all_time: Some(d(all)),
        }
    }

    #[test]
    fn a_missing_pnl_json_reads_as_nothing_sent() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let sent: Sent = files::read_json(&paths.pnl()).unwrap().unwrap_or_default();
        assert_eq!(sent, Sent::default());
    }

    #[test]
    fn slots_fall_on_the_hour_and_half_hour_outside_quiet_hours() {
        // London is UTC+0 in November.
        let c = cfg("Europe/London");
        let none = Sent::default();
        assert_eq!(due_id(&c, &none, "2026-11-20T22:30:05Z"), slot("2026-11-20T22:30"), "the last of the evening");
        assert_eq!(due_id(&c, &none, "2026-11-20T23:00:05Z"), None, "23:00 is quiet");
        assert_eq!(due_id(&c, &none, "2026-11-20T23:30:05Z"), None, "23:30 is quiet");
        assert_eq!(due_id(&c, &none, "2026-11-21T06:30:05Z"), None, "06:30 is quiet");
        assert_eq!(due_id(&c, &none, "2026-11-21T07:00:05Z"), slot("2026-11-21T07:00"), "the first of the morning");
        assert_eq!(due(&c, &none, ms("2026-11-20T14:30:05Z")).unwrap().time, "14:30");
    }

    #[test]
    fn nothing_is_due_while_switched_off() {
        let mut c = cfg("Europe/London");
        assert!(due(&c, &Sent::default(), ms("2026-11-20T14:00:05Z")).is_some());
        c.pnl_summary = false;
        assert_eq!(due_id(&c, &Sent::default(), "2026-11-20T14:00:05Z"), None);
    }

    #[test]
    fn a_slot_is_sent_once() {
        let c = cfg("Europe/London");
        let sent = Sent {
            last_slot: slot("2026-11-20T14:00"),
            last_ms: Some(0),
        };
        assert_eq!(due_id(&c, &sent, "2026-11-20T14:03:00Z"), None, "already sent");
        assert_eq!(due_id(&c, &sent, "2026-11-20T14:30:00Z"), slot("2026-11-20T14:30"));
    }

    #[test]
    fn a_slot_is_not_sent_past_its_grace() {
        let c = cfg("Europe/London");
        let none = Sent::default();
        assert_eq!(due_id(&c, &none, "2026-11-20T14:09:59Z"), slot("2026-11-20T14:00"));
        assert_eq!(due_id(&c, &none, "2026-11-20T14:11:00Z"), None, "14:11 is past the grace");
        assert_eq!(due_id(&c, &none, "2026-11-20T14:29:00Z"), None);
    }

    #[test]
    fn slots_and_quiet_hours_follow_the_configured_zone() {
        // New York is UTC-5 in November: 03:30 UTC is 22:30 there.
        let c = cfg("America/New_York");
        let none = Sent::default();
        assert_eq!(due_id(&c, &none, "2026-11-21T03:30:00Z"), slot("2026-11-20T22:30"));
        assert_eq!(due_id(&c, &none, "2026-11-21T04:00:00Z"), None, "23:00 in New York");
        assert_eq!(due_id(&c, &none, "2026-11-20T11:00:00Z"), None, "06:00 in New York");
        assert_eq!(due_id(&c, &none, "2026-11-20T12:00:00Z"), slot("2026-11-20T07:00"));
    }

    #[test]
    fn three_windows_are_signed_and_rounded() {
        let t = text("14:30", &windows("1234.6", "-4102.2", "18870"), &[], &HashMap::new());
        assert_eq!(t, "PnL summary, 14:30\n24h +$1,235\n30D -$4,102\nAll-time +$18,870\nNo open positions.");
    }

    #[test]
    fn a_window_without_history_reads_n_a() {
        let w = Windows {
            all_time: None,
            ..windows("10", "20", "0")
        };
        let t = text("09:00", &w, &[], &HashMap::new());
        assert!(t.contains("All-time n/a"), "{t}");
    }

    #[test]
    fn positions_are_listed_with_side_and_day_pnl_and_reconciled() {
        let pnl = HashMap::from([("BTC".to_string(), d("912")), ("ETH".to_string(), d("-140"))]);
        let t = text(
            "14:30",
            &windows("1235", "-4102", "18870"),
            &[position("BTC", "0.5"), position("ETH", "-2")],
            &pnl,
        );
        assert_eq!(
            t,
            "PnL summary, 14:30\n24h +$1,235\n30D -$4,102\nAll-time +$18,870\n\
             Open positions:\nBTC long +$912\nETH short -$140\nClosed / other +$463"
        );
    }

    #[test]
    fn a_position_without_a_day_old_price_reads_n_a_and_drops_the_reconciliation() {
        let pnl = HashMap::from([("BTC".to_string(), d("912"))]);
        let t = text("14:30", &windows("1235", "0", "0"), &[position("BTC", "1"), position("ETH", "-1")], &pnl);
        assert!(t.contains("ETH short n/a"), "{t}");
        assert!(!t.contains("Closed / other"), "{t}");
    }

    #[test]
    fn no_day_figure_drops_the_reconciliation() {
        let pnl = HashMap::from([("BTC".to_string(), d("912"))]);
        let w = Windows {
            day: None,
            ..windows("0", "1", "2")
        };
        let t = text("14:30", &w, &[position("BTC", "1")], &pnl);
        assert!(t.contains("24h n/a") && !t.contains("Closed / other"), "{t}");
    }
}
