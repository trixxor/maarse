//! Drawing the OLED pages (128x64, one colour). Pure: takes a [`View`]
//! snapshot and draws it, so the pages can also be rendered on a PC
//! (tools/display_preview) to look at them without a board.

use core::fmt::Write as _;
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::image::{Image, ImageRaw};
use embedded_graphics::mono_font::iso_8859_1::{FONT_5X8, FONT_6X10, FONT_6X13_BOLD};
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Line, PrimitiveStyle, Rectangle, RoundedRectangle};

use crate::art;
use embedded_graphics::text::{Alignment, Baseline, Text, TextStyleBuilder};

pub const W: i32 = 128;

/// The pages, in the order the button steps through them.
pub const PAGE_HOME: u8 = 0;
pub const PAGE_MESSAGES: u8 = 1;
pub const PAGE_NEIGHBOURS: u8 = 2;
pub const PAGE_RADIO: u8 = 3;
pub const PAGE_SYSTEM: u8 = 4;
pub const PAGE_SETTINGS: u8 = 5;
pub const PAGES: u8 = 6;

/// Settings page items.
pub const SET_ROLE: u8 = 0;
pub const SET_RADIO: u8 = 1;
pub const SET_ANNOUNCE: u8 = 2;
pub const SET_SCREEN: u8 = 3;
pub const SET_RESTART: u8 = 4;
pub const SET_ITEMS: u8 = 5;

pub type Text64 = heapless::String<64>;

/// Everything a page shows, read from the node by the display task.
#[derive(Default, Clone)]
pub struct View {
    pub page: u8,
    /// Settings: the highlighted item.
    pub sel: u8,
    /// Settings: the item waiting for a second hold to confirm.
    pub confirm: Option<u8>,
    pub my_id: u32,
    /// The host's ID when we are its LoRa modem (u32::MAX = unknown, 0 = not a modem).
    pub modem_host: u32,
    pub name: heapless::String<24>,
    /// Role setting (0 client, 1 repeater, 2 relay, 3 anchor, 4 auto) and the role in use.
    pub role_setting: u8,
    pub role_now: u8,
    pub nodes: u32,
    pub msg_count: u32,
    /// Newest first: (sender, text, seconds ago).
    pub recent: heapless::Vec<(u32, Text64, u32), 3>,
    /// (id, RSSI dBm, SNR dB, seconds ago).
    pub neighbours: heapless::Vec<(u32, i16, i8, u32), 4>,
    pub freq_hz: u32,
    pub bw_hz: u32,
    pub sf: u8,
    pub cr: u8,
    pub tx_dbm: i8,
    pub duty_limit_permille: u32,
    pub duty_used_permille: u32,
    pub rssi: i32,
    pub snr: i32,
    pub rx: u32,
    pub tx: u32,
    pub uptime_s: u32,
    pub battery_mv: u32,
    pub usb: bool,
    pub bt_on: bool,
    pub bt_connected: bool,
    pub wifi_supported: bool,
    pub wifi_linked: bool,
    pub wifi_ip: u32,
    pub heap_free: u32,
    pub crashes: u32,
    /// Screen timeout label for the settings page.
    pub screen: &'static str,
    /// A short notice over the page (role changed, announce sent ...).
    pub toast: Option<heapless::String<32>>,
    pub version: &'static str,
}

fn style(font: &'static embedded_graphics::mono_font::MonoFont<'static>, on: bool) -> MonoTextStyle<'static, BinaryColor> {
    MonoTextStyle::new(font, if on { BinaryColor::On } else { BinaryColor::Off })
}

fn text<D: DrawTarget<Color = BinaryColor>>(d: &mut D, s: &str, x: i32, y: i32, font: &'static embedded_graphics::mono_font::MonoFont<'static>) {
    let _ = Text::with_baseline(s, Point::new(x, y), style(font, true), Baseline::Top).draw(d);
}

