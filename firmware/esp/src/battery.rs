//! Batterimåling på Heltec WiFi LoRa 32 V4.
//!
//! Fra Meshtastic `variants/esp32s3/heltec_v4/variant.h`: batteriet måles
//! på GPIO1 (ADC1, kanal 0) via en deler, som tændes ved at sætte ADC_CTRL
//! (GPIO37) høj (`ADC_CTRL_ENABLED HIGH`). Dæmpning 2,5 dB (lav dæmpning
//! pga. deleren med høj impedans) og faktor `ADC_MULTIPLIER 4.9 * 1.045`
//! = 5,12. Deleren slukkes mellem målingerne, så den ikke trækker strøm.
//!
//! XIAO ESP32-S3 med Wio-SX1262-kittet har ingen batterideler til en ADC,
//! så dér rapporteres batteriet som ukendt (status uden `BATTERY_MV`,
//! telemetri med 0 mV). Ingen af de to boards har en USB-detektering, vi
//! kan læse, så ekstern strøm er ukendt.

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
            // Med kurvekalibrering er resultatet i mV ved benet.
            let mv = adc.read_blocking(&mut pin) as u32;
            *s = (mv * DIVIDER_X1000 / 1000).min(u16::MAX as u32) as u16;
            Timer::after_millis(5).await;
        }
        ctrl.set_low();
        power::store_battery(samples);
        Timer::after_secs(power::SAMPLE_EVERY_S).await;
    }
}
