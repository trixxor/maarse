//! The Denmark simulator engine: many nodes at real coordinates, each running
//! the unchanged `maarse_core::Router`, on the radio model of `maarse-sim`
//! (`maarse_sim::radio::Radio`), with the same collision, capture, CAD and
//! duty-cycle rules as the CLI. What is new compared to the CLI engine:
//!
//! * sparse links (spatial grid, only pairs within radio horizon and
//!   interference range are stored), so 2,000+ nodes fit;
//! * a send queue per node ordered by `qos::priority_with` (bulletins first);
//! * antenna height, per-node SF, batteries and power cuts;
//! * backbone links (TCP over the internet, Wi-Fi point-to-point) between hosts;
//! * timeline events (internet down, power cut, kill repeaters, alerts);
//! * signed area alerts (real `maarse_crypto::bulletin`) with coverage;
//! * an event trace in the `--trace-json` format of the CLI, extended with
//!   message tags so one message can be followed through the network.

use crate::scenario::*;
use maarse_core::airtime::LoraParams;
use maarse_core::dedup::packet_id;
use maarse_core::duty::DutyCycle;
use maarse_core::packet::{Header, Packet, encode};
use maarse_core::router::Buf;
use maarse_core::{Action, Address, Energy, PacketType, Role, Router};
use maarse_crypto::{Identity, PublicIdentity, bulletin};
use maarse_sim::radio::Radio;
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};
use serde_json::{Value, json};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::fmt::Write as _;

pub type Node = Router<128, 1024>;

/// Projection centre (Denmark).
const LAT0: f64 = 56.0;
const LON0: f64 = 10.5;
const M_PER_DEG_LAT: f64 = 110_574.0;
fn m_per_deg_lon() -> f64 {
    111_320.0 * LAT0.to_radians().cos()
}
pub fn to_xy(lat: f64, lon: f64) -> (f64, f64) {
    ((lon - LON0) * m_per_deg_lon(), (lat - LAT0) * M_PER_DEG_LAT)
}
pub fn to_ll(x: f64, y: f64) -> (f64, f64) {
    (LAT0 + y / M_PER_DEG_LAT, LON0 + x / m_per_deg_lon())
}

/// Grid cell for the neighbour search (metres).
const CELL: f64 = 10_000.0;
/// Overlap window kept for collision checks (same as the CLI: 20 s).
const TX_KEEP_MS: u64 = 20_000;
/// SNR the host reports for packets over TCP (maarse-host/src/links.rs).
const TCP_SNR: i8 = 10;
const TRACE_CAP: usize = 400_000;
/// Margin around an alert area inside which forwarders still forward it
/// (simulator-layer area scope, see AREA-ALERTS.md).
const AREA_MARGIN_M: f64 = 15_000.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Handheld,
    Relay,
    Repeater,
    Anchor,
    Host,
    Wifi,
    Mobile,
}

impl Kind {
    fn parse(s: &str) -> Kind {
        match s {
            "relay" => Kind::Relay,
            "repeater" => Kind::Repeater,
            "anchor" => Kind::Anchor,
            "host" => Kind::Host,
            "wifi" => Kind::Wifi,
            "mobile" => Kind::Mobile,
            _ => Kind::Handheld,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Kind::Handheld => "handheld",
            Kind::Relay => "relay",
            Kind::Repeater => "repeater",
            Kind::Anchor => "anchor",
            Kind::Host => "host",
            Kind::Wifi => "wifi",
            Kind::Mobile => "mobile",
        }
    }
    fn role(self) -> Role {
        match self {
            Kind::Handheld | Kind::Mobile => Role::Client,
            Kind::Relay => Role::Relay,
            Kind::Anchor => Role::Anchor,
            Kind::Repeater | Kind::Host | Kind::Wifi => Role::Repeater,
        }
    }
    fn default_h(self) -> f64 {
        match self {
            Kind::Handheld | Kind::Mobile => 1.5,
            Kind::Relay => 2.0,
            Kind::Repeater => 15.0,
            Kind::Anchor => 40.0,
            Kind::Host => 10.0,
            Kind::Wifi => 6.0,
        }
    }
    /// Antenna gain (dB): fixed stations have better antennas (CLI: 6 dB).
    fn ant_db(self) -> f64 {
        match self {
            Kind::Handheld | Kind::Mobile | Kind::Relay => 0.0,
            _ => 6.0,
        }
    }
    fn default_battery_wh(self) -> f64 {
        match self {
            Kind::Handheld | Kind::Mobile => 10.0,
            Kind::Relay => 20.0,
            Kind::Repeater | Kind::Wifi => 50.0,
            Kind::Anchor => 200.0,
            Kind::Host => 0.0,
        }
    }
    fn default_mains(self) -> bool {
        matches!(self, Kind::Repeater | Kind::Anchor | Kind::Host | Kind::Wifi)
    }
    /// Idle power draw (W).
    fn idle_w(self) -> f64 {
        match self {
            Kind::Handheld | Kind::Mobile => 0.05,
            Kind::Relay => 0.08,
            Kind::Repeater | Kind::Anchor | Kind::Wifi => 0.4,
            Kind::Host => 6.0,
        }
    }
    pub fn is_user(self) -> bool {
        matches!(self, Kind::Handheld | Kind::Mobile | Kind::Host | Kind::Relay)
    }
}

/// Height gain: crude, +10 dB per decade of height above 1.5 m.
fn height_db(h: f64) -> f64 {
    10.0 * (h.max(1.0) / 1.5).log10()
}

