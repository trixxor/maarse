//! Nodens hovedløkke: routing, kryptering, announces, filer, postkasser og
//! webklienter.

use crate::dht::{self, Contact, Record};
use crate::{ClientId, Config, In, LinkId};
use rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use maarse_core::companion::{Command, Event, MAX_TEXT, Stats, error_code, stat, text};
use maarse_core::content::{self, Content, FileOffer, Small};
use maarse_core::packet::{Header, encode};
use maarse_core::router::MAX_DIRECT_PAYLOAD;
use maarse_core::{Action, Address, PacketType, Role, Router};
use maarse_crypto::seal::{self, CHUNK_DATA, CryptoError, MAX_DM_TEXT};
use maarse_crypto::{
    Identity, PublicIdentity, channel_address, hashtag_key, public_channel_key, verify_announce,
};

/// Hvor ofte vi sender et announce.
const ANNOUNCE_EVERY: Duration = Duration::from_secs(600);
/// Beskeder fra ukendte afsendere gemmes, mens vi venter på et announce.
const PENDING_MAX: usize = 32;
/// Hvor ofte filer og postkasser tjekkes.
const HOUSEKEEPING_EVERY: Duration = Duration::from_secs(2);
/// Bidder af en fil, der må være undervejs på én gang.
const FILE_WINDOW: usize = 8;
/// En bid gensendes, hvis den ikke er kvitteret inden for denne tid.
const CHUNK_TIMEOUT: Duration = Duration::from_secs(20);
/// En fil opgives efter denne tid.
const FILE_TIMEOUT: Duration = Duration::from_secs(600);
/// Uden kvittering inden for denne tid lægges en besked i modtagerens postkasse.
const DEPOSIT_AFTER: Duration = Duration::from_secs(45);
/// Postkassen gemmer højst så mange beskeder pr. modtager, så længe.
const MAILBOX_PER_RECIPIENT: usize = 100;
const MAILBOX_TTL: Duration = Duration::from_secs(7 * 24 * 3600);
/// Hvor ofte DHT'en vedligeholdes (bootstrap, udgivelser, udløb).
const DHT_EVERY: Duration = Duration::from_secs(5);
/// Samme adresse slås højst op så ofte.
const DHT_LOOKUP_EVERY: Duration = Duration::from_secs(30);
/// Højst så mange gateways forbindes der til via DHT-opslag.
const DHT_MAX_DYNAMIC: usize = 32;
/// Announce-felt: postkassens adresse.
const APP_POSTBOX: u8 = 0x01;

enum Timer {
    Tx { packet: Vec<u8>, id: u64 },
    Tick,
    Announce,
    Housekeeping,
    Dht,
}

/// En fil, som en klient er ved at overføre til noden.
struct Upload {
    dest: Address,
    size: u32,
    mime: Small,
    name: Small,
    data: Vec<u8>,
}

struct OutFile {
    dest: Address,
    data: Vec<u8>,
    key: [u8; 32],
    chunks: u16,
    offer: Vec<u8>,
    offer_acked: bool,
    offer_sent: Instant,
    acked: Vec<bool>,
    done: u16,
    in_flight: HashMap<u16, Instant>,
    created: Instant,
}

struct InFile {
    from: Address,
    offer: FileOffer,
    parts: Vec<Option<Vec<u8>>>,
    got: u16,
    created: Instant,
}

/// Hvad en kvittering hører til.
enum Pending {
    Message,
    FileOffer(u64),
    FileChunk(u64, u16),
    Deposit(u64),
    MailDelivery(Address),
    /// Kontrolkommando til en fjern node (lægges aldrig i postkasse).
    Control,
}

/// En sendt direkte besked, der måske skal i postkassen.
struct OutMsg {
    dest: Address,
    sealed: Vec<u8>,
    sent: Instant,
    deposited: bool,
}

struct Mail {
    sealed: Vec<u8>,
    stored: SystemTime,
}

#[derive(Default)]
struct Counters {
    bytes_in: u64,
    bytes_out: u64,
    packets_in: u64,
    packets_out: u64,
    deposits: u32,
    mail_delivered: u32,
    errors: u32,
    last_snr: Option<i8>,
}

pub struct Node {
    identity: Identity,
    addr: Address,
    router: Router<1024, 4096>,
    known: HashMap<Address, PublicIdentity>,
    postboxes: HashMap<Address, Address>,
    my_postbox: Option<Address>,
    /// Kanaladresse -> (navn, nøgle)
    channels: HashMap<Address, (String, [u8; 32])>,
    links: HashMap<LinkId, (String, Sender<Vec<u8>>)>,
    clients: HashMap<ClientId, Sender<Vec<u8>>>,
    timers: BinaryHeap<Reverse<(Instant, u64)>>,
    timer_items: HashMap<u64, Timer>,
    seq: u64,
    tick_at: Option<u64>,
    cancelled: HashSet<u64>,
    pending_rx: Vec<(Vec<u8>, u8)>,
    awaiting: HashMap<u64, Pending>,
    outbox: HashMap<u64, OutMsg>,
    uploads: HashMap<(ClientId, u16), Upload>,
    out_files: HashMap<u64, OutFile>,
    in_files: HashMap<u64, InFile>,
    mailbox: HashMap<Address, HashMap<u64, Mail>>,
    max_file: u32,
    data_dir: PathBuf,
    start: Instant,
    counters: Counters,
    /// Hvem må administrere denne node over radio/backbone.
    admins: Vec<Address>,
    /// Seneste kontroltæller pr. administrator (mod genafspilning).
    control_seen: HashMap<Address, u32>,
    /// Seneste modtagne beskeder, så en klient, der forbinder senere, også
    /// får dem (fx beskeder hentet fra postkassen lige efter opstart).
    inbox: std::collections::VecDeque<Event>,
    /// Til tråde (DHT-opslag) og nye forbindelser.
    tx: Option<Sender<In>>,
    dht: Dht,
}

/// Backbone-DHT'ens tilstand (se dht.rs).
struct Dht {
    table: dht::Table,
    store: dht::Store,
    /// Vores offentlige endpoint; uden det udgiver vi intet.
    public_ep: Option<String>,
    /// Startpunkter (kendes kun ved endpoint).
    seeds: Vec<String>,
    /// Endpoints, vi allerede har en pakkeforbindelse til (eller er ved at få).
    peers: HashSet<String>,
    /// Gateways, vi har forbundet til på grund af et opslag.
    dynamic: HashSet<String>,
    /// Adresser, vi kan nå: seneste announce, hop og hvornår vi udgav det.
    heard: HashMap<Address, (Vec<u8>, u8, Option<Instant>)>,
    /// Hvornår vi sidst slog en adresse op (mod gentagne opslag).
    looked_up: HashMap<Address, Instant>,
    last_bootstrap: Option<Instant>,
    /// Beskeder til ukendte modtagere, der venter på et opslag.
    waiting: HashMap<Address, Vec<(ClientId, u16, Vec<u8>)>>,
    lookups: u32,
    found: u32,
}

fn unix_now() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as u32)
        .unwrap_or(0)
}

fn short(a: &Address) -> String {
    a.to_string()[..8].to_string()
}

fn token() -> [u8; 16] {
    let mut t = [0u8; 16];
    OsRng.fill_bytes(&mut t);
    t
}

