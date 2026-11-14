//! Firmwareopdatering på ESP32-S3 med ESP-IDF's to OTA-partitioner.
//!
//! Flash-layout (partitions.csv):
//!
//! ```text
//! 0x009000  nvs       16 KB
//! 0x00D000  otadata    8 KB   (hvilken app-partition bootloaderen starter)
//! 0x00F000  phy_init   4 KB
//! 0x010000  ota_0      2 MB   (samme sted som den gamle `factory`)
//! 0x210000  maarse    64 KB   (uændret: indstillinger, postkasse, OTA-metaside)
//! 0x220000  ota_1      2 MB
//! ```
//!
//! Den nye firmware skrives til den partition, der ikke kører, og
//! verificeres (signatur + SHA-256). Derefter peger `otadata` på den, og
//! noden genstarter. Er den nye app ugyldig, starter bootloaderen den
//! gamle. Den kørende app markeres som gyldig ved opstart ([`mark_booted`]),
//! så en bootloader med rollback slået til ikke ruller tilbage.

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

/// Markerer den kørende app som gyldig og finder mellemlageret (den
/// partition, der ikke kører). `None` med den gamle partitionstabel
/// (`factory`): så kan noden kun opdateres over USB med espflash.
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
        return "Imaget er ikke et ESP-IDF app-image";
    }
    let mut buf = [0u8; PARTITION_TABLE_MAX_LEN];
    let Ok(mut ota) = OtaUpdater::new(flash, &mut buf) else {
        return "Partitionstabellen har ikke to OTA-partitioner";
    };
    if ota.activate_next_partition().is_err() {
        return "Kunne ikke skrive otadata";
    }
    let _ = ota.set_current_ota_state(OtaImageState::New);
    esp_hal::system::software_reset()
}
