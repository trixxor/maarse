//! Range test: numbered beacons for field measurements of range, loss and
//! power use.
//!
//! One node is the *beacon* and sends a numbered packet every few seconds.
//! The other is the *receiver*: it reports every beacon it hears with RSSI and
//! SNR, and counts loss from gaps in the sequence numbers. The beacon can step
//! through SF7–SF12 on a fixed schedule. The receiver learns the schedule from
//! the beacons themselves and follows it, so both are on the same SF at the
//! same time.
//!
//! Beacon packets are not Må:rse packets: the first byte has version bits
//! `11`, so every Må:rse node rejects them and no repeater forwards them. The
//! test measures the direct link only.
//!
//! # Packet format (little endian)
//!
//! ```text
//! magic "\xD7RT" (3) | version (1) | sender (4, address prefix) | seq (u32) |
//! sf (1) | slot (1, index within the SF step) | per_step (1) | sf_mask (1) |
//! skipped (u16, beacons the sender could not transmit, cumulative) |
//! tx power (i8, dBm, as configured) | battery (u16, mV, 0 = unknown) |
//! flags (1: bit 0 position) | slot interval per SF in the cycle (1 byte each, s) |
//! [lat_e7 (i32) | lon_e7 (i32)] | padding (zeros)
//! ```
//!
//! # Schedule
//!
//! The cycle is the SFs in `sf_mask` from SF7 upwards (or only the current SF
//! when the mask is 0). Each SF gets one *step*: a guard time of
//! [`GUARD_MS`] followed by `per_step` slots of that SF's interval. A beacon is
//! sent at the start of each slot. Slot `j` (counted from the start of the
//! test, over all cycles) carries `seq = j`, so the SF of any sequence number is
//! known, also for beacons that were lost.
//!
//! The guard lets the receiver switch SF a little late (it estimates the step
//! boundary from received beacons, which may have been delayed by
//! listen-before-talk) without missing the first beacon of the next step.

/// First bytes of a beacon. 0xD7 has version bits `11`: never a Må:rse packet.
pub const MAGIC: [u8; 3] = [0xD7, b'R', b'T'];
pub const VERSION: u8 = 1;
/// Time at the start of every SF step before the first beacon.
pub const GUARD_MS: u64 = 1500;
/// Lowest and highest SF in a cycle.
pub const SF_MIN: u8 = 7;
pub const SF_MAX: u8 = 12;
/// Number of SFs a cycle can have.
pub const MAX_STEPS: usize = (SF_MAX - SF_MIN + 1) as usize;
/// Largest padding (extra zero bytes) in a beacon.
pub const MAX_PAD: u8 = 200;
/// Fixed part of a beacon, without slot intervals, position and padding.
pub const FIXED_LEN: usize = 22;
/// Tag in the upper 16 bits of the radio job ID of a beacon.
pub const TX_ID_TAG: u64 = 0x5254 << 48;

/// Modes in [`Config::mode`].
pub mod mode {
    pub const OFF: u8 = 0;
    pub const BEACON: u8 = 1;
    pub const RECEIVER: u8 = 2;
}

/// Flag in [`Config::flags`]: keep the setting in flash and resume after a restart.
pub const FLAG_PERSIST: u8 = 1;
/// Flag in [`Config::flags`]: the beacon sends `lat_e7`/`lon_e7` from the config.
/// Without it the beacon sends its shared position (rounded as chosen under
/// sharing), if any.
pub const FLAG_POSITION: u8 = 2;

/// Range test settings (command `SetRangeTest`, 15 bytes):
/// mode (1) | flags (1) | interval_s (u16) | sf_mask (1, bit 0 = SF7 … bit 5 =
/// SF12, 0 = stay on the current SF) | per_step (1) | pad (1) | lat_e7 (i32) |
/// lon_e7 (i32).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Config {
    pub mode: u8,
    pub flags: u8,
    /// Seconds between beacons (raised per SF if the duty cycle needs it).
    pub interval_s: u16,
    pub sf_mask: u8,
    /// Beacons per SF step (with an SF mask).
    pub per_step: u8,
    /// Extra zero bytes in each beacon, to test longer packets.
    pub pad: u8,
    pub lat_e7: i32,
    pub lon_e7: i32,
}

