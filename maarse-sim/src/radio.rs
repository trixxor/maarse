//! Radio model shared by the maarse-sim CLI and the Denmark simulator.

use maarse_core::airtime::LoraParams;

/// Rough radio model: log-distance path loss (n = 3) at 868 MHz.
pub struct Radio {
    pub lora: LoraParams,
    pub tx_dbm: f64,
    pub noise_dbm: f64,
    /// Lowest SNR that can be received at this spreading factor.
    pub snr_min: f64,
    /// Extra attenuation (dB per metre), e.g. forest.
    pub extra_db_m: f64,
}

impl Radio {
    pub fn new(sf: u8, tx_dbm: f64) -> Self {
        // Thermal noise at 125 kHz + 6 dB noise figure.
        let noise_dbm = -174.0 + 10.0 * 125_000f64.log10() + 6.0;
        let snr_min = match sf {
            7 => -7.5,
            8 => -10.0,
            9 => -12.5,
            10 => -15.0,
            11 => -17.5,
            _ => -20.0,
        };
        Radio {
            lora: LoraParams::new(sf),
            tx_dbm,
            noise_dbm,
            snr_min,
            extra_db_m: 0.0,
        }
    }

    pub fn rx_dbm(&self, dist_m: f64, gain_db: f64) -> f64 {
        let d = dist_m.max(1.0);
        let path_loss = 31.2 + 30.0 * d.log10();
        self.tx_dbm + gain_db - path_loss - self.extra_db_m * d
    }

    pub fn snr(&self, rx_dbm: f64) -> f64 {
        rx_dbm - self.noise_dbm
    }
}

