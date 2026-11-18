//! The backbone port guard's pure rules: limits, block list networks, token
//! buckets per link, the DHT request window and when a link is banned. The
//! sockets, the shared state and `blocked.txt` stay in `maarse-host`.

use core::net::IpAddr;

/// Incoming backbone connections from one IP address at the same time.
pub const MAX_PER_IP: usize = 8;
/// Incoming backbone connections in all.
pub const MAX_TOTAL: usize = 256;
/// DHT requests from one IP address per minute.
pub const DHT_PER_MIN: u32 = 120;
/// The DHT request window.
pub const DHT_WINDOW_MS: u64 = 60_000;
/// Per link: packets per second (sustained) and the burst allowed.
pub const PACKETS_PER_S: f64 = 100.0;
pub const PACKET_BURST: f64 = 300.0;
/// Per link: bytes per second (sustained); the burst is 4 s worth.
pub const BYTES_PER_S: f64 = 64.0 * 1024.0;
pub const BYTE_BURST: f64 = BYTES_PER_S * 4.0;
/// A link that keeps going over its rate is closed after this many drops.
pub const DROPS_BEFORE_BAN: u32 = 500;
/// A link sending this many packets that do not decode is closed.
pub const INVALID_BEFORE_BAN: u32 = 20;
/// How long an address is banned for each reason.
pub const BAN_RATE_MS: u64 = 10 * 60 * 1000;
pub const BAN_INVALID_MS: u64 = 15 * 60 * 1000;

/// A block list entry: an IP network (address and prefix length).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Net {
    pub addr: IpAddr,
    pub prefix: u8,
}

impl Net {
    pub fn parse(s: &str) -> Option<Net> {
        let (a, p) = s.split_once('/').map_or((s, None), |(a, p)| (a, Some(p)));
        let addr: IpAddr = a.trim().parse().ok()?;
        let max = if addr.is_ipv4() { 32 } else { 128 };
        let prefix = match p {
            Some(p) => p.trim().parse::<u8>().ok().filter(|&p| p <= max)?,
            None => max,
        };
        Some(Net { addr, prefix })
    }

    pub fn contains(&self, ip: &IpAddr) -> bool {
        fn bits(a: &IpAddr) -> (u128, u8) {
            match a {
                IpAddr::V4(v) => (u32::from(*v) as u128, 32),
                IpAddr::V6(v) => (u128::from(*v), 128),
            }
        }
        let ip = match ip {
            IpAddr::V6(v) => v.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(*ip),
            _ => *ip,
        };
        let ((a, la), (b, lb)) = (bits(&self.addr), bits(&ip));
        if la != lb {
            return false;
        }
        let shift = la - self.prefix;
        shift >= la || (a >> shift) == (b >> shift)
    }
}

impl core::fmt::Display for Net {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let max = if self.addr.is_ipv4() { 32 } else { 128 };
        if self.prefix == max { write!(f, "{}", self.addr) } else { write!(f, "{}/{}", self.addr, self.prefix) }
    }
}

/// Loopback is never limited, banned or blocked.
pub fn exempt(ip: &IpAddr) -> bool {
    ip.is_loopback() || matches!(ip, IpAddr::V6(v) if v.to_ipv4_mapped().is_some_and(|m| m.is_loopback()))
}

/// Token buckets for one link (packets and bytes), with the drop and
/// invalid counters that decide a ban.
#[derive(Clone, Debug)]
pub struct LinkRate {
    pkts: f64,
    bytes: f64,
    last_ms: u64,
    pub drops: u32,
    pub invalid: u32,
}

/// Why a link is closed and its address banned, and for how long.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ban {
    pub ms: u64,
    pub why: &'static str,
}

impl LinkRate {
    /// Full buckets at `now_ms`.
    pub fn new(now_ms: u64) -> Self {
        LinkRate { pkts: PACKET_BURST, bytes: BYTE_BURST, last_ms: now_ms, drops: 0, invalid: 0 }
    }

    /// May a packet of `len` bytes pass at `now_ms`?
    pub fn allow(&mut self, len: usize, now_ms: u64) -> bool {
        let dt = now_ms.saturating_sub(self.last_ms) as f64 / 1000.0;
        self.last_ms = self.last_ms.max(now_ms);
        self.pkts = (self.pkts + dt * PACKETS_PER_S).min(PACKET_BURST);
        self.bytes = (self.bytes + dt * BYTES_PER_S).min(BYTE_BURST);
        if self.pkts >= 1.0 && self.bytes >= len as f64 {
            self.pkts -= 1.0;
            self.bytes -= len as f64;
            true
        } else {
            self.drops += 1;
            false
        }
    }

