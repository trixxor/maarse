//! Message encryption.
//!
//! ## Direct messages
//!
//! One-way pattern in the style of Noise `X`, adapted to LoRa (64 bytes overhead):
//!
//! ```text
//! e_pub (32) | sender ⊕ k1 (16) | ChaCha20-Poly1305(k2, message) | tag (16)
//!
//! es = DH(e, R)          k1 = HKDF(es)                -> hides the sender
//! ss = DH(S, R)          k2 = HKDF(es || ss, e, R, S) -> encrypts and authenticates
//! ```
//!
//! * Only the recipient can see who the sender is.
//! * `ss` proves that the message comes from the sender. If the hidden
//!   sender is changed, `k2` becomes wrong, and decryption fails.
//! * The fresh key `e` means that a leaked sender key does not reveal
//!   old messages. Full forward secrecy against a leaked recipient key is given by
//!   the sessions in [`crate::ratchet`]; this format is their fallback.
//!
//! ## Channels
//!
//! ```text
//! nonce (8) | ChaCha20-Poly1305(k, sender (16) | message) | tag (16)
//! ```
//! Everyone with the channel key can read and write. The sender is not proven.

use crate::{Identity, PublicIdentity};
use chacha20poly1305::aead::AeadInPlace;
use chacha20poly1305::{ChaCha20Poly1305, Key, KeyInit, Nonce, Tag};
use hkdf::Hkdf;
use rand_core::CryptoRngCore;
use sha2::Sha256;
use maarse_core::Address;
use x25519_dalek::{EphemeralSecret, PublicKey as XPublic};

pub const DM_OVERHEAD: usize = 32 + 16 + 16;
pub const CHANNEL_OVERHEAD: usize = 8 + 16 + 16;
/// Largest text in a direct message (one packet with a full path of 12 hops).
pub const MAX_DM_TEXT: usize = maarse_core::router::MAX_DIRECT_PAYLOAD - DM_OVERHEAD;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CryptoError {
    BufferTooSmall,
    Malformed,
    /// We do not know the sender's keys yet (no announce heard).
    UnknownSender(Address),
    /// Wrong key, forged or corrupted message.
    Decrypt,
    /// Ratchet message, but no session with the sender (see [`crate::ratchet`]).
    NoSession(Address),
    /// The session with the sender does not match the message.
    Desync(Address),
}

fn mask_key(es: &[u8; 32], e_pub: &[u8; 32], recipient: &Address) -> [u8; 16] {
    let hk = Hkdf::<Sha256>::new(Some(b"maarse/dm/v0/sender"), es);
    let mut info = [0u8; 48];
    info[..32].copy_from_slice(e_pub);
    info[32..].copy_from_slice(&recipient.0);
    let mut k = [0u8; 16];
    // Cannot fail: 16 bytes is below HKDF's limit.
    let _ = hk.expand(&info, &mut k);
    k
}

fn message_key(
    es: &[u8; 32],
    ss: &[u8; 32],
    e_pub: &[u8; 32],
    recipient: &Address,
    sender: &Address,
) -> [u8; 32] {
    let mut ikm = [0u8; 64];
    ikm[..32].copy_from_slice(es);
    ikm[32..].copy_from_slice(ss);
    let hk = Hkdf::<Sha256>::new(Some(b"maarse/dm/v0"), &ikm);
    let mut info = [0u8; 64];
    info[..32].copy_from_slice(e_pub);
    info[32..48].copy_from_slice(&recipient.0);
    info[48..].copy_from_slice(&sender.0);
    let mut k = [0u8; 32];
    let _ = hk.expand(&info, &mut k);
    k
}

/// Encrypts a direct message to `recipient`. Writes to `out` and
/// returns the length (`msg.len() + DM_OVERHEAD`).
pub fn seal_direct(
    sender: &Identity,
    recipient: &PublicIdentity,
    rng: &mut impl CryptoRngCore,
    msg: &[u8],
    out: &mut [u8],
) -> Result<usize, CryptoError> {
    let total = msg.len() + DM_OVERHEAD;
    if out.len() < total {
        return Err(CryptoError::BufferTooSmall);
    }
    let r_dh = XPublic::from(recipient.dh);
    let r_addr = recipient.address();
    let s_addr = sender.address();

    let e = EphemeralSecret::random_from_rng(rng);
    let e_pub = XPublic::from(&e).to_bytes();
    let es = e.diffie_hellman(&r_dh);
    let ss = sender.dh.diffie_hellman(&r_dh);
    if !es.was_contributory() || !ss.was_contributory() {
        return Err(CryptoError::Malformed);
    }

    let mask = mask_key(es.as_bytes(), &e_pub, &r_addr);
    out[..32].copy_from_slice(&e_pub);
    for i in 0..16 {
        out[32 + i] = s_addr.0[i] ^ mask[i];
    }
    let key = message_key(es.as_bytes(), ss.as_bytes(), &e_pub, &r_addr, &s_addr);
    let (head, rest) = out.split_at_mut(48);
    let body = &mut rest[..msg.len()];
    body.copy_from_slice(msg);
    // The key is new for every message (fresh e), so a fixed nonce is safe.
    let tag = ChaCha20Poly1305::new(Key::from_slice(&key))
        .encrypt_in_place_detached(Nonce::from_slice(&[0u8; 12]), head, body)
        .map_err(|_| CryptoError::Malformed)?;
    rest[msg.len()..msg.len() + 16].copy_from_slice(&tag);
    Ok(total)
}

