//! The node's main loop: routing, encryption, announces, files, mailboxes and
//! web clients.

mod ext;
mod pages;
mod tun;
mod hist;
mod person;
mod identity;
mod alerts;
mod debugview;

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
/// A link's kind for telemetry: "radio", "trunk" or "backbone".
fn link_kind(name: &str) -> &'static str {
    if name.starts_with("trunk ") {
        "trunk"
    } else if crate::extra_links::is_backbone(name) {
        "backbone"
    } else {
        "radio"
    }
}

/// Backbone nodes seen before (one "address-hex endpoint" per line).
const DHT_CONTACTS_FILE: &str = "dht-contacts.txt";
/// Announce field: the mailbox's address.
const APP_POSTBOX: u8 = 0x01;
/// A new FS session is sent at most this often to the same contact (resync).
const RESYNC_EVERY: Duration = Duration::from_secs(60);

enum Timer {
    Tx { packet: Vec<u8>, id: u64, only: Only },
    Tick,
    Announce,
    Housekeeping,
    Dht,
}

/// Which links a packet goes out on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Only {
    All,
    /// TCP/UDP/fast serial host links only (bordercast: a request from the
    /// backbone for a node not heard over our radio stays off the air).
    Backbone,
    /// LoRa modems and slow links only (a cached announce answering a key
    /// request heard over the radio).
    Radio,
    /// This one link only (a trunk summary).
    Link(LinkId),
}

