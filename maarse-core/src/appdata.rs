//! App-data i announces: postkasse, forward secrecy, position og telemetri.
//!
//! ```text
//! 0x01 postkasse | adresse (16)          (fast længde, ældste felt)
//! 0x02 FS                                 (ingen data)
//! 0x03 | længde | position               (fra og med 0x03: type | længde | data)
//! 0x04 | længde | telemetri
//! 0x05 | længde | visningsnavn            (UTF-8, højst 24 bytes)
//! ```
//!
//! Nye felter skal stå efter `0x02`: ældre noder læser feltet `0x02` og
//! stopper ved det første felt, de ikke kender. Fra og med `0x03` har alle
//! felter en længde, så fremtidige felter kan springes over.
//!
//! Position (9 bytes): præcision (1) | bredde (i32, 1e-7 grader) | længde
//! (i32). Positionen er allerede afrundet til præcisionens gitter, før den
//! sendes (se [`Position::quantised`]); modtageren ser kun cellens midte.
//!
//! Telemetri (10 bytes): batteri mV (u16, 0 = ukendt) | flag (bit 0 =
//! ekstern strøm/USB, bit 1 = temperatur gyldig) | energistatus (0-3) |
//! oppetid s (u32) | temperatur (i16, 0,1 °C).
//!
//! Visningsnavn: valgfrit, selvvalgt og ikke bevist. Det må kun bruges til
//! visning; kontakter kendes stadig på adresse og sikkerhedsnummer.
//!
//! Alt i alt fylder det højst 17 + 1 + 11 + 12 + 26 = 67 bytes, under
//! `MAX_APP_DATA` (99 bytes efter arbejdsbeviset).

use crate::addr::Address;

pub const APP_POSTBOX: u8 = 0x01;
pub const APP_FS: u8 = 0x02;
pub const APP_POSITION: u8 = 0x03;
pub const APP_TELEMETRY: u8 = 0x04;
pub const APP_NAME: u8 = 0x05;

/// Længste visningsnavn i bytes (UTF-8).
pub const MAX_NAME: usize = 24;

pub type AppBuf = heapless::Vec<u8, 96>;
pub type Name = heapless::Vec<u8, MAX_NAME>;

/// Renser et visningsnavn: fjerner styretegn, trimmer mellemrum og
/// afkorter til [`MAX_NAME`] bytes uden at skære et tegn over. Tomt
/// navn giver `None`.
pub fn clean_name(raw: &[u8]) -> Option<Name> {
    let s = core::str::from_utf8(raw).ok()?;
    let mut out = Name::new();
    let mut buf = [0u8; 4];
    for c in s.trim().chars().filter(|c| !c.is_control()) {
        let e = c.encode_utf8(&mut buf).as_bytes();
        if out.extend_from_slice(e).is_err() {
            break;
        }
    }
    // Afslut ikke med mellemrum, hvis afkortningen endte der.
    while out.last() == Some(&b' ') {
        out.pop();
    }
    (!out.is_empty()).then_some(out)
}

/// Hvor præcist positionen deles. Standard er `Off` (deles ikke).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum Precision {
    #[default]
    Off = 0,
    /// 0,1° (ca. 11 km nord-syd, 6 km øst-vest i Danmark).
    Km10 = 1,
    /// 0,01° (ca. 1 km).
    Km1 = 2,
    /// 0,00001° (ca. 1 m): kun hvis brugeren udtrykkeligt vælger det.
    Precise = 3,
}