/// `offentlig` / `public` -> den offentlige kanal, alt andet -> `#navn`.
fn normalize_channel(name: &str) -> (String, [u8; 32]) {
    let n = name.trim().to_lowercase();
    if matches!(
        n.as_str(),
        "offentlig" | "#offentlig" | "public" | "#public" | ""
    ) {
        ("offentlig".into(), public_channel_key())
    } else {
        let n = if n.starts_with('#') {
            n
        } else {
            format!("#{n}")
        };
        let key = hashtag_key(&n);
        (n, key)
    }
}

/// Filnavn uden stier og mærkelige tegn.
fn safe_name(name: &[u8]) -> String {
    let s: String = String::from_utf8_lossy(name)
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | ' '))
        .collect();
    let s = s.trim().trim_start_matches('.').to_string();
    if s.is_empty() { "fil".into() } else { s }
}

fn small(b: &[u8]) -> Small {
    Small::from_slice(&b[..b.len().min(64)]).unwrap_or_default()
}

impl Node {
    pub fn new(cfg: &Config) -> std::io::Result<Self> {
        fs::create_dir_all(&cfg.data_dir)?;
        let key_path = cfg.data_dir.join("identity.key");
        let identity = match fs::read(&key_path) {
            Ok(b) if b.len() == 64 => {
                let mut k = [0u8; 64];
                k.copy_from_slice(&b);
                Identity::from_secret_bytes(&k)
            }
            _ => {
                let id = Identity::generate(&mut OsRng);
                fs::write(&key_path, id.to_secret_bytes())?;
                id
            }
        };
        let addr = identity.address();
        let mut router = Router::new(addr, Role::Repeater);
        // Kun hurtige links her: korte tidsslots, ellers venter hvert hop unødigt.
        router.slot_ms = 5;

        let mut node = Node {
            identity,
            addr,
            router,
            known: HashMap::new(),
            postboxes: HashMap::new(),
            my_postbox: cfg.postbox,
            channels: HashMap::new(),
            links: HashMap::new(),
            clients: HashMap::new(),
            timers: BinaryHeap::new(),
            timer_items: HashMap::new(),
            seq: 0,
            tick_at: None,
            cancelled: HashSet::new(),
            pending_rx: Vec::new(),
            awaiting: HashMap::new(),
            outbox: HashMap::new(),
            uploads: HashMap::new(),
            out_files: HashMap::new(),
            in_files: HashMap::new(),
            mailbox: HashMap::new(),
            max_file: cfg.max_file,
            data_dir: cfg.data_dir.clone(),
            start: Instant::now(),
            counters: Counters::default(),
            admins: Vec::new(),
            control_seen: HashMap::new(),
            inbox: std::collections::VecDeque::new(),
            tx: None,
            dht: Dht {
                table: dht::Table::new(addr),
                store: dht::Store::new(),
                public_ep: cfg.public_addr.clone(),
                seeds: cfg
                    .dht_seeds
                    .iter()
                    .chain(cfg.peers.iter())
                    .cloned()
                    .collect(),
                peers: cfg.peers.iter().cloned().collect(),
                dynamic: HashSet::new(),
                heard: HashMap::new(),
                looked_up: HashMap::new(),
                last_bootstrap: None,
                waiting: HashMap::new(),
                lookups: 0,
                found: 0,
            },
        };
        let saved = fs::read_to_string(node.data_dir.join("admins.txt")).unwrap_or_default();
        node.admins = saved.lines().filter_map(parse_addr).collect();
        if let Ok(b) = fs::read(node.data_dir.join("known.bin")) {
            for chunk in b.chunks_exact(64) {
                if let Some(p) = PublicIdentity::from_bytes(chunk) {
                    node.known.insert(p.address(), p);
                }
            }
        }
        let saved = fs::read_to_string(node.data_dir.join("channels.txt")).unwrap_or_default();
        node.join_channel("offentlig", false);
        for line in saved.lines().filter(|l| !l.trim().is_empty()) {
            node.join_channel(line, false);
        }
        node.load_mailbox();
        Ok(node)
    }

    pub fn address(&self) -> Address {
        self.addr
    }

    fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    fn schedule(&mut self, after: Duration, t: Timer) {
        self.seq += 1;
        self.timer_items.insert(self.seq, t);
        self.timers
            .push(Reverse((Instant::now() + after, self.seq)));
    }

    fn log(&self, msg: impl AsRef<str>) {
        println!("[node] {}", msg.as_ref());
    }

    // ---------- hovedløkke ----------

