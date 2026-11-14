//! # maarse-crypto
//!
//! Identiteter og signerede announces.
//!
//! * En identitet er to nøglepar: Ed25519 (signaturer) og X25519
//!   (nøgleudveksling).
//! * Adressen er de første 16 bytes af SHA-256 over begge offentlige nøgler.
//! * Et announce beviser, at en adresse tilhører et nøglepar, og kan bære
//!   lidt app-data (fx energistatus og position til kortet).
//!
//! Kryptering af beskeder ligger i [`seal`]. Ratchets kommer senere.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

pub mod seal;

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand_core::CryptoRngCore;
use sha2::{Digest, Sha256};
use maarse_core::Address;
use x25519_dalek::{PublicKey as XPublic, StaticSecret};

pub const PUBLIC_IDENTITY_LEN: usize = 64;
pub const SIGNATURE_LEN: usize = 64;
/// Fast del af et announce: nøgler + tidsstempel + længde af app-data.
pub const ANNOUNCE_FIXED: usize = PUBLIC_IDENTITY_LEN + 4 + 1;
/// Største app-data i et announce, så det stadig passer i én LoRa-pakke.
pub const MAX_APP_DATA: usize = maarse_core::MAX_PACKET - 18 - ANNOUNCE_FIXED - SIGNATURE_LEN;

/// Domæneseparation, så en announce-signatur ikke kan genbruges til andet.
const ANNOUNCE_CONTEXT: &[u8] = b"maarse/announce/v0";

/// Offentlig del af en identitet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PublicIdentity {
    pub sign: [u8; 32],
    pub dh: [u8; 32],
}

impl PublicIdentity {
    pub fn address(&self) -> Address {
        let mut h = Sha256::new();
        h.update(self.sign);
        h.update(self.dh);
        let digest = h.finalize();
        let mut a = [0u8; 16];
        a.copy_from_slice(&digest[..16]);
        Address(a)
    }

    pub fn to_bytes(&self) -> [u8; PUBLIC_IDENTITY_LEN] {
        let mut b = [0u8; PUBLIC_IDENTITY_LEN];
        b[..32].copy_from_slice(&self.sign);
        b[32..].copy_from_slice(&self.dh);
        b
    }

    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        if b.len() < PUBLIC_IDENTITY_LEN {
            return None;
        }
        Some(PublicIdentity {
            sign: b[..32].try_into().ok()?,
            dh: b[32..64].try_into().ok()?,
        })
    }
}

/// En nodes hemmelige identitet.
pub struct Identity {
    sign: SigningKey,
    dh: StaticSecret,
}

impl Identity {
    pub fn generate(rng: &mut impl CryptoRngCore) -> Self {
        let sign = SigningKey::generate(rng);
        let mut seed = [0u8; 32];
        rng.fill_bytes(&mut seed);
        Identity {
            sign,
            dh: StaticSecret::from(seed),
        }
    }

    /// Genskaber en identitet fra 64 gemte bytes (fx fra flash).
    pub fn from_secret_bytes(b: &[u8; 64]) -> Self {
        let mut s = [0u8; 32];
        s.copy_from_slice(&b[..32]);
        let mut d = [0u8; 32];
        d.copy_from_slice(&b[32..]);
        Identity {
            sign: SigningKey::from_bytes(&s),
            dh: StaticSecret::from(d),
        }
    }

    pub fn to_secret_bytes(&self) -> [u8; 64] {
        let mut b = [0u8; 64];
        b[..32].copy_from_slice(&self.sign.to_bytes());
        b[32..].copy_from_slice(self.dh.as_bytes());
        b
    }

    pub fn public(&self) -> PublicIdentity {
        PublicIdentity {
            sign: self.sign.verifying_key().to_bytes(),
            dh: XPublic::from(&self.dh).to_bytes(),
        }
    }

    pub fn address(&self) -> Address {
        self.public().address()
    }

    /// Bygger payloaden til et announce. Pakken sendes med destination =
    /// vores egen adresse og pakketype `Announce`.
    ///
    /// Format: offentlige nøgler (64) | tidsstempel (4) | app-data-længde (1)
    /// | app-data | signatur (64).
    pub fn announce(
        &self,
        timestamp: u32,
        app_data: &[u8],
    ) -> Option<heapless::Vec<u8, { maarse_core::MAX_PACKET }>> {
        if app_data.len() > MAX_APP_DATA {
            return None;
        }
        let mut body: heapless::Vec<u8, { maarse_core::MAX_PACKET }> = heapless::Vec::new();
        body.extend_from_slice(&self.public().to_bytes()).ok()?;
        body.extend_from_slice(&timestamp.to_be_bytes()).ok()?;
        body.push(app_data.len() as u8).ok()?;
        body.extend_from_slice(app_data).ok()?;
        let sig = self.sign.sign(&signed_message(&self.address(), &body)?);
        body.extend_from_slice(&sig.to_bytes()).ok()?;
        Some(body)
    }
}

