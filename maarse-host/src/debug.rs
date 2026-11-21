//! Debug view (/debug in the web client): everything that passes through
//! the host, for a person to watch and understand.
//!
//! The node pushes events (every packet in and out with its link and
//! decoded header, the router's decisions, DHT requests and lookups, post
//! office activity, log lines) into a ring buffer, and every 2 s a snapshot
//! of its tables (links, routes, neighbours, contacts, DHT, mailboxes,
//! trunks, counters). The web server hands both out as JSON
//! (`/api/debug/events?since=N`, `/api/debug/snapshot`); the page polls them.

use maarse_core::packet::{Header, PacketType};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Instant;

/// Events kept for the page (about 2-3 MB at most).
const KEEP: usize = 5000;

struct Ring {
    next: u64,
    events: VecDeque<(u64, String)>,
}

static RING: Mutex<Ring> = Mutex::new(Ring { next: 1, events: VecDeque::new() });
static SNAPSHOT: Mutex<String> = Mutex::new(String::new());
static TELEMETRY: Mutex<String> = Mutex::new(String::new());
/// Collecting at all (maarse-server only; the ordinary host keeps nothing).
static ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn enable() {
    ENABLED.store(true, std::sync::atomic::Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(std::sync::atomic::Ordering::Relaxed)
}

static PUBLIC_MAP: Mutex<String> = Mutex::new(String::new());
const TRAFFIC_SECONDS: u64 = 10;
static PUBLIC_TRAFFIC: Mutex<VecDeque<TrafficBucket>> = Mutex::new(VecDeque::new());
static TRAFFIC_SESSION: std::sync::LazyLock<u64> = std::sync::LazyLock::new(|| {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
});

#[derive(Default, serde::Serialize)]
struct TransportCounts {
    radio: u32,
    trunk: u32,
    backbone: u32,
}

#[derive(Default, serde::Serialize)]
struct TrafficBucket {
    second: u64,
    rx: TransportCounts,
    tx: TransportCounts,
}

/// Count locally observed frames. Public traffic never uses the debug ring.
pub fn count_public_packet(out: bool, transport: &str) {
    let mut buckets = PUBLIC_TRAFFIC.lock().unwrap_or_else(|e| e.into_inner());
    let second = since_start_ms() / 1000;
    if buckets.back().is_none_or(|b| b.second != second) {
        buckets.push_back(TrafficBucket { second, ..Default::default() });
    }
    while buckets.len() > TRAFFIC_SECONDS as usize + 1 {
        buckets.pop_front();
    }
    if let Some(b) = buckets.back_mut() {
        let counts = if out { &mut b.tx } else { &mut b.rx };
        let count = match transport {
            "radio" => &mut counts.radio,
            "trunk" => &mut counts.trunk,
            "backbone" => &mut counts.backbone,
            _ => return,
        };
        *count = count.saturating_add(1);
    }
}

/// Serve complete one-second buckets. A partial bucket is still changing.
pub fn public_traffic() -> String {
    let buckets = PUBLIC_TRAFFIC.lock().unwrap_or_else(|e| e.into_inner());
    let now = since_start_ms() / 1000;
    let recent: Vec<&TrafficBucket> = buckets.iter()
        .filter(|b| b.second >= now.saturating_sub(TRAFFIC_SECONDS) && b.second < now)
        .collect();
    json!({ "version": 1, "session": *TRAFFIC_SESSION, "second": now, "buckets": recent }).to_string()
}

/// The public map JSON (see `Node::public_map`), refreshed every 10 s.
pub fn set_public_map(s: String) {
    *PUBLIC_MAP.lock().unwrap_or_else(|e| e.into_inner()) = s;
}

pub fn public_map() -> String {
    let s = PUBLIC_MAP.lock().unwrap_or_else(|e| e.into_inner());
    if s.is_empty() { "{}".into() } else { s.clone() }
}

pub fn set_telemetry(s: String) {
    *TELEMETRY.lock().unwrap_or_else(|e| e.into_inner()) = s;
}

pub fn telemetry() -> String {
    let s = TELEMETRY.lock().unwrap_or_else(|e| e.into_inner());
    if s.is_empty() { "{}".into() } else { s.clone() }
}
static START: Mutex<Option<Instant>> = Mutex::new(None);

fn since_start_ms() -> u64 {
    let mut s = START.lock().unwrap_or_else(|e| e.into_inner());
    s.get_or_insert_with(Instant::now).elapsed().as_millis() as u64
}

/// Adds an event: `kind` (rx, tx, router, dht, mail, link, log, ...) and
/// its fields. The time (ms since start) and Unix time (ms) are added.
pub fn push(kind: &str, mut fields: Value) {
    if !enabled() {
        return;
    }
    let t = since_start_ms();
    if let Some(o) = fields.as_object_mut() {
        o.insert("kind".into(), kind.into());
        o.insert("t".into(), t.into());
        let wall = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        o.insert("wall".into(), wall.into());
    }
    let mut r = RING.lock().unwrap_or_else(|e| e.into_inner());
    let seq = r.next;
    r.next += 1;
    if let Some(o) = fields.as_object_mut() {
        o.insert("seq".into(), seq.into());
    }
    r.events.push_back((seq, fields.to_string()));
    while r.events.len() > KEEP {
        r.events.pop_front();
    }
}

/// Events after `since`, at most 1500 per call.
pub fn events_json(since: u64) -> String {
    let r = RING.lock().unwrap_or_else(|e| e.into_inner());
    let items: Vec<&str> = r.events.iter().filter(|(s, _)| *s > since).take(1500).map(|(_, j)| j.as_str()).collect();
    let next = r.events.iter().filter(|(s, _)| *s > since).take(1500).last().map_or(since.max(r.next - 1), |(s, _)| *s);
    format!("{{\"next\":{next},\"events\":[{}]}}", items.join(","))
}

pub fn set_snapshot(s: String) {
    *SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()) = s;
}

pub fn snapshot() -> String {
    let s = SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner());
    if s.is_empty() { "{}".into() } else { s.clone() }
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// A packet's header and size as JSON fields (type, hops, destination,
/// path, region, size, packet ID).
pub fn packet_fields(data: &[u8]) -> Value {
    match Header::decode(data) {
        Ok((h, used)) => {
            let payload = &data[used.min(data.len())..];
            let pid = maarse_core::dedup::packet_id(h.ptype, &h.dest, payload);
            json!({
                "ptype": format!("{:?}", h.ptype),
                "hops": h.hops,
                "dest": hex(&h.dest.0),
                "path": h.path.as_ref().map(|p| p.iter().map(|x| format!("{x:04x}")).collect::<Vec<_>>()),
                "region": h.region,
                "size": data.len(),
                "pid": format!("{pid:016x}"),
                "flood": h.path.is_none() && !matches!(h.ptype, PacketType::Hello | PacketType::Summary),
            })
        }
        Err(e) => json!({ "ptype": "invalid", "size": data.len(), "error": format!("{e:?}") }),
    }
}
