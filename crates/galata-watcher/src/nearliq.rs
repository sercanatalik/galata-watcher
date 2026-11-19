//! Positions close to their liquidation price.
//!
//! Distance is `|mark - liquidation| / mark`, in percent. The mark is the
//! position's value over its size, so no separate price is asked for. A
//! market alerted once is quiet for [`QUIET_MS`], unless its distance falls
//! below half the configured one; one market's alert never quiets another's.

use std::collections::BTreeMap;

use galata_desk_core::venue::Position;
use rust_decimal::Decimal;

use crate::alert::{self, Alert, Kind, Side};
use crate::state::NearSent;

pub const QUIET_MS: u64 = 30 * 60 * 1000;

/// A position's distance to liquidation, with the prices it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct Distance {
    pub market: String,
    pub long: bool,
    pub mark: Decimal,
    pub liquidation: Decimal,
    pub pct: Decimal,
}

/// `None` for a flat position, or one that cannot be liquidated.
pub fn distance(p: &Position) -> Option<Distance> {
    let liquidation = p.liquidation_px?;
    if p.szi.is_zero() || p.position_value.is_zero() {
        return None;
    }
    let mark = p.position_value / p.szi.abs();
    let pct = ((mark - liquidation).abs() / mark) * Decimal::ONE_HUNDRED;
    Some(Distance {
        market: p.coin.clone(),
        long: p.szi.is_sign_positive(),
        mark,
        liquidation,
        pct,
    })
}

/// Near-liquidation alerts for these positions, updating `sent`.
pub fn check(positions: &[Position], threshold_pct: Decimal, sent: &mut BTreeMap<String, NearSent>, now_ms: u64) -> Vec<Alert> {
    let half = threshold_pct / Decimal::TWO;
    let mut out = Vec::new();
    for d in positions.iter().filter_map(distance) {
        if d.pct > threshold_pct {
            continue;
        }
        if let Some(last) = sent.get(&d.market) {
            let quiet = now_ms.saturating_sub(last.at_ms) < QUIET_MS;
            let much_worse = d.pct < half && last.distance_pct >= half;
            if quiet && !much_worse {
                continue;
            }
        }
        sent.insert(
            d.market.clone(),
            NearSent {
                at_ms: now_ms,
                distance_pct: d.pct.round_dp(4),
            },
        );
        out.push(Alert {
            id: format!("near-{}-{now_ms}", d.market),
            kind: Kind::NearLiquidation,
            market: d.market.clone(),
            side: Some(if d.long { Side::Buy } else { Side::Sell }),
            size: None,
            price: Some(d.mark.round_dp(6).normalize()),
            event_ms: now_ms,
            detected_ms: now_ms,
            late: false,
            text: alert::near_text(&d.market, d.mark, d.liquidation, d.pct),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const MIN: u64 = 60_000;

    /// A position as the venue lists it, at `mark` with `liq`.
    fn position(coin: &str, szi: &str, mark: &str, liq: Option<&str>) -> Position {
        let szi_d: Decimal = szi.parse().unwrap();
        let value = szi_d.abs() * mark.parse::<Decimal>().unwrap();
        serde_json::from_value(json!({
            "coin": coin, "szi": szi, "leverage": {"type": "cross", "value": 10},
            "entryPx": mark, "positionValue": value.to_string(), "unrealizedPnl": "0",
            "returnOnEquity": "0", "liquidationPx": liq, "marginUsed": "1",
            "cumFunding": {"allTime": "0", "sinceOpen": "0", "sinceChange": "0"}
        }))
        .unwrap()
    }

    fn pct(s: &str) -> Decimal {
        s.parse().unwrap()
    }

    #[test]
    fn the_eth_example_is_13_3_percent_and_alerted() {
        let mut sent = BTreeMap::new();
        let out = check(&[position("ETH", "2", "3000", Some("2600"))], pct("15"), &mut sent, 0);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "ETH near liquidation: mark 3,000, liquidation 2,600, 13.3% away");
    }

    #[test]
    fn outside_the_zone_nothing_is_sent() {
        let mut sent = BTreeMap::new();
        let ps = [position("BTC", "1", "60000", Some("40000")), position("ETH", "-1", "3000", None)];
        assert!(check(&ps, pct("15"), &mut sent, 0).is_empty());
    }

    #[test]
    fn still_near_ten_minutes_later_is_quiet() {
        let mut sent = BTreeMap::new();
        check(&[position("ETH", "1", "3000", Some("2610"))], pct("15"), &mut sent, 0); // 13%
        let later = check(&[position("ETH", "1", "3000", Some("2640"))], pct("15"), &mut sent, 10 * MIN); // 12%
        assert!(later.is_empty());
    }

    #[test]
    fn getting_much_worse_is_alerted_inside_the_quiet_time() {
        let mut sent = BTreeMap::new();
        check(&[position("ETH", "1", "3000", Some("2610"))], pct("15"), &mut sent, 0); // 13%
        let later = check(&[position("ETH", "1", "3000", Some("2790"))], pct("15"), &mut sent, 10 * MIN); // 7%
        assert_eq!(later.len(), 1);
        // And then quiet again at 6%.
        let again = check(&[position("ETH", "1", "3000", Some("2820"))], pct("15"), &mut sent, 11 * MIN);
        assert!(again.is_empty());
    }

    #[test]
    fn another_market_is_not_quieted() {
        let mut sent = BTreeMap::new();
        check(&[position("ETH", "1", "3000", Some("2610"))], pct("15"), &mut sent, 0);
        let out = check(
            &[position("ETH", "1", "3000", Some("2610")), position("BTC", "-1", "60000", Some("66000"))],
            pct("15"),
            &mut sent,
            MIN,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].market, "BTC");
    }

    #[test]
    fn after_the_quiet_time_a_market_still_near_is_alerted_again() {
        let mut sent = BTreeMap::new();
        check(&[position("ETH", "1", "3000", Some("2610"))], pct("15"), &mut sent, 0);
        assert_eq!(check(&[position("ETH", "1", "3000", Some("2610"))], pct("15"), &mut sent, QUIET_MS).len(), 1);
    }

    #[test]
    fn a_short_is_measured_upwards() {
        let d = distance(&position("BTC", "-1", "60000", Some("66000"))).unwrap();
        assert!(!d.long);
        assert_eq!(d.pct, pct("10"));
    }
}
