//! Channels and shared counters between the tasks (radio, USB and node).

use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering::Relaxed};
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex as M;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use maarse_core::Address;
use maarse_core::MAX_PACKET;
use maarse_core::companion::{FrameBuf, MAX_FRAME, RadioConfig};

pub type Pkt = heapless::Vec<u8, MAX_PACKET>;
pub type Frame = heapless::Vec<u8, MAX_FRAME>;

pub struct TxReq {
    pub packet: Pkt,
    pub delay_ms: u32,
    pub id: u64,
    /// Send with this spreading factor instead of the current one (0 = the
    /// current). Used by range test beacons that step through SF7-SF12; the
    /// radio returns to its own settings for receiving.
    pub sf: u8,
}

/// Range test: `seq + 1` of the last beacon the radio transmitted (0 = none).
/// Set by the radio loop for jobs tagged with `rangetest::TX_ID_TAG`.
pub static BEACON_SENT: AtomicU32 = AtomicU32::new(0);

/// Received over LoRa: packet and SNR.
pub static RADIO_RX: Channel<M, (Pkt, i8), 4> = Channel::new();
/// To be sent over LoRa.
pub static RADIO_TX: Channel<M, TxReq, 8> = Channel::new();
/// Cancel a planned transmission.
pub static RADIO_CANCEL: Channel<M, u64, 8> = Channel::new();
/// New radio settings.
pub static RADIO_CFG: Signal<M, RadioConfig> = Signal::new();
/// Frames from USB (without length field).
pub static USB_IN: Channel<M, Frame, 4> = Channel::new();
/// Finished frames for USB (with length field).
pub static USB_OUT: Channel<M, FrameBuf, 16> = Channel::new();
/// Finished frames for Bluetooth (with length field).
pub static BLE_OUT: Channel<M, FrameBuf, 16> = Channel::new();

/// Packets received on the Wi-Fi backbone link (TCP), one Må:rse packet each.
pub static NET_RX: Channel<M, Pkt, 4> = Channel::new();
/// Packets to send on the Wi-Fi link, with the SNR they were heard with (only
/// used in modem mode, where the host needs it).
pub static NET_TX: Channel<M, (Pkt, i8), 8> = Channel::new();
/// The modem's own status frames for the host over Wi-Fi (`Stats` events,
/// without length field), sent every 30 s in Wi-Fi modem mode.
pub static NET_STATS: Channel<M, heapless::Vec<u8, { maarse_core::companion::MAX_FRAME }>, 3> = Channel::new();
/// A Wi-Fi modem working as an ordinary node while its link is down (for the display).
pub static MODEM_FALLBACK: AtomicBool = AtomicBool::new(false);
/// The node is the host's LoRa modem over Wi-Fi (set at boot by board code).
pub static WIFI_MODEM: AtomicBool = AtomicBool::new(false);
/// Set by board code that has Wi-Fi (ESP32-S3).
pub static WIFI_SUPPORTED: AtomicBool = AtomicBool::new(false);
/// Wi-Fi state (`companion::wifi_state`), kept up to date by the Wi-Fi task.
pub static WIFI_STATE: AtomicU32 = AtomicU32::new(0);
/// IPv4 address (big-endian octets in a u32), 0 = none.
pub static WIFI_IP: AtomicU32 = AtomicU32::new(0);
/// Memory diagnostics set by board code (0 = unknown): never-used main
/// stack, free heap and smallest free heap since boot, in bytes.
pub static STACK_FREE: AtomicU32 = AtomicU32::new(0);
pub static HEAP_FREE: AtomicU32 = AtomicU32::new(0);
pub static HEAP_MIN_FREE: AtomicU32 = AtomicU32::new(0);
pub static CRASHES: AtomicU32 = AtomicU32::new(0);
pub static PANIC_LINE: AtomicU32 = AtomicU32::new(0);
pub static PANIC_FILE: AtomicU32 = AtomicU32::new(0);
/// Bluetooth is running (advertising or connected), and a phone is connected.
pub static BLE_ACTIVE: AtomicBool = AtomicBool::new(false);
pub static BLE_CONNECTED: AtomicBool = AtomicBool::new(false);
/// Uptime (s) of the last frame from the USB host, 0 = none yet.
pub static USB_LAST_RX: AtomicU32 = AtomicU32::new(0);
/// The board's button asks to switch between Bluetooth and Wi-Fi.
pub static TOGGLE_RADIO: Signal<M, ()> = Signal::new();
/// Short note for the display: `code << 24 | until (uptime s)`; see
/// `NOTE_*`. 0 = none.
pub static DISPLAY_NOTE: AtomicU32 = AtomicU32::new(0);
pub const NOTE_TO_WIFI: u32 = 1;
pub const NOTE_TO_BLE: u32 = 2;
pub const NOTE_WIFI_NOT_SET_UP: u32 = 3;
pub const NOTE_ONE_RADIO: u32 = 4;

