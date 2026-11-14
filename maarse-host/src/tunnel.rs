//! IP-tunnel: IPv6 over Må:rse, så fx SSH og mosh kan køre mellem noder.
//!
//! Hver node har en IPv6-adresse udledt af dens Må:rse-adresse:
//! `fd` + de første 15 bytes (ULA, `fd00::/8`). En IP-pakke deles i stykker,
//! der hver sendes som en krypteret direkte besked (indholdstype `0x07`):
//!
//! ```text
//! 0x07 | pakke-ID (u16) | stykke nr. (u8) | antal stykker (u8) | data
//! ```
//!
//! IPv6 kræver pakker på mindst 1280 bytes, så en pakke er op til ca. 8
//! beskeder. Det er kun realistisk over TCP; hosten sender derfor kun
//! tunneltrafik til noder, der nås over TCP hele vejen.
//!
//! `--tun` opretter en netværksadapter (Wintun på Windows, `/dev/net/tun`
//! på Linux; kræver administrator/root og featuren `tun`). Uden adapter
//! svarer hosten stadig selv på ping (ICMPv6 echo) til sin adresse.

use maarse_core::Address;
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub const IP_FRAGMENT: u8 = 0x07;
/// Data pr. stykke: plads til type og stykke-header i en FS-besked.
pub const FRAG_DATA: usize = maarse_crypto::ratchet::MAX_FS_TEXT - 1 - 4;
/// Største IP-pakke, vi sender/samler (IPv6-minimum).
pub const MTU: usize = 1280;
const REASSEMBLY_TIMEOUT: Duration = Duration::from_secs(10);

/// IPv6-adressen for en Må:rse-adresse.
pub fn ip_of(a: &Address) -> [u8; 16] {
    let mut ip = [0u8; 16];
    ip[0] = 0xfd;
    ip[1..].copy_from_slice(&a.0[..15]);
    ip
}

pub fn ip_text(ip: &[u8; 16]) -> String {
    std::net::Ipv6Addr::from(*ip).to_string()
}

/// Mål-adressen i en IPv6-pakke.
pub fn ipv6_dst(p: &[u8]) -> Option<[u8; 16]> {
    (p.len() >= 40 && p[0] >> 4 == 6).then(|| p[24..40].try_into().unwrap())
}

/// Deler en IP-pakke i beskeder (med type 0x07 foran).
pub fn fragment(packet: &[u8], id: u16) -> Vec<Vec<u8>> {
    let chunks: Vec<&[u8]> = packet.chunks(FRAG_DATA).collect();
    let n = chunks.len() as u8;
    chunks
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let mut v = Vec::with_capacity(5 + c.len());
            v.push(IP_FRAGMENT);
            v.extend_from_slice(&id.to_le_bytes());
            v.push(i as u8);
            v.push(n);
            v.extend_from_slice(c);
            v
        })
        .collect()
}

/// Samler stykker til hele pakker.
#[derive(Default)]
pub struct Reassembly {
    parts: HashMap<(Address, u16), (Instant, Vec<Option<Vec<u8>>>)>,
}

impl Reassembly {
    /// Tager imod et stykke (uden typebyte). Returnerer pakken, når den er hel.
    pub fn add(&mut self, from: Address, data: &[u8], now: Instant) -> Option<Vec<u8>> {
        self.parts.retain(|_, (t, _)| now.duration_since(*t) < REASSEMBLY_TIMEOUT);
        if data.len() < 4 {
            return None;
        }
        let id = u16::from_le_bytes([data[0], data[1]]);
        let (idx, n) = (data[2] as usize, data[3] as usize);
        if n == 0 || idx >= n || n * FRAG_DATA > MTU + FRAG_DATA || self.parts.len() > 64 {
            return None;
        }
        let e = self.parts.entry((from, id)).or_insert_with(|| (now, vec![None; n]));
        if e.1.len() != n {
            return None;
        }
        e.1[idx] = Some(data[4..].to_vec());
        if e.1.iter().all(Option::is_some) {
            let (_, parts) = self.parts.remove(&(from, id))?;
            return Some(parts.into_iter().flatten().flatten().collect());
        }
        None
    }
}

