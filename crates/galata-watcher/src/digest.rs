//! The daily digest: a report whose arrival says the watcher is alive.
//!
//! The lane asks every few minutes; a digest is due once the configured
//! local time has passed on a local date without one. It lists each open
//! position with its PNL over the last 24 hours, whether it has a stop-loss
//! and a take-profit, and its distance to liquidation, then the armed levels and the alerts since the
//! last digest, with how many were not delivered.
//!
//! It keeps its own `digest.json`, so it never writes the check's state.

use std::collections::{BTreeSet, HashMap};
use std::io;

use galata_desk_core::venue::{Info, OpenOrder, Position, Transport};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::alert::{Alert, Kind};
use crate::check::{self, CheckError};
use crate::classify::trigger_kind;
use crate::config::Config;
use crate::daypnl::{day_pnl, usd};
use crate::files::{self, Paths};
use crate::nearliq;
use crate::state::State;

/// When the last digest went, by `digest.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Sent {
    pub last_ms: Option<u64>,
    /// The local date it was for: `2026-11-19`.
    pub last_date: Option<String>,
}

/// One delivery outcome, as galata-notify appends it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Delivery {
    pub alert_id: String,
    pub delivered: bool,
    pub at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug)]
pub struct Outcome {
    pub alert: Alert,
    pub sent: Sent,
}

/// The local date a digest is due for at `now_ms`, if one is due.
pub fn due(config: &Config, sent: &Sent, now_ms: u64) -> Option<String> {
    let tz = TimeZone::get(&config.digest.zone).ok()?;
    let local = Timestamp::from_millisecond(now_ms as i64).ok()?.to_zoned_datetime(tz);
    let date = local.date().to_string();
    let (h, m) = config.digest.hour_minute();
    let reached = (local.hour(), local.minute()) >= (h, m);
    (reached && sent.last_date.as_deref() != Some(date.as_str())).then_some(date)
}

/// Each position's market's dex: `xyz` for `xyz:NVDA`, empty for `BTC`.
fn dex_of(coin: &str) -> &str {
    coin.split_once(':').map_or("", |(dex, _)| dex)
}

struct Covered {
    stop: bool,
    take_profit: bool,
}

fn coverage(orders: &[OpenOrder]) -> HashMap<String, Covered> {
    let mut out: HashMap<String, Covered> = HashMap::new();
    for o in orders.iter().filter(|o| o.is_trigger) {
        let c = out.entry(o.coin.clone()).or_insert(Covered { stop: false, take_profit: false });
        match trigger_kind(&o.order_type) {
            Kind::StopLoss => c.stop = true,
            Kind::TakeProfit => c.take_profit = true,
            _ => {}
        }
    }
    out
}

/// A distance in percent, rounded: whole percent from 10% up, one decimal below.
fn pct(d: Decimal) -> String {
    let dp = if d >= Decimal::TEN { 0 } else { 1 };
    format!("{}%", d.round_dp(dp).normalize())
}

/// The digest's text. `pnl` is each position's 24-hour PNL by market; a
/// market missing from it reads `n/a`.
pub fn text(
    date: &str,
    positions: &[Position],
    pnl: &HashMap<String, Decimal>,
    orders: &[OpenOrder],
    armed: usize,
    alerts: usize,
    undelivered: usize,
) -> String {
    let cover = coverage(orders);
    let mut lines = vec![format!("Daily digest, {date}")];
    if positions.is_empty() {
        lines.push("No open positions.".into());
    }
    for p in positions {
        let side = if p.szi.is_sign_positive() { "long" } else { "short" };
        let c = cover.get(&p.coin);
        let stop = if c.is_some_and(|c| c.stop) { "stop-loss yes" } else { "NO STOP-LOSS" };
        let tp = if c.is_some_and(|c| c.take_profit) { "take-profit yes" } else { "take-profit no" };
        let liq = match nearliq::distance(p) {
            Some(d) => format!("{} from liquidation", pct(d.pct)),
            None => "no liquidation price".into(),
        };
        let day = pnl.get(&p.coin).map_or("n/a".into(), |d| usd(*d));
        lines.push(format!("{} {side}, 24h PnL {day}: {stop}, {tp}, {liq}", p.coin));
    }
    lines.push(format!("Armed call levels: {armed}"));
    if undelivered > 0 {
        lines.push(format!("Alerts since the last digest: {alerts} ({undelivered} not delivered)"));
    } else {
        lines.push(format!("Alerts since the last digest: {alerts}"));
    }
    lines.join("\n")
}

/// Alerts detected after `since_ms` (digests and PnL summaries excluded), and how many of
/// those have no successful delivery.
pub fn counts(paths: &Paths, since_ms: u64) -> io::Result<(usize, usize)> {
    let alerts: Vec<Alert> = files::read_lines(&paths.alerts())?;
    let delivered: BTreeSet<String> = files::read_lines::<Delivery>(&paths.deliveries())?
        .into_iter()
        .filter(|d| d.delivered)
        .map(|d| d.alert_id)
        .collect();
    let recent: Vec<&Alert> = alerts
        .iter()
        .filter(|a| !matches!(a.kind, Kind::Digest | Kind::PnlSummary) && a.detected_ms > since_ms)
        .collect();
    let undelivered = recent.iter().filter(|a| !delivered.contains(&a.id)).count();
    Ok((recent.len(), undelivered))
}

