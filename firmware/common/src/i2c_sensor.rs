//! One reader for every I2C sensor that an `I2cDesc` describes (the catalogue
//! and the sensors a person describes by hand), for both boards.

use embassy_time::Timer;
use embedded_hal_async::i2c::I2c;
use maarse_core::sensor::catalog::{I2cDesc, MAX_READ};

/// What one sensor cycle has already read, so that a chip with several values
/// (temperature and humidity) is measured once.
#[derive(Default)]
pub struct Cycle {
    pub bme: Option<(i32, u32, Option<u32>)>,
    done: heapless::Vec<(I2cDesc, Option<[u8; MAX_READ]>), 4>,
}

impl Cycle {
    /// The value of `d`: the chip is set up, triggered and read the first time
    /// a description of that read is asked for, and the answer is kept.
    pub async fn read_i2c<I: I2c>(&mut self, i2c: &mut I, d: &I2cDesc) -> Option<i32> {
        if let Some((_, got)) = self.done.iter().find(|(o, _)| o.same_read(d)) {
            return d.extract(&(*got)?);
        }
        let got = read_bytes(i2c, d).await.ok();
        if self.done.is_full() {
            self.done.remove(0);
        }
        let _ = self.done.push((*d, got));
        d.extract(&got?)
    }
}

/// Runs the steps of `d` and returns what the chip answered.
pub async fn read_bytes<I: I2c>(i2c: &mut I, d: &I2cDesc) -> Result<[u8; MAX_READ], ()> {
    let mut any_init = false;
    for cmd in d.init_commands() {
        i2c.write(d.addr, cmd).await.map_err(|_| ())?;
        any_init = true;
    }
    if any_init {
        // Let the chip take its set-up before the next command.
        Timer::after_millis(10).await;
    }
    if d.trigger_len > 0 {
        i2c.write(d.addr, d.trigger_bytes()).await.map_err(|_| ())?;
    }
    if d.wait_ms > 0 {
        Timer::after_millis(d.wait_ms as u64).await;
    }
    if d.ptr_len > 0 {
        i2c.write(d.addr, d.ptr_bytes()).await.map_err(|_| ())?;
        Timer::after_millis(1).await;
    }
    let mut buf = [0u8; MAX_READ];
    i2c.read(d.addr, &mut buf[..d.read_len as usize]).await.map_err(|_| ())?;
    Ok(buf)
}
