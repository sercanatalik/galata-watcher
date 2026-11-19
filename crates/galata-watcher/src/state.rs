//! What the poll remembers between checks, in `state.json`. Only the poll
//! writes it; the API reads it to show which levels have fired.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::config::Level;
use crate::files;

/// How far back each check re-reads fills, so a fill the venue lists late
/// is not missed. Fills already seen are recognised by trade id.
pub const OVERLAP_MS: u64 = 5 * 60 * 1000;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    /// Fills are read from here minus [`OVERLAP_MS`]. `None` before the first
    /// check, which starts from its own time and reports nothing older.
    pub cursor_ms: Option<u64>,
    /// Trade ids already handled, with the fill's time, inside the overlap.
    #[serde(default)]
    pub seen: BTreeMap<u64, u64>,
    /// The last near-liquidation alert per market.
    #[serde(default)]
    pub near: BTreeMap<String, NearSent>,
    /// Levels that have fired, by level id, at the generation they fired at.
    #[serde(default)]
    pub fired: BTreeMap<String, Fired>,
    /// When the last check succeeded.
    pub last_ok_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NearSent {
    pub at_ms: u64,
    #[serde(with = "rust_decimal::serde::str")]
    pub distance_pct: Decimal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fired {
    pub generation: u64,
    pub at_ms: u64,
}

impl State {
    pub fn load(path: &Path) -> io::Result<Self> {
        Ok(files::read_json(path)?.unwrap_or_default())
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        files::write_json(path, self)
    }

    /// When `level` fired, if it has fired at its current generation. A
    /// firing at an older generation was undone by a re-arm.
    pub fn fired_at(&self, level: &Level) -> Option<u64> {
        self.fired
            .get(&level.id)
            .filter(|f| f.generation == level.generation)
            .map(|f| f.at_ms)
    }

    pub fn is_armed(&self, level: &Level) -> bool {
        self.fired_at(level).is_none()
    }

    /// Forget trade ids older than the overlap needs, measured from `from_ms`.
    pub fn prune_seen(&mut self, from_ms: u64) {
        let keep_after = from_ms.saturating_sub(OVERLAP_MS * 2);
        self.seen.retain(|_, t| *t >= keep_after);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Direction;

    fn level(generation: u64) -> Level {
        Level {
            id: "l".into(),
            market: "BTC".into(),
            price: Decimal::from(58_000),
            direction: Direction::Below,
            note: None,
            generation,
        }
    }

    #[test]
    fn a_round_trip_keeps_everything() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let mut s = State {
            cursor_ms: Some(1_000),
            last_ok_ms: Some(2_000),
            ..State::default()
        };
        s.seen.insert(7, 900);
        s.near.insert(
            "ETH".into(),
            NearSent {
                at_ms: 5,
                distance_pct: "13.3".parse().unwrap(),
            },
        );
        s.fired.insert("l".into(), Fired { generation: 2, at_ms: 9 });
        s.save(&path).unwrap();
        assert_eq!(State::load(&path).unwrap(), s);
    }

    #[test]
    fn a_crash_mid_write_leaves_the_previous_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let s = State {
            cursor_ms: Some(1),
            ..State::default()
        };
        s.save(&path).unwrap();
        // What a crash between writing and renaming leaves behind.
        std::fs::write(dir.path().join(".state.json.tmp-1-0"), b"{\"cursor_ms\": 99, \"se").unwrap();
        assert_eq!(State::load(&path).unwrap(), s);
    }

    #[test]
    fn no_file_is_a_fresh_state() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(State::load(&dir.path().join("state.json")).unwrap(), State::default());
    }

    #[test]
    fn a_re_arm_undoes_an_older_firing() {
        let mut s = State::default();
        s.fired.insert("l".into(), Fired { generation: 0, at_ms: 9 });
        assert!(!s.is_armed(&level(0)));
        assert!(s.is_armed(&level(1)));
    }
}
