//! maarse-sim: simulator for Må:rse networks.
//!
//! Places repeaters and clients in an area, simulates LoRa radio with
//! range, collisions, half-duplex, listen-before-talk and duty cycle,
//! and runs `maarse_core::Router` unchanged in every node.
//!
//! For comparison it can also run "flood" mode, where all nodes
//! (clients too) repeat everything with a hop limit, like Meshtastic.
//!
//! Example:
//!   cargo run --release -p maarse-sim -- --nodes 120 --repeaters 30 --messages 60
//!   cargo run --release -p maarse-sim -- --mode flood --hop-limit 7
#![forbid(unsafe_code)]

mod trace;

use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use maarse_core::airtime::LoraParams;
use maarse_core::dedup::packet_id;
use maarse_core::duty::DutyCycle;
use maarse_core::packet::{Header, Packet, encode};
use maarse_core::router::{Buf, ack_token};
use maarse_core::{Action, Address, PacketType, Role, Router};

type Node = Router<256, 1024>;

// ---------- configuration ----------

#[derive(Clone, Debug)]
struct Config {
    nodes: usize,
    repeaters: usize,
    area_km: f64,
    messages: usize,
    channel_messages: usize,
    seed: u64,
    sf: u8,
    duty_permille: u32,
    mode: Mode,
    hop_limit: u8,
    tx_dbm: f64,
    repeater_gain_db: f64,
    duration_s: u64,
    /// Motorway: repeaters on a line, a car drives past.
    highway: bool,
    road_km: f64,
    car_kmh: f64,
    /// The repeaters' positions along the road (km); empty = evenly spread.
    repeater_km: Vec<f64>,
    /// The base stands at the start of the road (the car drives away from it).
    base_start: bool,
    /// Every Nth repeater gets the anchor role (0 = none).
    anchor_every: usize,
    /// Forest: all non-repeaters walk around, and the forest attenuates extra.
    forest: bool,
    /// Extra attenuation in forest (dB per km).
    forest_db_km: f64,
    walk_kmh: f64,
    /// Role for all participants (non-repeaters) in forest mode.
    group_role: Role,
    relay_hops: Option<u8>,
    /// Forest with a hill: repeater 12 km away, base by the repeater and one man
    /// on a hill in the middle of the forest (`None` = he walks around like the others).
    hill: bool,
    hill_role: Option<Role>,
    /// Extra antenna gain for the man on the hill (height, line of sight).
    hill_gain_db: f64,
    /// Flood suppression for relays: 0 = normal, 1 = bridge (relays that hear a
    /// repeater directly do not stay quiet), 2 = no suppression for relays.
    suppress: u8,
    /// Carry (store-carry-forward) in the router: copies per message that
    /// could not be delivered; 0 = off (see `Router` "Carry").
    carry_copies: u8,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    Maarse,
    Flood,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            nodes: 120,
            repeaters: 30,
            area_km: 30.0,
            messages: 60,
            channel_messages: 10,
            seed: 1,
            sf: 9,
            duty_permille: 100,
            mode: Mode::Maarse,
            hop_limit: 7,
            tx_dbm: 20.0,
            repeater_gain_db: 6.0,
            duration_s: 900,
            highway: false,
            road_km: 100.0,
            car_kmh: 110.0,
            repeater_km: Vec::new(),
            base_start: false,
            anchor_every: 0,
            forest: false,
            forest_db_km: 10.0,
            walk_kmh: 5.0,
            group_role: Role::Relay,
            relay_hops: None,
            hill: false,
            hill_role: Some(Role::Relay),
            hill_gain_db: 10.0,
            suppress: 0,
            carry_copies: 0,
        }
    }
}

fn parse_args() -> Config {
    let mut c = Config::default();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        let v = args.get(i + 1).cloned().unwrap_or_default();
        match args[i].as_str() {
            "--nodes" => c.nodes = v.parse().expect("--nodes"),
            "--repeaters" => c.repeaters = v.parse().expect("--repeaters"),
            "--area-km" => c.area_km = v.parse().expect("--area-km"),
            "--messages" => c.messages = v.parse().expect("--messages"),
            "--channel-messages" => c.channel_messages = v.parse().expect("--channel-messages"),
            "--seed" => c.seed = v.parse().expect("--seed"),
            "--sf" => c.sf = v.parse().expect("--sf"),
            "--duty" => c.duty_permille = v.parse().expect("--duty (promille)"),
            "--hop-limit" => c.hop_limit = v.parse().expect("--hop-limit"),
            "--duration" => c.duration_s = v.parse().expect("--duration"),
            "--road-km" => c.road_km = v.parse().expect("--road-km"),
            "--car-kmh" => c.car_kmh = v.parse().expect("--car-kmh"),
            "--carry" => c.carry_copies = v.parse().expect("--carry (copies)"),
            "--suppress" => {
                c.suppress = match v.as_str() {
                    "normal" => 0,
                    "legacy" => 9,
                    "bridge" => 1,
                    "none" => 2,
                    "bridge-relay" => 3,
                    "bridge-weak" => 4,
                    "bridge-weak-relay" => 5,
                    "bridge-relay-late" => 6,
                    "bridge-edge" => 7,
                    "bridge-edge-relay" => 8,
                    _ => panic!("--suppress normal|bridge|none"),
                }
            }
            "--hill-gain" => c.hill_gain_db = v.parse().expect("--hill-gain"),
            "--hill-role" => {
                c.hill_role = match v.as_str() {
                    "client" => Some(Role::Client),
                    "relay" => Some(Role::Relay),
                    "repeater" => Some(Role::Repeater),
                    "none" => None,
                    _ => panic!("--hill-role client|relay|repeater|none"),
                }
            }
            "--forest-db-km" => c.forest_db_km = v.parse().expect("--forest-db-km"),
            "--walk-kmh" => c.walk_kmh = v.parse().expect("--walk-kmh"),
            "--relay-hops" => c.relay_hops = Some(v.parse().expect("--relay-hops")),
            "--role" => {
                c.group_role = match v.as_str() {
                    "client" => Role::Client,
                    "relay" => Role::Relay,
                    "repeater" => Role::Repeater,
                    _ => panic!("--role client|relay|repeater"),
                }
            }
            "--anchor-every" => c.anchor_every = v.parse().expect("--anchor-every"),
            "--repeater-gain" => c.repeater_gain_db = v.parse().expect("--repeater-gain (dB)"),
            "--repeater-km" => {
                c.repeater_km = v.split(',').map(|x| x.trim().parse().expect("--repeater-km")).collect();
                c.repeaters = c.repeater_km.len();
                c.highway = true;
            }
            "--trace-json" => trace::open(&v),
            "--hill" => {
                c.hill = true;
                c.forest = true;
                c.repeaters = 1;
                i += 1;
                continue;
            }
            "--forest" => {
                c.forest = true;
                c.repeaters = c.hill as usize;
                i += 1;
                continue;
            }
            "--base-start" => {
                c.base_start = true;
                i += 1;
                continue;
            }
            "--highway" => {
                c.highway = true;
                i += 1;
                continue;
            }
            "--mode" => {
                c.mode = match v.as_str() {
                    "maarse" => Mode::Maarse,
                    "flood" => Mode::Flood,
                    _ => panic!("--mode maarse|flood"),
                }
            }
            "--help" | "-h" => {
                println!(
                    "maarse-sim [--nodes N] [--repeaters R] [--area-km A] [--messages M]\n\
                     \x20        [--channel-messages K] [--seed S] [--sf 7-12] [--duty permille]\n\
                     \x20        [--mode maarse|flood] [--hop-limit H] [--duration seconds]\n\
                     \x20        [--highway [--road-km 100] [--car-kmh 110] [--base-start]]\n\
                     \x20        [--repeater-km 0,30,...] [--repeater-gain dB] [--anchor-every N]\n\
                     \x20        [--forest [--forest-db-km 10] [--walk-kmh 5] [--role client|relay|repeater] [--relay-hops H]]\n\
                     \x20        [--hill [--hill-role relay|client|repeater|none] [--hill-gain 10]]\n\
                     \x20        [--carry copies] [--suppress normal|legacy|bridge|bridge-relay|bridge-weak|bridge-weak-relay|none]   (flood suppression for relays in the forest)\n\
                     \x20        [--trace-json file|-]   (event trace for maarse-lab)"
                );
                std::process::exit(0);
            }
            other => panic!("unknown argument: {other}"),
        }
        i += 2;
    }
    assert!(c.repeaters <= c.nodes, "more repeaters than nodes");
    c
}

