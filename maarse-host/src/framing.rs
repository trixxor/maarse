//! Byte-stream framing for serial and KISS links (pure, no I/O).
//!
//! Both link types use KISS byte stuffing: a frame is `FEND | body | FEND`,
//! where `FEND` (0xC0) inside the body is sent as `FESC TFEND` and `FESC`
//! (0xDB) as `FESC TFESC`. Because `FEND` never occurs inside a frame, the
//! decoder resynchronises at the next `FEND` after line noise or a lost byte.
//!
//! * **Serial backbone link** (`--serial-link`): body = `packet | crc16 (BE)`,
//!   CRC-16/CCITT-FALSE (poly 0x1021, init 0xFFFF) over the packet. Frames
//!   with a bad CRC are dropped.
//! * **KISS TNC** (`--kiss`, `--kiss-tcp`): body = `command | data`. Only
//!   data frames on port 0 (command 0x00) carry packets; the TNC does its own
//!   checksumming on the air. With a callsign set, the packet is wrapped in an
//!   AX.25 UI frame (destination `QST`, PID 0xF0) so the station identifies
//!   itself on every transmission.

use maarse_core::MAX_PACKET;

pub const FEND: u8 = 0xC0;
pub const FESC: u8 = 0xDB;
pub const TFEND: u8 = 0xDC;
pub const TFESC: u8 = 0xDD;

/// KISS command bytes (low nibble; high nibble is the port).
pub const KISS_DATA: u8 = 0x00;
pub const KISS_TXDELAY: u8 = 0x01;
pub const KISS_PERSIST: u8 = 0x02;
pub const KISS_SLOTTIME: u8 = 0x03;

/// Largest unstuffed body we keep: a packet plus room for an AX.25 header
/// (16 bytes), the KISS command byte and a CRC. Longer frames are dropped.
pub const MAX_BODY: usize = MAX_PACKET + 32;

/// CRC-16/CCITT-FALSE.
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 };
        }
    }
    crc
}

/// Wraps `body` in FEND and escapes it.
pub fn stuff(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 8);
    out.push(FEND);
    for &b in body {
        match b {
            FEND => out.extend_from_slice(&[FESC, TFEND]),
            FESC => out.extend_from_slice(&[FESC, TFESC]),
            _ => out.push(b),
        }
    }
    out.push(FEND);
    out
}

/// Streaming unstuffer: push any bytes, take whole frame bodies.
/// Empty frames (back-to-back FENDs), frames longer than [`MAX_BODY`] and
/// frames with an invalid escape are dropped.
pub struct Unstuffer {
    buf: Vec<u8>,
    esc: bool,
    bad: bool,
    frames: std::collections::VecDeque<Vec<u8>>,
}

impl Default for Unstuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl Unstuffer {
    pub fn new() -> Self {
        Unstuffer { buf: Vec::new(), esc: false, bad: false, frames: Default::default() }
    }

    pub fn push(&mut self, data: &[u8]) {
        for &b in data {
            if b == FEND {
                if !self.bad && !self.esc && !self.buf.is_empty() {
                    self.frames.push_back(std::mem::take(&mut self.buf));
                }
                self.buf.clear();
                self.esc = false;
                self.bad = false;
                continue;
            }
            if self.bad {
                continue;
            }
            let v = if self.esc {
                self.esc = false;
                match b {
                    TFEND => FEND,
                    TFESC => FESC,
                    _ => {
                        self.bad = true;
                        continue;
                    }
                }
            } else if b == FESC {
                self.esc = true;
                continue;
            } else {
                b
            };
            if self.buf.len() >= MAX_BODY {
                self.bad = true;
                self.buf.clear();
                continue;
            }
            self.buf.push(v);
        }
    }

    pub fn next_body(&mut self) -> Option<Vec<u8>> {
        self.frames.pop_front()
    }
}

/// An AX.25 callsign with SSID, e.g. `OZ1ABC-7`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Callsign {
    pub call: String,
    pub ssid: u8,
}

impl Callsign {
    pub fn parse(s: &str) -> Result<Callsign, String> {
        let s = s.trim().to_ascii_uppercase();
        let (call, ssid) = match s.split_once('-') {
            Some((c, n)) => (c.to_string(), n.parse::<u8>().map_err(|_| format!("invalid SSID in '{s}'"))?),
            None => (s.clone(), 0),
        };
        if call.is_empty() || call.len() > 6 || !call.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(format!("invalid callsign '{s}' (1-6 letters/digits)"));
        }
        if ssid > 15 {
            return Err(format!("invalid SSID in '{s}' (0-15)"));
        }
        Ok(Callsign { call, ssid })
    }

    fn encode(&self, last: bool, out: &mut Vec<u8>) {
        let mut c = [b' '; 6];
        c[..self.call.len()].copy_from_slice(self.call.as_bytes());
        out.extend(c.iter().map(|b| b << 1));
        out.push(0x60 | (self.ssid << 1) | last as u8);
    }
}