    pub fn run(mut self, rx: Receiver<In>, tx: Sender<In>) {
        self.tx = Some(tx);
        self.schedule(Duration::from_secs(1), Timer::Announce);
        self.schedule(Duration::from_millis(500), Timer::Dht);
        self.schedule(HOUSEKEEPING_EVERY, Timer::Housekeeping);
        loop {
            let wait = self
                .timers
                .peek()
                .map(|Reverse((t, _))| t.saturating_duration_since(Instant::now()))
                .unwrap_or(Duration::from_secs(1));
            match rx.recv_timeout(wait) {
                Ok(msg) => self.handle(msg),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    std::thread::sleep(wait.min(Duration::from_millis(200)));
                }
            }
            self.fire_timers();
        }
    }

    fn fire_timers(&mut self) {
        let now = Instant::now();
        while let Some(Reverse((t, seq))) = self.timers.peek().copied() {
            if t > now {
                break;
            }
            self.timers.pop();
            match self.timer_items.remove(&seq) {
                Some(Timer::Tx { packet, id }) => {
                    if !self.cancelled.remove(&id) {
                        self.send_links(&packet);
                    }
                }
                Some(Timer::Tick) => {
                    self.tick_at = None;
                    let actions = self.router.tick(self.now_ms());
                    self.apply(actions.into_iter().collect());
                }
                Some(Timer::Announce) => {
                    self.announce();
                    self.schedule(ANNOUNCE_EVERY, Timer::Announce);
                }
                Some(Timer::Housekeeping) => {
                    self.housekeeping();
                    self.schedule(HOUSEKEEPING_EVERY, Timer::Housekeeping);
                }
                Some(Timer::Dht) => {
                    self.dht_maintain();
                    self.schedule(DHT_EVERY, Timer::Dht);
                }
                None => {}
            }
        }
    }

    fn handle(&mut self, msg: In) {
        match msg {
            In::LinkUp { id, name, tx } => {
                self.log(format!("link oppe: {name}"));
                self.links.insert(id, (name, tx));
                self.announce();
                self.broadcast_log(format!("Link oppe ({} i alt)", self.links.len()));
            }
            In::LinkDown { id } => {
                if let Some((name, _)) = self.links.remove(&id) {
                    self.log(format!("link nede: {name}"));
                    self.broadcast_log(format!("Link nede ({} tilbage)", self.links.len()));
                }
            }
            In::Packet { data, snr, link } => {
                self.counters.packets_in += 1;
                self.counters.bytes_in += data.len() as u64;
                if self
                    .links
                    .get(&link)
                    .is_some_and(|(n, _)| n.starts_with("seriel"))
                {
                    self.counters.last_snr = Some(snr);
                }
                if let Ok((h, _)) = Header::decode(&data)
                    && h.ptype == PacketType::PathRequest
                {
                    self.dht_maybe_lookup(h.dest);
                }
                let now = self.now_ms();
                let actions = self.router.on_receive(&data, snr, now);
                self.apply(actions.into_iter().collect());
            }
            In::ClientUp { id, tx } => {
                self.clients.insert(id, tx);
                self.greet(id);
            }
            In::ClientDown { id } => {
                self.clients.remove(&id);
                self.uploads.retain(|(c, _), _| *c != id);
            }
            In::DhtRequest { data, reply } => {
                let _ = reply.send(self.dht_serve(&data));
            }
            In::DhtDone {
                target,
                publish,
                result,
            } => self.dht_done(target, publish, result),
            In::ClientFrame { id, frame } => match Command::decode(&frame) {
                Ok(cmd) => self.command(id, cmd),
                Err(_) => self.send_client(
                    id,
                    &Event::Error {
                        req: 0,
                        code: error_code::BAD_FRAME,
                        text: text(b"ugyldig ramme"),
                    },
                ),
            },
        }
    }

    // ---------- udsendelse ----------

    fn send_links(&mut self, packet: &[u8]) {
        for (_, tx) in self.links.values() {
            let _ = tx.send(packet.to_vec());
            self.counters.packets_out += 1;
            self.counters.bytes_out += packet.len() as u64;
        }
    }

    fn apply(&mut self, actions: Vec<Action>) {
        for a in actions {
            match a {
                Action::Transmit {
                    packet,
                    delay_ms,
                    id,
                } => {
                    if let Ok((h, _)) = Header::decode(&packet)
                        && h.ptype == PacketType::PathRequest
                    {
                        self.dht_maybe_lookup(h.dest);
                    }
                    if delay_ms == 0 {
                        self.send_links(&packet);
                    } else {
                        let t = Timer::Tx {
                            packet: packet.to_vec(),
                            id,
                        };
                        self.schedule(Duration::from_millis(delay_ms as u64), t);
                    }
                }
                Action::Cancel { id } => {
                    self.cancelled.insert(id);
                }
                Action::Deliver {
                    ptype,
                    dest,
                    payload,
                    hops,
                } => self.on_deliver(ptype, dest, &payload, hops),
                Action::RouteLearned { dest, hops } => {
                    self.broadcast(&Event::RouteLearned { dest, hops });
                    self.flush_mailbox(dest);
                }
                Action::Acked { msg_id } => self.on_acked(msg_id),
            }
        }
        if let Some(d) = self.router.next_deadline()
            && self.tick_at.is_none_or(|t| d < t)
        {
            self.tick_at = Some(d);
            let after = Duration::from_millis(d.saturating_sub(self.now_ms()));
            self.schedule(after, Timer::Tick);
        }
    }

    fn announce(&mut self) {
        let mut app = Vec::new();
        if let Some(p) = self.my_postbox {
            app.push(APP_POSTBOX);
            app.extend_from_slice(&p.0);
        }
        let Some(payload) = self.identity.announce(unix_now(), &app) else {
            return;
        };
        let header = Header::new(PacketType::Announce, self.addr);
        if let Ok(pkt) = encode(&header, &payload) {
            self.send_links(&pkt);
        }
        // Vores egen adresse kan altid nås her.
        self.dht.heard.insert(self.addr, (payload.to_vec(), 0, None));
    }

    /// Krypterer og sender en direkte besked. Returnerer beskedens ID.
    fn send_sealed(
        &mut self,
        dest: Address,
        plain: &[u8],
        kind: Pending,
    ) -> Result<u64, &'static str> {
        let recipient = *self
            .known
            .get(&dest)
            .ok_or("Modtageren er ikke hørt endnu (intet announce)")?;
        let mut ct = [0u8; 255];
        let n = seal::seal_direct(&self.identity, &recipient, &mut OsRng, plain, &mut ct)
            .map_err(|_| "Beskeden er for lang")?;
        let now = self.now_ms();
        let (msg_id, actions) = self
            .router
            .send_direct(dest, &ct[..n], token(), now)
            .map_err(|_| "Køen er fuld, prøv igen om lidt")?;
        if matches!(kind, Pending::Message) {
            self.outbox.insert(
                msg_id,
                OutMsg {
                    dest,
                    sealed: ct[..n].to_vec(),
                    sent: Instant::now(),
                    deposited: false,
                },
            );
        }
        self.awaiting.insert(msg_id, kind);
        self.apply(actions.into_iter().collect());
        Ok(msg_id)
    }

    fn on_acked(&mut self, msg_id: u64) {
        match self.awaiting.remove(&msg_id) {
            Some(Pending::Control) => {}
            Some(Pending::Message) | None => {
                self.outbox.remove(&msg_id);
                self.broadcast(&Event::Acked { msg_id });
            }
            Some(Pending::FileOffer(file_id)) => {
                if let Some(f) = self.out_files.get_mut(&file_id) {
                    f.offer_acked = true;
                }
                self.pump_file(file_id);
            }
            Some(Pending::FileChunk(file_id, idx)) => self.chunk_acked(file_id, idx),
            Some(Pending::Deposit(orig)) => {
                self.counters.deposits += 1;
                self.log("besked lagt i postkasse");
                self.broadcast(&Event::Deposited { msg_id: orig });
            }
            Some(Pending::MailDelivery(recipient)) => {
                if let Some(m) = self.mailbox.get_mut(&recipient) {
                    m.remove(&msg_id);
                }
                self.counters.mail_delivered += 1;
                self.log(format!("postkasse: leveret til {}", short(&recipient)));
                self.save_mailbox();
            }
        }
    }

    // ---------- modtagelse ----------

    fn on_deliver(&mut self, ptype: PacketType, dest: Address, payload: &[u8], hops: u8) {
        match ptype {
            PacketType::Announce if dest != self.addr => self.on_announce(dest, payload, hops),
            PacketType::Data => self.open_direct(payload.to_vec(), hops, true),
            PacketType::Fragment => self.on_chunk(payload),
            PacketType::Deposit => self.on_deposit(payload),
            PacketType::Channel => {
                let Some((_, key)) = self.channels.get(&dest) else {
                    return;
                };
                let mut out = [0u8; 256];
                match seal::open_channel(key, payload, &mut out) {
                    Ok((from, n)) if from != self.addr => {
                        self.broadcast(&Event::ChannelReceived {
                            channel: dest,
                            from,
                            hops,
                            text: text(&out[..n]),
                        });
                    }
                    Ok(_) => {}
                    Err(e) => self.log(format!("kanalbesked kunne ikke åbnes: {e:?}")),
                }
            }
            _ => {}
        }
    }

    fn on_announce(&mut self, dest: Address, payload: &[u8], hops: u8) {
        match verify_announce(&dest, payload) {
            Ok(a) => {
                if a.app_data.len() >= 17 && a.app_data[0] == APP_POSTBOX
                    && let Some(p) = Address::from_slice(&a.app_data[1..17]) {
                        self.postboxes.insert(dest, p);
                    }
                if self.known.insert(dest, a.identity).is_none() {
                    self.log(format!("ny node hørt: {}", short(&dest)));
                    if let Ok(mut f) = fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(self.data_dir.join("known.bin"))
                    {
                        let _ = f.write_all(&a.identity.to_bytes());
                    }
                }
                self.broadcast(&Event::Heard { addr: dest, hops });
                if self.dht.public_ep.is_some() {
                    let last = self.dht.heard.get(&dest).and_then(|h| h.2);
                    self.dht.heard.insert(dest, (payload.to_vec(), hops, last));
                }
                self.retry_pending();
                self.flush_mailbox(dest);
            }
            Err(e) => self.log(format!("afvist announce fra {}: {e:?}", short(&dest))),
        }
    }

    fn open_direct(&mut self, payload: Vec<u8>, hops: u8, first_time: bool) {
        let mut out = [0u8; 256];
        let known = &self.known;
        match seal::open_direct(
            &self.identity,
            &payload,
            |a| known.get(a).copied(),
            &mut out,
        ) {
            Ok((from, n)) => self.on_content(from, &out[..n], hops),
            Err(CryptoError::UnknownSender(a)) => {
                if first_time {
                    self.log(format!(
                        "besked fra ukendt {}: venter på announce",
                        short(&a)
                    ));
                    if self.pending_rx.len() >= PENDING_MAX {
                        self.pending_rx.remove(0);
                    }
                    self.pending_rx.push((payload, hops));
                }
            }
            Err(e) => {
                self.counters.errors += 1;
                self.log(format!("besked kunne ikke åbnes: {e:?}"));
            }
        }
    }

    fn on_content(&mut self, from: Address, plain: &[u8], hops: u8) {
        match content::parse(plain) {
            Some(Content::Text(t)) => {
                self.log(format!("besked fra {} ({hops} hop)", short(&from)));
                self.broadcast(&Event::DirectReceived {
                    from,
                    hops,
                    text: text(t),
                });
            }
            Some(Content::FileOffer(offer)) => self.on_file_offer(from, offer),
            Some(Content::Control(counter, frame)) => self.on_control(from, counter, frame),
            Some(Content::ControlReply(_, frame)) => {
                self.broadcast(&Event::RemoteReply { from, frame: text(frame) });
            }
            Some(Content::Unknown(t)) => self.log(format!("ukendt indholdstype {t}")),
            None => {}
        }
    }

    fn retry_pending(&mut self) {
        let pending = std::mem::take(&mut self.pending_rx);
        for (p, hops) in pending {
            let mut out = [0u8; 256];
            let known = &self.known;
            match seal::open_direct(&self.identity, &p, |a| known.get(a).copied(), &mut out) {
                Err(CryptoError::UnknownSender(_)) => self.pending_rx.push((p, hops)),
                _ => self.open_direct(p, hops, false),
            }
        }
    }

    // ---------- filer ----------

    fn start_file(&mut self, id: ClientId, req: u16) {
        let Some(up) = self.uploads.remove(&(id, req)) else {
            return self.send_client(
                id,
                &err_ev(req, error_code::NO_FILE, "Ingen fil under upload"),
            );
        };
        if up.data.len() != up.size as usize {
            return self.send_client(
                id,
                &err_ev(req, error_code::NO_FILE, "Filen blev ikke overført helt"),
            );
        }
        let chunks = up.data.len().div_ceil(CHUNK_DATA).max(1);
        if chunks > u16::MAX as usize {
            return self.send_client(
                id,
                &err_ev(req, error_code::FILE_TOO_LARGE, "Filen er for stor"),
            );
        }
        let mut file_id_b = [0u8; 8];
        OsRng.fill_bytes(&mut file_id_b);
        let file_id = u64::from_le_bytes(file_id_b);
        let mut key = [0u8; 32];
        OsRng.fill_bytes(&mut key);
        let digest = Sha256::digest(&up.data);
        let offer = FileOffer {
            file_id,
            size: up.size,
            chunks: chunks as u16,
            chunk_size: CHUNK_DATA as u8,
            key,
            hash: digest[..16].try_into().unwrap_or_default(),
            mime: up.mime,
            name: up.name,
        };
        let plain = offer.encode();
        match self.send_sealed(up.dest, &plain, Pending::FileOffer(file_id)) {
            Ok(_) => {
                self.out_files.insert(
                    file_id,
                    OutFile {
                        dest: up.dest,
                        data: up.data,
                        key,
                        chunks: chunks as u16,
                        offer: plain.to_vec(),
                        offer_acked: false,
                        offer_sent: Instant::now(),
                        acked: vec![false; chunks],
                        done: 0,
                        in_flight: HashMap::new(),
                        created: Instant::now(),
                    },
                );
                self.log(format!(
                    "sender fil ({chunks} bidder) til {}",
                    short(&up.dest)
                ));
                self.send_client(
                    id,
                    &Event::FileStarted {
                        req,
                        file_id,
                        chunks: chunks as u16,
                    },
                );
            }
            Err(e) => self.send_client(id, &err_ev(req, error_code::UNKNOWN_DEST, e)),
        }
    }

    /// Sender flere bidder, så længe vinduet tillader det.
    fn pump_file(&mut self, file_id: u64) {
        loop {
            let Some(f) = self.out_files.get(&file_id) else {
                return;
            };
            if !f.offer_acked || f.in_flight.len() >= FILE_WINDOW {
                return;
            }
            let Some(idx) =
                (0..f.chunks).find(|i| !f.acked[*i as usize] && !f.in_flight.contains_key(i))
            else {
                return;
            };
            let start = idx as usize * CHUNK_DATA;
            let end = (start + CHUNK_DATA).min(f.data.len());
            let mut ct = [0u8; 255];
            let Ok(n) = seal::seal_chunk(&f.key, file_id, idx, &f.data[start..end], &mut ct) else {
                return;
            };
            let dest = f.dest;
            let now = self.now_ms();
            match self.router.send_fragment(dest, &ct[..n], now) {
                Ok((msg_id, actions)) => {
                    self.awaiting
                        .insert(msg_id, Pending::FileChunk(file_id, idx));
                    if let Some(f) = self.out_files.get_mut(&file_id) {
                        f.in_flight.insert(idx, Instant::now());
                    }
                    self.apply(actions.into_iter().collect());
                }
                Err(_) => return, // Ingen rute lige nu; prøves igen senere.
            }
        }
    }

    fn chunk_acked(&mut self, file_id: u64, idx: u16) {
        let Some(f) = self.out_files.get_mut(&file_id) else {
            return;
        };
        f.in_flight.remove(&idx);
        if !f.acked[idx as usize] {
            f.acked[idx as usize] = true;
            f.done += 1;
        }
        let (done, total) = (f.done, f.chunks);
        self.broadcast(&Event::FileProgress {
            file_id,
            done,
            total,
            outgoing: true,
        });
        if done == total {
            self.out_files.remove(&file_id);
            self.log("fil leveret");
            self.broadcast(&Event::FileDone {
                file_id,
                ok: true,
                outgoing: true,
                text: text(b"leveret"),
            });
        } else {
            self.pump_file(file_id);
        }
    }

    fn on_file_offer(&mut self, from: Address, offer: FileOffer) {
        if offer.size > self.max_file
            || (offer.chunks as usize) * (offer.chunk_size as usize) < offer.size as usize
            || offer.chunk_size == 0
        {
            self.log(format!(
                "fil fra {} afvist (for stor eller ugyldig)",
                short(&from)
            ));
            return;
        }
        if self.in_files.contains_key(&offer.file_id) {
            return;
        }
        self.broadcast(&Event::FileIncoming {
            file_id: offer.file_id,
            from,
            size: offer.size,
            mime: text(&offer.mime),
            name: text(&offer.name),
        });
        self.log(format!(
            "fil på vej fra {}: {} bytes",
            short(&from),
            offer.size
        ));
        self.in_files.insert(
            offer.file_id,
            InFile {
                from,
                parts: vec![None; offer.chunks as usize],
                got: 0,
                created: Instant::now(),
                offer,
            },
        );
    }

    fn on_chunk(&mut self, payload: &[u8]) {
        let Some((file_id, idx)) = seal::chunk_header(payload) else {
            return;
        };
        let Some(f) = self.in_files.get_mut(&file_id) else {
            return;
        };
        if idx as usize >= f.parts.len() || f.parts[idx as usize].is_some() {
            return;
        }
        let mut out = [0u8; 255];
        let Ok(n) = seal::open_chunk(&f.offer.key, payload, &mut out) else {
            return;
        };
        f.parts[idx as usize] = Some(out[..n].to_vec());
        f.got += 1;
        let (got, total) = (f.got, f.offer.chunks);
        self.broadcast(&Event::FileProgress {
            file_id,
            done: got,
            total,
            outgoing: false,
        });
        if got == total {
            self.finish_incoming(file_id);
        }
    }

    fn finish_incoming(&mut self, file_id: u64) {
        let Some(f) = self.in_files.remove(&file_id) else {
            return;
        };
        let mut data: Vec<u8> = f.parts.into_iter().flatten().flatten().collect();
        data.truncate(f.offer.size as usize);
        let ok = data.len() == f.offer.size as usize && Sha256::digest(&data)[..16] == f.offer.hash;
        if !ok {
            self.broadcast(&Event::FileDone {
                file_id,
                ok: false,
                outgoing: false,
                text: text(b"kontrolsum passer ikke"),
            });
            return;
        }
        let dir = self.data_dir.join("filer");
        let _ = fs::create_dir_all(&dir);
        let name = format!("{}-{}", short(&f.from), safe_name(&f.offer.name));
        let _ = fs::write(dir.join(&name), &data);
        self.log(format!("fil modtaget: {name} ({} bytes)", data.len()));
        for (i, part) in data.chunks(MAX_TEXT).enumerate() {
            self.broadcast(&Event::FileChunk {
                file_id,
                offset: (i * MAX_TEXT) as u32,
                data: text(part),
            });
        }
        self.broadcast(&Event::FileDone {
            file_id,
            ok: true,
            outgoing: false,
            text: text(name.as_bytes()),
        });
    }

    // ---------- postkasse ----------

    fn on_deposit(&mut self, payload: &[u8]) {
        let Some(recipient) = Address::from_slice(payload) else {
            return;
        };
        let sealed = payload[16..].to_vec();
        let id = maarse_core::dedup::packet_id(PacketType::Data, &recipient, &sealed);
        let box_ = self.mailbox.entry(recipient).or_default();
        if box_.len() >= MAILBOX_PER_RECIPIENT {
            self.log(format!("postkasse fuld for {}", short(&recipient)));
            return;
        }
        box_.insert(
            id,
            Mail {
                sealed,
                stored: SystemTime::now(),
            },
        );
        let count = box_.len();
        self.log(format!(
            "postkasse: gemt besked til {} ({count} i alt)",
            short(&recipient)
        ));
        self.save_mailbox();
        // Måske er modtageren her allerede.
        self.flush_mailbox(recipient);
    }

    /// Prøver at levere ventende post til `recipient`.
    fn flush_mailbox(&mut self, recipient: Address) {
        let Some(mails) = self.mailbox.get(&recipient) else {
            return;
        };
        if mails.is_empty()
            || !self.router.has_route(&recipient) && !self.known.contains_key(&recipient)
        {
            return;
        }
        let items: Vec<Vec<u8>> = mails.values().map(|m| m.sealed.clone()).collect();
        self.log(format!(
            "postkasse: prøver at levere {} besked(er) til {}",
            items.len(),
            short(&recipient)
        ));
        for sealed in items {
            let now = self.now_ms();
            if let Ok((msg_id, actions)) = self.router.send_direct(recipient, &sealed, token(), now)
            {
                self.awaiting
                    .insert(msg_id, Pending::MailDelivery(recipient));
                self.apply(actions.into_iter().collect());
            }
        }
    }

    fn try_deposit(&mut self, msg_id: u64) {
        let Some(m) = self.outbox.get_mut(&msg_id) else {
            return;
        };
        if m.deposited {
            return;
        }
        let Some(postbox) = self.postboxes.get(&m.dest).copied() else {
            return;
        };
        if postbox == m.dest || 16 + m.sealed.len() > MAX_DIRECT_PAYLOAD {
            return;
        }
        m.deposited = true;
        let mut payload = m.dest.0.to_vec();
        payload.extend_from_slice(&m.sealed);
        let now = self.now_ms();
        if let Ok((dep_id, actions)) =
            self.router
                .send_typed(PacketType::Deposit, postbox, &payload, token(), now)
        {
            self.log(format!(
                "modtager svarer ikke; lægger besked i postkasse {}",
                short(&postbox)
            ));
            self.awaiting.insert(dep_id, Pending::Deposit(msg_id));
            self.apply(actions.into_iter().collect());
        }
    }

    fn save_mailbox(&self) {
        // Format: modtager (16) | gemt (u64 sek.) | længde (u16) | besked
        let mut b = Vec::new();
        for (r, mails) in &self.mailbox {
            for m in mails.values() {
                b.extend_from_slice(&r.0);
                let t = m
                    .stored
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                b.extend_from_slice(&t.to_le_bytes());
                b.extend_from_slice(&(m.sealed.len() as u16).to_le_bytes());
                b.extend_from_slice(&m.sealed);
            }
        }
        let _ = fs::write(self.data_dir.join("postkasse.bin"), b);
    }

    fn load_mailbox(&mut self) {
        let Ok(b) = fs::read(self.data_dir.join("postkasse.bin")) else {
            return;
        };
        let mut i = 0;
        while i + 26 <= b.len() {
            let r = Address::from_slice(&b[i..i + 16]).unwrap_or_default();
            let t = u64::from_le_bytes(b[i + 16..i + 24].try_into().unwrap_or_default());
            let n = u16::from_le_bytes([b[i + 24], b[i + 25]]) as usize;
            let Some(sealed) = b.get(i + 26..i + 26 + n) else {
                break;
            };
            let id = maarse_core::dedup::packet_id(PacketType::Data, &r, sealed);
            self.mailbox.entry(r).or_default().insert(
                id,
                Mail {
                    sealed: sealed.to_vec(),
                    stored: UNIX_EPOCH + Duration::from_secs(t),
                },
            );
            i += 26 + n;
        }
    }

    // ---------- vedligehold ----------

    fn housekeeping(&mut self) {
        let now = Instant::now();
        // Beskeder uden kvittering: læg dem i modtagerens postkasse.
        // MAARSE_DEPOSIT_AFTER_S gør det muligt at teste postkassen hurtigt.
        let after = std::env::var("MAARSE_DEPOSIT_AFTER_S")
            .ok()
            .and_then(|v| v.parse().ok())
            .map(Duration::from_secs)
            .unwrap_or(DEPOSIT_AFTER);
        let late: Vec<u64> = self
            .outbox
            .iter()
            .filter(|(_, m)| !m.deposited && now.duration_since(m.sent) > after)
            .map(|(id, _)| *id)
            .collect();
        for id in late {
            self.try_deposit(id);
        }
        self.outbox
            .retain(|_, m| now.duration_since(m.sent) < Duration::from_secs(3600));

        // Filer: gensend bidder uden kvittering, opgiv gamle.
        let ids: Vec<u64> = self.out_files.keys().copied().collect();
        for file_id in ids {
            let mut resend_offer = None;
            let mut failed = false;
            if let Some(f) = self.out_files.get_mut(&file_id) {
                if now.duration_since(f.created) > FILE_TIMEOUT {
                    failed = true;
                } else {
                    f.in_flight
                        .retain(|_, sent| now.duration_since(*sent) < CHUNK_TIMEOUT);
                    if !f.offer_acked && now.duration_since(f.offer_sent) > CHUNK_TIMEOUT {
                        f.offer_sent = now;
                        resend_offer = Some((f.dest, f.offer.clone()));
                    }
                }
            }
            if failed {
                self.out_files.remove(&file_id);
                self.broadcast(&Event::FileDone {
                    file_id,
                    ok: false,
                    outgoing: true,
                    text: text(b"modtageren svarede ikke"),
                });
                continue;
            }
            if let Some((dest, offer)) = resend_offer {
                let _ = self.send_sealed(dest, &offer, Pending::FileOffer(file_id));
            }
            self.pump_file(file_id);
        }
        // Halvfærdige indgående filer.
        let stale: Vec<u64> = self
            .in_files
            .iter()
            .filter(|(_, f)| now.duration_since(f.created) > FILE_TIMEOUT)
            .map(|(id, _)| *id)
            .collect();
        for file_id in stale {
            self.in_files.remove(&file_id);
            self.broadcast(&Event::FileDone {
                file_id,
                ok: false,
                outgoing: false,
                text: text(b"overforsel afbrudt"),
            });
        }
        // Gammel post.
        let before = self.mailbox.values().map(|m| m.len()).sum::<usize>();
        for mails in self.mailbox.values_mut() {
            mails.retain(|_, m| m.stored.elapsed().map(|e| e < MAILBOX_TTL).unwrap_or(true));
        }
        self.mailbox.retain(|_, m| !m.is_empty());
        if self.mailbox.values().map(|m| m.len()).sum::<usize>() != before {
            self.save_mailbox();
        }
    }

    // ---------- webklienter ----------

    fn send_client(&self, id: ClientId, ev: &Event) {
        if let Some(tx) = self.clients.get(&id) {
            let _ = tx.send(ev.encode().to_vec());
        }
    }

    fn broadcast(&mut self, ev: &Event) {
        if matches!(ev, Event::DirectReceived { .. } | Event::ChannelReceived { .. }) {
            if self.inbox.len() >= 100 {
                self.inbox.pop_front();
            }
            self.inbox.push_back(ev.clone());
        }
        let f = ev.encode().to_vec();
        for tx in self.clients.values() {
            let _ = tx.send(f.clone());
        }
    }

    fn broadcast_log(&mut self, msg: String) {
        self.broadcast(&Event::Log {
            text: text(msg.as_bytes()),
        });
    }

    fn greet(&self, id: ClientId) {
        self.send_client(id, &self.info());
        for (ch, (name, _)) in &self.channels {
            self.send_client(
                id,
                &Event::ChannelJoined {
                    channel: *ch,
                    name: text(name.as_bytes()),
                },
            );
        }
        for a in self.known.keys() {
            self.send_client(id, &Event::Heard { addr: *a, hops: 0 });
        }
        for ev in &self.inbox {
            self.send_client(id, ev);
        }
        let msg = format!(
            "Forbundet til maarse-host. {} link(s) oppe.",
            self.links.len()
        );
        self.send_client(
            id,
            &Event::Log {
                text: text(msg.as_bytes()),
            },
        );
    }

    fn info(&self) -> Event {
        Event::Info {
            addr: self.addr,
            repeater: true,
            version: maarse_core::companion::PROTOCOL_VERSION,
            max_text: (MAX_DM_TEXT - 1) as u8,
            max_file: self.max_file,
        }
    }

    fn join_channel(&mut self, name: &str, save: bool) -> (Address, String) {
        let (name, key) = normalize_channel(name);
        let ch = channel_address(&key);
        let _ = self.router.subscribe(ch);
        let new = self.channels.insert(ch, (name.clone(), key)).is_none();
        if save && new {
            let names: Vec<String> = self
                .channels
                .values()
                .map(|(n, _)| n.clone())
                .filter(|n| n != "offentlig")
                .collect();
            let _ = fs::write(self.data_dir.join("channels.txt"), names.join("\n"));
        }
        (ch, name)
    }

    fn command(&mut self, id: ClientId, cmd: Command) {
        match cmd {
            Command::GetInfo => self.send_client(id, &self.info()),
            Command::AnnounceNow => {
                self.announce();
                self.send_client(id, &Event::Log { text: text(b"Announce sendt") });
            }
            Command::JoinChannel { name } => {
                let name = String::from_utf8_lossy(&name).to_string();
                let (ch, name) = self.join_channel(&name, true);
                self.broadcast(&Event::ChannelJoined {
                    channel: ch,
                    name: text(name.as_bytes()),
                });
            }
            Command::SendDirect { req, dest, text: msg } => {
                if msg.len() > MAX_DM_TEXT - 1 {
                    return self.send_client(id, &err_ev(req, error_code::TOO_LARGE, "Beskeden er for lang"));
                }
                if !self.known.contains_key(&dest) && self.dht_can_lookup() {
                    // Ukendt modtager: slå op i backbone-DHT'en og send bagefter.
                    self.dht.waiting.entry(dest).or_default().push((id, req, msg.to_vec()));
                    self.dht.looked_up.remove(&dest);
                    self.dht_maybe_lookup(dest);
                    let t = format!("Slår {} op i DHT …", short(&dest));
                    return self.send_client(id, &Event::Log { text: text(t.as_bytes()) });
                }
                match self.send_sealed(dest, &content::text(&msg), Pending::Message) {
                    Ok(msg_id) => self.send_client(id, &Event::Sent { req, msg_id }),
                    Err(e) => self.send_client(id, &err_ev(req, error_code::UNKNOWN_DEST, e)),
                }
            }
            Command::SendChannel { req, channel, text: msg } => {
                let Some((_, key)) = self.channels.get(&channel).cloned() else {
                    return self.send_client(id, &err_ev(req, error_code::NOT_IN_CHANNEL, "Ikke med i kanalen"));
                };
                let mut nonce = [0u8; 8];
                OsRng.fill_bytes(&mut nonce);
                let mut ct = [0u8; 255];
                let res = seal::seal_channel(&key, &self.addr, nonce, &msg, &mut ct)
                    .ok()
                    .and_then(|n| self.router.send_channel(channel, &ct[..n]).ok());
                match res {
                    Some(actions) => {
                        self.send_client(id, &Event::Sent { req, msg_id: 0 });
                        self.apply(actions.into_iter().collect());
                    }
                    None => self.send_client(id, &err_ev(req, error_code::TOO_LARGE, "Beskeden er for lang")),
                }
            }
            Command::FileBegin { req, dest, size, mime, name } => {
                if size > self.max_file {
                    let m = format!("Filen er for stor (højst {} KB)", self.max_file / 1024);
                    return self.send_client(id, &err_ev(req, error_code::FILE_TOO_LARGE, &m));
                }
                if !self.known.contains_key(&dest) {
                    return self.send_client(
                        id,
                        &err_ev(req, error_code::UNKNOWN_DEST, "Modtageren er ikke hørt endnu (intet announce)"),
                    );
                }
                self.uploads.insert(
                    (id, req),
                    Upload {
                        dest,
                        size,
                        mime: small(&mime),
                        name: small(&name),
                        data: Vec::with_capacity(size as usize),
                    },
                );
            }
            Command::FileData { req, offset, data } => {
                if let Some(u) = self.uploads.get_mut(&(id, req))
                    && offset as usize == u.data.len()
                    && u.data.len() + data.len() <= u.size as usize
                {
                    u.data.extend_from_slice(&data);
                }
            }
            Command::FileEnd { req } => self.start_file(id, req),
            Command::Remote { req, dest, frame } => {
                let ok = matches!(
                    Command::decode(&frame),
                    Ok(Command::GetStats | Command::GetRadio | Command::SetRadio(_) | Command::SetRole { .. })
                );
                if !ok {
                    return self.send_client(id, &err_ev(req, error_code::BAD_FRAME, "Kun status, radio og rolle kan fjernstyres"));
                }
                // Tælleren følger uret, så den også stiger efter en genstart.
                let counter = unix_now();
                let plain = content::control(content::CONTROL, counter, &frame);
                match self.send_sealed(dest, &plain, Pending::Control) {
                    Ok(msg_id) => self.send_client(id, &Event::Sent { req, msg_id }),
                    Err(e) => self.send_client(id, &err_ev(req, error_code::UNKNOWN_DEST, e)),
                }
            }
            Command::SetAdmins { admins } => {
                self.admins = admins.to_vec();
                let lines: Vec<String> = self.admins.iter().map(|a| a.to_string()).collect();
                let _ = fs::write(self.data_dir.join("admins.txt"), lines.join("\n"));
                self.log(format!("{} administrator(er) gemt", self.admins.len()));
                self.send_client(id, &self.admins_event());
            }
            Command::GetAdmins => self.send_client(id, &self.admins_event()),
            Command::SetTime { .. } => {} // maarse-host har sit eget ur
            Command::GetRadio | Command::SetRadio(_) => self.send_client(
                id,
                &err_ev(
                    0,
                    error_code::BAD_FRAME,
                    "maarse-host har ingen egen radio. Forbind til noden via USB for at ændre radioindstillinger.",
                ),
            ),
            Command::SetRole { .. } => self.send_client(
                id,
                &err_ev(
                    0,
                    error_code::BAD_FRAME,
                    "maarse-host er en gateway og har altid rollen repeater. Forbind til noden via USB for at vælge rolle.",
                ),
            ),
            Command::GetStats => {
                let ev = Event::Stats { items: self.stats() };
                self.send_client(id, &ev);
            }
        }
    }
}

