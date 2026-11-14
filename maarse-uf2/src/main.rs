//! maarse-uf2: laver en UF2-fil ud fra firmwarens ELF-fil, så den kan
//! trækkes over på boardets USB-drev. Ingen eksterne værktøjer.
//!
//!   maarse-uf2 <firmware.elf> <ud.uf2> [familie-id, standard 0xADA52840]
#![forbid(unsafe_code)]

use std::fs;

use maarse_uf2::{load_segments, to_uf2};

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
