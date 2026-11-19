//! galata-watcher-api: serve the watcher's configuration and alert history.
//!
//! ```text
//! galata-watcher-api --data /data --bind 0.0.0.0:8790 [--token-file /run/secrets/api_token]
//! ```
//!
//! Bind it to loopback, a LAN or a Tailscale address only. It is never
//! published to the internet.
//!
//! It stops on SIGTERM or SIGINT, after finishing the requests in flight. In a
//! container it runs as PID 1, which the kernel never terminates by default,
//! so without this a stop would wait for a SIGKILL.

use std::process::ExitCode;

use galata_watcher::files::Paths;
use galata_watcher_api::{App, router};

fn parse_args() -> Result<(String, String, String), String> {
    let mut data = None;
    let mut bind = "127.0.0.1:8790".to_string();
    let mut token_file = "/run/secrets/api_token".to_string();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--data" => data = it.next(),
            "--bind" => bind = it.next().ok_or("--bind needs an address")?,
            "--token-file" => token_file = it.next().ok_or("--token-file needs a path")?,
            "--help" | "-h" => return Err("usage: galata-watcher-api --data DIR [--bind ADDR:PORT] [--token-file PATH]".into()),
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    Ok((data.ok_or("--data is required")?, bind, token_file))
}

/// Resolves on the first SIGTERM (launchd, `container stop`, `docker stop`)
/// or SIGINT (Ctrl-C).
async fn stop_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate()).expect("install the SIGTERM handler");
    tokio::select! {
        _ = term.recv() => {}
        _ = tokio::signal::ctrl_c() => {}
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let (data, bind, token_file) = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let token = match std::fs::read_to_string(&token_file) {
        Ok(t) if !t.trim().is_empty() => t.trim().to_string(),
        Ok(_) => {
            eprintln!("token file {token_file} is empty");
            return ExitCode::from(2);
        }
        Err(e) => {
            eprintln!("token file {token_file}: {}", e.kind());
            return ExitCode::from(2);
        }
    };
    let listener = match tokio::net::TcpListener::bind(&bind).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("cannot listen on {bind}: {e}");
            return ExitCode::from(1);
        }
    };
    eprintln!("galata-watcher-api listening on {bind}");
    let serve = axum::serve(listener, router(App::new(Paths::new(data), token))).with_graceful_shutdown(stop_signal());
    match serve.await {
        Ok(()) => {
            eprintln!("galata-watcher-api stopped");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(1)
        }
    }
}
