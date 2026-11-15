//! The node's main loop: routing, encryption, announces, files, mailboxes and
//! web clients.

mod ext;
mod pages;
mod tun;
mod hist;
mod identity;

use crate::dht::{self, Contact, Record};
use crate::filebox::{self, Boxed, FileBox, LORA_MAX_FILE, PathKind};
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
use maarse_core::packet::{Header, Packet, encode};
use maarse_core::stamp;
use maarse_core::router::MAX_DIRECT_PAYLOAD;
use maarse_core::{Action, Address, PacketType, Role, Router};
use maarse_crypto::bulletin;
use maarse_crypto::ratchet::{self, Format, MAX_FS_TEXT};
use maarse_crypto::seal::{self, CHUNK_DATA, CryptoError};
use maarse_crypto::{
    Identity, PublicIdentity, channel_address, hashtag_key, public_channel_key, verify_announce,
};

/// How often we send an announce.
const ANNOUNCE_EVERY: Duration = Duration::from_secs(600);
/// Messages from unknown senders are kept while we wait for an announce.
const PENDING_MAX: usize = 32;
/// How often files and mailboxes are checked.
const HOUSEKEEPING_EVERY: Duration = Duration::from_secs(2);
/// Chunks of a file that may be in flight at once.
const FILE_WINDOW: usize = 8;
/// A chunk is resent if it is not acknowledged within this time.
const CHUNK_TIMEOUT: Duration = Duration::from_secs(20);
/// A file is abandoned after this time.
const FILE_TIMEOUT: Duration = Duration::from_secs(600);
/// Without an acknowledgement within this time a message is put in the recipient's mailbox.
const DEPOSIT_AFTER: Duration = Duration::from_secs(45);
/// The mailbox keeps at most this many messages per recipient, for this long.
const MAILBOX_PER_RECIPIENT: usize = 100;
const MAILBOX_TTL: Duration = Duration::from_secs(7 * 24 * 3600);
/// How often the DHT is maintained (bootstrap, publications, expiry).
const DHT_EVERY: Duration = Duration::from_secs(5);
/// The same address is looked up at most this often.
const DHT_LOOKUP_EVERY: Duration = Duration::from_secs(30);
/// At most this many gateways are connected to via DHT lookups.
const DHT_MAX_DYNAMIC: usize = 32;
/// Announce field: the mailbox's address.
const APP_POSTBOX: u8 = 0x01;
/// A new FS session is sent at most this often to the same contact (resync).
const RESYNC_EVERY: Duration = Duration::from_secs(60);

enum Timer {
    Tx { packet: Vec<u8>, id: u64 },
    Tick,
    Announce,
    Housekeeping,
    Dht,
}

/// A file that a client is transferring to the node.
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
    /// The file's size in bytes (plaintext).
    size: u32,
    /// The sealed offer as it was sent (for the file mailbox).
    sealed_offer: Vec<u8>,
    /// Sealed chunks from the file mailbox (otherwise they are sealed on the way).
    sealed_chunks: Option<Vec<Vec<u8>>>,
    /// Delivered from the file mailbox.
    from_box: bool,
}

struct InFile {
    from: Address,
    offer: FileOffer,
    parts: Vec<Option<Vec<u8>>>,
    got: u16,
    created: Instant,
}

/// What an acknowledgement belongs to.
enum Pending {
    Message,
    FileOffer(u64),
    FileChunk(u64, u16),
    Deposit(u64),
    MailDelivery(Address),
    /// Control command to a remote node (never put in a mailbox).
    Control,
}

/// Shown in a conversation when a message from that contact could not be
/// decrypted (it arrived too late or the session was lost).
const UNREADABLE_NOTICE: &str =
    "[A message from this contact could not be decrypted. A new encryption session was started; ask them to send it again.]";

/// A sent direct message that may need to go to the mailbox.
struct OutMsg {
    dest: Address,
    /// The message in the static format, for the mailbox. A mailbox can
    /// hold it for days, and a forward-secrecy message that late may fall
    /// outside the recipient's skipped-key window and never open.
    mail_sealed: Vec<u8>,
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
    /// Contacts that advertise forward secrecy in their announce.
    fs_peers: HashSet<Address>,
    /// Forward secrecy sessions (stored in `sessions.bin`).
    sessions: crate::sessions::Sessions,
    /// When we last started a new session because of an unreadable message.
    resynced: HashMap<Address, Instant>,
    postboxes: HashMap<Address, Address>,
    my_postbox: Option<Address>,
    /// Channel address -> (name, key)
    channels: HashMap<Address, (String, [u8; 32])>,
    links: HashMap<LinkId, (String, Sender<Vec<u8>>)>,
    /// Regions that backbone peers have reported (strict, ids). Links without
    /// a hello (older hosts, serial) are not here and get everything.
    link_regions: HashMap<LinkId, (bool, Vec<u16>)>,
    /// Our region names (lower case).
    region_names: Vec<String>,
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
    pages: pages::Pages,
    /// IP tunnel: adapter (if `--tun`), reassembly of pieces and sequence number.
    tun_tx: Option<Sender<Vec<u8>>>,
    tunnel: crate::tunnel::Tunnel,
    /// Allow tunnel traffic over LoRa (`--tun-lora`).
    tun_lora: bool,
    out_files: HashMap<u64, OutFile>,
    in_files: HashMap<u64, InFile>,
    mailbox: HashMap<Address, HashMap<u64, Mail>>,
    max_file: u32,
    // ---------- file limits and file mailbox (see filebox.rs) ----------
    /// The link the packet we are processing came in on.
    rx_link: Option<LinkId>,
    /// Where each address was last heard: link, TCP?, hops.
    heard_via: HashMap<Address, (LinkId, bool, u8)>,
    filebox: FileBox,
    /// The latest sealed message from `send_sealed`.
    last_sealed: Vec<u8>,
    data_dir: PathBuf,
    start: Instant,
    counters: Counters,
    /// Who may administer this node over radio/backbone.
    admins: Vec<Address>,
    /// Last counter in an outgoing control command (must increase strictly).
    last_control: u32,
    /// Latest control counter per administrator (against replay).
    control_seen: HashMap<Address, u32>,
    /// Latest received messages, so a client that connects later also
    /// gets them (e.g. messages fetched from the mailbox right after startup).
    inbox: std::collections::VecDeque<Event>,
    /// For threads (DHT lookups) and new connections.
    tx: Option<Sender<In>>,
    dht: Dht,
    /// Diagnostics, sharing and MQTT (see node/ext.rs).
    ext: ext::Ext,
    /// Message history for several screens (see history.rs and node/hist.rs).
    hist: hist::Hist,
    /// Identities whose signed bulletins we show (`--bulletin-key`).
    bulletin_keys: Vec<PublicIdentity>,
}

/// The backbone DHT's state (see dht.rs).
struct Dht {
    table: dht::Table,
    store: dht::Store,
    /// Our public endpoint; without it we publish nothing.
    public_ep: Option<String>,
    /// Seeds (known only by endpoint).
    seeds: Vec<String>,
    /// Endpoints we already have a packet connection to (or are getting one).
    peers: HashSet<String>,
    /// Gateways we connected to because of a lookup.
    dynamic: HashSet<String>,
    /// Addresses we can reach: latest announce, hops and when we published it.
    heard: HashMap<Address, (Vec<u8>, u8, Option<Instant>)>,
    /// When we last looked up an address (against repeated lookups).
    looked_up: HashMap<Address, Instant>,
    last_bootstrap: Option<Instant>,
    /// Messages to unknown recipients waiting for a lookup.
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

/// For the log: which encryption format a direct message used.
fn format_name(f: Format) -> &'static str {
    match f {
        Format::Static => "static, without forward secrecy",
        Format::Init => "forward secrecy: init",
        Format::Ratchet => "forward secrecy",
    }
}

fn short(a: &Address) -> String {
    a.to_string()[..8].to_string()
}

fn token() -> [u8; 16] {
    let mut t = [0u8; 16];
    OsRng.fill_bytes(&mut t);
    t
}

/// Reads a private channel from `channels.txt`: `!<64 hex chars> <name>`.
fn parse_private_line(line: &str) -> Option<(String, [u8; 32])> {
    let rest = line.strip_prefix('!')?;
    let (hex, name) = rest.split_once(' ').unwrap_or((rest, ""));
    if hex.len() != 64 || !hex.is_ascii() {
        return None;
    }
    let mut key = [0u8; 32];
    for (i, b) in key.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some((name.to_string(), key))
}

