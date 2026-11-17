//! OLED display (128×64, SSD1306/SSD1315) on the Heltec V4.
//!
//! A boot screen, then pages the button steps through (drawn by
//! `display_ui`): home (the ID in use, name, counts), messages,
//! neighbours, radio, system and settings.
//!
//! The button (PRG/GPIO0):
//! - short press: next page; on the settings page the next item
//!   (after the last item: back to the home page)
//! - hold about 1 s: on the settings page change the highlighted item
//!   (restart and radio switch ask for a second hold); elsewhere go to
//!   the settings page
//! - hold 3 s: switch between Bluetooth and Wi-Fi (as before)
//!
//! The screen goes dark after the chosen time without a press (the first
//! press only wakes it).

use core::fmt::Write as _;
use core::sync::atomic::Ordering::Relaxed;
use embassy_futures::select::{Either, select};
use embassy_time::{Duration, Instant, Timer};
use esp_hal::Blocking;
use esp_hal::gpio::Input;
use esp_hal::i2c::master::I2c;
use ssd1306::prelude::*;
use ssd1306::{I2CDisplayInterface, Ssd1306};
use maarse_fw_common::bus::{
    BLE_ACTIVE, BLE_CONNECTED, CRASHES, DCMD_ANNOUNCE, DCMD_ROLE, DISPLAY_CMD, DISPLAY_NOTE, HEAP_FREE, KNOWN_NODES,
    MODEM_HOST_SHORT, MSG_COUNT, MY_SHORT, NEIGHBORS_SHOW, NODE_NAME, NOTE_ONE_RADIO, NOTE_TO_BLE, NOTE_TO_WIFI,
    NOTE_WIFI_NOT_SET_UP, RADIO_BW, RADIO_FREQ, RADIO_MISC, RECENT_MSGS, RESTART, ROLE_SHOW, RSTATS, TOGGLE_RADIO,
    USB_LAST_RX, WIFI_IP, WIFI_STATE, WIFI_SUPPORTED,
};
use maarse_core::companion::wifi_state;

use crate::display_ui::{self as ui, View};

/// A press shorter than this is a short press.
const SHORT: Duration = Duration::from_millis(600);
/// Held this long: switch radio (Bluetooth / Wi-Fi).
const VERY_LONG: Duration = Duration::from_secs(3);
/// A USB host counts as connected this long after its last frame (s).
const USB_FRESH_S: u32 = 30;
/// How long the boot screen shows.
const BOOT_SCREEN: Duration = Duration::from_millis(2500);
/// Screen timeouts to choose from (0 = always on).
const SCREEN_TIMEOUTS: [(u32, &str); 4] = [(30, "30 s"), (120, "2 min"), (600, "10 min"), (0, "always on")];
/// Roles in the order a hold steps through them (values as in ROLE_SHOW).
const ROLE_CYCLE: [u8; 5] = [0, 1, 2, 3, 4];
/// A notice shows this long.
const TOAST: Duration = Duration::from_secs(2);
/// The second hold that confirms restart or radio switch must come within this.
const CONFIRM: Duration = Duration::from_secs(5);

enum Press {
    Short,
    Long,
    VeryLong,
}

/// Waits for the button (or `timeout`) and tells how long it was held.
async fn press(button: &mut Input<'static>, timeout: Duration) -> Option<Press> {
    if let Either::First(()) = select(Timer::after(timeout), button.wait_for_falling_edge()).await {
        return None;
    }
    Timer::after_millis(30).await; // debounce
    let p = match select(button.wait_for_rising_edge(), Timer::after(SHORT)).await {
        Either::First(()) => Press::Short,
        Either::Second(()) => match select(button.wait_for_rising_edge(), Timer::after(VERY_LONG - SHORT)).await {
            Either::First(()) => Press::Long,
            Either::Second(()) => Press::VeryLong,
        },
    };
    if matches!(p, Press::VeryLong) {
        button.wait_for_rising_edge().await;
    }
    Timer::after_millis(80).await; // debounce
    Some(p)
}

fn toast(t: &str) -> Option<(heapless::String<32>, Instant)> {
    let mut s = heapless::String::new();
    let _ = s.push_str(t);
    Some((s, Instant::now() + TOAST))
}

