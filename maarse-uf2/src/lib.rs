//! Shared code for `maarse-uf2` (ELF -> UF2) and `maarse-sign` (signed
//! firmware updates).
#![forbid(unsafe_code)]

use std::collections::BTreeMap;

const MAGIC_START0: u32 = 0x0A32_4655;
const MAGIC_START1: u32 = 0x9E5D_5157;
const MAGIC_END: u32 = 0x0AB1_6F30;
const FLAG_FAMILY: u32 = 0x0000_2000;
pub const PAYLOAD: usize = 256;

pub fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
pub fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// Gets (physical address, data) for all LOAD segments with content.
pub fn load_segments(elf: &[u8]) -> Result<Vec<(u32, Vec<u8>)>, String> {
    if elf.len() < 52 || &elf[..4] != b"\x7fELF" || elf[4] != 1 || elf[5] != 1 {
        return Err("not a 32-bit little-endian ELF file".into());
    }
    let phoff = u32le(elf, 28) as usize;
    let phentsize = u16le(elf, 42) as usize;
    let phnum = u16le(elf, 44) as usize;
    let mut segs = Vec::new();
    for i in 0..phnum {
        let h = phoff + i * phentsize;
        let p_type = u32le(elf, h);
        let offset = u32le(elf, h + 4) as usize;
        let paddr = u32le(elf, h + 12);
        let filesz = u32le(elf, h + 16) as usize;
        if p_type == 1 && filesz > 0 {
            let data = elf
                .get(offset..offset + filesz)
                .ok_or("segment outside the file")?;
            segs.push((paddr, data.to_vec()));
        }
    }
    if segs.is_empty() {
        return Err("no data in the ELF file".into());
    }
    Ok(segs)
}

pub type Pages = BTreeMap<u32, [u8; PAYLOAD]>;

/// The segments spread over 256-byte pages (unused bytes are 0xFF).
pub fn pages(segs: &[(u32, Vec<u8>)]) -> Pages {
    let mut pages = Pages::new();
    for (addr, data) in segs {
        for (i, b) in data.iter().enumerate() {
            let a = addr + i as u32;
            let page = a & !(PAYLOAD as u32 - 1);
            pages.entry(page).or_insert([0xFF; PAYLOAD])[(a - page) as usize] = *b;
        }
    }
    pages
}

/// Reads the pages out of a UF2 file.
pub fn uf2_pages(uf2: &[u8]) -> Result<Pages, String> {
    if uf2.is_empty() || uf2.len() % 512 != 0 {
        return Err("not a UF2 file (the length is not a multiple of 512)".into());
    }
    let mut segs = Vec::new();
    for blk in uf2.chunks(512) {
        if u32le(blk, 0) != MAGIC_START0 || u32le(blk, 4) != MAGIC_START1 || u32le(blk, 508) != MAGIC_END {
            return Err("invalid UF2 block".into());
        }
        let addr = u32le(blk, 12);
        let n = u32le(blk, 16) as usize;
        if n > 476 {
            return Err("UF2 block with too much data".into());
        }
        segs.push((addr, blk[32..32 + n].to_vec()));
    }
    Ok(pages(&segs))
}

/// The canonical firmware image for over-the-air updates: all pages from
/// the lowest up to and including the highest, gaps filled with 0xFF. The same rule
/// is used by the web client when it gets a .uf2 file.
pub fn flatten(pages: &Pages) -> Option<(u32, Vec<u8>)> {
    let start = *pages.keys().next()?;
    let end = *pages.keys().last()? + PAYLOAD as u32;
    let mut out = vec![0xFF; (end - start) as usize];
    for (a, d) in pages {
        let o = (a - start) as usize;
        out[o..o + PAYLOAD].copy_from_slice(d);
    }
    Some((start, out))
}

/// Builds UF2 blocks of 256 bytes, aligned to 256-byte boundaries.
pub fn to_uf2(segs: &[(u32, Vec<u8>)], family: u32) -> Vec<u8> {
    let pages = pages(segs);
    let total = pages.len() as u32;
    let mut out = Vec::with_capacity(pages.len() * 512);
    for (n, (addr, data)) in pages.iter().enumerate() {
        let mut blk = [0u8; 512];
        let words = [
            MAGIC_START0,
            MAGIC_START1,
            FLAG_FAMILY,
            *addr,
            PAYLOAD as u32,
            n as u32,
            total,
            family,
        ];
        for (i, w) in words.iter().enumerate() {
            blk[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
        }
        blk[32..32 + PAYLOAD].copy_from_slice(data);
        blk[508..512].copy_from_slice(&MAGIC_END.to_le_bytes());
        out.extend_from_slice(&blk);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_are_aligned_and_complete() {
        let segs = vec![(0x27000, vec![1u8; 300]), (0x27200, vec![2u8; 10])];
        let uf2 = to_uf2(&segs, 0xADA52840);
        assert_eq!(uf2.len(), 3 * 512);
        assert_eq!(u32le(&uf2, 12), 0x27000);
        assert_eq!(u32le(&uf2, 512 + 12), 0x27100);
        assert_eq!(u32le(&uf2, 1024 + 12), 0x27200);
        assert_eq!(u32le(&uf2, 24), 3);
        assert_eq!(uf2[512 + 32 + 43], 1); // byte 299 of the first segment
        assert_eq!(uf2[512 + 32 + 44], 0xFF);
        // UF2 -> pages -> flat image gives the same as from the segments.
        let back = uf2_pages(&uf2).unwrap();
        assert_eq!(back, pages(&segs));
        let (start, img) = flatten(&back).unwrap();
        assert_eq!(start, 0x27000);
        assert_eq!(img.len(), 3 * 256);
        assert_eq!(img[299], 1);
        assert_eq!(img[300], 0xFF);
        assert_eq!(img[0x200], 2);
    }
}
