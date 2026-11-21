//! Telemetry for maarse-server: every number that can be learned from the
//! traffic passing through, kept as totals, histograms, a 24-hour series per
//! minute and a census of every node ever heard. Saved to
//! `<data>/telemetry.json` every 5 minutes and loaded at start.
//!
//! What a relay can see (messages themselves stay end-to-end encrypted):
//! * every packet's type, size, hop count, destination, region and route
//!   (the repeaters' path IDs), and the link it came in on or went out on;
//! * announces in full: name, role, coverage, mobile flag, post office
//!   class, capabilities, position (if shared), battery, power, uptime and
//!   temperature (if shared);
//! * when a direct message passes and when its receipt comes back (the
//!   receipt's address is derived from the message), so delivery time and
//!   success as seen from here; the same for route searches and answers;
//! * the attached LoRa modem's own counters (radio, airtime, duty cycle,
//!   signal, noise floor, battery, memory), polled every 30 s;
//! * carried messages (store-carry-forward): each handoff's giver and taker
//!   (path IDs), tokens and lifetime left are in the clear, and the
//!   "delivered" notice is addressed to a token derived from the message, so
//!   the time from the first handoff seen here to the notice;
//! * this host's own post office: when mail was stored and when delivered;
//! * a journey per direct message: every time it (or a carried copy, its
//!   receipt, its mail entry) passed here, with link, hops and route. The
//!   sender stays unknown: it is inside the encrypted message.

use maarse_core::Address;
use maarse_core::appdata::{self, AppData};
use maarse_core::packet::{Header, PacketType};
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};

const MINUTES: usize = 24 * 60;
const TYPES: usize = 17; // 16 packet types + invalid
const MAX_NODES: usize = 20_000;
const MAX_DESTS: usize = 5_000;
const MAX_PENDING: usize = 4_000;
const SAMPLES: usize = 1_000;
/// Message journeys kept (the most recently active).
const MAX_JOURNEYS: usize = 500;
/// Steps kept per journey.
const MAX_STEPS: usize = 40;

/// One thing that happened to a message here.
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Step {
    /// Unix ms.
    pub t: u64,
    pub what: String,
    /// Link names (in or out).
    pub link: String,
    pub hops: u8,
    pub path: Vec<String>,
    pub note: String,
}

/// Everything seen here about one direct message (by its packet ID).
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Journey {
    pub dest: String,
    pub first: u64,
    pub last: u64,
    pub steps: Vec<Step>,
    /// Done: receipt, carry notice or mail delivery seen (unix ms).
    pub done: Option<u64>,
}