impl Config {
    pub const LEN: usize = 15;

    pub fn to_bytes(&self) -> [u8; Self::LEN] {
        let mut b = [0u8; Self::LEN];
        b[0] = self.mode;
        b[1] = self.flags;
        b[2..4].copy_from_slice(&self.interval_s.to_le_bytes());
        b[4] = self.sf_mask;
        b[5] = self.per_step;
        b[6] = self.pad;
        b[7..11].copy_from_slice(&self.lat_e7.to_le_bytes());
        b[11..15].copy_from_slice(&self.lon_e7.to_le_bytes());
        b
    }

    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        if b.len() < Self::LEN {
            return None;
        }
        Some(Config {
            mode: b[0],
            flags: b[1],
            interval_s: u16::from_le_bytes([b[2], b[3]]),
            sf_mask: b[4],
            per_step: b[5],
            pad: b[6],
            lat_e7: i32::from_le_bytes(b[7..11].try_into().ok()?),
            lon_e7: i32::from_le_bytes(b[11..15].try_into().ok()?),
        })
    }

    /// Checks the settings and brings them into range: interval 2–255 s, 1–50
    /// beacons per step, padding at most [`MAX_PAD`].
    pub fn sanitized(&self) -> Result<Self, &'static str> {
        if self.mode > mode::RECEIVER {
            return Err("Unknown range test mode (0 = off, 1 = beacon, 2 = receiver)");
        }
        let mut c = *self;
        c.sf_mask &= (1 << MAX_STEPS) - 1;
        c.interval_s = c.interval_s.clamp(2, 255);
        c.per_step = c.per_step.clamp(1, 50);
        c.pad = c.pad.min(MAX_PAD);
        c.flags &= FLAG_PERSIST | FLAG_POSITION;
        Ok(c)
    }

    pub fn persist(&self) -> bool {
        self.flags & FLAG_PERSIST != 0
    }

    /// The SFs of the cycle, lowest first; only `current_sf` without a mask.
    pub fn sfs(&self, current_sf: u8) -> heapless::Vec<u8, MAX_STEPS> {
        sfs_of(self.sf_mask, current_sf)
    }
}

/// The SFs in a mask (bit 0 = SF7), or only `current_sf` if the mask is 0.
pub fn sfs_of(mask: u8, current_sf: u8) -> heapless::Vec<u8, MAX_STEPS> {
    let mut v = heapless::Vec::new();
    for i in 0..MAX_STEPS as u8 {
        if mask & (1 << i) != 0 {
            let _ = v.push(SF_MIN + i);
        }
    }
    if v.is_empty() {
        let _ = v.push(current_sf);
    }
    v
}

/// Smallest whole number of seconds between packets of `airtime_us` that
/// keeps the airtime within `duty_permille`, with 25 % margin for other traffic.
pub fn min_interval_s(airtime_us: u32, duty_permille: u16) -> u16 {
    let permille = duty_permille.max(1) as u64;
    // airtime / (permille / 1000) is the wall time per packet; µs -> s.
    let us = airtime_us as u64 * 1000 * 5 / 4 / permille;
    us.div_ceil(1_000_000).min(u16::MAX as u64) as u16
}

/// The beacon schedule: SF, slot interval and number of slots per step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Schedule {
    pub sfs: heapless::Vec<u8, MAX_STEPS>,
    /// Seconds between beacons, one per entry in `sfs`.
    pub intervals_s: heapless::Vec<u8, MAX_STEPS>,
    pub per_step: u8,
    /// Whether the beacon steps through SFs (a mask was given).
    pub cycling: bool,
}