fn text_inv<D: DrawTarget<Color = BinaryColor>>(d: &mut D, s: &str, x: i32, y: i32, font: &'static embedded_graphics::mono_font::MonoFont<'static>) {
    let _ = Text::with_baseline(s, Point::new(x, y), style(font, false), Baseline::Top).draw(d);
}

fn text_center<D: DrawTarget<Color = BinaryColor>>(d: &mut D, s: &str, y: i32, font: &'static embedded_graphics::mono_font::MonoFont<'static>) {
    let ts = TextStyleBuilder::new().alignment(Alignment::Center).baseline(Baseline::Top).build();
    let _ = Text::with_text_style(s, Point::new(W / 2, y), style(font, true), ts).draw(d);
}

fn text_right<D: DrawTarget<Color = BinaryColor>>(d: &mut D, s: &str, x: i32, y: i32, font: &'static embedded_graphics::mono_font::MonoFont<'static>, on: bool) {
    let ts = TextStyleBuilder::new().alignment(Alignment::Right).baseline(Baseline::Top).build();
    let _ = Text::with_text_style(s, Point::new(x, y), style(font, on), ts).draw(d);
}

fn fill<D: DrawTarget<Color = BinaryColor>>(d: &mut D, x: i32, y: i32, w: u32, h: u32, on: bool) {
    let c = if on { BinaryColor::On } else { BinaryColor::Off };
    let _ = Rectangle::new(Point::new(x, y), Size::new(w, h)).into_styled(PrimitiveStyle::with_fill(c)).draw(d);
}

fn frame<D: DrawTarget<Color = BinaryColor>>(d: &mut D, x: i32, y: i32, w: u32, h: u32) {
    let _ = Rectangle::new(Point::new(x, y), Size::new(w, h)).into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1)).draw(d);
}

/// Cuts `s` to `n` characters (not bytes: names and messages may hold æøå).
fn cut(s: &str, n: usize) -> Text64 {
    let mut out = Text64::new();
    for c in s.chars().take(n) {
        let _ = out.push(c);
    }
    out
}

fn ago(secs: u32) -> heapless::String<8> {
    let mut s = heapless::String::new();
    let _ = if secs < 60 {
        write!(s, "{secs}s")
    } else if secs < 3600 {
        write!(s, "{}m", secs / 60)
    } else if secs < 86_400 {
        write!(s, "{}h", secs / 3600)
    } else {
        write!(s, "{}d", secs / 86_400)
    };
    s
}

pub fn role_name(r: u8) -> &'static str {
    match r {
        0 => "client",
        1 => "repeater",
        2 => "relay",
        3 => "anchor",
        4 => "auto",
        _ => "?",
    }
}

/// The boot screen: mast, lightning and waves, "MÅ:RSE", "HELTEC V4 /
/// LORA MESH" (a picture from tools/make_display_art.py).
pub fn draw_boot<D: DrawTarget<Color = BinaryColor>>(d: &mut D, _v: &View) {
    let _ = d.clear(BinaryColor::Off);
    let raw = ImageRaw::<BinaryColor>::new(&art::BOOT, 128);
    let _ = Image::new(&raw, Point::zero()).draw(d);
}

/// The ID in big heavy digits (generated), centred at `y`.
fn big_hex<D: DrawTarget<Color = BinaryColor>>(d: &mut D, id: u32, y: i32) {
    let w = art::HEX_W as i32;
    let x0 = (W - 8 * w) / 2;
    for i in 0..8 {
        let digit = ((id >> (28 - 4 * i)) & 0xF) as usize;
        let bytes = &art::HEX[digit * art::HEX_BYTES..(digit + 1) * art::HEX_BYTES];
        let raw = ImageRaw::<BinaryColor>::new(bytes, art::HEX_W);
        let _ = Image::new(&raw, Point::new(x0 + i * w, y)).draw(d);
    }
}

