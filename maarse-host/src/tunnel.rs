//! IP tunnel: IPv6 over Må:rse, so that e.g. ping, SSH and mosh can run between nodes.
//!
//! Every node has an IPv6 address derived from its Må:rse address:
//! `fd` + the first 15 bytes (ULA, `fd00::/8`).
//!
//! ## Optimised for LoRa
//! * **Session:** the sender creates a random key and a session ID and
//!   sends them once in an encrypted direct message (content type `0x07`,
//!   type 1). After that the IP traffic goes as `Datagram` packets (type 10) with
//!   only 28 bytes of overhead (ID 8, counter 4, MAC 16) and without end-to-end
//!   acknowledgement; per-hop acknowledgement and retransmission still apply. The key
//!   is changed every 5 minutes.
//! * **Header compression:** the IPv6 header (40 bytes) becomes 2-7 bytes when
//!   the addresses are the sender's and recipient's own; the UDP header 8 -> 6.
//! * **MSS clamping:** TCP SYN gets an MSS so the segments fit in one packet.
//! * **Fragmentation and NACK:** larger packets are split into pieces; if pieces are missing
//!   after a few seconds, the recipient asks for exactly those.
//!
//! ```text
//! Datagram:    session ID (8) | counter (u32) | ChaCha20-Poly1305(content)
//! Content:     1 | packet ID | piece no. | count | data      (piece)
//!              2 | packet ID | missing pieces (u16, bitmask) (NACK)
//! Compressed:  0x80|flag | next header | [hop limit] | [traffic class+flow (4)] | ...
//!              UDP: source port | dest port | checksum | data (length is derived)
//!              0x00 | the whole IPv6 packet (when it cannot be compressed)
//! ```
//!
//! Over TCP the tunnel is always used; over LoRa only with `--tun-lora`.
//! `--tun` creates a network adapter (Wintun on Windows, `/dev/net/tun` on
//! Linux; requires administrator/root and the `tun` feature). Without an adapter the
//! host itself answers ping (ICMPv6 echo) to its address.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use maarse_core::Address;
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

/// Largest IP packet (the IPv6 minimum).
pub const MTU: usize = 1280;
/// Overhead in a datagram: session ID, counter and MAC.
pub const DG_OVERHEAD: usize = 8 + 4 + 16;
/// Data per piece in a datagram.
pub const FRAG_DATA: usize = maarse_core::router::MAX_DIRECT_PAYLOAD - DG_OVERHEAD - 4;
/// MSS for TCP through the tunnel: one segment per piece (the IPv6 header is
/// compressed; the TCP header with options is up to about 32 bytes).
pub const TUN_MSS: u16 = (FRAG_DATA - 7 - 32) as u16;
const REKEY_AFTER: Duration = Duration::from_secs(300);
const NACK_AFTER: Duration = Duration::from_secs(2);
const GIVE_UP: Duration = Duration::from_secs(12);
const KEEP_SENT: Duration = Duration::from_secs(15);

/// Control message (sent sealed as content type `0x07`): new session key.
pub const CTRL_KEY: u8 = 1;

/// The IPv6 address for a Må:rse address.
pub fn ip_of(a: &Address) -> [u8; 16] {
    let mut ip = [0u8; 16];
    ip[0] = 0xfd;
    ip[1..].copy_from_slice(&a.0[..15]);
    ip
}

pub fn ip_text(ip: &[u8; 16]) -> String {
    std::net::Ipv6Addr::from(*ip).to_string()
}

/// The destination address in an IPv6 packet.
pub fn ipv6_dst(p: &[u8]) -> Option<[u8; 16]> {
    (p.len() >= 40 && p[0] >> 4 == 6).then(|| p[24..40].try_into().unwrap())
}

// ---------- header compression ----------

