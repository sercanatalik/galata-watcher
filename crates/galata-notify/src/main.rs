//! galata-notify: send one message to the trader's Telegram chat.
//!
//! ```text
//! galata-notify --data /data --alert-id ID          < message
//! galata-notify --data /data --liveness broken|ok
//! ```
//!
//! The chat id comes from `config.json`. The bot token is read from a file
//! of its own (`/run/secrets/telegram_token` unless `--token-file` says
//! otherwise), because the lane starts every tool with an empty environment.
//! Every outcome is appended to `notify/deliveries.jsonl`.
//!
//! `--liveness broken` sends "watcher broken" (with the last check's error
//! from `health.json`) only when the `notify/broken` flag is absent, then
//! sets it. `--liveness ok` sends "recovered" only when the flag is present,
//! then clears it; without the flag it returns at once, touching no network.
//!
//! Exit codes: 0 sent, or nothing to send; 1 not delivered after retries;
//! 2 bad argument, configuration or token file.
//!
//! This binary does not depend on galata-desk-core, so other Galata lanes
//! can use it.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::json;

const TELEGRAM: &str = "https://api.telegram.org";
const TOKEN_FILE: &str = "/run/secrets/telegram_token";
const RETRIES: u32 = 2;

#[derive(Debug, PartialEq)]
enum Mode {
    Alert(String),
    Broken,
    Ok,
}

struct Args {
    data: PathBuf,
    mode: Mode,
    token_file: PathBuf,
    api_base: String,
    backoff: Duration,
}

const USAGE: &str = "usage: galata-notify --data DIR (--alert-id ID < message | --liveness broken|ok) [--token-file PATH]";

fn parse_args(args: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut data = None;
    let mut alert_id = None;
    let mut liveness = None;
    let mut token_file = PathBuf::from(TOKEN_FILE);
    let mut api_base = TELEGRAM.to_string();
    let mut backoff = Duration::from_secs(1);
    let mut it = args;
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().ok_or(format!("{name} needs a value"));
        match a.as_str() {
            "--data" => data = Some(PathBuf::from(value("--data")?)),
            "--alert-id" => alert_id = Some(value("--alert-id")?),
            "--liveness" => liveness = Some(value("--liveness")?),
            "--token-file" => token_file = PathBuf::from(value("--token-file")?),
            "--api-base" => api_base = value("--api-base")?,
            "--backoff-ms" => {
                backoff = Duration::from_millis(value("--backoff-ms")?.parse().map_err(|_| "--backoff-ms takes milliseconds")?)
            }
            "--help" | "-h" => return Err(USAGE.into()),
            other => return Err(format!("unknown argument `{other}`\n{USAGE}")),
        }
    }
    let mode = match (alert_id, liveness.as_deref()) {
        (Some(id), None) => Mode::Alert(id),
        (None, Some("broken")) => Mode::Broken,
        (None, Some("ok")) => Mode::Ok,
        (None, Some(other)) => return Err(format!("--liveness takes `broken` or `ok`, not `{other}`")),
        (Some(_), Some(_)) => return Err("give --alert-id or --liveness, not both".into()),
        (None, None) => return Err("--alert-id or --liveness is required".into()),
    };
    Ok(Args {
        data: data.ok_or("--data is required")?,
        mode,
        token_file,
        api_base,
        backoff,
    })
}

/// What galata-watcher's digest reads back; the same shape it defines.
#[derive(Serialize)]
struct Delivery<'a> {
    alert_id: &'a str,
    delivered: bool,
    at_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Deserialize)]
struct Config {
    telegram_chat: serde_json::Value,
}

#[derive(Deserialize, Default)]
struct Health {
    last_error: Option<String>,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn chat_id(data: &Path) -> Result<String, String> {
    let text = fs::read_to_string(data.join("config.json")).map_err(|e| format!("config.json: {e}"))?;
    let config: Config = serde_json::from_str(&text).map_err(|e| format!("config.json: {e}"))?;
    match config.telegram_chat {
        serde_json::Value::String(s) if !s.trim().is_empty() => Ok(s.trim().to_string()),
        serde_json::Value::Number(n) => Ok(n.to_string()),
        _ => Err("config.json: `telegram_chat` is missing".into()),
    }
}

fn token(path: &Path) -> Result<String, String> {
    let t = fs::read_to_string(path).map_err(|e| format!("token file {}: {}", path.display(), e.kind()))?;
    let t = t.trim().to_string();
    if t.is_empty() {
        return Err(format!("token file {} is empty", path.display()));
    }
    Ok(t)
}

/// An error in words that never include the request URL, which holds the
/// bot token.
fn describe(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "Telegram timed out".into()
    } else if e.is_connect() {
        "Telegram could not be reached".into()
    } else {
        "the request to Telegram failed".into()
    }
}

