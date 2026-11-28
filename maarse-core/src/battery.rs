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

/// What the time left rests on.
pub mod basis {
    /// Not known: no voltage, or the board's battery size is not known.
    pub const UNKNOWN: u8 = 0;
    /// On external power: there is no time left to give.
    pub const ON_POWER: u8 = 1;
    /// From how fast the voltage has fallen since the board came off power.
    pub const MEASURED: u8 = 2;
    /// From the battery size and typical currents, until enough is measured.
    pub const ESTIMATED: u8 = 3;
}

/// Charge in 0.1 %, from the same table as [`percent`].
pub fn permille(mv: u16) -> u16 {
    const OCV: [u16; 11] = [4160, 4020, 3940, 3870, 3810, 3760, 3740, 3720, 3680, 3620, 2990];
    if mv >= OCV[0] {
        return 1000;
    }
    for i in 1..OCV.len() {
        if mv >= OCV[i] {
            let (hi, lo) = (OCV[i - 1] as u32, OCV[i] as u32);
            return ((10 - i as u32) * 100 + (mv as u32 - lo) * 100 / (hi - lo)) as u16;
        }
    }
    0
}

/// Minutes between the samples of a [`Trend`].
pub const TREND_EVERY_MIN: u32 = 10;
/// Samples kept: six hours.
const TREND_LEN: usize = 36;
/// A fall is trusted after this long and this many samples.
const TREND_MIN_SPAN_MIN: u32 = 60;
const TREND_MIN_SAMPLES: usize = 6;
/// A fall slower than this (0.1 % per hour) cannot be told from noise.
const TREND_MIN_FALL: u32 = 2;
/// A rise of this much (0.1 %) means the battery was charged in between.
const CHARGED_RISE: u16 = 50;

/// How the battery has fallen since the board last came off power.
#[derive(Default)]
pub struct Trend {
    /// (minute since start, charge in 0.1 %)
    samples: heapless::Deque<(u32, u16), TREND_LEN>,
}

impl Trend {
    /// A voltage measured at `minute`. External power, no voltage and a
    /// charge in between start the measurement again.
    pub fn add(&mut self, minute: u32, mv: u16, ext_power: bool) {
        if ext_power || mv == 0 {
            self.samples.clear();
            return;
        }
        let now = permille(mv);
        if let Some(&(at, last)) = self.samples.back() {
            if now > last.saturating_add(CHARGED_RISE) {
                self.samples.clear();
            } else if minute < at.saturating_add(TREND_EVERY_MIN) {
                return;
            }
        }
        if self.samples.is_full() {
            self.samples.pop_front();
        }
        let _ = self.samples.push_back((minute, now));
    }

    /// The fall in 0.1 % per hour: the slope of the line that fits the samples
    /// best (least squares), so that one noisy reading decides little.
    pub fn fall_per_hour(&self) -> Option<u32> {
        let (first, last) = (self.samples.front()?.0, self.samples.back()?.0);
        if self.samples.len() < TREND_MIN_SAMPLES || last - first < TREND_MIN_SPAN_MIN {
            return None;
        }
        let (mut n, mut sx, mut sy, mut sxy, mut sxx) = (0i64, 0i64, 0i64, 0i64, 0i64);
        for &(at, y) in &self.samples {
            let (x, y) = ((at - first) as i64, y as i64);
            n += 1;
            sx += x;
            sy += y;
            sxy += x * y;
            sxx += x * x;
        }
        let below = n * sxx - sx * sx;
        if below == 0 {
            return None;
        }
        // Per hour, rounded; negative = rising.
        let per_hour = (-(n * sxy - sx * sy) * 60 + below / 2).div_euclid(below);
        (per_hour >= TREND_MIN_FALL as i64).then_some(per_hour as u32)
    }
}

/// Estimated current by consumer, in 0.1 mA.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Draw {
    pub base: u32,
    pub listen: u32,
    pub send: u32,
    pub gps: u32,
}

impl Draw {
    /// `base`, `rx`, `tx` and `gps` are typical currents (0.1 mA) while each
    /// runs; `tx_ms` of `total_ms` were spent sending. `gps` is 0 when off.
    pub fn new(base: u32, rx: u32, tx: u32, gps: u32, tx_ms: u64, total_ms: u64) -> Draw {
        let total = total_ms.max(1);
        let tx_ms = tx_ms.min(total);
        let share = |current: u32, ms: u64| ((current as u64 * ms + total / 2) / total) as u32;
        Draw { base, listen: share(rx, total - tx_ms), send: share(tx, tx_ms), gps }
    }

