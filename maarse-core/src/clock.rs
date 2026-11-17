//! Time over LoRa: hellos carry the time and a stratum, as in NTP.
//!
//! * Stratum 1: a host with a system clock (NTP) or GPS, or a node whose
//!   phone or PC set the time (`SetTime`). Stratum n + 1: learned from a
//!   neighbour at stratum n. 0: no time.
//! * A node without time takes the first it hears and moves to a neighbour
//!   with a lower stratum when one turns up.
//! * With time, it corrects only by more than [`CORRECT_MS`], and only from a
//!   neighbour at a lower stratum than its own.
//! * A jump of more than [`JUMP_MS`] needs two neighbours that agree (within
//!   [`AGREE_MS`]), so one wrong or lying clock cannot move it far.
//!
//! Security never depends on the clock: the inbox key's week is in the
//! message header, and ordering uses sequence numbers. The clock is for the
//! expiry of mail, old keys and the display.

/// Corrections smaller than this are ignored.
pub const CORRECT_MS: i64 = 2_000;
/// Jumps larger than this need two agreeing neighbours.
pub const JUMP_MS: i64 = 10 * 60_000;
/// Two neighbours agree when their clocks differ by at most this.
pub const AGREE_MS: i64 = 5_000;
/// Strata from here on are not passed on.
pub const MAX_STRATUM: u8 = 15;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Offer {
    from: u16,
    /// Their Unix time in ms minus our monotonic ms.
    offset: i64,
    stratum: u8,
    at_ms: u64,
}

/// The node's idea of the time.
#[derive(Clone, Debug, Default)]
pub struct Clock {
    /// Unix time in ms minus the monotonic ms, when known.
    offset: Option<i64>,
    stratum: u8,
    /// Recent offers that would be a big jump, waiting for a second opinion.
    jumps: heapless::Vec<Offer, 4>,
}

impl Clock {
    /// The time from the system, a phone or a PC (stratum 1 unless given).
    pub fn set(&mut self, unix_s: u32, now_ms: u64, stratum: u8) {
        self.offset = Some(unix_s as i64 * 1000 - now_ms as i64);
        self.stratum = stratum.max(1);
        self.jumps.clear();
    }

    /// Unix time (s) and stratum, if known.
    pub fn now(&self, now_ms: u64) -> Option<(u32, u8)> {
        let o = self.offset?;
        let ms = now_ms as i64 + o;
        (ms > 0).then(|| ((ms / 1000) as u32, self.stratum))
    }

    pub fn stratum(&self) -> u8 {
        if self.offset.is_some() { self.stratum } else { 0 }
    }

    /// A neighbour's time from its hello. Returns true if our time changed.
    pub fn offer(&mut self, from: u16, unix_s: u32, stratum: u8, now_ms: u64) -> bool {
        if stratum == 0 || stratum >= MAX_STRATUM || unix_s == 0 {
            return false;
        }
        let offset = unix_s as i64 * 1000 - now_ms as i64;
        let mine = stratum + 1;
        let Some(cur) = self.offset else {
            self.offset = Some(offset);
            self.stratum = mine;
            return true;
        };
        // Only from a neighbour at a lower stratum than ours (a set clock,
        // stratum 1, is never moved).
        if mine > self.stratum {
            return false;
        }
        let diff = (offset - cur).abs();
        if diff <= CORRECT_MS {
            // Close enough; a lower stratum still improves what we pass on.
            let better = mine < self.stratum;
            self.stratum = self.stratum.min(mine);
            return better;
        }
        if diff <= JUMP_MS {
            self.offset = Some(offset);
            self.stratum = mine;
            return true;
        }
        // A big jump: wait for a second neighbour that agrees.
        self.jumps.retain(|j| now_ms.saturating_sub(j.at_ms) < 10 * 60_000 && j.from != from);
        let agree = self.jumps.iter().any(|j| (j.offset - offset).abs() <= AGREE_MS);
        if agree {
            self.offset = Some(offset);
            self.stratum = mine;
            self.jumps.clear();
            return true;
        }
        if self.jumps.is_full() {
            self.jumps.remove(0);
        }
        let _ = self.jumps.push(Offer { from, offset, stratum, at_ms: now_ms });
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: u32 = 1_800_000_000;

    #[test]
    fn learns_and_prefers_lower_strata() {
        let mut c = Clock::default();
        assert_eq!(c.now(0), None);
        assert!(c.offer(1, T, 3, 1000));
        assert_eq!(c.now(1000), Some((T, 4)));
        assert_eq!(c.now(6000), Some((T + 5, 4)));
        // Same time from stratum 1: we are now stratum 2.
        assert!(c.offer(2, T + 5, 1, 6000));
        assert_eq!(c.now(6000).unwrap().1, 2);
        // A worse stratum cannot move us.
        assert!(!c.offer(3, T + 100, 5, 7000));
        assert_eq!(c.now(7000).unwrap().0, T + 6);
    }

    #[test]
    fn small_differences_are_ignored_and_medium_ones_corrected() {
        let mut c = Clock::default();
        c.offer(1, T, 2, 0);
        assert!(!c.offer(2, T + 1, 2, 0));
        assert_eq!(c.now(0).unwrap().0, T);
        assert!(c.offer(3, T + 60, 1, 0));
        assert_eq!(c.now(0), Some((T + 60, 2)));
    }

    #[test]
    fn a_big_jump_needs_two_neighbours() {
        let mut c = Clock::default();
        c.offer(1, T, 2, 0);
        // One neighbour says it is a day later: not believed.
        assert!(!c.offer(2, T + 86_400, 1, 0));
        assert_eq!(c.now(0).unwrap().0, T);
        // The same one again does not count twice.
        assert!(!c.offer(2, T + 86_400, 1, 1000));
        // A second, independent one agrees: jump.
        assert!(c.offer(3, T + 86_401, 1, 1000));
        assert_eq!(c.now(1000).unwrap().0, T + 86_401);
    }

    #[test]
    fn a_set_clock_is_not_moved_by_neighbours() {
        let mut c = Clock::default();
        c.set(T, 0, 1);
        assert!(!c.offer(1, T + 30, 1, 0));
        assert!(!c.offer(1, T + 30, 0, 0));
        assert_eq!(c.now(0), Some((T, 1)));
    }
}
