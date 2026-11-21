//! Print the market update as it would be sent now, ignoring the slot,
//! quiet hours, the switch and `markets.json`. Reads the configuration in
//! `--data`, the venue, and nothing else; writes nothing.
//!
//! ```sh
//! cargo run -p galata-watcher-poll --example markets_preview -- --data var/data          # HTML
//! cargo run -p galata-watcher-poll --example markets_preview -- --data var/data --plain  # plain text
//! ```
//!
//! To see it in Telegram:
//! `... | galata-notify --data <dir with config.json> --alert-id markets-test --html --token-file <token>`.

use std::time::{SystemTime, UNIX_EPOCH};

use galata_desk_core::venue::Http;
use galata_watcher::check;
use galata_watcher::files::Paths;
use galata_watcher::markets;
use galata_watcher::pnl;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let data = args.iter().position(|a| a == "--data").and_then(|i| args.get(i + 1)).ok_or("--data DIR is required")?;
    let plain = args.iter().any(|a| a == "--plain");

    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64;
    let config = check::load_config(&Paths::new(data))?;
    if config.watchlist.is_empty() {
        return Err("the configuration's `watchlist` is empty: nothing would be sent".into());
    }
    let quotes = markets::quotes(&Http::at(config.network.info_url()), &config.watchlist).await?;
    let time = pnl::trade_time(now, now, &config.digest.zone);
    let m = markets::message(&format!("{time} (test)"), &config.watchlist, &quotes);
    println!("{}", if plain { m.plain() } else { m.html() });
    Ok(())
}
