//! A remote radio change on trial.
//!
//! A wrong frequency or spreading factor sent to a repeater in the field would cut it off, and if it were saved
//! it would stay wrong after a reboot. So a change that comes through remote control is only *tried*: it is in
//! use, but flash keeps the old settings until `ConfirmRadio` arrives. If the time runs out first, the node goes
//! back to the settings in flash. This module is the small state machine; the node does the radio and flash.

use crate::companion::RadioConfig;

/// The trial lasts this many minutes when the request does not say.
pub const DEFAULT_MINUTES: u8 = 5;
/// The longest trial a request can ask for.
pub const MAX_MINUTES: u8 = 30;

/// A trial in progress: the settings in flash, and when to go back to them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trial {
    /// What flash holds (and what is returned to), never the unconfirmed settings.
    pub saved: RadioConfig,
    deadline_ms: u64,
}

/// What the node should do after a tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tick {
    /// Nothing.
    Idle,
    /// The time is up: switch back to these settings (already in flash, so do not save).
    Revert(RadioConfig),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RadioTrial(Option<Trial>);

impl RadioTrial {
    pub const fn new() -> Self {
        Self(None)
    }

    /// Starts (or restarts) a trial at `now_ms`. `in_use` is what the radio runs now; when a trial is already
    /// going, the settings in flash stay the ones it started from, so a second change cannot make an
    /// unconfirmed one permanent. `minutes` 0 = the default.
    pub fn start(&mut self, in_use: RadioConfig, now_ms: u64, minutes: u8) {
        let saved = self.0.map_or(in_use, |t| t.saved);
        let minutes = if minutes == 0 { DEFAULT_MINUTES } else { minutes.min(MAX_MINUTES) };
        self.0 = Some(Trial { saved, deadline_ms: now_ms.saturating_add(minutes as u64 * 60_000) });
    }

    /// `ConfirmRadio`: true if there was a trial, so the node must now save what it runs.
    pub fn confirm(&mut self) -> bool {
        self.0.take().is_some()
    }

    /// A local change (USB, Bluetooth) is saved at once and ends any trial.
    pub fn clear(&mut self) {
        self.0 = None;
    }

    /// Call now and then. When the time is up the trial ends and the settings to go back to are returned.
    pub fn tick(&mut self, now_ms: u64) -> Tick {
        match self.0 {
            Some(t) if now_ms >= t.deadline_ms => {
                self.0 = None;
                Tick::Revert(t.saved)
            }
            _ => Tick::Idle,
        }
    }

    /// Seconds left (0 = no trial).
    pub fn left_s(&self, now_ms: u64) -> u32 {
        self.0.map_or(0, |t| (t.deadline_ms.saturating_sub(now_ms) / 1000) as u32)
    }

    pub fn active(&self) -> bool {
        self.0.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(sf: u8) -> RadioConfig {
        RadioConfig { sf, ..RadioConfig::EU_DEFAULT }
    }

    #[test]
    fn unconfirmed_change_goes_back() {
        let mut t = RadioTrial::new();
        t.start(cfg(9), 1_000, 0);
        assert!(t.active());
        assert_eq!(t.left_s(1_000), 300);
        assert_eq!(t.tick(1_000 + 299_999), Tick::Idle);
        assert_eq!(t.tick(1_000 + 300_000), Tick::Revert(cfg(9)));
        assert!(!t.active());
        assert_eq!(t.tick(10_000_000), Tick::Idle);
    }

    #[test]
    fn confirmed_change_stays() {
        let mut t = RadioTrial::new();
        t.start(cfg(9), 0, 10);
        assert!(t.confirm());
        assert_eq!(t.tick(u64::MAX), Tick::Idle);
        // A repeated confirmation (its reply was lost) is harmless.
        assert!(!t.confirm());
    }

    #[test]
    fn second_change_still_returns_to_flash() {
        let mut t = RadioTrial::new();
        t.start(cfg(9), 0, 0); // SF9 in flash, trying SF11
        t.start(cfg(11), 60_000, 0); // trying SF12 on top: flash still SF9
        assert_eq!(t.tick(60_000 + 300_000), Tick::Revert(cfg(9)));
    }

    #[test]
    fn length_is_limited() {
        let mut t = RadioTrial::new();
        t.start(cfg(9), 0, 200);
        assert_eq!(t.left_s(0), MAX_MINUTES as u32 * 60);
    }

    #[test]
    fn local_change_ends_the_trial() {
        let mut t = RadioTrial::new();
        t.start(cfg(9), 0, 0);
        t.clear();
        assert_eq!(t.tick(u64::MAX), Tick::Idle);
        assert_eq!(t.left_s(0), 0);
    }
}
