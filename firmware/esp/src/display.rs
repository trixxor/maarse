//! OLED-display (128×64, SSD1306/SSD1315) på Heltec V4.
//!
//! Knappen (PRG/GPIO0) skifter mellem tre sider:
//! 1. oversigt: adresse, kendte noder, beskeder og den seneste besked
//! 2. radio: RSSI/SNR, pakker ind/ud, fejl og duty cycle
//! 3. slukket (sparer strøm og skærm)

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
use maarse_fw_common::bus::{KNOWN_NODES, LAST_MSG, MSG_COUNT, MY_SHORT, RSTATS};

type Line = heapless::String<22>;

#[embassy_executor::task]
pub async fn display_task(i2c: I2c<'static, Blocking>, mut button: Input<'static>) {
    let iface = I2CDisplayInterface::new(i2c);
    let mut d = Ssd1306::new(iface, DisplaySize128x64, DisplayRotation::Rotate0)
        .into_buffered_graphics_mode();
    if d.init().is_err() {
        return; // intet display: noden kører videre uden
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
                page = (page + 1) % 3;
                Timer::after_millis(150).await; // prel
            }
        }
    }
}

fn overview(l: &mut [Line; 6]) {
    let _ = write!(l[0], "Maarse {:08x}", MY_SHORT.load(Relaxed));
    let _ = write!(l[1], "Noder: {}", KNOWN_NODES.load(Relaxed));
    let _ = write!(l[2], "Beskeder: {}", MSG_COUNT.load(Relaxed));
    LAST_MSG.lock(|m| {
        let m = m.borrow();
        if m.0 != 0 {
            let _ = write!(l[3], "Fra {:08x}:", m.0);
            // Teksten fordeles på to linjer à 21 tegn.
            let mut chars = m.1.chars();
            let (a, b) = l.split_at_mut(5);
            for line in [&mut a[4], &mut b[0]] {
                for c in chars.by_ref().take(21) {
                    let c = if c.is_ascii() { c } else { '?' };
                    let _ = line.push(c);
                }
            }
        }
    });
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
        "Gns {} / {}",
        s.avg_rssi_x16.load(Relaxed) / 16,
        s.avg_snr_x16.load(Relaxed) / 16
    );
    let _ = write!(l[2], "Ind {} fejl {}", s.rx_ok.load(Relaxed), s.rx_err.load(Relaxed));
    let _ = write!(l[3], "Ud {} fejl {}", s.tx_ok.load(Relaxed), s.tx_err.load(Relaxed));
    let d = s.duty_used_permille.load(Relaxed);
    let _ = write!(l[4], "Duty {}.{}% ko {}", d / 10, d % 10, s.tx_queue.load(Relaxed));
    let _ = write!(l[5], "Optaget {} kass {}", s.cad_busy.load(Relaxed), s.dropped.load(Relaxed));
}