fn parse_addr(s: &str) -> Option<Address> {
    let s = s.trim();
    if s.len() != 32 {
        return None;
    }
    let b: Vec<u8> = (0..32).step_by(2).filter_map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect();
    Address::from_slice(&b)
}

impl Node {
    fn admins_event(&self) -> Event {
        Event::Admins { admins: self.admins.iter().copied().take(4).collect() }
    }

    /// En kontrolkommando fra en anden node. Afsenderen er bevist af
    /// krypteringen; den skal stå på listen over administratorer.
    fn on_control(&mut self, from: Address, counter: u32, frame: &[u8]) {
        let reply = |node: &mut Node, ev: Event| {
            let f = ev.encode();
            let plain = content::control(content::CONTROL_REPLY, counter, &f[2..]);
            let _ = node.send_sealed(from, &plain, Pending::Control);
        };
        if !self.admins.contains(&from) {
            self.log(format!("kontrolkommando fra {} afvist: ikke administrator", short(&from)));
            return reply(self, err_ev(0, error_code::BAD_FRAME, "Du er ikke administrator for denne node"));
        }
        let now = unix_now();
        let last = self.control_seen.get(&from).copied().unwrap_or(0);
        // Tælleren er afsenderens ur: den skal stige og må højst være 10 min gammel.
        if counter <= last || counter.abs_diff(now) > 600 {
            self.log(format!("kontrolkommando fra {} afvist: gammel eller genafspillet", short(&from)));
            return;
        }
        self.control_seen.insert(from, counter);
        match Command::decode(frame) {
            Ok(Command::GetStats) => {
                let items = self.stats();
                for chunk in items.chunks(25) {
                    reply(self, Event::Stats { items: chunk.iter().copied().collect() });
                }
            }
            Ok(Command::GetRadio | Command::SetRadio(_)) => {
                reply(self, err_ev(0, error_code::BAD_FRAME, "maarse-host har ingen egen radio"));
            }
            Ok(Command::SetRole { .. }) => {
                reply(self, err_ev(0, error_code::BAD_FRAME, "maarse-host har altid rollen repeater"));
            }
            _ => reply(self, err_ev(0, error_code::BAD_FRAME, "Kommandoen kan ikke fjernstyres")),
        }
    }