/// Compresses an IPv6 packet from `src` to `dst`. If the addresses do not match,
/// the packet is sent whole (prefixed with 0x00).
pub fn compress(p: &[u8], src: &[u8; 16], dst: &[u8; 16]) -> Vec<u8> {
    let plen = if p.len() >= 40 { u16::from_be_bytes([p[4], p[5]]) as usize } else { 0 };
    if p.len() < 40 || p[0] >> 4 != 6 || &p[8..24] != src || &p[24..40] != dst || p.len() != 40 + plen {
        let mut v = vec![0x00];
        v.extend_from_slice(p);
        return v;
    }
    let tcfl = [p[0] & 0x0f, p[1], p[2], p[3]];
    let (nh, hl) = (p[6], p[7]);
    let udp = nh == 17 && plen >= 8 && u16::from_be_bytes([p[44], p[45]]) as usize == plen;
    let mut flags = 0x80u8;
    if hl != 64 {
        flags |= 0x01;
    }
    if tcfl != [0; 4] {
        flags |= 0x02;
    }
    if udp {
        flags |= 0x04;
    }
    let mut v = vec![flags, nh];
    if hl != 64 {
        v.push(hl);
    }
    if tcfl != [0; 4] {
        v.extend_from_slice(&tcfl);
    }
    if udp {
        v.extend_from_slice(&p[40..44]); // ports
        v.extend_from_slice(&p[46..48]); // checksum
        v.extend_from_slice(&p[48..]);
    } else {
        v.extend_from_slice(&p[40..]);
    }
    v
}

/// Restores the IPv6 packet.
pub fn decompress(c: &[u8], src: &[u8; 16], dst: &[u8; 16]) -> Option<Vec<u8>> {
    let (&flags, rest) = c.split_first()?;
    if flags == 0x00 {
        return Some(rest.to_vec());
    }
    if flags & 0x80 == 0 {
        return None;
    }
    let (&nh, mut r) = rest.split_first()?;
    let hl = if flags & 0x01 != 0 {
        let (&h, t) = r.split_first()?;
        r = t;
        h
    } else {
        64
    };
    let mut tcfl = [0u8; 4];
    if flags & 0x02 != 0 {
        tcfl.copy_from_slice(r.get(..4)?);
        r = &r[4..];
    }
    let body: Vec<u8> = if flags & 0x04 != 0 {
        let h = r.get(..6)?;
        let data = &r[6..];
        let len = (8 + data.len()) as u16;
        let mut b = Vec::with_capacity(8 + data.len());
        b.extend_from_slice(&h[..4]);
        b.extend_from_slice(&len.to_be_bytes());
        b.extend_from_slice(&h[4..6]);
        b.extend_from_slice(data);
        b
    } else {
        r.to_vec()
    };
    if body.len() > MTU - 40 {
        return None;
    }
    let mut p = Vec::with_capacity(40 + body.len());
    p.push(0x60 | (tcfl[0] & 0x0f));
    p.extend_from_slice(&tcfl[1..4]);
    p.extend_from_slice(&(body.len() as u16).to_be_bytes());
    p.push(nh);
    p.push(hl);
    p.extend_from_slice(src);
    p.extend_from_slice(dst);
    p.extend_from_slice(&body);
    Some(p)
}

// ---------- MSS clamping ----------

/// Lowers the MSS in a TCP SYN (IPv6 without extension headers) to `mss` and
/// recomputes the checksum. Returns true if the packet was changed.
pub fn clamp_mss(p: &mut [u8], mss: u16) -> bool {
    if p.len() < 60 || p[0] >> 4 != 6 || p[6] != 6 {
        return false;
    }
    let t = 40;
    let off = ((p[t + 12] >> 4) as usize) * 4;
    if p[t + 13] & 0x02 == 0 || off < 20 || p.len() < t + off {
        return false; // not SYN
    }
    let mut i = t + 20;
    let mut changed = false;
    while i < t + off {
        match p[i] {
            0 => break,
            1 => i += 1,
            kind => {
                let Some(&len) = p.get(i + 1) else { break };
                if len < 2 || i + len as usize > t + off {
                    break;
                }
                if kind == 2 && len == 4 && u16::from_be_bytes([p[i + 2], p[i + 3]]) > mss {
                    p[i + 2..i + 4].copy_from_slice(&mss.to_be_bytes());
                    changed = true;
                }
                i += len as usize;
            }
        }
    }
    if changed {
        p[t + 16] = 0;
        p[t + 17] = 0;
        let c = l4_checksum(p, 6);
        p[t + 16..t + 18].copy_from_slice(&c.to_be_bytes());
    }
    changed
}