// ---------- radio ----------

/// Rough radio model: log-distance path loss (n = 3) at 868 MHz.
struct Radio {
    lora: LoraParams,
    tx_dbm: f64,
    noise_dbm: f64,
    /// Lowest SNR that can be received at this spreading factor.
    snr_min: f64,
    /// Extra attenuation (dB per metre), e.g. forest.
    extra_db_m: f64,
}

impl Radio {
    fn new(sf: u8, tx_dbm: f64) -> Self {
        // Thermal noise at 125 kHz + 6 dB noise figure.
        let noise_dbm = -174.0 + 10.0 * 125_000f64.log10() + 6.0;
        let snr_min = match sf {
            7 => -7.5,
            8 => -10.0,
            9 => -12.5,
            10 => -15.0,
            11 => -17.5,
            _ => -20.0,
        };
        Radio {
            lora: LoraParams::new(sf),
            tx_dbm,
            noise_dbm,
            snr_min,
            extra_db_m: 0.0,
        }
    }

    fn rx_dbm(&self, dist_m: f64, gain_db: f64) -> f64 {
        let d = dist_m.max(1.0);
        let path_loss = 31.2 + 30.0 * d.log10();
        self.tx_dbm + gain_db - path_loss - self.extra_db_m * d
    }

    fn snr(&self, rx_dbm: f64) -> f64 {
        rx_dbm - self.noise_dbm
    }
}

// ---------- simulator ----------

struct SimNode {
    router: Node,
    flood_seen: HashSet<u64>,
    pos: (f64, f64),
    repeater: bool,
    duty: DutyCycle,
    airtime_us: u64,
}

struct Tx {
    sender: usize,
    start: u64,
    end: u64,
    packet: Buf,
}

#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
enum Event {
    /// Try to send (CAD + duty cycle). `tries` counts backoffs.
    TryTx {
        node: usize,
        packet: Buf,
        tries: u8,
        id: u64,
    },
    Tick {
        node: usize,
    },
    TxEnd {
        tx: usize,
    },
    SendDirect {
        msg: usize,
    },
    SendChannel {
        from: usize,
        n: usize,
    },
    CheckAck {
        msg: usize,
        attempt: u8,
    },
    /// Move the car (motorway).
    Move,
}

struct Msg {
    from: usize,
    to: usize,
    payload: Vec<u8>,
    id: u64,
    sent_ms: u64,
    delivered_ms: Option<u64>,
    acked: bool,
}

#[derive(Default)]
struct Stats {
    tx_count: u64,
    collisions: u64,
    duty_drops: u64,
    channel_busy_drops: u64,
    routes_learned: u64,
    channel_sent: u64,
    channel_deliveries: u64,
    suppressed: u64,
    hop_retries: u64,
    by_type: std::collections::BTreeMap<String, u64>,
}

struct Sim {
    cfg: Config,
    radio: Radio,
    rng: SmallRng,
    nodes: Vec<SimNode>,
    gain: Vec<Vec<f64>>,
    txs: Vec<Tx>,
    queue: BinaryHeap<Reverse<(u64, u64, usize)>>,
    events: HashMap<usize, Event>,
    seq: u64,
    now: u64,
    msgs: Vec<Msg>,
    stats: Stats,
    public_channel: Address,
    /// Forest: each participant's next target.
    waypoints: Vec<(f64, f64)>,
    /// Bridges in `--suppress bridge-relay`: ignore duplicates from relays.
    bridges: Vec<bool>,
    cancelled: HashSet<(usize, u64)>,
    /// Reply ID -> (highest hop reached, path length, number of transmissions)
    resp_track: HashMap<u64, (u8, u8, u32)>,
    flood_pending: HashSet<(usize, u64)>,
    next_tick: Vec<u64>,
    /// Diagnostics of source-routed hops (see `trace_hop`).
    hop_trace: std::collections::BTreeMap<String, u32>,
}

