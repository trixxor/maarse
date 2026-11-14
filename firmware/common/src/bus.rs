//! Kanaler og fælles tællere mellem opgaverne (radio, USB og node).

use core::cell::RefCell;
use core::sync::atomic::{AtomicI32, AtomicU32, Ordering::Relaxed};
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
}

/// Modtaget over LoRa: pakke og SNR.
pub static RADIO_RX: Channel<M, (Pkt, i8), 4> = Channel::new();
/// Skal sendes over LoRa.
pub static RADIO_TX: Channel<M, TxReq, 8> = Channel::new();
/// Annullér en planlagt udsendelse.
pub static RADIO_CANCEL: Channel<M, u64, 8> = Channel::new();
/// Nye radioindstillinger.
pub static RADIO_CFG: Signal<M, RadioConfig> = Signal::new();
/// Rammer fra USB (uden længdefelt).
pub static USB_IN: Channel<M, Frame, 4> = Channel::new();
/// Færdige rammer til USB (med længdefelt).
pub static USB_OUT: Channel<M, FrameBuf, 16> = Channel::new();
/// Færdige rammer til Bluetooth (med længdefelt).
pub static BLE_OUT: Channel<M, FrameBuf, 16> = Channel::new();

/// Radiostatistik, opdateres af radio-opgaven.
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
    /// Glidende gennemsnit × 16.
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

/// Til displayet: antal kendte noder, antal modtagne beskeder og den seneste.
pub static KNOWN_NODES: AtomicU32 = AtomicU32::new(0);
pub static MSG_COUNT: AtomicU32 = AtomicU32::new(0);
pub type Preview = heapless::String<64>;
pub static LAST_MSG: Mutex<M, RefCell<(u32, Preview)>> =
    Mutex::new(RefCell::new((0, heapless::String::new())));
/// Nodens egen kortadresse (første 4 bytes), til displayet.
pub static MY_SHORT: AtomicU32 = AtomicU32::new(0);

/// Husker den seneste besked (afsenderens kortadresse og starten af teksten).
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

/// Fejlsøgning af radioen ved opstart (sættes af board-koden):
/// [0] sync word-register 0x0740 (forventet 0x1424), [1] status-byte,
/// [2] device errors efter TCXO + kalibrering (0 = ingen fejl),
/// [3] BUSY-niveau efter reset (0 = lav, som forventet).
pub static RADIO_DIAG: [AtomicU32; 4] = [
    AtomicU32::new(0xFFFF_FFFF),
    AtomicU32::new(0xFFFF_FFFF),
    AtomicU32::new(0xFFFF_FFFF),
    AtomicU32::new(0xFFFF_FFFF),
];
