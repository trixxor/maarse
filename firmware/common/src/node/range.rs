//! Range test on the node (see `maarse_core::rangetest`): the beacon sends
//! numbered packets, the receiver reports what it hears and follows the
//! beacon's SF schedule, and both keep an estimate of the charge used per
//! mode (off, beacon, receiver) for battery life comparisons.
//!
//! The setting lasts until the next restart, unless `FLAG_PERSIST` is set: then
//! it is stored in flash and resumed at boot (useful for a beacon that is
//! carried to its spot without a phone attached).

use super::{FwNode, Settings, emit, log, now_ms};
use crate::bus::{BEACON_SENT, Pkt, RADIO_CANCEL, RADIO_CFG, RADIO_TX, RSTATS, TxReq};
use crate::power;
use core::sync::atomic::Ordering::Relaxed;
use embassy_time::{Duration, Instant};
use maarse_core::companion::{
    Command, Event, PowerPhase, PowerReport, RadioConfig, RangeRx, RangeSfStat, RangeState, RangeTx,
};
use maarse_core::rangetest::{self, Beacon, Config, MAX_STEPS, SF_MIN, Schedule, mode};
use rand_core::{CryptoRng, RngCore};

/// How often a `Power` report is sent while a range test runs.
const REPORT_EVERY: Duration = Duration::from_secs(60);
/// A receiver that has heard nothing for this many cycles stops following the
/// schedule and waits on the first SF of the cycle until it hears the beacon again.
const LOST_AFTER_CYCLES: i64 = 3;
/// Most missing sequence numbers attributed to SFs one by one after a gap.
const MAX_GAP_WALK: u32 = 10_000;

#[derive(Clone, Copy)]
struct SfAcc {
    rx: u32,
    lost: u32,
    rssi_sum: i64,
    rssi_min: i16,
    snr_sum: i64,
    snr_min: i8,
}

impl Default for SfAcc {
    fn default() -> Self {
        SfAcc { rx: 0, lost: 0, rssi_sum: 0, rssi_min: 0, snr_sum: 0, snr_min: 0 }
    }
}

#[derive(Clone, Copy, Default)]
struct PhaseAcc {
    ms: u64,
    tx_ms: u64,
    /// Estimated charge in mA·ms.
    charge: u64,
}

pub(super) struct Range {
    cfg: Config,
    // ----- beacon -----
    start_ms: u64,
    sched: Option<Schedule>,
    pkt_len: usize,
    next_seq: u32,
    next_at: Option<Instant>,
    queued: u32,
    skipped: u32,
    /// Sequence number queued last and not yet confirmed as transmitted.
    pending: Option<u32>,
    // ----- receiver -----
    /// The beacon's schedule and the estimated start of a cycle (ms of uptime;
    /// may be negative).
    follow: Option<(Schedule, i64)>,
    /// (sender, seq, skipped) of the last beacon heard.
    last: Option<([u8; 4], u32, u16)>,
    last_heard_ms: u64,
    received: u32,
    lost: u32,
    per_sf: [SfAcc; MAX_STEPS],
    /// SF the radio is receiving on (0 = the radio's own setting).
    rx_sf: u8,
    rx_step: Option<usize>,
    switch_at: Option<Instant>,
    // ----- power -----
    acc_at_ms: u64,
    acc_tx_ms: u32,
    phases: [PhaseAcc; 3],
    next_report: Instant,
}

impl Range {
    pub(super) fn new() -> Self {
        Range {
            cfg: Config::default(),
            start_ms: 0,
            sched: None,
            pkt_len: 0,
            next_seq: 0,
            next_at: None,
            queued: 0,
            skipped: 0,
            pending: None,
            follow: None,
            last: None,
            last_heard_ms: 0,
            received: 0,
            lost: 0,
            per_sf: [SfAcc::default(); MAX_STEPS],
            rx_sf: 0,
            rx_step: None,
            switch_at: None,
            acc_at_ms: now_ms(),
            acc_tx_ms: 0,
            phases: [PhaseAcc::default(); 3],
            next_report: Instant::now() + REPORT_EVERY,
        }
    }

