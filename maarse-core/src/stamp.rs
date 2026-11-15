//! Proof of work ("stamp") on announces and path requests.
//!
//! Inspired by Reticulum 1.6. A stamp is a nonce such that
//!
//! ```text
//! SHA-256("maarse/stamp" | version | packet type | destination | binding | nonce)
//! ```
//!
//! has at least `D` leading zero bits. The hash input is at most 50 bytes, so
//! it fits in one SHA-256 block: verification costs one block compression.
//! Making a stamp costs `2^D` attempts on average.
//!
//! The stamp is at the end of the payload and is [`STAMP_LEN`] bytes:
//!
//! ```text
//! ... payload ... | version (1) | nonce (4, big-endian)
//! ```
//!
//! Binding (what the stamp is tied to):
//! * `Announce`: the destination is the identity's address, and the binding is
//!   the timestamp (payload bytes 64..68). A stamp can therefore be computed
//!   before the app data is known, but only reused for the same identity and time.
//! * `PathRequest`: the destination (the target) and the reply token (the payload's
//!   first 16 bytes, random per request). The sender is not in
//!   the packet, so it cannot be bound.
//!
//! The difficulty is a constant per packet type and stamp version. The version
//! is in the stamp, so it can be raised later: a new version gets a higher
//! table value, and old versions can be removed from [`difficulty`].
//!
//! ## Difficulty
//!
//! Measured on a PC (x86-64, one core, release, `sha2` with SHA-NI) about 21
//! million attempts/s: an announce stamp takes about 0.2 ms and a
//! path request about 0.01 ms (the test `stamp_benchmark`, run with
//! `cargo test --release -p maarse-core stamp_benchmark -- --ignored --nocapture`). On a 64 MHz
//! Cortex-M4 (nRF52840) one SHA-256 block in software costs about 4,000–5,000
//! cycles, i.e. about 13,000–16,000 attempts/s.
//!
//! * `Announce`: 12 bits = 4,096 attempts on average ≈ 0.3 s on an nRF52840
//!   (an ESP32-S3 at 240 MHz is faster). Announces are sent every 15–30
//!   minutes, so it is free for an honest node, but an attacker on the same
//!   hardware can make at most about 3 fake identities per second.
//! * `PathRequest`: 8 bits = 256 attempts on average ≈ 20 ms on an nRF52840. They
//!   are made more often (every new recipient, repeats after 15 s) and computed in
//!   the router without yielding the executor, so they must be cheap.
//!   The probability of more than 4,096 attempts is e^-16, so the worst
//!   wait is in practice below about 0.3 s.
//!
//! A PC is over 1000 times faster than a microcontroller, so the stamp
//! does not stop a determined attacker with a strong machine. It makes spam
//! expensive enough that cheap nodes cannot flood the net, and LoRa's
//! airtime limit takes care of the rest.

use crate::addr::Address;
use crate::packet::PacketType;
use sha2::{Digest, Sha256};

/// Current stamp version.
pub const STAMP_VERSION: u8 = 1;
/// The stamp's length: version (1) + nonce (4).
pub const STAMP_LEN: usize = 5;
/// Difficulty (zero bits) for announces in version 1.
pub const ANNOUNCE_BITS: u32 = 12;
/// Difficulty (zero bits) for path requests in version 1.
pub const PATH_REQUEST_BITS: u32 = 8;

const DOMAIN: &[u8] = b"maarse/stamp";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StampError {
    /// The payload is too short for stamp and binding.
    Missing,
    /// Unknown (or retired) stamp version.
    UnknownVersion(u8),
    /// The hash has too few zero bits.
    TooWeak,
}

/// Does the packet type require a stamp?
pub fn required(ptype: PacketType) -> bool {
    matches!(ptype, PacketType::Announce | PacketType::PathRequest)
}

/// Required zero bits for `ptype` in stamp version `version`.
pub fn difficulty(version: u8, ptype: PacketType) -> Option<u32> {
    match (version, ptype) {
        (1, PacketType::Announce) => Some(ANNOUNCE_BITS),
        (1, PacketType::PathRequest) => Some(PATH_REQUEST_BITS),
        _ => None,
    }
}

/// The payload bytes (without stamp) the stamp is bound to.
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

/// Number of leading zero bits.
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

