//! Diagnose (trace route, naboer), deling af position/telemetri og
//! MQTT-broen. Holdt ude af node.rs, så hovedfilen kun har få kroge.

use super::{Node, token};
use crate::ClientId;
use crate::mqtt::Mqtt;
use maarse_core::Address;
use maarse_core::appdata::{self, Telemetry};
use maarse_core::companion::{Command, Event, Sharing, error_code, neighbor_events, stat};
use maarse_core::diag;
use std::collections::HashMap;
use std::fs;

#[derive(Default)]
pub(super) struct Ext {
    /// Igangværende trace routes: mål -> (klient, req).
    traces: HashMap<Address, (ClientId, u16)>,
    pub(super) sharing: Sharing,
    pub(super) mqtt: Option<Mqtt>,
}

impl Ext {
    pub(super) fn load(data_dir: &std::path::Path, mqtt: Option<Mqtt>) -> Self {
        let s = fs::read_to_string(data_dir.join("deling.txt")).unwrap_or_default();
        let v: Vec<i64> = s.split_whitespace().filter_map(|x| x.parse().ok()).collect();
        let sharing = match v[..] {
            [p, lat, lon, t] => Sharing {
                precision: (p as u8).min(3),
                lat_e7: lat as i32,
                lon_e7: lon as i32,
                telemetry: t != 0,
            },
            _ => Sharing::default(),
        };
        Ext { traces: HashMap::new(), sharing, mqtt }
    }
}

impl Node {
    /// App-data til vores announce (postkasse, FS, evt. position og telemetri).
    pub(super) fn app_data(&self) -> Vec<u8> {
        let s = &self.ext.sharing;
        let tel = s.telemetry.then(|| Telemetry {
            battery_mv: 0,
            ext_power: true,
            energy: maarse_core::Energy::Full as u8,
            uptime_s: self.start.elapsed().as_secs() as u32,
            temp_dc: None,
        });
        appdata::build(self.my_postbox.as_ref(), true, s.position(), tel.as_ref()).to_vec()
    }

    /// Kommandoer til diagnose og deling. Returnerer kommandoen, hvis den
    /// ikke hører til her.
    pub(super) fn ext_command(&mut self, id: ClientId, cmd: Command) -> Option<Command> {
        match cmd {
            Command::TraceRoute { req, dest } => {
                if dest == self.addr {
                    self.send_client(id, &super::err_ev(req, error_code::UNKNOWN_DEST, "Det er os selv"));
                    return None;
                }
                self.ext.traces.insert(dest, (id, req));
                let now = self.now_ms();
                let actions = self.router.send_trace(dest, token(), now);
                self.apply(actions.into_iter().collect());
                self.send_client(id, &Event::Sent { req, msg_id: 0 });
            }
            Command::GetNeighbors => {
                for ev in neighbor_events(&self.router.neighbors, self.now_ms()) {
                    self.send_client(id, &ev);
                }
            }
            Command::GetSharing => self.send_client(id, &Event::Sharing(self.ext.sharing)),
            Command::SetSharing(s) => {
                self.ext.sharing = s;
                let _ = fs::write(
                    self.data_dir.join("deling.txt"),
                    format!("{} {} {} {}\n", s.precision, s.lat_e7, s.lon_e7, s.telemetry as u8),
                );
                self.broadcast(&Event::Sharing(s));
                // Det nye vises i næste announce; send et nu.
                self.announce();
            }
            other => return Some(other),
        }
        None
    }

    /// Svar på en trace route.
    pub(super) fn on_trace(&mut self, dest: Address, payload: &[u8]) {
        let Some(r) = diag::parse_response(payload) else { return };
        let Some((client, req)) = self.ext.traces.remove(&dest) else { return };
        let ev = Event::TraceResult {
            req,
            dest,
            path: r.path.iter().copied().collect(),
            out: r.out.iter().copied().collect(),
            back: r.back.iter().copied().collect(),
        };
        self.send_client(client, &ev);
    }

    /// Position og telemetri fra et verificeret announce.
    pub(super) fn on_app_data(&mut self, from: Address, hops: u8, app_data: &[u8]) {
        let a = appdata::parse(app_data);
        if a.position.is_none() && a.telemetry.is_none() {
            return;
        }
        self.broadcast(&Event::NodeInfo { addr: from, hops, position: a.position, telemetry: a.telemetry });
        if let Some(m) = self.ext.mqtt.as_mut() {
            if let Some(t) = &a.telemetry {
                m.telemetry(&from, hops, t);
            }
            if let Some(p) = &a.position {
                m.position(&from, hops, p);
            }
        }
    }

    pub(super) fn mqtt_channel(&self, channel: &Address, from: &Address, hops: u8, text: &[u8]) {
        if let Some(m) = &self.ext.mqtt {
            let name = self.channels.get(channel).map_or("?", |(n, _)| n.as_str());
            m.channel(name, from, hops, &String::from_utf8_lossy(text));
        }
    }

    pub(super) fn ext_stats(&self, items: &mut Vec<(u8, i32)>) {
        items.push((stat::NEIGHBORS, self.router.neighbors.len() as i32));
        items.push((stat::POSITION_PRECISION, self.ext.sharing.precision as i32));
        items.push((stat::ENERGY, self.router.energy as i32));
        items.push((stat::EXT_POWER, 1));
    }
}
