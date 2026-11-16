//! Alerts v2: signed public warnings that cannot be forged, replayed or
//! faked as text (design: docs/maybe/alerting-review.html, section 7).
//!
//! An alert is still an ordinary channel message on the well-known
//! [`crate::bulletin::CHANNEL_NAME`] channel, so every existing repeater
//! forwards it. The seal's sender field is the *issuer* key's address, a key
//! kept apart from the node identity and used only for alerts.
//!
//! ```text
//! alert:  0xB2 | flags | serial (u32) | issued (u32, Unix s) | expires (u16, min)
//!         | event | severity | [reference serial (u32), update/cancel only]
//!         | [area: lat, lon (i32, 1e-5 deg), radius (u16, 100 m)] | text
//!         | Ed25519 signature (64)
//! flags:  bits 0-1 status, bits 2-3 kind, bit 4 has area
//! signed: "maarse/alert/v2" | channel | issuer address | everything before the signature
//!
//! cert:   0xC1 | issuer public identity (64) | not_after (u32) | events (u16 mask)
//!         | max severity | flags | name length | name (UTF-8, <= 24) | signature (64)
//! signed: "maarse/alert-cert/v1" | everything before the signature, by a root key
//! ```
//!
//! Trust: a receiver trusts an issuer key it was configured with directly, or
//! an issuer whose certificate is signed by a configured root key; a
//! certificate limits the issuer to some events, a highest severity and a
//! last day. Replays are stopped by [`ReplayGuard`]: every alert has an
//! expiry, each (issuer, serial) is shown once, and clockless nodes use the
//! newest verified issue time as a lower bound on "now".

use crate::{Identity, PublicIdentity};
use ed25519_dalek::{Signature, Signer, Verifier, VerifyingKey};
use maarse_core::Address;
use sha2::{Digest, Sha256};

pub const TAG: u8 = 0xB2;
pub const CERT_TAG: u8 = 0xC1;
const CONTEXT: &[u8] = b"maarse/alert/v2";
const CERT_CONTEXT: &[u8] = b"maarse/alert-cert/v1";
const HEADER: usize = 14;
const SIG: usize = 64;
/// Longest issuer name in a certificate.
pub const MAX_NAME: usize = 24;
/// An issue time this far ahead of a known clock is refused.
pub const MAX_FUTURE_S: u32 = 600;

/// Status (CAP): only `ACTUAL` may ring loudly.
pub mod status {
    pub const ACTUAL: u8 = 0;
    pub const EXERCISE: u8 = 1;
    pub const TEST: u8 = 2;
    pub const SYSTEM: u8 = 3;
}

/// Message kind (CAP msgType). Update and cancel carry the serial of the
/// alert they refer to, which is always one of the same issuer's.
pub mod kind {
    pub const ALERT: u8 = 0;
    pub const UPDATE: u8 = 1;
    pub const CANCEL: u8 = 2;
}

/// Event codes (bit `n` in a certificate's event mask allows event `n`).
pub mod event {
    pub const INFO: u8 = 0;
    pub const EVACUATE: u8 = 1;
    pub const SHELTER: u8 = 2;
    pub const FIRE: u8 = 3;
    pub const FLOOD: u8 = 4;
    pub const WATER: u8 = 5;
    pub const CHEMICAL: u8 = 6;
    pub const POWER: u8 = 7;
    pub const WEATHER: u8 = 8;
    pub const MISSING: u8 = 9;
    pub const NAMES: &[&str] = &["info", "evacuate", "shelter", "fire", "flood", "water", "chemical", "power", "weather", "missing"];
}

/// Severity (CAP): 0 minor, 1 moderate, 2 severe, 3 extreme.
pub const SEVERITY_NAMES: &[&str] = &["minor", "moderate", "severe", "extreme"];