/// Checksum for TCP/UDP/ICMPv6 with the IPv6 pseudo header.
fn l4_checksum(p: &[u8], next_header: u8) -> u16 {
    let body = &p[40..];
    let mut sum: u32 = 0;
    let mut add = |b: &[u8]| {
        for c in b.chunks(2) {
            sum += u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)]) as u32;
        }
    };
    add(&p[8..40]);
    add(&(body.len() as u32).to_be_bytes());
    add(&[0, 0, 0, next_header]);
    add(body);
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

/// Ping reply: if `p` is an ICMPv6 echo request to `me`, the reply is returned.
pub fn echo_reply(p: &[u8], me: &[u8; 16]) -> Option<Vec<u8>> {
    if p.len() < 48 || p[0] >> 4 != 6 || p[6] != 58 || &p[24..40] != me || p[40] != 128 {
        return None;
    }
    let plen = u16::from_be_bytes([p[4], p[5]]) as usize;
    if p.len() < 40 + plen {
        return None;
    }
    let mut r = p[..40 + plen].to_vec();
    r[8..24].copy_from_slice(&p[24..40]);
    r[24..40].copy_from_slice(&p[8..24]);
    r[7] = 64;
    r[40] = 129;
    r[42] = 0;
    r[43] = 0;
    let c = l4_checksum(&r, 58);
    r[42..44].copy_from_slice(&c.to_be_bytes());
    Some(r)
}

// ---------- sessions ----------

fn seal(key: &[u8; 32], id: &[u8; 8], ctr: u32, plain: &[u8]) -> Vec<u8> {
    let mut nonce = [0u8; 12];
    nonce[..8].copy_from_slice(id);
    nonce[8..].copy_from_slice(&ctr.to_be_bytes());
    let mut head = Vec::with_capacity(DG_OVERHEAD + plain.len());
    head.extend_from_slice(id);
    head.extend_from_slice(&ctr.to_be_bytes());
    let ct = ChaCha20Poly1305::new(Key::from_slice(key))
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: plain, aad: &head })
        .expect("encryption cannot fail");
    head.extend_from_slice(&ct);
    head
}

fn open(key: &[u8; 32], dg: &[u8]) -> Option<(u32, Vec<u8>)> {
    if dg.len() < DG_OVERHEAD {
        return None;
    }
    let ctr = u32::from_be_bytes(dg[8..12].try_into().ok()?);
    let mut nonce = [0u8; 12];
    nonce.copy_from_slice(&dg[..12]);
    let plain = ChaCha20Poly1305::new(Key::from_slice(key))
        .decrypt(Nonce::from_slice(&nonce), Payload { msg: &dg[12..], aad: &dg[..12] })
        .ok()?;
    Some((ctr, plain))
}

struct OutSession {
    id: [u8; 8],
    key: [u8; 32],
    ctr: u32,
    created: Instant,
    pkt: u8,
}

struct InSession {
    peer: Address,
    key: [u8; 32],
    /// Highest counter and a bitmask over the 64 before it (against replays).
    max: u32,
    seen: u64,
    created: Instant,
}

impl InSession {
    fn accept(&mut self, ctr: u32) -> bool {
        if ctr > self.max || (self.max == 0 && self.seen == 0) {
            let shift = ctr.saturating_sub(self.max);
            self.seen = if shift >= 64 { 0 } else { self.seen << shift };
            self.seen |= 1;
            self.max = ctr;
            return true;
        }
        let back = self.max - ctr;
        if back >= 64 || self.seen & (1 << back) != 0 {
            return false;
        }
        self.seen |= 1 << back;
        true
    }
}

struct Partial {
    started: Instant,
    nacked: u8,
    parts: Vec<Option<Vec<u8>>>,
}

/// What the tunnel asks the node to do.
#[derive(Debug, PartialEq, Eq)]
pub enum TunOut {
    /// Send a datagram (PacketType::Datagram) to the node.
    Datagram(Address, Vec<u8>),
    /// Send a sealed control message (content type 0x07) to the node.
    Control(Address, Vec<u8>),
    /// A whole, decompressed IP packet from the node.
    Packet(Address, Vec<u8>),
}

