//! Diagnose: trace route, naboliste og støjgulv.
//!
//! ## Trace route
//!
//! En trace er en `Probe`-pakke (type 7) med kildesti, som følger den
//! kendte rute til målet. Hver repeater på vejen lægger den SNR, den hørte
//! pakken med, til bagerst i payloaden. Målet svarer ad den omvendte sti med
//! hoppenes adresse-ID'er og SNR-værdierne ud, og repeaterne på vejen tilbage
//! lægger deres SNR til på samme måde. Afsenderen ser til sidst SNR for hvert
//! spring begge veje (som MeshCores trace).
//!
//! ```text
//! payload: 0x7E | flag | fast længde (n) | fast del (n bytes) | SNR (i8) ...
//! forespørgsel (flag 0): fast del = token (16)
//! svar         (flag 1): fast del = antal hop k | k × sti-ID (2, BE)
//!                        | antal SNR ud m | m × SNR ud
//! ```
//!
//! Svaret adresseres til tokenet (som en kvittering), så målet ikke lærer,
//! hvem der spurgte. Pakke-ID'et (dublet-filter, implicitte kvitteringer)
//! regnes kun over den faste del, så det ikke ændrer sig, når repeatere
//! lægger SNR til. Ældre noder sender pakken uændret videre (de kender ikke
//! formatet); så mangler deres SNR i listen.
//!
//! Med 12 hop (største kildesti) fylder svaret 3 + 1 + 24 + 1 + 13 + 13 = 55
//! bytes; med header og sti under 100 bytes, langt under `MAX_PACKET`.

use crate::addr::{Address, PathId};
use crate::MAX_PATH;

pub const TRACE_MAGIC: u8 = 0x7E;
pub const TRACE_REQUEST: u8 = 0;
pub const TRACE_RESPONSE: u8 = 1;
/// Højst så mange SNR-værdier pr. retning (sti + modtageren).
pub const MAX_SNRS: usize = MAX_PATH + 1;

pub type TraceBuf = heapless::Vec<u8, 96>;

/// Længden af den del af en trace-payload, som pakke-ID'et regnes over.
/// `None`, hvis det ikke er en trace.
pub fn trace_fixed_len(payload: &[u8]) -> Option<usize> {
    if payload.len() < 3 || payload[0] != TRACE_MAGIC {
        return None;
    }
    let n = 3 + payload[2] as usize;
    (n <= payload.len()).then_some(n)
}

/// Bygger en forespørgsel.
pub fn trace_request(token: &Address) -> TraceBuf {
    let mut v = TraceBuf::new();
    let _ = v.extend_from_slice(&[TRACE_MAGIC, TRACE_REQUEST, 16]);
    let _ = v.extend_from_slice(&token.0);
    v
}

/// Lægger en SNR-værdi til (repeater eller modtager). Fuld: uændret.
pub fn trace_append(payload: &mut TraceBuf, snr: i8) {
    if let Some(n) = trace_fixed_len(payload)
        && payload.len() - n < MAX_SNRS
    {
        let _ = payload.push(snr as u8);
    }
}

/// En afkodet forespørgsel: token og SNR'er indtil nu.
pub fn parse_request(payload: &[u8]) -> Option<(Address, &[u8])> {
    let n = trace_fixed_len(payload)?;
    if payload[1] != TRACE_REQUEST || n != 19 {
        return None;
    }
    Some((Address::from_slice(&payload[3..19])?, &payload[19..]))
}

/// Svaret: sti (ID'er), SNR ud (inkl. målets egen) og SNR tilbage.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TraceResult {
    pub path: heapless::Vec<PathId, MAX_PATH>,
    pub out: heapless::Vec<i8, MAX_SNRS>,
    pub back: heapless::Vec<i8, MAX_SNRS>,
}

/// Bygger målets svar ud fra stien og forespørgslens SNR'er (inkl. målets).
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

// ---------- naboer ----------

/// En nabo, vi har hørt direkte (uden mellemled).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Neighbor {
    pub id: PathId,
    /// Fuld adresse, hvis vi har hørt naboens eget announce direkte.
    pub addr: Option<Address>,
    pub snr: i8,
    /// RSSI i dBm (0 = ukendt, fx på en host uden radio).
    pub rssi: i16,
    pub last_ms: u64,
    pub count: u32,
}

/// Tabel over direkte naboer. Fuld: den længst ikke hørte erstattes.
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
        let n = Neighbor { id, addr, snr, rssi, last_ms: now_ms, count: 1 };
        if self.items.is_full()
            && let Some(i) = self.items.iter().enumerate().min_by_key(|(_, n)| n.last_ms).map(|(i, _)| i)
        {
            self.items.swap_remove(i);
        }
        let _ = self.items.push(n);
    }

    /// Sorteret efter senest hørt først.
    pub fn sorted(&self) -> heapless::Vec<Neighbor, N> {
        let mut v = self.items.clone();
        v.sort_unstable_by_key(|n| core::cmp::Reverse(n.last_ms));
        v
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

// ---------- støjgulv ----------

/// 10·log10(1 + 10^(s/10)) i tiendedele dB for s = -10..=10.
const SUM_DB_X10: [i16; 21] = [
    4, 5, 6, 8, 10, 12, 15, 18, 21, 25, 30, 35, 41, 48, 55, 62, 70, 78, 86, 95, 104,
];

/// Anslår støjgulvet (dBm) ud fra en modtaget pakkes RSSI og SNR.
///
/// lora-phy giver ikke adgang til den øjeblikkelige RSSI, mens radioen
/// lytter. Pakkens RSSI er signal + støj, og SNR er signal / støj, så
/// støjen er N = RSSI − 10·log10(1 + 10^(SNR/10)).
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
        assert_eq!(trace_fixed_len(&req), Some(fixed), "fast del ændres ikke");
        let (t, snrs) = parse_request(&req).unwrap();
        assert_eq!(t, token);
        assert_eq!(snrs, &[5u8, (-12i8) as u8][..]);

        let mut out: heapless::Vec<u8, 16> = heapless::Vec::from_slice(snrs).unwrap();
        out.push(3).unwrap(); // målets egen
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
        // Fuld: nr. 2 (ældst) erstattes.
        t.heard(3, None, 0, -110, 40);
        assert_eq!(t.len(), 2);
        assert!(t.sorted().iter().all(|n| n.id != 2));
    }

    #[test]
    fn noise() {
        // SNR 0: signal = støj, RSSI er 3 dB over støjen.
        assert_eq!(noise_floor_dbm(-117, 0), -120);
        assert_eq!(noise_floor_dbm(-100, 20), -120);
        assert_eq!(noise_floor_dbm(-120, -15), -120);
        assert_eq!(noise_floor_dbm(-110, 10), -120);
    }
}
