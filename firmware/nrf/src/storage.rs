//! Flash placement on the nRF52840. The format itself is in
//! `maarse_fw_common::storage`.
//!
//! * 0xEA000–0xEBFFF: mailbox (8 KB)
//! * 0xEC000: identity, radio and administrators (4 KB)
//!
//! The app ends at 0x88000 at the latest (see build.rs); after that the staging area and
//! meta page for firmware updates (see ota.rs). The bootloader starts at 0xF4000.

use embassy_nrf::nvmc::Nvmc;
use maarse_fw_common::storage::FlashSettings;

pub const MAIL_START: u32 = 0xEA000;
pub const CFG_ADDR: u32 = 0xEC000;

pub type Flash = FlashSettings<Nvmc<'static>>;
