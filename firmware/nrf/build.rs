//! Chooses the memory layout by board.
//!
//! Both boards have Adafruit's UF2 bootloader and a SoftDevice in flash, which
//! the firmware does not use, but must be placed after:
//! * WisMesh Tag (RAK4630): S140 v6 -> the app starts at 0x26000
//! * XIAO nRF52840: S140 v7 -> the app starts at 0x27000
//! The app may go at most to 0x88000; 0x88000-0xE8FFF is the staging area for
//! firmware updates and 0xE9000 its meta page (see src/ota.rs).
//! 0xEA000-0xEBFFF is the mailbox, the page at 0xEC000 the node's identity.
use std::{env, fs, path::PathBuf};

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let wismesh = env::var_os("CARGO_FEATURE_BOARD_WISMESH_TAG").is_some();
    let xiao = env::var_os("CARGO_FEATURE_BOARD_XIAO_NRF52840").is_some();
    if wismesh == xiao {
        panic!(
            "Choose exactly one board: --features board-wismesh-tag or --features board-xiao-nrf52840"
        );
    }
    let origin: u32 = if wismesh { 0x26000 } else { 0x27000 };
    let end: u32 = 0x88000;
    let memory = format!(
        "MEMORY\n{{\n  FLASH : ORIGIN = 0x{origin:X}, LENGTH = 0x{:X}\n  RAM : ORIGIN = 0x20000008, LENGTH = 0x3FFF8\n}}\n",
        end - origin
    );
    fs::write(out.join("memory.x"), memory).unwrap();
    // The start address for the UF2 conversion in the script.
    fs::write(out.join("app-start.txt"), format!("0x{origin:X}")).unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=MAARSE_RELEASE_PUBKEY");
    println!("cargo:rerun-if-env-changed=MAARSE_FW_VERSION");
    if env::var("MAARSE_RELEASE_PUBKEY").map_or(true, |key| key.is_empty()) {
        println!(
            "cargo:warning=MAARSE_RELEASE_PUBKEY is not set: over-the-air firmware updates are disabled."
        );
    }
    println!("cargo:rustc-link-arg=--nmagic");
    println!("cargo:rustc-link-arg=-Tlink.x");
}
