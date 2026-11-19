//! Diagnostics (trace route, neighbours), sharing of position/telemetry and
//! the MQTT bridge. Kept out of node.rs so the main file only has a few hooks.

use super::{Node, token};
use crate::ClientId;
use crate::mqtt::Mqtt;
use maarse_core::Address;
use maarse_core::appdata::{self, Telemetry};
use maarse_core::companion::{Command, Event, Sharing, error_code, neighbor_events, stat, text};
use maarse_core::diag;
use std::collections::HashMap;
use std::fs;

#[derive(Default)]
pub(super) struct Ext {
    /// Trace routes in progress: target -> (client, req).
    traces: HashMap<Address, (ClientId, u16)>,
    pub(super) sharing: Sharing,
    pub(super) mqtt: Option<Mqtt>,
    /// Our display name (empty = none), stored in `name.txt`.
    pub(super) name: String,
    /// Display names heard in others' announces. Shown only; not proven.
    pub(super) names: HashMap<Address, (appdata::Name, u8)>,
    /// Our site (antenna height, gain, power, owner), stored in `site.txt`;
    /// sent in announces while the height is set.
    pub(super) site: appdata::Site,
}

/// `site.txt`: "height_dm gain_hdb tx_dbm" on the first line, the owner on the second.
fn load_site(data_dir: &std::path::Path) -> appdata::Site {
    let s = fs::read_to_string(data_dir.join("site.txt")).unwrap_or_default();
    let mut lines = s.lines();
    let v: Vec<i64> = lines.next().unwrap_or("").split_whitespace().filter_map(|x| x.parse().ok()).collect();
    match v[..] {
        [h, g, t] => appdata::Site {
            height_dm: h.clamp(0, u16::MAX as i64) as u16,
            gain_hdb: g.clamp(-128, 127) as i8,
            tx_dbm: t.clamp(-128, 127) as i8,
            owner: appdata::clean_owner(lines.next().unwrap_or("").as_bytes()),
        },
        _ => appdata::Site::default(),
    }
}

impl Ext {
    pub(super) fn load(data_dir: &std::path::Path, mqtt: Option<Mqtt>) -> Self {
        let s = fs::read_to_string(data_dir.join("sharing.txt")).unwrap_or_default();
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
        let name = fs::read_to_string(data_dir.join("name.txt"))
            .ok()
            .and_then(|s| appdata::clean_name(s.as_bytes()))
            .map(|n| String::from_utf8_lossy(&n).into_owned())
            .unwrap_or_default();
        Ext { traces: HashMap::new(), sharing, mqtt, name, names: HashMap::new(), site: load_site(data_dir) }
    }
}

impl Node {
    /// App data for our announce (mailbox, FS, optionally position and telemetry).
    pub(super) fn app_data(&self) -> Vec<u8> {
        let s = &self.ext.sharing;
        let tel = s.telemetry.then(|| Telemetry {
            battery_mv: 0,
            ext_power: true,
            energy: maarse_core::Energy::Full as u8,
            uptime_s: self.start.elapsed().as_secs() as u32,
            temp_dc: None,
        });
        let name = (!self.ext.name.is_empty()).then_some(self.ext.name.as_bytes());
        let mut v = appdata::build(self.my_postbox.as_ref(), true, s.position(), tel.as_ref(), name);
        let flags = if self.router.hears_repeater(self.now_ms()) { appdata::ROLE_HEARS_REPEATER } else { 0 };
        appdata::push_role(&mut v, self.router.role.as_u8(), flags);
        // Pages: announced on every announce while pages/index.mu exists.
        let pages = if !self.pages.off && super::pages::find_page(&self.data_dir.join("pages"), "index").is_some() { appdata::CAP_PAGES } else { 0 };
        appdata::push_caps(&mut v, appdata::MY_CAPS | appdata::CAP_SEQ | appdata::CAP_CALL_DG | appdata::CAP_FILE_PULL | pages);
        self.person_app_data(&mut v);
        appdata::push_site(&mut v, &self.ext.site);
        v.to_vec()
    }

    /// Commands for diagnostics and sharing. Returns the command if it
    /// does not belong here.
    pub(super) fn ext_command(&mut self, id: ClientId, cmd: Command) -> Option<Command> {
        match cmd {
            Command::TraceRoute { req, dest } => {
                if dest == self.addr {
                    self.send_client(id, &super::err_ev(req, error_code::UNKNOWN_DEST, "That is ourselves"));
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
                    self.data_dir.join("sharing.txt"),
                    format!("{} {} {} {}\n", s.precision, s.lat_e7, s.lon_e7, s.telemetry as u8),
                );
                self.broadcast(&Event::Sharing(s));
                // The change shows in the next announce; send one now.
                self.announce();
            }
            Command::GetName => self.send_client(id, &Event::Name { name: text(self.ext.name.as_bytes()) }),
            Command::GetSite => self.send_client(id, &Event::Site(self.ext.site.clone())),
            Command::SetSite(site) => {
                self.ext.site = site;
                let s = &self.ext.site;
                let _ = fs::write(
                    self.data_dir.join("site.txt"),
                    format!("{} {} {}
{}
", s.height_dm, s.gain_hdb, s.tx_dbm, String::from_utf8_lossy(&s.owner)),
                );
                self.broadcast(&Event::Site(self.ext.site.clone()));
                self.announce();
            }
            Command::SetName { name } => {
                self.ext.name = appdata::clean_name(&name)
                    .map(|n| String::from_utf8_lossy(&n).into_owned())
                    .unwrap_or_default();
                let _ = fs::write(self.data_dir.join("name.txt"), format!("{}
", self.ext.name));
                self.broadcast(&Event::Name { name: text(self.ext.name.as_bytes()) });
                self.announce();
            }
            other => return Some(other),
        }
        None
    }

    /// Reply to a trace route.
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

    /// Position and telemetry from a verified announce.
    pub(super) fn on_app_data(&mut self, from: Address, hops: u8, app_data: &[u8]) {
        let a = appdata::parse(app_data);
        if let Some(c) = a.caps
            && self.caps.insert(from, c) != Some(c)
        {
            self.save_caps();
        }
        if hops == 0
            && let Some(role) = a.role.and_then(maarse_core::router::Role::from_u8)
        {
            let now = self.now_ms();
            let hears = a.role_flags.map(|f| f & appdata::ROLE_HEARS_REPEATER != 0);
            self.router.note_role(from.path_id(), role, hears, 0, 0, now);
        }
        match &a.name {
            Some(n) => {
                self.ext.names.insert(from, (n.clone(), hops));
            }
            None => {
                self.ext.names.remove(&from);
            }
        }
        let pages = a.caps.is_some_and(|c| c & appdata::CAP_PAGES != 0);
        if a.position.is_none() && a.telemetry.is_none() && a.name.is_none() && !pages {
            return;
        }
        self.broadcast(&Event::NodeInfo {
            addr: from,
            hops,
            position: a.position,
            telemetry: a.telemetry,
            name: a.name.clone(),
            caps: a.caps,
        });
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

    /// Known display names for a client that has just connected.
    pub(super) fn send_known_names(&self, id: ClientId) {
        for (addr, (n, hops)) in &self.ext.names {
            let ev = Event::NodeInfo { addr: *addr, hops: *hops, position: None, telemetry: None, name: Some(n.clone()), caps: None };
            self.send_client(id, &ev);
        }
    }
}
