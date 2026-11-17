//! Linked devices: one person (master identity M), several devices.
//!
//! See `docs/maybe/multi-device.md`. This module holds the signed objects:
//!
//! * **Device certificate**: M says "device D is my device number i".
//!   No expiry; it ends only when a newer device list revokes it.
//!
//!   ```text
//!   signed: "maarse/device/v1" | M address (16) | device address (16) | index (1)
//!   wire:   M public identity (64) | index (1) | signature (64)        = 129 bytes
//!   ```
//!
//!   The device address is not on the wire: it is the address of whoever
//!   presents the certificate (the sender of the message, the announcer), so
//!   a certificate cannot be replayed for another device.
//!
//! * **Device list**: every device of M, and the revoked ones. The highest
//!   version wins.
//!
//!   ```text
//!   signed: "maarse/devlist/v1" | M address (16) | wire without the signature
//!   wire:   version (u32 LE) | count (1) | [address (16) | flags (1)]… | signature (64)
//!   flags:  bit 0 = revoked
//!   ```
//!
//! The master secret signs both. Devices keep their own identity for routing,
//! announces and encryption; the master secret stays on one device (and on
//! paper as the 24 words, see [`crate::recovery`]).

use crate::{Identity, PublicIdentity, PUBLIC_IDENTITY_LEN, SIGNATURE_LEN};
use ed25519_dalek::{Signature, Signer, Verifier, VerifyingKey};
use maarse_core::Address;

const CERT_CONTEXT: &[u8] = b"maarse/device/v1";
const LIST_CONTEXT: &[u8] = b"maarse/devlist/v1";

pub const CERT_LEN: usize = PUBLIC_IDENTITY_LEN + 1 + SIGNATURE_LEN;
/// Devices (including revoked ones) a list can hold: it must fit in one
/// encrypted direct message.
pub const MAX_DEVICES: usize = 4;
pub const LIST_MAX_LEN: usize = 4 + 1 + MAX_DEVICES * 17 + SIGNATURE_LEN;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PersonError {
    Malformed,
    BadSignature,
    /// The certificate is for another device than the one presenting it.
    WrongDevice,
    TooManyDevices,
}

/// A verified device certificate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceCert {
    pub master: PublicIdentity,
    pub device: Address,
    pub index: u8,
    pub signature: [u8; SIGNATURE_LEN],
}

fn cert_message(master: &Address, device: &Address, index: u8) -> [u8; 16 + 16 + 16 + 1] {
    let mut m = [0u8; 49];
    m[..16].copy_from_slice(CERT_CONTEXT);
    m[16..32].copy_from_slice(&master.0);
    m[32..48].copy_from_slice(&device.0);
    m[48] = index;
    m
}

impl DeviceCert {
    /// The master signs a certificate for `device`.
    pub fn issue(master: &Identity, device: &Address, index: u8) -> Self {
        let mp = master.public();
        let sig = master.signing_key().sign(&cert_message(&mp.address(), device, index));
        DeviceCert { master: mp, device: *device, index, signature: sig.to_bytes() }
    }

    pub fn person(&self) -> Address {
        self.master.address()
    }

    pub fn to_bytes(&self) -> [u8; CERT_LEN] {
        let mut b = [0u8; CERT_LEN];
        b[..64].copy_from_slice(&self.master.to_bytes());
        b[64] = self.index;
        b[65..].copy_from_slice(&self.signature);
        b
    }

    /// Reads and verifies a certificate presented by `device`.
    pub fn verify(device: &Address, b: &[u8]) -> Result<Self, PersonError> {
        if b.len() < CERT_LEN {
            return Err(PersonError::Malformed);
        }
        let master = PublicIdentity::from_bytes(&b[..64]).ok_or(PersonError::Malformed)?;
        let index = b[64];
        let signature: [u8; 64] = b[65..CERT_LEN].try_into().map_err(|_| PersonError::Malformed)?;
        let vk = VerifyingKey::from_bytes(&master.sign).map_err(|_| PersonError::Malformed)?;
        vk.verify(&cert_message(&master.address(), device, index), &Signature::from_bytes(&signature))
            .map_err(|_| PersonError::WrongDevice)?;
        Ok(DeviceCert { master, device: *device, index, signature })
    }
}