/// Et verificeret announce.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Announce<'a> {
    pub identity: PublicIdentity,
    pub timestamp: u32,
    pub app_data: &'a [u8],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnnounceError {
    Malformed,
    /// Adressen passer ikke til nøglerne.
    WrongAddress,
    BadSignature,
}

/// Verificerer et announce mod pakkens destinationsadresse.
pub fn verify_announce<'a>(
    dest: &Address,
    payload: &'a [u8],
) -> Result<Announce<'a>, AnnounceError> {
    if payload.len() < ANNOUNCE_FIXED + SIGNATURE_LEN {
        return Err(AnnounceError::Malformed);
    }
    let identity = PublicIdentity::from_bytes(payload).ok_or(AnnounceError::Malformed)?;
    if identity.address() != *dest {
        return Err(AnnounceError::WrongAddress);
    }
    let timestamp = u32::from_be_bytes(
        payload[64..68]
            .try_into()
            .map_err(|_| AnnounceError::Malformed)?,
    );
    let n = payload[68] as usize;
    let body_len = ANNOUNCE_FIXED + n;
    if payload.len() != body_len + SIGNATURE_LEN {
        return Err(AnnounceError::Malformed);
    }
    let sig_bytes: [u8; 64] = payload[body_len..]
        .try_into()
        .map_err(|_| AnnounceError::Malformed)?;
    let key = VerifyingKey::from_bytes(&identity.sign).map_err(|_| AnnounceError::BadSignature)?;
    let msg = signed_message(dest, &payload[..body_len]).ok_or(AnnounceError::Malformed)?;
    key.verify(&msg, &Signature::from_bytes(&sig_bytes))
        .map_err(|_| AnnounceError::BadSignature)?;
    Ok(Announce {
        identity,
        timestamp,
        app_data: &payload[ANNOUNCE_FIXED..body_len],
    })
}

fn signed_message(addr: &Address, body: &[u8]) -> Option<heapless::Vec<u8, 320>> {
    let mut m: heapless::Vec<u8, 320> = heapless::Vec::new();
    m.extend_from_slice(ANNOUNCE_CONTEXT).ok()?;
    m.extend_from_slice(&addr.0).ok()?;
    m.extend_from_slice(body).ok()?;
    Some(m)
}

/// Adresse for en kanal: hash af kanalens nøgle.
pub fn channel_address(key: &[u8; 32]) -> Address {
    let d = Sha256::digest(key);
    let mut a = [0u8; 16];
    a.copy_from_slice(&d[..16]);
    Address(a)
}

/// Nøgle til en hashtag-kanal som `#aarhus`: alle, der kender navnet, kan læse.
pub fn hashtag_key(name: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"maarse/hashtag/v0");
    h.update(name.as_bytes());
    h.finalize().into()
}

/// Nøglen til den offentlige kanal.
pub fn public_channel_key() -> [u8; 32] {
    Sha256::digest(b"maarse-public").into()
}

/// Domæneseparation for sikkerhedsnumre.
const SAFETY_CONTEXT: &[u8] = b"maarse/sikkerhedsnummer/v0";
/// Antal SHA-256-runder pr. fingeraftryk. Lavt nok til firmware, men gør det
/// lidt dyrere at lede efter en nøgle med et bestemt nummer.
pub const SAFETY_ITERATIONS: u32 = 512;

/// 30 decimale cifre (ASCII), der kun afhænger af én identitet.
///
/// `h0 = SHA-256(kontekst | sign | dh)`, `h(i+1) = SHA-256(h(i) | sign | dh)`,
/// i alt [`SAFETY_ITERATIONS`] runder. De første 30 bytes deles i 6 stykker
/// à 5 bytes; hvert stykke læses som big-endian tal modulo 100000 og skrives
/// med 5 cifre.
pub fn safety_fingerprint(id: &PublicIdentity) -> [u8; 30] {
    let key = id.to_bytes();
    let mut h: [u8; 32] = {
        let mut d = Sha256::new();
        d.update(SAFETY_CONTEXT);
        d.update(key);
        d.finalize().into()
    };
    for _ in 1..SAFETY_ITERATIONS {
        let mut d = Sha256::new();
        d.update(h);
        d.update(key);
        h = d.finalize().into();
    }
    let mut out = [b'0'; 30];
    for i in 0..6 {
        let mut v: u64 = 0;
        for b in &h[i * 5..i * 5 + 5] {
            v = (v << 8) | *b as u64;
        }
        let mut n = (v % 100_000) as u32;
        for j in (0..5).rev() {
            out[i * 5 + j] = b'0' + (n % 10) as u8;
            n /= 10;
        }
    }
    out
}

