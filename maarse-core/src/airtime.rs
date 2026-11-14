//! LoRa-sendetid (time on air) efter Semtechs formel (AN1200.13).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoraParams {
    /// Spreading factor 7..=12.
    pub sf: u8,
    /// Båndbredde i Hz, fx 125_000.
    pub bw_hz: u32,
    /// Coding rate 1..=4 svarer til 4/5..4/8.
    pub cr: u8,
    /// Antal præambel-symboler.
    pub preamble: u16,
}

impl LoraParams {
    pub const fn new(sf: u8) -> Self {
        LoraParams {
            sf,
            bw_hz: 125_000,
            cr: 1,
            preamble: 8,
        }
    }

    /// Symboltid i mikrosekunder.
    pub fn symbol_us(&self) -> u32 {
        ((1u64 << self.sf) * 1_000_000 / self.bw_hz as u64) as u32
    }

    /// Low data rate optimization kræves, når symboltiden er over 16 ms.
    fn low_dr_opt(&self) -> bool {
        self.symbol_us() > 16_000
    }

    /// Sendetid i mikrosekunder for en pakke på `len` bytes
    /// (eksplicit header, CRC slået til).
    pub fn airtime_us(&self, len: usize) -> u32 {
        let sf = self.sf as i64;
        let de = self.low_dr_opt() as i64;
        let tsym = self.symbol_us() as i64;
        // Præamblen: (n + 4,25) symboler = (4n + 17) / 4.
        let t_pre = (4 * self.preamble as i64 + 17) * tsym / 4;
        let num = 8 * len as i64 - 4 * sf + 28 + 16; // CRC=1, eksplicit header
        let den = 4 * (sf - 2 * de);
        let blocks = if num > 0 { (num + den - 1) / den } else { 0 };
        let n_payload = 8 + blocks * (self.cr as i64 + 4);
        (t_pre + n_payload * tsym) as u32
    }

    pub fn airtime_ms(&self, len: usize) -> u32 {
        self.airtime_us(len).div_ceil(1000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_reference_values() {
        // Udregnet i hånden efter AN1200.13: 125 kHz, CR 4/5,
        // 8 præambel-symboler, eksplicit header, CRC.
        let sf7 = LoraParams::new(7);
        assert_eq!(sf7.airtime_us(10), 41_216);
        let sf9 = LoraParams::new(9);
        assert_eq!(sf9.airtime_us(50), 328_704);
        // SF12 bruger low data rate optimization.
        let sf12 = LoraParams::new(12);
        assert_eq!(sf12.airtime_us(255), 9_019_392);
    }

    #[test]
    fn longer_is_slower() {
        let p = LoraParams::new(9);
        assert!(p.airtime_us(200) > p.airtime_us(100));
        assert!(LoraParams::new(10).airtime_us(100) > p.airtime_us(100));
    }
}
