//! Arbejdsbevis ("stempel") på announces og ruteforespørgsler.
//!
//! Inspireret af Reticulum 1.6. Et stempel er en nonce, så
//!
//! ```text
//! SHA-256("maarse/stamp" | version | pakketype | destination | binding | nonce)
//! ```
//!
//! har mindst `D` foranstillede nul-bits. Hashinputtet er højst 50 bytes, så
//! det passer i én SHA-256-blok: verifikation koster én blok-kompression.
//! At lave et stempel koster i gennemsnit `2^D` forsøg.
//!
//! Stemplet ligger sidst i payloaden og er [`STAMP_LEN`] bytes:
//!
//! ```text
//! ... payload ... | version (1) | nonce (4, big-endian)
//! ```
//!
//! Binding (det stemplet hænger fast på):
//! * `Announce`: destinationen er identitetens adresse, og bindingen er
//!   tidsstemplet (payload-bytes 64..68). Et stempel kan derfor regnes ud,
//!   før app-data kendes, men kun genbruges til samme identitet og tid.
//! * `PathRequest`: destinationen (målet) og svar-tokenet (payloadens
//!   første 16 bytes, tilfældigt pr. forespørgsel). Afsenderen står ikke i
//!   pakken, så den kan ikke bindes med.
//!
//! Sværhedsgraden er en konstant pr. pakketype og stempel-version. Versionen
//! står i stemplet, så den kan hæves senere: en ny version får en højere
//! tabelværdi, og gamle versioner kan fjernes fra [`difficulty`].
//!
//! ## Sværhedsgrad
//!
//! Målt på en PC (x86-64, én kerne, release, `sha2` med SHA-NI) ca. 21
//! mio. forsøg/s: et announce-stempel tager ca. 0,2 ms og en
//! ruteforespørgsel ca. 0,01 ms (testen `stamp_benchmark`, kør med
//! `cargo test --release -p maarse-core stamp_benchmark -- --ignored --nocapture`). På en 64 MHz
//! Cortex-M4 (nRF52840) koster én SHA-256-blok i software ca. 4.000–5.000
//! cykler, dvs. ca. 13.000–16.000 forsøg/s.
//!
//! * `Announce`: 12 bit = 4.096 forsøg i snit ≈ 0,3 s på en nRF52840
//!   (ESP32-S3 ved 240 MHz er hurtigere). Announces sendes hvert 15.–30.
//!   minut, så det er gratis for en ærlig node, men en angriber på samme
//!   hardware kan højst lave ca. 3 falske identiteter i sekundet.
//! * `PathRequest`: 8 bit = 256 forsøg i snit ≈ 20 ms på en nRF52840. De
//!   laves oftere (hver ny modtager, gentagelser efter 15 s) og regnes ud i
//!   routeren uden at give slip på executoren, så de skal være billige.
//!   Sandsynligheden for mere end 4.096 forsøg er e^-16, så den værste
//!   ventetid er i praksis under ca. 0,3 s.
//!
//! En PC er over 1000 gange hurtigere end en mikrocontroller, så stemplet
//! stopper ikke en målrettet angriber med en stærk maskine. Det gør spam
//! dyrt nok til, at billige noder ikke kan oversvømme nettet, og LoRa's
//! sendetidsgrænse tager resten.

use crate::addr::Address;
use crate::packet::PacketType;
use sha2::{Digest, Sha256};

/// Nuværende stempel-version.
pub const STAMP_VERSION: u8 = 1;
/// Stemplets længde: version (1) + nonce (4).
pub const STAMP_LEN: usize = 5;
/// Sværhedsgrad (nul-bits) for announces i version 1.
pub const ANNOUNCE_BITS: u32 = 12;
/// Sværhedsgrad (nul-bits) for ruteforespørgsler i version 1.
pub const PATH_REQUEST_BITS: u32 = 8;

const DOMAIN: &[u8] = b"maarse/stamp";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StampError {
    /// Payloaden er for kort til stempel og binding.
    Missing,
    /// Ukendt (eller udfaset) stempel-version.
    UnknownVersion(u8),
    /// Hashen har for få nul-bits.
    TooWeak,
}

/// Kræver pakketypen et stempel?
pub fn required(ptype: PacketType) -> bool {
    matches!(ptype, PacketType::Announce | PacketType::PathRequest)
}

/// Krævede nul-bits for `ptype` i stempel-version `version`.
pub fn difficulty(version: u8, ptype: PacketType) -> Option<u32> {
    match (version, ptype) {
        (1, PacketType::Announce) => Some(ANNOUNCE_BITS),
        (1, PacketType::PathRequest) => Some(PATH_REQUEST_BITS),
        _ => None,
    }
}

/// De payload-bytes (uden stempel), som stemplet bindes til.
fn binding(ptype: PacketType, body: &[u8]) -> Option<&[u8]> {
    match ptype {
        PacketType::Announce => body.get(64..68),
        PacketType::PathRequest => body.get(..16),
        _ => None,
    }
}