/// One entry of a device list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceEntry {
    pub addr: Address,
    pub revoked: bool,
}

/// A device list (verified when read with [`DeviceList::verify`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceList {
    pub version: u32,
    pub devices: heapless::Vec<DeviceEntry, MAX_DEVICES>,
    pub signature: [u8; SIGNATURE_LEN],
}

impl DeviceList {
    fn body(version: u32, devices: &[DeviceEntry]) -> heapless::Vec<u8, LIST_MAX_LEN> {
        let mut v = heapless::Vec::new();
        let _ = v.extend_from_slice(&version.to_le_bytes());
        let _ = v.push(devices.len() as u8);
        for d in devices {
            let _ = v.extend_from_slice(&d.addr.0);
            let _ = v.push(d.revoked as u8);
        }
        v
    }

    fn signed(person: &Address, body: &[u8]) -> heapless::Vec<u8, { 17 + 16 + LIST_MAX_LEN }> {
        let mut m = heapless::Vec::new();
        let _ = m.extend_from_slice(LIST_CONTEXT);
        let _ = m.extend_from_slice(&person.0);
        let _ = m.extend_from_slice(body);
        m
    }

    /// The master signs a list.
    pub fn issue(master: &Identity, version: u32, devices: &[DeviceEntry]) -> Result<Self, PersonError> {
        if devices.len() > MAX_DEVICES {
            return Err(PersonError::TooManyDevices);
        }
        let body = Self::body(version, devices);
        let sig = master.signing_key().sign(&Self::signed(&master.address(), &body));
        Ok(DeviceList {
            version,
            devices: heapless::Vec::from_slice(devices).map_err(|_| PersonError::TooManyDevices)?,
            signature: sig.to_bytes(),
        })
    }

    pub fn to_bytes(&self) -> heapless::Vec<u8, LIST_MAX_LEN> {
        let mut v = Self::body(self.version, &self.devices);
        let _ = v.extend_from_slice(&self.signature);
        v
    }

    /// Reads a list and checks it was signed by `master`.
    pub fn verify(master: &PublicIdentity, b: &[u8]) -> Result<Self, PersonError> {
        if b.len() < 5 + SIGNATURE_LEN {
            return Err(PersonError::Malformed);
        }
        let version = u32::from_le_bytes(b[..4].try_into().map_err(|_| PersonError::Malformed)?);
        let n = b[4] as usize;
        if n > MAX_DEVICES {
            return Err(PersonError::TooManyDevices);
        }
        let end = 5 + 17 * n;
        if b.len() < end + SIGNATURE_LEN {
            return Err(PersonError::Malformed);
        }
        let mut devices = heapless::Vec::new();
        for i in 0..n {
            let o = 5 + 17 * i;
            let addr = Address::from_slice(&b[o..o + 16]).ok_or(PersonError::Malformed)?;
            let _ = devices.push(DeviceEntry { addr, revoked: b[o + 16] & 1 != 0 });
        }
        let signature: [u8; 64] = b[end..end + SIGNATURE_LEN].try_into().map_err(|_| PersonError::Malformed)?;
        let vk = VerifyingKey::from_bytes(&master.sign).map_err(|_| PersonError::Malformed)?;
        vk.verify(&Self::signed(&master.address(), &b[..end]), &Signature::from_bytes(&signature))
            .map_err(|_| PersonError::BadSignature)?;
        Ok(DeviceList { version, devices, signature })
    }

    /// Devices that are not revoked.
    pub fn active(&self) -> impl Iterator<Item = Address> + '_ {
        self.devices.iter().filter(|d| !d.revoked).map(|d| d.addr)
    }

    pub fn is_revoked(&self, a: &Address) -> bool {
        self.devices.iter().any(|d| d.addr == *a && d.revoked)
    }
}

// ---------- inbox key: a weekly hash chain ----------