    fn stats(&self) -> Stats {
        let r = self.router.stats;
        let c = &self.counters;
        let clamp = |v: u64| v.min(i32::MAX as u64) as i32;
        let mut items: Vec<(u8, i32)> = vec![
            (stat::UPTIME_S, self.start.elapsed().as_secs() as i32),
            (stat::ROLE, 1),
            (stat::KNOWN_NODES, self.known.len() as i32),
            (stat::ROUTES, self.router.routes().len() as i32),
            (stat::CHANNELS, self.channels.len() as i32),
            (stat::LINKS, self.links.len() as i32),
            (stat::CLIENTS, self.clients.len() as i32),
            (stat::RX_PACKETS, r.rx_packets as i32),
            (stat::RX_INVALID, r.rx_invalid as i32),
            (stat::RX_DUPLICATES, r.rx_duplicates as i32),
            (stat::FORWARDED, r.forwarded as i32),
            (stat::ORIGINATED, r.originated as i32),
            (stat::DELIVERED, r.delivered as i32),
            (stat::ACKED, r.acked as i32),
            (stat::ROUTES_LEARNED, r.routes_learned as i32),
            (stat::PATH_REQUESTS, r.path_requests as i32),
            (stat::HOP_RETRIES, r.hop_retries as i32),
            (stat::FLOODS_CANCELLED, r.floods_cancelled as i32),
            (stat::LINK_PACKETS_IN, clamp(c.packets_in)),
            (stat::LINK_PACKETS_OUT, clamp(c.packets_out)),
            (stat::LINK_BYTES_IN, clamp(c.bytes_in)),
            (stat::LINK_BYTES_OUT, clamp(c.bytes_out)),
            (stat::MAILBOX_RECIPIENTS, self.mailbox.len() as i32),
            (
                stat::MAILBOX_MESSAGES,
                self.mailbox.values().map(|m| m.len()).sum::<usize>() as i32,
            ),
            (stat::OUTBOX_WAITING, self.outbox.len() as i32),
            (stat::DEPOSITS_MADE, c.deposits as i32),
            (stat::MAIL_DELIVERED, c.mail_delivered as i32),
            (stat::FILES_OUT, self.out_files.len() as i32),
            (stat::FILES_IN, self.in_files.len() as i32),
            (stat::ERRORS, c.errors as i32),
            (stat::DHT_CONTACTS, self.dht.table.len() as i32),
            (stat::DHT_RECORDS, self.dht.store.len() as i32),
            (stat::DHT_LOOKUPS, self.dht.lookups as i32),
            (stat::DHT_FOUND, self.dht.found as i32),
            (stat::DHT_PEERS, self.dht.dynamic.len() as i32),
        ];
        if let Some(snr) = c.last_snr {
            items.push((stat::LAST_SNR_DB, snr as i32));
        }
        items.into_iter().collect()
    }
}

