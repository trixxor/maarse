//! Signed bulletins: short notices from a trusted sender (e.g. an emergency
//! organisation) that anyone can verify offline.
//!
//! A bulletin is an ordinary channel message on the well-known channel
//! [`CHANNEL_NAME`], so it travels like any channel message (flood, regions,
//! repeaters' flash is not involved). Its plaintext is
//!
//! ```text
//! 0xB1 | timestamp (u32 BE, Unix seconds) | Ed25519 signature (64) | text
//! signature over: "maarse/bulletin/v1" | channel address (16) | timestamp | text
//! ```
//!
//! `0xB1` can never start valid UTF-8, so a bulletin is never mistaken for
//! plain text. The receiver checks the signature against public identities
//! it has been configured to trust ([`verify`]); the sender's announce is not
//! needed. Anyone who knows the channel name can post to it, so an
//! unverified message on the channel must not be shown as a bulletin.

use crate::{Identity, PublicIdentity, channel_address, hashtag_key};
use ed25519_dalek::{Signature, Signer, VerifyingKey};
use maarse_core::Address;
use sha2::{Digest, Sha256};

/// First plaintext byte of a bulletin.
pub const TAG: u8 = 0xB1;
/// Bytes a bulletin adds to its text.
pub const OVERHEAD: usize = 1 + 4 + 64;
/// The channel bulletins are sent on.
pub const CHANNEL_NAME: &str = "#bulletin";
const CONTEXT: &[u8] = b"maarse/bulletin/v1";

/// Key of the bulletin channel.
pub fn channel_key() -> [u8; 32] {
    hashtag_key(CHANNEL_NAME)
}

/// Address of the bulletin channel (the packets' destination).
pub fn channel() -> Address {
    channel_address(&channel_key())
}

fn digest(channel: &Address, timestamp: u32, text: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(CONTEXT);
    h.update(channel.0);
    h.update(timestamp.to_be_bytes());
    h.update(text);
    h.finalize().into()
}

/// Builds a bulletin's plaintext. `None` if `out` is too small.
pub fn sign(me: &Identity, timestamp: u32, text: &[u8], out: &mut [u8]) -> Option<usize> {
    let n = OVERHEAD + text.len();
    if out.len() < n {
        return None;
    }
    let sig = me.signing_key().sign(&digest(&channel(), timestamp, text));
    out[0] = TAG;
    out[1..5].copy_from_slice(&timestamp.to_be_bytes());
    out[5..69].copy_from_slice(&sig.to_bytes());
    out[69..n].copy_from_slice(text);
    Some(n)
}

/// Is this plaintext shaped like a bulletin (not yet verified)?
pub fn is_bulletin(plain: &[u8]) -> bool {
    plain.len() >= OVERHEAD && plain[0] == TAG
}

/// A verified bulletin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Verified<'a> {
    /// The trusted identity that signed it.
    pub signer: Address,
    pub timestamp: u32,
    pub text: &'a [u8],
}

/// Verifies a bulletin's plaintext. `sender` is the address from the channel
/// message; it must be one of the `trusted` identities, and the signature
/// must be that identity's.
pub fn verify<'a>(sender: &Address, plain: &'a [u8], trusted: &[PublicIdentity]) -> Option<Verified<'a>> {
    if !is_bulletin(plain) {
        return None;
    }
    let id = trusted.iter().find(|t| t.address() == *sender)?;
    let timestamp = u32::from_be_bytes(plain[1..5].try_into().ok()?);
    let sig = Signature::from_bytes(&plain[5..69].try_into().ok()?);
    let text = &plain[69..];
    let key = VerifyingKey::from_bytes(&id.sign).ok()?;
    key.verify_strict(&digest(&channel(), timestamp, text), &sig).ok()?;
    Some(Verified { signer: *sender, timestamp, text })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand_core::OsRng;

    #[test]
    fn round_trip_and_rejections() {
        let auth = Identity::generate(&mut OsRng);
        let other = Identity::generate(&mut OsRng);
        let mut buf = [0u8; 200];
        let n = sign(&auth, 1_700_000_000, b"Evacuate zone 3", &mut buf).unwrap();
        let plain = &buf[..n];
        let trusted = [auth.public()];

        let v = verify(&auth.address(), plain, &trusted).expect("verified");
        assert_eq!(v.text, b"Evacuate zone 3");
        assert_eq!(v.timestamp, 1_700_000_000);

        // Not trusted.
        assert!(verify(&auth.address(), plain, &[other.public()]).is_none());
        // Someone else claims to be the sender.
        assert!(verify(&other.address(), plain, &trusted).is_none());
        // Tampered text.
        let mut bad = plain.to_vec();
        *bad.last_mut().unwrap() ^= 1;
        assert!(verify(&auth.address(), &bad, &trusted).is_none());
        // Signed by someone else but sent under the trusted address.
        let n2 = sign(&other, 1_700_000_000, b"Evacuate zone 3", &mut buf).unwrap();
        assert!(verify(&auth.address(), &buf[..n2], &trusted).is_none());
        // Plain text is not a bulletin.
        assert!(!is_bulletin(b"hello there, this is a long enough plain text message to pass the length check..."));
    }
}