/// First byte of a message encrypted to a person's inbox (next to the
/// ratchet's `0xF0`/`0xF1`).
pub const INBOX: u8 = 0xF2;
/// A week's inbox secret.
pub use x25519_dalek::StaticSecret as InboxSecret;
/// type (1) | chain (1) | week (u16 LE) | ephemeral key (32) | sender ⊕ mask (16) | … | tag (16)
pub const INBOX_HEADER: usize = 1 + 1 + 2 + 32 + 16;
pub const INBOX_OVERHEAD: usize = INBOX_HEADER + 16;
const WEEK_S: u32 = 7 * 24 * 3600;

/// The week number of a Unix time (weeks since 1970).
pub fn week_of(unix: u32) -> u16 {
    (unix / WEEK_S) as u16
}

/// The inbox chain at one week:
///
/// ```text
/// K(n+1)            = HKDF-SHA256(K(n), info "maarse/inbox/next")
/// X25519 secret (n) = HKDF-SHA256(K(n), info "maarse/inbox/x25519")
/// ```
///
/// A device that knows K(n) derives every later week; nobody can go back.
/// `chain` changes when a device is revoked (a new random root).
#[derive(Clone, PartialEq, Eq)]
pub struct InboxChain {
    pub chain: u8,
    pub week: u16,
    pub k: [u8; 32],
}

impl core::fmt::Debug for InboxChain {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "InboxChain {{ chain: {}, week: {} }}", self.chain, self.week)
    }
}

pub const INBOX_STATE_LEN: usize = 1 + 2 + 32;

impl InboxChain {
    pub fn new(rng: &mut impl rand_core::CryptoRngCore, chain: u8, week: u16) -> Self {
        let mut k = [0u8; 32];
        rng.fill_bytes(&mut k);
        InboxChain { chain, week, k }
    }

    fn expand(k: &[u8; 32], info: &[u8]) -> [u8; 32] {
        let hk = hkdf::Hkdf::<sha2::Sha256>::new(None, k);
        let mut o = [0u8; 32];
        let _ = hk.expand(info, &mut o);
        o
    }

    /// The chain at a later week (`None` for an earlier one).
    pub fn at(&self, week: u16) -> Option<InboxChain> {
        if week < self.week || week - self.week > 520 {
            return None;
        }
        let mut k = self.k;
        for _ in self.week..week {
            k = Self::expand(&k, b"maarse/inbox/next");
        }
        Some(InboxChain { chain: self.chain, week, k })
    }

    pub fn secret(&self) -> x25519_dalek::StaticSecret {
        x25519_dalek::StaticSecret::from(Self::expand(&self.k, b"maarse/inbox/x25519"))
    }

    pub fn public(&self) -> [u8; 32] {
        x25519_dalek::PublicKey::from(&self.secret()).to_bytes()
    }

    pub fn to_bytes(&self) -> [u8; INBOX_STATE_LEN] {
        let mut b = [0u8; INBOX_STATE_LEN];
        b[0] = self.chain;
        b[1..3].copy_from_slice(&self.week.to_le_bytes());
        b[3..].copy_from_slice(&self.k);
        b
    }

    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        Some(InboxChain { chain: *b.first()?, week: u16::from_le_bytes([*b.get(1)?, *b.get(2)?]), k: b.get(3..35)?.try_into().ok()? })
    }
}

/// What a sender needs to write to a person's inbox this week.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InboxPublic {
    pub chain: u8,
    pub week: u16,
    pub key: [u8; 32],
}

impl InboxPublic {
    pub fn to_bytes(&self) -> [u8; 35] {
        let mut b = [0u8; 35];
        b[0] = self.chain;
        b[1..3].copy_from_slice(&self.week.to_le_bytes());
        b[3..].copy_from_slice(&self.key);
        b
    }
    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        Some(InboxPublic { chain: *b.first()?, week: u16::from_le_bytes([*b.get(1)?, *b.get(2)?]), key: b.get(3..35)?.try_into().ok()? })
    }
}