/// Sikkerhedsnummer for to kontakter: 60 cifre (ASCII), uafhængigt af
/// rækkefølgen. De to fingeraftryk sorteres og sættes sammen. Vises typisk i
/// 12 grupper à 5 cifre; er tallet ens på begge telefoner, er nøglerne ægte.
pub fn safety_number(a: &PublicIdentity, b: &PublicIdentity) -> [u8; 60] {
    let (fa, fb) = (safety_fingerprint(a), safety_fingerprint(b));
    let (lo, hi) = if fa <= fb { (fa, fb) } else { (fb, fa) };
    let mut out = [0u8; 60];
    out[..30].copy_from_slice(&lo);
    out[30..].copy_from_slice(&hi);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand_core::OsRng;

    #[test]
    fn address_is_stable_and_restorable() {
        let id = Identity::generate(&mut OsRng);
        let restored = Identity::from_secret_bytes(&id.to_secret_bytes());
        assert_eq!(id.address(), restored.address());
        assert_ne!(id.address(), Identity::generate(&mut OsRng).address());
    }

    #[test]
    fn announce_roundtrip() {
        let id = Identity::generate(&mut OsRng);
        let payload = id.announce(1234, b"energi=3").unwrap();
        let a = verify_announce(&id.address(), &payload).unwrap();
        assert_eq!(a.identity, id.public());
        assert_eq!(a.timestamp, 1234);
        assert_eq!(a.app_data, b"energi=3");
    }

    #[test]
    fn announce_fits_in_one_packet() {
        let id = Identity::generate(&mut OsRng);
        let payload = id.announce(0, &[0u8; MAX_APP_DATA]).unwrap();
        assert_eq!(payload.len() + 18, maarse_core::MAX_PACKET);
        assert!(id.announce(0, &[0u8; MAX_APP_DATA + 1]).is_none());
    }

    #[test]
    fn rejects_forged_announces() {
        let id = Identity::generate(&mut OsRng);
        let other = Identity::generate(&mut OsRng);
        let payload = id.announce(1, b"x").unwrap();

        // Rigtig signatur, men sendt som en anden adresse.
        assert_eq!(
            verify_announce(&other.address(), &payload),
            Err(AnnounceError::WrongAddress)
        );

        // Ændret app-data.
        let mut bad = payload.clone();
        let n = bad.len();
        bad[n - SIGNATURE_LEN - 1] ^= 1;
        assert_eq!(
            verify_announce(&id.address(), &bad),
            Err(AnnounceError::BadSignature)
        );

        // Afkortet.
        assert_eq!(
            verify_announce(&id.address(), &payload[..100]),
            Err(AnnounceError::Malformed)
        );
    }

    #[test]
    fn channel_keys_differ() {
        let a = channel_address(&hashtag_key("#aarhus"));
        let b = channel_address(&hashtag_key("#odense"));
        assert_ne!(a, b);
        assert_ne!(a, channel_address(&public_channel_key()));
    }

    /// Testvektor. Samme tal står i `tools/safety_number_test.mjs`, som
    /// tjekker webklientens JavaScript-udgave.
    #[test]
    fn safety_number_vector() {
        let a = Identity::from_secret_bytes(&[1u8; 64]).public();
        let b = Identity::from_secret_bytes(&[2u8; 64]).public();
        let hex = |k: &[u8]| k.iter().map(|x| format!("{x:02x}")).collect::<String>();
        assert_eq!(
            hex(&a.to_bytes()),
            "8a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5ca4e09292b651c278b9772c569f5fa9bb13d906b46ab68c9df9dc2b4409f8a209"
        );
        assert_eq!(
            hex(&b.to_bytes()),
            "8139770ea87d175f56a35466c34c7ecccb8d8a91b4ee37a25df60f5b8fc9b394ce8d3ad1ccb633ec7b70c17814a5c76ecd029685050d344745ba05870e587d59"
        );
        let n = safety_number(&a, &b);
        assert_eq!(&n[..], b"490814315733469534137526241006891633028785095264240772532833");
        // Rækkefølgen er ligegyldig.
        assert_eq!(n, safety_number(&b, &a));
    }

    #[test]
    fn safety_number_changes_with_key() {
        let a = Identity::generate(&mut OsRng).public();
        let b = Identity::generate(&mut OsRng).public();
        let mut b2 = b;
        b2.dh[0] ^= 1;
        assert_ne!(safety_number(&a, &b), safety_number(&a, &b2));
        assert!(safety_number(&a, &b).iter().all(|c| c.is_ascii_digit()));
    }
}
