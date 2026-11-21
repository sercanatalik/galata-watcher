//! Delivery, retries and the liveness flag, against a local stand-in for
//! Telegram's Bot API.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

const TOKEN: &str = "123456:SECRET-TOKEN";

/// Every request the fake saw: its path and JSON body.
type Seen = Arc<Mutex<Vec<(String, Value)>>>;

/// A fake Bot API. `accept(n)` decides whether the nth request (from 0)
/// succeeds. Returns the base URL, the request count and the bodies seen.
fn telegram(accept: impl Fn(usize) -> bool + Send + Sync + 'static) -> (String, Arc<AtomicUsize>, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let count = Arc::new(AtomicUsize::new(0));
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let accept = Arc::new(accept);
    let (c, s) = (count.clone(), seen.clone());
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (c, s, accept) = (c.clone(), s.clone(), accept.clone());
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut stream = stream;
                loop {
                    let mut request_line = String::new();
                    if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
                        return;
                    }
                    let path = request_line.split_whitespace().nth(1).unwrap_or("").to_string();
                    let mut length = 0;
                    loop {
                        let mut line = String::new();
                        reader.read_line(&mut line).unwrap();
                        if line == "\r\n" {
                            break;
                        }
                        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                            length = v.trim().parse().unwrap();
                        }
                    }
                    let mut body = vec![0; length];
                    reader.read_exact(&mut body).unwrap();
                    let n = c.fetch_add(1, Ordering::SeqCst);
                    s.lock().unwrap().push((path, serde_json::from_slice(&body).unwrap()));
                    let parsed: Value = s.lock().unwrap().last().unwrap().1.clone();
                    let unparsable = parsed["parse_mode"] == "HTML" && parsed["text"].as_str().unwrap_or("").contains("<broken");
                    let (status, text) = if unparsable {
                        ("400 Bad Request", json!({"ok": false, "description": "Bad Request: can't parse entities: unsupported start tag \"broken\""}).to_string())
                    } else if accept(n) {
                        ("200 OK", json!({"ok": true, "result": {}}).to_string())
                    } else {
                        ("429 Too Many Requests", json!({"ok": false, "description": "Too Many Requests: retry later"}).to_string())
                    };
                    let reply = format!("HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{text}", text.len());
                    if stream.write_all(reply.as_bytes()).is_err() {
                        return;
                    }
                }
            });
        }
    });
    (base, count, seen)
}

struct Data {
    dir: tempfile::TempDir,
}

impl Data {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.json"), json!({"address": "0x0", "telegram_chat": "777"}).to_string()).unwrap();
        std::fs::write(dir.path().join("token"), format!("{TOKEN}\n")).unwrap();
        std::fs::write(dir.path().join("health.json"), json!({"last_error": "venue: the venue answered HTTP 503"}).to_string()).unwrap();
        Data { dir }
    }
    fn notify(&self, base: &str, args: &[&str], stdin: &str) -> (i32, String) {
        let d = self.dir.path();
        let mut child = Command::new(env!("CARGO_BIN_EXE_galata-notify"))
            .args(["--data", d.to_str().unwrap(), "--token-file", d.join("token").to_str().unwrap()])
            .args(["--api-base", base, "--backoff-ms", "10"])
            .args(args)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        (out.status.code().unwrap(), String::from_utf8(out.stderr).unwrap())
    }
    fn deliveries(&self) -> Vec<Value> {
        std::fs::read_to_string(self.dir.path().join("notify/deliveries.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }
    fn broken(&self) -> bool {
        self.dir.path().join("notify/broken").exists()
    }
}

#[test]
fn rejected_once_then_accepted_is_one_delivery() {
    let data = Data::new();
    let (base, count, seen) = telegram(|n| n >= 1);
    let (code, _) = data.notify(&base, &["--alert-id", "sl-9"], "BTC stop-loss filled: sold 0.5 at 61,240\n");
    assert_eq!(code, 0);
    assert_eq!(count.load(Ordering::SeqCst), 2);
    let (path, body) = seen.lock().unwrap()[1].clone();
    assert_eq!(path, format!("/bot{TOKEN}/sendMessage"));
    assert_eq!(body["chat_id"], "777");
    assert_eq!(body["text"], "BTC stop-loss filled: sold 0.5 at 61,240");
    let d = data.deliveries();
    assert_eq!(d.len(), 1);
    assert_eq!((d[0]["alert_id"].as_str(), d[0]["delivered"].as_bool()), (Some("sl-9"), Some(true)));
}

#[test]
fn html_is_sent_with_its_parse_mode() {
    let (base, _, seen) = telegram(|_| true);
    let data = Data::new();
    let (code, _) = data.notify(&base, &["--alert-id", "sl-9", "--html"], "🔴 <b>BTC stop-loss filled</b>\n");
    assert_eq!(code, 0);
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].1["parse_mode"], "HTML");
    assert_eq!(seen[0].1["text"], "🔴 <b>BTC stop-loss filled</b>");
}

