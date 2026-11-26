//! Spectrum recordings made on the radio itself (`Command::SweepRecStart`, file format in
//! `maarse_core::sweeprec`).
//!
//! The radio task takes the sweeps (see `radio.rs`) and sends each row here. The node keeps the cells of the
//! recording in the two buffers it otherwise uses for file transfers, so a recording needs no RAM of its own (an
//! ESP32's stack is what is left of its RAM, and there was no room); no file is sent or received while a recording
//! runs. When the time is up the file is made in the send buffer and sent from there, to whoever asked: over the air to an
//! administrator who asked through `Remote`, or as an incoming file to the client on USB or Bluetooth.

use super::{FwNode, Settings, emit, log};
use crate::bus::{REC_REQ, REC_ROWS, REC_START_DELAY_MS, RecReq, wake_radio_loop};
use embassy_time::{Duration, Instant};
use maarse_core::Address;
use maarse_core::companion::{Event, error_code, sweep_rec_state as st, text};
use maarse_core::sweeprec::{Config, MAX_DURATION_S, Recorder};
use rand_core::{CryptoRng, RngCore};

const MIME: &[u8] = b"application/x-maarse-msr";
const NAME: &[u8] = b"Spectrum.msr";
/// The radio chips cover roughly this range.
const FIRST_KHZ_MIN: u32 = 150_000;
const LAST_KHZ_MAX: u32 = 960_000;
/// Default time between two sweeps, in tenths of a second: back to back, 10 sweeps a second. The radio hears nothing (and
/// sends nothing) while it records; with a sweep now and then the strongest value of a cell is mostly noise.
const DEFAULT_INTERVAL_DS: u8 = 1;
/// The longest recording in seconds: the radio is deaf all that time.
const MAX_RECORDING_S: u16 = 600;
/// The radio task may still be measuring when the time is up: rows that come this late are taken too.
const GRACE: Duration = Duration::from_millis(300);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    /// Sweeping; the cells are in the file buffers.
    Recording,
    /// The file is made in the send buffer and waits to be sent.
    Ready,
    /// The file is on its way over the air.
    Sending,
    /// Did not work; the client is told once.
    Failed,
}

pub(super) struct Rec {
    phase: Phase,
    recorder: Recorder,
    began: Instant,
    duration_s: u16,
    /// Length of the file, once it is made.
    len: usize,
    /// Who asked over the air (None = the client on USB or Bluetooth).
    dest: Option<Address>,
    /// The counter of the command that asked, used in replies.
    counter: u32,
}

impl Rec {
    pub(super) fn new() -> Self {
        Rec { phase: Phase::Idle, recorder: Recorder::empty(), began: Instant::from_ticks(0), duration_s: 0, len: 0, dest: None, counter: 0 }
    }

    /// When the node needs to look at the recording again.
    pub(super) fn deadline(&self) -> Option<Instant> {
        match self.phase {
            Phase::Idle => None,
            Phase::Sending => Some(Instant::now() + Duration::from_secs(1)),
            // Often while recording: a row waits in one slot until the node takes it, at most a tenth of a second apart.
            Phase::Recording => Some(Instant::now() + Duration::from_millis(40)),
            _ => Some(Instant::now() + Duration::from_millis(100)),
        }
    }
}

impl<R: RngCore + CryptoRng, S: Settings> FwNode<R, S> {
    /// A recording uses the file buffers: no file may start while it runs or waits to be sent.
    pub(super) fn rec_holds_buffers(&self) -> bool {
        matches!(self.rec.phase, Phase::Recording | Phase::Ready)
    }

    /// How the recording is going, as an event.
    pub(super) fn rec_status(&self) -> Event {
        let r = &self.rec;
        let elapsed = if r.phase == Phase::Recording { (Instant::now() - r.began).as_secs().saturating_sub(REC_START_DELAY_MS / 1000) as u16 } else if r.phase == Phase::Idle { 0 } else { r.duration_s };
        Event::SweepRecState {
            state: match r.phase {
                Phase::Idle => st::IDLE,
                Phase::Recording => st::RECORDING,
                Phase::Ready => st::READY,
                Phase::Sending => st::SENDING,
                Phase::Failed => st::FAILED,
            },
            elapsed_s: elapsed.min(r.duration_s),
            duration_s: r.duration_s,
            sweeps: r.recorder.sweeps(),
            bytes: r.len as u16,
        }
    }

