//! The LoRa loop (shared by all boards): receives all the time and sends planned packets with
//! listen-before-talk (CAD) and a duty cycle budget. The settings can be
//! changed while it runs (`RADIO_CFG`).

use crate::bus::{Pkt, RADIO_ASLEEP, RADIO_CANCEL, RADIO_CFG, RADIO_RX, RADIO_SLEEP, RADIO_TX, RSTATS, SWEEP, SweepReq, TxReq};
use core::sync::atomic::Ordering::Relaxed;
use embassy_futures::select::{Either, Either6, select, select6};
use embassy_time::{Duration, Instant, Timer};
use lora_phy::mod_params::{
    Bandwidth, CodingRate, ModulationParams, PacketParams, SpreadingFactor,
};
use lora_phy::mod_traits::RadioKind;
use lora_phy::{DelayNs, LoRa, RxMode};
use maarse_core::companion::RadioConfig;
use maarse_core::duty::DutyCycle;

const QUEUE: usize = 16;

struct Planned {
    due: Instant,
    req: TxReq,
    /// Priority, fixed when queued (`maarse_core::qos`).
    prio: u8,
}

/// Packets on the bulletin channel jump the queue, but only this many per
/// minute: the channel key is public, so anyone can post junk there, and a
/// flood of it must not push everything else aside. Real alerts are a few
/// packets; the rest goes at normal channel priority.
const BOOSTS_PER_MIN: u8 = 8;

struct BoostQuota {
    window: Instant,
    used: u8,
}

impl BoostQuota {
    fn take(&mut self) -> bool {
        let now = Instant::now();
        if now >= self.window + Duration::from_secs(60) {
            self.window = now;
            self.used = 0;
        }
        self.used < BOOSTS_PER_MIN && {
            self.used += 1;
            true
        }
    }
}

/// Small xorshift for backoff (does not need to be cryptographic).
struct Jitter(u64);
impl Jitter {
    fn next(&mut self, max: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % max.max(1)
    }
}

fn bandwidth(hz: u32) -> Bandwidth {
    match hz {
        7800 => Bandwidth::_7KHz,
        10400 => Bandwidth::_10KHz,
        15600 => Bandwidth::_15KHz,
        20800 => Bandwidth::_20KHz,
        31250 => Bandwidth::_31KHz,
        41700 => Bandwidth::_41KHz,
        62500 => Bandwidth::_62KHz,
        250_000 => Bandwidth::_250KHz,
        500_000 => Bandwidth::_500KHz,
        _ => Bandwidth::_125KHz,
    }
}

fn spreading(sf: u8) -> SpreadingFactor {
    match sf {
        5 => SpreadingFactor::_5,
        6 => SpreadingFactor::_6,
        7 => SpreadingFactor::_7,
        8 => SpreadingFactor::_8,
        10 => SpreadingFactor::_10,
        11 => SpreadingFactor::_11,
        12 => SpreadingFactor::_12,
        _ => SpreadingFactor::_9,
    }
}

fn coding(cr: u8) -> CodingRate {
    match cr {
        6 => CodingRate::_4_6,
        7 => CodingRate::_4_7,
        8 => CodingRate::_4_8,
        _ => CodingRate::_4_5,
    }
}

struct Params {
    cfg: RadioConfig,
    mdltn: ModulationParams,
    tx: PacketParams,
    rx: PacketParams,
}

fn params<RK: RadioKind, D: DelayNs>(lora: &mut LoRa<RK, D>, cfg: RadioConfig) -> Option<Params> {
    let mdltn = lora
        .create_modulation_params(
            spreading(cfg.sf),
            bandwidth(cfg.bw_hz),
            coding(cfg.cr),
            cfg.freq_hz,
        )
        .ok()?;
    let tx = lora
        .create_tx_packet_params(cfg.preamble, false, true, false, &mdltn)
        .ok()?;
    let rx = lora
        .create_rx_packet_params(cfg.preamble, false, 255, true, false, &mdltn)
        .ok()?;
    Some(Params { cfg, mdltn, tx, rx })
}