/// Svar på ping: er `p` en ICMPv6 echo request til `me`, returneres svaret.
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
    r[7] = 64; // hop limit
    r[40] = 129; // echo reply
    r[42] = 0;
    r[43] = 0;
    let c = icmpv6_checksum(&r);
    r[42..44].copy_from_slice(&c.to_be_bytes());
    Some(r)
}

fn icmpv6_checksum(p: &[u8]) -> u16 {
    let body = &p[40..];
    let mut sum: u32 = 0;
    let mut add = |b: &[u8]| {
        for c in b.chunks(2) {
            sum += u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)]) as u32;
        }
    };
    add(&p[8..40]); // kilde og mål
    add(&(body.len() as u32).to_be_bytes());
    add(&[0, 0, 0, 58]);
    add(body);
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

/// Opretter adapteren og starter læse- og skrivetråde. Pakker fra
/// adapteren sendes til noden som `In::TunPacket`; returnerer en kanal,
/// der skriver pakker til adapteren.
#[cfg(feature = "tun")]
pub fn spawn(
    me: [u8; 16],
    wintun: Option<String>,
    tx: std::sync::mpsc::Sender<crate::In>,
) -> std::io::Result<std::sync::mpsc::Sender<Vec<u8>>> {
    use std::sync::Arc;
    let mut b = tun_rs::DeviceBuilder::new()
        .name("maarse")
        .ipv6(std::net::Ipv6Addr::from(me), 8u8)
        .mtu(MTU as u16);
    if let Some(w) = wintun {
        b = b.wintun_file(w);
    }
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

    fn ping(src: [u8; 16], dst: [u8; 16], payload: &[u8]) -> Vec<u8> {
        let mut p = vec![0x60, 0, 0, 0];
        p.extend_from_slice(&((8 + payload.len()) as u16).to_be_bytes());
        p.extend_from_slice(&[58, 64]);
        p.extend_from_slice(&src);
        p.extend_from_slice(&dst);
        p.extend_from_slice(&[128, 0, 0, 0, 0x12, 0x34, 0, 1]);
        p.extend_from_slice(payload);
        let c = icmpv6_checksum(&p);
        p[42..44].copy_from_slice(&c.to_be_bytes());
        p
    }

    #[test]
    fn address_and_fragments_roundtrip() {
        let a = Address([0xab; 16]);
        let ip = ip_of(&a);
        assert_eq!(ip[0], 0xfd);
        assert!(ip_text(&ip).starts_with("fdab:"));
        let pkt: Vec<u8> = (0..1280u32).map(|i| i as u8).collect();
        let frags = fragment(&pkt, 7);
        assert!(frags.len() > 1 && frags.iter().all(|f| f.len() <= maarse_crypto::ratchet::MAX_FS_TEXT));
        let mut r = Reassembly::default();
        let now = Instant::now();
        // Stykkerne kan komme i vilkårlig rækkefølge.
        let mut out = None;
        for f in frags.iter().rev() {
            out = r.add(a, &f[1..], now).or(out);
        }
        assert_eq!(out.unwrap(), pkt);
        // Ufuldstændig pakke udløber.
        assert!(r.add(a, &frags[0][1..], now).is_none());
        assert!(r.add(a, &frags[1][1..], now + Duration::from_secs(11)).is_none());
    }

    #[test]
    fn answers_ping_with_valid_checksum() {
        let me = ip_of(&Address([1; 16]));
        let other = ip_of(&Address([2; 16]));
        let req = ping(other, me, b"hej");
        let rep = echo_reply(&req, &me).unwrap();
        assert_eq!(rep[40], 129);
        assert_eq!(&rep[8..24], &me);
        assert_eq!(&rep[24..40], &other);
        assert_eq!(icmpv6_checksum(&rep), 0, "kontrolsummen stemmer");
        assert!(echo_reply(&req, &other).is_none(), "ikke til os");
    }
}