#[test]
fn unparsable_html_is_sent_again_as_plain_text() {
    let (base, _, seen) = telegram(|_| true);
    let data = Data::new();
    let (code, _) = data.notify(&base, &["--alert-id", "lv-1", "--html"], "<b>BTC</b> <broken>58,000 &amp; up");
    assert_eq!(code, 0);
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2, "one HTML try, one plain");
    assert!(seen[1].1.get("parse_mode").is_none());
    assert_eq!(seen[1].1["text"], "BTC 58,000 & up");
    assert_eq!(data.deliveries().last().unwrap()["delivered"], true);
}

#[test]
fn plain_text_has_no_parse_mode() {
    let (base, _, seen) = telegram(|_| true);
    let data = Data::new();
    assert_eq!(data.notify(&base, &["--alert-id", "a"], "x < y").0, 0);
    assert!(seen.lock().unwrap()[0].1.get("parse_mode").is_none());
}

#[test]
fn always_rejected_is_recorded_as_a_failure_and_exits_1_without_the_token() {
    let data = Data::new();
    let (base, count, _) = telegram(|_| false);
    let (code, stderr) = data.notify(&base, &["--alert-id", "sl-9"], "x");
    assert_eq!(code, 1);
    assert_eq!(count.load(Ordering::SeqCst), 3, "one attempt and two retries");
    let d = data.deliveries();
    assert_eq!(d[0]["delivered"], false);
    let error = d[0]["error"].as_str().unwrap();
    assert!(error.contains("429"));
    assert!(!error.contains("SECRET") && !stderr.contains("SECRET"));
}

#[test]
fn an_unreachable_telegram_never_reveals_the_token() {
    let data = Data::new();
    // Nothing listens on port 9 of localhost.
    let (code, stderr) = data.notify("http://127.0.0.1:9", &["--alert-id", "a"], "x");
    assert_eq!(code, 1);
    let line = serde_json::to_string(&data.deliveries()).unwrap();
    assert!(!line.contains("SECRET") && !stderr.contains("SECRET"), "{line} {stderr}");
}

#[test]
fn broken_twice_sends_once_with_the_last_error() {
    let data = Data::new();
    let (base, count, seen) = telegram(|_| true);
    assert_eq!(data.notify(&base, &["--liveness", "broken"], "").0, 0);
    assert_eq!(data.notify(&base, &["--liveness", "broken"], "").0, 0);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert!(data.broken());
    let text = seen.lock().unwrap()[0].1["text"].as_str().unwrap().to_string();
    assert!(text.starts_with("galata-watcher broken"));
    assert!(text.contains("HTTP 503"));
}

#[test]
fn ok_without_the_flag_touches_no_network() {
    let data = Data::new();
    let (base, count, _) = telegram(|_| true);
    assert_eq!(data.notify(&base, &["--liveness", "ok"], "").0, 0);
    assert_eq!(count.load(Ordering::SeqCst), 0);
    assert!(data.deliveries().is_empty());
}

#[test]
fn ok_with_the_flag_sends_recovered_and_clears_it() {
    let data = Data::new();
    let (base, count, seen) = telegram(|_| true);
    data.notify(&base, &["--liveness", "broken"], "");
    assert_eq!(data.notify(&base, &["--liveness", "ok"], "").0, 0);
    assert_eq!(count.load(Ordering::SeqCst), 2);
    assert!(!data.broken());
    assert!(seen.lock().unwrap()[1].1["text"].as_str().unwrap().contains("recovered"));
}

#[test]
fn a_missing_token_file_exits_2() {
    let data = Data::new();
    std::fs::remove_file(data.dir.path().join("token")).unwrap();
    let (base, count, _) = telegram(|_| true);
    assert_eq!(data.notify(&base, &["--alert-id", "a"], "x").0, 2);
    assert_eq!(count.load(Ordering::SeqCst), 0);
}