/// The tunnel's state in a host.
#[derive(Default)]
pub struct Tunnel {
    out: HashMap<Address, OutSession>,
    inn: HashMap<[u8; 8], InSession>,
    /// Datagrams with an unknown session (the key may be on its way).
    early: VecDeque<(Instant, Vec<u8>)>,
    partial: HashMap<(Address, u8), Partial>,
    sent: HashMap<(Address, u8), (Instant, Vec<Vec<u8>>)>,
    pub stats: TunStats,
}

#[derive(Default, Debug, Clone, Copy)]
pub struct TunStats {
    pub sent_packets: u64,
    pub sent_datagrams: u64,
    pub recv_packets: u64,
    pub nacks_sent: u64,
    pub resent: u64,
    pub bytes_saved: u64,
}

impl Tunnel {
    /// Sends an IP packet from us (`me`) to `dest`.
    pub fn send(&mut self, me: &Address, dest: Address, packet: &[u8], now: Instant, rng: &mut impl FnMut(&mut [u8])) -> Vec<TunOut> {
        let mut out = Vec::new();
        let mut p = packet.to_vec();
        clamp_mss(&mut p, TUN_MSS);
        let c = compress(&p, &ip_of(me), &ip_of(&dest));
        self.stats.bytes_saved += p.len().saturating_sub(c.len()) as u64;
        let fresh = self.out.get(&dest).is_none_or(|s| now.duration_since(s.created) > REKEY_AFTER);
        if fresh {
            let mut id = [0u8; 8];
            let mut key = [0u8; 32];
            rng(&mut id);
            rng(&mut key);
            let mut ctrl = vec![CTRL_KEY];
            ctrl.extend_from_slice(&id);
            ctrl.extend_from_slice(&key);
            out.push(TunOut::Control(dest, ctrl));
            self.out.insert(dest, OutSession { id, key, ctr: 0, created: now, pkt: 0 });
        }
        let s = self.out.get_mut(&dest).expect("just created");
        s.pkt = s.pkt.wrapping_add(1);
        let pkt = s.pkt;
        let chunks: Vec<&[u8]> = c.chunks(FRAG_DATA).collect();
        if chunks.len() > 16 {
            return out; // larger than the MTU; should not happen
        }
        let mut frags = Vec::new();
        for (i, ch) in chunks.iter().enumerate() {
            let mut plain = vec![1, pkt, i as u8, chunks.len() as u8];
            plain.extend_from_slice(ch);
            s.ctr += 1;
            let dg = seal(&s.key, &s.id, s.ctr, &plain);
            frags.push(plain);
            out.push(TunOut::Datagram(dest, dg));
        }
        self.stats.sent_packets += 1;
        self.stats.sent_datagrams += chunks.len() as u64;
        if chunks.len() > 1 {
            self.sent.insert((dest, pkt), (now, frags));
        }
        out
    }

    /// A control message (content type 0x07) from `from`.
    pub fn on_control(&mut self, from: Address, data: &[u8], me: &Address, now: Instant) -> Vec<TunOut> {
        if data.len() < 41 || data[0] != CTRL_KEY {
            return Vec::new();
        }
        let id: [u8; 8] = data[1..9].try_into().unwrap();
        let key: [u8; 32] = data[9..41].try_into().unwrap();
        self.inn.retain(|_, s| s.peer != from || now.duration_since(s.created) < REKEY_AFTER * 2);
        self.inn.insert(id, InSession { peer: from, key, max: 0, seen: 0, created: now });
        // Datagrams that arrived before the key.
        let early: Vec<Vec<u8>> = std::mem::take(&mut self.early).into_iter().map(|(_, d)| d).collect();
        let mut out = Vec::new();
        for d in early {
            out.extend(self.on_datagram(&d, me, now));
        }
        out
    }

