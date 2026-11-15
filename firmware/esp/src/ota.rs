//! Firmware update on the ESP32-S3 with ESP-IDF's two OTA partitions.
//!
//! Flash-layout (partitions.csv):
//!
//! ```text
//! 0x009000  nvs       16 KB
//! 0x00D000  otadata    8 KB   (which app partition the bootloader starts)
//! 0x00F000  phy_init   4 KB
//! 0x010000  ota_0      2 MB   (the same place as the old `factory`)
//! 0x210000  maarse    64 KB   (unchanged: settings, mailbox, OTA meta page)
//! 0x220000  ota_1      2 MB
//! ```
//!
//! The new firmware is written to the partition that is not running, and
//! verified (signature + SHA-256). Then `otadata` points to it, and
//! the node restarts. If the new app is invalid, the bootloader starts the
//! old one. The running app is marked as valid at start-up ([`mark_booted`]),
//! so a bootloader with rollback enabled does not roll back.

use esp_bootloader_esp_idf::ota::OtaImageState;
use esp_bootloader_esp_idf::ota_updater::OtaUpdater;
use esp_bootloader_esp_idf::partitions::{self, PARTITION_TABLE_MAX_LEN};
use esp_storage::FlashStorage;
use maarse_crypto::ota::{Manifest, board};
use maarse_fw_common::storage::OtaConfig;

#[cfg(feature = "board-heltec-v4")]
const BOARD: u16 = board::HELTEC_V4;
#[cfg(not(feature = "board-heltec-v4"))]
const BOARD: u16 = board::XIAO_ESP32S3;

const OTA_0: u32 = 0x10000;
const OTA_1: u32 = 0x220000;
const SLOT: u32 = 0x200000;

/// Marks the running app as valid and finds the staging area (the
/// partition that is not running). `None` with the old partition table
/// (`factory`): then the node can only be updated over USB with espflash.
pub fn config(flash: &mut FlashStorage<'static>, meta: u32) -> Option<OtaConfig<FlashStorage<'static>>> {
    let mut buf = [0u8; PARTITION_TABLE_MAX_LEN];
    let booted = {
        let pt = partitions::read_partition_table(flash, &mut buf).ok()?;
        pt.booted_partition().ok()??.offset()
    };
    let staging = match booted {
        OTA_0 => OTA_1,
        OTA_1 => OTA_0,
        _ => return None,
    };
    mark_booted(flash);
    Some(OtaConfig { board: BOARD, staging, capacity: SLOT, meta, install })
}

fn mark_booted(flash: &mut FlashStorage<'static>) {
    let mut buf = [0u8; PARTITION_TABLE_MAX_LEN];
    if let Ok(mut ota) = OtaUpdater::new(flash, &mut buf)
        && matches!(ota.current_ota_state(), Ok(OtaImageState::New | OtaImageState::PendingVerify))
    {
        let _ = ota.set_current_ota_state(OtaImageState::Valid);
    }
}

fn install(flash: &mut FlashStorage<'static>, m: &Manifest, cfg: &OtaConfig<FlashStorage<'static>>) -> &'static str {
    let mut magic = [0u8; 4];
    if flash.read(cfg.staging, &mut magic).is_err() || magic[0] != 0xE9 || m.size > cfg.capacity {
        return "The image is not an ESP-IDF app image";
    }
    let mut buf = [0u8; PARTITION_TABLE_MAX_LEN];
    let Ok(mut ota) = OtaUpdater::new(flash, &mut buf) else {
        return "The partition table does not have two OTA partitions";
    };
    if ota.activate_next_partition().is_err() {
        return "Could not write otadata";
    }
    let _ = ota.set_current_ota_state(OtaImageState::New);
    esp_hal::system::software_reset()
}