/// The area the alert is for (the receiver decides whether it is inside).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Area {
    pub lat_e5: i32,
    pub lon_e5: i32,
    pub radius_100m: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Alert<'a> {
    pub status: u8,
    pub kind: u8,
    /// Per issuer, strictly increasing.
    pub serial: u32,
    pub issued: u32,
    /// Minutes after `issued` (at least 1).
    pub expires_min: u16,
    pub event: u8,
    pub severity: u8,
    /// The alert an update or cancel refers to (0 for an alert).
    pub reference: u32,
    pub area: Option<Area>,
    pub text: &'a [u8],
}

impl Alert<'_> {
    /// When the alert stops being valid (Unix s).
    pub fn ends(&self) -> u32 {
        self.issued.saturating_add(self.expires_min.max(1) as u32 * 60)
    }
}

fn digest(context: &[u8], issuer: Option<&Address>, signed: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(context);
    if let Some(i) = issuer {
        h.update(crate::bulletin::channel().0);
        h.update(i.0);
    }
    h.update(signed);
    h.finalize().into()
}

/// Is this plaintext shaped like an alert (not yet verified)?
pub fn is_alert(plain: &[u8]) -> bool {
    plain.len() >= HEADER + SIG && plain[0] == TAG
}

/// Is this plaintext shaped like an issuer certificate?
pub fn is_cert(plain: &[u8]) -> bool {
    plain.len() >= 1 + 64 + 9 + SIG && plain[0] == CERT_TAG
}

/// Builds an alert's plaintext, signed by the issuer key. `None` if `out` is
/// too small or a field is out of range.
pub fn sign(issuer: &Identity, a: &Alert, out: &mut [u8]) -> Option<usize> {
    if a.status > 3 || a.kind > 2 || a.severity > 3 {
        return None;
    }
    let mut n = 0;
    let mut put = |b: &[u8]| -> Option<()> {
        out.get_mut(n..n + b.len())?.copy_from_slice(b);
        n += b.len();
        Some(())
    };
    let flags = a.status | a.kind << 2 | (a.area.is_some() as u8) << 4;
    put(&[TAG, flags])?;
    put(&a.serial.to_be_bytes())?;
    put(&a.issued.to_be_bytes())?;
    put(&a.expires_min.max(1).to_be_bytes())?;
    put(&[a.event, a.severity])?;
    if a.kind != kind::ALERT {
        put(&a.reference.to_be_bytes())?;
    }
    if let Some(ar) = a.area {
        put(&ar.lat_e5.to_be_bytes())?;
        put(&ar.lon_e5.to_be_bytes())?;
        put(&ar.radius_100m.to_be_bytes())?;
    }
    put(a.text)?;
    let sig = issuer.signing_key().sign(&digest(CONTEXT, Some(&issuer.address()), &out[..n]));
    out.get_mut(n..n + SIG)?.copy_from_slice(&sig.to_bytes());
    Some(n + SIG)
}

/// Splits an alert into its fields, the signed bytes and the signature
/// (nothing is verified).
pub fn parse(plain: &[u8]) -> Option<(Alert<'_>, &[u8], &[u8])> {
    if !is_alert(plain) {
        return None;
    }
    let (signed, sig) = plain.split_at(plain.len() - SIG);
    let flags = signed[1];
    let u32_at = |i: usize| signed.get(i..i + 4).map(|b| u32::from_be_bytes(b.try_into().unwrap()));
    let mut a = Alert {
        status: flags & 3,
        kind: flags >> 2 & 3,
        serial: u32_at(2)?,
        issued: u32_at(6)?,
        expires_min: u16::from_be_bytes([signed[10], signed[11]]),
        event: signed[12],
        severity: signed[13],
        reference: 0,
        area: None,
        text: &[],
    };
    if a.kind > kind::CANCEL || a.severity > 3 {
        return None;
    }
    let mut i = HEADER;
    if a.kind != kind::ALERT {
        a.reference = u32_at(i)?;
        i += 4;
    }
    if flags & 0x10 != 0 {
        let lat = u32_at(i)? as i32;
        let lon = u32_at(i + 4)? as i32;
        let r = signed.get(i + 8..i + 10)?;
        a.area = Some(Area { lat_e5: lat, lon_e5: lon, radius_100m: u16::from_be_bytes([r[0], r[1]]) });
        i += 10;
    }
    a.text = signed.get(i..)?;
    Some((a, signed, sig))
}