/// Decrypts a direct message to `me`. `lookup` looks up the sender's
/// public keys (from heard announces). Returns sender and length.
pub fn open_direct(
    me: &Identity,
    payload: &[u8],
    lookup: impl Fn(&Address) -> Option<PublicIdentity>,
    out: &mut [u8],
) -> Result<(Address, usize), CryptoError> {
    if payload.len() < DM_OVERHEAD {
        return Err(CryptoError::Malformed);
    }
    let n = payload.len() - DM_OVERHEAD;
    if out.len() < n {
        return Err(CryptoError::BufferTooSmall);
    }
    let mut e_pub = [0u8; 32];
    e_pub.copy_from_slice(&payload[..32]);
    let my_addr = me.address();
    let es = me.dh.diffie_hellman(&XPublic::from(e_pub));
    if !es.was_contributory() {
        return Err(CryptoError::Decrypt);
    }
    let mask = mask_key(es.as_bytes(), &e_pub, &my_addr);
    let mut s = [0u8; 16];
    for i in 0..16 {
        s[i] = payload[32 + i] ^ mask[i];
    }
    let s_addr = Address(s);
    let sender = lookup(&s_addr).ok_or(CryptoError::UnknownSender(s_addr))?;
    if sender.address() != s_addr {
        return Err(CryptoError::Decrypt);
    }
    let ss = me.dh.diffie_hellman(&XPublic::from(sender.dh));
    let key = message_key(es.as_bytes(), ss.as_bytes(), &e_pub, &my_addr, &s_addr);
    out[..n].copy_from_slice(&payload[48..48 + n]);
    let tag = Tag::from_slice(&payload[48 + n..]);
    ChaCha20Poly1305::new(Key::from_slice(&key))
        .decrypt_in_place_detached(
            Nonce::from_slice(&[0u8; 12]),
            &payload[..48],
            &mut out[..n],
            tag,
        )
        .map_err(|_| CryptoError::Decrypt)?;
    Ok((s_addr, n))
}

fn channel_key(key: &[u8; 32]) -> [u8; 32] {
    let hk = Hkdf::<Sha256>::new(Some(b"maarse/channel/v0"), key);
    let mut k = [0u8; 32];
    let _ = hk.expand(b"message", &mut k);
    k
}

/// Encrypts a channel message. `nonce` must be 8 random bytes.
pub fn seal_channel(
    key: &[u8; 32],
    sender: &Address,
    nonce: [u8; 8],
    msg: &[u8],
    out: &mut [u8],
) -> Result<usize, CryptoError> {
    let total = msg.len() + CHANNEL_OVERHEAD;
    if out.len() < total {
        return Err(CryptoError::BufferTooSmall);
    }
    out[..8].copy_from_slice(&nonce);
    out[8..24].copy_from_slice(&sender.0);
    out[24..24 + msg.len()].copy_from_slice(msg);
    let mut n12 = [0u8; 12];
    n12[..8].copy_from_slice(&nonce);
    let (aad, rest) = out.split_at_mut(8);
    let body = &mut rest[..16 + msg.len()];
    let tag = ChaCha20Poly1305::new(Key::from_slice(&channel_key(key)))
        .encrypt_in_place_detached(Nonce::from_slice(&n12), aad, body)
        .map_err(|_| CryptoError::Malformed)?;
    rest[16 + msg.len()..32 + msg.len()].copy_from_slice(&tag);
    Ok(total)
}