    /// A datagram for us.
    pub fn on_datagram(&mut self, dg: &[u8], me: &Address, now: Instant) -> Vec<TunOut> {
        let mut out = Vec::new();
        let Some(id) = dg.get(..8).and_then(|b| <[u8; 8]>::try_from(b).ok()) else { return out };
        let Some(s) = self.inn.get_mut(&id) else {
            self.early.retain(|(t, _)| now.duration_since(*t) < Duration::from_secs(10));
            if self.early.len() < 32 {
                self.early.push_back((now, dg.to_vec()));
            }
            return out;
        };
        let Some((ctr, plain)) = open(&s.key, dg) else { return out };
        if !s.accept(ctr) {
            return out;
        }
        let peer = s.peer;
        match plain.first() {
            Some(1) if plain.len() >= 4 => {
                let (pkt, idx, n) = (plain[1], plain[2] as usize, plain[3] as usize);
                if n == 0 || idx >= n || n > 16 {
                    return out;
                }
                let data = plain[4..].to_vec();
                let whole = if n == 1 {
                    Some(data)
                } else {
                    let e = self.partial.entry((peer, pkt)).or_insert_with(|| Partial { started: now, nacked: 0, parts: vec![None; n] });
                    if e.parts.len() != n {
                        return out;
                    }
                    e.parts[idx] = Some(data);
                    if e.parts.iter().all(Option::is_some) {
                        let p = self.partial.remove(&(peer, pkt)).unwrap();
                        Some(p.parts.into_iter().flatten().flatten().collect())
                    } else {
                        None
                    }
                };
                if let Some(c) = whole
                    && let Some(p) = decompress(&c, &ip_of(&peer), &ip_of(me))
                {
                    self.stats.recv_packets += 1;
                    out.push(TunOut::Packet(peer, p));
                }
            }
            Some(2) if plain.len() >= 4 => {
                let (pkt, mask) = (plain[1], u16::from_be_bytes([plain[2], plain[3]]));
                if let Some((_, frags)) = self.sent.get(&(peer, pkt)) {
                    let frags: Vec<Vec<u8>> =
                        frags.iter().enumerate().filter(|(i, _)| mask & (1 << i) != 0).map(|(_, f)| f.clone()).collect();
                    for f in frags {
                        if let Some(o) = self.out.get_mut(&peer) {
                            o.ctr += 1;
                            out.push(TunOut::Datagram(peer, seal(&o.key, &o.id, o.ctr, &f)));
                            self.stats.resent += 1;
                        }
                    }
                }
            }
            _ => {}
        }
        out
    }

    /// Called regularly: NACK for incomplete packets, cleanup.
    pub fn tick(&mut self, now: Instant) -> Vec<TunOut> {
        let mut out = Vec::new();
        self.sent.retain(|_, (t, _)| now.duration_since(*t) < KEEP_SENT);
        self.partial.retain(|_, p| now.duration_since(p.started) < GIVE_UP);
        let mut nacks = Vec::new();
        for ((peer, pkt), p) in self.partial.iter_mut() {
            let due = NACK_AFTER * (p.nacked as u32 + 1);
            if p.nacked < 3 && now.duration_since(p.started) >= due {
                p.nacked += 1;
                let mask = p.parts.iter().enumerate().filter(|(_, x)| x.is_none()).fold(0u16, |m, (i, _)| m | 1 << i);
                nacks.push((*peer, *pkt, mask));
            }
        }
        for (peer, pkt, mask) in nacks {
            if let Some(o) = self.out.get_mut(&peer) {
                let mut plain = vec![2, pkt];
                plain.extend_from_slice(&mask.to_be_bytes());
                o.ctr += 1;
                out.push(TunOut::Datagram(peer, seal(&o.key, &o.id, o.ctr, &plain)));
                self.stats.nacks_sent += 1;
            }
            // Without an outgoing session we cannot send a NACK; the sender gets none.
        }
        out
    }
}

