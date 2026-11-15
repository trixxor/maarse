//! Addresses and path IDs.

use core::fmt;

/// A Må:rse address: the first 16 bytes of a SHA-256 hash.
///
/// For a node it is the hash of its public keys. For a channel it is the
/// hash of the channel key. The address is the same on LoRa, TCP and backbone.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Address(pub [u8; 16]);

impl Address {
    pub const LEN: usize = 16;

    pub fn from_slice(bytes: &[u8]) -> Option<Self> {
        let arr: [u8; 16] = bytes.get(..16)?.try_into().ok()?;
        Some(Address(arr))
    }

    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// Path ID derived from the address: the first 2 bytes.
    ///
    /// Used by default for repeaters. The ID only has to be unique among
    /// a repeater's neighbours, not globally.
    pub fn path_id(&self) -> PathId {
        u16::from_be_bytes([self.0[0], self.0[1]])
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Short form in debug output: the first 4 bytes.
        write!(
            f,
            "Address({:02x}{:02x}{:02x}{:02x}…)",
            self.0[0], self.0[1], self.0[2], self.0[3]
        )
    }
}

/// 2-byte ID for a repeater in a source path.
pub type PathId = u16;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_is_full_hex() {
        let a = Address([0xab; 16]);
        assert_eq!(a.to_string(), "ab".repeat(16));
    }

    #[test]
    fn from_slice_needs_16_bytes() {
        assert!(Address::from_slice(&[1; 15]).is_none());
        assert_eq!(Address::from_slice(&[7; 20]), Some(Address([7; 16])));
    }

    #[test]
    fn path_id_is_first_two_bytes() {
        let mut b = [0u8; 16];
        b[0] = 0x12;
        b[1] = 0x34;
        assert_eq!(Address(b).path_id(), 0x1234);
    }
}