    /// When the range test next needs the node's attention.
    pub(super) fn deadline(&self) -> Option<Instant> {
        if self.cfg.mode == mode::OFF {
            return None;
        }
        let mut d = self.next_report;
        for t in [self.next_at, self.switch_at].into_iter().flatten() {
            d = d.min(t);
        }
        Some(d)
    }
}

fn sf_index(sf: u8) -> Option<usize> {
    sf.checked_sub(SF_MIN).map(usize::from).filter(|&i| i < MAX_STEPS)
}

impl<R: RngCore + CryptoRng, S: Settings> FwNode<R, S> {
    /// Resumes a stored range test (called once at boot).
    pub(super) fn range_init(&mut self) {
        let cfg = self.settings.load_range();
        if cfg.persist() && cfg.mode != mode::OFF {
            self.range_set(cfg);
            log("Range test resumed from flash");
        }
    }

    /// `SetRangeTest`/`GetRangeTest`. Returns the command if it does not belong here.
    pub(super) fn range_command(&mut self, cmd: Command) -> Option<Command> {
        match cmd {
            Command::SetRangeTest(cfg) => match cfg.sanitized() {
                Ok(cfg) => {
                    self.range_set(cfg);
                    self.range_store(&cfg);
                    log(match cfg.mode {
                        mode::BEACON => "Range test: beacon started",
                        mode::RECEIVER => "Range test: receiver started",
                        _ => "Range test stopped",
                    });
                }
                Err(e) => super::err(0, maarse_core::companion::error_code::BAD_FRAME, e),
            },
            Command::GetRangeTest => {
                emit(&Event::RangeState(self.range_state()));
                self.power_account();
                emit(&Event::Power(self.power_report()));
            }
            other => return Some(other),
        }
        None
    }

    /// Keeps flash in line with `cfg`: stored when it should persist, cleared
    /// otherwise (written only when it changes).
    fn range_store(&mut self, cfg: &Config) {
        let want = if cfg.persist() && cfg.mode != mode::OFF { *cfg } else { Config::default() };
        if self.settings.load_range() != want {
            self.settings.save_range(&want);
        }
    }

    /// Stops what runs and starts `cfg` (already sanitized).
    fn range_set(&mut self, cfg: Config) {
        self.power_account();
        // Stop the old mode.
        if let Some(seq) = self.range.pending.take() {
            let _ = RADIO_CANCEL.try_send(rangetest::TX_ID_TAG | seq as u64);
        }
        if self.range.rx_sf != 0 && self.range.rx_sf != self.radio.sf {
            RADIO_CFG.signal(self.radio);
        }
        let old = core::mem::replace(&mut self.range, Range::new());
        // Power accounting continues across modes.
        self.range.acc_at_ms = old.acc_at_ms;
        self.range.acc_tx_ms = old.acc_tx_ms;
        self.range.phases = old.phases;
        self.range.cfg = cfg;
        let now = now_ms();
        match cfg.mode {
            mode::BEACON => {
                let sfs = cfg.sfs(self.radio.sf);
                let template = Beacon {
                    sender: [0; 4],
                    seq: 0,
                    sf: 0,
                    slot: 0,
                    per_step: 0,
                    sf_mask: 0,
                    skipped: 0,
                    tx_power_dbm: 0,
                    battery_mv: 0,
                    intervals_s: sfs.iter().map(|_| 0).collect(),
                    position: self.beacon_position(),
                };
                let len = template.encoded_len(cfg.pad).min(maarse_core::MAX_PACKET);
                let radio = self.radio;
                let sched = Schedule::for_beacon(&cfg, radio.sf, radio.duty_permille, |sf| {
                    RadioConfig { sf, ..radio }.airtime().airtime_us(len)
                });
                self.range.pkt_len = len;
                self.range.start_ms = now;
                self.range.next_at = Some(Instant::from_millis(now + sched.send_at_ms(0)));
                self.range.sched = Some(sched);
            }
            mode::RECEIVER => {
                let first = cfg.sfs(self.radio.sf)[0];
                self.set_rx_sf(first);
            }
            _ => {}
        }
        emit(&Event::RangeState(self.range_state()));
    }