/// (mask for the sender's address, message key). The mask depends only on
/// the ephemeral DH, the header and the person, so the receiver can undo it
/// before it knows who the sender is.
fn inbox_keys(es: &[u8; 32], ss: &[u8; 32], header: &[u8], person: &Address, sender: &Address) -> ([u8; 16], [u8; 32]) {
    let mut info = [0u8; 36 + 16 + 16];
    info[..36].copy_from_slice(&header[..36]);
    info[36..52].copy_from_slice(&person.0);
    let hm = hkdf::Hkdf::<sha2::Sha256>::new(Some(b"maarse/inbox/v1/sender"), es);
    let mut mask = [0u8; 16];
    let _ = hm.expand(&info[..52], &mut mask);
    info[52..].copy_from_slice(&sender.0);
    let mut ikm = [0u8; 64];
    ikm[..32].copy_from_slice(es);
    ikm[32..].copy_from_slice(ss);
    let hk = hkdf::Hkdf::<sha2::Sha256>::new(Some(b"maarse/inbox/v1"), &ikm);
    let mut k = [0u8; 32];
    let _ = hk.expand(&info, &mut k);
    (mask, k)
}

/// Encrypts `msg` to `person`'s inbox. Like the static direct format, but to
/// a key every device of the person holds. Returns the length.
pub fn seal_inbox(
    sender: &Identity,
    person: &Address,
    inbox: &InboxPublic,
    rng: &mut impl rand_core::CryptoRngCore,
    msg: &[u8],
    out: &mut [u8],
) -> Result<usize, crate::seal::CryptoError> {
    use crate::seal::CryptoError;
    use chacha20poly1305::aead::AeadInPlace;
    use chacha20poly1305::{ChaCha20Poly1305, Key, KeyInit, Nonce};
    let total = msg.len() + INBOX_OVERHEAD;
    if out.len() < total {
        return Err(CryptoError::BufferTooSmall);
    }
    let r = x25519_dalek::PublicKey::from(inbox.key);
    let e = x25519_dalek::EphemeralSecret::random_from_rng(rng);
    let e_pub = x25519_dalek::PublicKey::from(&e).to_bytes();
    let es = e.diffie_hellman(&r);
    let ss = sender.dh.diffie_hellman(&r);
    if !es.was_contributory() || !ss.was_contributory() {
        return Err(CryptoError::Malformed);
    }
    let me = sender.address();
    out[0] = INBOX;
    out[1] = inbox.chain;
    out[2..4].copy_from_slice(&inbox.week.to_le_bytes());
    out[4..36].copy_from_slice(&e_pub);
    let (mask, key) = inbox_keys(es.as_bytes(), ss.as_bytes(), &out[..36], person, &me);
    for i in 0..16 {
        out[36 + i] = me.0[i] ^ mask[i];
    }
    let (head, rest) = out.split_at_mut(INBOX_HEADER);
    rest[..msg.len()].copy_from_slice(msg);
    let tag = ChaCha20Poly1305::new(Key::from_slice(&key))
        .encrypt_in_place_detached(Nonce::from_slice(&[0u8; 12]), head, &mut rest[..msg.len()])
        .map_err(|_| CryptoError::Malformed)?;
    rest[msg.len()..msg.len() + 16].copy_from_slice(&tag);
    Ok(total)
}

/// The chain and week of an inbox message, to pick the key.
pub fn inbox_header(payload: &[u8]) -> Option<(u8, u16)> {
    (payload.len() >= INBOX_OVERHEAD && payload[0] == INBOX).then(|| (payload[1], u16::from_le_bytes([payload[2], payload[3]])))
}