impl Schedule {
    /// The schedule a beacon uses. `airtime_us(sf)` is the airtime of one
    /// beacon at that SF; the interval per SF is raised to keep within the duty cycle.
    pub fn for_beacon(
        cfg: &Config,
        current_sf: u8,
        duty_permille: u16,
        airtime_us: impl Fn(u8) -> u32,
    ) -> Self {
        let sfs = cfg.sfs(current_sf);
        let intervals_s = sfs
            .iter()
            .map(|&sf| {
                cfg.interval_s
                    .max(min_interval_s(airtime_us(sf), duty_permille))
                    .clamp(2, 255) as u8
            })
            .collect();
        Schedule { sfs, intervals_s, per_step: cfg.per_step.max(1), cycling: cfg.sf_mask != 0 }
    }

    fn slots_per_cycle(&self) -> u32 {
        self.sfs.len() as u32 * self.per_step as u32
    }

    /// Duration of step `i` (ms).
    pub fn step_ms(&self, i: usize) -> u64 {
        let iv = *self.intervals_s.get(i).unwrap_or(&2) as u64;
        if self.cycling { GUARD_MS + self.per_step as u64 * iv * 1000 } else { iv * 1000 }
    }

    pub fn cycle_ms(&self) -> u64 {
        (0..self.sfs.len()).map(|i| self.step_ms(i)).sum()
    }

    /// Offset of step `i` from the start of the cycle (ms).
    pub fn step_start_ms(&self, i: usize) -> u64 {
        (0..i).map(|j| self.step_ms(j)).sum()
    }

    /// (step, slot within step, cycle number) of sequence number `seq`.
    pub fn locate(&self, seq: u32) -> (usize, u8, u32) {
        if !self.cycling {
            return (0, 0, seq);
        }
        let per = self.slots_per_cycle().max(1);
        let r = seq % per;
        ((r / self.per_step as u32) as usize, (r % self.per_step as u32) as u8, seq / per)
    }

    /// The SF of sequence number `seq`.
    pub fn sf_of(&self, seq: u32) -> u8 {
        self.sfs[self.locate(seq).0]
    }

    /// When the beacon with `seq` is due, in ms after the start of the test.
    pub fn send_at_ms(&self, seq: u32) -> u64 {
        if !self.cycling {
            return seq as u64 * self.step_ms(0);
        }
        let (step, slot, cycle) = self.locate(seq);
        cycle as u64 * self.cycle_ms() + self.slot_offset_ms(step, slot)
    }

    /// Offset of slot `slot` in step `step` from the start of the cycle (ms).
    pub fn slot_offset_ms(&self, step: usize, slot: u8) -> u64 {
        let iv = *self.intervals_s.get(step).unwrap_or(&2) as u64;
        let guard = if self.cycling { GUARD_MS } else { 0 };
        self.step_start_ms(step) + guard + slot as u64 * iv * 1000
    }

    /// The step at `offset_ms` after the start of a cycle (any offset; it wraps)
    /// and the ms until the next step begins.
    pub fn step_at(&self, offset_ms: u64) -> (usize, u64) {
        let cycle = self.cycle_ms().max(1);
        let mut t = offset_ms % cycle;
        for i in 0..self.sfs.len() {
            let d = self.step_ms(i);
            if t < d {
                return (i, d - t);
            }
            t -= d;
        }
        (0, cycle)
    }
}

/// A received (or built) beacon.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Beacon {
    pub sender: [u8; 4],
    pub seq: u32,
    pub sf: u8,
    pub slot: u8,
    pub per_step: u8,
    pub sf_mask: u8,
    pub skipped: u16,
    pub tx_power_dbm: i8,
    pub battery_mv: u16,
    pub intervals_s: heapless::Vec<u8, MAX_STEPS>,
    pub position: Option<(i32, i32)>,
}

impl Beacon {
    /// Length of the encoded beacon with `pad` extra bytes.
    pub fn encoded_len(&self, pad: u8) -> usize {
        FIXED_LEN + self.intervals_s.len() + if self.position.is_some() { 8 } else { 0 } + pad as usize
    }

