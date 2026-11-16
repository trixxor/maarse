//! A small Bloom filter of addresses: who is behind a trunk (packet type
//! `Summary`). 1024 bits, 3 hashes: about 2 % false positives at 100
//! addresses, 14 % at 250 (a false positive only lets one request cross).

use crate::Address;
use crate::dedup::packet_id;
use crate::packet::PacketType;

pub const BYTES: usize = 128;
const BITS: u64 = BYTES as u64 * 8;

/// splitmix64's finalizer: FNV-1a's low bits depend only on low bits, so
/// the hash is mixed before bits are taken from it.
fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

fn hashes(a: &Address) -> [usize; 3] {
    let h = mix(packet_id(PacketType::Summary, a, b"bloom"));
    [(h % BITS) as usize, ((h >> 21) % BITS) as usize, ((h >> 42) % BITS) as usize]
}

pub fn insert(bits: &mut [u8; BYTES], a: &Address) {
    for i in hashes(a) {
        bits[i / 8] |= 1 << (i % 8);
    }
}

pub fn contains(bits: &[u8], a: &Address) -> bool {
    bits.len() >= BYTES && hashes(a).iter().all(|&i| bits[i / 8] & (1 << (i % 8)) != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn members_are_found() {
        let mut b = [0u8; BYTES];
        for i in 0..100u8 {
            insert(&mut b, &Address([i; 16]));
        }
        assert!((0..100u8).all(|i| contains(&b, &Address([i; 16]))));
        let false_pos = (100..=255u8).filter(|&i| contains(&b, &Address([i; 16]))).count();
        assert!(false_pos < 10, "{false_pos} false positives");
    }
}
