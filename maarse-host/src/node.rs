//! The node's main loop: routing, encryption, announces, files, mailboxes and
//! web clients.

mod ext;
mod pages;
mod pagelock;
mod calltest;
mod backbone;
mod tun;
mod hist;
mod person;
mod sensors;
pub(crate) use person::{BACKUP_FILE, RESTORE_FILE};
mod seq;
mod identity;
mod alerts;
mod debugview;
mod announce_freshness;

use crate::dht::{self, Contact, Record};
use crate::filebox::{self, Boxed, FileBox, LORA_MAX_FILE, PathKind};
use crate::{ClientId, Config, In, LinkId, LinkSender};
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
use maarse_core::{Action, Address, PacketType, Router};
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
/// Chunks of a file that may be in flight at once over the internet.
const FILE_WINDOW: usize = 8;
/// ... and when LoRa may be on the way: one at a time. A LoRa radio is half
/// duplex, and a chunk takes about 1.2 s on air (SF9): a burst of 8 kept the
/// sending radio deaf to the acks for ~10 s, the acks were lost, the route
/// was "repaired" and everything was sent again (the phone node's modem).
const FILE_WINDOW_LORA: usize = 1;
/// A chunk is resent if it is not acknowledged within this time.
const CHUNK_TIMEOUT: Duration = Duration::from_secs(20);
/// ... when LoRa may be on the way, where chunks go one at a time: a round
/// trip on one hop is 2-3 s (chunk 1.2 s + ack 0.2 s on air at SF9), and a
/// lost chunk must not stall the file for 20 s.
const CHUNK_TIMEOUT_LORA: Duration = Duration::from_secs(8);

/// The resend timeout over LoRa from the measured round trip (the idea from
/// Reticulum's Resource: timeouts from measured speed, not fixed values):
/// 2.5 x smoothed RTT + 0.5 s, within 2.5..20 s; 8 s before the first sample.
fn lora_chunk_timeout(srtt: Option<Duration>) -> Duration {
    srtt.map_or(CHUNK_TIMEOUT_LORA, |r| {
        (r * 5 / 2 + Duration::from_millis(500)).clamp(Duration::from_millis(2500), CHUNK_TIMEOUT)
    })
}

/// Updates a smoothed round trip with a new sample (1/4 weight).
fn smooth_rtt(srtt: &mut Option<Duration>, sample: Duration) {
    *srtt = Some(match *srtt {
        None => sample,
        Some(r) => (r * 3 + sample) / 4,
    });
}

/// Pull transfers: first batch, largest batch direct and behind relays.
const PULL_WINDOW_START: u8 = 4;
const PULL_WINDOW_MAX: u8 = 8;
const PULL_WINDOW_MAX_RELAYED: u8 = 2;
/// Requests in a row without a chunk before the recipient gives up asking
/// (the sender then offers again or puts the file in its mailbox).
const PULL_MAX_RETRIES: u8 = 12;
/// Pull requests are sealed with the file key like chunks, with an index
/// from here up (a fresh one per request, so no nonce is used twice; chunk
/// indices stay below it: 32768 chunks are far more than the largest file).
const PULL_REQ_INDEX: u16 = 0x8000;

/// A pull sender that hears no request for this long offers the file again
/// (the recipient answers with a request for what it is missing).
const PULL_STALL: Duration = Duration::from_secs(30);

/// Own beacons (Hello, Announce) wait this long at most for a LoRa file
/// transfer: they collided with chunks and acks on the half-duplex radio.
const BEACON_DEFER_MAX: Duration = Duration::from_secs(120);
/// A file is abandoned after this time.
const FILE_TIMEOUT: Duration = Duration::from_secs(600);
const MAX_INCOMING_FILES: usize = 32;
const MAX_INCOMING_FILES_PER_SENDER: usize = 8;
/// A page answer the reader has not taken by then is dropped (the app
/// stops waiting for a page after a minute, for a picture after two).
const PAGE_REPLY_TIMEOUT: Duration = Duration::from_secs(150);
/// Post office and backbone timings: the standard profile from
/// maarse-hostrules (the simulator can load others; the host runs this one).
const RULES: maarse_hostrules::rules::Rules = maarse_hostrules::rules::Rules::STANDARD;
/// Without an acknowledgement within this time a message is put in the recipient's mailbox.
const DEPOSIT_AFTER: Duration = Duration::from_millis(RULES.deposit_after_ms);
/// The mailbox keeps at most this many messages per recipient, for this long.
const MAX_MAIL_JOBS: usize = 16;
const FETCHED_MAIL_CEILING: u64 = 4 * 1024 * 1024;
const MAIL_RECORD_OVERHEAD: u64 = 16 + 8 + 2;
const MAILBOX_PER_RECIPIENT: usize = RULES.mailbox_per_recipient;
const MAILBOX_TTL: Duration = Duration::from_millis(RULES.mailbox_ttl_ms);
/// How often the DHT is maintained (bootstrap, publications, expiry).
const DHT_EVERY: Duration = Duration::from_millis(RULES.dht_every_ms);
/// The same address is looked up at most this often.
const DHT_LOOKUP_EVERY: Duration = Duration::from_millis(RULES.dht_lookup_every_ms);
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
    Observe,
    CallTest,
    /// A chunk of this file may be due for resending (LoRa timeout).
    FileCheck(u64),
    /// A pull request for this incoming file may be due again.
    PullCheck(u64),
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
const BORDERCAST_REPEAT: Duration = Duration::from_millis(RULES.bordercast_repeat_ms);
/// Key requests are answered from the announce cache at most this often per
/// destination.
const KEY_ANSWER_EVERY: Duration = Duration::from_millis(RULES.key_answer_every_ms);
/// Post office: how long a recipient counts as present after a sync, how
/// often we ask the replicas for a recipient we hear, and the first retry
/// of a delivery (then doubling, see `Rules::mail_retry_after`).
const MAIL_PRESENT: Duration = Duration::from_millis(RULES.mail_present_ms);
const MAIL_FETCH_EVERY: Duration = Duration::from_millis(RULES.mail_fetch_every_ms);
const MAIL_RETRY_FIRST: Duration = Duration::from_millis(RULES.mail_retry_first_ms);
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
    /// Smoothed round trip (offer or chunk -> its ack), for the resend
    /// timeout over LoRa.
    srtt: Option<Duration>,
    /// Push (we send, each chunk acknowledged) or pull (the recipient asks
    /// for batches); see `SendMode`.
    mode: SendMode,
    created: Instant,
    /// The file's size in bytes (plaintext).
    size: u32,
    /// The sealed offer as it was sent (for the file mailbox).
    sealed_offer: Vec<u8>,
    /// Sealed chunks from the file mailbox (otherwise they are sealed on the way).
    sealed_chunks: Option<Vec<Vec<u8>>>,
    /// Delivered from the file mailbox.
    from_box: bool,
    /// An answer to a page request (a page or a page picture): never put
    /// in the file mailbox, and dropped when the reader does not take it
    /// soon (the reader has stopped waiting; later it would only turn up
    /// as a stray file).
    page: bool,
}

/// How an outgoing file is sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SendMode {
    /// We send, one window at a time, each chunk acknowledged end to end.
    Push,
    /// The recipient takes files the pull way: after the offer is
    /// acknowledged we wait until this time for its first request, then
    /// fall back to pushing.
    AwaitRequest(Instant),
    /// The recipient asks for batches; the time of its latest request.
    Pull(Instant),
}

/// The recipient's side of a pull transfer.
struct PullRx {
    /// Chunks asked for per request (adapts: +1 per clean batch, -1 on a timeout).
    window: u8,
    /// Chunks asked for in the latest request and not yet here.
    asked: Vec<u16>,
    /// When the latest request counts as lost (or its batch as cut short).
    deadline: Instant,
    /// Requests in a row without a chunk arriving.
    retries: u8,
    /// Estimated time per chunk on air plus gaps (from arrivals).
    chunk_ms: u32,
    /// Latest chunk arrival in this batch (for `chunk_ms`).
    last_rx: Option<Instant>,
    /// The batch had a timeout (the window then does not grow).
    troubled: bool,
}

struct InFile {
    from: Address,
    /// The sending device (requests and cancels go here; `from` may be its person).
    sender: Address,
    /// Pull transfer state (the sender takes requests and we are on LoRa).
    pull: Option<PullRx>,
    /// Requests sealed so far (their index is PULL_REQ_INDEX + this).
    req_seq: u16,
    offer: FileOffer,
    parts: Vec<Option<Vec<u8>>>,
    got: u16,
    /// Latest offer or chunk: a half-received file is dropped after
    /// FILE_TIMEOUT without either (not after FILE_TIMEOUT in total: over
    /// LoRa a file from the sender's file mailbox may be resent much later).
    active: Instant,
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
    next_deposit: Option<Instant>,
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
    identity: std::sync::Arc<Identity>,
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
    /// When we last announced because a compact message could not be read.
    lost_session_announce: Option<Instant>,
    postboxes: HashMap<Address, Address>,
    my_postbox: Option<Address>,
    /// Channel address -> (name, key)
    channels: HashMap<Address, (String, [u8; 32])>,
    links: HashMap<LinkId, (String, LinkSender)>,
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
    /// Receipt IDs of mail a post office delivered to us (a `BUNDLE`) that we
    /// have not yet opened: when it opens, its sender gets a delivery receipt.
    receipt_wanted: std::collections::HashSet<u64>,
    max_file: u32,
    // ---------- file limits and file mailbox (see filebox.rs) ----------
    /// The link the packet we are processing came in on.
    rx_link: Option<LinkId>,
    /// SNR of the packet being handled (for the telemetry census).
    rx_snr: Option<i8>,
    /// maarse-server's telemetry (None on an ordinary host).
    tele: Option<Box<crate::telemetry::Telemetry>>,
    observers: crate::observe::Observers,
    public_channels: Vec<crate::observe::PublicChannel>,
    share_open_channels: bool,
    public_traffic: bool,
    open_channels: std::collections::VecDeque<crate::observe::PublicChannel>,
    public_rx_announce: Option<crate::observe::Sample>,
    next_tele: Instant,
    next_tele_save: Instant,
    last_forwarded: u32,
    /// Where each address was last heard: link, TCP?, hops.
    heard_via: HashMap<Address, (LinkId, bool, u8)>,
    /// Every announce we have heard (payload without stamp), to answer key
    /// requests: a host keeps all of them, a radio node cannot.
    announce_cache: HashMap<Address, Vec<u8>>,
    announce_freshness: announce_freshness::AnnounceFreshness,
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
    mail_replica_ids: HashSet<Address>,
    mail_jobs: std::sync::Arc<std::sync::atomic::AtomicUsize>,
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
    /// Hosts found in the DHT (reached over TCP, if through a relay).
    dht_hosts: HashSet<Address>,
    /// Files whose wait for a TCP route has been logged.
    file_wait_logged: HashSet<u64>,
    /// Since when our own beacons wait for a LoRa file transfer.
    beacon_deferred_since: Option<Instant>,
    /// Incoming files completed lately (a late offer is answered, not taken again).
    files_done: HashMap<u64, Instant>,
    /// Incoming files stopped lately, by us or the sender: a late offer must
    /// not start them again (the sender is told to stop once more).
    files_cancelled: HashMap<u64, Instant>,
    /// Pull transfers over LoRa on (off only to compare in tests).
    file_pull: bool,
    /// Answers calls by itself when `<data>/calltest/` exists.
    call_test: calltest::CallTest,
    /// When each link came up (backbone menu).
    link_up: HashMap<LinkId, Instant>,
    /// Sequence numbers per conversation (node/seq.rs).
    seqs: seq::Seqs,
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
    allowed_endpoints: Vec<String>,
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
        let identity = match identity::read_identity(&cfg.data_dir)? {
            Some(identity) => identity,
            None => {
                let id = Identity::generate(&mut OsRng);
                identity::write_identity(&cfg.data_dir, &id.to_secret_bytes())?;
                id
            }
        };
        let addr = identity.address();
        // The role chosen under Settings; by default an anchor (it splits long routes at the host, so a
        // path can be 12 hops on each side of the backbone), and for a phone node a client.
        let mut router = Router::new(addr, cfg.host_role().router_role());
        // Hellos tell the radio nodes nearby that a repeater/anchor is here,
        // and that we are a large post office.
        router.hello = true;
        // A host keeps mail (it has a disk), unless told not to.
        router.mail_class = if cfg.no_post_office { maarse_core::mail::CLASS_NONE } else { maarse_core::mail::CLASS_LARGE };
        router.ring = 4;
        // A host forwards every path request: its links are mostly TCP (no airtime to save), and it is
        // often the one way between two networks. Radio nodes drop one request in five (see the router).
        router.request_fwd_permille = 1000;
        // Only fast links here: short time slots, otherwise every hop waits needlessly.
        router.slot_ms = 5;

