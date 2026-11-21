//! The market update: every quarter of an hour outside the night, each
//! watched market's last price and its change over 24 hours.
//!
//! The markets are the configuration's `watchlist`, which galata-desk keeps
//! equal to its own. The lane asks every fifteen minutes; an update is due
//! as the PnL summary is (see [`crate::pnl::slot_due`]), at :00, :15, :30
//! and :45, and never while the watchlist is empty or the update is
//! switched off. A slot missed is never sent late.
//!
//! It asks the venue for market figures only, nothing about the account,
//! and keeps its own `markets.json`, so it never writes another mode's state.

use std::collections::{BTreeSet, HashMap};
use std::io;

use galata_desk_core::venue::{Transport, VenueError};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::json;

use crate::alert::{Alert, Kind, amount};
use crate::check::{self, CheckError};
use crate::config::Config;
use crate::files::{self, Paths};
use crate::message::{Align, Message, bold};
use crate::pnl::{Sent, Slot, slot_due};

/// Minutes between updates.
pub const EVERY_MIN: i8 = 15;

#[derive(Debug)]
pub struct Outcome {
    pub alert: Alert,
    pub sent: Sent,
}

/// The slot an update is due for at `now_ms`, if one is due. None while the
/// update is switched off or no market is watched.
pub fn due(config: &Config, sent: &Sent, now_ms: u64) -> Option<Slot> {
    if !config.market_update || config.watchlist.is_empty() {
        return None;
    }
    slot_due(&config.digest.zone, EVERY_MIN, sent, now_ms)
}

/// A market's price now and a day ago.
#[derive(Debug, Clone, PartialEq)]
pub struct Quote {
    /// The mid price, or the mark where the venue reports no mid: the
    /// "last" galata-desk's tiles show.
    pub last: Decimal,
    /// The price 24 hours ago, where the venue gives one.
    pub prev_day: Option<Decimal>,
}

impl Quote {
    /// The change over 24 hours, in percent.
    pub fn change_pct(&self) -> Option<Decimal> {
        let then = self.prev_day.filter(|p| !p.is_zero())?;
        Some((self.last - then) / then * Decimal::ONE_HUNDRED)
    }
}

#[derive(Deserialize)]
struct Universe {
    universe: Vec<Named>,
}

