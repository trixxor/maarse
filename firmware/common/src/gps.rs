//! GPS: the latest fix, shared between the board's GPS task (which reads the module with
//! `maarse_core::nmea`) and the node (which shares the position).

use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering::Relaxed};

/// The board has a GPS module (set by board code at start-up).
pub static HAS_MODULE: AtomicBool = AtomicBool::new(false);
/// The node wants a position (sharing is on): the board powers the module.
pub static WANTED: AtomicBool = AtomicBool::new(false);
/// Latest valid fix in degrees × 10^7.
pub static LAT_E7: AtomicI32 = AtomicI32::new(0);
pub static LON_E7: AtomicI32 = AtomicI32::new(0);
/// Uptime (s) of the latest valid fix, 0 = none yet.
pub static FIX_AT_S: AtomicU32 = AtomicU32::new(0);
/// Satellites in use, from the latest GGA; `NO_DATA` until the module has said anything.
pub static SATS: AtomicU32 = AtomicU32::new(NO_DATA);
pub const NO_DATA: u32 = u32::MAX;
/// Satellites in view (GSV); `NO_DATA` until the module has said anything.
pub static SATS_VIEW: AtomicU32 = AtomicU32::new(NO_DATA);
/// NMEA bytes received since the module was switched on.
pub static BYTES: AtomicU32 = AtomicU32::new(0);

/// A fix older than this is not used (the module lost the sky, or was switched off).
pub const MAX_AGE_S: u32 = 900;

/// The latest fix, if it is recent: (latitude, longitude) × 10^7.
pub fn fix() -> Option<(i32, i32)> {
    let at = FIX_AT_S.load(Relaxed);
    if at == 0 {
        return None;
    }
    let now = embassy_time::Instant::now().as_secs() as u32;
    (now.saturating_sub(at) <= MAX_AGE_S).then(|| (LAT_E7.load(Relaxed), LON_E7.load(Relaxed)))
}

/// Stores a fix. `now_s` must be at least 1.
pub fn store_fix(lat_e7: i32, lon_e7: i32, now_s: u32) {
    LAT_E7.store(lat_e7, Relaxed);
    LON_E7.store(lon_e7, Relaxed);
    FIX_AT_S.store(now_s.max(1), Relaxed);
}

/// What the status page shows: 0 = off, 1 = on but silent, 2 = searching, 3 = has a fix.
pub fn state() -> u8 {
    if !WANTED.load(Relaxed) {
        0
    } else if fix().is_some() {
        3
    } else if BYTES.load(Relaxed) == 0 {
        1
    } else {
        2
    }
}
