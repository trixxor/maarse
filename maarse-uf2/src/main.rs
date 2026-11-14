//! maarse-uf2: laver en UF2-fil ud fra firmwarens ELF-fil, så den kan
//! trækkes over på boardets USB-drev. Ingen eksterne værktøjer.
//!
//!   maarse-uf2 <firmware.elf> <ud.uf2> [familie-id, standard 0xADA52840]

use std::fs;

const MAGIC_START0: u32 = 0x0A32_4655;
const MAGIC_START1: u32 = 0x9E5D_5157;
const MAGIC_END: u32 = 0x0AB1_6F30;
const FLAG_FAMILY: u32 = 0x0000_2000;
const PAYLOAD: usize = 256;

fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// Henter (fysisk adresse, data) for alle LOAD-segmenter med indhold.
fn load_segments(elf: &[u8]) -> Result<Vec<(u32, Vec<u8>)>, String> {
    if elf.len() < 52 || &elf[..4] != b"\x7fELF" || elf[4] != 1 || elf[5] != 1 {
        return Err("ikke en 32-bit little-endian ELF-fil".into());
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
                .ok_or("segment uden for filen")?;
            segs.push((paddr, data.to_vec()));
        }
    }
    if segs.is_empty() {
        return Err("ingen data i ELF-filen".into());
    }
    Ok(segs)
}

/// Bygger UF2-blokke på 256 bytes, justeret til 256-byte-grænser.
fn to_uf2(segs: &[(u32, Vec<u8>)], family: u32) -> Vec<u8> {
    use std::collections::BTreeMap;
    let mut pages: BTreeMap<u32, [u8; PAYLOAD]> = BTreeMap::new();
    for (addr, data) in segs {
        for (i, b) in data.iter().enumerate() {
            let a = addr + i as u32;
            let page = a & !(PAYLOAD as u32 - 1);
            pages.entry(page).or_insert([0xFF; PAYLOAD])[(a - page) as usize] = *b;
        }
    }
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

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("Brug: maarse-uf2 <firmware.elf> <ud.uf2> [familie-id]");
        std::process::exit(2);
    }
    let family = args
        .get(3)
        .map(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).expect("ugyldigt familie-id"))
        .unwrap_or(0xADA5_2840);
    let elf = fs::read(&args[1]).unwrap_or_else(|e| {
        eprintln!("Kunne ikke læse {}: {e}", args[1]);
        std::process::exit(1);
    });
    let segs = load_segments(&elf).unwrap_or_else(|e| {
        eprintln!("Fejl: {e}");
        std::process::exit(1);
    });
    let uf2 = to_uf2(&segs, family);
    fs::write(&args[2], &uf2).expect("kunne ikke skrive UF2-filen");
    let start = segs.iter().map(|s| s.0).min().unwrap_or(0);
    println!(
        "{}: {} blokke, starter ved 0x{start:X}",
        args[2],
        uf2.len() / 512
    );
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
        assert_eq!(uf2[512 + 32 + 43], 1); // byte 299 af første segment
        assert_eq!(uf2[512 + 32 + 44], 0xFF);
    }
}
