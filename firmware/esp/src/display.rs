//! OLED display (128×64, SSD1306/SSD1315) on the Heltec V4.
//!
//! The button (PRG/GPIO0) switches between three pages:
//! 1. overview: address, known nodes, messages, connections (USB, BT,
//!    Wi-Fi with IP) and the latest message
//! 2. radio: RSSI/SNR, packets in/out, errors and duty cycle
//! 3. off (saves power and the screen)
//!
//! Holding the button for 3 s switches between Bluetooth and Wi-Fi (only one
//! runs at a time); the node saves the choice and restarts.

use core::fmt::Write as _;
use core::sync::atomic::Ordering::Relaxed;
use embassy_futures::select::{Either, select};
use embassy_time::{Duration, Timer};
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::mono_font::ascii::FONT_6X10;
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::text::{Baseline, Text};
use esp_hal::Blocking;
use esp_hal::gpio::Input;
use esp_hal::i2c::master::I2c;
use ssd1306::prelude::*;
use ssd1306::{I2CDisplayInterface, Ssd1306};
use maarse_fw_common::bus::{
    BLE_ACTIVE, BLE_CONNECTED, DISPLAY_NOTE, KNOWN_NODES, LAST_MSG, MODEM_HOST_SHORT, MSG_COUNT, MY_SHORT, NOTE_ONE_RADIO,
    NOTE_TO_BLE, NOTE_TO_WIFI, NOTE_WIFI_NOT_SET_UP, RSTATS, TOGGLE_RADIO, USB_LAST_RX, WIFI_IP, WIFI_STATE,
    WIFI_SUPPORTED,
};
use maarse_core::companion::wifi_state;

/// How long the button must be held to switch radio.
const LONG_PRESS: Duration = Duration::from_secs(3);
/// A USB host counts as connected this long after its last frame (s).
const USB_FRESH_S: u32 = 30;

type Line = heapless::String<22>;

#[embassy_executor::task]
pub async fn display_task(i2c: I2c<'static, Blocking>, mut button: Input<'static>) {
    let iface = I2CDisplayInterface::new(i2c);
    let mut d = Ssd1306::new(iface, DisplaySize128x64, DisplayRotation::Rotate0)
        .into_buffered_graphics_mode();
    if d.init().is_err() {
        return; // no display: the node carries on without it
    }
    let style = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
    let mut page = 0u8;
    loop {
        let mut lines: [Line; 6] = Default::default();
        match page {
            0 => overview(&mut lines),
            1 => radio(&mut lines),
            _ => {}
        }
        d.clear(BinaryColor::Off).ok();
        for (i, l) in lines.iter().enumerate() {
            Text::with_baseline(l, Point::new(0, i as i32 * 10 + 2), style, Baseline::Top)
                .draw(&mut d)
                .ok();
        }
        d.flush().ok();
        let _ = d.set_display_on(page < 2);

        match select(Timer::after(Duration::from_secs(1)), button.wait_for_falling_edge()).await {
            Either::First(()) => {}
            Either::Second(()) => {
                Timer::after_millis(50).await; // prel
                match select(button.wait_for_rising_edge(), Timer::after(LONG_PRESS)).await {
                    Either::First(()) => page = (page + 1) % 3,
                    Either::Second(()) => {
                        // Long press: switch radio, show the overview with the note.
                        TOGGLE_RADIO.signal(());
                        page = 0;
                        button.wait_for_rising_edge().await;
                    }
                }
                Timer::after_millis(150).await; // prel
            }
        }
    }
}

fn overview(l: &mut [Line; 6]) {
    // The ID in use on air: the host's when we are its LoRa modem, else our own.
    let _ = match MODEM_HOST_SHORT.load(Relaxed) {
        0 => write!(l[0], "Maarse {:08x}", MY_SHORT.load(Relaxed)),
        u32::MAX => write!(l[0], "Modem (host ID ?)"),
        h => write!(l[0], "ID {:08x} modem", h),
    };
    let _ = write!(l[1], "Nodes {}  Msgs {}", KNOWN_NODES.load(Relaxed), MSG_COUNT.load(Relaxed));
    links(&mut l[2]);
    wifi_line(&mut l[3]);
    if !note_line(&mut l[4]) {
        LAST_MSG.lock(|m| {
            let m = m.borrow();
            if m.0 != 0 {
                let _ = write!(l[4], "From {:08x}:", m.0);
                for c in m.1.chars().take(21) {
                    let _ = l[5].push(if c.is_ascii() { c } else { '?' });
                }
            }
        });
    }
}