/// Opens an inbox message with the week's secret. Returns sender and length.
pub fn open_inbox(
    secret: &x25519_dalek::StaticSecret,
    person: &Address,
    payload: &[u8],
    lookup: impl Fn(&Address) -> Option<PublicIdentity>,
    out: &mut [u8],
) -> Result<(Address, usize), crate::seal::CryptoError> {
    use crate::seal::CryptoError;
    use chacha20poly1305::aead::AeadInPlace;
    use chacha20poly1305::{ChaCha20Poly1305, Key, KeyInit, Nonce, Tag};
    if inbox_header(payload).is_none() {
        return Err(CryptoError::Malformed);
    }
    let n = payload.len() - INBOX_OVERHEAD;
    if out.len() < n {
        return Err(CryptoError::BufferTooSmall);
    }
    let e_pub: [u8; 32] = payload[4..36].try_into().map_err(|_| CryptoError::Malformed)?;
    let es = secret.diffie_hellman(&x25519_dalek::PublicKey::from(e_pub));
    if !es.was_contributory() {
        return Err(CryptoError::Decrypt);
    }
    let (mask, _) = inbox_keys(es.as_bytes(), &[0; 32], &payload[..36], person, &Address([0; 16]));
    let mut s = [0u8; 16];
    for i in 0..16 {
        s[i] = payload[36 + i] ^ mask[i];
    }
    let s_addr = Address(s);
    let sender = lookup(&s_addr).ok_or(CryptoError::UnknownSender(s_addr))?;
    if sender.address() != s_addr {
        return Err(CryptoError::Decrypt);
    }
    let ss = secret.diffie_hellman(&x25519_dalek::PublicKey::from(sender.dh));
    let (_, key) = inbox_keys(es.as_bytes(), ss.as_bytes(), &payload[..36], person, &s_addr);
    out[..n].copy_from_slice(&payload[INBOX_HEADER..INBOX_HEADER + n]);
    ChaCha20Poly1305::new(Key::from_slice(&key))
        .decrypt_in_place_detached(
            Nonce::from_slice(&[0u8; 12]),
            &payload[..INBOX_HEADER],
            &mut out[..n],
            Tag::from_slice(&payload[INBOX_HEADER + n..]),
        )
        .map_err(|_| CryptoError::Decrypt)?;
    Ok((s_addr, n))
}

// ---------- backup of the master (`.maarse-account`, v2) ----------

/// File header.
pub const BACKUP_MAGIC: &[u8; 4] = b"MAB2";
/// PBKDF2-HMAC-SHA256 rounds. With a 6-word code (66 bits) this is about
/// 2^85 work for someone who steals the file, with no server to stop guesses.
pub const BACKUP_ROUNDS: u32 = 600_000;
pub const BACKUP_OVERHEAD: usize = 4 + 16 + 12 + 16;
/// Words in a backup code.
pub const CODE_WORDS: usize = 6;

/// A backup code: 6 words from the BIP39 list (66 random bits).
pub fn backup_code(rng: &mut impl rand_core::CryptoRngCore) -> heapless::String<64> {
    let mut b = [0u8; 9];
    rng.fill_bytes(&mut b);
    let bits = u128::from_be_bytes([0, 0, 0, 0, 0, 0, 0, b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8]]);
    let mut s = heapless::String::new();
    for i in 0..CODE_WORDS {
        let idx = ((bits >> (11 * i)) & 0x7ff) as u16;
        if i > 0 {
            let _ = s.push(' ');
        }
        let _ = s.push_str(crate::recovery::word(idx));
    }
    s
}

/// Normalises a typed code: lowercase, single spaces.
pub fn normalise_code(code: &str, out: &mut heapless::String<128>) {
    out.clear();
    for (i, w) in code.split_whitespace().enumerate() {
        if i > 0 {
            let _ = out.push(' ');
        }
        for c in w.chars() {
            let _ = out.push(c.to_ascii_lowercase());
        }
    }
}

fn backup_key(code: &str, salt: &[u8; 16]) -> [u8; 32] {
    use hmac::{Hmac, Mac};
    type H = Hmac<sha2::Sha256>;
    // PBKDF2 with one output block (32 bytes).
    let mut code_n = heapless::String::<128>::new();
    normalise_code(code, &mut code_n);
    let mac = H::new_from_slice(code_n.as_bytes()).expect("HMAC takes any key");
    let mut m = mac.clone();
    m.update(salt);
    m.update(&1u32.to_be_bytes());
    let mut u: [u8; 32] = m.finalize().into_bytes().into();
    let mut t = u;
    for _ in 1..BACKUP_ROUNDS {
        let mut m = mac.clone();
        m.update(&u);
        u = m.finalize().into_bytes().into();
        for i in 0..32 {
            t[i] ^= u[i];
        }
    }
    t
}