fn err_ev(req: u16, code: u8, msg: &str) -> Event {
    Event::Error {
        req,
        code,
        text: text(msg.as_bytes()),
    }
}

// ---------- backbone-DHT ----------

impl Node {
    /// Besvarer en DHT-forespørgsel fra en anden host.
    fn dht_serve(&mut self, data: &[u8]) -> Vec<u8> {
        let me = self.addr;
        let Some((from, from_ep, req)) = dht::decode_req(data) else {
            return dht::encode_resp(&me, &dht::Resp::Stored(false));
        };
        if let Some(ep) = from_ep {
            self.dht.table.insert(Contact { id: from, ep });
        }
        let resp = match req {
            dht::Req::Ping => dht::Resp::Pong,
            dht::Req::FindNode(t) => dht::Resp::Nodes(self.dht_closest(&t)),
            dht::Req::FindValue(t) => match self.dht.store.get(&t) {
                Some(r) => dht::Resp::Value(r),
                None => dht::Resp::Nodes(self.dht_closest(&t)),
            },
            dht::Req::Store(r) => {
                let ok = self.dht.store.put(r);
                dht::Resp::Stored(ok)
            }
        };
        dht::encode_resp(&me, &resp)
    }

    /// De nærmeste kontakter, inklusive os selv, hvis vi er offentlige.
    fn dht_closest(&self, t: &Address) -> Vec<Contact> {
        let mut v = self.dht.table.closest(t, dht::K);
        if let Some(ep) = &self.dht.public_ep {
            v.push(Contact {
                id: self.addr,
                ep: ep.clone(),
            });
            v.sort_by_key(|c| dht::distance(&c.id, t));
            v.truncate(dht::K);
        }
        v
    }

