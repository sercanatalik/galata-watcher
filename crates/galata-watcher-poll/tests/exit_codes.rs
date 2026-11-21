//! The binary's exit codes and output, against a local stand-in for the
//! venue's info endpoint.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

const ME: &str = "0x00000000000000000000000000000000000000aa";

type Route = Arc<Mutex<Box<dyn FnMut(&Value) -> Option<Value> + Send>>>;

/// Serve POSTs on a free port, answering each JSON body with `route`, or
/// 503 when it returns `None`. Returns the URL.
fn serve(route: impl FnMut(&Value) -> Option<Value> + Send + 'static) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/info", listener.local_addr().unwrap());
    let route: Route = Arc::new(Mutex::new(Box::new(route)));
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let route = route.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut stream = stream;
                loop {
                    let mut length = 0usize;
                    let mut line = String::new();
                    loop {
                        line.clear();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 {
                            return;
                        }
                        if line == "\r\n" {
                            break;
                        }
                        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                            length = v.trim().parse().unwrap();
                        }
                    }
                    let mut body = vec![0; length];
                    reader.read_exact(&mut body).unwrap();
                    let request: Value = serde_json::from_slice(&body).unwrap();
                    let answer = (route.lock().unwrap())(&request);
                    let (status, text) = match answer {
                        Some(v) => ("200 OK", v.to_string()),
                        None => ("503 Service Unavailable", String::new()),
                    };
                    let reply = format!(
                        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{text}",
                        text.len()
                    );
                    if stream.write_all(reply.as_bytes()).is_err() {
                        return;
                    }
                }
            });
        }
    });
    url
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64
}

/// A venue where the account's BTC stop filled 30 seconds ago.
fn venue_with_a_stop_fill() -> impl FnMut(&Value) -> Option<Value> + Send + 'static {
    let time = now_ms() - 30_000;
    move |req| {
        Some(match req["type"].as_str()? {
            "userFillsByTime" => json!([{"coin": "BTC", "px": "61240", "sz": "0.5", "side": "A", "time": time,
                "tid": 1, "oid": 9, "dir": "Close Long", "crossed": true, "fee": "1", "closedPnl": "0", "hash": "0x"}]),
            "historicalOrders" => json!([{"order": {"coin": "BTC", "side": "A", "limitPx": "1", "sz": "0", "origSz": "0.5",
                "oid": 9, "timestamp": 0, "orderType": "Stop Market", "reduceOnly": true, "isTrigger": true,
                "triggerCondition": "", "triggerPx": "1", "isPositionTpsl": true}, "status": "triggered", "statusTimestamp": 0}]),
            "perpDexs" => json!([null]),
            "clearinghouseState" => json!({"marginSummary": {"accountValue": "0", "totalNtlPos": "0", "totalMarginUsed": "0"},
                "withdrawable": "0", "assetPositions": []}),
            _ => return None,
        })
    }
}

fn data(config: Value) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.json"), config.to_string()).unwrap();
    let last = now_ms() - 60_000;
    std::fs::write(
        dir.path().join("state.json"),
        json!({"cursor_ms": last, "last_ok_ms": last}).to_string(),
    )
    .unwrap();
    dir
}

fn poll(dir: &tempfile::TempDir, url: &str) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_galata-watcher-poll"))
        .args(["--data", dir.path().to_str().unwrap(), "--info-url", url])
        .env_clear()
        .output()
        .unwrap();
    (out.status.code().unwrap(), String::from_utf8(out.stdout).unwrap())
}

fn config() -> Value {
    json!({"address": ME, "telegram_chat": "1"})
}

#[test]
fn a_stop_fill_prints_one_alert_and_exits_0_then_3() {
    let dir = data(config());
    let url = serve(venue_with_a_stop_fill());
    let (code, out) = poll(&dir, &url);
    assert_eq!(code, 0);
    let lines: Vec<Value> = out.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["kind"], "stop_loss");
    assert_eq!(lines[0]["text"], "BTC stop-loss filled: sold 0.5 at 61,240");

    let (code, out) = poll(&dir, &url);
    assert_eq!((code, out.as_str()), (3, ""), "nothing new the second time");
}

