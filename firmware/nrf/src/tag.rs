//! RAK WisMesh Tag: button, LEDs, buzzer, power off and GPS.
//!
//! Pins (RAK4630 module, same as MeshCore's `rak_wismesh_tag` variant):
//!
//! | what         | pin   | use                                                   |
//! |--------------|-------|-------------------------------------------------------|
//! | green LED    | P1.03 | lit while the radio transmits (`radio.rs`)            |
//! | blue LED     | P1.04 | status (this file); lit from the start until `ui_task` runs |
//! | button       | P0.09 | active low, pull-up (the NFC1 pin, used as GPIO)      |
//! | buzzer       | P0.21 | a passive piezo, driven by timed pin toggles          |
//! | GPS enable   | P1.02 | high = module on                                      |
//! | GPS UART     | P0.15 (RX), P0.16 (TX) | 9600 baud NMEA                        |
//! | I2C          | P0.25 (SDA), P0.24 (SCL) | not used: no sensor setups on the tag |
//!
//! Button: one click shows the battery level (1-4 blinks), a double click mutes or unmutes
//! the buzzer, holding it for 3 seconds powers the tag off (a press wakes it again).
//!
//! Blue LED: lit from power-up until the node is running, so a tag that hangs while
//! starting shows a steady light. Then one short blink every 4 seconds when running, two
//! when a phone is connected over Bluetooth, five fast ones when the radio does not answer,
//! one long one when the battery is low. Three quick blinks and two notes for a message.
//!
//! The buzzer sounds for a new message only when no phone or PC is attached (they make
//! their own sound), and not when muted.

use core::sync::atomic::{AtomicBool, Ordering::Relaxed};
use embassy_futures::select::{Either, select};
use embassy_nrf::gpio::{Input, Output};
use embassy_nrf::uarte::UarteRxWithIdle;
use embassy_time::{Duration, Instant, Timer, with_timeout};
use maarse_core::nmea::{Parser, Sentence};
use maarse_fw_common::{bus, gps, power};

/// The buzzer is muted (RAM only: it is on again after a restart).
pub static MUTED: AtomicBool = AtomicBool::new(false);
/// The SX1262 did not answer at start-up.
pub static RADIO_FAILED: AtomicBool = AtomicBool::new(false);

/// Below this the battery blinks as low (mV), unless external power is connected.
const LOW_BATTERY_MV: u32 = 3400;
/// Seconds between the status blinks.
const BEAT_S: u64 = 4;
/// Holding the button this long powers the tag off.
const LONG_PRESS_MS: u64 = 3000;
/// A second click within this time is a double click.
const DOUBLE_MS: u64 = 350;

struct Ui {
    blue: Output<'static>,
    buzzer: Output<'static>,
}

impl Ui {
    async fn blink(&mut self, on_ms: u64, off_ms: u64, times: u32) {
        for _ in 0..times {
            self.blue.set_high();
            Timer::after_millis(on_ms).await;
            self.blue.set_low();
            Timer::after_millis(off_ms).await;
        }
    }

    /// Plays a tone; `hz` 0 is a pause.
    async fn note(&mut self, hz: u32, ms: u64) {
        if hz == 0 {
            Timer::after_millis(ms).await;
            return;
        }
        // A square wave by timed toggles. The PWM block of the chip is not used: its driver
        // waits in a busy loop for the end of a sequence, which would block the whole node.
        let half = Duration::from_micros(500_000 / hz as u64);
        let end = Instant::now() + Duration::from_millis(ms);
        let mut next = Instant::now();
        while Instant::now() < end {
            self.buzzer.set_high();
            next += half;
            Timer::at(next).await;
            self.buzzer.set_low();
            next += half;
            Timer::at(next).await;
        }
        self.buzzer.set_low();
    }

    async fn melody(&mut self, notes: &[(u32, u64)]) {
        for &(hz, ms) in notes {
            self.note(hz, ms).await;
        }
    }

    fn sound_allowed(&self) -> bool {
        if MUTED.load(Relaxed) {
            return false;
        }
        // A phone or a PC is attached: it plays its own sound.
        let usb = bus::USB_LAST_RX.load(Relaxed);
        let now = Instant::now().as_secs() as u32;
        let usb_recent = usb != 0 && now.saturating_sub(usb) < 30;
        !(bus::BLE_CONNECTED.load(Relaxed) || usb_recent)
    }