/// Decrypts a channel message. Returns the (claimed) sender and length.
pub fn open_channel(
    key: &[u8; 32],
    payload: &[u8],
    out: &mut [u8],
) -> Result<(Address, usize), CryptoError> {
    if payload.len() < CHANNEL_OVERHEAD {
        return Err(CryptoError::Malformed);
    }
    let n = payload.len() - CHANNEL_OVERHEAD;
    if out.len() < 16 + n {
        return Err(CryptoError::BufferTooSmall);
    }
    let mut n12 = [0u8; 12];
    n12[..8].copy_from_slice(&payload[..8]);
    let body = &mut out[..16 + n];
    body.copy_from_slice(&payload[8..24 + n]);
    let tag = Tag::from_slice(&payload[24 + n..]);
    ChaCha20Poly1305::new(Key::from_slice(&channel_key(key)))
        .decrypt_in_place_detached(Nonce::from_slice(&n12), &payload[..8], body, tag)
        .map_err(|_| CryptoError::Decrypt)?;
    let sender = Address::from_slice(&out[..16]).ok_or(CryptoError::Malformed)?;
    out.copy_within(16..16 + n, 0);
    Ok((sender, n))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{hashtag_key, public_channel_key};
    use rand_core::OsRng;

    fn pair() -> (Identity, Identity) {
        (
            Identity::generate(&mut OsRng),
            Identity::generate(&mut OsRng),
        )
    }

    #[test]
    fn direct_roundtrip() {
        let (a, b) = pair();
        let a_pub = a.public();
        let mut ct = [0u8; 255];
        let n = seal_direct(&a, &b.public(), &mut OsRng, b"hi from A", &mut ct).unwrap();
        assert_eq!(n, 9 + DM_OVERHEAD);
        let mut pt = [0u8; 255];
        let (from, len) = open_direct(
            &b,
            &ct[..n],
            |x| (*x == a_pub.address()).then_some(a_pub),
            &mut pt,
        )
        .unwrap();
        assert_eq!(from, a.address());
        assert_eq!(&pt[..len], b"hi from A");
    }

    #[test]
    fn sender_is_hidden_and_each_message_differs() {
        let (a, b) = pair();
        let mut c1 = [0u8; 100];
        let mut c2 = [0u8; 100];
        let n = seal_direct(&a, &b.public(), &mut OsRng, b"x", &mut c1).unwrap();
        seal_direct(&a, &b.public(), &mut OsRng, b"x", &mut c2).unwrap();
        assert_ne!(c1[..n], c2[..n]);
        let addr = a.address().0;
        assert!(!c1[..n].windows(16).any(|w| w == addr));
    }

    #[test]
    fn unknown_sender_is_reported() {
        let (a, b) = pair();
        let mut ct = [0u8; 100];
        let n = seal_direct(&a, &b.public(), &mut OsRng, b"x", &mut ct).unwrap();
        let mut pt = [0u8; 100];
        assert_eq!(
            open_direct(&b, &ct[..n], |_| None, &mut pt),
            Err(CryptoError::UnknownSender(a.address()))
        );
    }

    #[test]
    fn wrong_recipient_or_tampering_fails() {
        let (a, b) = pair();
        let c = Identity::generate(&mut OsRng);
        let a_pub = a.public();
        let look = |x: &Address| (*x == a_pub.address()).then_some(a_pub);
        let mut ct = [0u8; 100];
        let n = seal_direct(&a, &b.public(), &mut OsRng, b"hemmelig", &mut ct).unwrap();
        let mut pt = [0u8; 100];
        assert!(open_direct(&c, &ct[..n], look, &mut pt).is_err());
        for i in [0, 33, 50, n - 1] {
            let mut bad = ct;
            bad[i] ^= 1;
            assert!(
                open_direct(&b, &bad[..n], look, &mut pt).is_err(),
                "byte {i}"
            );
        }
    }

    #[test]
    fn impersonation_fails() {
        // C knows B's public key and claims to be A.
        let (a, b) = pair();
        let c = Identity::generate(&mut OsRng);
        let a_pub = a.public();
        let mut ct = [0u8; 100];
        let n = seal_direct(&c, &b.public(), &mut OsRng, b"I am A", &mut ct).unwrap();
        // Swap the hidden sender for A's address.
        let mut pt = [0u8; 100];
        let e_pub: [u8; 32] = ct[..32].try_into().unwrap();
        let es = b.dh.diffie_hellman(&XPublic::from(e_pub));
        let mask = mask_key(es.as_bytes(), &e_pub, &b.address());
        for i in 0..16 {
            ct[32 + i] = a.address().0[i] ^ mask[i];
        }
        let look = |x: &Address| (*x == a_pub.address()).then_some(a_pub);
        assert_eq!(
            open_direct(&b, &ct[..n], look, &mut pt),
            Err(CryptoError::Decrypt)
        );
    }

    #[test]
    fn channel_roundtrip_and_wrong_key() {
        let key = hashtag_key("#test");
        let me = Address([3; 16]);
        let mut ct = [0u8; 100];
        let n = seal_channel(&key, &me, [9; 8], b"hi channel", &mut ct).unwrap();
        let mut pt = [0u8; 100];
        let (from, len) = open_channel(&key, &ct[..n], &mut pt).unwrap();
        assert_eq!((from, &pt[..len]), (me, &b"hi channel"[..]));
        assert_eq!(
            open_channel(&public_channel_key(), &ct[..n], &mut pt),
            Err(CryptoError::Decrypt)
        );
    }

    #[test]
    fn max_text_fits_with_full_path() {
        let (a, b) = pair();
        let mut ct = [0u8; 255];
        let n = seal_direct(&a, &b.public(), &mut OsRng, &[b'x'; MAX_DM_TEXT], &mut ct).unwrap();
        assert_eq!(n, maarse_core::router::MAX_DIRECT_PAYLOAD);
    }
}