    /// Startliste til et opslag: nærmeste kendte + startpunkter.
    fn dht_start(&self, target: &Address) -> Vec<Contact> {
        let mut v = self.dht.table.closest(target, dht::K);
        if v.len() < dht::K {
            for s in &self.dht.seeds {
                if Some(s) != self.dht.public_ep.as_ref() && !v.iter().any(|c| &c.ep == s) {
                    v.push(Contact {
                        id: Address::default(),
                        ep: s.clone(),
                    });
                }
            }
        }
        v
    }

    /// Kører et opslag i en tråd. `publish`: gem posten hos de nærmeste bagefter.
    fn dht_spawn(&mut self, target: Address, want_value: bool, publish: Option<Record>) {
        let Some(tx) = self.tx.clone() else {
            return;
        };
        let start = self.dht_start(&target);
        if start.is_empty() {
            return;
        }
        let me = self.addr;
        let my_ep = self.dht.public_ep.clone();
        std::thread::spawn(move || {
            let ask = |c: &Contact, r: &dht::Req| dht::rpc(&c.ep, &me, my_ep.as_deref(), r);
            let result = dht::lookup(&me, start, &target, want_value, ask);
            let is_publish = publish.is_some();
            if let Some(rec) = publish {
                for c in &result.closest {
                    let _ = dht::rpc(&c.ep, &me, my_ep.as_deref(), &dht::Req::Store(rec.clone()));
                }
            }
            let _ = tx.send(In::DhtDone {
                target,
                publish: is_publish,
                result,
            });
        });
    }

