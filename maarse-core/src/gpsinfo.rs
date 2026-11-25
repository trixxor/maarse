//! GPS details for the Experimental GPS page: the layout of the `Gps` event (0xbe).
//!
//! `state | used | view | hdop x10 (u16) | altitude in decimetres (i32, [`NO_ALT`] = unknown) |
//! speed in cm/s (u16) | course in degrees (u16) | UTC as hhmmss (u32, 0 = unknown) |
//! latitude x 10^7 (i32) | longitude x 10^7 (i32) | n | n x satellite (7 bytes)`.
//! A satellite: `system | number | elevation | azimuth (u16) | signal (dB-Hz) | used`.

use crate::companion::{MAX_TEXT, Text};

/// Altitude when the module has not said.
pub const NO_ALT: i32 = i32::MIN;
/// The most satellites in one event (7 bytes each: 26 + 24 x 7 = 194 of 240).
pub const MAX_SATS: usize = 24;

/// Satellite systems.
pub const GPS: u8 = 0;
pub const GLONASS: u8 = 1;
pub const GALILEO: u8 = 2;
pub const BEIDOU: u8 = 3;
pub const OTHER: u8 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sat {
    pub sys: u8,
    pub prn: u8,
    pub elevation: u8,
    pub azimuth: u16,
    /// Signal strength in dB-Hz, 0 = not tracked.
    pub snr: u8,
    pub used: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Info {
    /// 0 off, 1 silent, 2 searching, 3 has a position (as `stat::GPS_STATE`).
    pub state: u8,
    pub used: u8,
    pub view: u8,
    pub hdop_x10: u16,
    pub alt_dm: i32,
    pub speed_cms: u16,
    pub course: u16,
    pub utc: u32,
    pub lat_e7: i32,
    pub lon_e7: i32,
    pub sats: heapless::Vec<Sat, MAX_SATS>,
}

impl Info {
    pub fn to_bytes(&self) -> Text {
        let mut v = Text::new();
        let _ = v.extend_from_slice(&[self.state, self.used, self.view]);
        let _ = v.extend_from_slice(&self.hdop_x10.to_le_bytes());
        let _ = v.extend_from_slice(&self.alt_dm.to_le_bytes());
        let _ = v.extend_from_slice(&self.speed_cms.to_le_bytes());
        let _ = v.extend_from_slice(&self.course.to_le_bytes());
        let _ = v.extend_from_slice(&self.utc.to_le_bytes());
        let _ = v.extend_from_slice(&self.lat_e7.to_le_bytes());
        let _ = v.extend_from_slice(&self.lon_e7.to_le_bytes());
        let _ = v.push(self.sats.len() as u8);
        for s in &self.sats {
            if v.len() + 7 > MAX_TEXT {
                break;
            }
            let _ = v.extend_from_slice(&[s.sys, s.prn, s.elevation]);
            let _ = v.extend_from_slice(&s.azimuth.to_le_bytes());
            let _ = v.extend_from_slice(&[s.snr, s.used as u8]);
        }
        v
    }

    pub fn from_bytes(b: &[u8]) -> Option<Info> {
        if b.len() < 26 {
            return None;
        }
        let u16at = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
        let i32at = |i: usize| i32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        let n = b[25] as usize;
        let mut sats = heapless::Vec::new();
        for k in 0..n {
            let o = 26 + k * 7;
            let c = b.get(o..o + 7)?;
            sats.push(Sat { sys: c[0], prn: c[1], elevation: c[2], azimuth: u16::from_le_bytes([c[3], c[4]]), snr: c[5], used: c[6] != 0 }).ok()?;
        }
        Some(Info {
            state: b[0],
            used: b[1],
            view: b[2],
            hdop_x10: u16at(3),
            alt_dm: i32at(5),
            speed_cms: u16at(9),
            course: u16at(11),
            utc: u32::from_le_bytes([b[13], b[14], b[15], b[16]]),
            lat_e7: i32at(17),
            lon_e7: i32at(21),
            sats,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut sats = heapless::Vec::new();
        for i in 0..MAX_SATS as u8 {
            sats.push(Sat { sys: i % 4, prn: i, elevation: 10 + i, azimuth: 300 + i as u16, snr: 30, used: i % 2 == 0 }).unwrap();
        }
        let i = Info { state: 3, used: 9, view: 14, hdop_x10: 11, alt_dm: 420, speed_cms: 12, course: 84, utc: 223107, lat_e7: 561_561_200, lon_e7: 102_107_000, sats };
        let b = i.to_bytes();
        assert!(b.len() <= MAX_TEXT);
        assert_eq!(Info::from_bytes(&b), Some(i));
    }
}