impl Sim {
    fn new(cfg: Config) -> Self {
        let mut rng = SmallRng::seed_from_u64(cfg.seed);
        let mut radio = Radio::new(cfg.sf, cfg.tx_dbm);
        let _ = &mut radio; // the forest's attenuation is computed in Sim::link_rx
        let side = cfg.area_km * 1000.0;
        let public_channel = Address([0xc0; 16]);
        let mut nodes = Vec::new();
        for i in 0..cfg.nodes {
            let repeater = i < cfg.repeaters;
            let mut addr = [0u8; 16];
            rng.fill(&mut addr[..]);
            let role = if repeater && cfg.anchor_every > 0 && i % cfg.anchor_every == cfg.anchor_every - 1 {
                Role::Anchor
            } else if repeater {
                Role::Repeater
            } else if cfg.forest {
                cfg.group_role
            } else {
                Role::Client
            };
            let mut router = Node::new(Address(addr), role);
            let _ = router.subscribe(public_channel);
            if cfg.highway && i == cfg.nodes - 1 {
                router.mobile = true; // the car
            }
            router.carry_copies = cfg.carry_copies;
            if cfg.forest && !repeater {
                router.mobile = true;
                if let Some(h) = cfg.relay_hops {
                    router.relay_hops = h;
                }
            }
            nodes.push(SimNode {
                router,
                flood_seen: HashSet::new(),
                pos: if cfg.highway {
                    let road = cfg.road_km * 1000.0;
                    if repeater {
                        // Evenly spread along the road, a little off the carriageway.
                        match cfg.repeater_km.get(i) {
                            Some(km) => (km * 1000.0, 300.0),
                            None => (i as f64 * road / (cfg.repeaters.max(2) - 1) as f64, 300.0),
                        }
                    } else if i == cfg.nodes - 1 {
                        (0.0, 0.0) // the car starts at the beginning of the road
                    } else if i == cfg.repeaters {
                        // fixed node at the end of the road (or the start with --base-start)
                        (if cfg.base_start { 0.0 } else { road }, 1500.0)
                    } else {
                        (rng.gen_range(0.0..road), rng.gen_range(-3000.0..3000.0))
                    }
                } else {
                    (rng.gen_range(0.0..side), rng.gen_range(0.0..side))
                },
                repeater,
                duty: DutyCycle::new(cfg.duty_permille, 0),
                airtime_us: 0,
            });
        }
        // Link budget between all pairs (repeaters stand high and have better antennas).
        let n = nodes.len();
        let mut gain = vec![vec![f64::NEG_INFINITY; n]; n];
        for a in 0..n {
            for b in 0..n {
                if a == b {
                    continue;
                }
                let (pa, pb) = (nodes[a].pos, nodes[b].pos);
                let d = ((pa.0 - pb.0).powi(2) + (pa.1 - pb.1).powi(2)).sqrt();
                let g = cfg.repeater_gain_db
                    * (nodes[a].repeater as u8 + nodes[b].repeater as u8) as f64;
                gain[a][b] = radio.rx_dbm(d, g);
            }
        }
        Sim {
            cfg,
            radio,
            rng,
            nodes,
            gain,
            txs: Vec::new(),
            queue: BinaryHeap::new(),
            events: HashMap::new(),
            seq: 0,
            now: 0,
            msgs: Vec::new(),
            stats: Stats::default(),
            public_channel,
            waypoints: Vec::new(),
            bridges: Vec::new(),
            cancelled: HashSet::new(),
            resp_track: HashMap::new(),
            flood_pending: HashSet::new(),
            next_tick: vec![u64::MAX; n],
            hop_trace: Default::default(),
        }
    }

    fn schedule(&mut self, at: u64, ev: Event) {
        let key = self.seq as usize;
        self.seq += 1;
        self.events.insert(key, ev);
        self.queue.push(Reverse((at, self.seq, key)));
    }

    fn audible(&self, from: usize, to: usize) -> bool {
        self.radio.snr(self.gain[from][to]) >= self.radio.snr_min
    }

    fn setup_traffic(&mut self) {
        if self.cfg.highway {
            return self.setup_highway();
        }
        if self.cfg.hill {
            self.setup_hill();
        } else if self.cfg.forest {
            self.recompute_links();
        }
        let clients: Vec<usize> = (self.cfg.repeaters..self.cfg.nodes).collect();
        let hill = self.hill_nodes();
        assert!(clients.len() >= 2, "there must be at least two clients");
        let span = self.cfg.duration_s * 1000 * 2 / 3;
        for m in 0..self.cfg.messages {
            let (from, to) = if let Some((base, h)) = hill {
                // Base <-> a random man in the forest (not the one on the hill).
                let mut w = base;
                while w == base || (w == h && self.cfg.hill_role.is_some()) {
                    w = clients[self.rng.gen_range(0..clients.len())];
                }
                if m % 2 == 0 { (base, w) } else { (w, base) }
            } else {
                let from = clients[self.rng.gen_range(0..clients.len())];
                let mut to = from;
                while to == from {
                    to = clients[self.rng.gen_range(0..clients.len())];
                }
                (from, to)
            };
            let mut payload = format!("message {m} from {from} to {to}: ").into_bytes();
            payload.resize(60, b'.');
            let id = packet_id(PacketType::Data, &self.nodes[to].router.addr, &payload);
            self.msgs.push(Msg {
                from,
                to,
                payload,
                id,
                sent_ms: 0,
                delivered_ms: None,
                acked: false,
            });
            let at = self.rng.gen_range(1000..span.max(1001));
            self.schedule(at, Event::SendDirect { msg: m });
        }
        for n in 0..self.cfg.channel_messages {
            let from = clients[self.rng.gen_range(0..clients.len())];
            let at = self.rng.gen_range(1000..span.max(1001));
            self.schedule(at, Event::SendChannel { from, n });
        }
        if self.cfg.forest {
            self.schedule(5000, Event::Move);
        }
    }

    /// Index of the base and the man on the hill (only with `--hill`).
    fn hill_nodes(&self) -> Option<(usize, usize)> {
        self.cfg.hill.then(|| (self.cfg.repeaters, self.cfg.nodes - 1))
    }

    /// Is the man on the hill standing still (and not just walking around)?
    fn on_hill(&self, i: usize) -> bool {
        self.cfg.hill_role.is_some() && self.hill_nodes().is_some_and(|(_, h)| h == i)
    }

    /// Does the node walk around in the forest?
    fn walker(&self, i: usize) -> bool {
        if self.nodes[i].repeater || self.on_hill(i) {
            return false;
        }
        self.hill_nodes().is_none_or(|(base, _)| i != base)
    }

    /// Received power from a to b. The forest only attenuates when at least one of them
    /// stands in the forest; from the hill down into the forest it attenuates half as much.
    fn link_rx(&self, a: usize, b: usize) -> f64 {
        let (pa, pb) = (self.nodes[a].pos, self.nodes[b].pos);
        let d = ((pa.0 - pb.0).powi(2) + (pa.1 - pb.1).powi(2)).sqrt();
        let mut g = self.cfg.repeater_gain_db * (self.nodes[a].repeater as u8 + self.nodes[b].repeater as u8) as f64;
        let hill = self.on_hill(a) || self.on_hill(b);
        if hill {
            g += self.cfg.hill_gain_db;
        }
        let forest = match (self.walker(a), self.walker(b)) {
            (true, true) => 1.0,
            (true, false) | (false, true) if hill => 0.5,
            (true, false) | (false, true) => 1.0,
            (false, false) if self.cfg.hill => 0.0,
            (false, false) => 1.0,
        };
        self.radio.rx_dbm(d, g) - self.cfg.forest_db_km / 1000.0 * d * forest
    }

