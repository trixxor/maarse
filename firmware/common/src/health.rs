//! Watchdog supervision: each long-running loop says by when it will be back,
//! and the hardware watchdog is only fed while no loop is late. A task that
//! hangs (a radio that never answers, a busy loop) then restarts the board.
//!
//! The board code starts the hardware watchdog with [`TIMEOUT_S`] and runs
//! [`supervise`] in a task of its own.

use core::sync::atomic::{AtomicU32, Ordering::Relaxed};
use embassy_time::{Instant, Timer};

/// Hardware watchdog timeout.
pub const TIMEOUT_S: u32 = 60;
/// How often the watchdog is fed.
pub const FEED_EVERY_S: u64 = 10;
/// Time a loop may take on top of the wait it announced.
pub const SLACK_S: u32 = 30;
/// Not supervised.
const RESTING: u32 = u32::MAX;

/// The uptime in seconds by which a loop must have reported again.
pub struct Beat(AtomicU32);

impl Beat {
    pub const fn resting() -> Self {
        Beat(AtomicU32::new(RESTING))
    }

    /// Must report again before `by_s` seconds of uptime.
    pub const fn due_by(by_s: u32) -> Self {
        Beat(AtomicU32::new(by_s))
    }

    /// "I am alive, and will be back within `wait_s` seconds."
    pub fn expect(&self, now_s: u32, wait_s: u32) {
        let by = now_s.saturating_add(wait_s).saturating_add(SLACK_S);
        self.0.store(by.min(RESTING - 1), Relaxed);
    }

    /// The loop waits for something that may take for ever (the radio asleep).
    pub fn rest(&self) {
        self.0.store(RESTING, Relaxed);
    }

    pub fn late(&self, now_s: u32) -> bool {
        let by = self.0.load(Relaxed);
        by != RESTING && now_s > by
    }
}

/// The node loop. Supervised from the start, so a start-up that hangs ends in a restart too.
pub static NODE: Beat = Beat::due_by(TIMEOUT_S);
/// The radio loop. Resting until it runs: a board without a working radio stays up.
pub static RADIO: Beat = Beat::resting();

/// Uptime in whole seconds.
pub fn now_s() -> u32 {
    Instant::now().as_secs() as u32
}

/// Feeds the watchdog for as long as no loop is late.
pub async fn supervise(mut feed: impl FnMut()) -> ! {
    loop {
        let now = now_s();
        if !NODE.late(now) && !RADIO.late(now) {
            feed();
        }
        Timer::after_secs(FEED_EVERY_S).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn late_only_after_the_announced_wait_and_the_slack() {
        let b = Beat::resting();
        b.expect(100, 20);
        assert!(!b.late(100));
        assert!(!b.late(120 + SLACK_S));
        assert!(b.late(121 + SLACK_S));
    }

    #[test]
    fn reporting_again_moves_the_deadline() {
        let b = Beat::resting();
        b.expect(0, 0);
        assert!(b.late(1000));
        b.expect(1000, 0);
        assert!(!b.late(1000 + SLACK_S));
    }

    #[test]
    fn a_resting_loop_is_never_late() {
        let b = Beat::due_by(10);
        assert!(b.late(11));
        b.rest();
        assert!(!b.late(u32::MAX - 1));
    }

    #[test]
    fn a_very_long_wait_does_not_wrap() {
        let b = Beat::resting();
        b.expect(1000, u32::MAX);
        assert!(!b.late(u32::MAX - 1));
    }
}