/// Radio horizon (4/3 earth) between two antenna heights, metres.
fn horizon_m(h1: f64, h2: f64) -> f64 {
    4_120.0 * (h1.max(0.5).sqrt() + h2.max(0.5).sqrt())
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DeadWhy {
    Alive,
    Power,
    Battery,
    Killed,
}

struct Queued {
    prio: u8,
    enq: u64,
    seq: u64,
    packet: Buf,
    id: u64,
    tries: u8,
}

pub struct SimNode {
    pub router: Box<Node>,
    pub kind: Kind,
    pub x: f64,
    pub y: f64,
    pub h: f64,
    pub sf: u8,
    pub dead: DeadWhy,
    /// Mains power available.
    pub powered: bool,
    pub mains: bool,
    pub battery_wh: f64,
    pub battery_max: f64,
    pub internet: bool,
    pub region: String,
    /// (other node, received power in dBm), sorted by node. Contains every pair
    /// down to the interference limit (snr_min - 6 dB).
    nbrs: Vec<(u32, f32)>,
    txq: Vec<Queued>,
    busy_until: u64,
    svc_at: u64,
    duty: DutyCycle,
    pub airtime_us: u64,
    pub duty_hits: u32,
    route: Vec<(f64, f64)>,
    route_pos: f64,
    route_dir: f64,
    speed: f64,
    links: Vec<usize>,
    pub label: String,
}

impl SimNode {
    pub fn alive(&self) -> bool {
        self.dead == DeadWhy::Alive
    }
    fn gain_to(&self, j: u32) -> Option<f32> {
        self.nbrs.binary_search_by_key(&j, |e| e.0).ok().map(|k| self.nbrs[k].1)
    }
}

struct Link {
    a: usize,
    b: usize,
    wifi: bool,
    up: bool,
}

struct Tx {
    id: u64,
    sender: usize,
    start: u64,
    end: u64,
    packet: Buf,
}

#[allow(clippy::large_enum_variant)]
enum Ev {
    Ready { node: usize, packet: Buf, id: u64 },
    Service { node: usize },
    TxEnd { tx: u64 },
    Tick { node: usize },
    Backbone { from: usize, to: usize, packet: Buf },
    Traffic,
    Move,
    Energy,
    Neighbors,
    Timeline { idx: usize },
}

pub struct Msg {
    pub from: usize,
    pub to: usize,
    pub payload: Vec<u8>,
    pub id: u64,
    pub sent_ms: u64,
    pub delivered_ms: Option<u64>,
    pub acked_ms: Option<u64>,
}

pub struct Alert {
    pub from: usize,
    pub kind: String,
    pub text: String,
    pub sent_ms: u64,
    pub poly: Vec<(f64, f64)>,
    pub circle: Option<(f64, f64, f64)>,
    pub pid: u64,
    /// First verified reception per node (ms).
    pub recv: HashMap<usize, u64>,
    pub inside: Vec<usize>,
}

impl Alert {
    fn contains(&self, x: f64, y: f64) -> bool {
        if let Some((cx, cy, r)) = self.circle {
            return (x - cx).hypot(y - cy) <= r;
        }
        point_in_poly(x, y, &self.poly)
    }
    fn distance(&self, x: f64, y: f64) -> f64 {
        if self.contains(x, y) {
            return 0.0;
        }
        if let Some((cx, cy, r)) = self.circle {
            return (x - cx).hypot(y - cy) - r;
        }
        let n = self.poly.len();
        (0..n)
            .map(|i| seg_dist((x, y), self.poly[i], self.poly[(i + 1) % n]))
            .fold(f64::INFINITY, f64::min)
    }
}

pub fn point_in_poly(x: f64, y: f64, p: &[(f64, f64)]) -> bool {
    let mut inside = false;
    let n = p.len();
    if n < 3 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = p[i];
        let (xj, yj) = p[j];
        if ((yi > y) != (yj > y)) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn seg_dist(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
    (p.0 - a.0 - t * dx).hypot(p.1 - a.1 - t * dy)
}

fn area_xy(a: &Area) -> (Vec<(f64, f64)>, Option<(f64, f64, f64)>) {
    let circle = a.circle.map(|[la, lo, r]| {
        let (x, y) = to_xy(la, lo);
        (x, y, r)
    });
    let poly = a.polygon.iter().map(|[la, lo]| to_xy(*la, *lo)).collect();
    (poly, circle)
}

#[derive(Default)]
pub struct Stats {
    pub tx_count: u64,
    pub collisions: u64,
    pub duty_drops: u64,
    pub busy_drops: u64,
    pub cancelled: u64,
    pub backbone_packets: u64,
    pub channel_sent: u64,
    pub channel_deliveries: u64,
    pub channel_receivers: u64,
    pub scope_dropped: u64,
    /// Queue delay per priority (0..=4): sum ms, count, max.
    pub qdelay: [(u64, u64, u64); 5],
    pub by_type: std::collections::BTreeMap<String, u64>,
}

pub struct Engine {
    pub sc: Scenario,
    radios: Vec<Radio>,
    rng: SmallRng,
    pub nodes: Vec<SimNode>,
    links: Vec<Link>,
    queue: BinaryHeap<Reverse<(u64, u64)>>,
    events: HashMap<u64, Ev>,
    seq: u64,
    pub now: u64,
    active: Vec<Tx>,
    next_tx: u64,
    pub msgs: Vec<Msg>,
    pub alerts: Vec<Alert>,
    pub stats: Stats,
    /// Packet id -> tag (message index, or alert index | ALERT_TAG).
    tags: HashMap<u64, u32>,
    cancelled: HashSet<(usize, u64)>,
    trace: Vec<String>,
    trace_base: u64,
    grid: HashMap<(i32, i32), Vec<u32>>,
    max_range: f64,
    internet_global: bool,
    internet_down: HashSet<String>,
    public_channel: Address,
    bulletin_channel: Address,
    authority: Identity,
    trusted: Vec<PublicIdentity>,
    traffic: Traffic,
    /// Packet ids of announces we injected (for reach queries).
    pub announces: Vec<(usize, u64, u64)>,
    timeline_done: usize,
}

const ALERT_TAG: u32 = 0x8000_0000;

fn ptype_name(p: PacketType) -> &'static str {
    match p {
        PacketType::Data => "Data",
        PacketType::Ack => "Ack",
        PacketType::PathRequest => "PathRequest",
        PacketType::PathResponse => "PathResponse",
        PacketType::Announce => "Announce",
        PacketType::Channel => "Channel",
        PacketType::Fragment => "Fragment",
        PacketType::Carry => "Carry",
        PacketType::Datagram => "Datagram",
        PacketType::Deposit => "Deposit",
        PacketType::Control => "Control",
        PacketType::Probe => "Probe",
    }
}

impl Engine {
    pub fn new(sc: Scenario) -> Engine {
        let mut rng = SmallRng::seed_from_u64(sc.seed);
        let radios: Vec<Radio> = (7..=12).map(|sf| Radio::new(sf, sc.tx_dbm)).collect();
        let public_channel = Address([0xc0; 16]);
        let bulletin_channel = bulletin::channel();
        // The authority's signing key is derived from the seed, so runs repeat.
        let mut secret = [0u8; 64];
        rng.fill(&mut secret[..]);
        let authority = Identity::from_secret_bytes(&secret);
        let trusted = vec![authority.public()];
        let f = sc.features.clone();
        let mut nodes = Vec::with_capacity(sc.nodes.len());
        for spec in &sc.nodes {
            let kind = Kind::parse(&spec.kind);
            let mut addr = [0u8; 16];
            rng.fill(&mut addr[..]);
            let sf = if spec.sf >= 7 && spec.sf <= 12 { spec.sf } else { sc.sf.clamp(7, 12) };
            let mut router = Box::new(Node::new(Address(addr), kind.role()));
            let _ = router.subscribe(public_channel);
            let _ = router.subscribe(bulletin_channel);
            router.mobile = kind == Kind::Mobile;
            router.carry_copies = if f.carry { if spec.carry > 0 { spec.carry } else { 2 } } else { 0 };
            router.local_repair = f.local_repair;
            router.load_control = f.load_control;
            if f.sf_slot {
                router.set_slot(maarse_core::qos::slot_ms(&LoraParams::new(sf)), f.scale_timeouts);
            }
            if f.regions && !spec.region.is_empty() {
                router.regions = maarse_core::region::parse_list(&format!("dk,{}", spec.region));
            }
            let (x, y) = to_xy(spec.lat, spec.lon);
            let battery = if spec.battery_wh < 0.0 { kind.default_battery_wh() } else { spec.battery_wh };
            let route: Vec<(f64, f64)> = spec.route.iter().map(|[a, b]| to_xy(*a, *b)).collect();
            nodes.push(SimNode {
                router,
                kind,
                x,
                y,
                h: if spec.h > 0.0 { spec.h } else { kind.default_h() },
                sf,
                dead: DeadWhy::Alive,
                powered: true,
                mains: spec.mains.unwrap_or(kind.default_mains()),
                battery_wh: battery,
                battery_max: battery,
                internet: spec.internet.unwrap_or(kind == Kind::Host),
                region: spec.region.clone(),
                nbrs: Vec::new(),
                txq: Vec::new(),
                busy_until: 0,
                svc_at: u64::MAX,
                duty: DutyCycle::new(sc.duty_permille, 0),
                airtime_us: 0,
                duty_hits: 0,
                speed: if spec.speed_kmh > 0.0 { spec.speed_kmh / 3.6 } else { 30.0 },
                route_pos: 0.0,
                route_dir: 1.0,
                route,
                links: Vec::new(),
                label: spec.label.clone(),
            });
        }
        let mut links = Vec::new();
        for (k, l) in sc.links.iter().enumerate() {
            if l.a < nodes.len() && l.b < nodes.len() && l.a != l.b {
                nodes[l.a].links.push(k);
                nodes[l.b].links.push(k);
            }
            links.push(Link { a: l.a, b: l.b, wifi: l.kind == "wifi", up: l.up });
        }
        let traffic = sc.traffic.clone();
        let mut e = Engine {
            radios,
            rng,
            nodes,
            links,
            queue: BinaryHeap::new(),
            events: HashMap::new(),
            seq: 0,
            now: 0,
            active: Vec::new(),
            next_tx: 0,
            msgs: Vec::new(),
            alerts: Vec::new(),
            stats: Stats::default(),
            tags: HashMap::new(),
            cancelled: HashSet::new(),
            trace: Vec::new(),
            trace_base: 0,
            grid: HashMap::new(),
            max_range: 0.0,
            internet_global: true,
            internet_down: HashSet::new(),
            public_channel,
            bulletin_channel,
            authority,
            trusted,
            traffic,
            announces: Vec::new(),
            timeline_done: 0,
            sc,
        };
        e.build_links();
        e.seed_neighbors();
        // Timeline, sorted by time (stable).
        let mut order: Vec<usize> = (0..e.sc.events.len()).collect();
        order.sort_by(|a, b| e.sc.events[*a].t_s.total_cmp(&e.sc.events[*b].t_s));
        for idx in order {
            let at = (e.sc.events[idx].t_s.max(0.0) * 1000.0) as u64;
            e.schedule(at, Ev::Timeline { idx });
        }
        e.schedule(1000, Ev::Traffic);
        e.schedule(2000, Ev::Move);
        e.schedule(10_000, Ev::Energy);
        e.schedule(60_000, Ev::Neighbors);
        e
    }

    fn schedule(&mut self, at: u64, ev: Ev) {
        self.seq += 1;
        self.events.insert(self.seq, ev);
        self.queue.push(Reverse((at, self.seq)));
    }

    fn radio(&self, sf: u8) -> &Radio {
        &self.radios[(sf.clamp(7, 12) - 7) as usize]
    }

    // ---------- links ----------

    fn cell(x: f64, y: f64) -> (i32, i32) {
        ((x / CELL).floor() as i32, (y / CELL).floor() as i32)
    }

    fn pair_rx(&self, a: usize, b: usize) -> Option<f32> {
        let (na, nb) = (&self.nodes[a], &self.nodes[b]);
        if na.sf != nb.sf {
            return None;
        }
        let d = (na.x - nb.x).hypot(na.y - nb.y);
        if d > horizon_m(na.h, nb.h) {
            return None;
        }
        let g = na.kind.ant_db() + nb.kind.ant_db() + height_db(na.h) + height_db(nb.h);
        let r = self.radio(na.sf);
        let rx = r.rx_dbm(d, g);
        (r.snr(rx) >= r.snr_min - 6.0).then_some(rx as f32)
    }

    fn build_links(&mut self) {
        self.grid.clear();
        let maxh = self.nodes.iter().map(|n| n.h).fold(1.5, f64::max);
        self.max_range = horizon_m(maxh, maxh);
        for (i, n) in self.nodes.iter().enumerate() {
            self.grid.entry(Self::cell(n.x, n.y)).or_default().push(i as u32);
        }
        for n in &mut self.nodes {
            n.nbrs.clear();
        }
        let reach = (self.max_range / CELL).ceil() as i32;
        for i in 0..self.nodes.len() {
            let (cx, cy) = Self::cell(self.nodes[i].x, self.nodes[i].y);
            let mut found = Vec::new();
            for gx in cx - reach..=cx + reach {
                for gy in cy - reach..=cy + reach {
                    if let Some(v) = self.grid.get(&(gx, gy)) {
                        for &j in v {
                            if (j as usize) > i
                                && let Some(rx) = self.pair_rx(i, j as usize)
                            {
                                found.push((j, rx));
                            }
                        }
                    }
                }
            }
            for (j, rx) in found {
                self.nodes[i].nbrs.push((j, rx));
                self.nodes[j as usize].nbrs.push((i as u32, rx));
            }
        }
        for n in &mut self.nodes {
            n.nbrs.sort_unstable_by_key(|e| e.0);
        }
    }

    /// Recompute the links of one (moving) node.
    fn relink(&mut self, i: usize) {
        let old = std::mem::take(&mut self.nodes[i].nbrs);
        for (j, _) in old {
            let v = &mut self.nodes[j as usize].nbrs;
            if let Ok(k) = v.binary_search_by_key(&(i as u32), |e| e.0) {
                v.remove(k);
            }
        }
        let reach = (self.max_range / CELL).ceil() as i32;
        let (cx, cy) = Self::cell(self.nodes[i].x, self.nodes[i].y);
        let mut found = Vec::new();
        for gx in cx - reach..=cx + reach {
            for gy in cy - reach..=cy + reach {
                if let Some(v) = self.grid.get(&(gx, gy)) {
                    for &j in v {
                        if j as usize != i
                            && let Some(rx) = self.pair_rx(i, j as usize)
                        {
                            found.push((j, rx));
                        }
                    }
                }
            }
        }
        for &(j, rx) in &found {
            let v = &mut self.nodes[j as usize].nbrs;
            let k = v.binary_search_by_key(&(i as u32), |e| e.0).unwrap_or_else(|k| k);
            v.insert(k, (i as u32, rx));
        }
        found.sort_unstable_by_key(|e| e.0);
        self.nodes[i].nbrs = found;
    }

    fn audible_rx(&self, from: usize, rx: f32) -> bool {
        let r = self.radio(self.nodes[from].sf);
        r.snr(rx as f64) >= r.snr_min
    }

    /// The CLI hands each router what neighbour announces would tell it (role and
    /// "hears a repeater"); we do the same for audible, live pairs.
    fn seed_neighbors(&mut self) {
        let now = self.now;
        let n = self.nodes.len();
        let reports: Vec<(u16, Role, bool)> = (0..n)
            .map(|j| {
                let rt = &self.nodes[j].router;
                (rt.addr.path_id(), rt.role, rt.hears_repeater(now))
            })
            .collect();
        for i in 0..n {
            if !self.nodes[i].alive() {
                continue;
            }
            let list: Vec<(u32, f32)> = self.nodes[i]
                .nbrs
                .iter()
                .copied()
                .filter(|&(j, rx)| self.nodes[j as usize].alive() && self.audible_rx(i, rx))
                .collect();
            for (j, rx) in list {
                let (id, role, hears) = reports[j as usize];
                let snr = self.radio(self.nodes[i].sf).snr(rx as f64).clamp(-30.0, 30.0) as i8;
                let rt = &mut self.nodes[i].router;
                rt.neighbors.heard(id, None, snr, 0, now);
                rt.neighbors.set_role(id, role, Some(hears));
            }
        }
    }

    // ---------- trace ----------

    fn tr(&mut self, s: String) {
        if !self.sc.trace {
            return;
        }
        if self.trace.len() >= TRACE_CAP {
            let cut = TRACE_CAP / 2;
            self.trace.drain(..cut);
            self.trace_base += cut as u64;
        }
        self.trace.push(s);
    }

    fn tag_json(&self, pid: u64) -> String {
        match self.tags.get(&pid) {
            Some(&t) if t & ALERT_TAG != 0 => format!(",\"alert\":{}", t & !ALERT_TAG),
            Some(&t) => format!(",\"msg\":{t}"),
            None => String::new(),
        }
    }

    // ---------- running ----------

    /// Runs the simulation until `until` ms (or until `max_events` events).
    pub fn run_until(&mut self, until: u64, max_events: usize) -> usize {
        let mut done = 0;
        while let Some(&Reverse((at, key))) = self.queue.peek() {
            if at > until || done >= max_events {
                break;
            }
            self.queue.pop();
            self.now = at;
            if let Some(ev) = self.events.remove(&key) {
                self.handle(ev);
                done += 1;
            }
        }
        if done < max_events {
            self.now = self.now.max(until);
        }
        done
    }

    fn handle(&mut self, ev: Ev) {
        match ev {
            Ev::Ready { node, packet, id } => self.ready(node, packet, id),
            Ev::Service { node } => {
                if self.nodes[node].svc_at == self.now {
                    self.nodes[node].svc_at = u64::MAX;
                    self.service(node);
                }
            }
            Ev::TxEnd { tx } => self.tx_end(tx),
            Ev::Tick { node } => {
                if !self.nodes[node].alive() {
                    return;
                }
                let before = self.nodes[node].router.stats;
                let actions = self.nodes[node].router.tick(self.now);
                self.after_router(node, before, None);
                self.apply(node, actions.into_iter().collect(), None);
            }
            Ev::Backbone { from, to, packet } => {
                if !self.nodes[to].alive() {
                    return;
                }
                let pid = Packet::decode(&packet).map(|p| packet_id(p.header.ptype, &p.header.dest, p.payload)).unwrap_or(0);
                let tag = self.tags.get(&pid).copied();
                let tj = self.tag_json(pid);
                self.tr(format!("{{\"t\":{},\"ev\":\"bb\",\"from\":{from},\"to\":{to},\"pid\":\"{pid:016x}\"{tj}}}", self.now));
                let before = self.nodes[to].router.stats;
                let actions = self.nodes[to].router.on_receive(&packet, TCP_SNR, self.now);
                self.after_router(to, before, tag);
                self.apply(to, actions.into_iter().collect(), tag);
            }
            Ev::Traffic => {
                self.traffic_tick();
                self.schedule(self.now + 1000, Ev::Traffic);
            }
            Ev::Move => {
                self.move_nodes();
                self.schedule(self.now + 2000, Ev::Move);
            }
            Ev::Energy => {
                self.energy_tick(10.0);
                self.schedule(self.now + 10_000, Ev::Energy);
            }
            Ev::Neighbors => {
                self.seed_neighbors();
                self.schedule(self.now + 60_000, Ev::Neighbors);
            }
            Ev::Timeline { idx } => {
                let ev = self.sc.events[idx].clone();
                self.timeline_done += 1;
                self.inject(&ev);
            }
        }
    }

    /// Note router counter changes as trace events (local repair, carry handoff).
    fn after_router(&mut self, node: usize, before: maarse_core::router::RouterStats, tag: Option<u32>) {
        let s = self.nodes[node].router.stats;
        let tj = match tag {
            Some(t) if t & ALERT_TAG == 0 => format!(",\"msg\":{t}"),
            _ => String::new(),
        };
        if s.local_repairs > before.local_repairs {
            self.tr(format!("{{\"t\":{},\"ev\":\"repair\",\"n\":{node}{tj}}}", self.now));
        }
        if s.carry_handoffs > before.carry_handoffs {
            self.tr(format!("{{\"t\":{},\"ev\":\"carry\",\"n\":{node}{tj}}}", self.now));
        }
        if s.hop_retries > before.hop_retries {
            self.tr(format!("{{\"t\":{},\"ev\":\"retry\",\"n\":{node}{tj}}}", self.now));
        }
        if s.quota_dropped > before.quota_dropped {
            self.tr(format!("{{\"t\":{},\"ev\":\"drop\",\"n\":{node},\"why\":\"quota\"{tj}}}", self.now));
        }
    }

    /// Carries out a router's actions. `tag` is the message the triggering
    /// packet belonged to; new packets inherit it (causal tagging).
    fn apply(&mut self, node: usize, actions: Vec<Action>, tag: Option<u32>) {
        for a in actions {
            match a {
                Action::Transmit { packet, delay_ms, id } => {
                    let Ok(p) = Packet::decode(&packet) else { continue };
                    let pid = packet_id(p.header.ptype, &p.header.dest, p.payload);
                    if let Some(t) = tag {
                        self.tags.entry(pid).or_insert(t);
                    }
                    // Simulator-layer area scope for alerts (proposal).
                    if self.sc.features.area_scope
                        && p.header.hops > 0
                        && let Some(&t) = self.tags.get(&pid)
                        && t & ALERT_TAG != 0
                    {
                        let al = &self.alerts[(t & !ALERT_TAG) as usize];
                        if al.distance(self.nodes[node].x, self.nodes[node].y) > AREA_MARGIN_M {
                            self.stats.scope_dropped += 1;
                            let tj = self.tag_json(pid);
                            self.tr(format!("{{\"t\":{},\"ev\":\"drop\",\"n\":{node},\"why\":\"area\",\"pid\":\"{pid:016x}\"{tj}}}", self.now));
                            continue;
                        }
                    }
                    // Backbone: a host sends every packet to all its links too
                    // (maarse-host), filtered by the peer's regions.
                    if self.sc.features.backbone {
                        let region = p.header.region;
                        let peers: Vec<(usize, bool)> = self.nodes[node]
                            .links
                            .iter()
                            .filter_map(|&k| self.link_ok(k).then(|| {
                                let l = &self.links[k];
                                (if l.a == node { l.b } else { l.a }, l.wifi)
                            }))
                            .collect();
                        for (peer, wifi) in peers {
                            if region != 0 && !self.nodes[peer].router.forwards_region(region) {
                                continue;
                            }
                            self.stats.backbone_packets += 1;
                            let lat = if wifi { 5 } else { 40 };
                            self.schedule(self.now + delay_ms as u64 + lat, Ev::Backbone { from: node, to: peer, packet: packet.clone() });
                        }
                    }
                    let slot = self.nodes[node].router.slot_ms.max(1) as u64;
                    let jitter = self.rng.gen_range(0..slot);
                    self.schedule(self.now + delay_ms as u64 + jitter, Ev::Ready { node, packet, id });
                }
                Action::Cancel { id } => {
                    let q = &mut self.nodes[node].txq;
                    if let Some(k) = q.iter().position(|e| e.id == id) {
                        q.remove(k);
                    } else {
                        self.cancelled.insert((node, id));
                    }
                    self.stats.cancelled += 1;
                    let tj = self.tag_json(id);
                    self.tr(format!("{{\"t\":{},\"ev\":\"cancel\",\"n\":{node},\"pid\":\"{id:016x}\"{tj}}}", self.now));
                }
                Action::Deliver { ptype, dest, payload, hops } => self.on_deliver(node, ptype, dest, &payload, hops),
                Action::RouteLearned { dest, hops } => {
                    let d = self.nodes.iter().position(|n| n.router.addr == dest).map_or(-1, |i| i as i64);
                    let tj = match tag {
                        Some(t) if t & ALERT_TAG == 0 => format!(",\"msg\":{t}"),
                        _ => String::new(),
                    };
                    self.tr(format!("{{\"t\":{},\"ev\":\"route\",\"n\":{node},\"dest\":{d},\"hops\":{hops}{tj}}}", self.now));
                }
                Action::Acked { msg_id } | Action::CarryAcked { msg_id } => {
                    if let Some(i) = self.msgs.iter().position(|m| m.id == msg_id && m.from == node)
                        && self.msgs[i].acked_ms.is_none()
                    {
                        self.msgs[i].acked_ms = Some(self.now);
                        self.tr(format!("{{\"t\":{},\"ev\":\"ack\",\"msg\":{i}}}", self.now));
                    }
                }
            }
        }
        if let Some(d) = self.nodes[node].router.next_deadline() {
            // Ticks are cheap to repeat; schedule one at the deadline.
            self.schedule(d.max(self.now), Ev::Tick { node });
        }
    }

    fn link_ok(&self, k: usize) -> bool {
        let l = &self.links[k];
        if !l.up || l.a >= self.nodes.len() || l.b >= self.nodes.len() {
            return false;
        }
        let (a, b) = (&self.nodes[l.a], &self.nodes[l.b]);
        if !a.alive() || !b.alive() {
            return false;
        }
        if l.wifi {
            return true;
        }
        self.has_internet(l.a) && self.has_internet(l.b)
    }

    fn has_internet(&self, i: usize) -> bool {
        let n = &self.nodes[i];
        n.internet && n.powered && self.internet_global && !self.internet_down.contains(&n.region)
    }

    fn on_deliver(&mut self, node: usize, ptype: PacketType, dest: Address, payload: &[u8], hops: u8) {
        match ptype {
            PacketType::Data => {
                let id = packet_id(PacketType::Data, &dest, payload);
                if let Some(i) = self.msgs.iter().position(|m| m.id == id && m.to == node)
                    && self.msgs[i].delivered_ms.is_none()
                {
                    self.msgs[i].delivered_ms = Some(self.now);
                    self.tr(format!("{{\"t\":{},\"ev\":\"deliver\",\"msg\":{i},\"n\":{node},\"hops\":{hops}}}", self.now));
                }
            }
            PacketType::Channel if dest == self.public_channel => self.stats.channel_deliveries += 1,
            PacketType::Channel if dest == self.bulletin_channel => {
                // The bulletin sender is the authority; verify like a client would.
                let sender = self.authority.address();
                let Some(v) = bulletin::verify(&sender, payload, &self.trusted) else { return };
                let Some(idx) = parse_area_index(v.text) else { return };
                if idx >= self.alerts.len() || self.alerts[idx].recv.contains_key(&node) {
                    return;
                }
                let (x, y) = (self.nodes[node].x, self.nodes[node].y);
                let inside = self.alerts[idx].contains(x, y);
                self.alerts[idx].recv.insert(node, self.now);
                self.tr(format!("{{\"t\":{},\"ev\":\"alert_rx\",\"alert\":{idx},\"n\":{node},\"inside\":{inside},\"hops\":{hops}}}", self.now));
            }
            _ => {}
        }
    }

    fn ready(&mut self, node: usize, packet: Buf, id: u64) {
        if !self.nodes[node].alive() {
            return;
        }
        if self.cancelled.remove(&(node, id)) {
            return;
        }
        let prio = maarse_core::qos::priority_with(&packet, &self.bulletin_channel);
        self.seq += 1;
        let q = Queued { prio, enq: self.now, seq: self.seq, packet, id, tries: 0 };
        self.nodes[node].txq.push(q);
        self.want_service(node, self.now);
    }

    fn want_service(&mut self, node: usize, at: u64) {
        let at = at.max(self.nodes[node].busy_until);
        if self.nodes[node].svc_at <= at {
            return;
        }
        self.nodes[node].svc_at = at;
        self.schedule(at, Ev::Service { node });
    }

    fn channel_busy(&self, node: usize) -> bool {
        self.active.iter().any(|t| {
            t.start <= self.now
                && self.now < t.end
                && (t.sender == node || self.nodes[node].gain_to(t.sender as u32).is_some_and(|rx| self.audible_rx(t.sender, rx)))
        })
    }

    fn service(&mut self, node: usize) {
        if !self.nodes[node].alive() {
            self.nodes[node].txq.clear();
            return;
        }
        if self.nodes[node].txq.is_empty() {
            return;
        }
        // Highest priority first, then oldest.
        let k = (0..self.nodes[node].txq.len())
            .min_by_key(|&k| {
                let q = &self.nodes[node].txq[k];
                (q.prio, q.seq)
            })
            .unwrap();
        if self.channel_busy(node) {
            let q = &mut self.nodes[node].txq[k];
            q.tries += 1;
            if q.tries > 8 {
                let q = self.nodes[node].txq.remove(k);
                self.stats.busy_drops += 1;
                let tj = self.tag_json(q.id);
                self.tr(format!("{{\"t\":{},\"ev\":\"drop\",\"n\":{node},\"why\":\"busy\",\"pid\":\"{:016x}\"{tj}}}", self.now, q.id));
            } else {
                self.tr(format!("{{\"t\":{},\"ev\":\"defer\",\"n\":{node},\"why\":\"busy\"}}", self.now));
            }
            let backoff = self.rng.gen_range(100..600);
            self.want_service(node, self.now + backoff);
            return;
        }
        let sf = self.nodes[node].sf;
        let airtime = self.radio(sf).lora.airtime_us(self.nodes[node].txq[k].packet.len());
        if !self.nodes[node].duty.try_consume(self.now, airtime) {
            let wait = self.nodes[node].duty.wait_ms(self.now, airtime);
            self.nodes[node].duty_hits += 1;
            let q = &mut self.nodes[node].txq[k];
            q.tries += 1;
            if wait > 120_000 || q.tries > 8 {
                let q = self.nodes[node].txq.remove(k);
                self.stats.duty_drops += 1;
                let tj = self.tag_json(q.id);
                self.tr(format!("{{\"t\":{},\"ev\":\"drop\",\"n\":{node},\"why\":\"duty\",\"pid\":\"{:016x}\"{tj}}}", self.now, q.id));
                self.want_service(node, self.now + 50);
            } else {
                self.tr(format!("{{\"t\":{},\"ev\":\"defer\",\"n\":{node},\"why\":\"duty\"}}", self.now));
                self.want_service(node, self.now + wait);
            }
            return;
        }
        let q = self.nodes[node].txq.remove(k);
        let delay = self.now - q.enq;
        let s = &mut self.stats.qdelay[q.prio.min(4) as usize];
        s.0 += delay;
        s.1 += 1;
        s.2 = s.2.max(delay);
        self.nodes[node].airtime_us += airtime as u64;
        self.nodes[node].router.note_airtime(self.now, airtime.div_ceil(1000));
        self.stats.tx_count += 1;
        let end = self.now + (airtime as u64).div_ceil(1000);
        let (pname, hops, region, pid) = match Packet::decode(&q.packet) {
            Ok(p) => (ptype_name(p.header.ptype), p.header.hops, p.header.region, packet_id(p.header.ptype, &p.header.dest, p.payload)),
            Err(_) => ("?", 0, 0, q.id),
        };
        let pname = if pname == "Channel" && q.packet.get(2..18) == Some(&self.bulletin_channel.0[..]) { "Bulletin" } else { pname };
        *self.stats.by_type.entry(pname.to_string()).or_default() += 1;
        let tx = self.next_tx;
        self.next_tx += 1;
        let tj = self.tag_json(pid);
        self.tr(format!(
            "{{\"t\":{},\"ev\":\"tx\",\"n\":{node},\"id\":{tx},\"end\":{end},\"air\":{airtime},\"p\":\"{pname}\",\"pid\":\"{pid:016x}\",\"hops\":{hops},\"rg\":{region},\"prio\":{},\"qd\":{delay}{tj}}}",
            self.now, q.prio
        ));
        self.active.push(Tx { id: tx, sender: node, start: self.now, end, packet: q.packet });
        self.nodes[node].busy_until = end;
        self.schedule(end, Ev::TxEnd { tx });
        if !self.nodes[node].txq.is_empty() {
            self.want_service(node, end);
        }
    }

    fn tx_end(&mut self, tx: u64) {
        let now = self.now;
        self.active.retain(|t| t.end + TX_KEEP_MS > now);
        let Some(t) = self.active.iter().find(|t| t.id == tx) else { return };
        let (sender, start, end) = (t.sender, t.start, t.end);
        let packet = t.packet.clone();
        let overlapping: Vec<usize> = self
            .active
            .iter()
            .filter(|o| o.id != tx && o.start < end && o.end > start)
            .map(|o| o.sender)
            .collect();
        if !self.nodes[sender].alive() {
            return;
        }
        let pid = Packet::decode(&packet).map(|p| packet_id(p.header.ptype, &p.header.dest, p.payload)).unwrap_or(0);
        let tag = self.tags.get(&pid).copied();
        let r_sf = self.nodes[sender].sf;
        let (snr_min, noise) = {
            let r = self.radio(r_sf);
            (r.snr_min, r.noise_dbm)
        };
        let receivers: Vec<(u32, f32)> = self.nodes[sender].nbrs.clone();
        let mut ok = String::new();
        let mut coll = String::new();
        for (r, p) in receivers {
            let r = r as usize;
            let snr = p as f64 - noise;
            if snr < snr_min || !self.nodes[r].alive() {
                continue;
            }
            self.nodes[r].router.note_airtime(start, ((end - start) as u32).max(1));
            let lost = overlapping.iter().any(|&o| {
                o == r
                    || (o != sender
                        && self.nodes[o]
                            .gain_to(r as u32)
                            .is_some_and(|g| (g as f64 - noise) >= snr_min - 6.0 && g > p - 6.0))
            });
            if lost {
                self.stats.collisions += 1;
                let _ = write!(coll, "{}{r}", if coll.is_empty() { "" } else { "," });
                continue;
            }
            let _ = write!(ok, "{}{r}", if ok.is_empty() { "" } else { "," });
            let snr8 = snr.clamp(-30.0, 30.0) as i8;
            let before = self.nodes[r].router.stats;
            let actions = self.nodes[r].router.on_receive(&packet, snr8, now);
            self.after_router(r, before, tag);
            self.apply(r, actions.into_iter().collect(), tag);
        }
        let tj = self.tag_json(pid);
        self.tr(format!("{{\"t\":{now},\"ev\":\"rx\",\"id\":{tx},\"n\":{sender},\"pid\":\"{pid:016x}\",\"ok\":[{ok}],\"coll\":[{coll}]{tj}}}"));
    }

    // ---------- traffic ----------

    fn users(&self) -> Vec<usize> {
        (0..self.nodes.len()).filter(|&i| self.nodes[i].kind.is_user() && self.nodes[i].alive()).collect()
    }

    fn poisson_count(&mut self, per_s: f64) -> usize {
        let mut n = per_s.floor() as usize;
        if self.rng.gen_bool((per_s - per_s.floor()).clamp(0.0, 1.0)) {
            n += 1;
        }
        n
    }

    fn traffic_tick(&mut self) {
        let t = self.now as f64 / 1000.0;
        let tr = self.traffic.clone();
        if t < tr.start_s || (tr.stop_s > 0.0 && t > tr.stop_s) {
            return;
        }
        let users = self.users();
        if users.len() >= 2 {
            for _ in 0..self.poisson_count(tr.direct_per_min / 60.0) {
                let from = users[self.rng.gen_range(0..users.len())];
                let to = self.pick_dest(from, &users, &tr);
                if let Some(to) = to {
                    self.send_message(from, to);
                }
            }
            for _ in 0..self.poisson_count(tr.channel_per_min / 60.0) {
                let from = users[self.rng.gen_range(0..users.len())];
                self.send_channel(from);
            }
        }
        if tr.spam_per_min > 0.0 {
            for &s in &tr.spammers {
                if s < self.nodes.len() && self.nodes[s].alive() {
                    for _ in 0..self.poisson_count(tr.spam_per_min / 60.0) {
                        self.send_channel(s);
                    }
                }
            }
        }
    }

    fn pick_dest(&mut self, from: usize, users: &[usize], tr: &Traffic) -> Option<usize> {
        if self.rng.gen_bool(tr.local_share.clamp(0.0, 1.0)) {
            let (x, y) = (self.nodes[from].x, self.nodes[from].y);
            let lim = tr.local_km * 1000.0;
            let near: Vec<usize> = users
                .iter()
                .copied()
                .filter(|&u| u != from && (self.nodes[u].x - x).hypot(self.nodes[u].y - y) <= lim)
                .collect();
            if !near.is_empty() {
                return Some(near[self.rng.gen_range(0..near.len())]);
            }
        }
        for _ in 0..8 {
            let to = users[self.rng.gen_range(0..users.len())];
            if to != from {
                return Some(to);
            }
        }
        None
    }

    /// Sends a direct message (also used by the UI). Returns the message index.
    pub fn send_message(&mut self, from: usize, to: usize) -> usize {
        let m = self.msgs.len();
        let mut payload = format!("message {m} from {from} to {to}: ").into_bytes();
        payload.resize(60, b'.');
        let id = packet_id(PacketType::Data, &self.nodes[to].router.addr, &payload);
        self.msgs.push(Msg { from, to, payload: payload.clone(), id, sent_ms: self.now, delivered_ms: None, acked_ms: None });
        self.tags.insert(id, m as u32);
        self.tr(format!("{{\"t\":{},\"ev\":\"send\",\"msg\":{m},\"from\":{from},\"to\":{to},\"pid\":\"{id:016x}\"}}", self.now));
        if !self.nodes[from].alive() {
            return m;
        }
        let dest = self.nodes[to].router.addr;
        let mut token = [0u8; 16];
        self.rng.fill(&mut token[..]);
        let before = self.nodes[from].router.stats;
        if let Ok((_, actions)) = self.nodes[from].router.send_direct(dest, &payload, token, self.now) {
            self.after_router(from, before, Some(m as u32));
            self.apply(from, actions.into_iter().collect(), Some(m as u32));
        }
        m
    }

    fn send_channel(&mut self, from: usize) {
        let n = self.stats.channel_sent;
        let mut payload = format!("channel message {n} from {from}").into_bytes();
        payload.resize(40, b'.');
        self.stats.channel_sent += 1;
        self.stats.channel_receivers += self.users().len().saturating_sub(1) as u64;
        let ch = self.public_channel;
        if let Ok(actions) = self.nodes[from].router.send_channel(ch, &payload) {
            self.apply(from, actions.into_iter().collect(), None);
        }
    }

    /// Sends an announce from `node` (unsigned body of real size + a real stamp).
    /// Returns its packet id.
    pub fn announce(&mut self, node: usize) -> u64 {
        let addr = self.nodes[node].router.addr;
        let mut body: heapless::Vec<u8, { maarse_core::MAX_PACKET }> = heapless::Vec::new();
        let mut keys = [0u8; 64];
        self.rng.fill(&mut keys[..]);
        let _ = body.extend_from_slice(&keys);
        let _ = body.extend_from_slice(&((self.now / 1000) as u32 + 1_700_000_000).to_be_bytes());
        let _ = body.push(0);
        let _ = body.extend_from_slice(&[0u8; 64]);
        maarse_core::stamp::append(PacketType::Announce, &addr, &mut body);
        let header = Header::new(PacketType::Announce, addr);
        let Ok(packet) = encode(&header, &body) else { return 0 };
        let pid = packet_id(PacketType::Announce, &addr, &body);
        self.announces.push((node, pid, self.now));
        self.tr(format!("{{\"t\":{},\"ev\":\"announce\",\"n\":{node},\"pid\":\"{pid:016x}\"}}", self.now));
        if self.nodes[node].alive() {
            self.apply(node, vec![Action::Transmit { packet, delay_ms: 0, id: pid }], None);
        }
        pid
    }

    // ---------- alerts ----------

    pub fn send_alert(&mut self, from: usize, kind: &str, area: &Area, text: &str) -> Option<usize> {
        if from >= self.nodes.len() {
            return None;
        }
        let (poly, circle) = area_xy(area);
        let idx = self.alerts.len();
        // Plaintext: "A" | alert index (u16) | kind (u8) | area | text.
        // Area: 'C' lat lon (i32 1e-5 deg) r (u32 m) or 'P' n (u8) n x (lat lon).
        let mut t: Vec<u8> = vec![b'A'];
        t.extend_from_slice(&(idx as u16).to_be_bytes());
        t.push(match kind {
            "evacuation" => 1,
            "fire" => 2,
            "flood" => 3,
            "water" => 4,
            _ => 0,
        });
        if let Some([la, lo, r]) = area.circle {
            t.push(b'C');
            t.extend_from_slice(&((la * 1e5) as i32).to_be_bytes());
            t.extend_from_slice(&((lo * 1e5) as i32).to_be_bytes());
            t.extend_from_slice(&(r as u32).to_be_bytes());
        } else {
            let pts: Vec<&[f64; 2]> = area.polygon.iter().take(12).collect();
            t.push(b'P');
            t.push(pts.len() as u8);
            for [la, lo] in pts {
                t.extend_from_slice(&((la * 1e5) as i32).to_be_bytes());
                t.extend_from_slice(&((lo * 1e5) as i32).to_be_bytes());
            }
        }
        let room = 150usize.saturating_sub(t.len() + bulletin::OVERHEAD);
        t.extend(text.bytes().take(room));
        let mut out = [0u8; 255];
        let n = bulletin::sign(&self.authority, 1_700_000_000 + (self.now / 1000) as u32, &t, &mut out)?;
        let inside: Vec<usize> = (0..self.nodes.len())
            .filter(|&i| {
                let (x, y) = (self.nodes[i].x, self.nodes[i].y);
                if let Some((cx, cy, r)) = circle { (x - cx).hypot(y - cy) <= r } else { point_in_poly(x, y, &poly) }
            })
            .collect();
        let region = if self.sc.features.regions { self.alert_region(&inside) } else { 0 };
        let payload = &out[..n];
        let pid = packet_id(PacketType::Channel, &self.bulletin_channel, payload);
        self.alerts.push(Alert {
            from,
            kind: kind.to_string(),
            text: text.to_string(),
            sent_ms: self.now,
            poly,
            circle,
            pid,
            recv: HashMap::new(),
            inside,
        });
        self.tags.insert(pid, idx as u32 | ALERT_TAG);
        self.tr(format!("{{\"t\":{},\"ev\":\"alert\",\"alert\":{idx},\"n\":{from},\"kind\":\"{kind}\",\"pid\":\"{pid:016x}\",\"rg\":{region}}}", self.now));
        let ch = self.bulletin_channel;
        if self.nodes[from].alive()
            && let Ok(actions) = self.nodes[from].router.send_channel_in(ch, region, payload)
        {
            self.apply(from, actions.into_iter().collect(), Some(idx as u32 | ALERT_TAG));
        }
        Some(idx)
    }

    /// The region for an alert: the one region all nodes in the area share, else `dk`.
    fn alert_region(&self, inside: &[usize]) -> u16 {
        let mut names: Vec<&str> = inside.iter().map(|&i| self.nodes[i].region.as_str()).filter(|s| !s.is_empty()).collect();
        names.sort_unstable();
        names.dedup();
        match names.as_slice() {
            [one] => maarse_core::region::region_id(one),
            _ => maarse_core::region::region_id("dk"),
        }
    }

    // ---------- timeline ----------

    pub fn inject(&mut self, ev: &TimedEvent) {
        let what = ev.kind.as_str();
        self.tr(format!("{{\"t\":{},\"ev\":\"event\",\"what\":\"{what}\",\"region\":\"{}\"}}", self.now, ev.region));
        match what {
            "internet_down" => {
                if ev.region.is_empty() {
                    self.internet_global = false;
                } else {
                    self.internet_down.insert(ev.region.clone());
                }
            }
            "internet_up" => {
                if ev.region.is_empty() {
                    self.internet_global = true;
                    self.internet_down.clear();
                } else {
                    self.internet_down.remove(&ev.region);
                }
            }
            "power_cut" => {
                let (poly, circle) = area_xy(&ev.area);
                for i in 0..self.nodes.len() {
                    let (x, y) = (self.nodes[i].x, self.nodes[i].y);
                    let hit = if let Some((cx, cy, r)) = circle { (x - cx).hypot(y - cy) <= r } else { point_in_poly(x, y, &poly) };
                    if hit && self.nodes[i].mains {
                        self.nodes[i].powered = false;
                        if self.nodes[i].battery_wh <= 0.0 && self.nodes[i].alive() {
                            self.kill(i, DeadWhy::Power);
                        }
                    }
                }
                self.update_energy_levels();
            }
            "power_restore" => {
                for i in 0..self.nodes.len() {
                    if self.nodes[i].mains {
                        self.nodes[i].powered = true;
                        if matches!(self.nodes[i].dead, DeadWhy::Power | DeadWhy::Battery) {
                            self.nodes[i].dead = DeadWhy::Alive;
                        }
                    }
                }
                self.update_energy_levels();
                self.seed_neighbors();
            }
            "kill_random" => {
                let want = if ev.node_kind.is_empty() { "repeater" } else { ev.node_kind.as_str() };
                let mut pool: Vec<usize> =
                    (0..self.nodes.len()).filter(|&i| self.nodes[i].alive() && (want == "any" || self.nodes[i].kind.name() == want)).collect();
                for _ in 0..ev.n.min(pool.len()) {
                    let k = self.rng.gen_range(0..pool.len());
                    let i = pool.swap_remove(k);
                    self.kill(i, DeadWhy::Killed);
                }
            }
            "kill" => {
                if ev.node < self.nodes.len() {
                    self.kill(ev.node, DeadWhy::Killed);
                }
            }
            "revive" => {
                for n in &mut self.nodes {
                    if n.dead == DeadWhy::Killed {
                        n.dead = DeadWhy::Alive;
                    }
                }
                self.seed_neighbors();
            }
            "link" => {
                if let Some(l) = self.links.get_mut(ev.index) {
                    l.up = ev.up;
                }
            }
            "alert" => {
                let text = if ev.text.is_empty() { ev.alert.clone() } else { ev.text.clone() };
                self.send_alert(ev.from, &ev.alert, &ev.area, &text);
            }
            "announce" => {
                if ev.node < self.nodes.len() {
                    self.announce(ev.node);
                }
            }
            "set_traffic" => {
                if let Some(t) = &ev.traffic {
                    self.traffic = t.clone();
                }
            }
            _ => {}
        }
    }

    fn kill(&mut self, i: usize, why: DeadWhy) {
        if self.nodes[i].alive() {
            self.nodes[i].dead = why;
            self.nodes[i].txq.clear();
            let w = match why {
                DeadWhy::Power => "power",
                DeadWhy::Battery => "battery",
                _ => "killed",
            };
            self.tr(format!("{{\"t\":{},\"ev\":\"dead\",\"n\":{i},\"why\":\"{w}\"}}", self.now));
        }
    }

    // ---------- energy and movement ----------

    fn on_battery(&self, i: usize) -> bool {
        let n = &self.nodes[i];
        !(n.mains && n.powered)
    }

    fn update_energy_levels(&mut self) {
        for i in 0..self.nodes.len() {
            let on_batt = self.on_battery(i);
            let n = &mut self.nodes[i];
            n.router.energy = if !on_batt || n.battery_max <= 0.0 {
                Energy::Full
            } else {
                let f = n.battery_wh / n.battery_max;
                if f > 0.6 {
                    Energy::Full
                } else if f > 0.3 {
                    Energy::Ok
                } else if f > 0.1 {
                    Energy::Low
                } else {
                    Energy::Critical
                }
            };
        }
    }

    fn energy_tick(&mut self, dt_s: f64) {
        for i in 0..self.nodes.len() {
            if !self.nodes[i].alive() || !self.on_battery(i) || self.nodes[i].battery_max <= 0.0 {
                continue;
            }
            let n = &mut self.nodes[i];
            // Idle draw; the transmitter (about 1 W at 20 dBm) is in airtime_us.
            n.battery_wh -= n.kind.idle_w() * dt_s / 3600.0;
            if n.battery_wh <= 0.0 {
                n.battery_wh = 0.0;
                self.kill(i, DeadWhy::Battery);
            }
        }
        self.update_energy_levels();
    }

    fn move_nodes(&mut self) {
        let movers: Vec<usize> = (0..self.nodes.len()).filter(|&i| self.nodes[i].route.len() >= 2).collect();
        for i in movers {
            let n = &mut self.nodes[i];
            let segs: Vec<f64> = n.route.windows(2).map(|w| (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1)).collect();
            let total: f64 = segs.iter().sum();
            if total <= 0.0 {
                continue;
            }
            n.route_pos += n.route_dir * n.speed * 2.0;
            if n.route_pos >= total {
                n.route_pos = total;
                n.route_dir = -1.0;
            } else if n.route_pos <= 0.0 {
                n.route_pos = 0.0;
                n.route_dir = 1.0;
            }
            let mut s = n.route_pos;
            let mut p = n.route[0];
            for (k, l) in segs.iter().enumerate() {
                if s <= *l || k == segs.len() - 1 {
                    let f = if *l > 0.0 { (s / l).min(1.0) } else { 0.0 };
                    let (a, b) = (n.route[k], n.route[k + 1]);
                    p = (a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f);
                    break;
                }
                s -= l;
            }
            let old = Self::cell(n.x, n.y);
            n.x = p.0;
            n.y = p.1;
            let new = Self::cell(p.0, p.1);
            if old != new {
                if let Some(v) = self.grid.get_mut(&old) {
                    v.retain(|&j| j as usize != i);
                }
                self.grid.entry(new).or_default().push(i as u32);
            }
            self.relink(i);
        }
    }

    // ---------- queries ----------

    pub fn summary(&self) -> Value {
        let n = self.msgs.len();
        let delivered: Vec<u64> = self.msgs.iter().filter_map(|m| m.delivered_ms.map(|d| d - m.sent_ms)).collect();
        let mut lat = delivered.clone();
        lat.sort_unstable();
        let acked = self.msgs.iter().filter(|m| m.acked_ms.is_some()).count();
        let pct = |a: usize| if n > 0 { a as f64 / n as f64 * 100.0 } else { 0.0 };
        let sum = |f: fn(&maarse_core::router::RouterStats) -> u32| self.nodes.iter().map(|x| f(&x.router.stats) as u64).sum::<u64>();
        let busiest = self.nodes.iter().map(|x| x.airtime_us).max().unwrap_or(0);
        let qd: Vec<Value> = self
            .stats
            .qdelay
            .iter()
            .map(|(s, c, m)| json!({"mean_ms": if *c > 0 { *s as f64 / *c as f64 } else { 0.0 }, "count": c, "max_ms": m}))
            .collect();
        json!({
            "t": self.now,
            "nodes": self.nodes.len(),
            "alive": self.nodes.iter().filter(|x| x.alive()).count(),
            "messages": n,
            "delivered_pct": pct(delivered.len()),
            "acked_pct": pct(acked),
            "latency_median_ms": lat.get(lat.len() / 2).copied().unwrap_or(0),
            "latency_p90_ms": lat.get(lat.len() * 9 / 10).copied().unwrap_or(0),
            "tx": self.stats.tx_count,
            "airtime_s": self.nodes.iter().map(|x| x.airtime_us).sum::<u64>() as f64 / 1e6,
            "busiest_airtime_s": busiest as f64 / 1e6,
            "collisions": self.stats.collisions,
            "duty_drops": self.stats.duty_drops,
            "duty_hits": self.nodes.iter().map(|x| x.duty_hits as u64).sum::<u64>(),
            "busy_drops": self.stats.busy_drops,
            "cancelled": self.stats.cancelled,
            "backbone_packets": self.stats.backbone_packets,
            "scope_dropped": self.stats.scope_dropped,
            "channel_sent": self.stats.channel_sent,
            "channel_reach_pct": if self.stats.channel_receivers > 0 { self.stats.channel_deliveries as f64 / self.stats.channel_receivers as f64 * 100.0 } else { 0.0 },
            "local_repairs": sum(|s| s.local_repairs),
            "hop_retries": sum(|s| s.hop_retries),
            "carry_handoffs": sum(|s| s.carry_handoffs),
            "carry_delivered": sum(|s| s.carry_delivered),
            "routes_learned": sum(|s| s.routes_learned),
            "path_requests": sum(|s| s.path_requests),
            "quota_dropped": sum(|s| s.quota_dropped),
            "queue_delay": qd,
            "by_type": self.stats.by_type,
            "internet": self.internet_global,
            "internet_down": self.internet_down.iter().collect::<Vec<_>>(),
            "alerts": (0..self.alerts.len()).map(|i| self.alert_summary(i, false)).collect::<Vec<_>>(),
            "trace_end": self.trace_base + self.trace.len() as u64,
        })
    }

    pub fn alert_summary(&self, i: usize, per_node: bool) -> Value {
        let a = &self.alerts[i];
        let alive_inside: Vec<usize> = a.inside.iter().copied().filter(|&n| self.nodes[n].alive()).collect();
        let got: Vec<u64> = alive_inside.iter().filter_map(|n| a.recv.get(n).map(|t| t - a.sent_ms)).collect();
        let mut lat = got.clone();
        lat.sort_unstable();
        let outside = a.recv.keys().filter(|n| !a.inside.contains(n)).count();
        let mut v = json!({
            "index": i, "kind": a.kind, "text": a.text, "from": a.from, "sent_ms": a.sent_ms,
            "inside": a.inside.len(), "inside_alive": alive_inside.len(), "inside_received": got.len(),
            "coverage_pct": if alive_inside.is_empty() { 0.0 } else { got.len() as f64 / alive_inside.len() as f64 * 100.0 },
            "latency_median_ms": lat.get(lat.len() / 2).copied().unwrap_or(0),
            "latency_p90_ms": lat.get(lat.len() * 9 / 10).copied().unwrap_or(0),
            "outside_received": outside,
            "pid": format!("{:016x}", a.pid),
        });
        if per_node {
            let recv: Vec<Value> = a.recv.iter().map(|(n, t)| json!([n, t - a.sent_ms, a.inside.contains(n)])).collect();
            v["recv"] = json!(recv);
            v["inside_nodes"] = json!(a.inside);
        }
        v
    }

    /// Compact per-node state for the map.
    pub fn node_state(&self) -> Value {
        let comp = self.components();
        let mut alive = String::with_capacity(self.nodes.len());
        let mut x = Vec::with_capacity(self.nodes.len());
        let mut y = Vec::with_capacity(self.nodes.len());
        let mut batt = Vec::with_capacity(self.nodes.len());
        let mut air = Vec::with_capacity(self.nodes.len());
        let mut q = Vec::with_capacity(self.nodes.len());
        for (i, n) in self.nodes.iter().enumerate() {
            alive.push(match n.dead {
                DeadWhy::Alive if self.on_battery(i) && n.mains => 'b',
                DeadWhy::Alive => 'a',
                DeadWhy::Power => 'p',
                DeadWhy::Battery => 'e',
                DeadWhy::Killed => 'k',
            });
            let (la, lo) = to_ll(n.x, n.y);
            x.push((la * 1e5).round() / 1e5);
            y.push((lo * 1e5).round() / 1e5);
            batt.push(if n.battery_max > 0.0 { (n.battery_wh / n.battery_max * 100.0).round() as i32 } else { -1 });
            air.push((n.airtime_us / 1000) as u64);
            q.push(n.txq.len());
        }
        let links: Vec<bool> = (0..self.links.len()).map(|k| self.link_ok(k)).collect();
        json!({"state": alive, "lat": x, "lon": y, "battery": batt, "airtime_ms": air, "queue": q, "comp": comp, "links_ok": links})
    }

    /// Connected components ("islands") of the forwarding graph: forwarders
    /// that hear each other both ways, plus working backbone links; users join
    /// the component of a forwarder they hear (or stay alone).
    pub fn components(&self) -> Vec<i32> {
        let n = self.nodes.len();
        let mut parent: Vec<usize> = (0..n).collect();
        fn find(p: &mut [usize], mut i: usize) -> usize {
            while p[i] != i {
                p[i] = p[p[i]];
                i = p[i];
            }
            i
        }
        let fwd = |i: usize| self.nodes[i].alive() && self.nodes[i].router.role.forwards();
        for i in 0..n {
            if !fwd(i) {
                continue;
            }
            for &(j, rx) in &self.nodes[i].nbrs {
                let j = j as usize;
                if j > i && fwd(j) && self.audible_rx(i, rx) {
                    let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                    parent[a] = b;
                }
            }
        }
        for k in 0..self.links.len() {
            if self.sc.features.backbone && self.link_ok(k) {
                let (a, b) = (find(&mut parent, self.links[k].a), find(&mut parent, self.links[k].b));
                parent[a] = b;
            }
        }
        for i in 0..n {
            if fwd(i) || !self.nodes[i].alive() {
                continue;
            }
            let best = self.nodes[i]
                .nbrs
                .iter()
                .filter(|&&(j, rx)| fwd(j as usize) && self.audible_rx(i, rx))
                .max_by(|a, b| a.1.total_cmp(&b.1));
            if let Some(&(j, _)) = best {
                let r = find(&mut parent, j as usize);
                parent[i] = r;
            }
        }
        (0..n).map(|i| if self.nodes[i].alive() { find(&mut parent, i) as i32 } else { -1 }).collect()
    }

    pub fn inspect(&self, i: usize) -> Value {
        let n = &self.nodes[i];
        let rt = &n.router;
        let ids: HashMap<u16, usize> = self.nodes.iter().enumerate().map(|(k, x)| (x.router.id, k)).collect();
        let addrs: HashMap<Address, usize> = self.nodes.iter().enumerate().map(|(k, x)| (x.router.addr, k)).collect();
        let nb: Vec<Value> = rt
            .neighbors
            .sorted()
            .iter()
            .map(|e| json!({"node": ids.get(&e.id), "snr": e.snr, "last_s": e.last_ms as f64 / 1000.0, "count": e.count, "role": e.role.map(|r| format!("{r:?}"))}))
            .collect();
        let routes: Vec<Value> = rt
            .routes()
            .iter()
            .map(|r| json!({"dest": addrs.get(&r.dest), "path": r.path.iter().map(|p| ids.get(p)).collect::<Vec<_>>(), "cost": r.cost, "learned_s": r.learned_ms as f64 / 1000.0, "last_s": r.last_seen_ms as f64 / 1000.0}))
            .collect();
        let queue: Vec<Value> = n.txq.iter().map(|q| json!({"prio": q.prio, "waited_ms": self.now - q.enq, "len": q.packet.len(), "tries": q.tries})).collect();
        let radio_nbrs: Vec<Value> = n
            .nbrs
            .iter()
            .filter(|&&(_, rx)| self.audible_rx(i, rx))
            .map(|&(j, rx)| json!([j, (self.radio(n.sf).snr(rx as f64) * 10.0).round() / 10.0]))
            .collect();
        let s = rt.stats;
        let (la, lo) = to_ll(n.x, n.y);
        json!({
            "index": i, "kind": n.kind.name(), "label": n.label, "role": format!("{:?}", rt.role), "sf": n.sf, "h": n.h,
            "lat": la, "lon": lo, "region": n.region, "state": format!("{:?}", n.dead), "powered": n.powered, "mains": n.mains,
            "on_battery": self.on_battery(i), "battery_wh": n.battery_wh, "battery_max_wh": n.battery_max,
            "energy": format!("{:?}", rt.energy), "internet": self.has_internet(i), "mobile": rt.mobile,
            "carry_copies": rt.carry_copies, "carried": rt.carried_count(), "local_repair": rt.local_repair,
            "slot_ms": rt.slot_ms,
            "airtime_s": n.airtime_us as f64 / 1e6,
            "duty_budget_used_pct": n.airtime_us as f64 / 1000.0 / (self.sc.duty_permille as f64 / 1000.0 * self.now.max(1) as f64) * 100.0,
            "duty_hits": n.duty_hits,
            "neighbors": nb, "radio_neighbors": radio_nbrs, "routes": routes, "queue": queue,
            "links": n.links.iter().map(|&k| json!({"index": k, "peer": if self.links[k].a == i { self.links[k].b } else { self.links[k].a }, "kind": if self.links[k].wifi { "wifi" } else { "tcp" }, "ok": self.link_ok(k)})).collect::<Vec<_>>(),
            "stats": {
                "rx_packets": s.rx_packets, "rx_duplicates": s.rx_duplicates, "forwarded": s.forwarded, "originated": s.originated,
                "delivered": s.delivered, "acked": s.acked, "routes_learned": s.routes_learned, "path_requests": s.path_requests,
                "hop_retries": s.hop_retries, "local_repairs": s.local_repairs, "carry_handoffs": s.carry_handoffs,
                "carry_delivered": s.carry_delivered, "carry_notices": s.carry_notices, "floods_cancelled": s.floods_cancelled,
                "bad_stamps": s.bad_stamps, "quota_dropped": s.quota_dropped, "quota_held": s.quota_held,
            },
        })
    }

    /// Trace lines from sequence number `since` (at most `max`), optionally filtered.
    pub fn events(&self, since: u64, max: usize, filter: &dyn Fn(&str) -> bool) -> (u64, Vec<&str>) {
        let start = since.max(self.trace_base);
        let off = (start - self.trace_base) as usize;
        let mut out = Vec::new();
        let mut k = off;
        while k < self.trace.len() && out.len() < max {
            if filter(&self.trace[k]) {
                out.push(self.trace[k].as_str());
            }
            k += 1;
        }
        (self.trace_base + k as u64, out)
    }

    /// How far a flood reached: for every node that heard packet `pid`, the
    /// fewest hops, whether it forwarded, and why it stopped if not.
    pub fn reach(&self, pid: &str) -> Value {
        let key = format!("\"pid\":\"{pid}\"");
        let mut hops: HashMap<usize, u8> = HashMap::new();
        let mut first: HashMap<usize, u64> = HashMap::new();
        let mut forwarded: HashSet<usize> = HashSet::new();
        let mut cancelled: HashSet<usize> = HashSet::new();
        let mut dropped: HashMap<usize, String> = HashMap::new();
        let mut tx_hops: HashMap<u64, (usize, u8)> = HashMap::new();
        let mut origin = None;
        let mut region = 0u16;
        let mut collisions = 0usize;
        for line in &self.trace {
            if !line.contains(&key) {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
            let t = v["t"].as_u64().unwrap_or(0);
            let n = v["n"].as_u64().unwrap_or(0) as usize;
            match v["ev"].as_str().unwrap_or("") {
                "tx" => {
                    let h = v["hops"].as_u64().unwrap_or(0) as u8;
                    region = v["rg"].as_u64().unwrap_or(0) as u16;
                    if h == 0 && origin.is_none() {
                        origin = Some(n);
                        hops.insert(n, 0);
                        first.insert(n, t);
                    } else {
                        forwarded.insert(n);
                    }
                    tx_hops.insert(v["id"].as_u64().unwrap_or(0), (n, h));
                }
                "rx" => {
                    let (_, h) = tx_hops.get(&v["id"].as_u64().unwrap_or(u64::MAX)).copied().unwrap_or((n, 0));
                    for r in v["ok"].as_array().into_iter().flatten() {
                        let r = r.as_u64().unwrap_or(0) as usize;
                        let e = hops.entry(r).or_insert(h + 1);
                        *e = (*e).min(h + 1);
                        first.entry(r).or_insert(t);
                    }
                    collisions += v["coll"].as_array().map_or(0, |a| a.len());
                }
                "bb" => {
                    let to = v["to"].as_u64().unwrap_or(0) as usize;
                    let from = v["from"].as_u64().unwrap_or(0) as usize;
                    let h = hops.get(&from).copied().unwrap_or(0);
                    hops.entry(to).or_insert(h + 1);
                    first.entry(to).or_insert(t);
                }
                "cancel" => {
                    cancelled.insert(n);
                }
                "drop" => {
                    dropped.insert(n, v["why"].as_str().unwrap_or("").to_string());
                }
                _ => {}
            }
        }
        let mut nodes: Vec<Value> = Vec::new();
        for (&n, &h) in &hops {
            let why = if Some(n) == origin {
                "origin".to_string()
            } else if forwarded.contains(&n) {
                "forwarded".into()
            } else if let Some(w) = dropped.get(&n) {
                w.clone()
            } else if cancelled.contains(&n) {
                "suppressed (neighbours covered the area)".into()
            } else if !self.nodes[n].router.role.forwards() {
                "role does not forward".into()
            } else if region != 0 && !self.nodes[n].router.forwards_region(region) {
                "outside region".into()
            } else if h as usize >= maarse_core::MAX_HOPS as usize {
                "hop limit".into()
            } else if !self.nodes[n].alive() {
                "node dead".into()
            } else {
                "not forwarded (duplicate / not needed)".into()
            };
            nodes.push(json!([n, h, first.get(&n).copied().unwrap_or(0), why]));
        }
        json!({"pid": pid, "origin": origin, "region": region, "heard": hops.len(), "forwarded": forwarded.len(), "collisions": collisions, "nodes": nodes})
    }

    pub fn msg_list(&self, from: usize, max: usize) -> Value {
        let v: Vec<Value> = self
            .msgs
            .iter()
            .enumerate()
            .skip(from)
            .take(max)
            .map(|(i, m)| json!([i, m.from, m.to, m.sent_ms, m.delivered_ms, m.acked_ms, format!("{:016x}", m.id)]))
            .collect();
        json!({"total": self.msgs.len(), "msgs": v})
    }

    /// Every trace line about message `m` (sorted by time) for "follow a message".
    pub fn follow(&self, m: usize) -> Value {
        let k1 = format!("\"msg\":{m},");
        let k2 = format!("\"msg\":{m}}}");
        let lines: Vec<Value> = self
            .trace
            .iter()
            .filter(|l| l.contains(&k1) || l.contains(&k2))
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        let info = self.msgs.get(m).map(|x| json!({"from": x.from, "to": x.to, "sent_ms": x.sent_ms, "delivered_ms": x.delivered_ms, "acked_ms": x.acked_ms, "pid": format!("{:016x}", x.id)}));
        json!({"msg": m, "info": info, "events": lines})
    }
}

/// Alert index from a verified bulletin text written by `send_alert`.
fn parse_area_index(text: &[u8]) -> Option<usize> {
    if text.len() < 4 || text[0] != b'A' {
        return None;
    }
    Some(u16::from_be_bytes([text[1], text[2]]) as usize)
}

