//! Each open position's PNL over the last 24 hours: the price move, the
//! trades and their fees, and funding. Shared by the daily digest and the
//! PnL summary, so the two always give the same figure.

use std::collections::HashMap;

use galata_desk_core::venue::{Info, Position, Side, Transport, UserFill, UserFundingRow};
use rust_decimal::Decimal;

use crate::alert;
use crate::check::CheckError;

pub const DAY_MS: u64 = 24 * 60 * 60 * 1000;
/// Fills come at most this many to an answer, funding rows at most 500.
const FILLS_PAGE: usize = 2_000;
const FUNDING_PAGE: usize = 500;
/// Pages read before giving up on the rest of the day.
const MAX_PAGES: usize = 5;

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

/// The account's fills from `start_ms` on, as many pages as [`MAX_PAGES`].
pub async fn fills_since<T: Transport>(info: &Info<T>, user: &str, start_ms: u64) -> Result<Vec<UserFill>, CheckError> {
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
pub async fn funding_since<T: Transport>(info: &Info<T>, user: &str, start_ms: u64) -> Result<Vec<UserFundingRow>, CheckError> {
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
pub async fn price_at<T: Transport>(info: &Info<T>, coin: &str, at_ms: u64) -> Result<Option<Decimal>, CheckError> {
    let candles = info.candle_snapshot(coin, "1m", at_ms.saturating_sub(60 * 60 * 1000), at_ms).await?;
    Ok(candles.iter().filter(|c| c.t <= at_ms).max_by_key(|c| c.t).map(|c| c.c))
}

/// Each position's PNL over the 24 hours before `now_ms`, by market.
/// A market whose price a day ago is not to be had is left out.
pub async fn day_pnl<T: Transport>(info: &Info<T>, user: &str, positions: &[Position], now_ms: u64) -> Result<HashMap<String, Decimal>, CheckError> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn position(coin: &str, szi: &str) -> Position {
        serde_json::from_value(json!({
            "coin": coin, "szi": szi, "leverage": {"type": "cross", "value": 10},
            "entryPx": "100", "positionValue": "100", "unrealizedPnl": "0",
            "returnOnEquity": "0", "liquidationPx": null, "marginUsed": "1",
            "cumFunding": {"allTime": "0", "sinceOpen": "0", "sinceChange": "0"}
        }))
        .unwrap()
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
        let p = position("BTC", "1");
        let pnl = window_pnl(&p, &[], &[funding("BTC", "-0.5"), funding("ETH", "9")], Some(d("90")));
        assert_eq!(pnl, Some(d("9.5")));
        // Short the same: the move is a loss.
        let p = position("BTC", "-1");
        assert_eq!(window_pnl(&p, &[], &[], Some(d("90"))), Some(d("-10")));
    }

    #[test]
    fn a_position_opened_today_counts_from_its_fills_and_needs_no_old_price() {
        // Bought 1 at 95 with a 0.1 fee; now worth 100.
        let p = position("BTC", "1");
        let fills = [fill("BTC", "B", "1", "95", "0.1"), fill("ETH", "B", "5", "1", "0")];
        assert_eq!(window_pnl(&p, &fills, &[], None), Some(d("4.9")));
    }

    #[test]
    fn a_position_added_to_counts_both_parts() {
        // Held 1 from 90, bought 1 more at 98; 2 now worth 200 (mark 100).
        let mut p = position("BTC", "2");
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
    }
}
