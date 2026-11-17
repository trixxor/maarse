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
    fn link_words_are_stable_per_device() {
        let d = Identity::generate(&mut OsRng);
        assert_eq!(link_words(&d.public()), link_words(&d.public()));
        assert_ne!(link_words(&d.public()), link_words(&Identity::generate(&mut OsRng).public()));
    }
}
