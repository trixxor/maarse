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
        PacketType::PathRequest | PacketType::Announce | PacketType::Probe | PacketType::KeyRequest | PacketType::MailSync => PRIO_FLOOD,
        // Hellos are small and keep the neighbor picture fresh.
        PacketType::Hello | PacketType::Summary => PRIO_CHANNEL,
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
/// Used by the firmware together with scaled timeouts (`Router::set_slot`,
/// [`scale_timeout`]). In `maarse-sim` (5 seeds) the SF slot alone was worse
/// than the fixed 200 ms (120 nodes, SF12: 59 % vs. 65 % delivered), but with
/// the ack, path retry and hop timeouts scaled by `slot / 200` it was better at
/// SF11 (90 % vs. 79 %) and SF12 (70 % vs. 65 %), and with 200 nodes on 12 km
/// at SF11 (99 % vs. 94 %) and SF12 (86 % vs. 79 %). SF9 was unchanged. Try it
/// with `maarse-sim --slot sf --timeouts scaled`.
pub fn slot_ms(params: &LoraParams) -> u32 {
    params.airtime_ms(SLOT_PACKET).clamp(20, 2_000)
}

/// The slot the router's fixed timeouts were tuned for (ms).
pub const BASE_SLOT_MS: u32 = 200;

/// Scales a timeout tuned for a 200 ms slot to `slot_ms`, never below
/// `base`: `base * slot_ms / 200`.
pub fn scale_timeout(base: u32, slot_ms: u32) -> u32 {
    let scaled = (base as u64 * slot_ms as u64 / BASE_SLOT_MS as u64).min(u32::MAX as u64) as u32;
    scaled.max(base)
}

/// Length of one bucket of the channel load window (ms).
pub const LOAD_BUCKET_MS: u64 = 10_000;
/// Number of buckets: the window is 60 s.
pub const LOAD_BUCKETS: usize = 6;
/// Channel load (per mille of the window) from which load control acts.
pub const LOAD_HIGH_PERMILLE: u16 = 250;
/// Path requests a node may originate or forward per minute under high load.
pub const PATH_QUOTA_PER_MIN: u8 = 6;

/// Airtime heard and sent over a sliding window of 60 s (six 10 s buckets).
///
/// Used by the experimental load control (`Router::load_control`, off by
/// default and not used by the firmware): in `maarse-sim` it helped under
/// stress at SF11 but cost deliveries at normal load and at SF12.
///
/// The radio layer adds the airtime of every packet it sends or receives;
/// [`ChannelLoad::permille`] gives the share of the window the channel
/// was busy. Fixed size, no heap.
#[derive(Clone, Debug, Default)]
pub struct ChannelLoad {
    busy_ms: [u32; LOAD_BUCKETS],
    /// Index (time / bucket length) of the newest bucket.
    head: u64,
}

impl ChannelLoad {
    pub const fn new() -> Self {
        ChannelLoad { busy_ms: [0; LOAD_BUCKETS], head: 0 }
    }

    fn advance(&mut self, now_ms: u64) {
        let idx = now_ms / LOAD_BUCKET_MS;
        if idx <= self.head {
            return;
        }
        let gap = (idx - self.head).min(LOAD_BUCKETS as u64);
        for k in 1..=gap {
            self.busy_ms[((self.head + k) % LOAD_BUCKETS as u64) as usize] = 0;
        }
        self.head = idx;
    }

    /// Records `airtime_ms` of channel use (heard or sent) at `now_ms`.
    pub fn add(&mut self, now_ms: u64, airtime_ms: u32) {
        self.advance(now_ms);
        let b = &mut self.busy_ms[(self.head % LOAD_BUCKETS as u64) as usize];
        *b = b.saturating_add(airtime_ms);
    }

    /// Busy share of the last 60 s in per mille (0..=1000).
    pub fn permille(&mut self, now_ms: u64) -> u16 {
        self.advance(now_ms);
        let busy: u64 = self.busy_ms.iter().map(|&b| b as u64).sum();
        // The newest bucket is only partly over; count only the elapsed part.
        let window = (LOAD_BUCKETS as u64 - 1) * LOAD_BUCKET_MS + now_ms % LOAD_BUCKET_MS + 1;
        let window = window.min(now_ms + 1);
        (busy * 1000 / window).min(1000) as u16
    }
}

/// Counts path requests per minute and refuses more than
/// [`PATH_QUOTA_PER_MIN`] while the channel load is high.
#[derive(Clone, Debug, Default)]
pub struct PathQuota {
    minute: u64,
    used: u8,
}

impl PathQuota {
    pub const fn new() -> Self {
        PathQuota { minute: 0, used: 0 }
    }

    /// May one more path request be sent now? Counts it if yes. Under
    /// normal load every request is allowed (and still counted).
    pub fn take(&mut self, now_ms: u64, load_high: bool) -> bool {
        let minute = now_ms / 60_000;
        if minute != self.minute {
            self.minute = minute;
            self.used = 0;
        }
        if load_high && self.used >= PATH_QUOTA_PER_MIN {
            return false;
        }
        self.used = self.used.saturating_add(1);
        true
    }
}

/// Announce interval for the measured load: doubled at high load,
/// quadrupled when the channel is busy more than half the time.
pub fn announce_interval_ms(base_ms: u32, load_permille: u16) -> u32 {
    if load_permille >= 500 {
        base_ms.saturating_mul(4)
    } else if load_permille >= LOAD_HIGH_PERMILLE {
        base_ms.saturating_mul(2)
    } else {
        base_ms
    }
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

    #[test]
    fn timeouts_scale_but_never_shrink() {
        assert_eq!(scale_timeout(20_000, 200), 20_000);
        assert_eq!(scale_timeout(20_000, 100), 20_000);
        assert_eq!(scale_timeout(20_000, 1_300), 130_000);
    }

    #[test]
    fn channel_load_follows_a_sliding_window() {
        let mut l = ChannelLoad::new();
        assert_eq!(l.permille(0), 0);
        // 30 s of airtime in one minute = 50 %.
        for t in 0..60u64 {
            l.add(t * 1000, 500);
        }
        let p = l.permille(59_999);
        assert!((480..=520).contains(&p), "{p}");
        // Two quiet minutes later it is gone.
        assert_eq!(l.permille(180_000), 0);
        // Early in the run the window is shorter than 60 s.
        let mut l = ChannelLoad::new();
        l.add(1_000, 1_000);
        assert!(l.permille(1_999) >= 500);
    }

    #[test]
    fn path_quota_only_limits_under_load() {
        let mut q = PathQuota::new();
        for _ in 0..20 {
            assert!(q.take(1_000, false));
        }
        let mut q = PathQuota::new();
        for _ in 0..PATH_QUOTA_PER_MIN {
            assert!(q.take(1_000, true));
        }
        assert!(!q.take(2_000, true));
        assert!(q.take(61_000, true), "new minute");
    }

    #[test]
    fn announces_slow_down_under_load() {
        assert_eq!(announce_interval_ms(1000, 100), 1000);
        assert_eq!(announce_interval_ms(1000, 300), 2000);
        assert_eq!(announce_interval_ms(1000, 600), 4000);
    }
}
