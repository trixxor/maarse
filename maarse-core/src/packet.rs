//! Pakkeformat.
//!
//! ```text
//! +-------+------+----------------+--------------------------+---------+
//! | flag  | hop  | destination    | sti (valgfri)            | payload |
//! | 1 B   | 1 B  | 16 B           | 1 B antal + 2 B pr. hop  | resten  |
//! +-------+------+----------------+--------------------------+---------+
//! ```
//!
//! Flag-byten: bit 7-6 version, bit 5-2 pakketype, bit 1 sti til stede,
//! bit 0 region til stede (tidligere reserveret til "kompakt tilstand").
//!
//! Er bit 0 sat, følger 2 bytes region (big endian) lige efter stien (eller
//! efter destinationen, hvis der ikke er nogen sti). Se [`crate::region`].
//! Regionen bruges kun på floods (kanaler); en pakke uden region er bit for
//! bit som før. Ældre noder afviser pakker med bit 0 sat og videresender
//! derfor ikke regionsbeskeder – floods uden region virker som altid.
//!
//! Afsenderen står aldrig i headeren. AEAD-tagget er en del af payloaden
//! og håndteres af kryptolaget.
//!
//! Sti-feltets betydning afhænger af pakketypen:
//! * `PathRequest`: den sti, forespørgslen har taget indtil nu (record route).
//! * alle andre: en kildesti, som pakken skal følge. `hop` er indekset for
//!   næste repeater i stien.

use crate::addr::{Address, PathId};
use crate::{MAX_HOPS, MAX_PACKET, MAX_PATH};

pub const VERSION: u8 = 0;
/// Fast del af headeren: flag + hop + destination.
pub const FIXED_HEADER: usize = 18;

const FLAG_HAS_PATH: u8 = 0b10;
const FLAG_REGION: u8 = 0b01;

pub type Path = heapless::Vec<PathId, MAX_PATH>;

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PacketType {
    /// Direkte besked til en node.
    Data = 0,
    /// Kvittering for en direkte besked.
    Ack = 1,
    /// Signeret meddelelse om en identitet.
    Announce = 2,
    /// Flood for at finde en rute.
    PathRequest = 3,
    /// Svar med den fundne rute.
    PathResponse = 4,
    /// Fjernstyring af repeatere.
    Control = 5,
    /// Fragment af en større besked eller ressource.
    Fragment = 6,
    /// Dækningsmåling til kortet.
    Probe = 7,
    /// Kanalbesked (floodes lokalt).
    Channel = 8,
    /// Besked til en postkasse: modtagerens adresse (16) + krypteret besked.
    Deposit = 9,
}