    /// Starts a recording for `requester` (None = the client on USB or Bluetooth). Returns the event to answer with.
    #[allow(clippy::too_many_arguments)]
    #[inline(never)]
    pub(super) fn rec_start(
        &mut self,
        requester: Option<Address>,
        counter: u32,
        start_khz: u32,
        step_khz: u16,
        duration_s: u16,
        interval_ds: u8,
        thr_db: u8,
    ) -> Event {
        let refuse = |msg: &str| Event::Error { req: 0, code: error_code::BAD_FRAME, text: text(msg.as_bytes()) };
        if self.rec.phase != Phase::Idle {
            return refuse("A spectrum recording is already running");
        }
        if !self.files_idle() {
            return refuse("A file is being transferred; try again when it is done");
        }
        let last = start_khz.saturating_add(step_khz as u32 * (maarse_core::sweeprec::BINS as u32 - 1));
        if step_khz == 0 || start_khz < FIRST_KHZ_MIN || last > LAST_KHZ_MAX {
            return refuse("The frequencies do not fit the radio");
        }
        if duration_s == 0 || duration_s > MAX_RECORDING_S.min(MAX_DURATION_S) {
            return refuse("The duration must be between 1 s and 10 min");
        }
        let interval_ms = u16::from(if interval_ds == 0 { DEFAULT_INTERVAL_DS } else { interval_ds }) * 100;
        let cfg = Config { start_khz, step_khz, duration_s, interval_ms, thr_db: if thr_db == 0 { 6 } else { thr_db } };
        let mut tables = self.files.rec_tables();
        if !self.rec.recorder.start(cfg, &mut tables) {
            return refuse("The recording does not fit");
        }
        self.rec.phase = Phase::Recording;
        self.rec.began = Instant::now();
        self.rec.duration_s = duration_s;
        self.rec.len = 0;
        self.rec.dest = requester;
        self.rec.counter = counter;
        REC_REQ.signal(Some(RecReq { start_khz, step_khz, duration_s, interval_ms }));
        wake_radio_loop();
        log("Spectrum recording started");
        self.rec_status()
    }

    /// Stops a recording and drops what it has.
    pub(super) fn rec_stop(&mut self) -> Event {
        REC_REQ.signal(None);
        wake_radio_loop();
        REC_ROWS.reset();
        if self.rec.phase != Phase::Sending {
            self.rec.phase = Phase::Idle;
        }
        self.rec.len = 0;
        log("Spectrum recording stopped");
        self.rec_status()
    }

    /// Tells the one who asked how it went (one over the air gets a control reply).
    fn rec_notify(&mut self) {
        let ev = self.rec_status();
        match self.rec.dest {
            Some(a) => {
                let f = ev.encode();
                let plain = maarse_core::content::control(maarse_core::content::CONTROL_REPLY, self.rec.counter, &f[2..]);
                let _ = self.send_plain(a, &plain);
            }
            None => emit(&ev),
        }
    }

    /// Takes the rows the radio has measured.
    #[inline(never)]
    fn rec_take_rows(&mut self) {
        if let Some(r) = REC_ROWS.try_take() {
            let masked = maarse_core::sweeprec::is_overload(&r.row);
            let mut tables = self.files.rec_tables();
            self.rec.recorder.push(&mut tables, r.t_ms, &r.row, masked);
        }
    }

    /// Looks at a recording a few times a second while one runs: takes the rows, makes the file when the time is up,
    /// and sends it.
    #[inline(never)]
    pub(super) fn rec_tick(&mut self) {
        match self.rec.phase {
            Phase::Idle => {}
            Phase::Recording => {
                self.rec_take_rows();
                let end = self.rec.began + Duration::from_millis(REC_START_DELAY_MS) + Duration::from_secs(self.rec.duration_s as u64);
                if Instant::now() < end + GRACE {
                    return;
                }
                REC_REQ.signal(None);
                wake_radio_loop();
                self.rec_take_rows();
                let mut tables = self.files.rec_tables();
                match self.rec.recorder.finish_in_place(&mut tables) {
                    // Over LoRa a file is at most 8 KB (`maarse_crypto::files::MAX_FILE`).
                    Ok(n) if n <= maarse_crypto::files::MAX_FILE => {
                        self.rec.len = n;
                        self.rec.phase = Phase::Ready;
                        log("Spectrum recording done");
                    }
                    _ => {
                        self.rec.phase = Phase::Failed;
                        log("The spectrum recording is too large to send");
                    }
                }
            }
            Phase::Ready => {
                let (len, dest) = (self.rec.len, self.rec.dest);
                match dest {
                    Some(dest) => match self.file_send_staged(dest, len, MIME, NAME) {
                        Ok(()) => {
                            self.rec.phase = Phase::Sending;
                            self.rec_notify();
                        }
                        // Busy with another file: try again in a moment. Anything else will not get better.
                        Err(e) if e.starts_with("A file") => {}
                        Err(e) => {
                            log(e);
                            self.rec.phase = Phase::Failed;
                        }
                    },
                    None => match self.file_staged_to_client(len, MIME, NAME) {
                        Ok(()) => self.rec.phase = Phase::Idle,
                        Err(e) if e.starts_with("A file") => {}
                        Err(e) => {
                            log(e);
                            self.rec.phase = Phase::Failed;
                        }
                    },
                }
            }
            Phase::Sending => {
                if !self.file_sending() {
                    self.rec.phase = Phase::Idle;
                }
            }
            Phase::Failed => {
                log("The spectrum recording failed");
                self.rec_notify();
                self.rec.phase = Phase::Idle;
            }
        }
    }
}
