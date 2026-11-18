//! Protection of the backbone port: limits per IP address and in all, a rate
//! limit per link, temporary bans for peers that misbehave, and a block list
//! (IP addresses, networks and node addresses) kept in `<data>/blocked.txt`
//! and changed from the dashboard without a restart.
//!
//! Loopback is never limited or banned, so the host cannot lock itself out.

use maarse_core::Address;
use std::collections::HashMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

// The limits, block list networks, token buckets and ban rules live in
// maarse-hostrules (shared with the simulator).
pub use maarse_hostrules::guard::{DHT_PER_MIN, MAX_PER_IP, MAX_TOTAL, Net};
use maarse_hostrules::guard::{BYTES_PER_S, DhtWindow, LinkRate, PACKETS_PER_S, exempt};

/// Milliseconds on the guard's own monotonic clock (for the pure rules).
fn now_ms() -> u64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

#[derive(Default)]
struct State {
    active: HashMap<IpAddr, usize>,
    total: usize,
    bans: HashMap<IpAddr, (Instant, Duration, String)>,
    blocked_nets: Vec<Net>,
    blocked_nodes: Vec<Address>,
    dht: HashMap<IpAddr, DhtWindow>,
    file: Option<PathBuf>,
    refused: u64,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with<T>(f: impl FnOnce(&mut State) -> T) -> T {
    let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
    f(s.get_or_insert_with(State::default))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn parse_node(s: &str) -> Option<Address> {
    let s = s.trim();
    let b: Vec<u8> = (0..s.len()).step_by(2).filter_map(|i| s.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok())).collect();
    Address::from_slice(&b).filter(|_| s.len() == 32)
}

/// Loads `<data>/blocked.txt` and adds the `--block` entries.
pub fn init(data_dir: &std::path::Path, cli: &[String]) {
    let file = data_dir.join("blocked.txt");
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    with(|s| {
        s.file = Some(file);
        for l in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            add_entry(s, l);
        }
        for e in cli {
            add_entry(s, e);
        }
    });
}

fn add_entry(s: &mut State, e: &str) -> bool {
    if let Some(a) = parse_node(e) {
        if !s.blocked_nodes.contains(&a) {
            s.blocked_nodes.push(a);
        }
        return true;
    }
    if let Some(n) = Net::parse(e) {
        if !s.blocked_nets.contains(&n) {
            s.blocked_nets.push(n);
        }
        return true;
    }
    false
}

fn save(s: &State) {
    let Some(f) = &s.file else { return };
    let mut text = String::from("# Blocked IP addresses, networks (a.b.c.d/24) and node addresses (32 hex), one per line.\n");
    for n in &s.blocked_nets {
        text += &format!("{n}\n");
    }
    for a in &s.blocked_nodes {
        text += &format!("{}\n", hex(&a.0));
    }
    let _ = std::fs::write(f, text);
}

/// Adds an IP address, network or node address to the block list.
pub fn block(entry: &str) -> Result<(), String> {
    if Net::parse(entry).is_some_and(|n| exempt(&n.addr)) {
        return Err("this machine (loopback) is never blocked".into());
    }
    with(|s| {
        if !add_entry(s, entry) {
            return Err(format!("not an IP address, network or node address: {entry}"));
        }
        save(s);
        Ok(())
    })
}

pub fn unblock(entry: &str) -> bool {
    with(|s| {
        let before = s.blocked_nets.len() + s.blocked_nodes.len();
        if let Some(n) = Net::parse(entry) {
            s.blocked_nets.retain(|x| *x != n);
        }
        if let Some(a) = parse_node(entry) {
            s.blocked_nodes.retain(|x| *x != a);
        }
        s.bans.retain(|ip, _| Net::parse(entry).is_none_or(|n| !n.contains(ip)));
        let changed = before != s.blocked_nets.len() + s.blocked_nodes.len();
        save(s);
        changed
    })
}

pub fn node_blocked(a: &Address) -> bool {
    with(|s| s.blocked_nodes.contains(a))
}

/// Why `ip` may not connect now, if it may not.
fn refusal(s: &mut State, ip: &IpAddr) -> Option<String> {
    if exempt(ip) {
        return None;
    }
    if let Some(n) = s.blocked_nets.iter().find(|n| n.contains(ip)) {
        return Some(format!("blocked ({n})"));
    }
    s.bans.retain(|_, (t, d, _)| t.elapsed() < *d);
    if let Some((t, d, why)) = s.bans.get(ip) {
        return Some(format!("banned for {} more min ({why})", (d.saturating_sub(t.elapsed())).as_secs() / 60 + 1));
    }
    None
}

/// A connection's place; freed when dropped.
#[derive(Debug)]
pub struct Ticket(IpAddr);

impl Drop for Ticket {
    fn drop(&mut self) {
        let ip = self.0;
        with(|s| {
            s.total = s.total.saturating_sub(1);
            if let Some(n) = s.active.get_mut(&ip) {
                *n -= 1;
                if *n == 0 {
                    s.active.remove(&ip);
                }
            }
        });
    }
}

