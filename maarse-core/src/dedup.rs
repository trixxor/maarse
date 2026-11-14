//! Dublet-filter.
//!
//! En pakke genkendes på et 64-bit hash af pakketype, destination og payload.
//! Hop-tæller og sti er udeladt, fordi de ændrer sig undervejs, så samme
//! pakke modtaget ad to veje får samme ID.

use crate::addr::Address;
use crate::packet::PacketType;

/// 64-bit FNV-1a over pakkens faste indhold.
pub fn packet_id(ptype: PacketType, dest: &Address, payload: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h = OFFSET;
    let mut feed = |b: u8| {
        h ^= b as u64;
        h = h.wrapping_mul(PRIME);
    };
    feed(ptype as u8);
    dest.0.iter().copied().for_each(&mut feed);
    payload.iter().copied().for_each(&mut feed);
    h
}

/// Ringbuffer med de seneste `N` pakke-ID'er.
///
/// 8 bytes pr. ID: 2.000 ID'er fylder 16 KB.
pub struct DedupCache<const N: usize> {
    ids: [u64; N],
    next: usize,
    len: usize,
}

impl<const N: usize> Default for DedupCache<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> DedupCache<N> {
    pub const fn new() -> Self {
        DedupCache {
            ids: [0; N],
            next: 0,
            len: 0,
        }
    }

    pub fn contains(&self, id: u64) -> bool {
        self.ids[..self.len].contains(&id)
    }

    /// Indsætter ID'et. Returnerer `true` hvis det er nyt.
    pub fn insert(&mut self, id: u64) -> bool {
        if N == 0 || self.contains(id) {
            return false;
        }
        self.ids[self.next] = id;
        self.next = (self.next + 1) % N;
        self.len = (self.len + 1).min(N);
        true
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_ignores_nothing_it_should_include() {
        let a = Address([1; 16]);
        let b = Address([2; 16]);
        let base = packet_id(PacketType::Data, &a, b"x");
        assert_eq!(base, packet_id(PacketType::Data, &a, b"x"));
        assert_ne!(base, packet_id(PacketType::Ack, &a, b"x"));
        assert_ne!(base, packet_id(PacketType::Data, &b, b"x"));
        assert_ne!(base, packet_id(PacketType::Data, &a, b"y"));
    }

    #[test]
    fn detects_duplicates_and_evicts_oldest() {
        let mut c = DedupCache::<3>::new();
        assert!(c.insert(1));
        assert!(!c.insert(1));
        assert!(c.insert(2));
        assert!(c.insert(3));
        assert!(c.insert(4)); // smider 1 ud
        assert!(!c.contains(1));
        assert!(c.contains(4));
        assert_eq!(c.len(), 3);
    }
}
