//! Sensor hardware on the ESP32-S3 boards, and deep sleep for sensor nodes.
//!
//! Heltec WiFi LoRa 32 V4: the pins that are free on the headers (the radio,
//! the PA, the OLED, the battery and the buttons use the rest):
//!
//! | pin id    | use                                                     |
//! |-----------|---------------------------------------------------------|
//! | 3, 6      | digital in/out                                          |
//! | 4         | digital in/out, or the analogue input (ADC1 channel 3)  |
//! | 38-41     | digital in/out (the GNSS header's pins; free without GNSS) |
//! | 47, 48    | I2C bus 1: SDA = 47, SCL = 48 (the OLED has its own bus) |
//!
//! Power a sensor from "Ve" (switched by Vext, off while asleep) or from 3V3.
//! The XIAO ESP32-S3 has no free pins here: it only reads the builtin values.
//!
//! **Deep sleep.** A node that sleeps switches the PA, the front end and Vext
//! off, the radio chip goes to sleep, and the chip deep sleeps until the next
//! cycle. The wake restarts the firmware; the node picks up what it needs from
//! flash (see node/sensor.rs), and the clock goes on in the RTC timer. Outputs
//! do not keep their level through a deep sleep (v1): do not use an output
//! that must stay on while the node sleeps.

use core::sync::atomic::Ordering::Relaxed;
use embassy_futures::select::{Either, select};
use embassy_time::Timer;
use esp_hal::Async;
use esp_hal::gpio::{Flex, InputConfig, Level, OutputConfig, Pull as HPull};
use esp_hal::i2c::master::I2c;
use maarse_core::sensor::{Input, Pull, Source};
use maarse_fw_common::bus::{ADC_REQ, ADC_USER_MV, RADIO_ASLEEP, SENSE_DONE, SENSE_REQ, SET_OUT, SLEEP_REQ};
use maarse_fw_common::power;

/// The digital pins of a Heltec V4, by pin id (the GPIO number).
pub const DIGITAL: [u8; 7] = [3, 4, 6, 38, 39, 40, 41];

pub struct Pins {
    pub gpio: [Option<Flex<'static>>; 7],
    pub i2c: Option<I2c<'static, Async>>,
}

fn pull(p: Pull) -> HPull {
    match p {
        Pull::None => HPull::None,
        Pull::Up => HPull::Up,
        Pull::Down => HPull::Down,
    }
}

impl Pins {
    fn pin(&mut self, id: u8) -> Option<&mut Flex<'static>> {
        let i = DIGITAL.iter().position(|d| *d == id)?;
        self.gpio[i].as_mut()
    }

    async fn read(&mut self, s: &Source, bme: &mut Option<(i32, u32, Option<u32>)>) -> Option<i32> {
        match s.input {
            Input::Battery => Some(power::BATTERY_MV.load(Relaxed) as i32).filter(|v| *v > 0),
            Input::ChipTemp => power::temperature_dc().map(|t| t as i32 * 10),
            Input::GpioIn { pin, pull: p, invert, .. } => {
                let f = self.pin(pin)?;
                f.set_output_enable(false);
                f.apply_input_config(&InputConfig::default().with_pull(pull(p)));
                f.set_input_enable(true);
                Timer::after_micros(300).await;
                Some((f.is_high() ^ invert) as i32)
            }
            Input::Adc { pin: 4, mul16 } => {
                ADC_USER_MV.store(-1, Relaxed);
                ADC_REQ.signal(());
                for _ in 0..40 {
                    Timer::after_millis(10).await;
                    let mv = ADC_USER_MV.load(Relaxed);
                    if mv >= 0 {
                        return Some(mv * mul16 as i32 / 16);
                    }
                }
                None
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
            Input::GpioOut { .. } | Input::Adc { .. } | Input::Counter { .. } => None,
        }
    }

    fn set_out(&mut self, s: &Source, on: bool) {
        if let Input::GpioOut { pin, active_low } = s.input
            && let Some(f) = self.pin(pin)
        {
            f.apply_output_config(&OutputConfig::default());
            f.set_output_enable(true);
            f.set_level(if on ^ active_low { Level::High } else { Level::Low });
        }
    }
}

#[embassy_executor::task]
pub async fn sensor_task(mut pins: Pins) {
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

/// Waits for the node's request to sleep, then switches everything off and
/// deep sleeps. The wake resets the chip.
#[embassy_executor::task]
pub async fn sleep_task(lpwr: esp_hal::peripherals::LPWR<'static>) {
    let secs = SLEEP_REQ.wait().await;
    // The radio loop puts the SX1262 to sleep; give it a moment.
    let _ = select(RADIO_ASLEEP.wait(), Timer::after_secs(3)).await;
    #[cfg(feature = "board-heltec-v4")]
    {
        use esp_hal::gpio::Output;
        // SAFETY: the pins were handed to code that leaked their drivers at
        // boot; nothing else uses them any more, and the chip sleeps next.
        unsafe {
            // PA power and chip enable of the front end off, battery divider off, Vext off.
            core::mem::forget(Output::new(esp_hal::peripherals::GPIO7::steal(), Level::Low, OutputConfig::default()));
            core::mem::forget(Output::new(esp_hal::peripherals::GPIO2::steal(), Level::Low, OutputConfig::default()));
            core::mem::forget(Output::new(esp_hal::peripherals::GPIO37::steal(), Level::Low, OutputConfig::default()));
            core::mem::forget(Output::new(esp_hal::peripherals::GPIO36::steal(), Level::High, OutputConfig::default()));
        }
    }
    // Wake pins (a reed switch, a button): the pin holds its level through the
    // sleep with its pull, and the opposite level wakes the chip. A pin that is
    // already at its wake level is not armed (it would wake the chip at once, every
    // time); the next timer wake looks again.
    let wake = maarse_fw_common::bus::WAKE_PINS.lock(|w| w.borrow().clone());
    for (pin, high) in wake {
        // SAFETY: the pin belongs to the sensor task, which is idle now; the chip sleeps next.
        let mut f = esp_hal::gpio::Flex::new(unsafe { esp_hal::gpio::AnyPin::steal(pin) });
        f.set_output_enable(false);
        f.apply_input_config(&InputConfig::default().with_pull(if high { HPull::Down } else { HPull::Up }));
        f.set_input_enable(true);
        Timer::after_micros(300).await;
        if f.is_high() != high {
            let _ = f.apply_wakeup_config(&esp_hal::gpio::WakeupConfig::default().with_low_power_path(true));
            f.listen(if high { esp_hal::gpio::Event::HighLevel } else { esp_hal::gpio::Event::LowLevel });
        }
        core::mem::forget(f);
    }
    let mut lp = esp_hal::rtc_cntl::sleep::LowPower::new(lpwr);
    lp.set_wakeup_deadline(esp_hal::time::Instant::now() + esp_hal::time::Duration::from_secs(secs as u64));
    lp.sleep_deep(esp_hal::rtc_cntl::sleep::RtcSleepConfig::deep())
}
