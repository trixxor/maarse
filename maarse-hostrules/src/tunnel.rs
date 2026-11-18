//! The IP tunnel's pure rules: IPv6 addresses from Må:rse addresses, header
//! compression, MSS clamping, ping replies, pieces, NACK bitmasks, the
//! replay window and the timings. Sessions, encryption and the TUN adapter
//! stay in `maarse-host`.
//!
//! ```text
//! Content:     1 | packet ID | piece no. | count | data      (piece)
//!              2 | packet ID | missing pieces (u16, bitmask) (NACK)
//! Compressed:  0x80|flag | next header | [hop limit] | [traffic class+flow (4)] | ...
//!              UDP: source port | dest port | checksum | data (length is derived)
//!              0x00 | the whole IPv6 packet (when it cannot be compressed)
//! ```

use maarse_core::Address;

/// Largest IP packet (the IPv6 minimum).
pub const MTU: usize = 1280;
/// Overhead in a datagram: session ID, counter and MAC.
pub const DG_OVERHEAD: usize = 8 + 4 + 16;
/// Data per piece in a datagram.
pub const FRAG_DATA: usize = maarse_core::router::MAX_DIRECT_PAYLOAD - DG_OVERHEAD - 4;
/// MSS for TCP through the tunnel: one segment per piece (the IPv6 header is
/// compressed; the TCP header with options is up to about 32 bytes).
pub const TUN_MSS: u16 = (FRAG_DATA - 7 - 32) as u16;
/// Most pieces per packet (the NACK bitmask is 16 bits).
pub const MAX_PIECES: usize = 16;
/// A new session key after this time.
pub const REKEY_AFTER_MS: u64 = 300_000;
/// First NACK for a packet with missing pieces; later ones after 2×, 3×.
pub const NACK_AFTER_MS: u64 = 2_000;
/// At most this many NACKs per packet.
pub const MAX_NACKS: u8 = 3;
/// An incomplete packet is dropped after this time.
pub const GIVE_UP_MS: u64 = 12_000;
/// Sent pieces are kept this long for NACKs.
pub const KEEP_SENT_MS: u64 = 15_000;
/// Datagrams for an unknown session wait this long for the key.
pub const EARLY_KEEP_MS: u64 = 10_000;
/// At most this many such datagrams wait.
pub const EARLY_MAX: usize = 32;

/// Content type of a piece and of a NACK inside a datagram.
pub const PIECE: u8 = 1;
pub const NACK: u8 = 2;

/// The IPv6 address for a Må:rse address.
pub fn ip_of(a: &Address) -> [u8; 16] {
    let mut ip = [0u8; 16];
    ip[0] = 0xfd;
    ip[1..].copy_from_slice(&a.0[..15]);
    ip
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
pub fn l4_checksum(p: &[u8], next_header: u8) -> u16 {
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

// ---------- pieces and NACKs ----------

/// Splits a compressed packet into piece contents (`1 | pkt | i | n | data`).
/// `None` when it needs more than [`MAX_PIECES`] (larger than the MTU).
pub fn pieces(c: &[u8], pkt: u8) -> Option<Vec<Vec<u8>>> {
    let chunks: Vec<&[u8]> = c.chunks(FRAG_DATA).collect();
    if chunks.len() > MAX_PIECES {
        return None;
    }
    Some(
        chunks
            .iter()
            .enumerate()
            .map(|(i, ch)| {
                let mut plain = vec![PIECE, pkt, i as u8, chunks.len() as u8];
                plain.extend_from_slice(ch);
                plain
            })
            .collect(),
    )
}

/// The NACK content asking for the pieces in `mask` of packet `pkt`.
pub fn nack(pkt: u8, mask: u16) -> Vec<u8> {
    let mut plain = vec![NACK, pkt];
    plain.extend_from_slice(&mask.to_be_bytes());
    plain
}

/// The bitmask of the pieces still missing.
pub fn missing_mask<T>(parts: &[Option<T>]) -> u16 {
    parts.iter().enumerate().filter(|(_, x)| x.is_none()).fold(0u16, |m, (i, _)| m | 1 << i)
}

/// The pieces a NACK with `mask` asks for, in order.
pub fn asked_pieces<T>(sent: &[T], mask: u16) -> impl Iterator<Item = &T> {
    sent.iter().enumerate().filter(move |(i, _)| mask & (1 << i) != 0).map(|(_, f)| f)
}

/// Is a NACK due for a packet started `age_ms` ago that has had `nacked` NACKs?
pub fn nack_due(nacked: u8, age_ms: u64) -> bool {
    nacked < MAX_NACKS && age_ms >= NACK_AFTER_MS * (nacked as u64 + 1)
}

/// The receiver's replay window: the highest counter and a bitmask over the
/// 64 before it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReplayWindow {
    pub max: u32,
    pub seen: u64,
}

impl ReplayWindow {
    /// Takes `ctr` if it is new; false for a replay or one too old.
    pub fn accept(&mut self, ctr: u32) -> bool {
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
    fn address_is_ula_from_the_maarse_address() {
        let mut b = [0u8; 16];
        for (i, x) in b.iter_mut().enumerate() {
            *x = i as u8 + 1;
        }
        let ip = ip_of(&Address(b));
        assert_eq!(ip[0], 0xfd);
        assert_eq!(&ip[1..], &b[..15]);
        let p = ping(ip_of(&a(1)), ip, b"x");
        assert_eq!(ipv6_dst(&p), Some(ip));
        assert_eq!(ipv6_dst(&p[..39]), None);
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

    #[test]
    fn pieces_and_nack_masks() {
        let c = vec![0x5a; FRAG_DATA * 2 + 1];
        let p = pieces(&c, 9).unwrap();
        assert_eq!(p.len(), 3);
        assert_eq!(&p[1][..4], &[PIECE, 9, 1, 3]);
        assert_eq!(p.iter().map(|x| x.len() - 4).sum::<usize>(), c.len());
        assert!(pieces(&vec![0; FRAG_DATA * MAX_PIECES + 1], 1).is_none());
        let parts = [Some(1), None, Some(3), None];
        let mask = missing_mask(&parts);
        assert_eq!(mask, 0b1010);
        assert_eq!(nack(9, mask), vec![NACK, 9, 0, 0b1010]);
        let sent = ["a", "b", "c", "d"];
        assert_eq!(asked_pieces(&sent, mask).copied().collect::<Vec<_>>(), vec!["b", "d"]);
    }

    #[test]
    fn nack_timing() {
        assert!(!nack_due(0, NACK_AFTER_MS - 1));
        assert!(nack_due(0, NACK_AFTER_MS));
        assert!(!nack_due(1, NACK_AFTER_MS * 2 - 1));
        assert!(nack_due(2, NACK_AFTER_MS * 3));
        assert!(!nack_due(MAX_NACKS, GIVE_UP_MS), "at most three");
        assert_eq!((REKEY_AFTER_MS, GIVE_UP_MS), (300_000, 12_000));
    }

    #[test]
    fn replay_window() {
        let mut w = ReplayWindow::default();
        assert!(w.accept(1));
        assert!(!w.accept(1), "replay");
        assert!(w.accept(5));
        assert!(w.accept(3), "late but new");
        assert!(!w.accept(3));
        assert!(w.accept(100));
        assert!(!w.accept(30), "too old");
    }
}