    /// The position a beacon sends: from the config, otherwise the shared one
    /// (rounded as chosen under sharing), otherwise none.
    fn beacon_position(&self) -> Option<(i32, i32)> {
        if self.range.cfg.flags & rangetest::FLAG_POSITION != 0 {
            return Some((self.range.cfg.lat_e7, self.range.cfg.lon_e7));
        }
        self.shared_position().map(|p| (p.lat_e7, p.lon_e7))
    }

    /// New radio settings were applied (`SetRadio`): a receiver that is on
    /// another SF for the range test goes back to it with the new settings.
    pub(super) fn range_radio_changed(&mut self) {
        if self.range.cfg.mode == mode::RECEIVER && self.range.rx_sf != 0 && self.range.rx_sf != self.radio.sf {
            RADIO_CFG.signal(RadioConfig { sf: self.range.rx_sf, ..self.radio });
        }
    }

    fn set_rx_sf(&mut self, sf: u8) {
        if sf != self.range.rx_sf {
            self.range.rx_sf = sf;
            RADIO_CFG.signal(RadioConfig { sf, ..self.radio });
        }
    }

    /// Called from `on_timer`.
    pub(super) fn range_tick(&mut self) {
        if self.range.cfg.mode == mode::OFF {
            return;
        }
        let now = Instant::now();
        if self.range.next_at.is_some_and(|t| t <= now) && !self.modem && !self.wifi_modem() {
            self.send_beacon();
        }
        if self.range.switch_at.is_some_and(|t| t <= now) {
            self.follow_schedule();
        }
        if now >= self.range.next_report {
            self.range.next_report = now + REPORT_EVERY;
            self.power_account();
            emit(&Event::Power(self.power_report()));
        }
    }

    fn send_beacon(&mut self) {
        let Some(sched) = self.range.sched.clone() else { return };
        let now = now_ms();
        // Was the previous beacon transmitted? (Duty cycle limit or a busy
        // channel can hold it back; then it is dropped and counted.)
        if let Some(seq) = self.range.pending.take()
            && BEACON_SENT.load(Relaxed) != seq.wrapping_add(1)
        {
            self.range.skipped += 1;
            let _ = RADIO_CANCEL.try_send(rangetest::TX_ID_TAG | seq as u64);
        }
        // Slots that passed while the node was busy are skipped, not sent in a burst.
        while self.range.start_ms + sched.send_at_ms(self.range.next_seq + 1) <= now {
            self.range.next_seq += 1;
            self.range.skipped += 1;
        }
        let seq = self.range.next_seq;
        let (step, slot, _) = sched.locate(seq);
        let sf = sched.sfs[step];
        let b = Beacon {
            sender: [self.addr.0[0], self.addr.0[1], self.addr.0[2], self.addr.0[3]],
            seq,
            sf,
            slot,
            per_step: sched.per_step,
            sf_mask: if sched.cycling { self.range.cfg.sf_mask } else { 0 },
            skipped: self.range.skipped.min(u16::MAX as u32) as u16,
            tx_power_dbm: self.radio.tx_power_dbm,
            battery_mv: power::BATTERY_MV.load(Relaxed).min(u16::MAX as u32) as u16,
            intervals_s: sched.intervals_s.clone(),
            position: self.beacon_position(),
        };
        let mut buf = [0u8; maarse_core::MAX_PACKET];
        let n = b.encode(self.range.cfg.pad, &mut buf);
        let id = rangetest::TX_ID_TAG | seq as u64;
        if let Ok(packet) = Pkt::from_slice(&buf[..n])
            && n > 0
            && RADIO_TX.try_send(TxReq { packet, delay_ms: 0, id, sf }).is_ok()
        {
            self.range.pending = Some(seq);
            self.range.queued += 1;
            let air_us = RadioConfig { sf, ..self.radio }.airtime().airtime_us(n);
            let air_ms = air_us.div_ceil(1000);
            let charge = air_ms as u64 * power::tx_ma_x10(self.radio.tx_power_dbm) as u64 / 10;
            emit(&Event::RangeTx(RangeTx {
                uptime_ms: now as u32,
                seq,
                sf,
                slot,
                interval_s: sched.intervals_s[step] as u16,
                airtime_ms: air_ms.min(u16::MAX as u32) as u16,
                charge_ma_ms: charge.min(u32::MAX as u64) as u32,
                skipped: b.skipped,
            }));
        } else {
            self.range.skipped += 1;
        }
        self.range.next_seq = seq + 1;
        self.range.next_at = Some(Instant::from_millis(self.range.start_ms + sched.send_at_ms(seq + 1)));
    }

