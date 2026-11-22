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
use embassy_futures::select::{Either3, select3};
use embassy_nrf::gpio::{Flex, OutputDrive, Pull};
use embassy_nrf::twim::Twim;
use maarse_core::sensor::{Input, Pull as RPull, Source, kind};
use maarse_fw_common::bus::{PIN_WAKE, SENSE_DONE, SENSE_REQ, SET_OUT};
use maarse_fw_common::i2c_sensor::Cycle;
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

    async fn read(&mut self, s: &Source, cy: &mut Cycle) -> Option<i32> {
        match s.input {
            Input::Battery => Some(power::BATTERY_MV.load(Relaxed) as i32).filter(|v| *v > 0),
            // 0.1 °C in `power`, 0.01 °C in a reading.
            Input::ChipTemp => power::temperature_dc().map(|t| t as i32 * 10),
            Input::GpioIn { pin, pull: p, invert, .. } => {
                let f = self.pin(pin)?;
                f.set_as_input(pull(p));
                // Let a pull-up settle before reading.
                embassy_time::Timer::after_micros(200).await;
                Some((f.is_high() ^ invert) as i32)
            }
            Input::I2c(_) => {
                let d = s.input.i2c_desc()?;
                let i2c = self.i2c.as_mut()?;
                cy.read_i2c(i2c, &d).await
            }
            Input::Bme280 { addr, what } => {
                if cy.bme.is_none() {
                    let i2c = self.i2c.as_mut()?;
                    cy.bme = maarse_fw_common::bme280::read(i2c, addr).await.ok();
                }
                let (t, p, h) = cy.bme?;
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

/// Waits for the first wake pin to go to its wake level (a reed switch, a
/// button) and says which. The pin is level-triggered by the chip's PORT
/// event, so the CPU can sleep meanwhile (System ON). A pin that is still at
/// its wake level is waited for after it has been away: one event, one wake.
#[cfg(feature = "board-xiao-nrf52840")]
async fn wait_wake(pins: &mut Pins, wake: &[(u8, bool)]) -> u8 {
    async fn one(f: Option<&mut Flex<'static>>, high: Option<bool>, id: u8) -> u8 {
        match (f, high) {
            (Some(f), Some(true)) => {
                f.wait_for_rising_edge().await;
                id
            }
            (Some(f), Some(false)) => {
                f.wait_for_falling_edge().await;
                id
            }
            _ => core::future::pending().await,
        }
    }
    let level = |id: u8| wake.iter().find(|w| w.0 == id).map(|w| w.1);
    let Pins { d0, d6, d7, .. } = pins;
    match select3(one(Some(d0), level(0), 0), one(d6.as_mut(), level(6), 6), one(d7.as_mut(), level(7), 7)).await {
        Either3::First(p) | Either3::Second(p) | Either3::Third(p) => p,
    }
}

#[cfg(not(feature = "board-xiao-nrf52840"))]
async fn wait_wake(_pins: &mut Pins, _wake: &[(u8, bool)]) -> u8 {
    core::future::pending().await
}

#[embassy_executor::task]
pub async fn sensor_task(mut pins: Pins, wake: heapless::Vec<(u8, bool), 2>) {
    let _ = kind::TEMPERATURE;
    loop {
        // Wake pins are inputs held at the level that does not wake the node.
        for &(pin, high) in wake.iter() {
            if let Some(f) = pins.pin(pin) {
                f.set_as_input(if high { Pull::Down } else { Pull::Up });
            }
        }
        let woke = wait_wake(&mut pins, &wake);
        match select3(SENSE_REQ.wait(), SET_OUT.receive(), woke).await {
            Either3::Third(pin) => PIN_WAKE.signal(pin),
            Either3::First(sources) => {
                let mut out = [None; 8];
                let mut cy = Cycle::default();
                for (i, s) in sources.iter().enumerate().take(8) {
                    out[i] = pins.read(s, &mut cy).await;
                }
                let _ = SENSE_DONE.try_send(out);
            }
            Either3::Second((s, on)) => pins.set_out(&s, on),
        }
    }
}