async fn send_once(client: &reqwest::Client, base: &str, token: &str, chat: &str, text: &str) -> Result<(), String> {
    let url = format!("{base}/bot{token}/sendMessage");
    let response = client
        .post(url)
        .json(&json!({ "chat_id": chat, "text": text, "disable_web_page_preview": true }))
        .send()
        .await
        .map_err(|e| describe(&e))?;
    let status = response.status().as_u16();
    let body: serde_json::Value = response.json().await.unwrap_or_default();
    if status == 200 && body["ok"] == true {
        Ok(())
    } else {
        let why = body["description"].as_str().unwrap_or("no description");
        // Telegram's description never echoes the token, but be certain.
        Err(format!("Telegram answered {status}: {}", why.replace(token, "<token>")))
    }
}

/// Send with retries, backing off between attempts.
async fn send(args: &Args, token: &str, chat: &str, text: &str) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|_| "the HTTP client could not be built".to_string())?;
    let mut last = String::new();
    for attempt in 0..=RETRIES {
        if attempt > 0 {
            tokio::time::sleep(args.backoff * 2u32.pow(attempt - 1)).await;
        }
        match send_once(&client, &args.api_base, token, chat, text).await {
            Ok(()) => return Ok(()),
            Err(e) => last = e,
        }
    }
    Err(last)
}

fn record(data: &Path, alert_id: &str, outcome: &Result<(), String>) -> std::io::Result<()> {
    let dir = data.join("notify");
    fs::create_dir_all(&dir)?;
    let line = serde_json::to_string(&Delivery {
        alert_id,
        delivered: outcome.is_ok(),
        at_ms: now_ms(),
        error: outcome.as_ref().err().cloned(),
    })?;
    let mut f = fs::OpenOptions::new().create(true).append(true).open(dir.join("deliveries.jsonl"))?;
    f.write_all(line.as_bytes())?;
    f.write_all(b"\n")?;
    f.sync_all()
}

async fn run(args: Args) -> ExitCode {
    let flag = args.data.join("notify").join("broken");
    let (alert_id, text) = match &args.mode {
        Mode::Ok if !flag.exists() => return ExitCode::SUCCESS,
        Mode::Broken if flag.exists() => return ExitCode::SUCCESS,
        Mode::Ok => (format!("liveness-ok-{}", now_ms()), "galata-watcher recovered: checks are succeeding again.".to_string()),
        Mode::Broken => {
            let health: Health = fs::read(args.data.join("health.json"))
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default();
            let why = health.last_error.unwrap_or_else(|| "no error recorded".into());
            (
                format!("liveness-broken-{}", now_ms()),
                format!("galata-watcher broken: 3 checks in a row failed. Last error: {why}"),
            )
        }
        Mode::Alert(id) => {
            let mut text = String::new();
            if std::io::stdin().read_to_string(&mut text).is_err() || text.trim().is_empty() {
                eprintln!("the message is read from stdin, and none was given");
                return ExitCode::from(2);
            }
            (id.clone(), text.trim_end().to_string())
        }
    };

    let (chat, token) = match (chat_id(&args.data), token(&args.token_file)) {
        (Ok(c), Ok(t)) => (c, t),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };

    let outcome = send(&args, &token, &chat, &text).await;
    if let Err(e) = record(&args.data, &alert_id, &outcome) {
        eprintln!("deliveries.jsonl: {e}");
    }
    match outcome {
        Ok(()) => {
            let flag_result = match args.mode {
                Mode::Broken => fs::write(&flag, now_ms().to_string()),
                Mode::Ok => fs::remove_file(&flag),
                Mode::Alert(_) => Ok(()),
            };
            if let Err(e) = flag_result {
                eprintln!("notify/broken: {e}");
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("not delivered: {e}");
            ExitCode::from(1)
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    match parse_args(std::env::args().skip(1)) {
        Ok(args) => run(args).await,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Result<Args, String> {
        parse_args(s.split_whitespace().map(String::from))
    }

    #[test]
    fn modes_are_read_from_arguments() {
        assert_eq!(args("--data d --alert-id sl-1").unwrap().mode, Mode::Alert("sl-1".into()));
        assert_eq!(args("--data d --liveness broken").unwrap().mode, Mode::Broken);
        assert!(args("--data d").is_err());
        assert!(args("--data d --liveness maybe").is_err());
        assert!(args("--data d --alert-id a --liveness ok").is_err());
    }
}