    /// A received packet that may be a range test beacon. Returns `true` if it
    /// was one (it never goes to the router: it is not a Må:rse packet).
    pub(super) fn range_on_radio(&mut self, pkt: &[u8], snr: i8) -> bool {
        let Some(b) = Beacon::parse(pkt) else { return false };
        if self.range.cfg.mode != mode::RECEIVER {
            return true;
        }
        let now = now_ms();
        let rssi = RSTATS.last_rssi.load(Relaxed).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        let sched = b.schedule();
        // Loss from the sequence gap since the last beacon from the same sender.
        let lost = match self.range.last {
            Some((sender, seq, skipped)) if sender == b.sender => {
                rangetest::lost_between(seq, skipped, b.seq, b.skipped).unwrap_or(0)
            }
            _ => 0,
        };
        if lost > 0
            && let Some((_, last_seq, _)) = self.range.last
        {
            for m in (last_seq + 1..b.seq).take(lost.min(MAX_GAP_WALK) as usize) {
                if let Some(i) = sf_index(sched.sf_of(m)) {
                    self.range.per_sf[i].lost += 1;
                }
            }
        }
        self.range.lost += lost;
        self.range.received += 1;
        if let Some(i) = sf_index(b.sf) {
            let a = &mut self.range.per_sf[i];
            if a.rx == 0 || rssi < a.rssi_min {
                a.rssi_min = rssi;
            }
            if a.rx == 0 || snr < a.snr_min {
                a.snr_min = snr;
            }
            a.rx += 1;
            a.rssi_sum += rssi as i64;
            a.snr_sum += snr as i64;
        }
        self.range.last = Some((b.sender, b.seq, b.skipped));
        self.range.last_heard_ms = now;
        emit(&Event::RangeRx(RangeRx {
            uptime_ms: now as u32,
            sender: b.sender,
            seq: b.seq,
            sf: b.sf,
            rssi,
            snr,
            len: pkt.len().min(255) as u8,
            lost: lost.min(u16::MAX as u32) as u16,
            skipped: b.skipped,
            tx_power_dbm: b.tx_power_dbm,
            battery_mv: b.battery_mv,
            position: b.position,
        }));
        // Follow the beacon's schedule. The beacon started sending about one
        // airtime ago (the packet has just been received); listen-before-talk
        // may have delayed it, so the estimate errs late, which the guard
        // at the start of each step absorbs.
        if sched.cycling {
            let air_ms = (RadioConfig { sf: b.sf, ..self.radio }.airtime().airtime_us(pkt.len()) / 1000) as i64;
            let est = rangetest::cycle_start_estimate(&sched, b.seq, now as i64, air_ms);
            let cur = match &self.range.follow {
                Some((old, cur)) if *old == sched => Some(*cur),
                _ => None,
            };
            let start = rangetest::refine_cycle_start(cur, est, sched.cycle_ms() as i64);
            self.range.follow = Some((sched, start));
            self.follow_schedule();
        } else {
            self.range.follow = None;
            self.range.switch_at = None;
        }
        true
    }

    /// Puts the receiver on the SF the beacon is using now and plans the next switch.
    fn follow_schedule(&mut self) {
        let now = now_ms() as i64;
        let Some((sched, start)) = self.range.follow.clone() else {
            self.range.switch_at = None;
            return;
        };
        let cycle = sched.cycle_ms() as i64;
        if now - self.range.last_heard_ms as i64 > LOST_AFTER_CYCLES * cycle {
            // Lost: wait on the first SF of the cycle, where the beacon returns every cycle.
            self.range.follow = None;
            self.range.switch_at = None;
            self.range.rx_step = None;
            self.set_rx_sf(sched.sfs[0]);
            log("Range test: the beacon has not been heard for 3 cycles; waiting on the first SF");
            emit(&Event::RangeState(self.range_state()));
            return;
        }
        let (step, left) = sched.step_at((now - start).rem_euclid(cycle) as u64);
        self.set_rx_sf(sched.sfs[step]);
        self.range.switch_at = Some(Instant::from_millis(now as u64 + left.max(1)));
        if self.range.rx_step != Some(step) {
            self.range.rx_step = Some(step);
            emit(&Event::RangeState(self.range_state()));
        }
    }