/// The radio's main loop. Called from a board-specific task.
/// `led` is lit during transmission.
pub async fn radio_loop<RK: RadioKind, D: DelayNs>(
    lora: &mut LoRa<RK, D>,
    mut led: impl FnMut(bool),
    seed: u64,
    cfg: RadioConfig,
) {
    let Some(mut p) = params(lora, cfg).or_else(|| params(lora, RadioConfig::EU_DEFAULT)) else {
        return;
    };
    let mut duty = DutyCycle::new(p.cfg.duty_permille as u32, Instant::now().as_millis());
    let mut jitter = Jitter(seed | 1);
    let mut queue: heapless::Vec<Planned, QUEUE> = heapless::Vec::new();
    // Signed bulletins go before other channel traffic (see maarse_core::qos).
    let urgent = maarse_crypto::bulletin::channel();
    let mut boost = BoostQuota { window: Instant::now(), used: 0 };
    let mut buf = [0u8; 255];

    loop {
        RSTATS.loops.fetch_add(1, Relaxed);
        while let Ok(id) = RADIO_CANCEL.try_receive() {
            queue.retain(|q| q.req.id != id);
        }
        while let Ok(req) = RADIO_TX.try_receive() {
            plan(&mut queue, req, &urgent, &mut boost);
        }
        RSTATS.tx_queue.store(queue.len() as u32, Relaxed);
        RSTATS
            .duty_used_permille
            .store(duty.used_permille(Instant::now().as_millis()), Relaxed);

        // Is there anything to send now?
        let now = Instant::now();
        // Of the packets that are due, the most urgent goes first (acks before
        // data, data before channel traffic, floods last), then the oldest.
        if let Some(i) = queue
            .iter()
            .enumerate()
            .filter(|(_, q)| q.due <= now)
            .min_by_key(|(_, q)| (q.prio, q.due))
            .map(|(i, _)| i)
        {
            let q = queue.swap_remove(i);
            stage(1);
            let _ = lora.enter_standby().await;
            // A job may ask for another spreading factor (range test beacons):
            // listen-before-talk and transmission then use that SF.
            let mut alt = match q.req.sf {
                0 => None,
                sf if sf == p.cfg.sf => None,
                sf => params(lora, RadioConfig { sf, ..p.cfg }),
            };
            let (tx_cfg, mdltn, txp) = match alt.as_mut() {
                Some(a) => (a.cfg, &a.mdltn, &mut a.tx),
                None => (p.cfg, &p.mdltn, &mut p.tx),
            };
            stage(2);
            // Listen-before-talk: if the channel is busy, try a little later.
            let busy = match lora.prepare_for_cad(mdltn).await {
                Ok(()) => {
                    stage(3);
                    lora.cad(mdltn).await.unwrap_or(false)
                }
                Err(_) => false,
            };
            let now_ms = Instant::now().as_millis();
            let air = tx_cfg.airtime().airtime_us(q.req.packet.len());
            if busy {
                RSTATS.cad_busy.fetch_add(1, Relaxed);
                let back = 100 + jitter.next(500);
                let _ = queue.push(Planned {
                    due: Instant::now() + Duration::from_millis(back),
                    req: q.req,
                    prio: q.prio,
                });
            } else if !duty.try_consume(now_ms, air) {
                RSTATS.duty_deferred.fetch_add(1, Relaxed);
                let wait = duty.wait_ms(now_ms, air).min(600_000);
                let _ = queue.push(Planned {
                    due: Instant::now() + Duration::from_millis(wait),
                    req: q.req,
                    prio: q.prio,
                });
            } else {
                led(true);
                stage(4);
                let ok = lora
                    .prepare_for_tx(
                        mdltn,
                        txp,
                        tx_cfg.tx_power_dbm as i32,
                        &q.req.packet,
                    )
                    .await
                    .is_ok()
                    && {
                        stage(5);
                        lora.tx().await.is_ok()
                    };
                led(false);
                if ok {
                    RSTATS.tx_ok.fetch_add(1, Relaxed);
                    RSTATS
                        .bytes_tx
                        .fetch_add(q.req.packet.len() as u32, Relaxed);
                    RSTATS.airtime_ms.fetch_add(air.div_ceil(1000), Relaxed);
                    if q.req.id & 0xFFFF_0000_0000_0000 == maarse_core::rangetest::TX_ID_TAG {
                        crate::bus::BEACON_SENT.store((q.req.id as u32).wrapping_add(1), Relaxed);
                    }
                } else {
                    RSTATS.tx_err.fetch_add(1, Relaxed);
                }
            }
            continue;
        }

        // Listen until a packet arrives, a new job, new settings
        // or the next planned time.
        let next = queue
            .iter()
            .map(|q| q.due)
            .min()
            .unwrap_or(now + Duration::from_secs(30));
        stage(6);
        if lora
            .prepare_for_rx(RxMode::Continuous, &p.mdltn, &p.rx)
            .await
            .is_err()
        {
            Timer::after_millis(100).await;
            continue;
        }
        stage(7);
        match select6(
            lora.rx(&p.rx, &mut buf),
            RADIO_TX.receive(),
            RADIO_CFG.wait(),
            Timer::at(next),
            RADIO_SLEEP.wait(),
            SWEEP.wait(),
        )
        .await
        {
            Either6::First(Ok((len, status))) => {
                let len = len as usize;
                RSTATS.received(len, status.rssi as i32, status.snr as i32);
                match Pkt::from_slice(&buf[..len]) {
                    Ok(pkt) => {
                        let snr = status.snr.clamp(-30, 30) as i8;
                        if RADIO_RX.try_send((pkt, snr)).is_err() {
                            RSTATS.dropped.fetch_add(1, Relaxed);
                        }
                    }
                    Err(_) => {
                        RSTATS.dropped.fetch_add(1, Relaxed);
                    }
                }
            }
            Either6::First(Err(_)) => {
                RSTATS.rx_err.fetch_add(1, Relaxed);
            }
            Either6::Second(req) => plan(&mut queue, req, &urgent, &mut boost),
            Either6::Third(cfg) => {
                let _ = lora.enter_standby().await;
                if let Some(np) = params(lora, cfg) {
                    // Keep the airtime already used unless the limit itself
                    // changed: the range test receiver switches SF often, and
                    // a fresh budget on every switch would defeat the limiter.
                    if np.cfg.duty_permille != p.cfg.duty_permille {
                        duty = DutyCycle::new(np.cfg.duty_permille as u32, Instant::now().as_millis());
                    }
                    p = np;
                }
            }
            Either6::Fourth(()) => {}
            Either6::Fifth(true) => {
                // A sensor node sleeps: nothing is sent or heard until woken.
                queue.clear();
                let _ = lora.sleep(true).await;
                RADIO_ASLEEP.signal(());
                loop {
                    match select(RADIO_SLEEP.wait(), RADIO_TX.receive()).await {
                        Either::First(false) => break,
                        // Anything asked to be sent while asleep is dropped.
                        _ => {}
                    }
                }
            }
            Either6::Fifth(false) => {}
            Either6::Sixth(req) => sweep(lora, &p, req).await,
        }
    }
}

