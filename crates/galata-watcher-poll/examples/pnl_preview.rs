//! Print the PnL summary as it would be sent now, ignoring the slot,
//! quiet hours and `pnl.json`. Reads the configuration in `--data`, the
//! venue, and nothing else; writes nothing.
//!
//! ```sh
//! cargo run -p galata-watcher-poll --example pnl_preview -- --data var/data          # HTML
//! cargo run -p galata-watcher-poll --example pnl_preview -- --data var/data --plain  # plain text
//! ```
//!
//! To see it in Telegram:
//! `... | galata-notify --data <dir with config.json> --alert-id pnl-test --html --token-file <token>`.

use std::time::{SystemTime, UNIX_EPOCH};

use galata_desk_core::venue::{Http, Info};
use galata_watcher::check;
use galata_watcher::daypnl::day_pnl;
use galata_watcher::files::Paths;
use galata_watcher::pnl::{self, LAST_TRADES, Windows};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let data = args.iter().position(|a| a == "--data").and_then(|i| args.get(i + 1)).ok_or("--data DIR is required")?;
    let plain = args.iter().any(|a| a == "--plain");

    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64;
    let config = check::load_config(&Paths::new(data))?;
    let info = Info::new(Http::at(config.network.info_url()));

    let positions = check::positions(&info, &config.address).await?;
    let windows = Windows::from_portfolio(&info.portfolio(&config.address).await?);
    let day = day_pnl(&info, &config.address, &positions, now).await?;
    let fills = info.user_fills(&config.address).await?;
    let trades: Vec<_> = pnl::last_trades(&fills, LAST_TRADES)
        .into_iter()
        .map(|t| (pnl::trade_time(t.time_ms, now, &config.digest.zone), t))
        .collect();
    let time = pnl::trade_time(now, now, &config.digest.zone);
    let m = pnl::message(&format!("{time} (test)"), &windows, &positions, &day, &trades);
    println!("{}", if plain { m.plain() } else { m.html() });
    Ok(())
}
