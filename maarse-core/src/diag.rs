//! Diagnostics: trace route, neighbour list and noise floor.
//!
//! ## Trace route
//!
//! A trace is a `Probe` packet (type 7) with a source path that follows the
//! known route to the target. Every repeater on the way appends the SNR it heard
//! the packet with to the end of the payload. The target answers along the reverse
//! path with the hops' address IDs and the outbound SNR values, and the repeaters on
//! the way back add their SNR the same way. In the end the sender sees the SNR for
//! every hop both ways (like MeshCore's trace).
//!
//! ```text
//! payload: 0x7E | flags | fixed length (n) | fixed part (n bytes) | SNR (i8) ...
//! request (flags 0): fixed part = token (16)
//! reply   (flags 1): fixed part = hop count k | k × path ID (2, BE)
//!                        | outbound SNR count m | m × outbound SNR
//! ```
//!
//! The reply is addressed to the token (like an acknowledgement), so the target
//! does not learn who asked. The packet ID (duplicate filter, implicit acks)
//! is only computed over the fixed part, so it does not change when repeaters
//! add SNR. Older nodes forward the packet unchanged (they do not know the
//! format); their SNR is then missing from the list.
//!
//! With 12 hops (the largest source path) the reply takes 3 + 1 + 24 + 1 + 13 + 13 = 55
//! bytes; with header and path below 100 bytes, far below `MAX_PACKET`.

use crate::addr::{Address, PathId};
use crate::MAX_PATH;

pub const TRACE_MAGIC: u8 = 0x7E;
pub const TRACE_REQUEST: u8 = 0;
pub const TRACE_RESPONSE: u8 = 1;
/// At most this many SNR values per direction (path + the recipient).
pub const MAX_SNRS: usize = MAX_PATH + 1;

pub type TraceBuf = heapless::Vec<u8, 96>;

/// The length of the part of a trace payload that the packet ID is computed over.
/// `None` if it is not a trace.
pub fn trace_fixed_len(payload: &[u8]) -> Option<usize> {
    if payload.len() < 3 || payload[0] != TRACE_MAGIC {
        return None;
    }
    let n = 3 + payload[2] as usize;
    (n <= payload.len()).then_some(n)
}

/// Builds a request.
pub fn trace_request(token: &Address) -> TraceBuf {
    let mut v = TraceBuf::new();
    let _ = v.extend_from_slice(&[TRACE_MAGIC, TRACE_REQUEST, 16]);
    let _ = v.extend_from_slice(&token.0);
    v
}

/// Appends an SNR value (repeater or recipient). Full: unchanged.
pub fn trace_append(payload: &mut TraceBuf, snr: i8) {
    if let Some(n) = trace_fixed_len(payload)
        && payload.len() - n < MAX_SNRS
    {
        let _ = payload.push(snr as u8);
    }
}

/// A decoded request: token and SNRs so far.
pub fn parse_request(payload: &[u8]) -> Option<(Address, &[u8])> {
    let n = trace_fixed_len(payload)?;
    if payload[1] != TRACE_REQUEST || n != 19 {
        return None;
    }
    Some((Address::from_slice(&payload[3..19])?, &payload[19..]))
}

/// The reply: path (IDs), outbound SNR (incl. the target's own) and SNR back.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TraceResult {
    pub path: heapless::Vec<PathId, MAX_PATH>,
    pub out: heapless::Vec<i8, MAX_SNRS>,
    pub back: heapless::Vec<i8, MAX_SNRS>,
}

/// Builds the target's reply from the path and the request's SNRs (incl. the target's).
pub fn trace_response(path: &[PathId], snrs_out: &[u8]) -> TraceBuf {
    let mut v = TraceBuf::new();
    let k = path.len().min(MAX_PATH);
    let m = snrs_out.len().min(MAX_SNRS);
    let _ = v.extend_from_slice(&[TRACE_MAGIC, TRACE_RESPONSE, (1 + 2 * k + 1 + m) as u8, k as u8]);
    for id in &path[..k] {
        let _ = v.extend_from_slice(&id.to_be_bytes());
    }
    let _ = v.push(m as u8);
    let _ = v.extend_from_slice(&snrs_out[..m]);
    v
}

pub fn parse_response(payload: &[u8]) -> Option<TraceResult> {
    let n = trace_fixed_len(payload)?;
    if payload[1] != TRACE_RESPONSE {
        return None;
    }
    let f = &payload[3..n];
    let k = *f.first()? as usize;
    if k > MAX_PATH {
        return None;
    }
    let ids = f.get(1..1 + 2 * k)?;
    let m = *f.get(1 + 2 * k)? as usize;
    let out = f.get(2 + 2 * k..2 + 2 * k + m)?;
    let mut r = TraceResult::default();
    for c in ids.chunks_exact(2) {
        let _ = r.path.push(u16::from_be_bytes([c[0], c[1]]));
    }
    r.out = out.iter().take(MAX_SNRS).map(|&b| b as i8).collect();
    r.back = payload[n..].iter().take(MAX_SNRS).map(|&b| b as i8).collect();
    Some(r)
}