/// AX.25 UI header length without digipeaters: two addresses, control, PID.
const AX25_HEADER: usize = 16;

/// Framing of one link type.
#[derive(Clone, Debug)]
pub enum Framing {
    /// Host-to-host serial line with CRC-16.
    SerialLink,
    /// KISS TNC; optionally an AX.25 UI wrapper with our callsign.
    Kiss { callsign: Option<Callsign> },
}

impl Framing {
    /// Encodes one Må:rse packet into the bytes to write.
    pub fn encode(&self, pkt: &[u8]) -> Vec<u8> {
        let mut body = Vec::with_capacity(pkt.len() + AX25_HEADER + 3);
        match self {
            Framing::SerialLink => {
                body.extend_from_slice(pkt);
                body.extend_from_slice(&crc16(pkt).to_be_bytes());
            }
            Framing::Kiss { callsign } => {
                body.push(KISS_DATA);
                if let Some(me) = callsign {
                    Callsign { call: "QST".into(), ssid: 0 }.encode(false, &mut body);
                    me.encode(true, &mut body);
                    body.extend_from_slice(&[0x03, 0xF0]);
                }
                body.extend_from_slice(pkt);
            }
        }
        stuff(&body)
    }

    /// Extracts the Må:rse packet from an unstuffed frame body, or `None` if
    /// the frame is damaged or is not a data frame for us.
    pub fn decode(&self, body: &[u8]) -> Option<Vec<u8>> {
        let pkt = match self {
            Framing::SerialLink => {
                if body.len() < 3 {
                    return None;
                }
                let (pkt, crc) = body.split_at(body.len() - 2);
                if crc16(pkt).to_be_bytes() != crc {
                    return None;
                }
                pkt
            }
            Framing::Kiss { callsign } => {
                let (&cmd, data) = body.split_first()?;
                if cmd != KISS_DATA {
                    return None; // Another port or a command echo.
                }
                if callsign.is_some() { strip_ax25(data)? } else { data }
            }
        };
        (!pkt.is_empty() && pkt.len() <= MAX_PACKET).then(|| pkt.to_vec())
    }
}

/// Removes an AX.25 UI header (any number of digipeaters), returns the info field.
fn strip_ax25(f: &[u8]) -> Option<&[u8]> {
    let mut i = 0;
    loop {
        let last = *f.get(i + 6)? & 1 == 1;
        i += 7;
        if last {
            break;
        }
        if i >= 7 * 10 {
            return None;
        }
    }
    if i < 14 || f.get(i)? & 0xEF != 0x03 || *f.get(i + 1)? != 0xF0 {
        return None;
    }
    Some(&f[i + 2..])
}