impl TryFrom<u8> for PacketType {
    type Error = DecodeError;
    fn try_from(v: u8) -> Result<Self, DecodeError> {
        use PacketType::*;
        Ok(match v {
            0 => Data,
            1 => Ack,
            2 => Announce,
            3 => PathRequest,
            4 => PathResponse,
            5 => Control,
            6 => Fragment,
            7 => Probe,
            8 => Channel,
            9 => Deposit,
            _ => return Err(DecodeError::UnknownType(v)),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    TooShort,
    BadVersion(u8),
    UnknownType(u8),
    PathTooLong(u8),
    HopLimit(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncodeError {
    /// Header + payload er større end en LoRa-pakke.
    TooLarge { needed: usize },
    /// Bufferen er for lille.
    BufferTooSmall,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub ptype: PacketType,
    pub hops: u8,
    pub dest: Address,
    pub path: Option<Path>,
    /// Region (0 = ingen). Se [`crate::region`].
    pub region: u16,
}

impl Header {
    pub fn new(ptype: PacketType, dest: Address) -> Self {
        Header {
            ptype,
            hops: 0,
            dest,
            path: None,
            region: 0,
        }
    }

    pub fn with_region(mut self, region: u16) -> Self {
        self.region = region;
        self
    }

    pub fn with_path(mut self, path: Path) -> Self {
        self.path = Some(path);
        self
    }

    pub fn encoded_len(&self) -> usize {
        FIXED_HEADER
            + self.path.as_ref().map_or(0, |p| 1 + 2 * p.len())
            + if self.region != 0 { 2 } else { 0 }
    }

    /// Hvor mange bytes der er plads til i payloaden med denne header.
    pub fn max_payload(&self) -> usize {
        MAX_PACKET - self.encoded_len()
    }

    pub fn encode(&self, out: &mut [u8]) -> Result<usize, EncodeError> {
        let len = self.encoded_len();
        if out.len() < len {
            return Err(EncodeError::BufferTooSmall);
        }
        let mut flags = (VERSION << 6) | ((self.ptype as u8) << 2);
        if self.path.is_some() {
            flags |= FLAG_HAS_PATH;
        }
        if self.region != 0 {
            flags |= FLAG_REGION;
        }
        out[0] = flags;
        out[1] = self.hops;
        out[2..18].copy_from_slice(&self.dest.0);
        if let Some(path) = &self.path {
            out[18] = path.len() as u8;
            for (i, id) in path.iter().enumerate() {
                out[19 + 2 * i..21 + 2 * i].copy_from_slice(&id.to_be_bytes());
            }
        }
        if self.region != 0 {
            out[len - 2..len].copy_from_slice(&self.region.to_be_bytes());
        }
        Ok(len)
    }

    /// Afkoder en header og returnerer den sammen med antal brugte bytes.
    pub fn decode(buf: &[u8]) -> Result<(Header, usize), DecodeError> {
        if buf.len() < FIXED_HEADER {
            return Err(DecodeError::TooShort);
        }
        let flags = buf[0];
        let version = flags >> 6;
        if version != VERSION {
            return Err(DecodeError::BadVersion(version));
        }
        let ptype = PacketType::try_from((flags >> 2) & 0x0f)?;
        let hops = buf[1];
        if hops > MAX_HOPS {
            return Err(DecodeError::HopLimit(hops));
        }
        let dest = Address::from_slice(&buf[2..18]).ok_or(DecodeError::TooShort)?;
        let mut used = FIXED_HEADER;
        let path = if flags & FLAG_HAS_PATH != 0 {
            let n = *buf.get(18).ok_or(DecodeError::TooShort)?;
            if n as usize > MAX_PATH {
                return Err(DecodeError::PathTooLong(n));
            }
            let end = 19 + 2 * n as usize;
            if buf.len() < end {
                return Err(DecodeError::TooShort);
            }
            let mut p = Path::new();
            for chunk in buf[19..end].chunks_exact(2) {
                // Kan ikke fejle: n <= MAX_PATH er tjekket ovenfor.
                let _ = p.push(u16::from_be_bytes([chunk[0], chunk[1]]));
            }
            used = end;
            Some(p)
        } else {
            None
        };
        let mut region = 0;
        if flags & FLAG_REGION != 0 {
            let b = buf.get(used..used + 2).ok_or(DecodeError::TooShort)?;
            region = u16::from_be_bytes([b[0], b[1]]);
            used += 2;
        }
        Ok((
            Header {
                ptype,
                hops,
                dest,
                path,
                region,
            },
            used,
        ))
    }
}

/// En afkodet pakke, der låner payloaden fra modtagebufferen.
#[derive(Debug, PartialEq, Eq)]
pub struct Packet<'a> {
    pub header: Header,
    pub payload: &'a [u8],
}

impl<'a> Packet<'a> {
    pub fn decode(buf: &'a [u8]) -> Result<Self, DecodeError> {
        if buf.len() > MAX_PACKET {
            return Err(DecodeError::TooShort);
        }
        let (header, used) = Header::decode(buf)?;
        Ok(Packet {
            header,
            payload: &buf[used..],
        })
    }
}

/// Koder header + payload til en buffer, der kan sendes.
pub fn encode(
    header: &Header,
    payload: &[u8],
) -> Result<heapless::Vec<u8, MAX_PACKET>, EncodeError> {
    let needed = header.encoded_len() + payload.len();
    if needed > MAX_PACKET {
        return Err(EncodeError::TooLarge { needed });
    }
    let mut buf = [0u8; MAX_PACKET];
    let n = header.encode(&mut buf)?;
    buf[n..n + payload.len()].copy_from_slice(payload);
    // Kan ikke fejle: needed <= MAX_PACKET.
    Ok(heapless::Vec::from_slice(&buf[..needed]).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(ids: &[u16]) -> Path {
        Path::from_slice(ids).unwrap()
    }

    #[test]
    fn roundtrip_without_path() {
        let h = Header::new(PacketType::Channel, Address([9; 16]));
        let buf = encode(&h, b"hej").unwrap();
        assert_eq!(buf.len(), 18 + 3);
        let p = Packet::decode(&buf).unwrap();
        assert_eq!(p.header, h);
        assert_eq!(p.payload, b"hej");
    }

    #[test]
    fn roundtrip_with_full_path() {
        let ids: [u16; 12] = core::array::from_fn(|i| i as u16 * 1000);
        let mut h = Header::new(PacketType::Data, Address([1; 16])).with_path(path(&ids));
        h.hops = 5;
        assert_eq!(h.encoded_len(), 18 + 1 + 24);
        let payload = [0x55u8; 100];
        let buf = encode(&h, &payload).unwrap();
        let p = Packet::decode(&buf).unwrap();
        assert_eq!(p.header, h);
        assert_eq!(p.payload, &payload[..]);
    }

    #[test]
    fn rejects_too_large() {
        let h = Header::new(PacketType::Data, Address::default());
        let payload = [0u8; MAX_PACKET - FIXED_HEADER + 1];
        assert_eq!(
            encode(&h, &payload),
            Err(EncodeError::TooLarge {
                needed: MAX_PACKET + 1
            })
        );
        assert!(encode(&h, &payload[..h.max_payload()]).is_ok());
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!(Packet::decode(&[0; 10]), Err(DecodeError::TooShort));

        let h = Header::new(PacketType::Data, Address::default());
        // Regionsflag uden plads til regionen.
        let mut buf = encode(&h, b"x").unwrap();
        buf[0] |= 0b01;
        assert_eq!(Packet::decode(&buf), Err(DecodeError::TooShort));

        let mut buf = encode(&h, b"x").unwrap();
        buf[0] |= 1 << 6;
        assert_eq!(Packet::decode(&buf), Err(DecodeError::BadVersion(1)));

        let mut buf = encode(&h, b"x").unwrap();
        buf[0] = 15 << 2;
        assert_eq!(Packet::decode(&buf), Err(DecodeError::UnknownType(15)));

        let mut buf = encode(&h, b"x").unwrap();
        buf[1] = MAX_HOPS + 1;
        assert_eq!(
            Packet::decode(&buf),
            Err(DecodeError::HopLimit(MAX_HOPS + 1))
        );
    }

    #[test]
    fn rejects_bad_path() {
        let h = Header::new(PacketType::Data, Address::default()).with_path(path(&[1, 2]));
        let mut buf = encode(&h, b"").unwrap();
        buf[18] = 13;
        assert_eq!(Packet::decode(&buf), Err(DecodeError::PathTooLong(13)));
        buf[18] = 5; // længere end bufferen
        assert_eq!(Packet::decode(&buf), Err(DecodeError::TooShort));
    }

    #[test]
    fn roundtrip_with_region() {
        let h = Header::new(PacketType::Channel, Address([3; 16])).with_region(0xbeef);
        assert_eq!(h.encoded_len(), 20);
        let buf = encode(&h, b"hej").unwrap();
        assert_eq!(buf[0] & 1, 1);
        assert_eq!(&buf[18..20], &[0xbe, 0xef]);
        let p = Packet::decode(&buf).unwrap();
        assert_eq!(p.header, h);
        assert_eq!(p.payload, b"hej");

        // Sammen med sti: regionen står efter stien.
        let h = h.with_path(path(&[7, 8]));
        let buf = encode(&h, b"x").unwrap();
        assert_eq!(&buf[23..25], &[0xbe, 0xef]);
        assert_eq!(Packet::decode(&buf).unwrap().header, h);

        // Uden region er formatet uændret.
        let h = Header::new(PacketType::Channel, Address([3; 16]));
        assert_eq!(encode(&h, b"x").unwrap()[0] & 1, 0);
    }

    #[test]
    fn all_types_roundtrip() {
        for t in 0u8..=9 {
            let ty = PacketType::try_from(t).unwrap();
            let h = Header::new(ty, Address([t; 16]));
            let buf = encode(&h, &[t]).unwrap();
            assert_eq!(Packet::decode(&buf).unwrap().header.ptype, ty);
        }
    }
}