/// Reads what the pages show from the node.
fn snapshot(page: u8, sel: u8, confirm: Option<u8>, screen: &'static str) -> View {
    let now = Instant::now().as_secs() as u32;
    let usb = USB_LAST_RX.load(Relaxed);
    let misc = RADIO_MISC.load(Relaxed);
    let role = ROLE_SHOW.load(Relaxed);
    let mut v = View {
        page,
        sel,
        confirm,
        my_id: MY_SHORT.load(Relaxed),
        modem_host: MODEM_HOST_SHORT.load(Relaxed),
        role_setting: role as u8,
        role_now: (role >> 8) as u8,
        nodes: KNOWN_NODES.load(Relaxed),
        msg_count: MSG_COUNT.load(Relaxed),
        freq_hz: RADIO_FREQ.load(Relaxed),
        bw_hz: RADIO_BW.load(Relaxed),
        sf: misc as u8,
        cr: (misc >> 8) as u8,
        tx_dbm: (misc >> 16) as u8 as i8,
        duty_limit_permille: misc >> 24,
        duty_used_permille: RSTATS.duty_used_permille.load(Relaxed),
        rssi: RSTATS.last_rssi.load(Relaxed),
        snr: RSTATS.last_snr.load(Relaxed),
        rx: RSTATS.rx_ok.load(Relaxed),
        tx: RSTATS.tx_ok.load(Relaxed),
        uptime_s: now,
        battery_mv: maarse_fw_common::power::BATTERY_MV.load(Relaxed),
        usb: usb != 0 && now.saturating_sub(usb) < USB_FRESH_S,
        bt_on: BLE_ACTIVE.load(Relaxed),
        bt_connected: BLE_CONNECTED.load(Relaxed),
        wifi_supported: WIFI_SUPPORTED.load(Relaxed),
        wifi_linked: WIFI_STATE.load(Relaxed) == wifi_state::LINKED as u32,
        wifi_ip: WIFI_IP.load(Relaxed),
        heap_free: HEAP_FREE.load(Relaxed),
        crashes: CRASHES.load(Relaxed),
        screen,
        version: env!("CARGO_PKG_VERSION"),
        ..View::default()
    };
    NODE_NAME.lock(|n| v.name = n.borrow().clone());
    RECENT_MSGS.lock(|r| {
        for (from, t, at) in r.borrow().iter() {
            let _ = v.recent.push((*from, t.clone(), now.saturating_sub(*at)));
        }
    });
    NEIGHBORS_SHOW.lock(|n| v.neighbours = n.borrow().clone());
    v
}

/// A notice from the node (after a radio switch by a long hold).
fn node_note() -> Option<&'static str> {
    let n = DISPLAY_NOTE.load(Relaxed);
    let now = Instant::now().as_secs() as u32;
    if n == 0 || now > (n & 0x00FF_FFFF) {
        return None;
    }
    match n >> 24 {
        NOTE_TO_WIFI => Some("Switching to Wi-Fi"),
        NOTE_TO_BLE => Some("Switching to BT"),
        NOTE_WIFI_NOT_SET_UP => Some("Wi-Fi not set up"),
        NOTE_ONE_RADIO => Some("Only one radio here"),
        _ => None,
    }
}