/// Shows `code` on the display for `secs` seconds.
pub fn note(code: u32, secs: u32) {
    let until = embassy_time::Instant::now().as_secs() as u32 + secs;
    DISPLAY_NOTE.store(code << 24 | (until & 0x00FF_FFFF), Relaxed);
}

/// Ask the board to restart (to apply saved Wi-Fi settings).
pub static RESTART: Signal<M, ()> = Signal::new();

/// Radio statistics, updated by the radio task.
pub struct RadioStats {
    pub rx_ok: AtomicU32,
    pub rx_err: AtomicU32,
    pub tx_ok: AtomicU32,
    pub tx_err: AtomicU32,
    pub cad_busy: AtomicU32,
    pub duty_deferred: AtomicU32,
    pub dropped: AtomicU32,
    pub airtime_ms: AtomicU32,
    pub bytes_rx: AtomicU32,
    pub bytes_tx: AtomicU32,
    pub last_rssi: AtomicI32,
    pub last_snr: AtomicI32,
    /// Moving average × 16.
    pub avg_rssi_x16: AtomicI32,
    pub avg_snr_x16: AtomicI32,
    pub min_rssi: AtomicI32,
    pub max_rssi: AtomicI32,
    pub tx_queue: AtomicU32,
    pub duty_used_permille: AtomicU32,
    pub stage: AtomicU32,
    pub loops: AtomicU32,
}

pub static RSTATS: RadioStats = RadioStats {
    rx_ok: AtomicU32::new(0),
    rx_err: AtomicU32::new(0),
    tx_ok: AtomicU32::new(0),
    tx_err: AtomicU32::new(0),
    cad_busy: AtomicU32::new(0),
    duty_deferred: AtomicU32::new(0),
    dropped: AtomicU32::new(0),
    airtime_ms: AtomicU32::new(0),
    bytes_rx: AtomicU32::new(0),
    bytes_tx: AtomicU32::new(0),
    last_rssi: AtomicI32::new(0),
    last_snr: AtomicI32::new(0),
    avg_rssi_x16: AtomicI32::new(0),
    avg_snr_x16: AtomicI32::new(0),
    min_rssi: AtomicI32::new(0),
    max_rssi: AtomicI32::new(-200),
    tx_queue: AtomicU32::new(0),
    duty_used_permille: AtomicU32::new(0),
    stage: AtomicU32::new(0),
    loops: AtomicU32::new(0),
};

impl RadioStats {
    pub fn received(&self, len: usize, rssi: i32, snr: i32) {
        let first = self.rx_ok.fetch_add(1, Relaxed) == 0;
        crate::power::note_packet(rssi, snr);
        self.bytes_rx.fetch_add(len as u32, Relaxed);
        self.last_rssi.store(rssi, Relaxed);
        self.last_snr.store(snr, Relaxed);
        let ema = |a: &AtomicI32, v: i32| {
            let old = a.load(Relaxed);
            a.store(
                if first {
                    v * 16
                } else {
                    old + (v * 16 - old) / 8
                },
                Relaxed,
            );
        };
        ema(&self.avg_rssi_x16, rssi);
        ema(&self.avg_snr_x16, snr);
        if first || rssi < self.min_rssi.load(Relaxed) {
            self.min_rssi.store(rssi, Relaxed);
        }
        if rssi > self.max_rssi.load(Relaxed) {
            self.max_rssi.store(rssi, Relaxed);
        }
    }
}

/// For the display: number of known nodes, number of received messages and the latest one.
pub static KNOWN_NODES: AtomicU32 = AtomicU32::new(0);
pub static MSG_COUNT: AtomicU32 = AtomicU32::new(0);
pub type Preview = heapless::String<64>;
pub static LAST_MSG: Mutex<M, RefCell<(u32, Preview)>> =
    Mutex::new(RefCell::new((0, heapless::String::new())));
/// The node's own short address (first 4 bytes), for the display.
pub static MY_SHORT: AtomicU32 = AtomicU32::new(0);

/// Remembers the latest message (the sender's short address and the start of the text).
pub fn note_message(from: &Address, text: &[u8]) {
    MSG_COUNT.fetch_add(1, Relaxed);
    let short = u32::from_be_bytes([from.0[0], from.0[1], from.0[2], from.0[3]]);
    let mut p = Preview::new();
    let s = core::str::from_utf8(text).unwrap_or("?");
    for c in s.chars() {
        if p.push(c).is_err() {
            break;
        }
    }
    LAST_MSG.lock(|m| *m.borrow_mut() = (short, p));
}

/// Radio debugging at start-up (set by the board code):
/// [0] sync word register 0x0740 (expected 0x1424), [1] status byte,
/// [2] device errors after TCXO + calibration (0 = no errors),
/// [3] BUSY level after reset (0 = low, as expected).
pub static RADIO_DIAG: [AtomicU32; 4] = [
    AtomicU32::new(0xFFFF_FFFF),
    AtomicU32::new(0xFFFF_FFFF),
    AtomicU32::new(0xFFFF_FFFF),
    AtomicU32::new(0xFFFF_FFFF),
];