    /// The hill scenario: place repeater, base and the man on the hill.
    fn setup_hill(&mut self) {
        let side = self.cfg.area_km * 1000.0;
        let (base, hill) = self.hill_nodes().unwrap();
        self.nodes[0].pos = (side + 12_000.0, side / 2.0);
        self.nodes[base].pos = (side + 12_300.0, side / 2.0 + 200.0);
        self.nodes[base].router.mobile = false;
        self.nodes[base].router.role = Role::Client;
        if let Some(r) = self.cfg.hill_role {
            self.nodes[hill].pos = (side / 2.0, side / 2.0);
            self.nodes[hill].router.role = r;
            self.nodes[hill].router.mobile = false;
        }
        self.recompute_links();
    }

    fn recompute_links(&mut self) {
        let n = self.nodes.len();
        for a in 0..n {
            for b in a + 1..n {
                let rx = self.link_rx(a, b);
                self.gain[a][b] = rx;
                self.gain[b][a] = rx;
            }
        }
        self.apply_suppress();
    }

    /// Bridge mode (proposal): a relay that hears a repeater directly is
    /// exempt from flood suppression. The simulator uses the link budget as
    /// "heard"; on hardware it would be an announce with a role field.
    fn apply_suppress(&mut self) {
        let n = self.nodes.len();
        // Real behaviour: nodes learn their repeater neighbours from 0-hop
        // announces with the role field. The simulator does not send
        // announces, so it hands each node what such an announce would say.
        if self.cfg.suppress == 0 {
            let now = self.now;
            // What each node would put in its announce right now.
            let reports: Vec<(u16, Role, bool)> = (0..n)
                .map(|j| {
                    let rt = &self.nodes[j].router;
                    (rt.addr.path_id(), rt.role, rt.hears_repeater(now))
                })
                .collect();
            for i in 0..n {
                for j in 0..n {
                    if j == i || !(self.audible(j, i) && self.audible(i, j)) {
                        continue;
                    }
                    let (id, role, hears) = reports[j];
                    let snr = self.radio.snr(self.gain[j][i]).clamp(-30.0, 30.0) as i8;
                    let rt = &mut self.nodes[i].router;
                    rt.neighbors.heard(id, None, snr, 0, now);
                    rt.neighbors.set_role(id, role, Some(hears));
                }
            }
        }
        for i in 0..n {
            if self.nodes[i].router.role != Role::Relay {
                continue;
            }
            let hears_rep = (0..n).any(|r| r != i && self.nodes[r].repeater && self.audible(r, i) && self.audible(i, r));
            // Best SNR to a repeater; "weak" = below the router's limit for
            // weak copies (the edge of coverage).
            let best = (0..n)
                .filter(|&r| r != i && self.nodes[r].repeater && self.audible(r, i) && self.audible(i, r))
                .map(|r| self.radio.snr(self.gain[r][i]))
                .fold(f64::NEG_INFINITY, f64::max);
            // Edge: we hear a repeater, and at least one of our neighbours (not a
            // repeater) hears no repeater. Then we are needed as a bridge.
            let edge = hears_rep
                && (0..n).any(|j| {
                    j != i
                        && !self.nodes[j].repeater
                        && self.audible(j, i)
                        && self.audible(i, j)
                        && !(0..n).any(|r| self.nodes[r].repeater && self.audible(j, r) && self.audible(r, j))
                });
            let limit = std::env::var("BRIDGE_SNR").ok().and_then(|v| v.parse().ok()).unwrap_or(self.nodes[i].router.weak_snr_db as f64);
            let weak = hears_rep && best < limit;
            if self.bridges.len() != n {
                self.bridges = vec![false; n];
            }
            self.bridges[i] = (matches!(self.cfg.suppress, 3 | 6) && hears_rep)
                || (self.cfg.suppress == 5 && weak)
                || (self.cfg.suppress == 8 && edge);
            let late = std::env::var("BRIDGE_LATE_SLOTS").ok().and_then(|v| v.parse().ok()).unwrap_or(8u32);
            self.nodes[i].router.extra_flood_delay_ms =
                if self.cfg.suppress == 6 && hears_rep { late * maarse_core::router::DEFAULT_SLOT_MS } else { 0 };
            let quiet = match self.cfg.suppress {
                1 => hears_rep,
                4 => weak,
                7 => edge,
                2 => true,
                _ => false,
            };
            self.nodes[i].router.suppress_after = if quiet { u8::MAX } else { 1 };
        }
    }

    /// Forest: each participant walks towards a random target and picks a new one on
    /// arrival. Links to and from the participants are recomputed every 5 seconds.
    fn move_walkers(&mut self) {
        let side = self.cfg.area_km * 1000.0;
        let n = self.nodes.len();
        if self.waypoints.is_empty() {
            for _ in 0..n {
                let w = (self.rng.gen_range(0.0..side), self.rng.gen_range(0.0..side));
                self.waypoints.push(w);
            }
        }
        let step = self.cfg.walk_kmh / 3.6 * 5.0;
        for i in 0..n {
            if !self.walker(i) {
                continue;
            }
            let (p, w) = (self.nodes[i].pos, self.waypoints[i]);
            let (dx, dy) = (w.0 - p.0, w.1 - p.1);
            let d = (dx * dx + dy * dy).sqrt();
            if d <= step {
                self.nodes[i].pos = w;
                self.waypoints[i] = (self.rng.gen_range(0.0..side), self.rng.gen_range(0.0..side));
            } else {
                self.nodes[i].pos = (p.0 + dx / d * step, p.1 + dy / d * step);
            }
            trace::moved(self.now, i, self.nodes[i].pos);
        }
        self.recompute_links();
        self.schedule(self.now + 5000, Event::Move);
    }