impl Precision {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Precision::Km10,
            2 => Precision::Km1,
            3 => Precision::Precise,
            _ => Precision::Off,
        }
    }
    /// Gitterets størrelse i 1e-7 grader.
    fn step(self) -> i32 {
        match self {
            Precision::Off => 0,
            Precision::Km10 => 1_000_000,
            Precision::Km1 => 100_000,
            Precision::Precise => 100,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Position {
    pub precision: Precision,
    pub lat_e7: i32,
    pub lon_e7: i32,
}

fn snap(v: i32, step: i32) -> i32 {
    if step <= 1 {
        return v;
    }
    // Midten af cellen: afslører ikke mere end cellen.
    (v.div_euclid(step) * step).saturating_add(step / 2)
}

impl Position {
    /// Afrundet til præcisionens gitter. `None` ved `Off` eller ugyldige
    /// koordinater.
    pub fn quantised(self) -> Option<Position> {
        let step = self.precision.step();
        if step == 0
            || !(-900_000_000..=900_000_000).contains(&self.lat_e7)
            || !(-1_800_000_000..=1_800_000_000).contains(&self.lon_e7)
        {
            return None;
        }
        Some(Position {
            precision: self.precision,
            lat_e7: snap(self.lat_e7, step),
            lon_e7: snap(self.lon_e7, step),
        })
    }

    fn encode(&self, out: &mut AppBuf) {
        let _ = out.extend_from_slice(&[APP_POSITION, 9, self.precision as u8]);
        let _ = out.extend_from_slice(&self.lat_e7.to_le_bytes());
        let _ = out.extend_from_slice(&self.lon_e7.to_le_bytes());
    }

    fn decode(d: &[u8]) -> Option<Self> {
        if d.len() < 9 {
            return None;
        }
        let p = Precision::from_u8(d[0]);
        if p == Precision::Off {
            return None;
        }
        Some(Position {
            precision: p,
            lat_e7: i32::from_le_bytes(d[1..5].try_into().ok()?),
            lon_e7: i32::from_le_bytes(d[5..9].try_into().ok()?),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Telemetry {
    /// 0 = ukendt (ingen batterimåling på boardet).
    pub battery_mv: u16,
    pub ext_power: bool,
    /// [`crate::Energy`] som tal (0 = kritisk ... 3 = fuld).
    pub energy: u8,
    pub uptime_s: u32,
    /// Temperatur i 0,1 °C, hvis kendt.
    pub temp_dc: Option<i16>,
}

impl Telemetry {
    fn encode(&self, out: &mut AppBuf) {
        let flags = self.ext_power as u8 | (self.temp_dc.is_some() as u8) << 1;
        let _ = out.extend_from_slice(&[APP_TELEMETRY, 10]);
        let _ = out.extend_from_slice(&self.battery_mv.to_le_bytes());
        let _ = out.extend_from_slice(&[flags, self.energy]);
        let _ = out.extend_from_slice(&self.uptime_s.to_le_bytes());
        let _ = out.extend_from_slice(&self.temp_dc.unwrap_or(0).to_le_bytes());
    }

    fn decode(d: &[u8]) -> Option<Self> {
        if d.len() < 10 {
            return None;
        }
        Some(Telemetry {
            battery_mv: u16::from_le_bytes([d[0], d[1]]),
            ext_power: d[2] & 1 != 0,
            energy: d[3].min(3),
            uptime_s: u32::from_le_bytes(d[4..8].try_into().ok()?),
            temp_dc: (d[2] & 2 != 0).then(|| i16::from_le_bytes([d[8], d[9]])),
        })
    }
}

/// Afkodet app-data.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppData {
    pub postbox: Option<Address>,
    pub fs: bool,
    pub position: Option<Position>,
    pub telemetry: Option<Telemetry>,
    pub name: Option<Name>,
}

/// Bygger app-data. Positionen afrundes her (en præcis position sendes
/// kun med `Precision::Precise`).
pub fn build(
    postbox: Option<&Address>,
    fs: bool,
    position: Option<Position>,
    telemetry: Option<&Telemetry>,
    name: Option<&[u8]>,
) -> AppBuf {
    let mut v = AppBuf::new();
    if let Some(p) = postbox {
        let _ = v.push(APP_POSTBOX);
        let _ = v.extend_from_slice(&p.0);
    }
    if fs {
        let _ = v.push(APP_FS);
    }
    if let Some(p) = position.and_then(Position::quantised) {
        p.encode(&mut v);
    }
    if let Some(t) = telemetry {
        t.encode(&mut v);
    }
    if let Some(n) = name.and_then(clean_name) {
        let _ = v.extend_from_slice(&[APP_NAME, n.len() as u8]);
        let _ = v.extend_from_slice(&n);
    }
    v
}

pub fn parse(b: &[u8]) -> AppData {
    let mut a = AppData::default();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            APP_POSTBOX => {
                a.postbox = b.get(i + 1..i + 17).and_then(Address::from_slice);
                i += 17;
            }
            APP_FS => {
                a.fs = true;
                i += 1;
            }
            t if t >= APP_POSITION => {
                let Some(&len) = b.get(i + 1) else { break };
                let Some(d) = b.get(i + 2..i + 2 + len as usize) else { break };
                match t {
                    APP_POSITION => a.position = Position::decode(d),
                    APP_TELEMETRY => a.telemetry = Telemetry::decode(d),
                    APP_NAME => a.name = clean_name(d),
                    _ => {}
                }
                i += 2 + len as usize;
            }
            _ => break,
        }
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    const AARHUS: (i32, i32) = (561_567_000, 102_107_000); // 56,1567 N, 10,2107 Ø

    fn pos(p: Precision) -> Position {
        Position { precision: p, lat_e7: AARHUS.0, lon_e7: AARHUS.1 }
    }

    #[test]
    fn quantisation() {
        assert_eq!(pos(Precision::Off).quantised(), None);
        let q = pos(Precision::Km1).quantised().unwrap();
        assert_eq!((q.lat_e7, q.lon_e7), (561_550_000, 102_150_000));
        let q = pos(Precision::Km10).quantised().unwrap();
        assert_eq!((q.lat_e7, q.lon_e7), (561_500_000, 102_500_000));
        let q = pos(Precision::Precise).quantised().unwrap();
        assert_eq!((q.lat_e7, q.lon_e7), (561_567_050, 102_107_050));
        // Negative koordinater afrundes til cellen nedenunder.
        let s = Position { precision: Precision::Km1, lat_e7: -334_567, lon_e7: -1 };
        let q = s.quantised().unwrap();
        assert_eq!((q.lat_e7, q.lon_e7), (-350_000, -50_000));
        assert!(Position { lat_e7: 950_000_000, ..pos(Precision::Km1) }.quantised().is_none());
    }

    #[test]
    fn roundtrip_and_old_parser() {
        let t = Telemetry { battery_mv: 3870, ext_power: true, energy: 2, uptime_s: 99, temp_dc: Some(-15) };
        let b = build(Some(&Address([9; 16])), true, Some(pos(Precision::Km1)), Some(&t), None);
        assert!(b.len() <= 41);
        let a = parse(&b);
        assert_eq!(a.postbox, Some(Address([9; 16])));
        assert!(a.fs);
        assert_eq!(a.position, pos(Precision::Km1).quantised());
        assert_eq!(a.telemetry, Some(t));
        // Ældre noders FS-tjek ser stadig FS.
        assert!(b.contains(&APP_FS));
        // Uden position (standard) sendes ingen.
        let b = build(None, true, Some(pos(Precision::Off)), None, None);
        assert_eq!(&b[..], &[APP_FS]);
        // Ukendt fremtidigt felt springes over.
        let mut b = AppBuf::new();
        b.extend_from_slice(&[APP_FS, 0x20, 2, 1, 1]).unwrap();
        t.encode(&mut b);
        assert_eq!(parse(&b).telemetry, Some(t));
        // Afkortet felt giver ingen panik.
        assert_eq!(parse(&[APP_POSITION, 9, 1]).position, None);
    }

    #[test]
    fn names() {
        let long = "Æblegrød-og-flødeskum-hver-dag";
        let b = build(Some(&Address([9; 16])), true, Some(pos(Precision::Precise)), None, Some(long.as_bytes()));
        let a = parse(&b);
        let n = a.name.unwrap();
        assert!(n.len() <= MAX_NAME);
        assert!(core::str::from_utf8(&n).is_ok());
        assert!(long.starts_with(core::str::from_utf8(&n).unwrap()));
        assert!(a.position.is_some());
        // Fuldt udfyldt app-data er under grænsen.
        let t = Telemetry { battery_mv: 1, ..Default::default() };
        let full = build(Some(&Address([9; 16])), true, Some(pos(Precision::Km1)), Some(&t), Some(long.as_bytes()));
        assert!(full.len() <= 67);
        assert!(parse(&full).telemetry.is_some());
        assert_eq!(clean_name(b"  
 "), None);
        assert_eq!(&clean_name(b" Ida ").unwrap()[..], b"Ida");
        assert_eq!(clean_name(&[0xff, 0xfe]), None);
        // Ældre parser (uden 0x05) springer feltet over: fremtidige felter har længde.
        assert_eq!(parse(&[APP_NAME, 3, b'I', b'd', b'a', APP_FS]).name.as_deref(), Some(&b"Ida"[..]));
    }
}