/// Encrypts `plain` with the code: magic | salt | nonce | ChaCha20-Poly1305.
pub fn backup_seal(code: &str, rng: &mut impl rand_core::CryptoRngCore, plain: &[u8], out: &mut [u8]) -> Option<usize> {
    use chacha20poly1305::aead::AeadInPlace;
    use chacha20poly1305::{ChaCha20Poly1305, Key, KeyInit, Nonce};
    let n = plain.len() + BACKUP_OVERHEAD;
    if out.len() < n {
        return None;
    }
    let mut salt = [0u8; 16];
    let mut nonce = [0u8; 12];
    rng.fill_bytes(&mut salt);
    rng.fill_bytes(&mut nonce);
    let key = backup_key(code, &salt);
    out[..4].copy_from_slice(BACKUP_MAGIC);
    out[4..20].copy_from_slice(&salt);
    out[20..32].copy_from_slice(&nonce);
    let (head, rest) = out.split_at_mut(32);
    rest[..plain.len()].copy_from_slice(plain);
    let tag = ChaCha20Poly1305::new(Key::from_slice(&key))
        .encrypt_in_place_detached(Nonce::from_slice(&nonce), &head[..4], &mut rest[..plain.len()])
        .ok()?;
    rest[plain.len()..plain.len() + 16].copy_from_slice(&tag);
    Some(n)
}

/// Opens a backup with the code. `None` for a wrong code or a damaged file.
pub fn backup_open(code: &str, file: &[u8], out: &mut [u8]) -> Option<usize> {
    use chacha20poly1305::aead::AeadInPlace;
    use chacha20poly1305::{ChaCha20Poly1305, Key, KeyInit, Nonce, Tag};
    if file.len() < BACKUP_OVERHEAD || &file[..4] != BACKUP_MAGIC {
        return None;
    }
    let n = file.len() - BACKUP_OVERHEAD;
    if out.len() < n {
        return None;
    }
    let salt: [u8; 16] = file[4..20].try_into().ok()?;
    let key = backup_key(code, &salt);
    out[..n].copy_from_slice(&file[32..32 + n]);
    ChaCha20Poly1305::new(Key::from_slice(&key))
        .decrypt_in_place_detached(Nonce::from_slice(&file[20..32]), &file[..4], &mut out[..n], Tag::from_slice(&file[32 + n..]))
        .ok()?;
    Some(n)
}

