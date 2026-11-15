//! maarse-uf2: makes a UF2 file from the firmware's ELF file, so it can
//! be dragged onto the board's USB drive. No external tools.
//!
//!   maarse-uf2 <firmware.elf> <out.uf2> [family-id, default 0xADA52840]
#![forbid(unsafe_code)]

use std::fs;

use maarse_uf2::{load_segments, to_uf2};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: maarse-uf2 <firmware.elf> <out.uf2> [family-id]");
        std::process::exit(2);
    }
    let family = args
        .get(3)
        .map(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).expect("invalid family id"))
        .unwrap_or(0xADA5_2840);
    let elf = fs::read(&args[1]).unwrap_or_else(|e| {
        eprintln!("Could not read {}: {e}", args[1]);
        std::process::exit(1);
    });
    let segs = load_segments(&elf).unwrap_or_else(|e| {
        eprintln!("Error: {e}");
        std::process::exit(1);
    });
    let uf2 = to_uf2(&segs, family);
    fs::write(&args[2], &uf2).expect("could not write the UF2 file");
    let start = segs.iter().map(|s| s.0).min().unwrap_or(0);
    println!(
        "{}: {} blocks, starting at 0x{start:X}",
        args[2],
        uf2.len() / 512
    );
}