/// `public` -> the public channel, everything else -> `#name`.
fn normalize_channel(name: &str) -> (String, [u8; 32]) {
    let n = name.trim().to_lowercase();
    if matches!(
        n.as_str(),
        "public" | "#public" | ""
    ) {
        ("public".into(), public_channel_key())
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

/// File name without paths and odd characters.
fn safe_name(name: &[u8]) -> String {
    let s: String = String::from_utf8_lossy(name)
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | ' '))
        .collect();
    let s = s.trim().trim_start_matches('.').to_string();
    if s.is_empty() { "file".into() } else { s }
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
                identity::write_identity(&cfg.data_dir, &id.to_secret_bytes())?;
                id
            }
        };
        let addr = identity.address();
        let mut router = Router::new(addr, Role::Repeater);
        // Only fast links here: short time slots, otherwise every hop waits needlessly.
        router.slot_ms = 5;

        let mut node = Node {
            identity,
            addr,
            router,
            known: HashMap::new(),
            fs_peers: HashSet::new(),
            sessions: crate::sessions::Sessions::load(cfg.data_dir.join("sessions.bin")),
            resynced: HashMap::new(),
            postboxes: HashMap::new(),
            my_postbox: cfg.postbox,
            channels: HashMap::new(),
            links: HashMap::new(),
            link_regions: HashMap::new(),
            region_names: Vec::new(),
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
            pages: Default::default(),
            tun_tx: None,
            tunnel: Default::default(),
            tun_lora: cfg.tun_lora,
            out_files: HashMap::new(),
            in_files: HashMap::new(),
            mailbox: HashMap::new(),
            max_file: cfg.max_file,
            rx_link: None,
            heard_via: HashMap::new(),
            filebox: FileBox::open(
                cfg.data_dir.join("file-mailbox"),
                cfg.max_file,
                cfg.filebox_per_recipient,
                cfg.filebox_total,
            ),
            last_sealed: Vec::new(),
            data_dir: cfg.data_dir.clone(),
            start: Instant::now(),
            counters: Counters::default(),
            admins: Vec::new(),
            last_control: 0,
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
            ext: ext::Ext::load(
                &cfg.data_dir,
                cfg.mqtt.clone().map(|m| crate::mqtt::Mqtt::spawn(m, format!("maarse-{}", short(&addr)))),
            ),
            hist: hist::Hist::open(&cfg.data_dir, cfg.history_days),
            bulletin_keys: cfg.bulletin_keys.clone(),
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
        node.join_channel("public", false);
        if !node.bulletin_keys.is_empty() {
            node.join_channel(bulletin::CHANNEL_NAME, false);
        }
        for line in saved.lines().filter(|l| !l.trim().is_empty()) {
            if let Some((name, key)) = parse_private_line(line) {
                node.join_private_channel(&name, key, false);
            } else {
                node.join_channel(line, false);
            }
        }
        node.load_mailbox();
        // Regions: the command line wins over the saved ones (regions.txt: one name
        // per line, and optionally the line "!strict").
        if cfg.regions.is_empty() && !cfg.strict_region {
            let saved = fs::read_to_string(node.data_dir.join("regions.txt")).unwrap_or_default();
            let strict = saved.lines().any(|l| l.trim() == "!strict");
            let names: Vec<&str> = saved.lines().filter(|l| !l.trim().starts_with('!')).collect();
            node.set_regions(&names.join(","), strict);
        } else {
            node.set_regions(&cfg.regions.join(","), cfg.strict_region);
        }
        Ok(node)
    }

    pub fn address(&self) -> Address {
        self.addr
    }

    pub fn public_identity(&self) -> PublicIdentity {
        self.identity.public()
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

    // ---------- main loop ----------

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
                self.log(format!("link up: {name}"));
                if name.starts_with("tcp") {
                    let _ = tx.send(self.region_hello());
                    // A backbone link came up (perhaps a split healed):
                    // republish everything we can reach at the next maintenance.
                    for (_, _, last) in self.dht.heard.values_mut() {
                        *last = None;
                    }
                }
                self.links.insert(id, (name, tx));
                self.announce();
                self.broadcast_log(format!("Link up ({} in total)", self.links.len()));
            }
            In::LinkRegions { id, strict, regions } => {
                if let Some((name, _)) = self.links.get(&id) {
                    self.log(format!("{name}: regions {regions:04x?}{}", if strict { " (strict)" } else { "" }));
                }
                self.link_regions.insert(id, (strict, regions));
            }
            In::LinkDown { id } => {
                self.link_regions.remove(&id);
                self.heard_via.retain(|_, v| v.0 != id);
                if let Some((name, _)) = self.links.remove(&id) {
                    self.log(format!("link down: {name}"));
                    self.broadcast_log(format!("Link down ({} left)", self.links.len()));
                }
            }
            In::Packet { data, snr, link } => {
                self.counters.packets_in += 1;
                self.counters.bytes_in += data.len() as u64;
                if self
                    .links
                    .get(&link)
                    .is_some_and(|(n, _)| !n.starts_with("tcp"))
                {
                    // A LoRa modem (USB or Wi-Fi): the SNR is real.
                    self.counters.last_snr = Some(snr);
                }
                // Only route requests with a valid proof of work trigger lookups.
                if let Ok(p) = Packet::decode(&data)
                    && p.header.ptype == PacketType::PathRequest
                    && stamp::verify(p.header.ptype, &p.header.dest, p.payload).is_ok()
                {
                    let h = p.header;
                    self.dht_maybe_lookup(h.dest);
                }
                let now = self.now_ms();
                let actions = self.router.on_receive(&data, snr, now);
                self.rx_link = Some(link);
                self.apply(actions.into_iter().collect());
                self.rx_link = None;
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
            In::TunPacket(p) => self.on_tun_packet(&p),
            In::DhtDone {
                target,
                publish,
                result,
            } => self.dht_done(target, publish, result),
            In::ClientFrame { id, frame } if crate::history::is_history_command(&frame) => {
                self.hist_command(id, &frame)
            }
            In::ClientFrame { id, frame } => match Command::decode(&frame) {
                Ok(cmd) => self.command(id, cmd),
                Err(_) => self.send_client(
                    id,
                    &Event::Error {
                        req: 0,
                        code: error_code::BAD_FRAME,
                        text: text(b"invalid frame"),
                    },
                ),
            },
        }
    }

    // ---------- transmission ----------

    fn send_links(&mut self, packet: &[u8]) {
        let header = Header::decode(packet).ok().map(|(h, _)| h);
        let region = header.as_ref().map_or(0, |h| h.region);
        // File chunks to a recipient reached over TCP are not sent out over
        // a LoRa modem (they would only occupy the channel).
        let tcp_only = header
            .as_ref()
            .is_some_and(|h| h.ptype == PacketType::Fragment && self.path_kind(&h.dest) == PathKind::Tcp);
        for (id, (name, tx)) in &self.links {
            if tcp_only && !name.starts_with("tcp") {
                continue;
            }
            // Floods with a region only to peers that serve the region.
            if region != 0
                && let Some((strict, regions)) = self.link_regions.get(id)
                && !maarse_core::region::forwards(regions, *strict, region)
            {
                continue;
            }
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
                Action::CarryAcked { msg_id } => self.on_carry_acked(msg_id),
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
        let app = self.app_data();
        let Some(payload) = self.identity.announce(unix_now(), &app) else {
            return;
        };
        // Proof of work (about 4,096 attempts, under 1 ms on a PC).
        let mut stamped = payload.clone();
        if !stamp::append(PacketType::Announce, &self.addr, &mut stamped) {
            return;
        }
        let header = Header::new(PacketType::Announce, self.addr);
        if let Ok(pkt) = encode(&header, &stamped) {
            self.send_links(&pkt);
        }
        // Our own address can always be reached here.
        self.dht.heard.insert(self.addr, (payload.to_vec(), 0, None));
    }

    /// Encrypts and sends a direct message. Returns the message's ID.
    fn send_sealed(
        &mut self,
        dest: Address,
        plain: &[u8],
        kind: Pending,
    ) -> Result<u64, &'static str> {
        let recipient = *self
            .known
            .get(&dest)
            .ok_or("The recipient has not been heard yet (no announce)")?;
        let mut ct = [0u8; 255];
        let fs_ok = self.fs_peers.contains(&dest) || self.sessions.contains(&dest);
        let (n, format) =
            ratchet::seal(&self.identity, &recipient, &mut self.sessions, fs_ok, &mut OsRng, plain, &mut ct)
                .map_err(|_| "The message is too long")?;
        self.sessions.save();
        self.last_sealed = ct[..n].to_vec();
        if matches!(kind, Pending::Message) {
            self.log(format!("message to {} ({})", short(&dest), format_name(format)));
        }
        let now = self.now_ms();
        let (msg_id, actions) = self
            .router
            .send_direct(dest, &ct[..n], token(), now)
            .map_err(|_| "The queue is full, try again shortly")?;
        if matches!(kind, Pending::Message) {
            let mail_sealed = if format == Format::Static {
                ct[..n].to_vec()
            } else {
                let mut st = [0u8; 255];
                match ratchet::seal(&self.identity, &recipient, &mut self.sessions, false, &mut OsRng, plain, &mut st) {
                    Ok((m, _)) => st[..m].to_vec(),
                    Err(_) => ct[..n].to_vec(),
                }
            };
            self.outbox.insert(
                msg_id,
                OutMsg {
                    dest,
                    mail_sealed,
                    sent: Instant::now(),
                    deposited: false,
                },
            );
        }
        self.awaiting.insert(msg_id, kind);
        self.apply(actions.into_iter().collect());
        Ok(msg_id)
    }

    /// A message delivered by carrying: like an acknowledgement, but the
    /// app is told "delivered (carried)".
    fn on_carry_acked(&mut self, msg_id: u64) {
        match self.awaiting.get(&msg_id) {
            Some(Pending::Message) | None => {
                self.awaiting.remove(&msg_id);
                self.outbox.remove(&msg_id);
                self.broadcast(&Event::CarryAcked { msg_id });
            }
            Some(_) => self.on_acked(msg_id),
        }
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
                self.log("message put in mailbox");
                self.broadcast(&Event::Deposited { msg_id: orig });
            }
            Some(Pending::MailDelivery(recipient)) => {
                if let Some(m) = self.mailbox.get_mut(&recipient) {
                    m.remove(&msg_id);
                }
                self.counters.mail_delivered += 1;
                self.log(format!("mailbox: delivered to {}", short(&recipient)));
                self.save_mailbox();
            }
        }
    }

    // ---------- reception ----------

    fn on_deliver(&mut self, ptype: PacketType, dest: Address, payload: &[u8], hops: u8) {
        match ptype {
            PacketType::Announce if dest != self.addr => self.on_announce(dest, payload, hops),
            PacketType::Data => self.open_direct(payload.to_vec(), hops, true),
            PacketType::Fragment => self.on_chunk(payload),
            PacketType::Deposit => self.on_deposit(payload),
            PacketType::Datagram => self.on_tun_datagram(payload),
            PacketType::Probe => self.on_trace(dest, payload),
            PacketType::Channel => {
                let Some((_, key)) = self.channels.get(&dest) else {
                    return;
                };
                let mut out = [0u8; 256];
                match seal::open_channel(key, payload, &mut out) {
                    Ok((from, n)) if from != self.addr && dest == bulletin::channel() => {
                        self.on_bulletin(dest, from, hops, &out[..n]);
                    }
                    Ok((from, n)) if from != self.addr => {
                        self.mqtt_channel(&dest, &from, hops, &out[..n]);
                        self.broadcast(&Event::ChannelReceived {
                            channel: dest,
                            from,
                            hops,
                            text: text(&out[..n]),
                        });
                    }
                    Ok(_) => {}
                    Err(e) => self.log(format!("channel message could not be opened: {e:?}")),
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
                if ratchet::announces_fs(a.app_data) {
                    self.fs_peers.insert(dest);
                }
                self.on_app_data(dest, hops, a.app_data);
                if self.known.insert(dest, a.identity).is_none() {
                    self.log(format!("new node heard: {}", short(&dest)));
                    if let Ok(mut f) = fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(self.data_dir.join("known.bin"))
                    {
                        let _ = f.write_all(&a.identity.to_bytes());
                    }
                }
                if let Some(link) = self.rx_link {
                    let tcp = self.links.get(&link).is_some_and(|(n, _)| n.starts_with("tcp"));
                    self.heard_via.insert(dest, (link, tcp, hops));
                }
                self.broadcast(&Event::Heard { addr: dest, hops, identity: Some(a.identity.to_bytes()) });
                if self.dht.public_ep.is_some() {
                    let last = self.dht.heard.get(&dest).and_then(|h| h.2);
                    self.dht.heard.insert(dest, (payload.to_vec(), hops, last));
                }
                self.retry_pending();
                self.flush_mailbox(dest);
            }
            Err(e) => self.log(format!("rejected announce from {}: {e:?}", short(&dest))),
        }
    }

    fn open_direct(&mut self, payload: Vec<u8>, hops: u8, first_time: bool) {
        let mut out = [0u8; 256];
        let known = &self.known;
        let res = ratchet::open(
            &self.identity,
            &payload,
            &mut self.sessions,
            |a| known.get(a).copied(),
            &mut out,
        );
        self.sessions.save();
        match res {
            Ok(o) => {
                if o.format != Format::Static {
                    // It supports forward secrecy, even if we have not heard so.
                    self.fs_peers.insert(o.from);
                }
                self.on_content(o.from, &out[..o.len], hops, o.format)
            }
            Err(CryptoError::UnknownSender(a)) => {
                if first_time {
                    self.log(format!(
                        "message from unknown {}: waiting for announce",
                        short(&a)
                    ));
                }
                if self.pending_rx.len() >= PENDING_MAX {
                    self.pending_rx.remove(0);
                }
                self.pending_rx.push((payload, hops));
            }
            Err(CryptoError::NoSession(a) | CryptoError::Desync(a)) => {
                self.counters.errors += 1;
                self.log(format!(
                    "message from {} could not be read (the session is lost or out of step)",
                    short(&a)
                ));
                // Show it in the conversation too, so the loss is not silent.
                self.broadcast(&Event::DirectReceived { from: a, hops, text: text(UNREADABLE_NOTICE.as_bytes()) });
                self.resync(a);
            }
            Err(e) => {
                self.counters.errors += 1;
                self.log(format!("message could not be opened: {e:?}"));
            }
        }
    }

    /// Starts a new FS session with `peer` and sends an empty resync message,
    /// so the peer switches to it (at most every [`RESYNC_EVERY`]).
    fn resync(&mut self, peer: Address) {
        let Some(id) = self.known.get(&peer).copied() else {
            return;
        };
        if self.resynced.get(&peer).is_some_and(|t| t.elapsed() < RESYNC_EVERY) {
            return;
        }
        self.resynced.insert(peer, Instant::now());
        if ratchet::reset(&self.identity, &id, &mut self.sessions, &mut OsRng).is_ok() {
            self.fs_peers.insert(peer);
            let _ = self.send_sealed(peer, &[content::RESYNC], Pending::Control);
        }
    }

    /// A message on the bulletin channel: shown only if it is signed by a
    /// trusted identity (`--bulletin-key`); everything else is dropped.
    fn on_bulletin(&mut self, channel: Address, from: Address, hops: u8, plain: &[u8]) {
        match bulletin::verify(&from, plain, &self.bulletin_keys) {
            Some(v) => {
                self.log(format!("verified bulletin from {}", short(&v.signer)));
                let mut shown = b"[Verified bulletin] ".to_vec();
                shown.extend_from_slice(v.text);
                self.mqtt_channel(&channel, &from, hops, &shown);
                self.broadcast(&Event::ChannelReceived { channel, from, hops, text: text(&shown) });
            }
            None => self.log(format!("dropped an unverified message on {} from {}", bulletin::CHANNEL_NAME, short(&from))),
        }
    }

    fn on_content(&mut self, from: Address, plain: &[u8], hops: u8, format: Format) {
        match content::parse(plain) {
            Some(Content::PageRequest(path)) => self.on_page_request(from, &path),
            Some(Content::Call(data)) => self.on_call(from, data),
            Some(Content::Ip(data)) => self.on_tun_control(from, data),
            Some(Content::Resync) => {
                self.log(format!("{} started a new encryption session", short(&from)));
            }
            Some(Content::Text(t)) => {
                self.log(format!(
                    "message from {} ({hops} hops, {})",
                    short(&from),
                    format_name(format)
                ));
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
            Some(Content::Unknown(t)) => self.log(format!("unknown content type {t}")),
            None => {}
        }
    }

    fn retry_pending(&mut self) {
        let pending = std::mem::take(&mut self.pending_rx);
        for (p, hops) in pending {
            // If the sender is still unknown, it is put back in the queue.
            self.open_direct(p, hops, false);
        }
    }

    // ---------- files ----------

    fn start_file(&mut self, id: ClientId, req: u16) {
        let Some(up) = self.uploads.remove(&(id, req)) else {
            return self.send_client(
                id,
                &err_ev(req, error_code::NO_FILE, "No file being uploaded"),
            );
        };
        if up.data.len() != up.size as usize {
            return self.send_client(
                id,
                &err_ev(req, error_code::NO_FILE, "The file was not transferred completely"),
            );
        }
        let (dest, size) = (up.dest, up.size);
        let meta = crate::history::file_content(size, &up.mime, &up.name);
        match self.send_file_bytes(dest, up.data, up.mime, up.name) {
            Ok((file_id, chunks)) => {
                self.hist_sent(id, crate::history::KIND_FILE, dest, 0, &meta);
                self.send_client(id, &Event::FileStarted { req, file_id, chunks });
            }
            Err((code, e)) => self.send_client(id, &err_ev(req, code, e)),
        }
    }

    /// Sends `data` as a file to `dest` (offer and then chunks).
    /// Returns (file ID, number of chunks).
    pub(crate) fn send_file_bytes(
        &mut self,
        dest: Address,
        data: Vec<u8>,
        mime: content::Small,
        name: content::Small,
    ) -> Result<(u64, u16), (u8, &'static str)> {
        let size = data.len() as u32;
        let chunks = data.len().div_ceil(CHUNK_DATA).max(1);
        if chunks > u16::MAX as usize {
            return Err((error_code::FILE_TOO_LARGE, "The file is too large"));
        }
        let mut file_id_b = [0u8; 8];
        OsRng.fill_bytes(&mut file_id_b);
        let file_id = u64::from_le_bytes(file_id_b);
        let mut key = [0u8; 32];
        OsRng.fill_bytes(&mut key);
        let digest = Sha256::digest(&data);
        let offer = FileOffer {
            file_id,
            size,
            chunks: chunks as u16,
            chunk_size: CHUNK_DATA as u8,
            key,
            hash: digest[..16].try_into().unwrap_or_default(),
            mime,
            name,
        };
        let plain = offer.encode();
        self.send_sealed(dest, &plain, Pending::FileOffer(file_id))
            .map_err(|e| (error_code::UNKNOWN_DEST, e))?;
        self.out_files.insert(
            file_id,
            OutFile {
                dest,
                data,
                key,
                chunks: chunks as u16,
                offer: plain.to_vec(),
                offer_acked: false,
                offer_sent: Instant::now(),
                acked: vec![false; chunks],
                done: 0,
                in_flight: HashMap::new(),
                created: Instant::now(),
                size,
                sealed_offer: std::mem::take(&mut self.last_sealed),
                sealed_chunks: None,
                from_box: false,
            },
        );
        self.log(format!("sending file ({chunks} chunks) to {}", short(&dest)));
        Ok((file_id, chunks as u16))
    }

    /// Sends more chunks as long as the window allows.
    fn pump_file(&mut self, file_id: u64) {
        loop {
            let Some(f) = self.out_files.get(&file_id) else {
                return;
            };
            if !f.offer_acked || f.in_flight.len() >= FILE_WINDOW {
                return;
            }
            // Over 8 KB only along a route we know is TCP all the way.
            if f.size > LORA_MAX_FILE && self.path_kind(&f.dest) != PathKind::Tcp {
                return;
            }
            let Some(idx) =
                (0..f.chunks).find(|i| !f.acked[*i as usize] && !f.in_flight.contains_key(i))
            else {
                return;
            };
            let mut ct = [0u8; 255];
            let n = if let Some(c) = f.sealed_chunks.as_ref() {
                let c = &c[idx as usize];
                ct[..c.len()].copy_from_slice(c);
                c.len()
            } else {
                let start = idx as usize * CHUNK_DATA;
                let end = (start + CHUNK_DATA).min(f.data.len());
                let Ok(n) = seal::seal_chunk(&f.key, file_id, idx, &f.data[start..end], &mut ct) else {
                    return;
                };
                n
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
                Err(_) => return, // No route right now; retried later.
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
            let boxed = self.out_files.remove(&file_id).is_some_and(|f| f.from_box);
            if boxed {
                self.filebox.remove(file_id);
            }
            self.log(if boxed { "file delivered from the file mailbox" } else { "file delivered" });
            self.broadcast(&Event::FileDone {
                file_id,
                ok: true,
                outgoing: true,
                text: text(if boxed { b"delivered from mailbox" } else { b"delivered" }),
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
                "file from {} rejected (too large or invalid)",
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
            "file on its way from {}: {} bytes",
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
                text: text(b"checksum mismatch"),
            });
            return;
        }
        let dir = self.data_dir.join("files");
        let _ = fs::create_dir_all(&dir);
        let name = format!("{}-{}", short(&f.from), safe_name(&f.offer.name));
        let _ = fs::write(dir.join(&name), &data);
        self.log(format!("file received: {name} ({} bytes)", data.len()));
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

    // ---------- file limits and file mailbox (see filebox.rs) ----------

    /// The route type to `dest` based on where it was last heard.
    fn path_kind(&self, dest: &Address) -> PathKind {
        let heard = self.heard_via.get(dest);
        if heard.is_none() {
            return PathKind::Unknown;
        }
        let in_dht = |a: &Address| self.dht.table.closest(a, 1).first().is_some_and(|c| c.id == *a);
        // Known backbone hosts: direct TCP neighbours (0 hops) and DHT contacts.
        let host_ids: HashSet<u16> = self
            .heard_via
            .iter()
            .filter(|(a, (_, tcp, hops))| *tcp && (*hops == 0 || in_dht(a)))
            .map(|(a, _)| a.path_id())
            .collect();
        let route = self.router.routes().get(dest).map(|r| r.path.clone());
        let dest_is_host = heard.is_some_and(|(_, tcp, hops)| *tcp && *hops == 0) || in_dht(dest);
        filebox::classify(&filebox::PathFacts {
            heard_tcp: heard.map(|h| h.1),
            route: route.as_deref(),
            dest_is_host,
            host_ids: &host_ids,
        })
    }

    fn file_limit(&self, dest: &Address) -> filebox::Limit {
        // The host does not know the modem's radio settings; the EU default
        // (SF9, 125 kHz, 10 % duty cycle) is used to estimate the airtime.
        let radio = maarse_core::companion::RadioConfig::EU_DEFAULT;
        filebox::limit(self.path_kind(dest), self.max_file, self.filebox.enabled(), &radio)
    }

    /// Puts an outgoing file in the file mailbox (sealed offer and chunks).
    fn deposit_file(&mut self, file_id: u64) -> bool {
        let Some(f) = self.out_files.get(&file_id) else {
            return false;
        };
        let mut chunks = Vec::with_capacity(f.chunks as usize);
        for idx in 0..f.chunks {
            let start = idx as usize * CHUNK_DATA;
            let end = (start + CHUNK_DATA).min(f.data.len());
            let mut ct = [0u8; 255];
            let Ok(n) = seal::seal_chunk(&f.key, file_id, idx, &f.data[start..end], &mut ct) else {
                return false;
            };
            chunks.push(ct[..n].to_vec());
        }
        let b = Boxed {
            recipient: f.dest,
            file_id,
            stored: filebox::unix_secs(SystemTime::now()),
            size: f.size,
            offer: f.sealed_offer.clone(),
            chunks,
        };
        match self.filebox.deposit(&b) {
            Ok(()) => {
                self.out_files.remove(&file_id);
                self.counters.deposits += 1;
                self.log(format!(
                    "recipient not answering; file ({} bytes) put in the file mailbox for {}",
                    b.size,
                    short(&b.recipient)
                ));
                self.broadcast(&Event::FileDone {
                    file_id,
                    ok: true,
                    outgoing: true,
                    text: text("put in mailbox".as_bytes()),
                });
                true
            }
            Err(e) => {
                self.log(format!("file could not be put in the file mailbox: {}", e.message()));
                false
            }
        }
    }

    /// Tries to deliver waiting files to `recipient`. Files over 8 KB
    /// wait until the recipient is reached over TCP.
    fn flush_filebox(&mut self, recipient: Address) {
        let waiting = self.filebox.waiting_for(&recipient);
        if waiting.is_empty() || !self.known.contains_key(&recipient) {
            return;
        }
        let kind = self.path_kind(&recipient);
        for file_id in waiting {
            if self.out_files.contains_key(&file_id) {
                continue;
            }
            let Some(b) = self.filebox.load(file_id) else {
                continue;
            };
            if b.size > LORA_MAX_FILE && kind != PathKind::Tcp {
                self.log(format!(
                    "file mailbox: file for {} ({} bytes) waiting for a TCP route",
                    short(&recipient),
                    b.size
                ));
                continue;
            }
            let now = self.now_ms();
            let Ok((msg_id, actions)) = self.router.send_direct(recipient, &b.offer, token(), now) else {
                continue;
            };
            self.log(format!("file mailbox: delivering file ({} bytes) to {}", b.size, short(&recipient)));
            let n = b.chunks.len();
            self.out_files.insert(
                file_id,
                OutFile {
                    dest: recipient,
                    data: Vec::new(),
                    key: [0; 32],
                    chunks: n as u16,
                    offer: Vec::new(),
                    offer_acked: false,
                    offer_sent: Instant::now(),
                    acked: vec![false; n],
                    done: 0,
                    in_flight: HashMap::new(),
                    created: Instant::now(),
                    size: b.size,
                    sealed_offer: b.offer,
                    sealed_chunks: Some(b.chunks),
                    from_box: true,
                },
            );
            self.awaiting.insert(msg_id, Pending::FileOffer(file_id));
            self.apply(actions.into_iter().collect());
        }
    }

    // ---------- mailbox ----------

    fn on_deposit(&mut self, payload: &[u8]) {
        let Some(recipient) = Address::from_slice(payload) else {
            return;
        };
        let sealed = payload[16..].to_vec();
        let id = maarse_core::dedup::packet_id(PacketType::Data, &recipient, &sealed);
        let box_ = self.mailbox.entry(recipient).or_default();
        if box_.len() >= MAILBOX_PER_RECIPIENT {
            self.log(format!("mailbox full for {}", short(&recipient)));
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
            "mailbox: stored message for {} ({count} in total)",
            short(&recipient)
        ));
        self.save_mailbox();
        // Maybe the recipient is already here.
        self.flush_mailbox(recipient);
    }

    /// Tries to deliver waiting mail to `recipient`.
    fn flush_mailbox(&mut self, recipient: Address) {
        self.flush_filebox(recipient);
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
            "mailbox: trying to deliver {} message(s) to {}",
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
        if postbox == m.dest || 16 + m.mail_sealed.len() > MAX_DIRECT_PAYLOAD {
            return;
        }
        m.deposited = true;
        let mut payload = m.dest.0.to_vec();
        payload.extend_from_slice(&m.mail_sealed);
        let now = self.now_ms();
        if let Ok((dep_id, actions)) =
            self.router
                .send_typed(PacketType::Deposit, postbox, &payload, token(), now)
        {
            self.log(format!(
                "recipient not answering; putting message in mailbox {}",
                short(&postbox)
            ));
            self.awaiting.insert(dep_id, Pending::Deposit(msg_id));
            self.apply(actions.into_iter().collect());
        }
    }

    fn save_mailbox(&self) {
        // Format: recipient (16) | stored (u64 sec.) | length (u16) | message
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
        let _ = fs::write(self.data_dir.join("mailbox.bin"), b);
    }

    fn load_mailbox(&mut self) {
        let Ok(b) = fs::read(self.data_dir.join("mailbox.bin")) else {
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

    // ---------- maintenance ----------

    fn housekeeping(&mut self) {
        self.tun_tick();
        self.hist_maintain();
        let now = Instant::now();
        // Messages without an acknowledgement: put them in the recipient's mailbox.
        // MAARSE_DEPOSIT_AFTER_S makes it possible to test the mailbox quickly.
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

        // Files: resend chunks without an acknowledgement. If the recipient does not answer,
        // the file is put in the file mailbox (if enabled), otherwise it is abandoned.
        let ids: Vec<u64> = self.out_files.keys().copied().collect();
        for file_id in ids {
            let Some(f) = self.out_files.get(&file_id) else {
                continue;
            };
            let age = now.duration_since(f.created);
            let late = !f.offer_acked && age > after;
            if !f.from_box && self.filebox.enabled() && (late || age > FILE_TIMEOUT) && self.deposit_file(file_id) {
                continue;
            }
            if age > FILE_TIMEOUT {
                // From the file mailbox: the file stays and is tried again
                // the next time the recipient shows up.
                if self.out_files.remove(&file_id).is_some_and(|f| f.from_box) {
                    continue;
                }
                self.broadcast(&Event::FileDone {
                    file_id,
                    ok: false,
                    outgoing: true,
                    text: text(b"the recipient did not answer"),
                });
                continue;
            }
            let mut resend_offer = None;
            if let Some(f) = self.out_files.get_mut(&file_id) {
                f.in_flight
                    .retain(|_, sent| now.duration_since(*sent) < CHUNK_TIMEOUT);
                if !f.offer_acked && now.duration_since(f.offer_sent) > CHUNK_TIMEOUT {
                    f.offer_sent = now;
                    resend_offer = Some((f.dest, f.offer.clone(), f.from_box, f.sealed_offer.clone()));
                }
            }
            match resend_offer {
                Some((dest, _, true, sealed)) => {
                    let t = self.now_ms();
                    if let Ok((msg_id, actions)) = self.router.send_direct(dest, &sealed, token(), t) {
                        self.awaiting.insert(msg_id, Pending::FileOffer(file_id));
                        self.apply(actions.into_iter().collect());
                    }
                }
                Some((dest, offer, false, _)) => {
                    if self.send_sealed(dest, &offer, Pending::FileOffer(file_id)).is_ok() {
                        let sealed = std::mem::take(&mut self.last_sealed);
                        if let Some(f) = self.out_files.get_mut(&file_id) {
                            f.sealed_offer = sealed;
                        }
                    }
                }
                None => {}
            }
            self.pump_file(file_id);
        }
        // Old files in the file mailbox.
        for (r, id) in self.filebox.expire(SystemTime::now()) {
            self.log(format!("file mailbox: file {id:016x} for {} expired after 7 days", short(&r)));
        }
        // Half-finished incoming files.
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
                text: text(b"transfer aborted"),
            });
        }
        // Old mail.
        let before = self.mailbox.values().map(|m| m.len()).sum::<usize>();
        for mails in self.mailbox.values_mut() {
            mails.retain(|_, m| m.stored.elapsed().map(|e| e < MAILBOX_TTL).unwrap_or(true));
        }
        self.mailbox.retain(|_, m| !m.is_empty());
        if self.mailbox.values().map(|m| m.len()).sum::<usize>() != before {
            self.save_mailbox();
        }
    }

    // ---------- web clients ----------

    fn send_client(&self, id: ClientId, ev: &Event) {
        if let Some(tx) = self.clients.get(&id) {
            let _ = tx.send(ev.encode().to_vec());
        }
    }

    fn broadcast(&mut self, ev: &Event) {
        self.hist_observe(ev);
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
        for (a, pk) in &self.known {
            self.send_client(id, &Event::Heard { addr: *a, hops: 0, identity: Some(pk.to_bytes()) });
        }
        self.hist_greet(id);
        for ev in &self.inbox {
            self.send_client(id, ev);
        }
        let msg = format!(
            "Connected to maarse-host. {} link(s) up.",
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
            max_text: (MAX_FS_TEXT - 1) as u8,
            max_file: self.max_file,
            identity: Some(self.identity.public().to_bytes()),
        }
    }

    fn region_hello(&self) -> Vec<u8> {
        crate::links::region_hello(self.router.strict_region, &self.router.regions)
    }

    /// Sets regions (comma-separated) and tells backbone peers.
    fn set_regions(&mut self, names: &str, strict: bool) {
        self.region_names = maarse_core::region::names(names)
            .map(str::to_lowercase)
            .take(maarse_core::region::MAX_REGIONS)
            .collect();
        self.router.regions = maarse_core::region::parse_list(&self.region_names.join(","));
        self.router.strict_region = strict;
        let hello = self.region_hello();
        for (name, tx) in self.links.values() {
            if name.starts_with("tcp") {
                let _ = tx.send(hello.clone());
            }
        }
    }

    fn regions_event(&self) -> Event {
        let names = self.region_names.join(",");
        Event::Regions {
            strict: self.router.strict_region,
            names: text(&names.as_bytes()[..names.len().min(MAX_TEXT)]),
        }
    }

    fn join_channel(&mut self, name: &str, save: bool) -> (Address, String) {
        // `name@region`: the region tags messages we send to the channel.
        let (base, region) = maarse_core::region::split_channel(name);
        let (mut name, key) = normalize_channel(base);
        if !region.is_empty() {
            name = format!("{name}@{}", region.to_lowercase());
        }
        self.join_key(name, key, save)
    }

    /// Private channel with a random key from an invitation.
    fn join_private_channel(&mut self, name: &str, key: [u8; 32], save: bool) -> (Address, String) {
        let mut name: String = name.trim().chars().filter(|c| !c.is_control()).collect();
        if name.is_empty() {
            name = "private".into();
        }
        self.join_key(name, key, save)
    }

    fn join_key(&mut self, name: String, key: [u8; 32], save: bool) -> (Address, String) {
        let ch = channel_address(&key);
        let _ = self.router.subscribe(ch);
        let old = self.channels.insert(ch, (name.clone(), key));
        let new = old.is_none_or(|(n, _)| n != name);
        if save && new {
            // One line per channel: the name for public/hashtag channels,
            // `!<key in hex> <name>` for private ones.
            let lines: Vec<String> = self
                .channels
                .values()
                .filter(|(_, k)| *k != public_channel_key())
                .map(|(n, k)| {
                    if normalize_channel(n).1 == *k {
                        n.clone()
                    } else {
                        let hex: String = k.iter().map(|b| format!("{b:02x}")).collect();
                        format!("!{hex} {n}")
                    }
                })
                .collect();
            let _ = fs::write(self.data_dir.join("channels.txt"), lines.join("\n"));
        }
        (ch, name)
    }

    fn command(&mut self, id: ClientId, cmd: Command) {
        let Some(cmd) = self.ext_command(id, cmd) else { return };
        let Some(cmd) = self.page_command(id, cmd) else { return };
        match cmd {
            Command::GetInfo => {
                self.send_client(id, &self.info());
                self.send_known_names(id);
            }
            Command::AnnounceNow => {
                self.announce();
                self.send_client(id, &Event::Log { text: text(b"Announce sent") });
            }
            Command::JoinChannel { name } => {
                let name = String::from_utf8_lossy(&name).to_string();
                let (ch, name) = self.join_channel(&name, true);
                self.broadcast(&Event::ChannelJoined {
                    channel: ch,
                    name: text(name.as_bytes()),
                });
            }
            Command::JoinPrivateChannel { key, name } => {
                let name = String::from_utf8_lossy(&name).to_string();
                let (ch, name) = self.join_private_channel(&name, key, true);
                self.broadcast(&Event::ChannelJoined {
                    channel: ch,
                    name: text(name.as_bytes()),
                });
            }
            Command::SendDirect { req, dest, text: msg } => {
                if msg.len() > MAX_FS_TEXT - 1 {
                    return self.send_client(id, &err_ev(req, error_code::TOO_LARGE, "The message is too long"));
                }
                if !self.known.contains_key(&dest) && self.dht_can_lookup() {
                    // Unknown recipient: look it up in the backbone DHT and send afterwards.
                    self.dht.waiting.entry(dest).or_default().push((id, req, msg.to_vec()));
                    self.dht.looked_up.remove(&dest);
                    self.dht_maybe_lookup(dest);
                    let t = format!("Looking up {} in the DHT …", short(&dest));
                    return self.send_client(id, &Event::Log { text: text(t.as_bytes()) });
                }
                match self.send_sealed(dest, &content::text(&msg), Pending::Message) {
                    Ok(msg_id) => {
                        self.send_client(id, &Event::Sent { req, msg_id });
                        self.hist_sent(id, crate::history::KIND_DIRECT, dest, msg_id, &msg);
                    }
                    Err(e) => self.send_client(id, &err_ev(req, error_code::UNKNOWN_DEST, e)),
                }
            }
            Command::SendChannel { req, channel, text: msg } => {
                let Some((chname, key)) = self.channels.get(&channel).cloned() else {
                    return self.send_client(id, &err_ev(req, error_code::NOT_IN_CHANNEL, "Not a member of the channel"));
                };
                let region = maarse_core::region::region_id(maarse_core::region::split_channel(&chname).1);
                let mut nonce = [0u8; 8];
                OsRng.fill_bytes(&mut nonce);
                let mut ct = [0u8; 255];
                // On the bulletin channel every message is signed (see bulletin.rs).
                let mut signed = [0u8; 255];
                let body: &[u8] = if channel == bulletin::channel() {
                    let ts = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as u32);
                    match bulletin::sign(&self.identity, ts, &msg, &mut signed) {
                        Some(n) => &signed[..n],
                        None => {
                            return self.send_client(id, &err_ev(req, error_code::TOO_LARGE, "The message is too long"));
                        }
                    }
                } else {
                    &msg
                };
                let res = seal::seal_channel(&key, &self.addr, nonce, body, &mut ct)
                    .ok()
                    .and_then(|n| self.router.send_channel_in(channel, region, &ct[..n]).ok());
                match res {
                    Some(actions) => {
                        self.send_client(id, &Event::Sent { req, msg_id: 0 });
                        self.hist_sent(id, crate::history::KIND_CHANNEL, channel, 0, &msg);
                        self.apply(actions.into_iter().collect());
                    }
                    None => self.send_client(id, &err_ev(req, error_code::TOO_LARGE, "The message is too long")),
                }
            }
            Command::GetFileLimit { dest } => {
                let l = self.file_limit(&dest);
                self.send_client(
                    id,
                    &Event::FileLimit {
                        dest,
                        max_bytes: l.max_bytes,
                        via_lora: l.via_lora,
                        mailbox: l.mailbox,
                        est_airtime_s: l.est_airtime_s,
                    },
                );
            }
            Command::FileBegin { req, dest, size, mime, name } => {
                let l = self.file_limit(&dest);
                if size > l.max_bytes {
                    let m = if l.via_lora && !l.mailbox {
                        format!(
                            "The file is too large: the recipient is only reached over LoRa, where the limit is {} KB",
                            l.max_bytes / 1024
                        )
                    } else {
                        format!("The file is too large (at most {} KB)", l.max_bytes / 1024)
                    };
                    return self.send_client(id, &err_ev(req, error_code::FILE_TOO_LARGE, &m));
                }
                if !self.known.contains_key(&dest) {
                    return self.send_client(
                        id,
                        &err_ev(req, error_code::UNKNOWN_DEST, "The recipient has not been heard yet (no announce)"),
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
                    Ok(Command::GetStats
                        | Command::GetRadio
                        | Command::SetRadio(_)
                        | Command::SetRole { .. }
                        | Command::OtaBegin { .. }
                        | Command::OtaChunk { .. }
                        | Command::OtaEnd { .. }
                        | Command::OtaStatus)
                );
                if !ok {
                    return self.send_client(id, &err_ev(req, error_code::BAD_FRAME, "Only status, radio, role and firmware can be controlled remotely"));
                }
                // The counter follows the clock, so it also increases after a restart,
                // but always increases (firmware chunks can come several per second).
                let counter = unix_now().max(self.last_control.wrapping_add(1));
                self.last_control = counter;
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
                self.log(format!("{} administrator(s) saved", self.admins.len()));
                self.send_client(id, &self.admins_event());
            }
            Command::GetAdmins => self.send_client(id, &self.admins_event()),
            Command::SetRegions { strict, names } => {
                let names = String::from_utf8_lossy(&names).to_string();
                self.set_regions(&names, strict);
                let mut lines = self.region_names.clone();
                if strict {
                    lines.push("!strict".into());
                }
                let _ = fs::write(self.data_dir.join("regions.txt"), lines.join("\n"));
                self.log(format!("regions: {:?}{}", self.region_names, if strict { " (strict)" } else { "" }));
                self.send_client(id, &self.regions_event());
            }
            Command::GetRegions => self.send_client(id, &self.regions_event()),
            Command::OtaBegin { req, .. } | Command::OtaChunk { req, .. } | Command::OtaEnd { req, .. } => {
                self.send_client(id, &ota_unsupported(req))
            }
            Command::OtaStatus => self.send_client(id, &ota_unsupported(0)),
            Command::SetIdentity { confirm, secret } => self.set_identity(id, confirm, secret),

            Command::SetTime { .. } => {} // maarse-host has its own clock
            Command::SetWifi(_) | Command::GetWifi => self.send_client(
                id,
                &err_ev(0, error_code::BAD_FRAME, "maarse-host uses the computer's network; Wi-Fi settings are for ESP32 nodes."),
            ),
            Command::SetRangeTest(_) | Command::GetRangeTest => self.send_client(
                id,
                &err_ev(
                    0,
                    error_code::BAD_FRAME,
                    "The range test runs on the radio node itself. Connect to it via USB or Bluetooth.",
                ),
            ),
            Command::GetRadio | Command::SetRadio(_) => self.send_client(
                id,
                &err_ev(
                    0,
                    error_code::BAD_FRAME,
                    "maarse-host has no radio of its own. Connect to the node via USB to change radio settings.",
                ),
            ),
            Command::SetRole { .. } => self.send_client(
                id,
                &err_ev(
                    0,
                    error_code::BAD_FRAME,
                    "maarse-host is a gateway and always has the repeater role. Connect to the node via USB to choose a role.",
                ),
            ),
            Command::GetStats => {
                let ev = Event::Stats { items: self.stats() };
                self.send_client(id, &ev);
            }
            // Handled in node/ext.rs.
            Command::TraceRoute { .. }
            | Command::GetNeighbors
            | Command::SetSharing(_)
            | Command::GetSharing
            | Command::SetName { .. }
            | Command::GetName
            | Command::RequestPage { .. }
            | Command::SendCall { .. } => {}
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

    /// A control command from another node. The sender is proven by
    /// the encryption; it must be on the list of administrators.
    fn on_control(&mut self, from: Address, counter: u32, frame: &[u8]) {
        let reply = |node: &mut Node, ev: Event| {
            let f = ev.encode();
            let plain = content::control(content::CONTROL_REPLY, counter, &f[2..]);
            let _ = node.send_sealed(from, &plain, Pending::Control);
        };
        if !self.admins.contains(&from) {
            self.log(format!("control command from {} rejected: not an administrator", short(&from)));
            return reply(self, err_ev(0, error_code::BAD_FRAME, "You are not an administrator of this node"));
        }
        let now = unix_now();
        let last = self.control_seen.get(&from).copied().unwrap_or(0);
        // The counter is the sender's clock: it must increase and be at most 10 min old.
        if counter <= last || counter.abs_diff(now) > 600 {
            self.log(format!("control command from {} rejected: old or replayed", short(&from)));
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
                reply(self, err_ev(0, error_code::BAD_FRAME, "maarse-host has no radio of its own"));
            }
            Ok(Command::SetRole { .. }) => {
                reply(self, err_ev(0, error_code::BAD_FRAME, "maarse-host always has the repeater role"));
            }
            _ => reply(self, err_ev(0, error_code::BAD_FRAME, "The command cannot be controlled remotely")),
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
            (stat::BAD_STAMPS, r.bad_stamps as i32),
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
        self.ext_stats(&mut items);
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

// ---------- backbone DHT ----------

impl Node {
    /// Answers a DHT request from another host.
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

    /// The closest contacts, including ourselves if we are public.
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

    /// Start list for a lookup: closest known + seeds.
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

    /// Runs a lookup in a thread. `publish`: store the record at the closest nodes afterwards.
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
        // Small backbone: store on every known host (see dht.rs "Full replication").
        let everyone = if publish.is_some() && self.dht.table.len() <= dht::FULL_REPLICA_MAX {
            self.dht.table.all()
        } else {
            Vec::new()
        };
        std::thread::spawn(move || {
            let ask = |c: &Contact, r: &dht::Req| dht::rpc(&c.ep, &me, my_ep.as_deref(), r);
            let result = dht::lookup(&me, start, &target, want_value, ask);
            let is_publish = publish.is_some();
            if let Some(rec) = publish {
                let mut done: HashSet<String> = HashSet::new();
                for c in result.closest.iter().chain(everyone.iter()) {
                    if done.insert(c.ep.clone()) {
                        let _ = dht::rpc(&c.ep, &me, my_ep.as_deref(), &dht::Req::Store(rec.clone()));
                    }
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
                "DHT: {} can be reached via {} ({})",
                short(&target),
                short(&rec.gw),
                rec.ep
            ));
            // The announce is genuine (checked in lookup): we now know the keys.
            self.on_announce(rec.dest, &rec.announce, rec.hops.saturating_add(1));
            self.dht_connect(&rec);
        }
        for (id, req, msg) in waiting {
            match self.send_sealed(target, &content::text(&msg), Pending::Message) {
                Ok(msg_id) => {
                    self.send_client(id, &Event::Sent { req, msg_id });
                    self.hist_sent(id, crate::history::KIND_DIRECT, target, msg_id, &msg);
                }
                Err(_) => self.send_client(
                    id,
                    &err_ev(req, error_code::UNKNOWN_DEST, "The address was not found in the DHT"),
                ),
            }
        }
    }

    fn dht_can_lookup(&self) -> bool {
        self.tx.is_some() && (!self.dht.seeds.is_empty() || self.dht.table.len() > 0)
    }

    /// Connects directly to the gateway from a DHT record.
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
        self.broadcast_log(format!("Connecting to gateway {} (found in the DHT)", rec.ep));
        crate::links::spawn_tcp_peer(rec.ep.clone(), tx);
    }

    /// Looks up an unknown address if we do not already know a route.
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
            // We store the record ourselves.
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

    /// Bootstrap, publications and cleanup.
    fn dht_maintain(&mut self) {
        self.dht.store.expire();
        self.dht
            .looked_up
            .retain(|_, t| t.elapsed() < DHT_LOOKUP_EVERY * 4);
        // Bootstrap: look ourselves up so we learn our neighbours in the ID space.
        let want = self.dht.table.len() < dht::K;
        let due = self
            .dht
            .last_bootstrap
            .is_none_or(|t| t.elapsed() > Duration::from_secs(if want { 15 } else { 600 }));
        if due && !self.dht.seeds.is_empty() || due && self.dht.table.len() > 0 {
            self.dht.last_bootstrap = Some(Instant::now());
            self.dht_spawn(self.addr, false, None);
        }
        // Publish the addresses we can reach (only as a public host).
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

#[cfg(test)]
pub(super) mod file_interop_tests {
    //! The firmware's file sender (`maarse_crypto::files`) against the host's receiver.
    use super::*;
    use maarse_crypto::files::{AckResult, Outgoing};

    pub(super) fn cfg(dir: PathBuf) -> Config {
        Config {
            data_dir: dir,
            http: None,
            listen: None,
            peers: Vec::new(),
            serial: None,
            baud: 115_200,
            open_browser: false,
            max_file: 1024 * 1024,
            filebox_per_recipient: 20 << 20,
            filebox_total: 200 << 20,
            postbox: None,
            public_addr: None,
            dht_seeds: Vec::new(),
            regions: Vec::new(),
            strict_region: false,
            meshcore: None,
            meshcore_baud: 115_200,
            meshcore_bridge: Vec::new(),
            mqtt: None,
            history_days: None,
            tun: false,
            wintun: None,
            tun_lora: false,
            service: false,
            bulletin_keys: Vec::new(),
        }
    }

    #[test]
    fn firmware_sender_to_host_receiver() {
        let dir = std::env::temp_dir().join(format!("maarse-file-{}", std::process::id()));
        let mut node = Node::new(&cfg(dir.clone())).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        node.clients.insert(1, tx);

        let data: Vec<u8> = (0..8000u32).map(|i| (i * 7 % 256) as u8).collect();
        let from = Address([4; 16]);
        let (mut out, offer) =
            Outgoing::new(77, [9; 32], node.addr, &data, b"image/webp", b"map.webp", 0).unwrap();
        // The offer goes the same way as over radio: encoded and parsed.
        let enc = offer.encode();
        let Some(Content::FileOffer(parsed)) = content::parse(&enc) else { panic!() };
        node.on_file_offer(from, parsed);
        out.offer_sent(1, 0);
        assert_eq!(out.on_ack(1, 0), Some(AckResult::OfferAcked));
        let mut msg = 1;
        while !out.finished() {
            while let Some(i) = out.next_chunk() {
                msg += 1;
                let mut ct = [0u8; 255];
                let n = out.seal_chunk(i, &data, &mut ct).unwrap();
                out.chunk_sent(i, msg, 0);
                node.on_chunk(&ct[..n]);
                out.on_ack(msg, 0);
            }
        }
        let mut got = Vec::new();
        let mut done = None;
        for f in rx.try_iter() {
            match Event::decode(&f[2..]).unwrap() {
                Event::FileChunk { file_id: 77, offset, data } => {
                    assert_eq!(offset as usize, got.len());
                    got.extend_from_slice(&data);
                }
                Event::FileDone { file_id: 77, ok, .. } => done = Some(ok),
                _ => {}
            }
        }
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(done, Some(true));
        assert_eq!(got, data);
    }
}

/// maarse-host cannot itself be updated over the air (it is updated as an
/// ordinary program), but forwards firmware to remote nodes with `Remote`.
fn ota_unsupported(req: u16) -> Event {
    Event::OtaState {
        req,
        state: maarse_core::companion::ota_state::UNSUPPORTED,
        next: 0,
        size: 0,
        version: 0,
        current: 0,
        text: maarse_core::companion::text("maarse-host is not updated over the air; use Remote for a node".as_bytes()),
    }
}

#[cfg(test)]
mod filebox_node_tests {
    //! File limits by route and the file mailbox (see filebox.rs).
    use super::*;
    use std::sync::mpsc::{Receiver, channel};

    fn node(name: &str) -> (Node, Receiver<Vec<u8>>, PathBuf) {
        let dir = std::env::temp_dir().join(format!("maarse-fb-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut c = file_interop_tests::cfg(dir.clone());
        c.data_dir = dir.clone();
        let mut n = Node::new(&c).unwrap();
        let (tx, rx) = channel();
        n.clients.insert(1, tx);
        (n, rx, dir)
    }

    fn events(rx: &Receiver<Vec<u8>>) -> Vec<Event> {
        rx.try_iter().filter_map(|f| Event::decode(&f[2..]).ok()).collect()
    }

    fn know(a: &mut Node, b: &Node) {
        a.known.insert(b.addr, b.identity.public());
    }

    /// `peer` is heard with `hops` hops over a link named `link`.
    fn heard(n: &mut Node, peer: Address, link: &str, hops: u8) {
        let (tx, _rx) = channel();
        let id = n.links.len() as LinkId + 1;
        n.links.insert(id, (link.to_string(), tx));
        n.heard_via.insert(peer, (id, link.starts_with("tcp"), hops));
    }

    fn begin(n: &mut Node, req: u16, dest: Address, size: u32) {
        n.command(1, Command::FileBegin { req, dest, size, mime: text(b"image/webp"), name: text(b"b.webp") });
    }

    fn upload(n: &mut Node, req: u16, dest: Address, data: &[u8]) {
        begin(n, req, dest, data.len() as u32);
        for (i, c) in data.chunks(MAX_TEXT).enumerate() {
            n.command(1, Command::FileData { req, offset: (i * MAX_TEXT) as u32, data: text(c) });
        }
        n.command(1, Command::FileEnd { req });
    }

    #[test]
    fn path_kinds_from_links() {
        let (mut n, _rx, dir) = node("kinds");
        let (tcp, lora, radio_behind_host, none) =
            (Address([1; 16]), Address([2; 16]), Address([3; 16]), Address([4; 16]));
        heard(&mut n, tcp, "tcp out 127.0.0.1:4242", 0);
        heard(&mut n, lora, "serial COM3", 0);
        heard(&mut n, radio_behind_host, "tcp in 10.0.0.2:5000", 1);
        assert_eq!(n.path_kind(&tcp), PathKind::Tcp);
        assert_eq!(n.path_kind(&lora), PathKind::Lora);
        assert_eq!(n.path_kind(&radio_behind_host), PathKind::Lora);
        assert_eq!(n.path_kind(&none), PathKind::Unknown);
        // The link goes down: then we no longer know where the recipient is.
        let id = n.heard_via[&tcp].0;
        n.handle(In::LinkDown { id });
        assert_eq!(n.path_kind(&tcp), PathKind::Unknown);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn file_limit_and_file_begin_by_path() {
        let (mut a, rx, dir) = node("limit");
        let (b, _rxb, dir_b) = node("limit-b");
        know(&mut a, &b);
        heard(&mut a, b.addr, "serial COM3", 1);
        a.command(1, Command::GetFileLimit { dest: b.addr });
        let ev = events(&rx);
        let Some(Event::FileLimit { max_bytes, via_lora, mailbox, est_airtime_s, .. }) =
            ev.iter().find(|e| matches!(e, Event::FileLimit { .. })).cloned()
        else {
            panic!("{ev:?}")
        };
        assert_eq!((max_bytes, via_lora, mailbox), (8192, true, false));
        assert!(est_airtime_s > 0);
        // Too large for LoRa: rejected with an explanation.
        begin(&mut a, 7, b.addr, 20_000);
        let ev = events(&rx);
        let err = ev.iter().find_map(|e| match e {
            Event::Error { req: 7, code, text } => Some((*code, String::from_utf8_lossy(text).to_string())),
            _ => None,
        });
        let (code, msg) = err.expect("no error");
        assert_eq!(code, error_code::FILE_TOO_LARGE);
        assert!(msg.contains("LoRa") && msg.contains("8 KB"), "{msg}");
        // 8 KB is fine.
        begin(&mut a, 8, b.addr, 8192);
        assert!(!events(&rx).iter().any(|e| matches!(e, Event::Error { .. })));
        // Over TCP: up to 1 MB.
        heard(&mut a, b.addr, "tcp out 127.0.0.1:4242", 0);
        begin(&mut a, 9, b.addr, 1024 * 1024);
        assert!(!events(&rx).iter().any(|e| matches!(e, Event::Error { .. })));
        begin(&mut a, 10, b.addr, 1024 * 1024 + 1);
        assert!(events(&rx).iter().any(|e| matches!(e, Event::Error { req: 10, .. })));
        let _ = fs::remove_dir_all(dir);
        let _ = fs::remove_dir_all(dir_b);
    }

    #[test]
    fn large_file_not_sent_over_lora() {
        let (mut a, rx, dir) = node("gate");
        let (b, _rxb, dir_b) = node("gate-b");
        know(&mut a, &b);
        heard(&mut a, b.addr, "tcp out 127.0.0.1:4242", 0);
        upload(&mut a, 1, b.addr, &vec![5u8; 20_000]);
        let file_id = events(&rx)
            .iter()
            .find_map(|e| match e {
                Event::FileStarted { file_id, .. } => Some(*file_id),
                _ => None,
            })
            .unwrap();
        // The route switches to LoRa before the offer is acknowledged.
        heard(&mut a, b.addr, "serial COM3", 0);
        a.out_files.get_mut(&file_id).unwrap().offer_acked = true;
        a.pump_file(file_id);
        assert!(a.out_files[&file_id].in_flight.is_empty(), "chunks sent over LoRa");
        let _ = fs::remove_dir_all(dir);
        let _ = fs::remove_dir_all(dir_b);
    }

    #[test]
    fn deposit_then_deliver_over_tcp() {
        let (mut a, rx, dir) = node("dep");
        let (mut b, rxb, dir_b) = node("dep-b");
        know(&mut a, &b);
        know(&mut b, &a);
        // B is offline (no knowledge of the route): 1 MB via the mailbox.
        a.command(1, Command::GetFileLimit { dest: b.addr });
        assert!(events(&rx).iter().any(|e| matches!(
            e,
            Event::FileLimit { max_bytes: 1048576, mailbox: true, .. }
        )));
        let data: Vec<u8> = (0..20_000u32).map(|i| (i * 13 % 251) as u8).collect();
        upload(&mut a, 1, b.addr, &data);
        let file_id = events(&rx)
            .iter()
            .find_map(|e| match e {
                Event::FileStarted { file_id, .. } => Some(*file_id),
                _ => None,
            })
            .unwrap();
        assert!(a.deposit_file(file_id));
        assert!(events(&rx).iter().any(|e| matches!(
            e,
            Event::FileDone { ok: true, outgoing: true, text, .. } if &text[..] == "put in mailbox".as_bytes()
        )));
        assert_eq!(a.filebox.waiting_for(&b.addr), vec![file_id]);
        // B comes back, but only over LoRa: the file (20 KB) stays.
        heard(&mut a, b.addr, "serial COM3", 0);
        a.flush_filebox(b.addr);
        assert!(!a.out_files.contains_key(&file_id));
        // B is back over TCP: the offer and chunks are sent from the mailbox.
        heard(&mut a, b.addr, "tcp out 127.0.0.1:4242", 0);
        a.flush_filebox(b.addr);
        assert!(a.out_files[&file_id].from_box);
        let boxed = a.filebox.load(file_id).unwrap();
        // B can open the offer and chunks (encrypted end to end from A).
        b.open_direct(boxed.offer.clone(), 1, true);
        for c in &boxed.chunks {
            b.on_chunk(c);
        }
        let mut got = Vec::new();
        let mut ok = None;
        for e in events(&rxb) {
            match e {
                Event::FileChunk { file_id: f, data, .. } if f == file_id => got.extend_from_slice(&data),
                Event::FileDone { file_id: f, ok: o, .. } if f == file_id => ok = Some(o),
                _ => {}
            }
        }
        assert_eq!(ok, Some(true));
        assert_eq!(got, data);
        // The acknowledgements reach A: the file is deleted from the mailbox.
        let offer_ids: Vec<u64> = a
            .awaiting
            .iter()
            .filter(|(_, p)| matches!(p, Pending::FileOffer(f) if *f == file_id))
            .map(|(id, _)| *id)
            .collect();
        for id in offer_ids {
            a.on_acked(id);
        }
        // (Without a route in the test the chunks are not sent; the acknowledgements are given directly.)
        for idx in 0..boxed.chunks.len() as u16 {
            a.chunk_acked(file_id, idx);
        }
        assert!(!a.out_files.contains_key(&file_id));
        assert!(a.filebox.waiting_for(&b.addr).is_empty());
        assert!(events(&rx).iter().any(|e| matches!(
            e,
            Event::FileDone { ok: true, text, .. } if &text[..] == b"delivered from mailbox"
        )));
        let _ = fs::remove_dir_all(dir);
        let _ = fs::remove_dir_all(dir_b);
    }
}

#[cfg(test)]
mod bulletin_tests {
    //! Signed bulletins on the #bulletin channel (see maarse_crypto::bulletin).
    use super::*;
    use std::sync::mpsc::{Receiver, channel};

    fn node(name: &str, trusted: Vec<PublicIdentity>) -> (Node, Receiver<Vec<u8>>) {
        let dir = std::env::temp_dir().join(format!("maarse-bul-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut c = file_interop_tests::cfg(dir);
        c.bulletin_keys = trusted;
        let mut n = Node::new(&c).unwrap();
        let (tx, rx) = channel();
        n.clients.insert(1, tx);
        (n, rx)
    }

    /// A channel payload on #bulletin from `from`, signed by `signer`.
    fn sealed(from: &Node, signer: &Node, text: &[u8]) -> Vec<u8> {
        let mut plain = [0u8; 255];
        let n = bulletin::sign(&signer.identity, 1_700_000_000, text, &mut plain).unwrap();
        let mut ct = [0u8; 255];
        let m = seal::seal_channel(&bulletin::channel_key(), &from.addr, [1; 8], &plain[..n], &mut ct).unwrap();
        ct[..m].to_vec()
    }

    fn shown(rx: &Receiver<Vec<u8>>) -> Vec<String> {
        rx.try_iter()
            .filter_map(|f| Event::decode(&f[2..]).ok())
            .filter_map(|e| match e {
                Event::ChannelReceived { text, .. } => Some(String::from_utf8_lossy(&text).into_owned()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn only_trusted_bulletins_are_shown() {
        let (auth, _) = node("auth", Vec::new());
        let (mallory, _) = node("mallory", Vec::new());
        let (mut b, rx) = node("b", vec![auth.identity.public()]);
        let ch = bulletin::channel();
        assert!(b.channels.contains_key(&ch), "joined #bulletin");

        b.on_deliver(PacketType::Channel, ch, &sealed(&auth, &auth, b"Bridge closed"), 2);
        assert_eq!(shown(&rx), vec!["[Verified bulletin] Bridge closed".to_string()]);

        // An untrusted sender, and a forgery under the trusted address.
        b.on_deliver(PacketType::Channel, ch, &sealed(&mallory, &mallory, b"Fake"), 2);
        b.on_deliver(PacketType::Channel, ch, &sealed(&auth, &mallory, b"Fake"), 2);
        assert!(shown(&rx).is_empty());
    }

    #[test]
    fn no_bulletin_channel_without_keys() {
        let (n, _) = node("nokeys", Vec::new());
        assert!(!n.channels.contains_key(&bulletin::channel()));
    }
}