    fn range_state(&self) -> RangeState {
        let r = &self.range;
        let receiver = r.cfg.mode == mode::RECEIVER;
        let sf = match r.cfg.mode {
            mode::RECEIVER if r.rx_sf != 0 => r.rx_sf,
            mode::BEACON => r.sched.as_ref().map_or(self.radio.sf, |s| s.sf_of(r.next_seq)),
            _ => self.radio.sf,
        };
        let per_sf = r
            .per_sf
            .iter()
            .enumerate()
            .filter(|(_, a)| a.rx > 0 || a.lost > 0)
            .map(|(i, a)| {
                let n = a.rx.max(1) as i64;
                RangeSfStat {
                    sf: SF_MIN + i as u8,
                    rx: a.rx.min(u16::MAX as u32) as u16,
                    lost: a.lost.min(u16::MAX as u32) as u16,
                    rssi_avg: (a.rssi_sum / n) as i16,
                    rssi_min: a.rssi_min,
                    snr_avg_x4: (a.snr_sum * 4 / n) as i16,
                    snr_min: a.snr_min,
                }
            })
            .collect();
        RangeState {
            config: r.cfg,
            sf,
            synced: r.follow.is_some(),
            count: if receiver { r.received } else { r.queued },
            missed: if receiver { r.lost } else { r.skipped },
            seq: if receiver { r.last.map_or(0, |l| l.1) } else { r.next_seq },
            per_sf,
        }
    }

    /// Adds the time since the last call to the current mode's power account.
    pub(super) fn power_account(&mut self) {
        let now = now_ms();
        let tx_total = RSTATS.airtime_ms.load(Relaxed);
        let r = &mut self.range;
        let dt = now.saturating_sub(r.acc_at_ms);
        let tx = tx_total.wrapping_sub(r.acc_tx_ms) as u64;
        r.acc_at_ms = now;
        r.acc_tx_ms = tx_total;
        let p = &mut r.phases[(r.cfg.mode as usize).min(2)];
        p.ms += dt;
        p.tx_ms += tx.min(dt);
        p.charge += rangetest::charge_ma_ms(
            power::BASE_MA_X10.load(Relaxed),
            power::RX_MA_X10.load(Relaxed),
            power::tx_ma_x10(self.radio.tx_power_dbm),
            dt,
            tx,
        );
    }

    fn power_report(&self) -> PowerReport {
        let r = &self.range;
        let c = |v: u32| v.min(u16::MAX as u32) as u16;
        let mut phases = [PowerPhase::default(); 3];
        for (o, p) in phases.iter_mut().zip(r.phases.iter()) {
            *o = PowerPhase {
                secs: (p.ms / 1000).min(u32::MAX as u64) as u32,
                tx_ms: p.tx_ms.min(u32::MAX as u64) as u32,
                charge_mas: (p.charge / 1000).min(u32::MAX as u64) as u32,
            };
        }
        PowerReport {
            uptime_s: self.started.elapsed().as_secs() as u32,
            battery_mv: c(power::BATTERY_MV.load(Relaxed)),
            ext_power: power::EXT_POWER.load(Relaxed).clamp(-1, 1) as i8,
            mode: r.cfg.mode,
            tx_power_dbm: self.radio.tx_power_dbm,
            base_ma_x10: c(power::BASE_MA_X10.load(Relaxed)),
            rx_ma_x10: c(power::RX_MA_X10.load(Relaxed)),
            tx_ma_x10: c(power::tx_ma_x10(self.radio.tx_power_dbm)),
            tx_packets: RSTATS.tx_ok.load(Relaxed),
            phases,
        }
    }
}
