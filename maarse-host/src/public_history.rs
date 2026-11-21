//! Up to 24 hours of the public feed, kept on disk so that the website can
//! step back in time (`GET /api/public/history`).
//!
//! Per minute: the packets received and sent per transport and per packet
//! type. Besides that the announces (which node was heard, and when) and the
//! messages of open channels. Nothing here is private: it is the same
//! information `/api/public/traffic` and `/api/public/network` show live,
//! only kept for longer. The file is written about once a minute, so a
//! restart loses at most that much.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const STEP_MS: u64 = 60_000;
const MAX_ANNOUNCES: usize = 100_000;
const MAX_MESSAGES: usize = 5_000;
const MAX_HOURS: u64 = 24 * 7;
const FLUSH: Duration = Duration::from_secs(60);
const FILE: &str = "public-history.json";

#[derive(Clone, Default, Serialize, Deserialize)]
struct Counts {
    radio: u32,
    trunk: u32,
    backbone: u32,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct Minute {
    t: u64,
    rx: Counts,
    tx: Counts,
    types: BTreeMap<String, u32>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Announce {
    t: u64,
    addr: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct Message {
    time: u64,
    channel: String,
    sender: String,
    text: String,
}

#[derive(Default, Serialize, Deserialize)]
struct State {
    minutes: VecDeque<Minute>,
    announces: VecDeque<Announce>,
    messages: VecDeque<Message>,
}

struct Store {
    path: Option<PathBuf>,
    hours: u64,
    state: State,
    last_flush: Option<Instant>,
    dirty: bool,
}

static STORE: Mutex<Store> = Mutex::new(Store {
    path: None,
    hours: 0,
    state: State { minutes: VecDeque::new(), announces: VecDeque::new(), messages: VecDeque::new() },
    last_flush: None,
    dirty: false,
});

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn lock() -> std::sync::MutexGuard<'static, Store> {
    STORE.lock().unwrap_or_else(|err| err.into_inner())
}

/// Start keeping history for `hours` hours in `dir` (0 = keep nothing). Reads
/// what an earlier run left behind.
pub fn init(dir: &Path, hours: u64) {
    let mut store = lock();
    store.hours = hours.min(MAX_HOURS);
    store.state = State::default();
    store.dirty = false;
    if store.hours == 0 {
        store.path = None;
        return;
    }
    let path = dir.join(FILE);
    if let Ok(bytes) = std::fs::read(&path)
        && let Ok(state) = serde_json::from_slice::<State>(&bytes)
    {
        store.state = state;
        prune(&mut store);
    }
    store.path = Some(path);
}

pub fn enabled() -> bool {
    lock().hours > 0
}

fn prune(store: &mut Store) {
    let oldest = now_ms().saturating_sub(store.hours * 3_600_000);
    while store.state.minutes.front().is_some_and(|m| m.t + STEP_MS <= oldest) {
        store.state.minutes.pop_front();
    }
    while store.state.announces.front().is_some_and(|a| a.t < oldest) || store.state.announces.len() > MAX_ANNOUNCES {
        store.state.announces.pop_front();
    }
    while store.state.messages.front().is_some_and(|m| m.time < oldest) || store.state.messages.len() > MAX_MESSAGES {
        store.state.messages.pop_front();
    }
}

fn flush_if_due(store: &mut Store) {
    let due = store.dirty && store.last_flush.is_none_or(|t| t.elapsed() >= FLUSH);
    let Some(path) = store.path.clone().filter(|_| due) else { return };
    store.last_flush = Some(Instant::now());
    prune(store);
    let Ok(bytes) = serde_json::to_vec(&store.state) else { return };
    let pending = path.with_extension("tmp");
    if std::fs::write(&pending, bytes).is_ok() && std::fs::rename(&pending, &path).is_ok() {
        store.dirty = false;
    }
}

/// Count one packet (`out` = sent) of a packet type on a transport
/// (`radio`, `trunk` or `backbone`).
pub fn count(out: bool, transport: &str, ptype: &str) {
    let mut store = lock();
    if store.hours == 0 {
        return;
    }
    let start = now_ms() / STEP_MS * STEP_MS;
    if store.state.minutes.back().is_none_or(|m| m.t != start) {
        store.state.minutes.push_back(Minute { t: start, ..Default::default() });
        prune(&mut store);
    }
    let Some(minute) = store.state.minutes.back_mut() else { return };
    let counts = if out { &mut minute.tx } else { &mut minute.rx };
    let slot = match transport {
        "radio" => &mut counts.radio,
        "trunk" => &mut counts.trunk,
        "backbone" => &mut counts.backbone,
        _ => return,
    };
    *slot = slot.saturating_add(1);
    *minute.types.entry(ptype.to_string()).or_insert(0) += 1;
    store.dirty = true;
    flush_if_due(&mut store);
}

/// A node was heard announcing itself.
pub fn announce(addr: &str) {
    let mut store = lock();
    if store.hours == 0 {
        return;
    }
    store.state.announces.push_back(Announce { t: now_ms(), addr: addr.to_string() });
    store.dirty = true;
    prune(&mut store);
    flush_if_due(&mut store);
}

/// A message on an open channel.
pub fn message(time: u64, channel: &str, sender: &str, text: &str) {
    let mut store = lock();
    if store.hours == 0 {
        return;
    }
    store.state.messages.push_back(Message { time, channel: channel.to_string(), sender: sender.to_string(), text: text.to_string() });
    store.dirty = true;
    prune(&mut store);
    flush_if_due(&mut store);
}

/// The history from `since` (Unix milliseconds) on, as JSON.
pub fn json(since: u64) -> String {
    let store = lock();
    let s = &store.state;
    serde_json::json!({
        "version": 1,
        "step_ms": STEP_MS,
        "now": now_ms(),
        "hours": store.hours,
        "buckets": s.minutes.iter().filter(|m| m.t + STEP_MS > since).collect::<Vec<_>>(),
        "announces": s.announces.iter().filter(|a| a.t >= since).collect::<Vec<_>>(),
        "messages": s.messages.iter().filter(|m| m.time >= since).collect::<Vec<_>>(),
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_per_minute_and_type_and_comes_back_as_json() {
        let dir = std::env::temp_dir().join(format!("maarse-history-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        init(&dir, 24);
        count(false, "backbone", "Announce");
        count(false, "backbone", "Announce");
        count(true, "radio", "Data");
        announce("0123456789abcdef0123456789abcdef");
        message(now_ms(), "public", "01234567", "hello");
        let v: serde_json::Value = serde_json::from_str(&json(0)).unwrap();
        assert_eq!(v["hours"], 24);
        assert_eq!(v["buckets"][0]["rx"]["backbone"], 2);
        assert_eq!(v["buckets"][0]["tx"]["radio"], 1);
        assert_eq!(v["buckets"][0]["types"]["Announce"], 2);
        assert_eq!(v["announces"][0]["addr"], "0123456789abcdef0123456789abcdef");
        assert_eq!(v["messages"][0]["text"], "hello");
        // Nothing older than `since` is returned.
        let later: serde_json::Value = serde_json::from_str(&json(now_ms() + 3_600_000)).unwrap();
        assert!(later["buckets"].as_array().unwrap().is_empty());
        init(&dir, 0);
        count(false, "backbone", "Announce");
        assert!(serde_json::from_str::<serde_json::Value>(&json(0)).unwrap()["buckets"].as_array().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
