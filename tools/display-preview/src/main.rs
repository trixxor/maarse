//! Renders every OLED page of the Heltec firmware with sample data, using
//! the firmware's own drawing code, and writes them as PBM images
//! (`display-preview <out dir>`; tools/display_preview.py turns them into
//! one PNG sheet).

#[path = "../../../firmware/esp/src/art.rs"]
#[allow(dead_code)]
mod art;
#[path = "../../../firmware/esp/src/display_ui.rs"]
#[allow(dead_code)]
mod display_ui;

use display_ui as ui;
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use std::io::Write;

struct Screen {
    px: [[bool; 128]; 64],
}

impl OriginDimensions for Screen {
    fn size(&self) -> Size {
        Size::new(128, 64)
    }
}

impl DrawTarget for Screen {
    type Color = BinaryColor;
    type Error = core::convert::Infallible;
    fn draw_iter<I: IntoIterator<Item = Pixel<BinaryColor>>>(&mut self, pixels: I) -> Result<(), Self::Error> {
        for Pixel(p, c) in pixels {
            if (0..128).contains(&p.x) && (0..64).contains(&p.y) {
                self.px[p.y as usize][p.x as usize] = c == BinaryColor::On;
            }
        }
        Ok(())
    }
}

fn save(s: &Screen, path: &std::path::Path) {
    let mut f = std::fs::File::create(path).unwrap();
    writeln!(f, "P1\n128 64").unwrap();
    for row in s.px.iter() {
        let line: Vec<&str> = row.iter().map(|b| if *b { "1" } else { "0" }).collect();
        writeln!(f, "{}", line.join(" ")).unwrap();
    }
}

fn s<const N: usize>(t: &str) -> heapless::String<N> {
    let mut out = heapless::String::new();
    let _ = out.push_str(t);
    out
}

fn sample() -> ui::View {
    let mut v = ui::View {
        my_id: 0xb0029bf6,
        modem_host: 0x256665f1,
        role_setting: 4,
        role_now: 0,
        nodes: 7,
        msg_count: 23,
        freq_hz: 869_525_000,
        bw_hz: 125_000,
        sf: 9,
        cr: 5,
        tx_dbm: 10,
        duty_limit_permille: 100,
        duty_used_permille: 53,
        rssi: -87,
        snr: 9,
        rx: 216,
        tx: 231,
        uptime_s: 3 * 3600 + 25 * 60,
        battery_mv: 3920,
        usb: true,
        bt_on: true,
        bt_connected: true,
        wifi_supported: true,
        wifi_linked: false,
        heap_free: 151_000,
        crashes: 0,
        screen: "2 min",
        version: "0.1.0",
        ..Default::default()
    };
    v.name = s("Dennis bil");
    let _ = v.recent.push((0x3eb445ec, s("Hej! Så kom billedet igennem"), 42));
    let _ = v.recent.push((0x0139feef, s("Er der nogen på public?"), 600));
    let _ = v.recent.push((0x3eb445ec, s("Det kører eller hvad?"), 7200));
    let _ = v.neighbours.push((0x3eb445ec, -71, 11, 4));
    let _ = v.neighbours.push((0x0139feef, -96, 3, 40));
    let _ = v.neighbours.push((0x43916f06, -112, -6, 300));
    v
}

fn main() {
    let out = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "display-preview".into()));
    std::fs::create_dir_all(&out).unwrap();
    let mut scr = Screen { px: [[false; 128]; 64] };
    let base = sample();

    ui::draw_boot(&mut scr, &base);
    save(&scr, &out.join("00-boot.pbm"));

    let names = ["home", "messages", "neighbours", "radio", "system", "settings"];
    for page in 0..ui::PAGES {
        let v = ui::View { page, ..base.clone() };
        ui::draw(&mut scr, &v);
        save(&scr, &out.join(format!("{:02}-{}.pbm", page + 1, names[page as usize])));
    }
    // Variants: own node (not a modem), settings with a confirm, a notice.
    let mut v = ui::View { page: ui::PAGE_HOME, modem_host: 0, ..base.clone() };
    v.name.clear();
    ui::draw(&mut scr, &v);
    save(&scr, &out.join("10-home-own-node.pbm"));
    let v = ui::View { page: ui::PAGE_SETTINGS, sel: ui::SET_RESTART, confirm: Some(ui::SET_RESTART), ..base.clone() };
    ui::draw(&mut scr, &v);
    save(&scr, &out.join("11-settings-confirm.pbm"));
    let mut v = ui::View { page: ui::PAGE_SETTINGS, sel: ui::SET_ROLE, ..base.clone() };
    v.toast = Some(s("Role: repeater"));
    ui::draw(&mut scr, &v);
    save(&scr, &out.join("12-settings-toast.pbm"));
    println!("written to {}", out.display());
}
