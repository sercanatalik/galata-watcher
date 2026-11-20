//! What the watcher is told to watch, as kept in `config.json`.
//!
//! A configuration is checked whole: every problem is named, and one that
//! has any is not used. There is no setting that switches off liquidation or
//! near-liquidation alerts, and a field that tries to is named as a problem
//! rather than ignored.

use std::collections::{BTreeMap, BTreeSet};

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The allowed range for the near-liquidation distance, in percent.
pub const NEAR_LIQ_MIN: Decimal = Decimal::from_parts(2, 0, 0, false, 0);
pub const NEAR_LIQ_MAX: Decimal = Decimal::from_parts(50, 0, 0, false, 0);
pub const NEAR_LIQ_DEFAULT: Decimal = Decimal::from_parts(15, 0, 0, false, 0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Network {
    #[default]
    Mainnet,
    Testnet,
}

impl Network {
    pub fn info_url(self) -> String {
        match self {
            Network::Mainnet => galata_desk_core::network::Network::Mainnet.info_url(),
            Network::Testnet => galata_desk_core::network::Network::Testnet.info_url(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Above,
    Below,
}

impl Direction {
    pub fn word(self) -> &'static str {
        match self {
            Direction::Above => "above",
            Direction::Below => "below",
        }
    }
}

/// A price the trader wants to be told about.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Level {
    pub id: String,
    pub market: String,
    #[serde(with = "rust_decimal::serde::str")]
    pub price: Decimal,
    pub direction: Direction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Raised by every re-arm. A firing recorded at an older generation no
    /// longer counts, so the level is armed again.
    #[serde(default)]
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Digest {
    /// `HH:MM`, 24-hour.
    pub time: String,
    /// An IANA zone name: `UTC`, `Europe/Istanbul`.
    pub zone: String,
}

impl Default for Digest {
    fn default() -> Self {
        Self {
            time: "07:00".into(),
            zone: "UTC".into(),
        }
    }
}

impl Digest {
    /// Hour and minute; only call on a validated configuration.
    pub fn hour_minute(&self) -> (i8, i8) {
        parse_hh_mm(&self.time).unwrap_or((7, 0))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub version: u64,
    /// The watched account, lower case.
    pub address: String,
    #[serde(default)]
    pub network: Network,
    pub telegram_chat: String,
    #[serde(with = "rust_decimal::serde::str")]
    pub near_liquidation_pct: Decimal,
    #[serde(default)]
    pub digest: Digest,
    /// Take-profit and stop-loss fill alerts, for every ticker. On unless
    /// set to `false`.
    #[serde(default = "on", deserialize_with = "stop_switch")]
    pub stop_alerts: bool,
    /// The half-hourly PnL summary. On unless set to `false`.
    #[serde(default = "on")]
    pub pnl_summary: bool,
    #[serde(default)]
    pub levels: Vec<Level>,
}

fn on() -> bool {
    true
}

/// `true` or `false`. A `config.json` saved by an earlier watcher holds a
/// map of tickers instead: it reads as on unless it switched one off.
fn stop_switch<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Either {
        One(bool),
        ByTicker(BTreeMap<String, bool>),
    }
    Ok(match Either::deserialize(d)? {
        Either::One(on) => on,
        Either::ByTicker(m) => m.values().all(|on| *on),
    })
}

/// What a writer said about a level's state, kept so the API can tell a
/// re-arm from a level left as it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatedLevel {
    Armed,
    Fired,
    Unstated,
}

/// A configuration that passed every check, with what the writer said about
/// each level's state, by level id.
#[derive(Debug, Clone, PartialEq)]
pub struct Checked {
    pub config: Config,
    pub stated: BTreeMap<String, StatedLevel>,
}

const TOP_KEYS: &[&str] = &[
    "version",
    "address",
    "network",
    "telegram_chat",
    "near_liquidation_pct",
    "digest",
    "stop_alerts",
    "pnl_summary",
    "levels",
];
const LEVEL_KEYS: &[&str] = &[
    "id",
    "market",
    "price",
    "direction",
    "note",
    "generation",
    "state",
    "fired_at",
];

/// Check a configuration document. Every problem found is returned, each
/// naming its field; none means the document can be used as it is.
pub fn check(doc: &Value) -> Result<Checked, Vec<String>> {
    let mut problems = Vec::new();
    let Some(obj) = doc.as_object() else {
        return Err(vec!["the configuration must be a JSON object".into()]);
    };
    for key in obj.keys() {
        if !TOP_KEYS.contains(&key.as_str()) {
            if key.contains("liquidation") {
                problems.push(format!(
                    "`{key}`: liquidation and near-liquidation alerts cannot be switched off"
                ));
            } else {
                problems.push(format!("`{key}`: not a configuration field"));
            }
        }
    }

    let version = obj.get("version").and_then(Value::as_u64).unwrap_or(0);

    let address = match obj.get("address").and_then(Value::as_str) {
        Some(a) if is_address(a) => a.to_ascii_lowercase(),
        Some(_) => {
            problems.push("`address`: must be 0x followed by 40 hexadecimal characters".into());
            String::new()
        }
        None => {
            problems.push("`address`: required".into());
            String::new()
        }
    };

    let network = match obj.get("network") {
        None | Some(Value::Null) => Network::Mainnet,
        Some(v) => serde_json::from_value(v.clone()).unwrap_or_else(|_| {
            problems.push("`network`: must be `mainnet` or `testnet`".into());
            Network::Mainnet
        }),
    };

    let telegram_chat = match obj.get("telegram_chat") {
        Some(Value::String(s)) if !s.trim().is_empty() => s.trim().to_string(),
        Some(Value::Number(n)) => n.to_string(),
        _ => {
            problems.push("`telegram_chat`: required: the chat id the bot sends to".into());
            String::new()
        }
    };

    let near_liquidation_pct = match obj.get("near_liquidation_pct") {
        None | Some(Value::Null) => NEAR_LIQ_DEFAULT,
        Some(v) => match decimal(v) {
            Some(d) if (NEAR_LIQ_MIN..=NEAR_LIQ_MAX).contains(&d) => d,
            _ => {
                problems.push(format!(
                    "`near_liquidation_pct`: must be a number from {NEAR_LIQ_MIN} to {NEAR_LIQ_MAX}"
                ));
                NEAR_LIQ_DEFAULT
            }
        },
    };

    let digest = match obj.get("digest") {
        None | Some(Value::Null) => Digest::default(),
        Some(v) => match serde_json::from_value::<Digest>(v.clone()) {
            Ok(d) => {
                if parse_hh_mm(&d.time).is_none() {
                    problems.push("`digest.time`: must be HH:MM, 24-hour".into());
                }
                if jiff::tz::TimeZone::get(&d.zone).is_err() {
                    problems.push(format!("`digest.zone`: `{}` is not a known time zone", d.zone));
                }
                d
            }
            Err(_) => {
                problems.push("`digest`: must be {\"time\": \"HH:MM\", \"zone\": \"<IANA zone>\"}".into());
                Digest::default()
            }
        },
    };

    let stop_alerts = match obj.get("stop_alerts") {
        None | Some(Value::Null) => true,
        Some(Value::Bool(b)) => *b,
        // Written by an earlier watcher, which set it per ticker. An empty
        // map, the only kind it ever wrote by default, meant on.
        Some(Value::Object(m)) if m.is_empty() => true,
        Some(_) => {
            problems.push("`stop_alerts`: must be true or false".into());
            true
        }
    };

    let pnl_summary = match obj.get("pnl_summary") {
        None | Some(Value::Null) => true,
        Some(Value::Bool(b)) => *b,
        Some(_) => {
            problems.push("`pnl_summary`: must be true or false".into());
            true
        }
    };

    let mut levels = Vec::new();
    let mut stated = BTreeMap::new();
    match obj.get("levels") {
        None | Some(Value::Null) => {}
        Some(Value::Array(rows)) => {
            let mut ids = BTreeSet::new();
            for (i, row) in rows.iter().enumerate() {
                if let Some((level, state)) = check_level(i, row, &mut problems) {
                    if !ids.insert(level.id.clone()) {
                        problems.push(format!("`levels[{i}].id`: `{}` is used twice", level.id));
                    }
                    stated.insert(level.id.clone(), state);
                    levels.push(level);
                }
            }
        }
        Some(_) => problems.push("`levels`: must be a list".into()),
    }

    if problems.is_empty() {
        Ok(Checked {
            config: Config {
                version,
                address,
                network,
                telegram_chat,
                near_liquidation_pct,
                digest,
                stop_alerts,
                pnl_summary,
                levels,
            },
            stated,
        })
    } else {
        Err(problems)
    }
}

fn check_level(i: usize, row: &Value, problems: &mut Vec<String>) -> Option<(Level, StatedLevel)> {
    let Some(obj) = row.as_object() else {
        problems.push(format!("`levels[{i}]`: must be an object"));
        return None;
    };
    let before = problems.len();
    for key in obj.keys() {
        if !LEVEL_KEYS.contains(&key.as_str()) {
            problems.push(format!("`levels[{i}].{key}`: not a level field"));
        }
    }
    let market = obj
        .get("market")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(str::to_string);
    if market.is_none() {
        problems.push(format!("`levels[{i}].market`: required"));
    }
    let price = obj.get("price").and_then(decimal).filter(|p| *p > Decimal::ZERO);
    if price.is_none() {
        problems.push(format!("`levels[{i}].price`: must be a positive number"));
    }
    let direction = obj
        .get("direction")
        .and_then(|v| serde_json::from_value::<Direction>(v.clone()).ok());
    if direction.is_none() {
        problems.push(format!("`levels[{i}].direction`: must be `above` or `below`"));
    }
    let note = match obj.get("note") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if s.trim().is_empty() => None,
        Some(Value::String(s)) => Some(s.trim().to_string()),
        Some(_) => {
            problems.push(format!("`levels[{i}].note`: must be text"));
            None
        }
    };
    let generation = obj.get("generation").and_then(Value::as_u64).unwrap_or(0);
    let state = match obj.get("state").and_then(Value::as_str) {
        None => StatedLevel::Unstated,
        Some("armed") => StatedLevel::Armed,
        Some("fired") => StatedLevel::Fired,
        Some(_) => {
            problems.push(format!("`levels[{i}].state`: must be `armed` or `fired`"));
            StatedLevel::Unstated
        }
    };
    if problems.len() > before {
        return None;
    }
    let (market, price, direction) = (market?, price?, direction?);
    let id = match obj.get("id").and_then(Value::as_str).map(str::trim) {
        Some(id) if !id.is_empty() => id.to_string(),
        // A level written by hand without an id gets one from what it is,
        // so the same level keeps the same id across edits.
        _ => format!("{}-{}-{}", market, direction.word(), price.normalize()),
    };
    Some((
        Level {
            id,
            market,
            price,
            direction,
            note,
            generation,
        },
        state,
    ))
}

fn decimal(v: &Value) -> Option<Decimal> {
    match v {
        Value::String(s) => s.trim().parse().ok(),
        Value::Number(n) => n.to_string().parse().ok(),
        _ => None,
    }
}

fn is_address(a: &str) -> bool {
    a.len() == 42 && a.starts_with("0x") && a[2..].chars().all(|c| c.is_ascii_hexdigit())
}

fn parse_hh_mm(s: &str) -> Option<(i8, i8)> {
    let (h, m) = s.split_once(':')?;
    if h.len() != 2 || m.len() != 2 {
        return None;
    }
    let (h, m): (i8, i8) = (h.parse().ok()?, m.parse().ok()?);
    ((0..24).contains(&h) && (0..60).contains(&m)).then_some((h, m))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    pub fn valid() -> Value {
        json!({
            "address": "0x00000000000000000000000000000000000000AA",
            "telegram_chat": "12345",
            "near_liquidation_pct": "15",
            "digest": {"time": "07:00", "zone": "Europe/Istanbul"},
            "stop_alerts": false,
            "levels": [{"id": "btc-58k", "market": "BTC", "price": "58000", "direction": "below", "note": "support"}]
        })
    }

    #[test]
    fn a_valid_configuration_is_accepted_with_defaults_filled() {
        let c = check(&valid()).unwrap().config;
        assert_eq!(c.address, "0x00000000000000000000000000000000000000aa");
        assert_eq!(c.network, Network::Mainnet);
        assert!(!c.stop_alerts);
        assert_eq!(c.levels[0].generation, 0);
        assert_eq!(c.digest.hour_minute(), (7, 0));
    }

    #[test]
    fn stop_alerts_are_one_switch_on_by_default() {
        let mut doc = valid();
        doc.as_object_mut().unwrap().remove("stop_alerts");
        assert!(check(&doc).unwrap().config.stop_alerts);
        doc["stop_alerts"] = json!({});
        assert!(check(&doc).unwrap().config.stop_alerts, "an earlier watcher's empty map");
        doc["stop_alerts"] = json!({"SOL": false});
        assert_eq!(check(&doc).unwrap_err(), ["`stop_alerts`: must be true or false"]);
    }

    #[test]
    fn the_pnl_summary_is_one_switch_on_by_default() {
        let doc = valid();
        assert!(check(&doc).unwrap().config.pnl_summary, "absent reads on");
        let mut off = doc.clone();
        off["pnl_summary"] = json!(false);
        assert!(!check(&off).unwrap().config.pnl_summary);
        off["pnl_summary"] = json!("sometimes");
        assert_eq!(check(&off).unwrap_err(), ["`pnl_summary`: must be true or false"]);
        let saved = serde_json::to_value(check(&doc).unwrap().config).unwrap();
        let mut earlier = saved.clone();
        earlier.as_object_mut().unwrap().remove("pnl_summary");
        assert!(serde_json::from_value::<Config>(earlier).unwrap().pnl_summary, "a config.json without it");
    }

    #[test]
    fn a_config_json_saved_by_an_earlier_watcher_still_reads() {
        let mut saved = serde_json::to_value(check(&valid()).unwrap().config).unwrap();
        saved["stop_alerts"] = json!({});
        assert!(serde_json::from_value::<Config>(saved.clone()).unwrap().stop_alerts);
        saved["stop_alerts"] = json!({"SOL": false});
        assert!(!serde_json::from_value::<Config>(saved).unwrap().stop_alerts);
    }

    #[test]
    fn a_distance_of_80_percent_is_refused_naming_the_distance() {
        let mut doc = valid();
        doc["near_liquidation_pct"] = json!(80);
        let problems = check(&doc).unwrap_err();
        assert_eq!(problems.len(), 1);
        assert!(problems[0].starts_with("`near_liquidation_pct`"));
    }

    #[test]
    fn the_distance_range_is_inclusive() {
        for ok in ["2", "50"] {
            let mut doc = valid();
            doc["near_liquidation_pct"] = json!(ok);
            assert!(check(&doc).is_ok(), "{ok}");
        }
        for bad in ["1.99", "50.01", "-5"] {
            let mut doc = valid();
            doc["near_liquidation_pct"] = json!(bad);
            assert!(check(&doc).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_field_that_would_switch_off_liquidation_alerts_is_named() {
        let mut doc = valid();
        doc["liquidation_alerts"] = json!(false);
        let problems = check(&doc).unwrap_err();
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("`liquidation_alerts`"));
        assert!(problems[0].contains("cannot be switched off"));
    }

    #[test]
    fn every_problem_is_named_at_once() {
        let doc = json!({
            "address": "0x123",
            "levels": [{"price": "-1", "direction": "sideways"}],
            "digest": {"time": "7am", "zone": "Mars/Base"}
        });
        let problems = check(&doc).unwrap_err();
        for field in [
            "`address`",
            "`telegram_chat`",
            "`levels[0].market`",
            "`levels[0].price`",
            "`levels[0].direction`",
            "`digest.time`",
            "`digest.zone`",
        ] {
            assert!(problems.iter().any(|p| p.starts_with(field)), "{field} in {problems:?}");
        }
    }

    #[test]
    fn a_level_without_an_id_gets_one_from_what_it_is() {
        let mut doc = valid();
        doc["levels"] = json!([{"market": "BTC", "price": "58000.0", "direction": "below"}]);
        let c = check(&doc).unwrap().config;
        assert_eq!(c.levels[0].id, "BTC-below-58000");
    }

    #[test]
    fn two_levels_with_one_id_are_refused() {
        let mut doc = valid();
        doc["levels"] = json!([
            {"id": "a", "market": "BTC", "price": "1", "direction": "below"},
            {"id": "a", "market": "ETH", "price": "1", "direction": "above"}
        ]);
        let problems = check(&doc).unwrap_err();
        assert!(problems[0].contains("used twice"));
    }

    #[test]
    fn a_stated_level_state_is_kept_for_the_api() {
        let mut doc = valid();
        doc["levels"][0]["state"] = json!("armed");
        let checked = check(&doc).unwrap();
        assert_eq!(checked.stated["btc-58k"], StatedLevel::Armed);
    }

    #[test]
    fn a_saved_configuration_reads_back_the_same() {
        let c = check(&valid()).unwrap().config;
        let again: Config = serde_json::from_value(serde_json::to_value(&c).unwrap()).unwrap();
        assert_eq!(c, again);
        assert_eq!(check(&serde_json::to_value(&c).unwrap()).unwrap().config, c);
    }
}
