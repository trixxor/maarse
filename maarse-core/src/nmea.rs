//! NMEA parser for GPS modules (used by the firmware's GPS task).
//!
//! Read from any talker (`GP`, `GN`, `GL`, `GA`, `GB`, ...): `RMC` (valid flag, position, speed,
//! course, time), `GGA` (satellites in use, accuracy, height), `GSV` (every satellite in view with
//! direction and signal strength) and `GSA` (which satellites are used). A sentence with a wrong
//! checksum is dropped. One sentence can give several results: call [`Parser::feed`] for each byte
//! and then [`Parser::next`] until it gives `None`.

use crate::gpsinfo::{BEIDOU, GALILEO, GLONASS, GPS, OTHER};

/// What a sentence told us.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sentence {
    /// A valid position (degrees × 10^7).
    Fix { lat_e7: i32, lon_e7: i32 },
    /// RMC without a fix.
    NoFix,
    /// Satellites in use.
    Sats(u8),
    /// Satellites in view (the total from a GSV sentence, for that system).
    InView(u8),
    /// Height above sea level in decimetres.
    Alt(i32),
    /// Horizontal dilution of precision × 10 (lower is better).
    Hdop(u16),
    /// Speed in cm/s and course in degrees.
    Motion { speed_cms: u16, course: u16 },
    /// Time of day (UTC) as hhmmss.
    Utc(u32),
    /// One satellite in view; `snr` 0 = not tracked.
    Sat { sys: u8, prn: u8, elevation: u8, azimuth: u16, snr: u8 },
    /// The satellites used for the fix (empty slots are not included).
    Used { prns: [u8; 12], n: u8 },
}

const QUEUE: usize = 10;

/// Collects bytes into sentences.
pub struct Parser {
    line: heapless::Vec<u8, 100>,
    overflow: bool,
    queue: heapless::Deque<Sentence, QUEUE>,
}

impl Default for Parser {
    fn default() -> Self {
        Self::new()
    }
}

impl Parser {
    pub const fn new() -> Self {
        Parser { line: heapless::Vec::new(), overflow: false, queue: heapless::Deque::new() }
    }

    /// Feeds one byte; returns the first result when a sentence ends (more can follow: [`Parser::next`]).
    pub fn feed(&mut self, b: u8) -> Option<Sentence> {
        match b {
            b'$' => {
                self.line.clear();
                self.overflow = false;
                None
            }
            b'\n' | b'\r' => {
                if !(self.overflow || self.line.is_empty()) {
                    let line = core::mem::take(&mut self.line);
                    parse(&line, &mut self.queue);
                }
                self.line.clear();
                self.overflow = false;
                self.queue.pop_front()
            }
            _ => {
                if self.line.push(b).is_err() {
                    self.overflow = true;
                }
                None
            }
        }
    }

    /// The next result of the sentence that just ended.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Option<Sentence> {
        self.queue.pop_front()
    }
}

fn hex(c: u8) -> Option<u8> {
    (c as char).to_digit(16).map(|d| d as u8)
}

fn push(q: &mut heapless::Deque<Sentence, QUEUE>, s: Sentence) {
    let _ = q.push_back(s);
}

/// Whole number from digits only (empty = none).
fn uint(f: &[u8]) -> Option<u32> {
    if f.is_empty() {
        return None;
    }
    let mut n = 0u32;
    for c in f {
        n = n.checked_mul(10)?.checked_add(c.checked_sub(b'0').filter(|d| *d < 10)? as u32)?;
    }
    Some(n)
}

/// A decimal number with one decimal kept, as an integer × 10 (`"1.25"` = 12, `"-3.5"` = -35).
fn dec10(f: &[u8]) -> Option<i32> {
    let (neg, f) = match f.first()? {
        b'-' => (true, &f[1..]),
        _ => (false, f),
    };
    let dot = f.iter().position(|c| *c == b'.');
    let (whole, frac) = match dot {
        Some(i) => (&f[..i], &f[i + 1..]),
        None => (f, &f[..0]),
    };
    let w = if whole.is_empty() { 0 } else { uint(whole)? as i32 };
    let d = frac.first().map_or(Some(0), |c| c.checked_sub(b'0').filter(|d| *d < 10).map(|d| d as i32))?;
    let v = w.checked_mul(10)?.checked_add(d)?;
    Some(if neg { -v } else { v })
}

fn system(talker: &[u8]) -> u8 {
    match talker {
        b"GP" => GPS,
        b"GL" => GLONASS,
        b"GA" => GALILEO,
        b"GB" | b"BD" => BEIDOU,
        _ => OTHER,
    }
}

/// Parses `GPRMC,...*hh` (without the leading `$`) into the queue.
fn parse(s: &[u8], q: &mut heapless::Deque<Sentence, QUEUE>) {
    let _ = parse_inner(s, q);
}

