//! Firmwareopdatering på nRF52840 (Adafruit UF2-bootloader + S140).
//!
//! Flash-layout (se også build.rs og storage.rs):
//!
//! ```text
//! 0x00000-0x00FFF  MBR
//! 0x01000-0x25FFF  SoftDevice S140 v6 (WisMesh)   / -0x26FFF S140 v7 (XIAO)
//! 0x26000/0x27000  appen (højst til 0x87FFF: 392/388 KB)
//! 0x88000-0xE8FFF  mellemlager til ny firmware (388 KB)
//! 0xE9000          OTA-metaside (manifest + bitmap over færdige blokke)
//! 0xEA000-0xEBFFF  postkasse
//! 0xEC000          identitet, radio, administratorer
//! 0xF4000-         Adafruit-bootloaderen (røres aldrig)
//! ```
//!
//! Installation: Adafruits bootloader kan ikke kopiere fra en bank, som
//! appen har skrevet (dens dual bank-DFU styres af dens egne indstillinger
//! under en BLE/seriel DFU-session og er ikke et stabilt API). Derfor
//! kopierer appen selv: en lille rutine, der ligger i RAM (sektionen
//! `.data`, kopieres til RAM af cortex-m-rt ved opstart), sletter appens
//! sider og skriver imaget fra mellemlageret direkte via NVMC-registrene med
//! interrupts slået fra, og genstarter. Kopien tager ca. 10 sekunder for
//! 300 KB. Bootloaderen, SoftDevicen og indstillingerne røres ikke.
//!
//! Risiko: mister noden strømmen i de ca. 10 sekunder, er appen halvt
//! skrevet. Bootloaderen er intakt, så noden kan altid reddes med
//! dobbelttryk på reset (UF2-drevet) og en .uf2-fil - men det kræver fysisk
//! adgang. Imaget er verificeret (signatur + SHA-256) to gange før kopien.

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
        // Imaget skal kunne ligge fra APP_START og ikke ind i mellemlageret.
        capacity: (META - STAGING).min(STAGING - APP_START),
        meta: META,
        install,
    }
}

fn read_word(addr: u32) -> u32 {
    // SAFETY: adressen ligger i intern flash, som altid kan læses.
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

fn install(_flash: &mut Nvmc<'static>, m: &Manifest, cfg: &OtaConfig<Nvmc<'static>>) -> &'static str {
    if m.size == 0 || m.size > cfg.capacity || APP_START + m.size > STAGING {
        return "Imaget er for stort";
    }
    // Simpel kontrol af vektortabellen: stakpointer i RAM, reset-vektor
    // (thumb) inde i det nye image.
    let sp = read_word(STAGING);
    let reset = read_word(STAGING + 4);
    if !(0x2000_0000..=0x2004_0000).contains(&sp)
        || reset & 1 == 0
        || !(APP_START..APP_START + m.size).contains(&(reset & !1))
    {
        return "Imaget ligner ikke en app til denne adresse (vektortabel)";
    }
    let len = m.size.div_ceil(PAGE) * PAGE;
    cortex_m::interrupt::disable();
    // SAFETY: interrupts er slået fra, og rutinen ligger i RAM; den læser
    // kun mellemlageret og skriver kun appens område under STAGING.
    unsafe { copy_and_reset(APP_START, STAGING, len) }
}

const NVMC_READY: *const u32 = 0x4001_E400 as *const u32;
const NVMC_CONFIG: *mut u32 = 0x4001_E504 as *mut u32;
const NVMC_ERASEPAGE: *mut u32 = 0x4001_E508 as *mut u32;
const SCB_AIRCR: *mut u32 = 0xE000_ED0C as *mut u32;

/// Kopierer `len` bytes (hele sider) fra `src` til `dst` og genstarter.
/// Ligger i RAM og kalder intet i flash: kun volatile registeradgang og
/// simple løkker (ingen indeksering, der kan panikke, ingen memcpy).
#[unsafe(link_section = ".data.maarse_ota_copy")]
#[inline(never)]
unsafe extern "C" fn copy_and_reset(dst: u32, src: u32, len: u32) -> ! {
    unsafe {
        let mut page = 0u32;
        while page < len {
            // Prøv hver side op til tre gange, hvis kontrollæsningen fejler.
            let mut tries = 0u32;
            loop {
                while core::ptr::read_volatile(NVMC_READY) == 0 {}
                core::ptr::write_volatile(NVMC_CONFIG, 2); // slet
                core::ptr::write_volatile(NVMC_ERASEPAGE, dst + page);
                while core::ptr::read_volatile(NVMC_READY) == 0 {}
                core::ptr::write_volatile(NVMC_CONFIG, 1); // skriv
                let mut o = 0u32;
                while o < PAGE {
                    let w = core::ptr::read_volatile((src + page + o) as *const u32);
                    core::ptr::write_volatile((dst + page + o) as *mut u32, w);
                    while core::ptr::read_volatile(NVMC_READY) == 0 {}
                    o += 4;
                }
                core::ptr::write_volatile(NVMC_CONFIG, 0); // kun læs
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
        // Systemreset (SYSRESETREQ).
        core::arch::asm!("dsb");
        core::ptr::write_volatile(SCB_AIRCR, 0x05FA_0004);
        loop {
            core::arch::asm!("nop");
        }
    }
}
