//! Sensor hardware on the nRF52840: reads the sources of the recipe and sets
//! the outputs.
//!
//! The XIAO nRF52840 with the Wio-SX1262 uses most of the header for the
//! radio (D1-D5 and D8-D10), so only these pins are free:
//!
//! | pin id | header | nRF pin | use                         |
//! |--------|--------|---------|-----------------------------|
//! | 0      | D0     | P0.02   | digital in/out              |
//! | 6      | D6     | P1.11   | digital in/out, or I2C SDA  |
//! | 7      | D7     | P1.12   | digital in/out, or I2C SCL  |
//!
//! A recipe with a BME280/BMP280 takes D6 and D7 for the I2C bus. Which of the
//! two is decided at start-up from the stored recipe (`Recipe::pin_plan`), and
//! the node restarts when a new recipe changes it.
//!
//! The RAK WisMesh Tag has no free pins: it only reads the battery and the
//! chip temperature.

use core::sync::atomic::Ordering::Relaxed;
use embassy_futures::select::{Either, select};
use embassy_nrf::gpio::{Flex, OutputDrive, Pull};
use embassy_nrf::twim::Twim;
use maarse_core::sensor::{Input, Pull as RPull, Source, kind};
use maarse_fw_common::bus::{SENSE_DONE, SENSE_REQ, SET_OUT};
use maarse_fw_common::power;

pub struct Pins {
    #[cfg(feature = "board-xiao-nrf52840")]
    pub d0: Flex<'static>,
    #[cfg(feature = "board-xiao-nrf52840")]
    pub d6: Option<Flex<'static>>,
    #[cfg(feature = "board-xiao-nrf52840")]
    pub d7: Option<Flex<'static>>,
    pub i2c: Option<Twim<'static>>,
}

fn pull(p: RPull) -> Pull {
    match p {
        RPull::None => Pull::None,
        RPull::Up => Pull::Up,
        RPull::Down => Pull::Down,
    }
}

impl Pins {
    fn pin(&mut self, id: u8) -> Option<&mut Flex<'static>> {
        match id {
            #[cfg(feature = "board-xiao-nrf52840")]
            0 => Some(&mut self.d0),
            #[cfg(feature = "board-xiao-nrf52840")]
            6 => self.d6.as_mut(),
            #[cfg(feature = "board-xiao-nrf52840")]
            7 => self.d7.as_mut(),
            _ => None,
        }
    }

    async fn read(&mut self, s: &Source, bme: &mut Option<(i32, u32, Option<u32>)>) -> Option<i32> {
        match s.input {
            Input::Battery => Some(power::BATTERY_MV.load(Relaxed) as i32).filter(|v| *v > 0),
            // 0.1 °C in `power`, 0.01 °C in a reading.
            Input::ChipTemp => power::temperature_dc().map(|t| t as i32 * 10),
            Input::GpioIn { pin, pull: p, invert } => {
                let f = self.pin(pin)?;
                f.set_as_input(pull(p));
                // Let a pull-up settle before reading.
                embassy_time::Timer::after_micros(200).await;
                Some((f.is_high() ^ invert) as i32)
            }
            Input::Bme280 { addr, what } => {
                if bme.is_none() {
                    let i2c = self.i2c.as_mut()?;
                    *bme = maarse_fw_common::bme280::read(i2c, addr).await.ok();
                }
                let (t, p, h) = (*bme)?;
                match what {
                    0 => Some(t),
                    1 => h.map(|h| h as i32),
                    _ => Some(p as i32),
                }
            }
            // The output is reported by the node (the level the recipe set).
            Input::GpioOut { .. } => None,
            // Not on this board (yet): no analogue input, no edge counter.
            Input::Adc { .. } | Input::Counter { .. } => None,
        }
    }

    fn set_out(&mut self, s: &Source, on: bool) {
        if let Input::GpioOut { pin, active_low } = s.input
            && let Some(f) = self.pin(pin)
        {
            f.set_as_output(OutputDrive::Standard);
            if on ^ active_low {
                f.set_high();
            } else {
                f.set_low();
            }
        }
    }
}

#[embassy_executor::task]
pub async fn sensor_task(mut pins: Pins) {
    let _ = kind::TEMPERATURE;
    loop {
        match select(SENSE_REQ.wait(), SET_OUT.receive()).await {
            Either::First(sources) => {
                let mut out = [None; 8];
                let mut bme = None;
                for (i, s) in sources.iter().enumerate().take(8) {
                    out[i] = pins.read(s, &mut bme).await;
                }
                let _ = SENSE_DONE.try_send(out);
            }
            Either::Second((s, on)) => pins.set_out(&s, on),
        }
    }
}
