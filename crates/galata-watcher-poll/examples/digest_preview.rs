//! Print the daily digest as it would be sent now, ignoring the digest time
//! and `digest.json`. Reads the configuration, state and alert history in
//! `--data`, and the venue; writes nothing.
//!
//! ```sh
//! cargo run -p galata-watcher-poll --example digest_preview -- --data var/data          # HTML
//! cargo run -p galata-watcher-poll --example digest_preview -- --data var/data --plain  # plain text
//! ```
//!
//! To see it in Telegram:
//! `... | galata-notify --data <dir with config.json> --alert-id digest-test --html --token-file <token>`.

use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use galata_desk_core::venue::{Http, Info};
use galata_watcher::check;
use galata_watcher::daypnl::{DAY_MS, day_pnl};
use galata_watcher::digest::{self, Sent};
use galata_watcher::files::{self, Paths};
use galata_watcher::state::State;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let data = args.iter().position(|a| a == "--data").and_then(|i| args.get(i + 1)).ok_or("--data DIR is required")?;
    let plain = args.iter().any(|a| a == "--plain");

    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64;
    let paths = Paths::new(data);
    let config = check::load_config(&paths)?;
    let info = Info::new(Http::at(config.network.info_url()));

    let state = State::load(&paths.state())?;
    let positions = check::positions(&info, &config.address).await?;
    let dexes: BTreeSet<&str> = positions.iter().map(|p| p.coin.split_once(':').map_or("", |(dex, _)| dex)).collect();
    let mut orders = Vec::new();
    for dex in dexes {
        orders.extend(info.open_orders(&config.address, dex).await?);
    }
    let pnl = day_pnl(&info, &config.address, &positions, now).await?;
    let armed = config.levels.iter().filter(|l| state.is_armed(l)).count();
    let sent: Sent = files::read_json(&paths.digest())?.unwrap_or_default();
    let (alerts, undelivered) = digest::counts(&paths, sent.last_ms.unwrap_or(now.saturating_sub(DAY_MS)))?;
    // Today's local date, as the digest names it once its time has passed.
    let today = digest::due(&config, &Sent::default(), now).unwrap_or_else(|| "today".into());
    let date = format!("{today} (test)");
    let m = digest::message(&date, &positions, &pnl, &orders, armed, alerts, undelivered);
    println!("{}", if plain { m.plain() } else { m.html() });
    Ok(())
}
