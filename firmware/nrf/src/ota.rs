//! Firmware update on the nRF52840 (Adafruit UF2 bootloader + S140).
//!
//! Flash layout (see also build.rs and storage.rs):
//!
//! ```text
//! 0x00000-0x00FFF  MBR
//! 0x01000-0x25FFF  SoftDevice S140 v6 (WisMesh)   / -0x26FFF S140 v7 (XIAO)
//! 0x26000/0x27000  the app (at most up to 0x87FFF: 392/388 KB)
//! 0x88000-0xE8FFF  staging area for new firmware (388 KB)
//! 0xE9000          OTA meta page (manifest + bitmap of finished blocks)
//! 0xEA000-0xEBFFF  mailbox
//! 0xEC000          identity, radio, administrators
//! 0xF4000-         the Adafruit bootloader (never touched)
//! ```
//!
//! Installation: Adafruit's bootloader cannot copy from a bank that
//! the app has written (its dual bank DFU is controlled by its own settings
//! during a BLE/serial DFU session and is not a stable API). So
//! the app copies itself: a small routine that lives in RAM (the section
//! `.data`, copied to RAM by cortex-m-rt at start-up) erases the app's
//! pages and writes the image from the staging area directly via the NVMC registers with
//! interrupts disabled, and restarts. The copy takes about 10 seconds for
//! 300 KB. The bootloader, the SoftDevice and the settings are not touched.
//!
//! Risk: if the node loses power during those 10 seconds or so, the app is half
//! written. The bootloader is intact, so the node can always be rescued with
//! a double press on reset (the UF2 drive) and a .uf2 file - but that needs physical
//! access. The image is verified (signature + SHA-256) twice before the copy.

use embassy_nrf::nvmc::Nvmc;
use maarse_crypto::ota::{Manifest, board};
use maarse_fw_common::storage::OtaConfig;

#[cfg(feature = "board-wismesh-tag")]
pub const APP_START: u32 = 0x26000;
#[cfg(feature = "board-wismesh-tag")]
const BOARD: u16 = board::WISMESH_TAG;
#[cfg(not(feature = "board-wismesh-tag"))]
pub const APP_START: u32 = 0x27000;
#[cfg(not(feature = "board-wismesh-tag"))]
const BOARD: u16 = board::XIAO_NRF52840;

pub const STAGING: u32 = 0x88000;
pub const META: u32 = 0xE9000;
const PAGE: u32 = 4096;

pub fn config() -> OtaConfig<Nvmc<'static>> {
    OtaConfig {
        board: BOARD,
        staging: STAGING,
        // The image must fit from APP_START and not into the staging area.
        capacity: (META - STAGING).min(STAGING - APP_START),
        meta: META,
        install,
    }
}

fn read_word(addr: u32) -> u32 {
    // SAFETY: the address is in internal flash, which can always be read.
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

fn install(_flash: &mut Nvmc<'static>, m: &Manifest, cfg: &OtaConfig<Nvmc<'static>>) -> &'static str {
    if m.size == 0 || m.size > cfg.capacity || APP_START + m.size > STAGING {
        return "The image is too large";
    }
    // Simple check of the vector table: stack pointer in RAM, reset vector
    // (thumb) inside the new image.
    let sp = read_word(STAGING);
    let reset = read_word(STAGING + 4);
    if !(0x2000_0000..=0x2004_0000).contains(&sp)
        || reset & 1 == 0
        || !(APP_START..APP_START + m.size).contains(&(reset & !1))
    {
        return "The image does not look like an app for this address (vector table)";
    }
    let len = m.size.div_ceil(PAGE) * PAGE;
    cortex_m::interrupt::disable();
    // SAFETY: interrupts are disabled, and the routine is in RAM; it only reads
    // the staging area and only writes the app's area below STAGING.
    unsafe { copy_and_reset(APP_START, STAGING, len) }
}

const NVMC_READY: *const u32 = 0x4001_E400 as *const u32;
const NVMC_CONFIG: *mut u32 = 0x4001_E504 as *mut u32;
const NVMC_ERASEPAGE: *mut u32 = 0x4001_E508 as *mut u32;
const SCB_AIRCR: *mut u32 = 0xE000_ED0C as *mut u32;

/// Copies `len` bytes (whole pages) from `src` to `dst` and restarts.
/// Lives in RAM and calls nothing in flash: only volatile register access and
/// simple loops (no indexing that can panic, no memcpy).
#[unsafe(link_section = ".data.maarse_ota_copy")]
#[inline(never)]
unsafe extern "C" fn copy_and_reset(dst: u32, src: u32, len: u32) -> ! {
    unsafe {
        let mut page = 0u32;
        while page < len {
            // Try each page up to three times if the verify read fails.
            let mut tries = 0u32;
            loop {
                while core::ptr::read_volatile(NVMC_READY) == 0 {}
                core::ptr::write_volatile(NVMC_CONFIG, 2); // erase
                core::ptr::write_volatile(NVMC_ERASEPAGE, dst + page);
                while core::ptr::read_volatile(NVMC_READY) == 0 {}
                core::ptr::write_volatile(NVMC_CONFIG, 1); // write
                let mut o = 0u32;
                while o < PAGE {
                    let w = core::ptr::read_volatile((src + page + o) as *const u32);
                    core::ptr::write_volatile((dst + page + o) as *mut u32, w);
                    while core::ptr::read_volatile(NVMC_READY) == 0 {}
                    o += 4;
                }
                core::ptr::write_volatile(NVMC_CONFIG, 0); // read only
                let mut ok = true;
                o = 0;
                while o < PAGE {
                    if core::ptr::read_volatile((src + page + o) as *const u32)
                        != core::ptr::read_volatile((dst + page + o) as *const u32)
                    {
                        ok = false;
                    }
                    o += 4;
                }
                tries += 1;
                if ok || tries >= 3 {
                    break;
                }
            }
            page += PAGE;
        }
        // System reset (SYSRESETREQ).
        core::arch::asm!("dsb");
        core::ptr::write_volatile(SCB_AIRCR, 0x05FA_0004);
        loop {
            core::arch::asm!("nop");
        }
    }
}