/// An issuer certificate: what a root lets an issuer key do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cert {
    pub issuer: PublicIdentity,
    /// Last valid moment (Unix s).
    pub not_after: u32,
    /// Allowed events (bit per `event::*`).
    pub events: u16,
    pub max_severity: u8,
    /// Shown as the sender of the issuer's alerts.
    pub name: heapless::String<MAX_NAME>,
}

/// Builds a certificate, signed by a root key.
pub fn sign_cert(root: &Identity, c: &Cert, out: &mut [u8]) -> Option<usize> {
    let name = c.name.as_bytes();
    let n = 1 + 64 + 4 + 2 + 1 + 1 + 1 + name.len();
    if out.len() < n + SIG {
        return None;
    }
    out[0] = CERT_TAG;
    out[1..65].copy_from_slice(&c.issuer.to_bytes());
    out[65..69].copy_from_slice(&c.not_after.to_be_bytes());
    out[69..71].copy_from_slice(&c.events.to_be_bytes());
    out[71] = c.max_severity;
    out[72] = 0;
    out[73] = name.len() as u8;
    out[74..n].copy_from_slice(name);
    let sig = root.signing_key().sign(&digest(CERT_CONTEXT, None, &out[..n]));
    out[n..n + SIG].copy_from_slice(&sig.to_bytes());
    Some(n + SIG)
}

/// Verifies a certificate against the root keys.
pub fn verify_cert(plain: &[u8], roots: &[PublicIdentity]) -> Option<Cert> {
    if !is_cert(plain) {
        return None;
    }
    let len = *plain.get(73)? as usize;
    if len > MAX_NAME {
        return None;
    }
    let n = 74 + len;
    let signed = plain.get(..n)?;
    let sig = Signature::from_bytes(plain.get(n..n + SIG)?.try_into().ok()?);
    let d = digest(CERT_CONTEXT, None, signed);
    roots.iter().find(|r| VerifyingKey::from_bytes(&r.sign).is_ok_and(|k| k.verify(&d, &sig).is_ok()))?;
    Some(Cert {
        issuer: PublicIdentity::from_bytes(&plain[1..65])?,
        not_after: u32::from_be_bytes(plain[65..69].try_into().ok()?),
        events: u16::from_be_bytes([plain[69], plain[70]]),
        max_severity: plain[71],
        name: heapless::String::try_from(core::str::from_utf8(&plain[74..n]).ok()?).ok()?,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reject {
    Malformed,
    /// Neither a trusted key nor a certificate for this issuer.
    Untrusted,
    BadSignature,
    /// The certificate does not allow this event or severity.
    OutOfScope,
    /// The certificate had run out when the alert was issued.
    CertExpired,
}

/// How an alert was trusted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Via {
    /// A key configured on this node.
    Key,
    /// A certificate from a root key (carries the issuer's name).
    Cert(heapless::String<MAX_NAME>),
}

/// Verifies an alert from `issuer` (the seal's sender).
pub fn verify<'a>(issuer: &Address, plain: &'a [u8], keys: &[PublicIdentity], certs: &[Cert]) -> Result<(Alert<'a>, Via), Reject> {
    let (a, signed, sig) = parse(plain).ok_or(Reject::Malformed)?;
    let (key, via) = if let Some(k) = keys.iter().find(|k| k.address() == *issuer) {
        (k.sign, Via::Key)
    } else {
        let c = certs.iter().find(|c| c.issuer.address() == *issuer).ok_or(Reject::Untrusted)?;
        if c.events & (1u16.checked_shl(a.event as u32).unwrap_or(0)) == 0 || a.severity > c.max_severity {
            return Err(Reject::OutOfScope);
        }
        if a.issued > c.not_after {
            return Err(Reject::CertExpired);
        }
        (c.issuer.sign, Via::Cert(c.name.clone()))
    };
    let sig = Signature::from_bytes(sig.try_into().map_err(|_| Reject::Malformed)?);
    VerifyingKey::from_bytes(&key)
        .map_err(|_| Reject::BadSignature)?
        .verify(&digest(CONTEXT, Some(issuer), signed), &sig)
        .map_err(|_| Reject::BadSignature)?;
    Ok((a, via))
}

/// What the replay guard says about a verified alert.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    /// Show it.
    New,
    /// Already shown (a scheduled repeat or another path): ignore quietly.
    Repeat,
    /// Past its expiry (an old alert sent again).
    Expired,
    /// Issued too far in the future.
    Future,
}