fn parse_inner(s: &[u8], q: &mut heapless::Deque<Sentence, QUEUE>) -> Option<()> {
    let star = s.iter().position(|c| *c == b'*')?;
    let (body, sum) = (&s[..star], s.get(star + 1..star + 3)?);
    let want = hex(sum[0])? << 4 | hex(sum[1])?;
    if body.iter().fold(0u8, |a, c| a ^ c) != want {
        return None;
    }
    let mut f = body.split(|c| *c == b',');
    let head = f.next()?;
    if head.len() != 5 {
        return None;
    }
    match &head[2..] {
        b"RMC" => {
            // time, status, lat, N/S, lon, E/W, speed (knots), course, date
            let time = f.next()?;
            let status = f.next()?;
            if let Some(t) = uint(time.split(|c| *c == b'.').next()?) {
                push(q, Sentence::Utc(t));
            }
            if status != b"A" {
                push(q, Sentence::NoFix);
                return Some(());
            }
            let lat = coord(f.next()?, 2)?;
            let ns = f.next()?;
            let lon = coord(f.next()?, 3)?;
            let ew = f.next()?;
            let lat = if ns == b"S" { -lat } else { lat };
            let lon = if ew == b"W" { -lon } else { lon };
            push(q, Sentence::Fix { lat_e7: lat, lon_e7: lon });
            let knots = f.next().and_then(dec10);
            let course = f.next().and_then(dec10);
            if let Some(kn) = knots {
                // knots x10 -> cm/s: 1 knot = 51.4444 cm/s
                let cms = (kn.max(0) as i64 * 5144 / 1000).min(u16::MAX as i64) as u16;
                let course = (course.unwrap_or(0).max(0) / 10).min(359) as u16;
                push(q, Sentence::Motion { speed_cms: cms, course });
            }
        }
        b"GGA" => {
            // time, lat, N/S, lon, E/W, quality, satellites, hdop, altitude, M
            for _ in 0..5 {
                f.next()?;
            }
            let _quality = f.next()?;
            let sats = f.next()?;
            let mut n = 0u32;
            for c in sats {
                n = n * 10 + (c.checked_sub(b'0').filter(|d| *d < 10)? as u32);
            }
            push(q, Sentence::Sats(n.min(255) as u8));
            if let Some(h) = f.next().and_then(dec10) {
                push(q, Sentence::Hdop(h.clamp(0, u16::MAX as i32) as u16));
            }
            if let Some(a) = f.next().and_then(dec10) {
                push(q, Sentence::Alt(a));
            }
        }
        b"GSV" => {
            // sentences, this sentence, satellites in view, then up to 4 x (number, elevation, azimuth, signal)
            let sys = system(&head[..2]);
            f.next()?;
            f.next()?;
            let view = uint(f.next()?)?;
            push(q, Sentence::InView(view.min(255) as u8));
            while let (Some(prn), Some(el), Some(az)) = (f.next(), f.next(), f.next()) {
                // The signal strength is empty for a satellite that is not tracked.
                let snr = f.next().and_then(uint).unwrap_or(0);
                let (Some(prn), Some(el), Some(az)) = (uint(prn), uint(el), uint(az)) else { continue };
                push(q, Sentence::Sat { sys, prn: prn.min(255) as u8, elevation: el.min(90) as u8, azimuth: (az % 360) as u16, snr: snr.min(99) as u8 });
            }
        }
        b"GSA" => {
            // mode, fix type, 12 satellite numbers, PDOP, HDOP, VDOP
            f.next()?;
            f.next()?;
            let mut prns = [0u8; 12];
            let mut n = 0u8;
            for _ in 0..12 {
                if let Some(p) = uint(f.next()?)
                    && p > 0
                {
                    prns[n as usize] = p.min(255) as u8;
                    n += 1;
                }
            }
            push(q, Sentence::Used { prns, n });
            f.next()?;
            if let Some(h) = f.next().and_then(dec10) {
                push(q, Sentence::Hdop(h.clamp(0, u16::MAX as i32) as u16));
            }
        }
        _ => {}
    }
    Some(())
}

