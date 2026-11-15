//! Load control: send-queue priorities and a flood slot that follows the
//! radio settings.
//!
//! The radio layer only sees encoded packets, so the priority is read from
//! the first header byte (packet type and the path flag). Lower numbers go
//! first. A packet that is not a Må:rse packet (e.g. a range test beacon)
//! gets [`PRIO_DIRECT`].

use crate::Address;
use crate::airtime::LoraParams;
use crate::packet::{PacketType, VERSION};

/// Acknowledgements, path responses and carry "delivered" notices: small,
/// and holding them back makes the sender retransmit.
pub const PRIO_CONTROL: u8 = 0;
/// Direct messages, datagrams, carried messages, mailbox deposits, remote control.
pub const PRIO_DIRECT: u8 = 1;
/// Channel messages.
pub const PRIO_CHANNEL: u8 = 2;
/// Fragments (files, voice, long messages): can be interrupted between pieces.
pub const PRIO_BULK: u8 = 3;
/// Path requests, announces and probes: floods that can wait.
pub const PRIO_FLOOD: u8 = 4;

const FLAG_HAS_PATH: u8 = 0b10;

/// The send priority of an encoded packet (0 = most urgent).
pub fn priority(packet: &[u8]) -> u8 {
    let Some(&flags) = packet.first() else {
        return PRIO_DIRECT;
    };
    if flags >> 6 != VERSION {
        return PRIO_DIRECT;
    }
    let Ok(ptype) = PacketType::try_from((flags >> 2) & 0x0f) else {
        return PRIO_DIRECT;
    };
    match ptype {
        PacketType::Ack | PacketType::PathResponse => PRIO_CONTROL,
        // A Carry packet without a path is the "delivered" notice.
        PacketType::Carry if flags & FLAG_HAS_PATH == 0 => PRIO_CONTROL,
        PacketType::Data
        | PacketType::Datagram
        | PacketType::Carry
        | PacketType::Deposit
        | PacketType::Control => PRIO_DIRECT,
        PacketType::Channel => PRIO_CHANNEL,
        PacketType::Fragment => PRIO_BULK,
        PacketType::PathRequest | PacketType::Announce | PacketType::Probe => PRIO_FLOOD,
    }
}

/// Like [`priority`], but channel messages to `urgent_channel` (the signed
/// bulletin channel) are sent with [`PRIO_DIRECT`] instead of [`PRIO_CHANNEL`].
pub fn priority_with(packet: &[u8], urgent_channel: &Address) -> u8 {
    let p = priority(packet);
    if p == PRIO_CHANNEL && packet.get(2..18) == Some(&urgent_channel.0[..]) {
        PRIO_DIRECT
    } else {
        p
    }
}

/// Length of the "short packet" a flood slot is sized for (an ack or a
/// small path request), in bytes.
const SLOT_PACKET: usize = 24;

/// Flood slot length for these radio settings: about the airtime of a short
/// packet. About 200 ms at SF9/125 kHz (the fixed default) and 1.3 s at SF12.
///
/// Experimental, not used by the firmware: in `maarse-sim` (120 nodes, 5 seeds)
/// it gave no gain at SF11 and fewer deliveries at SF12 (55 % vs. 66 %),
/// probably because the ack and path request timeouts stay fixed. Try it with
/// `maarse-sim --slot sf`.
pub fn slot_ms(params: &LoraParams) -> u32 {
    params.airtime_ms(SLOT_PACKET).clamp(20, 2_000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Header, Path};

    fn encoded(h: Header) -> heapless::Vec<u8, 64> {
        let mut buf = [0u8; 64];
        let n = h.encode(&mut buf).unwrap();
        heapless::Vec::from_slice(&buf[..n]).unwrap()
    }

    #[test]
    fn priorities_follow_packet_type() {
        let a = Address([7; 16]);
        assert_eq!(priority(&encoded(Header::new(PacketType::Ack, a))), PRIO_CONTROL);
        assert_eq!(priority(&encoded(Header::new(PacketType::Data, a))), PRIO_DIRECT);
        assert_eq!(priority(&encoded(Header::new(PacketType::Channel, a))), PRIO_CHANNEL);
        assert_eq!(priority(&encoded(Header::new(PacketType::Fragment, a))), PRIO_BULK);
        assert_eq!(priority(&encoded(Header::new(PacketType::Announce, a))), PRIO_FLOOD);
        // Carry: notice (no path) vs. carried message (one-element path).
        assert_eq!(priority(&encoded(Header::new(PacketType::Carry, a))), PRIO_CONTROL);
        let mut p = Path::new();
        p.push(3).unwrap();
        assert_eq!(priority(&encoded(Header::new(PacketType::Carry, a).with_path(p))), PRIO_DIRECT);
        // Not a Må:rse packet.
        assert_eq!(priority(&[0xff, 1, 2]), PRIO_DIRECT);
        assert_eq!(priority(&[]), PRIO_DIRECT);
    }

    #[test]
    fn urgent_channel_goes_before_other_channels() {
        let urgent = Address([9; 16]);
        let other = Address([8; 16]);
        assert_eq!(priority_with(&encoded(Header::new(PacketType::Channel, urgent)), &urgent), PRIO_DIRECT);
        assert_eq!(priority_with(&encoded(Header::new(PacketType::Channel, other)), &urgent), PRIO_CHANNEL);
        assert_eq!(priority_with(&encoded(Header::new(PacketType::Ack, urgent)), &urgent), PRIO_CONTROL);
    }

    #[test]
    fn slot_follows_spreading_factor() {
        let p = |sf| LoraParams { sf, bw_hz: 125_000, cr: 1, preamble: 8 };
        let s9 = slot_ms(&p(9));
        assert!((150..=260).contains(&s9), "SF9 slot {s9}");
        assert!(slot_ms(&p(12)) > 4 * s9);
        assert!(slot_ms(&p(7)) < s9);
    }
}