#[derive(Deserialize)]
struct Named {
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Ctx {
    mark_px: Decimal,
    #[serde(default)]
    mid_px: Option<Decimal>,
    #[serde(default)]
    prev_day_px: Option<Decimal>,
}

/// The dex a market trades on: `xyz` for `xyz:NVDA`, empty for the main dex.
fn dex_of(market: &str) -> &str {
    market.split_once(':').map_or("", |(dex, _)| dex)
}

/// A quote for each of `markets` the venue lists, by market. One question
/// per dex; a market the venue does not list is left out.
///
/// galata-desk-core's asset context drops the day-old price, so the watcher
/// asks for it itself, through desk-core's `Transport`.
pub async fn quotes<T: Transport>(transport: &T, markets: &[String]) -> Result<HashMap<String, Quote>, VenueError> {
    let dexes: BTreeSet<&str> = markets.iter().map(|m| dex_of(m)).collect();
    let mut out = HashMap::new();
    for dex in dexes {
        let bytes = transport.post(json!({ "type": "metaAndAssetCtxs", "dex": dex })).await?;
        let (meta, ctxs): (Universe, Vec<Ctx>) = serde_json::from_slice(&bytes).map_err(|e| VenueError::Shape {
            query: "metaAndAssetCtxs",
            path: String::new(),
            detail: e.to_string(),
        })?;
        // The two lists are in step: the nth context is the nth market's.
        for (market, ctx) in meta.universe.into_iter().zip(ctxs) {
            if markets.contains(&market.name) {
                out.insert(
                    market.name,
                    Quote {
                        last: ctx.mid_px.unwrap_or(ctx.mark_px),
                        prev_day: ctx.prev_day_px,
                    },
                );
            }
        }
    }
    Ok(out)
}

/// A change as a person reads it, signed: `+0.29%`, `-2.33%`.
pub fn pct(d: Decimal) -> String {
    let r = d.round_dp(2);
    let sign = if r.is_sign_negative() && !r.is_zero() { "-" } else { "+" };
    format!("{sign}{:.2}%", r.abs())
}

/// The update: one row per watched market, in the watchlist's order. A
/// market without a quote, or without a day-old price, reads `n/a` there.
pub fn message(time: &str, markets: &[String], quotes: &HashMap<String, Quote>) -> Message {
    let rows = markets
        .iter()
        .map(|m| {
            let quote = quotes.get(m);
            vec![
                m.clone(),
                quote.map_or("n/a".into(), |q| amount(q.last)),
                quote.and_then(Quote::change_pct).map_or("n/a".into(), pct),
            ]
        })
        .collect();
    Message::new()
        .icon("📈")
        .line(vec![bold(format!("Markets, {time}"))])
        .table(rows, &[Align::Left, Align::Right, Align::Right])
}

/// The update's plain text; see [`message`].
pub fn text(time: &str, markets: &[String], quotes: &HashMap<String, Quote>) -> String {
    message(time, markets, quotes).plain()
}

/// The update, if one is due. Saves nothing; see [`commit`].
pub async fn run<T: Transport>(transport: &T, paths: &Paths, now_ms: u64) -> Result<Option<Outcome>, CheckError> {
    let config = check::load_config(paths)?;
    let sent: Sent = files::read_json(&paths.markets())?.unwrap_or_default();
    let Some(slot) = due(&config, &sent, now_ms) else {
        return Ok(None);
    };
    let quotes = quotes(transport, &config.watchlist).await?;
    let body = message(&slot.time, &config.watchlist, &quotes);
    Ok(Some(Outcome {
        alert: Alert {
            id: format!("markets-{}", slot.id),
            kind: Kind::MarketUpdate,
            market: String::new(),
            side: None,
            size: None,
            price: None,
            event_ms: now_ms,
            detected_ms: now_ms,
            late: false,
            text: String::new(),
            html: None,
        }
        .with_message(&body),
        sent: Sent {
            last_slot: Some(slot.id),
            last_ms: Some(now_ms),
        },
    }))
}

/// Record the update as sent: the alert first, then `markets.json`.
pub fn commit(paths: &Paths, outcome: &Outcome) -> io::Result<()> {
    files::append_line(&paths.alerts(), &outcome.alert)?;
    files::write_json(&paths.markets(), &outcome.sent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;
    use jiff::Timestamp;

    fn cfg(watchlist: &[&str]) -> Config {
        config::check(&json!({
            "address": "0x00000000000000000000000000000000000000aa",
            "telegram_chat": "1",
            "digest": {"time": "07:00", "zone": "Europe/London"},
            "watchlist": watchlist
        }))
        .unwrap()
        .config
    }

    fn ms(rfc3339: &str) -> u64 {
        rfc3339.parse::<Timestamp>().unwrap().as_millisecond() as u64
    }

    fn due_id(c: &Config, sent: &Sent, at: &str) -> Option<String> {
        due(c, sent, ms(at)).map(|s| s.id)
    }

    fn d(s: &str) -> Decimal {
        s.parse().unwrap()
    }

    fn quote(last: &str, prev: Option<&str>) -> Quote {
        Quote {
            last: d(last),
            prev_day: prev.map(d),
        }
    }

    #[test]
    fn slots_fall_on_every_quarter_hour_outside_quiet_hours() {
        // London is UTC+0 in November.
        let c = cfg(&["BTC"]);
        let none = Sent::default();
        for (at, slot) in [
            ("2026-11-20T14:00:05Z", "2026-11-20T14:00"),
            ("2026-11-20T14:15:05Z", "2026-11-20T14:15"),
            ("2026-11-20T14:30:05Z", "2026-11-20T14:30"),
            ("2026-11-20T14:45:05Z", "2026-11-20T14:45"),
            ("2026-11-20T22:45:05Z", "2026-11-20T22:45"),
            ("2026-11-21T07:00:05Z", "2026-11-21T07:00"),
        ] {
            assert_eq!(due_id(&c, &none, at).as_deref(), Some(slot), "{at}");
        }
        assert_eq!(due_id(&c, &none, "2026-11-20T23:00:05Z"), None, "23:00 is quiet");
        assert_eq!(due_id(&c, &none, "2026-11-21T06:45:05Z"), None, "06:45 is quiet");
        assert_eq!(due(&c, &none, ms("2026-11-20T14:47:00Z")).unwrap().time, "14:45");
    }

    #[test]
    fn a_slot_is_sent_once_and_not_past_its_grace() {
        let c = cfg(&["BTC"]);
        let sent = Sent {
            last_slot: Some("2026-11-20T14:15".into()),
            last_ms: Some(0),
        };
        assert_eq!(due_id(&c, &sent, "2026-11-20T14:18:00Z"), None, "already sent");
        assert_eq!(due_id(&c, &sent, "2026-11-20T14:30:00Z").as_deref(), Some("2026-11-20T14:30"));
        assert_eq!(due_id(&c, &Sent::default(), "2026-11-20T14:26:00Z"), None, "eleven minutes in");
    }

    #[test]
    fn nothing_is_due_switched_off_or_with_nothing_watched() {
        let at = ms("2026-11-20T14:15:05Z");
        let mut c = cfg(&["BTC"]);
        assert!(due(&c, &Sent::default(), at).is_some());
        c.market_update = false;
        assert!(due(&c, &Sent::default(), at).is_none());
        assert!(due(&cfg(&[]), &Sent::default(), at).is_none());
    }

    #[test]
    fn a_change_is_signed_and_to_two_places() {
        assert_eq!(pct(d("0.2932")), "+0.29%");
        assert_eq!(pct(d("-2.335")), "-2.34%");
        assert_eq!(pct(d("-0.004")), "+0.00%");
        assert_eq!(pct(d("12")), "+12.00%");
        assert_eq!(quote("110", Some("100")).change_pct(), Some(d("10")));
        assert_eq!(quote("110", None).change_pct(), None);
        assert_eq!(quote("110", Some("0")).change_pct(), None);
    }

    #[test]
    fn each_watched_market_reads_last_and_change_in_the_watchlists_order() {
        let markets: Vec<String> = ["xyz:NVDA", "BTC", "GONE", "NEW"].map(String::from).to_vec();
        let quotes = HashMap::from([
            ("BTC".to_string(), quote("64798.5", Some("64609.0"))),
            ("xyz:NVDA".to_string(), quote("157.615", Some("161.37"))),
            ("NEW".to_string(), quote("1.5", None)),
        ]);
        let m = message("14:15", &markets, &quotes);
        assert_eq!(
            m.plain(),
            "Markets, 14:15\nxyz:NVDA 157.615 -2.33%\nBTC 64,798.5 +0.29%\nGONE n/a n/a\nNEW 1.5 n/a"
        );
        assert_eq!(
            m.html(),
            "📈 <b>Markets, 14:15</b>\n\
             <blockquote><code>xyz:NVDA   157.615  -2.33%</code>\n\
             <code>BTC       64,798.5  +0.29%</code>\n\
             <code>GONE           n/a     n/a</code>\n\
             <code>NEW            1.5     n/a</code></blockquote>"
        );
    }
}