/// May a new connection from `ip` come in?
pub fn admit(ip: IpAddr) -> Result<Ticket, String> {
    with(|s| {
        if let Some(why) = refusal(s, &ip) {
            s.refused += 1;
            return Err(why);
        }
        if !exempt(&ip) {
            if s.total >= MAX_TOTAL {
                s.refused += 1;
                return Err(format!("{MAX_TOTAL} connections already"));
            }
            if s.active.get(&ip).copied().unwrap_or(0) >= MAX_PER_IP {
                s.refused += 1;
                return Err(format!("{MAX_PER_IP} connections from this address already"));
            }
        }
        s.total += 1;
        *s.active.entry(ip).or_default() += 1;
        Ok(Ticket(ip))
    })
}

/// May `ip` make another DHT request now?
pub fn dht_allowed(ip: IpAddr) -> bool {
    with(|s| {
        if exempt(&ip) {
            return true;
        }
        if refusal(s, &ip).is_some() {
            return false;
        }
        let now = now_ms();
        if s.dht.len() > 10_000 {
            s.dht.retain(|_, w| w.live(now));
        }
        s.dht.entry(ip).or_insert(DhtWindow::new(now)).allow(now)
    })
}

/// Bans `ip` for a while (not loopback).
pub fn ban(ip: IpAddr, d: Duration, why: &str) {
    if exempt(&ip) {
        return;
    }
    with(|s| {
        s.bans.insert(ip, (Instant::now(), d, why.to_string()));
    });
    println!("[guard] banned {ip} for {} min: {why}", d.as_secs() / 60);
    crate::debug::push("admin", serde_json::json!({"op": format!("banned for {} min: {why}", d.as_secs() / 60), "ip": ip.to_string()}));
}

/// Token buckets for one link (packets and bytes); see [`LinkRate`].
pub struct Rate(pub LinkRate);

impl Default for Rate {
    fn default() -> Self {
        Rate(LinkRate::new(now_ms()))
    }
}

impl Rate {
    /// A packet of `len` bytes: `Ok(true)` passes, `Ok(false)` is dropped,
    /// `Err` closes the link with a ban.
    pub fn on_packet(&mut self, len: usize) -> Result<bool, maarse_hostrules::guard::Ban> {
        self.0.on_packet(len, now_ms())
    }

    /// A packet that did not decode: a ban when there are too many.
    pub fn on_invalid(&mut self) -> Option<maarse_hostrules::guard::Ban> {
        self.0.on_invalid()
    }
}

/// For the dashboard.
pub fn json() -> serde_json::Value {
    with(|s| {
        s.bans.retain(|_, (t, d, _)| t.elapsed() < *d);
        serde_json::json!({
            "limits": {"per_ip": MAX_PER_IP, "total": MAX_TOTAL, "dht_per_min": DHT_PER_MIN, "packets_per_s": PACKETS_PER_S, "bytes_per_s": BYTES_PER_S},
            "active": s.active.iter().map(|(ip, n)| serde_json::json!({"ip": ip.to_string(), "links": n})).collect::<Vec<_>>(),
            "total": s.total,
            "refused": s.refused,
            "bans": s.bans.iter().map(|(ip, (t, d, why))| serde_json::json!({"ip": ip.to_string(), "left_s": d.saturating_sub(t.elapsed()).as_secs(), "why": why})).collect::<Vec<_>>(),
            "blocked": s.blocked_nets.iter().map(|n| n.to_string()).chain(s.blocked_nodes.iter().map(|a| hex(&a.0))).collect::<Vec<_>>(),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limit_allows_bursts_and_then_drops() {
        let mut r = Rate::default();
        let passed = (0..1000).filter(|_| r.on_packet(20) == Ok(true)).count();
        assert!((300..=310).contains(&passed), "{passed}");
        assert!(r.0.drops > 600);
    }

    #[test]
    fn limits_bans_and_block_list() {
        let ip: IpAddr = "198.51.100.200".parse().unwrap();
        let tickets: Vec<Ticket> = (0..MAX_PER_IP).map(|_| admit(ip).unwrap()).collect();
        assert!(admit(ip).is_err(), "one too many from one address");
        drop(tickets);
        assert!(admit(ip).is_ok(), "freed when the links close");
        let lo: IpAddr = "127.0.0.1".parse().unwrap();
        let many: Vec<Ticket> = (0..MAX_PER_IP * 2).map(|_| admit(lo).unwrap()).collect();
        drop(many);
        ban(ip, Duration::from_secs(60), "test");
        assert!(admit(ip).unwrap_err().contains("banned"));
        ban(lo, Duration::from_secs(60), "test");
        assert!(admit(lo).is_ok(), "loopback is never banned");
        block("192.0.2.0/24").unwrap();
        assert!(admit("192.0.2.9".parse().unwrap()).unwrap_err().contains("blocked"));
        assert!(block("127.0.0.1").is_err(), "loopback is never blocked");
        block(&"ab".repeat(16)).unwrap();
        assert!(node_blocked(&Address([0xab; 16])));
        assert!(unblock("192.0.2.0/24"));
        assert!(admit("192.0.2.9".parse().unwrap()).is_ok());
        assert!(block("not an address").is_err());
    }
}