    /// Motorway: the fixed node at the end of the road and the car write in turn.
    fn setup_highway(&mut self) {
        let car = self.cfg.nodes - 1;
        let base = self.cfg.repeaters;
        assert!(car > base, "the motorway needs at least two clients");
        let span = self.cfg.duration_s * 1000 * 9 / 10;
        let n = self.cfg.messages.max(1) as u64;
        for m in 0..self.cfg.messages {
            let (from, to) = if m % 2 == 0 { (base, car) } else { (car, base) };
            let mut payload = format!("message {m} from {from} to {to}: ").into_bytes();
            payload.resize(60, b'.');
            let id = packet_id(PacketType::Data, &self.nodes[to].router.addr, &payload);
            self.msgs.push(Msg {
                from,
                to,
                payload,
                id,
                sent_ms: 0,
                delivered_ms: None,
                acked: false,
            });
            self.schedule(5000 + m as u64 * span / n, Event::SendDirect { msg: m });
        }
        self.schedule(2000, Event::Move);
    }

    fn move_car(&mut self) {
        let car = self.cfg.nodes - 1;
        let road = self.cfg.road_km * 1000.0;
        let step = self.cfg.car_kmh / 3.6 * 2.0;
        self.nodes[car].pos.0 = (self.nodes[car].pos.0 + step).min(road);
        trace::moved(self.now, car, self.nodes[car].pos);
        for o in 0..self.nodes.len() {
            if o == car {
                continue;
            }
            let (pa, pb) = (self.nodes[car].pos, self.nodes[o].pos);
            let d = ((pa.0 - pb.0).powi(2) + (pa.1 - pb.1).powi(2)).sqrt();
            let g = self.cfg.repeater_gain_db * self.nodes[o].repeater as u8 as f64;
            let rx = self.radio.rx_dbm(d, g);
            self.gain[car][o] = rx;
            self.gain[o][car] = rx;
        }
        if self.cfg.carry_copies > 0 {
            // The simulator sends no announces; give the routers what they
            // would learn from them (neighbours and their roles).
            self.apply_suppress();
        }
        self.schedule(self.now + 2000, Event::Move);
    }

    fn token(&mut self) -> [u8; 16] {
        let mut t = [0u8; 16];
        self.rng.fill(&mut t[..]);
        t
    }

    fn run(&mut self) {
        self.setup_traffic();
        self.trace_setup();
        let end = self.cfg.duration_s * 1000;
        while let Some(Reverse((at, _, key))) = self.queue.pop() {
            if at > end {
                break;
            }
            self.now = at;
            let Some(ev) = self.events.remove(&key) else {
                continue;
            };
            self.handle(ev);
        }
    }

    fn handle(&mut self, ev: Event) {
        match ev {
            Event::SendDirect { msg } => self.send_direct(msg, 0),
            Event::Move if self.cfg.forest => self.move_walkers(),
            Event::Move => self.move_car(),
            Event::CheckAck { msg, attempt } => {
                let m = &self.msgs[msg];
                // In Må:rse mode the router itself handles retransmission and
                // route repair; the simulator does not interfere.
                if m.acked || attempt >= 2 || self.cfg.mode == Mode::Maarse {
                    return;
                }
                // No acknowledgement: forget the route and try again (the route is repaired).
                let (from, to) = (m.from, m.to);
                let dest = self.nodes[to].router.addr;
                if self.cfg.mode == Mode::Maarse {
                    self.nodes[from].router.forget_route(&dest);
                }
                self.send_direct(msg, attempt + 1);
            }
            Event::SendChannel { from, n } => {
                let mut payload = format!("channel message {n} from {from}").into_bytes();
                payload.resize(40, b'.');
                self.stats.channel_sent += 1;
                let ch = self.public_channel;
                match self.cfg.mode {
                    Mode::Maarse => {
                        if let Ok(actions) = self.nodes[from].router.send_channel(ch, &payload) {
                            self.apply(from, actions.into_iter().collect());
                        }
                    }
                    Mode::Flood => self.flood_originate(from, PacketType::Channel, ch, &payload),
                }
            }
            Event::TryTx {
                node,
                packet,
                tries,
                id,
            } => self.try_tx(node, packet, tries, id),
            Event::Tick { node } => {
                self.next_tick[node] = u64::MAX;
                let actions = self.nodes[node].router.tick(self.now);
                self.stats.hop_retries += actions.len() as u64;
                self.apply(node, actions.into_iter().collect());
            }
            Event::TxEnd { tx } => self.tx_end(tx),
        }
    }

    fn send_direct(&mut self, msg: usize, attempt: u8) {
        let (from, to) = (self.msgs[msg].from, self.msgs[msg].to);
        trace::send(self.now, msg, from, to);
        let payload = self.msgs[msg].payload.clone();
        let dest = self.nodes[to].router.addr;
        if attempt == 0 {
            self.msgs[msg].sent_ms = self.now;
        }
        match self.cfg.mode {
            Mode::Maarse => {
                let token = self.token();
                let actions = if attempt == 0 {
                    match self.nodes[from]
                        .router
                        .send_direct(dest, &payload, token, self.now)
                    {
                        Ok((_, a)) => a.into_iter().collect(),
                        Err(_) => Vec::new(),
                    }
                } else {
                    // The message is still in the queue if the route was never found;
                    // otherwise it is sent anew.
                    let mut a: Vec<Action> = self.nodes[from]
                        .router
                        .retry_path(dest, token, self.now)
                        .into_iter()
                        .collect();
                    let token2 = self.token();
                    if let Ok((_, b)) = self.nodes[from]
                        .router
                        .send_direct(dest, &payload, token2, self.now)
                    {
                        a.extend(b);
                    }
                    a
                };
                self.apply(from, actions);
            }
            Mode::Flood => self.flood_originate(from, PacketType::Data, dest, &payload),
        }
        self.schedule(self.now + 60_000, Event::CheckAck { msg, attempt });
    }

    /// Carries out a router's actions.
    fn apply(&mut self, node: usize, actions: Vec<Action>) {
        for a in actions {
            match a {
                Action::Transmit {
                    packet,
                    delay_ms,
                    id,
                } => {
                    let slot = self.nodes[node].router.slot_ms.max(1) as u64;
                    let jitter = self.rng.gen_range(0..slot);
                    self.schedule(
                        self.now + delay_ms as u64 + jitter,
                        Event::TryTx {
                            node,
                            packet,
                            tries: 0,
                            id,
                        },
                    );
                }
                Action::Cancel { id } => {
                    self.cancelled.insert((node, id));
                }
                Action::Deliver {
                    ptype,
                    dest,
                    payload,
                    ..
                } => self.on_deliver(node, ptype, dest, &payload),
                Action::RouteLearned { .. } => self.stats.routes_learned += 1,
                Action::Acked { msg_id } | Action::CarryAcked { msg_id } => {
                    if let Some((i, m)) = self
                        .msgs
                        .iter_mut()
                        .enumerate()
                        .find(|(_, m)| m.id == msg_id && m.from == node)
                    {
                        if !m.acked {
                            trace::ack(self.now, i);
                        }
                        m.acked = true;
                    }
                }
            }
        }
        // Schedule retransmission of hops that were not heard being forwarded.
        if let Some(d) = self.nodes[node].router.next_deadline()
            && d < self.next_tick[node]
        {
            self.next_tick[node] = d;
            self.schedule(d.max(self.now), Event::Tick { node });
        }
    }