    /// Writes the beacon into `out`; returns the length (0 if it does not fit).
    pub fn encode(&self, pad: u8, out: &mut [u8]) -> usize {
        let n = self.encoded_len(pad);
        if out.len() < n {
            return 0;
        }
        out[..3].copy_from_slice(&MAGIC);
        out[3] = VERSION;
        out[4..8].copy_from_slice(&self.sender);
        out[8..12].copy_from_slice(&self.seq.to_le_bytes());
        out[12] = self.sf;
        out[13] = self.slot;
        out[14] = self.per_step;
        out[15] = self.sf_mask;
        out[16..18].copy_from_slice(&self.skipped.to_le_bytes());
        out[18] = self.tx_power_dbm as u8;
        out[19..21].copy_from_slice(&self.battery_mv.to_le_bytes());
        out[21] = self.position.is_some() as u8;
        let mut o = FIXED_LEN;
        out[o..o + self.intervals_s.len()].copy_from_slice(&self.intervals_s);
        o += self.intervals_s.len();
        if let Some((lat, lon)) = self.position {
            out[o..o + 4].copy_from_slice(&lat.to_le_bytes());
            out[o + 4..o + 8].copy_from_slice(&lon.to_le_bytes());
            o += 8;
        }
        out[o..n].fill(0);
        n
    }

    /// Parses a received packet; `None` if it is not a beacon.
    pub fn parse(b: &[u8]) -> Option<Self> {
        if b.len() < FIXED_LEN || b[..3] != MAGIC || b[3] != VERSION {
            return None;
        }
        let sf_mask = b[15];
        let steps = if sf_mask == 0 { 1 } else { (sf_mask & ((1 << MAX_STEPS) - 1)).count_ones() as usize };
        let mut o = FIXED_LEN;
        let intervals_s = heapless::Vec::from_slice(b.get(o..o + steps)?).ok()?;
        o += steps;
        let position = if b[21] & 1 != 0 {
            let p = b.get(o..o + 8)?;
            Some((i32::from_le_bytes(p[..4].try_into().ok()?), i32::from_le_bytes(p[4..].try_into().ok()?)))
        } else {
            None
        };
        Some(Beacon {
            sender: b[4..8].try_into().ok()?,
            seq: u32::from_le_bytes(b[8..12].try_into().ok()?),
            sf: b[12],
            slot: b[13],
            per_step: b[14].max(1),
            sf_mask,
            skipped: u16::from_le_bytes([b[16], b[17]]),
            tx_power_dbm: b[18] as i8,
            battery_mv: u16::from_le_bytes([b[19], b[20]]),
            intervals_s,
            position,
        })
    }

    /// The sender's schedule, as far as it can be read from the beacon.
    pub fn schedule(&self) -> Schedule {
        let sfs = sfs_of(self.sf_mask, self.sf);
        let mut intervals_s = self.intervals_s.clone();
        intervals_s.truncate(sfs.len());
        while intervals_s.len() < sfs.len() {
            let _ = intervals_s.push(2);
        }
        Schedule { sfs, intervals_s, per_step: self.per_step, cycling: self.sf_mask != 0 }
    }
}

/// Receiver: the start of the beacon's cycle (ms on the receiver's clock,
/// may be negative) estimated from beacon `seq`, received completely at
/// `rx_end_ms` after `airtime_ms` on air. Listen-before-talk may have delayed
/// the beacon, so the estimate can only be late, never early.
pub fn cycle_start_estimate(sched: &Schedule, seq: u32, rx_end_ms: i64, airtime_ms: i64) -> i64 {
    let (step, slot, _) = sched.locate(seq);
    rx_end_ms - airtime_ms - sched.slot_offset_ms(step, slot) as i64
}