/// Which links are up right now: USB host, Bluetooth phone, Wi-Fi.
fn links(l: &mut Line) {
    let now = embassy_time::Instant::now().as_secs() as u32;
    let _ = l.push_str("Link:");
    let mut any = false;
    let usb = USB_LAST_RX.load(Relaxed);
    if usb != 0 && now.saturating_sub(usb) < USB_FRESH_S {
        let _ = l.push_str(" USB");
        any = true;
    }
    if BLE_CONNECTED.load(Relaxed) {
        let _ = l.push_str(" BT");
        any = true;
    } else if BLE_ACTIVE.load(Relaxed) {
        let _ = l.push_str(" (BT)");
        any = true;
    }
    if WIFI_STATE.load(Relaxed) == wifi_state::LINKED as u32 {
        let _ = l.push_str(" WiFi");
        any = true;
    }
    if !any {
        let _ = l.push_str(" none");
    }
}

/// Wi-Fi status with the IP address, when this firmware has Wi-Fi.
fn wifi_line(l: &mut Line) {
    if !WIFI_SUPPORTED.load(Relaxed) {
        return;
    }
    let ip = WIFI_IP.load(Relaxed).to_be_bytes();
    if maarse_fw_common::bus::MODEM_FALLBACK.load(Relaxed) {
        // A Wi-Fi modem whose link is down works as an ordinary node.
        let _ = write!(l, "no host: node");
        return;
    }
    let _ = match WIFI_STATE.load(Relaxed) as u8 {
        wifi_state::LINKED if maarse_fw_common::bus::WIFI_MODEM.load(Relaxed) => {
            write!(l, "{}.{}.{}.{} modem", ip[0], ip[1], ip[2], ip[3])
        }
        wifi_state::LINKED => write!(l, "{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]),
        wifi_state::ONLINE => write!(l, "{}.{}.{}.{} no host", ip[0], ip[1], ip[2], ip[3]),
        wifi_state::JOINING => write!(l, "WiFi joining..."),
        wifi_state::SKIPPED_AFTER_CRASH => write!(l, "WiFi off (crashed)"),
        wifi_state::BAD_SETTINGS | wifi_state::DRIVER_FAILED => write!(l, "WiFi failed"),
        _ if BLE_ACTIVE.load(Relaxed) => write!(l, "Hold btn 3s: WiFi"),
        _ => Ok(()),
    };
}

/// A short note after a long press. Returns `false` when there is none.
fn note_line(l: &mut Line) -> bool {
    let n = DISPLAY_NOTE.load(Relaxed);
    let now = embassy_time::Instant::now().as_secs() as u32;
    if n == 0 || now > (n & 0x00FF_FFFF) {
        return false;
    }
    let _ = l.push_str(match n >> 24 {
        NOTE_TO_WIFI => "Switching to WiFi...",
        NOTE_TO_BLE => "Switching to BT...",
        NOTE_WIFI_NOT_SET_UP => "WiFi not set up (USB)",
        NOTE_ONE_RADIO => "Only one radio here",
        _ => "",
    });
    true
}

fn radio(l: &mut [Line; 6]) {
    let s = &RSTATS;
    let _ = write!(
        l[0],
        "RSSI {} SNR {}",
        s.last_rssi.load(Relaxed),
        s.last_snr.load(Relaxed)
    );
    let _ = write!(
        l[1],
        "Avg {} / {}",
        s.avg_rssi_x16.load(Relaxed) / 16,
        s.avg_snr_x16.load(Relaxed) / 16
    );
    let _ = write!(l[2], "In {} err {}", s.rx_ok.load(Relaxed), s.rx_err.load(Relaxed));
    let _ = write!(l[3], "Out {} err {}", s.tx_ok.load(Relaxed), s.tx_err.load(Relaxed));
    let d = s.duty_used_permille.load(Relaxed);
    let _ = write!(l[4], "Duty {}.{}% q {}", d / 10, d % 10, s.tx_queue.load(Relaxed));
    let _ = write!(l[5], "Busy {} drop {}", s.cad_busy.load(Relaxed), s.dropped.load(Relaxed));
}