/// Creates the adapter and starts reader and writer threads. Packets from
/// the adapter are sent to the node as `In::TunPacket`; returns a channel
/// that writes packets to the adapter.
#[cfg(feature = "tun")]
pub fn spawn(
    me: [u8; 16],
    wintun: Option<String>,
    tx: std::sync::mpsc::Sender<crate::In>,
) -> std::io::Result<std::sync::mpsc::Sender<Vec<u8>>> {
    use std::sync::Arc;
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut b = tun_rs::DeviceBuilder::new()
        .name("maarse")
        .ipv6(std::net::Ipv6Addr::from(me), 8u8)
        .mtu(MTU as u16);
    // Wintun's DLL path exists on Windows only.
    #[cfg(windows)]
    if let Some(w) = wintun {
        b = b.wintun_file(w);
    }
    #[cfg(not(windows))]
    let _ = wintun;
    let dev = Arc::new(b.build_sync()?);
    let reader = dev.clone();
    std::thread::spawn(move || {
        let mut buf = vec![0u8; 65536];
        while let Ok(n) = reader.recv(&mut buf) {
            if n > 0 && tx.send(crate::In::TunPacket(buf[..n].to_vec())).is_err() {
                break;
            }
        }
    });
    let (wtx, wrx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        for p in wrx {
            let _ = dev.send(&p);
        }
    });
    Ok(wtx)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ipv6(src: [u8; 16], dst: [u8; 16], nh: u8, body: &[u8]) -> Vec<u8> {
        let mut p = vec![0x60, 0, 0, 0];
        p.extend_from_slice(&(body.len() as u16).to_be_bytes());
        p.extend_from_slice(&[nh, 64]);
        p.extend_from_slice(&src);
        p.extend_from_slice(&dst);
        p.extend_from_slice(body);
        p
    }

    fn ping(src: [u8; 16], dst: [u8; 16], payload: &[u8]) -> Vec<u8> {
        let mut body = vec![128, 0, 0, 0, 0x12, 0x34, 0, 1];
        body.extend_from_slice(payload);
        let mut p = ipv6(src, dst, 58, &body);
        let c = l4_checksum(&p, 58);
        p[42..44].copy_from_slice(&c.to_be_bytes());
        p
    }

    fn a(n: u8) -> Address {
        Address([n; 16])
    }

    #[test]
    fn compression_roundtrips_and_saves() {
        let (s, d) = (ip_of(&a(1)), ip_of(&a(2)));
        let icmp = ping(s, d, b"hi!");
        let c = compress(&icmp, &s, &d);
        assert_eq!(c.len(), 2 + 8 + 3, "the IPv6 header becomes 2 bytes");
        assert_eq!(decompress(&c, &s, &d).unwrap(), icmp);
        // UDP: the length is derived.
        let mut udp = vec![0x1f, 0x90, 0xea, 0x60, 0, 13, 0xab, 0xcd];
        udp.extend_from_slice(b"mosh!");
        let up = ipv6(s, d, 17, &udp);
        let c = compress(&up, &s, &d);
        assert_eq!(c.len(), 2 + 6 + 5);
        assert_eq!(decompress(&c, &s, &d).unwrap(), up);
        // Other addresses: sent whole.
        let other = ping(ip_of(&a(9)), d, b"x");
        let c = compress(&other, &s, &d);
        assert_eq!(c[0], 0);
        assert_eq!(decompress(&c, &s, &d).unwrap(), other);
        // Hop limit and flow label are kept.
        let mut odd = icmp.clone();
        odd[7] = 3;
        odd[2] = 0x55;
        assert_eq!(decompress(&compress(&odd, &s, &d), &s, &d).unwrap(), odd);
    }

    #[test]
    fn mss_is_clamped_with_valid_checksum() {
        let (s, d) = (ip_of(&a(1)), ip_of(&a(2)));
        // TCP SYN with MSS 1220 and a NOP.
        let mut tcp = vec![0xc3, 0x50, 0x00, 0x16, 0, 0, 0, 1, 0, 0, 0, 0, 0x60, 0x02, 0xff, 0xff, 0, 0, 0, 0];
        tcp.extend_from_slice(&[2, 4, 0x04, 0xc4]);
        tcp[12] = 0x60; // 24 bytes header
        let mut p = ipv6(s, d, 6, &tcp);
        let c = l4_checksum(&p, 6);
        p[56..58].copy_from_slice(&c.to_be_bytes());
        assert!(clamp_mss(&mut p, TUN_MSS));
        assert_eq!(u16::from_be_bytes([p[62], p[63]]), TUN_MSS);
        assert_eq!(l4_checksum(&p, 6), 0, "the checksum is correct");
        assert!(!clamp_mss(&mut p, TUN_MSS), "already low enough");
    }

    fn rng() -> impl FnMut(&mut [u8]) {
        let mut n = 7u8;
        move |b: &mut [u8]| {
            for x in b {
                n = n.wrapping_mul(31).wrapping_add(17);
                *x = n;
            }
        }
    }

    fn deliver(from: &Address, to: &mut Tunnel, me: &Address, outs: Vec<TunOut>, drop: &[usize], now: Instant) -> Vec<TunOut> {
        let mut res = Vec::new();
        let mut i = 0;
        for o in outs {
            match o {
                TunOut::Control(_, c) => res.extend(to.on_control(*from, &c, me, now)),
                TunOut::Datagram(_, d) => {
                    if !drop.contains(&i) {
                        res.extend(to.on_datagram(&d, me, now));
                    }
                    i += 1;
                }
                other => res.push(other),
            }
        }
        res
    }

    #[test]
    fn session_small_packet_one_datagram_and_replay_is_dropped() {
        let (ma, mb) = (a(1), a(2));
        let (mut ta, mut tb) = (Tunnel::default(), Tunnel::default());
        let now = Instant::now();
        let p = ping(ip_of(&ma), ip_of(&mb), b"hi!");
        let out = ta.send(&ma, mb, &p, now, &mut rng());
        let dgs: Vec<&Vec<u8>> = out.iter().filter_map(|o| if let TunOut::Datagram(_, d) = o { Some(d) } else { None }).collect();
        assert_eq!(dgs.len(), 1);
        assert!(dgs[0].len() <= DG_OVERHEAD + 4 + 2 + 8 + 3, "ping takes {} bytes", dgs[0].len());
        let replay = dgs[0].clone();
        // The datagram arrives before the key: it is kept and delivered when the key arrives.
        let got = tb.on_datagram(&replay, &mb, now);
        assert!(got.is_empty());
        let ctrl = out.iter().find_map(|o| if let TunOut::Control(_, c) = o { Some(c.clone()) } else { None }).unwrap();
        let got = tb.on_control(ma, &ctrl, &mb, now);
        assert_eq!(got, vec![TunOut::Packet(ma, p.clone())]);
        // A replay is rejected.
        assert!(tb.on_datagram(&replay, &mb, now).is_empty());
    }

    #[test]
    fn large_packet_is_split_and_nack_recovers_lost_piece() {
        let (ma, mb) = (a(1), a(2));
        let (mut ta, mut tb) = (Tunnel::default(), Tunnel::default());
        let now = Instant::now();
        // B needs an outgoing session to be able to send a NACK.
        let warm = ping(ip_of(&mb), ip_of(&ma), b"x");
        let o = tb.send(&mb, ma, &warm, now, &mut rng());
        deliver(&mb, &mut ta, &ma, o, &[], now);
        let big = ping(ip_of(&ma), ip_of(&mb), &[0x5a; 1000]);
        let out = ta.send(&ma, mb, &big, now, &mut rng());
        assert!(out.iter().filter(|o| matches!(o, TunOut::Datagram(..))).count() >= 6);
        // Piece 2 is lost.
        assert!(deliver(&ma, &mut tb, &mb, out, &[2], now).is_empty());
        let nack = tb.tick(now + Duration::from_secs(3));
        assert_eq!(nack.len(), 1);
        // NACK to A, which sends the missing piece again.
        let resend = deliver(&mb, &mut ta, &ma, nack, &[], now);
        assert_eq!(ta.stats.resent, 1);
        let got = deliver(&ma, &mut tb, &mb, resend, &[], now);
        assert_eq!(got, vec![TunOut::Packet(ma, big)]);
    }

    #[test]
    fn answers_ping_with_valid_checksum() {
        let me = ip_of(&a(1));
        let other = ip_of(&a(2));
        let req = ping(other, me, b"hi!");
        let rep = echo_reply(&req, &me).unwrap();
        assert_eq!(rep[40], 129);
        assert_eq!(l4_checksum(&rep, 58), 0);
        assert!(echo_reply(&req, &other).is_none());
    }
}
