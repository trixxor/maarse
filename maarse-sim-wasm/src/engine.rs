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
//!   message tags so one message can be followed through the network, the
//!   source path and intended next hop of every routed transmission, the SNR
//!   of every reception and the cause of every collision;
//! * clutter areas (forest) and radio degradation in an area (storm, jamming);
//! * walkers that wander around a point;
//! * an optional DHT-directed backbone for path requests (`dht_backbone`);
//! * reboots (RAM lost; known contacts lost unless `persist_known`), with
//!   messages counted as *shown* only when the recipient knows the sender;
//! * an optional "home routing" model (`home_routing`): fixed infrastructure,
//!   home repeaters and a location lookup at the gateway, see [`Engine::send_message`].

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
use std::collections::BinaryHeap;
// Deterministic maps: std's RandomState gives every new map other keys (and
// each Sim in the same process a different iteration order), so results
// depended on what ran before in the process.
type DetHash = std::hash::BuildHasherDefault<std::collections::hash_map::DefaultHasher>;
type HashMap<K, V> = std::collections::HashMap<K, V, DetHash>;
type HashSet<K> = std::collections::HashSet<K, DetHash>;
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
/// Home routing: links between fixed nodes count as infrastructure when their
/// SNR is this far above the limit (a margin, like a planned link).
const INFRA_MARGIN_DB: f64 = 5.0;
/// Home routing: a lookup without an answer is asked again after this long
/// (a datagram's last hop has no retransmission), at most `LOOKUP_TRIES`
/// times; then the sender falls back to a path request.
const LOOKUP_RETRY_MS: u64 = 4_000;
const LOOKUP_TRIES: u8 = 3;

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
    /// Wandering: radius (m) around `home`, and the current target point.
    wander: f64,
    home: (f64, f64),
    target: Option<(f64, f64)>,
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
    /// LoRa trunk (see `LinkSpec`): airtime, duty cycle and loss apply.
    lora: bool,
    sf: u8,
    snr: i8,
    loss: f64,
    duty: u32,
    /// The trunk channel is busy (sending or in its duty pause) until then.
    busy_until: u64,
    label: String,
    group: String,
    packets: u64,
    airtime_ms: u64,
    dropped: u64,
}

struct Tx {
    id: u64,
    sender: usize,
    start: u64,
    end: u64,
    packet: Buf,
    /// Intended next hop of a routed packet (-1: a flood, or unknown).
    nx: i64,
    /// A flood (no source path): copies at other nodes are redundant.
    flood: bool,
    /// A trunk transmission on the mesh channel: it occupies the channel and
    /// interferes, but mesh nodes do not receive it (directional antenna).
    noise: bool,
}

/// An area with extra radio loss (clutter or degradation), in metres.
struct LossArea {
    poly: Vec<(f64, f64)>,
    circle: Option<(f64, f64, f64)>,
    /// Bounding box (min x, min y, max x, max y).
    bbox: (f64, f64, f64, f64),
    db: f64,
    /// Canopy model (clutter only): height of the trees/roofs and the
    /// saturation loss; `None` = linear model.
    canopy: Option<(f64, f64)>,
}

impl LossArea {
    fn new(poly: Vec<(f64, f64)>, circle: Option<(f64, f64, f64)>, db: f64) -> LossArea {
        let bbox = if let Some((x, y, r)) = circle {
            (x - r, y - r, x + r, y + r)
        } else {
            poly.iter().fold((f64::MAX, f64::MAX, f64::MIN, f64::MIN), |b, p| (b.0.min(p.0), b.1.min(p.1), b.2.max(p.0), b.3.max(p.1)))
        };
        LossArea { poly, circle, bbox, db, canopy: None }
    }
    fn contains(&self, x: f64, y: f64) -> bool {
        if x < self.bbox.0 || y < self.bbox.1 || x > self.bbox.2 || y > self.bbox.3 {
            return false;
        }
        if let Some((cx, cy, r)) = self.circle {
            return (x - cx).hypot(y - cy) <= r;
        }
        point_in_poly(x, y, &self.poly)
    }
    /// Loss (dB) of the link a-b with antenna heights ha, hb.
    fn loss(&self, a: (f64, f64), b: (f64, f64), ha: f64, hb: f64) -> f64 {
        match self.canopy {
            None => self.inside_len(a, b, None) / 1000.0 * self.db,
            Some((canopy, max_db)) => {
                // Per end (ITU-R P.833 terminal loss): the signal leaves the
                // canopy near one antenna and enters it again near the other,
                // so each half saturates on its own.
                let half = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
                let hm = (ha + hb) / 2.0;
                let gamma = self.db / 1000.0;
                let sat = |d: f64| if d > 0.0 { max_db * (1.0 - (-gamma * d / max_db).exp()) } else { 0.0 };
                sat(self.inside_len(a, half, Some((ha, hm, canopy)))) + sat(self.inside_len(half, b, Some((hm, hb, canopy))))
            }
        }
    }

    /// Length (m) of the segment a-b that runs inside the area, by sampling;
    /// with `below` = (ha, hb, canopy) only where the straight line between
    /// the antennas is below the canopy.
    fn inside_len(&self, a: (f64, f64), b: (f64, f64), below: Option<(f64, f64, f64)>) -> f64 {
        let (lo_x, hi_x) = (a.0.min(b.0), a.0.max(b.0));
        let (lo_y, hi_y) = (a.1.min(b.1), a.1.max(b.1));
        if hi_x < self.bbox.0 || hi_y < self.bbox.1 || lo_x > self.bbox.2 || lo_y > self.bbox.3 {
            return 0.0;
        }
        let d = (a.0 - b.0).hypot(a.1 - b.1);
        let n = ((d / 250.0).ceil() as usize).clamp(4, 64);
        let inside = (0..n)
            .filter(|&k| {
                let f = (k as f64 + 0.5) / n as f64;
                if let Some((ha, hb, canopy)) = below
                    && ha + (hb - ha) * f >= canopy
                {
                    return false;
                }
                self.contains(a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f)
            })
            .count();
        d * inside as f64 / n as f64
    }
}

#[allow(clippy::large_enum_variant)]
enum Ev {
    /// Sleepy handhelds wake or fall asleep.
    SleepTick,
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
    LookupTimeout { msg: usize },
    Announce { node: usize },
    /// Give up on a message whose recipient's key never came (key request).
    KeyWait { msg: usize },
    /// A packet arrives over a LoRa trunk (sent from `start`).
    Trunk { link: usize, to: usize, packet: Buf, start: u64 },
    /// 45 s after a message: drop it at a post office if not acknowledged.
    MailCheck { msg: usize },
    /// Post office upkeep: home office changes, hints, periodic syncs.
    MailTick,
    /// A node asks for its mail (after it came back).
    MailSyncAt { node: usize },
    /// Mail between two hosts over a direct TCP connection (never radio).
    MailTcp { to: usize, payload: Vec<u8> },
}

/// Copies a sibling sends per sync request.
const BACKFILL_MAX: usize = 6;

pub struct Msg {
    pub from: usize,
    pub to: usize,
    pub payload: Vec<u8>,
    pub id: u64,
    pub sent_ms: u64,
    pub delivered_ms: Option<u64>,
    pub acked_ms: Option<u64>,
    /// Shown to the recipient (it knew the sender, now or after its announce).
    pub shown_ms: Option<u64>,
    /// Dropped by the recipient: unknown sender, nothing kept.
    pub dropped_unknown: bool,
    /// Never sent: the sender no longer knew the recipient's keys.
    pub unsent: bool,
    /// When sent, exactly one end heard a repeater (into or out of an area
    /// without coverage: what edge bridges are for).
    pub cross: bool,
    /// A sibling copy: the index of the original message.
    pub sync_of: Option<usize>,
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
    /// Collisions split by what was lost: a copy of a flood (redundant), a
    /// routed packet at a bystander, or a routed packet at its intended next
    /// hop (the ones that cost a retransmission). Half-duplex: the receiver
    /// was transmitting itself.
    pub coll_flood: u64,
    pub coll_next_hop: u64,
    pub coll_half_duplex: u64,
    pub dht_lookups: u64,
    pub dht_misses: u64,
    /// Home routing: lookups asked, answered with a route, known unreachable,
    /// and fallbacks to a path request (by reason).
    pub home_lookups: u64,
    pub home_routed: u64,
    pub home_unreachable: u64,
    /// Lookups answered by the sender's own home (local registry).
    pub home_local: u64,
    pub reboots: u64,
    pub periodic_announces: u64,
    /// Automatic forest mode: role changes.
    pub role_switches: u64,
    pub home_fallback: std::collections::BTreeMap<String, u64>,
    /// Queue delay per priority (0..=4): sum ms, count, max.
    pub qdelay: [(u64, u64, u64); 5],
    pub by_type: std::collections::BTreeMap<String, u64>,
    /// Airtime per packet type (ms, all transmissions).
    pub air_by_type: std::collections::BTreeMap<String, u64>,
    pub key_requests: u64,
    pub key_answers: u64,
    pub bordercast_kept: u64,
    pub trunk_packets: u64,
    pub trunk_dropped: u64,
    pub trunk_jammed: u64,
    pub dup_queued: u64,
    pub wakeups: u64,
    pub mail_deposits: u64,
    pub mail_stored: u64,
    pub mail_full: u64,
    pub mail_ops: u64,
    pub mail_deliveries: u64,
    pub mail_syncs: u64,
    pub mail_tcp: u64,
    pub mail_deferred: u64,
    pub mail_retries: u64,
    pub mail_refetches: u64,
    pub mail_consolidated: u64,
    pub redirect_hints: u64,
    pub office_carried: u64,
    pub sync_presses: u64,
    pub backfilled: u64,
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
    /// Path id / address -> node index (for readable traces).
    ids: HashMap<u16, usize>,
    addr_idx: HashMap<Address, usize>,
    clutter: Vec<LossArea>,
    degrade: Vec<LossArea>,
    /// DHT backbone: gateway host per node (-1 = none), recomputed when dirty.
    gw: Vec<i32>,
    gw_dirty: bool,
    /// Direct backbone connections made after a DHT lookup.
    dyn_links: HashSet<(usize, usize)>,
    /// Path request reply token -> tag, so the path response is traced too.
    token_tags: HashMap<Address, u32>,
    /// Home routing model: infrastructure links between fixed nodes, the home
    /// (fixed) node of every node, radio islands, and open lookups.
    infra: Vec<Vec<u32>>,
    home: Vec<i32>,
    rcomp: Vec<i32>,
    infra_dirty: bool,
    lookups: HashMap<usize, Lookup>,
    /// Contacts after a reboot: `forgot[i]` = node i lost its contacts and
    /// knows only those in `relearned[i]` (announces heard since). `held[i]`:
    /// messages it keeps until their sender's announce.
    forgot: Vec<bool>,
    relearned: Vec<HashSet<u32>>,
    held: Vec<Vec<usize>>,
    /// Announce schedule as in the firmware: after a start every 15 min until
    /// a repeater or anchor is heard (`settled`), then every `announce_s`.
    /// `next_ann` is the one valid scheduled announce per node.
    settled: Vec<bool>,
    /// Sleepy handhelds: the radio is off (nothing is received).
    asleep: Vec<bool>,
    /// Which nodes sleep, and their phase in the cycle (s).
    sleepy: Vec<Option<u64>>,
    next_ann: Vec<u64>,
    /// The first announce since the (re)start has been sent.
    announced: Vec<bool>,
    /// Automatic forest mode: last time a repeater was heard, and since when
    /// it has been heard without a break (0 = not now).
    rep_last: Vec<u64>,
    rep_since: Vec<u64>,
    /// Distinct nodes whose announce each node has heard (how many contacts
    /// a node would have to keep).
    heard_from: Vec<HashSet<u32>>,
    /// Limited contact tables (`contact_capacity` > 0): contact -> last
    /// heard, pinned contacts, flash writes, and circles of friends.
    contacts: Vec<HashMap<u32, u64>>,
    /// Bridge choice each node last announced (named bridges).
    told_choice: Vec<Option<u16>>,
    /// Contacts each node has written to or heard from (contact policy "used").
    used: Vec<HashSet<u32>>,
    /// Messages waiting for their recipient's key (key request), per sender.
    await_key: Vec<Vec<usize>>,
    /// Last key request per (node, wanted) and last answer per (node, about).
    key_asked: HashMap<(usize, usize), u64>,
    key_answered: HashMap<(usize, usize), u64>,
    /// Nodes each host heard announcing over its own radio (bordercast).
    local_seen: Vec<HashSet<u32>>,
    /// Bordercast: path requests a host kept off its radio (dest, when).
    bc_dropped: Vec<Vec<(usize, u64)>>,
    /// True while handling a packet that came over the backbone.
    bb_ctx: bool,
    /// True while handling a packet that came over a LoRa trunk.
    trunk_ctx: bool,
    /// Neighbor roles have been handed over once (see `seed_neighbors`).
    seeded_once: bool,
    /// Radio islands for trunk summaries (recomputed after `move`).
    islands: Vec<usize>,
    /// Mail bundles in flight: bundle msg ID -> (office, recipient, messages).
    bundles: HashMap<u64, (usize, usize, Vec<usize>)>,
    /// When each node's next router tick is scheduled (`u64::MAX`: none).
    tick_at: Vec<u64>,
    /// Trunk bordercast: requests kept off a trunk (link, from, dest) -> when.
    trunk_dropped: HashMap<(usize, usize, usize), u64>,
    /// Post offices: stored mail per node (recipient -> message indices),
    /// class, recipients present (until), and per user the home office, the
    /// previous one, recently received messages and the last sync.
    mailbox: Vec<HashMap<usize, Vec<usize>>>,
    /// Linked devices: (listener, device) -> when the listener last got that
    /// device's announce.
    heard_device: HashMap<(usize, usize), u64>,
    /// Linked devices: sync request messages (not counted as mail).
    pub sync_req: HashSet<usize>,
    /// (office, message) handed to carriers (`office_carry`).
    office_handed: HashSet<(usize, usize)>,
    mail_class: Vec<u8>,
    present: Vec<HashMap<usize, u64>>,
    home_po: Vec<Option<usize>>,
    prev_home: Vec<Option<usize>>,
    got: Vec<Vec<usize>>,
    last_sync: Vec<u64>,
    /// "named": each user's fixed post box (its home host at the start).
    named_box: Vec<Option<usize>>,
    host_list: Vec<usize>,
    id_idx: HashMap<u16, usize>,
    /// Host-to-host mail waiting for TCP to come back (from, to, payload).
    mail_deferred: Vec<(usize, usize, Vec<u8>)>,
    /// Retry delivery of held mail: (office, recipient) -> (next try, step).
    mail_retry: HashMap<(usize, usize), (u64, u32)>,
    /// Deliveries in flight: (office, message) -> when sent.
    mail_sent: HashMap<(usize, usize), u64>,
    /// What each office has heard a node name as its home office (from the
    /// node's announces that reached it): (office, node) -> home.
    heard_home: HashMap<(usize, usize), usize>,
    /// Hosts: nodes heard over their own radio (node -> when), whether the
    /// host was on the backbone at the last tick, and refetches due.
    radio_seen: Vec<HashMap<usize, u64>>,
    was_on_backbone: Vec<bool>,
    refetch_at: Vec<(u64, usize)>,
    pinned: Vec<HashSet<u32>>,
    flash_writes: Vec<u32>,
    friends: Vec<Vec<usize>>,
}