/// Header: the mast icon, the title in heavy capitals, link tags and the
/// battery on the right, a thin line under it.
fn header<D: DrawTarget<Color = BinaryColor>>(d: &mut D, title: &str, v: &View) {
    let icon = ImageRaw::<BinaryColor>::new(&art::ICON, art::ICON_W);
    let _ = Image::new(&icon, Point::new(0, 1)).draw(d);
    let mut caps: heapless::String<24> = heapless::String::new();
    for c in title.chars() {
        for u in c.to_uppercase() {
            let _ = caps.push(u);
        }
    }
    let _ = Text::with_baseline(&caps, Point::new(art::ICON_W as i32 + 3, -1), style(&FONT_6X13_BOLD, true), Baseline::Top).draw(d);
    let mut tags: heapless::String<16> = heapless::String::new();
    if v.wifi_linked {
        let _ = tags.push_str("WiFi ");
    }
    if v.bt_connected {
        let _ = tags.push_str("BT ");
    }
    if v.usb {
        let _ = tags.push_str("USB");
    }
    let tags = tags.trim_end();
    text_right(d, tags, W - 1, 2, &FONT_5X8, true);
    if v.battery_mv > 0 {
        let x = W - 1 - (tags.chars().count() as i32 * 5) - 14;
        let pct = battery_pct(v.battery_mv);
        frame(d, x, 2, 10, 7);
        fill(d, x + 10, 4, 1, 3, true);
        fill(d, x + 2, 4, (6 * pct / 100).max(1), 3, true);
    }
    let _ = Line::new(Point::new(0, 12), Point::new(W - 1, 12)).into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1)).draw(d);
}

fn battery_pct(mv: u32) -> u32 {
    // Li-ion: 3.3 V empty .. 4.15 V full.
    (mv.saturating_sub(3300) * 100 / 850).min(100)
}

/// Page dots at the bottom.
fn dots<D: DrawTarget<Color = BinaryColor>>(d: &mut D, page: u8) {
    let n = PAGES as i32;
    let x0 = W / 2 - (n * 6) / 2;
    for i in 0..n {
        let x = x0 + i * 6;
        if i == page as i32 {
            fill(d, x, 61, 4, 3, true);
        } else {
            fill(d, x + 1, 62, 2, 1, true);
        }
    }
}

/// Signal bars for an RSSI (4 bars: > -80, > -95, > -110, > -125 dBm).
fn bars<D: DrawTarget<Color = BinaryColor>>(d: &mut D, x: i32, y: i32, rssi: i16) {
    let n = [-80, -95, -110, -125].iter().filter(|t| rssi > **t).count() as i32;
    for i in 0..4 {
        let h = 2 + i * 2;
        if i < n {
            fill(d, x + i * 3, y + 8 - h, 2, h as u32, true);
        } else {
            fill(d, x + i * 3, y + 7, 2, 1, true);
        }
    }
}

pub fn draw<D: DrawTarget<Color = BinaryColor>>(d: &mut D, v: &View) {
    let _ = d.clear(BinaryColor::Off);
    match v.page {
        PAGE_HOME => home(d, v),
        PAGE_MESSAGES => messages(d, v),
        PAGE_NEIGHBOURS => neighbours(d, v),
        PAGE_RADIO => radio(d, v),
        PAGE_SYSTEM => system(d, v),
        _ => settings(d, v),
    }
    dots(d, v.page);
    if let Some(t) = &v.toast {
        // A rounded box over the middle of the page.
        let w = (t.chars().count() as u32 * 6 + 12).min(W as u32 - 4);
        let x = (W - w as i32) / 2;
        let rr = RoundedRectangle::with_equal_corners(Rectangle::new(Point::new(x, 22), Size::new(w, 18)), Size::new(4, 4));
        let _ = rr.into_styled(PrimitiveStyle::with_fill(BinaryColor::Off)).draw(d);
        let _ = rr.into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1)).draw(d);
        text_center(d, t, 24, &FONT_6X13_BOLD);
    }
}