/// Overhead for one chunk of a file: file ID (8) + index (2) + tag (16).
pub const CHUNK_OVERHEAD: usize = 8 + 2 + 16;
/// Bytes of file data per chunk, so a chunk fits in one packet with a full path.
pub const CHUNK_DATA: usize = maarse_core::router::MAX_DIRECT_PAYLOAD - CHUNK_OVERHEAD;

fn chunk_nonce(file_id: u64, index: u16) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[..8].copy_from_slice(&file_id.to_le_bytes());
    n[8..10].copy_from_slice(&index.to_le_bytes());
    n
}

/// Encrypts chunk no. `index` of a file with the file's own key (which is sent
/// in the encrypted file offer).
pub fn seal_chunk(
    key: &[u8; 32],
    file_id: u64,
    index: u16,
    data: &[u8],
    out: &mut [u8],
) -> Result<usize, CryptoError> {
    let total = data.len() + CHUNK_OVERHEAD;
    if out.len() < total {
        return Err(CryptoError::BufferTooSmall);
    }
    out[..8].copy_from_slice(&file_id.to_le_bytes());
    out[8..10].copy_from_slice(&index.to_le_bytes());
    let (head, rest) = out.split_at_mut(10);
    rest[..data.len()].copy_from_slice(data);
    let tag = ChaCha20Poly1305::new(Key::from_slice(key))
        .encrypt_in_place_detached(
            Nonce::from_slice(&chunk_nonce(file_id, index)),
            head,
            &mut rest[..data.len()],
        )
        .map_err(|_| CryptoError::Malformed)?;
    rest[data.len()..data.len() + 16].copy_from_slice(&tag);
    Ok(total)
}

/// Reads file ID and index without decrypting (to find the right file).
pub fn chunk_header(payload: &[u8]) -> Option<(u64, u16)> {
    if payload.len() < CHUNK_OVERHEAD {
        return None;
    }
    let id = u64::from_le_bytes(payload[..8].try_into().ok()?);
    let idx = u16::from_le_bytes(payload[8..10].try_into().ok()?);
    Some((id, idx))
}

/// Decrypts a chunk. Returns the length of data in `out`.
pub fn open_chunk(key: &[u8; 32], payload: &[u8], out: &mut [u8]) -> Result<usize, CryptoError> {
    let (file_id, index) = chunk_header(payload).ok_or(CryptoError::Malformed)?;
    let n = payload.len() - CHUNK_OVERHEAD;
    if out.len() < n {
        return Err(CryptoError::BufferTooSmall);
    }
    out[..n].copy_from_slice(&payload[10..10 + n]);
    ChaCha20Poly1305::new(Key::from_slice(key))
        .decrypt_in_place_detached(
            Nonce::from_slice(&chunk_nonce(file_id, index)),
            &payload[..10],
            &mut out[..n],
            Tag::from_slice(&payload[10 + n..]),
        )
        .map_err(|_| CryptoError::Decrypt)?;
    Ok(n)
}

#[cfg(test)]
mod chunk_tests {
    use super::*;

    #[test]
    fn chunk_roundtrip_and_tamper() {
        let key = [7u8; 32];
        let mut ct = [0u8; 255];
        let n = seal_chunk(&key, 42, 3, &[0xab; CHUNK_DATA], &mut ct).unwrap();
        assert_eq!(n, maarse_core::router::MAX_DIRECT_PAYLOAD);
        assert_eq!(chunk_header(&ct[..n]), Some((42, 3)));
        let mut pt = [0u8; 255];
        assert_eq!(open_chunk(&key, &ct[..n], &mut pt), Ok(CHUNK_DATA));
        // A swapped index or a different key is rejected.
        let mut bad = ct;
        bad[8] = 4;
        assert!(open_chunk(&key, &bad[..n], &mut pt).is_err());
        assert!(open_chunk(&[8u8; 32], &ct[..n], &mut pt).is_err());
    }
}
