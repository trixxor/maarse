//! maarse-sim: simulator for Må:rse-netværk.
//!
//! Placerer repeatere og klienter i et område, simulerer LoRa-radio med
//! rækkevidde, kollisioner, half-duplex, listen-before-talk og duty cycle,
//! og kører `maarse_core::Router` uændret i hver node.
//!
//! Til sammenligning kan den også køre "flood"-tilstand, hvor alle noder
//! (også klienter) gentager alt med en hop-grænse, ligesom Meshtastic.
//!
//! Eksempel:
//!   cargo run --release -p maarse-sim -- --nodes 120 --repeaters 30 --messages 60
//!   cargo run --release -p maarse-sim -- --mode flood --hop-limit 7
#![forbid(unsafe_code)]

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

// ---------- konfiguration ----------

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
    /// Motorvej: repeatere på en linje, en bil kører forbi.
    highway: bool,
    road_km: f64,
    car_kmh: f64,
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
                     \x20        [--channel-messages K] [--seed S] [--sf 7-12] [--duty promille]\n\
                     \x20        [--mode maarse|flood] [--hop-limit H] [--duration sekunder]\n\
                     \x20        [--highway [--road-km 100] [--car-kmh 110]]"
                );
                std::process::exit(0);
            }
            other => panic!("ukendt argument: {other}"),
        }
        i += 2;
    }
    assert!(c.repeaters <= c.nodes, "flere repeatere end noder");
    c
}

// ---------- radio ----------

/// Grov radiomodel: log-distance path loss (n = 3) ved 868 MHz.
struct Radio {
    lora: LoraParams,
    tx_dbm: f64,
    noise_dbm: f64,
    /// Mindste SNR, der kan modtages ved denne spreading factor.
    snr_min: f64,
}

impl Radio {
    fn new(sf: u8, tx_dbm: f64) -> Self {
        // Termisk støj ved 125 kHz + 6 dB støjtal.
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
        }
    }

    fn rx_dbm(&self, dist_m: f64, gain_db: f64) -> f64 {
        let d = dist_m.max(1.0);
        let path_loss = 31.2 + 30.0 * d.log10();
        self.tx_dbm + gain_db - path_loss
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
    /// Forsøg at sende (CAD + duty cycle). `tries` tæller backoffs.
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
    /// Flyt bilen (motorvej).
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
    cancelled: HashSet<(usize, u64)>,
    /// Svar-ID -> (højeste hop nået, stiens længde, antal udsendelser)
    resp_track: HashMap<u64, (u8, u8, u32)>,
    flood_pending: HashSet<(usize, u64)>,
    next_tick: Vec<u64>,
    /// Diagnose af kildestyrede hop (se `trace_hop`).
    hop_trace: std::collections::BTreeMap<String, u32>,
}