fn home<D: DrawTarget<Color = BinaryColor>>(d: &mut D, v: &View) {
    header(d, "Må:rse", v);
    let (label, id) = match v.modem_host {
        0 => ("My ID", v.my_id),
        u32::MAX => ("Modem for a host", 0),
        h => ("Phone's ID (modem)", h),
    };
    text(d, label, 1, 14, &FONT_5X8);
    if v.modem_host == u32::MAX {
        text_center(d, "--------", 25, &FONT_6X13_BOLD);
    } else {
        big_hex(d, id, 23);
    }
    // Name, or the role.
    let mut line: heapless::String<32> = heapless::String::new();
    if !v.name.is_empty() {
        let _ = write!(line, "~{}", cut(&v.name, 14));
    } else if v.modem_host == 0 {
        let _ = write!(line, "Role: {}", role_name(v.role_now));
    } else {
        let _ = line.push_str("The phone routes");
    }
    text(d, &line, 1, 43, &FONT_5X8);
    let mut c: heapless::String<24> = heapless::String::new();
    let _ = write!(c, "{} nodes  {} msgs", v.nodes, v.msg_count);
    text(d, &c, 1, 52, &FONT_5X8);
}

fn messages<D: DrawTarget<Color = BinaryColor>>(d: &mut D, v: &View) {
    header(d, "Messages", v);
    if v.recent.is_empty() {
        text_center(d, "No messages yet", 30, &FONT_6X10);
        return;
    }
    for (i, (from, t, secs)) in v.recent.iter().enumerate() {
        let y = 14 + i as i32 * 15;
        let mut h: heapless::String<24> = heapless::String::new();
        let _ = write!(h, "{from:08x}");
        text(d, &h, 1, y, &FONT_5X8);
        text_right(d, &ago(*secs), W - 1, y, &FONT_5X8, true);
        let _ = Line::new(Point::new(44, y + 4), Point::new(W - 22, y + 4)).into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1)).draw(d);
        text(d, &cut(t, 25), 1, y + 7, &FONT_5X8);
    }
}

fn neighbours<D: DrawTarget<Color = BinaryColor>>(d: &mut D, v: &View) {
    let mut title: heapless::String<20> = heapless::String::new();
    let _ = write!(title, "Nearby ({})", v.neighbours.len());
    header(d, &title, v);
    if v.neighbours.is_empty() {
        text_center(d, "No one heard yet", 30, &FONT_6X10);
        return;
    }
    for (i, (id, rssi, snr, secs)) in v.neighbours.iter().enumerate() {
        let y = 14 + i as i32 * 12;
        bars(d, 1, y, *rssi);
        let mut s: heapless::String<32> = heapless::String::new();
        let _ = write!(s, "{id:08x}");
        text(d, &s, 16, y, &FONT_6X10);
        let mut m: heapless::String<20> = heapless::String::new();
        if *rssi != 0 {
            let _ = write!(m, "{rssi} {snr:+}");
        } else {
            let _ = write!(m, "snr {snr:+}");
        }
        text(d, &m, 68, y + 1, &FONT_5X8);
        text_right(d, &ago(*secs), W - 1, y + 1, &FONT_5X8, true);
    }
}