/// Remembers which alerts were shown, and the newest verified issue time
/// (a lower bound on the time for nodes without a clock).
pub struct ReplayGuard<const N: usize> {
    floor: u32,
    seen: heapless::Vec<(Address, u32), N>,
    next: usize,
}

impl<const N: usize> Default for ReplayGuard<N> {
    fn default() -> Self {
        ReplayGuard { floor: 0, seen: heapless::Vec::new(), next: 0 }
    }
}

impl<const N: usize> ReplayGuard<N> {
    /// The lower bound on the time.
    pub fn floor(&self) -> u32 {
        self.floor
    }

    /// Checks a verified alert and, if it is new, remembers it. `now` is the
    /// time if this node knows it.
    pub fn check(&mut self, issuer: &Address, a: &Alert, now: Option<u32>) -> Check {
        if now.is_some_and(|t| a.issued > t.saturating_add(MAX_FUTURE_S)) {
            return Check::Future;
        }
        let t = now.unwrap_or(0).max(self.floor);
        if t > a.ends() {
            return Check::Expired;
        }
        if self.seen.iter().any(|(i, s)| i == issuer && *s == a.serial) {
            return Check::Repeat;
        }
        if self.seen.len() < N {
            let _ = self.seen.push((*issuer, a.serial));
        } else if N > 0 {
            self.seen[self.next] = (*issuer, a.serial);
            self.next = (self.next + 1) % N;
        }
        self.floor = self.floor.max(a.issued);
        Check::New
    }

    /// Stored form: floor (u32) | (address (16) | serial (u32))*.
    pub fn to_bytes(&self, out: &mut [u8]) -> usize {
        let mut n = 4;
        out[..4].copy_from_slice(&self.floor.to_be_bytes());
        for (a, s) in &self.seen {
            if out.len() < n + 20 {
                break;
            }
            out[n..n + 16].copy_from_slice(&a.0);
            out[n + 16..n + 20].copy_from_slice(&s.to_be_bytes());
            n += 20;
        }
        n
    }

    pub fn from_bytes(b: &[u8]) -> Self {
        let mut g = Self::default();
        if let Some(f) = b.get(..4) {
            g.floor = u32::from_be_bytes(f.try_into().unwrap());
            for c in b[4..].chunks_exact(20) {
                let _ = g.seen.push((Address(c[..16].try_into().unwrap()), u32::from_be_bytes(c[16..].try_into().unwrap())));
            }
        }
        g
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand_core::OsRng;

    fn alert(serial: u32, issued: u32) -> Alert<'static> {
        Alert {
            status: status::ACTUAL,
            kind: kind::ALERT,
            serial,
            issued,
            expires_min: 60,
            event: event::FLOOD,
            severity: 2,
            reference: 0,
            area: Some(Area { lat_e5: 5_615_000, lon_e5: 1_020_000, radius_100m: 50 }),
            text: b"Flooding at the harbour, move to higher ground",
        }
    }