impl Sim {
    fn new(cfg: Config) -> Self {
        let mut rng = SmallRng::seed_from_u64(cfg.seed);
        let radio = Radio::new(cfg.sf, cfg.tx_dbm);
        let side = cfg.area_km * 1000.0;
        let public_channel = Address([0xc0; 16]);
        let mut nodes = Vec::new();
        for i in 0..cfg.nodes {
            let repeater = i < cfg.repeaters;
            let mut addr = [0u8; 16];
            rng.fill(&mut addr[..]);
            let role = if repeater {
                Role::Repeater
            } else {
                Role::Client
            };
            let mut router = Node::new(Address(addr), role);
            let _ = router.subscribe(public_channel);
            if cfg.highway && i == cfg.nodes - 1 {
                router.mobile = true; // bilen
            }
            nodes.push(SimNode {
                router,
                flood_seen: HashSet::new(),
                pos: if cfg.highway {
                    let road = cfg.road_km * 1000.0;
                    if repeater {
                        // Jævnt fordelt langs vejen, lidt fra kørebanen.
                        (i as f64 * road / (cfg.repeaters.max(2) - 1) as f64, 300.0)
                    } else if i == cfg.nodes - 1 {
                        (0.0, 0.0) // bilen starter ved vejens begyndelse
                    } else if i == cfg.repeaters {
                        (road, 1500.0) // fast node i vejens ende
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
        // Linkbudget mellem alle par (repeatere står højt og har bedre antenner).
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
        let clients: Vec<usize> = (self.cfg.repeaters..self.cfg.nodes).collect();
        assert!(clients.len() >= 2, "der skal være mindst to klienter");
        let span = self.cfg.duration_s * 1000 * 2 / 3;
        for m in 0..self.cfg.messages {
            let from = clients[self.rng.gen_range(0..clients.len())];
            let mut to = from;
            while to == from {
                to = clients[self.rng.gen_range(0..clients.len())];
            }
            let mut payload = format!("besked {m} fra {from} til {to}: ").into_bytes();
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
    }

    /// Motorvej: den faste node i vejens ende og bilen skriver skiftevis.
    fn setup_highway(&mut self) {
        let car = self.cfg.nodes - 1;
        let base = self.cfg.repeaters;
        assert!(car > base, "motorvej kræver mindst to klienter");
        let span = self.cfg.duration_s * 1000 * 9 / 10;
        let n = self.cfg.messages.max(1) as u64;
        for m in 0..self.cfg.messages {
            let (from, to) = if m % 2 == 0 { (base, car) } else { (car, base) };
            let mut payload = format!("besked {m} fra {from} til {to}: ").into_bytes();
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
        self.schedule(self.now + 2000, Event::Move);
    }

    fn token(&mut self) -> [u8; 16] {
        let mut t = [0u8; 16];
        self.rng.fill(&mut t[..]);
        t
    }

    fn run(&mut self) {
        self.setup_traffic();
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
            Event::Move => self.move_car(),
            Event::CheckAck { msg, attempt } => {
                let m = &self.msgs[msg];
                // I Må:rse-tilstand står routeren selv for gensendelse og
                // reparation af ruten; simulatoren blander sig ikke.
                if m.acked || attempt >= 2 || self.cfg.mode == Mode::Maarse {
                    return;
                }
                // Ingen kvittering: glem ruten og prøv igen (ruten repareres).
                let (from, to) = (m.from, m.to);
                let dest = self.nodes[to].router.addr;
                if self.cfg.mode == Mode::Maarse {
                    self.nodes[from].router.forget_route(&dest);
                }
                self.send_direct(msg, attempt + 1);
            }
            Event::SendChannel { from, n } => {
                let mut payload = format!("kanalbesked {n} fra {from}").into_bytes();
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
                    // Beskeden ligger stadig i køen, hvis ruten aldrig blev fundet;
                    // ellers sendes den på ny.
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

    /// Udfører en routers handlinger.
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
                Action::Acked { msg_id } => {
                    if let Some(m) = self
                        .msgs
                        .iter_mut()
                        .find(|m| m.id == msg_id && m.from == node)
                    {
                        m.acked = true;
                    }
                }
            }
        }
        // Planlæg gensendelse af hop, der ikke blev hørt sendt videre.
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
                if let Some(m) = self.msgs.iter_mut().find(|m| m.id == id && m.to == node)
                    && m.delivered_ms.is_none()
                {
                    m.delivered_ms = Some(self.now);
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
                *self.stats.by_type.entry("PathResponse-sidste-hop".into()).or_default() += 1;
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
        self.schedule(end, Event::TxEnd { tx: id });
    }

    fn tx_end(&mut self, tx: usize) {
        let (sender, start, end) = (self.txs[tx].sender, self.txs[tx].start, self.txs[tx].end);
        let packet = self.txs[tx].packet.clone();
        // Andre sendinger, der overlapper i tid.
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
        for r in 0..self.nodes.len() {
            if r == sender || !self.audible(sender, r) {
                continue;
            }
            let p = self.gain[sender][r];
            // Half-duplex: modtageren sendte selv. Kollision: en anden sending
            // er hørbar og ikke mindst 6 dB svagere (capture effect).
            let lost = overlapping.iter().any(|&o| {
                o == r
                    || (o != sender
                        && self.radio.snr(self.gain[o][r]) >= self.radio.snr_min - 6.0
                        && self.gain[o][r] > p - 6.0)
            });
            if lost && std::env::var("NO_COLL").is_err() {
                self.stats.collisions += 1;
                continue;
            }
            let snr = self.radio.snr(p).clamp(-30.0, 30.0) as i8;
            match self.cfg.mode {
                Mode::Maarse => {
                    let actions = self.nodes[r].router.on_receive(&packet, snr, self.now);
                    self.apply(r, actions.into_iter().collect());
                }
                Mode::Flood => self.flood_receive(r, &packet, snr),
            }
        }
    }

    /// Diagnose (DEBUG_ACK): for et kildestyret hop, hørte næste repeater
    /// på stien pakken, og hvis ikke, hvad overdøvede den? Opdelt efter
    /// linkets SNR-margen over følsomhedsgrænsen.
    fn trace_hop(&mut self, tx: usize, overlapping: &[usize]) {
        let (sender, start, end) = (self.txs[tx].sender, self.txs[tx].start, self.txs[tx].end);
        let Ok(p) = Packet::decode(&self.txs[tx].packet) else { return };
        let Some(path) = p.header.path.as_ref() else { return };
        let Some(&next) = path.get(p.header.hops as usize) else { return };
        let Some(r) = self.nodes.iter().position(|n| n.router.id == next) else { return };
        let p_rx = self.gain[sender][r];
        let why = if !self.audible(sender, r) {
            "uhørbar".to_string()
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
            format!("kollision med {}", t.unwrap_or_default())
        } else {
            "ok".into()
        };
        let margin = ((self.radio.snr(p_rx) - self.radio.snr_min) / 5.0).floor() as i32 * 5;
        let key = format!("{:?}, margen {margin:>2}-{} dB: {why}", p.header.ptype, margin + 5);
        *self.hop_trace.entry(key).or_default() += 1;
    }

    // ---------- flood-tilstand (sammenligning, Meshtastic-lignende) ----------

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
            // Som Meshtastic: hører vi en anden sende det videre, dropper vi vores.
            if self.flood_pending.remove(&(r, id)) {
                self.cancelled.insert((r, id));
            }
            return;
        }
        let me = self.nodes[r].router.addr;
        if h.ptype == PacketType::Data && h.dest == me {
            self.on_deliver(r, PacketType::Data, h.dest, pkt.payload);
            // Kvittering floodes tilbage.
            let ack = ack_token(id);
            self.flood_originate(r, PacketType::Ack, ack, &[]);
            return;
        }
        if h.ptype == PacketType::Ack
            && let Some(m) = self
                .msgs
                .iter_mut()
                .find(|m| ack_token(m.id) == h.dest && m.from == r)
        {
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

    // ---------- rapport ----------

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

        println!("Må:rse-simulator — tilstand: {:?}", c.mode);
        println!(
            "  {} noder ({} repeatere) på {:.0}×{:.0} km, SF{}, duty cycle {:.1} %, {} s, frø {}",
            c.nodes,
            c.repeaters,
            c.area_km,
            c.area_km,
            c.sf,
            c.duty_permille as f64 / 10.0,
            c.duration_s,
            c.seed
        );
        println!("  Gennemsnitligt antal naboer pr. node: {neighbours:.1}");
        println!();
        println!("Direkte beskeder ({}):", self.msgs.len());
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
            println!("  Til bilen: {d1}/{n1} leveret, fra bilen: {d2}/{n2} leveret");
        }
        println!(
            "  Leveret:        {:>5.1} %",
            delivered.len() as f64 / n * 100.0
        );
        println!("  Kvitteret:      {:>5.1} %", acked as f64 / n * 100.0);
        println!(
            "  Forsinkelse:    median {:.1} s, 90 % under {:.1} s",
            median as f64 / 1000.0,
            p90 as f64 / 1000.0
        );
        if c.mode == Mode::Maarse {
            println!("  Ruter fundet:   {}", self.stats.routes_learned);
        }
        println!();
        println!("Kanalbeskeder ({}):", self.stats.channel_sent);
        println!(
            "  Nåede i snit    {:>5.1} % af de andre noder",
            self.stats.channel_deliveries as f64 / receivers as f64 * 100.0
        );
        println!();
        if std::env::var("DEBUG_ACK").is_ok() {
            let mut hist: std::collections::BTreeMap<String, u32> = Default::default();
            for (maxh, len, tx) in self.resp_track.values() {
                let k = if maxh == len { "nåede sidste hop".to_string() } else if *tx > 6 { format!("stoppede ved {maxh}/{len} efter >6 udsendelser") } else { format!("stoppede ved {maxh}/{len} efter {tx} udsendelser") };
                *hist.entry(k).or_default() += 1;
            }
            for (k, v) in hist.iter().take(12) {
                println!("  [debug] svar {k}: {v}");
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
            println!("  [debug] nåbare beskeder (graf): {reach}/{}", self.msgs.len());
            let pr: u32 = self.nodes.iter().map(|n| n.router.stats.path_requests).sum();
            let hr: u32 = self.nodes.iter().map(|n| n.router.stats.hop_retries).sum();
            println!("  [debug] ruteforespørgsler startet: {pr}, hop gensendt: {hr}");
            println!("  [debug] Acked-handlinger i alt: {acked}, ruter lært: {rl}");
            for m in self
                .msgs
                .iter()
                .filter(|m| m.delivered_ms.is_some() && !m.acked)
                .take(5)
            {
                println!(
                    "  [debug] uden kvittering: {} -> {} sendt {} leveret {:?}",
                    m.from, m.to, m.sent_ms, m.delivered_ms
                );
            }
        }
        println!("Radio:");
        println!("  Udsendelser:    {}", self.stats.tx_count);
        println!("  Sendetid i alt: {:.1} s", total_air as f64 / 1e6);
        println!(
            "  Travleste node: {:.1} s = {:.0} % af sit budget",
            busiest as f64 / 1e6,
            busiest as f64 / budget_us.max(1) as f64 * 100.0
        );
        println!(
            "  Tabt i kollision (pr. modtager): {}",
            self.stats.collisions
        );
        println!(
            "  Droppet pga. duty cycle: {}, pga. optaget kanal: {}",
            self.stats.duty_drops, self.stats.channel_busy_drops
        );
        println!(
            "  Videresendelser annulleret (området dækket): {}",
            self.stats.suppressed
        );
        if c.mode == Mode::Maarse {
            println!("  Gensendte hop: {}", self.stats.hop_retries);
        }
        let types: Vec<String> = self
            .stats
            .by_type
            .iter()
            .map(|(k, v)| format!("{k} {v}"))
            .collect();
        println!("  Udsendelser pr. type: {}", types.join(", "));
    }
}

fn main() {
    let cfg = parse_args();
    let mut sim = Sim::new(cfg);
    sim.run();
    sim.report();
}
