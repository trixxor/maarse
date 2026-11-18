//! Radio model shared by the maarse-sim CLI and the Denmark simulator.

use maarse_core::airtime::LoraParams;

/// Rough radio model: log-distance path loss (n = 3), 868 MHz and 125 kHz
/// unless [`Radio::with_band`] says otherwise.
pub struct Radio {
    pub lora: LoraParams,
    pub tx_dbm: f64,
    pub noise_dbm: f64,
    /// Lowest SNR that can be received at this spreading factor.
    pub snr_min: f64,
    /// Extra attenuation (dB per metre), e.g. forest.
    pub extra_db_m: f64,
    /// Path loss at 1 m (free space at the carrier frequency).
    pub loss_1m_db: f64,
}

impl Radio {
    pub fn new(sf: u8, tx_dbm: f64) -> Self {
        Self::with_band(sf, tx_dbm, 868.0, 125_000)
    }

    /// Another carrier frequency (MHz) and bandwidth (Hz): the 1 m path loss
    /// follows free space (20 log f - 27.55), the noise floor the bandwidth,
    /// and the airtime the bandwidth. 868 MHz / 125 kHz give the numbers of
    /// [`Radio::new`] exactly.
    pub fn with_band(sf: u8, tx_dbm: f64, freq_mhz: f64, bw_hz: u32) -> Self {
        let bw_hz = if bw_hz == 0 { 125_000 } else { bw_hz };
        // Thermal noise at the bandwidth + 6 dB noise figure.
        let noise_dbm = -174.0 + 10.0 * (bw_hz as f64).log10() + 6.0;
        let loss_1m_db = if freq_mhz <= 0.0 || (freq_mhz - 868.0).abs() < 1e-9 { 31.2 } else { 20.0 * freq_mhz.log10() - 27.55 };
        let mut lora = LoraParams::new(sf);
        lora.bw_hz = bw_hz;
        let snr_min = match sf {
            7 => -7.5,
            8 => -10.0,
            9 => -12.5,
            10 => -15.0,
            11 => -17.5,
            _ => -20.0,
        };
        Radio {
            lora,
            tx_dbm,
            noise_dbm,
            snr_min,
            extra_db_m: 0.0,
            loss_1m_db,
        }
    }

    pub fn rx_dbm(&self, dist_m: f64, gain_db: f64) -> f64 {
        let d = dist_m.max(1.0);
        let path_loss = self.loss_1m_db + 30.0 * d.log10();
        self.tx_dbm + gain_db - path_loss - self.extra_db_m * d
    }

    pub fn snr(&self, rx_dbm: f64) -> f64 {
        rx_dbm - self.noise_dbm
    }
}