/// Bordercast: a second request for the same destination within this time
/// goes on the radio anyway (the node may be here but unheard).
const BORDERCAST_REPEAT: Duration = Duration::from_secs(120);
/// Key requests are answered from the announce cache at most this often per
/// destination.
const KEY_ANSWER_EVERY: Duration = Duration::from_secs(30);
/// Post office: how long a recipient counts as present after a sync, how
/// often we ask the replicas for a recipient we hear, and the first retry
/// of a delivery (then doubling, up to 32 times that).
const MAIL_PRESENT: Duration = Duration::from_secs(600);
const MAIL_FETCH_EVERY: Duration = Duration::from_secs(600);
const MAIL_RETRY_FIRST: Duration = Duration::from_secs(300);
/// Trunks: a request for a node heard over our own radio crosses only on a
/// second request within this time.
const TRUNK_REPEAT: Duration = Duration::from_secs(120);

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
    MailDelivery(Address),
    /// A bundle of stored messages for the recipient (their IDs).
    MailBundle(Address, Vec<u64>),
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
    /// Boxed: the router is about 150 KB, too big to move around on the stack.
    router: Box<Router<1024, 4096>>,
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
    /// Mail for a person delivered to one of its devices: the delivery's
    /// message ID -> the stored mail's ID (see node/person.rs).
    mail_alias: HashMap<u64, u64>,
    max_file: u32,
    // ---------- file limits and file mailbox (see filebox.rs) ----------
    /// The link the packet we are processing came in on.
    rx_link: Option<LinkId>,
    /// SNR of the packet being handled (for the telemetry census).
    rx_snr: Option<i8>,
    /// maarse-server's telemetry (None on an ordinary host).
    tele: Option<Box<crate::telemetry::Telemetry>>,
    next_tele: Instant,
    next_tele_save: Instant,
    last_forwarded: u32,
    /// Where each address was last heard: link, TCP?, hops.
    heard_via: HashMap<Address, (LinkId, bool, u8)>,
    /// Every announce we have heard (payload without stamp), to answer key
    /// requests: a host keeps all of them, a radio node cannot.
    announce_cache: HashMap<Address, Vec<u8>>,
    /// Bordercast: requests from the backbone kept off the radio, by
    /// destination, and key requests answered, by destination.
    bc_dropped: HashMap<Address, Instant>,
    /// Which link a path ID was last heard on (its own 0-hop hello or
    /// announce, or as the previous hop of a routed packet), so a routed
    /// packet goes only to the link of its next hop.
    pid_link: HashMap<u16, (LinkId, Instant)>,
    /// Routed packets sent to one link lately: a second send of the same
    /// packet (a hop retry) goes to every link, in case the guess was wrong.
    targeted: HashMap<u64, Instant>,
    key_answered: HashMap<Address, Instant>,
    bordercast: bool,
    /// Post office: recipients present (synced lately), replicas asked,
    /// delivery retries (next try, step).
    present: HashMap<Address, Instant>,
    fetch_asked: HashMap<Address, Instant>,
    mail_retry: HashMap<Address, (Instant, u32)>,
    /// Trunks: the addresses heard at the far end of each trunk link, and
    /// requests kept off a trunk (by destination).
    trunk_peers: HashMap<LinkId, HashSet<Address>>,
    trunk_dropped: HashMap<Address, Instant>,
    /// Backbone: keep this many backbone links (`--min-peers`), status lines.
    min_peers: usize,
    backbone: bool,
    next_peer_try: Instant,
    next_status: Instant,
    next_contacts_save: Instant,
    /// Packets and bytes per link (rx, tx, rx bytes, tx bytes) for the debug view.
    link_stats: HashMap<LinkId, [u64; 4]>,
    saved_contacts: usize,
    /// What other servers offer: from DHT `Caps` answers, and per link from
    /// their link hello.
    caps_of: HashMap<Address, (dht::Caps, Instant)>,
    link_caps: HashMap<LinkId, dht::Caps>,
    next_caps_poll: Instant,
    no_post_office: bool,
    no_relay: bool,
    mail_quota: u64,
    /// Who is behind each trunk (the far end's Bloom filter), and when we
    /// next send ours.
    trunk_bloom: HashMap<LinkId, (Vec<u8>, Instant)>,
    next_summary: Instant,
    /// What other nodes can read (`appdata::CAP_*`).
    caps: HashMap<Address, u8>,
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
    /// Linked devices: our person, other devices' certificates (node/person.rs).
    persons: person::Persons,
    /// Identities whose signed bulletins and alerts we show (`--bulletin-key`).
    bulletin_keys: Vec<PublicIdentity>,
    /// Alerts v2: roots, certificates, replay guard, issuer key (node/alerts.rs).
    alerts: alerts::Alerts,
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
        // An anchor by default: it splits long routes at the host, so a path
        // can be 12 hops on each side of the backbone.
        let mut router = Router::new(addr, if cfg.repeater_role { Role::Repeater } else { Role::Anchor });
        // Hellos tell the radio nodes nearby that a repeater/anchor is here,
        // and that we are a large post office.
        router.hello = true;
        // A host keeps mail (it has a disk), unless told not to.
        router.mail_class = if cfg.no_post_office { maarse_core::mail::CLASS_NONE } else { maarse_core::mail::CLASS_LARGE };
        router.ring = 4;
        // Only fast links here: short time slots, otherwise every hop waits needlessly.
        router.slot_ms = 5;

        let mut node = Node {
            identity,
            addr,
            router: Box::new(router),
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
            mail_alias: HashMap::new(),
            max_file: cfg.max_file,
            rx_link: None,
            rx_snr: None,
            tele: cfg.backbone.then(|| Box::new(crate::telemetry::Telemetry::load(&cfg.data_dir.join("telemetry.json")))),
            next_tele: Instant::now() + Duration::from_secs(5),
            next_tele_save: Instant::now() + Duration::from_secs(300),
            last_forwarded: 0,
            heard_via: HashMap::new(),
            announce_cache: HashMap::new(),
            bc_dropped: HashMap::new(),
            pid_link: HashMap::new(),
            targeted: HashMap::new(),
            key_answered: HashMap::new(),
            bordercast: !cfg.no_bordercast,
            present: HashMap::new(),
            fetch_asked: HashMap::new(),
            mail_retry: HashMap::new(),
            trunk_peers: HashMap::new(),
            trunk_dropped: HashMap::new(),
            min_peers: cfg.min_peers,
            backbone: cfg.backbone,
            next_peer_try: Instant::now() + Duration::from_secs(20),
            next_status: Instant::now() + Duration::from_secs(60),
            next_contacts_save: Instant::now() + Duration::from_secs(30),
            saved_contacts: 0,
            caps_of: HashMap::new(),
            link_caps: HashMap::new(),
            next_caps_poll: Instant::now() + Duration::from_secs(30),
            no_post_office: cfg.no_post_office,
            no_relay: cfg.no_relay,
            mail_quota: cfg.mail_quota_mb as u64 * 1024 * 1024,
            link_stats: HashMap::new(),
            trunk_bloom: HashMap::new(),
            next_summary: Instant::now() + Duration::from_secs(60),
            caps: HashMap::new(),
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
            persons: person::Persons::load(&cfg.data_dir, &addr),
            bulletin_keys: cfg.bulletin_keys.clone(),
            alerts: alerts::Alerts::load(cfg)?,
        };
        // Backbone nodes seen before: a restart finds the network again even
        // when every seed is gone.
        for c in dht::load_contacts(&node.data_dir.join(DHT_CONTACTS_FILE)) {
            if node.dht.public_ep.as_ref() != Some(&c.ep) {
                if !node.dht.seeds.contains(&c.ep) {
                    node.dht.seeds.push(c.ep.clone());
                }
                node.dht.table.insert(c);
            }
        }
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
        if !node.bulletin_keys.is_empty() || node.alerts.active() {
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

    /// Our alert issuer key, if we have one.
    pub fn issuer_public(&self) -> Option<PublicIdentity> {
        self.alerts.issuer_public()
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
        crate::debug::push("log", serde_json::json!({ "text": msg.as_ref() }));
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
                Some(Timer::Tx { packet, id, only }) => {
                    if !self.cancelled.remove(&id) {
                        self.send_links_only(&packet, only);
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
                if crate::extra_links::is_backbone(&name) {
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
                self.pid_link.retain(|_, (l, _)| *l != id);
                self.link_regions.remove(&id);
                self.link_caps.remove(&id);
                self.heard_via.retain(|_, v| v.0 != id);
                if let Some((name, _)) = self.links.remove(&id) {
                    self.log(format!("link down: {name}"));
                    self.broadcast_log(format!("Link down ({} left)", self.links.len()));
                }
            }
            In::Packet { data, snr, link } => {
                // Blocked nodes (guard.rs): nothing to, from or about them.
                if let Ok((h, _)) = Header::decode(&data)
                    && crate::guard::node_blocked(&h.dest)
                {
                    return;
                }
                self.counters.packets_in += 1;
                self.counters.bytes_in += data.len() as u64;
                self.learn_pid_link(&data, link);
                {
                    let c = self.link_stats.entry(link).or_default();
                    c[0] += 1;
                    c[2] += data.len() as u64;
                    let mut f = crate::debug::packet_fields(&data);
                    f["link"] = self.links.get(&link).map(|(n, _)| n.clone()).unwrap_or_default().into();
                    f["snr"] = snr.into();
                    crate::debug::push("rx", f);
                    let named = self.links.get(&link).map(|(n, _)| (n.clone(), link_kind(n)));
                    if let (Some(t), Some(n)) = (self.tele.as_mut(), named) {
                        t.packet(false, &[n], &data, Some(snr));
                    }
                }
                if self
                    .links
                    .get(&link)
                    .is_some_and(|(n, _)| n.starts_with("serial ") || n.starts_with("wifi-modem"))
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
                if self.links.get(&link).is_some_and(|(n, _)| n.starts_with("trunk "))
                    && let Ok(p) = Packet::decode(&data)
                    && p.header.hops == 0
                {
                    match p.header.ptype {
                        PacketType::Hello | PacketType::Announce => {
                            self.trunk_peers.entry(link).or_default().insert(p.header.dest);
                        }
                        PacketType::Summary if p.payload.len() >= maarse_core::bloom::BYTES => {
                            self.trunk_bloom.insert(link, (p.payload.to_vec(), Instant::now()));
                        }
                        _ => {}
                    }
                }
                let now = self.now_ms();
                let actions = self.router.on_receive(&data, snr, now);
                self.rx_link = Some(link);
                self.rx_snr = Some(snr);
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
            In::LinkCaps { id, caps } => {
                if let Some((name, _)) = self.links.get(&id) {
                    self.log(format!(
                        "{name}: {} v{}{}{}{}",
                        if caps.server { "server" } else { "host" },
                        caps.version,
                        if caps.radio { ", radio" } else { "" },
                        if caps.post_office { ", post office" } else { ", no post office" },
                        if caps.relay { ", relay" } else { "" }
                    ));
                }
                self.link_caps.insert(id, caps);
            }
            In::DhtCaps { id, caps } => {
                if let Some(c) = caps {
                    self.caps_of.insert(id, (c, Instant::now()));
                }
            }
            In::ModemFrame { link, frame } => {
                let name = self.links.get(&link).map(|(n, _)| n.clone()).unwrap_or_default();
                match Event::decode(&frame) {
                    Ok(Event::Stats { items }) => {
                        let items: Vec<(u8, i32)> = items.iter().map(|(k, v)| (*k, *v)).collect();
                        crate::debug::push("modem", serde_json::json!({"link": name, "stats": items.len()}));
                        if let Some(t) = self.tele.as_mut() {
                            t.modem_stats(&name, &items);
                        }
                    }
                    Ok(Event::Radio(cfg)) => {
                        if let Some(t) = self.tele.as_mut() {
                            t.modem_radio(&name, format!("{:.3} MHz, SF{}, BW {} kHz, CR4/{}, {} dBm, duty {} ‰", cfg.freq_hz as f64 / 1e6, cfg.sf, cfg.bw_hz / 1000, cfg.cr, cfg.tx_power_dbm, cfg.duty_permille));
                        }
                    }
                    _ => {}
                }
            }
            In::MailFetched { recipient, items } if self.person_mail_fetched(recipient, &items) => {}
            In::MailFetched { recipient, items } => {
                let n = items.len();
                for sealed in items {
                    // Fetched to hand over to a recipient on our radio: kept
                    // for delivery even without a post office here.
                    let (npo, quota) = (self.no_post_office, self.mail_quota);
                    (self.no_post_office, self.mail_quota) = (false, 0);
                    self.store_mail(recipient, sealed, false);
                    (self.no_post_office, self.mail_quota) = (npo, quota);
                }
                if n > 0 {
                    self.log(format!("post office: fetched {n} message(s) for {} from the replicas", short(&recipient)));
                    self.flush_mailbox(recipient);
                }
            }
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

    /// Notes which link a path ID is behind (see `pid_link`).
    fn learn_pid_link(&mut self, data: &[u8], link: LinkId) {
        let Ok((h, _)) = Header::decode(data) else { return };
        let pid = match (&h.path, h.ptype) {
            (None, PacketType::Hello | PacketType::Announce) if h.hops == 0 => Some(h.dest.path_id()),
            // A route search records its forwarders: the last one sent it to us.
            (Some(p), PacketType::PathRequest) => p.last().copied(),
            // Source routed: `hops` is the next forwarder's index, so the
            // previous one is just before it.
            (Some(p), _) if h.hops > 0 => p.get(h.hops as usize - 1).copied(),
            _ => None,
        };
        if let Some(pid) = pid {
            if self.pid_link.len() > 50_000 {
                self.pid_link.retain(|_, (_, t)| t.elapsed() < Duration::from_secs(3600));
            }
            self.pid_link.insert(pid, (link, Instant::now()));
        }
    }

    /// The one link a routed packet should go out on, if it is known: the
    /// next forwarder's (`path[hops]`), or the destination's at the end of
    /// the path. Floods and route searches go everywhere as before.
    fn next_hop_link(&self, h: &Header) -> Option<LinkId> {
        let path = h.path.as_ref()?;
        if h.ptype == PacketType::PathRequest {
            return None;
        }
        let fresh = |l: &(LinkId, Instant)| (l.1.elapsed() < Duration::from_secs(1800) && self.links.contains_key(&l.0)).then_some(l.0);
        match path.get(h.hops as usize) {
            Some(pid) => self.pid_link.get(pid).and_then(fresh),
            None => self.heard_via.get(&h.dest).filter(|v| self.links.contains_key(&v.0)).map(|v| v.0),
        }
    }

    fn send_links(&mut self, packet: &[u8]) {
        self.send_links_only(packet, Only::All);
    }

    /// Bordercast (after ZRP): should a request that came in over the
    /// backbone stay off our radio? Yes unless its destination was heard
    /// over our own radio, or this is a second request for it.
    fn backbone_only(&mut self, packet: &[u8]) -> bool {
        if !self.bordercast {
            return false;
        }
        let Some(link) = self.rx_link else { return false };
        if !self.links.get(&link).is_some_and(|(n, _)| crate::extra_links::is_backbone(n)) {
            return false;
        }
        let Ok((h, _)) = Header::decode(packet) else { return false };
        if !matches!(h.ptype, PacketType::PathRequest | PacketType::KeyRequest) {
            return false;
        }
        if self.heard_via.get(&h.dest).is_some_and(|(_, tcp, _)| !tcp) {
            return false;
        }
        self.bc_dropped.retain(|_, t| t.elapsed() < BORDERCAST_REPEAT);
        if self.bc_dropped.remove(&h.dest).is_some() {
            return false;
        }
        self.bc_dropped.insert(h.dest, Instant::now());
        true
    }

    /// A key request heard over the radio for a node whose announce we
    /// keep: send that announce on the radio (the node itself may be far).
    fn answer_key_request(&mut self, dest: Address) {
        let from_radio = self
            .rx_link
            .and_then(|l| self.links.get(&l))
            .is_some_and(|(n, _)| !crate::extra_links::is_backbone(n));
        if !from_radio || self.key_answered.get(&dest).is_some_and(|t| t.elapsed() < KEY_ANSWER_EVERY) {
            return;
        }
        let Some(payload) = self.announce_cache.get(&dest) else { return };
        let mut body = maarse_core::router::Buf::from_slice(payload).unwrap_or_default();
        if !stamp::append(PacketType::Announce, &dest, &mut body) {
            return;
        }
        let Ok(packet) = maarse_core::packet::encode(&Header::new(PacketType::Announce, dest), &body) else { return };
        self.key_answered.insert(dest, Instant::now());
        self.log(format!("key request for {}: sent its announce", short(&dest)));
        self.send_links_only(&packet, Only::Radio);
    }

    /// Trunks (long-distance LoRa links) are a backup, like a floating
    /// static route: routed packets cross only when the trunk is their next
    /// hop, alerts and bulletins always, our own hellos and announces, and
    /// path or key requests only when TCP cannot reach the other end and
    /// the destination is not known on this side (else on a second
    /// request). Nothing else: a 10 % LoRa channel carries about one packet
    /// every two seconds. maarse-sim (docs/denmark/README.md, "Long-distance
    /// LoRa trunks"): without internet a third more messages across the
    /// water, with losses near zero; letting every flood across saturates it.
    fn trunk_allows(&mut self, link: LinkId, packet: &[u8]) -> bool {
        let Ok(p) = Packet::decode(packet) else { return false };
        let h = &p.header;
        let peers = self.trunk_peers.get(&link).cloned().unwrap_or_default();
        if let Some(path) = &h.path
            && h.ptype != PacketType::PathRequest
        {
            // Next hop at the far end, or the path is used up: then it is for
            // the anchor at the far end (two hosts facing each other across a
            // trunk are anchors with an empty leg between them).
            return match path.get(h.hops as usize) {
                Some(next) => peers.iter().any(|a| a.path_id() == *next),
                None => !peers.is_empty(),
            };
        }
        match h.ptype {
            PacketType::Hello | PacketType::Announce => h.dest == self.addr,
            PacketType::Channel => h.dest == bulletin::channel(),
            PacketType::PathRequest | PacketType::KeyRequest => {
                // TCP reaches the other end: stay off the trunk.
                let tcp_up = self.links.values().any(|(n, _)| crate::extra_links::is_backbone(n));
                if tcp_up && peers.iter().any(|a| self.heard_via.get(a).is_some_and(|(_, tcp, _)| *tcp)) {
                    return false;
                }
                // The far end's summary says who is behind the trunk.
                if let Some((bits, t)) = self.trunk_bloom.get(&link)
                    && t.elapsed() < Duration::from_secs(1800)
                {
                    if maarse_core::bloom::contains(bits, &h.dest) {
                        return true;
                    }
                    self.trunk_dropped.retain(|_, t| t.elapsed() < TRUNK_REPEAT);
                    if self.trunk_dropped.remove(&h.dest).is_some() {
                        return true;
                    }
                    self.trunk_dropped.insert(h.dest, Instant::now());
                    return false;
                }
                let here = self
                    .heard_via
                    .get(&h.dest)
                    .is_some_and(|(l, tcp, _)| !*tcp && *l != link && !self.links.get(l).is_some_and(|(n, _)| n.starts_with("trunk ")));
                if !here {
                    return true;
                }
                self.trunk_dropped.retain(|_, t| t.elapsed() < TRUNK_REPEAT);
                if self.trunk_dropped.remove(&h.dest).is_some() {
                    return true;
                }
                self.trunk_dropped.insert(h.dest, Instant::now());
                false
            }
            _ => false,
        }
    }

    fn send_links_only(&mut self, packet: &[u8], only: Only) {
        // Decide for the trunks first (needs &mut self).
        let trunk_ids: Vec<LinkId> = self.links.iter().filter(|(_, (n, _))| n.starts_with("trunk ")).map(|(id, _)| *id).collect();
        let mut trunk_skip: HashSet<LinkId> = HashSet::new();
        for id in trunk_ids {
            if only == Only::Link(id) {
                continue;
            }
            if only == Only::Backbone || !self.trunk_allows(id, packet) {
                trunk_skip.insert(id);
            }
        }
        let header = Header::decode(packet).ok().map(|(h, _)| h);
        // A routed packet goes only to the link of its next hop, if known
        // (before, it went out on every backbone link).
        let mut target = header.as_ref().and_then(|h| self.next_hop_link(h));
        if target.is_some() {
            let pid = maarse_core::dedup::packet_id(header.as_ref().unwrap().ptype, &header.as_ref().unwrap().dest, packet);
            self.targeted.retain(|_, t| t.elapsed() < Duration::from_secs(10));
            if self.targeted.insert(pid, Instant::now()).is_some() {
                target = None; // a retry: everywhere, as before
            }
        }
        let region = header.as_ref().map_or(0, |h| h.region);
        // File chunks to a recipient reached over TCP are not sent out over
        // a LoRa modem (they would only occupy the channel).
        let tcp_only = header
            .as_ref()
            .is_some_and(|h| h.ptype == PacketType::Fragment && self.path_kind(&h.dest) == PathKind::Tcp);
        let mut sent_to: Vec<(LinkId, String)> = Vec::new();
        for (id, (name, tx)) in &self.links {
            if trunk_skip.contains(id) {
                continue;
            }
            if target.is_some_and(|t| t != *id) && only != Only::Link(*id) {
                continue;
            }
            if let Only::Link(l) = only
                && l != *id
            {
                continue;
            }
            let backbone = crate::extra_links::is_backbone(name);
            if (tcp_only || only == Only::Backbone) && !backbone {
                continue;
            }
            if only == Only::Radio && backbone {
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
            sent_to.push((*id, name.clone()));
        }
        let mut f = crate::debug::packet_fields(packet);
        f["links"] = sent_to.iter().map(|(_, n)| n.clone()).collect::<Vec<_>>().into();
        f["skipped_trunks"] = trunk_skip.len().into();
        crate::debug::push("tx", f);
        if let Some(t) = self.tele.as_mut()
            && !sent_to.is_empty()
        {
            let named: Vec<(String, &'static str)> = sent_to.iter().map(|(_, n)| (n.clone(), link_kind(n))).collect();
            t.packet(true, &named, packet, None);
        }
        for (id, _) in sent_to {
            let c = self.link_stats.entry(id).or_default();
            c[1] += 1;
            c[3] += packet.len() as u64;
        }
    }

    fn apply(&mut self, actions: Vec<Action>) {
        for a in &actions {
            let ev = match a {
                Action::Transmit { .. } => None,
                Action::Deliver { ptype, dest, hops, payload } => {
                    Some(serde_json::json!({"action": "deliver", "ptype": format!("{ptype:?}"), "dest": crate::debug::hex(&dest.0), "hops": hops, "size": payload.len()}))
                }
                Action::Acked { msg_id } => Some(serde_json::json!({"action": "acked", "msg_id": format!("{msg_id:016x}")})),
                Action::RouteLearned { dest, hops } => Some(serde_json::json!({"action": "route learned", "dest": crate::debug::hex(&dest.0), "hops": hops})),
                Action::Cancel { id } => Some(serde_json::json!({"action": "flood cancelled (neighbours already sent it)", "pid": format!("{id:016x}")})),
                other => {
                    let s = format!("{other:?}");
                    Some(serde_json::json!({"action": s.chars().take(80).collect::<String>()}))
                }
            };
            if let Some(t) = self.tele.as_mut() {
                match a {
                    Action::Transmit { .. } => {}
                    Action::Deliver { .. } => t.router_event("delivered"),
                    Action::Acked { .. } => t.router_event("our message acknowledged"),
                    Action::RouteLearned { .. } => t.router_event("route learned"),
                    Action::Cancel { .. } => t.router_event("flood cancelled"),
                    other => t.router_event(format!("{other:?}").split([' ', '{', '(']).next().unwrap_or("other")),
                }
            }
            if let Some(e) = ev {
                crate::debug::push("router", e);
            }
        }
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
                    let only = if self.backbone_only(&packet) { Only::Backbone } else { Only::All };
                    if delay_ms == 0 {
                        self.send_links_only(&packet, only);
                    } else {
                        let t = Timer::Tx {
                            packet: packet.to_vec(),
                            id,
                            only,
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
        // Text is compressed for a node that says it can read it.
        let z;
        let plain = if self.caps.get(&dest).is_some_and(|c| c & maarse_core::appdata::CAP_TEXTZ != 0) {
            z = content::compress(plain);
            &z[..]
        } else {
            plain
        };
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
            Some(Pending::MailBundle(recipient, ids)) => {
                let mut done = Vec::new();
                if let Some(m) = self.mailbox.get_mut(&recipient) {
                    for id in &ids {
                        if let Some(mail) = m.remove(id) {
                            done.push((*id, unix_ms(mail.stored)));
                        }
                    }
                }
                if let Some(t) = self.tele.as_mut() {
                    t.mail_delivered(&recipient, &done);
                }
                self.counters.mail_delivered += ids.len() as u32;
                self.save_mailbox();
                self.update_tim();
                let replicas = self.mail_replicas(&recipient);
                if !replicas.is_empty() {
                    let me = self.addr;
                    let my_ep = self.dht.public_ep.clone();
                    let mids: Vec<u32> = ids.iter().map(|id| (*id >> 32) as u32).collect();
                    std::thread::spawn(move || {
                        for c in replicas {
                            let _ = dht::rpc(&c.ep, &me, my_ep.as_deref(), &dht::Req::MailDelivered { recipient, ids: mids.clone() });
                        }
                    });
                }
            }
            Some(Pending::MailDelivery(recipient)) => {
                // Mail for a person was sent to one of its devices under another ID.
                let msg_id = self.mail_alias.remove(&msg_id).unwrap_or(msg_id);
                if let Some(mail) = self.mailbox.get_mut(&recipient).and_then(|m| m.remove(&msg_id))
                    && let Some(t) = self.tele.as_mut()
                {
                    t.mail_delivered(&recipient, &[(msg_id, unix_ms(mail.stored))]);
                }
                self.update_tim();
                // Delete the copies too.
                let replicas = self.mail_replicas(&recipient);
                if !replicas.is_empty() {
                    let me = self.addr;
                    let my_ep = self.dht.public_ep.clone();
                    let ids = vec![(msg_id >> 32) as u32];
                    std::thread::spawn(move || {
                        for c in replicas {
                            let _ = dht::rpc(&c.ep, &me, my_ep.as_deref(), &dht::Req::MailDelivered { recipient, ids: ids.clone() });
                        }
                    });
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
            // Someone asks who we are: announce. Who another node is: answer
            // from the announces we keep.
            PacketType::KeyRequest if dest == self.addr => self.announce(),
            PacketType::KeyRequest => self.answer_key_request(dest),
            PacketType::MailSync => self.on_mail_sync(dest, payload),
            PacketType::Data => self.open_direct(payload.to_vec(), hops, true),
            PacketType::Fragment => self.on_chunk(payload),
            PacketType::Deposit => self.on_deposit(payload, hops),
            PacketType::Datagram => self.on_tun_datagram(payload),
            PacketType::Probe => self.on_trace(dest, payload),
            PacketType::Channel => {
                let Some((_, key)) = self.channels.get(&dest) else {
                    return;
                };
                let mut out = [0u8; 256];
                match seal::open_channel(key, payload, &mut out) {
                    Ok((from, n)) if from != self.addr && dest == bulletin::channel() => {
                        self.on_bulletin(from, hops, &out[..n]);
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
                self.announce_cache.insert(dest, payload.to_vec());
                if self.tele.is_some() {
                    let (via, backbone) = self
                        .rx_link
                        .and_then(|l| self.links.get(&l))
                        .map(|(n, _)| (n.clone(), crate::extra_links::is_backbone(n)))
                        .unwrap_or_else(|| ("(local)".into(), false));
                    let app = maarse_core::appdata::parse(a.app_data);
                    let snr = self.rx_snr;
                    if let Some(t) = self.tele.as_mut() {
                        t.announce(&dest, &app, hops, &via, backbone, snr);
                    }
                }
                self.on_app_data(dest, hops, a.app_data);
                self.person_on_announce(dest, a.app_data);
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
                    let tcp = self.links.get(&link).is_some_and(|(n, _)| crate::extra_links::is_backbone(n));
                    self.heard_via.insert(dest, (link, tcp, hops));
                    // Heard over our radio: maybe the replicas hold mail for
                    // it ("X is here", like a mobile network registering a
                    // phone).
                    if !tcp && self.fetch_asked.get(&dest).is_none_or(|t| t.elapsed() >= MAIL_FETCH_EVERY) {
                        self.fetch_mail(dest, Vec::new());
                    }
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
        // Mail for our person, encrypted to its inbox key (node/person.rs).
        match self.try_open_inbox(&payload, hops) {
            Some(Ok(())) => return,
            Some(Err(CryptoError::UnknownSender(a))) => {
                if first_time {
                    self.log(format!("inbox message from unknown {}: waiting for announce", short(&a)));
                }
                if self.pending_rx.len() >= PENDING_MAX {
                    self.pending_rx.remove(0);
                }
                return self.pending_rx.push((payload, hops));
            }
            // Not an inbox message after all: the other formats.
            _ => {}
        }
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
    fn on_content(&mut self, from: Address, plain: &[u8], hops: u8, format: Format) {
        let expanded = content::expand(plain);
        let plain: &[u8] = expanded.as_deref().unwrap_or(plain);
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
                // A device of a person is shown as the person.
                let peer = self.person_peer(&from);
                self.broadcast(&Event::DirectReceived {
                    from: peer,
                    hops,
                    text: text(t),
                });
                self.person_received(peer, t);
            }
            Some(Content::Person(o, data)) => self.on_person(from, o, data),
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

    /// A `Deposit`: STORE (a sender or a small post office hands us a
    /// message), SYNC (deliver a recipient's mail, wherever it is now) or
    /// DELIVERED (delete). Older senders send `recipient | sealed` without an
    /// op byte; that is stored as before.
    fn on_deposit(&mut self, payload: &[u8], hops: u8) {
        use maarse_core::mail::{self, Deposit};
        let Some(d) = Deposit::parse(payload) else {
            if let Some(recipient) = Address::from_slice(payload) {
                self.store_mail(recipient, payload[16..].to_vec(), true);
            }
            return;
        };
        match d.op {
            mail::OP_STORE => {
                self.store_mail(d.recipient, d.body.to_vec(), true);
            }
            mail::OP_SYNC => {
                let have: Vec<u32> = d.ids().collect();
                self.present.insert(d.recipient, Instant::now());
                self.flush_mailbox(d.recipient);
                self.fetch_mail(d.recipient, have);
            }
            mail::OP_DELIVERED => {
                let ids: Vec<u32> = d.ids().collect();
                self.drop_mail(d.recipient, &ids);
            }
            mail::OP_BUNDLE if d.recipient == self.addr => {
                let items: Vec<Vec<u8>> = d.bundle().map(|m| m.to_vec()).collect();
                for m in items {
                    self.open_direct(m, hops, true);
                }
            }
            _ => {}
        }
    }

    /// Stores a sealed message for `recipient`; with `replicate`, copies go
    /// to the recipient's replica hosts (rendezvous hashing over the hosts in
    /// the DHT), over TCP only.
    /// Keeps a message for `recipient`; `false` if refused (no post office
    /// here, or the quota is full).
    fn store_mail(&mut self, recipient: Address, sealed: Vec<u8>, replicate: bool) -> bool {
        let over_quota = self.mail_quota > 0 && self.mail_bytes() + sealed.len() as u64 > self.mail_quota;
        if self.no_post_office || over_quota {
            if replicate {
                // Mail from our own side: not kept here, but still handed to
                // the recipient's replica post offices.
                let replicas = self.mail_replicas(&recipient);
                self.log(format!("post office: not kept here, sent to {} replica(s) for {}", replicas.len(), short(&recipient)));
                let me = self.addr;
                let my_ep = self.dht.public_ep.clone();
                std::thread::spawn(move || {
                    for c in replicas {
                        let _ = dht::rpc(&c.ep, &me, my_ep.as_deref(), &dht::Req::MailStore { recipient, sealed: sealed.clone() });
                    }
                });
                return true;
            }
            self.log(format!("post office: refused mail for {} ({})", short(&recipient), if self.no_post_office { "no post office here" } else { "quota full" }));
            return false;
        }
        crate::debug::push("mail", serde_json::json!({"op": "store", "recipient": crate::debug::hex(&recipient.0), "size": sealed.len(), "replicate": replicate}));
        if let Some(t) = self.tele.as_mut() {
            t.mail(if replicate { "stored (sent here)" } else { "stored (copy from another host)" });
        }
        let id = maarse_core::dedup::packet_id(PacketType::Data, &recipient, &sealed);
        let box_ = self.mailbox.entry(recipient).or_default();
        if box_.contains_key(&id) {
            return true;
        }
        if box_.len() >= MAILBOX_PER_RECIPIENT {
            self.log(format!("mailbox full for {}", short(&recipient)));
            return false;
        }
        box_.insert(id, Mail { sealed: sealed.clone(), stored: SystemTime::now() });
        let count = box_.len();
        if let Some(t) = self.tele.as_mut() {
            t.mail_stored(id, &recipient, !replicate);
        }
        self.log(format!("post office: stored message for {} ({count} in total)", short(&recipient)));
        self.save_mailbox();
        self.update_tim();
        self.mail_retry.entry(recipient).or_insert((Instant::now() + MAIL_RETRY_FIRST, 0));
        if replicate {
            let replicas = self.mail_replicas(&recipient);
            if !replicas.is_empty() {
                let me = self.addr;
                let my_ep = self.dht.public_ep.clone();
                std::thread::spawn(move || {
                    for c in replicas {
                        let _ = dht::rpc(&c.ep, &me, my_ep.as_deref(), &dht::Req::MailStore { recipient, sealed: sealed.clone() });
                    }
                });
            }
        }
        // Maybe the recipient is already here.
        self.flush_mailbox(recipient);
        true
    }

    /// The hosts (other than us) that keep `recipient`'s mail.
    fn mail_replicas(&self, recipient: &Address) -> Vec<Contact> {
        // Only hosts that keep mail (unknown counts as yes, as before).
        let mut contacts: Vec<Contact> = self
            .dht
            .table
            .all()
            .into_iter()
            .filter(|c| self.caps_of.get(&c.id).is_none_or(|(k, _)| k.post_office))
            .collect();
        let mut addrs: Vec<Address> = contacts.iter().map(|c| c.id).collect();
        if self.dht.public_ep.is_some() && !self.no_post_office {
            addrs.push(self.addr);
        }
        // Rendezvous hashing: the REPLICAS highest scores (as mail::replicas).
        addrs.sort_by_key(|a| std::cmp::Reverse(maarse_core::mail::score(a, recipient)));
        addrs.dedup();
        let pick: Vec<Address> = addrs.into_iter().take(maarse_core::mail::REPLICAS).collect();
        contacts.retain(|c| pick.contains(&c.id) && c.id != self.addr);
        contacts
    }

    /// Asks the replicas for `recipient`'s mail (answers come back as
    /// `In::MailFetched`), at most every `MAIL_FETCH_EVERY` per recipient
    /// unless the recipient has just synced.
    fn fetch_mail(&mut self, recipient: Address, have: Vec<u32>) {
        let Some(tx) = self.tx.clone() else { return };
        let replicas = self.mail_replicas(&recipient);
        if replicas.is_empty() {
            return;
        }
        if self.fetch_asked.get(&recipient).is_some_and(|t| t.elapsed() < Duration::from_secs(30)) {
            return;
        }
        self.fetch_asked.insert(recipient, Instant::now());
        let me = self.addr;
        let my_ep = self.dht.public_ep.clone();
        std::thread::spawn(move || {
            let mut items: Vec<Vec<u8>> = Vec::new();
            for c in replicas {
                if let Some((_, dht::Resp::Mail(v))) = dht::rpc(&c.ep, &me, my_ep.as_deref(), &dht::Req::MailFetch { recipient, have: have.clone() }) {
                    for m in v {
                        if !items.contains(&m) {
                            items.push(m);
                        }
                    }
                }
            }
            let _ = tx.send(In::MailFetched { recipient, items });
        });
    }

    /// Deletes delivered messages (by mail ID).
    fn drop_mail(&mut self, recipient: Address, ids: &[u32]) {
        crate::debug::push("mail", serde_json::json!({"op": "delivered elsewhere, deleted", "recipient": crate::debug::hex(&recipient.0), "ids": ids.len()}));
        if let Some(t) = self.tele.as_mut() {
            t.mail("deleted (delivered elsewhere)");
        }
        if let Some(m) = self.mailbox.get_mut(&recipient) {
            m.retain(|id, _| !ids.contains(&((*id >> 32) as u32)));
        }
        self.save_mailbox();
        self.update_tim();
    }

    /// Recipients with mail waiting, told in our hellos ("mail waiting").
    fn update_tim(&mut self) {
        let ids: Vec<u16> = self.mailbox.iter().filter(|(_, m)| !m.is_empty()).map(|(a, _)| a.path_id()).take(4).collect();
        self.router.mail_waiting.clear();
        for id in ids {
            let _ = self.router.mail_waiting.push(id);
        }
    }

    /// A `MailSync` heard over the radio: the asker is here. Deliver what we
    /// hold; if it named us its home post office, fetch the copies first.
    fn on_mail_sync(&mut self, asker: Address, payload: &[u8]) {
        let (home, have) = maarse_core::mail::parse_sync(payload);
        let holds = self.mailbox.get(&asker).is_some_and(|m| !m.is_empty());
        let named = home == Some(self.addr.path_id());
        // A device of a person: the person's mail too.
        self.person_mail_sync(asker, named);
        if !holds && !named {
            return;
        }
        self.present.insert(asker, Instant::now());
        self.flush_mailbox(asker);
        if named {
            self.fetch_mail(asker, have.to_vec());
        }
    }

    /// Tries to deliver waiting mail to `recipient`.
    fn flush_mailbox(&mut self, recipient: Address) {
        crate::debug::push("mail", serde_json::json!({"op": "deliver waiting mail", "recipient": crate::debug::hex(&recipient.0), "waiting": self.mailbox.get(&recipient).map_or(0, |m| m.len())}));
        if let Some(t) = self.tele.as_mut() {
            t.mail("delivery attempt");
        }
        // Mail kept for a person goes to one of its devices.
        if !self.known.contains_key(&recipient) && self.flush_person_mail(recipient, None) {
            return;
        }
        self.flush_filebox(recipient);
        let Some(mails) = self.mailbox.get(&recipient) else {
            return;
        };
        if mails.is_empty()
            || !self.router.has_route(&recipient) && !self.known.contains_key(&recipient)
        {
            return;
        }
        // A recipient that reads bundles gets several messages per packet.
        if self.caps.get(&recipient).is_some_and(|c| c & maarse_core::appdata::CAP_BUNDLE != 0) && mails.len() >= 2 {
            let all: Vec<(u64, Vec<u8>)> = mails.iter().map(|(id, m)| (*id, m.sealed.clone())).collect();
            let max = MAX_DIRECT_PAYLOAD - stamp::STAMP_LEN;
            let mut i = 0;
            while i < all.len() {
                let mut body: maarse_core::router::Buf = maarse_core::router::Buf::new();
                let mut ids = Vec::new();
                while i < all.len() && maarse_core::mail::bundle_push(&mut body, &all[i].1, max) {
                    ids.push(all[i].0);
                    i += 1;
                }
                if ids.is_empty() {
                    i += 1;
                    continue;
                }
                let mut p: maarse_core::router::Buf = maarse_core::router::Buf::new();
                if maarse_core::mail::build(&mut p, &recipient, maarse_core::mail::OP_BUNDLE, &body) {
                    let now = self.now_ms();
                    if let Ok((msg_id, actions)) = self.router.send_typed(PacketType::Deposit, recipient, &p, token(), now) {
                        self.awaiting.insert(msg_id, Pending::MailBundle(recipient, ids));
                        self.apply(actions.into_iter().collect());
                    }
                }
            }
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

    /// A message from one of our users has not been acknowledged within
    /// `DEPOSIT_AFTER`: we are a post office, so we keep it (and copies go to
    /// the recipient's replicas) and deliver when the recipient turns up.
    fn try_deposit(&mut self, msg_id: u64) {
        let Some(m) = self.outbox.get_mut(&msg_id) else {
            return;
        };
        if m.deposited || 16 + m.mail_sealed.len() > MAX_DIRECT_PAYLOAD {
            return;
        }
        m.deposited = true;
        let (dest, sealed) = (m.dest, m.mail_sealed.clone());
        self.log(format!("recipient not answering; keeping the message in our post office for {}", short(&dest)));
        self.store_mail(dest, sealed, true);
        self.counters.deposits += 1;
        self.broadcast(&Event::Deposited { msg_id });
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
        self.backbone_upkeep();
        if crate::debug::enabled() {
            self.debug_snapshot();
            self.telemetry_upkeep();
        }
        // Every 10 minutes: tell each trunk's far end who is on our side.
        if Instant::now() >= self.next_summary {
            self.next_summary = Instant::now() + Duration::from_secs(600);
            let trunks: Vec<LinkId> = self.links.iter().filter(|(_, (n, _))| n.starts_with("trunk ")).map(|(id, _)| *id).collect();
            for link in trunks {
                let mut bits = [0u8; maarse_core::bloom::BYTES];
                for (a, (l, tcp, _)) in &self.heard_via {
                    if !*tcp && *l != link && !self.links.get(l).is_some_and(|(n, _)| n.starts_with("trunk ")) {
                        maarse_core::bloom::insert(&mut bits, a);
                    }
                }
                maarse_core::bloom::insert(&mut bits, &self.addr);
                if let Ok(pkt) = encode(&Header::new(PacketType::Summary, self.addr), &bits) {
                    self.send_links_only(&pkt, Only::Link(link));
                }
            }
        }
        // Post office: without the backbone, try again to deliver what we
        // hold after 5, 15 and 35 minutes, then wait for the recipient to ask.
        // With the backbone, presence ("X is here") and syncs deliver more
        // cheaply (maarse-sim: unlimited retries cost 40 % more airtime).
        let backbone = self.links.values().any(|(n, _)| crate::extra_links::is_backbone(n));
        let due: Vec<Address> = self.mail_retry.iter().filter(|(_, (t, _))| Instant::now() >= *t).map(|(a, _)| *a).collect();
        for r in due {
            let step = self.mail_retry[&r].1 + 1;
            if backbone || step > 3 || !self.mailbox.get(&r).is_some_and(|m| !m.is_empty()) {
                self.mail_retry.remove(&r);
                continue;
            }
            self.mail_retry.insert(r, (Instant::now() + MAIL_RETRY_FIRST * (1 << step), step));
            self.flush_mailbox(r);
        }
        self.present.retain(|_, t| t.elapsed() < MAIL_PRESENT);
        self.tun_tick();
        self.hist_maintain();
        self.person_maintain();
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
        crate::links::region_hello_with_caps(self.router.strict_region, &self.router.regions, &self.my_caps())
    }

    /// Bytes of mail kept for others.
    fn mail_bytes(&self) -> u64 {
        self.mailbox.values().flat_map(|m| m.values()).map(|x| x.sealed.len() as u64).sum()
    }

    /// What this host offers the backbone (see `dht::Caps`).
    fn my_caps(&self) -> dht::Caps {
        let free = if self.mail_quota == 0 { u32::MAX } else { (self.mail_quota.saturating_sub(self.mail_bytes()) / (1024 * 1024)) as u32 };
        dht::Caps {
            version: env!("CARGO_PKG_VERSION").to_string(),
            server: self.backbone,
            radio: self.links.values().any(|(n, _)| link_kind(n) != "backbone"),
            regions: self.router.regions.iter().copied().collect(),
            post_office: !self.no_post_office && (self.mail_quota == 0 || self.mail_bytes() < self.mail_quota),
            mail_free_mb: free,
            relay: !self.no_relay && self.dht.public_ep.is_some(),
        }
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
            if crate::extra_links::is_backbone(&name) {
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
        let Some(cmd) = self.person_command(id, cmd) else { return };
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
                        self.person_sent(dest, &msg);
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
                // Nothing typed on the bulletin channel is signed: alerts are
                // issued on purpose with SendAlert and a separate issuer key.
                if channel == bulletin::channel() {
                    return self.send_client(id, &err_ev(req, error_code::NOT_IN_CHANNEL, "Alerts are issued with the alert form (an issuer key is needed)"));
                }
                let body: &[u8] = &msg;
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
            Command::SetWifi(_) | Command::GetWifi | Command::SetModemKey { .. } => self.send_client(
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
            Command::SendAlert { req, meta, text: body } => match self.send_alert(&meta, &body) {
                Ok(serial) => self.send_client(id, &Event::Sent { req, msg_id: serial as u64 }),
                Err((code, e)) => self.send_client(id, &err_ev(req, code, e)),
            },
            // Handled in node/person.rs before we get here.
            Command::LinkDevice { .. } | Command::GetPerson | Command::PersonSetup { .. } => {}
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
            (stat::ROLE, self.router.role.as_u8() as i32),
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
        if let Some((from, ep, req)) = dht::decode_req(data) {
            let r = format!("{req:?}");
            crate::debug::push("dht", serde_json::json!({"op": "request served", "from": crate::debug::hex(&from.0), "ep": ep, "req": r.chars().take(100).collect::<String>()}));
            if let Some(t) = self.tele.as_mut() {
                t.dht(&format!("served {}", r.split([' ', '{', '(']).next().unwrap_or("?")));
            }
        }
        let me = self.addr;
        let Some((from, from_ep, req)) = dht::decode_req(data) else {
            return dht::encode_resp(&me, &dht::Resp::Stored(false));
        };
        if crate::guard::node_blocked(&from) {
            return dht::encode_resp(&me, &dht::Resp::Stored(false));
        }
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
            dht::Req::MailStore { recipient, sealed } => {
                // Refused (Stored(false)) without a post office or over the quota.
                dht::Resp::Stored(self.store_mail(recipient, sealed, false))
            }
            dht::Req::Caps => dht::Resp::Caps(self.my_caps()),
            dht::Req::MailFetch { recipient, have } => dht::Resp::Mail(
                self.mailbox
                    .get(&recipient)
                    .map(|m| m.iter().filter(|(id, _)| !have.contains(&((**id >> 32) as u32))).map(|(_, v)| v.sealed.clone()).collect())
                    .unwrap_or_default(),
            ),
            dht::Req::MailDelivered { recipient, ids } => {
                self.drop_mail(recipient, &ids);
                dht::Resp::Stored(true)
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
        crate::debug::push("dht", serde_json::json!({
            "op": if publish { "publish done" } else { "lookup done" },
            "target": crate::debug::hex(&target.0),
            "found": result.record.as_ref().map(|r| r.ep.clone()),
        }));
        if let Some(t) = self.tele.as_mut() {
            t.dht(if publish { "published" } else if result.record.is_some() { "lookup found" } else { "lookup not found" });
        }
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

    /// Backbone upkeep: keep `min_peers` backbone links open (to nodes from
    /// the DHT, spread over the ID space), save the known nodes, and print a
    /// status line every 10 minutes.
    fn backbone_upkeep(&mut self) {
        let now = Instant::now();
        let up: Vec<String> = self
            .links
            .values()
            .filter(|(n, _)| crate::extra_links::is_backbone(n))
            .map(|(n, _)| n.clone())
            .collect();
        if self.min_peers > 0 && up.len() < self.min_peers && now >= self.next_peer_try {
            self.next_peer_try = now + Duration::from_secs(60);
            if let Some(tx) = self.tx.clone() {
                // Spread: the closest contact in every other bucket first.
                let mut cands = self.dht.table.spread();
                cands.retain(|c| !self.dht.peers.contains(&c.ep) && self.dht.public_ep.as_ref() != Some(&c.ep));
                for c in cands.into_iter().take(self.min_peers - up.len()) {
                    self.dht.peers.insert(c.ep.clone());
                    self.broadcast_log(format!("Backbone: connecting to {} to keep {} links", c.ep, self.min_peers));
                    crate::links::spawn_tcp_peer(c.ep, tx.clone());
                }
            }
        }
        // Saved when the table changed (checked every 30 s) and every 10 min.
        // Ask a few DHT nodes what they offer (again after 30 minutes).
        if now >= self.next_caps_poll {
            self.next_caps_poll = now + Duration::from_secs(60);
            if let Some(tx) = self.tx.clone() {
                let due: Vec<Contact> = self
                    .dht
                    .table
                    .all()
                    .into_iter()
                    .filter(|c| self.caps_of.get(&c.id).is_none_or(|(_, t)| t.elapsed() > Duration::from_secs(1800)))
                    .take(6)
                    .collect();
                let me = self.addr;
                let my_ep = self.dht.public_ep.clone();
                for c in due {
                    let tx = tx.clone();
                    let my_ep = my_ep.clone();
                    std::thread::spawn(move || {
                        let caps = match dht::rpc(&c.ep, &me, my_ep.as_deref(), &dht::Req::Caps) {
                            Some((_, dht::Resp::Caps(k))) => Some(k),
                            _ => None,
                        };
                        let _ = tx.send(In::DhtCaps { id: c.id, caps });
                    });
                }
            }
        }
        if now >= self.next_contacts_save {
            self.next_contacts_save = now + Duration::from_secs(30);
            let all = self.dht.table.all();
            if all.len() != self.saved_contacts || now >= self.next_status {
                self.saved_contacts = all.len();
                dht::save_contacts(&self.data_dir.join(DHT_CONTACTS_FILE), &all);
            }
        }
        if self.backbone && now >= self.next_status {
            self.next_status = now + Duration::from_secs(600);
            println!(
                "[status] links {} (backbone {}), DHT nodes {}, records {}, reachable addresses {}, forwarded {}, mail kept {}",
                self.links.len(),
                up.len(),
                self.dht.table.len(),
                self.dht.store.len(),
                self.dht.heard.len(),
                self.router.stats.forwarded,
                self.mailbox.values().map(|m| m.len()).sum::<usize>(),
            );
        }
    }

    /// maarse-server: forwarded counts into the minute series, the
    /// telemetry JSON for the dashboard every 10 s, saved every 5 min.
    fn telemetry_upkeep(&mut self) {
        let fw = self.router.stats.forwarded;
        let me = self.ext.sharing.position().map(|p| (p.lat_e7 as f64 / 1e7, p.lon_e7 as f64 / 1e7));
        let now = Instant::now();
        let Some(t) = self.tele.as_mut() else { return };
        for _ in 0..fw.saturating_sub(self.last_forwarded).min(10_000) {
            t.forwarded();
        }
        self.last_forwarded = fw;
        if now >= self.next_tele {
            self.next_tele = now + Duration::from_secs(10);
            crate::debug::set_telemetry(t.json(me).to_string());
        }
        if now >= self.next_tele_save {
            self.next_tele_save = now + Duration::from_secs(300);
            t.save(&self.data_dir.join("telemetry.json"));
        }
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
        let no_relay = self.no_relay;
        let heard_via = &self.heard_via;
        let me = self.addr;
        let due: Vec<Address> = self
            .dht
            .heard
            .iter()
            // Without relaying, only what we reach over our own radio.
            .filter(|(a, _)| !no_relay || **a == me || heard_via.get(a).is_some_and(|(_, tcp, _)| !*tcp))
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
            serial_links: Vec::new(),
            kiss: Vec::new(),
            kiss_tcp: Vec::new(),
            kiss_opts: Default::default(),
            udp_listen: None,
            udp_peers: Vec::new(),
            udp_broadcast: None,
            open_browser: false,
            require_key: false,
            max_file: 1024 * 1024,
            filebox_per_recipient: 20 << 20,
            filebox_total: 200 << 20,
            postbox: None,
            public_addr: None,
            dht_seeds: Vec::new(),
            seed_dns: Vec::new(),
            min_peers: 0,
            backbone: false,
            no_post_office: false,
            no_relay: false,
            mail_quota_mb: 0,
            modem_key: None,
            modem_ips: Vec::new(),
            block: Vec::new(),
            regions: Vec::new(),
            strict_region: false,
            repeater_role: false,
            trunks: Vec::new(),
            no_bordercast: false,
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
            alert_roots: Vec::new(),
            issuer_key: None,
            issuer_cert: None,
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
        n.heard_via.insert(peer, (id, crate::extra_links::is_backbone(link), hops));
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
    fn trunk_is_a_backup_and_carries_only_what_must_cross() {
        let (mut n, _rx, dir) = node("trunk");
        let far = Address([9; 16]);
        heard(&mut n, far, "trunk COM9", 0);
        let trunk = n.heard_via[&far].0;
        n.trunk_peers.entry(trunk).or_default().insert(far);
        let pr = |dest: Address| {
            let mut p: maarse_core::router::Buf = maarse_core::router::Buf::from_slice(&[7; 16]).unwrap();
            assert!(stamp::append(PacketType::PathRequest, &dest, &mut p));
            encode(&Header::new(PacketType::PathRequest, dest), &p).unwrap()
        };
        let elsewhere = Address([5; 16]);
        // No TCP: a request for a node not known here crosses.
        assert!(n.trunk_allows(trunk, &pr(elsewhere)));
        // A node heard over our own radio: only on the second request.
        let local = Address([6; 16]);
        heard(&mut n, local, "serial COM6", 0);
        assert!(!n.trunk_allows(trunk, &pr(local)));
        assert!(n.trunk_allows(trunk, &pr(local)));
        // Someone else's announce never crosses; our own hello does.
        let other_ann = encode(&Header::new(PacketType::Announce, elsewhere), &[0; 8]).unwrap();
        assert!(!n.trunk_allows(trunk, &other_ann));
        let hello = encode(&Header::new(PacketType::Hello, n.addr), &[0, 0, 0xFF, 0xFF]).unwrap();
        assert!(n.trunk_allows(trunk, &hello));
        // TCP reaches the far end: requests stay off the trunk.
        heard(&mut n, far, "tcp out 127.0.0.1:4242", 0);
        assert!(!n.trunk_allows(trunk, &pr(elsewhere)));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn deposit_ops_store_and_delete() {
        use maarse_core::mail;
        let (mut n, _rx, dir) = node("postoffice");
        let r = Address([3; 16]);
        let mut p: Vec<u8> = Vec::new();
        p.extend_from_slice(&r.0);
        p.push(mail::OP_STORE);
        p.extend_from_slice(b"sealed message");
        n.on_deposit(&p, 0);
        assert_eq!(n.mailbox.get(&r).map(|m| m.len()), Some(1));
        assert!(n.router.mail_waiting.contains(&r.path_id()), "told in our hello");
        let id = (maarse_core::dedup::packet_id(PacketType::Data, &r, b"sealed message") >> 32) as u32;
        let mut d: Vec<u8> = r.0.to_vec();
        d.push(mail::OP_DELIVERED);
        d.extend_from_slice(&id.to_be_bytes());
        n.on_deposit(&d, 0);
        assert_eq!(n.mailbox.get(&r).map(|m| m.len()), Some(0));
        assert!(n.router.mail_waiting.is_empty());
        let _ = fs::remove_dir_all(dir);
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

    /// A channel payload on #bulletin from `from`, signed by `signer` (v1).
    fn sealed(from: &Node, signer: &Node, text: &[u8]) -> Vec<u8> {
        let mut plain = [0u8; 255];
        let n = bulletin::sign(&signer.identity, unix_now(), text, &mut plain).unwrap();
        let mut ct = [0u8; 255];
        let m = seal::seal_channel(&bulletin::channel_key(), &from.addr, [1; 8], &plain[..n], &mut ct).unwrap();
        ct[..m].to_vec()
    }

    fn shown(rx: &Receiver<Vec<u8>>) -> Vec<String> {
        rx.try_iter()
            .filter_map(|f| Event::decode(&f[2..]).ok())
            .filter_map(|e| match e {
                Event::Alert { text, trust, .. } if trust == maarse_core::companion::alert_trust::LEGACY => {
                    Some(String::from_utf8_lossy(&text).into_owned())
                }
                Event::ChannelReceived { .. } | Event::Alert { .. } => panic!("unexpected event {e:?}"),
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

        // An old v1 bulletin is shown as an alert event (never as chat text),
        // once.
        let p = sealed(&auth, &auth, b"Bridge closed");
        b.on_deliver(PacketType::Channel, ch, &p, 2);
        b.on_deliver(PacketType::Channel, ch, &p, 2);
        assert_eq!(shown(&rx), vec!["Bridge closed".to_string()]);

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


#[cfg(test)]
mod caps_tests {
    //! What a host offers the backbone (dht::Caps) and that it keeps to it.
    use super::*;

    fn node(name: &str, f: impl FnOnce(&mut Config)) -> Node {
        let dir = std::env::temp_dir().join(format!("maarse-caps-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut c = file_interop_tests::cfg(dir);
        f(&mut c);
        Node::new(&c).unwrap()
    }

    fn ask(n: &mut Node, req: dht::Req) -> dht::Resp {
        let b = dht::encode_req(&Address([9; 16]), None, &req);
        dht::decode_resp(&n.dht_serve(&b)).unwrap().1
    }

    #[test]
    fn no_post_office_refuses_copies_and_says_so() {
        let mut n = node("npo", |c| c.no_post_office = true);
        let dht::Resp::Caps(c) = ask(&mut n, dht::Req::Caps) else { panic!("no caps") };
        assert!(!c.post_office);
        assert_eq!(n.router.mail_class, maarse_core::mail::CLASS_NONE, "hellos say no post office");
        let r = ask(&mut n, dht::Req::MailStore { recipient: Address([3; 16]), sealed: vec![1; 40] });
        assert_eq!(r, dht::Resp::Stored(false));
        assert!(n.mailbox.is_empty());
    }

    #[test]
    fn quota_is_kept_and_told() {
        let mut n = node("quota", |c| c.mail_quota_mb = 1);
        n.mail_quota = 2000; // bytes, small for the test
        let msg = |b: u8| vec![b; 900];
        assert_eq!(ask(&mut n, dht::Req::MailStore { recipient: Address([3; 16]), sealed: msg(1) }), dht::Resp::Stored(true));
        assert_eq!(ask(&mut n, dht::Req::MailStore { recipient: Address([4; 16]), sealed: msg(2) }), dht::Resp::Stored(true));
        assert_eq!(ask(&mut n, dht::Req::MailStore { recipient: Address([5; 16]), sealed: msg(3) }), dht::Resp::Stored(false), "over the quota");
        let dht::Resp::Caps(c) = ask(&mut n, dht::Req::Caps) else { panic!() };
        assert!(c.post_office && c.mail_free_mb == 0, "still a post office, but full");
    }


    #[test]
    fn replicas_skip_hosts_without_a_post_office() {
        let mut n = node("rep", |_| {});
        for i in 1..=5u8 {
            n.dht.table.insert(Contact { id: Address([i; 16]), ep: format!("10.0.{i}.1:4242") });
        }
        let r = Address([0x77; 16]);
        let before: Vec<Address> = n.mail_replicas(&r).iter().map(|c| c.id).collect();
        let banned = before[0];
        n.caps_of.insert(banned, (dht::Caps { post_office: false, ..Default::default() }, Instant::now()));
        let after: Vec<Address> = n.mail_replicas(&r).iter().map(|c| c.id).collect();
        assert!(!after.contains(&banned));
        assert_eq!(after.len(), before.len());
    }
}

#[cfg(test)]
mod next_hop_tests {
    //! Routed packets go only to the next hop's link (and a retry everywhere).
    use super::*;
    use std::sync::mpsc::{Receiver, channel};

    fn node() -> (Node, Receiver<Vec<u8>>, Receiver<Vec<u8>>) {
        let dir = std::env::temp_dir().join(format!("maarse-nexthop-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut n = Node::new(&file_interop_tests::cfg(dir)).unwrap();
        let (ta, ra) = channel();
        let (tb, rb) = channel();
        n.links.insert(1, ("tcp in 10.0.0.1:1".into(), ta));
        n.links.insert(2, ("tcp in 10.0.0.2:1".into(), tb));
        (n, ra, rb)
    }

    #[test]
    fn routed_packet_goes_to_one_link_and_a_retry_to_all() {
        let (mut n, ra, rb) = node();
        // Path ID 0x2222 was the previous hop of a packet that came in on link 2.
        let mut h = Header::new(PacketType::Data, Address([5; 16])).with_path(heapless::Vec::from_slice(&[0x2222, 0x3333]).unwrap());
        h.hops = 1;
        n.learn_pid_link(&encode(&h, b"x").unwrap(), 2);
        // Our own packet whose next hop is 0x2222.
        let mut out = Header::new(PacketType::Data, Address([6; 16])).with_path(heapless::Vec::from_slice(&[0x2222]).unwrap());
        out.hops = 0;
        let pkt = encode(&out, b"hello").unwrap();
        n.send_links(&pkt);
        assert_eq!((ra.try_iter().count(), rb.try_iter().count()), (0, 1), "only the next hop's link");
        n.send_links(&pkt);
        assert_eq!((ra.try_iter().count(), rb.try_iter().count()), (1, 1), "a retry goes everywhere");
        // A flood still goes everywhere.
        n.send_links(&encode(&Header::new(PacketType::Announce, Address([7; 16])), b"a").unwrap());
        assert_eq!((ra.try_iter().count(), rb.try_iter().count()), (1, 1));
        // Unknown next hop: everywhere.
        let mut u = Header::new(PacketType::Data, Address([8; 16])).with_path(heapless::Vec::from_slice(&[0x4444]).unwrap());
        u.hops = 0;
        n.send_links(&encode(&u, b"y").unwrap());
        assert_eq!((ra.try_iter().count(), rb.try_iter().count()), (1, 1));
    }
}

/// `t` as unix ms (0 before 1970).
fn unix_ms(t: SystemTime) -> u64 {
    t.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}