// ---------- neighbours ----------

/// A neighbour we have heard directly (with no one in between).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Neighbor {
    pub id: PathId,
    /// Full address, if we have heard the neighbour's own announce directly.
    pub addr: Option<Address>,
    pub snr: i8,
    /// RSSI in dBm (0 = unknown, e.g. on a host without a radio).
    pub rssi: i16,
    pub last_ms: u64,
    pub count: u32,
    /// Role from the neighbor's own announce (role TLV), if heard.
    pub role: Option<crate::router::Role>,
    /// Last time we heard this neighbor forward a flood (0 = never).
    pub forwarded_ms: u64,
    /// Last time we heard a repeater re-forward a flood this neighbor
    /// forwarded, i.e. a repeater can hear it (0 = never).
    pub covered_ms: u64,
    /// The neighbor's own report (role field flag) of whether it hears a
    /// repeater directly. `None` if it has not said (older firmware).
    pub hears_repeater: Option<bool>,
    /// The neighbor's report of how many edge bridges it hears (role flag
    /// bits 1-2), `None` if it has not said.
    pub bridges: Option<u8>,
    /// The edge bridge the neighbor has chosen (role field), if any: a
    /// bridge stays while a neighbor names it, as OLSR multipoint relays
    /// stay while their selectors name them.
    pub bridge_choice: Option<PathId>,
    /// Post office class from the neighbor's hello (0 = none).
    pub mail_class: u8,
}

/// Table of direct neighbours. Full: the one not heard for the longest is replaced.
pub struct NeighborTable<const N: usize> {
    items: heapless::Vec<Neighbor, N>,
}

impl<const N: usize> Default for NeighborTable<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> NeighborTable<N> {
    pub const fn new() -> Self {
        NeighborTable { items: heapless::Vec::new() }
    }

    pub fn heard(&mut self, id: PathId, addr: Option<Address>, snr: i8, rssi: i16, now_ms: u64) {
        if let Some(n) = self.items.iter_mut().find(|n| n.id == id) {
            n.snr = snr;
            n.rssi = rssi;
            n.last_ms = now_ms;
            n.count = n.count.saturating_add(1);
            if addr.is_some() {
                n.addr = addr;
            }
            return;
        }
        let n = Neighbor { id, addr, snr, rssi, last_ms: now_ms, count: 1, role: None, forwarded_ms: 0, covered_ms: 0, hears_repeater: None, bridges: None, bridge_choice: None, mail_class: 0 };
        // Full: drop the neighbor heard longest ago, but keep repeaters and
        // anchors (the edge bridge rule needs them) while others can go.
        let infra = |n: &Neighbor| matches!(n.role, Some(crate::router::Role::Repeater | crate::router::Role::Anchor));
        if self.items.is_full()
            && let Some(i) = self
                .items
                .iter()
                .enumerate()
                .min_by_key(|(_, n)| (infra(n), n.last_ms))
                .map(|(i, _)| i)
        {
            self.items.swap_remove(i);
        }
        let _ = self.items.push(n);
    }

    /// Records a neighbor's role and, if known, whether it hears a
    /// repeater (from its announce).
    pub fn set_role(&mut self, id: PathId, role: crate::router::Role, hears_repeater: Option<bool>) {
        if let Some(n) = self.items.iter_mut().find(|n| n.id == id) {
            n.role = Some(role);
            if hears_repeater.is_some() {
                n.hears_repeater = hears_repeater;
            }
        }
    }

    /// How many edge bridges the neighbor says it hears.
    pub fn set_bridges(&mut self, id: PathId, bridges: u8) {
        if let Some(n) = self.items.iter_mut().find(|n| n.id == id) {
            n.bridges = Some(bridges);
        }
    }

    /// The neighbor's post office class (from its hello).
    pub fn set_mail_class(&mut self, id: PathId, class: u8) {
        if let Some(n) = self.items.iter_mut().find(|n| n.id == id) {
            n.mail_class = class;
        }
    }

    /// The edge bridge the neighbor says it has chosen (`None`: none).
    pub fn set_bridge_choice(&mut self, id: PathId, choice: Option<PathId>) {
        if let Some(n) = self.items.iter_mut().find(|n| n.id == id) {
            n.bridge_choice = choice;
        }
    }

    pub fn get(&self, id: PathId) -> Option<&Neighbor> {
        self.items.iter().find(|n| n.id == id)
    }

