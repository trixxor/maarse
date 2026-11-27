//! Packet format.
//!
//! ```text
//! +-------+------+----------------+--------------------------+---------+
//! | flags | hop  | destination    | path (optional)          | payload |
//! | 1 B   | 1 B  | 16 B           | 1 B count + 2 B per hop  | rest    |
//! +-------+------+----------------+--------------------------+---------+
//! ```
//!
//! The flags byte: bits 7-6 version, bits 5-2 packet type, bit 1 path present,
//! bit 0 region present (previously reserved for "compact mode").
//!
//! If bit 0 is set, 2 bytes of region (big endian) follow right after the path (or
//! after the destination if there is no path). See [`crate::region`].
//! The region is only used on floods (channels); a packet without a region is bit for
//! bit as before. Older nodes reject packets with bit 0 set and therefore
//! do not forward region messages – floods without a region work as always.
//!
//! The sender is never in the header. The AEAD tag is part of the payload
//! and is handled by the crypto layer.
//!
//! The meaning of the path field depends on the packet type:
//! * `PathRequest`: the path the request has taken so far (record route).
//! * all others: a source path the packet must follow. `hop` is the index of the
//!   next repeater in the path.

use crate::addr::{Address, PathId};
use crate::{MAX_HOPS, MAX_PACKET, MAX_PATH};

pub const VERSION: u8 = 0;
/// Fixed part of the header: flags + hop + destination.
pub const FIXED_HEADER: usize = 18;

const FLAG_HAS_PATH: u8 = 0b10;
const FLAG_REGION: u8 = 0b01;

pub type Path = heapless::Vec<PathId, MAX_PATH>;

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PacketType {
    /// Direct message to a node.
    Data = 0,
    /// Acknowledgement of a direct message.
    Ack = 1,
    /// Signed announcement of an identity.
    Announce = 2,
    /// Flood to find a route.
    PathRequest = 3,
    /// Reply with the route found.
    PathResponse = 4,
    /// Reserved: defined but never sent. Remote control of repeaters uses content
    /// type `0x02` inside an encrypted `Data` message.
    Control = 5,
    /// Fragment of a larger message or resource.
    Fragment = 6,
    /// Trace route: collects the hops and their SNR (not used for map coverage).
    Probe = 7,
    /// Channel message (flooded locally).
    Channel = 8,
    /// Message for a mailbox: the recipient's address (16) + encrypted message.
    Deposit = 9,
    /// Datagram: direct packet without end-to-end acknowledgement (used by
    /// the IP tunnel). Per-hop acknowledgement and retransmission still apply.
    Datagram = 10,
    /// Store-carry-forward: a direct message handed to one neighbour to carry
    /// (or to its destination), see `Router` "Carry". Addressed with a
    /// one-element path naming the receiving neighbour; never flooded.
    Carry = 11,
    /// Neighbor hello (never forwarded, `hops` 0): `dest` is the sender's
    /// address, payload `role (1) | flags (1) | chosen bridge (PathId, u16
    /// BE, 0xFFFF = none)`. Sent on a Trickle timer (RFC 6206): soon after
    /// something changes, then at doubling intervals. Tells neighbors our
    /// role, whether we hear a repeater, our edge bridge and whether we move,
    /// without the identity and proof of work of an announce.
    Hello = 12,
    /// Key request: "who is `dest`?". A local flood (scope `LOCAL_SCOPE`)
    /// with payload `requester (PathId, u16 BE) | nonce (u16 BE)`. The
    /// destination answers with its announce; a node that keeps announces
    /// (a host) may answer with the stored one.
    KeyRequest = 13,
    /// Mail sync: "I am here; do you have mail for me?" A flood of at most
    /// `mail::MAIL_SCOPE` hops; `dest` is the asker's address, payload its
    /// home post office (PathId u16 BE, 0xFFFF = none), the IDs of messages
    /// it already has (u32 BE each, at most 8) and a nonce (u16 BE). Post
    /// offices that hear it and hold its mail deliver; the home post office
    /// also fetches the copies (see `mail`).
    MailSync = 14,
    /// Trunk summary (never forwarded, `hops` 0): `dest` is the sender, the
    /// payload a Bloom filter (`bloom::BYTES`) of the nodes heard on its side
    /// of a long-distance trunk, so the far end sends a request across only
    /// for a node behind it.
    Summary = 15,
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
            10 => Datagram,
            11 => Carry,
            12 => Hello,
            13 => KeyRequest,
            14 => MailSync,
            15 => Summary,
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
    /// Header + payload is larger than a LoRa packet.
    TooLarge { needed: usize },
    /// The buffer is too small.
    BufferTooSmall,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub ptype: PacketType,
    pub hops: u8,
    pub dest: Address,
    pub path: Option<Path>,
    /// Region (0 = none). See [`crate::region`].
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

    /// How many bytes there is room for in the payload with this header.
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

    /// Decodes a header and returns it together with the number of bytes used.
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
                // Cannot fail: n <= MAX_PATH is checked above.
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

/// A decoded packet that borrows the payload from the receive buffer.
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

/// Encodes header + payload into a buffer that can be sent.
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
    // Cannot fail: needed <= MAX_PACKET.
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
        // Region flag without room for the region.
        let mut buf = encode(&h, b"x").unwrap();
        buf[0] |= 0b01;
        assert_eq!(Packet::decode(&buf), Err(DecodeError::TooShort));

        let mut buf = encode(&h, b"x").unwrap();
        buf[0] |= 1 << 6;
        assert_eq!(Packet::decode(&buf), Err(DecodeError::BadVersion(1)));

        // All 16 packet types are in use (15 = Summary); there is no unknown one left.
        let mut buf = encode(&h, b"x").unwrap();
        buf[0] = 15 << 2;
        assert_eq!(Packet::decode(&buf).map(|p| p.header.ptype), Ok(PacketType::Summary));

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
        buf[18] = 5; // longer than the buffer
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

        // Together with a path: the region comes after the path.
        let h = h.with_path(path(&[7, 8]));
        let buf = encode(&h, b"x").unwrap();
        assert_eq!(&buf[23..25], &[0xbe, 0xef]);
        assert_eq!(Packet::decode(&buf).unwrap().header, h);

        // Without a region the format is unchanged.
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
