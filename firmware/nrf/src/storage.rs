//! Flash-placering på nRF52840. Selve formatet ligger i
//! `maarse_fw_common::storage`.
//!
//! * 0xEA000–0xEBFFF: postkasse (8 KB)
//! * 0xEC000: identitet, radio og administratorer (4 KB)
//!
//! Appen slutter ved 0xEA000 (se build.rs); bootloaderen ligger fra 0xF4000.

use embassy_nrf::nvmc::Nvmc;
use maarse_fw_common::storage::FlashSettings;

pub const MAIL_START: u32 = 0xEA000;
pub const CFG_ADDR: u32 = 0xEC000;

pub type Flash = FlashSettings<Nvmc<'static>>;