#[embassy_executor::task]
pub async fn display_task(i2c: I2c<'static, Blocking>, mut button: Input<'static>) {
    let iface = I2CDisplayInterface::new(i2c);
    let mut d = Ssd1306::new(iface, DisplaySize128x64, DisplayRotation::Rotate0).into_buffered_graphics_mode();
    if d.init().is_err() {
        return; // no display: the node carries on without it
    }

    // Boot screen (a press skips it).
    ui::draw_boot(&mut d, &snapshot(0, 0, None, ""));
    d.flush().ok();
    let _ = select(Timer::after(BOOT_SCREEN), button.wait_for_falling_edge()).await;

    let mut page = ui::PAGE_HOME;
    let mut sel = 0u8;
    let mut screen_i = 1usize; // 2 min
    let mut last_input = Instant::now();
    let mut on = true;
    let mut toast_msg: Option<(heapless::String<32>, Instant)> = None;
    let mut confirm: Option<(u8, Instant)> = None;

    loop {
        let (timeout_s, screen_label) = SCREEN_TIMEOUTS[screen_i];
        if confirm.is_some_and(|(_, until)| Instant::now() > until) {
            confirm = None;
        }
        if toast_msg.as_ref().is_some_and(|(_, until)| Instant::now() > *until) {
            toast_msg = None;
        }
        let asleep = timeout_s > 0 && last_input.elapsed() > Duration::from_secs(timeout_s as u64);
        if asleep && node_note().is_none() {
            if on {
                let _ = d.set_display_on(false);
                on = false;
            }
        } else {
            let mut v = snapshot(page, sel, confirm.map(|c| c.0), screen_label);
            v.toast = toast_msg.as_ref().map(|t| t.0.clone()).or_else(|| node_note().map(|n| {
                let mut s = heapless::String::new();
                let _ = s.push_str(n);
                s
            }));
            ui::draw(&mut d, &v);
            d.flush().ok();
            if !on {
                let _ = d.set_display_on(true);
                on = true;
            }
        }

        let Some(p) = press(&mut button, Duration::from_secs(1)).await else {
            continue;
        };
        let was_dark = !on;
        last_input = Instant::now();
        if was_dark {
            continue; // the first press only wakes the screen
        }
        match p {
            Press::VeryLong => {
                // Long-standing shortcut: switch Bluetooth / Wi-Fi (restarts).
                TOGGLE_RADIO.signal(());
                page = ui::PAGE_HOME;
            }
            Press::Short if page == ui::PAGE_SETTINGS => {
                confirm = None;
                sel += 1;
                if sel >= ui::SET_ITEMS {
                    sel = 0;
                    page = ui::PAGE_HOME;
                }
            }
            Press::Short => page = (page + 1) % ui::PAGES,
            Press::Long if page != ui::PAGE_SETTINGS => {
                page = ui::PAGE_SETTINGS;
                sel = 0;
            }
            Press::Long => {
                let confirmed = confirm.is_some_and(|(i, _)| i == sel);
                match sel {
                    ui::SET_ROLE => {
                        let now = ROLE_SHOW.load(Relaxed) as u8;
                        let i = ROLE_CYCLE.iter().position(|r| *r == now).unwrap_or(0);
                        let next = ROLE_CYCLE[(i + 1) % ROLE_CYCLE.len()];
                        let _ = DISPLAY_CMD.try_send(DCMD_ROLE + next);
                        let mut t: heapless::String<32> = heapless::String::new();
                        let _ = write!(t, "Role: {}", ui::role_name(next));
                        toast_msg = toast(&t);
                    }
                    ui::SET_RADIO if !WIFI_SUPPORTED.load(Relaxed) => toast_msg = toast("Only one radio here"),
                    ui::SET_RADIO | ui::SET_RESTART if !confirmed => confirm = Some((sel, Instant::now() + CONFIRM)),
                    ui::SET_RADIO => {
                        confirm = None;
                        TOGGLE_RADIO.signal(());
                    }
                    ui::SET_RESTART => {
                        toast_msg = toast("Restarting");
                        let mut v = snapshot(page, sel, None, screen_label);
                        v.toast = toast_msg.as_ref().map(|t| t.0.clone());
                        ui::draw(&mut d, &v);
                        d.flush().ok();
                        Timer::after_millis(600).await;
                        RESTART.signal(());
                    }
                    ui::SET_ANNOUNCE => {
                        let _ = DISPLAY_CMD.try_send(DCMD_ANNOUNCE);
                        toast_msg = toast(if MODEM_HOST_SHORT.load(Relaxed) != 0 { "The phone announces" } else { "Announce sent" });
                    }
                    ui::SET_SCREEN => {
                        screen_i = (screen_i + 1) % SCREEN_TIMEOUTS.len();
                        let mut t: heapless::String<32> = heapless::String::new();
                        let _ = write!(t, "Screen: {}", SCREEN_TIMEOUTS[screen_i].1);
                        toast_msg = toast(&t);
                    }
                    _ => {}
                }
            }
        }
    }
}
