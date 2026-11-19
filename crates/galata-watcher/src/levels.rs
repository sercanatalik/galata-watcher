//! Call levels: a price crossing that the trader asked to hear about.
//!
//! Each check reads the market's one-minute candles since the previous
//! check, and a level fires when a candle's high reaches an `above` level or
//! its low reaches a `below` one, so a wick through it counts. A level fires
//! once, then stays fired until re-armed (its generation raised). After
//! downtime, levels are judged only from the latest minutes: price moves
//! while the watcher was down do not fire them.

use std::collections::HashMap;

use galata_desk_core::venue::WireCandle;

use crate::alert::{self, Alert, Kind};
use crate::config::{Direction, Level};
use crate::state::{Fired, State};

const MINUTE_MS: u64 = 60_000;
/// A previous check older than this means the watcher was down.
const CONTINUOUS_MS: u64 = 3 * MINUTE_MS;

/// The start of the stretch this check judges levels over: just before the
/// previous check if it was recent, otherwise only the last two minutes.
pub fn window_start(last_ok_ms: Option<u64>, now_ms: u64) -> u64 {
    match last_ok_ms {
        Some(last) if now_ms.saturating_sub(last) <= CONTINUOUS_MS => last.saturating_sub(MINUTE_MS),
        _ => now_ms.saturating_sub(2 * MINUTE_MS),
    }
}

/// The markets whose candles this check needs: those with an armed level.
pub fn markets_to_read(levels: &[Level], state: &State) -> Vec<String> {
    let mut markets: Vec<String> = levels
        .iter()
        .filter(|l| state.is_armed(l))
        .map(|l| l.market.clone())
        .collect();
    markets.sort();
    markets.dedup();
    markets
}

fn crossed(level: &Level, c: &WireCandle) -> bool {
    match level.direction {
        Direction::Above => c.h >= level.price,
        Direction::Below => c.l <= level.price,
    }
}

/// Fire the armed levels whose price was crossed within the window,
/// recording each in `state`. `candles` holds each market's candles.
pub fn check(levels: &[Level], candles: &HashMap<String, Vec<WireCandle>>, state: &mut State, window_start_ms: u64, now_ms: u64) -> Vec<Alert> {
    let mut out = Vec::new();
    for level in levels {
        if !state.is_armed(level) {
            continue;
        }
        let Some(cs) = candles.get(&level.market) else { continue };
        let hit = cs
            .iter()
            .filter(|c| c.t + MINUTE_MS > window_start_ms)
            .find(|c| crossed(level, c));
        let Some(c) = hit else { continue };
        let event_ms = c.t.max(window_start_ms).min(now_ms);
        state.fired.insert(
            level.id.clone(),
            Fired {
                generation: level.generation,
                at_ms: event_ms,
            },
        );
        out.push(Alert {
            id: format!("level-{}-{}", level.id, level.generation),
            kind: Kind::Level,
            market: level.market.clone(),
            side: None,
            size: None,
            price: Some(level.price.normalize()),
            event_ms,
            detected_ms: now_ms,
            late: false,
            text: alert::level_text(&level.market, level.direction.word(), level.price, level.note.as_deref()),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;

    const T0: u64 = 1_795_000_000_000;

    fn candle(t: u64, h: &str, l: &str, c: &str) -> WireCandle {
        serde_json::from_value(serde_json::json!({
            "t": t, "T": t + 59_999, "s": "BTC", "i": "1m", "o": c, "c": c, "h": h, "l": l, "v": "1", "n": 1
        }))
        .unwrap()
    }

    fn level(direction: Direction, price: u32, generation: u64) -> Level {
        Level {
            id: "btc".into(),
            market: "BTC".into(),
            price: Decimal::from(price),
            direction,
            note: Some("support".into()),
            generation,
        }
    }

    fn btc(cs: Vec<WireCandle>) -> HashMap<String, Vec<WireCandle>> {
        HashMap::from([("BTC".to_string(), cs)])
    }

    #[test]
    fn a_wick_through_a_below_level_fires_it() {
        let mut state = State::default();
        let out = check(
            &[level(Direction::Below, 58_000, 0)],
            &btc(vec![candle(T0, "58400", "57950", "58300")]),
            &mut state,
            T0,
            T0 + 60_000,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "BTC crossed below 58,000: support");
        assert!(!state.is_armed(&level(Direction::Below, 58_000, 0)));
    }

    #[test]
    fn a_high_just_short_of_an_above_level_does_not_fire() {
        let mut state = State::default();
        let out = check(
            &[level(Direction::Above, 70_000, 0)],
            &btc(vec![candle(T0, "69990", "69000", "69500")]),
            &mut state,
            T0,
            T0 + 60_000,
        );
        assert!(out.is_empty());
    }

    #[test]
    fn a_level_beyond_which_price_stays_fires_once() {
        let mut state = State::default();
        let l = [level(Direction::Below, 58_000, 0)];
        let mut sent = 0;
        for m in 0..60 {
            let t = T0 + m * 60_000;
            sent += check(&l, &btc(vec![candle(t, "57500", "57000", "57200")]), &mut state, t, t + 60_000).len();
        }
        assert_eq!(sent, 1);
    }

    #[test]
    fn a_re_armed_level_fires_again() {
        let mut state = State::default();
        let cs = btc(vec![candle(T0, "58400", "57950", "58300")]);
        assert_eq!(check(&[level(Direction::Below, 58_000, 0)], &cs, &mut state, T0, T0 + 60_000).len(), 1);
        let rearmed = [level(Direction::Below, 58_000, 1)];
        assert_eq!(markets_to_read(&rearmed, &state), vec!["BTC"]);
        assert_eq!(check(&rearmed, &cs, &mut state, T0, T0 + 60_000).len(), 1);
    }

    #[test]
    fn levels_are_not_judged_over_downtime() {
        // Down for an hour; the crossing happened 30 minutes ago.
        let now = T0 + 60 * 60_000;
        let start = window_start(Some(T0), now);
        assert_eq!(start, now - 2 * 60_000);
        let mut state = State::default();
        let out = check(
            &[level(Direction::Below, 58_000, 0)],
            &btc(vec![candle(T0 + 30 * 60_000, "58400", "57950", "58300"), candle(now - 60_000, "58500", "58200", "58300")]),
            &mut state,
            start,
            now,
        );
        assert!(out.is_empty());
    }

    #[test]
    fn a_continuous_watch_reads_from_just_before_the_previous_check() {
        assert_eq!(window_start(Some(T0), T0 + 60_000), T0 - 60_000);
        assert_eq!(window_start(None, T0), T0 - 120_000);
    }
}
