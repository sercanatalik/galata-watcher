//! An alert, and the words it is sent in.
//!
//! The text names the market, the event, side, size and prices, and for a
//! level the trader's own note. The digest adds each position's 24-hour PnL,
//! and the PnL summary the account's PnL over 24h, 30D and all time. Nothing
//! else reaches it: no account value, balance, address, key or token, because
//! nothing else is passed in.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// An event older than this when it is found was missed while the watcher
/// was down, and says so.
pub const LATE_MS: u64 = 3 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    TakeProfit,
    StopLoss,
    /// A trigger order of a type this build does not recognise filled.
    TriggerUnclassified,
    /// A fill whose order the venue never listed.
    FillUnclassified,
    Liquidation,
    NearLiquidation,
    Level,
    Digest,
    /// The half-hourly PnL summary.
    PnlSummary,
}

impl Kind {
    /// The event name the lane emits: `account.stop_loss`.
    pub fn event_name(self) -> String {
        let v = serde_json::to_value(self).expect("a unit enum serialises");
        format!("account.{}", v.as_str().expect("as a string"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Buy,
    Sell,
}

impl Side {
    /// The venue's letter: `B` bid, `A` ask.
    pub fn from_venue(letter: &str) -> Self {
        if letter == "B" { Side::Buy } else { Side::Sell }
    }
    fn past(self) -> &'static str {
        match self {
            Side::Buy => "bought",
            Side::Sell => "sold",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Alert {
    /// Stable for the event it describes, so a re-detected event has the
    /// same id: `tp-<oid>`, `liq-<coin>-<tid>`, `level-<id>-<generation>`.
    pub id: String,
    pub kind: Kind,
    pub market: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side: Option<Side>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "opt_dec")]
    pub size: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "opt_dec")]
    pub price: Option<Decimal>,
    /// When it happened, milliseconds since the epoch.
    pub event_ms: u64,
    /// When the watcher found it.
    pub detected_ms: u64,
    pub late: bool,
    /// The message, final.
    pub text: String,
}

mod opt_dec {
    use rust_decimal::Decimal;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &Option<Decimal>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(d) => s.serialize_str(&d.normalize().to_string()),
            None => s.serialize_none(),
        }
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Decimal>, D::Error> {
        let v: Option<serde_json::Value> = Option::deserialize(d)?;
        Ok(v.and_then(|v| match v {
            serde_json::Value::String(s) => s.parse().ok(),
            serde_json::Value::Number(n) => n.to_string().parse().ok(),
            _ => None,
        }))
    }
}

/// A number as a person reads it: no trailing zeros, thousands grouped.
/// `61240.0` reads `61,240`; `0.50` reads `0.5`.
pub fn amount(d: Decimal) -> String {
    let s = d.normalize().to_string();
    let (sign, s) = s.strip_prefix('-').map_or(("", s.as_str()), |r| ("-", r));
    let (int, frac) = s.split_once('.').map_or((s, None), |(i, f)| (i, Some(f)));
    let mut grouped = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    match frac {
        Some(f) => format!("{sign}{grouped}.{f}"),
        None => format!("{sign}{grouped}"),
    }
}

/// A UTC time as `2026-11-19 03:12 UTC`.
pub fn utc(ms: u64) -> String {
    match jiff::Timestamp::from_millisecond(ms as i64) {
        Ok(t) => t.strftime("%Y-%m-%d %H:%M UTC").to_string(),
        Err(_) => format!("{ms} ms"),
    }
}

fn late_prefix(late: bool, event_ms: u64) -> String {
    if late {
        format!("[late: happened {}] ", utc(event_ms))
    } else {
        String::new()
    }
}

/// The text of a filled order: a take-profit, a stop-loss, or a fill the
/// watcher could not classify.
pub fn fill_text(kind: Kind, market: &str, side: Side, size: Decimal, price: Decimal, late: bool, event_ms: u64) -> String {
    let what = match kind {
        Kind::TakeProfit => "take-profit filled",
        Kind::StopLoss => "stop-loss filled",
        Kind::TriggerUnclassified => "trigger fill (unclassified)",
        _ => "fill (unclassified)",
    };
    format!(
        "{}{market} {what}: {} {} at {}",
        late_prefix(late, event_ms),
        side.past(),
        amount(size),
        amount(price)
    )
}

/// `long` when the liquidated position was long.
pub fn liquidation_text(market: &str, long: bool, size: Decimal, price: Decimal, late: bool, event_ms: u64) -> String {
    format!(
        "{}{market} {} LIQUIDATED: {} at {}",
        late_prefix(late, event_ms),
        if long { "long" } else { "short" },
        amount(size),
        amount(price)
    )
}

pub fn near_text(market: &str, mark: Decimal, liquidation: Decimal, distance_pct: Decimal) -> String {
    format!(
        "{market} near liquidation: mark {}, liquidation {}, {}% away",
        amount(mark.round_dp(6)),
        amount(liquidation.round_dp(6)),
        distance_pct.round_dp(1).normalize()
    )
}

pub fn level_text(market: &str, word: &str, price: Decimal, note: Option<&str>) -> String {
    match note {
        Some(n) => format!("{market} crossed {word} {}: {n}", amount(price)),
        None => format!("{market} crossed {word} {}", amount(price)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Decimal {
        s.parse().unwrap()
    }

    #[test]
    fn amounts_read_as_a_person_writes_them() {
        assert_eq!(amount(d("61240.0")), "61,240");
        assert_eq!(amount(d("0.50")), "0.5");
        assert_eq!(amount(d("1234567.891")), "1,234,567.891");
        assert_eq!(amount(d("-2805.285")), "-2,805.285");
        assert_eq!(amount(d("999")), "999");
    }

    #[test]
    fn the_overnight_stop_reads_as_the_spec_says() {
        let t = fill_text(Kind::StopLoss, "BTC", Side::Sell, d("0.5"), d("61240"), false, 0);
        assert_eq!(t, "BTC stop-loss filled: sold 0.5 at 61,240");
    }

    #[test]
    fn a_late_alert_says_when_it_happened() {
        // 2026-11-19 03:12 UTC
        let ms = 1_795_057_920_000;
        let t = fill_text(Kind::StopLoss, "BTC", Side::Sell, d("0.5"), d("61240"), true, ms);
        assert_eq!(t, "[late: happened 2026-11-19 03:12 UTC] BTC stop-loss filled: sold 0.5 at 61,240");
    }

    #[test]
    fn the_near_liquidation_example_reads_13_3_percent() {
        let t = near_text("ETH", d("3000"), d("2600"), d("13.3333"));
        assert_eq!(t, "ETH near liquidation: mark 3,000, liquidation 2,600, 13.3% away");
    }

    #[test]
    fn event_names_follow_the_kind() {
        assert_eq!(Kind::StopLoss.event_name(), "account.stop_loss");
        assert_eq!(Kind::NearLiquidation.event_name(), "account.near_liquidation");
        assert_eq!(Kind::PnlSummary.event_name(), "account.pnl_summary");
    }
}