    fn on_deliver(&mut self, node: usize, ptype: PacketType, dest: Address, payload: &[u8]) {
        match ptype {
            PacketType::Data => {
                let id = packet_id(PacketType::Data, &dest, payload);
                if let Some((i, m)) = self
                    .msgs
                    .iter_mut()
                    .enumerate()
                    .find(|(_, m)| m.id == id && m.to == node)
                    && m.delivered_ms.is_none()
                {
                    m.delivered_ms = Some(self.now);
                    trace::deliver(self.now, i, node);
                }
            }
            PacketType::Channel if dest == self.public_channel => {
                self.stats.channel_deliveries += 1
            }
            _ => {}
        }
    }

    fn channel_busy(&self, node: usize) -> bool {
        self.txs
            .iter()
            .rev()
            .take_while(|t| t.end + 20_000 > self.now)
            .any(|t| {
                t.start <= self.now
                    && self.now < t.end
                    && (t.sender == node || self.audible(t.sender, node))
            })
    }

    fn try_tx(&mut self, node: usize, packet: Buf, tries: u8, id: u64) {
        if self.cancelled.remove(&(node, id)) {
            self.stats.suppressed += 1;
            return;
        }
        self.flood_pending.remove(&(node, id));
        if self.channel_busy(node) {
            trace::defer(self.now, node, "busy");
            if tries >= 8 {
                self.stats.channel_busy_drops += 1;
                return;
            }
            let backoff = self.rng.gen_range(100..600);
            self.schedule(
                self.now + backoff,
                Event::TryTx {
                    node,
                    packet,
                    tries: tries + 1,
                    id,
                },
            );
            return;
        }
        let airtime = self.radio.lora.airtime_us(packet.len());
        if !self.nodes[node].duty.try_consume(self.now, airtime) {
            let wait = self.nodes[node].duty.wait_ms(self.now, airtime);
            trace::defer(self.now, node, "duty");
            if wait > 120_000 || tries >= 8 {
                self.stats.duty_drops += 1;
                return;
            }
            self.schedule(
                self.now + wait,
                Event::TryTx {
                    node,
                    packet,
                    tries: tries + 1,
                    id,
                },
            );
            return;
        }
        self.nodes[node].airtime_us += airtime as u64;
        self.stats.tx_count += 1;
        if let Ok(p) = Packet::decode(&packet) {
            *self
                .stats
                .by_type
                .entry(format!("{:?}", p.header.ptype))
                .or_default() += 1;
            if matches!(p.header.ptype, PacketType::PathResponse | PacketType::Ack) {
                let id = packet_id(p.header.ptype, &p.header.dest, p.payload);
                let len = p.header.path.as_ref().map_or(0, |x| x.len()) as u8;
                let e = self.resp_track.entry(id).or_insert((0, len, 0));
                e.0 = e.0.max(p.header.hops);
                e.2 += 1;
            }
            if p.header.ptype == PacketType::PathResponse && p.header.hops == 0 {
                *self.stats.by_type.entry("PathResponse-start".into()).or_default() += 1;
            }
            if p.header.ptype == PacketType::PathResponse
                && p.header.path.as_ref().is_some_and(|x| x.len() == p.header.hops as usize)
            {
                *self.stats.by_type.entry("PathResponse-last-hop".into()).or_default() += 1;
            }
        }
        let end = self.now + (airtime as u64).div_ceil(1000);
        self.txs.push(Tx {
            sender: node,
            start: self.now,
            end,
            packet,
        });
        let id = self.txs.len() - 1;
        if trace::on() {
            let p = Packet::decode(&self.txs[id].packet).map_or("?".into(), |p| format!("{:?}", p.header.ptype));
            trace::tx(self.now, node, id, end, airtime, &p);
        }
        self.schedule(end, Event::TxEnd { tx: id });
    }

    fn tx_end(&mut self, tx: usize) {
        let (sender, start, end) = (self.txs[tx].sender, self.txs[tx].start, self.txs[tx].end);
        let packet = self.txs[tx].packet.clone();
        // Other transmissions that overlap in time.
        let overlapping: Vec<usize> = self
            .txs
            .iter()
            .enumerate()
            .filter(|(i, o)| *i != tx && o.start < end && o.end > start && o.end + 20_000 > start)
            .map(|(_, o)| o.sender)
            .collect();
        if std::env::var("DEBUG_ACK").is_ok() {
            self.trace_hop(tx, &overlapping);
        }
        let mut rx = trace::Rx::default();
        for r in 0..self.nodes.len() {
            if r == sender || !self.audible(sender, r) {
                continue;
            }
            let p = self.gain[sender][r];
            // Half-duplex: the receiver was sending itself. Collision: another transmission
            // is audible and not at least 6 dB weaker (capture effect).
            let lost = overlapping.iter().any(|&o| {
                o == r
                    || (o != sender
                        && self.radio.snr(self.gain[o][r]) >= self.radio.snr_min - 6.0
                        && self.gain[o][r] > p - 6.0)
            });
            if lost && std::env::var("NO_COLL").is_err() {
                self.stats.collisions += 1;
                rx.coll(r);
                continue;
            }
            rx.ok(r);
            let mut snr = self.radio.snr(p).clamp(-30.0, 30.0) as i8;
            // Bridge rule (proposal): a duplicate from a relay does not count in
            // the bridge's flood suppression; a duplicate from a repeater does.
            if self.bridges.get(r).copied().unwrap_or(false)
                && self.nodes[sender].router.role == Role::Relay
                && self.nodes[r].router.seen(&packet)
            {
                snr = -30;
            }
            match self.cfg.mode {
                Mode::Maarse => {
                    let actions = self.nodes[r].router.on_receive(&packet, snr, self.now);
                    self.apply(r, actions.into_iter().collect());
                }
                Mode::Flood => self.flood_receive(r, &packet, snr),
            }
        }
        rx.emit(self.now, tx);
    }