/// A KISS parameter command frame (`--kiss-txdelay` etc.).
pub fn kiss_param(cmd: u8, value: u8) -> Vec<u8> {
    stuff(&[cmd & 0x0F, value])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(f: &Framing, pkt: &[u8]) -> Vec<Vec<u8>> {
        let mut u = Unstuffer::new();
        u.push(&f.encode(pkt));
        let mut out = Vec::new();
        while let Some(b) = u.next_body() {
            out.extend(f.decode(&b));
        }
        out
    }

    #[test]
    fn crc_check_value() {
        assert_eq!(crc16(b"123456789"), 0x29B1);
    }

    #[test]
    fn stuffing_escapes_specials() {
        assert_eq!(stuff(&[1, FEND, FESC, 2]), vec![FEND, 1, FESC, TFEND, FESC, TFESC, 2, FEND]);
    }

    #[test]
    fn serial_link_roundtrip_with_specials() {
        let pkt: Vec<u8> = (0..MAX_PACKET).map(|i| (i * 7) as u8 ^ 0xC0).collect();
        assert_eq!(roundtrip(&Framing::SerialLink, &pkt), vec![pkt]);
    }

    #[test]
    fn kiss_roundtrip_plain_and_ax25() {
        let pkt = vec![0x30, FEND, 7, FESC, 9];
        assert_eq!(roundtrip(&Framing::Kiss { callsign: None }, &pkt), vec![pkt.clone()]);
        let f = Framing::Kiss { callsign: Some(Callsign::parse("oz1abc-7").unwrap()) };
        let enc = f.encode(&pkt);
        assert_eq!(&enc[..3], &[FEND, 0x00, b'Q' << 1]);
        assert_eq!(roundtrip(&f, &pkt), vec![pkt]);
    }

    #[test]
    fn kiss_ignores_other_ports_and_commands() {
        let f = Framing::Kiss { callsign: None };
        assert_eq!(f.decode(&[0x10, 1, 2]), None); // port 1
        assert_eq!(f.decode(&[KISS_TXDELAY, 50]), None);
        assert_eq!(f.decode(&[0x00]), None); // empty data
    }

    #[test]
    fn bad_crc_is_dropped() {
        let f = Framing::SerialLink;
        let mut enc = f.encode(b"hello");
        enc[2] ^= 0x01;
        let mut u = Unstuffer::new();
        u.push(&enc);
        let b = u.next_body().unwrap();
        assert_eq!(f.decode(&b), None);
    }

    #[test]
    fn resync_after_garbage_and_split_reads() {
        let f = Framing::SerialLink;
        let mut stream = vec![0x55, 0xAA, FESC, 0x01, 0x99]; // noise, incl. a bad escape
        stream.extend(f.encode(b"one"));
        stream.extend([0x12, 0x34]); // noise between frames
        stream.extend(f.encode(&[FEND, FESC, 3]));
        let mut out = Vec::new();
        // Feed one byte at a time.
        let mut u = Unstuffer::new();
        for b in &stream {
            u.push(std::slice::from_ref(b));
            while let Some(body) = u.next_body() {
                out.extend(f.decode(&body));
            }
        }
        assert_eq!(out, vec![b"one".to_vec(), vec![FEND, FESC, 3]]);
    }

    #[test]
    fn overlong_frame_is_dropped_then_recovers() {
        let mut u = Unstuffer::new();
        let mut s = vec![FEND];
        s.extend(std::iter::repeat_n(0x41, MAX_BODY + 10));
        s.push(FEND);
        s.extend(Framing::SerialLink.encode(b"ok"));
        u.push(&s);
        let b = u.next_body().unwrap();
        assert_eq!(Framing::SerialLink.decode(&b), Some(b"ok".to_vec()));
        assert!(u.next_body().is_none());
    }

    #[test]
    fn callsign_parsing() {
        assert_eq!(Callsign::parse("N0CALL").unwrap(), Callsign { call: "N0CALL".into(), ssid: 0 });
        assert!(Callsign::parse("TOOLONG1").is_err());
        assert!(Callsign::parse("AB-16").is_err());
        assert!(Callsign::parse("").is_err());
    }

    #[test]
    fn kiss_param_frame() {
        assert_eq!(kiss_param(KISS_TXDELAY, 30), vec![FEND, 0x01, 30, FEND]);
    }

    #[test]
    fn udp_roundtrip_and_strays() {
        let d = udp_encode(0xDEADBEEF, b"pkt");
        assert_eq!(udp_decode(&d), Some((0xDEADBEEF, &b"pkt"[..])));
        assert_eq!(udp_decode(&d[..UDP_PREFIX]), None); // empty packet
        assert_eq!(udp_decode(b"GET / HTTP/1.1\r\n"), None);
        let mut v2 = d.clone();
        v2[2] = 2;
        assert_eq!(udp_decode(&v2), None); // other version
        assert_eq!(udp_decode(&udp_encode(1, &vec![0; MAX_PACKET + 1])), None);
    }
}

/// UDP datagram prefix: magic `M 0xA5`, version 1, then the sender's
/// instance id (u32 BE), which lets a host drop its own broadcasts.
pub const UDP_MAGIC: [u8; 3] = [b'M', 0xA5, 1];
pub const UDP_PREFIX: usize = 7;

/// Encodes one packet as a UDP datagram.
pub fn udp_encode(instance: u32, pkt: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(UDP_PREFIX + pkt.len());
    v.extend_from_slice(&UDP_MAGIC);
    v.extend_from_slice(&instance.to_be_bytes());
    v.extend_from_slice(pkt);
    v
}

/// Decodes a datagram into `(sender instance, packet)`; stray datagrams give `None`.
pub fn udp_decode(d: &[u8]) -> Option<(u32, &[u8])> {
    if d.len() <= UDP_PREFIX || d[..3] != UDP_MAGIC || d.len() - UDP_PREFIX > MAX_PACKET {
        return None;
    }
    Some((u32::from_be_bytes([d[3], d[4], d[5], d[6]]), &d[UDP_PREFIX..]))
}