    fn dht_done(&mut self, target: Address, publish: bool, result: dht::Lookup) {
        for id in &result.dead {
            self.dht.table.remove(id);
        }
        for c in result.alive {
            self.dht.table.insert(c);
        }
        if publish {
            return;
        }
        let waiting = self.dht.waiting.remove(&target).unwrap_or_default();
        if let Some(rec) = result.record {
            self.dht.found += 1;
            self.log(format!(
                "DHT: {} kan nås via {} ({})",
                short(&target),
                short(&rec.gw),
                rec.ep
            ));
            // Announcet er ægte (tjekket i lookup): vi kender nu nøglerne.
            self.on_announce(rec.dest, &rec.announce, rec.hops.saturating_add(1));
            self.dht_connect(&rec);
        }
        for (id, req, msg) in waiting {
            match self.send_sealed(target, &content::text(&msg), Pending::Message) {
                Ok(msg_id) => self.send_client(id, &Event::Sent { req, msg_id }),
                Err(_) => self.send_client(
                    id,
                    &err_ev(req, error_code::UNKNOWN_DEST, "Adressen blev ikke fundet i DHT'en"),
                ),
            }
        }
    }

    fn dht_can_lookup(&self) -> bool {
        self.tx.is_some() && (!self.dht.seeds.is_empty() || self.dht.table.len() > 0)
    }

    /// Forbinder direkte til gatewayen fra en DHT-post.
    fn dht_connect(&mut self, rec: &Record) {
        if rec.gw == self.addr
            || self.dht.public_ep.as_ref() == Some(&rec.ep)
            || self.dht.peers.contains(&rec.ep)
            || self.dht.dynamic.len() >= DHT_MAX_DYNAMIC
        {
            return;
        }
        let Some(tx) = self.tx.clone() else {
            return;
        };
        self.dht.peers.insert(rec.ep.clone());
        self.dht.dynamic.insert(rec.ep.clone());
        self.broadcast_log(format!("Forbinder til gateway {} (fundet i DHT)", rec.ep));
        crate::links::spawn_tcp_peer(rec.ep.clone(), tx);
    }

    /// Slår en ukendt adresse op, hvis vi ikke allerede kender en rute.
    fn dht_maybe_lookup(&mut self, dest: Address) {
        if dest == self.addr
            || self.router.has_route(&dest)
            || self.dht.heard.contains_key(&dest)
            || self.channels.contains_key(&dest)
        {
            return;
        }
        if self
            .dht
            .looked_up
            .get(&dest)
            .is_some_and(|t| t.elapsed() < DHT_LOOKUP_EVERY)
        {
            return;
        }
        self.dht.looked_up.insert(dest, Instant::now());
        self.dht.lookups += 1;
        if let Some(rec) = self.dht.store.get(&dest) {
            // Vi gemmer selv posten.
            let result = dht::Lookup {
                record: Some(rec),
                ..Default::default()
            };
            return self.dht_done(dest, false, result);
        }
        if !self.dht_can_lookup() {
            return;
        }
        self.dht_spawn(dest, true, None);
    }

    /// Bootstrap, udgivelser og oprydning.
    fn dht_maintain(&mut self) {
        self.dht.store.expire();
        self.dht
            .looked_up
            .retain(|_, t| t.elapsed() < DHT_LOOKUP_EVERY * 4);
        // Bootstrap: slå os selv op, så vi lærer vores naboer i ID-rummet.
        let want = self.dht.table.len() < dht::K;
        let due = self
            .dht
            .last_bootstrap
            .is_none_or(|t| t.elapsed() > Duration::from_secs(if want { 15 } else { 600 }));
        if due && !self.dht.seeds.is_empty() || due && self.dht.table.len() > 0 {
            self.dht.last_bootstrap = Some(Instant::now());
            self.dht_spawn(self.addr, false, None);
        }
        // Udgiv de adresser, vi kan nå (kun som offentlig host).
        let Some(ep) = self.dht.public_ep.clone() else {
            return;
        };
        let due: Vec<Address> = self
            .dht
            .heard
            .iter()
            .filter(|(_, (_, _, last))| last.is_none_or(|t| t.elapsed() > dht::REPUBLISH))
            .map(|(a, _)| *a)
            .take(16)
            .collect();
        for dest in due {
            let Some((announce, hops, last)) = self.dht.heard.get_mut(&dest) else {
                continue;
            };
            *last = Some(Instant::now());
            let rec = Record {
                dest,
                gw: self.addr,
                ep: ep.clone(),
                hops: *hops,
                age_s: 0,
                announce: announce.clone(),
            };
            self.dht.store.put(rec.clone());
            self.dht_spawn(dest, false, Some(rec));
        }
    }
}
