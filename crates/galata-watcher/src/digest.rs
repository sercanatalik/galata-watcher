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

use galata_desk_core::venue::{Info, OpenOrder, Position, Side, Transport, UserFill, UserFundingRow};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::alert::{self, Alert, Kind};
use crate::check::{self, CheckError};
use crate::classify::trigger_kind;
use crate::config::Config;
use crate::files::{self, Paths};
use crate::nearliq;
use crate::state::State;

const DAY_MS: u64 = 24 * 60 * 60 * 1000;
/// Fills come at most this many to an answer, funding rows at most 500.
const FILLS_PAGE: usize = 2_000;
const FUNDING_PAGE: usize = 500;
/// Pages read before giving up on the rest of the day.
const MAX_PAGES: usize = 5;

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

/// A position's PNL over the window `fills` and `funding` cover: what it is
/// worth now, less what it was worth at the start (at `price_then`), less
/// what was paid for the trades in between and their fees, plus the funding
/// it received. A position opened inside the window needs no `price_then`.
/// `None` when one is needed and missing.
pub fn window_pnl(p: &Position, fills: &[UserFill], funding: &[UserFundingRow], price_then: Option<Decimal>) -> Option<Decimal> {
    if p.szi.is_zero() {
        return None;
    }
    let mut bought = Decimal::ZERO;
    let mut paid = Decimal::ZERO;
    for f in fills.iter().filter(|f| f.coin == p.coin) {
        let signed = match f.side {
            Side::Buy => f.sz,
            Side::Sell => -f.sz,
        };
        bought += signed;
        paid += signed * f.px + f.fee;
    }
    let received: Decimal = funding.iter().filter(|r| r.delta.coin == p.coin).map(|r| r.delta.usdc).sum();
    let size_then = p.szi - bought;
    let value_then = if size_then.is_zero() { Decimal::ZERO } else { size_then * price_then? };
    // position_value is |size| x mark.
    let value_now = if p.szi.is_sign_positive() { p.position_value } else { -p.position_value };
    Some(value_now - value_then - paid + received)
}

/// Dollars as a person reads them, signed: `+$1,235`, `-$12.40`. Whole
/// dollars from $100 up.
pub fn usd(d: Decimal) -> String {
    let sign = if d.is_sign_negative() && !d.round_dp(2).is_zero() { "-" } else { "+" };
    let a = d.abs();
    if a >= Decimal::ONE_HUNDRED {
        format!("{sign}${}", alert::amount(a.round()))
    } else {
        format!("{sign}${:.2}", a.round_dp(2))
    }
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

/// Alerts detected after `since_ms` (digests excluded), and how many of
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
        .filter(|a| a.kind != Kind::Digest && a.detected_ms > since_ms)
        .collect();
    let undelivered = recent.iter().filter(|a| !delivered.contains(&a.id)).count();
    Ok((recent.len(), undelivered))
}

/// The account's fills from `start_ms` on, as many pages as [`MAX_PAGES`].
async fn fills_since<T: Transport>(info: &Info<T>, user: &str, start_ms: u64) -> Result<Vec<UserFill>, CheckError> {
    let mut out: Vec<UserFill> = Vec::new();
    let mut from = start_ms;
    for _ in 0..MAX_PAGES {
        let page = info.user_fills_by_time(user, from).await?;
        let full = page.len() >= FILLS_PAGE;
        let last = page.iter().map(|f| f.time).max();
        out.extend(page);
        match last {
            // The next page starts at the last fill's time, which may repeat fills; dedup below.
            Some(t) if full && t > from => from = t,
            _ => break,
        }
    }
    out.sort_by_key(|f| (f.time, f.tid));
    out.dedup_by_key(|f| f.tid);
    Ok(out)
}

/// The account's funding payments from `start_ms` on, as many pages as [`MAX_PAGES`].
async fn funding_since<T: Transport>(info: &Info<T>, user: &str, start_ms: u64) -> Result<Vec<UserFundingRow>, CheckError> {
    let mut out = Vec::new();
    let mut from = start_ms;
    for _ in 0..MAX_PAGES {
        let page = info.user_funding(user, from, None).await?;
        let full = page.len() >= FUNDING_PAGE;
        let last = page.iter().map(|r| r.time).max();
        out.extend(page);
        match last {
            Some(t) if full => from = t + 1,
            _ => break,
        }
    }
    Ok(out)
}

/// A market's price at `at_ms`: the close of the last minute candle that
/// opened in the hour before it.
async fn price_at<T: Transport>(info: &Info<T>, coin: &str, at_ms: u64) -> Result<Option<Decimal>, CheckError> {
    let candles = info.candle_snapshot(coin, "1m", at_ms.saturating_sub(60 * 60 * 1000), at_ms).await?;
    Ok(candles.iter().filter(|c| c.t <= at_ms).max_by_key(|c| c.t).map(|c| c.c))
}

