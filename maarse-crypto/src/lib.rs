//! # maarse-crypto
//!
//! Identities and signed announces.
//!
//! * An identity is two key pairs: Ed25519 (signatures) and X25519
//!   (key exchange).
//! * The address is the first 16 bytes of SHA-256 over both public keys.
//! * An announce proves that an address belongs to a key pair, and can carry
//!   a little app data (e.g. energy status and position for the map).
//!
//! Message encryption is in [`seal`]; forward secrecy for conversations
//! (sessions with a ratchet) in [`ratchet`].

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

pub mod alert;
pub mod bulletin;
pub mod files;
pub mod ota;
pub mod person;
pub mod meshcore_access;
pub mod ratchet;
pub mod recovery;
pub mod seal;

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use rand_core::CryptoRngCore;
use sha2::{Digest, Sha256};
use maarse_core::Address;
use x25519_dalek::{PublicKey as XPublic, StaticSecret};

pub const PUBLIC_IDENTITY_LEN: usize = 64;
pub const SIGNATURE_LEN: usize = 64;
/// Fixed part of an announce: keys + timestamp + app data length.
pub const ANNOUNCE_FIXED: usize = PUBLIC_IDENTITY_LEN + 4 + 1;
/// Largest app data in an announce, so it still fits in one LoRa packet.
pub const MAX_APP_DATA: usize =
    maarse_core::MAX_PACKET - 18 - ANNOUNCE_FIXED - SIGNATURE_LEN - maarse_core::stamp::STAMP_LEN;

/// Domain separation, so an announce signature cannot be reused for anything else.
const ANNOUNCE_CONTEXT: &[u8] = b"maarse/announce/v0";

fn dht_route_digest(route: &[u8]) -> [u8; 32] {
    Sha256::new().chain_update(b"maarse/dht-route/v1").chain_update(route).finalize().into()
}

fn dht_exchange_digest(context: &[u8], challenge: &[u8; 32], request: &[u8], response: &[u8]) -> [u8; 32] {
    Sha256::new().chain_update(context)
        .chain_update(challenge)
        .chain_update((request.len() as u64).to_le_bytes()).chain_update(request)
        .chain_update(response).finalize().into()
}

