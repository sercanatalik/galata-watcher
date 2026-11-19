//! The pinned galata-desk-core reads the recorded answers this tree relies on.

use galata_desk_core::venue::{ClearinghouseState, HistoricalOrder, UserFill, WireCandle};

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

#[test]
fn triggered_orders_decode_with_their_trigger_fields() {
    let rows: Vec<HistoricalOrder> =
        serde_json::from_str(&fixture("historicalOrders_triggers.json")).unwrap();
    let triggered: Vec<_> = rows.iter().filter(|r| r.status == "triggered").collect();
    assert_eq!(triggered.len(), 6);
    assert!(triggered.iter().all(|r| r.order.is_trigger));
    assert!(triggered.iter().any(|r| r.order.is_position_tpsl));
    assert!(triggered.iter().any(|r| !r.order.is_position_tpsl));
}

#[test]
fn trigger_fills_decode_with_their_order_id() {
    let fills: Vec<UserFill> =
        serde_json::from_str(&fixture("userFillsByTime_triggers.json")).unwrap();
    assert!(fills.iter().all(|f| f.oid != 0 && !f.dir.is_empty()));
}

#[test]
fn positions_and_candles_decode() {
    let state: ClearinghouseState =
        serde_json::from_str(&fixture("clearinghouseState.json")).unwrap();
    assert!(!state.asset_positions.is_empty());
    let candles: Vec<WireCandle> =
        serde_json::from_str(&fixture("candleSnapshot_BTC.json")).unwrap();
    assert!(!candles.is_empty());
}