/// Each position's PNL over the 24 hours before `now_ms`, by market.
/// A market whose price a day ago is not to be had is left out.
async fn day_pnl<T: Transport>(info: &Info<T>, user: &str, positions: &[Position], now_ms: u64) -> Result<HashMap<String, Decimal>, CheckError> {
    let start = now_ms.saturating_sub(DAY_MS);
    let fills = fills_since(info, user, start).await?;
    let funding = funding_since(info, user, start).await?;
    let mut out = HashMap::new();
    for p in positions {
        let price_then = price_at(info, &p.coin, start).await?;
        if let Some(d) = window_pnl(p, &fills, &funding, price_then) {
            out.insert(p.coin.clone(), d);
        }
    }
    Ok(out)
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

    fn fill(coin: &str, side: &str, sz: &str, px: &str, fee: &str) -> UserFill {
        serde_json::from_value(json!({
            "coin": coin, "px": px, "sz": sz, "side": side, "time": 1, "closedPnl": "0",
            "fee": fee, "tid": 1
        }))
        .unwrap()
    }

    fn funding(coin: &str, usdc: &str) -> UserFundingRow {
        serde_json::from_value(json!({
            "time": 1, "delta": {"type": "funding", "coin": coin, "usdc": usdc, "szi": "1", "fundingRate": "0"}
        }))
        .unwrap()
    }

    fn d(s: &str) -> Decimal {
        s.parse().unwrap()
    }

    #[test]
    fn a_position_held_all_day_makes_the_price_move_plus_funding() {
        // 1 BTC long, now worth 100; it was 90 a day ago.
        let p = position("BTC", "1", None);
        let pnl = window_pnl(&p, &[], &[funding("BTC", "-0.5"), funding("ETH", "9")], Some(d("90")));
        assert_eq!(pnl, Some(d("9.5")));
        // Short the same: the move is a loss.
        let p = position("BTC", "-1", None);
        assert_eq!(window_pnl(&p, &[], &[], Some(d("90"))), Some(d("-10")));
    }

    #[test]
    fn a_position_opened_today_counts_from_its_fills_and_needs_no_old_price() {
        // Bought 1 at 95 with a 0.1 fee; now worth 100.
        let p = position("BTC", "1", None);
        let fills = [fill("BTC", "B", "1", "95", "0.1"), fill("ETH", "B", "5", "1", "0")];
        assert_eq!(window_pnl(&p, &fills, &[], None), Some(d("4.9")));
    }

    #[test]
    fn a_position_added_to_counts_both_parts() {
        // Held 1 from 90, bought 1 more at 98; 2 now worth 200 (mark 100).
        let mut p = position("BTC", "2", None);
        p.position_value = d("200");
        let fills = [fill("BTC", "B", "1", "98", "0")];
        assert_eq!(window_pnl(&p, &fills, &[], Some(d("90"))), Some(d("12")));
        assert_eq!(window_pnl(&p, &fills, &[], None), None, "needs the old price");
    }

    #[test]
    fn dollars_are_signed_and_rounded() {
        assert_eq!(usd(d("1234.56")), "+$1,235");
        assert_eq!(usd(d("-98765.4")), "-$98,765");
        assert_eq!(usd(d("12.4")), "+$12.40");
        assert_eq!(usd(d("-0.004")), "+$0.00");
        assert_eq!(pct(d("54.73")), "55%");
        assert_eq!(pct(d("4.26")), "4.3%");
    }

    #[test]
    fn no_positions_still_makes_a_digest() {
        let t = text("2026-11-19", &[], &HashMap::new(), &[], 0, 0, 0);
        assert!(t.contains("No open positions."));
    }

    #[test]
    fn counts_exclude_digests_and_read_delivery() {
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
        for a in [mk("old", Kind::StopLoss, 5), mk("a", Kind::StopLoss, 20), mk("b", Kind::Level, 30), mk("digest-x", Kind::Digest, 40)] {
            files::append_line(&paths.alerts(), &a).unwrap();
        }
        files::append_line(&paths.deliveries(), &Delivery { alert_id: "a".into(), delivered: true, at_ms: 21, error: None }).unwrap();
        files::append_line(&paths.deliveries(), &Delivery { alert_id: "b".into(), delivered: false, at_ms: 31, error: Some("x".into()) }).unwrap();
        assert_eq!(counts(&paths, 10).unwrap(), (2, 1));
    }
}