/// Public part of an identity.
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

    /// Is `signature` this node's signature on the mail bundle `id`
    /// addressed to it (see [`Identity::sign_delivery`])?
    pub fn verify_delivery(&self, id: u64, signature: &maarse_core::router::Signature) -> bool {
        let Ok(key) = VerifyingKey::from_bytes(&self.sign) else { return false };
        key.verify_strict(&delivery_message(&self.address(), id), &Signature::from_bytes(signature)).is_ok()
    }

    pub fn verify_dht_reply(&self, challenge: &[u8; 32], request: &[u8], response: &[u8], signature: &[u8; SIGNATURE_LEN]) -> bool {
        let Ok(key) = VerifyingKey::from_bytes(&self.sign) else { return false };
        key.verify_strict(&dht_exchange_digest(b"maarse/dht-reply/v1", challenge, request, response), &Signature::from_bytes(signature)).is_ok()
    }

    pub fn verify_dht_request(&self, server_challenge: &[u8; 32], client_challenge: &[u8; 32], request: &[u8], signature: &[u8; SIGNATURE_LEN]) -> bool {
        let Ok(key) = VerifyingKey::from_bytes(&self.sign) else { return false };
        key.verify_strict(&dht_exchange_digest(b"maarse/dht-request/v1", server_challenge, request, client_challenge), &Signature::from_bytes(signature)).is_ok()
    }

    pub fn verify_dht_route(&self, route: &[u8], signature: &[u8; SIGNATURE_LEN]) -> bool {
        let Ok(key) = VerifyingKey::from_bytes(&self.sign) else { return false };
        key.verify_strict(&dht_route_digest(route), &Signature::from_bytes(signature)).is_ok()
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

/// A node's secret identity.
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

    /// Restores an identity from 64 stored bytes (e.g. from flash).
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

    pub fn sign_dht_reply(&self, challenge: &[u8; 32], request: &[u8], response: &[u8]) -> [u8; SIGNATURE_LEN] {
        self.sign.sign(&dht_exchange_digest(b"maarse/dht-reply/v1", challenge, request, response)).to_bytes()
    }

    pub fn sign_dht_request(&self, server_challenge: &[u8; 32], client_challenge: &[u8; 32], request: &[u8]) -> [u8; SIGNATURE_LEN] {
        self.sign.sign(&dht_exchange_digest(b"maarse/dht-request/v1", server_challenge, request, client_challenge)).to_bytes()
    }

    pub fn sign_dht_route(&self, route: &[u8]) -> [u8; SIGNATURE_LEN] {
        self.sign.sign(&dht_route_digest(route)).to_bytes()
    }

    /// The proof that a direct message reached its recipient (see
    /// `maarse_core::router`, "Delivery proof"). `payload` is the sealed
    /// message as sent to `recipient`; `peer` is the other party. Only the
    /// two of them can compute it, from the secret their long-term keys
    /// share. `None` for a peer whose key makes that secret public.
    ///
    /// ```text
    /// HKDF-SHA256(salt "maarse/receipt/v1", DH(me, peer),
    ///             info recipient || SHA-256(payload)), 8 bytes
    /// ```
    pub fn receipt_proof(&self, peer: &PublicIdentity, recipient: &Address, payload: &[u8]) -> Option<maarse_core::router::Proof> {
        let shared = self.dh.diffie_hellman(&XPublic::from(peer.dh));
        if !shared.was_contributory() {
            return None;
        }
        let mut info = [0u8; 48];
        info[..16].copy_from_slice(&recipient.0);
        info[16..].copy_from_slice(&Sha256::digest(payload));
        let mut proof = [0u8; maarse_core::router::PROOF_LEN];
        hkdf::Hkdf::<Sha256>::new(Some(b"maarse/receipt/v1"), shared.as_bytes()).expand(&info, &mut proof).ok()?;
        Some(proof)
    }

    /// Our signature that the mail bundle with packet ID `id`, addressed to
    /// us, has arrived (the payload of its acknowledgement).
    #[inline(never)]
    pub fn sign_delivery(&self, id: u64) -> maarse_core::router::Signature {
        self.sign.sign(&delivery_message(&self.address(), id)).to_bytes()
    }

    pub(crate) fn signing_key(&self) -> &SigningKey {
        &self.sign
    }

    /// Builds the payload of an announce. The packet is sent with destination =
    /// our own address and packet type `Announce`.
    ///
    /// Format: public keys (64) | timestamp (4) | app data length (1)
    /// | app data | signature (64).
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

/// Payload of an announce (see [`Identity::announce`]).
pub type AnnounceBuf = heapless::Vec<u8, { maarse_core::MAX_PACKET }>;

/// A verified announce.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Announce<'a> {
    pub identity: PublicIdentity,
    pub timestamp: u32,
    pub app_data: &'a [u8],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnnounceError {
    Malformed,
    /// The address does not match the keys.
    WrongAddress,
    BadSignature,
}

/// Reads the timestamp from an announcement that has already been verified.
pub fn announce_timestamp(verified: &[u8]) -> Option<u32> {
    Some(u32::from_be_bytes(verified.get(64..68)?.try_into().ok()?))
}

/// Verifies an announce against the packet's destination address.
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
    key.verify_strict(&msg, &Signature::from_bytes(&sig_bytes))
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

/// What is signed for a mail bundle: context | recipient | packet ID.
fn delivery_message(recipient: &Address, id: u64) -> [u8; 40] {
    let mut m = [0u8; 40];
    m[..16].copy_from_slice(b"maarse/bundle/v1");
    m[16..32].copy_from_slice(&recipient.0);
    m[32..].copy_from_slice(&id.to_le_bytes());
    m
}

/// Address of a channel: hash of the channel key.
pub fn channel_address(key: &[u8; 32]) -> Address {
    let d = Sha256::digest(key);
    let mut a = [0u8; 16];
    a.copy_from_slice(&d[..16]);
    Address(a)
}

/// Key for a hashtag channel like `#aarhus`: everyone who knows the name can read.
pub fn hashtag_key(name: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"maarse/hashtag/v0");
    h.update(name.as_bytes());
    h.finalize().into()
}

/// The key of the public channel.
pub fn public_channel_key() -> [u8; 32] {
    Sha256::digest(b"maarse-public").into()
}

/// Domain separation for safety numbers.
const SAFETY_CONTEXT: &[u8] = b"maarse/safety-number/v0";
/// Number of SHA-256 rounds per fingerprint. Low enough for firmware, but makes it
/// a little more expensive to search for a key with a particular number.
pub const SAFETY_ITERATIONS: u32 = 512;

