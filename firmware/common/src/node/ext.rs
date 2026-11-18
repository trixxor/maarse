//! Diagnostics (trace route, neighbours, noise floor), battery/energy-driven routing
//! and sharing of position/telemetry. Kept out of node.rs, so the main file
//! only has a few hooks.

use super::{FwNode, Settings, emit, err, log, now_ms};
use crate::power;
use core::sync::atomic::Ordering::Relaxed;
use embassy_time::{Duration, Instant};
use maarse_core::appdata::{self, AppBuf, Telemetry};
use maarse_core::companion::{Command, Event, Sharing, Stats, error_code, neighbor_events, stat, text};
use maarse_core::{Address, Energy, battery, diag};
use rand_core::{CryptoRng, RngCore};

/// How often the energy status is updated from the battery measurement.
const POWER_EVERY: Duration = Duration::from_secs(power::SAMPLE_EVERY_S);

pub(super) struct Ext {
    pub(super) sharing: Sharing,
    /// Our display name (empty = none).
    pub(super) name: appdata::Name,
    /// Trace routes in progress: target -> req.
    traces: heapless::Vec<(Address, u16), 4>,
    pub(super) next_power: Instant,
}

impl Ext {
    pub(super) fn new() -> Self {
        Ext {
            sharing: Sharing::default(),
            name: appdata::Name::new(),
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
            temp_dc: power::temperature_dc(),
        });
        let name = (!self.ext.name.is_empty()).then_some(&self.ext.name[..]);
        // Name our home post office, so senders' offices can find our mail.
        let home = self.home_po.and_then(|h| h.1);
        let mut v = appdata::build(home.as_ref(), true, s.position(), tel.as_ref(), name);
        let flags = if self.router.hears_repeater(now_ms()) { appdata::ROLE_HEARS_REPEATER } else { 0 }
            | if self.router.mobile { appdata::ROLE_MOBILE } else { 0 };
        appdata::push_role_bridge(&mut v, self.router.role.as_u8(), flags, self.router.chosen_bridge(now_ms()));
        appdata::push_caps(&mut v, appdata::MY_CAPS);
        v
    }

    /// Updates the energy status from the battery (called from `on_timer`).
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
                Energy::Full => "Energy: full",
                Energy::Ok => "Energy: ok",
                Energy::Low => "Energy: low - only forwarding direct messages",
                Energy::Critical => "Energy: critical - forwarding nothing",
            });
        }
    }

    /// Commands for diagnostics and sharing. Returns the command if it
    /// does not belong here.
    pub(super) fn ext_command(&mut self, cmd: Command) -> Option<Command> {
        match cmd {
            Command::TraceRoute { req, dest } => {
                if dest == self.addr {
                    err(req, error_code::UNKNOWN_DEST, "That is ourselves");
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
                log("Sharing saved; shown in the next announce");
            }
            Command::GetName => emit(&Event::Name { name: text(&self.ext.name) }),
            Command::SetName { name } => {
                self.ext.name = appdata::clean_name(&name).unwrap_or_default();
                self.settings.save_name(&self.ext.name);
                emit(&Event::Name { name: text(&self.ext.name) });
                log("Name saved; shown in the next announce");
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
        let role = a.role.and_then(maarse_core::router::Role::from_u8);
        let hears = a.role_flags.map(|f| f & appdata::ROLE_HEARS_REPEATER != 0);
        if hops == 0
            && let Some(role) = role
        {
            self.router.note_role(from.path_id(), role, hears, 0, 0, now_ms());
            self.router.neighbors.set_bridge_choice(from.path_id(), a.role_bridge);
        }
        if let Some(f) = a.role_flags {
            self.router.note_mobile(from, f & appdata::ROLE_MOBILE != 0, now_ms());
        }
        if let Some(c) = a.caps {
            if self.caps.insert(from, c).is_err() {
                if let Some(k) = self.caps.keys().next().copied() {
                    self.caps.remove(&k);
                }
                let _ = self.caps.insert(from, c);
            }
        }
        if let Some(h) = hears {
            // Who has no coverage (boundary mode), and which edge bridges
            // are around (for our own choice of bridge).
            self.router.note_coverage(from, h);
            if h && role == Some(maarse_core::router::Role::Relay) {
                self.router.note_bridge(from.path_id(), hops, now_ms());
            }
        }
        let pages = a.caps.is_some_and(|c| c & maarse_core::appdata::CAP_PAGES != 0);
        if a.position.is_some() || a.telemetry.is_some() || a.name.is_some() || pages {
            emit(&Event::NodeInfo { addr: from, hops, position: a.position, telemetry: a.telemetry, name: a.name, caps: a.caps });
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
        for (k, a) in [
            (stat::STACK_FREE, &crate::bus::STACK_FREE),
            (stat::HEAP_FREE, &crate::bus::HEAP_FREE),
            (stat::HEAP_MIN_FREE, &crate::bus::HEAP_MIN_FREE),
            (stat::CRASHES, &crate::bus::CRASHES),
            (stat::PANIC_LINE, &crate::bus::PANIC_LINE),
            (stat::PANIC_FILE, &crate::bus::PANIC_FILE),
        ] {
            let v = a.load(Relaxed);
            if v > 0 {
                put(k, v as i32);
            }
        }
        if let Some(t) = power::temperature_dc() {
            put(stat::TEMPERATURE_DC, t as i32);
        }
        if let Some(n) = power::noise_floor() {
            put(stat::NOISE_FLOOR_DBM, n);
        }
        put(stat::NEIGHBORS, self.router.neighbors.len() as i32);
        put(stat::POSITION_PRECISION, self.ext.sharing.precision as i32);
    }
}
