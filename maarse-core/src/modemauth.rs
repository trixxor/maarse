//! Proof that a Wi-Fi LoRa modem belongs to the host it connects to.
//!
//! A node in Wi-Fi modem mode connects to a host's backbone port and sends
//! `MM`; the host would then treat it as its own radio. Anyone could do that,
//! so a host with a modem key asks first:
//!
//! ```text
//! host  -> modem: length | 0xFF 'M' 'K' | nonce (16)              (a challenge)
//! modem -> host:  length | 0x7F | 0xFF 'M' 'R' | tag (16)          (in the snr byte's place)
//! tag = first 16 bytes of SHA-256("maarse/modem-auth" | key | nonce)
//! key = SHA-256("maarse/modem-key" | passphrase)
//! ```
//!
//! The passphrase itself never crosses the network, and a recorded answer is
//! worthless for the next (random) challenge.

use sha2::{Digest, Sha256};

/// Start of the host's challenge.
pub const CHALLENGE: [u8; 3] = [0xFF, b'M', b'K'];
/// Start of the modem's answer (after its 0x7F marker byte).
pub const RESPONSE: [u8; 3] = [0xFF, b'M', b'R'];
/// The marker in the modem's "snr" byte for an answer.
pub const RESPONSE_MARK: u8 = 0x7F;
pub const NONCE_LEN: usize = 16;
pub const TAG_LEN: usize = 16;

/// The key a passphrase gives (stored in the modem, kept by the host).
pub fn key(passphrase: &[u8]) -> [u8; 32] {
    Sha256::new().chain_update(b"maarse/modem-key").chain_update(passphrase).finalize().into()
}

/// The answer to a challenge.
pub fn tag(key: &[u8; 32], nonce: &[u8]) -> [u8; TAG_LEN] {
    let h: [u8; 32] = Sha256::new().chain_update(b"maarse/modem-auth").chain_update(key).chain_update(nonce).finalize().into();
    let mut t = [0u8; TAG_LEN];
    t.copy_from_slice(&h[..TAG_LEN]);
    t
}

/// Compares in constant time.
pub fn check(key: &[u8; 32], nonce: &[u8], answer: &[u8]) -> bool {
    let t = tag(key, nonce);
    answer.len() == TAG_LEN && t.iter().zip(answer).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answer_matches_only_the_right_key_and_nonce() {
        let k = key(b"long modem passphrase");
        let n = [7u8; NONCE_LEN];
        assert!(check(&k, &n, &tag(&k, &n)));
        assert!(!check(&key(b"other"), &n, &tag(&k, &n)));
        assert!(!check(&k, &[8u8; NONCE_LEN], &tag(&k, &n)));
        assert!(!check(&k, &n, &tag(&k, &n)[..15]));
    }
}