/// 30 decimal digits (ASCII) that depend on one identity only.
///
/// `h0 = SHA-256(context | sign | dh)`, `h(i+1) = SHA-256(h(i) | sign | dh)`,
/// [`SAFETY_ITERATIONS`] rounds in total. The first 30 bytes are split into 6 pieces
/// of 5 bytes; each piece is read as a big-endian number modulo 100000 and written
/// with 5 digits.
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

/// Safety number for two contacts: 60 digits (ASCII), independent of the
/// order. The two fingerprints are sorted and joined. Usually shown as
/// 12 groups of 5 digits; if the number is the same on both phones, the keys are genuine.
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

    /// A signing key of small order accepts one signature for any message:
    /// it must not pass as an identity.
    #[test]
    fn announce_with_a_weak_signing_key_is_rejected() {
        let mut weak = [0u8; 32];
        weak[0] = 1; // The identity point.
        let id = PublicIdentity { sign: weak, dh: [9; 32] };
        let mut p = id.to_bytes().to_vec();
        p.extend_from_slice(&1_000u32.to_be_bytes());
        p.push(0);
        p.extend_from_slice(&weak); // R
        p.extend_from_slice(&[0; 32]); // s
        assert_eq!(verify_announce(&id.address(), &p), Err(AnnounceError::BadSignature));
    }

    #[test]
    fn only_sender_and_recipient_compute_the_delivery_proof() {
        let (a, b, c) = (Identity::generate(&mut OsRng), Identity::generate(&mut OsRng), Identity::generate(&mut OsRng));
        let to = b.address();
        let sender = a.receipt_proof(&b.public(), &to, b"sealed").unwrap();
        assert_eq!(b.receipt_proof(&a.public(), &to, b"sealed"), Some(sender));
        // Bound to the message and to its direction.
        assert_ne!(a.receipt_proof(&b.public(), &to, b"sealeD"), Some(sender));
        assert_ne!(a.receipt_proof(&b.public(), &a.address(), b"sealed"), Some(sender));
        // Somebody else, who knows both public keys.
        assert_ne!(c.receipt_proof(&b.public(), &to, b"sealed"), Some(sender));
        assert_ne!(c.receipt_proof(&a.public(), &to, b"sealed"), Some(sender));
        let weak = PublicIdentity { sign: [7; 32], dh: [0; 32] };
        assert_eq!(a.receipt_proof(&weak, &to, b"sealed"), None);
    }

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
        use maarse_core::packet::{Header, Packet, encode};
        use maarse_core::stamp::{self, STAMP_LEN};
        use maarse_core::PacketType;
        let id = Identity::generate(&mut OsRng);
        let mut payload = id.announce(0, &[0u8; MAX_APP_DATA]).unwrap();
        assert_eq!(payload.len() + 18 + STAMP_LEN, maarse_core::MAX_PACKET);
        // With a stamp it fills exactly one packet, and it can still be verified.
        let dest = id.address();
        assert!(stamp::append(PacketType::Announce, &dest, &mut payload));
        let pkt = encode(&Header::new(PacketType::Announce, dest), &payload).unwrap();
        let p = Packet::decode(&pkt).unwrap();
        let body = stamp::verify(PacketType::Announce, &dest, p.payload).unwrap();
        assert!(verify_announce(&dest, body).is_ok());
        assert!(id.announce(0, &[0u8; MAX_APP_DATA + 1]).is_none());
    }

    #[test]
    fn rejects_forged_announces() {
        let id = Identity::generate(&mut OsRng);
        let other = Identity::generate(&mut OsRng);
        let payload = id.announce(1, b"x").unwrap();

        // A real signature, but sent as a different address.
        assert_eq!(
            verify_announce(&other.address(), &payload),
            Err(AnnounceError::WrongAddress)
        );

        // Changed app data.
        let mut bad = payload.clone();
        let n = bad.len();
        bad[n - SIGNATURE_LEN - 1] ^= 1;
        assert_eq!(
            verify_announce(&id.address(), &bad),
            Err(AnnounceError::BadSignature)
        );

        // Truncated.
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

    /// Test vector. The same number is in `tools/safety_number_test.mjs`, which
    /// checks the web client's JavaScript version.
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
        assert_eq!(&n[..], b"091047891378483515820899286603282445802009510472517494707964");
        // The order does not matter.
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