/// The digest, if one is due. Saves nothing; see [`commit`].
pub async fn run<T: Transport>(info: &Info<T>, paths: &Paths, now_ms: u64) -> Result<Option<Outcome>, CheckError> {
    let config = check::load_config(paths)?;
    let sent: Sent = files::read_json(&paths.digest())?.unwrap_or_default();
    let Some(date) = due(&config, &sent, now_ms) else {
        return Ok(None);
    };
    let state = State::load(&paths.state())?;
    let positions = check::positions(info, &config.address).await?;
    let dexes: BTreeSet<&str> = positions.iter().map(|p| dex_of(&p.coin)).collect();
    let mut orders = Vec::new();
    for dex in dexes {
        orders.extend(info.open_orders(&config.address, dex).await?);
    }
    let pnl = day_pnl(info, &config.address, &positions, now_ms).await?;
    let armed = config.levels.iter().filter(|l| state.is_armed(l)).count();
    let since = sent.last_ms.unwrap_or(now_ms.saturating_sub(24 * 60 * 60 * 1000));
    let (alerts, undelivered) = counts(paths, since)?;
    let body = text(&date, &positions, &pnl, &orders, armed, alerts, undelivered);
    Ok(Some(Outcome {
        alert: Alert {
            id: format!("digest-{date}"),
            kind: Kind::Digest,
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
            last_ms: Some(now_ms),
            last_date: Some(date),
        },
    }))
}

/// Record the digest as sent: the alert first, then `digest.json`.
pub fn commit(paths: &Paths, outcome: &Outcome) -> io::Result<()> {
    files::append_line(&paths.alerts(), &outcome.alert)?;
    files::write_json(&paths.digest(), &outcome.sent)
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

    fn position(coin: &str, szi: &str, liq: Option<&str>) -> Position {
        serde_json::from_value(json!({
            "coin": coin, "szi": szi, "leverage": {"type": "cross", "value": 10},
            "entryPx": "100", "positionValue": "100", "unrealizedPnl": "0",
            "returnOnEquity": "0", "liquidationPx": liq, "marginUsed": "1",
            "cumFunding": {"allTime": "0", "sinceOpen": "0", "sinceChange": "0"}
        }))
        .unwrap()
    }

    fn trigger(coin: &str, order_type: &str) -> OpenOrder {
        serde_json::from_value(json!({
            "coin": coin, "side": "A", "limitPx": "1", "sz": "0", "origSz": "0", "oid": 1,
            "timestamp": 0, "orderType": order_type, "reduceOnly": true, "isTrigger": true,
            "triggerCondition": "", "triggerPx": "1", "isPositionTpsl": true
        }))
        .unwrap()
    }

    #[test]
    fn a_digest_is_due_once_a_day_after_its_local_time() {
        let c = cfg("Europe/Istanbul"); // UTC+3
        let none = Sent::default();
        assert_eq!(due(&c, &none, ms("2026-11-19T03:59:00Z")), None, "06:59 local");
        assert_eq!(due(&c, &none, ms("2026-11-19T04:00:00Z")).as_deref(), Some("2026-11-19"));
        let sent = Sent {
            last_ms: Some(0),
            last_date: Some("2026-11-19".into()),
        };
        assert_eq!(due(&c, &sent, ms("2026-11-19T15:00:00Z")), None, "already sent today");
        assert_eq!(due(&c, &sent, ms("2026-11-20T04:05:00Z")).as_deref(), Some("2026-11-20"));
    }

    #[test]
    fn a_position_without_a_stop_is_marked() {
        let pnl = HashMap::from([("BTC".to_string(), Decimal::new(123456, 2))]);
        let t = text(
            "2026-11-19",
            &[position("BTC", "1", Some("80")), position("ETH", "-1", Some("120"))],
            &pnl,
            &[trigger("BTC", "Stop Market")],
            2,
            3,
            1,
        );
        assert_eq!(
            t,
            "Daily digest, 2026-11-19\n\
             BTC long, 24h PnL +$1,235: stop-loss yes, take-profit no, 20% from liquidation\n\
             ETH short, 24h PnL n/a: NO STOP-LOSS, take-profit no, 20% from liquidation\n\
             Armed call levels: 2\n\
             Alerts since the last digest: 3 (1 not delivered)"
        );
    }

    fn d(s: &str) -> Decimal {
        s.parse().unwrap()
    }

    #[test]
    fn distances_are_rounded() {
        assert_eq!(pct(d("54.73")), "55%");
        assert_eq!(pct(d("4.26")), "4.3%");
    }

    #[test]
    fn no_positions_still_makes_a_digest() {
        let t = text("2026-11-19", &[], &HashMap::new(), &[], 0, 0, 0);
        assert!(t.contains("No open positions."));
    }

    #[test]
    fn counts_exclude_digests_and_summaries_and_read_delivery() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let mk = |id: &str, kind: Kind, at: u64| Alert {
            id: id.into(),
            kind,
            market: "BTC".into(),
            side: None,
            size: None,
            price: None,
            event_ms: at,
            detected_ms: at,
            late: false,
            text: String::new(),
        };
        for a in [mk("old", Kind::StopLoss, 5), mk("a", Kind::StopLoss, 20), mk("b", Kind::Level, 30), mk("digest-x", Kind::Digest, 40), mk("pnl-x", Kind::PnlSummary, 50)] {
            files::append_line(&paths.alerts(), &a).unwrap();
        }
        files::append_line(&paths.deliveries(), &Delivery { alert_id: "a".into(), delivered: true, at_ms: 21, error: None }).unwrap();
        files::append_line(&paths.deliveries(), &Delivery { alert_id: "b".into(), delivered: false, at_ms: 31, error: Some("x".into()) }).unwrap();
        assert_eq!(counts(&paths, 10).unwrap(), (2, 1));
    }
}
