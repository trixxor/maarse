//! Battery measurement on the Heltec WiFi LoRa 32 V4.
//!
//! From Meshtastic `variants/esp32s3/heltec_v4/variant.h`: the battery is measured
//! on GPIO1 (ADC1, channel 0) via a divider, which is switched on by setting ADC_CTRL
//! (GPIO37) high (`ADC_CTRL_ENABLED HIGH`). Attenuation 2.5 dB (low attenuation
//! because of the high-impedance divider) and factor `ADC_MULTIPLIER 4.9 * 1.045`
//! = 5.12. The divider is switched off between measurements, so it draws no current.
//!
//! The XIAO ESP32-S3 with the Wio-SX1262 kit has no battery divider to an ADC,
//! so there the battery is reported as unknown (status without `BATTERY_MV`,
//! telemetry with 0 mV). Neither of the two boards has USB detection we
//! can read, so external power is unknown.

use esp_hal::analog::adc::{Adc, AdcCalCurve, AdcConfig, AdcPin, Attenuation};
use esp_hal::gpio::Output;
use esp_hal::peripherals::{ADC1, GPIO1};
use embassy_time::Timer;
use maarse_fw_common::power;

/// `4.9 * 1.045` × 1000.
const DIVIDER_X1000: u32 = 5121;

type Pin = AdcPin<GPIO1<'static>, ADC1<'static>, AdcCalCurve<ADC1<'static>>>;

pub fn setup(adc1: ADC1<'static>, gpio1: GPIO1<'static>) -> (Adc<'static, ADC1<'static>, esp_hal::Blocking>, Pin) {
    let mut cfg = AdcConfig::new();
    let pin = cfg.enable_pin_with_cal::<_, AdcCalCurve<ADC1<'static>>>(gpio1, Attenuation::_2p5dB);
    (Adc::new(adc1, cfg), pin)
}

#[embassy_executor::task]
pub async fn battery_task(
    mut adc: Adc<'static, ADC1<'static>, esp_hal::Blocking>,
    mut pin: Pin,
    mut ctrl: Output<'static>,
) {
    loop {
        ctrl.set_high();
        Timer::after_millis(10).await;
        let mut samples = [0u16; power::SAMPLES];
        for s in samples.iter_mut() {
            // With curve calibration the result is in mV at the pin.
            let mv = adc.read_blocking(&mut pin) as u32;
            *s = (mv * DIVIDER_X1000 / 1000).min(u16::MAX as u32) as u16;
            Timer::after_millis(5).await;
        }
        ctrl.set_low();
        power::store_battery(samples);
        Timer::after_secs(power::SAMPLE_EVERY_S).await;
    }
}