/// How long the radio listens on each step before it reads the strength (µs).
/// Measured on the tag (RAK4630, SX1262): while the receiver stays on and only the frequency moves, 1 ms
/// after each step gives every value (300 µs loses the first); a sweep of 64 steps takes about 100 ms. Setting
/// the receiver up again on every step took 835 ms per sweep and needed 2 ms.
const SWEEP_SETTLE_US: u32 = 1000;
/// A run of sweeps ends by itself after this long, so messages are not missed by accident.
const SWEEP_MAX: Duration = Duration::from_secs(120);

/// A spectrum sweep: measures the strength on `count` frequencies, `sweeps` times, and sends each row as a
/// `Sweep` event. A new request while it runs replaces it (`sweeps` 0 stops it). The radio hears no packets meanwhile.
async fn sweep<RK: RadioKind, D: DelayNs>(lora: &mut LoRa<RK, D>, p: &Params, first: SweepReq) {
    let mut req = first;
    let began = Instant::now();
    let mut seq: u16 = 0;
    'run: while req.sweeps > 0 && req.count > 0 && Instant::now() < began + SWEEP_MAX {
        if lora.prepare_for_rx(RxMode::Continuous, &p.mdltn, &p.rx).await.is_err() {
            break;
        }
        let mut left = req.sweeps;
        while left > 0 {
            let mut row: maarse_core::companion::Text = maarse_core::companion::Text::new();
            for i in 0..req.count.min(128) as u32 {
                let khz = req.start_khz.saturating_add(i * req.step_khz as u32);
                // The first step sets the receiver up (longer wait); the others only move the frequency.
                let settle = if i == 0 { 2000 } else { SWEEP_SETTLE_US };
                let v = lora.sweep_step_fast(khz.saturating_mul(1000), settle, i == 0).await.unwrap_or(255);
                let _ = row.push(v);
            }
            crate::node::emit(&maarse_core::companion::Event::Sweep { seq, start_khz: req.start_khz, step_khz: req.step_khz, rssi: row });
            seq = seq.wrapping_add(1);
            if left != 255 {
                left -= 1;
            }
            // A new request replaces this one; time is up after SWEEP_MAX.
            if let Some(next) = SWEEP.try_take() {
                req = next;
                continue 'run;
            }
            if Instant::now() >= began + SWEEP_MAX {
                break 'run;
            }
            // Let the link carry the row before the next one, so rows are not dropped on the way
            // (Bluetooth moves about 20 bytes per notification); at most 150 ms.
            let wait_until = Instant::now() + Duration::from_millis(150);
            let usb_recent = {
                let t = crate::bus::USB_LAST_RX.load(Relaxed);
                t != 0 && (Instant::now().as_secs() as u32).saturating_sub(t) < 30
            };
            while ((crate::bus::BLE_CONNECTED.load(Relaxed) && crate::bus::BLE_OUT.len() > 3) || (usb_recent && crate::bus::USB_OUT.len() > 3))
                && Instant::now() < wait_until
            {
                Timer::after_millis(2).await;
            }
            Timer::after_millis(1).await;
        }
        break;
    }
    let _ = lora.enter_standby().await;
}