/// `ddmm.mmmm` (or `dddmm.mmmm`) to degrees × 10^7.
fn coord(f: &[u8], deg_digits: usize) -> Option<i32> {
    if f.len() < deg_digits + 3 {
        return None;
    }
    let mut deg = 0i64;
    for c in &f[..deg_digits] {
        deg = deg * 10 + (c.checked_sub(b'0').filter(|d| *d < 10)? as i64);
    }
    // Minutes with up to 5 decimals, as an integer in 10^-5 minutes.
    let mut min_int = 0i64;
    let mut frac = 0i64;
    let mut frac_digits = 0;
    let mut seen_dot = false;
    for c in &f[deg_digits..] {
        match *c {
            b'.' if !seen_dot => seen_dot = true,
            b'0'..=b'9' => {
                if seen_dot {
                    if frac_digits < 5 {
                        frac = frac * 10 + (*c - b'0') as i64;
                        frac_digits += 1;
                    }
                } else {
                    min_int = min_int * 10 + (*c - b'0') as i64;
                }
            }
            _ => return None,
        }
    }
    while frac_digits < 5 {
        frac *= 10;
        frac_digits += 1;
    }
    let minutes_e5 = min_int * 100_000 + frac;
    if min_int >= 60 {
        return None;
    }
    // minutes / 60 degrees = minutes_e5 / 6_000_000 degrees = * 10^7 / that.
    let e7 = deg * 10_000_000 + minutes_e5 * 10_000_000 / 6_000_000;
    i32::try_from(e7).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// All results of the sentences in `s`.
    fn all(p: &mut Parser, s: &str) -> std::vec::Vec<Sentence> {
        let mut out = std::vec::Vec::new();
        for b in s.bytes() {
            if let Some(x) = p.feed(b) {
                out.push(x);
                while let Some(y) = p.next() {
                    out.push(y);
                }
            }
        }
        out
    }

    fn mk(body: &str) -> std::string::String {
        let sum = body.bytes().fold(0u8, |a, c| a ^ c);
        std::format!("${body}*{sum:02X}\r\n")
    }

    #[test]
    fn rmc_fix() {
        let mut p = Parser::new();
        // The checksum below is not real: the sentence must be dropped.
        let r = all(&mut p, "$GNRMC,123519,A,5539.1234,N,01234.5678,E,0.5,84.4,230394,,*3F\r\n");
        assert!(r.is_empty());
        let r = all(&mut p, &mk("GNRMC,123519,A,5539.1234,N,01234.5678,E,0.5,84.4,230394,,"));
        assert_eq!(r[0], Sentence::Utc(123519));
        assert_eq!(r[1], Sentence::Fix { lat_e7: 556_520_566, lon_e7: 125_761_300 });
        // 0.5 knots = 25 cm/s, course 84
        assert_eq!(r[2], Sentence::Motion { speed_cms: 25, course: 84 });
    }

    #[test]
    fn southwest_and_no_fix() {
        let mut p = Parser::new();
        let r = all(&mut p, &mk("GPRMC,1,A,3351.0000,S,15112.0000,W,,,,,"));
        assert!(r.contains(&Sentence::Fix { lat_e7: -338_500_000, lon_e7: -1_512_000_000 }));
        let r = all(&mut p, &mk("GPRMC,1,V,,,,,,,,,"));
        assert!(r.contains(&Sentence::NoFix));
    }

    #[test]
    fn gga_satellites_height_accuracy() {
        let mut p = Parser::new();
        let r = all(&mut p, &mk("GPGGA,123519,5539.1234,N,01234.5678,E,1,08,0.9,10.0,M,,M,,"));
        assert_eq!(r, [Sentence::Sats(8), Sentence::Hdop(9), Sentence::Alt(100)]);
        let r = all(&mut p, &mk("GPGGA,123519,5539.1234,N,01234.5678,E,1,08,1.1,-3.5,M,,M,,"));
        assert_eq!(r[2], Sentence::Alt(-35));
    }

    #[test]
    fn gsv_satellites() {
        let mut p = Parser::new();
        let r = all(&mut p, &mk("GPGSV,3,1,11,03,03,111,00,04,15,270,32,06,01,010,,13,06,292,25"));
        assert_eq!(r[0], Sentence::InView(11));
        assert_eq!(r[1], Sentence::Sat { sys: 0, prn: 3, elevation: 3, azimuth: 111, snr: 0 });
        assert_eq!(r[2], Sentence::Sat { sys: 0, prn: 4, elevation: 15, azimuth: 270, snr: 32 });
        assert_eq!(r[3], Sentence::Sat { sys: 0, prn: 6, elevation: 1, azimuth: 10, snr: 0 });
        assert_eq!(r[4], Sentence::Sat { sys: 0, prn: 13, elevation: 6, azimuth: 292, snr: 25 });
        let r = all(&mut p, &mk("GAGSV,1,1,02,05,40,180,38,08,22,045,31"));
        assert!(matches!(r[1], Sentence::Sat { sys: 2, prn: 5, .. }));
    }

    #[test]
    fn gsa_used() {
        let mut p = Parser::new();
        let r = all(&mut p, &mk("GNGSA,A,3,04,05,,09,12,,,24,,,,,2.5,1.3,2.1"));
        let Sentence::Used { prns, n } = r[0] else { panic!() };
        assert_eq!(&prns[..n as usize], &[4u8, 5, 9, 12, 24][..]);
        assert_eq!(r[1], Sentence::Hdop(13));
    }

    #[test]
    fn garbage_is_ignored() {
        let mut p = Parser::new();
        assert!(all(&mut p, "\u{1}\u{2}$$$GPRMC,\r\n$\r\n").is_empty());
        let long = "A".repeat(300);
        assert!(all(&mut p, &std::format!("${long}\r\n")).is_empty());
    }
}