fn prefix(version: u8, ptype: PacketType, dest: &Address, binding: &[u8]) -> Sha256 {
    let mut h = Sha256::new();
    h.update(DOMAIN);
    h.update([version, ptype as u8]);
    h.update(dest.0);
    h.update(binding);
    h
}

/// Antal foranstillede nul-bits.
pub fn leading_zero_bits(hash: &[u8]) -> u32 {
    let mut n = 0;
    for b in hash {
        if *b == 0 {
            n += 8;
        } else {
            return n + b.leading_zeros();
        }
    }
    n
}

/// Deler en payload i (indhold, stempel). `None`, hvis den er for kort.
pub fn split(payload: &[u8]) -> Option<(&[u8], &[u8])> {
    let n = payload.len().checked_sub(STAMP_LEN)?;
    Some(payload.split_at(n))
}

/// Payloaden uden stempel. Er pakken ikke stemplet (eller for kort),
/// returneres den uændret.
pub fn strip(ptype: PacketType, payload: &[u8]) -> &[u8] {
    if required(ptype) {
        split(payload).map_or(payload, |(b, _)| b)
    } else {
        payload
    }
}

/// Tjekker stemplet og returnerer payloaden uden stempel. Koster én hash.
pub fn verify<'a>(
    ptype: PacketType,
    dest: &Address,
    payload: &'a [u8],
) -> Result<&'a [u8], StampError> {
    let (body, stamp) = split(payload).ok_or(StampError::Missing)?;
    let bind = binding(ptype, body).ok_or(StampError::Missing)?;
    let version = stamp[0];
    let bits = difficulty(version, ptype).ok_or(StampError::UnknownVersion(version))?;
    let mut h = prefix(version, ptype, dest, bind);
    h.update(&stamp[1..]);
    if leading_zero_bits(&h.finalize()) >= bits {
        Ok(body)
    } else {
        Err(StampError::TooWeak)
    }
}

/// Søgning efter et stempel, der kan deles op i bidder, så en firmware kan
/// give executoren fri ind imellem (se [`StampSearch::step`]).
#[derive(Clone)]
pub struct StampSearch {
    prefix: Sha256,
    bits: u32,
    nonce: u32,
}

impl StampSearch {
    /// Starter en søgning for en pakke med indholdet `body` (uden stempel).
    /// `None`, hvis pakketypen ikke stemples, eller `body` er for kort.
    pub fn new(ptype: PacketType, dest: &Address, body: &[u8]) -> Option<Self> {
        Self::with_bits(ptype, dest, body, difficulty(STAMP_VERSION, ptype)?)
    }

    /// Som [`StampSearch::new`], men med en valgfri sværhedsgrad (til test).
    pub fn with_bits(ptype: PacketType, dest: &Address, body: &[u8], bits: u32) -> Option<Self> {
        let bind = binding(ptype, body)?;
        Some(StampSearch {
            prefix: prefix(STAMP_VERSION, ptype, dest, bind),
            bits,
            nonce: 0,
        })
    }

    /// Prøver højst `n` nonces. Returnerer stemplet, når det er fundet.
    pub fn step(&mut self, n: u32) -> Option<[u8; STAMP_LEN]> {
        for _ in 0..n {
            let nonce = self.nonce.to_be_bytes();
            self.nonce = self.nonce.wrapping_add(1);
            let mut h = self.prefix.clone();
            h.update(nonce);
            if leading_zero_bits(&h.finalize()) >= self.bits {
                let mut s = [0u8; STAMP_LEN];
                s[0] = STAMP_VERSION;
                s[1..].copy_from_slice(&nonce);
                return Some(s);
            }
        }
        None
    }

    /// Antal forsøg indtil nu.
    pub fn tries(&self) -> u32 {
        self.nonce
    }
}

/// Regner et stempel ud i ét stræk (blokerer, til host og router).
pub fn compute(ptype: PacketType, dest: &Address, body: &[u8]) -> Option<[u8; STAMP_LEN]> {
    let mut s = StampSearch::new(ptype, dest, body)?;
    loop {
        if let Some(st) = s.step(u32::MAX) {
            return Some(st);
        }
    }
}