    /// Initial picture for the event trace: positions and audible links.
    fn trace_setup(&self) {
        if !trace::on() {
            return;
        }
        let c = &self.cfg;
        let nodes: Vec<(f64, f64, &str)> = self
            .nodes
            .iter()
            .map(|n| {
                let r = if n.router.mobile { "car" } else if n.repeater { "r" } else { "c" };
                (n.pos.0, n.pos.1, r)
            })
            .collect();
        let mut links = Vec::new();
        for a in 0..self.nodes.len() {
            for b in a + 1..self.nodes.len() {
                if self.audible(a, b) || self.audible(b, a) {
                    links.push((a, b, self.radio.snr(self.gain[a][b])));
                }
            }
        }
        let size = if c.highway {
            (c.road_km * 1000.0, 6000.0)
        } else {
            (c.area_km * 1000.0, c.area_km * 1000.0)
        };
        let mode = if c.mode == Mode::Maarse { "maarse" } else { "flood" };
        trace::setup(mode, c.highway, size, c.duration_s * 1000, &nodes, &links);
    }

    /// Diagnostics (DEBUG_ACK): for a source-routed hop, did the next repeater
    /// on the path hear the packet, and if not, what drowned it? Split by
    /// the link's SNR margin above the sensitivity limit.
    fn trace_hop(&mut self, tx: usize, overlapping: &[usize]) {
        let (sender, start, end) = (self.txs[tx].sender, self.txs[tx].start, self.txs[tx].end);
        let Ok(p) = Packet::decode(&self.txs[tx].packet) else { return };
        let Some(path) = p.header.path.as_ref() else { return };
        let Some(&next) = path.get(p.header.hops as usize) else { return };
        let Some(r) = self.nodes.iter().position(|n| n.router.id == next) else { return };
        let p_rx = self.gain[sender][r];
        let why = if !self.audible(sender, r) {
            "inaudible".to_string()
        } else if overlapping.contains(&r) {
            "half-duplex".into()
        } else if let Some(o) = self.txs.iter().enumerate().find(|(i, o)| {
            *i != tx
                && o.start < end
                && o.end > start
                && o.sender != sender
                && self.radio.snr(self.gain[o.sender][r]) >= self.radio.snr_min - 6.0
                && self.gain[o.sender][r] > p_rx - 6.0
        }) {
            let t = Packet::decode(&o.1.packet).map(|q| format!("{:?}", q.header.ptype));
            format!("collision with {}", t.unwrap_or_default())
        } else {
            "ok".into()
        };
        let margin = ((self.radio.snr(p_rx) - self.radio.snr_min) / 5.0).floor() as i32 * 5;
        let key = format!("{:?}, margin {margin:>2}-{} dB: {why}", p.header.ptype, margin + 5);
        *self.hop_trace.entry(key).or_default() += 1;
    }

    // ---------- flood mode (comparison, Meshtastic-like) ----------

    fn flood_originate(&mut self, from: usize, ptype: PacketType, dest: Address, payload: &[u8]) {
        let header = Header::new(ptype, dest);
        if let Ok(packet) = encode(&header, payload) {
            let id = packet_id(ptype, &dest, payload);
            self.nodes[from].flood_seen.insert(id);
            self.schedule(
                self.now,
                Event::TryTx {
                    node: from,
                    packet,
                    tries: 0,
                    id,
                },
            );
        }
    }

    fn flood_receive(&mut self, r: usize, buf: &[u8], snr: i8) {
        let Ok(pkt) = Packet::decode(buf) else { return };
        let h = &pkt.header;
        let id = packet_id(h.ptype, &h.dest, pkt.payload);
        if !self.nodes[r].flood_seen.insert(id) {
            // Like Meshtastic: if we hear someone else forward it, we drop ours.
            if self.flood_pending.remove(&(r, id)) {
                self.cancelled.insert((r, id));
            }
            return;
        }
        let me = self.nodes[r].router.addr;
        if h.ptype == PacketType::Data && h.dest == me {
            self.on_deliver(r, PacketType::Data, h.dest, pkt.payload);
            // The acknowledgement is flooded back.
            let ack = ack_token(id);
            self.flood_originate(r, PacketType::Ack, ack, &[]);
            return;
        }
        if h.ptype == PacketType::Ack
            && let Some((i, m)) = self
                .msgs
                .iter_mut()
                .enumerate()
                .find(|(_, m)| ack_token(m.id) == h.dest && m.from == r)
        {
            if !m.acked {
                trace::ack(self.now, i);
            }
            m.acked = true;
            return;
        }
        if h.ptype == PacketType::Channel {
            self.on_deliver(r, PacketType::Channel, h.dest, pkt.payload);
        }
        if h.hops + 1 >= self.cfg.hop_limit {
            return;
        }
        let mut header = h.clone();
        header.hops += 1;
        if let Ok(packet) = encode(&header, pkt.payload) {
            let slot = maarse_core::router::DEFAULT_SLOT_MS as u64;
            let bucket = ((snr as i64).clamp(-20, 19) + 20) as u64 / 5;
            let delay = bucket * slot + self.rng.gen_range(0..slot);
            self.flood_pending.insert((r, id));
            self.schedule(
                self.now + delay,
                Event::TryTx {
                    node: r,
                    packet,
                    tries: 0,
                    id,
                },
            );
        }
    }

    // ---------- report ----------

