//! Battery, power source and noise floor: measurements that the board code and
//! the radio loop put here, and that the node reads (status, energy-driven
//! routing, telemetry).

use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering::Relaxed};
use maarse_core::battery::Average;

/// Average battery voltage in mV (0 = unknown / no measurement).
pub static BATTERY_MV: AtomicU32 = AtomicU32::new(0);
/// External power (USB): -1 = unknown, 0 = no, 1 = yes.
pub static EXT_POWER: AtomicI32 = AtomicI32::new(-1);
/// Chip temperature in 0.1 °C (`i32::MIN` = unknown). Measured by the board code
/// (nRF52840: via MPSL, which owns the TEMP peripheral; esp-hal 1.2 has no
/// driver for the ESP32-S3's sensor). The chip
/// is typically a little warmer than the surroundings.
pub static TEMP_DC: AtomicI32 = AtomicI32::new(i32::MIN);

/// The temperature in 0.1 °C, if it has been measured.
pub fn temperature_dc() -> Option<i16> {
    let t = TEMP_DC.load(Relaxed);
    (t != i32::MIN).then(|| t.clamp(i16::MIN as i32, i16::MAX as i32) as i16)
}

/// Estimated noise floor × 16 (dBm), moving average.
pub static NOISE_X16: AtomicI32 = AtomicI32::new(0);
static NOISE_SEEN: AtomicBool = AtomicBool::new(false);

/// How often the battery is measured, and how many measurements are taken each time.
pub const SAMPLE_EVERY_S: u64 = 60;
pub const SAMPLES: usize = 8;

/// Stores the average of a series of measurements (mV).
pub fn store_battery(samples: impl IntoIterator<Item = u16>) {
    let mut a = Average::new();
    for s in samples {
        a.add(s);
    }
    BATTERY_MV.store(a.take() as u32, Relaxed);
}

/// Updates the noise floor from a received packet (called by the radio loop).
pub fn note_packet(rssi: i32, snr: i32) {
    let n = maarse_core::diag::noise_floor_dbm(rssi.clamp(-200, 0) as i16, snr.clamp(-30, 30) as i8) as i32;
    if NOISE_SEEN.swap(true, Relaxed) {
        let old = NOISE_X16.load(Relaxed);
        NOISE_X16.store(old + (n * 16 - old) / 8, Relaxed);
    } else {
        NOISE_X16.store(n * 16, Relaxed);
    }
}

/// The noise floor (dBm), if we have received anything.
pub fn noise_floor() -> Option<i32> {
    NOISE_SEEN.load(Relaxed).then(|| NOISE_X16.load(Relaxed) / 16)
}

// ----- current profile for power estimates (range test) -----
//
// The boards cannot measure current, so the range test estimates charge from
// time and TX on-time with these typical currents, in 0.1 mA. The board code
// sets them at boot (see `set_profile`); they are estimates, not measurements,
// and the web client lets the user replace them with measured values.

/// Everything except the LoRa radio: MCU, Bluetooth, display, regulators.
pub static BASE_MA_X10: AtomicU32 = AtomicU32::new(50);
/// LoRa receive (the radio is receiving whenever it is not transmitting).
pub static RX_MA_X10: AtomicU32 = AtomicU32::new(53);
/// Extra transmit current on top of the SX1262 itself (an external PA).
pub static TX_EXTRA_MA_X10: AtomicU32 = AtomicU32::new(0);

/// Sets the board's current profile (all in 0.1 mA).
pub fn set_profile(base_ma_x10: u32, rx_ma_x10: u32, tx_extra_ma_x10: u32) {
    BASE_MA_X10.store(base_ma_x10, Relaxed);
    RX_MA_X10.store(rx_ma_x10, Relaxed);
    TX_EXTRA_MA_X10.store(tx_extra_ma_x10, Relaxed);
}

/// Typical current of the GPS module while it runs, in 0.1 mA (0 = none).
pub static GPS_MA_X10: AtomicU32 = AtomicU32::new(0);
/// The size of the board's battery in mAh (0 = not known: no time left is given).
pub static CAPACITY_MAH: AtomicU32 = AtomicU32::new(0);

/// Estimated transmit current at `dbm` (radio setting), in 0.1 mA.
pub fn tx_ma_x10(dbm: i8) -> u32 {
    maarse_core::rangetest::sx1262_tx_ma_x10(dbm) + TX_EXTRA_MA_X10.load(Relaxed)
}