/// Receiver: combines a new cycle start estimate with the current one. An
/// earlier estimate is taken at once (it had less listen-before-talk delay),
/// a later one is followed slowly (clock drift between the boards), and a
/// jump of more than 5 s means the beacon restarted: start over.
pub fn refine_cycle_start(cur: Option<i64>, est: i64, cycle_ms: i64) -> i64 {
    let Some(cur) = cur else { return est };
    let cycle = cycle_ms.max(1);
    // Compare with the nearest cycle start of the current estimate.
    let d = (est - cur).rem_euclid(cycle);
    let d = if d > cycle / 2 { d - cycle } else { d };
    if d.abs() > 5000 {
        est
    } else if d < 0 {
        cur + d
    } else {
        cur + d / 8
    }
}

/// Number of beacons lost between two received ones from the same sender:
/// the gap in sequence numbers minus the beacons the sender reports it did
/// not transmit. `None` if `seq` does not follow `last_seq` (the beacon
/// restarted or the packet is a duplicate): start counting again.
pub fn lost_between(last_seq: u32, last_skipped: u16, seq: u32, skipped: u16) -> Option<u32> {
    if seq <= last_seq {
        return None;
    }
    let gap = seq - last_seq - 1;
    let not_sent = skipped.wrapping_sub(last_skipped) as u32;
    Some(gap.saturating_sub(not_sent))
}

// ---------- power estimates ----------

/// Estimated SX1262 supply current while transmitting, in 0.1 mA, at
/// `dbm` with the high-power PA set up for +22 dBm (as lora-phy does).
/// From the datasheet's typical values (118 mA at +22 dBm, 3.3 V, DC-DC),
/// interpolated linearly down to about 60 mA at +14 dBm. An estimate only.
pub fn sx1262_tx_ma_x10(dbm: i8) -> u32 {
    let d = dbm.clamp(-9, 22) as i32;
    let ma_x10 = if d >= 14 { 600 + (d - 14) * 580 / 8 } else { 600 - (14 - d) * 30 };
    ma_x10.max(250) as u32
}