/// Debugging: 1 standby, 2 CAD preparation, 3 CAD, 4 TX preparation,
/// 5 TX, 6 RX preparation, 7 listening.
fn stage(n: u32) {
    RSTATS.stage.store(n, Relaxed);
}

fn plan(queue: &mut heapless::Vec<Planned, QUEUE>, req: TxReq, urgent: &maarse_core::Address, boost: &mut BoostQuota) {
    let plain = maarse_core::qos::priority(&req.packet);
    let mut prio = maarse_core::qos::priority_with(&req.packet, urgent);
    if prio != plain && !boost.take() {
        prio = plain;
    }
    // The same packet already waiting (a retransmission while the first copy
    // is still queued, e.g. behind the duty cycle): one copy is enough. Each
    // extra copy delayed the queue further and made the previous hop
    // retransmit again (maarse-sim: thousands of copies of one message on a
    // repeater that was out of duty cycle).
    if req.id != 0 && queue.iter().any(|q| q.req.id == req.id && q.req.packet == req.packet) {
        RSTATS.dropped.fetch_add(1, Relaxed);
        return;
    }
    let due = Instant::now() + Duration::from_millis(req.delay_ms as u64);
    if queue.is_full() {
        // Full queue: drop the least urgent packet, the latest of those. If the
        // new packet is less urgent than everything queued, drop it instead.
        RSTATS.dropped.fetch_add(1, Relaxed);
        let key = |prio: u8, due: Instant| (prio, due);
        let worst = queue
            .iter()
            .enumerate()
            .max_by_key(|(_, q)| key(q.prio, q.due))
            .map(|(i, q)| (i, key(q.prio, q.due)));
        match worst {
            Some((i, k)) if k >= key(prio, due) => {
                queue.swap_remove(i);
            }
            _ => return,
        }
    }
    let _ = queue.push(Planned { due, req, prio });
}