#[test]
fn a_venue_failure_exits_1_and_leaves_state_alone() {
    let dir = data(config());
    let before = std::fs::read(dir.path().join("state.json")).unwrap();
    let url = serve(|_| None);
    let (code, out) = poll(&dir, &url);
    assert_eq!((code, out.as_str()), (1, ""));
    assert_eq!(std::fs::read(dir.path().join("state.json")).unwrap(), before);
    let health: Value = serde_json::from_slice(&std::fs::read(dir.path().join("health.json")).unwrap()).unwrap();
    assert!(health["last_error"].as_str().unwrap().contains("503"));
}

#[test]
fn an_invalid_configuration_exits_2() {
    let dir = data(json!({"address": "0x1"}));
    let url = serve(venue_with_a_stop_fill());
    assert_eq!(poll(&dir, &url).0, 2);
}

#[test]
fn an_unknown_argument_exits_2() {
    let out = Command::new(env!("CARGO_BIN_EXE_galata-watcher-poll")).arg("--bogus").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}

/// A venue with no positions and a recorded-looking PnL history.
fn venue_for_a_summary(req: &Value) -> Option<Value> {
    let window = |pnl: &str| json!({"accountValueHistory": [[0, "1"]], "pnlHistory": [[0, "0"], [1, pnl]], "vlm": "0"});
    Some(match req["type"].as_str()? {
        "perpDexs" => json!([null]),
        "clearinghouseState" => json!({"marginSummary": {"accountValue": "0", "totalNtlPos": "0", "totalMarginUsed": "0"},
            "withdrawable": "0", "assetPositions": []}),
        "portfolio" => json!([["day", window("1234.6")], ["month", window("-4102.2")], ["allTime", window("18870")]]),
        "userFillsByTime" | "userFunding" | "userFills" => json!([]),
        _ => return None,
    })
}

/// A fixed-offset zone whose local hour is `hour` now.
fn zone_at(hour: i64) -> String {
    let utc_hour = (now_ms() / 3_600_000 % 24) as i64;
    let offset = (hour - utc_hour + 36) % 24 - 12; // -12..=11
    // Etc/GMT names are sign-inverted: Etc/GMT-3 is UTC+3.
    match offset {
        0 => "Etc/GMT".into(),
        o if o > 0 => format!("Etc/GMT-{o}"),
        o => format!("Etc/GMT+{}", -o),
    }
}

fn poll_pnl(dir: &tempfile::TempDir, url: &str) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_galata-watcher-poll"))
        .args(["--data", dir.path().to_str().unwrap(), "--info-url", url, "--pnl"])
        .env_clear()
        .output()
        .unwrap();
    (out.status.code().unwrap(), String::from_utf8(out.stdout).unwrap())
}

fn summary_config(zone: &str) -> Value {
    json!({"address": ME, "telegram_chat": "1", "digest": {"time": "07:00", "zone": zone}})
}

#[test]
fn a_summary_in_quiet_hours_exits_3() {
    let dir = data(summary_config(&zone_at(2)));
    let url = serve(venue_for_a_summary);
    assert_eq!(poll_pnl(&dir, &url), (3, String::new()));
}

/// Exit 0 needs the wall clock inside a slot's first ten minutes, which no
/// zone can arrange; outside them the run must exit 3. The 0 path itself is
/// covered without a clock by galata-watcher's `tests/pnl.rs`.
#[test]
fn a_due_summary_prints_one_alert_and_exits_0_then_3() {
    let dir = data(summary_config(&zone_at(12)));
    let url = serve(venue_for_a_summary);
    let in_grace = now_ms() / 60_000 % 30 < 9;
    let (code, out) = poll_pnl(&dir, &url);
    if !in_grace {
        assert_eq!((code, out.as_str()), (3, ""), "past the slot's grace");
        return;
    }
    assert_eq!(code, 0);
    let line: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(line["kind"], "pnl_summary");
    assert!(line["id"].as_str().unwrap().starts_with("pnl-"));
    assert!(line["text"].as_str().unwrap().contains("24h +$1,235\n30D -$4,102\nAll-time +$18,870"));
    assert_eq!(poll_pnl(&dir, &url), (3, String::new()), "once per slot");
}

#[test]
fn digest_and_pnl_together_exit_2() {
    let out = Command::new(env!("CARGO_BIN_EXE_galata-watcher-poll"))
        .args(["--data", "/nonexistent", "--digest", "--pnl"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}