    pub fn total(&self) -> u32 {
        self.base + self.listen + self.send + self.gps
    }
}

/// The time left: (what it rests on, minutes). `capacity_mah` 0 = unknown.
pub fn time_left(mv: u16, ext_power: bool, trend: &Trend, capacity_mah: u32, draw: &Draw) -> (u8, Option<u32>) {
    if ext_power {
        return (basis::ON_POWER, None);
    }
    if mv == 0 {
        return (basis::UNKNOWN, None);
    }
    let charge = permille(mv) as u64;
    if let Some(fall) = trend.fall_per_hour() {
        return (basis::MEASURED, Some((charge * 60 / fall as u64) as u32));
    }
    if capacity_mah == 0 || draw.total() == 0 {
        return (basis::UNKNOWN, None);
    }
    // mAh left = capacity x charge / 1000; hours = mAh / (total / 10).
    let minutes = capacity_mah as u64 * charge * 600 / (1000 * draw.total() as u64);
    (basis::ESTIMATED, Some(minutes as u32))
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

    /// A voltage that gives about `p` tenths of a percent.
    fn mv_for(p: u16) -> u16 {
        (2990..=4200).find(|&mv| permille(mv) >= p).unwrap()
    }

    #[test]
    fn permille_agrees_with_percent() {
        for mv in (2900..4300).step_by(7) {
            assert!((permille(mv) / 10).abs_diff(percent(mv) as u16) <= 1, "{mv}");
        }
    }

    #[test]
    fn a_fall_is_measured_after_an_hour_and_noise_does_not_decide_it() {
        let mut t = Trend::default();
        // 2 % per hour with readings that jump a little.
        for i in 0..6u32 {
            let noise = [0i32, 4, -3, 2, -4, 1][i as usize];
            t.add(i * 10, mv_for((800 - i as i32 * 10 / 3 + noise) as u16), false);
        }
        assert_eq!(t.fall_per_hour(), None, "not an hour yet");
        for i in 6..18u32 {
            t.add(i * 10, mv_for((800 - i * 10 / 3) as u16), false);
        }
        let fall = t.fall_per_hour().expect("measured");
        assert!((15..=25).contains(&fall), "{fall}");
    }

    #[test]
    fn power_and_charging_start_the_measurement_again() {
        let mut t = Trend::default();
        for i in 0..8u32 {
            t.add(i * 10, mv_for(800 - i as u16 * 10), false);
        }
        assert!(t.fall_per_hour().is_some());
        t.add(80, 4100, true);
        assert_eq!(t.fall_per_hour(), None);
        for i in 9..17u32 {
            t.add(i * 10, mv_for(500 - i as u16 * 5), false);
        }
        assert!(t.fall_per_hour().is_some());
        // Charged without the board seeing power.
        t.add(170, mv_for(900), false);
        assert_eq!(t.fall_per_hour(), None);
    }

    #[test]
    fn a_flat_or_rising_battery_gives_no_fall() {
        let mut t = Trend::default();
        for i in 0..12u32 {
            t.add(i * 10, 3800, false);
        }
        assert_eq!(t.fall_per_hour(), None);
    }

    #[test]
    fn draw_is_shared_by_time_on_air() {
        // 5 mA base, 5.3 mA listening, 118 mA sending 1 % of the time, GPS 23 mA.
        let d = Draw::new(50, 53, 1180, 230, 1_000, 100_000);
        assert_eq!(d, Draw { base: 50, listen: 52, send: 12, gps: 230 });
        assert_eq!(d.total(), 344);
    }

    #[test]
    fn time_left_says_what_it_rests_on() {
        let t = Trend::default();
        let d = Draw { base: 50, listen: 50, send: 0, gps: 0 };
        assert_eq!(time_left(4000, true, &t, 1000, &d), (basis::ON_POWER, None));
        assert_eq!(time_left(0, false, &t, 1000, &d), (basis::UNKNOWN, None));
        assert_eq!(time_left(4000, false, &t, 0, &d), (basis::UNKNOWN, None));
        // Full, 1000 mAh at 10 mA: 100 hours.
        assert_eq!(time_left(4200, false, &t, 1000, &d), (basis::ESTIMATED, Some(6000)));
        let mut t = Trend::default();
        for i in 0..8u32 {
            t.add(i * 10, mv_for(800 - i as u16 * 10), false);
        }
        let (what, minutes) = time_left(mv_for(500), false, &t, 1000, &d);
        assert_eq!(what, basis::MEASURED);
        // 50 % falling about 6 % per hour: about 8 hours.
        assert!((420..=600).contains(&minutes.unwrap()), "{minutes:?}");
    }

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