/// Splits a payload into (content, stamp). `None` if it is too short.
pub fn split(payload: &[u8]) -> Option<(&[u8], &[u8])> {
    let n = payload.len().checked_sub(STAMP_LEN)?;
    Some(payload.split_at(n))
}

/// The payload without stamp. If the packet is not stamped (or too short),
/// it is returned unchanged.
pub fn strip(ptype: PacketType, payload: &[u8]) -> &[u8] {
    if required(ptype) {
        split(payload).map_or(payload, |(b, _)| b)
    } else {
        payload
    }
}

/// Checks the stamp and returns the payload without stamp. Costs one hash.
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

/// Search for a stamp that can be split into pieces, so a firmware can
/// yield to the executor in between (see [`StampSearch::step`]).
#[derive(Clone)]
pub struct StampSearch {
    prefix: Sha256,
    bits: u32,
    nonce: u32,
}

impl StampSearch {
    /// Starts a search for a packet with the content `body` (without stamp).
    /// `None` if the packet type is not stamped, or `body` is too short.
    pub fn new(ptype: PacketType, dest: &Address, body: &[u8]) -> Option<Self> {
        Self::with_bits(ptype, dest, body, difficulty(STAMP_VERSION, ptype)?)
    }

    /// Like [`StampSearch::new`], but with an optional difficulty (for tests).
    pub fn with_bits(ptype: PacketType, dest: &Address, body: &[u8], bits: u32) -> Option<Self> {
        let bind = binding(ptype, body)?;
        Some(StampSearch {
            prefix: prefix(STAMP_VERSION, ptype, dest, bind),
            bits,
            nonce: 0,
        })
    }

    /// Tries at most `n` nonces. Returns the stamp when it is found.
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

    /// Number of attempts so far.
    pub fn tries(&self) -> u32 {
        self.nonce
    }
}

/// Computes a stamp in one go (blocking, for host and router).
pub fn compute(ptype: PacketType, dest: &Address, body: &[u8]) -> Option<[u8; STAMP_LEN]> {
    let mut s = StampSearch::new(ptype, dest, body)?;
    loop {
        if let Some(st) = s.step(u32::MAX) {
            return Some(st);
        }
    }
}

/// Appends a stamp to the end of `payload`. `false` if there is no room,
/// or the packet type is not stamped.
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
        // Without a stamp: the last 5 bytes are read as a stamp with version 0.
        assert!(verify(PacketType::Announce, &dest, &body).is_err());
        assert_eq!(
            verify(PacketType::PathRequest, &dest, &[1, 2, 3]),
            Err(StampError::Missing)
        );
        let mut p = body.clone();
        assert!(append(PacketType::Announce, &dest, &mut p));
        // A different timestamp or a different identity: the stamp does not fit.
        let mut other_ts = p.clone();
        other_ts[67] ^= 1;
        assert_eq!(
            verify(PacketType::Announce, &dest, &other_ts),
            Err(StampError::TooWeak)
        );
        // (With a 2^-12 chance it fits by accident; the binding is fixed, so
        // the test is deterministic.)
        assert_eq!(
            verify(PacketType::Announce, &Address([4; 16]), &p),
            Err(StampError::TooWeak)
        );
        // Unknown version.
        let mut v = p.clone();
        let n = v.len();
        v[n - STAMP_LEN] = 9;
        assert_eq!(
            verify(PacketType::Announce, &dest, &v),
            Err(StampError::UnknownVersion(9))
        );
        // App data is not bound: only the signature protects it.
        let mut app = p.clone();
        app[70] ^= 1;
        assert!(verify(PacketType::Announce, &dest, &app).is_ok());
    }

    #[test]
    fn difficulty_boundary() {
        let dest = Address([5; 16]);
        let token = [1u8; 16];
        // Find a stamp with exactly PATH_REQUEST_BITS zero bits (and no more).
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

    /// Measure attempts/s on the host:
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
            "{per_s:.0} attempts/s; announce ({ANNOUNCE_BITS} bits) ≈ {:.2} ms, path request ({PATH_REQUEST_BITS} bits) ≈ {:.3} ms",
            (1u64 << ANNOUNCE_BITS) as f64 / per_s * 1e3,
            (1u64 << PATH_REQUEST_BITS) as f64 / per_s * 1e3
        );
    }
}