/// Estimated charge (mA·ms = µC) for `dt_ms` of running, of which `tx_ms`
/// transmitting: the rest of the time the radio is receiving. Currents in 0.1 mA.
pub fn charge_ma_ms(base_ma_x10: u32, rx_ma_x10: u32, tx_ma_x10: u32, dt_ms: u64, tx_ms: u64) -> u64 {
    let tx_ms = tx_ms.min(dt_ms);
    (base_ma_x10 as u64 * dt_ms + rx_ma_x10 as u64 * (dt_ms - tx_ms) + tx_ma_x10 as u64 * tx_ms) / 10
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::airtime::LoraParams;

    fn air(sf: u8) -> u32 {
        LoraParams { sf, bw_hz: 125_000, cr: 1, preamble: 16 }.airtime_us(30)
    }

    #[test]
    fn config_roundtrip_and_sanitize() {
        let c = Config { mode: 1, flags: 3, interval_s: 15, sf_mask: 0b11_1111, per_step: 5, pad: 10, lat_e7: 561_000_000, lon_e7: -102_000_000 };
        assert_eq!(Config::from_bytes(&c.to_bytes()), Some(c));
        assert!(Config::from_bytes(&[0; 14]).is_none());
        let s = Config { mode: 2, flags: 0xff, interval_s: 0, sf_mask: 0xff, per_step: 0, pad: 255, ..Default::default() }
            .sanitized()
            .unwrap();
        assert_eq!((s.interval_s, s.sf_mask, s.per_step, s.pad, s.flags), (2, 0x3f, 1, MAX_PAD, 3));
        assert!(Config { mode: 3, ..Default::default() }.sanitized().is_err());
        assert_eq!(&c.sfs(9)[..], &[7, 8, 9, 10, 11, 12]);
        assert_eq!(&Config { sf_mask: 0b100100, ..c }.sfs(9)[..], &[9, 12]);
        assert_eq!(&Config { sf_mask: 0, ..c }.sfs(9)[..], &[9]);
    }

    #[test]
    fn duty_cycle_raises_interval() {
        // SF12, 30 bytes, preamble 16: about 1.9 s on air. At 10 % that is 19 s
        // per packet, plus 25 % margin = 24 s.
        let a = air(12);
        assert!((1_800_000..2_000_000).contains(&a), "{a}");
        let m = min_interval_s(a, 100);
        assert!((23..=25).contains(&m), "{m}");
        assert!(min_interval_s(a, 10) > 200);
        assert_eq!(min_interval_s(air(7), 1000), 1);
        let cfg = Config { mode: 1, interval_s: 5, sf_mask: 0b10_0001, per_step: 3, ..Default::default() };
        let s = Schedule::for_beacon(&cfg, 9, 100, air);
        assert_eq!(&s.sfs[..], &[7, 12]);
        assert_eq!(s.intervals_s[0], 5);
        assert_eq!(s.intervals_s[1] as u16, m);
    }

    #[test]
    fn schedule_math() {
        let s = Schedule {
            sfs: heapless::Vec::from_slice(&[7, 9, 12]).unwrap(),
            intervals_s: heapless::Vec::from_slice(&[5, 10, 30]).unwrap(),
            per_step: 2,
            cycling: true,
        };
        assert_eq!(s.step_ms(0), 11_500);
        assert_eq!(s.step_ms(2), 61_500);
        assert_eq!(s.cycle_ms(), 11_500 + 21_500 + 61_500);
        assert_eq!(s.locate(0), (0, 0, 0));
        assert_eq!(s.locate(3), (1, 1, 0));
        assert_eq!(s.locate(7), (0, 1, 1));
        assert_eq!(s.sf_of(4), 12);
        assert_eq!(s.sf_of(6), 7);
        assert_eq!(s.send_at_ms(0), 1500);
        assert_eq!(s.send_at_ms(1), 6500);
        assert_eq!(s.send_at_ms(2), 11_500 + 1500);
        assert_eq!(s.send_at_ms(6), s.cycle_ms() + 1500);
        assert_eq!(s.step_at(0), (0, 11_500));
        assert_eq!(s.step_at(11_500), (1, 21_500));
        assert_eq!(s.step_at(s.cycle_ms() + 12_000), (1, 21_000));
        // Every beacon is sent inside its own step, after the guard.
        for seq in 0..20 {
            let t = s.send_at_ms(seq);
            let (step, left) = s.step_at(t);
            assert_eq!(s.sfs[step], s.sf_of(seq));
            assert!(left >= 5000);
        }
        let fixed = Schedule { sfs: heapless::Vec::from_slice(&[9]).unwrap(), intervals_s: heapless::Vec::from_slice(&[10]).unwrap(), per_step: 5, cycling: false };
        assert_eq!(fixed.send_at_ms(3), 30_000);
        assert_eq!(fixed.sf_of(1234), 9);
    }

    #[test]
    fn beacon_roundtrip() {
        let b = Beacon {
            sender: [1, 2, 3, 4],
            seq: 70_000,
            sf: 10,
            slot: 2,
            per_step: 5,
            sf_mask: 0b1111,
            skipped: 3,
            tx_power_dbm: -3,
            battery_mv: 3987,
            intervals_s: heapless::Vec::from_slice(&[5, 6, 9, 15]).unwrap(),
            position: Some((561_234_567, -101_234_567)),
        };
        let mut buf = [0u8; 255];
        let n = b.encode(20, &mut buf);
        assert_eq!(n, FIXED_LEN + 4 + 8 + 20);
        assert_eq!(Beacon::parse(&buf[..n]), Some(b.clone()));
        // Never a valid Må:rse packet.
        assert!(crate::packet::Packet::decode(&buf[..n]).is_err());
        let fixed = Beacon { sf_mask: 0, intervals_s: heapless::Vec::from_slice(&[10]).unwrap(), position: None, ..b.clone() };
        let n = fixed.encode(0, &mut buf);
        assert_eq!(n, FIXED_LEN + 1);
        let p = Beacon::parse(&buf[..n]).unwrap();
        assert_eq!(p, fixed);
        assert_eq!(&p.schedule().sfs[..], &[10]);
        assert!(!p.schedule().cycling);
        assert!(Beacon::parse(&buf[..n - 1]).is_none());
        assert!(Beacon::parse(b"\xD7RT\x02 something else.....").is_none());
        assert_eq!(b.encode(0, &mut [0u8; 10]), 0);
        let s = b.schedule();
        assert_eq!(&s.sfs[..], &[7, 8, 9, 10]);
        assert_eq!(s.per_step, 5);
    }

    /// A beacon with random listen-before-talk delays and a receiver whose
    /// clock runs 400 ppm fast: the receiver must be on the right SF whenever a
    /// beacon starts, after it has heard the first one.
    #[test]
    fn receiver_follows_schedule() {
        let cfg = Config { mode: 1, interval_s: 3, sf_mask: 0b11_1111, per_step: 2, ..Default::default() };
        let sched = Schedule::for_beacon(&cfg, 9, 1000, air);
        let cycle = sched.cycle_ms() as i64;
        let mut rng = 12345u64;
        let mut rand = |max: u64| {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng % max
        };
        let rx_clock = |t_beacon: i64| t_beacon + t_beacon * 400 / 1_000_000 + 777_000; // offset + drift
        let mut start: Option<i64> = None;
        for seq in 0..400u32 {
            let sf = sched.sf_of(seq);
            // Listen-before-talk backoff: usually none, sometimes up to 1.2 s.
            let delay = if rand(4) == 0 { rand(1200) as i64 } else { 0 };
            let tx_start = sched.send_at_ms(seq) as i64 + delay;
            let air_ms = (air(sf) / 1000) as i64;
            if let Some(cs) = start {
                // Where does the receiver think the beacon is when it starts sending?
                let (step, _) = sched.step_at((rx_clock(tx_start) - cs).rem_euclid(cycle) as u64);
                assert_eq!(sched.sfs[step], sf, "seq {seq}");
            }
            // Every fifth beacon is lost.
            if rand(5) == 0 {
                continue;
            }
            let rx_end = rx_clock(tx_start + air_ms);
            let est = cycle_start_estimate(&sched, seq, rx_end, air_ms);
            start = Some(refine_cycle_start(start, est, cycle));
        }
        // A restarted beacon (new cycle start far away) is picked up at once.
        let cur = start.unwrap();
        assert_eq!(refine_cycle_start(Some(cur), cur + 20_000, cycle), cur + 20_000);
        assert_eq!(refine_cycle_start(Some(cur), cur - 300, cycle), cur - 300);
        assert_eq!(refine_cycle_start(Some(cur), cur + 800, cycle), cur + 100);
        assert_eq!(refine_cycle_start(Some(cur), cur + cycle + 8, cycle), cur + 1);
    }

    #[test]
    fn loss_from_gaps() {
        assert_eq!(lost_between(10, 0, 11, 0), Some(0));
        assert_eq!(lost_between(10, 0, 14, 0), Some(3));
        // The beacon itself skipped two of them.
        assert_eq!(lost_between(10, 1, 14, 3), Some(1));
        assert_eq!(lost_between(10, 0, 10, 0), None);
        assert_eq!(lost_between(10, 0, 3, 0), None);
    }

    #[test]
    fn power_estimates() {
        assert_eq!(sx1262_tx_ma_x10(22), 1180);
        assert_eq!(sx1262_tx_ma_x10(14), 600);
        assert!(sx1262_tx_ma_x10(18) > 800 && sx1262_tx_ma_x10(18) < 950);
        assert_eq!(sx1262_tx_ma_x10(-9), 250);
        // One hour at 5 mA base + 5 mA receive, 36 s of it at 118 mA.
        let c = charge_ma_ms(50, 50, 1180, 3_600_000, 36_000);
        // mA·ms -> mAh
        let mah = c as f64 / 3_600_000.0;
        assert!((mah - (5.0 + 5.0 * 0.99 + 118.0 * 0.01)).abs() < 0.01, "{mah}");
        assert_eq!(charge_ma_ms(10, 10, 10, 100, 500), 200);
    }
}
