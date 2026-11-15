//! Service page support: an in-memory ring buffer of everything the host
//! prints, plus the status JSON for `/api/service/status`.
//!
//! The crate root shadows `println!`/`eprintln!` with macros that also call
//! [`push`], so every log line ends up here without touching each call site.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// How many lines the host keeps in memory.
pub const LOG_CAPACITY: usize = 2000;

/// One remembered log line.
#[derive(Clone, Debug, PartialEq)]
pub struct LogLine {
    /// Sequence number, starting at 1 and never reused while the process runs.
    pub seq: u64,
    /// Unix time in milliseconds.
    pub t_ms: u64,
    /// Written to stderr.
    pub err: bool,
    pub text: String,
}

/// Fixed-size ring of log lines with monotonically increasing sequence numbers.
pub struct LogRing {
    cap: usize,
    next: u64,
    lines: VecDeque<LogLine>,
}

impl LogRing {
    pub const fn new(cap: usize) -> Self {
        LogRing { cap, next: 1, lines: VecDeque::new() }
    }

    /// Adds a (possibly multi-line) message; each line gets its own entry.
    pub fn push(&mut self, text: &str, err: bool, t_ms: u64) {
        for line in text.split('\n') {
            let line = line.trim_end_matches('\r');
            if self.lines.len() >= self.cap {
                self.lines.pop_front();
            }
            self.lines.push_back(LogLine { seq: self.next, t_ms, err, text: line.to_string() });
            self.next += 1;
        }
    }

    /// Lines with `seq > since`, oldest first.
    pub fn since(&self, since: u64) -> Vec<LogLine> {
        self.lines.iter().filter(|l| l.seq > since).cloned().collect()
    }

    /// Sequence number of the newest line (0 when empty).
    pub fn last_seq(&self) -> u64 {
        self.next - 1
    }

    /// Sequence number of the oldest line still kept (0 when empty).
    pub fn first_seq(&self) -> u64 {
        self.lines.front().map(|l| l.seq).unwrap_or(0)
    }
}

static LOG: Mutex<LogRing> = Mutex::new(LogRing::new(LOG_CAPACITY));
static START: Mutex<Option<(Instant, u64)>> = Mutex::new(None);

fn unix_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Remembers a printed line. Called by the crate's `println!`/`eprintln!`.
pub fn push(text: &str, err: bool) {
    let mut log = LOG.lock().unwrap_or_else(|e| e.into_inner());
    log.push(text, err, unix_ms());
}

/// Marks the process start time (call once at startup).
pub fn mark_start() {
    let mut s = START.lock().unwrap_or_else(|e| e.into_inner());
    if s.is_none() {
        *s = Some((Instant::now(), unix_ms() / 1000));
    }
}

/// JSON for `GET /api/service/log?since=N`.
pub fn log_json(since: u64) -> String {
    let log = LOG.lock().unwrap_or_else(|e| e.into_inner());
    let lines: Vec<serde_json::Value> = log
        .since(since)
        .into_iter()
        .map(|l| serde_json::json!({ "seq": l.seq, "t": l.t_ms, "err": l.err, "text": l.text }))
        .collect();
    serde_json::json!({
        "first": log.first_seq(),
        "last": log.last_seq(),
        "capacity": LOG_CAPACITY,
        "lines": lines,
    })
    .to_string()
}

/// JSON for `GET /api/service/status`.
pub fn status_json(data_dir: &Path) -> String {
    let (uptime, started) = START
        .lock()
        .ok()
        .and_then(|s| *s)
        .map(|(i, t)| (i.elapsed().as_secs(), t))
        .unwrap_or((0, 0));
    let devices: serde_json::Value =
        serde_json::from_str(&crate::links::devices_json()).unwrap_or(serde_json::json!({ "devices": [] }));
    let data = std::fs::canonicalize(data_dir).unwrap_or_else(|_| data_dir.to_path_buf());
    let mut data = data.display().to_string();
    if let Some(s) = data.strip_prefix(r"\\?\") {
        data = s.to_string();
    }
    serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "pid": std::process::id(),
        "uptime_s": uptime,
        "started": started,
        "data_dir": data,
        "service": crate::settings::SERVICE.load(std::sync::atomic::Ordering::Relaxed),
        "restart_exit_code": crate::settings::RESTART_EXIT_CODE,
        "links": devices["devices"],
        "log_last": LOG.lock().map(|l| l.last_seq()).unwrap_or(0),
    })
    .to_string()
}

/// Stops the host cleanly with exit code 0 (a service manager leaves it stopped).
pub fn stop() -> ! {
    println!("\n[service] stop requested from the web client, exiting with code 0\n");
    std::process::exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_keeps_newest_lines_and_sequence() {
        let mut r = LogRing::new(3);
        assert_eq!((r.first_seq(), r.last_seq()), (0, 0));
        for i in 1..=5 {
            r.push(&format!("line {i}"), i == 2, i);
        }
        assert_eq!(r.first_seq(), 3);
        assert_eq!(r.last_seq(), 5);
        let all = r.since(0);
        assert_eq!(all.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["line 3", "line 4", "line 5"]);
        assert_eq!(r.since(4).len(), 1);
        assert!(r.since(5).is_empty());
    }

    #[test]
    fn ring_splits_multiline_messages() {
        let mut r = LogRing::new(10);
        r.push("\nhello\r\nworld\n", true, 7);
        let v = r.since(0);
        assert_eq!(v.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["", "hello", "world", ""]);
        assert!(v.iter().all(|l| l.err && l.t_ms == 7));
        assert_eq!(r.last_seq(), 4);
    }
}