    /// A packet that did not decode: a ban when there are too many.
    pub fn on_invalid(&mut self) -> Option<Ban> {
        self.invalid += 1;
        (self.invalid > INVALID_BEFORE_BAN).then_some(Ban { ms: BAN_INVALID_MS, why: "too many invalid packets" })
    }

    /// A packet of `len` bytes at `now_ms`: `Ok(true)` passes, `Ok(false)`
    /// is dropped, `Err` closes the link with a ban.
    pub fn on_packet(&mut self, len: usize, now_ms: u64) -> Result<bool, Ban> {
        if self.allow(len, now_ms) {
            return Ok(true);
        }
        if self.drops > DROPS_BEFORE_BAN {
            return Err(Ban { ms: BAN_RATE_MS, why: "kept sending over the rate limit" });
        }
        Ok(false)
    }
}

/// DHT requests from one address in the current minute.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DhtWindow {
    pub start_ms: u64,
    pub count: u32,
}

impl DhtWindow {
    pub fn new(now_ms: u64) -> Self {
        DhtWindow { start_ms: now_ms, count: 0 }
    }

    /// Counts a request at `now_ms`; true while within [`DHT_PER_MIN`].
    pub fn allow(&mut self, now_ms: u64) -> bool {
        if now_ms.saturating_sub(self.start_ms) > DHT_WINDOW_MS {
            *self = DhtWindow::new(now_ms);
        }
        self.count += 1;
        self.count <= DHT_PER_MIN
    }

    /// Still worth keeping at `now_ms`?
    pub fn live(&self, now_ms: u64) -> bool {
        now_ms.saturating_sub(self.start_ms) < DHT_WINDOW_MS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nets_match() {
        let n = Net::parse("203.0.113.0/24").unwrap();
        assert!(n.contains(&"203.0.113.77".parse().unwrap()));
        assert!(!n.contains(&"203.0.114.1".parse().unwrap()));
        assert!(n.contains(&"::ffff:203.0.113.5".parse().unwrap()), "IPv4-mapped IPv6");
        assert!(Net::parse("2001:db8::/32").unwrap().contains(&"2001:db8:1::9".parse().unwrap()));
        assert!(Net::parse("0.0.0.0/0").unwrap().contains(&"8.8.8.8".parse().unwrap()));
        assert!(Net::parse("1.2.3.4/33").is_none() && Net::parse("nonsense").is_none());
        assert_eq!(Net::parse("198.51.100.9").unwrap().to_string(), "198.51.100.9");
    }

    #[test]
    fn loopback_is_exempt() {
        assert!(exempt(&"127.0.0.1".parse().unwrap()));
        assert!(exempt(&"::1".parse().unwrap()));
        assert!(exempt(&"::ffff:127.0.0.1".parse().unwrap()));
        assert!(!exempt(&"192.0.2.1".parse().unwrap()));
    }

    #[test]
    fn rate_limit_allows_bursts_and_then_drops() {
        let mut r = LinkRate::new(0);
        let passed = (0..1000).filter(|_| r.allow(20, 0)).count();
        assert_eq!(passed, 300);
        assert_eq!(r.drops, 700);
        // One second later, 100 more packets.
        let passed = (0..1000).filter(|_| r.allow(20, 1000)).count();
        assert_eq!(passed, 100);
    }

    #[test]
    fn bytes_limit_too() {
        let mut r = LinkRate::new(0);
        assert!(r.allow(BYTE_BURST as usize, 0));
        assert!(!r.allow(1, 0));
        assert!(!r.allow(1024, 15), "64 KB/s refills 1 KB in 16 ms");
        assert!(r.allow(1024, 16));
    }

    #[test]
    fn bans() {
        let mut r = LinkRate::new(0);
        for _ in 0..INVALID_BEFORE_BAN {
            assert_eq!(r.on_invalid(), None);
        }
        assert_eq!(r.on_invalid().map(|b| b.ms), Some(BAN_INVALID_MS));
        let mut r = LinkRate::new(0);
        let mut ban = None;
        for i in 0..2000 {
            if let Err(b) = r.on_packet(10, 0) {
                ban = Some((i, b));
                break;
            }
        }
        let (i, b) = ban.unwrap();
        assert_eq!(i, PACKET_BURST as u32 + DROPS_BEFORE_BAN);
        assert_eq!(b.ms, BAN_RATE_MS);
    }

    #[test]
    fn dht_window() {
        let mut w = DhtWindow::new(0);
        assert!((0..DHT_PER_MIN).all(|_| w.allow(10)));
        assert!(!w.allow(10));
        assert!(!w.allow(DHT_WINDOW_MS), "the window is a full minute");
        assert!(w.allow(DHT_WINDOW_MS + 1), "a new minute");
        assert!(w.live(DHT_WINDOW_MS + 2) && !w.live(2 * DHT_WINDOW_MS + 1));
    }
}