/// Lægger et stempel til sidst i `payload`. `false`, hvis der ikke er plads,
/// eller pakketypen ikke stemples.
pub fn append<const N: usize>(
    ptype: PacketType,
    dest: &Address,
    payload: &mut heapless::Vec<u8, N>,
) -> bool {
    match compute(ptype, dest, payload) {
        Some(s) => payload.extend_from_slice(&s).is_ok(),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn announce_body() -> heapless::Vec<u8, 255> {
        let mut b = heapless::Vec::new();
        b.extend_from_slice(&[7u8; 64]).unwrap();
        b.extend_from_slice(&1234u32.to_be_bytes()).unwrap();
        b.extend_from_slice(&[0u8; 65]).unwrap();
        b
    }

    #[test]
    fn valid_stamp_is_accepted_and_stripped() {
        let dest = Address([3; 16]);
        let body = announce_body();
        let mut p = body.clone();
        assert!(append(PacketType::Announce, &dest, &mut p));
        assert_eq!(p.len(), body.len() + STAMP_LEN);
        assert_eq!(verify(PacketType::Announce, &dest, &p), Ok(&body[..]));
        assert_eq!(strip(PacketType::Announce, &p), &body[..]);

        let mut r: heapless::Vec<u8, 64> = heapless::Vec::from_slice(&[9u8; 16]).unwrap();
        assert!(append(PacketType::PathRequest, &dest, &mut r));
        assert!(verify(PacketType::PathRequest, &dest, &r).is_ok());
    }

    #[test]
    fn wrong_or_missing_stamp_is_rejected() {
        let dest = Address([3; 16]);
        let body = announce_body();
        // Uden stempel: de sidste 5 bytes læses som stempel med version 0.
        assert!(verify(PacketType::Announce, &dest, &body).is_err());
        assert_eq!(
            verify(PacketType::PathRequest, &dest, &[1, 2, 3]),
            Err(StampError::Missing)
        );
        let mut p = body.clone();
        assert!(append(PacketType::Announce, &dest, &mut p));
        // Andet tidsstempel eller anden identitet: stemplet passer ikke.
        let mut other_ts = p.clone();
        other_ts[67] ^= 1;
        assert_eq!(
            verify(PacketType::Announce, &dest, &other_ts),
            Err(StampError::TooWeak)
        );
        // (Med 2^-12 chance passer det tilfældigt; bindingen er fast, så
        // testen er deterministisk.)
        assert_eq!(
            verify(PacketType::Announce, &Address([4; 16]), &p),
            Err(StampError::TooWeak)
        );
        // Ukendt version.
        let mut v = p.clone();
        let n = v.len();
        v[n - STAMP_LEN] = 9;
        assert_eq!(
            verify(PacketType::Announce, &dest, &v),
            Err(StampError::UnknownVersion(9))
        );
        // App-data er ikke bundet: kun signaturen beskytter dem.
        let mut app = p.clone();
        app[70] ^= 1;
        assert!(verify(PacketType::Announce, &dest, &app).is_ok());
    }

    #[test]
    fn difficulty_boundary() {
        let dest = Address([5; 16]);
        let token = [1u8; 16];
        // Find et stempel med præcis PATH_REQUEST_BITS nul-bits (og ikke flere).
        let mut s = StampSearch::with_bits(PacketType::PathRequest, &dest, &token, 0).unwrap();
        let (exact, below) = {
            let (mut exact, mut below) = (None, None);
            while exact.is_none() || below.is_none() {
                let st = s.step(1).unwrap();
                let mut h = prefix(STAMP_VERSION, PacketType::PathRequest, &dest, &token);
                h.update(&st[1..]);
                let z = leading_zero_bits(&h.finalize());
                if z == PATH_REQUEST_BITS && exact.is_none() {
                    exact = Some(st);
                }
                if z == PATH_REQUEST_BITS - 1 && below.is_none() {
                    below = Some(st);
                }
            }
            (exact.unwrap(), below.unwrap())
        };
        let mk = |st: [u8; STAMP_LEN]| {
            let mut p = token.to_vec();
            p.extend_from_slice(&st);
            p
        };
        assert!(verify(PacketType::PathRequest, &dest, &mk(exact)).is_ok());
        assert_eq!(
            verify(PacketType::PathRequest, &dest, &mk(below)),
            Err(StampError::TooWeak)
        );
    }

    #[test]
    fn leading_zeros() {
        assert_eq!(leading_zero_bits(&[0, 0, 0x80]), 16);
        assert_eq!(leading_zero_bits(&[0, 0x0f]), 12);
        assert_eq!(leading_zero_bits(&[0; 4]), 32);
    }

    #[test]
    fn chunked_search_matches_one_shot() {
        let dest = Address([6; 16]);
        let body = announce_body();
        let mut s = StampSearch::new(PacketType::Announce, &dest, &body).unwrap();
        let st = loop {
            if let Some(st) = s.step(100) {
                break st;
            }
        };
        assert_eq!(Some(st), compute(PacketType::Announce, &dest, &body));
    }

    /// Mål forsøg/s på værten:
    /// `cargo test --release -p maarse-core stamp_benchmark -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn stamp_benchmark() {
        let dest = Address([8; 16]);
        let body = announce_body();
        let mut s = StampSearch::with_bits(PacketType::Announce, &dest, &body, 64).unwrap();
        let t = std::time::Instant::now();
        assert!(s.step(2_000_000).is_none());
        let per_s = 2_000_000.0 / t.elapsed().as_secs_f64();
        println!(
            "{per_s:.0} forsøg/s; announce ({ANNOUNCE_BITS} bit) ≈ {:.2} ms, ruteforespørgsel ({PATH_REQUEST_BITS} bit) ≈ {:.3} ms",
            (1u64 << ANNOUNCE_BITS) as f64 / per_s * 1e3,
            (1u64 << PATH_REQUEST_BITS) as f64 / per_s * 1e3
        );
    }
}
