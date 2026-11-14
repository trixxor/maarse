//! Diagnose (trace route, naboer, støjgulv), batteri/energistyret routing
//! og deling af position/telemetri. Holdt ude af node.rs, så hovedfilen
//! kun har få kroge.

use super::{FwNode, Settings, emit, err, log, now_ms};
use crate::power;
use core::sync::atomic::Ordering::Relaxed;
use embassy_time::{Duration, Instant};
use maarse_core::appdata::{self, AppBuf, Telemetry};
use maarse_core::companion::{Command, Event, Sharing, Stats, error_code, neighbor_events, stat};
use maarse_core::{Address, Energy, battery, diag};
use rand_core::{CryptoRng, RngCore};

/// Så ofte opdateres energistatus ud fra batterimålingen.
const POWER_EVERY: Duration = Duration::from_secs(power::SAMPLE_EVERY_S);

pub(super) struct Ext {
    pub(super) sharing: Sharing,
    /// Igangværende trace routes: mål -> req.
    traces: heapless::Vec<(Address, u16), 4>,
    pub(super) next_power: Instant,
}

impl Ext {
    pub(super) fn new() -> Self {
        Ext {
            sharing: Sharing::default(),
            traces: heapless::Vec::new(),
            next_power: Instant::now() + Duration::from_secs(5),
        }
    }
}

impl<R: RngCore + CryptoRng, S: Settings> FwNode<R, S> {
    pub(super) fn app_data(&self) -> AppBuf {
        let s = &self.ext.sharing;
        let tel = s.telemetry.then(|| Telemetry {
            battery_mv: power::BATTERY_MV.load(Relaxed).min(u16::MAX as u32) as u16,
            ext_power: power::EXT_POWER.load(Relaxed) == 1,
            energy: self.router.energy as u8,
            uptime_s: self.started.elapsed().as_secs() as u32,
            // nRF52840's TEMP-enhed ejes af MPSL (Bluetooth); ingen temperatur.
            temp_dc: None,
        });
        appdata::build(None, true, s.position(), tel.as_ref())
    }

    /// Opdaterer energistatus ud fra batteriet (kaldes fra `on_timer`).
    pub(super) fn power_tick(&mut self) {
        if Instant::now() < self.ext.next_power {
            return;
        }
        self.ext.next_power = Instant::now() + POWER_EVERY;
        let mv = power::BATTERY_MV.load(Relaxed).min(u16::MAX as u32) as u16;
        let ext = power::EXT_POWER.load(Relaxed) == 1;
        let old = self.router.energy;
        let new = battery::energy_for(mv, ext, old);
        if new != old {
            self.router.energy = new;
            log(match new {
                Energy::Full => "Energi: fuld",
                Energy::Ok => "Energi: ok",
                Energy::Low => "Energi: lav - videresender kun direkte beskeder",
                Energy::Critical => "Energi: kritisk - videresender intet",
            });
        }
    }

    /// Kommandoer til diagnose og deling. Returnerer kommandoen, hvis den
    /// ikke hører til her.
    pub(super) fn ext_command(&mut self, cmd: Command) -> Option<Command> {
        match cmd {
            Command::TraceRoute { req, dest } => {
                if dest == self.addr {
                    err(req, error_code::UNKNOWN_DEST, "Det er os selv");
                    return None;
                }
                if self.ext.traces.is_full() {
                    self.ext.traces.remove(0);
                }
                self.ext.traces.retain(|t| t.0 != dest);
                let _ = self.ext.traces.push((dest, req));
                let mut token = [0u8; 16];
                self.rng.fill_bytes(&mut token);
                let actions = self.router.send_trace(dest, token, now_ms());
                self.apply(actions);
                emit(&Event::Sent { req, msg_id: 0 });
            }
            Command::GetNeighbors => {
                for ev in neighbor_events(&self.router.neighbors, now_ms()) {
                    emit(&ev);
                }
            }
            Command::GetSharing => emit(&Event::Sharing(self.ext.sharing)),
            Command::SetSharing(s) => {
                self.ext.sharing = s;
                self.settings.save_sharing(&s);
                emit(&Event::Sharing(s));
                log("Deling gemt; vises i næste announce");
            }
            other => return Some(other),
        }
        None
    }

    pub(super) fn on_trace(&mut self, dest: Address, payload: &[u8]) {
        let Some(r) = diag::parse_response(payload) else { return };
        let Some(i) = self.ext.traces.iter().position(|t| t.0 == dest) else { return };
        let (_, req) = self.ext.traces.remove(i);
        emit(&Event::TraceResult {
            req,
            dest,
            path: r.path.iter().copied().collect(),
            out: r.out.iter().copied().collect(),
            back: r.back.iter().copied().collect(),
        });
    }

    pub(super) fn on_app_data(&mut self, from: Address, hops: u8, app_data: &[u8]) {
        let a = appdata::parse(app_data);
        if a.position.is_some() || a.telemetry.is_some() {
            emit(&Event::NodeInfo { addr: from, hops, position: a.position, telemetry: a.telemetry });
        }
    }

    pub(super) fn ext_stats(&self, items: &mut Stats) {
        let mut put = |k: u8, v: i32| {
            let _ = items.push((k, v));
        };
        let mv = power::BATTERY_MV.load(Relaxed);
        if mv > 0 {
            put(stat::BATTERY_MV, mv as i32);
            put(stat::BATTERY_PERCENT, battery::percent(mv.min(u16::MAX as u32) as u16) as i32);
        }
        put(stat::EXT_POWER, power::EXT_POWER.load(Relaxed));
        put(stat::ENERGY, self.router.energy as i32);
        if let Some(n) = power::noise_floor() {
            put(stat::NOISE_FLOOR_DBM, n);
        }
        put(stat::NEIGHBORS, self.router.neighbors.len() as i32);
        put(stat::POSITION_PRECISION, self.ext.sharing.precision as i32);
    }
}