pub fn type_index(p: Option<PacketType>) -> usize {
    p.map_or(16, |t| t as usize)
}
pub fn type_name(i: usize) -> &'static str {
    const N: [&str; TYPES] = [
        "Data", "Ack", "Announce", "PathRequest", "PathResponse", "Control", "Fragment", "Probe", "Channel", "Deposit", "Datagram", "Carry",
        "Hello", "KeyRequest", "MailSync", "Summary", "invalid",
    ];
    N.get(i).copied().unwrap_or("?")
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Great-circle distance in km.
pub fn km(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (la1, lo1, la2, lo2) = (a.0.to_radians(), a.1.to_radians(), b.0.to_radians(), b.1.to_radians());
    let h = ((la2 - la1) / 2.0).sin().powi(2) + la1.cos() * la2.cos() * ((lo2 - lo1) / 2.0).sin().powi(2);
    2.0 * 6371.0 * h.sqrt().asin()
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Minute {
    pub t: u64,
    pub rx: u32,
    pub tx: u32,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub by_type: Vec<u32>,
    pub unique: u32,
    pub delivered: u32,
    pub forwarded: u32,
    pub data_seen: u32,
    pub acks_matched: u32,
    pub routes_found: u32,
    pub announces: u32,
    pub dht: u32,
    pub mail: u32,
    pub invalid: u32,
    pub snr_sum: i64,
    pub snr_n: u32,
    pub radio_rx: u32,
    pub backbone_rx: u32,
    pub nodes: u32,
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct NodeInfo {
    pub name: String,
    pub role: Option<String>,
    pub hears_repeater: Option<bool>,
    pub mobile: bool,
    pub mail_class: u8,
    pub caps: Option<u8>,
    pub bridge: Option<u16>,
    pub postbox: Option<String>,
    pub fs: bool,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub precision: u8,
    pub battery_mv: Option<u16>,
    pub ext_power: Option<bool>,
    pub energy: Option<u8>,
    pub uptime_s: Option<u32>,
    pub temp_c: Option<f32>,
    pub first_seen: u64,
    pub last_seen: u64,
    pub announces: u32,
    pub packets_to: u64,
    pub min_hops: u8,
    pub last_hops: u8,
    pub via: String,
    pub via_backbone: bool,
    pub snr: Option<i8>,
    pub path_id: u16,
    /// Site of a fixed node (from its announce): antenna height (m), gain
    /// (dBi), transmit power (dBm), owner.
    #[serde(default)]
    pub site_h: Option<f32>,
    #[serde(default)]
    pub site_gain: Option<f32>,
    #[serde(default)]
    pub site_tx: Option<i8>,
    #[serde(default)]
    pub owner: Option<String>,
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct DestStat {
    pub data: u64,
    pub bytes: u64,
    pub path_requests: u64,
    pub key_requests: u64,
    pub acks: u64,
    pub other: u64,
    pub last: u64,
    pub last_path: Vec<String>,
    pub last_hops: u8,
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct LinkStat {
    pub rx: Vec<u64>,
    pub tx: Vec<u64>,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub kind: String,
    pub first: u64,
    pub last: u64,
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct RepeaterStat {
    pub seen: u64,
    pub first_hop: u64,
    pub last: u64,
    pub positions: u64,
    pub addr: Option<String>,
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ModemStat {
    pub last: Vec<(u8, i32)>,
    pub last_t: u64,
    pub radio: String,
    pub series: VecDeque<Vec<i64>>,
}

/// Keys of the modem counters kept as a series (see companion::stat).
pub const MODEM_SERIES: [u8; 14] = [30, 31, 32, 33, 36, 37, 38, 39, 40, 41, 44, 49, 80, 88];

/// One page (or picture) this server shares: how often and how it was read.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct PageStat {
    pub served: u64,
    pub lora: u64,
    pub net: u64,
    pub pictures: u64,
    pub bytes: u64,
    pub notes: u64,
    /// Readers turned away by a lock.
    pub refused: u64,
    pub last: u64,
    /// Distinct readers (short addresses), at most 500.
    pub readers: Vec<String>,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct Telemetry {
    pub started: u64,
    pub minutes: VecDeque<Minute>,
    pub rx_by_type: Vec<u64>,
    pub tx_by_type: Vec<u64>,
    pub rx_bytes_by_type: Vec<u64>,
    pub hops_hist: Vec<u64>,
    pub size_hist: Vec<u64>,
    pub snr_hist: Vec<u64>,
    pub path_len_hist: Vec<u64>,
    pub region_counts: HashMap<u16, u64>,
    pub nodes: HashMap<String, NodeInfo>,
    pub dests: HashMap<String, DestStat>,
    pub links: HashMap<String, LinkStat>,
    pub repeaters: HashMap<u16, RepeaterStat>,
    pub modems: HashMap<String, ModemStat>,
    pub data_seen: u64,
    pub data_acked: u64,
    pub latency_ms: VecDeque<(u64, u32, u8)>,
    pub route_searches: u64,
    pub routes_found: u64,
    pub route_ms: VecDeque<(u64, u32, u8)>,
    pub router: HashMap<String, u64>,
    pub dht_ops: HashMap<String, u64>,
    pub mail_ops: HashMap<String, u64>,
    /// Carry: handoffs seen, distinct messages carried, delivered notices
    /// matched, and (unix s, ms from the first handoff seen to the notice,
    /// handoffs seen) per delivered message.
    #[serde(default)]
    pub carry_handoffs: u64,
    #[serde(default)]
    pub carry_messages: u64,
    #[serde(default)]
    pub carry_delivered: u64,
    #[serde(default)]
    pub carry_ms: VecDeque<(u64, u32, u8)>,
    /// This host's post office: (unix s, ms the mail waited here, messages
    /// in the delivery packet).
    #[serde(default)]
    pub mail_ms: VecDeque<(u64, u32, u8)>,
    /// Journeys by message packet ID (hex).
    #[serde(default)]
    pub journeys: HashMap<String, Journey>,
    /// The page server: per page path, and page loads per hour (unix hour, loads).
    #[serde(default)]
    pub pages: HashMap<String, PageStat>,
    #[serde(default)]
    pub page_hours: VecDeque<(u64, u32)>,
    #[serde(skip)]
    pending_data: HashMap<Address, (u64, Address, u8, u64)>,
    /// Carried message: notice token -> (first seen ms, message ID, handoffs).
    #[serde(skip)]
    pending_carry: HashMap<Address, (u64, u64, u8)>,
    #[serde(skip)]
    pending_route: HashMap<Address, (u64, Address, bool)>,
    #[serde(skip)]
    minute_pids: std::collections::HashSet<u64>,
    #[serde(skip)]
    minute_nodes: std::collections::HashSet<Address>,
}

fn now_s() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}
fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

impl Telemetry {
    pub fn new() -> Self {
        let mut t = Telemetry { started: now_s(), ..Default::default() };
        t.fix_sizes();
        t
    }

    fn fix_sizes(&mut self) {
        for (v, n) in [
            (&mut self.rx_by_type, TYPES),
            (&mut self.tx_by_type, TYPES),
            (&mut self.rx_bytes_by_type, TYPES),
            (&mut self.hops_hist, 17),
            (&mut self.size_hist, 9),
            (&mut self.snr_hist, 18),
            (&mut self.path_len_hist, 13),
        ] {
            v.resize(n, 0);
        }
    }

    pub fn load(path: &std::path::Path) -> Self {
        match std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str::<Telemetry>(&s).ok()) {
            Some(mut t) => {
                t.fix_sizes();
                t
            }
            None => Self::new(),
        }
    }

    pub fn save(&self, path: &std::path::Path) {
        if let Ok(s) = serde_json::to_string(self) {
            let tmp = path.with_extension("tmp");
            if std::fs::write(&tmp, s).is_ok() {
                let _ = std::fs::rename(&tmp, path);
            }
        }
    }

    /// The current minute's bucket (a new one each minute, 24 h kept).
    fn minute(&mut self) -> &mut Minute {
        let m = now_s() / 60 * 60;
        if self.minutes.back().is_none_or(|b| b.t != m) {
            self.minute_pids.clear();
            self.minute_nodes.clear();
            self.minutes.push_back(Minute { t: m, by_type: vec![0; TYPES], ..Default::default() });
            while self.minutes.len() > MINUTES {
                self.minutes.pop_front();
            }
        }
        self.minutes.back_mut().unwrap()
    }

    fn dest(&mut self, a: &Address) -> &mut DestStat {
        if self.dests.len() >= MAX_DESTS && !self.dests.contains_key(&hex(&a.0)) {
            if let Some(k) = self.dests.iter().min_by_key(|(_, d)| d.last).map(|(k, _)| k.clone()) {
                self.dests.remove(&k);
            }
        }
        let d = self.dests.entry(hex(&a.0)).or_default();
        d.last = now_s();
        d
    }

    /// A packet received (`out` false) or sent on `links` (`out` true).
    pub fn packet(&mut self, out: bool, links: &[(String, &'static str)], data: &[u8], snr: Option<i8>) {
        let decoded = Header::decode(data).ok();
        let ti = type_index(decoded.as_ref().map(|(h, _)| h.ptype));
        let size = data.len() as u64;
        {
            let backbone = links.iter().any(|(_, k)| *k != "radio");
            let m = self.minute();
            m.by_type[ti] += 1;
            if out {
                m.tx += 1;
                m.tx_bytes += size;
            } else {
                m.rx += 1;
                m.rx_bytes += size;
                if ti == 16 {
                    m.invalid += 1;
                }
                if backbone {
                    m.backbone_rx += 1;
                } else {
                    m.radio_rx += 1;
                }
                if let Some(s) = snr.filter(|_| !backbone) {
                    m.snr_sum += s as i64;
                    m.snr_n += 1;
                }
            }
        }
        let now = now_s();
        for (name, kind) in links {
            let l = self.links.entry(name.clone()).or_insert_with(|| LinkStat { rx: vec![0; TYPES], tx: vec![0; TYPES], first: now, ..Default::default() });
            l.kind = kind.to_string();
            l.last = now;
            if out {
                l.tx[ti] += 1;
                l.tx_bytes += size;
            } else {
                l.rx[ti] += 1;
                l.rx_bytes += size;
            }
        }
        if out {
            self.tx_by_type[ti] += 1;
        } else {
            self.rx_by_type[ti] += 1;
            self.rx_bytes_by_type[ti] += size;
            self.size_hist[(size as usize / 32).min(8)] += 1;
            if let Some(s) = snr.filter(|_| links.iter().all(|(_, k)| *k == "radio")) {
                self.snr_hist[((s as i32 + 20).clamp(0, 35) / 2) as usize] += 1;
            }
        }
        let Some((h, used)) = decoded else { return };
        let payload = &data[used.min(data.len())..];
        let pid = maarse_core::dedup::packet_id(h.ptype, &h.dest, payload);
        if self.minute_pids.insert(pid) {
            self.minute().unique += 1;
        }
        if !out {
            self.hops_hist[(h.hops as usize).min(16)] += 1;
            if h.region != 0 {
                *self.region_counts.entry(h.region).or_default() += 1;
            }
        }
        // Repeaters on the route.
        if let Some(p) = &h.path {
            self.path_len_hist[p.len().min(12)] += 1;
            for (i, id) in p.iter().enumerate() {
                let r = self.repeaters.entry(*id).or_default();
                r.seen += 1;
                r.positions += i as u64;
                r.last = now;
                if i == 0 {
                    r.first_hop += 1;
                }
            }
        }
        let path_s: Vec<String> = h.path.as_ref().map(|p| p.iter().map(|x| format!("{x:04x}")).collect()).unwrap_or_default();
        let t = now_ms();
        match h.ptype {
            PacketType::Data | PacketType::Fragment | PacketType::Deposit => {
                let hops = h.hops;
                if h.ptype == PacketType::Data {
                    let what = if out { "sent on" } else { "received" };
                    self.journey_step(pid, &h.dest, Step { t, what: what.into(), link: link_names(links), hops, path: path_s.clone(), note: String::new() }, false);
                }
                let d = self.dest(&h.dest);
                d.data += 1;
                d.bytes += size;
                d.last_path = path_s;
                d.last_hops = hops;
                // Its receipt's address, to time the delivery as seen here.
                let token = maarse_core::router::ack_token(pid);
                if !self.pending_data.contains_key(&token) {
                    if self.pending_data.len() >= MAX_PENDING {
                        self.pending_data.retain(|_, v| t.saturating_sub(v.0) < 120_000);
                    }
                    self.pending_data.insert(token, (t, h.dest, hops, pid));
                    self.data_seen += 1;
                    self.minute().data_seen += 1;
                }
            }
            PacketType::Ack => {
                self.dest(&h.dest).acks += 1;
                if let Some((t0, dest, hops, mid)) = self.pending_data.remove(&h.dest) {
                    let note = format!("{:.1} s after it passed here", t.saturating_sub(t0) as f64 / 1000.0);
                    self.journey_step(mid, &dest, Step { t, what: "receipt".into(), link: link_names(links), hops: h.hops, path: path_s.clone(), note }, true);
                    self.data_acked += 1;
                    self.minute().acks_matched += 1;
                    push_sample(&mut self.latency_ms, (now, t.saturating_sub(t0) as u32, hops));
                    if let Some(n) = self.nodes.get_mut(&hex(&dest.0)) {
                        n.packets_to += 1;
                    }
                }
            }
            PacketType::PathRequest => {
                self.dest(&h.dest).path_requests += 1;
                if let Some(tok) = payload.get(..16).and_then(Address::from_slice)
                    && !self.pending_route.contains_key(&tok)
                {
                    if self.pending_route.len() >= MAX_PENDING {
                        self.pending_route.retain(|_, v| t.saturating_sub(v.0) < 120_000);
                    }
                    // The first message to a new destination travels inside
                    // the request (token 16 + message + stamp 5); the answer
                    // is then also its receipt.
                    let carried = payload.len() > 16 + maarse_core::stamp::STAMP_LEN;
                    if carried {
                        self.data_seen += 1;
                        self.minute().data_seen += 1;
                    }
                    self.pending_route.insert(tok, (t, h.dest, carried));
                    self.route_searches += 1;
                }
            }
            PacketType::PathResponse => {
                if let Some((t0, _, carried)) = self.pending_route.remove(&h.dest) {
                    self.routes_found += 1;
                    self.minute().routes_found += 1;
                    let len = payload.first().copied().unwrap_or(0);
                    push_sample(&mut self.route_ms, (now, t.saturating_sub(t0) as u32, len));
                    if carried {
                        self.data_acked += 1;
                        self.minute().acks_matched += 1;
                        push_sample(&mut self.latency_ms, (now, t.saturating_sub(t0) as u32, len));
                    }
                }
            }
            PacketType::Carry => self.carry_packet(&h, payload, links, out, t),
            PacketType::KeyRequest => self.dest(&h.dest).key_requests += 1,
            PacketType::Announce | PacketType::Hello | PacketType::Channel | PacketType::Summary => {}
            _ => self.dest(&h.dest).other += 1,
        }
    }

    /// A verified announce from `dest`, heard `hops` away on `via`.
    pub fn announce(&mut self, dest: &Address, app: &AppData, hops: u8, via: &str, backbone: bool, snr: Option<i8>) {
        let now = now_s();
        if self.nodes.len() >= MAX_NODES && !self.nodes.contains_key(&hex(&dest.0)) {
            if let Some(k) = self.nodes.iter().min_by_key(|(_, n)| n.last_seen).map(|(k, _)| k.clone()) {
                self.nodes.remove(&k);
            }
        }
        if self.minute_nodes.insert(*dest) {
            self.minute().nodes += 1;
        }
        self.minute().announces += 1;
        let n = self.nodes.entry(hex(&dest.0)).or_insert_with(|| NodeInfo { first_seen: now, min_hops: u8::MAX, ..Default::default() });
        n.last_seen = now;
        n.announces += 1;
        n.last_hops = hops;
        n.min_hops = n.min_hops.min(hops);
        n.via = via.to_string();
        n.via_backbone = backbone;
        n.path_id = dest.path_id();
        if hops == 0 && !backbone {
            n.snr = snr;
        }
        if let Some(name) = &app.name {
            n.name = String::from_utf8_lossy(name).into_owned();
        }
        if let Some(site) = app.site.as_ref().filter(|s| s.is_set()) {
            n.site_h = Some(site.height_dm as f32 / 10.0);
            n.site_gain = Some(site.gain_hdb as f32 / 2.0);
            n.site_tx = Some(site.tx_dbm);
            n.owner = (!site.owner.is_empty()).then(|| String::from_utf8_lossy(&site.owner).into_owned());
        }
        if let Some(r) = app.role.and_then(maarse_core::router::Role::from_u8) {
            n.role = Some(format!("{r:?}"));
        }
        if let Some(f) = app.role_flags {
            n.hears_repeater = Some(f & appdata::ROLE_HEARS_REPEATER != 0);
            n.mobile = f & appdata::ROLE_MOBILE != 0;
            n.mail_class = (f & appdata::ROLE_MAIL_MASK) >> appdata::ROLE_MAIL_SHIFT;
        }
        n.caps = app.caps.or(n.caps);
        n.bridge = app.role_bridge;
        n.fs = app.fs;
        n.postbox = app.postbox.map(|p| hex(&p.0));
        if let Some(p) = app.position {
            n.lat = Some(p.lat_e7 as f64 / 1e7);
            n.lon = Some(p.lon_e7 as f64 / 1e7);
            n.precision = p.precision as u8;
        } else {
            n.lat = None;
            n.lon = None;
            n.precision = 0;
        }
        if let Some(t) = &app.telemetry {
            n.battery_mv = (t.battery_mv > 0).then_some(t.battery_mv);
            n.ext_power = Some(t.ext_power);
            n.energy = Some(t.energy);
            n.uptime_s = Some(t.uptime_s);
            n.temp_c = t.temp_dc.map(|d| d as f32 / 10.0);
        }
        // Map path IDs to addresses for the repeater table.
        if let Some(r) = self.repeaters.get_mut(&dest.path_id()) {
            r.addr = Some(hex(&dest.0));
        }
    }

    /// Adds a step to message `id`'s journey (`done` marks it finished).
    pub fn journey_step(&mut self, id: u64, dest: &Address, step: Step, done: bool) {
        let key = format!("{id:016x}");
        if self.journeys.len() >= MAX_JOURNEYS && !self.journeys.contains_key(&key) {
            if let Some(k) = self.journeys.iter().min_by_key(|(_, j)| j.last).map(|(k, _)| k.clone()) {
                self.journeys.remove(&k);
            }
        }
        let t = step.t;
        let j = self.journeys.entry(key).or_insert_with(|| Journey { dest: hex(&dest.0), first: t, ..Default::default() });
        j.last = t;
        if done && j.done.is_none() {
            j.done = Some(t);
        }
        if j.steps.len() < MAX_STEPS {
            j.steps.push(step);
        }
    }

    /// A `Carry` packet: a handoff (path = the taker) or a delivered notice
    /// (no path, addressed to the message's notice token).
    fn carry_packet(&mut self, h: &Header, payload: &[u8], links: &[(String, &'static str)], out: bool, t: u64) {
        let link = link_names(links);
        let Some(path) = &h.path else {
            if let Some((t0, id, n)) = self.pending_carry.remove(&h.dest) {
                self.carry_delivered += 1;
                push_sample(&mut self.carry_ms, (t / 1000, t.saturating_sub(t0).min(u32::MAX as u64) as u32, n));
                let dest = self.journeys.get(&format!("{id:016x}")).and_then(|j| Address::from_slice(&decode_hex(&j.dest))).unwrap_or(Address([0; 16]));
                let note = format!("{} after the first handoff seen here", fmt_wait(t.saturating_sub(t0)));
                self.journey_step(id, &dest, Step { t, what: "carry delivered notice".into(), link, note, ..Default::default() }, true);
            }
            return;
        };
        if payload.len() < 5 {
            return;
        }
        let tokens = payload[0];
        let ttl_s = u16::from_be_bytes([payload[1], payload[2]]);
        let from = u16::from_be_bytes([payload[3], payload[4]]);
        let msg = &payload[5..];
        let id = maarse_core::dedup::packet_id(PacketType::Data, &h.dest, msg);
        self.carry_handoffs += 1;
        let token = maarse_core::router::ack_token(id);
        match self.pending_carry.get_mut(&token) {
            Some(e) => e.2 = e.2.saturating_add(1),
            None => {
                if self.pending_carry.len() >= MAX_PENDING {
                    // Carried copies live up to hours: drop the oldest.
                    if let Some(k) = self.pending_carry.iter().min_by_key(|(_, v)| v.0).map(|(k, _)| *k) {
                        self.pending_carry.remove(&k);
                    }
                }
                self.pending_carry.insert(token, (t, id, 1));
                self.carry_messages += 1;
            }
        }
        let to = path.first().copied().unwrap_or(0);
        let what = if tokens == 0 { "carried to the recipient" } else { "carry handoff" };
        let note = format!("{from:04x} -> {to:04x}, {tokens} token(s), {} min left{}", ttl_s / 60, if out { " (sent by us)" } else { "" });
        self.journey_step(id, &h.dest, Step { t, what: what.into(), link, hops: h.hops, path: vec![format!("{from:04x}"), format!("{to:04x}")], note }, false);
    }

    /// This host's post office stored message `id` for `recipient`.
    pub fn mail_stored(&mut self, id: u64, recipient: &Address, copy: bool) {
        let note = if copy { "copy from another host" } else { "sent here" }.to_string();
        self.journey_step(id, recipient, Step { t: now_ms(), what: "stored at this post office".into(), note, ..Default::default() }, false);
    }

    /// This host's post office delivered `ids` (message ID, stored unix ms)
    /// to `recipient` in one packet.
    pub fn mail_delivered(&mut self, recipient: &Address, ids: &[(u64, u64)]) {
        let t = now_ms();
        for &(id, stored) in ids {
            let ms = t.saturating_sub(stored);
            push_sample(&mut self.mail_ms, (t / 1000, ms.min(u32::MAX as u64) as u32, ids.len().min(255) as u8));
            self.journey_step(id, recipient, Step { t, what: "delivered from this post office".into(), note: format!("waited {}", fmt_wait(ms)), ..Default::default() }, true);
        }
    }

    /// A page or picture was served (or refused) to `reader`.
    pub fn page(&mut self, path: &str, reader: &Address, lora: bool, bytes: usize, picture: bool, refused: bool) {
        let now = now_s();
        let p = self.pages.entry(path.to_string()).or_default();
        if refused {
            p.refused += 1;
        } else {
            p.served += 1;
            if lora { p.lora += 1 } else { p.net += 1 }
            if picture { p.pictures += 1 }
            p.bytes += bytes as u64;
        }
        p.last = now;
        let r = reader.0[..4].iter().map(|b| format!("{b:02x}")).collect::<String>();
        if !p.readers.contains(&r) && p.readers.len() < 500 {
            p.readers.push(r);
        }
        if !refused && !picture {
            let h = now / 3600;
            match self.page_hours.back_mut() {
                Some((hh, n)) if *hh == h => *n += 1,
                _ => self.page_hours.push_back((h, 1)),
            }
            while self.page_hours.len() > 24 * 14 {
                self.page_hours.pop_front();
            }
        }
    }

    /// A note was written on a page's board.
    pub fn page_note(&mut self, path: &str) {
        self.pages.entry(path.to_string()).or_default().notes += 1;
    }

    pub fn router_event(&mut self, what: &str) {
        *self.router.entry(what.to_string()).or_default() += 1;
        let m = self.minute();
        if what == "delivered" {
            m.delivered += 1;
        }
    }

    pub fn forwarded(&mut self) {
        self.minute().forwarded += 1;
    }

    pub fn dht(&mut self, op: &str) {
        *self.dht_ops.entry(op.to_string()).or_default() += 1;
        self.minute().dht += 1;
    }

    pub fn mail(&mut self, op: &str) {
        *self.mail_ops.entry(op.to_string()).or_default() += 1;
        self.minute().mail += 1;
    }

    /// The modem's own counters (`Event::Stats` from the firmware).
    pub fn modem_stats(&mut self, link: &str, items: &[(u8, i32)]) {
        let m = self.modems.entry(link.to_string()).or_default();
        // The firmware sends two frames (the first is full): merge by key.
        for (k, v) in items {
            match m.last.iter_mut().find(|(x, _)| x == k) {
                Some(e) => e.1 = *v,
                None => m.last.push((*k, *v)),
            }
        }
        m.last.sort_unstable_by_key(|(k, _)| *k);
        m.last_t = now_s();
        if !items.iter().any(|(k, _)| *k == 30) {
            return; // a series row per main frame only
        }
        let mut row = vec![m.last_t as i64];
        for k in MODEM_SERIES {
            row.push(items.iter().find(|(x, _)| *x == k).map_or(i64::MIN, |(_, v)| *v as i64));
        }
        m.series.push_back(row);
        while m.series.len() > 2880 {
            m.series.pop_front();
        }
    }

    pub fn modem_radio(&mut self, link: &str, text: String) {
        self.modems.entry(link.to_string()).or_default().radio = text;
    }

    /// The public map (`--public-map`): nodes heard in the last 7 days that
    /// share a position, with role and site. Nothing about messages.
    pub fn public_nodes(&self) -> Vec<Value> {
        let since = now_s().saturating_sub(7 * 24 * 3600);
        self.nodes
            .iter()
            .filter(|(_, n)| !n.mobile && n.precision > 0 && n.last_seen >= since && n.lat.is_some() && n.lon.is_some())
            .map(|(a, n)| {
                json!({"addr": a, "name": n.name, "role": n.role, "mobile": n.mobile, "lat": n.lat, "lon": n.lon, "precision": n.precision,
                    "height_m": n.site_h, "gain_dbi": n.site_gain, "tx_dbm": n.site_tx,
                    "last_seen": n.last_seen, "position_source": "self_reported"})
            })
            .collect()
    }

    /// Everything for the dashboard; `me` is our own position if known.
    pub fn json(&self, me: Option<(f64, f64)>) -> Value {
        let types: Vec<&str> = (0..TYPES).map(type_name).collect();
        let nodes: Vec<Value> = self
            .nodes
            .iter()
            .map(|(a, n)| {
                let dist = match (me, n.lat, n.lon) {
                    (Some(m), Some(la), Some(lo)) if n.precision > 0 => Some((km(m, (la, lo)) * 10.0).round() / 10.0),
                    _ => None,
                };
                let mut v = serde_json::to_value(n).unwrap_or(Value::Null);
                v["addr"] = a.clone().into();
                v["km"] = dist.into();
                v
            })
            .collect();
        let pct = |v: &VecDeque<(u64, u32, u8)>, q: f64| -> Option<u32> {
            if v.is_empty() {
                return None;
            }
            let mut x: Vec<u32> = v.iter().map(|s| s.1).collect();
            x.sort_unstable();
            Some(x[((x.len() - 1) as f64 * q) as usize])
        };
        let mut recent: Vec<(&String, &Journey)> = self.journeys.iter().collect();
        recent.sort_by_key(|(_, j)| std::cmp::Reverse(j.last));
        let journeys: Vec<Value> =
            recent.into_iter().take(200).map(|(k, j)| json!({"id": k, "dest": j.dest, "first": j.first, "last": j.last, "done": j.done, "steps": j.steps})).collect();
        json!({
            "started": self.started,
            "pages": self.pages.iter().map(|(k, p)| json!({"path": k, "served": p.served, "lora": p.lora, "net": p.net, "pictures": p.pictures,
                "bytes": p.bytes, "notes": p.notes, "refused": p.refused, "last": p.last, "readers": p.readers.len()})).collect::<Vec<_>>(),
            "page_hours": self.page_hours,
            "types": types,
            "minutes": self.minutes,
            "rx_by_type": self.rx_by_type,
            "tx_by_type": self.tx_by_type,
            "rx_bytes_by_type": self.rx_bytes_by_type,
            "hops_hist": self.hops_hist,
            "size_hist": self.size_hist,
            "snr_hist": self.snr_hist,
            "path_len_hist": self.path_len_hist,
            "regions": self.region_counts,
            "nodes": nodes,
            "dests": self.dests,
            "links": self.links,
            "repeaters": self.repeaters,
            "modems": self.modems.iter().map(|(k, m)| (k.clone(), json!({"last": m.last, "last_t": m.last_t, "radio": m.radio, "series": m.series, "series_keys": MODEM_SERIES}))).collect::<HashMap<_, _>>(),
            "delivery": {
                "data_seen": self.data_seen, "data_acked": self.data_acked,
                "latency_p50": pct(&self.latency_ms, 0.5), "latency_p90": pct(&self.latency_ms, 0.9), "latency_max": pct(&self.latency_ms, 1.0),
                "latency": self.latency_ms,
                "route_searches": self.route_searches, "routes_found": self.routes_found,
                "route_p50": pct(&self.route_ms, 0.5), "route_p90": pct(&self.route_ms, 0.9),
                "route": self.route_ms,
            },
            "router": self.router,
            "dht_ops": self.dht_ops,
            "mail_ops": self.mail_ops,
            "carry": {
                "handoffs": self.carry_handoffs, "messages": self.carry_messages, "delivered": self.carry_delivered,
                "p50": pct(&self.carry_ms, 0.5), "p90": pct(&self.carry_ms, 0.9), "max": pct(&self.carry_ms, 1.0),
                "samples": self.carry_ms,
            },
            "mail_time": {
                "p50": pct(&self.mail_ms, 0.5), "p90": pct(&self.mail_ms, 0.9), "max": pct(&self.mail_ms, 1.0),
                "samples": self.mail_ms,
            },
            "journeys": journeys,
            "me": me.map(|(a, b)| json!([a, b])),
        })
    }
}

fn link_names(links: &[(String, &'static str)]) -> String {
    links.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", ")
}

fn decode_hex(s: &str) -> Vec<u8> {
    (0..s.len() / 2).filter_map(|i| u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok()).collect()
}

fn fmt_wait(ms: u64) -> String {
    let s = ms / 1000;
    if s < 120 {
        format!("{s} s")
    } else if s < 7200 {
        format!("{} min", s / 60)
    } else {
        format!("{:.1} h", s as f64 / 3600.0)
    }
}

fn push_sample(v: &mut VecDeque<(u64, u32, u8)>, s: (u64, u32, u8)) {
    v.push_back(s);
    while v.len() > SAMPLES {
        v.pop_front();
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use maarse_core::packet::{Header, encode};

    #[test]
    fn delivery_and_route_times_are_matched() {
        let mut t = Telemetry::new();
        let link = [("serial COM6".to_string(), "radio")];
        let dest = Address([5; 16]);
        let data = encode(&Header::new(PacketType::Data, dest).with_path(heapless::Vec::from_slice(&[0x1111, 0x2222]).unwrap()), b"sealed").unwrap();
        t.packet(false, &link, &data, Some(7));
        let (h, used) = Header::decode(&data).unwrap();
        let pid = maarse_core::dedup::packet_id(h.ptype, &h.dest, &data[used..]);
        let ack = encode(&Header::new(PacketType::Ack, maarse_core::router::ack_token(pid)), b"").unwrap();
        t.packet(false, &link, &ack, Some(5));
        assert_eq!((t.data_seen, t.data_acked), (1, 1));
        assert_eq!(t.latency_ms.len(), 1);
        assert_eq!(t.repeaters[&0x1111].first_hop, 1);
        assert_eq!(t.dests[&hex(&dest.0)].data, 1);
        assert_eq!(t.rx_by_type[0] + t.rx_by_type[1], 2);
        // A route search and its answer.
        let tok = [9u8; 16];
        let mut pr = tok.to_vec();
        pr.extend_from_slice(&[1, 0, 0, 0, 0]);
        t.packet(false, &link, &encode(&Header::new(PacketType::PathRequest, dest), &pr).unwrap(), None);
        t.packet(false, &link, &encode(&Header::new(PacketType::PathResponse, Address(tok)), &[2, 0x11, 0x11, 0x22, 0x22]).unwrap(), None);
        assert_eq!((t.route_searches, t.routes_found), (1, 1));
        assert_eq!((t.data_seen, t.data_acked), (1, 1), "a request without a message is no message");
        let mut carried = [8u8; 16].to_vec();
        carried.extend_from_slice(b"first message inside the request");
        carried.extend_from_slice(&[1, 0, 0, 0, 0]);
        t.packet(false, &link, &encode(&Header::new(PacketType::PathRequest, dest), &carried).unwrap(), None);
        t.packet(false, &link, &encode(&Header::new(PacketType::PathResponse, Address([8; 16])), &[1, 0x11, 0x11]).unwrap(), None);
        assert_eq!((t.data_seen, t.data_acked), (2, 2), "the message in the request and its receipt (the answer)");
        assert_eq!(t.route_ms[0].2, 2);
        let j = t.json(None);
        assert_eq!(j["delivery"]["data_acked"], 2);
        // The Data and its receipt form one journey, finished.
        assert_eq!(t.journeys[&format!("{pid:016x}")].steps.len(), 2);
        assert!(t.journeys[&format!("{pid:016x}")].done.is_some());
    }

    #[test]
    fn carry_and_post_office_times_are_matched() {
        let mut t = Telemetry::new();
        let link = [("serial COM6".to_string(), "radio")];
        let dest = Address([6; 16]);
        let msg = b"sealed message";
        // Two handoffs of the same carried message: 0x1111 gives 0x2222 two tokens, then 0x2222 hands it to the recipient.
        for (from, to, tokens) in [(0x1111u16, 0x2222u16, 2u8), (0x2222, dest.path_id(), 0)] {
            let mut p = vec![tokens, 0x07, 0x08];
            p.extend_from_slice(&from.to_be_bytes());
            p.extend_from_slice(msg);
            let pkt = encode(&Header::new(PacketType::Carry, dest).with_path(heapless::Vec::from_slice(&[to]).unwrap()), &p).unwrap();
            t.packet(false, &link, &pkt, Some(3));
        }
        assert_eq!((t.carry_handoffs, t.carry_messages), (2, 1));
        // The delivered notice: a Carry without a path to the notice token.
        let id = maarse_core::dedup::packet_id(PacketType::Data, &dest, msg);
        let notice = encode(&Header::new(PacketType::Carry, maarse_core::router::ack_token(id)), b"").unwrap();
        t.packet(false, &link, &notice, Some(3));
        assert_eq!(t.carry_delivered, 1);
        assert_eq!(t.carry_ms[0].2, 2, "two handoffs seen");
        let j = &t.journeys[&format!("{id:016x}")];
        assert_eq!(j.steps.len(), 3);
        assert!(j.done.is_some());
        assert!(j.steps[0].note.contains("1111 -> 2222"));
        // The post office: stored, then delivered.
        t.mail_stored(42, &dest, false);
        t.mail_delivered(&dest, &[(42, now_ms() - 90_000)]);
        assert_eq!(t.mail_ms.len(), 1);
        assert!(t.mail_ms[0].1 >= 90_000);
        assert!(t.journeys[&format!("{:016x}", 42u64)].done.is_some());
        let v = t.json(None);
        assert_eq!(v["carry"]["delivered"], 1);
        assert_eq!(v["journeys"].as_array().map(|a| a.len()), Some(2));
    }

    #[test]
    fn announces_fill_the_census_and_survive_a_save() {
        let mut t = Telemetry::new();
        let mut app = AppData::default();
        app.name = Some(heapless::Vec::from_slice(b"Hill repeater").unwrap());
        app.role = Some(1);
        app.position = Some(appdata::Position { precision: appdata::Precision::Precise, lat_e7: 561_500_000, lon_e7: 102_000_000 });
        t.announce(&Address([7; 16]), &app, 2, "tcp 1.2.3.4:4242", true, None);
        let j = t.json(Some((55.68, 12.57)));
        let n = &j["nodes"][0];
        assert_eq!(n["name"], "Hill repeater");
        assert_eq!(n["role"], "Repeater");
        assert!(n["km"].as_f64().unwrap() > 100.0, "Aarhus-Copenhagen is about 150 km");
        let p = std::env::temp_dir().join(format!("maarse-tele-{}.json", std::process::id()));
        t.save(&p);
        let back = Telemetry::load(&p);
        assert_eq!(back.nodes.len(), 1);
        assert_eq!(back.rx_by_type.len(), TYPES);
    }
}