    /// A beep asked for by the host.
    async fn beep(&mut self) {
        if MUTED.load(Relaxed) {
            self.blink(60, 80, 3).await;
            return;
        }
        self.blue.set_high();
        self.melody(&[(1760, 70), (0, 40), (2349, 110)]).await;
        self.blue.set_low();
    }

    async fn on_message(&mut self) {
        if self.sound_allowed() {
            self.blue.set_high();
            self.melody(&[(1760, 70), (0, 40), (2349, 110)]).await;
            self.blue.set_low();
            Timer::after_millis(60).await;
            self.blink(60, 80, 2).await;
        } else {
            self.blink(60, 80, 3).await;
        }
    }

    async fn heartbeat(&mut self) {
        let batt = power::BATTERY_MV.load(Relaxed);
        let ext = power::EXT_POWER.load(Relaxed) == 1;
        if RADIO_FAILED.load(Relaxed) {
            self.blink(50, 90, 5).await;
        } else if batt != 0 && batt < LOW_BATTERY_MV && !ext {
            self.blink(400, 0, 1).await;
        } else if bus::BLE_CONNECTED.load(Relaxed) {
            self.blink(40, 120, 2).await;
        } else {
            self.blink(30, 0, 1).await;
        }
    }

    /// One click: 1-4 blinks for the battery level.
    async fn show_battery(&mut self) {
        let mv = power::BATTERY_MV.load(Relaxed);
        let n = match mv {
            0 => 1,
            1..3500 => 1,
            3500..3700 => 2,
            3700..3900 => 3,
            _ => 4,
        };
        self.blink(250, 250, n).await;
    }

    async fn toggle_mute(&mut self) {
        let muted = !MUTED.load(Relaxed);
        MUTED.store(muted, Relaxed);
        if muted {
            self.blink(80, 80, 2).await;
        } else {
            self.blue.set_high();
            self.melody(&[(1500, 70), (2000, 90)]).await;
            self.blue.set_low();
        }
    }
}

/// Button, LED patterns and buzzer.
#[embassy_executor::task]
pub async fn ui_task(blue: Output<'static>, buzzer: Output<'static>, mut button: Input<'static>) {
    let mut ui = Ui { blue, buzzer };
    // Woken by a press: wait for the button to be released, or it would count as a long press.
    let _ = with_timeout(Duration::from_secs(5), button.wait_for_high()).await;
    ui.blue.set_low();
    if !MUTED.load(Relaxed) {
        ui.note(2200, 40).await;
    }
    let mut seen = bus::MSG_COUNT.load(Relaxed);
    let mut next_beat = Instant::now() + Duration::from_secs(BEAT_S);
    loop {
        match select(button.wait_for_low(), select(Timer::after_millis(250), bus::BEEP.wait())).await {
            Either::First(()) => handle_button(&mut ui, &mut button).await,
            // The host got a message (over LoRa or TCP) and asks for a beep: it plays even
            // with a phone or PC attached, because the host chose it (`--radio-quiet` stops it).
            Either::Second(Either::Second(())) => ui.beep().await,
            Either::Second(Either::First(())) => {
                let n = bus::MSG_COUNT.load(Relaxed);
                if n != seen {
                    seen = n;
                    ui.on_message().await;
                } else if Instant::now() >= next_beat {
                    ui.heartbeat().await;
                    next_beat = Instant::now() + Duration::from_secs(BEAT_S);
                }
            }
        }
    }
}

async fn handle_button(ui: &mut Ui, button: &mut Input<'static>) {
    Timer::after_millis(30).await; // contact bounce
    if button.is_high() {
        return;
    }
    if with_timeout(Duration::from_millis(LONG_PRESS_MS), button.wait_for_high()).await.is_err() {
        power_off(ui, button).await;
    }
    // Released: a second click soon after makes it a double click.
    if with_timeout(Duration::from_millis(DOUBLE_MS), button.wait_for_low()).await.is_ok() {
        let _ = with_timeout(Duration::from_secs(2), button.wait_for_high()).await;
        ui.toggle_mute().await;
    } else {
        ui.show_battery().await;
    }
}

