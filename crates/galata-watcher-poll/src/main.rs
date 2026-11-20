//! galata-watcher-poll: check the account once and print what is new.
//!
//! ```text
//! galata-watcher-poll --data /data [--digest | --pnl] [--info-url URL]
//! ```
//!
//! Prints one JSON alert per line, then saves its state. Exit codes, read by
//! the lane's runner:
//!
//! | code | meaning |
//! |---|---|
//! | 0 | alerts printed |
//! | 3 | nothing new (for `--digest`: no digest due; for `--pnl`: no slot due, or quiet hours) |
//! | 1 | broken: the venue or the data volume failed; nothing saved but health |
//! | 2 | bad argument or invalid configuration |

use std::io::Write;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use galata_desk_core::venue::{Http, Info};
use galata_watcher::alert::Alert;
use galata_watcher::check::{self, CheckError};
use galata_watcher::digest;
use galata_watcher::pnl;
use galata_watcher::files::Paths;

struct Args {
    data: String,
    digest: bool,
    pnl: bool,
    info_url: Option<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut data = None;
    let mut digest = false;
    let mut pnl = false;
    let mut info_url = None;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--data" => data = it.next(),
            "--digest" => digest = true,
            "--pnl" => pnl = true,
            "--info-url" => info_url = it.next(),
            "--help" | "-h" => {
                return Err("usage: galata-watcher-poll --data DIR [--digest | --pnl] [--info-url URL]".into());
            }
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    if digest && pnl {
        return Err("--digest and --pnl are separate runs".into());
    }
    Ok(Args {
        data: data.ok_or("--data is required")?,
        digest,
        pnl,
        info_url,
    })
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn print(alerts: &[Alert]) -> std::io::Result<()> {
    let mut out = std::io::stdout().lock();
    for a in alerts {
        serde_json::to_writer(&mut out, a)?;
        out.write_all(b"\n")?;
    }
    out.flush()
}

fn code(e: &CheckError) -> ExitCode {
    match e {
        CheckError::Config(_) => ExitCode::from(2),
        _ => ExitCode::from(1),
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let paths = Paths::new(&args.data);
    let now = now_ms();

    let url = match (&args.info_url, check::load_config(&paths)) {
        (Some(u), _) => u.clone(),
        (None, Ok(c)) => c.network.info_url(),
        (None, Err(e)) => {
            eprintln!("{e}");
            return code(&e);
        }
    };
    let http = Http::at(url);
    let info = Info::new(http.clone());

    if args.digest {
        return match digest::run(&info, &paths, now).await {
            Ok(None) => ExitCode::from(3),
            Ok(Some(outcome)) => {
                if let Err(e) = print(std::slice::from_ref(&outcome.alert)).and_then(|_| digest::commit(&paths, &outcome)) {
                    eprintln!("data: {e}");
                    return ExitCode::from(1);
                }
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("{e}");
                code(&e)
            }
        };
    }

    if args.pnl {
        return match pnl::run(&info, &paths, now).await {
            Ok(None) => ExitCode::from(3),
            Ok(Some(outcome)) => {
                if let Err(e) = print(std::slice::from_ref(&outcome.alert)).and_then(|_| pnl::commit(&paths, &outcome)) {
                    eprintln!("data: {e}");
                    return ExitCode::from(1);
                }
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("{e}");
                code(&e)
            }
        };
    }

    match check::run(&http, &info, &paths, now).await {
        Ok(outcome) => {
            if let Err(e) = print(&outcome.alerts).and_then(|_| check::commit(&paths, &outcome)) {
                eprintln!("data: {e}");
                return ExitCode::from(1);
            }
            if outcome.alerts.is_empty() { ExitCode::from(3) } else { ExitCode::SUCCESS }
        }
        Err(e) => {
            eprintln!("{e}");
            let _ = check::record_failure(&paths, &e, now);
            code(&e)
        }
    }
}