        let mut node = Node {
            identity: identity.into(),
            addr,
            router: Box::new(router),
            known: HashMap::new(),
            fs_peers: HashSet::new(),
            sessions: crate::sessions::Sessions::load(cfg.data_dir.join("sessions.bin")),
            resynced: HashMap::new(),
            lost_session_announce: None,
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
            pages: pages::Pages::new(cfg),
            tun_tx: None,
            tunnel: Default::default(),
            tun_lora: cfg.tun_lora,
            out_files: HashMap::new(),
            in_files: HashMap::new(),
            mailbox: HashMap::new(),
            mail_alias: HashMap::new(),
            receipt_wanted: std::collections::HashSet::new(),
            max_file: cfg.max_file,
            rx_link: None,
            rx_snr: None,
            tele: (cfg.backbone || cfg.public_map).then(|| Box::new(crate::telemetry::Telemetry::load(&cfg.data_dir.join("telemetry.json")))),
            observers: crate::observe::Observers::new(cfg.observe_to.clone(), &cfg.observe_allow),
            public_channels: cfg.public_channels.clone(),
            share_open_channels: cfg.share_open_channels,
            public_traffic: cfg.public_traffic,
            open_channels: std::collections::VecDeque::new(),
            public_rx_announce: None,
            next_tele: Instant::now() + Duration::from_secs(5),
            next_tele_save: Instant::now() + Duration::from_secs(300),
            last_forwarded: 0,
            heard_via: HashMap::new(),
            announce_cache: HashMap::new(),
            announce_freshness: announce_freshness::AnnounceFreshness::open(&cfg.data_dir.join("announce-freshness.bin"), unix_now())?,
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
            mail_replica_ids: cfg.mail_replica_ids.iter().copied().collect(),
            mail_jobs: Default::default(),
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
                allowed_endpoints: cfg.dht_seeds.iter().chain(&cfg.peers)
                    .chain(&cfg.dht_private_endpoints).cloned().collect(),
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
            dht_hosts: HashSet::new(),
            file_wait_logged: HashSet::new(),
            beacon_deferred_since: None,
            files_done: HashMap::new(),
            files_cancelled: HashMap::new(),
            file_pull: true,
            call_test: calltest::CallTest::load(&cfg.data_dir),
            link_up: HashMap::new(),
            seqs: seq::Seqs::load(&cfg.data_dir),
            bulletin_keys: cfg.bulletin_keys.clone(),
            alerts: alerts::Alerts::load(cfg)?,
        };
        node.sensors_open();
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
        // What other nodes can do (file pull, compressed text, ...), from
        // their announces. Kept on disk: announces come every 10 minutes
        // (later during a file transfer), and without the caps a restarted
        // node took files the slow push way until it heard one.
        let saved = fs::read_to_string(node.data_dir.join(CAPS_FILE)).unwrap_or_default();
        for line in saved.lines() {
            if let Some((a, c)) = line.split_once(' ')
                && let (Some(a), Ok(c)) = (parse_addr(a), c.trim().parse::<u8>())
            {
                node.caps.insert(a, c);
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
        if self.observers.target.is_some() {
            self.schedule(crate::observe::FLUSH, Timer::Observe);
        }
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
                    if !self.cancelled.remove(&id) && !self.defer_beacon(&packet, id, only) {
                        self.send_links_only(&packet, only);
                    }
                }
                Some(Timer::FileCheck(file_id)) => self.file_resend_check(file_id),
                Some(Timer::PullCheck(file_id)) => self.pull_check(file_id),
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
                Some(Timer::CallTest) => self.call_test_tick(),
                Some(Timer::Dht) => {
                    self.dht_maintain();
                    self.schedule(DHT_EVERY, Timer::Dht);
                }
                Some(Timer::Observe) => {
                    self.flush_observations();
                    self.schedule(crate::observe::FLUSH, Timer::Observe);
                }
                None => {}
            }
        }
    }

    fn handle(&mut self, msg: In) {
        match msg {
            In::LinkUp { id, name, tx } => {
                self.log(format!("link up: {name}"));
                self.link_up.insert(id, Instant::now());
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
                self.link_up.remove(&id);
                self.link_caps.remove(&id);
                self.heard_via.retain(|_, v| v.0 != id);
                if let Some((name, _)) = self.links.remove(&id) {
                    self.log(format!("link down: {name}"));
                    self.broadcast_log(format!("Link down ({} left)", self.links.len()));
                }
            }
            In::ObservationReport(report) => self.observers.receive(&self.identity, &report),
            In::Packet { data, snr, link, open_channel } => {
                // Blocked nodes (guard.rs): nothing to, from or about them.
                if let Ok((h, _)) = Header::decode(&data)
                    && crate::guard::node_blocked(&h.dest)
                {
                    return;
                }
                self.counters.packets_in += 1;
                self.counters.bytes_in += data.len() as u64;
                if let Ok(packet) = Packet::decode(&data) {
                    if let Some(channel) = open_channel.filter(|channel| channel.is_for(&packet.header)) {
                        self.open_channels.retain(|known| known.route() != channel.route());
                        self.open_channels.truncate(255);
                        self.open_channels.push_front(channel);
                    }
                    let announce = self.public_traffic && packet.header.ptype == PacketType::Announce;
                    let open = self.record_open_channel(&packet);
                    if (announce || open || self.observers.enabled())
                        && let Some((name, _)) = self.links.get(&link)
                    {
                        let measured = ["serial ", "wifi-modem", "modem ", "trunk "].iter().any(|prefix| name.starts_with(prefix)).then_some(snr);
                        let sample = crate::observe::Sample::new(&packet, link_kind(name), false, measured);
                        if announce {
                            self.public_rx_announce = Some(sample);
                        }
                        self.observers.record(self.addr, sample, !announce && (open || self.observers.collecting()));
                    }
                }
                self.learn_pid_link(&data, link);
                {
                    let c = self.link_stats.entry(link).or_default();
                    c[0] += 1;
                    c[2] += data.len() as u64;
                    let mut f = crate::debug::packet_fields(&data);
                    f["link"] = self.links.get(&link).map(|(n, _)| n.clone()).unwrap_or_default().into();
                    f["snr"] = snr.into();
                    let named = self.links.get(&link).map(|(n, _)| (n.clone(), link_kind(n)));
                    if crate::web::PUBLIC_TRAFFIC.load(std::sync::atomic::Ordering::Relaxed)
                        && f.get("pid").is_some()
                        && let Some((_, kind)) = &named
                    {
                        crate::debug::count_public_packet(false, kind);
                        crate::public_history::count(false, kind, f.get("ptype").and_then(|v| v.as_str()).unwrap_or("?"));
                    }
                    crate::debug::push("rx", f);
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
                self.public_rx_announce = None;
            }
            In::ClientUp { id, tx } => {
                self.clients.insert(id, tx);
                self.greet(id);
            }
            In::ClientDown { id } => {
                self.clients.remove(&id);
                self.uploads.retain(|(c, _), _| *c != id);
            }
            In::DhtRequest { data, challenge, reply } => {
                let response = self.dht_serve(&data);
                let response = match challenge {
                    Some(challenge) => dht::sign_reply(&self.identity, &challenge, &data, response),
                    None => response,
                };
                let _ = reply.send(response);
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
                    // GPS details, spectrum rows and a spectrum recording (state, and the file the radio hands over) go to
                    // the clients as they are. A radio in modem mode does not receive files from the air, so a file
                    // event from it can only be its own recording.
                    Ok(ev @ (Event::Gps { .. } | Event::Sweep { .. } | Event::SweepRecState { .. } | Event::FileIncoming { .. } | Event::FileChunk { .. } | Event::FileDone { .. })) => {
                        let f = ev.encode().to_vec();
                        for tx in self.clients.values() {
                            let _ = tx.send(f.clone());
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
            In::MailReplicated { msg_id } => self.mark_mail_deposited(msg_id),
            In::MailFetched { recipient, items } if self.person_mail_fetched(recipient, &items) => {}
            In::MailFetched { recipient, items } => {
                let n = items.len();
                for sealed in items {
                    // Fetched to hand over to a recipient on our radio: kept
                    // for delivery even without a post office here.
                    self.store_mail_local(recipient, sealed, false, true);
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
            In::DhtCheckDone { client, req, lines } => {
                let n = lines.len();
                for (i, (level, text)) in lines.into_iter().enumerate() {
                    self.send_client(client, &Event::DhtLine { req, level, last: i + 1 == n, text: maarse_core::companion::text(text.as_bytes()) });
                }
            }
            In::ClientFrame { id, frame } if is_get_backbones(&frame) => self.backbones(id),
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

    fn open_channel_name(&self, header: &Header) -> Option<crate::observe::PublicChannel> {
        if header.ptype != PacketType::Channel || header.path.is_some() {
            return None;
        }
        self.channels.get(&header.dest)
            .and_then(|(name, key)| crate::observe::PublicChannel::from_local(name, key, header))
            .or_else(|| self.open_channels.iter().find(|channel| channel.is_for(header)).cloned())
    }

    fn record_open_channel(&self, packet: &Packet<'_>) -> bool {
        if packet.header.ptype != PacketType::Channel {
            return false;
        }
        if let Some(channel) = self.public_channels.iter().find(|channel| channel.is_for(&packet.header)) {
            return channel.record(packet);
        }
        self.share_open_channels
            && self.open_channel_name(&packet.header)
                .or_else(|| crate::observe::PublicChannel::builtin(&packet.header))
                .is_some_and(|channel| channel.record(packet))
    }

    /// A trace route asked to use only the radio or only the internet links
    /// (companion command 0x20, `via`) keeps its path request and probe on that kind of
    /// link for a minute; everything else goes where it always went.
    fn forced_only(&mut self, packet: &[u8], only: Only) -> Only {
        if only != Only::All || self.ext.forced.is_empty() {
            return only;
        }
        self.ext.forced.retain(|_, (_, t)| t.elapsed() < Duration::from_secs(60));
        let Ok((h, _)) = Header::decode(packet) else { return only };
        if !matches!(h.ptype, PacketType::PathRequest | PacketType::Probe) {
            return only;
        }
        self.ext.forced.get(&h.dest).map_or(only, |(o, _)| *o)
    }

    fn send_links_only(&mut self, packet: &[u8], only: Only) {
        let only = self.forced_only(packet, only);
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
        let descriptor = header.as_ref().and_then(|header| self.open_channel_name(header)).map(|channel| channel.frame());
        let decoded = if self.observers.enabled() || self.share_open_channels || !self.public_channels.is_empty() {
            Packet::decode(packet).ok()
        } else {
            None
        };
        let mut open = None;
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
            let queued = if let Some(name) = &descriptor
                && self.link_caps.get(id).is_some_and(|caps| caps.open_channel_names)
            {
                tx.send_named_channel(packet.to_vec(), name.clone())
            } else {
                tx.send(packet.to_vec()).is_ok()
            };
            if queued && let Some(decoded) = &decoded {
                let open = *open.get_or_insert_with(|| self.record_open_channel(decoded));
                if open || self.observers.enabled() {
                    let sample = crate::observe::Sample::new(decoded, link_kind(name), true, None);
                    self.observers.record(self.addr, sample, open || self.observers.collecting());
                }
            }
            if queued && header.is_some() && crate::web::PUBLIC_TRAFFIC.load(std::sync::atomic::Ordering::Relaxed) {
                crate::debug::count_public_packet(true, link_kind(name));
                if let Some(h) = &header {
                    crate::public_history::count(true, link_kind(name), &format!("{:?}", h.ptype));
                }
            }
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
                    if self.defer_beacon(&packet, id, only) {
                        // waits for the file transfer (see defer_beacon)
                    } else if delay_ms == 0 {
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
        // Messages are numbered for nodes that read it, when it still fits.
        let numbered;
        let plain = if matches!(kind, Pending::Message)
            && matches!(plain.first(), Some(&content::TEXT) | Some(&content::TEXT_Z))
            && plain.len() + 5 <= MAX_FS_TEXT
            && self.caps.get(&dest).is_some_and(|c| c & maarse_core::appdata::CAP_SEQ != 0)
        {
            let mut v = vec![content::SEQ];
            v.extend_from_slice(&self.seqs.next(&dest).to_le_bytes());
            v.extend_from_slice(plain);
            numbered = v;
            &numbered[..]
        } else {
            plain
        };
        let mut ct = [0u8; 255];
        let fs_ok = self.fs_peers.contains(&dest) || self.sessions.contains(&dest);
        // Compact messages (22 bytes of overhead instead of 69) for a node that reads them.
        let compact_ok = self.caps.get(&dest).is_some_and(|c| c & maarse_core::appdata::CAP_COMPACT != 0);
        let (n, format) = ratchet::seal_compact(
            &self.identity,
            &recipient,
            &mut self.sessions,
            fs_ok,
            compact_ok,
            &mut OsRng,
            plain,
            &mut ct,
        )
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
                    next_deposit: None,
                },
            );
        }
        self.awaiting.insert(msg_id, kind);
        self.apply(actions.into_iter().collect());
        Ok(msg_id)
    }

    /// Call audio: sealed like a message but sent as a datagram, without
    /// end-to-end acknowledgement or resending. A late audio packet is
    /// worthless, and acknowledging 17 packets a second filled the send
    /// queue so packets were dropped (choppy calls). Falls back to an
    /// ordinary message for a peer that does not take call datagrams.
    fn send_call_audio(&mut self, dest: Address, plain: &[u8]) -> Result<(), &'static str> {
        if !self.caps.get(&dest).is_some_and(|c| c & maarse_core::appdata::CAP_CALL_DG != 0) {
            return self.send_sealed(dest, plain, Pending::Control).map(|_| ());
        }
        let recipient = *self.known.get(&dest).ok_or("The recipient has not been heard yet (no announce)")?;
        let mut ct = [0u8; 255];
        let fs_ok = self.fs_peers.contains(&dest) || self.sessions.contains(&dest);
        let (n, _) = ratchet::seal(&self.identity, &recipient, &mut self.sessions, fs_ok, &mut OsRng, plain, &mut ct)
            .map_err(|_| "The message is too long")?;
        self.sessions.save();
        let mut dg = maarse_core::appdata::CALL_DATAGRAM.to_vec();
        dg.extend_from_slice(&ct[..n]);
        let now = self.now_ms();
        let (_, actions) = self
            .router
            .send_typed(PacketType::Datagram, dest, &dg, token(), now)
            .map_err(|e| match e {
                maarse_core::router::SendError::TooLarge => "The call packet is too large",
                _ => "The queue is full, try again shortly",
            })?;
        self.apply(actions.into_iter().collect());
        Ok(())
    }

    /// Sealed content as a datagram (no end-to-end acknowledgement; the
    /// pull transfer resends by itself), like call audio. Falls back to an
    /// ordinary message for a peer that does not take sealed datagrams.
    fn send_sealed_datagram(&mut self, dest: Address, plain: &[u8]) -> Result<(), &'static str> {
        self.send_call_audio(dest, plain)
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
                let mut wait = None;
                if let Some(f) = self.out_files.get_mut(&file_id) {
                    f.offer_acked = true;
                    let sample = f.offer_sent.elapsed();
                    smooth_rtt(&mut f.srtt, sample);
                    wait = Some((f.dest, f.srtt));
                }
                // The recipient takes files the pull way: wait for its request.
                if let Some((dest, srtt)) = wait
                    && self.pull_capable(&dest)
                    && self.out_files.get(&file_id).is_some_and(|f| !matches!(f.mode, SendMode::Pull(_)))
                {
                    let until = lora_chunk_timeout(srtt) * 2 + Duration::from_secs(2);
                    if let Some(f) = self.out_files.get_mut(&file_id) {
                        f.mode = SendMode::AwaitRequest(Instant::now() + until);
                    }
                    self.schedule(until + Duration::from_millis(50), Timer::FileCheck(file_id));
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
                self.counters.mail_delivered += 1;
                self.log(format!("mailbox: delivered to {}", short(&recipient)));
                self.save_mailbox();
            }
        }
    }

    // ---------- reception ----------

    fn on_deliver(&mut self, ptype: PacketType, dest: Address, payload: &[u8], hops: u8) {
        match ptype {
            PacketType::Announce if dest != self.addr => { self.on_announce(dest, payload, hops); }
            // Someone asks who we are: announce. Who another node is: answer
            // from the announces we keep.
            PacketType::KeyRequest if dest == self.addr => self.announce(),
            PacketType::KeyRequest => self.answer_key_request(dest),
            PacketType::MailSync => self.on_mail_sync(dest, payload),
            PacketType::Data => self.open_direct(payload.to_vec(), hops, true),
            PacketType::Fragment => self.on_chunk(payload),
            PacketType::Deposit => self.on_deposit(payload, hops),
            // Call audio: opened like a direct message, but never acknowledged.
            PacketType::Datagram if payload.starts_with(&maarse_core::appdata::CALL_DATAGRAM) => {
                self.open_direct(payload[8..].to_vec(), hops, true)
            }
            // A chunk of a file we pull (its id is one we are receiving).
            PacketType::Datagram
                if seal::chunk_header(payload).is_some_and(|(id, _)| self.in_files.contains_key(&id)) =>
            {
                self.on_chunk(payload)
            }
            // A pull request for a file we send (sealed with its key).
            PacketType::Datagram
                if seal::chunk_header(payload)
                    .is_some_and(|(id, idx)| idx >= PULL_REQ_INDEX && self.out_files.contains_key(&id)) =>
            {
                self.on_sealed_request(payload)
            }
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
                        // A device of a person is shown as the person. One of
                        // our own devices wrote it: it is our own message,
                        // shown on this screen as sent by us.
                        let peer = self.person_peer(&from);
                        if self.is_own_person(&peer) {
                            self.hist_sent(0, crate::history::KIND_CHANNEL, dest, 0, &out[..n]);
                            return;
                        }
                        self.mqtt_channel(&dest, &from, hops, &out[..n]);
                        self.broadcast(&Event::ChannelReceived {
                            channel: dest,
                            from: peer,
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

    fn on_announce(&mut self, dest: Address, payload: &[u8], hops: u8) -> bool {
        match verify_announce(&dest, payload) {
            Ok(a) => {
                match self.announce_freshness.accept(dest, a.timestamp, unix_now()) {
                    Ok(true) => {}
                    Ok(false) => return false,
                    Err(error) => {
                        self.log(format!("could not persist announce freshness: {error}"));
                        return false;
                    }
                }
                if a.app_data.len() >= 17 && a.app_data[0] == APP_POSTBOX
                    && let Some(p) = Address::from_slice(&a.app_data[1..17]) {
                        self.postboxes.insert(dest, p);
                    }
                if ratchet::announces_fs(a.app_data) {
                    self.fs_peers.insert(dest);
                }
                // An announce can mean a restart: the next message to it goes in the long form.
                if let Some(s) = ratchet::SessionStore::get_mut(&mut self.sessions, &dest) {
                    s.force_full();
                }
                self.announce_cache.insert(dest, payload.to_vec());
                let app = maarse_core::appdata::parse(a.app_data);
                if let Some(sample) = self.public_rx_announce.take() {
                    let mobile = app.role_flags.is_some_and(|flags| flags & maarse_core::appdata::ROLE_MOBILE != 0);
                    let subject = (app.position.is_some() && !mobile).then_some(dest);
                    self.observers.publish_announce(self.addr, sample, subject);
                }
                if self.tele.is_some() {
                    let (via, backbone) = self
                        .rx_link
                        .and_then(|l| self.links.get(&l))
                        .map(|(n, _)| (n.clone(), crate::extra_links::is_backbone(n)))
                        .unwrap_or_else(|| ("(local)".into(), false));
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
                true
            }
            Err(e) => {
                self.log(format!("rejected announce from {}: {e:?}", short(&dest)));
                false
            }
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
                // Mail from a post office: tell the sender it arrived.
                let rid = maarse_core::mail::receipt_id(&self.addr, &payload);
                if self.receipt_wanted.remove(&rid) {
                    self.send_receipt(o.from, rid);
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
                // Show it in the conversation too, so the loss is not silent,
                // but once per new session: a burst of old packets (a file
                // transfer, queued messages) gives one notice, not one each.
                if !self.resynced.get(&a).is_some_and(|t| t.elapsed() < RESYNC_EVERY) {
                    self.broadcast(&Event::DirectReceived { from: a, hops, text: text(UNREADABLE_NOTICE.as_bytes()) });
                }
                self.resync(a);
            }
            Err(CryptoError::Decrypt) if payload.first() == Some(&ratchet::COMPACT) => {
                self.counters.errors += 1;
                self.log("compact message could not be read (the session may be lost): announcing, so the sender switches to the long form".to_string());
                if self.lost_session_announce.is_none_or(|t| t.elapsed() > Duration::from_secs(30)) {
                    self.lost_session_announce = Some(Instant::now());
                    self.announce();
                }
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
            Some(Content::BoardPost(path, note)) => {
                if let Err(e) = self.on_board_post(from, &String::from_utf8_lossy(&path), note) {
                    self.log(format!("note from {} not kept: {e}", short(&from)));
                }
            }
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
                self.person_received(peer, from, t);
            }
            Some(Content::Person(o, data)) => self.on_person(from, o, data),
            Some(Content::Seq(n, inner)) => {
                self.seq_received(from, n, hops);
                let inner = inner.to_vec();
                self.on_content(from, &inner, hops, format);
            }
            Some(Content::FileOffer(offer)) => self.on_file_offer(from, offer),
            Some(Content::FileCancel(file_id, by_recipient)) => self.on_file_cancel(from, file_id, by_recipient),
            Some(Content::FileRequest(file_id, base, count, missing)) => self.on_file_request(Some(from), file_id, base, count, missing),
            Some(Content::Control(counter, frame)) => self.on_control(from, counter, frame),
            Some(Content::ControlReply(_, frame)) => {
                if !self.on_sensor_reply(from, frame) {
                    self.broadcast(&Event::RemoteReply { from, frame: text(frame) });
                }
            }
            Some(Content::Sensor(data)) => self.on_sensor(from, data),
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
                srtt: None,
                mode: SendMode::Push,
                created: Instant::now(),
                size,
                sealed_offer: std::mem::take(&mut self.last_sealed),
                sealed_chunks: None,
                from_box: false,
                page: false,
            },
        );
        self.log(format!("sending file ({chunks} chunks) to {}", short(&dest)));
        Ok((file_id, chunks as u16))
    }

    /// A LoRa chunk's timer fired: resend what is overdue.
    fn file_resend_check(&mut self, file_id: u64) {
        let Some(f) = self.out_files.get_mut(&file_id) else {
            return;
        };
        let timeout = lora_chunk_timeout(f.srtt);
        let now = Instant::now();
        f.in_flight.retain(|_, sent| now.duration_since(*sent) < timeout);
        self.pump_file(file_id);
    }

    /// A file transfer over LoRa is going on (chunks out or in lately).
    fn lora_file_busy(&self) -> bool {
        let now = Instant::now();
        self.out_files.values().any(|f| f.offer_acked && f.done < f.chunks && self.path_kind(&f.dest) != PathKind::Tcp)
            || self.in_files.values().any(|f| {
                now.duration_since(f.active) < Duration::from_secs(15) && self.path_kind(&f.from) != PathKind::Tcp
            })
    }

    /// Our own Hello or Announce while a LoRa file transfer is going on:
    /// put it off (checked again in 10 s, sent after BEACON_DEFER_MAX at
    /// the latest) so it does not collide with chunks and acks.
    fn defer_beacon(&mut self, packet: &[u8], id: u64, only: Only) -> bool {
        let Ok((h, _)) = Header::decode(packet) else { return false };
        if !matches!(h.ptype, PacketType::Hello | PacketType::Announce) || h.dest != self.addr || h.hops != 0 {
            return false;
        }
        if !self.lora_file_busy() {
            self.beacon_deferred_since = None;
            return false;
        }
        let since = *self.beacon_deferred_since.get_or_insert_with(Instant::now);
        if since.elapsed() >= BEACON_DEFER_MAX {
            self.beacon_deferred_since = None;
            return false;
        }
        self.schedule(Duration::from_secs(10), Timer::Tx { packet: packet.to_vec(), id, only });
        true
    }

    /// Sends more chunks as long as the window allows.
    fn pump_file(&mut self, file_id: u64) {
        loop {
            let Some(f) = self.out_files.get(&file_id) else {
                return;
            };
            let tcp = self.path_kind(&f.dest) == PathKind::Tcp;
            let window = if tcp { FILE_WINDOW } else { FILE_WINDOW_LORA };
            if !f.offer_acked || f.in_flight.len() >= window {
                return;
            }
            // Pull: the recipient asks; we send only what it asks for.
            match f.mode {
                SendMode::Push => {}
                SendMode::Pull(_) => return,
                SendMode::AwaitRequest(until) if Instant::now() < until => return,
                SendMode::AwaitRequest(_) => {
                    let dest = f.dest;
                    self.log(format!("file to {}: no request from the recipient, sending the push way", short(&dest)));
                    if let Some(f) = self.out_files.get_mut(&file_id) {
                        f.mode = SendMode::Push;
                    }
                    continue;
                }
            }
            // One file at a time to each recipient, the oldest first, over
            // any route. Over LoRa two files in parallel put two chunks on the
            // air and the radio talks over the acks; over TCP several big
            // files at once overfill the router's send queue and stall. The
            // rest follow (housekeeping pumps every file).
            {
                let mine = (f.created, file_id);
                let dest = f.dest;
                if self.out_files.iter().any(|(id, o)| *id != file_id && o.dest == dest && (o.created, *id) < mine) {
                    return;
                }
            }
            // Over 8 KB only along a route we know is TCP all the way.
            if f.size > LORA_MAX_FILE && self.path_kind(&f.dest) != PathKind::Tcp {
                if self.file_wait_logged.insert(file_id) {
                    self.log(format!("file to {} waits: no route known to be internet all the way (over 8 KB is not sent over LoRa)", short(&f.dest)));
                }
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
                    let mut check = None;
                    if let Some(f) = self.out_files.get_mut(&file_id) {
                        f.in_flight.insert(idx, Instant::now());
                        check = Some((f.dest, f.srtt));
                    }
                    if let Some((dest, srtt)) = check
                        && self.path_kind(&dest) != PathKind::Tcp
                    {
                        self.schedule(lora_chunk_timeout(srtt) + Duration::from_millis(50), Timer::FileCheck(file_id));
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
        if let Some(sent) = f.in_flight.remove(&idx) {
            smooth_rtt(&mut f.srtt, sent.elapsed());
        }
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
            self.file_delivered(file_id);
        } else {
            self.pump_file(file_id);
        }
    }

    /// Every chunk of an outgoing file has arrived.
    fn file_delivered(&mut self, file_id: u64) {
        {
            let finished = self.out_files.remove(&file_id);
            let boxed = finished.as_ref().is_some_and(|f| f.from_box);
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
            // The next file to the same recipient (waiting its turn over LoRa).
            if let Some(dest) = finished.map(|f| f.dest) {
                let next: Vec<u64> = self.out_files.iter().filter(|(_, o)| o.dest == dest).map(|(id, _)| *id).collect();
                for id in next {
                    self.pump_file(id);
                }
            }
        }
    }

    /// The user stops a file, outgoing or incoming; the other side is told
    /// (FILE_CANCEL), so it stops too.
    fn cancel_file(&mut self, file_id: u64) {
        // Outgoing: being sent, or kept in the file mailbox.
        let dest = self.out_files.remove(&file_id).map(|f| f.dest).or_else(|| self.filebox.recipient(file_id));
        if let Some(dest) = dest {
            self.filebox.remove(file_id);
            self.log(format!("file {file_id:016x} to {} cancelled by the user", short(&dest)));
            let _ = self.send_sealed(dest, &content::file_cancel(file_id, false), Pending::Control);
            self.broadcast(&Event::FileDone { file_id, ok: false, outgoing: true, text: text(b"cancelled") });
            return;
        }
        // Incoming: drop what we have and ask the sender to stop.
        if let Some(f) = self.in_files.remove(&file_id) {
            self.files_cancelled.insert(file_id, Instant::now());
            self.log(format!("file {file_id:016x} from {} cancelled by the user", short(&f.from)));
            let _ = self.send_sealed(f.from, &content::file_cancel(file_id, true), Pending::Control);
            self.broadcast(&Event::FileDone { file_id, ok: false, outgoing: false, text: text(b"cancelled") });
        }
    }

    /// The other side stopped a file transfer.
    fn on_file_cancel(&mut self, from: Address, file_id: u64, by_recipient: bool) {
        if by_recipient {
            let ours = self.out_files.get(&file_id).map(|f| f.dest).or_else(|| self.filebox.recipient(file_id));
            if ours.is_some_and(|d| d == from || self.person_peer(&d) == self.person_peer(&from)) {
                self.out_files.remove(&file_id);
                self.filebox.remove(file_id);
                self.log(format!("file {file_id:016x} cancelled by the recipient {}", short(&from)));
                self.broadcast(&Event::FileDone { file_id, ok: false, outgoing: true, text: text(b"cancelled by the recipient") });
            }
        } else if self.in_files.get(&file_id).is_some_and(|f| f.from == from || f.from == self.person_peer(&from)) {
            self.in_files.remove(&file_id);
            self.files_cancelled.insert(file_id, Instant::now());
            self.log(format!("file {file_id:016x} cancelled by the sender {}", short(&from)));
            self.broadcast(&Event::FileDone { file_id, ok: false, outgoing: false, text: text(b"cancelled by the sender") });
        }
    }

    fn on_file_offer(&mut self, from: Address, offer: FileOffer) {
        let chunk_size = usize::from(offer.chunk_size);
        if offer.size > self.max_file
            || chunk_size == 0
            || chunk_size > CHUNK_DATA
            || usize::from(offer.chunks) != (offer.size as usize).div_ceil(chunk_size).max(1)
        {
            self.log(format!(
                "file from {} rejected (too large or invalid)",
                short(&from)
            ));
            return;
        }
        let sender = from;
        let file_id = offer.file_id;
        // A page we did not ask for (an old answer, say from the sender's
        // file mailbox): pages belong on the Pages screen, never in a chat
        // as a file. Tell the sender to stop, so it drops it.
        if pages::is_page_file(&offer.mime, &offer.name) && !self.page_was_asked(&sender, &offer.name) {
            if !self.files_cancelled.contains_key(&file_id) {
                self.log(format!("page '{}' from {} not asked for; refused", String::from_utf8_lossy(&offer.name), short(&sender)));
            }
            self.files_cancelled.insert(file_id, Instant::now());
            let _ = self.send_sealed(sender, &content::file_cancel(file_id, true), Pending::Control);
            return;
        }
        // Stopped (an offer resent before the stop reached the sender, or the
        // sender missed our stop): tell it to stop again, do not start over.
        if self.files_cancelled.contains_key(&file_id) {
            self.log(format!("file {file_id:016x} offered again after it was stopped; telling {} to stop", short(&sender)));
            let _ = self.send_sealed(sender, &content::file_cancel(file_id, true), Pending::Control);
            return;
        }
        // Already received (the sender missed our final request): tell it again.
        if self.files_done.contains_key(&file_id) {
            if self.pull_capable(&sender) {
                let _ = self.send_sealed_datagram(sender, &content::file_request(file_id, offer.chunks, 0, 0));
            }
            return;
        }
        // Offered again (resent, e.g. from the sender's file mailbox): keep
        // the chunks we have, so only the rest has to come (pull: ask for it).
        if let Some(f) = self.in_files.get_mut(&file_id) {
            if f.sender != sender || f.offer != offer { return; }
            f.active = Instant::now();
            let (got, total) = (f.got, f.offer.chunks);
            self.log(format!("file offered again by {}: {got} of {total} chunks already here", short(&from)));
            self.pull_start(file_id);
            return;
        }
        if self.in_files.len() >= MAX_INCOMING_FILES
            || self.in_files.values().filter(|file| file.sender == sender).count() >= MAX_INCOMING_FILES_PER_SENDER {
            return;
        }
        // A file from a device of a person shows in the person's conversation.
        let from = self.person_peer(&from);
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
                active: Instant::now(),
                sender,
                pull: None,
                req_seq: 0,
                offer,
            },
        );
        self.pull_start(file_id);
    }

    /// The other side takes files the pull way and we reach it over LoRa.
    fn pull_capable(&self, peer: &Address) -> bool {
        self.file_pull
            && self.caps.get(peer).is_some_and(|c| c & maarse_core::appdata::CAP_FILE_PULL != 0)
            && self.path_kind(peer) != PathKind::Tcp
    }

    /// Starts (or resumes) asking for an incoming file, if its sender takes
    /// requests. One pull at a time per sender (two would put our requests
    /// on the air while the sender sends the other file's batch).
    fn pull_start(&mut self, file_id: u64) {
        let Some(f) = self.in_files.get(&file_id) else { return };
        let sender = f.sender;
        if !self.pull_capable(&sender) {
            return;
        }
        let busy = self.in_files.iter().any(|(id, o)| *id != file_id && o.sender == sender && o.pull.is_some());
        if busy {
            return; // started when the other file is done (finish_incoming)
        }
        let relayed = self.heard_via.get(&sender).is_some_and(|(_, _, h)| *h > 0);
        let start = if relayed { PULL_WINDOW_MAX_RELAYED } else { PULL_WINDOW_START };
        let chunk_ms = maarse_core::companion::RadioConfig::EU_DEFAULT.airtime().airtime_us(maarse_core::MAX_PACKET) / 1000 + 150;
        let f = self.in_files.get_mut(&file_id).unwrap();
        if f.pull.is_none() {
            f.pull = Some(PullRx {
                window: start,
                asked: Vec::new(),
                deadline: Instant::now(),
                retries: 0,
                chunk_ms,
                last_rx: None,
                troubled: false,
            });
        }
        self.pull_request(file_id);
    }

    /// Asks the sender for the next batch of missing chunks.
    fn pull_request(&mut self, file_id: u64) {
        let Some(f) = self.in_files.get_mut(&file_id) else { return };
        let Some(p) = f.pull.as_mut() else { return };
        let total = f.offer.chunks;
        let base = (0..total).find(|i| f.parts[*i as usize].is_none()).unwrap_or(total);
        let mut missing = 0u32;
        let mut asked = Vec::new();
        for i in 0..content::FILE_REQUEST_SPAN {
            let idx = base + i;
            if idx >= total {
                break;
            }
            if f.parts[idx as usize].is_none() {
                missing |= 1 << i;
                if asked.len() < p.window as usize {
                    asked.push(idx);
                }
            }
        }
        // Expect the first chunk of the batch within two chunk times plus
        // the request's own trip (each chunk then moves the deadline on).
        let wait = Duration::from_millis(p.chunk_ms as u64 * 2 + 1500 + p.retries as u64 * 500);
        p.deadline = Instant::now() + wait;
        p.asked = asked;
        p.last_rx = None;
        let count = p.window;
        let sender = f.sender;
        self.schedule(wait + Duration::from_millis(20), Timer::PullCheck(file_id));
        self.send_pull_request(file_id, sender, base, count, missing);
    }

    /// A request sealed with the file key (about 60 bytes on air instead of
    /// 112 as a sealed message: it is sent once per batch).
    fn send_pull_request(&mut self, file_id: u64, sender: Address, base: u16, count: u8, missing: u32) {
        let Some(f) = self.in_files.get_mut(&file_id) else { return };
        if f.req_seq >= PULL_REQ_INDEX {
            return; // 32768 requests for one file: never in practice
        }
        let idx = PULL_REQ_INDEX + f.req_seq;
        f.req_seq += 1;
        let key = f.offer.key;
        let mut ct = [0u8; 64];
        let Ok(n) = seal::seal_chunk(&key, file_id, idx, &content::file_request(file_id, base, count, missing), &mut ct) else {
            return;
        };
        let now = self.now_ms();
        if let Ok((_, actions)) = self.router.send_typed(PacketType::Datagram, sender, &ct[..n], token(), now) {
            self.apply(actions.into_iter().collect());
        }
    }

    /// A pull request's time is up: ask again (smaller batch), or give up.
    fn pull_check(&mut self, file_id: u64) {
        let Some(f) = self.in_files.get_mut(&file_id) else { return };
        let Some(p) = f.pull.as_mut() else { return };
        if Instant::now() < p.deadline {
            return; // a newer request is under way
        }
        p.retries += 1;
        p.troubled = true;
        p.window = (p.window.saturating_sub(1)).max(2);
        if p.retries > PULL_MAX_RETRIES {
            // The sender offers again (or keeps the file in its mailbox);
            // we then resume from what we have.
            f.pull = None;
            self.log(format!("file {file_id:016x}: no answer to our requests, waiting for the sender"));
            return;
        }
        self.pull_request(file_id);
    }

    /// A chunk of a pulled file arrived.
    fn pull_on_chunk(&mut self, file_id: u64, idx: u16) {
        let Some(f) = self.in_files.get_mut(&file_id) else { return };
        let Some(p) = f.pull.as_mut() else { return };
        let now = Instant::now();
        if let Some(prev) = p.last_rx {
            // Gap between two chunks of one batch: airtime plus spacing.
            let gap = now.duration_since(prev).as_millis().min(10_000) as u32;
            p.chunk_ms = (p.chunk_ms * 3 + gap) / 4;
        }
        p.last_rx = Some(now);
        p.retries = 0;
        let last_of_batch = p.asked.last().is_some_and(|l| idx >= *l);
        p.asked.retain(|i| *i != idx);
        if !p.asked.is_empty() && last_of_batch {
            // The batch is over but some of it was lost: ask for the gaps now.
            p.troubled = true;
            self.pull_request(file_id);
            return;
        }
        if p.asked.is_empty() {
            // The whole batch is here: next one at once, a bit bigger if clean.
            let relayed = self.heard_via.get(&f.sender).is_some_and(|(_, _, h)| *h > 0);
            let max = if relayed { PULL_WINDOW_MAX_RELAYED } else { PULL_WINDOW_MAX };
            if !p.troubled {
                p.window = (p.window + 1).min(max);
            }
            p.troubled = false;
            if f.got < f.offer.chunks {
                self.pull_request(file_id);
            }
        } else {
            // More of this batch to come: the next chunk within two chunk
            // times (one lost chunk in between is fine; the batch's last one
            // then triggers the next request).
            p.deadline = now + Duration::from_millis(p.chunk_ms as u64 * 2 + 500);
            let wait = p.deadline.duration_since(now);
            self.schedule(wait + Duration::from_millis(20), Timer::PullCheck(file_id));
        }
    }

    /// A request sealed with the file key: only the recipient has the key.
    fn on_sealed_request(&mut self, payload: &[u8]) {
        let Some((file_id, _)) = seal::chunk_header(payload) else { return };
        let Some(key) = self.out_files.get(&file_id).map(|f| f.key) else { return };
        let mut out = [0u8; 64];
        let Ok(n) = seal::open_chunk(&key, payload, &mut out) else { return };
        if let Some(Content::FileRequest(id, base, count, missing)) = content::parse(&out[..n])
            && id == file_id
        {
            self.on_file_request(None, file_id, base, count, missing);
        }
    }

    /// The recipient asks for chunks (we send the pull way). `from`: the
    /// sender of a sealed message (checked); None for a request sealed with
    /// the file key.
    fn on_file_request(&mut self, from: Option<Address>, file_id: u64, base: u16, count: u8, missing: u32) {
        let Some(dest) = self.out_files.get(&file_id).map(|f| f.dest) else {
            return;
        };
        if let Some(from) = from
            && dest != from
            && self.person_peer(&dest) != self.person_peer(&from)
        {
            return;
        }
        let f = self.out_files.get_mut(&file_id).unwrap();
        f.mode = SendMode::Pull(Instant::now());
        f.in_flight.clear();
        // Everything below base, and what the request does not list as missing, has arrived.
        for i in 0..f.chunks {
            let here = i < base
                || (i - base < content::FILE_REQUEST_SPAN && missing & (1 << (i - base)) == 0);
            if here && !f.acked[i as usize] {
                f.acked[i as usize] = true;
                f.done += 1;
            }
        }
        let (done, total) = (f.done, f.chunks);
        self.broadcast(&Event::FileProgress { file_id, done, total, outgoing: true });
        if base >= total || done >= total {
            self.file_delivered(file_id);
            return;
        }
        let want: Vec<u16> = (0..content::FILE_REQUEST_SPAN)
            .filter(|i| missing & (1 << i) != 0)
            .filter_map(|i| base.checked_add(i))
            .filter(|i| *i < total)
            .take(count.max(1) as usize)
            .collect();
        for idx in want {
            let Some(ct) = self.sealed_chunk(file_id, idx) else { return };
            let now = self.now_ms();
            if let Ok((_, actions)) = self.router.send_typed(PacketType::Datagram, dest, &ct, token(), now) {
                self.apply(actions.into_iter().collect());
            }
        }
    }

    /// A chunk sealed for sending (from the file mailbox, or sealed now).
    fn sealed_chunk(&self, file_id: u64, idx: u16) -> Option<Vec<u8>> {
        let f = self.out_files.get(&file_id)?;
        if let Some(c) = f.sealed_chunks.as_ref() {
            return c.get(idx as usize).cloned();
        }
        let start = idx as usize * CHUNK_DATA;
        let end = (start + CHUNK_DATA).min(f.data.len());
        let mut ct = [0u8; 255];
        let n = seal::seal_chunk(&f.key, file_id, idx, f.data.get(start..end)?, &mut ct).ok()?;
        Some(ct[..n].to_vec())
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
        let chunk_size = usize::from(f.offer.chunk_size);
        let offset = usize::from(idx) * chunk_size;
        let expected = (f.offer.size as usize).saturating_sub(offset).min(chunk_size);
        if n != expected { return; }
        f.parts[idx as usize] = Some(out[..n].to_vec());
        f.got += 1;
        f.active = Instant::now();
        let (got, total) = (f.got, f.offer.chunks);
        // A page or a page picture: the clients get each piece at once, so
        // they can draw the page while the rest comes (the whole file follows
        // at the end as for any file; the clients place pieces by offset).
        if pages::is_page_file(&f.offer.mime, &f.offer.name) {
            let off = idx as usize * f.offer.chunk_size as usize;
            for (k, part) in out[..n].chunks(MAX_TEXT).enumerate() {
                self.broadcast(&Event::FileChunk { file_id, offset: (off + k * MAX_TEXT) as u32, data: text(part) });
            }
        }
        self.broadcast(&Event::FileProgress {
            file_id,
            done: got,
            total,
            outgoing: false,
        });
        if got == total {
            self.finish_incoming(file_id);
        } else {
            self.pull_on_chunk(file_id, idx);
        }
    }

    fn finish_incoming(&mut self, file_id: u64) {
        let Some(f) = self.in_files.remove(&file_id) else { return; };
        let data: Vec<u8> = f.parts.into_iter().flatten().flatten().collect();
        let valid = data.len() == f.offer.size as usize && Sha256::digest(&data)[..16] == f.offer.hash;
        let sender = f.sender;
        // A completion receipt must only acknowledge an intact file.
        if valid {
            self.files_done.insert(file_id, Instant::now());
            if f.pull.is_some() {
                let idx = PULL_REQ_INDEX + f.req_seq.min(PULL_REQ_INDEX - 1);
                let mut ct = [0u8; 64];
                if let Ok(n) = seal::seal_chunk(&f.offer.key, file_id, idx, &content::file_request(file_id, f.offer.chunks, 0, 0), &mut ct) {
                    let now = self.now_ms();
                    if let Ok((_, actions)) = self.router.send_typed(PacketType::Datagram, sender, &ct[..n], token(), now) {
                        self.apply(actions.into_iter().collect());
                    }
                }
            }
        }
        if f.pull.is_some() {
            let next = self.in_files.iter().filter(|(_, other)| other.sender == sender).map(|(id, _)| *id).min();
            if let Some(id) = next { self.pull_start(id); }
        }
        if !valid {
            self.broadcast(&Event::FileDone { file_id, ok: false, outgoing: false, text: text(b"checksum mismatch") });
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
        // A host found in the DHT (say behind NAT, through a relay; we may
        // never have heard its announce), or any node heard when we have
        // no radio at all (a phone node) and have heard over the backbone:
        // nothing on the way can be LoRa.
        // Ourselves (the owner looking at their own pages): no link at all,
        // nothing on the way is LoRa.
        if *dest == self.addr {
            return PathKind::Tcp;
        }
        let no_radio = !self.links.values().any(|(n, _)| !crate::extra_links::is_backbone(n));
        let heard = self.heard_via.get(dest);
        // Last heard on our own radio: not a TCP host, whatever the DHT
        // says. A DHT record outlives the internet connection (a phone that
        // was online earlier and is now only on LoRa); trusting it kept
        // file chunks off the radio, so pages never arrived.
        let on_our_radio = heard.is_some_and(|(l, tcp, _)| !*tcp && self.links.contains_key(l));
        if (self.dht_hosts.contains(dest) && !on_our_radio) || (no_radio && heard.is_some()) {
            return PathKind::Tcp;
        }
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
                    srtt: None,
                    mode: SendMode::Push,
                    created: Instant::now(),
                    size: b.size,
                    sealed_offer: b.offer,
                    sealed_chunks: Some(b.chunks),
                    from_box: true,
                    page: false,
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
            mail::OP_BUNDLE if d.recipient == self.addr => {
                let items: Vec<Vec<u8>> = d.bundle().map(|m| m.to_vec()).collect();
                for m in items {
                    if self.receipt_wanted.len() >= 256 {
                        self.receipt_wanted.clear();
                    }
                    self.receipt_wanted.insert(mail::receipt_id(&self.addr, &m));
                    self.open_direct(m, hops, true);
                }
            }
            // A recipient says it got mail we deposited: the same as an ack.
            mail::OP_RECEIPT if d.recipient == self.addr => {
                let ids: Vec<u64> = d.receipts().collect();
                for id in ids {
                    let found = self
                        .outbox
                        .iter()
                        .find(|(_, m)| mail::receipt_id(&m.dest, &m.mail_sealed) == id)
                        .map(|(msg_id, _)| *msg_id);
                    if let Some(msg_id) = found {
                        self.log(format!("delivery receipt for message {msg_id:016x}"));
                        self.on_acked(msg_id);
                    }
                }
            }
            _ => {}
        }
    }

    /// Tells `sender` that its mail, delivered by a post office, reached us.
    fn send_receipt(&mut self, sender: Address, id: u64) {
        let mut p: maarse_core::router::Buf = maarse_core::router::Buf::new();
        if !maarse_core::mail::build(&mut p, &sender, maarse_core::mail::OP_RECEIPT, &id.to_le_bytes()) {
            return;
        }
        let now = self.now_ms();
        if let Ok((msg_id, actions)) = self.router.send_typed(PacketType::Deposit, sender, &p, token(), now) {
            self.awaiting.insert(msg_id, Pending::Control);
            self.apply(actions.into_iter().collect());
        }
    }

    /// Accepts mail locally and optionally sends copies to replica hosts.
    fn store_mail(&mut self, recipient: Address, sealed: Vec<u8>, replicate: bool) -> bool {
        if !Self::valid_mail_size(&sealed) {
            return false;
        }
        let stored = self.store_mail_local(recipient, sealed.clone(), replicate, false);
        if replicate {
            self.replicate_mail(recipient, sealed, None);
        }
        stored
    }

    fn replicate_mail(&self, recipient: Address, sealed: Vec<u8>, confirm: Option<u64>) {
        let replicas = self.mail_replicas(&recipient);
        if replicas.is_empty() {
            return;
        }
        let identity = self.identity.clone();
        let my_ep = self.dht.public_ep.clone();
        let configured = self.dht.allowed_endpoints.clone();
        let reply = confirm.zip(self.tx.clone());
        let allow_legacy = self.mail_replica_ids.is_empty();
        self.start_mail_job(move || {
            let request = dht::Req::MailStore { recipient, sealed };
            let mut reply = reply;
            for c in replicas {
                let response = dht::rpc(&c, &configured, &identity, my_ep.as_deref(), &request, allow_legacy);
                if matches!(response, Some((_, dht::Resp::Stored(true))))
                    && let Some((msg_id, tx)) = reply.take()
                {
                    let _ = tx.send(In::MailReplicated { msg_id });
                }
            }
        });
    }

    fn valid_mail_size(sealed: &[u8]) -> bool {
        !sealed.is_empty() && sealed.len() <= MAX_DIRECT_PAYLOAD
    }

    fn store_mail_local(&mut self, recipient: Address, sealed: Vec<u8>, replicate: bool, fetched: bool) -> bool {
        if !Self::valid_mail_size(&sealed) || (self.no_post_office && !fetched) {
            return false;
        }
        let id = maarse_core::dedup::packet_id(PacketType::Data, &recipient, &sealed);
        let existing = self.mailbox.get(&recipient);
        if existing.is_some_and(|box_| box_.contains_key(&id)) {
            return true;
        }
        if existing.is_some_and(|box_| box_.len() >= MAILBOX_PER_RECIPIENT) {
            self.log(format!("mailbox full for {}", short(&recipient)));
            return false;
        }
        // Fetched mail shares the main budget and has a finite ceiling even
        // when ordinary post storage is explicitly configured as unlimited.
        let quota = match (fetched, self.mail_quota) {
            (true, 0) => FETCHED_MAIL_CEILING,
            (true, quota) => quota.min(FETCHED_MAIL_CEILING),
            (false, quota) => quota,
        };
        if quota > 0 && self.mail_bytes() + MAIL_RECORD_OVERHEAD + sealed.len() as u64 > quota {
            self.log(format!("post office: quota full for {}", short(&recipient)));
            return false;
        }

        let box_ = self.mailbox.entry(recipient).or_default();
        box_.insert(id, Mail { sealed, stored: SystemTime::now() });
        let count = box_.len();
        if !self.save_mailbox() {
            let box_ = self.mailbox.get_mut(&recipient).unwrap();
            box_.remove(&id);
            if box_.is_empty() {
                self.mailbox.remove(&recipient);
            }
            return false;
        }

        if let Some(t) = self.tele.as_mut() {
            t.mail(if replicate { "stored (sent here)" } else { "stored (copy from another host)" });
            t.mail_stored(id, &recipient, !replicate);
        }
        self.log(format!("post office: stored message for {} ({count} in total)", short(&recipient)));
        self.update_tim();
        self.mail_retry.entry(recipient).or_insert((Instant::now() + MAIL_RETRY_FIRST, 0));
        if !fetched {
            self.flush_mailbox(recipient);
        }
        true
    }

    /// Limit network work generated by inbound mail, with no unbounded queue.
    fn start_mail_job(&self, job: impl FnOnce() + Send + 'static) -> bool {
        use std::sync::atomic::Ordering::Relaxed;

        struct Slot(std::sync::Arc<std::sync::atomic::AtomicUsize>);
        impl Drop for Slot {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Relaxed);
            }
        }

        if self.mail_jobs.try_update(Relaxed, Relaxed, |active| {
            (active < MAX_MAIL_JOBS).then_some(active + 1)
        }).is_err() {
            return false;
        }
        let slot = Slot(self.mail_jobs.clone());
        std::thread::Builder::new().spawn(move || {
            let _slot = slot;
            job();
        }).is_ok()
    }

    fn mail_replicas(&self, recipient: &Address) -> Vec<Contact> {
        // Only hosts that keep mail (unknown counts as yes, as before).
        let mut contacts: Vec<Contact> = self
            .dht
            .table
            .all()
            .into_iter()
            .filter(|c| self.mail_replica_ids.is_empty() || self.mail_replica_ids.contains(&c.id))
            .filter(|c| self.caps_of.get(&c.id).is_none_or(|(k, _)| k.post_office))
            .collect();
        let mut addrs: Vec<Address> = contacts.iter().map(|c| c.id).collect();
        if self.dht.public_ep.is_some() && !self.no_post_office
            && (self.mail_replica_ids.is_empty() || self.mail_replica_ids.contains(&self.addr))
        {
            addrs.push(self.addr);
        }
        // Rendezvous hashing: the REPLICAS highest scores (as mail::replicas).
        addrs.sort_by_key(|a| std::cmp::Reverse(maarse_core::mail::score(a, recipient)));
        addrs.dedup();
        let pick: Vec<Address> = addrs.into_iter().take(maarse_core::mail::REPLICAS).collect();
        contacts.retain(|c| pick.contains(&c.id) && c.id != self.addr);
        contacts
    }

    /// Has the backbone heard `recipient`: in the DHT's heard table, or
    /// heard by us over a TCP link?
    fn backbone_knows(&self, recipient: &Address) -> bool {
        self.dht.heard.contains_key(recipient) || self.heard_via.get(recipient).is_some_and(|(_, tcp, _)| *tcp)
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
        let identity = self.identity.clone();
        let my_ep = self.dht.public_ep.clone();
        let configured = self.dht.allowed_endpoints.clone();
        let allow_legacy = self.mail_replica_ids.is_empty();
        if self.start_mail_job(move || {
            let mut items: Vec<Vec<u8>> = Vec::new();
            for c in replicas {
                if let Some((_, dht::Resp::Mail(v))) = dht::rpc(&c, &configured, &identity, my_ep.as_deref(), &dht::Req::MailFetch { recipient, have: have.clone() }, allow_legacy) {
                    for m in v {
                        if !items.contains(&m) {
                            items.push(m);
                        }
                    }
                }
            }
            let _ = tx.send(In::MailFetched { recipient, items });
        }) {
            self.fetch_asked.insert(recipient, Instant::now());
        }
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
        // Also for a single message (up to 255 bytes): a bundle is how the
        // recipient learns the mail came from a post office and sends the
        // sender a delivery receipt.
        let bundle_ok = mails.len() >= 2 || mails.values().all(|m| m.sealed.len() <= 255);
        if self.caps.get(&recipient).is_some_and(|c| c & maarse_core::appdata::CAP_BUNDLE != 0) && bundle_ok {
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
        if m.deposited || m.mail_sealed.len() + 16 > MAX_DIRECT_PAYLOAD || !Self::valid_mail_size(&m.mail_sealed) {
            return;
        }
        m.next_deposit = Some(Instant::now() + Duration::from_secs(60));
        let (dest, sealed) = (m.dest, m.mail_sealed.clone());
        let stored = self.store_mail_local(dest, sealed.clone(), true, false);
        if stored {
            self.mark_mail_deposited(msg_id);
        }
        self.replicate_mail(dest, sealed, (!stored).then_some(msg_id));
    }

    fn mark_mail_deposited(&mut self, msg_id: u64) {
        if let Some(message) = self.outbox.get_mut(&msg_id)
            && !message.deposited
        {
            message.deposited = true;
            self.counters.deposits += 1;
            self.broadcast(&Event::Deposited { msg_id });
        }
    }

    fn save_mailbox(&self) -> bool {
        let path = self.data_dir.join("mailbox.bin");
        let temp = path.with_extension("tmp");
        let result = (|| -> std::io::Result<()> {
            use std::io::Write;

            let mut writer = std::io::BufWriter::new(fs::File::create(&temp)?);
            // Format: recipient (16) | stored (u64 sec.) | length (u16) | message
            for (recipient, mails) in &self.mailbox {
                for mail in mails.values() {
                    let stored = mail.stored.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
                    writer.write_all(&recipient.0)?;
                    writer.write_all(&stored.to_le_bytes())?;
                    writer.write_all(&(mail.sealed.len() as u16).to_le_bytes())?;
                    writer.write_all(&mail.sealed)?;
                }
            }
            writer.flush()?;
            writer.get_ref().sync_all()?;
            drop(writer);
            fs::rename(&temp, &path)
        })();
        if let Err(err) = result {
            let _ = fs::remove_file(&temp);
            eprintln!("Could not save mailbox {}: {err}", path.display());
            return false;
        }
        true
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
        }
        // Telemetry exists on maarse-server and with --public-map.
        self.telemetry_upkeep();
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
            let next = RULES.mail_retry_after(step);
            // Presence only delivers for a recipient the backbone has heard;
            // one behind a trunk or a radio-only host never shows up there,
            // so keep retrying (maarse-sim preset bornholm_alone).
            let cheap = backbone && self.backbone_knows(&r);
            let Some(next) = next.filter(|_| !cheap && self.mailbox.get(&r).is_some_and(|m| !m.is_empty())) else {
                self.mail_retry.remove(&r);
                continue;
            };
            self.mail_retry.insert(r, (Instant::now() + Duration::from_millis(next), step));
            self.flush_mailbox(r);
        }
        self.present.retain(|_, t| t.elapsed() < MAIL_PRESENT);
        self.tun_tick();
        self.hist_maintain();
        self.person_maintain();
        self.sensors_tick();
        self.seqs.save();
        // A host has the system clock (NTP): stratum 1 in our hellos.
        let now = self.now_ms();
        self.router.clock.set(unix_now(), now, 1);
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
            .filter(|(_, m)| !m.deposited && now.duration_since(m.sent) > after
                && m.next_deposit.is_none_or(|retry| now >= retry))
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
            if f.page && age > PAGE_REPLY_TIMEOUT {
                let dest = f.dest;
                self.out_files.remove(&file_id);
                self.log(format!("page to {} not taken; dropped", short(&dest)));
                self.broadcast(&Event::FileDone { file_id, ok: false, outgoing: true, text: text(b"page not taken") });
                continue;
            }
            if !f.from_box && !f.page && self.filebox.enabled() && (late || age > FILE_TIMEOUT) && self.deposit_file(file_id) {
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
            // Pull: no request for a while (lost, or the recipient gave up
            // asking): offer again; the recipient answers with what it lacks.
            if let Some(f) = self.out_files.get_mut(&file_id)
                && let SendMode::Pull(last) = f.mode
                && now.duration_since(last) > PULL_STALL
            {
                f.mode = SendMode::Push;
                f.offer_acked = false;
                f.offer_sent = now - CHUNK_TIMEOUT - Duration::from_secs(1);
            }
            let mut resend_offer = None;
            let tcp = self.out_files.get(&file_id).is_some_and(|f| self.path_kind(&f.dest) == PathKind::Tcp);
            let srtt = self.out_files.get(&file_id).and_then(|f| f.srtt);
            let timeout = if tcp { CHUNK_TIMEOUT } else { lora_chunk_timeout(srtt) };
            if let Some(f) = self.out_files.get_mut(&file_id) {
                f.in_flight
                    .retain(|_, sent| now.duration_since(*sent) < timeout);
                if !f.offer_acked && now.duration_since(f.offer_sent) > timeout {
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
        self.files_done.retain(|_, t| now.duration_since(*t) < Duration::from_secs(24 * 3600));
        self.files_cancelled.retain(|_, t| now.duration_since(*t) < Duration::from_secs(24 * 3600));
        // Half-finished incoming files.
        let stale: Vec<u64> = self
            .in_files
            .iter()
            .filter(|(_, f)| now.duration_since(f.active) > FILE_TIMEOUT)
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
            crate::fwupdate::beep_radios();
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

    fn flush_observations(&mut self) {
        let Some(target) = self.observers.target.as_ref() else { return };
        let Some((_, (_, tx))) = self.links.iter().find(|(id, (name, _))| {
            *name == target.link && self.link_caps.get(id).is_some_and(|caps| caps.observe)
        }) else { return };
        if let Some(report) = self.observers.report(&self.identity) {
            tx.send_observations(report);
        }
    }

    /// Bytes of mail kept for others.
    fn mail_bytes(&self) -> u64 {
        self.mailbox.values().flat_map(|m| m.values()).map(|mail| MAIL_RECORD_OVERHEAD + mail.sealed.len() as u64).sum()
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
            call_test: self.call_test.enabled(),
            observe: self.observers.collecting(),
            open_channel_names: true,
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
                .filter(|(n, k)| n != "public" || *k != public_channel_key())
                .map(|(n, k)| {
                    if normalize_channel(maarse_core::region::split_channel(n).0).1 == *k {
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
        let Some(cmd) = self.sensor_command(id, cmd) else { return };
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
                        | Command::GetStatsKeys { .. }
                        | Command::GetRadio
                        | Command::SetRadio(_)
                        | Command::SetRadioTrial { .. }
                        | Command::ConfirmRadio
                        | Command::GetNeighbors
                        | Command::SetRole { .. }
                        | Command::OtaBegin { .. }
                        | Command::OtaChunk { .. }
                        | Command::OtaEnd { .. }
                        | Command::OtaStatus
                        | Command::Sensor { .. }
                        | Command::SweepRecStart { .. }
                        | Command::SweepRecStop
                        | Command::SweepRecStatus)
                );
                if !ok {
                    return self.send_client(id, &err_ev(req, error_code::BAD_FRAME, "Only status, radio, neighbours, role, firmware, sensors and spectrum recordings can be controlled remotely"));
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
            Command::GetRadio | Command::SetRadio(_) | Command::SetRadioTrial { .. } | Command::ConfirmRadio => self.send_client(
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
            // Only the radio knows: passed on to the USB and Bluetooth radios.
            Command::GetGps | Command::Sweep { .. } | Command::SweepRecStart { .. } | Command::SweepRecStop | Command::SweepRecStatus => crate::fwupdate::to_radios(&cmd.encode()),
            Command::GetStats => {
                let host = self.stats();
                let extra = self.radio_stats(&host);
                let ev = Event::Stats { items: host };
                self.send_client(id, &ev);
                // The radio's own counters (GPS, Bluetooth, USB, battery) in frames of their own.
                for chunk in extra.chunks(40) {
                    self.send_client(id, &Event::Stats { items: chunk.iter().copied().collect() });
                }
            }
            Command::GetStatsKeys { keys } => {
                let host = self.stats();
                let extra = self.radio_stats(&host);
                let items: Vec<(u8, i32)> = host.into_iter().chain(extra).filter(|(k, _)| keys.contains(k)).collect();
                self.send_client(id, &Event::Stats { items: items.into_iter().take(59).collect() });
            }
            // Handled in node/ext.rs.
            Command::TraceRoute { .. }
            | Command::DhtCheck { .. }
            | Command::GetNeighbors
            | Command::SetSharing(_)
            | Command::GetSharing
            | Command::SetName { .. }
            | Command::GetName
            | Command::SetSite(_)
            | Command::GetSite
            | Command::RequestPage { .. }
            | Command::PostBoard { .. }
            | Command::SendCall { .. } => {}
            Command::SendAlert { req, meta, text: body } => match self.send_alert(&meta, &body) {
                Ok(serial) => self.send_client(id, &Event::Sent { req, msg_id: serial as u64 }),
                Err((code, e)) => self.send_client(id, &err_ev(req, code, e)),
            },
            // Handled in node/person.rs before we get here.
            Command::LinkDevice { .. } | Command::GetPerson | Command::PersonSetup { .. } => {}
            Command::CancelFile { file_id } => self.cancel_file(file_id),
            // Handled in node/sensors.rs before we get here.
            Command::Sensor { .. } => {}
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

/// Other nodes' capabilities: one "address caps" line each.
const CAPS_FILE: &str = "caps.txt";

impl Node {
    fn save_caps(&self) {
        let lines: Vec<String> = self.caps.iter().map(|(a, c)| format!("{a} {c}")).collect();
        let _ = fs::write(self.data_dir.join(CAPS_FILE), lines.join("\n"));
    }

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
            Ok(Command::GetStatsKeys { keys }) => {
                let items: Vec<(u8, i32)> = self.stats().into_iter().filter(|(k, _)| keys.contains(k)).collect();
                reply(self, Event::Stats { items: items.into_iter().take(59).collect() });
            }
            Ok(Command::GetRadio | Command::SetRadio(_) | Command::SetRadioTrial { .. } | Command::ConfirmRadio) => {
                reply(self, err_ev(0, error_code::BAD_FRAME, "maarse-host has no radio of its own"));
            }
            Ok(Command::SetRole { .. }) => {
                reply(self, err_ev(0, error_code::BAD_FRAME, "maarse-host always has the repeater role"));
            }
            _ => reply(self, err_ev(0, error_code::BAD_FRAME, "The command cannot be controlled remotely")),
        }
    }

    /// The attached LoRa radio's own counters, without the keys this host reports itself.
    fn radio_stats(&self, host: &Stats) -> Vec<(u8, i32)> {
        let (_, _, own) = crate::links::radio_summary();
        own.into_iter()
            .filter(|(k, _)| matches!(*k, 30..=59 | 80..=113 | 116..=117) && !host.iter().any(|(h, _)| h == k))
            .collect()
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
        let (radios, idle, _) = crate::links::radio_summary();
        items.push((stat::RADIOS_CONNECTED, radios as i32));
        if let Some(s) = idle {
            items.push((stat::RADIO_IDLE_S, s.min(i32::MAX as u64) as i32));
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
        let rsp_contact = match (&from_ep, &req) {
            (Some(ep), dht::Req::Store(record))
                if record.gw == from && &record.ep == ep && record.verified_gateway() =>
            {
                Some(Contact { id: from, ep: ep.clone() })
            }
            _ => None,
        };
        let resp = match req {
            dht::Req::Ping => dht::Resp::Pong,
            dht::Req::FindNode(t) => dht::Resp::Nodes(self.dht_closest(&t)),
            dht::Req::FindValue(t) => match self.dht.store.get(&t) {
                Some(r) => dht::Resp::Value(r),
                None => dht::Resp::Nodes(self.dht_closest(&t)),
            },
            dht::Req::Store(r) => {
                let ok = self.dht.store.put(r);
                if ok && let Some(contact) = rsp_contact {
                    self.dht.table.insert(contact);
                }
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
            // Legacy peers may still send this unauthenticated operation.
            dht::Req::MailDelivered { .. } => dht::Resp::Stored(false),
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
        let identity = self.identity.clone();
        let my_ep = self.dht.public_ep.clone();
        let configured = self.dht.allowed_endpoints.clone();
        // Small backbone: store on every known host (see dht.rs "Full replication").
        let everyone = if publish.is_some() && self.dht.table.len() <= dht::FULL_REPLICA_MAX {
            self.dht.table.all()
        } else {
            Vec::new()
        };
        std::thread::spawn(move || {
            let ask = |c: &Contact, r: &dht::Req| dht::rpc(c, &configured, &identity, my_ep.as_deref(), r, true);
            let result = dht::lookup(&me, start, &target, want_value, ask);
            let is_publish = publish.is_some();
            if let Some(rec) = publish {
                let mut done: HashSet<String> = HashSet::new();
                for c in result.closest.iter().chain(everyone.iter()) {
                    if done.insert(c.ep.clone()) {
                        let _ = dht::rpc(c, &configured, &identity, my_ep.as_deref(), &dht::Req::Store(rec.clone()), true);
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
            // A host in the DHT is reached over TCP (through its gateway) -
            // unless we hear it over our own radio: a DHT record can be old
            // (a phone that was online earlier), and trusting it sent our
            // acks to the backbone while the phone was right here on LoRa.
            let trusted_route = rec.verified_gateway() || self.dht.allowed_endpoints.contains(&rec.ep);
            let on_our_radio = self.heard_via.get(&rec.dest).is_some_and(|(l, tcp, _)| !*tcp && self.links.contains_key(l));
            let eligible_host = !on_our_radio && (rec.hops == 0 || self.known.contains_key(&rec.dest));
            // The announce is genuine (checked in lookup): we now know the
            // keys. It did not come in on a link: no rx_link, or on_announce
            // would note the node as heard on whatever link the latest
            // packet used (often the backbone).
            let rx = self.rx_link.take();
            let accepted = self.on_announce(rec.dest, &rec.announce, rec.hops.saturating_add(1));
            self.rx_link = rx;
            if accepted && trusted_route {
                if eligible_host {
                    self.dht_hosts.insert(rec.dest);
                }
                self.dht_connect(&rec);
            }
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
        crate::links::spawn_discovered_tcp_peer(rec.ep.clone(), self.dht.allowed_endpoints.clone(), tx);
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
                    crate::links::spawn_discovered_tcp_peer(c.ep, self.dht.allowed_endpoints.clone(), tx.clone());
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
                let my_ep = self.dht.public_ep.clone();
                for c in due {
                    let tx = tx.clone();
                    let my_ep = my_ep.clone();
                    let configured = self.dht.allowed_endpoints.clone();
                    let identity = self.identity.clone();
                    std::thread::spawn(move || {
                        let caps = match dht::rpc(&c, &configured, &identity, my_ep.as_deref(), &dht::Req::Caps, true) {
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

    /// The public map: this host and position-sharing nodes, except flagged mobiles.
    /// See `--public-map`.
    fn public_map(&self) -> serde_json::Value {
        let pos = self.ext.sharing.position().map(|p| (p.lat_e7 as f64 / 1e7, p.lon_e7 as f64 / 1e7));
        let site = &self.ext.site;
        let nodes = self.tele.as_ref().map(|t| t.public_nodes()).unwrap_or_default();
        serde_json::json!({
            "version": 1,
            "host": {"addr": crate::debug::hex(&self.addr.0), "name": self.ext.name, "lat": pos.map(|p| p.0), "lon": pos.map(|p| p.1),
                "height_m": site.is_set().then(|| site.height_dm as f32 / 10.0), "gain_dbi": site.is_set().then(|| site.gain_hdb as f32 / 2.0),
                "tx_dbm": site.is_set().then_some(site.tx_dbm), "owner": (!site.owner.is_empty()).then(|| String::from_utf8_lossy(&site.owner).into_owned())},
            "nodes": nodes,
            "links": [],
        })
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
            if crate::web::PUBLIC_MAP.load(std::sync::atomic::Ordering::Relaxed) {
                let map = self.public_map();
                crate::debug::set_public_map(map.to_string());
            }
            if now >= self.next_tele_save {
                self.next_tele_save = now + Duration::from_secs(300);
                if let Some(t) = self.tele.as_ref() {
                    t.save(&self.data_dir.join("telemetry.json"));
                }
            }
            return;
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
            let rec = Record::signed(&self.identity, dest, ep.clone(), *hops, announce.clone());
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
            allow_legacy_web_token: false,
            public_map: false,
            map_url: None,
            public_traffic: false,
            public_history_hours: 24,
            observe_to: None,
            observe_allow: Vec::new(),
            public_channels: Vec::new(),
            share_open_channels: false,
            max_file: 1024 * 1024,
            filebox_per_recipient: 20 << 20,
            filebox_total: 200 << 20,
            postbox: None,
            public_addr: None,
            dht_seeds: Vec::new(),
            dht_private_endpoints: Vec::new(),
            seed_dns: Vec::new(),
            min_peers: 0,
            backbone: false,
            no_post_office: false,
            no_relay: false,
            mail_quota_mb: 0,
            mail_replica_ids: Vec::new(),
            no_pages: false,
            pages_burst_lora: 5,
            pages_burst_net: 60,
            board_keep: 50,
            board_every: 60,
            no_previews: false,
            modem_key: None,
            modem_ips: Vec::new(),
            modem_tcp: Vec::new(),
            block: Vec::new(),
            regions: Vec::new(),
            strict_region: false,
            role: None,
            repeater_role: false,
            trunks: Vec::new(),
            no_bordercast: false,
            meshcore_gateway: None,
            meshcore_worker: std::net::SocketAddr::from(([127, 0, 0, 1], 5057)),
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
    fn host_role_follows_the_setting_and_the_kind_of_host() {
        use crate::HostRole;
        use maarse_core::Role;
        let unique = OsRng.next_u64();
        let role_of = |role: Option<HostRole>, repeater_role: bool| {
            let mut c = cfg(std::env::temp_dir().join(format!("maarse-role-{unique}-{role:?}-{repeater_role}")));
            c.role = role;
            c.repeater_role = repeater_role;
            let node = Node::new(&c).unwrap();
            let _ = std::fs::remove_dir_all(&c.data_dir);
            node.router.role
        };
        // An ordinary host is an anchor, as before; the choice and the older flag decide otherwise.
        assert_eq!(role_of(None, false), Role::Anchor);
        assert_eq!(role_of(Some(HostRole::Client), false), Role::Client);
        assert_eq!(role_of(Some(HostRole::Repeater), false), Role::Repeater);
        assert_eq!(role_of(None, true), Role::Repeater);
        assert_eq!(role_of(Some(HostRole::Anchor), true), Role::Anchor);
        // A phone node is a client until the owner chooses another role.
        crate::set_phone_mode(true);
        assert_eq!(role_of(None, false), Role::Client);
        assert_eq!(role_of(Some(HostRole::Anchor), false), Role::Anchor);
        assert_eq!(role_of(Some(HostRole::Repeater), false), Role::Repeater);
        crate::set_phone_mode(false);
        assert_eq!(HostRole::parse(" Client "), Some(HostRole::Client));
        assert_eq!(HostRole::parse("relay"), None);
    }

    #[test]
    fn incoming_files_bound_bookkeeping_and_validate_sizes() {
        let unique = OsRng.next_u64();
        let dir = std::env::temp_dir().join(format!("maarse-file-bounds-{unique}"));
        let mut node = Node::new(&cfg(dir.clone())).unwrap();
        let sender = Address([4; 16]);
        let data = b"short file";
        let (_, mut offer) = Outgoing::new(1, [9; 32], node.addr, data, b"text/plain", b"test.txt", 0).unwrap();
        offer.chunks = 2;
        node.on_file_offer(sender, offer.clone());
        assert!(node.in_files.is_empty());
        offer.chunks = 1;
        for id in 1..=MAX_INCOMING_FILES_PER_SENDER as u64 + 1 {
            offer.file_id = id;
            node.on_file_offer(sender, offer.clone());
        }
        assert_eq!(node.in_files.len(), MAX_INCOMING_FILES_PER_SENDER);
        let mut sealed = [0; 255];
        let used = seal::seal_chunk(&offer.key, 1, 0, b"short", &mut sealed).unwrap();
        node.on_chunk(&sealed[..used]);
        assert_eq!(node.in_files[&1].got, 0);
        let used = seal::seal_chunk(&offer.key, 1, 0, data, &mut sealed).unwrap();
        node.on_chunk(&sealed[..used]);
        assert!(!node.in_files.contains_key(&1));
        let _ = fs::remove_dir_all(dir);
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
        n.links.insert(id, (link.to_string(), tx.into()));
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
    fn deposit_stores_and_refuses_unsigned_remote_deletion() {
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
        assert_eq!(n.mailbox.get(&r).map(|m| m.len()), Some(1));
        assert!(n.router.mail_waiting.contains(&r.path_id()));
        let request = dht::encode_req(&Address([4; 16]), None, &dht::Req::MailDelivered {
            recipient: r,
            ids: vec![id],
        });
        assert_eq!(dht::decode_resp(&n.dht_serve(&request)), Some((n.addr, dht::Resp::Stored(false))));
        assert_eq!(n.mailbox.get(&r).map(|m| m.len()), Some(1));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn delivery_receipt_from_recipient_marks_deposited_message_delivered() {
        use maarse_core::mail;
        let (mut n, rx, dir) = node("receipt");
        let recipient = Address([8; 16]);
        let sealed = b"sealed for the post office".to_vec();
        let msg_id = 0x1234_5678_9abc_def0u64;
        n.outbox.insert(msg_id, OutMsg { dest: recipient, mail_sealed: sealed.clone(), sent: Instant::now(), deposited: true, next_deposit: None });
        n.awaiting.insert(msg_id, Pending::Message);
        events(&rx);

        // A receipt for something we never sent changes nothing.
        let mut p: Vec<u8> = n.addr.0.to_vec();
        p.push(mail::OP_RECEIPT);
        p.extend_from_slice(&mail::receipt_id(&recipient, b"other").to_le_bytes());
        n.on_deposit(&p, 1);
        assert!(n.outbox.contains_key(&msg_id));
        assert!(events(&rx).is_empty());

        // The receipt for our copy: the app is told "acked" and the message is done.
        let mut p: Vec<u8> = n.addr.0.to_vec();
        p.push(mail::OP_RECEIPT);
        p.extend_from_slice(&mail::receipt_id(&recipient, &sealed).to_le_bytes());
        n.on_deposit(&p, 1);
        assert!(n.outbox.is_empty());
        assert!(!n.awaiting.contains_key(&msg_id));
        assert!(events(&rx).iter().any(|e| matches!(e, Event::Acked { msg_id: m } if *m == msg_id)));

        // A receipt addressed to someone else is not ours to act on.
        n.outbox.insert(msg_id, OutMsg { dest: recipient, mail_sealed: sealed.clone(), sent: Instant::now(), deposited: true, next_deposit: None });
        let mut p: Vec<u8> = Address([1; 16]).0.to_vec();
        p.push(mail::OP_RECEIPT);
        p.extend_from_slice(&mail::receipt_id(&recipient, &sealed).to_le_bytes());
        n.on_deposit(&p, 1);
        assert!(n.outbox.contains_key(&msg_id));
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
    fn old_dht_record_does_not_keep_a_radio_node_off_the_radio() {
        let (mut n, _rx, dir) = node("dht-radio");
        let phone = Address([5; 16]);
        // The phone was online: found in the DHT, heard over the backbone.
        heard(&mut n, phone, "tcp out 127.0.0.1:4242", 1);
        n.dht_hosts.insert(phone);
        assert_eq!(n.path_kind(&phone), PathKind::Tcp);
        // It lost the internet and is now heard on our radio only.
        heard(&mut n, phone, "serial COM3", 0);
        assert_eq!(n.path_kind(&phone), PathKind::Lora);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn own_page_is_answered_at_once() {
        let (mut n, rx, dir) = node("own-page");
        fs::create_dir_all(dir.join("pages")).unwrap();
        fs::write(dir.join("pages").join("demo.mu"), "# Demo\n").unwrap();
        n.command(1, Command::RequestPage { req: 7, dest: n.addr, path: text(b"demo") });
        let ev = events(&rx);
        let me = n.addr;
        assert!(ev.iter().any(|e| matches!(e, Event::FileIncoming { from, name, .. } if *from == me && name.as_slice() == b"demo")), "no page from ourselves");
        assert!(ev.iter().any(|e| matches!(e, Event::FileDone { ok: true, outgoing: false, .. })), "page not finished");
        assert!(n.out_files.is_empty(), "nothing goes over a link");
        // Our own pages are not "over LoRa" (pictures load at once).
        assert!(!n.file_limit(&me).via_lora, "own node counted as LoRa");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn caps_survive_a_restart() {
        let (mut n, _rx, dir) = node("caps-restart");
        let pc = Address([6; 16]);
        n.caps.insert(pc, maarse_core::appdata::CAP_FILE_PULL);
        n.save_caps();
        drop(n);
        let mut c = file_interop_tests::cfg(dir.clone());
        c.data_dir = dir.clone();
        let n = Node::new(&c).unwrap();
        assert_eq!(n.caps.get(&pc), Some(&maarse_core::appdata::CAP_FILE_PULL), "pull still known after a restart");
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
        n.mail_quota = 300; // bytes, small for the test
        let msg = |b: u8| vec![b; 100];
        assert_eq!(ask(&mut n, dht::Req::MailStore { recipient: Address([3; 16]), sealed: msg(1) }), dht::Resp::Stored(true));
        assert_eq!(ask(&mut n, dht::Req::MailStore { recipient: Address([4; 16]), sealed: msg(2) }), dht::Resp::Stored(true));
        assert_eq!(ask(&mut n, dht::Req::MailStore { recipient: Address([5; 16]), sealed: msg(3) }), dht::Resp::Stored(false), "over the quota");
        n.handle(In::MailFetched { recipient: Address([5; 16]), items: vec![msg(3)] });
        assert!(!n.mailbox.contains_key(&Address([5; 16])), "fetched mail must honor the quota");
        let dht::Resp::Caps(c) = ask(&mut n, dht::Req::Caps) else { panic!() };
        assert!(c.post_office && c.mail_free_mb == 0, "still a post office, but full");
    }


    #[test]
    fn failed_mailbox_write_is_not_acknowledged() {
        let mut n = node("disk-failure", |_| {});
        let recipient = Address([3; 16]);
        fs::create_dir(n.data_dir.join("mailbox.tmp")).unwrap();
        assert_eq!(ask(&mut n, dht::Req::MailStore { recipient, sealed: vec![1; 40] }), dht::Resp::Stored(false));
        assert!(!n.mailbox.contains_key(&recipient));
        assert!(!n.data_dir.join("mailbox.bin").exists());
    }

    #[test]
    fn replica_allowlist_restricts_the_placement_pool() {
        let approved = Address([7; 16]);
        let mut n = node("restricted-replicas", |cfg| cfg.mail_replica_ids = vec![approved]);
        n.dht.table.insert(Contact { id: approved, ep: "8.8.8.8:4242".into() });
        n.dht.table.insert(Contact { id: Address([1; 16]), ep: "9.9.9.9:4242".into() });
        assert_eq!(n.mail_replicas(&Address([3; 16])).into_iter().map(|c| c.id).collect::<Vec<_>>(),
            vec![approved]);
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
        n.links.insert(1, ("tcp in 10.0.0.1:1".into(), ta.into()));
        n.links.insert(2, ("tcp in 10.0.0.2:1".into(), tb.into()));
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

#[cfg(test)]
mod radio_file_e2e_tests {
    //! Two hosts on one radio-like link (as the phone node's Bluetooth modem):
    //! a file under the LoRa limit must arrive, chunk by chunk.
    use super::*;
    use std::sync::mpsc::{Receiver, channel};

    struct Side {
        n: Node,
        out: Receiver<Vec<u8>>,
        ev: Receiver<Vec<u8>>,
        _dir: PathBuf,
    }

    fn side(name: &str) -> Side {
        let dir = std::env::temp_dir().join(format!("maarse-e2e-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut c = file_interop_tests::cfg(dir.clone());
        c.data_dir = dir.clone();
        let mut n = Node::new(&c).unwrap();
        let (etx, ev) = channel();
        n.clients.insert(1, etx);
        let (ltx, out) = channel();
        n.handle(In::LinkUp { id: 1, name: "modem 127.0.0.1:47081".into(), tx: ltx.into() });
        // Resends and file timeouts run in housekeeping (Node::run schedules it).
        n.schedule(HOUSEKEEPING_EVERY, Timer::Housekeeping);
        Side { n, out, ev, _dir: dir }
    }

    /// Moves packets both ways and fires timers for `secs` of real time;
    /// `lose` drops every n-th packet (0 = none). Stops early when `done`.
    fn run(a: &mut Side, b: &mut Side, secs: u64, lose: usize, done: &mut dyn FnMut(&[Event], &[Event]) -> bool) -> (Vec<Event>, Vec<Event>) {
        let (mut ea, mut eb) = (Vec::new(), Vec::new());
        let end = Instant::now() + Duration::from_secs(secs);
        let mut count = 0usize;
        while Instant::now() < end {
            for (from, to) in [(0, 1), (1, 0)] {
                let pkts: Vec<Vec<u8>> = if from == 0 { a.out.try_iter().collect() } else { b.out.try_iter().collect() };
                for p in pkts {
                    count += 1;
                    if lose > 0 && count % lose == 0 {
                        continue;
                    }
                    let dst = if to == 0 { &mut a.n } else { &mut b.n };
                    dst.handle(In::Packet { data: p, snr: 8, link: 1, open_channel: None });
                }
            }
            a.n.fire_timers();
            b.n.fire_timers();
            ea.extend(a.ev.try_iter().filter_map(|f| Event::decode(&f[2..]).ok()));
            eb.extend(b.ev.try_iter().filter_map(|f| Event::decode(&f[2..]).ok()));
            if done(&ea, &eb) {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        (ea, eb)
    }

    fn file_arrives(lose: usize) {
        file_arrives_with(lose, true);
    }

    /// `announces`: let the link-up announces through (routes known); else A
    /// only knows B's key (as after a restart with stored contacts) and must
    /// find the route first.
    fn file_arrives_with(lose: usize, announces: bool) {
        let mut a = side(&format!("a{lose}{announces}"));
        let mut b = side(&format!("b{lose}{announces}"));
        if announces {
            run(&mut a, &mut b, 3, 0, &mut |_, _| false);
        } else {
            let _: Vec<_> = a.out.try_iter().collect();
            let _: Vec<_> = b.out.try_iter().collect();
            a.n.known.insert(b.n.addr, b.n.identity.public());
            b.n.known.insert(a.n.addr, a.n.identity.public());
        }
        assert!(a.n.known.contains_key(&b.n.addr), "A does not know B");
        let data: Vec<u8> = (0..7526u32).map(|i| (i * 31 % 251) as u8).collect();
        let req = 5;
        let dest = b.n.addr;
        a.n.command(1, Command::FileBegin { req, dest, size: data.len() as u32, mime: text(b"image/webp"), name: text(b"t.webp") });
        for (i, c) in data.chunks(MAX_TEXT).enumerate() {
            a.n.command(1, Command::FileData { req, offset: (i * MAX_TEXT) as u32, data: text(c) });
        }
        a.n.command(1, Command::FileEnd { req });
        // One chunk at a time over LoRa; a lost chunk or ack waits
        // CHUNK_TIMEOUT_LORA, so a lossy link needs time.
        let start = Instant::now();
        let (ea, eb) = run(&mut a, &mut b, 150, lose, &mut |ea, _| {
            ea.iter().any(|e| matches!(e, Event::FileDone { outgoing: true, .. }))
        });
        let done_a: Vec<_> = ea.iter().filter(|e| matches!(e, Event::FileDone { .. } | Event::Error { .. })).collect();
        let got_b = eb.iter().any(|e| matches!(e, Event::FileDone { ok: true, outgoing: false, .. }));
        println!("lose every {lose}: {:.1} s", start.elapsed().as_secs_f64());
        assert!(got_b, "B did not receive the file (lose every {lose}); A: {done_a:?}");
    }

    #[test]
    fn small_file_over_radio_link() {
        file_arrives(0);
    }

    #[test]
    fn small_file_over_lossy_radio_link() {
        file_arrives(10);
    }

    #[test]
    fn small_file_over_radio_link_without_route() {
        file_arrives_with(0, false);
    }

    /// One half-duplex LoRa radio (the modem): sends one packet at a time
    /// with its airtime, holds at most 16 waiting (as the firmware's queue),
    /// and hears nothing while it transmits.
    struct Radio {
        /// (packet, not before): USB/Bluetooth latency, then CAD backoffs.
        queue: std::collections::VecDeque<(Vec<u8>, Instant)>,
        cur: Option<(Vec<u8>, Instant, Instant)>,
        sent: Vec<(Instant, Instant)>,
        dropped: usize,
        rng: u64,
    }

    impl Radio {
        fn new(seed: u64) -> Self {
            Radio { queue: Default::default(), cur: None, sent: Vec::new(), dropped: 0, rng: seed | 1 }
        }
        fn rand_ms(&mut self, max: u64) -> Duration {
            self.rng ^= self.rng << 13;
            self.rng ^= self.rng >> 7;
            self.rng ^= self.rng << 17;
            Duration::from_millis(self.rng % max.max(1))
        }
    }

    /// Airtime scale in percent: 25 keeps the tests short; the benchmarks
    /// (`cargo test -- --ignored bench_`) use 100, the real airtime.
    static SCALE_PCT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(25);

    /// Airtime at SF9 / 125 kHz / CR 4/5 / preamble 16 (the radio settings
    /// in use), scaled by SCALE_PCT.
    fn airtime(len: usize) -> Duration {
        let scale = SCALE_PCT.load(std::sync::atomic::Ordering::Relaxed) as f64 / 100.0;
        let ts = 512.0 / 125_000.0;
        // CR 4/5, explicit header, CRC; no low data rate optimisation at
        // SF9/125 kHz (symbol 4.1 ms < 16 ms): 4 x SF = 36 bits per block.
        let n = 8.0 + (((8 * len) as f64 - 36.0 + 28.0 + 16.0) / 36.0).ceil().max(0.0) * 5.0;
        Duration::from_secs_f64(((16.0 + 4.25) * ts + n * ts) * scale)
    }

    impl Radio {
        fn push(&mut self, p: Vec<u8>) {
            if self.queue.len() >= 16 {
                self.dropped += 1;
            } else {
                // Host -> radio and back over Bluetooth/USB and the host's own
                // handling: 80..200 ms (0.28 s per chunk round was measured on
                // the bench for both ends together).
                let at = Instant::now() + Duration::from_millis(80) + self.rand_ms(120);
                self.queue.push_back((p, at));
            }
        }
        /// A packet that has finished its airtime, if any; starts the next
        /// one when it is due and the channel is free (`busy`: the other
        /// radio is sending, which CAD hears: back off 100..600 ms).
        fn step(&mut self, now: Instant, busy: bool) -> Option<(Vec<u8>, Instant, Instant)> {
            let mut done = None;
            if self.cur.as_ref().is_some_and(|c| c.2 <= now) {
                done = self.cur.take();
            }
            if self.cur.is_none() && self.queue.front().is_some_and(|q| q.1 <= now) {
                if busy {
                    let back = Duration::from_millis(100) + self.rand_ms(500);
                    if let Some(q) = self.queue.front_mut() {
                        q.1 = now + back;
                    }
                } else if let Some((p, _)) = self.queue.pop_front() {
                    let end = now + airtime(p.len());
                    self.sent.push((now, end));
                    self.cur = Some((p, now, end));
                }
            }
            self.sent.retain(|s| now.duration_since(s.1) < Duration::from_secs(30));
            done
        }
        /// Did we transmit at any time during `from..to` (deaf then)?
        fn deaf(&self, from: Instant, to: Instant) -> bool {
            self.sent.iter().any(|s| s.0 < to && from < s.1)
        }
    }

    /// Like `run`, but packets go through two half-duplex radios.
    fn run_radio(a: &mut Side, b: &mut Side, secs: u64, done: &mut dyn FnMut(&[Event], &[Event]) -> bool) -> (Vec<Event>, Vec<Event>, usize, usize) {
        run_radio_lossy(a, b, secs, 0.0, done)
    }

    /// `loss`: chance that a packet is lost on air anyway (collisions with
    /// other traffic, fading), on top of half-duplex deafness.
    fn run_radio_lossy(a: &mut Side, b: &mut Side, secs: u64, loss: f64, done: &mut dyn FnMut(&[Event], &[Event]) -> bool) -> (Vec<Event>, Vec<Event>, usize, usize) {
        let (mut ea, mut eb) = (Vec::new(), Vec::new());
        let (mut ra, mut rb) = (Radio::new(0x1234_5678), Radio::new(0x8765_4321));
        let mut lost = 0;
        let t0 = Instant::now();
        let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut unlucky = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % 10_000) as f64 / 10_000.0 < loss
        };
        let end = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < end {
            for p in a.out.try_iter() {
                ra.push(p);
            }
            for p in b.out.try_iter() {
                rb.push(p);
            }
            let now = Instant::now();
            let show = std::env::var("RADIO_DEBUG").is_ok();
            let kind = |p: &[u8]| Header::decode(p).map(|(h, _)| format!("{:?}", h.ptype)).unwrap_or_default();
            let (a_busy, b_busy) = (rb.cur.is_some(), ra.cur.is_some());
            if let Some((p, s, e)) = ra.step(now, a_busy) {
                if rb.deaf(s, e) || unlucky() {
                    lost += 1;
                    if show { println!("RADIO lost A->B {} ({} B){}", kind(&p), p.len(), if rb.deaf(s, e) { " B was sending" } else { "" }); }
                } else {
                    if std::env::var("RADIO_TRACE").is_ok() { println!("TRACE {:7.2} A->B {:?} {} B air {:.2}", t0.elapsed().as_secs_f64(), Header::decode(&p).map(|(h, _)| h.ptype).ok(), p.len(), e.duration_since(s).as_secs_f64()); }
                    b.n.handle(In::Packet { data: p, snr: 8, link: 1, open_channel: None })
                }
            }
            if let Some((p, s, e)) = rb.step(now, b_busy) {
                if ra.deaf(s, e) || unlucky() {
                    lost += 1;
                    if show { println!("RADIO lost B->A {} ({} B){}", kind(&p), p.len(), if ra.deaf(s, e) { " A was sending" } else { "" }); }
                } else {
                    if std::env::var("RADIO_TRACE").is_ok() { println!("TRACE {:7.2} B->A {:?} {} B air {:.2}", t0.elapsed().as_secs_f64(), Header::decode(&p).map(|(h, _)| h.ptype).ok(), p.len(), e.duration_since(s).as_secs_f64()); }
                    a.n.handle(In::Packet { data: p, snr: 8, link: 1, open_channel: None })
                }
            }
            a.n.fire_timers();
            b.n.fire_timers();
            ea.extend(a.ev.try_iter().filter_map(|f| Event::decode(&f[2..]).ok()));
            eb.extend(b.ev.try_iter().filter_map(|f| Event::decode(&f[2..]).ok()));
            if done(&ea, &eb) {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        (ea, eb, lost, ra.dropped + rb.dropped)
    }

    /// A, B on half-duplex radios, announced to each other in turn.
    fn radio_pair(name: &str) -> (Side, Side) {
        let mut a = side(&format!("{name}-a"));
        let mut b = side(&format!("{name}-b"));
        run_radio(&mut a, &mut b, 1, &mut |_, _| false);
        b.n.announce();
        run_radio(&mut a, &mut b, 2, &mut |_, _| false);
        a.n.announce();
        run_radio(&mut a, &mut b, 2, &mut |_, _| false);
        assert!(a.n.known.contains_key(&b.n.addr), "A does not know B");
        (a, b)
    }

    fn upload(n: &mut Node, req: u16, dest: Address, data: &[u8]) {
        n.command(1, Command::FileBegin { req, dest, size: data.len() as u32, mime: text(b"image/webp"), name: text(b"t.webp") });
        for (i, c) in data.chunks(MAX_TEXT).enumerate() {
            n.command(1, Command::FileData { req, offset: (i * MAX_TEXT) as u32, data: text(c) });
        }
        n.command(1, Command::FileEnd { req });
    }

    /// Two pictures to the same recipient at once (e.g. the file mailbox
    /// delivering while a new one is sent): one after the other over LoRa,
    /// both arrive.
    #[test]
    fn two_files_over_half_duplex_radio() {
        let (mut a, mut b) = radio_pair("hd2");
        let dest = b.n.addr;
        let one: Vec<u8> = (0..3000u32).map(|i| (i * 7 % 251) as u8).collect();
        let two: Vec<u8> = (0..3000u32).map(|i| (i * 11 % 251) as u8).collect();
        upload(&mut a.n, 1, dest, &one);
        upload(&mut a.n, 2, dest, &two);
        let start = Instant::now();
        let (ea, eb, lost, dropped) = run_radio(&mut a, &mut b, 90, &mut |_, eb| {
            eb.iter().filter(|e| matches!(e, Event::FileDone { ok: true, outgoing: false, .. })).count() >= 2
        });
        let got = eb.iter().filter(|e| matches!(e, Event::FileDone { ok: true, outgoing: false, .. })).count();
        println!("two files: {:.1} s, {lost} lost while deaf, {dropped} dropped", start.elapsed().as_secs_f64());
        let done_a: Vec<_> = ea.iter().filter(|e| matches!(e, Event::FileDone { .. })).collect();
        assert_eq!(got, 2, "B got {got} of 2 files; A: {done_a:?}; {lost} lost while deaf");
    }

    /// A file offered again (from the sender's file mailbox) keeps the
    /// chunks already received, and a half-received file is only dropped
    /// after FILE_TIMEOUT without activity, not FILE_TIMEOUT after the offer.
    #[test]
    fn receiver_keeps_chunks_when_offered_again() {
        let (mut a, mut b) = radio_pair("again");
        let data: Vec<u8> = (0..3000u32).map(|i| (i * 13 % 251) as u8).collect();
        upload(&mut a.n, 1, b.n.addr, &data);
        // Stop halfway (B has a few chunks), as when the link breaks.
        run_radio(&mut a, &mut b, 30, &mut |_, eb| {
            eb.iter().any(|e| matches!(e, Event::FileProgress { outgoing: false, done, .. } if *done >= 3))
        });
        let id = *b.n.in_files.keys().next().expect("B has a half-received file");
        let f = b.n.in_files.get_mut(&id).unwrap();
        assert!(f.got >= 3 && f.got < f.offer.chunks, "not half received: {} of {}", f.got, f.offer.chunks);
        let got_before = f.got;
        let offer = f.offer.clone();
        let from = f.from;
        b.n.on_file_offer(from, offer);
        assert_eq!(b.n.in_files[&id].got, got_before, "chunks were thrown away on the second offer");
        b.n.housekeeping();
        assert!(b.n.in_files.contains_key(&id), "an active half-received file was dropped");
        b.n.in_files.get_mut(&id).unwrap().active = Instant::now() - FILE_TIMEOUT - Duration::from_secs(1);
        b.n.housekeeping();
        assert!(!b.n.in_files.contains_key(&id), "an idle half-received file was kept");
    }

    /// 7.5 KB over one LoRa hop with real airtime, `loss` of the packets
    /// lost on air. Returns seconds until the receiver has the file.
    fn bench_file(name: &str, loss: f64) -> f64 {
        bench_file_mode(name, loss, true)
    }

    /// `pull`: the new way (recipient asks for batches) or the old push way.
    fn bench_file_mode(name: &str, loss: f64, pull: bool) -> f64 {
        SCALE_PCT.store(100, std::sync::atomic::Ordering::Relaxed);
        let (mut a, mut b) = radio_pair(name);
        a.n.file_pull = pull;
        b.n.file_pull = pull;
        let data: Vec<u8> = (0..7526u32).map(|i| (i * 17 % 251) as u8).collect();
        upload(&mut a.n, 1, b.n.addr, &data);
        let start = Instant::now();
        let (_, eb, lost, _) = run_radio_lossy(&mut a, &mut b, 900, loss, &mut |_, eb| {
            eb.iter().any(|e| matches!(e, Event::FileDone { ok: true, outgoing: false, .. }))
        });
        let secs = start.elapsed().as_secs_f64();
        let ok = eb.iter().any(|e| matches!(e, Event::FileDone { ok: true, outgoing: false, .. }));
        println!("BENCH {name} ({}): loss {:.0} %: {secs:.0} s (real airtime), {lost} packets lost, received {ok}", if pull { "pull" } else { "push" }, loss * 100.0);
        assert!(ok, "not received");
        secs
    }

    /// Benchmarks with real airtime (minutes each): run with
    /// `cargo test -p maarse-host --lib -- --ignored --nocapture bench_`.
    #[test]
    #[ignore]
    fn bench_file_no_loss() {
        bench_file("bench0", 0.0);
    }

    #[test]
    #[ignore]
    fn bench_file_12pct_loss() {
        bench_file("bench12", 0.12);
    }

    #[test]
    #[ignore]
    fn bench_file_push_no_loss() {
        bench_file_mode("bench0push", 0.0, false);
    }

    #[test]
    #[ignore]
    fn bench_file_push_12pct_loss() {
        bench_file_mode("bench12push", 0.12, false);
    }

    /// Starts a 3 KB file from A to B and runs until B has a few chunks.
    fn half_sent(name: &str) -> (Side, Side, u64) {
        let (mut a, mut b) = radio_pair(name);
        let data: Vec<u8> = (0..3000u32).map(|i| (i * 19 % 251) as u8).collect();
        upload(&mut a.n, 1, b.n.addr, &data);
        run_radio(&mut a, &mut b, 30, &mut |_, eb| {
            eb.iter().any(|e| matches!(e, Event::FileProgress { outgoing: false, done, .. } if *done >= 3))
        });
        let id = *a.n.out_files.keys().next().expect("A is sending");
        assert!(b.n.in_files.contains_key(&id), "B is not receiving");
        (a, b, id)
    }

    fn file_done_text(ev: &[Event], outgoing: bool) -> Option<String> {
        ev.iter().find_map(|e| match e {
            Event::FileDone { outgoing: o, ok: false, text, .. } if *o == outgoing => Some(String::from_utf8_lossy(text).to_string()),
            _ => None,
        })
    }

    /// The sender stops: the recipient is told and drops what it has.
    #[test]
    fn sender_stops_a_file() {
        let (mut a, mut b, id) = half_sent("stop-s");
        a.n.command(1, Command::CancelFile { file_id: id });
        let start = Instant::now();
        let (ea, eb, _, _) = run_radio(&mut a, &mut b, 60, &mut |_, eb| file_done_text(eb, false).is_some());
        println!("sender stop reached the recipient after {:.1} s", start.elapsed().as_secs_f64());
        assert_eq!(file_done_text(&ea, true).as_deref(), Some("cancelled"));
        assert_eq!(file_done_text(&eb, false).as_deref(), Some("cancelled by the sender"));
        assert!(a.n.out_files.is_empty() && b.n.in_files.is_empty());
    }

    /// The recipient stops: the sender is told, stops sending and drops the
    /// file from its file mailbox too.
    #[test]
    fn recipient_stops_a_file() {
        let (mut a, mut b, id) = half_sent("stop-r");
        b.n.command(1, Command::CancelFile { file_id: id });
        let (ea, eb, _, _) = run_radio(&mut a, &mut b, 15, &mut |ea, _| file_done_text(ea, true).is_some());
        assert_eq!(file_done_text(&eb, false).as_deref(), Some("cancelled"));
        assert_eq!(file_done_text(&ea, true).as_deref(), Some("cancelled by the recipient"));
        assert!(a.n.out_files.is_empty() && b.n.in_files.is_empty());
        assert!(a.n.filebox.recipient(id).is_none(), "still in the file mailbox");
    }

    /// An offer that arrives after the file was stopped (resent before the
    /// stop got through) does not start it again, from either end.
    #[test]
    fn late_offer_after_stop_is_refused() {
        for by_recipient in [true, false] {
            let (mut a, mut b, id) = half_sent(if by_recipient { "late-r" } else { "late-s" });
            let offer = b.n.in_files[&id].offer.clone();
            if by_recipient {
                b.n.command(1, Command::CancelFile { file_id: id });
            } else {
                a.n.command(1, Command::CancelFile { file_id: id });
                run_radio(&mut a, &mut b, 60, &mut |_, eb| file_done_text(eb, false).is_some());
            }
            let sender = a.n.addr;
            b.n.on_file_offer(sender, offer);
            assert!(b.n.in_files.is_empty(), "a late offer started the file again (by_recipient {by_recipient})");
        }
    }

    /// Over LoRa a node gets five pages in half a minute; the sixth request
    /// gets one "Slow down" page and the seventh nothing.
    #[test]
    fn page_requests_over_lora_are_limited_with_one_notice() {
        let (mut a, mut b) = radio_pair("pageburst");
        let pages = b.n.data_dir.join("pages");
        fs::create_dir_all(&pages).unwrap();
        for i in 0..7 {
            fs::write(pages.join(format!("p{i}.mu")), format!("# Page {i}
")).unwrap();
        }
        fs::write(pages.join("index.mu"), "# Front
").unwrap();
        for i in 0..7u16 {
            a.n.command(1, Command::RequestPage { req: 20 + i, dest: b.n.addr, path: text(format!("p{i}").as_bytes()) });
        }
        let (ea, _, _, _) = run_radio(&mut a, &mut b, 240, &mut |_, _| false);
        let done = ea.iter().filter(|e| matches!(e, Event::FileDone { outgoing: false, ok: true, .. })).count();
        assert_eq!(done, 6, "five pages and one slow-down notice");
    }

    /// The markup of the last page A received in `ea`.
    fn received_markup(ea: &[Event]) -> String {
        let mut data = Vec::new();
        for e in ea {
            match e {
                Event::FileIncoming { .. } => data.clear(),
                Event::FileChunk { data: d, .. } => data.extend_from_slice(d),
                _ => {}
            }
        }
        String::from_utf8(miniz_oxide::inflate::decompress_to_vec(&data).unwrap_or_default()).unwrap_or_default()
    }

    /// A locked page over LoRa: a stranger gets "locked", a member gets the
    /// page without the lock lines; a member writes on its notice board.
    #[test]
    fn locked_page_and_notice_board() {
        let (mut a, mut b) = radio_pair("pagelock");
        let pages = b.n.data_dir.join("pages");
        fs::create_dir_all(&pages).unwrap();
        fs::write(pages.join("crew.mu"), "@allow 00000000000000000000000000000001\n@title Crew\n# Crew\n@board 5\n").unwrap();
        let get = |a: &mut Side, b: &mut Side, req: u16| {
            a.n.command(1, Command::RequestPage { req, dest: b.n.addr, path: text(b"crew") });
            let (ea, _, _, _) = run_radio(a, b, 120, &mut |ea, _| ea.iter().any(|e| matches!(e, Event::FileDone { outgoing: false, .. })));
            received_markup(&ea)
        };
        let m = get(&mut a, &mut b, 1);
        assert!(m.contains("This page is locked"), "a stranger got: {m}");
        assert!(!m.contains("@allow"), "the list left the host");
        // A is put on the list.
        let me: String = a.n.addr.0.iter().map(|x| format!("{x:02x}")).collect();
        fs::write(pages.join("crew.mu"), format!("@allow {me} 00000000000000000000000000000001\n@title Crew\n# Crew\n@board 5\n")).unwrap();
        let m = get(&mut a, &mut b, 2);
        assert!(m.contains("# Crew") && m.contains("@lockinfo 3"), "a member got: {m}");
        assert!(!m.contains("@allow"), "the list left the host");
        // A writes a note; it is on the page next time.
        a.n.command(1, Command::PostBoard { req: 3, dest: b.n.addr, path: text(b"crew"), text: text(b"Sail at 9?") });
        run_radio(&mut a, &mut b, 60, &mut |_, _| pages.join("crew.board").is_file());
        let m = get(&mut a, &mut b, 4);
        assert!(m.contains("@boardpath crew") && m.contains("\tSail at 9?"), "no note on the page: {m}");
        // A second note within a minute is not kept.
        assert!(b.n.on_board_post(a.n.addr, "crew", b"again").is_err());
    }

    /// A page nobody asked for (an old answer) is refused: the reader never
    /// sees it as a file, and the sender drops it instead of keeping it in
    /// the file mailbox.
    #[test]
    fn page_not_asked_for_is_refused_and_dropped() {
        let (mut a, mut b) = radio_pair("pageunasked");
        let pages = b.n.data_dir.join("pages");
        fs::create_dir_all(&pages).unwrap();
        fs::write(pages.join("index.mu"), "# Front\n").unwrap();
        // Asked for: it arrives.
        a.n.command(1, Command::RequestPage { req: 1, dest: b.n.addr, path: text(b"index") });
        let (ea, _, _, _) = run_radio(&mut a, &mut b, 120, &mut |ea, _| ea.iter().any(|e| matches!(e, Event::FileDone { outgoing: false, .. })));
        assert!(ea.iter().any(|e| matches!(e, Event::FileDone { outgoing: false, ok: true, .. })), "the asked page came");
        // Sent again without A asking (A no longer waits): refused.
        a.n.pages = Default::default();
        b.n.on_page_request(a.n.addr, &content::Small::from_slice(b"index").unwrap());
        assert!(b.n.out_files.values().any(|f| f.page), "page answer marked");
        let (ea, _, _, _) = run_radio(&mut a, &mut b, 120, &mut |_, _| false);
        assert!(!ea.iter().any(|e| matches!(e, Event::FileIncoming { .. })), "an unasked page reached the client");
        assert!(b.n.out_files.is_empty(), "the sender still holds the page");
        assert_eq!(b.n.filebox.len(), 0, "a page went to the file mailbox");
    }

    /// A lost session shows one notice in the chat, however many messages
    /// arrive that cannot be read.
    #[test]
    fn lost_session_gives_one_notice_not_one_per_message() {
        let (mut a, mut b) = radio_pair("lostsess");
        let notices = |e: &[Event]| e.iter().filter(|e| matches!(e, Event::DirectReceived { text: t, .. } if t.as_slice() == UNREADABLE_NOTICE.as_bytes())).count();
        // A first message sets up the session both ways.
        a.n.command(1, Command::SendDirect { req: 1, dest: b.n.addr, text: text(b"hello") });
        run_radio(&mut a, &mut b, 60, &mut |_, eb| eb.iter().any(|e| matches!(e, Event::DirectReceived { .. })));
        b.n.command(1, Command::SendDirect { req: 2, dest: a.n.addr, text: text(b"hi") });
        run_radio(&mut a, &mut b, 60, &mut |ea, _| ea.iter().any(|e| matches!(e, Event::DirectReceived { .. })));
        // B loses its sessions (a reinstall, say); A sends three more.
        b.n.sessions = crate::sessions::Sessions::load(b.n.data_dir.join("gone.bin"));
        for (i, m) in [b"one".as_slice(), b"two", b"three"].into_iter().enumerate() {
            a.n.command(1, Command::SendDirect { req: 10 + i as u16, dest: b.n.addr, text: text(m) });
        }
        let (_, eb, _, _) = run_radio(&mut a, &mut b, 90, &mut |_, _| false);
        assert_eq!(notices(&eb), 1, "one notice for the lost session");
    }

    /// A picture on a page over LoRa: B makes the small copy in the style
    /// A asks for (dots: a 1-bit PNG; grey: a small JPEG), and it arrives.
    #[test]
    fn page_picture_over_lora_is_made_small() {
        let (mut a, mut b) = radio_pair("pagepic");
        let pages = b.n.data_dir.join("pages");
        fs::create_dir_all(&pages).unwrap();
        let img = image::RgbImage::from_fn(800, 600, |x, y| image::Rgb([(x / 4) as u8, (y / 3) as u8, ((x + y) / 8) as u8]));
        image::DynamicImage::ImageRgb8(img).save(pages.join("map.jpg")).unwrap();
        // Over LoRa the smallest copy comes whatever the style asks.
        for (style, mime, max) in [("dots", "", 4000u32), ("colour", "", 4000)] {
            a.n.command(1, Command::RequestPage { req: 9, dest: b.n.addr, path: text(format!("_{style}/map.jpg").as_bytes()) });
            let (ea, _, _, _) = run_radio(&mut a, &mut b, 120, &mut |ea, _| ea.iter().any(|e| matches!(e, Event::FileDone { outgoing: false, .. })));
            let got = ea.iter().find_map(|e| match e {
                Event::FileIncoming { mime: m, size, .. } => Some((String::from_utf8_lossy(m).to_string(), *size)),
                _ => None,
            });
            let (m, size) = got.unwrap_or_else(|| panic!("no picture for {style}"));
            assert!(m == "image/png" || m == "image/webp", "{style}: {m}{mime}");
            assert!(size <= max, "{style}: {size} bytes");
            assert!(ea.iter().any(|e| matches!(e, Event::FileDone { outgoing: false, ok: true, .. })), "{style} not finished");
            println!("{style}: {size} bytes over LoRa");
        }
        // Asked for the better copy afterwards: colour, bigger, within 8 KB.
        a.n.command(1, Command::RequestPage { req: 10, dest: b.n.addr, path: text(b"_c8/map.jpg") });
        let (ea, _, _, _) = run_radio(&mut a, &mut b, 240, &mut |ea, _| ea.iter().any(|e| matches!(e, Event::FileDone { outgoing: false, .. })));
        let got = ea.iter().find_map(|e| match e {
            Event::FileIncoming { mime: m, size, .. } => Some((String::from_utf8_lossy(m).to_string(), *size)),
            _ => None,
        });
        let (m, size) = got.expect("no better picture");
        assert_eq!(m, "image/webp");
        assert!(size > 1000 && size <= 8192, "better picture {size} bytes");
        assert!(ea.iter().any(|e| matches!(e, Event::FileDone { outgoing: false, ok: true, .. })), "better picture not finished");
        println!("better picture: {size} bytes over LoRa");
        // Forced by the page ("colour!"): colour straight away, within 8 KB.
        a.n.command(1, Command::RequestPage { req: 11, dest: b.n.addr, path: text(b"_fcolour/map.jpg") });
        let (ea, _, _, _) = run_radio(&mut a, &mut b, 240, &mut |ea, _| ea.iter().any(|e| matches!(e, Event::FileDone { outgoing: false, .. })));
        let size = ea.iter().find_map(|e| match e { Event::FileIncoming { size, .. } => Some(*size), _ => None }).expect("no forced picture");
        assert!(size > 1000 && size <= 8192, "forced colour {size} bytes");
        println!("forced colour: {size} bytes over LoRa");
    }

    /// The phone node's case: a 7.5 KB picture over one LoRa hop through
    /// half-duplex modems. A burst of chunks makes the sender deaf to the
    /// acks, so chunks must go one at a time over LoRa.
    #[test]
    fn small_file_over_half_duplex_radio() {
        let mut a = side("hd-a");
        let mut b = side("hd-b");
        // The link-up announces collide (both send at once): announce in turn.
        run_radio(&mut a, &mut b, 1, &mut |_, _| false);
        b.n.announce();
        run_radio(&mut a, &mut b, 2, &mut |_, _| false);
        a.n.announce();
        run_radio(&mut a, &mut b, 2, &mut |_, _| false);
        assert!(a.n.known.contains_key(&b.n.addr), "A does not know B");
        let data: Vec<u8> = (0..7526u32).map(|i| (i * 31 % 251) as u8).collect();
        let (req, dest) = (5, b.n.addr);
        a.n.command(1, Command::FileBegin { req, dest, size: data.len() as u32, mime: text(b"image/webp"), name: text(b"t.webp") });
        for (i, c) in data.chunks(MAX_TEXT).enumerate() {
            a.n.command(1, Command::FileData { req, offset: (i * MAX_TEXT) as u32, data: text(c) });
        }
        a.n.command(1, Command::FileEnd { req });
        let start = Instant::now();
        let (ea, eb, lost, dropped) = run_radio(&mut a, &mut b, 90, &mut |ea, _| {
            ea.iter().any(|e| matches!(e, Event::FileDone { outgoing: true, .. }))
        });
        let got_b = eb.iter().any(|e| matches!(e, Event::FileDone { ok: true, outgoing: false, .. }));
        let done_a: Vec<_> = ea.iter().filter(|e| matches!(e, Event::FileDone { .. })).collect();
        println!("half duplex: {:.1} s (x4 for real airtime), {lost} lost while deaf, {dropped} dropped from full queues", start.elapsed().as_secs_f64());
        assert!(got_b, "B did not receive the file; A: {done_a:?}; {lost} lost while deaf, {dropped} dropped");
        assert!(done_a.iter().any(|e| matches!(e, Event::FileDone { ok: true, .. })), "A was not told it was delivered: {done_a:?}");
    }
}

/// GetBackbones is the bare command byte. Sensor commands share the byte 0x39 but carry content,
/// so matching on the first byte alone swallowed every sensor command from a client.
fn is_get_backbones(frame: &[u8]) -> bool {
    frame == [backbone::CMD_GET_BACKBONES]
}

#[cfg(test)]
mod get_backbones_tests {
    use super::*;

    #[test]
    fn only_the_bare_byte_is_get_backbones() {
        assert!(is_get_backbones(&[backbone::CMD_GET_BACKBONES]));
        assert!(!is_get_backbones(&[backbone::CMD_GET_BACKBONES, 1, 2, 3]));
        assert!(!is_get_backbones(&[]));
    }
}
