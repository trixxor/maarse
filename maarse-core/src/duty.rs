//! Sendetidsbudget (duty cycle) som token bucket.
//!
//! EU 868 MHz tillader typisk 1 % eller 10 % sendetid afhængigt af
//! underbåndet. Budgettet fyldes jævnt op, så en node kan sende en kort
//! byge, men aldrig mere end grænsen over tid.

/// Vinduet, budgettet regnes over (1 time).
pub const WINDOW_MS: u64 = 3_600_000;

#[derive(Clone, Debug)]
pub struct DutyCycle {
    /// Grænse i promille (10 = 1 %, 100 = 10 %).
    permille: u32,
    /// Opsparet sendetid i mikrosekunder.
    tokens_us: u64,
    capacity_us: u64,
    last_ms: u64,
}

impl DutyCycle {
    /// Starter med fuldt budget.
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
        // elapsed ms * ‰ / 1000 = ms sendetid = elapsed * ‰ µs
        self.tokens_us = (self.tokens_us + elapsed * self.permille as u64).min(self.capacity_us);
        self.last_ms = now_ms.max(self.last_ms);
    }

    /// Tilgængelig sendetid lige nu (µs).
    pub fn available_us(&mut self, now_ms: u64) -> u64 {
        self.refill(now_ms);
        self.tokens_us
    }

    /// Trækker sendetiden fra budgettet, hvis der er råd.
    pub fn try_consume(&mut self, now_ms: u64, airtime_us: u32) -> bool {
        self.refill(now_ms);
        if self.tokens_us >= airtime_us as u64 {
            self.tokens_us -= airtime_us as u64;
            true
        } else {
            false
        }
    }

    /// Hvor stor en del af budgettet der er brugt lige nu (promille).
    pub fn used_permille(&mut self, now_ms: u64) -> u32 {
        self.refill(now_ms);
        if self.capacity_us == 0 {
            return 0;
        }
        ((self.capacity_us - self.tokens_us) * 1000 / self.capacity_us) as u32
    }

    /// Hvor længe (ms) der skal ventes, før `airtime_us` kan sendes.
    pub fn wait_ms(&mut self, now_ms: u64, airtime_us: u32) -> u64 {
        self.refill(now_ms);
        let need = (airtime_us as u64).saturating_sub(self.tokens_us);
        if need == 0 || self.permille == 0 {
            return if need == 0 { 0 } else { u64::MAX };
        }
        need.div_ceil(self.permille as u64)
    }
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
        // 1 % af 1000 ms = 10 ms
        assert_eq!(d.available_us(1000), 10_000);
        assert_eq!(d.wait_ms(1000, 20_000), 1000);
        assert!(d.try_consume(2000, 20_000));
    }

    #[test]
    fn never_exceeds_capacity() {
        let mut d = DutyCycle::new(100, 0);
        assert_eq!(d.available_us(10 * WINDOW_MS), 360_000_000);
    }
}
