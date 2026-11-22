//! BME280 / BMP280 over I2C, for both boards. The maths is in
//! `maarse_core::sensor::bme280` (tested against the data sheet's example).

use embassy_time::Timer;
use embedded_hal_async::i2c::I2c;
use maarse_core::sensor::bme280::{Calib, ID_BME280, ID_BMP280};

/// One forced-mode measurement: temperature in 0.01 °C, pressure in Pa and
/// humidity in 0.01 %RH (`None` on a BMP280, which has no humidity sensor).
pub async fn read<I: I2c>(i2c: &mut I, addr: u8) -> Result<(i32, u32, Option<u32>), ()> {
    let mut id = [0u8; 1];
    i2c.write_read(addr, &[0xD0], &mut id).await.map_err(|_| ())?;
    let has_humidity = match id[0] {
        ID_BME280 => true,
        ID_BMP280 | 0x56 | 0x57 => false,
        _ => return Err(()),
    };
    let mut a = [0u8; 24];
    i2c.write_read(addr, &[0x88], &mut a).await.map_err(|_| ())?;
    let (mut h1, mut h) = (0u8, [0u8; 7]);
    if has_humidity {
        let mut b = [0u8; 1];
        i2c.write_read(addr, &[0xA1], &mut b).await.map_err(|_| ())?;
        h1 = b[0];
        i2c.write_read(addr, &[0xE1], &mut h).await.map_err(|_| ())?;
        // Humidity oversampling x1 (must be written before ctrl_meas).
        i2c.write(addr, &[0xF2, 0x01]).await.map_err(|_| ())?;
    }
    // Temperature and pressure oversampling x1, forced mode: one measurement,
    // then back to sleep (0.1 µA).
    i2c.write(addr, &[0xF4, 0x25]).await.map_err(|_| ())?;
    // The longest measurement (all three at x1) takes about 9.3 ms.
    Timer::after_millis(15).await;
    let mut data = [0u8; 8];
    i2c.write_read(addr, &[0xF7], &mut data).await.map_err(|_| ())?;
    Ok(Calib::from_regs(&a, h1, &h).compensate(&data, has_humidity))
}
