//! Vælger hukommelseslayout efter board.
//!
//! Begge boards har Adafruits UF2-bootloader og en SoftDevice i flash, som
//! firmwaren ikke bruger, men skal ligge efter:
//! * WisMesh Tag (RAK4630): S140 v6 -> appen starter ved 0x26000
//! * XIAO nRF52840: S140 v7 -> appen starter ved 0x27000
//! 0xEA000-0xEBFFF er postkasse, siden 0xEC000 nodens identitet.
use std::{env, fs, path::PathBuf};

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let wismesh = env::var_os("CARGO_FEATURE_BOARD_WISMESH_TAG").is_some();
    let xiao = env::var_os("CARGO_FEATURE_BOARD_XIAO_NRF52840").is_some();
    if wismesh == xiao {
        panic!(
            "Vælg præcis ét board: --features board-wismesh-tag eller --features board-xiao-nrf52840"
        );
    }
    let origin: u32 = if wismesh { 0x26000 } else { 0x27000 };
    let end: u32 = 0xEA000;
    let memory = format!(
        "MEMORY\n{{\n  FLASH : ORIGIN = 0x{origin:X}, LENGTH = 0x{:X}\n  RAM : ORIGIN = 0x20000008, LENGTH = 0x3FFF8\n}}\n",
        end - origin
    );
    fs::write(out.join("memory.x"), memory).unwrap();
    // Startadressen til UF2-konverteringen i scriptet.
    fs::write(out.join("app-start.txt"), format!("0x{origin:X}")).unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rustc-link-arg=--nmagic");
    println!("cargo:rustc-link-arg=-Tlink.x");
}
