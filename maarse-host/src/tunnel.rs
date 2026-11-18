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

// The pure rules (addresses, header compression, MSS clamping, ping
// replies, pieces, NACKs, replay window, timings) live in maarse-hostrules.
pub use maarse_hostrules::tunnel::{DG_OVERHEAD, MTU, TUN_MSS, clamp_mss, compress, decompress, echo_reply, ip_of, ipv6_dst};
use maarse_hostrules::tunnel as rules;
const REKEY_AFTER: Duration = Duration::from_millis(rules::REKEY_AFTER_MS);
const GIVE_UP: Duration = Duration::from_millis(rules::GIVE_UP_MS);
const KEEP_SENT: Duration = Duration::from_millis(rules::KEEP_SENT_MS);

/// Control message (sent sealed as content type `0x07`): new session key.
pub const CTRL_KEY: u8 = 1;

pub fn ip_text(ip: &[u8; 16]) -> String {
    std::net::Ipv6Addr::from(*ip).to_string()
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
    /// Against replays.
    window: rules::ReplayWindow,
    created: Instant,
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
        let Some(frags) = rules::pieces(&c, pkt) else {
            return out; // larger than the MTU; should not happen
        };
        for plain in &frags {
            s.ctr += 1;
            out.push(TunOut::Datagram(dest, seal(&s.key, &s.id, s.ctr, plain)));
        }
        self.stats.sent_packets += 1;
        self.stats.sent_datagrams += frags.len() as u64;
        if frags.len() > 1 {
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
        self.inn.insert(id, InSession { peer: from, key, window: rules::ReplayWindow::default(), created: now });
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
            self.early.retain(|(t, _)| now.duration_since(*t) < Duration::from_millis(rules::EARLY_KEEP_MS));
            if self.early.len() < rules::EARLY_MAX {
                self.early.push_back((now, dg.to_vec()));
            }
            return out;
        };
        let Some((ctr, plain)) = open(&s.key, dg) else { return out };
        if !s.window.accept(ctr) {
            return out;
        }
        let peer = s.peer;
        match plain.first() {
            Some(&rules::PIECE) if plain.len() >= 4 => {
                let (pkt, idx, n) = (plain[1], plain[2] as usize, plain[3] as usize);
                if n == 0 || idx >= n || n > rules::MAX_PIECES {
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
            Some(&rules::NACK) if plain.len() >= 4 => {
                let (pkt, mask) = (plain[1], u16::from_be_bytes([plain[2], plain[3]]));
                if let Some((_, frags)) = self.sent.get(&(peer, pkt)) {
                    let frags: Vec<Vec<u8>> = rules::asked_pieces(frags, mask).cloned().collect();
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
            if rules::nack_due(p.nacked, now.duration_since(p.started).as_millis() as u64) {
                p.nacked += 1;
                let mask = rules::missing_mask(&p.parts);
                nacks.push((*peer, *pkt, mask));
            }
        }
        for (peer, pkt, mask) in nacks {
            if let Some(o) = self.out.get_mut(&peer) {
                let plain = rules::nack(pkt, mask);
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
        let c = maarse_hostrules::tunnel::l4_checksum(&p, 58);
        p[42..44].copy_from_slice(&c.to_be_bytes());
        p
    }

    fn a(n: u8) -> Address {
        Address([n; 16])
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
}
