//! A stop reaches the API. In a container it runs as PID 1, which the kernel
//! never terminates by default, so the binary must handle SIGTERM itself.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn sigterm_stops_the_api_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let token = dir.path().join("token");
    std::fs::write(&token, "t0ken\n").unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_galata-watcher-api"))
        .args(["--data", dir.path().to_str().unwrap(), "--bind", "127.0.0.1:0"])
        .args(["--token-file", token.to_str().unwrap()])
        .env_clear()
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut line = String::new();
    stderr.read_line(&mut line).unwrap();
    assert!(line.contains("listening"), "{line}");

    let killed = Command::new("kill").args(["-TERM", &child.id().to_string()]).status().unwrap();
    assert!(killed.success());

    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(started.elapsed() < Duration::from_secs(5), "still running five seconds after SIGTERM");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(status.code(), Some(0));
    line.clear();
    stderr.read_line(&mut line).unwrap();
    assert!(line.contains("stopped"), "{line}");
}
