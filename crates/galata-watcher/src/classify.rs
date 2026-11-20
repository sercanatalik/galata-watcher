//! Which new fills are worth an alert, and which alert.
//!
//! From the venue's records (see design decision 3):
//!
//! - a fill carrying a `liquidation` object that names the watched account is
//!   a liquidation, however its `dir` reads;
//! - otherwise a fill belongs to the order with its `oid`. A trigger order
//!   whose type starts `Take Profit` is a take-profit; any other trigger type
//!   containing `Stop` (including `Trailing Stop Market`) is a stop-loss; any
//!   other trigger type is reported unclassified rather than dropped;
//! - a fill of an ordinary order is not alerted;
//! - a fill whose order the venue has not listed yet is held to the next
//!   check, and reported unclassified once it has waited [`HOLD_MS`].
//!
//! The fills of one order in one check become one alert, as do one market's
//! liquidation fills.

use std::collections::{BTreeMap, HashMap};

use galata_desk_core::venue::HistoricalOrder;
use rust_decimal::Decimal;

use crate::alert::{self, Alert, Kind, LATE_MS, Side};
use crate::config::Config;
use crate::fills::Fill;

/// How long a fill of an order the venue has not listed is held back.
pub const HOLD_MS: u64 = 10 * 60 * 1000;

/// What an order is, as far as alerts care.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderKind {
    pub order_type: String,
    pub is_trigger: bool,
}

/// One entry per order id, from the venue's order history.
pub fn order_kinds(rows: &[HistoricalOrder]) -> HashMap<u64, OrderKind> {
    rows.iter()
        .map(|r| {
            (
                r.order.oid,
                OrderKind {
                    order_type: r.order.order_type.clone(),
                    is_trigger: r.order.is_trigger,
                },
            )
        })
        .collect()
}

