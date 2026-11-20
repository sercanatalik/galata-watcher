//! The watcher's files on its data volume, and the two ways it writes them:
//! whole files replaced atomically, and logs appended a line at a time.
//!
//! Each file has one writer (design decision 4), so no file needs a lock.

use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

/// Where each file lives under the data directory.
#[derive(Debug, Clone)]
pub struct Paths {
    pub root: PathBuf,
}

impl Paths {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
    /// Written by the API; read by the poll.
    pub fn config(&self) -> PathBuf {
        self.root.join("config.json")
    }
    /// Written by the poll only.
    pub fn state(&self) -> PathBuf {
        self.root.join("state.json")
    }
    /// Written by the poll's digest mode only: when the last digest went.
    pub fn digest(&self) -> PathBuf {
        self.root.join("digest.json")
    }
    /// Written by the poll's PnL mode only: the last summary's slot.
    pub fn pnl(&self) -> PathBuf {
        self.root.join("pnl.json")
    }
    /// Written by the poll only: the outcome of the latest check.
    pub fn health(&self) -> PathBuf {
        self.root.join("health.json")
    }
    /// Appended by the poll only: every alert detected.
    pub fn alerts(&self) -> PathBuf {
        self.root.join("alerts.jsonl")
    }
    /// Appended by galata-notify only: every delivery attempt's outcome.
    pub fn deliveries(&self) -> PathBuf {
        self.root.join("notify").join("deliveries.jsonl")
    }
    /// Present while the watcher is known to be broken. galata-notify only.
    pub fn broken_flag(&self) -> PathBuf {
        self.root.join("notify").join("broken")
    }
}

/// Replace `path` with `bytes` so a reader sees the old file or the new one,
/// never a part of either: write a temporary file beside it, then rename.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(dir)?;
    let mut tmp = tempfile_in(dir, path)?;
    tmp.1.write_all(bytes)?;
    tmp.1.sync_all()?;
    drop(tmp.1);
    fs::rename(&tmp.0, path)
}

fn tempfile_in(dir: &Path, path: &Path) -> io::Result<(PathBuf, fs::File)> {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    for n in 0..100u32 {
        let candidate = dir.join(format!(".{name}.tmp-{}-{n}", std::process::id()));
        match OpenOptions::new().write(true).create_new(true).open(&candidate) {
            Ok(file) => return Ok((candidate, file)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other("no free temporary file name"))
}

/// Write a value as pretty JSON, atomically.
pub fn write_json<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    bytes.push(b'\n');
    write_atomic(path, &bytes)
}

/// Read a JSON file; `None` when it does not exist.
pub fn read_json<T: DeserializeOwned>(path: &Path) -> io::Result<Option<T>> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {e}", path.display()))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Append one value as one JSON line, flushed to disk before returning.
pub fn append_line<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut line = serde_json::to_vec(value).map_err(io::Error::other)?;
    line.push(b'\n');
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(&line)?;
    file.sync_all()
}

/// Every line of a JSON-lines file that parses as `T`, in file order. A
/// missing file is empty. A line that does not parse (a torn last line
/// after a crash) is skipped rather than failing the whole read.
pub fn read_lines<T: DeserializeOwned>(path: &Path) -> io::Result<Vec<T>> {
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for line in io::BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str(&line) {
            out.push(v);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_atomic_write_leaves_no_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.json");
        write_atomic(&path, b"one").unwrap();
        write_atomic(&path, b"two").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn a_torn_last_line_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("l.jsonl");
        append_line(&path, &serde_json::json!({"a": 1})).unwrap();
        fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(b"{\"a\": 2").unwrap();
        let rows: Vec<serde_json::Value> = read_lines(&path).unwrap();
        assert_eq!(rows.len(), 1);
    }
}