/// A location lookup of the home routing model (one per message).
struct Lookup {
    from: usize,
    to: usize,
    gw: usize,
    /// Forwarders from the sender to the registry node (way back reversed).
    up: Vec<usize>,
    /// The answer: forwarders from the sender to the destination.
    route: Vec<usize>,
    tries: u8,
    done: bool,
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
        PacketType::Hello => "Hello",
        PacketType::KeyRequest => "KeyRequest",
        PacketType::MailSync => "MailSync",
        PacketType::Summary => "Summary",
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
            // Home routing model: hosts are anchors, so a route longer than a
            // source path can hold is split into legs at the gateways.
            let role = if (f.home_routing || f.host_anchor) && kind == Kind::Host { Role::Anchor } else { kind.role() };
            let mut router = Box::new(Node::new(Address(addr), role));
            let _ = router.subscribe(public_channel);
            let _ = router.subscribe(bulletin_channel);
            // Cars, and handhelds given a route (travellers): the simulator
            // does not model how a node notices that it moves.
            router.mobile = f.mobile_mode && (kind == Kind::Mobile || spec.route.len() >= 2);
            router.carry_smart = sc.carry_smart;
            router.hello = sc.neighbor_info == "hello";
            router.ring = sc.path_ring;
            if sc.announce_scope > 0 {
                router.announce_scope = sc.announce_scope;
            }
            router.weak_hop_penalty = sc.weak_hop_penalty;
            router.drop_unacked_routes = sc.drop_unacked_routes;
            if sc.proxy_reply && kind == Kind::Host {
                router.proxy_reply = true;
            }
            if sc.host_slow_timers && kind == Kind::Host {
                router.path_retry_ms = 90_000;
                router.anchor_ttl_ms = 300_000;
            }
            if sc.mobile_route_s > 0.0 {
                router.mobile_route_ms = (sc.mobile_route_s * 1000.0) as u64;
            }
            router.carry_copies = if f.carry { if spec.carry > 0 { spec.carry } else { 2 } } else { 0 };
            if sc.carry_ttl_s > 0.0 {
                router.carry_ttl_ms = (sc.carry_ttl_s * 1000.0) as u64;
            }
            if sc.carry_max > 0 {
                router.carry_max = sc.carry_max;
            }
            router.local_repair = f.local_repair;
            router.load_control = f.load_control;
            router.relay_hops = sc.relay_hops.clamp(1, maarse_core::router::MAX_RELAY_HOPS);
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
                wander: spec.wander_m.max(0.0),
                home: (x, y),
                target: None,
            });
        }
        let mut links = Vec::new();
        for (k, l) in sc.links.iter().enumerate() {
            if l.a < nodes.len() && l.b < nodes.len() && l.a != l.b {
                nodes[l.a].links.push(k);
                nodes[l.b].links.push(k);
            }
            let lora = l.kind == "lora";
            links.push(Link {
                a: l.a,
                b: l.b,
                wifi: l.kind == "wifi",
                up: l.up,
                lora,
                sf: if (7..=12).contains(&l.sf) { l.sf } else { 9 },
                snr: if l.snr_db != 0.0 { l.snr_db.clamp(-20.0, 20.0) as i8 } else { 5 },
                loss: l.loss.clamp(0.0, 1.0),
                duty: if l.duty_permille > 0 { l.duty_permille.min(1000) } else { 100 },
                busy_until: 0,
                label: if l.label.is_empty() && lora { format!("LoRa trunk {}-{}", l.a, l.b) } else { l.label.clone() },
                group: l.group.clone(),
                packets: 0,
                airtime_ms: 0,
                dropped: 0,
            });
        }
        let traffic = sc.traffic.clone();
        let mut e = Engine {
            radios,
            rng,
            nodes,
            links,
            queue: BinaryHeap::new(),
            events: HashMap::default(),
            seq: 0,
            now: 0,
            active: Vec::new(),
            next_tx: 0,
            msgs: Vec::new(),
            alerts: Vec::new(),
            stats: Stats::default(),
            tags: HashMap::default(),
            cancelled: HashSet::default(),
            trace: Vec::new(),
            trace_base: 0,
            grid: HashMap::default(),
            max_range: 0.0,
            internet_global: true,
            internet_down: HashSet::default(),
            public_channel,
            bulletin_channel,
            authority,
            trusted,
            traffic,
            announces: Vec::new(),
            timeline_done: 0,
            ids: HashMap::default(),
            addr_idx: HashMap::default(),
            clutter: Vec::new(),
            degrade: Vec::new(),
            gw: Vec::new(),
            gw_dirty: true,
            dyn_links: HashSet::default(),
            token_tags: HashMap::default(),
            infra: Vec::new(),
            home: Vec::new(),
            rcomp: Vec::new(),
            infra_dirty: true,
            lookups: HashMap::default(),
            forgot: Vec::new(),
            relearned: Vec::new(),
            held: Vec::new(),
            settled: Vec::new(),
            asleep: Vec::new(),
            sleepy: Vec::new(),
            next_ann: Vec::new(),
            announced: Vec::new(),
            rep_last: Vec::new(),
            rep_since: Vec::new(),
            heard_from: Vec::new(),
            contacts: Vec::new(),
            told_choice: Vec::new(),
            used: Vec::new(),
            await_key: Vec::new(),
            key_asked: HashMap::default(),
            key_answered: HashMap::default(),
            local_seen: Vec::new(),
            bc_dropped: Vec::new(),
            bb_ctx: false,
            trunk_ctx: false,
            seeded_once: false,
            islands: Vec::new(),
            bundles: HashMap::default(),
            tick_at: Vec::new(),
            trunk_dropped: HashMap::default(),
            mailbox: Vec::new(),
            heard_device: HashMap::default(),
            sync_req: HashSet::default(),
            office_handed: HashSet::default(),
            mail_class: Vec::new(),
            present: Vec::new(),
            home_po: Vec::new(),
            prev_home: Vec::new(),
            got: Vec::new(),
            last_sync: Vec::new(),
            named_box: Vec::new(),
            host_list: Vec::new(),
            id_idx: HashMap::default(),
            mail_deferred: Vec::new(),
            mail_retry: HashMap::default(),
            mail_sent: HashMap::default(),
            heard_home: HashMap::default(),
            radio_seen: Vec::new(),
            was_on_backbone: Vec::new(),
            refetch_at: Vec::new(),
            pinned: Vec::new(),
            flash_writes: Vec::new(),
            friends: Vec::new(),
            sc,
        };
        for (k, n) in e.nodes.iter().enumerate() {
            e.ids.entry(n.router.id).or_insert(k);
            e.addr_idx.insert(n.router.addr, k);
        }
        e.clutter = e
            .sc
            .clutter
            .iter()
            .filter(|c| c.polygon.len() >= 3 && c.db_per_km > 0.0)
            .map(|c| {
                let mut a = LossArea::new(c.polygon.iter().map(|[la, lo]| to_xy(*la, *lo)).collect(), None, c.db_per_km);
                if e.sc.clutter_model != "linear" {
                    a.canopy = Some((c.canopy_m, c.max_db));
                }
                a
            })
            .collect();
        e.build_links();
        e.seed_neighbors();
        // Timeline, sorted by time (stable).
        let mut order: Vec<usize> = (0..e.sc.events.len()).collect();
        order.sort_by(|a, b| e.sc.events[*a].t_s.total_cmp(&e.sc.events[*b].t_s));
        for idx in order {
            let at = (e.sc.events[idx].t_s.max(0.0) * 1000.0) as u64;
            e.schedule(at, Ev::Timeline { idx });
        }
        let n = e.nodes.len();
        e.forgot = vec![false; n];
        e.relearned = vec![HashSet::default(); n];
        e.held = vec![Vec::new(); n];
        // The network has been running for a while: every node has settled.
        e.settled = vec![true; n];
        e.announced = vec![true; n];
        e.rep_last = vec![0; n];
        e.heard_from = vec![HashSet::default(); n];
        e.contacts = vec![HashMap::default(); n];
        e.told_choice = vec![None; n];
        e.tick_at = vec![u64::MAX; n];
        e.init_mail();
        e.used = vec![HashSet::default(); n];
        e.await_key = vec![Vec::new(); n];
        e.local_seen = vec![HashSet::default(); n];
        e.bc_dropped = vec![Vec::new(); n];
        if e.sc.neighbor_info == "hello" {
            // First hellos within the first 10 s, as after a start.
            for i in 0..n {
                let at = e.rng.gen_range(0..10_000);
                e.schedule(at, Ev::Tick { node: i });
            }
        }
        e.pinned = vec![HashSet::default(); n];
        e.flash_writes = vec![0; n];
        e.friends = vec![Vec::new(); n];
        if e.sc.traffic.friends > 0 {
            e.make_friends();
        }
        e.rep_since = vec![0; n];
        e.next_ann = vec![u64::MAX; n];
        if e.sc.announce_s > 0.0 {
            let iv = if e.sc.announce_warmup_s > 0.0 { (e.sc.announce_warmup_s * 1000.0) as u64 } else { (e.sc.announce_s * 1000.0) as u64 };
            for i in 0..n {
                let at = e.rng.gen_range(0..iv.max(1));
                e.plan_announce(i, at);
            }
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
        let g = na.kind.ant_db() + nb.kind.ant_db() + height_db(na.h) + height_db(nb.h) - self.extra_loss(a, b);
        let r = self.radio(na.sf);
        let rx = r.rx_dbm(d, g);
        (r.snr(rx) >= r.snr_min - 6.0).then_some(rx as f32)
    }

    /// Extra loss (dB) on the link a-b: clutter along the line, plus
    /// degradation areas that contain either end.
    fn extra_loss(&self, a: usize, b: usize) -> f64 {
        if self.clutter.is_empty() && self.degrade.is_empty() {
            return 0.0;
        }
        let (pa, pb) = ((self.nodes[a].x, self.nodes[a].y), (self.nodes[b].x, self.nodes[b].y));
        let (ha, hb) = (self.nodes[a].h, self.nodes[b].h);
        let clutter: f64 = self.clutter.iter().map(|c| c.loss(pa, pb, ha, hb)).sum();
        let degrade: f64 = self.degrade.iter().filter(|g| g.contains(pa.0, pa.1) || g.contains(pb.0, pb.1)).map(|g| g.db).sum();
        clutter + degrade
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
        // "announce" and "hello": the link is there (any traffic shows it),
        // but roles come only from packets, as in the firmware.
        // The first seeding hands over roles in every mode: the network has
        // been running, so neighbors know each other; later changes travel
        // only in packets.
        let instant = self.sc.neighbor_info.is_empty() || self.sc.neighbor_info == "instant" || !self.seeded_once;
        self.seeded_once = true;
        let n = self.nodes.len();
        let reports: Vec<(u16, Role, bool, u8, Option<u16>)> = (0..n)
            .map(|j| {
                let rt = &self.nodes[j].router;
                (rt.addr.path_id(), rt.role, rt.hears_repeater(now), rt.bridges_heard(now), rt.chosen_bridge(now))
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
                let (id, role, hears, bridges, choice) = reports[j as usize];
                let snr = self.radio(self.nodes[i].sf).snr(rx as f64).clamp(-30.0, 30.0) as i8;
                let rt = &mut self.nodes[i].router;
                rt.neighbors.heard(id, None, snr, 0, now);
                if instant {
                    rt.neighbors.set_role(id, role, Some(hears));
                    rt.neighbors.set_bridges(id, bridges);
                    rt.neighbors.set_bridge_choice(id, choice);
                }
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
                if self.now >= self.tick_at[node] {
                    self.tick_at[node] = u64::MAX;
                }
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
                self.bb_ctx = true;
                self.apply(to, actions.into_iter().collect(), tag);
                self.bb_ctx = false;
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
                if self.sc.features.auto_relay {
                    self.auto_relay_step();
                }
                // Nodes that started recently settle once they hear a repeater
                // (the firmware notices at once; here within a minute).
                for i in 0..self.nodes.len() {
                    if !self.settled[i] && self.nodes[i].alive() && self.found(i) {
                        self.settled[i] = true;
                        if !self.announced[i] {
                            continue; // the first announce goes out as planned
                        }
                        let at = if self.sc.announce_s > 0.0 {
                            let iv = (self.sc.announce_s * 1000.0) as u64;
                            self.now + iv / 10 + self.rng.gen_range(0..(iv - iv / 10).max(1))
                        } else {
                            u64::MAX
                        };
                        self.next_ann[i] = u64::MAX;
                        if at != u64::MAX {
                            self.plan_announce(i, at);
                        }
                    }
                }
                self.gw_dirty = true;
        self.infra_dirty = true;
                self.seed_neighbors();
                self.schedule(self.now + 60_000, Ev::Neighbors);
            }
            Ev::Timeline { idx } => {
                let ev = self.sc.events[idx].clone();
                self.timeline_done += 1;
                self.inject(&ev);
            }
            Ev::Announce { node } => {
                if self.next_ann[node] != self.now {
                    return; // replaced by a newer schedule
                }
                self.next_ann[node] = u64::MAX;
                if !self.nodes[node].alive() {
                    return;
                }
                self.stats.periodic_announces += 1;
                self.announce(node);
                self.announced[node] = true;
                if !self.settled[node] && self.found(node) {
                    self.settled[node] = true;
                }
                // ±10 % per interval, so nodes never fall into step.
                let bridge = self.sc.auto_bridge == "named" && self.nodes[node].router.role == Role::Relay && self.nodes[node].router.hears_repeater(self.now);
                let every = if bridge {
                    maarse_core::router::BRIDGE_ANNOUNCE_S as f64
                } else if self.settled[node] {
                    self.sc.announce_s
                } else {
                    900.0
                };
                if every > 0.0 {
                    let iv = every * 1000.0 * self.rng.gen_range(0.9..1.1);
                    self.plan_announce(node, self.now + iv as u64);
                }
            }
            Ev::MailCheck { msg } => self.mail_check(msg),
            Ev::SleepTick => {
                self.sleep_tick();
                self.schedule(self.now + 5_000, Ev::SleepTick);
            }
            Ev::MailTick => {
                self.mail_tick();
                self.schedule(self.now + 15_000, Ev::MailTick);
            }
            Ev::MailTcp { to, payload } => {
                if self.nodes[to].alive() && self.on_backbone(to) {
                    self.on_deposit(to, &payload);
                }
            }
            Ev::MailSyncAt { node } => {
                if self.nodes[node].alive() {
                    self.mail_sync(node);
                }
            }
            Ev::Trunk { link, to, packet, start } => {
                if !self.link_ok(link) || !self.nodes[to].alive() {
                    return;
                }
                if self.sc.trunk_mesh_channel && self.trunk_jammed(link, to, start) {
                    self.links[link].dropped += 1;
                    self.stats.trunk_jammed += 1;
                    return;
                }
                if self.links[link].loss > 0.0 && self.rng.gen_bool(self.links[link].loss) {
                    self.links[link].dropped += 1;
                    return;
                }
                let snr = self.links[link].snr;
                let pid = Packet::decode(&packet).map(|p| packet_id(p.header.ptype, &p.header.dest, p.payload)).unwrap_or(0);
                let tag = self.tags.get(&pid).copied();
                let tj = self.tag_json(pid);
                self.tr(format!("{{\"t\":{},\"ev\":\"trunk\",\"link\":{link},\"to\":{to},\"pid\":\"{pid:016x}\"{tj}}}", self.now));
                let before = self.nodes[to].router.stats;
                let actions = self.nodes[to].router.on_receive(&packet, snr, self.now);
                self.after_router(to, before, tag);
                // Not the backbone: a request that arrives over a trunk is
                // flooded on this side's radio (it may be crossing a transit
                // region to the next trunk), as maarse-host does (a trunk is
                // not a backbone link there either).
                self.trunk_ctx = true;
                self.apply(to, actions.into_iter().collect(), tag);
                self.trunk_ctx = false;
            }
            Ev::KeyWait { msg } => {
                let from = self.msgs[msg].from;
                if let Some(k) = self.await_key[from].iter().position(|&m| m == msg) {
                    self.await_key[from].remove(k);
                    self.msgs[msg].unsent = true;
                    self.tr(format!("{{\"t\":{},\"ev\":\"drop\",\"msg\":{msg},\"n\":{from},\"why\":\"no answer to the key request\"}}", self.now));
                }
            }
            Ev::LookupTimeout { msg } => match self.lookups.get(&msg) {
                Some(l) if !l.done && l.tries < LOOKUP_TRIES => self.ask(msg),
                Some(l) if !l.done => self.home_fallback(msg, "timeout"),
                _ => {}
            },
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
                    // A path response is sent from the router's tick (no causing
                    // packet): it belongs to the request whose reply token it
                    // is addressed to.
                    let tag = tag.or_else(|| {
                        (p.header.ptype == PacketType::PathResponse).then(|| self.token_tags.get(&p.header.dest).copied()).flatten()
                    });
                    if let Some(t) = tag {
                        self.tags.entry(pid).or_insert(t);
                        if p.header.ptype == PacketType::PathRequest
                            && let Some(tok) = p.payload.get(..16).and_then(Address::from_slice)
                        {
                            self.token_tags.entry(tok).or_insert(t);
                        }
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
                        let dht = self.sc.features.dht_backbone;
                        let directed = dht && p.header.ptype == PacketType::PathRequest;
                        // LoRa trunks are handled apart: they cost airtime.
                        let trunks: Vec<usize> = self.nodes[node].links.iter().copied().filter(|&k| self.links[k].lora && self.link_ok(k)).collect();
                        for k in trunks {
                            self.trunk_send(k, node, &p, &packet, delay_ms as u64);
                        }
                        let mut peers: Vec<(usize, bool)> = self.nodes[node]
                            .links
                            .iter()
                            .filter(|&&k| !self.links[k].lora)
                            .filter_map(|&k| self.link_ok(k).then(|| {
                                let l = &self.links[k];
                                (if l.a == node { l.b } else { l.a }, l.wifi)
                            }))
                            // DHT mode: path requests do not go to every TCP peer.
                            .filter(|&(_, wifi)| !directed || wifi)
                            .collect();
                        if directed && self.has_internet(node) {
                            // DHT lookup: the host that can reach the destination,
                            // then a direct connection to it (maarse-host dht_connect).
                            let target = self.addr_idx.get(&p.header.dest).copied();
                            let gw = target.map_or(-1, |t| self.gateway_of(t));
                            self.stats.dht_lookups += 1;
                            let tj = self.tag_json(pid);
                            let dst = target.map_or(-1, |t| t as i64);
                            if gw >= 0 && gw as usize != node && self.has_internet(gw as usize) {
                                let gw = gw as usize;
                                self.dyn_links.insert((node.min(gw), node.max(gw)));
                                self.tr(format!("{{\"t\":{},\"ev\":\"dht\",\"n\":{node},\"to\":{gw},\"dst\":{dst},\"pid\":\"{pid:016x}\"{tj}}}", self.now));
                                peers.push((gw, false));
                            } else if gw as usize != node {
                                self.stats.dht_misses += 1;
                                self.tr(format!("{{\"t\":{},\"ev\":\"dht\",\"n\":{node},\"to\":-1,\"dst\":{dst},\"pid\":\"{pid:016x}\"{tj}}}", self.now));
                            }
                        } else if (dht || self.sc.features.home_routing) && p.header.path.is_some() && self.has_internet(node) {
                            // Routed packets also use the direct connections.
                            let extra: Vec<usize> = self
                                .dyn_links
                                .iter()
                                .filter_map(|&(a, b)| if a == node { Some(b) } else if b == node { Some(a) } else { None })
                                .filter(|&o| self.has_internet(o) && self.nodes[o].alive())
                                .collect();
                            for o in extra {
                                if !peers.iter().any(|&(q, _)| q == o) {
                                    peers.push((o, false));
                                }
                            }
                        }
                        for (peer, wifi) in peers {
                            if region != 0 && !self.nodes[peer].router.forwards_region(region) {
                                continue;
                            }
                            self.stats.backbone_packets += 1;
                            let lat = if wifi { 5 } else { 40 };
                            self.schedule(self.now + delay_ms as u64 + lat, Ev::Backbone { from: node, to: peer, packet: packet.clone() });
                        }
                    }
                    if self.bb_ctx && self.sc.features.bordercast && self.nodes[node].kind == Kind::Host && self.keep_off_radio(node, &packet) {
                        continue;
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
                Action::Acked { msg_id } | Action::CarryAcked { msg_id } if self.mail_acked(node, msg_id) => {}
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
            // One pending tick per node at a time (hellos give every router a
            // deadline; scheduling one per action flooded the event queue).
            let d = d.max(self.now);
            if d < self.tick_at[node] || self.tick_at[node] < self.now {
                self.tick_at[node] = d;
                self.schedule(d, Ev::Tick { node });
            }
        }
    }

    /// Is node `i` on the TCP backbone: internet and at least one TCP link up?
    fn on_backbone(&self, i: usize) -> bool {
        self.has_internet(i) && self.nodes[i].links.iter().any(|&k| !self.links[k].lora && !self.links[k].wifi && self.link_ok(k))
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
        if l.wifi || l.lora {
            return true;
        }
        self.has_internet(l.a) && self.has_internet(l.b)
    }

    fn has_internet(&self, i: usize) -> bool {
        let n = &self.nodes[i];
        n.internet && n.powered && self.internet_global && !self.internet_down.contains(&n.region)
    }

    fn on_deliver(&mut self, node: usize, ptype: PacketType, dest: Address, payload: &[u8], hops: u8) {
        if ptype == PacketType::Announce
            && self.sc.person_fresh
            && let Some(&who) = self.addr_idx.get(&dest)
            && self.sc.device_groups.iter().any(|g| g.contains(&who))
        {
            self.heard_device.insert((node, who), self.now);
        }
        if ptype == PacketType::Announce
            && !self.mailbox.is_empty()
            && (!self.bb_ctx || self.sc.mail == "named")
            && let Some(&who) = self.addr_idx.get(&dest)
            && self.holds_mail(node, who)
        {
            // A post office hears a recipient it holds mail for.
            self.deliver_mail(node, who, &[]);
        }
        match ptype {
            PacketType::Deposit => self.on_deposit(node, payload),
            PacketType::MailSync => self.on_mail_sync(node, dest, payload),
            PacketType::KeyRequest => {
                let Some(&who) = self.addr_idx.get(&dest) else { return };
                let fresh = |e: &Self, k: (usize, usize)| e.key_answered.get(&k).is_none_or(|&t| e.now >= t + 30_000);
                if who == node {
                    // We are asked for: announce (at most every 30 s).
                    if fresh(self, (node, node)) {
                        self.key_answered.insert((node, node), self.now);
                        self.announce(node);
                    }
                } else if self.sc.features.key_cache
                    && self.nodes[node].kind == Kind::Host
                    && !self.bb_ctx
                    && self.heard_from[node].contains(&(who as u32))
                    && fresh(self, (node, who))
                {
                    self.key_answered.insert((node, who), self.now);
                    self.reannounce(node, who);
                }
            }
            PacketType::Announce => {
                if let Some(&who) = self.addr_idx.get(&dest)
                    && who != node
                {
                    self.heard_from[node].insert(who as u32);
                    if self.mail_class.get(node).is_some_and(|&c| c > 0)
                        && let Some(h) = self.home_po.get(who).copied().flatten()
                    {
                        // The announce names the sender's home office.
                        self.heard_home.insert((node, who), h);
                    }
                    if !self.bb_ctx && !self.trunk_ctx && self.nodes[node].kind == Kind::Host {
                        self.local_seen[node].insert(who as u32);
                        if !self.radio_seen.is_empty() {
                            self.radio_seen[node].insert(who, self.now);
                        }
                    }
                    if self.sc.features.mobile_mode {
                        let m = self.nodes[who].router.mobile;
                        self.nodes[node].router.note_mobile(dest, m, self.now);
                    }
                    if hops == 0 && !(self.sc.neighbor_info.is_empty() || self.sc.neighbor_info == "instant") {
                        // The role field of a 0-hop announce (as the firmware).
                        let (id, role, h, choice) = {
                            let rt = &self.nodes[who].router;
                            (rt.id, rt.role, rt.hears_repeater(self.now), rt.chosen_bridge(self.now))
                        };
                        let rt = &mut self.nodes[node].router;
                        rt.neighbors.heard(id, Some(dest), 0, 0, self.now);
                        rt.neighbors.set_role(id, role, Some(h));
                        rt.neighbors.set_bridge_choice(id, choice);
                    }
                    // The announce's role flags say whether the sender hears
                    // a repeater (boundary mode for edge bridges).
                    let hears = self.nodes[who].router.hears_repeater(self.now);
                    self.nodes[node].router.note_coverage(dest, hears);
                    if hears && self.nodes[who].router.role == Role::Relay {
                        let id = self.nodes[who].router.id;
                        self.nodes[node].router.note_bridge(id, hops, self.now);
                    }
                }
                if self.sc.contact_capacity > 0
                    && let Some(&who) = self.addr_idx.get(&dest)
                    && who != node
                {
                    self.learn_contact(node, who);
                }
                // The node now knows the announcer's keys again.
                if let Some(&who) = self.addr_idx.get(&dest)
                    && who != node
                    && (self.sc.contact_capacity > 0 || (self.forgot[node] && self.relearned[node].insert(who as u32)))
                {
                    // Messages waiting for this key go out now.
                    let go: Vec<usize> = self.await_key[node].iter().copied().filter(|&m| self.msgs[m].to == who).collect();
                    self.await_key[node].retain(|&m| self.msgs[m].to != who);
                    for m in go {
                        self.continue_send(m);
                    }
                    let waiting: Vec<usize> = self.held[node].iter().copied().filter(|&m| self.msgs[m].from == who).collect();
                    self.held[node].retain(|&m| self.msgs[m].from != who);
                    for m in waiting {
                        self.msgs[m].shown_ms = Some(self.now);
                        self.tr(format!("{{\"t\":{},\"ev\":\"shown\",\"msg\":{m},\"n\":{node},\"after_hold\":true}}", self.now));
                    }
                }
            }
            PacketType::Data if payload.starts_with(b"BNDL") => {
                let n = payload.get(4).copied().unwrap_or(0) as usize;
                for c in payload.get(5..5 + 4 * n).unwrap_or(&[]).chunks_exact(4) {
                    let i = u32::from_be_bytes([c[0], c[1], c[2], c[3]]) as usize;
                    if i < self.msgs.len() && self.msgs[i].to == node && self.msgs[i].delivered_ms.is_none() {
                        self.msgs[i].delivered_ms = Some(self.now);
                        self.sibling_copies(i, node);
                        let from = self.msgs[i].from;
                        if self.knows(node, from) {
                            self.msgs[i].shown_ms = Some(self.now);
                        }
                        if !self.got.is_empty() {
                            self.got[node].push(i);
                            if self.got[node].len() > 8 {
                                self.got[node].remove(0);
                            }
                        }
                    }
                }
            }
            PacketType::Data => {
                let id = packet_id(PacketType::Data, &dest, payload);
                if let Some(i) = self.msgs.iter().position(|m| m.id == id && m.to == node)
                    && self.msgs[i].delivered_ms.is_none()
                {
                    self.msgs[i].delivered_ms = Some(self.now);
                    self.sibling_copies(i, node);
                    self.tr(format!("{{\"t\":{},\"ev\":\"deliver\",\"msg\":{i},\"n\":{node},\"hops\":{hops}}}", self.now));
                    if !self.got.is_empty() {
                        self.got[node].push(i);
                        if self.got[node].len() > 8 {
                            self.got[node].remove(0);
                        }
                    }
                    // The app can show it only if it knows the sender's keys.
                    let from = self.msgs[i].from;
                    if self.knows(node, from) {
                        self.msgs[i].shown_ms = Some(self.now);
                        self.pin(node, from);
                    } else if self.sc.features.hold_unknown {
                        self.held[node].push(i);
                        if self.sc.features.key_request {
                            self.ask_key(node, from);
                        }
                        self.tr(format!("{{\"t\":{},\"ev\":\"held\",\"msg\":{i},\"n\":{node},\"why\":\"unknown sender\"}}", self.now));
                    } else {
                        self.msgs[i].dropped_unknown = true;
                        self.tr(format!("{{\"t\":{},\"ev\":\"drop\",\"msg\":{i},\"n\":{node},\"why\":\"unknown sender\"}}", self.now));
                    }
                }
            }
            PacketType::Datagram => self.on_lookup_datagram(node, payload),
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

    /// For the trace: the source path as node indices (`,"path":[..]`; for a
    /// path request `,"trail":[..]`, the repeaters it passed; empty for other
    /// floods), the intended next hop (-1 for floods) and the final
    /// destination node (-1 if the destination is a token or a channel).
    fn route_info(&self, h: &Header) -> (String, i64, i64) {
        let idx = |id: &u16| self.ids.get(id).map_or(-1, |&k| k as i64);
        let dst = self.addr_idx.get(&h.dest).map_or(-1, |&k| k as i64);
        let Some(path) = &h.path else { return (String::new(), -1, dst) };
        let nodes: Vec<String> = path.iter().map(|id| idx(id).to_string()).collect();
        // A path request carries the trail so far, but it is a flood: no next hop.
        if h.ptype == PacketType::PathRequest {
            return (format!(",\"trail\":[{}]", nodes.join(",")), -1, dst);
        }
        let nx = match path.get(h.hops as usize) {
            Some(id) => idx(id),
            // Last stretch: the destination itself (if it is a node).
            None => dst,
        };
        (format!(",\"path\":[{}]", nodes.join(",")), nx, dst)
    }

    fn ready(&mut self, node: usize, packet: Buf, id: u64) {
        if !self.nodes[node].alive() {
            return;
        }
        if self.cancelled.remove(&(node, id)) {
            return;
        }
        // The same packet already waiting: one copy is enough (as the firmware).
        if !self.sc.no_queue_dedup && self.nodes[node].txq.iter().any(|q| q.id == id && q.packet == packet) {
            self.stats.dup_queued += 1;
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
        let (pname, hops, region, pid, route) = match Packet::decode(&q.packet) {
            Ok(p) => (
                ptype_name(p.header.ptype),
                p.header.hops,
                p.header.region,
                packet_id(p.header.ptype, &p.header.dest, p.payload),
                self.route_info(&p.header),
            ),
            Err(_) => ("?", 0, 0, q.id, (String::new(), -1, -1)),
        };
        let (path_json, nx, dst) = route;
        let pname = if pname == "Channel" && q.packet.get(2..18) == Some(&self.bulletin_channel.0[..]) { "Bulletin" } else { pname };
        *self.stats.by_type.entry(pname.to_string()).or_default() += 1;
        *self.stats.air_by_type.entry(pname.to_string()).or_default() += (airtime as u64).div_ceil(1000);
        let tx = self.next_tx;
        self.next_tx += 1;
        let tj = self.tag_json(pid);
        self.tr(format!(
            "{{\"t\":{},\"ev\":\"tx\",\"n\":{node},\"id\":{tx},\"end\":{end},\"air\":{airtime},\"p\":\"{pname}\",\"pid\":\"{pid:016x}\",\"hops\":{hops},\"rg\":{region},\"prio\":{},\"qd\":{delay},\"nx\":{nx},\"dst\":{dst}{path_json}{tj}}}",
            self.now, q.prio
        ));
        let flood = nx < 0 && !path_json.starts_with(",\"path\"");
        self.active.push(Tx { id: tx, sender: node, start: self.now, end, packet: q.packet, nx, flood, noise: false });
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
        let (sender, start, end, nx, flood) = (t.sender, t.start, t.end, t.nx, t.flood);
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
        let mut snrs = String::new();
        let mut coll = String::new();
        let mut cause = String::new();
        for (r, p) in receivers {
            let r = r as usize;
            let snr = p as f64 - noise;
            if snr < snr_min || !self.nodes[r].alive() || self.asleep.get(r).copied().unwrap_or(false) {
                continue;
            }
            self.nodes[r].router.note_airtime(start, ((end - start) as u32).max(1));
            // Lost if the receiver was transmitting itself (half-duplex), or
            // another audible transmission was not at least 6 dB weaker. The
            // cause is the strongest interferer (-1 = half-duplex).
            let lost = if overlapping.contains(&r) {
                Some(-1i64)
            } else {
                overlapping
                    .iter()
                    .filter(|&&o| o != sender)
                    .filter_map(|&o| self.nodes[o].gain_to(r as u32).map(|g| (o, g)))
                    .filter(|&(_, g)| (g as f64 - noise) >= snr_min - 6.0 && g > p - 6.0)
                    .max_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(o, _)| o as i64)
            };
            if let Some(by) = lost {
                self.stats.collisions += 1;
                if by < 0 {
                    self.stats.coll_half_duplex += 1;
                }
                if flood {
                    self.stats.coll_flood += 1;
                } else if r as i64 == nx {
                    self.stats.coll_next_hop += 1;
                }
                let sep = if coll.is_empty() { "" } else { "," };
                let _ = write!(coll, "{sep}{r}");
                let _ = write!(cause, "{sep}{by}");
                continue;
            }
            let sep = if ok.is_empty() { "" } else { "," };
            let _ = write!(ok, "{sep}{r}");
            let _ = write!(snrs, "{sep}{}", snr.round() as i32);
            let snr8 = snr.clamp(-30.0, 30.0) as i8;
            let before = self.nodes[r].router.stats;
            let actions = self.nodes[r].router.on_receive(&packet, snr8, now);
            self.after_router(r, before, tag);
            self.apply(r, actions.into_iter().collect(), tag);
        }
        let tj = self.tag_json(pid);
        self.tr(format!(
            "{{\"t\":{now},\"ev\":\"rx\",\"id\":{tx},\"n\":{sender},\"pid\":\"{pid:016x}\",\"nx\":{nx},\"ok\":[{ok}],\"snr\":[{snrs}],\"coll\":[{coll}],\"cw\":[{cause}]{tj}}}"
        ));
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
        if tr.friends > 0 && !self.friends[from].is_empty() && self.rng.gen_bool(tr.friend_share.clamp(0.0, 1.0)) {
            let f = &self.friends[from];
            let to = f[self.rng.gen_range(0..f.len())];
            if self.nodes[to].alive() {
                return Some(to);
            }
        }
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
    ///
    /// With `home_routing` (a model of a protocol that does not exist yet) a
    /// sender without a route first asks the gateway of its island where the
    /// destination is. The gateway knows every node's home: the fixed node
    /// (repeater, mast, host, Wi-Fi node) it hears best, as a registry fed by
    /// announces would. The fixed nodes know the routes between them (links
    /// with a 5 dB margin, shortest in hops), as a proactive infrastructure
    /// routing would, since they do not move. The answer is a source path:
    /// sender's home ... (gateway ⇢ TCP ⇢ gateway) ... destination's home. The
    /// lookup itself is a real datagram over the radio in both directions, and
    /// the message then goes through the real routers along that path, with
    /// their per-hop acks, local repair and fallback to a path request when it
    /// fails. Without an answer, the sender falls back to a path request; when
    /// the destination is known to be unreachable (another island without a
    /// working gateway), nothing is flooded.
    pub fn send_message(&mut self, from: usize, to: usize) -> usize {
        let to = self.person_device(from, to);
        let m = self.msgs.len();
        let mut payload = format!("message {m} from {from} to {to}: ").into_bytes();
        payload.resize(self.msg_size(), b'.');
        let id = packet_id(PacketType::Data, &self.nodes[to].router.addr, &payload);
        let cross = self.nodes[from].router.hears_repeater(self.now) != self.nodes[to].router.hears_repeater(self.now);
        self.msgs.push(Msg { from, to, payload: payload.clone(), id, sent_ms: self.now, delivered_ms: None, acked_ms: None, shown_ms: None, dropped_unknown: false, unsent: false, cross, sync_of: None });
        self.tags.insert(id, m as u32);
        self.tr(format!("{{\"t\":{},\"ev\":\"send\",\"msg\":{m},\"from\":{from},\"to\":{to},\"pid\":\"{id:016x}\"}}", self.now));
        if !self.nodes[from].alive() {
            return m;
        }
        if !self.sc.mail.is_empty() {
            self.schedule(self.now + maarse_core::mail::DEPOSIT_AFTER_MS, Ev::MailCheck { msg: m });
        }
        if !self.knows(from, to) {
            if self.sc.features.key_request {
                // Ask for the key; the message goes once the announce comes.
                self.await_key[from].push(m);
                self.schedule(self.now + 60_000, Ev::KeyWait { msg: m });
                self.ask_key(from, to);
                return m;
            }
            // The app cannot encrypt to a contact whose keys it lost.
            self.msgs[m].unsent = true;
            self.tr(format!("{{\"t\":{},\"ev\":\"drop\",\"msg\":{m},\"n\":{from},\"why\":\"recipient unknown to the sender\"}}", self.now));
            return m;
        }
        if self.sc.sleepy_mail_first && self.sc.mail == "post" && self.sleepy.get(to).is_some_and(Option::is_some) {
            // A sleepy recipient: straight to the post office.
            self.schedule(self.now + 500, Ev::MailCheck { msg: m });
            return m;
        }
        self.continue_send(m);
        m
    }

    /// Linked devices: the device of `to`'s person that `from` sends to (the
    /// first one it has a route to, else the first in the group).
    fn person_device(&self, from: usize, to: usize) -> usize {
        let Some(g) = self.sc.device_groups.iter().find(|g| g.contains(&to)) else { return to };
        if g.contains(&from) {
            return to;
        }
        let routes = self.nodes[from].router.routes();
        let known = g.iter().copied().filter_map(|d| routes.get(&self.nodes.get(d)?.router.addr).map(|r| (d, r.last_seen_ms, r.cost)));
        let pick = if self.sc.person_fresh {
            // The device whose announce the sender got last (the newest
            // announce for the person wins), else the first with a route.
            let heard = g.iter().copied().filter_map(|d| self.heard_device.get(&(from, d)).map(|&t| (d, t))).max_by_key(|&(_, t)| t).map(|(d, _)| d);
            heard.or_else(|| known.map(|(d, _, _)| d).next())
        } else {
            known.map(|(d, _, _)| d).next()
        };
        pick.unwrap_or(g[0])
    }

    /// Offices off the backbone hand the mail they hold to carriers: the
    /// office's router carries it (spray with tokens to the neighbours it
    /// meets, straight to the recipient when heard). Each message once per
    /// office; the office keeps its own copy.
    fn office_carry(&mut self) {
        let ttl = if self.sc.carry_ttl_s > 0.0 { (self.sc.carry_ttl_s * 1000.0) as u64 } else { maarse_core::router::CARRY_TTL_MS };
        let tokens = self.sc.office_carry_tokens.max(1);
        for o in 0..self.mailbox.len() {
            if self.mailbox[o].is_empty() || !self.nodes[o].alive() || self.on_backbone(o) {
                continue;
            }
            let held: Vec<usize> = self.mailbox[o].values().flatten().copied().filter(|&m| self.msgs[m].delivered_ms.is_none() && !self.office_handed.contains(&(o, m))).collect();
            let mut took = false;
            for m in held {
                let dest = self.nodes[self.msgs[m].to].router.addr;
                let payload = self.msgs[m].payload.clone();
                if self.nodes[o].router.carry_for(dest, &payload, tokens, ttl, self.now) {
                    self.office_handed.insert((o, m));
                    self.stats.office_carried += 1;
                    took = true;
                }
            }
            if took {
                self.apply(o, Vec::new(), None);
            }
        }
    }

    /// Linked devices: the mailbox key of `r`'s person (its first device),
    /// or `r` itself. Offices keep mail for the person, not the device.
    fn pkey(&self, r: usize) -> usize {
        self.sc.device_groups.iter().find(|g| g.contains(&r)).map_or(r, |g| g[0])
    }

    /// Linked devices: the mailbox keys mail for device `r` can be under
    /// (the person's, and the device's own for sibling copies).
    fn mail_keys(&self, r: usize) -> Vec<usize> {
        let k = self.pkey(r);
        if k == r { vec![r] } else { vec![k, r] }
    }

    /// Office `node` holds mail for device `r` (or its person).
    fn holds_mail(&self, node: usize, r: usize) -> bool {
        self.mail_keys(r).iter().any(|k| self.mailbox[node].get(k).is_some_and(|v| !v.is_empty()))
    }

    /// Linked devices: every device of `r`'s person (or just `r`).
    fn devices(&self, r: usize) -> Vec<usize> {
        self.sc.device_groups.iter().find(|g| g.contains(&r)).cloned().unwrap_or_else(|| vec![r])
    }

    /// Linked devices: office `node` hands the person's mail (kept under
    /// `key`) to `device`: each waiting message is re-addressed to it
    /// (messages one device already has are dropped from the box).
    fn retarget_mail(&mut self, node: usize, key: usize, device: usize) {
        let Some(list) = self.mailbox[node].get(&key).cloned() else { return };
        let dest = self.nodes[device].router.addr;
        let mut keep = Vec::new();
        for m in list {
            if self.msgs[m].delivered_ms.is_some() {
                continue;
            }
            if self.msgs[m].to != device && !self.devices(device).contains(&self.msgs[m].from) {
                self.tags.remove(&self.msgs[m].id);
                self.msgs[m].to = device;
                self.msgs[m].id = packet_id(PacketType::Data, &dest, &self.msgs[m].payload);
                self.tags.insert(self.msgs[m].id, m as u32);
            }
            keep.push(m);
        }
        self.mailbox[node].insert(key, keep);
    }

    /// Linked devices: `node` received message `m` for its person; send a
    /// copy to each sibling device (not for copies themselves).
    fn sibling_copies(&mut self, m: usize, node: usize) {
        let Some(g) = self.sc.device_groups.iter().find(|g| g.contains(&node)).cloned() else { return };
        let from = self.msgs[m].from;
        if g.contains(&from) {
            // From a sibling: it is active. A sync request: send it what it lacks.
            self.heard_device.insert((node, from), self.now);
            if self.sync_req.remove(&m) {
                self.backfill(node, from);
            }
            return;
        }
        if !self.sc.sibling_sync || self.msgs[m].sync_of.is_some() {
            return;
        }
        let n = self.nodes.len();
        for s in g.into_iter().filter(|&s| s != node && s < n) {
            // Redirect hint: a sibling heard from in the last hour is the
            // active one; the ack tells the sender to use it next time.
            if self.sc.redirect_hint && self.heard_device.get(&(node, s)).is_some_and(|&t| self.now < t + 3_600_000) {
                self.heard_device.insert((from, s), self.now);
                self.stats.redirect_hints += 1;
            }
            let c = self.send_message(node, s);
            self.msgs[c].sync_of = Some(m);
        }
    }

    /// The person's original message `m` (or a copy of it) reached `d`.
    fn device_has(&self, d: usize, m: usize) -> bool {
        self.msgs.iter().enumerate().any(|(i, x)| x.to == d && x.delivered_ms.is_some() && (i == m || x.sync_of == Some(m)))
    }

    /// Sync button: device `d` fetches its mail and asks each sibling for
    /// what it lacks (a small request message; the sibling answers with copies).
    pub fn person_sync(&mut self, d: usize) {
        if d >= self.nodes.len() || !self.nodes[d].alive() {
            return;
        }
        self.stats.sync_presses += 1;
        if !self.sc.mail.is_empty() {
            self.mail_sync(d);
        }
        let Some(g) = self.sc.device_groups.iter().find(|g| g.contains(&d)).cloned() else { return };
        for s in g.into_iter().filter(|&s| s != d) {
            let r = self.send_message(d, s);
            self.sync_req.insert(r);
        }
    }

    /// Sibling `s` sends `d` a copy of every message (at most
    /// `BACKFILL_MAX` per request; the device asks again) for the person that
    /// `s` has and `d` lacks.
    fn backfill(&mut self, s: usize, d: usize) {
        let g = self.devices(d);
        let missing: Vec<usize> = (0..self.msgs.len())
            .filter(|&m| self.msgs[m].sync_of.is_none() && !self.sync_req.contains(&m) && g.contains(&self.msgs[m].to) && !g.contains(&self.msgs[m].from))
            .filter(|&m| self.device_has(s, m) && !self.device_has(d, m))
            // Not what is already on its way (a copy sent in the last 30 min).
            .filter(|&m| !self.msgs.iter().any(|x| x.sync_of == Some(m) && x.to == d && x.delivered_ms.is_none() && self.now < x.sent_ms + 1_800_000))
            // A few per request, so a burst does not crowd out the copies in flight.
            .take(BACKFILL_MAX)
            .collect();
        for m in missing {
            let c = self.send_message(s, d);
            self.msgs[c].sync_of = Some(m);
            self.stats.backfilled += 1;
        }
    }

    /// The sender knows the recipient's key: route the message.
    fn continue_send(&mut self, m: usize) {
        let (from, to) = (self.msgs[m].from, self.msgs[m].to);
        self.pin(from, to);
        let dest = self.nodes[to].router.addr;
        if self.sc.features.home_routing && !self.nodes[from].router.has_route(&dest) {
            self.start_lookup(m);
            return;
        }
        self.send_real(m);
    }

    /// Sends a packet over LoRa trunk `k` from `node`, if the policy wants
    /// it: floods only when the other end cannot be reached over TCP
    /// ("backup", like a floating static route), routed packets only when
    /// the trunk is their next hop. One packet at a time, each taking its
    /// airtime, then a pause for the duty cycle; a queue over 30 s drops.
    fn trunk_send(&mut self, k: usize, node: usize, p: &Packet, packet: &Buf, delay_ms: u64) {
        let peer = if self.links[k].a == node { self.links[k].b } else { self.links[k].a };
        let routed = p.header.path.is_some() && p.header.ptype != PacketType::PathRequest;
        let own = p.header.dest == self.nodes[node].router.addr;
        let want = if routed {
            // Its next hop is the trunk's far end, or its path is used up: then
            // it is for the anchor at the far end (two anchors facing each
            // other across a trunk have an empty leg between them).
            match p.header.path.as_ref().and_then(|path| path.get(p.header.hops as usize)).copied() {
                Some(next) => next == self.nodes[peer].router.id,
                None => self.nodes[peer].router.role == Role::Anchor,
            }
        } else if self.sc.trunk_policy == "always" {
            true
        } else if p.header.ptype == PacketType::Channel && p.header.dest == self.bulletin_channel {
            // Alerts and bulletins always cross.
            true
        } else if matches!(p.header.ptype, PacketType::Hello | PacketType::Announce) {
            // Only the endpoint's own: keys for nodes behind the trunk come
            // from key requests answered on the other side.
            own
        } else if self.on_backbone(node) && self.on_backbone(peer) {
            // TCP reaches the other end: the trunk is only a backup.
            false
        } else if matches!(p.header.ptype, PacketType::PathRequest | PacketType::KeyRequest) {
            if self.sc.trunk_hash || self.sc.trunk_summary {
                self.trunk_choose(k, node, peer, &p.header.dest)
            } else {
                // Bordercast over the trunk: a request stays on this side when its
                // destination is known to be here (or until a second request).
                self.sc.trunk_policy == "backup-all" || self.trunk_behind(k, node, peer, &p.header.dest)
            }
        } else {
            false
        };
        if !want {
            return;
        }
        let air = maarse_core::airtime::LoraParams::new(self.links[k].sf).airtime_ms(packet.len()) as u64;
        // One frequency: trunks with a common endpoint share the channel there.
        let group: Vec<usize> = if self.sc.trunk_same_channel {
            let (a, b) = (self.links[k].a, self.links[k].b);
            (0..self.links.len()).filter(|&j| self.links[j].lora && (self.links[j].a == a || self.links[j].b == a || self.links[j].a == b || self.links[j].b == b)).collect()
        } else {
            vec![k]
        };
        let busy = group.iter().map(|&j| self.links[j].busy_until).max().unwrap_or(0);
        let start = (self.now + delay_ms).max(busy);
        let flood = matches!(p.header.ptype, PacketType::PathRequest | PacketType::KeyRequest);
        let limit = if self.sc.trunk_priority && flood { 5_000 } else { 30_000 };
        if start > self.now + limit {
            self.links[k].dropped += 1;
            self.stats.trunk_dropped += 1;
            return;
        }
        let duty = self.links[k].duty.max(1) as u64;
        // This radio's duty pause; the shared channel is busy while it sends.
        self.links[k].busy_until = start + air + air * (1000 - duty.min(1000)) / duty;
        for &j in &group {
            if j != k {
                self.links[j].busy_until = self.links[j].busy_until.max(start + air);
            }
        }
        self.links[k].packets += 1;
        self.links[k].airtime_ms += air;
        self.stats.trunk_packets += 1;
        if self.sc.trunk_mesh_channel {
            // On the mesh channel: the sending end's area hears a carrier.
            let tx = self.next_tx;
            self.next_tx += 1;
            self.active.push(Tx { id: tx, sender: node, start, end: start + air, packet: packet.clone(), nx: -1, flood: false, noise: true });
        }
        self.schedule(start + air, Ev::Trunk { link: k, to: peer, packet: packet.clone(), start });
    }

    /// Is a trunk packet arriving at `to` (sent from `start`) jammed by a
    /// mesh transmission there within 6 dB (after the antenna's rejection)?
    fn trunk_jammed(&self, link: usize, to: usize, start: u64) -> bool {
        let end = self.now;
        let r = self.radio(self.nodes[to].sf);
        let trunk_dbm = r.noise_dbm + self.links[link].snr as f64;
        self.active.iter().any(|t| {
            !t.noise
                && t.start < end
                && t.end > start
                && (t.sender == to || self.nodes[to].gain_to(t.sender as u32).is_some_and(|rx| rx as f64 - self.sc.trunk_rejection_db > trunk_dbm - 6.0))
        })
    }

    /// Radio islands (nodes connected by audible mesh links, no trunks or
    /// TCP), for the trunk summaries.
    fn radio_islands(&self) -> Vec<usize> {
        let n = self.nodes.len();
        let mut parent: Vec<usize> = (0..n).collect();
        fn find(p: &mut [usize], mut i: usize) -> usize {
            while p[i] != i {
                p[i] = p[p[i]];
                i = p[i];
            }
            i
        }
        for i in 0..n {
            for &(j, rx) in &self.nodes[i].nbrs {
                if self.audible_rx(i, rx) {
                    let (a, b) = (find(&mut parent, i), find(&mut parent, j as usize));
                    parent[a] = b;
                }
            }
        }
        (0..n).map(|i| find(&mut parent, i)).collect()
    }

    /// Trunk choice for a request for `dest` arriving at trunk end `node`:
    /// with summaries, only trunks whose far end is on `dest`'s side; with
    /// hashing, only the one with the highest rendezvous score among the
    /// group's (live) trunks. Unknown destinations and second requests go
    /// over every trunk the summary allows.
    fn trunk_choose(&mut self, k: usize, node: usize, peer: usize, dest: &Address) -> bool {
        let Some(&d) = self.addr_idx.get(dest) else { return true };
        if self.islands.len() != self.nodes.len() {
            self.islands = self.radio_islands();
        }
        let now = self.now;
        let key = (k, node, d);
        let repeat = self.trunk_dropped.get(&key).is_some_and(|&t| now < t + 120_000);
        if self.sc.trunk_summary {
            // The far side has the destination?
            if self.islands[d] != self.islands[peer] {
                // Not behind this trunk; on the second request only if it is
                // not known on this side either.
                if self.islands[d] == self.islands[node] {
                    return false;
                }
                if !repeat {
                    self.trunk_dropped.insert(key, now);
                    return false;
                }
                self.trunk_dropped.remove(&key);
                return true;
            }
        }
        if self.sc.trunk_hash && !repeat {
            let group = self.links[k].group.clone();
            let addr = self.nodes[d].router.addr;
            let mut best = (0u64, k);
            for j in 0..self.links.len() {
                let l = &self.links[j];
                if !l.lora || l.group != group || group.is_empty() || !self.link_ok(j) {
                    continue;
                }
                let far = if self.islands[l.a] == self.islands[node] { l.b } else { l.a };
                if self.sc.trunk_summary && self.islands[d] != self.islands[far] {
                    continue;
                }
                let s = maarse_core::mail::score(&addr, &self.nodes[l.a].router.addr) ^ (j as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
                if s >= best.0 {
                    best = (s, j);
                }
            }
            if best.1 != k {
                self.trunk_dropped.insert(key, now);
                return false;
            }
        }
        self.trunk_dropped.remove(&key);
        true
    }

    /// May a request for `dest` cross trunk `k` from `node`? Yes unless
    /// `dest` is known to be on this side (heard announcing over this end's
    /// own radio) - then it goes over only on a second request. The end
    /// knows its own side already; nothing has to be exchanged.
    fn trunk_behind(&mut self, k: usize, node: usize, _peer: usize, dest: &Address) -> bool {
        let Some(&d) = self.addr_idx.get(dest) else { return true };
        if !self.local_seen[node].contains(&(d as u32)) {
            return true;
        }
        let now = self.now;
        let key = (k, node, d);
        if self.trunk_dropped.get(&key).is_some_and(|&t| now < t + 120_000) {
            self.trunk_dropped.remove(&key);
            return true;
        }
        self.trunk_dropped.insert(key, now);
        false
    }

    /// Size of a text message on the air (compressed or not).
    fn msg_size(&self) -> usize {
        if self.sc.text_compress { 34 } else { 60 }
    }

    // ---------- post offices ----------

    fn init_mail(&mut self) {
        let n = self.nodes.len();
        self.mailbox = vec![HashMap::default(); n];
        self.radio_seen = vec![HashMap::default(); n];
        self.was_on_backbone = vec![true; n];
        self.present = vec![HashMap::default(); n];
        self.home_po = vec![None; n];
        self.prev_home = vec![None; n];
        self.got = vec![Vec::new(); n];
        self.last_sync = vec![0; n];
        self.mail_class = (0..n)
            .map(|i| match self.nodes[i].kind {
                Kind::Host if !self.sc.mail.is_empty() => maarse_core::mail::CLASS_LARGE,
                Kind::Repeater | Kind::Anchor if self.sc.mail == "post" && self.sc.repeater_mail => maarse_core::mail::CLASS_SMALL,
                _ => 0,
            })
            .collect();
        for i in 0..n {
            self.nodes[i].router.mail_class = self.mail_class[i];
            self.id_idx.insert(self.nodes[i].router.id, i);
        }
        // Replicas only on hosts with internet (the ones in the host DHT).
        self.host_list = (0..n).filter(|&i| self.nodes[i].kind == Kind::Host && self.nodes[i].internet).collect();
        self.named_box = (0..n).map(|i| if self.nodes[i].kind.is_user() { self.nearest_office(i, 3) } else { None }).collect();
        if self.sc.mail == "post" {
            self.schedule(15_000, Ev::MailTick);
        }
        // Sleepy handhelds: chosen by index (no draw from the random stream,
        // so other runs are unchanged), each with its own phase.
        self.asleep = vec![false; n];
        self.sleepy = (0..n)
            .map(|i| {
                let h = (i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 40;
                let pick = (h % 1000) as f64 / 1000.0 < self.sc.sleepy_share;
                (self.nodes[i].kind == Kind::Handheld && pick).then_some(h % 100_000)
            })
            .collect();
        if self.sleepy.iter().any(Option::is_some) {
            self.schedule(5_000, Ev::SleepTick);
        }
    }

    /// Sleepy handhelds: radio on for `wake_s` of every cycle; a node that
    /// wakes asks its post office for waiting mail.
    fn sleep_tick(&mut self) {
        let cycle = ((self.sc.sleep_s + self.sc.wake_s) * 1000.0) as u64;
        let wake = (self.sc.wake_s * 1000.0) as u64;
        if cycle == 0 {
            return;
        }
        for i in 0..self.nodes.len() {
            let Some(phase) = self.sleepy[i] else { continue };
            let asleep = (self.now + phase * 37) % cycle >= wake;
            if self.asleep[i] && !asleep {
                self.stats.wakeups += 1;
                if self.sc.mail == "post" && self.nodes[i].alive() {
                    self.mail_sync(i);
                }
            }
            self.asleep[i] = asleep;
        }
    }

    /// The nearest post office of at least `class` to node `i` (by distance).
    fn nearest_office(&self, i: usize, class: u8) -> Option<usize> {
        let (x, y) = (self.nodes[i].x, self.nodes[i].y);
        (0..self.nodes.len())
            .filter(|&j| j != i && self.mail_class.get(j).is_some_and(|&c| c >= class) && self.nodes[j].alive())
            .min_by(|&a, &b| {
                let da = (self.nodes[a].x - x).hypot(self.nodes[a].y - y);
                let db = (self.nodes[b].x - x).hypot(self.nodes[b].y - y);
                da.total_cmp(&db)
            })
    }

    /// A user's post office now: the best one its hellos told it about,
    /// else the nearest within 20 km.
    fn current_office(&mut self, i: usize) -> Option<usize> {
        if let Some((id, _, _)) = self.nodes[i].router.best_post_office(self.now)
            && let Some(&j) = self.id_idx.get(&id)
        {
            return Some(j);
        }
        let j = self.nearest_office(i, 1)?;
        let d = (self.nodes[j].x - self.nodes[i].x).hypot(self.nodes[j].y - self.nodes[i].y);
        (d < 20_000.0).then_some(j)
    }

    /// Sends a `Deposit` with `op` about `recipient` from `node` to `office`.
    fn send_deposit(&mut self, node: usize, office: usize, recipient: usize, op: u8, body: &[u8]) {
        if node == office || !self.nodes[node].alive() {
            return;
        }
        let mut p: heapless::Vec<u8, 160> = heapless::Vec::new();
        let rec = self.nodes[recipient].router.addr;
        if !maarse_core::mail::build(&mut p, &rec, op, body) {
            return;
        }
        if self.nodes[node].kind == Kind::Host && self.nodes[office].kind == Kind::Host {
            // Host to host: only over a direct TCP connection (as the DHT
            // uses), never radio. Without TCP it waits until TCP is back
            // (hinted handoff).
            if self.on_backbone(node) && self.on_backbone(office) {
                self.stats.mail_tcp += 1;
                self.schedule(self.now + 50, Ev::MailTcp { to: office, payload: p.to_vec() });
            } else if self.mail_deferred.len() < 10_000 {
                self.stats.mail_deferred += 1;
                self.mail_deferred.push((node, office, p.to_vec()));
            }
            return;
        }
        let dest = self.nodes[office].router.addr;
        let mut token = [0u8; 16];
        self.rng.fill(&mut token[..]);
        self.stats.mail_ops += 1;
        self.tr(format!("{{\"t\":{},\"ev\":\"mail_send\",\"n\":{node},\"to\":{office},\"op\":{op},\"rec\":{recipient}}}", self.now));
        let before = self.nodes[node].router.stats;
        if let Ok((_, actions)) = self.nodes[node].router.send_typed(PacketType::Deposit, dest, &p, token, self.now) {
            self.after_router(node, before, None);
            self.apply(node, actions.into_iter().collect(), None);
        }
    }

    /// A `Deposit` over the radio (routed), also between hosts: for
    /// consolidation when the backbone is down.
    fn send_deposit_lora(&mut self, node: usize, office: usize, recipient: usize, op: u8, body: &[u8]) {
        if node == office || !self.nodes[node].alive() {
            return;
        }
        let mut p: heapless::Vec<u8, 160> = heapless::Vec::new();
        let rec = self.nodes[recipient].router.addr;
        if !maarse_core::mail::build(&mut p, &rec, op, body) {
            return;
        }
        let dest = self.nodes[office].router.addr;
        let mut token = [0u8; 16];
        self.rng.fill(&mut token[..]);
        self.stats.mail_ops += 1;
        let before = self.nodes[node].router.stats;
        if let Ok((_, actions)) = self.nodes[node].router.send_typed(PacketType::Deposit, dest, &p, token, self.now) {
            self.after_router(node, before, None);
            self.apply(node, actions.into_iter().collect(), None);
        }
    }

    /// A STORE body: copy flag (0 = from the sender, 1 = a copy), the
    /// message index, padded to the size of a sealed message.
    fn store_body(m: usize, copy: bool) -> heapless::Vec<u8, 64> {
        let mut b: heapless::Vec<u8, 64> = heapless::Vec::new();
        let _ = b.push(copy as u8);
        let _ = b.extend_from_slice(&(m as u32).to_be_bytes());
        while b.len() < 60 {
            let _ = b.push(0);
        }
        b
    }

    fn id_list(ids: &[usize]) -> heapless::Vec<u8, 64> {
        let mut b: heapless::Vec<u8, 64> = heapless::Vec::new();
        for &m in ids.iter().take(8) {
            let _ = b.extend_from_slice(&(m as u32).to_be_bytes());
        }
        b
    }

    /// 45 s after message `m`: no ack, so drop it at a post office.
    fn mail_check(&mut self, m: usize) {
        if self.msgs[m].delivered_ms.is_some() || self.msgs[m].acked_ms.is_some() || self.msgs[m].unsent {
            return;
        }
        let (from, to) = (self.msgs[m].from, self.msgs[m].to);
        if !self.nodes[from].alive() {
            return;
        }
        let office = if self.sc.mail == "named" { self.named_box[to] } else { self.current_office(from) };
        let Some(office) = office else { return };
        self.stats.mail_deposits += 1;
        let body = Self::store_body(m, false);
        if office == from {
            self.store(from, to, m, false);
        } else {
            self.send_deposit(from, office, to, maarse_core::mail::OP_STORE, &body);
        }
    }

    /// The replica hosts of `recipient` (rendezvous hashing over all hosts).
    fn replica_hosts(&self, recipient: usize) -> Vec<usize> {
        let addrs: Vec<Address> = self.host_list.iter().map(|&h| self.nodes[h].router.addr).collect();
        let r: heapless::Vec<Address, { maarse_core::mail::REPLICAS }> = maarse_core::mail::replicas(&self.nodes[self.pkey(recipient)].router.addr, &addrs);
        r.iter().filter_map(|a| self.addr_idx.get(a).copied()).collect()
    }

    /// Stores message `m` for `recipient` at post office `node` (quotas: a
    /// small office keeps 4 per recipient and 30 in all, a host 100 per
    /// recipient) and passes copies on.
    fn store(&mut self, node: usize, recipient: usize, m: usize, copy: bool) {
        let class = self.mail_class[node];
        if class == 0 {
            return;
        }
        let device = recipient;
        // Mail from outside is kept for the person; copies, sync requests and
        // anything else between a person's own devices for that device.
        let own = self.devices(recipient).contains(&self.msgs[m].from);
        let recipient = if self.msgs[m].sync_of.is_some() || own { recipient } else { self.pkey(recipient) };
        let total: usize = self.mailbox[node].values().map(|v| v.len()).sum();
        let list = self.mailbox[node].entry(recipient).or_default();
        if list.contains(&m) {
            return;
        }
        let (per, all) = if class >= 3 { (100, usize::MAX) } else { (4, 30) };
        if list.len() >= per || total >= all {
            self.stats.mail_full += 1;
            return;
        }
        list.push(m);
        self.stats.mail_stored += 1;
        self.update_tim(node);
        if !copy && self.sc.mail == "post" {
            // The office that took the message tries to deliver it itself
            // now and then (5, 10, 20, 40 ... min), like a mail server.
            self.mail_retry.entry((node, recipient)).or_insert((self.now + 300_000, 0));
        }
        if self.sc.mail == "post" && !copy {
            let body = Self::store_body(m, true);
            if class >= 3 {
                // Three host copies, over TCP.
                for r in self.replica_hosts(recipient) {
                    if r != node {
                        self.send_deposit(node, r, recipient, maarse_core::mail::OP_STORE, &body);
                    }
                }
                // Off the backbone: a copy to the recipient's home office over
                // LoRa, so one sync there (or to it) gets everything.
                if self.sc.mail_consolidate
                    && !self.on_backbone(node)
                    && let Some(h) = self.heard_home.get(&(node, recipient)).copied()
                    && h != node
                {
                    self.stats.mail_consolidated += 1;
                    self.send_deposit_lora(node, h, recipient, maarse_core::mail::OP_STORE, &body);
                }
            } else if let Some(h) = self.nearest_office(node, 3) {
                // A small office passes a copy to the nearest host.
                self.send_deposit(node, h, recipient, maarse_core::mail::OP_STORE, &body);
            }
        }
        let here = self.devices(device).into_iter().find(|d| self.present[node].get(d).is_some_and(|&t| self.now < t));
        if let Some(here) = here {
            // Only the new one: the rest is on its way already.
            let others: Vec<usize> = self.mailbox[node].get(&recipient).map(|v| v.iter().copied().filter(|&x| x != m).collect()).unwrap_or_default();
            self.deliver_mail(node, here, &others);
        }
    }

    /// The recipients with mail waiting, told in the office's hello.
    fn update_tim(&mut self, node: usize) {
        let ids: Vec<u16> = self.mailbox[node].iter().filter(|(_, v)| !v.is_empty()).flat_map(|(&r, _)| self.devices(r)).map(|r| self.nodes[r].router.id).take(4).collect();
        let w = &mut self.nodes[node].router.mail_waiting;
        w.clear();
        for id in ids {
            let _ = w.push(id);
        }
    }

    /// Office `node` sends what it holds for `recipient` (except `have`).
    fn deliver_mail(&mut self, node: usize, recipient: usize, have: &[usize]) {
        let key = self.pkey(recipient);
        if key != recipient {
            // Linked devices: the device's own copies, then the person's mail.
            self.deliver_mail_key(node, recipient, recipient, have);
        }
        if !self.sc.device_groups.is_empty() {
            self.retarget_mail(node, key, recipient);
        }
        self.deliver_mail_key(node, recipient, key, have);
    }

    /// `deliver_mail` for the mail kept under `key` at `node`.
    fn deliver_mail_key(&mut self, node: usize, recipient: usize, key: usize, have: &[usize]) {
        // Not what the recipient has, and not what was sent in the last minute.
        let now = self.now;
        let sent = &self.mail_sent;
        let mut list: Vec<usize> = self.mailbox[node]
            .get(&key)
            .map(|v| v.iter().copied().filter(|m| !have.contains(m) && sent.get(&(node, *m)).is_none_or(|&t| now >= t + 60_000)).collect())
            .unwrap_or_default();
        if self.sc.mail_paced {
            // One at a time over a long route (the next when this one is
            // acknowledged): a burst would overflow the LoRa trunks.
            let local = self.on_backbone(node) && self.radio_seen.get(node).is_some_and(|m| m.get(&recipient).is_some_and(|&t| now < t + 3_600_000));
            let inflight = self.mailbox[node].get(&key).is_some_and(|v| v.iter().any(|m| sent.get(&(node, *m)).is_some_and(|&t| now < t + 60_000)));
            if !local {
                if inflight {
                    return;
                }
                list.truncate(1);
            }
        }
        if list.is_empty() || !self.nodes[node].alive() {
            return;
        }
        for &m in &list {
            self.mail_sent.insert((node, m), now);
            self.tr(format!("{{\"t\":{now},\"ev\":\"mail_deliver\",\"n\":{node},\"rec\":{recipient},\"msg\":{m}}}"));
        }
        let dest = self.nodes[recipient].router.addr;
        if self.sc.mail_bundle && list.len() >= 2 {
            // Up to three messages per packet: "BNDL" | count | indices,
            // padded to the size the messages would take.
            for chunk in list.chunks(3) {
                let mut payload = b"BNDL".to_vec();
                payload.push(chunk.len() as u8);
                for &m in chunk {
                    payload.extend_from_slice(&(m as u32).to_be_bytes());
                }
                payload.resize((self.msg_size() * chunk.len()).min(maarse_core::router::MAX_DIRECT_PAYLOAD), 0);
                let mut token = [0u8; 16];
                self.rng.fill(&mut token[..]);
                self.stats.mail_deliveries += 1;
                let before = self.nodes[node].router.stats;
                if let Ok((msg_id, actions)) = self.nodes[node].router.send_direct(dest, &payload, token, self.now) {
                    self.bundles.insert(msg_id, (node, key, chunk.to_vec()));
                    self.after_router(node, before, Some(chunk[0] as u32));
                    self.apply(node, actions.into_iter().collect(), Some(chunk[0] as u32));
                }
            }
            return;
        }
        for m in list {
            let payload = self.msgs[m].payload.clone();
            let mut token = [0u8; 16];
            self.rng.fill(&mut token[..]);
            self.stats.mail_deliveries += 1;
            let before = self.nodes[node].router.stats;
            if let Ok((_, actions)) = self.nodes[node].router.send_direct(dest, &payload, token, self.now) {
                self.after_router(node, before, Some(m as u32));
                self.apply(node, actions.into_iter().collect(), Some(m as u32));
            }
        }
    }

    /// An ack for mail an office delivered: delete it everywhere.
    fn mail_acked(&mut self, node: usize, msg_id: u64) -> bool {
        if let Some((office, r, ms)) = self.bundles.remove(&msg_id) {
            if let Some(v) = self.mailbox[office].get_mut(&r) {
                v.retain(|x| !ms.contains(x));
            }
            for m in &ms {
                self.mail_sent.remove(&(office, *m));
            }
            self.update_tim(office);
            if self.sc.mail == "post" {
                let body = Self::id_list(&ms);
                let others: Vec<usize> = if self.mail_class[office] >= 3 { self.replica_hosts(r) } else { self.nearest_office(office, 3).into_iter().collect() };
                for o in others {
                    if o != office {
                        self.send_deposit(office, o, r, maarse_core::mail::OP_DELIVERED, &body);
                    }
                }
            }
            return true;
        }
        if self.mailbox.is_empty() || self.mailbox[node].is_empty() {
            return false;
        }
        let hit = self.mailbox[node].iter().find_map(|(&r, v)| v.iter().copied().find(|&m| self.msgs[m].id == msg_id).map(|m| (r, m)));
        let Some((r, m)) = hit else { return false };
        if let Some(v) = self.mailbox[node].get_mut(&r) {
            v.retain(|&x| x != m);
        }
        self.update_tim(node);
        self.mail_sent.remove(&(node, m));
        if self.sc.mail_paced {
            // Paced: the next message now.
            self.deliver_mail(node, r, &[]);
        }
        if self.sc.mail == "post" {
            let body = Self::id_list(&[m]);
            let others: Vec<usize> = if self.mail_class[node] >= 3 { self.replica_hosts(r) } else { self.nearest_office(node, 3).into_iter().collect() };
            for o in others {
                if o != node {
                    self.send_deposit(node, o, r, maarse_core::mail::OP_DELIVERED, &body);
                }
            }
        }
        true
    }

    fn on_deposit(&mut self, node: usize, payload: &[u8]) {
        let Some(d) = maarse_core::mail::Deposit::parse(payload) else { return };
        let Some(&rec) = self.addr_idx.get(&d.recipient) else { return };
        let op = d.op;
        self.tr(format!("{{\"t\":{},\"ev\":\"mail_op\",\"n\":{node},\"op\":{op},\"rec\":{rec}}}", self.now));
        match d.op {
            maarse_core::mail::OP_STORE => {
                let copy = d.body.first().copied() == Some(1);
                let m = d.body.get(1..5).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize);
                if let Some(m) = m.filter(|&m| m < self.msgs.len()) {
                    self.store(node, rec, m, copy);
                }
            }
            maarse_core::mail::OP_SYNC => {
                // Deliver to the recipient (wherever it is); a host also
                // fetches the copies.
                let have: Vec<usize> = d.ids().map(|x| x as usize).collect();
                self.present[node].insert(rec, self.now + 600_000);
                self.deliver_mail(node, rec, &have);
                if self.mail_class[node] >= 3 {
                    self.fetch_copies(node, rec, &have);
                }
            }
            maarse_core::mail::OP_FETCH => {
                let have: Vec<usize> = d.ids().map(|x| x as usize).collect();
                let Some(&asker) = d.asker().as_ref().and_then(|a| self.addr_idx.get(a)) else { return };
                let list: Vec<usize> = self.mail_keys(rec).iter().flat_map(|k| self.mailbox[node].get(k).cloned().unwrap_or_default()).filter(|m| !have.contains(m)).collect();
                for m in list {
                    let body = Self::store_body(m, true);
                    self.send_deposit(node, asker, rec, maarse_core::mail::OP_STORE, &body);
                }
            }
            maarse_core::mail::OP_DELIVERED => {
                let ids: Vec<usize> = d.ids().map(|x| x as usize).collect();
                for key in self.mail_keys(rec) {
                    if let Some(v) = self.mailbox[node].get_mut(&key) {
                        v.retain(|m| !ids.contains(m));
                    }
                }
                self.update_tim(node);
            }
            _ => {}
        }
    }

    /// Host `node` asks the replicas for `rec`'s mail (they send copies).
    fn fetch_copies(&mut self, node: usize, rec: usize, have: &[usize]) {
        let mut body: heapless::Vec<u8, 64> = heapless::Vec::new();
        let _ = body.extend_from_slice(&self.nodes[node].router.addr.0);
        for &m in have.iter().take(8) {
            let _ = body.extend_from_slice(&(m as u32).to_be_bytes());
        }
        for r in self.replica_hosts(rec) {
            if r != node {
                self.send_deposit(node, r, rec, maarse_core::mail::OP_FETCH, &body);
            }
        }
    }

    /// A `MailSync` from `dest` heard by `node`.
    fn on_mail_sync(&mut self, node: usize, dest: Address, payload: &[u8]) {
        if self.mail_class.get(node).is_none_or(|&c| c == 0) {
            return;
        }
        let Some(&rec) = self.addr_idx.get(&dest) else { return };
        if !self.bb_ctx && self.nodes[node].kind == Kind::Host {
            self.radio_seen[node].insert(rec, self.now);
        }
        let (home, have) = maarse_core::mail::parse_sync(payload);
        let have: Vec<usize> = have.iter().map(|&x| x as usize).collect();
        let holds = self.holds_mail(node, rec);
        let named = home == Some(self.nodes[node].router.id);
        if !holds && !named {
            return;
        }
        self.present[node].insert(rec, self.now + 600_000);
        self.deliver_mail(node, rec, &have);
        if named {
            if self.mail_class[node] >= 3 {
                self.fetch_copies(node, rec, &have);
            } else if let Some(h) = self.nearest_office(node, 3) {
                // A small office asks its host to deliver (and fetch).
                let body = Self::id_list(&have);
                self.send_deposit(node, h, rec, maarse_core::mail::OP_SYNC, &body);
            }
        }
    }

    /// User `i` asks for its mail (a `MailSync` naming its home office).
    fn mail_sync(&mut self, i: usize) {
        let home = self.current_office(i);
        self.home_po[i] = home.or(self.home_po[i]);
        let home_id = home.map(|h| self.nodes[h].router.id);
        let have: Vec<u32> = self.got[i].iter().map(|&m| m as u32).collect();
        self.last_sync[i] = self.now;
        self.stats.mail_syncs += 1;
        let actions = self.nodes[i].router.request_mail(home_id, &have);
        self.apply(i, actions.into_iter().collect(), None);
    }

    /// Every 15 s: users notice a new home office (then sync here and ask
    /// the previous one too), act on "mail waiting" hints, and sync every
    /// 6 hours anyway.
    fn mail_tick(&mut self) {
        if self.sc.office_carry {
            self.office_carry();
        }
        // Host-to-host mail that waited for TCP.
        if !self.mail_deferred.is_empty() {
            let all = std::mem::take(&mut self.mail_deferred);
            let (ready, wait): (Vec<_>, Vec<_>) = all.into_iter().partition(|(a, b, _)| self.on_backbone(*a) && self.on_backbone(*b) && self.nodes[*b].alive());
            self.mail_deferred = wait;
            if !ready.is_empty() {
                for (_, to, payload) in ready {
                    self.stats.mail_tcp += 1;
                    self.schedule(self.now + 50, Ev::MailTcp { to, payload });
                }
            }
        }
        // Backbone back: fetch the mail of the nodes heard over our radio in
        // the last 24 h (now + 1 min, and again after 5 min).
        if self.sc.mail_refetch {
            for &h in &self.host_list.clone() {
                let on = self.on_backbone(h);
                if on && !self.was_on_backbone[h] {
                    self.refetch_at.push((self.now + 60_000, h));
                    self.refetch_at.push((self.now + 300_000, h));
                }
                self.was_on_backbone[h] = on;
            }
            let now = self.now;
            let due: Vec<usize> = self.refetch_at.iter().filter(|(t, _)| *t <= now).map(|(_, h)| *h).collect();
            self.refetch_at.retain(|(t, _)| *t > now);
            for h in due {
                let seen: Vec<usize> = self.radio_seen[h].iter().filter(|(_, t)| now < **t + 86_400_000).map(|(&r, _)| r).collect();
                for r in seen {
                    self.present[h].insert(r, now + 600_000);
                    self.deliver_mail(h, r, &[]);
                    self.fetch_copies(h, r, &[]);
                    self.stats.mail_refetches += 1;
                }
            }
        }
        // Offices retry delivering what they took in.
        let due: Vec<(usize, usize)> = self.mail_retry.iter().filter(|(_, (t, _))| *t <= self.now).map(|(&k, _)| k).collect();
        for (office, rec) in due {
            let holds = self.mailbox[office].get(&rec).is_some_and(|v| !v.is_empty());
            if !holds || !self.nodes[office].alive() {
                self.mail_retry.remove(&(office, rec));
                continue;
            }
            // With the backbone, presence and syncs deliver more cheaply;
            // without it, at most three tries (5, 15, 35 min), then the mail
            // waits for the recipient to ask.
            let step = self.mail_retry[&(office, rec)].1 + 1;
            let max = if self.sc.mail_long_retry { 6 } else { 3 };
            if step > max || self.on_backbone(office) {
                self.mail_retry.remove(&(office, rec));
            } else {
                // 5, 15, 35 min, then (long retries) 1, 2 and 4 hours after that.
                let wait = match step {
                    1 => 600_000,
                    2 => 1_200_000,
                    3 => 1_500_000,
                    4 => 3_600_000,
                    _ => 7_200_000,
                };
                self.mail_retry.insert((office, rec), (self.now + wait, step));
            }
            if !self.on_backbone(office) {
                self.stats.mail_retries += 1;
                self.deliver_mail(office, rec, &[]);
            }
        }
        for i in 0..self.nodes.len() {
            if !self.nodes[i].kind.is_user() || !self.nodes[i].alive() {
                continue;
            }
            let now = self.now;
            let hint = self.nodes[i].router.take_mail_hint();
            let office = self.current_office(i);
            if office.is_some() && office != self.home_po[i] {
                let prev = self.home_po[i];
                self.prev_home[i] = prev;
                self.home_po[i] = office;
                if now > self.last_sync[i] + 60_000 {
                    self.mail_sync(i);
                }
                if let Some(p) = prev
                    && Some(p) != office
                {
                    // Moved: ask the previous home office too (it delivers
                    // over whatever route reaches us, LoRa trunks included).
                    let have: Vec<usize> = self.got[i].clone();
                    let body = Self::id_list(&have);
                    self.send_deposit(i, p, i, maarse_core::mail::OP_SYNC, &body);
                }
            } else if (hint && now > self.last_sync[i] + 600_000) || now > self.last_sync[i] + 6 * 3600_000 {
                self.mail_sync(i);
            }
        }
    }

    /// Node `node` asks "who is `wanted`?" (at most every 30 s per pair).
    fn ask_key(&mut self, node: usize, wanted: usize) {
        if self.key_asked.get(&(node, wanted)).is_some_and(|&t| self.now < t + 30_000) {
            return;
        }
        self.key_asked.insert((node, wanted), self.now);
        let addr = self.nodes[wanted].router.addr;
        self.stats.key_requests += 1;
        let actions = self.nodes[node].router.request_key(addr);
        self.apply(node, actions.into_iter().collect(), None);
    }

    /// Bordercast: should host `node` keep this TCP-borne packet off its
    /// radio? Path and key requests go out only for a destination heard
    /// announcing in the host's own region, or on a second request.
    fn keep_off_radio(&mut self, node: usize, packet: &[u8]) -> bool {
        let Ok(p) = Packet::decode(packet) else { return false };
        if !matches!(p.header.ptype, PacketType::PathRequest | PacketType::KeyRequest) {
            return false;
        }
        let Some(&dest) = self.addr_idx.get(&p.header.dest) else { return true };
        if self.local_seen[node].contains(&(dest as u32)) {
            return false;
        }
        let now = self.now;
        self.bc_dropped[node].retain(|&(_, t)| now < t + 120_000);
        if let Some(k) = self.bc_dropped[node].iter().position(|&(d, _)| d == dest) {
            self.bc_dropped[node].remove(k);
            return false;
        }
        self.bc_dropped[node].push((dest, now));
        self.stats.bordercast_kept += 1;
        true
    }

    /// Host `host` answers a key request with `who`'s announce it keeps.
    fn reannounce(&mut self, host: usize, who: usize) {
        let addr = self.nodes[who].router.addr;
        let mut body: heapless::Vec<u8, { maarse_core::MAX_PACKET }> = heapless::Vec::new();
        let mut keys = [0u8; 64];
        self.rng.fill(&mut keys[..]);
        let _ = body.extend_from_slice(&keys);
        let _ = body.extend_from_slice(&((self.now / 1000) as u32 + 1_700_000_000).to_be_bytes());
        // App data: the role field, as the firmware sends it (forwarders read
        // the mobile flag from it to give a moving node's announce full scope).
        let mut app = maarse_core::appdata::AppBuf::new();
        let rt = &self.nodes[who].router;
        let flags = if rt.mobile { maarse_core::appdata::ROLE_MOBILE } else { 0 };
        maarse_core::appdata::push_role(&mut app, rt.role.as_u8(), flags);
        let _ = body.push(app.len() as u8);
        let _ = body.extend_from_slice(&app);
        let _ = body.extend_from_slice(&[0u8; 64]);
        maarse_core::stamp::append(PacketType::Announce, &addr, &mut body);
        let header = Header::new(PacketType::Announce, addr);
        let Ok(packet) = encode(&header, &body) else { return };
        let pid = packet_id(PacketType::Announce, &addr, &body);
        self.stats.key_answers += 1;
        self.apply(host, vec![Action::Transmit { packet, delay_ms: 0, id: pid }], None);
    }

    /// Hands message `m` to its sender's router (route or path request).
    fn send_real(&mut self, m: usize) {
        let (from, to) = (self.msgs[m].from, self.msgs[m].to);
        if !self.nodes[from].alive() {
            return;
        }
        let payload = self.msgs[m].payload.clone();
        let dest = self.nodes[to].router.addr;
        let mut token = [0u8; 16];
        self.rng.fill(&mut token[..]);
        let before = self.nodes[from].router.stats;
        if let Ok((_, actions)) = self.nodes[from].router.send_direct(dest, &payload, token, self.now) {
            self.after_router(from, before, Some(m as u32));
            self.apply(from, actions.into_iter().collect(), Some(m as u32));
        }
    }

    // ---------- home routing model ----------

    fn is_fixed(&self, i: usize) -> bool {
        let n = &self.nodes[i];
        n.alive() && matches!(n.kind, Kind::Repeater | Kind::Anchor | Kind::Host | Kind::Wifi) && n.route.len() < 2 && n.wander <= 0.0
    }

    /// Infrastructure links, homes and radio islands (recomputed when the
    /// network changed: deaths, events, every 60 s for moving nodes).
    fn refresh_infra(&mut self) {
        if !self.infra_dirty && self.home.len() == self.nodes.len() {
            return;
        }
        self.infra_dirty = false;
        let n = self.nodes.len();
        let fixed: Vec<bool> = (0..n).map(|i| self.is_fixed(i)).collect();
        let good = |e: &Engine, i: usize, rx: f32| {
            let r = e.radio(e.nodes[i].sf);
            r.snr(rx as f64) >= r.snr_min + INFRA_MARGIN_DB
        };
        let mut infra = vec![Vec::new(); n];
        let mut home = vec![-1i32; n];
        for i in 0..n {
            if !self.nodes[i].alive() {
                continue;
            }
            if fixed[i] {
                home[i] = i as i32;
                let mut l: Vec<(u32, f32)> = self.nodes[i].nbrs.iter().copied().filter(|&(j, rx)| fixed[j as usize] && good(self, i, rx)).collect();
                // Strongest first, so the shortest path prefers strong links.
                l.sort_by(|a, b| b.1.total_cmp(&a.1));
                infra[i] = l.into_iter().map(|(j, _)| j).collect();
            } else if let Some(&(j, _)) = self.nodes[i]
                .nbrs
                .iter()
                .filter(|&&(j, rx)| fixed[j as usize] && self.audible_rx(i, rx))
                .max_by(|a, b| a.1.total_cmp(&b.1))
            {
                home[i] = j as i32;
            }
        }
        self.infra = infra;
        self.home = home;
        self.rcomp = self.components_with(false);
    }

    /// Shortest path in hops over infrastructure links, `a` and `b` included.
    fn infra_path(&self, a: usize, b: usize) -> Option<Vec<usize>> {
        if a == b {
            return Some(vec![a]);
        }
        let mut prev = vec![u32::MAX; self.nodes.len()];
        prev[a] = a as u32;
        let mut q = std::collections::VecDeque::from([a]);
        while let Some(u) = q.pop_front() {
            for &v in &self.infra[u] {
                let v = v as usize;
                if prev[v] != u32::MAX {
                    continue;
                }
                prev[v] = u as u32;
                if v == b {
                    let mut path = vec![b];
                    let mut x = b;
                    while x != a {
                        x = prev[x] as usize;
                        path.push(x);
                    }
                    path.reverse();
                    return Some(path);
                }
                q.push_back(v);
            }
        }
        None
    }

    /// Forwarders between `from` and `to` (both excluded) along the fixed
    /// infrastructure, via the gateways if they are on different islands.
    /// When that is longer than a source path holds, the route is split at the
    /// sender's gateway (an anchor): the sender's leg ends just before it, and
    /// the gateway gets its own route for the rest (`Ok((leg, Some((gw, rest))))`).
    /// Err: why there is no such path ("no_home", "no_infra_path",
    /// "unreachable", "too_long").
    fn home_path(&mut self, from: usize, to: usize) -> Result<(Vec<usize>, Option<(usize, Vec<usize>)>), &'static str> {
        self.refresh_infra();
        let (ha, hb) = (self.home[from], self.home[to]);
        if ha < 0 || hb < 0 {
            return Err("no_home");
        }
        let (ha, hb) = (ha as usize, hb as usize);
        let strip = |c: Vec<usize>| -> Vec<usize> { c.into_iter().filter(|&x| x != from && x != to).collect() };
        // Two candidates: all the way by radio (same island), or up to the
        // sender's gateway, over the backbone, and down from the destination's
        // gateway. The one with fewer radio hops wins.
        let radio = if self.rcomp[ha] == self.rcomp[hb] { self.infra_path(ha, hb).map(strip) } else { None };
        let (g1, g2) = (self.gateway_of(from), self.gateway_of(to));
        let via = if g1 >= 0 && g2 >= 0 && g1 != g2 {
            let (g1, g2) = (g1 as usize, g2 as usize);
            match (self.infra_path(ha, g1), self.infra_path(g2, hb)) {
                (Some(u), Some(d)) => Some((g1, g2, u, d)),
                _ => None,
            }
        } else {
            None
        };
        let via_len = via.as_ref().map(|(_, _, u, d)| u.len() + d.len());
        match (&radio, via_len) {
            (Some(r), Some(v)) if r.len() <= v.min(maarse_core::MAX_PATH) => return Ok((r.clone(), None)),
            (Some(r), None) if r.len() <= maarse_core::MAX_PATH => return Ok((r.clone(), None)),
            (Some(_), None) => return Err("too_long"),
            (None, None) if self.rcomp[ha] == self.rcomp[hb] => return Err("no_infra_path"),
            (None, None) => return Err("unreachable"),
            _ => {}
        }
        let Some((g1, g2, up, down)) = via else { return Err("no_infra_path") };
        // The gateways connect directly after the lookup (DHT).
        self.dyn_links.insert((g1.min(g2), g1.max(g2)));
        let mut whole = up.clone();
        whole.extend(down.iter().copied());
        let path = strip(whole);
        if path.len() <= maarse_core::MAX_PATH {
            return Ok((path, None));
        }
        // Split at the sender's gateway (an anchor): leg 1 ends before it.
        let leg: Vec<usize> = strip(up).into_iter().filter(|&x| x != g1).collect();
        let rest = strip(down);
        if leg.len() > maarse_core::MAX_PATH || rest.len() > maarse_core::MAX_PATH || g1 == from {
            return Err("too_long");
        }
        Ok((leg, Some((g1, rest))))
    }

    fn path_of(&self, nodes: &[usize]) -> maarse_core::Path {
        let mut p = maarse_core::Path::new();
        for &x in nodes {
            let _ = p.push(self.nodes[x].router.id);
        }
        p
    }

    fn nodes_json(v: &[usize]) -> String {
        v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(",")
    }

    /// Sends a lookup datagram from `a` to `b` along the installed path.
    fn send_datagram(&mut self, a: usize, b: usize, path: &[usize], body: &[u8], m: usize) {
        let dest = self.nodes[b].router.addr;
        let p = self.path_of(path);
        self.nodes[a].router.add_route(dest, p, self.now);
        let mut token = [0u8; 16];
        self.rng.fill(&mut token[..]);
        let before = self.nodes[a].router.stats;
        if let Ok((_, actions)) = self.nodes[a].router.send_typed(PacketType::Datagram, dest, body, token, self.now) {
            self.after_router(a, before, Some(m as u32));
            self.apply(a, actions.into_iter().collect(), Some(m as u32));
        }
    }

    fn start_lookup(&mut self, m: usize) {
        let (from, to) = (self.msgs[m].from, self.msgs[m].to);
        self.stats.home_lookups += 1;
        self.refresh_infra();
        // Local registry: announces flood `LOCAL_SCOPE` hops, so every fixed
        // node already hears the announces of nodes that close. The sender's
        // own home answers for them (one hop away, or the sender itself).
        let (h, t) = (self.home[from], self.home[to]);
        if h >= 0 && t >= 0 && self.rcomp[h as usize] == self.rcomp[t as usize] {
            let (h, t) = (h as usize, t as usize);
            if self.infra_path(h, t).is_some_and(|c| c.len() <= maarse_core::LOCAL_SCOPE as usize + 1) {
                self.stats.home_local += 1;
                self.tr(format!("{{\"t\":{},\"ev\":\"lookup\",\"n\":{from},\"to\":{h},\"local\":true,\"path\":[],\"msg\":{m}}}", self.now));
                self.lookups.insert(m, Lookup { from, to, gw: h, up: Vec::new(), route: Vec::new(), tries: 0, done: false });
                return self.ask(m);
            }
        }
        let gw = self.gateway_of(from);
        if gw < 0 {
            return self.home_fallback(m, "no_gateway");
        }
        let gw = gw as usize;
        let up: Vec<usize> = match (self.home[from], gw) {
            (h, g) if h >= 0 => match self.infra_path(h as usize, g) {
                Some(c) => c.into_iter().filter(|&x| x != from && x != gw).collect(),
                None => return self.home_fallback(m, "no_infra_path"),
            },
            _ => return self.home_fallback(m, "no_home"),
        };
        if up.len() > maarse_core::MAX_PATH {
            return self.home_fallback(m, "too_long");
        }
        self.tr(format!("{{\"t\":{},\"ev\":\"lookup\",\"n\":{from},\"to\":{gw},\"path\":[{}],\"msg\":{m}}}", self.now, Self::nodes_json(&up)));
        self.lookups.insert(m, Lookup { from, to, gw, up, route: Vec::new(), tries: 0, done: false });
        self.ask(m);
    }

    /// Sends (or resends) the lookup question to the registry node.
    fn ask(&mut self, m: usize) {
        let Some(l) = self.lookups.get_mut(&m) else { return };
        l.tries += 1;
        let (from, reg, up) = (l.from, l.gw, l.up.clone());
        self.schedule(self.now + LOOKUP_RETRY_MS, Ev::LookupTimeout { msg: m });
        if reg == from {
            return self.answer_lookup(m);
        }
        let mut body = vec![b'L'];
        body.extend_from_slice(&(m as u32).to_be_bytes());
        self.send_datagram(from, reg, &up, &body, m);
    }

    fn on_lookup_datagram(&mut self, node: usize, payload: &[u8]) {
        if payload.len() < 5 {
            return;
        }
        let m = u32::from_be_bytes([payload[1], payload[2], payload[3], payload[4]]) as usize;
        let Some(l) = self.lookups.get(&m) else { return };
        if l.done {
            return;
        }
        match payload[0] {
            b'L' if node == l.gw => self.answer_lookup(m),
            b'R' | b'N' | b'U' if node == l.from => self.finish_lookup(m, payload[0]),
            _ => {}
        }
    }

    /// The gateway answers: R (a route follows), U (known unreachable) or N.
    fn answer_lookup(&mut self, m: usize) {
        let (from, to, gw, up) = {
            let l = &self.lookups[&m];
            (l.from, l.to, l.gw, l.up.clone())
        };
        let res = self.home_path(from, to);
        let (code, path) = match &res {
            Ok((p, _)) => (b'R', p.clone()),
            Err("unreachable") => (b'U', Vec::new()),
            Err(_) => (b'N', Vec::new()),
        };
        // A split route: the gateway (anchor) gets its own route to the
        // destination now (the registry is on the gateways).
        if let Ok((_, Some((anchor, rest)))) = &res {
            let dest = self.nodes[to].router.addr;
            let p = self.path_of(rest);
            self.nodes[*anchor].router.add_route(dest, p, self.now);
            self.tr(format!("{{\"t\":{},\"ev\":\"route_split\",\"n\":{anchor},\"path\":[{}],\"msg\":{m}}}", self.now, Self::nodes_json(rest)));
        }
        let why = match &res {
            Ok(_) => "",
            Err(w) => w,
        };
        self.tr(format!(
            "{{\"t\":{},\"ev\":\"lookup_answer\",\"n\":{gw},\"to\":{from},\"ok\":{},\"why\":\"{why}\",\"path\":[{}],\"msg\":{m}}}",
            self.now,
            code == b'R',
            Self::nodes_json(&path)
        ));
        if let Some(l) = self.lookups.get_mut(&m) {
            l.route = path.clone();
        }
        if gw == from {
            return self.finish_lookup(m, code);
        }
        let mut body = vec![code];
        body.extend_from_slice(&(m as u32).to_be_bytes());
        let back: Vec<usize> = up.iter().rev().copied().collect();
        self.send_datagram(gw, from, &back, &body, m);
        let _ = to;
    }

    fn finish_lookup(&mut self, m: usize, code: u8) {
        let Some(l) = self.lookups.get_mut(&m) else { return };
        l.done = true;
        let (from, to, path) = (l.from, l.to, l.route.clone());
        match code {
            b'R' => {
                self.stats.home_routed += 1;
                let dest = self.nodes[to].router.addr;
                let p = self.path_of(&path);
                self.nodes[from].router.add_route(dest, p, self.now);
                self.tr(format!("{{\"t\":{},\"ev\":\"lookup_done\",\"n\":{from},\"path\":[{}],\"msg\":{m}}}", self.now, Self::nodes_json(&path)));
                self.send_real(m);
            }
            b'U' => {
                // Known unreachable: do not flood the country for nothing (in
                // the real system the message would wait in the mailbox).
                self.stats.home_unreachable += 1;
                self.tr(format!("{{\"t\":{},\"ev\":\"lookup_done\",\"n\":{from},\"unreachable\":true,\"msg\":{m}}}", self.now));
            }
            _ => self.home_fallback(m, "no_path"),
        }
    }

    /// No usable answer: fall back to an ordinary path request.
    fn home_fallback(&mut self, m: usize, why: &str) {
        if let Some(l) = self.lookups.get_mut(&m) {
            l.done = true;
        }
        *self.stats.home_fallback.entry(why.to_string()).or_default() += 1;
        let from = self.msgs[m].from;
        self.tr(format!("{{\"t\":{},\"ev\":\"lookup_fallback\",\"n\":{from},\"why\":\"{why}\",\"msg\":{m}}}", self.now));
        self.send_real(m);
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
        // App data: the role field, as the firmware sends it (forwarders read
        // the mobile flag from it to give a moving node's announce full scope).
        let mut app = maarse_core::appdata::AppBuf::new();
        let rt = &self.nodes[node].router;
        let flags = if rt.mobile { maarse_core::appdata::ROLE_MOBILE } else { 0 };
        maarse_core::appdata::push_role(&mut app, rt.role.as_u8(), flags);
        let _ = body.push(app.len() as u8);
        let _ = body.extend_from_slice(&app);
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
            recv: HashMap::default(),
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

    /// Nodes inside an area; an empty area means every node.
    fn nodes_in(&self, area: &Area) -> Vec<usize> {
        let (poly, circle) = area_xy(area);
        let everywhere = circle.is_none() && poly.len() < 3;
        (0..self.nodes.len())
            .filter(|&i| {
                let (x, y) = (self.nodes[i].x, self.nodes[i].y);
                everywhere || if let Some((cx, cy, r)) = circle { (x - cx).hypot(y - cy) <= r } else { point_in_poly(x, y, &poly) }
            })
            .collect()
    }

    fn revive_node(&mut self, i: usize) {
        if self.nodes[i].dead == DeadWhy::Killed {
            self.nodes[i].dead = DeadWhy::Alive;
            self.tr(format!("{{\"t\":{},\"ev\":\"revive\",\"n\":{i}}}", self.now));
            self.reboot(i);
            if self.sc.mail == "post" && self.nodes[i].kind.is_user() {
                // Ask for mail once hellos have told us the post offices.
                let at = self.now + 30_000 + self.rng.gen_range(0..20_000);
                self.schedule(at, Ev::MailSyncAt { node: i });
            }
        }
    }

    /// Does node `i`'s app know node `j`'s keys?
    fn knows(&self, i: usize, j: usize) -> bool {
        if self.sc.contact_capacity > 0 {
            return self.pinned[i].contains(&(j as u32)) || self.contacts[i].contains_key(&(j as u32));
        }
        !self.forgot[i] || self.relearned[i].contains(&(j as u32))
    }

    /// Node `i` heard `j`'s announce: into the contact table (evicting the
    /// one heard longest ago when full). Without pinning, every new entry is
    /// a flash write (the firmware stores every new contact).
    fn learn_contact(&mut self, i: usize, j: usize) {
        let j = j as u32;
        if self.pinned[i].contains(&j) {
            return;
        }
        let fresh = self.contacts[i].insert(j, self.now).is_none();
        if fresh && !self.sc.features.pin_contacts {
            self.flash_writes[i] += 1;
        }
        if self.contacts[i].len() > self.sc.contact_capacity {
            let used = &self.used[i];
            let pick = if self.sc.contact_policy == "used" {
                // Strangers (never written to or heard from) go first.
                self.contacts[i].iter().filter(|e| !used.contains(e.0)).min_by_key(|e| *e.1).or_else(|| self.contacts[i].iter().min_by_key(|e| *e.1))
            } else {
                self.contacts[i].iter().min_by_key(|e| *e.1)
            };
            if let Some((&old, _)) = pick {
                self.contacts[i].remove(&old);
            }
        }
    }

    /// With `pin_contacts`: node `i` talked with `j` (or `j` is a saved
    /// contact), so it is kept for good and written to flash once.
    fn pin(&mut self, i: usize, j: usize) {
        self.used[i].insert(j as u32);
        if !self.sc.features.pin_contacts || self.sc.contact_capacity == 0 {
            return;
        }
        let j = j as u32;
        if self.pinned[i].len() < self.sc.pinned_capacity && self.pinned[i].insert(j) {
            self.contacts[i].remove(&j);
            self.flash_writes[i] += 1;
        }
    }

    /// Circles of friends: `traffic.friends` random users within `local_km`.
    fn make_friends(&mut self) {
        let users: Vec<usize> = (0..self.nodes.len()).filter(|&i| self.nodes[i].kind.is_user()).collect();
        let lim = self.sc.traffic.local_km * 1000.0;
        for &u in &users {
            let mut near: Vec<usize> = users
                .iter()
                .copied()
                .filter(|&v| v != u && (self.nodes[v].x - self.nodes[u].x).hypot(self.nodes[v].y - self.nodes[u].y) <= lim)
                .collect();
            let mut pick = Vec::new();
            for _ in 0..self.sc.traffic.friends.min(near.len()) {
                let k = self.rng.gen_range(0..near.len());
                pick.push(near.swap_remove(k));
            }
            self.friends[u] = pick;
        }
        // Friends are saved contacts: pinned from the start (if pinning).
        if self.sc.features.pin_contacts && self.sc.contact_capacity > 0 {
            for u in 0..self.nodes.len() {
                let f = self.friends[u].clone();
                for v in f {
                    self.pin(u, v);
                    self.pin(v, u);
                }
            }
            self.flash_writes.iter_mut().for_each(|w| *w = 0);
        }
    }

    /// A fresh router for node `i`, as after a reboot: same address, role and
    /// settings, but no routes, neighbours, sessions or queues.
    fn fresh_router(&self, i: usize) -> Box<Node> {
        let f = &self.sc.features;
        let spec = &self.sc.nodes[i];
        let old = &self.nodes[i].router;
        let mut r = Box::new(Node::new(old.addr, old.role));
        let _ = r.subscribe(self.public_channel);
        let _ = r.subscribe(self.bulletin_channel);
        r.mobile = old.mobile;
        r.hello = old.hello;
        r.path_retry_ms = old.path_retry_ms;
        r.proxy_reply = old.proxy_reply;
        r.ring = old.ring;
        r.weak_hop_penalty = old.weak_hop_penalty;
        r.drop_unacked_routes = old.drop_unacked_routes;
        r.anchor_ttl_ms = old.anchor_ttl_ms;
        r.boundary = old.boundary;
        r.carry_copies = old.carry_copies;
        r.carry_max = old.carry_max;
        r.carry_ttl_ms = old.carry_ttl_ms;
        r.carry_smart = old.carry_smart;
        r.local_repair = f.local_repair;
        r.load_control = f.load_control;
        r.relay_hops = old.relay_hops;
        if f.sf_slot {
            r.set_slot(maarse_core::qos::slot_ms(&LoraParams::new(self.nodes[i].sf)), f.scale_timeouts);
        }
        if f.regions && !spec.region.is_empty() {
            r.regions = maarse_core::region::parse_list(&format!("dk,{}", spec.region));
        }
        r
    }

    /// Off and on again: everything in RAM is lost (routes, neighbours,
    /// queue, kept messages); the contacts too unless `persist_known`. The
    /// node announces 5 s after starting, as the firmware does.
    fn reboot(&mut self, i: usize) {
        if !self.nodes[i].alive() {
            return;
        }
        self.stats.reboots += 1;
        let kept: Vec<maarse_core::routes::Route> = if self.sc.features.persist_routes {
            let mut v: Vec<_> = self.nodes[i].router.routes().iter().filter(|r| r.confirmed_ms > 0).cloned().collect();
            v.sort_unstable_by_key(|r| Reverse(r.confirmed_ms));
            v.truncate(32);
            v
        } else {
            Vec::new()
        };
        self.nodes[i].router = self.fresh_router(i);
        for r in kept {
            self.nodes[i].router.add_route(r.dest, r.path, self.now);
        }
        self.nodes[i].txq.clear();
        for m in std::mem::take(&mut self.held[i]) {
            self.msgs[m].dropped_unknown = true;
        }
        if !self.sc.features.persist_known {
            self.forgot[i] = true;
            self.relearned[i].clear();
            self.contacts[i].clear();
            self.pinned[i].clear();
        } else if self.sc.contact_capacity > 0 && !self.sc.features.pin_contacts {
            // The flash holds the 63 most recently learned contacts.
            let mut v: Vec<(u32, u64)> = self.contacts[i].iter().map(|(&k, &t)| (k, t)).collect();
            v.sort_unstable_by_key(|&(_, t)| Reverse(t));
            v.truncate(63);
            self.contacts[i] = v.into_iter().collect();
        } else if self.sc.contact_capacity > 0 {
            // Only pinned contacts are in flash.
            self.contacts[i].clear();
        }
        self.gw_dirty = true;
        self.infra_dirty = true;
        self.tr(format!("{{\"t\":{},\"ev\":\"reboot\",\"n\":{i},\"kept_contacts\":{}}}", self.now, self.sc.features.persist_known));
        // Announce 5-35 s after the start (at random, as the firmware does),
        // then every 15 min until a repeater is heard.
        self.settled[i] = false;
        self.announced[i] = false;
        let first = self.now + 5000 + self.rng.gen_range(0..30_000);
        self.plan_announce(i, first);
    }

    /// Automatic forest mode (checked every minute): a handheld without a
    /// repeater or anchor in range for `auto_relay_after_s` becomes a relay; one that has
    /// heard one for 2 min without a break becomes a client again.
    fn auto_relay_step(&mut self) {
        let to_relay_ms = (self.sc.auto_relay_after_s.max(0.0) * 1000.0) as u64;
        const TO_CLIENT_MS: u64 = 120_000;
        let hold_ms = if self.sc.bridge_hold_s > 0.0 { (self.sc.bridge_hold_s * 1000.0) as u64 } else { TO_CLIENT_MS };
        let carry = if self.sc.features.carry { 4 } else { 0 };
        for i in 0..self.nodes.len() {
            if self.nodes[i].kind != Kind::Handheld || !self.nodes[i].alive() {
                continue;
            }
            let hears = self.nodes[i].router.hears_repeater(self.now);
            if hears {
                self.rep_last[i] = self.now;
                if self.rep_since[i] == 0 {
                    self.rep_since[i] = self.now.max(1);
                }
            } else {
                self.rep_since[i] = 0;
            }
            let role = self.nodes[i].router.role;
            let rt = &self.nodes[i].router;
            let mpr = self.sc.auto_bridge == "mpr";
            let named = self.sc.auto_bridge == "named";
            let fwd_n = rt.forwarding_neighbors(self.now);
            let mut edge = false;
            // A client steps in only after 2 min of steady coverage, when it
            // knows its neighbors (after a mass restart every neighbor first
            // looks uncovered).
            let steady = role == Role::Relay || (self.rep_since[i] > 0 && self.now.saturating_sub(self.rep_since[i]) >= TO_CLIENT_MS);
            if self.sc.features.auto_edge && hears && steady {
                if named {
                    // OLSR selectors: stay while a neighbor has chosen us;
                    // step in for a neighbor without coverage that has no
                    // bridge yet.
                    edge = rt.unserved_neighbor(self.now) || (role == Role::Relay && rt.bridge_named(self.now));
                } else if mpr {
                    // OLSR-style: only for a neighbor without coverage that
                    // has no bridge yet; a bridge stays while one has fewer
                    // than two.
                    edge = rt.needs_bridge(self.now, if role == Role::Relay { 2 } else { 1 });
                } else {
                    edge = rt.has_uncovered_neighbor(self.now);
                    if edge
                        && let Some(other) = rt.bridge_neighbor(self.now)
                        && (role == Role::Client || other < rt.id)
                    {
                        edge = false;
                    }
                }
                let p = if self.sc.bridge_p < 0.0 {
                    // Adaptive (Trickle-like): the more neighbors that are
                    // candidates too, the longer each waits.
                    let floor = if self.sc.bridge_p > -1.0 { -self.sc.bridge_p } else { 0.05 };
                    (1.0 / (1.0 + rt.covered_neighbors(self.now) as f64)).max(floor)
                } else if self.sc.bridge_p > 0.0 {
                    self.sc.bridge_p.min(1.0)
                } else {
                    0.25
                };
                if edge && role == Role::Client && !self.rng.gen_bool(p) {
                    // Spread the switches so neighbors do not all become
                    // bridges in the same round.
                    edge = false;
                }
            }
            if edge {
                // A bridge stays a relay; the 2 min towards client start
                // again once it is no longer needed.
                self.rep_since[i] = self.now.max(1);
            }
            let dense = self.sc.auto_max_fwd > 0 && fwd_n >= self.sc.auto_max_fwd;
            let crowded = self.sc.auto_drop_fwd > 0 && fwd_n >= self.sc.auto_drop_fwd;
            let to = if role == Role::Client && (edge || (!hears && !dense && self.now.saturating_sub(self.rep_last[i]) >= to_relay_ms)) {
                Some(Role::Relay)
            } else if role == Role::Relay && hears && !edge && self.now.saturating_sub(self.rep_since[i]) >= hold_ms {
                Some(Role::Client)
            } else if role == Role::Relay && !hears && crowded && self.rng.gen_bool(0.25) {
                // Thread-style: enough forwarders around, step back.
                Some(Role::Client)
            } else {
                None
            };
            self.nodes[i].router.boundary = self.sc.bridge_boundary;
            if named && self.sc.announce_s > 0.0 {
                // As the firmware: a new bridge choice goes out in an
                // announce soon, so the chosen bridge knows it is needed.
                let c = self.nodes[i].router.chosen_bridge(self.now);
                if c != self.told_choice[i] {
                    self.told_choice[i] = c;
                    let soon = self.now + 5_000 + self.rng.gen_range(0..30_000);
                    if soon < self.next_ann[i] {
                        self.plan_announce(i, soon);
                    }
                }
            }
            if let Some(r) = to {
                self.nodes[i].router.set_role(r);
                self.nodes[i].router.carry_copies = if r == Role::Relay { carry } else { 0 };
                self.stats.role_switches += 1;
                self.tr(format!("{{\"t\":{},\"ev\":\"role\",\"n\":{i},\"role\":\"{:?}\"}}", self.now, r));
                if self.sc.announce_s > 0.0 {
                    // As the firmware: neighbors learn the new role from an
                    // announce soon after the switch.
                    let soon = self.now + 5_000 + self.rng.gen_range(0..30_000);
                    if soon < self.next_ann[i] {
                        self.plan_announce(i, soon);
                    }
                }
            }
        }
    }

    /// Found by the network: hears a repeater, or (named bridges) has a
    /// bridge to rely on, so it can announce at the long interval.
    fn found(&self, i: usize) -> bool {
        let rt = &self.nodes[i].router;
        rt.hears_repeater(self.now) || (self.sc.auto_bridge == "named" && rt.chosen_bridge(self.now).is_some())
    }

    /// Schedules node `i`'s next announce (replacing any earlier one).
    fn plan_announce(&mut self, i: usize, at: u64) {
        self.next_ann[i] = at;
        self.schedule(at, Ev::Announce { node: i });
    }

    pub fn inject(&mut self, ev: &TimedEvent) {
        let what = ev.kind.as_str();
        self.tr(format!("{{\"t\":{},\"ev\":\"event\",\"what\":\"{what}\",\"region\":\"{}\"}}", self.now, ev.region));
        self.gw_dirty = true;
        self.infra_dirty = true;
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
                for i in self.nodes_in(&ev.area) {
                    if self.nodes[i].mains {
                        self.nodes[i].powered = false;
                        if self.nodes[i].battery_wh <= 0.0 && self.nodes[i].alive() {
                            self.kill(i, DeadWhy::Power);
                        }
                    }
                }
                self.update_energy_levels();
            }
            "power_restore" => {
                for i in self.nodes_in(&ev.area) {
                    if self.nodes[i].mains {
                        self.nodes[i].powered = true;
                        if matches!(self.nodes[i].dead, DeadWhy::Power | DeadWhy::Battery) {
                            self.nodes[i].dead = DeadWhy::Alive;
                            // It was off: it starts again from scratch.
                            self.reboot(i);
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
                let list = if ev.nodes.is_empty() { vec![ev.node] } else { ev.nodes.clone() };
                let n_nodes = self.nodes.len();
                for i in list.into_iter().filter(|&i| i < n_nodes) {
                    self.kill(i, DeadWhy::Killed);
                }
            }
            "kill_area" => {
                let want = ev.node_kind.as_str();
                for i in self.nodes_in(&ev.area) {
                    if want.is_empty() || want == "any" || self.nodes[i].kind.name() == want {
                        self.kill(i, DeadWhy::Killed);
                    }
                }
            }
            "revive" => {
                let list: Vec<usize> = if ev.nodes.is_empty() { (0..self.nodes.len()).collect() } else { ev.nodes.clone() };
                let n_nodes = self.nodes.len();
                for i in list.into_iter().filter(|&i| i < n_nodes) {
                    self.revive_node(i);
                }
                self.seed_neighbors();
            }
            "reboot" => {
                let want = ev.node_kind.as_str();
                let ok_kind = |e: &Engine, i: usize| want.is_empty() || want == "any" || e.nodes[i].kind.name() == want;
                let mut list: Vec<usize> = if !ev.nodes.is_empty() {
                    ev.nodes.clone()
                } else if ev.area.circle.is_some() || ev.area.polygon.len() >= 3 {
                    self.nodes_in(&ev.area)
                } else {
                    (0..self.nodes.len()).collect()
                };
                let n_nodes = self.nodes.len();
                list.retain(|&i| i < n_nodes && self.nodes[i].alive() && ok_kind(self, i));
                if ev.n > 0 && ev.nodes.is_empty() {
                    // `n` random ones of those.
                    let mut pick = Vec::new();
                    for _ in 0..ev.n.min(list.len()) {
                        let k = self.rng.gen_range(0..list.len());
                        pick.push(list.swap_remove(k));
                    }
                    list = pick;
                }
                for i in list {
                    self.reboot(i);
                }
                self.seed_neighbors();
            }
            "host_internet" => {
                if ev.node < self.nodes.len() {
                    self.nodes[ev.node].internet = ev.up;
                }
            }
            "degrade" => {
                let (poly, circle) = area_xy(&ev.area);
                if (circle.is_some() || poly.len() >= 3) && ev.db > 0.0 {
                    self.degrade.push(LossArea::new(poly, circle, ev.db));
                    self.build_links();
                }
            }
            "degrade_clear" => {
                self.degrade.clear();
                self.build_links();
            }
            "move" => {
                // Teleport a node (a journey): to the centre of `area.circle`.
                if ev.node < self.nodes.len()
                    && let Some([la, lo, _]) = ev.area.circle
                {
                    let (x, y) = to_xy(la, lo);
                    self.nodes[ev.node].x = x;
                    self.nodes[ev.node].y = y;
                    self.build_links();
                    self.seed_neighbors();
                    self.islands.clear();
                }
            }
            "sync" => self.person_sync(ev.node),
            "message" => {
                if ev.from < self.nodes.len() && ev.node < self.nodes.len() {
                    self.send_message(ev.from, ev.node);
                }
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
        self.gw_dirty = true;
        self.infra_dirty = true;
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
        let walkers: Vec<usize> = (0..self.nodes.len()).filter(|&i| self.nodes[i].wander > 0.0 && self.nodes[i].route.len() < 2).collect();
        for i in walkers {
            let step = self.nodes[i].speed * 2.0;
            let target = match self.nodes[i].target {
                Some(t) if (t.0 - self.nodes[i].x).hypot(t.1 - self.nodes[i].y) > step => t,
                _ => {
                    // A new random point in the disc around home.
                    let r = self.nodes[i].wander * self.rng.gen_range(0.0f64..1.0).sqrt();
                    let a = self.rng.gen_range(0.0..std::f64::consts::TAU);
                    let t = (self.nodes[i].home.0 + r * a.cos(), self.nodes[i].home.1 + r * a.sin());
                    self.nodes[i].target = Some(t);
                    t
                }
            };
            let n = &self.nodes[i];
            let d = (target.0 - n.x).hypot(target.1 - n.y);
            let f = if d > 0.0 { (step / d).min(1.0) } else { 0.0 };
            let p = (n.x + (target.0 - n.x) * f, n.y + (target.1 - n.y) * f);
            self.place(i, p);
        }
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
            self.place(i, p);
        }
    }

    /// Moves node `i` to `p` and recomputes its links.
    fn place(&mut self, i: usize, p: (f64, f64)) {
        let n = &mut self.nodes[i];
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
            "coll_flood": self.stats.coll_flood,
            "coll_next_hop": self.stats.coll_next_hop,
            "coll_half_duplex": self.stats.coll_half_duplex,
            "dht_lookups": self.stats.dht_lookups,
            "dht_misses": self.stats.dht_misses,
            "home_lookups": self.stats.home_lookups,
            "home_routed": self.stats.home_routed,
            "home_unreachable": self.stats.home_unreachable,
            "home_local": self.stats.home_local,
            "reboots": self.stats.reboots,
            "role_switches": self.stats.role_switches,
            "contacts_heard": self.contacts_heard(),
            "mail_deposits": self.stats.mail_deposits,
            "mail_stored": self.stats.mail_stored,
            "mail_full": self.stats.mail_full,
            "mail_ops": self.stats.mail_ops,
            "mail_deliveries": self.stats.mail_deliveries,
            "mail_syncs": self.stats.mail_syncs,
            "mail_tcp": self.stats.mail_tcp,
            "mail_deferred": self.stats.mail_deferred,
            "mail_retries": self.stats.mail_retries,
            "mail_refetches": self.stats.mail_refetches,
            "mail_consolidated": self.stats.mail_consolidated,
            "redirect_hints": self.stats.redirect_hints,
            "office_carried": self.stats.office_carried,
            "sync_presses": self.stats.sync_presses,
            "backfilled": self.stats.backfilled,
            "trunk_packets": self.stats.trunk_packets,
            "trunk_dropped": self.stats.trunk_dropped,
            "trunk_jammed": self.stats.trunk_jammed,
            "proxy_replies": self.nodes.iter().map(|x| x.router.stats.proxy_replies as u64).sum::<u64>(),
            "trunks": self.links.iter().filter(|l| l.lora).map(|l| json!({"label": l.label, "packets": l.packets, "airtime_s": l.airtime_ms as f64 / 1000.0, "dropped": l.dropped})).collect::<Vec<_>>(),
            "hellos": self.nodes.iter().map(|x| x.router.stats.hellos as u64).sum::<u64>(),
            "key_requests": self.stats.key_requests,
            "key_answers": self.stats.key_answers,
            "bordercast_kept": self.stats.bordercast_kept,
            "cross_msgs": self.msgs.iter().filter(|m| m.cross).count(),
            "cross_delivered_pct": self.cross_delivered_pct(),
            "flash_writes_mean": self.flash_writes.iter().map(|&w| w as f64).sum::<f64>() / self.nodes.len().max(1) as f64,
            "flash_writes_max": self.flash_writes.iter().copied().max().unwrap_or(0),
            "relays_now": self.nodes.iter().filter(|n| n.alive() && n.router.role == Role::Relay).count(),
            "periodic_announces": self.stats.periodic_announces,
            "shown_pct": pct(self.msgs.iter().filter(|m| m.shown_ms.is_some()).count()),
            "false_acks": self.msgs.iter().filter(|m| m.acked_ms.is_some() && m.shown_ms.is_none()).count(),
            "dropped_unknown": self.msgs.iter().filter(|m| m.dropped_unknown).count(),
            "held_waiting": self.held.iter().map(|h| h.len()).sum::<usize>(),
            "shown_after_hold": self.msgs.iter().filter(|m| m.shown_ms.is_some() && m.delivered_ms.is_some_and(|d| m.shown_ms.unwrap() > d)).count(),
            "unsent_unknown": self.msgs.iter().filter(|m| m.unsent).count(),
            "home_fallback": self.stats.home_fallback,
            "degraded_areas": self.degrade.len(),
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
            "carry_rerouted": sum(|s| s.carry_rerouted),
            "routes_learned": sum(|s| s.routes_learned),
            "path_requests": sum(|s| s.path_requests),
            "quota_dropped": sum(|s| s.quota_dropped),
            "queue_delay": qd,
            "by_type": self.stats.by_type,
            "air_by_type": self.stats.air_by_type,
            "internet": self.internet_global,
            "internet_down": self.internet_down.iter().collect::<Vec<_>>(),
            "alerts": (0..self.alerts.len()).map(|i| self.alert_summary(i, false)).collect::<Vec<_>>(),
            "trace_end": self.trace_base + self.trace.len() as u64,
        })
    }

    /// Delivered share of messages into or out of an area without coverage.
    fn cross_delivered_pct(&self) -> f64 {
        let c = self.msgs.iter().filter(|m| m.cross).count().max(1);
        100.0 * self.msgs.iter().filter(|m| m.cross && m.delivered_ms.is_some()).count() as f64 / c as f64
    }

    /// How many distinct announcers the nodes have heard (mean, median, p90,
    /// max, and how many nodes heard more than the 63 contacts kept in flash).
    fn contacts_heard(&self) -> Value {
        let mut v: Vec<usize> = self.heard_from.iter().map(|h| h.len()).collect();
        v.sort_unstable();
        let n = v.len().max(1);
        let mean = v.iter().sum::<usize>() as f64 / n as f64;
        let over = v.iter().filter(|&&x| x > 63).count();
        json!({"mean": mean, "median": v.get(v.len() / 2).copied().unwrap_or(0), "p90": v.get(v.len() * 9 / 10).copied().unwrap_or(0), "max": v.last().copied().unwrap_or(0), "over_63": over})
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
        // Role now: c client, r relay, b relay that hears a repeater (edge
        // bridge), R repeater, A anchor.
        let mut role = String::with_capacity(self.nodes.len());
        for (i, n) in self.nodes.iter().enumerate() {
            role.push(match n.router.role {
                Role::Client => 'c',
                Role::Relay if n.router.hears_repeater(self.now) => 'b',
                Role::Relay => 'r',
                Role::Repeater => 'R',
                Role::Anchor => 'A',
            });
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
        let trunks: Vec<Value> = self.links.iter().filter(|l| l.lora).map(|l| json!({"label": l.label, "packets": l.packets, "airtime_s": l.airtime_ms as f64 / 1000.0, "dropped": l.dropped})).collect();
        json!({"state": alive, "role": role, "lat": x, "lon": y, "battery": batt, "airtime_ms": air, "queue": q, "comp": comp, "links_ok": links, "trunks": trunks})
    }

    /// DHT backbone: the host with internet that can reach node `t` by radio
    /// (the nearest one in its radio island), or -1.
    fn gateway_of(&mut self, t: usize) -> i32 {
        if self.gw_dirty || self.gw.len() != self.nodes.len() {
            self.gw_dirty = false;
            let comp = self.components_with(false);
            let hosts: Vec<usize> = (0..self.nodes.len()).filter(|&i| self.nodes[i].alive() && self.has_internet(i)).collect();
            self.gw = (0..self.nodes.len())
                .map(|i| {
                    if comp[i] < 0 {
                        return -1;
                    }
                    let d = |h: usize| (self.nodes[h].x - self.nodes[i].x).hypot(self.nodes[h].y - self.nodes[i].y);
                    hosts
                        .iter()
                        .copied()
                        .filter(|&h| comp[h] == comp[i])
                        .min_by(|&a, &b| d(a).total_cmp(&d(b)))
                        .map_or(-1, |h| h as i32)
                })
                .collect();
        }
        self.gw.get(t).copied().unwrap_or(-1)
    }

    /// Connected components ("islands") of the forwarding graph: forwarders
    /// that hear each other both ways, plus working backbone links; users join
    /// the component of a forwarder they hear (or stay alone).
    pub fn components(&self) -> Vec<i32> {
        self.components_with(true)
    }

    /// Like [`Engine::components`]; `backbone = false` gives radio islands only.
    pub fn components_with(&self, backbone: bool) -> Vec<i32> {
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
            if backbone && self.sc.features.backbone && self.link_ok(k) {
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
            "dead_hops": rt.dead_hops().iter().filter(|(_, until)| *until > self.now).map(|(id, until)| json!({"node": ids.get(id), "for_s": (until - self.now) as f64 / 1000.0})).collect::<Vec<_>>(),
            "gateway": self.gw.get(i).copied().unwrap_or(-1),
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
        let mut hops: HashMap<usize, u8> = HashMap::default();
        let mut first: HashMap<usize, u64> = HashMap::default();
        let mut forwarded: HashSet<usize> = HashSet::default();
        let mut cancelled: HashSet<usize> = HashSet::default();
        let mut dropped: HashMap<usize, String> = HashMap::default();
        let mut tx_hops: HashMap<u64, (usize, u8)> = HashMap::default();
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
            .map(|(i, m)| json!([i, m.from, m.to, m.sent_ms, m.delivered_ms, m.acked_ms, format!("{:016x}", m.id), m.sync_of]))
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

