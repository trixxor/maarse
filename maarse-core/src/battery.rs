//! Battery voltage → energy status (see [`crate::Energy`]).
//!
//! The thresholds apply to one Li-ion/LiPo cell measured without heavy load
//! (the nodes draw 5-15 mA in RX, so the voltage drop is small):
//!
//! | voltage      | approx. charge | status   | why                                        |
//! |--------------|----------------|----------|--------------------------------------------|
//! | ≥ 3.90 V     | > 70 %         | `Full`   | plenty; forward everything                 |
//! | 3.60-3.90 V  | 20-70 %        | `Ok`     | the flat part of the discharge curve       |
//! | 3.40-3.60 V  | 5-20 %         | `Low`    | below the "knee" the voltage falls fast    |
//! | < 3.40 V     | < 5 %          | `Critical` | close to the protection circuit cut-off  |
//!
//! The curve matches RAK's OCV table in Meshtastic's `rak_wismeshtag` variant
//! (4160, 4020, 3940, 3870, 3810, 3760, 3740, 3720, 3680, 3620, 2990 mV for
//! 100 %, 90 %, ..., 0 %): 3.9 V ≈ 75 %, 3.6 V ≈ 10-15 %.
//!
//! So that the node does not flip back and forth when the voltage sits right
//! at a threshold (and rises a little when the load drops), the voltage must
//! be [`HYSTERESIS_MV`] above the threshold to go one step up.

use crate::Energy;

/// Limits (mV) for `Low`, `Ok` and `Full`.
pub const THRESHOLDS_MV: [u16; 3] = [3400, 3600, 3900];
pub const HYSTERESIS_MV: u16 = 50;

fn level(mv: u16, offset: u16) -> u8 {
    THRESHOLDS_MV.iter().filter(|&&t| mv >= t + offset).count() as u8
}

fn from_level(l: u8) -> Energy {
    match l {
        0 => Energy::Critical,
        1 => Energy::Low,
        2 => Energy::Ok,
        _ => Energy::Full,
    }
}

/// New energy status from an (average) voltage and the previous status.
/// `mv == 0` means unknown (no measurement): mains power is then assumed.
/// If the node runs on USB power, it is always `Full`.
pub fn energy_for(mv: u16, ext_power: bool, prev: Energy) -> Energy {
    if ext_power || mv == 0 {
        return Energy::Full;
    }
    let down = level(mv, 0);
    let prev_l = prev as u8;
    if down <= prev_l {
        return from_level(down);
    }
    // Going up requires hysteresis.
    from_level(level(mv, HYSTERESIS_MV).max(prev_l))
}

/// Approximate charge in percent (linear between the points of the OCV table).
pub fn percent(mv: u16) -> u8 {
    const OCV: [u16; 11] = [4160, 4020, 3940, 3870, 3810, 3760, 3740, 3720, 3680, 3620, 2990];
    if mv >= OCV[0] {
        return 100;
    }
    for i in 1..OCV.len() {
        if mv >= OCV[i] {
            let (hi, lo) = (OCV[i - 1] as u32, OCV[i] as u32);
            let base = (10 - i as u32) * 10;
            return (base + (mv as u32 - lo) * 10 / (hi - lo)) as u8;
        }
    }
    0
}

/// Moving average of measurements (mV), so single swings (e.g. during
/// transmission) do not move the status.
#[derive(Clone, Copy, Debug, Default)]
pub struct Average {
    sum: u32,
    n: u32,
}

impl Average {
    pub const fn new() -> Self {
        Average { sum: 0, n: 0 }
    }
    pub fn add(&mut self, mv: u16) {
        self.sum += mv as u32;
        self.n += 1;
    }
    /// The average, and reset (0 without measurements).
    pub fn take(&mut self) -> u16 {
        let v = self.sum.checked_div(self.n).unwrap_or(0);
        *self = Self::new();
        v as u16
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thresholds() {
        assert_eq!(energy_for(4100, false, Energy::Full), Energy::Full);
        assert_eq!(energy_for(3800, false, Energy::Full), Energy::Ok);
        assert_eq!(energy_for(3500, false, Energy::Ok), Energy::Low);
        assert_eq!(energy_for(3300, false, Energy::Low), Energy::Critical);
        // Can drop several steps at once.
        assert_eq!(energy_for(3300, false, Energy::Full), Energy::Critical);
    }

    #[test]
    fn hysteresis_upwards() {
        // Just above the threshold: stay where we are.
        assert_eq!(energy_for(3920, false, Energy::Ok), Energy::Ok);
        assert_eq!(energy_for(3950, false, Energy::Ok), Energy::Full);
        assert_eq!(energy_for(3420, false, Energy::Critical), Energy::Critical);
        assert_eq!(energy_for(3460, false, Energy::Critical), Energy::Low);
        // Charging from critical to full in one go.
        assert_eq!(energy_for(4150, false, Energy::Critical), Energy::Full);
    }

    #[test]
    fn usb_and_unknown() {
        assert_eq!(energy_for(3300, true, Energy::Critical), Energy::Full);
        assert_eq!(energy_for(0, false, Energy::Low), Energy::Full);
    }

    #[test]
    fn percent_and_average() {
        assert_eq!(percent(4200), 100);
        assert_eq!(percent(2900), 0);
        assert_eq!(percent(3810), 60);
        assert!((70..=80).contains(&percent(3900)));
        let mut a = Average::new();
        a.add(3700);
        a.add(3900);
        assert_eq!(a.take(), 3800);
        assert_eq!(a.take(), 0);
    }
}