    fn report(&self) {
        let c = &self.cfg;
        let n = self.msgs.len().max(1) as f64;
        let delivered: Vec<&Msg> = self
            .msgs
            .iter()
            .filter(|m| m.delivered_ms.is_some())
            .collect();
        let acked = self.msgs.iter().filter(|m| m.acked).count();
        let mut lat: Vec<u64> = delivered
            .iter()
            .map(|m| m.delivered_ms.unwrap() - m.sent_ms)
            .collect();
        lat.sort_unstable();
        let median = lat.get(lat.len() / 2).copied().unwrap_or(0);
        let p90 = lat.get(lat.len() * 9 / 10).copied().unwrap_or(0);
        let total_air: u64 = self.nodes.iter().map(|x| x.airtime_us).sum();
        let budget_us = c.duty_permille as u64 * c.duration_s * 1000;
        let busiest = self.nodes.iter().map(|x| x.airtime_us).max().unwrap_or(0);
        let neighbours: f64 = (0..self.nodes.len())
            .map(|a| {
                (0..self.nodes.len())
                    .filter(|&b| a != b && self.audible(a, b))
                    .count() as f64
            })
            .sum::<f64>()
            / self.nodes.len() as f64;
        let receivers = (c.nodes - 1) as u64 * self.stats.channel_sent.max(1);

        println!("Må:rse simulator — mode: {:?}", c.mode);
        println!(
            "  {} nodes ({} repeaters) on {:.0}×{:.0} km, SF{}, duty cycle {:.1} %, {} s, seed {}",
            c.nodes,
            c.repeaters,
            c.area_km,
            c.area_km,
            c.sf,
            c.duty_permille as f64 / 10.0,
            c.duration_s,
            c.seed
        );
        println!("  Average number of neighbours per node: {neighbours:.1}");
        println!();
        println!("Direct messages ({}):", self.msgs.len());
        if c.highway {
            let car = c.nodes - 1;
            let to_car = self.msgs.iter().filter(|m| m.to == car);
            let (n1, d1) = to_car.fold((0, 0), |(n, d), m| {
                (n + 1, d + m.delivered_ms.is_some() as usize)
            });
            let from_car = self.msgs.iter().filter(|m| m.from == car);
            let (n2, d2) = from_car.fold((0, 0), |(n, d), m| {
                (n + 1, d + m.delivered_ms.is_some() as usize)
            });
            println!("  To the car: {d1}/{n1} delivered, from the car: {d2}/{n2} delivered");
        }
        println!(
            "  Delivered:      {:>5.1} %",
            delivered.len() as f64 / n * 100.0
        );
        println!("  Acknowledged:   {:>5.1} %", acked as f64 / n * 100.0);
        println!(
            "  Latency:        median {:.1} s, 90 % below {:.1} s",
            median as f64 / 1000.0,
            p90 as f64 / 1000.0
        );
        if c.mode == Mode::Maarse {
            println!("  Routes found:   {}", self.stats.routes_learned);
        }
        if c.carry_copies > 0 {
            println!(
                "  Carried:        {} delivered by carrying, {} handoffs, {} delivered notices ({} copies)",
                self.nodes.iter().map(|n| n.router.stats.carry_delivered).sum::<u32>(),
                self.nodes.iter().map(|n| n.router.stats.carry_handoffs).sum::<u32>(),
                self.nodes.iter().map(|n| n.router.stats.carry_notices).sum::<u32>(),
                c.carry_copies
            );
        }
        println!();
        println!("Channel messages ({}):", self.stats.channel_sent);
        println!(
            "  Reached on avg  {:>5.1} % of the other nodes",
            self.stats.channel_deliveries as f64 / receivers as f64 * 100.0
        );
        println!();
        if std::env::var("DEBUG_ACK").is_ok() {
            let mut hist: std::collections::BTreeMap<String, u32> = Default::default();
            for (maxh, len, tx) in self.resp_track.values() {
                let k = if maxh == len { "reached the last hop".to_string() } else if *tx > 6 { format!("stopped at {maxh}/{len} after >6 transmissions") } else { format!("stopped at {maxh}/{len} after {tx} transmissions") };
                *hist.entry(k).or_default() += 1;
            }
            for (k, v) in hist.iter().take(12) {
                println!("  [debug] reply {k}: {v}");
            }
            let acked: u32 = self.nodes.iter().map(|n| n.router.stats.acked).sum();
            let rl: u32 = self
                .nodes
                .iter()
                .map(|n| n.router.stats.routes_learned)
                .sum();
            let reach = self.msgs.iter().filter(|m| {
                let mut seen = vec![false; self.nodes.len()];
                let mut st = vec![m.from];
                seen[m.from] = true;
                while let Some(x) = st.pop() {
                    if x == m.to { return true; }
                    if x != m.from && !self.nodes[x].repeater { continue; }
                    for (y, sy) in seen.iter_mut().enumerate() {
                        if !*sy && self.audible(x, y) && self.audible(y, x) {
                            *sy = true;
                            st.push(y);
                        }
                    }
                }
                false
            }).count();
            for (k, v) in &self.hop_trace {
                println!("  [debug] hop {k}: {v}");
            }
            println!("  [debug] reachable messages (graph): {reach}/{}", self.msgs.len());
            let pr: u32 = self.nodes.iter().map(|n| n.router.stats.path_requests).sum();
            let hr: u32 = self.nodes.iter().map(|n| n.router.stats.hop_retries).sum();
            println!("  [debug] path requests started: {pr}, hops retransmitted: {hr}");
            println!("  [debug] Acked actions in total: {acked}, routes learned: {rl}");
            for m in self
                .msgs
                .iter()
                .filter(|m| m.delivered_ms.is_some() && !m.acked)
                .take(5)
            {
                println!(
                    "  [debug] without acknowledgement: {} -> {} sent {} delivered {:?}",
                    m.from, m.to, m.sent_ms, m.delivered_ms
                );
            }
        }
        if std::env::var("DEBUG_CARRY").is_ok() {
            for m in self.msgs.iter().filter(|m| m.delivered_ms.is_none()) {
                println!("  [carry] lost {} -> {} sent {:.0} s", m.from, m.to, m.sent_ms as f64 / 1000.0);
            }
            for (i, n) in self.nodes.iter().enumerate() {
                if n.router.carried_count() > 0 || n.router.stats.carry_handoffs > 0 {
                    println!(
                        "  [carry] node {i}: carries {}, handoffs {}, x {:.0} m",
                        n.router.carried_count(),
                        n.router.stats.carry_handoffs,
                        n.pos.0
                    );
                }
            }
        }
        println!("Radio:");
        println!("  Transmissions:  {}", self.stats.tx_count);
        println!("  Total airtime:  {:.1} s", total_air as f64 / 1e6);
        println!(
            "  Busiest node:   {:.1} s = {:.0} % of its budget",
            busiest as f64 / 1e6,
            busiest as f64 / budget_us.max(1) as f64 * 100.0
        );
        println!(
            "  Lost to collisions (per receiver): {}",
            self.stats.collisions
        );
        println!(
            "  Dropped due to duty cycle: {}, due to busy channel: {}",
            self.stats.duty_drops, self.stats.channel_busy_drops
        );
        println!(
            "  Forwards cancelled (area covered): {}",
            self.stats.suppressed
        );
        if c.mode == Mode::Maarse {
            println!("  Retransmitted hops: {}", self.stats.hop_retries);
        }
        let types: Vec<String> = self
            .stats
            .by_type
            .iter()
            .map(|(k, v)| format!("{k} {v}"))
            .collect();
        println!("  Transmissions per type: {}", types.join(", "));
    }
}

fn main() {
    let cfg = parse_args();
    let mut sim = Sim::new(cfg);
    sim.run();
    trace::finish();
    sim.report();
}