fn radio<D: DrawTarget<Color = BinaryColor>>(d: &mut D, v: &View) {
    header(d, "Radio", v);
    let mut s: heapless::String<32> = heapless::String::new();
    let _ = write!(s, "{}.{:03} MHz", v.freq_hz / 1_000_000, (v.freq_hz / 1000) % 1000);
    text(d, &s, 1, 14, &FONT_6X13_BOLD);
    s.clear();
    let _ = write!(s, "SF{}", v.sf);
    text_right(d, &s, W - 1, 14, &FONT_6X13_BOLD, true);
    s.clear();
    let _ = write!(s, "BW {}k  CR 4/{}  {} dBm", v.bw_hz / 1000, v.cr, v.tx_dbm);
    text(d, &s, 1, 28, &FONT_5X8);
    s.clear();
    let _ = write!(s, "Last {} dBm  SNR {:+}", v.rssi, v.snr);
    text(d, &s, 1, 36, &FONT_5X8);
    s.clear();
    let _ = write!(s, "In {}  Out {}", v.rx, v.tx);
    text(d, &s, 1, 44, &FONT_5X8);
    // Duty cycle used of the limit, as a bar.
    text(d, "Duty", 1, 52, &FONT_5X8);
    let lim = v.duty_limit_permille.max(1);
    let frac = (v.duty_used_permille.min(lim) * 60 / lim) as u32;
    frame(d, 26, 52, 62, 7);
    fill(d, 27, 53, frac, 5, true);
    s.clear();
    let _ = write!(s, "{}.{}/{}%", v.duty_used_permille / 10, v.duty_used_permille % 10, lim / 10);
    text_right(d, &s, W - 1, 52, &FONT_5X8, true);
}

fn system<D: DrawTarget<Color = BinaryColor>>(d: &mut D, v: &View) {
    header(d, "System", v);
    let up = v.uptime_s;
    let mut s: heapless::String<32> = heapless::String::new();
    let _ = write!(s, "Up {}d {:02}:{:02}", up / 86_400, (up / 3600) % 24, (up / 60) % 60);
    text(d, &s, 1, 15, &FONT_5X8);
    s.clear();
    if v.battery_mv > 0 {
        let _ = write!(s, "Battery {}.{:02} V ({}%)", v.battery_mv / 1000, (v.battery_mv % 1000) / 10, battery_pct(v.battery_mv));
    } else {
        let _ = s.push_str("Battery: not measured");
    }
    text(d, &s, 2, 22, &FONT_5X8);
    s.clear();
    let _ = write!(s, "Links: {}{}{}", if v.usb { "USB " } else { "" }, if v.bt_connected { "BT " } else if v.bt_on { "(BT) " } else { "" }, if v.wifi_linked { "WiFi" } else { "" });
    text(d, &s, 2, 31, &FONT_5X8);
    s.clear();
    if v.wifi_ip != 0 {
        let ip = v.wifi_ip.to_be_bytes();
        let _ = write!(s, "IP {}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
    } else {
        let _ = write!(s, "Heap {} kB free", v.heap_free / 1024);
    }
    text(d, &s, 2, 40, &FONT_5X8);
    s.clear();
    let _ = write!(s, "v{}  crashes {}", v.version, v.crashes);
    text(d, &s, 2, 49, &FONT_5X8);
}

fn settings<D: DrawTarget<Color = BinaryColor>>(d: &mut D, v: &View) {
    header(d, "Settings", v);
    let radio = if v.wifi_supported { if v.bt_on { "Bluetooth" } else { "Wi-Fi" } } else { "Bluetooth" };
    let items: [(&str, &str); 5] = [
        ("Role", role_name(v.role_setting)),
        ("Radio", radio),
        ("Announce", "send now"),
        ("Screen", v.screen),
        ("Restart", ""),
    ];
    for (i, (k, val)) in items.iter().enumerate() {
        let y = 14 + i as i32 * 9;
        let on = v.sel as usize == i;
        if on {
            fill(d, 0, y, W as u32, 9, true);
        }
        let draw = |d: &mut D, s: &str, x: i32| {
            if on { text_inv(d, s, x, y + 1, &FONT_5X8) } else { text(d, s, x, y + 1, &FONT_5X8) }
        };
        draw(d, if on { ">" } else { " " }, 1);
        draw(d, k, 8);
        let shown = if v.confirm == Some(i as u8) { "hold to confirm" } else { val };
        text_right(d, shown, W - 2, y + 1, &FONT_5X8, !on);
    }
}
