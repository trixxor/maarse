//! LoRa-løkken (fælles for alle boards): modtager hele tiden og sender planlagte pakker med
//! listen-before-talk (CAD) og duty cycle-budget. Indstillingerne kan
//! ændres, mens den kører (`RADIO_CFG`).

use crate::bus::{Pkt, RADIO_CANCEL, RADIO_CFG, RADIO_RX, RADIO_TX, RSTATS, TxReq};
use core::sync::atomic::Ordering::Relaxed;
use embassy_futures::select::{Either4, select4};
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
}

/// Lille xorshift til backoff (behøver ikke være kryptografisk).
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

/// Radioens hovedløkke. Kaldes fra en board-specifik opgave.
/// `led` tændes under udsendelse.
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
    let mut buf = [0u8; 255];

    loop {
        RSTATS.loops.fetch_add(1, Relaxed);
        while let Ok(id) = RADIO_CANCEL.try_receive() {
            queue.retain(|q| q.req.id != id);
        }
        while let Ok(req) = RADIO_TX.try_receive() {
            plan(&mut queue, req);
        }
        RSTATS.tx_queue.store(queue.len() as u32, Relaxed);
        RSTATS
            .duty_used_permille
            .store(duty.used_permille(Instant::now().as_millis()), Relaxed);

        // Er der noget, der skal sendes nu?
        let now = Instant::now();
        if let Some(i) = queue.iter().position(|q| q.due <= now) {
            let q = queue.swap_remove(i);
            stage(1);
            let _ = lora.enter_standby().await;
            stage(2);
            // Listen-before-talk: er kanalen optaget, prøv lidt senere.
            let busy = match lora.prepare_for_cad(&p.mdltn).await {
                Ok(()) => {
                    stage(3);
                    lora.cad(&p.mdltn).await.unwrap_or(false)
                }
                Err(_) => false,
            };
            let now_ms = Instant::now().as_millis();
            let air = p.cfg.airtime().airtime_us(q.req.packet.len());
            if busy {
                RSTATS.cad_busy.fetch_add(1, Relaxed);
                let back = 100 + jitter.next(500);
                let _ = queue.push(Planned {
                    due: Instant::now() + Duration::from_millis(back),
                    req: q.req,
                });
            } else if !duty.try_consume(now_ms, air) {
                RSTATS.duty_deferred.fetch_add(1, Relaxed);
                let wait = duty.wait_ms(now_ms, air).min(600_000);
                let _ = queue.push(Planned {
                    due: Instant::now() + Duration::from_millis(wait),
                    req: q.req,
                });
            } else {
                led(true);
                stage(4);
                let ok = lora
                    .prepare_for_tx(
                        &p.mdltn,
                        &mut p.tx,
                        p.cfg.tx_power_dbm as i32,
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
                } else {
                    RSTATS.tx_err.fetch_add(1, Relaxed);
                }
            }
            continue;
        }

        // Lyt, indtil der kommer en pakke, en ny opgave, nye indstillinger
        // eller næste planlagte tid.
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
        match select4(
            lora.rx(&p.rx, &mut buf),
            RADIO_TX.receive(),
            RADIO_CFG.wait(),
            Timer::at(next),
        )
        .await
        {
            Either4::First(Ok((len, status))) => {
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
            Either4::First(Err(_)) => {
                RSTATS.rx_err.fetch_add(1, Relaxed);
            }
            Either4::Second(req) => plan(&mut queue, req),
            Either4::Third(cfg) => {
                let _ = lora.enter_standby().await;
                if let Some(np) = params(lora, cfg) {
                    p = np;
                    duty = DutyCycle::new(p.cfg.duty_permille as u32, Instant::now().as_millis());
                }
            }
            Either4::Fourth(()) => {}
        }
    }
}

/// Fejlsøgning: 1 standby, 2 CAD-forberedelse, 3 CAD, 4 TX-forberedelse,
/// 5 TX, 6 RX-forberedelse, 7 lytter.
fn stage(n: u32) {
    RSTATS.stage.store(n, Relaxed);
}

fn plan(queue: &mut heapless::Vec<Planned, QUEUE>, req: TxReq) {
    let due = Instant::now() + Duration::from_millis(req.delay_ms as u64);
    if queue.is_full() {
        // Fuld kø: drop den, der skal sendes senest.
        RSTATS.dropped.fetch_add(1, Relaxed);
        if let Some(i) = queue
            .iter()
            .enumerate()
            .max_by_key(|(_, q)| q.due)
            .map(|(i, _)| i)
        {
            queue.swap_remove(i);
        }
    }
    let _ = queue.push(Planned { due, req });
}
