//! Battery measurement (SAADC) and USB power on the nRF52840 boards.
//!
//! The reference is the internal 0.6 V with gain 1/5, i.e. 3.0 V full
//! scale at 12 bits (like `analogReference(AR_INTERNAL_3_0)` in the Arduino
//! code Meshtastic and MeshCore use).
//!
//! * XIAO nRF52840: VBAT via divider R17 = 1 MΩ / R18 = 510 kΩ to P0.31
//!   (AIN7); the divider is switched on by pulling P0.14 (VBAT_ENABLE) low. Factor
//!   (1000 + 510) / 510 = 2.961 (Meshtastic `seeed_xiao_nrf52840_kit`
//!   uses 3.0; MeshCore `XiaoNrf52Board::getBattMilliVolts` reads with
//!   AR_INTERNAL_3_0 and 12 bits). P0.14 is kept low all the time: if it is high,
//!   P0.31 gets the full battery voltage, which is above the pin's maximum.
//! * RAK4630 (WisMesh Tag): divider R22 = 1 MΩ / R24 = 1.5 MΩ to P0.05
//!   (AIN3), factor 1.73 (Meshtastic `rak_wismeshtag`, calibrated from
//!   the nominal 1.667).
//!
//! USB power is seen in the POWER register USBREGSTATUS (VBUSDETECT).

use embassy_nrf::gpio::Output;
use embassy_nrf::saadc::Saadc;
use embassy_time::Timer;
use maarse_fw_common::power;

/// Voltage (mV) per ADC step × 1000: 3000 mV / 4096 × divider.
#[cfg(feature = "board-xiao-nrf52840")]
const DIVIDER_X1000: u32 = 2961;
#[cfg(feature = "board-wismesh-tag")]
const DIVIDER_X1000: u32 = 1730;

pub fn vbus_present() -> bool {
    // POWER (0x4000_0000) + USBREGSTATUS (0x438), bit 0 = VBUSDETECT.
    // Read only; embassy-nrf's own VBUS detection is not used (USB
    // runs with SoftwareVbusDetect).
    const USBREGSTATUS: *const u32 = 0x4000_0438 as *const u32;
    // SAFETY: the register always exists on the nRF52840 and can be read at any time.
    unsafe { core::ptr::read_volatile(USBREGSTATUS) & 1 != 0 }
}

#[embassy_executor::task]
pub async fn battery_task(mut adc: Saadc<'static, 1>, enable: Option<Output<'static>>) {
    // The divider must be on (low) all the time on the XIAO (see above).
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
