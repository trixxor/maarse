//! Batterimåling (SAADC) og USB-strøm på nRF52840-boardene.
//!
//! Referencen er den interne 0,6 V med forstærkning 1/5, dvs. 3,0 V fuldt
//! udslag ved 12 bit (som `analogReference(AR_INTERNAL_3_0)` i Arduino-
//! koden, Meshtastic og MeshCore bruger).
//!
//! * XIAO nRF52840: VBAT via deler R17 = 1 MΩ / R18 = 510 kΩ til P0.31
//!   (AIN7); deleren tændes ved at trække P0.14 (VBAT_ENABLE) lav. Faktor
//!   (1000 + 510) / 510 = 2,961 (Meshtastic `seeed_xiao_nrf52840_kit`
//!   bruger 3,0; MeshCore `XiaoNrf52Board::getBattMilliVolts` læser med
//!   AR_INTERNAL_3_0 og 12 bit). P0.14 holdes lav hele tiden: står den høj,
//!   får P0.31 hele batterispændingen, som er over benets maksimum.
//! * RAK4630 (WisMesh Tag): deler R22 = 1 MΩ / R24 = 1,5 MΩ til P0.05
//!   (AIN3), faktor 1,73 (Meshtastic `rak_wismeshtag`, kalibreret ud fra
//!   den nominelle 1,667).
//!
//! USB-strøm ses i POWER-registeret USBREGSTATUS (VBUSDETECT).

use embassy_nrf::gpio::Output;
use embassy_nrf::saadc::Saadc;
use embassy_time::Timer;
use maarse_fw_common::power;

/// Spænding (mV) pr. ADC-trin × 1000: 3000 mV / 4096 × deler.
#[cfg(feature = "board-xiao-nrf52840")]
const DIVIDER_X1000: u32 = 2961;
#[cfg(feature = "board-wismesh-tag")]
const DIVIDER_X1000: u32 = 1730;

fn vbus_present() -> bool {
    // POWER (0x4000_0000) + USBREGSTATUS (0x438), bit 0 = VBUSDETECT.
    // Kun læsning; embassy-nrf's egen VBUS-detektering bruges ikke (USB
    // kører med SoftwareVbusDetect).
    const USBREGSTATUS: *const u32 = 0x4000_0438 as *const u32;
    // SAFETY: registeret findes altid på nRF52840 og kan læses når som helst.
    unsafe { core::ptr::read_volatile(USBREGSTATUS) & 1 != 0 }
}

#[embassy_executor::task]
pub async fn battery_task(mut adc: Saadc<'static, 1>, enable: Option<Output<'static>>) {
    // Deleren skal være tændt (lav) hele tiden på XIAO (se ovenfor).
    let _enable = enable;
    Timer::after_millis(10).await;
    adc.calibrate().await;
    loop {
        let mut samples = [0u16; power::SAMPLES];
        for s in samples.iter_mut() {
            let mut buf = [0i16; 1];
            adc.sample(&mut buf).await;
            let raw = buf[0].max(0) as u64;
            *s = (raw * 3000 * DIVIDER_X1000 as u64 / 4096 / 1000) as u16;
            Timer::after_millis(5).await;
        }
        power::store_battery(samples);
        power::EXT_POWER.store(vbus_present() as i32, core::sync::atomic::Ordering::Relaxed);
        Timer::after_secs(power::SAMPLE_EVERY_S).await;
    }
}
