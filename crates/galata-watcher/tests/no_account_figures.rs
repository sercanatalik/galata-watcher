//! No alert text carries the account's address or value, whatever the
//! fills and positions. A small deterministic generator stands in for a
//! property-testing crate.

use std::collections::{BTreeMap, HashMap};

use galata_desk_core::venue::{ClearinghouseState, Position};
use galata_watcher::classify::{self, OrderKind};
use galata_watcher::config;
use galata_watcher::fills::{Fill, Liquidation};
use galata_watcher::nearliq;
use rust_decimal::Decimal;
use serde_json::json;

const ME: &str = "0x00000000000000000000000000000000000000aa";

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn dec(&mut self, max: u64) -> Decimal {
        Decimal::new((self.below(max * 1000) + 1) as i64, 3)
    }
}

fn account_value() -> String {
    let text = std::fs::read_to_string(format!("{}/tests/fixtures/clearinghouseState.json", env!("CARGO_MANIFEST_DIR"))).unwrap();
    let state: ClearinghouseState = serde_json::from_str(&text).unwrap();
    galata_watcher::alert::amount(state.margin_summary.account_value)
}

#[test]
fn no_text_names_the_address_or_the_account_value() {
    let config = config::check(&json!({"address": ME, "telegram_chat": "1"})).unwrap().config;
    let value = account_value();
    let types = ["Stop Market", "Take Profit Limit", "Trailing Stop Market", "Limit", "Odd"];
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut checked = 0;

    for round in 0..300u64 {
        let now = 1_795_000_000_000 + round * 60_000;
        let mut fills = Vec::new();
        let mut orders = HashMap::new();
        for i in 0..rng.below(8) + 1 {
            let oid = rng.below(5) + 1;
            orders.insert(
                oid,
                OrderKind {
                    order_type: types[rng.below(types.len() as u64) as usize].into(),
                    is_trigger: rng.below(4) != 0,
                },
            );
            fills.push(Fill {
                coin: ["BTC", "ETH", "xyz:NVDA"][rng.below(3) as usize].into(),
                px: rng.dec(100_000),
                sz: rng.dec(50),
                side: if rng.below(2) == 0 { "A" } else { "B" }.into(),
                time: now - rng.below(20 * 60_000),
                tid: round * 100 + i,
                oid,
                dir: "Close Long".into(),
                liquidation: (rng.below(5) == 0).then(|| Liquidation { liquidated_user: ME.into() }),
            });
        }
        let mut texts: Vec<String> = classify::classify(&fills, Some(&orders), &config, now)
            .alerts
            .into_iter()
            .map(|a| a.text)
            .collect();

        let positions: Vec<Position> = (0..3)
            .map(|_| {
                let mark = rng.dec(100_000);
                let sz = rng.dec(10);
                serde_json::from_value(json!({
                    "coin": "ETH", "szi": sz.to_string(), "leverage": {"type": "cross", "value": 10},
                    "entryPx": mark.to_string(), "positionValue": (mark * sz).to_string(),
                    "unrealizedPnl": "0", "returnOnEquity": "0",
                    "liquidationPx": (mark * Decimal::new(90, 2)).to_string(), "marginUsed": "1",
                    "cumFunding": {"allTime": "0", "sinceOpen": "0", "sinceChange": "0"}
                }))
                .unwrap()
            })
            .collect();
        let mut sent = BTreeMap::new();
        texts.extend(nearliq::check(&positions, Decimal::from(15), &mut sent, now).into_iter().map(|a| a.text));

        for t in texts {
            assert!(!t.to_lowercase().contains(&ME[2..]), "{t}");
            assert!(!t.contains(&value), "{t} contains the account value {value}");
            checked += 1;
        }
    }
    assert!(checked > 300, "the generator produced too few alerts ({checked})");
}

#[test]
fn a_liquidation_reads_market_side_size_and_price() {
    let t = galata_watcher::alert::liquidation_text("BTC", true, "1.6".parse().unwrap(), "62151".parse().unwrap(), false, 0);
    assert_eq!(t, "BTC long LIQUIDATED: 1.6 at 62,151");
}
