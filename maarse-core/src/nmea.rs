//! NMEA parser for GPS modules (used by the firmware's GPS task).
//!
//! Only `RMC` (valid flag, position), `GGA` (satellites in use) and `GSV` (satellites in view) are read, from any talker
//! (`GP`, `GN`, `GL`, ...). A sentence with a wrong checksum is dropped.

/// What a sentence told us.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sentence {
    /// A valid position (degrees × 10^7).
    Fix { lat_e7: i32, lon_e7: i32 },
    /// RMC without a fix.
    NoFix,
    /// Satellites in use.
    Sats(u8),
    /// Satellites in view (the total from a GSV sentence).
    InView(u8),
}

/// Collects bytes into sentences.
pub struct Parser {
    line: heapless::Vec<u8, 100>,
    overflow: bool,
}

impl Default for Parser {
    fn default() -> Self {
        Self::new()
    }
}

impl Parser {
    pub const fn new() -> Self {
        Parser { line: heapless::Vec::new(), overflow: false }
    }

    /// Feeds one byte; returns a result when a sentence ends.
    pub fn feed(&mut self, b: u8) -> Option<Sentence> {
        match b {
            b'$' => {
                self.line.clear();
                self.overflow = false;
                None
            }
            b'\n' | b'\r' => {
                let out = if self.overflow || self.line.is_empty() { None } else { parse(&self.line) };
                self.line.clear();
                self.overflow = false;
                out
            }
            _ => {
                if self.line.push(b).is_err() {
                    self.overflow = true;
                }
                None
            }
        }
    }
}

fn hex(c: u8) -> Option<u8> {
    (c as char).to_digit(16).map(|d| d as u8)
}

/// Parses `GPRMC,...*hh` (without the leading `$`).
fn parse(s: &[u8]) -> Option<Sentence> {
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
            // time, status, lat, N/S, lon, E/W
            let _time = f.next()?;
            let status = f.next()?;
            if status != b"A" {
                return Some(Sentence::NoFix);
            }
            let lat = coord(f.next()?, 2)?;
            let ns = f.next()?;
            let lon = coord(f.next()?, 3)?;
            let ew = f.next()?;
            let lat = if ns == b"S" { -lat } else { lat };
            let lon = if ew == b"W" { -lon } else { lon };
            Some(Sentence::Fix { lat_e7: lat, lon_e7: lon })
        }
        b"GGA" => {
            // time, lat, N/S, lon, E/W, quality, satellites
            for _ in 0..5 {
                f.next()?;
            }
            let _quality = f.next()?;
            let sats = f.next()?;
            let mut n = 0u32;
            for c in sats {
                n = n * 10 + (c.checked_sub(b'0').filter(|d| *d < 10)? as u32);
            }
            Some(Sentence::Sats(n.min(255) as u8))
        }
        b"GSV" => {
            // sentences, this sentence, satellites in view
            f.next()?;
            f.next()?;
            let mut n = 0u32;
            for c in f.next()? {
                n = n * 10 + (c.checked_sub(b'0').filter(|d| *d < 10)? as u32);
            }
            Some(Sentence::InView(n.min(255) as u8))
        }
        _ => None,
    }
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

    fn feed_all(p: &mut Parser, s: &str) -> Option<Sentence> {
        let mut out = None;
        for b in s.bytes() {
            if let Some(x) = p.feed(b) {
                out = Some(x);
            }
        }
        out
    }

    #[test]
    fn rmc_fix() {
        let mut p = Parser::new();
        let r = feed_all(&mut p, "$GNRMC,123519,A,5539.1234,N,01234.5678,E,0.5,84.4,230394,,*3F\r\n");
        // Checksum above is not real: it must be dropped.
        assert_eq!(r, None);
        let body = "GNRMC,123519,A,5539.1234,N,01234.5678,E,0.5,84.4,230394,,";
        let sum = body.bytes().fold(0u8, |a, c| a ^ c);
        let line = std::format!("${body}*{sum:02X}\r\n");
        assert_eq!(
            feed_all(&mut p, &line),
            Some(Sentence::Fix { lat_e7: 556_520_566, lon_e7: 125_761_300 })
        );
    }

    #[test]
    fn southwest_and_no_fix() {
        let mut p = Parser::new();
        let mk = |body: &str| {
            let sum = body.bytes().fold(0u8, |a, c| a ^ c);
            std::format!("${body}*{sum:02X}\r\n")
        };
        let s = mk("GPRMC,1,A,3351.0000,S,15112.0000,W,,,,,");
        assert_eq!(feed_all(&mut p, &s), Some(Sentence::Fix { lat_e7: -338_500_000, lon_e7: -1_512_000_000 }));
        let s = mk("GPRMC,1,V,,,,,,,,,");
        assert_eq!(feed_all(&mut p, &s), Some(Sentence::NoFix));
    }

    #[test]
    fn gga_satellites() {
        let mut p = Parser::new();
        let body = "GPGGA,123519,5539.1234,N,01234.5678,E,1,08,0.9,10.0,M,,M,,";
        let sum = body.bytes().fold(0u8, |a, c| a ^ c);
        let line = std::format!("${body}*{sum:02X}\r\n");
        assert_eq!(feed_all(&mut p, &line), Some(Sentence::Sats(8)));
    }

    #[test]
    fn gsv_in_view() {
        let mut p = Parser::new();
        let body = "GPGSV,3,1,11,03,03,111,00,04,15,270,00,06,01,010,00,13,06,292,00";
        let sum = body.bytes().fold(0u8, |a, c| a ^ c);
        let line = std::format!("${body}*{sum:02X}
");
        assert_eq!(feed_all(&mut p, &line), Some(Sentence::InView(11)));
    }

    #[test]
    fn garbage_is_ignored() {
        let mut p = Parser::new();
        assert_eq!(feed_all(&mut p, "\u{1}\u{2}$$$GPRMC,\r\n$\r\n"), None);
        let long = "A".repeat(300);
        assert_eq!(feed_all(&mut p, &std::format!("${long}\r\n")), None);
    }
}
