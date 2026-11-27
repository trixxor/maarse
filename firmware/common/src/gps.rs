//! GPS: the latest fix, shared between the board's GPS task (which reads the module with
//! `maarse_core::nmea`) and the node (which shares the position).

use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering::Relaxed};
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex as M;
use maarse_core::gpsinfo::{self, Info, NO_ALT, Sat};

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

// ---------- details for the Experimental GPS page ----------

/// Height (decimetres), accuracy (HDOP x 10), speed (cm/s), course and UTC (hhmmss) from the latest sentences.
pub static ALT_DM: AtomicI32 = AtomicI32::new(NO_ALT);
pub static HDOP_X10: AtomicU32 = AtomicU32::new(0);
pub static SPEED_CMS: AtomicU32 = AtomicU32::new(0);
pub static COURSE: AtomicU32 = AtomicU32::new(0);
pub static UTC: AtomicU32 = AtomicU32::new(0);

/// A satellite is dropped from the list when it was not heard of for this long (s).
const SAT_MAX_AGE_S: u32 = 30;

#[derive(Clone, Copy)]
struct Row {
    sat: Sat,
    seen_s: u32,
}

struct Table {
    rows: heapless::Vec<Row, 40>,
    used: [u8; 12],
    used_n: u8,
    used_seen_s: u32,
}

static TABLE: Mutex<M, RefCell<Table>> =
    Mutex::new(RefCell::new(Table { rows: heapless::Vec::new(), used: [0; 12], used_n: 0, used_seen_s: 0 }));

/// A satellite in view (from GSV).
pub fn note_sat(sys: u8, prn: u8, elevation: u8, azimuth: u16, snr: u8, now_s: u32) {
    TABLE.lock(|t| {
        let mut t = t.borrow_mut();
        let sat = Sat { sys, prn, elevation, azimuth, snr, used: false };
        match t.rows.iter_mut().find(|r| r.sat.sys == sys && r.sat.prn == prn) {
            Some(r) => *r = Row { sat, seen_s: now_s },
            None => {
                if t.rows.is_full() {
                    t.rows.retain(|r| now_s.saturating_sub(r.seen_s) <= SAT_MAX_AGE_S);
                }
                let _ = t.rows.push(Row { sat, seen_s: now_s });
            }
        }
    });
}

/// The satellites used for the fix (from GSA).
pub fn note_used(prns: &[u8], now_s: u32) {
    TABLE.lock(|t| {
        let mut t = t.borrow_mut();
        let n = prns.len().min(12);
        t.used = [0; 12];
        t.used[..n].copy_from_slice(&prns[..n]);
        t.used_n = n as u8;
        t.used_seen_s = now_s;
    });
}

/// Forgets satellites and details (the module is switched off).
pub fn clear() {
    TABLE.lock(|t| {
        let mut t = t.borrow_mut();
        t.rows.clear();
        t.used_n = 0;
    });
    ALT_DM.store(NO_ALT, Relaxed);
    HDOP_X10.store(0, Relaxed);
    SPEED_CMS.store(0, Relaxed);
    COURSE.store(0, Relaxed);
    UTC.store(0, Relaxed);
}

/// Everything the GPS page shows, for the `Gps` event.
pub fn info(now_s: u32) -> Info {
    let (lat, lon) = fix().unwrap_or((0, 0));
    let mut sats: heapless::Vec<Sat, { gpsinfo::MAX_SATS }> = heapless::Vec::new();
    TABLE.lock(|t| {
        let t = t.borrow();
        let used_fresh = now_s.saturating_sub(t.used_seen_s) <= SAT_MAX_AGE_S;
        // The strongest first, so a full list drops the weakest.
        let mut rows: heapless::Vec<Row, 40> = t.rows.iter().copied().filter(|r| now_s.saturating_sub(r.seen_s) <= SAT_MAX_AGE_S).collect();
        maarse_core::sort::sort_by(&mut rows[..], |a, b| a.sat.snr > b.sat.snr);
        for r in rows {
            let mut sat = r.sat;
            sat.used = used_fresh && t.used[..t.used_n as usize].contains(&sat.prn);
            let _ = sats.push(sat);
        }
    });
    let sats_used = sats.iter().filter(|s| s.used).count() as u8;
    let view = sats.len() as u8;
    Info {
        state: state(),
        used: match SATS.load(Relaxed) {
            NO_DATA => sats_used,
            n => n.min(255) as u8,
        },
        view,
        hdop_x10: HDOP_X10.load(Relaxed) as u16,
        alt_dm: ALT_DM.load(Relaxed),
        speed_cms: SPEED_CMS.load(Relaxed) as u16,
        course: COURSE.load(Relaxed) as u16,
        utc: UTC.load(Relaxed),
        lat_e7: lat,
        lon_e7: lon,
        sats,
    }
}
