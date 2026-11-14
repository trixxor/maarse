//! Batteri, strømkilde og støjgulv: målinger, som board-koden og
//! radioløkken lægger her, og som noden læser (status, energistyret
//! routing, telemetri).

use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering::Relaxed};
use maarse_core::battery::Average;

/// Gennemsnitlig batterispænding i mV (0 = ukendt / ingen måling).
pub static BATTERY_MV: AtomicU32 = AtomicU32::new(0);
/// Ekstern strøm (USB): -1 = ukendt, 0 = nej, 1 = ja.
pub static EXT_POWER: AtomicI32 = AtomicI32::new(-1);
/// Chip-temperatur i 0,1 °C (`i32::MIN` = ukendt). Måles af board-koden
/// (nRF52840: via MPSL, der ejer TEMP-enheden; esp-hal 1.2 har ingen
/// driver til ESP32-S3's sensor). Chippen
/// er typisk lidt varmere end omgivelserne.
pub static TEMP_DC: AtomicI32 = AtomicI32::new(i32::MIN);

/// Temperaturen i 0,1 °C, hvis den er målt.
pub fn temperature_dc() -> Option<i16> {
    let t = TEMP_DC.load(Relaxed);
    (t != i32::MIN).then(|| t.clamp(i16::MIN as i32, i16::MAX as i32) as i16)
}

/// Anslået støjgulv × 16 (dBm), glidende gennemsnit.
pub static NOISE_X16: AtomicI32 = AtomicI32::new(0);
static NOISE_SEEN: AtomicBool = AtomicBool::new(false);

/// Så ofte måles batteriet, og så mange målinger tages hver gang.
pub const SAMPLE_EVERY_S: u64 = 60;
pub const SAMPLES: usize = 8;

/// Gemmer gennemsnittet af en serie målinger (mV).
pub fn store_battery(samples: impl IntoIterator<Item = u16>) {
    let mut a = Average::new();
    for s in samples {
        a.add(s);
    }
    BATTERY_MV.store(a.take() as u32, Relaxed);
}

/// Opdaterer støjgulvet ud fra en modtaget pakke (kaldes af radioløkken).
pub fn note_packet(rssi: i32, snr: i32) {
    let n = maarse_core::diag::noise_floor_dbm(rssi.clamp(-200, 0) as i16, snr.clamp(-30, 30) as i8) as i32;
    if NOISE_SEEN.swap(true, Relaxed) {
        let old = NOISE_X16.load(Relaxed);
        NOISE_X16.store(old + (n * 16 - old) / 8, Relaxed);
    } else {
        NOISE_X16.store(n * 16, Relaxed);
    }
}

/// Støjgulvet (dBm), hvis vi har modtaget noget.
pub fn noise_floor() -> Option<i32> {
    NOISE_SEEN.load(Relaxed).then(|| NOISE_X16.load(Relaxed) / 16)
}