    /// The neighbor forwarded a flood we heard.
    pub fn mark_forwarded(&mut self, id: PathId, now_ms: u64) {
        if let Some(n) = self.items.iter_mut().find(|n| n.id == id) {
            n.forwarded_ms = now_ms;
        }
    }

    /// A repeater re-forwarded a flood from this neighbor.
    pub fn mark_covered(&mut self, id: PathId, now_ms: u64) {
        if let Some(n) = self.items.iter_mut().find(|n| n.id == id) {
            n.covered_ms = now_ms;
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &Neighbor> {
        self.items.iter()
    }

    /// Sorted by most recently heard first.
    pub fn sorted(&self) -> heapless::Vec<Neighbor, N> {
        let mut v = self.items.clone();
        crate::sort::sort_by(&mut v, |a, b| a.last_ms > b.last_ms);
        v
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

// ---------- noise floor ----------

/// 10·log10(1 + 10^(s/10)) i tiendedele dB for s = -10..=10.
const SUM_DB_X10: [i16; 21] = [
    4, 5, 6, 8, 10, 12, 15, 18, 21, 25, 30, 35, 41, 48, 55, 62, 70, 78, 86, 95, 104,
];

/// Estimates the noise floor (dBm) from a received packet's RSSI and SNR.
///
/// lora-phy gives no access to the instantaneous RSSI while the radio
/// listens. The packet's RSSI is signal + noise, and SNR is signal / noise, so
/// the noise is N = RSSI − 10·log10(1 + 10^(SNR/10)).
pub fn noise_floor_dbm(rssi: i16, snr: i8) -> i16 {
    let s = snr as i16;
    let x10 = if s < -10 {
        0
    } else if s > 10 {
        s * 10
    } else {
        SUM_DB_X10[(s + 10) as usize]
    };
    rssi - (x10 + 5) / 10
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_roundtrip() {
        let token = Address([7; 16]);
        let mut req = trace_request(&token);
        let fixed = trace_fixed_len(&req).unwrap();
        trace_append(&mut req, 5);
        trace_append(&mut req, -12);
        assert_eq!(trace_fixed_len(&req), Some(fixed), "fixed part does not change");
        let (t, snrs) = parse_request(&req).unwrap();
        assert_eq!(t, token);
        assert_eq!(snrs, &[5u8, (-12i8) as u8][..]);

        let mut out: heapless::Vec<u8, 16> = heapless::Vec::from_slice(snrs).unwrap();
        out.push(3).unwrap(); // the target's own
        let mut resp = trace_response(&[0x1111, 0x2222], &out);
        trace_append(&mut resp, -1);
        trace_append(&mut resp, 2);
        trace_append(&mut resp, 9);
        let r = parse_response(&resp).unwrap();
        assert_eq!(&r.path[..], &[0x1111, 0x2222]);
        assert_eq!(&r.out[..], &[5, -12, 3]);
        assert_eq!(&r.back[..], &[-1, 2, 9]);
        assert!(parse_request(&resp).is_none());
        assert!(parse_response(&req).is_none());
    }

    #[test]
    fn trace_fits_and_caps() {
        let path: heapless::Vec<u16, 12> = (0..12).collect();
        let mut resp = trace_response(&path, &[1; 13]);
        for _ in 0..20 {
            trace_append(&mut resp, 1);
        }
        let r = parse_response(&resp).unwrap();
        assert_eq!(r.back.len(), MAX_SNRS);
        assert!(resp.len() <= 55);
        assert!(trace_fixed_len(&[0x7E, 0, 40]).is_none());
        assert!(trace_fixed_len(b"hej").is_none());
    }

    #[test]
    fn neighbors() {
        let mut t: NeighborTable<2> = NeighborTable::new();
        t.heard(1, None, 5, -90, 10);
        t.heard(2, None, 3, -100, 20);
        t.heard(1, Some(Address([1; 16])), 6, -88, 30);
        let s = t.sorted();
        assert_eq!(s[0].id, 1);
        assert_eq!(s[0].count, 2);
        assert_eq!(s[0].addr, Some(Address([1; 16])));
        // Full: no. 2 (oldest) is replaced.
        t.heard(3, None, 0, -110, 40);
        assert_eq!(t.len(), 2);
        assert!(t.sorted().iter().all(|n| n.id != 2));
    }

    #[test]
    fn noise() {
        // SNR 0: signal = noise, RSSI is 3 dB above the noise.
        assert_eq!(noise_floor_dbm(-117, 0), -120);
        assert_eq!(noise_floor_dbm(-100, 20), -120);
        assert_eq!(noise_floor_dbm(-120, -15), -120);
        assert_eq!(noise_floor_dbm(-110, 10), -120);
    }
}