    #[test]
    fn signed_alert_round_trips_and_forgeries_fail() {
        let issuer = Identity::generate(&mut OsRng);
        let other = Identity::generate(&mut OsRng);
        let mut buf = [0u8; 255];
        let n = sign(&issuer, &alert(7, 1_800_000_000), &mut buf).unwrap();
        let plain = &buf[..n];
        let (a, via) = verify(&issuer.address(), plain, &[issuer.public()], &[]).unwrap();
        assert_eq!(a, alert(7, 1_800_000_000));
        assert_eq!(via, Via::Key);
        // Untrusted, wrong sender, tampered.
        assert_eq!(verify(&issuer.address(), plain, &[other.public()], &[]), Err(Reject::Untrusted));
        assert_eq!(verify(&other.address(), plain, &[issuer.public(), other.public()], &[]), Err(Reject::BadSignature));
        let mut bad = plain.to_vec();
        bad[3] ^= 1; // the serial
        assert_eq!(verify(&issuer.address(), &bad, &[issuer.public()], &[]), Err(Reject::BadSignature));
        // A cancel refers to a serial.
        let c = Alert { kind: kind::CANCEL, reference: 7, serial: 8, area: None, text: b"", ..alert(8, 1_800_000_100) };
        let n = sign(&issuer, &c, &mut buf).unwrap();
        assert_eq!(verify(&issuer.address(), &buf[..n], &[issuer.public()], &[]).unwrap().0, c);
    }

    #[test]
    fn certificates_scope_the_issuer() {
        let root = Identity::generate(&mut OsRng);
        let issuer = Identity::generate(&mut OsRng);
        let cert = Cert {
            issuer: issuer.public(),
            not_after: 1_900_000_000,
            events: 1 << event::FLOOD | 1 << event::INFO,
            max_severity: 2,
            name: heapless::String::try_from("Beredskab Nord").unwrap(),
        };
        let mut cb = [0u8; 200];
        let cn = sign_cert(&root, &cert, &mut cb).unwrap();
        assert!(verify_cert(&cb[..cn], &[issuer.public()]).is_none(), "not signed by a root");
        let c = verify_cert(&cb[..cn], &[root.public()]).unwrap();
        assert_eq!(c, cert);
        let mut buf = [0u8; 255];
        let n = sign(&issuer, &alert(1, 1_800_000_000), &mut buf).unwrap();
        let (_, via) = verify(&issuer.address(), &buf[..n], &[], core::slice::from_ref(&c)).unwrap();
        assert_eq!(via, Via::Cert(heapless::String::try_from("Beredskab Nord").unwrap()));
        let n = sign(&issuer, &Alert { event: event::FIRE, ..alert(2, 1_800_000_000) }, &mut buf).unwrap();
        assert_eq!(verify(&issuer.address(), &buf[..n], &[], core::slice::from_ref(&c)), Err(Reject::OutOfScope));
        let n = sign(&issuer, &Alert { severity: 3, ..alert(3, 1_800_000_000) }, &mut buf).unwrap();
        assert_eq!(verify(&issuer.address(), &buf[..n], &[], core::slice::from_ref(&c)), Err(Reject::OutOfScope));
        let n = sign(&issuer, &alert(4, 1_950_000_000), &mut buf).unwrap();
        assert_eq!(verify(&issuer.address(), &buf[..n], &[], core::slice::from_ref(&c)), Err(Reject::CertExpired));
    }

    #[test]
    fn replays_are_refused() {
        let who = Address([1; 16]);
        let mut g = ReplayGuard::<4>::default();
        let a = alert(10, 1_800_000_000);
        assert_eq!(g.check(&who, &a, Some(1_800_000_060)), Check::New);
        assert_eq!(g.check(&who, &a, Some(1_800_000_120)), Check::Repeat);
        // The same alert after its expiry (an old real alert sent again).
        let mut g2 = ReplayGuard::<4>::default();
        assert_eq!(g2.check(&who, &a, Some(1_800_000_000 + 3601)), Check::Expired);
        assert_eq!(g2.check(&who, &alert(11, 1_900_000_000), Some(1_800_000_000)), Check::Future);
        // Without a clock: a newer verified alert moves the floor, and the
        // old one is then expired.
        let mut g3 = ReplayGuard::<4>::default();
        assert_eq!(g3.check(&who, &alert(12, 1_800_010_000), None), Check::New);
        assert_eq!(g3.check(&who, &a, None), Check::Expired);
        // Stored and restored.
        let mut b = [0u8; 200];
        let n = g.to_bytes(&mut b);
        let mut back = ReplayGuard::<4>::from_bytes(&b[..n]);
        assert_eq!(back.check(&who, &a, Some(1_800_000_200)), Check::Repeat);
    }
}