/// The alert kind for a fill of a trigger order of this type.
pub fn trigger_kind(order_type: &str) -> Kind {
    if order_type.starts_with("Take Profit") {
        Kind::TakeProfit
    } else if order_type.contains("Stop") {
        Kind::StopLoss
    } else {
        Kind::TriggerUnclassified
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct Classified {
    pub alerts: Vec<Alert>,
    /// Trade ids dealt with (alerted or deliberately not), with their times.
    pub handled: Vec<(u64, u64)>,
    /// Fills held for the next check: their times, so the cursor does not
    /// move past them.
    pub held_times: Vec<u64>,
}

#[derive(Default)]
struct Group {
    kind: Option<Kind>,
    market: String,
    side: Option<Side>,
    long: bool,
    size: Decimal,
    notional: Decimal,
    event_ms: u64,
    first_tid: u64,
}

impl Group {
    fn add(&mut self, f: &Fill) {
        if self.market.is_empty() {
            self.market = f.coin.clone();
            self.side = Some(Side::from_venue(&f.side));
            self.long = f.dir.contains("Long");
            self.first_tid = f.tid;
        }
        self.size += f.sz;
        self.notional += f.sz * f.px;
        self.event_ms = self.event_ms.max(f.time);
        self.first_tid = self.first_tid.min(f.tid);
    }
    fn price(&self) -> Decimal {
        if self.size.is_zero() {
            Decimal::ZERO
        } else {
            (self.notional / self.size).round_dp(8).normalize()
        }
    }
}

/// Classify fills not seen before. `orders` is the venue's order history,
/// or `None` when it was not asked for (every fill is a liquidation fill).
pub fn classify(fills: &[Fill], orders: Option<&HashMap<u64, OrderKind>>, config: &Config, now_ms: u64) -> Classified {
    let mut out = Classified::default();
    // Keyed so the alerts come out in a stable order.
    let mut by_order: HashMap<u64, Group> = HashMap::new();
    let mut order_seq: Vec<u64> = Vec::new();
    let mut liquidations: BTreeMap<String, Group> = BTreeMap::new();

    for f in fills {
        if f.liquidates(&config.address) {
            liquidations.entry(f.coin.clone()).or_default().add(f);
            out.handled.push((f.tid, f.time));
            continue;
        }
        if f.liquidation.is_some() {
            // The account liquidated somebody else: an ordinary fill.
            out.handled.push((f.tid, f.time));
            continue;
        }
        let kind = match orders.and_then(|o| o.get(&f.oid)) {
            Some(o) if !o.is_trigger => None,
            Some(o) => Some(trigger_kind(&o.order_type)),
            None if now_ms.saturating_sub(f.time) >= HOLD_MS => Some(Kind::FillUnclassified),
            None => {
                out.held_times.push(f.time);
                continue;
            }
        };
        out.handled.push((f.tid, f.time));
        let Some(kind) = kind else { continue };
        if matches!(kind, Kind::TakeProfit | Kind::StopLoss) && !config.stop_alerts {
            continue;
        }
        let group = by_order.entry(f.oid).or_insert_with(|| {
            order_seq.push(f.oid);
            Group::default()
        });
        group.kind = Some(kind);
        group.add(f);
    }

    for oid in order_seq {
        let g = &by_order[&oid];
        let kind = g.kind.expect("set with every fill");
        let late = now_ms.saturating_sub(g.event_ms) > LATE_MS;
        let side = g.side.unwrap_or(Side::Sell);
        let prefix = match kind {
            Kind::TakeProfit => "tp",
            Kind::StopLoss => "sl",
            Kind::TriggerUnclassified => "trigger",
            _ => "fill",
        };
        out.alerts.push(Alert {
            id: format!("{prefix}-{oid}"),
            kind,
            market: g.market.clone(),
            side: Some(side),
            size: Some(g.size.normalize()),
            price: Some(g.price()),
            event_ms: g.event_ms,
            detected_ms: now_ms,
            late,
            text: alert::fill_text(kind, &g.market, side, g.size, g.price(), late, g.event_ms),
        });
    }

    for (coin, g) in liquidations {
        let late = now_ms.saturating_sub(g.event_ms) > LATE_MS;
        out.alerts.push(Alert {
            id: format!("liq-{coin}-{}", g.first_tid),
            kind: Kind::Liquidation,
            market: coin.clone(),
            side: g.side,
            size: Some(g.size.normalize()),
            price: Some(g.price()),
            event_ms: g.event_ms,
            detected_ms: now_ms,
            late,
            text: alert::liquidation_text(&coin, g.long, g.size, g.price(), late, g.event_ms),
        });
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;
    use serde_json::json;

    const ME: &str = "0x00000000000000000000000000000000000000aa";

    fn fixture<T: serde::de::DeserializeOwned>(name: &str) -> T {
        let text = std::fs::read_to_string(format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    fn cfg(stop_alerts: serde_json::Value) -> Config {
        config::check(&json!({"address": ME, "telegram_chat": "1", "stop_alerts": stop_alerts}))
            .unwrap()
            .config
    }

    fn fill(oid: u64, tid: u64, time: u64, sz: &str, px: &str) -> Fill {
        Fill {
            coin: "BTC".into(),
            px: px.parse().unwrap(),
            sz: sz.parse().unwrap(),
            side: "A".into(),
            time,
            tid,
            oid,
            dir: "Close Long".into(),
            liquidation: None,
        }
    }

    fn kinds(pairs: &[(u64, &str, bool)]) -> HashMap<u64, OrderKind> {
        pairs
            .iter()
            .map(|(oid, t, trig)| {
                (
                    *oid,
                    OrderKind {
                        order_type: t.to_string(),
                        is_trigger: *trig,
                    },
                )
            })
            .collect()
    }

    #[test]
    fn the_recorded_trigger_fills_are_take_profits_and_stops_one_alert_per_order() {
        let fills: Vec<Fill> = fixture("userFillsByTime_triggers.json");
        let orders: Vec<HistoricalOrder> = fixture("historicalOrders_triggers.json");
        let kinds = order_kinds(&orders);
        let now = fills.iter().map(|f| f.time).max().unwrap() + 30_000;
        let out = classify(&fills, Some(&kinds), &cfg(json!(true)), now);

        let triggered: Vec<_> = orders.iter().filter(|r| r.status == "triggered").collect();
        assert_eq!(out.alerts.len(), triggered.len(), "one alert per triggered order, none for the ordinary fills");
        for r in triggered {
            let a = out.alerts.iter().find(|a| a.id.ends_with(&r.order.oid.to_string())).unwrap();
            let want = if r.order.order_type.starts_with("Take Profit") { Kind::TakeProfit } else { Kind::StopLoss };
            assert_eq!(a.kind, want, "{}", r.order.order_type);
        }
        assert_eq!(out.handled.len(), fills.len());
        assert!(out.held_times.is_empty());
    }

    #[test]
    fn three_partial_fills_of_one_take_profit_are_one_alert_at_the_average_price() {
        let fills = [
            fill(9, 1, 1_000, "1", "100"),
            fill(9, 2, 1_100, "1", "101"),
            fill(9, 3, 1_200, "2", "102"),
        ];
        let out = classify(&fills, Some(&kinds(&[(9, "Take Profit Market", true)])), &cfg(json!(true)), 2_000);
        assert_eq!(out.alerts.len(), 1);
        let a = &out.alerts[0];
        assert_eq!(a.size, Some("4".parse().unwrap()));
        assert_eq!(a.price, Some("101.25".parse().unwrap()));
        assert_eq!(a.text, "BTC take-profit filled: sold 4 at 101.25");
    }

    #[test]
    fn an_ordinary_limit_fill_is_not_alerted() {
        let out = classify(&[fill(5, 1, 1_000, "1", "1")], Some(&kinds(&[(5, "Limit", false)])), &cfg(json!(true)), 2_000);
        assert!(out.alerts.is_empty());
        assert_eq!(out.handled, vec![(1, 1_000)]);
    }

    #[test]
    fn an_unknown_trigger_type_is_reported_unclassified() {
        let out = classify(&[fill(5, 1, 1_000, "1", "2218")], Some(&kinds(&[(5, "Moon Trigger", true)])), &cfg(json!(true)), 2_000);
        assert_eq!(out.alerts[0].kind, Kind::TriggerUnclassified);
        assert_eq!(out.alerts[0].text, "BTC trigger fill (unclassified): sold 1 at 2,218");
    }

    #[test]
    fn a_trailing_stop_is_a_stop_loss() {
        assert_eq!(trigger_kind("Trailing Stop Market"), Kind::StopLoss);
        assert_eq!(trigger_kind("Stop Limit"), Kind::StopLoss);
        assert_eq!(trigger_kind("Take Profit Limit"), Kind::TakeProfit);
    }

    #[test]
    fn a_fill_of_an_unlisted_order_is_held_then_reported() {
        let f = [fill(77, 1, 1_000, "1", "1")];
        let early = classify(&f, Some(&HashMap::new()), &cfg(json!(true)), 1_000 + 60_000);
        assert!(early.alerts.is_empty() && early.handled.is_empty());
        assert_eq!(early.held_times, vec![1_000]);

        let later = classify(&f, Some(&HashMap::new()), &cfg(json!(true)), 1_000 + HOLD_MS);
        assert_eq!(later.alerts[0].kind, Kind::FillUnclassified);
    }

    #[test]
    fn switched_off_no_tp_or_sl_fill_is_alerted_and_switched_on_each_order_is() {
        let mut eth = fill(1, 1, 1_000, "1", "1");
        eth.coin = "ETH".into();
        let mut sol = fill(2, 2, 1_000, "1", "1");
        sol.coin = "SOL".into();
        let orders = kinds(&[(1, "Stop Market", true), (2, "Take Profit Market", true)]);
        let fills = [eth, sol];
        let off = classify(&fills, Some(&orders), &cfg(json!(false)), 2_000);
        assert!(off.alerts.is_empty());
        assert_eq!(off.handled.len(), 2, "a switched-off fill is still handled, not re-read");
        let on = classify(&fills, Some(&orders), &cfg(json!(true)), 2_000);
        let kinds: Vec<_> = on.alerts.iter().map(|a| (a.market.as_str(), a.kind)).collect();
        assert_eq!(kinds, [("ETH", Kind::StopLoss), ("SOL", Kind::TakeProfit)]);
    }

    #[test]
    fn the_recorded_liquidations_are_alerted_once_per_market_and_a_liquidators_fill_is_not() {
        let fills: Vec<Fill> = fixture("userFills_liquidation.json");
        let now = fills.iter().map(|f| f.time).max().unwrap() + 30_000;
        let out = classify(&fills, None, &cfg(json!(false)), now);
        assert!(!out.alerts.is_empty());
        assert!(out.alerts.iter().all(|a| a.kind == Kind::Liquidation));
        let markets: std::collections::BTreeSet<_> = out.alerts.iter().map(|a| &a.market).collect();
        assert_eq!(markets.len(), out.alerts.len(), "one alert per market");
        // The synthetic liquidator's fill (tid 1) is handled but not counted.
        let btc = out.alerts.iter().find(|a| a.market == "BTC").unwrap();
        let mine: Decimal = fills
            .iter()
            .filter(|f| f.coin == "BTC" && f.liquidates(ME))
            .map(|f| f.sz)
            .sum();
        assert_eq!(btc.size, Some(mine.normalize()));
        assert!(btc.text.contains("long LIQUIDATED"), "{}", btc.text);
        assert_eq!(out.handled.len(), fills.len());
    }

    #[test]
    fn a_liquidation_is_alerted_with_stop_alerts_off() {
        let mut f = fill(1, 1, 1_000, "1", "1");
        f.coin = "SOL".into();
        f.liquidation = Some(crate::fills::Liquidation { liquidated_user: ME.to_uppercase().replace("0X", "0x") });
        let out = classify(&[f], None, &cfg(json!(false)), 2_000);
        assert_eq!(out.alerts.len(), 1);
        assert_eq!(out.alerts[0].kind, Kind::Liquidation);
    }

    #[test]
    fn an_event_found_more_than_three_minutes_late_is_marked_late() {
        let out = classify(&[fill(9, 1, 0, "1", "1")], Some(&kinds(&[(9, "Stop Market", true)])), &cfg(json!(true)), LATE_MS + 1);
        assert!(out.alerts[0].late);
        assert!(out.alerts[0].text.starts_with("[late: happened"));
    }
}
