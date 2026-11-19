//! Airtime budget (duty cycle) as a token bucket.
//!
//! EU 868 MHz typically allows 1 % or 10 % airtime depending on the
//! sub-band. The budget fills up evenly, so a node can send a short
//! burst, but never more than the limit over time.

/// Vinduet, budgettet regnes over (1 time).
pub const WINDOW_MS: u64 = 3_600_000;

#[derive(Clone, Debug)]
pub struct DutyCycle {
    /// Limit in permille (10 = 1 %, 100 = 10 %).
    permille: u32,
    /// Saved-up airtime in microseconds.
    tokens_us: u64,
    capacity_us: u64,
    last_ms: u64,
}

impl DutyCycle {
    /// Starts with a full budget.
    pub fn new(permille: u32, now_ms: u64) -> Self {
        let capacity_us = WINDOW_MS * permille as u64; // ms * ‰ = µs
        DutyCycle {
            permille,
            tokens_us: capacity_us,
            capacity_us,
            last_ms: now_ms,
        }
    }

    fn refill(&mut self, now_ms: u64) {
        let elapsed = now_ms.saturating_sub(self.last_ms);
        // elapsed ms * ‰ / 1000 = ms airtime = elapsed * ‰ µs
        self.tokens_us = (self.tokens_us + elapsed * self.permille as u64).min(self.capacity_us);
        self.last_ms = now_ms.max(self.last_ms);
    }

    /// Available airtime right now (µs).
    pub fn available_us(&mut self, now_ms: u64) -> u64 {
        self.refill(now_ms);
        self.tokens_us
    }

    /// Subtracts the airtime from the budget if there is enough.
    pub fn try_consume(&mut self, now_ms: u64, airtime_us: u32) -> bool {
        self.refill(now_ms);
        if self.tokens_us >= airtime_us as u64 {
            self.tokens_us -= airtime_us as u64;
            true
        } else {
            false
        }
    }

    /// How much of the budget is used right now (permille).
    pub fn used_permille(&mut self, now_ms: u64) -> u32 {
        self.refill(now_ms);
        if self.capacity_us == 0 {
            return 0;
        }
        ((self.capacity_us - self.tokens_us) * 1000 / self.capacity_us) as u32
    }

    /// How long (ms) to wait before `airtime_us` can be sent.
    pub fn wait_ms(&mut self, now_ms: u64, airtime_us: u32) -> u64 {
        self.refill(now_ms);
        let need = (airtime_us as u64).saturating_sub(self.tokens_us);
        if need == 0 || self.permille == 0 {
            return if need == 0 { 0 } else { u64::MAX };
        }
        need.div_ceil(self.permille as u64)
    }
}

/// The EU duty-cycle limit (permille) for a LoRa channel at `freq_hz`
/// with bandwidth `bw_hz` (ERC Rec 70-03 annex 1, bands h1.3-h1.7): the
/// whole channel must lie inside one sub-band. 869.4-869.65 MHz allows 10 %,
/// 865-868.6 and 869.7-870 MHz 1 %, 863-865 and 868.7-869.2 MHz 0.1 %;
/// anything else (gaps, other bands) gets 0.1 %, the strictest rule.
pub fn eu_band_permille(freq_hz: u32, bw_hz: u32) -> u32 {
    const BANDS: [(u32, u32, u32); 6] = [
        (869_400_000, 869_650_000, 100),
        (865_000_000, 868_000_000, 10),
        (868_000_000, 868_600_000, 10),
        (869_700_000, 870_000_000, 10),
        (863_000_000, 865_000_000, 1),
        (868_700_000, 869_200_000, 1),
    ];
    let (lo, hi) = (freq_hz.saturating_sub(bw_hz / 2), freq_hz.saturating_add(bw_hz / 2));
    BANDS.iter().find(|(a, b, _)| lo >= *a && hi <= *b).map_or(1, |(_, _, p)| *p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_percent_is_36_seconds_per_hour() {
        let mut d = DutyCycle::new(10, 0);
        assert_eq!(d.available_us(0), 36_000_000);
    }

    #[test]
    fn spends_and_refills() {
        let mut d = DutyCycle::new(10, 0);
        assert!(d.try_consume(0, 36_000_000));
        assert_eq!(d.used_permille(0), 1000);
        assert!(!d.try_consume(0, 1));
        // 1 % of 1000 ms = 10 ms
        assert_eq!(d.available_us(1000), 10_000);
        assert_eq!(d.wait_ms(1000, 20_000), 1000);
        assert!(d.try_consume(2000, 20_000));
    }

    #[test]
    fn never_exceeds_capacity() {
        let mut d = DutyCycle::new(100, 0);
        assert_eq!(d.available_us(10 * WINDOW_MS), 360_000_000);
    }

    #[test]
    fn eu_sub_bands() {
        // The Må:rse default: 869.525 MHz, 125 kHz, inside the 10 % band.
        assert_eq!(eu_band_permille(869_525_000, 125_000), 100);
        // 250 kHz no longer fits in 869.4-869.65: strictest rule.
        assert_eq!(eu_band_permille(869_525_000, 300_000), 1);
        assert_eq!(eu_band_permille(868_100_000, 125_000), 10);
        assert_eq!(eu_band_permille(867_500_000, 125_000), 10);
        assert_eq!(eu_band_permille(869_000_000, 125_000), 1);
        assert_eq!(eu_band_permille(915_000_000, 125_000), 1);
    }
}