/// Short words the user compares on both screens when linking, so a device
/// is only added on purpose ("check that the new device shows the same").
pub fn link_words(device: &PublicIdentity) -> [&'static str; 4] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(b"maarse/link/v1");
    h.update(device.to_bytes());
    let d = h.finalize();
    core::array::from_fn(|i| crate::recovery::word(u16::from_be_bytes([d[2 * i], d[2 * i + 1]]) & 0x7ff))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand_core::OsRng;

    #[test]
    fn certificate_binds_the_device() {
        let m = Identity::generate(&mut OsRng);
        let d = Identity::generate(&mut OsRng);
        let other = Identity::generate(&mut OsRng);
        let c = DeviceCert::issue(&m, &d.address(), 1);
        let wire = c.to_bytes();
        let v = DeviceCert::verify(&d.address(), &wire).unwrap();
        assert_eq!(v.person(), m.address());
        assert_eq!(v.index, 1);
        // Another device cannot present it.
        assert_eq!(DeviceCert::verify(&other.address(), &wire), Err(PersonError::WrongDevice));
        // A changed index breaks the signature.
        let mut bad = wire;
        bad[64] = 2;
        assert!(DeviceCert::verify(&d.address(), &bad).is_err());
    }

    #[test]
    fn device_list_round_trips_and_is_signed() {
        let m = Identity::generate(&mut OsRng);
        let a = Identity::generate(&mut OsRng).address();
        let b = Identity::generate(&mut OsRng).address();
        let l = DeviceList::issue(&m, 3, &[DeviceEntry { addr: a, revoked: false }, DeviceEntry { addr: b, revoked: true }]).unwrap();
        let wire = l.to_bytes();
        let v = DeviceList::verify(&m.public(), &wire).unwrap();
        assert_eq!(v.version, 3);
        assert_eq!(v.active().collect::<Vec<_>>(), vec![a]);
        assert!(v.is_revoked(&b));
        // Signed by someone else: refused.
        let x = Identity::generate(&mut OsRng);
        assert_eq!(DeviceList::verify(&x.public(), &wire), Err(PersonError::BadSignature));
        // Tampered entry: refused.
        let mut bad = wire.clone();
        bad[5 + 17 + 16] = 0; // un-revoke the second device
        assert_eq!(DeviceList::verify(&m.public(), &bad), Err(PersonError::BadSignature));
        // A full list still fits in one FS-encrypted message with a type byte.
        assert!(LIST_MAX_LEN < crate::ratchet::MAX_FS_TEXT);
        assert!(CERT_LEN < crate::ratchet::MAX_FS_TEXT);
        let five = [DeviceEntry { addr: a, revoked: false }; 5];
        assert_eq!(DeviceList::issue(&m, 1, &five), Err(PersonError::TooManyDevices));
    }

    #[test]
    fn inbox_chain_goes_forward_only() {
        let c = InboxChain::new(&mut OsRng, 1, 100);
        let later = c.at(103).unwrap();
        // Two steps from week 101 give the same as three from 100.
        assert_eq!(c.at(101).unwrap().at(103).unwrap().k, later.k);
        assert!(later.at(102).is_none());
        assert_ne!(c.public(), later.public());
        assert_eq!(InboxChain::from_bytes(&later.to_bytes()), Some(later));
    }

    #[test]
    fn inbox_message_opens_with_the_week_key() {
        let sender = Identity::generate(&mut OsRng);
        let person = Identity::generate(&mut OsRng).address();
        let root = InboxChain::new(&mut OsRng, 2, 2900);
        let week = root.at(2902).unwrap();
        let p = InboxPublic { chain: 2, week: 2902, key: week.public() };
        let mut ct = [0u8; 255];
        let n = seal_inbox(&sender, &person, &p, &mut OsRng, b"hello person", &mut ct).unwrap();
        assert_eq!(n, 12 + INBOX_OVERHEAD);
        assert_eq!(inbox_header(&ct[..n]), Some((2, 2902)));
        // A device that only knew week 2900 derives the key itself.
        let lookup = |a: &Address| (*a == sender.address()).then(|| sender.public());
        let mut out = [0u8; 255];
        let (from, m) = open_inbox(&root.at(2902).unwrap().secret(), &person, &ct[..n], lookup, &mut out).unwrap();
        assert_eq!((from, &out[..m]), (sender.address(), &b"hello person"[..]));
        // Another week's key, or another person, fails.
        assert!(open_inbox(&root.at(2903).unwrap().secret(), &person, &ct[..n], lookup, &mut out).is_err());
        assert!(open_inbox(&week.secret(), &Address([1; 16]), &ct[..n], lookup, &mut out).is_err());
        // A changed byte fails.
        ct[60] ^= 1;
        assert!(open_inbox(&week.secret(), &person, &ct[..n], lookup, &mut out).is_err());
        // A full-length text fits in one packet.
        assert!(maarse_core::router::MAX_DIRECT_PAYLOAD - INBOX_OVERHEAD >= 143);
    }

    #[test]
    fn backup_opens_only_with_the_code() {
        let code = backup_code(&mut OsRng);
        assert_eq!(code.split(' ').count(), CODE_WORDS);
        let mut f = [0u8; 300];
        let n = backup_seal(&code, &mut OsRng, b"master secret", &mut f).unwrap();
        let mut out = [0u8; 300];
        // Typed in capitals with extra spaces: still fine.
        let typed = std::format!("  {}  ", code.to_uppercase().replace(' ', "   "));
        let m = backup_open(&typed, &f[..n], &mut out).unwrap();
        assert_eq!(&out[..m], b"master secret");
        assert!(backup_open("wrong code words here a b", &f[..n], &mut out).is_none());
        f[40] ^= 1;
        assert!(backup_open(&code, &f[..n], &mut out).is_none());
    }

    #[test]
    fn link_words_are_stable_per_device() {
        let d = Identity::generate(&mut OsRng);
        assert_eq!(link_words(&d.public()), link_words(&d.public()));
        assert_ne!(link_words(&d.public()), link_words(&Identity::generate(&mut OsRng).public()));
    }
}