/// Switches the tag off (System OFF, about 2 µA); the button wakes it.
async fn power_off(ui: &mut Ui, button: &mut Input<'static>) -> ! {
    ui.blue.set_high();
    ui.melody(&[(2000, 90), (1500, 90), (1000, 160)]).await;
    // The radio first: it must not be left transmitting or listening.
    bus::RADIO_SLEEP.signal(true);
    let _ = with_timeout(Duration::from_secs(1), bus::RADIO_ASLEEP.wait()).await;
    ui.blue.set_low();
    // Wait for the release, or the sense below would wake the chip at once.
    button.wait_for_high().await;
    Timer::after_millis(100).await;
    cortex_m::interrupt::disable();
    // SAFETY: plain register writes to the GPIO and POWER blocks, which always exist. The
    // GPS module and the LEDs go off (P1.02, P1.03, P1.04); the button pin (P0.09) gets a
    // pull-up and wakes the chip on a low level; then System OFF, which does not return.
    unsafe {
        const P1_OUTCLR: *mut u32 = 0x5000_080C as *mut u32;
        const P0_PIN_CNF9: *mut u32 = (0x5000_0700 + 4 * 9) as *mut u32;
        const POWER_SYSTEMOFF: *mut u32 = 0x4000_0500 as *mut u32;
        core::ptr::write_volatile(P1_OUTCLR, (1 << 2) | (1 << 3) | (1 << 4));
        // Input, connected, pull-up, sense low.
        core::ptr::write_volatile(P0_PIN_CNF9, (3 << 16) | (3 << 2));
        core::ptr::write_volatile(POWER_SYSTEMOFF, 1);
    }
    loop {
        cortex_m::asm::wfe();
    }
}

/// Powers the GPS module while the node shares a position, and keeps the latest fix.
#[embassy_executor::task]
pub async fn gps_task(mut enable: Output<'static>, mut rx: UarteRxWithIdle<'static>) {
    gps::HAS_MODULE.store(true, Relaxed);
    let mut parser = Parser::new();
    let mut buf = [0u8; 64];
    loop {
        if !gps::WANTED.load(Relaxed) {
            enable.set_low();
            Timer::after_secs(2).await;
            continue;
        }
        enable.set_high();
        gps::BYTES.store(0, Relaxed);
        gps::SATS.store(gps::NO_DATA, Relaxed);
        gps::SATS_VIEW.store(gps::NO_DATA, Relaxed);
        gps::clear();
        while gps::WANTED.load(Relaxed) {
            if let Ok(Ok(n)) = with_timeout(Duration::from_secs(2), rx.read_until_idle(&mut buf)).await {
                gps::BYTES.fetch_add(n as u32, Relaxed);
                for &b in &buf[..n] {
                    let mut next = parser.feed(b);
                    while let Some(sentence) = next {
                        let now = Instant::now().as_secs() as u32;
                        match sentence {
                            Sentence::Fix { lat_e7, lon_e7 } => gps::store_fix(lat_e7, lon_e7, now),
                            Sentence::Sats(n) => gps::SATS.store(n as u32, Relaxed),
                            Sentence::InView(n) => gps::SATS_VIEW.store(n as u32, Relaxed),
                            Sentence::Alt(dm) => gps::ALT_DM.store(dm, Relaxed),
                            Sentence::Hdop(h) => gps::HDOP_X10.store(h as u32, Relaxed),
                            Sentence::Motion { speed_cms, course } => {
                                gps::SPEED_CMS.store(speed_cms as u32, Relaxed);
                                gps::COURSE.store(course as u32, Relaxed);
                            }
                            Sentence::Utc(t) => gps::UTC.store(t, Relaxed),
                            Sentence::Sat { sys, prn, elevation, azimuth, snr } => gps::note_sat(sys, prn, elevation, azimuth, snr, now),
                            Sentence::Used { prns, n } => gps::note_used(&prns[..n as usize], now),
                            Sentence::NoFix => {}
                        }
                        next = parser.next();
                    }
                }
            }
        }
        enable.set_low();
    }
}
