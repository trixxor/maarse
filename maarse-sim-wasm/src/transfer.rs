//! Content transfers in the simulated world: files, voice messages,
//! pictures, Pages, calls and the IP tunnel.
//!
//! What is real and what is a model:
//!
//! - **Real rules** (from `maarse-hostrules`, the same code maarse-host
//!   runs): route kind ([`filebox::classify`]), size limit and file mailbox
//!   ([`filebox::limit`], [`filebox::LORA_MAX_FILE`], [`filebox::FILEBOX_TTL_MS`]),
//!   call refusal ([`calls::call_refusal`]), page burst rule and picture copy
//!   sizes ([`pages`]), tunnel header compression, pieces and NACK timing
//!   ([`tunnel`]), the editable [`Rules`] profile.
//! - **Real packets**: every offer, chunk, pull request, page request, call
//!   frame and tunnel piece is a packet through the node's real
//!   `maarse_core::Router` (the same packet types maarse-host uses: offers,
//!   page requests, call set-up and tunnel keys as acknowledged `Data`;
//!   pushed chunks as `Fragment`; pulled chunks, pull requests, call audio
//!   and tunnel pieces as `Datagram`), with the real sizes (sealing overhead
//!   included). The payload bytes are a simulator header plus padding, not
//!   ciphertext.
//! - **Model**: which link a recipient was "heard on" (the host's
//!   `heard_via`) is taken from the simulated links: a route counts as
//!   internet all the way when a chain of TCP links through hosts (or two
//!   hosts both on the backbone) joins the two ends. The file mailbox sits at
//!   the sender's host; the transfer from the mailbox still runs from the
//!   sender's router. The SSH/ping/bulk workloads are traffic models (no real
//!   TCP stack); the IPv6 packets they send are built byte for byte and
//!   compressed and split with the real tunnel code.

use super::*;
use maarse_core::companion::{Event, MAX_TEXT, RadioConfig, error_code, text};
use maarse_core::content::{self, FileOffer, Small};
use maarse_crypto::seal::{CHUNK_DATA, CHUNK_OVERHEAD, DM_OVERHEAD};
use maarse_hostrules::{calls, filebox, filebox::PathKind, pages, rules::Rules, tunnel};

/// Simulator payload prefix of every transfer packet.
const MAGIC: &[u8; 2] = b"XF";

// Packet operations (third payload byte).
const OP_OFFER: u8 = b'O';
const OP_CHUNK: u8 = b'C';
const OP_REQUEST: u8 = b'R';
const OP_CANCEL: u8 = b'X';
const OP_PAGE: u8 = b'P';
const OP_RING: u8 = b'K';
const OP_AUDIO: u8 = b'A';
const OP_PHONE_CALL: u8 = b'Q';
const OP_TUN_KEY: u8 = b'T';
const OP_TUN_PIECE: u8 = b'D';
const OP_TUN_NACK: u8 = b'N';
const OP_BOARD: u8 = b'B';

// Timer kinds (`Ev::Xfer { what }`).
const W_CHECK: u8 = 1;
const W_PULL: u8 = 2;
const W_BOX: u8 = 3;
const W_AUDIO_A: u8 = 5;
const W_AUDIO_B: u8 = 6;
const W_CALL_END: u8 = 7;
const W_TUN: u8 = 8;
const W_PAGE_WAIT: u8 = 10;
const W_TUN_NOKEY: u8 = 11;
const W_ECHO: u8 = 12;
const W_SPEED: u8 = 13;

// maarse-host's file transfer constants (node.rs).
/// Chunks in flight over the internet ...
const FILE_WINDOW: usize = 8;
/// ... and when LoRa may be on the way: one at a time.
const FILE_WINDOW_LORA: usize = 1;
/// Resend after this without an ack over the internet ...
const CHUNK_TIMEOUT_MS: u64 = 20_000;
/// ... and over LoRa before the first round trip is measured.
const CHUNK_TIMEOUT_LORA_MS: u64 = 8_000;
const PULL_WINDOW_START: u8 = 4;
const PULL_WINDOW_MAX: u8 = 8;
const PULL_WINDOW_MAX_RELAYED: u8 = 2;
const PULL_MAX_RETRIES: u8 = 12;
/// No request from a pulling recipient for this long: offer again.
const PULL_STALL_MS: u64 = 30_000;
/// A transfer without completion is given up (or put in the file mailbox) after this.
const FILE_TIMEOUT_MS: u64 = 600_000;
/// The mailbox is checked for recipients that can be reached this often.
const BOX_POLL_MS: u64 = 10_000;

/// Codec2 at 1200 bit/s: 150 bytes of audio per second.
pub const VOICE_BYTES_PER_S: u32 = 1200 / 8;
pub const VOICE_MIME: &str = "audio/x-codec2";
/// Opus in the client: 16 kbit/s, 20 ms frames, up to three frames per packet.
const OPUS_FRAME_BYTES: usize = 16_000 / 8 / 50;
const CALL_FRAMES_PER_PACKET: usize = 3;
const CALL_PACKET_MS: u64 = 20 * CALL_FRAMES_PER_PACKET as u64;

fn lora_chunk_timeout(srtt: Option<u64>) -> u64 {
    srtt.map_or(CHUNK_TIMEOUT_LORA_MS, |r| (r * 5 / 2 + 500).clamp(2_500, CHUNK_TIMEOUT_MS))
}

fn kind_name(k: PathKind) -> &'static str {
    match k {
        PathKind::Tcp => "tcp",
        PathKind::Lora => "lora",
        PathKind::Unknown => "unknown",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemKind {
    File,
    Voice,
    Picture,
    /// A page request (the answer is a child item of kind `PageReply`).
    Page,
    PageReply,
    /// A note for a page's notice board.
    Board,
    Call,
    Tunnel,
}

impl ItemKind {
    fn name(self) -> &'static str {
        match self {
            ItemKind::File => "file",
            ItemKind::Voice => "voice",
            ItemKind::Picture => "picture",
            ItemKind::Page => "page",
            ItemKind::PageReply => "page_reply",
            ItemKind::Board => "board",
            ItemKind::Call => "call",
            ItemKind::Tunnel => "tunnel",
        }
    }
}

/// The recipient's pull state (maarse-host `PullRx`).
#[derive(Clone, Debug, Default)]
struct Pull {
    window: u8,
    asked: Vec<u16>,
    deadline: u64,
    retries: u8,
    chunk_ms: u64,
    last_rx: Option<u64>,
    troubled: bool,
    seq: u16,
    relayed: bool,
}

/// A tunnel packet in pieces at the receiving end.
#[derive(Clone, Debug)]
struct Reasm {
    parts: Vec<bool>,
    first: u64,
    nacks: u8,
    what: u8,
    reference: u32,
}

/// What a tunnel workload's packet is.
const T_PING: u8 = 1;
const T_PONG: u8 = 2;
const T_KEY: u8 = 3;
const T_ECHO: u8 = 4;
const T_SCREEN: u8 = 5;
const T_BULK: u8 = 6;
const T_BULK_ACK: u8 = 7;

#[derive(Clone, Debug, Default)]
struct Tun {
    workload: String,
    started: u64,
    next_seq: [u32; 2],
    /// Sent pieces kept for NACKs: (dir, seq) -> (piece lengths, what, ref, when).
    sent: HashMap<(u8, u32), (Vec<usize>, u8, u32, u64)>,
    rx: HashMap<(u8, u32), Reasm>,
    /// Requests waiting for their answer: reference -> (first sent, last sent, tries).
    open: HashMap<u32, (u64, u64, u8)>,
    rtts: Vec<u64>,
    /// Workload position: pings / keystrokes sent, bulk segments sent.
    next: u32,
    total: u32,
    acked: u32,
    retrans: u32,
    lost_pieces: u32,
    nacks: u32,
    pieces: u32,
    bytes_on_air: u64,
    /// The last screen update (ssh).
    screen_at: u64,
    srtt: Option<u64>,
    finished: Option<u64>,
    first_done: Option<u64>,
    last_done: Option<u64>,
}

#[derive(Clone, Debug, Default)]
struct CallStats {
    sent: [u32; 2],
    recv: [u32; 2],
    lat: Vec<u64>,
    started: u64,
    until: u64,
    seq: [u32; 2],
    /// Call test service: 0 = an ordinary call, else calls::test::MUSIC/ECHO/SPEED.
    mode: u8,
    /// Echo: packets to send back (due time).
    echo: std::collections::VecDeque<(u64, u64)>,
}

#[derive(Clone, Debug)]
pub struct Item {
    pub kind: ItemKind,
    pub from: usize,
    pub to: usize,
    pub bytes: u32,
    pub chunks: u16,
    pub state: &'static str,
    pub why: String,
    pub created: u64,
    pub delivered: Option<u64>,
    pub route: PathKind,
    pub mime: String,
    pub name: String,
    pub file_id: u64,
    /// The file's bytes when a phone sent it (handed to the receiving phone).
    data: Option<Vec<u8>>,
    // Sender.
    acked: Vec<bool>,
    done: u16,
    in_flight: Vec<(u16, u64)>,
    offer_acked: bool,
    offer_sent: u64,
    pull_mode: bool,
    last_req: u64,
    srtt: Option<u64>,
    started: u64,
    box_host: Option<usize>,
    boxed_at: Option<u64>,
    from_box: bool,
    // Receiver.
    got: Vec<bool>,
    ngot: u16,
    offered: bool,
    pull: Option<Pull>,
    // Pages.
    path: String,
    /// A board note's text.
    note: String,
    parent: Option<usize>,
    child: Option<usize>,
    // Calls and tunnel.
    call: Option<CallStats>,
    tun: Option<Tun>,
    secs: u32,
    /// Call test mode (see `CallStats::mode`).
    mode: u8,
    kb: u32,
    pub phone_req: Option<u16>,
    /// A page answer that is the host's "Slow down" page.
    slow: bool,
}

impl Item {
    fn new(kind: ItemKind, from: usize, to: usize, now: u64) -> Item {
        Item {
            kind,
            from,
            to,
            bytes: 0,
            chunks: 0,
            state: "queued",
            why: String::new(),
            created: now,
            delivered: None,
            route: PathKind::Unknown,
            mime: String::new(),
            name: String::new(),
            file_id: 0,
            data: None,
            acked: Vec::new(),
            done: 0,
            in_flight: Vec::new(),
            offer_acked: false,
            offer_sent: 0,
            pull_mode: false,
            last_req: 0,
            srtt: None,
            started: now,
            box_host: None,
            boxed_at: None,
            from_box: false,
            got: Vec::new(),
            ngot: 0,
            offered: false,
            pull: None,
            path: String::new(),
            note: String::new(),
            parent: None,
            child: None,
            call: None,
            tun: None,
            secs: 0,
            mode: 0,
            kb: 0,
            phone_req: None,
            slow: false,
        }
    }
    fn finished(&self) -> bool {
        matches!(self.state, "delivered" | "refused" | "failed" | "cancelled" | "done")
    }
}

/// A file a phone is uploading (`FileBegin`/`FileData`).
#[derive(Clone, Debug)]
struct Upload {
    dest: usize,
    size: u32,
    mime: String,
    name: String,
    data: Vec<u8>,
}

#[derive(Default)]
pub struct Xfers {
    pub items: Vec<Item>,
    /// (node, router msg id) -> (item, op, chunk index).
    acks: HashMap<(usize, u64), (usize, u8, u16)>,
    /// Page burst: (host, reader) -> times served.
    served: HashMap<(usize, usize), Vec<u64>>,
    /// Notice boards: (host, page file) -> kept note lines, newest last.
    pub board_notes: HashMap<(usize, String), Vec<String>>,
    /// Last note per (host, reader), unix seconds.
    noted: HashMap<(usize, usize), u64>,
    uploads: HashMap<(usize, u16), Upload>,
    counter: u32,
}

fn header(op: u8, item: usize) -> Vec<u8> {
    let mut v = MAGIC.to_vec();
    v.push(op);
    v.extend_from_slice(&(item as u32).to_be_bytes());
    v
}

fn pad(mut v: Vec<u8>, len: usize) -> Vec<u8> {
    let len = len.min(maarse_core::router::MAX_DIRECT_PAYLOAD);
    if v.len() < len {
        v.resize(len, 0);
    }
    v
}

fn be16(b: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([b[at], b[at + 1]])
}
fn be32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// The size of a sealed file offer on the air (the real offer encoding plus
/// the sealing overhead of a direct message).
fn offer_len(size: u32, chunks: u16, mime: &str, name: &str) -> usize {
    let small = |s: &str| Small::from_slice(&s.as_bytes()[..s.len().min(64)]).unwrap_or_default();
    let offer = FileOffer { file_id: 0, size, chunks, chunk_size: CHUNK_DATA as u8, key: [0; 32], hash: [0; 16], mime: small(mime), name: small(name) };
    offer.encode().len() + DM_OVERHEAD
}

/// A real IPv6 packet (TCP with timestamps, or ICMPv6 echo) of `data` bytes
/// from `src` to `dst`.
fn ipv6_packet(src: &[u8; 16], dst: &[u8; 16], icmp: bool, data: usize) -> Vec<u8> {
    let l4 = if icmp { 8 } else { 32 };
    let plen = l4 + data;
    let mut p = vec![0u8; 40 + plen];
    p[0] = 0x60;
    p[4..6].copy_from_slice(&(plen as u16).to_be_bytes());
    p[6] = if icmp { 58 } else { 6 };
    p[7] = 64;
    p[8..24].copy_from_slice(src);
    p[24..40].copy_from_slice(dst);
    if icmp {
        p[40] = 128;
    } else {
        p[40..42].copy_from_slice(&50_000u16.to_be_bytes());
        p[42..44].copy_from_slice(&22u16.to_be_bytes());
        p[52] = 0x80; // data offset 8 words
        p[53] = 0x18; // PSH ACK
    }
    for (k, b) in p[40 + l4..].iter_mut().enumerate() {
        *b = (k as u8).wrapping_mul(37).wrapping_add(11);
    }
    p
}

fn median_p95(v: &[u64]) -> (Option<u64>, Option<u64>) {
    if v.is_empty() {
        return (None, None);
    }
    let mut s = v.to_vec();
    s.sort_unstable();
    (Some(s[s.len() / 2]), Some(s[((s.len() * 95).div_ceil(100)).saturating_sub(1).min(s.len() - 1)]))
}

impl Engine {
    fn rules(&self) -> Rules {
        self.sc.rules
    }

    fn xf_sched(&mut self, at: u64, item: usize, what: u8) {
        self.schedule(at.max(self.now), Ev::Xfer { item, what });
    }

    /// Node `i` is an app user: no radio, on a host over TCP.
    fn is_app(&self, i: usize) -> bool {
        !self.nodes[i].radio && self.nodes[i].kind != Kind::Host
    }

    /// The node counts as "at a host" for the host's rules (a host, or an
    /// app at a host).
    pub(crate) fn at_host(&self, i: usize) -> bool {
        self.nodes[i].kind == Kind::Host || self.is_app(i)
    }

    /// The host whose file mailbox keeps `i`'s files: itself (a host) or the
    /// host an app user is connected to. Firmware nodes have none.
    fn box_host(&self, i: usize) -> Option<usize> {
        if self.nodes[i].kind == Kind::Host {
            return Some(i);
        }
        if !self.is_app(i) {
            return None;
        }
        self.nodes[i].links.iter().filter(|&&k| !self.links[k].lora && self.link_ok(k)).map(|&k| if self.links[k].a == i { self.links[k].b } else { self.links[k].a }).find(|&o| self.nodes[o].kind == Kind::Host)
    }

    /// Is there a way from `a` to `b` over TCP links only, through hosts
    /// (two hosts both on the backbone count as connected)?
    fn tcp_way(&self, a: usize, b: usize) -> bool {
        let n = self.nodes.len();
        let mut seen = vec![false; n];
        let mut stack = vec![a];
        seen[a] = true;
        let mut internet = false;
        while let Some(i) = stack.pop() {
            if i == b {
                return true;
            }
            if i != a && self.nodes[i].kind != Kind::Host {
                continue; // only hosts pass packets on over TCP
            }
            let mut next: Vec<usize> = self.nodes[i]
                .links
                .iter()
                .filter(|&&k| !self.links[k].lora && self.link_ok(k))
                .map(|&k| if self.links[k].a == i { self.links[k].b } else { self.links[k].a })
                .collect();
            for &(x, y) in &self.dyn_links {
                if (x == i || y == i) && self.has_internet(x) && self.has_internet(y) {
                    next.push(if x == i { y } else { x });
                }
            }
            if !internet && self.nodes[i].kind == Kind::Host && self.on_backbone(i) {
                internet = true;
                next.extend((0..n).filter(|&j| self.nodes[j].kind == Kind::Host && self.nodes[j].alive() && self.on_backbone(j)));
            }
            for j in next {
                if !seen[j] && self.nodes[j].alive() {
                    seen[j] = true;
                    stack.push(j);
                }
            }
        }
        false
    }

    /// The route kind from `a` to `b` with the host's real `classify`.
    pub fn xf_path_kind(&self, a: usize, b: usize) -> PathKind {
        if a == b {
            return PathKind::Tcp;
        }
        if !self.nodes[b].alive() {
            return PathKind::Unknown;
        }
        let none = std::collections::HashSet::new();
        if self.tcp_way(a, b) {
            return filebox::classify(&filebox::PathFacts { heard_tcp: Some(true), route: None, dest_is_host: self.at_host(b), host_ids: &none });
        }
        // Heard over the radio: a route the router knows, a neighbour in
        // range, or the same radio island (repeaters joining the two).
        let dest = self.nodes[b].router.addr;
        let heard = self.nodes[a].radio
            && self.nodes[b].radio
            && (self.nodes[a].router.routes().get(&dest).is_some()
                || self.nodes[a].nbrs.iter().any(|&(j, rx)| j as usize == b && self.audible_rx(a, rx))
                || {
                    let c = self.components_with(false);
                    c[a] >= 0 && c[a] == c[b]
                });
        filebox::classify(&filebox::PathFacts { heard_tcp: heard.then_some(false), route: None, dest_is_host: false, host_ids: &none })
    }

    /// The file limit from `a` to `b`, as the host (or the firmware) gives it.
    pub fn xf_limit(&self, a: usize, b: usize) -> filebox::Limit {
        let kind = self.xf_path_kind(a, b);
        let radio = RadioConfig::EU_DEFAULT;
        if self.box_host(a).is_none() {
            // Firmware: the file buffer is 8 KB, whatever the route.
            let max = filebox::LORA_MAX_FILE;
            return filebox::Limit { max_bytes: max, via_lora: true, mailbox: false, est_airtime_s: filebox::est_airtime_s(max, &radio) };
        }
        let r = self.rules();
        filebox::limit(kind, r.max_file, r.filebox_on, &radio)
    }

    fn xf_trace(&mut self, k: usize, node: usize, extra: &str) {
        let it = &self.xf.items[k];
        let s = format!(
            "{{\"t\":{},\"ev\":\"item\",\"item\":{k},\"n\":{node},\"kind\":\"{}\",\"state\":\"{}\",\"done\":{},\"total\":{}{extra}}}",
            self.now,
            it.kind.name(),
            it.state,
            it.done.max(it.ngot),
            it.chunks
        );
        self.tr(s);
    }

    fn xf_set(&mut self, k: usize, state: &'static str, why: &str) {
        let node = if matches!(state, "delivered") { self.xf.items[k].to } else { self.xf.items[k].from };
        self.xf.items[k].state = state;
        self.xf.items[k].why = why.to_string();
        if state == "delivered" && self.xf.items[k].delivered.is_none() {
            self.xf.items[k].delivered = Some(self.now);
        }
        let w = serde_json::to_string(why).unwrap_or_default();
        self.xf_trace(k, node, &format!(",\"why\":{w}"));
        // A page answer moves its request along.
        if let Some(p) = self.xf.items[k].parent {
            let slow = self.xf.items[k].slow;
            let st = if state == "sending" || state == "queued" { "answering" } else if slow && state == "delivered" { "refused" } else { state };
            let why = if slow { "slow down" } else { why };
            self.xf.items[p].state = st;
            self.xf.items[p].why = why.to_string();
            if state == "delivered" {
                self.xf.items[p].delivered = Some(self.now);
            }
            self.xf_trace(p, node, &format!(",\"why\":{w}"));
        }
    }

    /// Sends a transfer packet from `node` to `to` through the real router.
    fn xf_send(&mut self, node: usize, to: usize, ptype: PacketType, payload: &[u8]) -> Option<u64> {
        if !self.nodes[node].alive() {
            return None;
        }
        let dest = self.nodes[to].router.addr;
        let mut token = [0u8; 16];
        self.rng.fill(&mut token[..]);
        let before = self.nodes[node].router.stats;
        let r = if ptype == PacketType::Fragment {
            self.nodes[node].router.send_fragment(dest, payload, self.now)
        } else {
            self.nodes[node].router.send_typed(ptype, dest, payload, token, self.now)
        };
        match r {
            Ok((id, actions)) => {
                self.after_router(node, before, None);
                self.apply(node, actions.into_iter().collect(), None);
                Some(id)
            }
            Err(_) => None,
        }
    }

    fn xf_nonce(&mut self) -> [u8; 4] {
        self.xf.counter += 1;
        self.xf.counter.to_be_bytes()
    }

    // ---------- starting ----------

    fn xf_push(&mut self, it: Item) -> usize {
        self.xf.items.push(it);
        self.xf.items.len() - 1
    }

    /// Starts a file (also voice and pictures) from `from` to `to`.
    /// Returns the item, or the host's refusal (code, text).
    pub fn xf_send_file(&mut self, from: usize, to: usize, kind: ItemKind, bytes: u32, mime: &str, name: &str, data: Option<Vec<u8>>) -> Result<usize, (u8, String)> {
        if !self.nodes[from].alive() {
            return Err((error_code::QUEUE_FULL, "Simulator: this node is down; nothing was sent".into()));
        }
        if self.is_app(from) && self.box_host(from).is_none() {
            return Err((error_code::UNKNOWN_DEST, "Simulator: this app is not connected to a host".into()));
        }
        let l = self.xf_limit(from, to);
        if bytes > l.max_bytes {
            let m = if self.box_host(from).is_none() {
                "The file is too large (at most 8 KB over LoRa)".to_string()
            } else if l.via_lora && !l.mailbox {
                format!("The file is too large: the recipient is only reached over LoRa, where the limit is {} KB", l.max_bytes / 1024)
            } else {
                format!("The file is too large (at most {} KB)", l.max_bytes / 1024)
            };
            return Err((error_code::FILE_TOO_LARGE, m));
        }
        let mut it = Item::new(kind, from, to, self.now);
        it.bytes = bytes;
        it.chunks = maarse_crypto::files::chunks_for(bytes as usize);
        it.mime = mime.to_string();
        it.name = name.to_string();
        it.data = data;
        it.file_id = self.rng.r#gen();
        it.box_host = self.box_host(from);
        let k = self.xf_push(it);
        self.xf_trace(k, from, &format!(",\"from\":{from},\"to\":{to},\"bytes\":{bytes},\"max\":{}", l.max_bytes));
        self.xf_start(k);
        Ok(k)
    }

    /// (Re)starts sending item `k`: offer, or straight to the file mailbox.
    fn xf_start(&mut self, k: usize) {
        let (from, to, bytes) = (self.xf.items[k].from, self.xf.items[k].to, self.xf.items[k].bytes);
        let kind = self.xf_path_kind(from, to);
        let page = self.xf.items[k].kind == ItemKind::PageReply;
        {
            let it = &mut self.xf.items[k];
            it.route = kind;
            let n = it.chunks as usize;
            it.acked = vec![false; n];
            it.got = vec![false; n];
            it.done = 0;
            it.ngot = 0;
            it.in_flight.clear();
            it.offer_acked = false;
            it.offered = false;
            it.pull = None;
            it.pull_mode = kind != PathKind::Tcp;
            it.started = self.now;
        }
        let boxed = self.xf.items[k].box_host.is_some() && self.rules().filebox_on && !page;
        // Unknown route, or over 8 KB without a TCP route: the file waits in
        // the mailbox (maarse-host holds such chunks back and deposits the file).
        if boxed && (kind == PathKind::Unknown || (bytes > filebox::LORA_MAX_FILE && kind != PathKind::Tcp)) {
            let why = if kind == PathKind::Unknown { "the recipient cannot be reached now" } else { "no route known to be internet all the way (over 8 KB is not sent over LoRa)" };
            return self.xf_to_box(k, why);
        }
        self.xf_set(k, "queued", &format!("offer sent ({} route)", kind_name(kind)));
        self.xf_offer(k);
        self.xf_sched(self.now + 2_000, k, W_CHECK);
    }

    fn xf_offer(&mut self, k: usize) {
        let it = &self.xf.items[k];
        let (from, to) = (it.from, it.to);
        let len = offer_len(it.bytes, it.chunks, &it.mime, &it.name);
        let mut p = header(OP_OFFER, k);
        let nonce = self.xf_nonce();
        p.extend_from_slice(&nonce);
        let p = pad(p, len);
        self.xf.items[k].offer_sent = self.now;
        if let Some(id) = self.xf_send(from, to, PacketType::Data, &p) {
            self.xf.acks.insert((from, id), (k, OP_OFFER, 0));
        }
    }

    fn xf_to_box(&mut self, k: usize, why: &str) {
        if self.xf.items[k].boxed_at.is_none() {
            self.xf.items[k].boxed_at = Some(self.now);
        }
        self.xf.items[k].in_flight.clear();
        self.xf.items[k].pull = None;
        let host = self.xf.items[k].box_host.unwrap_or(self.xf.items[k].from);
        self.xf_set(k, "mailbox", why);
        self.xf_trace(k, host, ",\"mailbox\":true");
        let (from, file_id) = (self.xf.items[k].from, self.xf.items[k].file_id);
        if self.xf.items[k].phone_req.is_some() {
            self.phone_emit(from, &Event::FileDone { file_id, ok: true, outgoing: true, text: text(b"put in mailbox") });
        }
        self.xf_sched(self.now + BOX_POLL_MS, k, W_BOX);
    }

    // ---------- timers ----------

    pub(crate) fn xfer_timer(&mut self, k: usize, what: u8) {
        if k >= self.xf.items.len() {
            return;
        }
        match what {
            W_CHECK => self.xf_check(k),
            W_PULL => self.xf_pull_check(k),
            W_BOX => self.xf_box_poll(k),
            W_AUDIO_A | W_AUDIO_B => self.xf_audio(k, (what == W_AUDIO_B) as u8),
            W_CALL_END => self.xf_call_end(k),
            W_ECHO => self.xf_echo(k),
            W_SPEED => self.xf_speed(k),
            W_TUN => self.xf_tun_tick(k),
            W_TUN_NOKEY => {
                if self.xf.items[k].kind == ItemKind::Tunnel && self.xf.items[k].state == "queued" {
                    self.xf_set(k, "failed", "the session key was not acknowledged");
                }
            }
            W_PAGE_WAIT => {
                if self.xf.items[k].kind == ItemKind::Page && matches!(self.xf.items[k].state, "queued" | "asked") {
                    self.xf_set(k, "failed", "no page came (the host did not answer)");
                }
            }
            _ => {}
        }
    }

    /// Sender housekeeping: resend overdue chunks, offer again, time out.
    fn xf_check(&mut self, k: usize) {
        if self.xf.items[k].finished() || self.xf.items[k].state == "mailbox" {
            return;
        }
        let it = &self.xf.items[k];
        let age = self.now.saturating_sub(it.started);
        let boxed = it.box_host.is_some() && self.rules().filebox_on && it.kind != ItemKind::PageReply;
        // Not taken within the deposit time, or no completion within the
        // file timeout: into the mailbox (if any), else given up.
        let late = !it.offer_acked && age > self.rules().deposit_after_ms;
        if boxed && !it.from_box && (late || age > FILE_TIMEOUT_MS) {
            return self.xf_to_box(k, "the recipient did not take the file in time");
        }
        if age > FILE_TIMEOUT_MS {
            if it.from_box {
                return self.xf_to_box(k, "the recipient did not answer; the file stays in the mailbox");
            }
            let why = if it.kind == ItemKind::PageReply { "page not taken" } else { "the recipient did not answer" };
            let (from, file_id, phone) = (it.from, it.file_id, it.phone_req.is_some());
            self.xf_set(k, "failed", why);
            if phone {
                self.phone_emit(from, &Event::FileDone { file_id, ok: false, outgoing: true, text: text(why.as_bytes()) });
            }
            return;
        }
        let it = &self.xf.items[k];
        if !it.offer_acked && self.now.saturating_sub(it.offer_sent) > 30_000 {
            self.xf_offer(k);
        } else if it.pull_mode && it.offer_acked && self.now.saturating_sub(it.last_req.max(it.offer_sent)) > PULL_STALL_MS {
            // A pulling recipient went quiet: offer again (it resumes).
            self.xf_offer(k);
        } else if !it.pull_mode {
            let timeout = if it.route == PathKind::Tcp { CHUNK_TIMEOUT_MS } else { lora_chunk_timeout(it.srtt) };
            let now = self.now;
            self.xf.items[k].in_flight.retain(|&(_, t)| now.saturating_sub(t) < timeout);
            self.xf_pump(k);
        }
        self.xf_sched(self.now + 2_000, k, W_CHECK);
    }

    fn xf_box_poll(&mut self, k: usize) {
        if self.xf.items[k].state != "mailbox" {
            return;
        }
        let it = &self.xf.items[k];
        if self.now.saturating_sub(it.boxed_at.unwrap_or(self.now)) >= filebox::FILEBOX_TTL_MS {
            return self.xf_set(k, "failed", "expired in the file mailbox");
        }
        let kind = self.xf_path_kind(it.from, it.to);
        let ok = match kind {
            PathKind::Unknown => false,
            PathKind::Lora => it.bytes <= filebox::LORA_MAX_FILE,
            PathKind::Tcp => true,
        };
        if ok && self.nodes[it.from].alive() {
            self.xf.items[k].from_box = true;
            self.xf_trace(k, self.xf.items[k].from, ",\"from_box\":true");
            self.xf_start(k);
        } else {
            self.xf_sched(self.now + BOX_POLL_MS, k, W_BOX);
        }
    }

    // ---------- push (internet) ----------

    fn xf_pump(&mut self, k: usize) {
        loop {
            let it = &self.xf.items[k];
            if it.finished() || !it.offer_acked || it.pull_mode {
                return;
            }
            let window = if it.route == PathKind::Tcp { FILE_WINDOW } else { FILE_WINDOW_LORA };
            if it.in_flight.len() >= window {
                return;
            }
            // Over 8 KB only along a route known to be TCP all the way.
            if it.bytes > filebox::LORA_MAX_FILE && self.xf_path_kind(it.from, it.to) != PathKind::Tcp {
                return;
            }
            let Some(idx) = (0..it.chunks).find(|i| !it.acked[*i as usize] && !it.in_flight.iter().any(|(j, _)| j == i)) else { return };
            let (from, to) = (it.from, it.to);
            let p = self.xf_chunk_payload(k, idx);
            match self.xf_send(from, to, PacketType::Fragment, &p) {
                Some(id) => {
                    self.xf.acks.insert((from, id), (k, OP_CHUNK, idx));
                    self.xf.items[k].in_flight.push((idx, self.now));
                }
                None => return, // no route now; retried by the check
            }
        }
    }

    fn xf_chunk_payload(&self, k: usize, idx: u16) -> Vec<u8> {
        let it = &self.xf.items[k];
        let start = idx as usize * CHUNK_DATA;
        let len = (it.bytes as usize).saturating_sub(start).min(CHUNK_DATA);
        let mut p = header(OP_CHUNK, k);
        p.extend_from_slice(&idx.to_be_bytes());
        pad(p, CHUNK_OVERHEAD + len)
    }

    fn xf_chunk_acked(&mut self, k: usize, idx: u16) {
        let now = self.now;
        let it = &mut self.xf.items[k];
        if let Some(pos) = it.in_flight.iter().position(|(j, _)| *j == idx) {
            let sent = it.in_flight.remove(pos).1;
            let s = now.saturating_sub(sent);
            it.srtt = Some(it.srtt.map_or(s, |r| (r * 7 + s) / 8));
        }
        if (idx as usize) < it.acked.len() && !it.acked[idx as usize] {
            it.acked[idx as usize] = true;
            it.done += 1;
        }
        self.xf_progress_out(k);
        if self.xf.items[k].done >= self.xf.items[k].chunks {
            self.xf_sender_done(k);
        } else {
            self.xf_pump(k);
        }
    }

    fn xf_progress_out(&mut self, k: usize) {
        let it = &self.xf.items[k];
        let (from, file_id, done, total) = (it.from, it.file_id, it.done, it.chunks);
        if it.state == "queued" {
            self.xf_set(k, "sending", "");
        }
        self.phone_emit(from, &Event::FileProgress { file_id, done, total, outgoing: true });
    }

    /// The sender knows every chunk arrived.
    fn xf_sender_done(&mut self, k: usize) {
        let it = &self.xf.items[k];
        let (from, file_id, boxed) = (it.from, it.file_id, it.from_box);
        if it.state != "delivered" {
            self.xf_set(k, "delivered", if boxed { "delivered from mailbox" } else { "delivered" });
        }
        self.xf.items[k].in_flight.clear();
        self.xf.items[k].acked.iter_mut().for_each(|a| *a = true);
        self.xf.items[k].done = self.xf.items[k].chunks;
        let t: &[u8] = if boxed { b"delivered from mailbox" } else { b"delivered" };
        self.phone_emit(from, &Event::FileDone { file_id, ok: true, outgoing: true, text: text(t) });
    }

    // ---------- receiving ----------

    fn xf_on_offer(&mut self, k: usize, node: usize, hops: u8) {
        let it = &self.xf.items[k];
        if node != it.to || it.finished() && it.ngot >= it.chunks {
            // Already received: tell a pulling sender again.
            if node == it.to && it.pull_mode {
                self.xf_send_request(k, it.chunks, 0, 0);
            }
            return;
        }
        if matches!(it.state, "cancelled" | "failed") {
            return;
        }
        if !it.offered {
            self.xf.items[k].offered = true;
            let it = &self.xf.items[k];
            let ev = Event::FileIncoming { file_id: it.file_id, from: self.nodes[it.from].router.addr, size: it.bytes, mime: text(it.mime.as_bytes()), name: text(it.name.as_bytes()) };
            self.phone_emit(node, &ev);
        }
        if self.xf.items[k].pull_mode {
            self.xf_pull_start(k, hops > 0);
        }
    }

    fn xf_pull_start(&mut self, k: usize, relayed: bool) {
        let chunk_ms = RadioConfig::EU_DEFAULT.airtime().airtime_us(maarse_core::MAX_PACKET) as u64 / 1000 + 150;
        let it = &mut self.xf.items[k];
        if it.pull.is_none() {
            it.pull = Some(Pull { window: if relayed { PULL_WINDOW_MAX_RELAYED } else { PULL_WINDOW_START }, chunk_ms, relayed, ..Default::default() });
        }
        self.xf_pull_request(k);
    }

    fn xf_pull_request(&mut self, k: usize) {
        let now = self.now;
        let it = &mut self.xf.items[k];
        let total = it.chunks;
        let Some(p) = it.pull.as_mut() else { return };
        let base = (0..total).find(|i| !it.got[*i as usize]).unwrap_or(total);
        let mut missing = 0u32;
        let mut asked = Vec::new();
        for i in 0..content::FILE_REQUEST_SPAN {
            let idx = base + i;
            if idx >= total {
                break;
            }
            if !it.got[idx as usize] {
                missing |= 1 << i;
                if asked.len() < p.window as usize {
                    asked.push(idx);
                }
            }
        }
        let wait = p.chunk_ms * 2 + 1500 + p.retries as u64 * 500;
        p.deadline = now + wait;
        p.asked = asked;
        p.last_rx = None;
        let count = p.window;
        self.xf_sched(now + wait + 20, k, W_PULL);
        self.xf_send_request(k, base, count, missing);
    }

    /// A pull request sealed with the file key, as a datagram (~42 bytes).
    fn xf_send_request(&mut self, k: usize, base: u16, count: u8, missing: u32) {
        let (from, to) = (self.xf.items[k].from, self.xf.items[k].to);
        let seq = self.xf.items[k].pull.as_ref().map_or(0, |p| p.seq);
        if let Some(p) = self.xf.items[k].pull.as_mut() {
            p.seq = p.seq.wrapping_add(1);
        }
        let mut b = header(OP_REQUEST, k);
        b.extend_from_slice(&base.to_be_bytes());
        b.push(count);
        b.extend_from_slice(&missing.to_be_bytes());
        b.extend_from_slice(&seq.to_be_bytes());
        let b = pad(b, 16 + CHUNK_OVERHEAD);
        self.xf_send(to, from, PacketType::Datagram, &b);
    }

    fn xf_pull_check(&mut self, k: usize) {
        let now = self.now;
        let it = &mut self.xf.items[k];
        if it.finished() && it.ngot >= it.chunks {
            return;
        }
        let Some(p) = it.pull.as_mut() else { return };
        if now < p.deadline {
            return;
        }
        p.retries += 1;
        p.troubled = true;
        p.window = p.window.saturating_sub(1).max(2);
        if p.retries > PULL_MAX_RETRIES {
            it.pull = None; // the sender offers again; we resume then
            let to = it.to;
            self.xf_trace(k, to, ",\"why\":\"no answer to our requests, waiting for the sender\"");
            return;
        }
        self.xf_pull_request(k);
    }

    /// The sender gets a pull request.
    fn xf_on_request(&mut self, k: usize, node: usize, b: &[u8]) {
        if b.len() < 16 || node != self.xf.items[k].from {
            return;
        }
        let (base, count, missing) = (be16(b, 7), b[9], be32(b, 10));
        let it = &mut self.xf.items[k];
        if matches!(it.state, "cancelled" | "failed" | "mailbox") {
            return;
        }
        it.last_req = self.now;
        it.offer_acked = true;
        let total = it.chunks;
        for i in 0..total {
            let here = i < base || (i - base < content::FILE_REQUEST_SPAN && missing & (1 << (i - base)) == 0);
            if here && !it.acked[i as usize] {
                it.acked[i as usize] = true;
                it.done += 1;
            }
        }
        if base >= total || self.xf.items[k].done >= total {
            self.xf_progress_out(k);
            return self.xf_sender_done(k);
        }
        self.xf_progress_out(k);
        let want: Vec<u16> = (0..content::FILE_REQUEST_SPAN).filter(|i| missing & (1 << i) != 0).map(|i| base + i).filter(|i| *i < total).take(count.max(1) as usize).collect();
        let (from, to) = (self.xf.items[k].from, self.xf.items[k].to);
        for idx in want {
            let p = self.xf_chunk_payload(k, idx);
            self.xf_send(from, to, PacketType::Datagram, &p);
        }
    }

    fn xf_on_chunk(&mut self, k: usize, node: usize, b: &[u8]) {
        if b.len() < 9 || node != self.xf.items[k].to {
            return;
        }
        let idx = be16(b, 7);
        let now = self.now;
        let it = &mut self.xf.items[k];
        if matches!(it.state, "cancelled" | "failed") || idx as usize >= it.got.len() || it.got[idx as usize] {
            return;
        }
        it.got[idx as usize] = true;
        it.ngot += 1;
        let (got, total, file_id) = (it.ngot, it.chunks, it.file_id);
        if it.state == "queued" {
            it.state = "sending";
        }
        self.phone_emit(node, &Event::FileProgress { file_id, done: got, total, outgoing: false });
        if got >= total {
            return self.xf_receiver_done(k);
        }
        // Pull: batch bookkeeping as maarse-host's pull_on_chunk.
        let it = &mut self.xf.items[k];
        let Some(p) = it.pull.as_mut() else { return };
        if let Some(prev) = p.last_rx {
            let gap = now.saturating_sub(prev).min(10_000);
            p.chunk_ms = (p.chunk_ms * 3 + gap) / 4;
        }
        p.last_rx = Some(now);
        p.retries = 0;
        let last_of_batch = p.asked.last().is_some_and(|l| idx >= *l);
        p.asked.retain(|i| *i != idx);
        if !p.asked.is_empty() && last_of_batch {
            p.troubled = true;
            return self.xf_pull_request(k);
        }
        if p.asked.is_empty() {
            let max = if p.relayed { PULL_WINDOW_MAX_RELAYED } else { PULL_WINDOW_MAX };
            if !p.troubled {
                p.window = (p.window + 1).min(max);
            }
            p.troubled = false;
            self.xf_pull_request(k);
        } else {
            p.deadline = now + p.chunk_ms * 2 + 500;
            let at = p.deadline + 20;
            self.xf_sched(at, k, W_PULL);
        }
    }

    fn xf_receiver_done(&mut self, k: usize) {
        let it = &self.xf.items[k];
        let (to, file_id, pull) = (it.to, it.file_id, it.pull.is_some());
        if pull {
            // Tell the sender everything is here.
            self.xf_send_request(k, self.xf.items[k].chunks, 0, 0);
            self.xf.items[k].pull = None;
        }
        self.xf.items[k].delivered.get_or_insert(self.now);
        if self.xf.items[k].state != "delivered" {
            self.xf_set(k, "delivered", "every chunk arrived");
        }
        if self.phones.attached.contains_key(&to) {
            let it = &self.xf.items[k];
            let data = it.data.clone().unwrap_or_else(|| vec![0u8; it.bytes as usize]);
            let name = it.name.clone();
            for (i, part) in data.chunks(MAX_TEXT).enumerate() {
                self.phone_emit(to, &Event::FileChunk { file_id, offset: (i * MAX_TEXT) as u32, data: text(part) });
            }
            self.phone_emit(to, &Event::FileDone { file_id, ok: true, outgoing: false, text: text(name.as_bytes()) });
        }
    }

    // ---------- cancel ----------

    /// Stops item `k` from `by` (the sender or the recipient); the other
    /// side is told with a sealed message.
    pub fn xf_cancel(&mut self, k: usize, by: usize) -> bool {
        if k >= self.xf.items.len() || self.xf.items[k].finished() {
            return false;
        }
        let it = &self.xf.items[k];
        let (from, to, file_id) = (it.from, it.to, it.file_id);
        let by_recipient = by == to;
        let other = if by_recipient { from } else { to };
        let mut p = header(OP_CANCEL, k);
        p.push(by_recipient as u8);
        let nonce = self.xf_nonce();
        p.extend_from_slice(&nonce);
        let p = pad(p, 10 + DM_OVERHEAD);
        self.xf_send(by, other, PacketType::Data, &p);
        self.xf.items[k].in_flight.clear();
        self.xf.items[k].pull = None;
        self.xf_set(k, "cancelled", if by_recipient { "cancelled by the recipient" } else { "cancelled by the sender" });
        self.phone_emit(by, &Event::FileDone { file_id, ok: false, outgoing: !by_recipient, text: text(b"cancelled") });
        true
    }

    fn xf_on_cancel(&mut self, k: usize, node: usize, b: &[u8]) {
        let by_recipient = b.get(7) == Some(&1);
        let file_id = self.xf.items[k].file_id;
        let t: &[u8] = if by_recipient { b"cancelled by the recipient" } else { b"cancelled by the sender" };
        self.phone_emit(node, &Event::FileDone { file_id, ok: false, outgoing: by_recipient, text: text(t) });
        self.xf_trace(k, node, ",\"why\":\"the other side was told\"");
    }

    // ---------- Pages ----------

    /// `reader` asks `host` for a page; `bytes` overrides the answer's size.
    pub fn xf_page(&mut self, reader: usize, host: usize, path: &str, bytes: Option<u32>) -> Result<usize, (u8, String)> {
        let Some(plain) = content::page_request(path.as_bytes()) else {
            return Err((error_code::TOO_LARGE, "Invalid path".into()));
        };
        if !self.nodes[reader].alive() {
            return Err((error_code::QUEUE_FULL, "Simulator: this node is down; nothing was sent".into()));
        }
        let mut it = Item::new(ItemKind::Page, reader, host, self.now);
        it.path = String::from_utf8_lossy(&plain[1..]).into_owned();
        it.bytes = bytes.unwrap_or(0);
        it.route = self.xf_path_kind(reader, host);
        let k = self.xf_push(it);
        let mut p = header(OP_PAGE, k);
        let nonce = self.xf_nonce();
        p.extend_from_slice(&nonce);
        p.extend_from_slice(&(plain.len() as u16).to_be_bytes());
        p.extend_from_slice(&plain);
        let p = pad(p, plain.len() + DM_OVERHEAD);
        self.xf_trace(k, reader, &format!(",\"from\":{reader},\"to\":{host},\"path\":{}", serde_json::to_string(path).unwrap_or_default()));
        self.xf.items[k].state = "asked";
        self.xf_send(reader, host, PacketType::Data, &p);
        self.xf_sched(self.now + pages::ASKED_KEEP_MS, k, W_PAGE_WAIT);
        Ok(k)
    }

    /// The simulated page markup for `path` at `host`.
    /// The page file at `path` on a site: `path`, else `path/index`.
    fn site_page<'a>(site: &'a crate::scenario::Site, path: &str) -> Option<(String, &'a String)> {
        if let Some(m) = site.pages.get(path) {
            return Some((path.to_string(), m));
        }
        let idx = format!("{path}/index");
        site.pages.get(&idx).map(|m| (idx, m))
    }

    /// May `reader` open `path` on `host`'s site? As maarse-host's
    /// `access`: the page's own lock and every folder's `index` above it;
    /// None = open, Some(hidden) = refused.
    fn site_refuses(&self, host: usize, reader: usize, path: &str) -> Option<bool> {
        use maarse_hostrules::pagelock::{lock_folders, lock_of, parse_hex};
        let site = self.site(host)?;
        if reader == host {
            return None;
        }
        let now_s = 1_700_000_000 + self.now / 1000;
        let mut files: Vec<&String> = Self::site_page(site, path).map(|(_, m)| m).into_iter().collect();
        for f in lock_folders(path) {
            if let Some(m) = site.pages.get(&format!("{f}/index"))
                && !files.iter().any(|x| std::ptr::eq(*x, m))
            {
                files.push(m);
            }
        }
        let me = self.nodes[reader].router.addr;
        for m in files {
            let lock = lock_of(m);
            if !lock.applies(now_s) {
                continue;
            }
            let member = lock.allow.iter().any(|a| match a.strip_prefix("group:") {
                Some(g) => site.groups.get(g).is_some_and(|v| v.contains(&reader)),
                None => parse_hex(a) == Some(me),
            });
            if !member {
                return Some(lock.hidden);
            }
        }
        None
    }

    /// A host with a site serves its own page files (as maarse-host serves
    /// `pages/<path>.mu`), with locks and notice boards applied for the
    /// reader; an unknown path gets a "Not found" page.
    fn page_markup(&self, host: usize, path: &str, reader: usize) -> String {
        use maarse_hostrules::pagelock::{board_line, hex, note_line, outside_code, refused_page, strip_locks, BOARD_KEEP};
        let name = if self.nodes[host].label.is_empty() { format!("Host {host}") } else { self.nodes[host].label.clone() };
        let path = path.trim_matches('/');
        let path = if path.is_empty() { "index" } else { path };
        let Some(site) = self.site(host) else {
            return format!("@title {path}\n# {path}\n\nA page served by {name} in the Denmark simulator.\n\n[Front page](/)\n");
        };
        let Some((file, markup)) = Self::site_page(site, path) else {
            return format!("@title Not found\n# Not found\n\n{} has no page called \"{path}\".\n\n[Front page](/)\n", site.name);
        };
        if let Some(hidden) = self.site_refuses(host, reader, path) {
            return refused_page(path, hidden, &hex(&self.nodes[reader].router.addr));
        }
        let markup = strip_locks(markup);
        if !outside_code(&markup).any(|l| l.trim_start().starts_with("@board")) {
            return markup;
        }
        // The newest notes after each board line (maarse-host `with_notes`).
        let notes = self.xf.board_notes.get(&(host, file.clone())).cloned().unwrap_or_default();
        let mut out = String::with_capacity(markup.len() + notes.len() * 80);
        let mut fence = false;
        for line in markup.lines() {
            out.push_str(line);
            out.push('\n');
            let t = line.trim_start();
            if t.starts_with("```") {
                fence = !fence;
                continue;
            }
            if fence {
                continue;
            }
            if let Some(n) = board_line(t, BOARD_KEEP) {
                out.push_str(&format!("@boardpath {file}\n"));
                for note in notes.iter().rev().take(n) {
                    out.push_str(&note_line(note));
                }
            }
        }
        out
    }

    /// `reader` writes a note on `path` at `host` (maarse-host
    /// `on_board_post`): a board must be there, the lock must let the
    /// reader in, one note a minute, at most BOARD_KEEP kept.
    fn board_post(&mut self, host: usize, reader: usize, path: &str, text: &[u8]) -> Result<(), &'static str> {
        use maarse_hostrules::pagelock::{clean_note, has_board, hex, may_note, BOARD_EVERY_S, BOARD_KEEP};
        let path = path.trim_matches('/');
        let path = if path.is_empty() { "index" } else { path };
        let site = self.site(host).ok_or("no such page")?;
        let (file, markup) = Self::site_page(site, path).ok_or("no such page")?;
        if !has_board(markup) {
            return Err("this page has no notice board");
        }
        if self.site_refuses(host, reader, path).is_some() {
            return Err("locked");
        }
        let now_s = 1_700_000_000 + self.now / 1000;
        if !may_note(self.xf.noted.get(&(host, reader)).copied(), now_s, BOARD_EVERY_S) {
            return Err("one note a minute");
        }
        let text = clean_note(text).ok_or("empty note")?;
        let name = self.phones.attached.get(&reader).map(|p| p.name.clone()).unwrap_or_else(|| self.nodes[reader].label.clone()).replace(['\t', '\n'], " ");
        let line = format!("{now_s}\t{}\t{name}\t{text}", hex(&self.nodes[reader].router.addr));
        let notes = self.xf.board_notes.entry((host, file)).or_default();
        notes.push(line);
        let skip = notes.len().saturating_sub(BOARD_KEEP);
        notes.drain(..skip);
        self.xf.noted.insert((host, reader), now_s);
        Ok(())
    }

    /// `reader` sends a note for the board on `path` to `host`: a sealed
    /// `BOARD_POST` message, as the clients send it.
    pub fn xf_board(&mut self, reader: usize, host: usize, path: &str, note: &str) -> Result<usize, (u8, String)> {
        let Some(plain) = content::board_post(path.as_bytes(), note.as_bytes()) else {
            return Err((error_code::TOO_LARGE, "Invalid path or note too long (100 bytes)".into()));
        };
        if !self.nodes[reader].alive() {
            return Err((error_code::QUEUE_FULL, "Simulator: this node is down; nothing was sent".into()));
        }
        let mut it = Item::new(ItemKind::Board, reader, host, self.now);
        it.path = path.to_string();
        it.route = self.xf_path_kind(reader, host);
        let k = self.xf_push(it);
        self.xf.items[k].note = note.to_string();
        let mut p = header(OP_BOARD, k);
        let nonce = self.xf_nonce();
        p.extend_from_slice(&nonce);
        p.extend_from_slice(&(plain.len() as u16).to_be_bytes());
        p.extend_from_slice(&plain);
        let p = pad(p, plain.len() + DM_OVERHEAD);
        self.xf_trace(k, reader, &format!(",\"from\":{reader},\"to\":{host},\"path\":{}", serde_json::to_string(path).unwrap_or_default()));
        self.xf.items[k].state = "queued";
        self.xf_send(reader, host, PacketType::Data, &p);
        Ok(k)
    }

    fn xf_on_board(&mut self, k: usize, node: usize) {
        let (reader, host) = (self.xf.items[k].from, self.xf.items[k].to);
        if node != host || self.xf.items[k].state != "queued" {
            return;
        }
        let (path, note) = (self.xf.items[k].path.clone(), self.xf.items[k].note.clone());
        match self.board_post(host, reader, &path, note.as_bytes()) {
            Ok(()) => self.xf_set(k, "done", "note posted"),
            Err(e) => self.xf_set(k, "refused", e),
        }
    }

    fn xf_on_page_request(&mut self, k: usize, node: usize, b: &[u8]) {
        let reader = self.xf.items[k].from;
        if node != self.xf.items[k].to || self.nodes[node].kind != Kind::Host || self.xf.items[k].child.is_some() {
            return; // only hosts serve pages (and once per request)
        }
        let r = self.rules();
        let lim = self.xf_limit(node, reader);
        let burst = if lim.via_lora { r.pages_burst_lora } else { r.pages_burst_net };
        let now = self.now;
        let times = self.xf.served.entry((node, reader)).or_default();
        let verdict = pages::burst(times, now, burst);
        let path = self.xf.items[k].path.clone();
        let _ = b;
        let (data, mime, name) = match verdict {
            pages::Burst::Quiet => {
                return self.xf_set(k, "refused", "too many page requests; the host stays quiet");
            }
            pages::Burst::SlowDown => {
                let msg = format!("# Slow down\n\nToo many pages at once: at most {burst} in {} seconds. Wait a moment and try again.\n\n[Front page](/)\n", pages::WINDOW_MS / 1000);
                (miniz_oxide::deflate::compress_to_vec(msg.as_bytes(), 9), String::from_utf8_lossy(content::PAGE_MIME).into_owned(), path.clone())
            }
            pages::Burst::Serve => {
                let given = self.xf.items[k].bytes;
                if let Some((style, _)) = pages::picture_path(&path) {
                    use pages::PicStyle::*;
                    let size = match style {
                        Grey | Dots | Colour | ForceGrey | ForceDots | ForceColour => pages::GREY_TARGET,
                        Colour4 => pages::COLOUR4_MAX,
                        Colour8 => pages::LORA_PICTURE_MAX,
                        Full => if lim.via_lora { pages::LORA_PICTURE_MAX } else { 64 * 1024 },
                    };
                    let size = if given > 0 { given as usize } else { size };
                    let size = pages::page_max(lim.via_lora, size.min(lim.max_bytes as usize));
                    (vec![0u8; size], "image/webp".to_string(), path.clone())
                } else if given > 0 {
                    let size = pages::page_max(lim.via_lora, (given as usize).min(lim.max_bytes as usize));
                    (vec![0u8; size], String::from_utf8_lossy(content::PAGE_MIME).into_owned(), path.clone())
                } else {
                    let m = self.page_markup(node, &path, reader);
                    (miniz_oxide::deflate::compress_to_vec(m.as_bytes(), 9), String::from_utf8_lossy(content::PAGE_MIME).into_owned(), path.clone())
                }
            }
        };
        let bytes = data.len() as u32;
        let mut c = Item::new(ItemKind::PageReply, node, reader, self.now);
        c.bytes = bytes;
        c.chunks = maarse_crypto::files::chunks_for(bytes as usize);
        c.mime = mime;
        c.name = name;
        c.data = Some(data);
        c.file_id = self.rng.r#gen();
        c.parent = Some(k);
        c.slow = verdict == pages::Burst::SlowDown;
        c.path = path;
        let ck = self.xf_push(c);
        self.xf.items[k].child = Some(ck);
        self.xf.items[k].bytes = bytes;
        self.xf_trace(ck, node, &format!(",\"from\":{node},\"to\":{reader},\"bytes\":{bytes},\"page_of\":{k}"));
        self.xf_start(ck);
    }

    // ---------- calls ----------

    /// Starts a call of `secs` seconds; refused unless TCP all the way.
    pub fn xf_call(&mut self, from: usize, to: usize, secs: u32) -> usize {
        self.xf_call_mode(from, to, secs, calls::test::MUSIC)
    }

    /// A call; when `to` runs the call test service, `mode` picks music,
    /// echo or speed (the ring's mode byte), otherwise it is ignored.
    pub fn xf_call_mode(&mut self, from: usize, to: usize, secs: u32, mode: u8) -> usize {
        let mut it = Item::new(ItemKind::Call, from, to, self.now);
        it.secs = secs.max(1);
        it.mode = if self.sc.nodes.get(to).is_some_and(|n| n.calltest) { mode.clamp(calls::test::MUSIC, calls::test::SPEED) } else { 0 };
        it.route = self.xf_path_kind(from, to);
        let route = it.route;
        let k = self.xf_push(it);
        self.xf_trace(k, from, &format!(",\"from\":{from},\"to\":{to},\"route\":\"{}\"", kind_name(route)));
        if let Some((_, why)) = calls::call_refusal(route) {
            self.xf_set(k, "refused", why);
            return k;
        }
        self.xf_set(k, "ringing", "");
        self.xf_ring(k, from, to, 0);
        k
    }

    fn xf_ring(&mut self, k: usize, a: usize, b: usize, kind: u8) {
        let mut p = header(OP_RING, k);
        p.push(kind);
        let nonce = self.xf_nonce();
        p.extend_from_slice(&nonce);
        let p = pad(p, 2 + 2 + DM_OVERHEAD);
        self.xf_send(a, b, PacketType::Data, &p);
    }

    fn xf_on_ring(&mut self, k: usize, node: usize, b: &[u8]) {
        let kind = b.get(7).copied().unwrap_or(0);
        let (from, to) = (self.xf.items[k].from, self.xf.items[k].to);
        if kind == 0 && node == to && self.xf.items[k].state == "ringing" {
            // The callee answers at once.
            self.xf_ring(k, to, from, 1);
        } else if kind == 1 && node == from && self.xf.items[k].state == "ringing" {
            use calls::test as ct;
            let mode = self.xf.items[k].mode;
            let secs = self.xf.items[k].secs as u64;
            // The speed test hangs up by itself after 5 s; any call ends after 10 min.
            let len = if mode == ct::SPEED { ct::SPEED_FOR_MS } else { (secs * 1000).min(ct::MAX_CALL_MS) };
            self.xf.items[k].call = Some(CallStats { started: self.now, until: self.now + len, mode, ..Default::default() });
            let what = match mode {
                ct::MUSIC => "answered by the call test service: music",
                ct::ECHO => "answered by the call test service: echo",
                ct::SPEED => "answered by the call test service: speed test",
                _ => "answered",
            };
            self.xf_set(k, "active", what);
            match mode {
                // Music: the service talks, the caller listens.
                ct::MUSIC => self.xf_sched(self.now + 7, k, W_AUDIO_B),
                // Echo: the caller talks; the service sends it back (xf_on_audio).
                ct::ECHO => self.xf_sched(self.now, k, W_AUDIO_A),
                ct::SPEED => self.xf_sched(self.now + 7, k, W_SPEED),
                _ => {
                    self.xf_sched(self.now, k, W_AUDIO_A);
                    self.xf_sched(self.now + 7, k, W_AUDIO_B);
                }
            }
            self.xf_sched(self.now + len + 3_000, k, W_CALL_END);
        }
    }

    fn xf_audio(&mut self, k: usize, dir: u8) {
        if self.xf.items[k].state != "active" {
            return;
        }
        let Some(c) = self.xf.items[k].call.as_mut() else { return };
        if self.now >= c.until {
            return;
        }
        let _ = c;
        self.xf_audio_packet(k, dir, self.now);
        self.xf_sched(self.now + CALL_PACKET_MS, k, if dir == 0 { W_AUDIO_A } else { W_AUDIO_B });
    }

    /// Sends one call audio packet in direction `dir` (0 caller to callee),
    /// stamped with `stamp` (when the audio was spoken).
    fn xf_audio_packet(&mut self, k: usize, dir: u8, stamp: u64) {
        let Some(c) = self.xf.items[k].call.as_mut() else { return };
        let seq = c.seq[dir as usize];
        c.seq[dir as usize] += 1;
        c.sent[dir as usize] += 1;
        let (a, b) = if dir == 0 { (self.xf.items[k].from, self.xf.items[k].to) } else { (self.xf.items[k].to, self.xf.items[k].from) };
        let mut p = header(OP_AUDIO, k);
        p.push(dir);
        p.extend_from_slice(&seq.to_be_bytes());
        p.extend_from_slice(&stamp.to_be_bytes());
        // CALL_DATAGRAM tag + sealed (content byte, kind, seq u16, frames).
        let len = maarse_core::appdata::CALL_DATAGRAM.len() + DM_OVERHEAD + 1 + 1 + 2 + CALL_FRAMES_PER_PACKET * (1 + OPUS_FRAME_BYTES);
        let p = pad(p, len);
        self.xf_send(a, b, PacketType::Datagram, &p);
    }

    /// Echo: send back what came in a second ago, stamped with the time the
    /// caller spoke, so the caller's latency is the full round trip.
    fn xf_echo(&mut self, k: usize) {
        let now = self.now;
        while let Some(&(due, spoke)) = self.xf.items[k].call.as_ref().and_then(|c| c.echo.front()) {
            if due > now {
                break;
            }
            if let Some(c) = self.xf.items[k].call.as_mut() {
                c.echo.pop_front();
            }
            self.xf_audio_packet(k, 1, spoke);
        }
    }

    /// Speed test: SPEED_PACKETS full packets every 60 ms until it hangs up.
    fn xf_speed(&mut self, k: usize) {
        if self.xf.items[k].state != "active" || self.xf.items[k].call.as_ref().is_none_or(|c| self.now >= c.until) {
            return;
        }
        for _ in 0..calls::test::SPEED_PACKETS {
            self.xf_audio_packet(k, 1, self.now);
        }
        self.xf_sched(self.now + calls::test::PACKET_MS, k, W_SPEED);
    }

    fn xf_on_audio(&mut self, k: usize, node: usize, b: &[u8]) {
        if b.len() < 20 {
            return;
        }
        let dir = b[7];
        let sent = u64::from_be_bytes(b[12..20].try_into().unwrap_or([0; 8]));
        let want = if dir == 0 { self.xf.items[k].to } else { self.xf.items[k].from };
        if node != want {
            return;
        }
        let now = self.now;
        let mut echo = false;
        if let Some(c) = self.xf.items[k].call.as_mut() {
            c.recv[dir as usize & 1] += 1;
            c.lat.push(now.saturating_sub(sent));
            if c.mode == calls::test::ECHO && dir == 0 {
                // (due, spoken at): what the caller said comes back after 1 s.
                c.echo.push_back((now + calls::test::ECHO_DELAY_MS, sent));
                echo = true;
            }
        }
        if echo {
            self.xf_sched(now + calls::test::ECHO_DELAY_MS, k, W_ECHO);
        }
    }

    fn xf_call_end(&mut self, k: usize) {
        if self.xf.items[k].state != "active" {
            return;
        }
        let Some(c) = self.xf.items[k].call.clone() else { return };
        let sent = c.sent[0] + c.sent[1];
        let recv = c.recv[0] + c.recv[1];
        let (med, p95) = median_p95(&c.lat);
        let loss = if sent > 0 { 100.0 * (sent - recv.min(sent)) as f64 / sent as f64 } else { 0.0 };
        let mut why = format!("call ended: {recv}/{sent} audio packets, loss {loss:.1} %, latency median {} ms, p95 {} ms", med.unwrap_or(0), p95.unwrap_or(0));
        if c.mode == calls::test::SPEED {
            // What the caller counted: payload per second over the 5 s.
            let bytes = c.recv[1] as u64 * (CALL_FRAMES_PER_PACKET * (1 + OPUS_FRAME_BYTES)) as u64;
            why = format!("speed test: {} of {} packets arrived, {:.1} KB/s; latency median {} ms", c.recv[1], c.sent[1], bytes as f64 / 1024.0 / (calls::test::SPEED_FOR_MS as f64 / 1000.0), med.unwrap_or(0));
        } else if c.mode == calls::test::ECHO {
            why = format!("echo test: {} of {} came back; {recv}/{sent} packets in all, latency median {} ms (the echo's includes the 1 s delay)", c.recv[1], c.sent[0], med.unwrap_or(0));
        }
        self.xf_set(k, "done", &why);
    }

    // ---------- IP tunnel ----------

    /// Opens a tunnel session from `from` to `to` and runs a workload
    /// ("ping", "ssh" or "bulk" of `kb` KB).
    pub fn xf_tunnel(&mut self, from: usize, to: usize, workload: &str, kb: u32, tun_lora: bool) -> usize {
        let mut it = Item::new(ItemKind::Tunnel, from, to, self.now);
        it.route = self.xf_path_kind(from, to);
        it.kb = kb.max(1);
        let route = it.route;
        let workload = match workload {
            "ping" | "ssh" | "bulk" => workload,
            _ => "ping",
        };
        it.tun = Some(Tun { workload: workload.to_string(), ..Default::default() });
        let k = self.xf_push(it);
        self.xf_trace(k, from, &format!(",\"from\":{from},\"to\":{to},\"route\":\"{}\",\"workload\":\"{workload}\"", kind_name(route)));
        // maarse-host (node/tun.rs): IP packets go over LoRa only with --tun-lora.
        let via_lora = route != PathKind::Tcp;
        if via_lora && !(tun_lora || self.rules().tun_lora) {
            self.xf_set(k, "refused", "IP over LoRa only with --tun-lora (the route is not internet all the way)");
            return k;
        }
        if !self.at_host(from) || !self.at_host(to) {
            self.xf_set(k, "refused", "the IP tunnel runs on hosts (maarse-host), not on firmware nodes");
            return k;
        }
        // The session key goes in a sealed message; then datagrams.
        let mut p = header(OP_TUN_KEY, k);
        let nonce = self.xf_nonce();
        p.extend_from_slice(&nonce);
        let p = pad(p, 1 + 32 + 4 + DM_OVERHEAD);
        self.xf_set(k, "queued", "session key sent");
        if let Some(id) = self.xf_send(from, to, PacketType::Data, &p) {
            self.xf.acks.insert((from, id), (k, OP_TUN_KEY, 0));
        }
        self.xf_sched(self.now + 60_000, k, W_TUN_NOKEY);
        k
    }

    fn xf_tun_begin(&mut self, k: usize) {
        if self.xf.items[k].state != "queued" {
            return;
        }
        let kb = self.xf.items[k].kb;
        let now = self.now;
        if let Some(t) = self.xf.items[k].tun.as_mut() {
            t.started = now;
            t.total = match t.workload.as_str() {
                "ping" => 10,
                "ssh" => 40,
                _ => (kb as usize * 1024).div_ceil(tunnel::TUN_MSS as usize) as u32,
            };
            t.screen_at = now;
        }
        self.xf_set(k, "active", "session open");
        self.xf_sched(now, k, W_TUN);
    }

    /// Sends one IP packet over the tunnel: real IPv6 bytes, real header
    /// compression, real pieces; every piece a datagram.
    fn xf_tun_send(&mut self, k: usize, dir: u8, what: u8, reference: u32, data: usize) {
        let (a, b) = if dir == 0 { (self.xf.items[k].from, self.xf.items[k].to) } else { (self.xf.items[k].to, self.xf.items[k].from) };
        let (sa, sb) = (tunnel::ip_of(&self.nodes[a].router.addr), tunnel::ip_of(&self.nodes[b].router.addr));
        let icmp = matches!(what, T_PING | T_PONG);
        let pkt = ipv6_packet(&sa, &sb, icmp, data);
        let c = tunnel::compress(&pkt, &sa, &sb);
        let Some(t) = self.xf.items[k].tun.as_mut() else { return };
        let seq = t.next_seq[dir as usize];
        t.next_seq[dir as usize] += 1;
        let Some(pieces) = tunnel::pieces(&c, seq as u8) else { return };
        let lens: Vec<usize> = pieces.iter().map(|p| p.len()).collect();
        t.sent.insert((dir, seq), (lens.clone(), what, reference, self.now));
        let n = lens.len() as u8;
        for (i, len) in lens.iter().enumerate() {
            self.xf_tun_piece(k, a, b, dir, seq, i as u8, n, what, reference, *len);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn xf_tun_piece(&mut self, k: usize, a: usize, b: usize, dir: u8, seq: u32, i: u8, n: u8, what: u8, reference: u32, len: usize) {
        let mut p = header(OP_TUN_PIECE, k);
        p.push(dir);
        p.extend_from_slice(&seq.to_be_bytes());
        p.push(i);
        p.push(n);
        p.push(what);
        p.extend_from_slice(&reference.to_be_bytes());
        let p = pad(p, tunnel::DG_OVERHEAD + len);
        if let Some(t) = self.xf.items[k].tun.as_mut() {
            t.pieces += 1;
            t.bytes_on_air += p.len() as u64;
        }
        self.xf_send(a, b, PacketType::Datagram, &p);
    }

    fn xf_on_tun_piece(&mut self, k: usize, node: usize, b: &[u8]) {
        if b.len() < 19 {
            return;
        }
        let (dir, seq, i, n, what, reference) = (b[7], be32(b, 8), b[12], b[13], b[14], be32(b, 15));
        let want = if dir == 0 { self.xf.items[k].to } else { self.xf.items[k].from };
        if node != want || n == 0 || i >= n {
            return;
        }
        let now = self.now;
        let Some(t) = self.xf.items[k].tun.as_mut() else { return };
        let r = t.rx.entry((dir, seq)).or_insert_with(|| Reasm { parts: vec![false; n as usize], first: now, nacks: 0, what, reference });
        if r.parts.len() != n as usize || r.parts[i as usize] {
            return;
        }
        r.parts[i as usize] = true;
        if r.parts.iter().all(|x| *x) {
            // Mark complete (kept so late duplicates are ignored).
            r.nacks = u8::MAX;
            r.parts.clear();
            self.xf_tun_packet(k, dir, what, reference);
        }
    }

    /// A whole IP packet arrived at the end of direction `dir`.
    fn xf_tun_packet(&mut self, k: usize, dir: u8, what: u8, reference: u32) {
        let now = self.now;
        match what {
            T_PING => self.xf_tun_send(k, 1 - dir, T_PONG, reference, 56),
            T_KEY => self.xf_tun_send(k, 1 - dir, T_ECHO, reference, 36),
            T_BULK => self.xf_tun_send(k, 1 - dir, T_BULK_ACK, reference, 0),
            T_PONG | T_ECHO | T_BULK_ACK => {
                let Some(t) = self.xf.items[k].tun.as_mut() else { return };
                if let Some((first, last, _)) = t.open.remove(&reference) {
                    let rtt = now.saturating_sub(first);
                    t.rtts.push(rtt);
                    let sample = now.saturating_sub(last);
                    t.srtt = Some(t.srtt.map_or(sample, |r| (r * 7 + sample) / 8));
                    t.acked += 1;
                    t.first_done.get_or_insert(now);
                    t.last_done = Some(now);
                }
            }
            _ => {} // screen updates: counted as pieces only
        }
    }

    /// The tunnel workload's clock (every 50 ms while active).
    fn xf_tun_tick(&mut self, k: usize) {
        if self.xf.items[k].state != "active" {
            return;
        }
        let now = self.now;
        let route_tcp = self.xf.items[k].route == PathKind::Tcp;
        let Some(t) = self.xf.items[k].tun.clone() else { return };
        // NACKs and give-ups for packets in pieces (both directions).
        for ((dir, seq), r) in &t.rx {
            if r.nacks == u8::MAX {
                continue;
            }
            let age = now.saturating_sub(r.first);
            if age >= tunnel::GIVE_UP_MS {
                if let Some(tt) = self.xf.items[k].tun.as_mut() {
                    tt.rx.remove(&(*dir, *seq));
                    tt.lost_pieces += 1;
                }
            } else if tunnel::nack_due(r.nacks, age) {
                let mask = r.parts.iter().enumerate().filter(|(_, x)| !**x).fold(0u16, |m, (i, _)| m | 1 << i);
                if let Some(tt) = self.xf.items[k].tun.as_mut() {
                    if let Some(rr) = tt.rx.get_mut(&(*dir, *seq)) {
                        rr.nacks += 1;
                    }
                    tt.nacks += 1;
                }
                let (a, b) = if *dir == 0 { (self.xf.items[k].to, self.xf.items[k].from) } else { (self.xf.items[k].from, self.xf.items[k].to) };
                let mut p = header(OP_TUN_NACK, k);
                p.push(*dir);
                p.extend_from_slice(&seq.to_be_bytes());
                p.extend_from_slice(&mask.to_be_bytes());
                let _ = (r.what, r.reference);
                let p = pad(p, tunnel::DG_OVERHEAD + tunnel::nack(0, mask).len());
                self.xf_send(a, b, PacketType::Datagram, &p);
            }
        }
        if let Some(tt) = self.xf.items[k].tun.as_mut() {
            tt.sent.retain(|_, v| now.saturating_sub(v.3) < tunnel::KEEP_SENT_MS);
        }
        let rto = t.srtt.map_or(if route_tcp { 1_000 } else { 6_000 }, |r| (r * 2).max(if route_tcp { 200 } else { 2_000 }));
        let step = now.saturating_sub(t.started);
        match t.workload.as_str() {
            "ping" => {
                // One echo request a second; an answer missing for 5 s is lost.
                if t.next < t.total && step >= t.next as u64 * 1000 {
                    let r = t.next;
                    if let Some(tt) = self.xf.items[k].tun.as_mut() {
                        tt.next += 1;
                        tt.open.insert(r, (now, now, 1));
                    }
                    self.xf_tun_send(k, 0, T_PING, r, 56);
                }
                let done = t.next >= t.total && (t.open.is_empty() || step > t.total as u64 * 1000 + 5_000);
                if done {
                    return self.xf_tun_finish(k);
                }
            }
            "ssh" => {
                // A keystroke every 150 ms, echoed; a screen update each second.
                if t.next < t.total && step >= t.next as u64 * 150 {
                    let r = t.next;
                    if let Some(tt) = self.xf.items[k].tun.as_mut() {
                        tt.next += 1;
                        tt.open.insert(r, (now, now, 1));
                    }
                    self.xf_tun_send(k, 0, T_KEY, r, 36);
                }
                if now.saturating_sub(t.screen_at) >= 1_000 && t.next < t.total {
                    if let Some(tt) = self.xf.items[k].tun.as_mut() {
                        tt.screen_at = now;
                    }
                    self.xf_tun_send(k, 1, T_SCREEN, 0, 500);
                }
                self.xf_tun_resend(k, T_KEY, 36, rto, 5);
                let t = self.xf.items[k].tun.as_ref().unwrap();
                if t.next >= t.total && (t.open.is_empty() || step > 120_000) {
                    return self.xf_tun_finish(k);
                }
            }
            _ => {
                // Bulk copy: segments of the clamped MSS, four in flight,
                // each acknowledged; resend after the timeout.
                let window = 4usize;
                let mss = tunnel::TUN_MSS as usize;
                while let Some(tt) = self.xf.items[k].tun.as_ref() {
                    if tt.next >= tt.total || tt.open.len() >= window {
                        break;
                    }
                    let r = tt.next;
                    let left = (self.xf.items[k].kb as usize * 1024).saturating_sub(r as usize * mss).min(mss);
                    if let Some(tt) = self.xf.items[k].tun.as_mut() {
                        tt.next += 1;
                        tt.open.insert(r, (now, now, 1));
                    }
                    self.xf_tun_send(k, 0, T_BULK, r, left);
                }
                self.xf_tun_resend(k, T_BULK, mss, rto, 8);
                let t = self.xf.items[k].tun.as_ref().unwrap();
                if t.acked >= t.total || step > 900_000 {
                    return self.xf_tun_finish(k);
                }
            }
        }
        self.xf_sched(now + 50, k, W_TUN);
    }

    /// Requests without an answer after `rto` are sent again (TCP's
    /// retransmission in the model), up to `tries` times.
    fn xf_tun_resend(&mut self, k: usize, what: u8, data: usize, rto: u64, tries: u8) {
        let now = self.now;
        let Some(t) = self.xf.items[k].tun.as_mut() else { return };
        let due: Vec<u32> = t.open.iter().filter(|(_, v)| now.saturating_sub(v.1) >= rto && v.2 < tries).map(|(r, _)| *r).collect();
        for r in &due {
            if let Some(v) = t.open.get_mut(r) {
                v.1 = now;
                v.2 += 1;
            }
            t.retrans += 1;
        }
        for r in due {
            self.xf_tun_send(k, 0, what, r, data);
        }
    }

    fn xf_on_tun_nack(&mut self, k: usize, node: usize, b: &[u8]) {
        if b.len() < 14 {
            return;
        }
        let (dir, seq, mask) = (b[7], be32(b, 8), be16(b, 12));
        let sender = if dir == 0 { self.xf.items[k].from } else { self.xf.items[k].to };
        if node != sender {
            return;
        }
        let Some((lens, what, reference, _)) = self.xf.items[k].tun.as_ref().and_then(|t| t.sent.get(&(dir, seq)).cloned()) else { return };
        let other = if dir == 0 { self.xf.items[k].to } else { self.xf.items[k].from };
        let n = lens.len() as u8;
        let asked: Vec<(usize, usize)> = tunnel::asked_pieces(&lens.iter().copied().enumerate().collect::<Vec<_>>(), mask).copied().collect();
        for (i, len) in asked {
            self.xf_tun_piece(k, sender, other, dir, seq, i as u8, n, what, reference, len);
        }
    }

    fn xf_tun_finish(&mut self, k: usize) {
        let Some(t) = self.xf.items[k].tun.clone() else { return };
        let (med, p95) = median_p95(&t.rtts);
        let why = match t.workload.as_str() {
            "bulk" => {
                let bytes = (self.xf.items[k].kb as u64 * 1024).min(t.acked as u64 * tunnel::TUN_MSS as u64);
                let secs = t.last_done.unwrap_or(self.now).saturating_sub(t.started).max(1) as f64 / 1000.0;
                format!("copied {} of {} KB in {secs:.1} s ({:.2} KB/s), {} resent, {} pieces", bytes / 1024, self.xf.items[k].kb, bytes as f64 / 1024.0 / secs, t.retrans, t.pieces)
            }
            w => format!("{w}: {}/{} answered, round trip median {} ms, p95 {} ms, {} resent, {} NACKs", t.acked, t.total, med.unwrap_or(0), p95.unwrap_or(0), t.retrans, t.nacks),
        };
        if let Some(tt) = self.xf.items[k].tun.as_mut() {
            tt.finished = Some(self.now);
        }
        self.xf_set(k, "done", &why);
    }

    // ---------- engine hooks ----------

    /// An ack for a transfer packet. True if it was one.
    pub(crate) fn xfer_acked(&mut self, node: usize, msg_id: u64) -> bool {
        let Some((k, op, idx)) = self.xf.acks.remove(&(node, msg_id)) else { return false };
        match op {
            OP_OFFER => {
                if !self.xf.items[k].offer_acked {
                    self.xf.items[k].offer_acked = true;
                    if self.xf.items[k].state == "queued" {
                        self.xf_set(k, "sending", "offer taken");
                    }
                    self.xf_pump(k);
                }
            }
            OP_CHUNK => self.xf_chunk_acked(k, idx),
            OP_TUN_KEY => self.xf_tun_begin(k),
            _ => {}
        }
        true
    }

    /// A transfer packet delivered to `node`. True if it was one.
    pub(crate) fn xfer_on_deliver(&mut self, node: usize, ptype: PacketType, b: &[u8], hops: u8) -> bool {
        if !matches!(ptype, PacketType::Data | PacketType::Fragment | PacketType::Datagram) || b.len() < 7 || &b[..2] != MAGIC {
            return false;
        }
        let k = be32(b, 3) as usize;
        if b[2] == OP_PHONE_CALL {
            self.phone_call_rx(node, b);
            return true;
        }
        if k >= self.xf.items.len() {
            return true;
        }
        match b[2] {
            OP_OFFER => self.xf_on_offer(k, node, hops),
            OP_CHUNK => self.xf_on_chunk(k, node, b),
            OP_REQUEST => self.xf_on_request(k, node, b),
            OP_CANCEL => self.xf_on_cancel(k, node, b),
            OP_PAGE => self.xf_on_page_request(k, node, b),
            OP_BOARD => self.xf_on_board(k, node),
            OP_RING => self.xf_on_ring(k, node, b),
            OP_AUDIO => self.xf_on_audio(k, node, b),
            OP_TUN_KEY => {}
            OP_TUN_PIECE => self.xf_on_tun_piece(k, node, b),
            OP_TUN_NACK => self.xf_on_tun_nack(k, node, b),
            _ => {}
        }
        true
    }

    /// Call data from a phone (`SendCall`): refused unless TCP all the way,
    /// else sent like maarse-host does (audio as datagrams, the rest as
    /// acknowledged messages). Returns the refusal.
    pub(crate) fn xf_phone_call(&mut self, from: usize, to: usize, data: &[u8]) -> Option<(u8, &'static str)> {
        if let Some(r) = calls::call_refusal(self.xf_path_kind(from, to)) {
            return Some(r);
        }
        let mut p = MAGIC.to_vec();
        p.push(OP_PHONE_CALL);
        p.extend_from_slice(&(from as u32).to_be_bytes());
        p.extend_from_slice(data);
        let audio = data.first() == Some(&4);
        let len = p.len() + DM_OVERHEAD + if audio { maarse_core::appdata::CALL_DATAGRAM.len() } else { 0 };
        if !audio {
            let nonce = self.xf_nonce();
            p.extend_from_slice(&nonce); // distinct packet ids for repeated control messages
        }
        let p = pad(p, len.max(7));
        let sent = self.xf_send(from, to, if audio { PacketType::Datagram } else { PacketType::Data }, &p);
        if sent.is_none() {
            return Some((error_code::UNKNOWN_DEST, "The queue is full, try again shortly"));
        }
        None
    }

    fn phone_call_rx(&mut self, node: usize, b: &[u8]) {
        let from = be32(b, 3) as usize;
        if from >= self.nodes.len() {
            return;
        }
        let audio = b.get(7) == Some(&4);
        // Control messages carry a 4-byte nonce after the data; audio is padded.
        let body = &b[7..];
        let data: Vec<u8> = if audio {
            // kind 4 | seq u16 | (len | frame)…  — cut at the padding.
            let mut end = 3.min(body.len());
            while end < body.len() && body[end] > 0 && end + 1 + body[end] as usize <= body.len() {
                end += 1 + body[end] as usize;
            }
            body[..end].to_vec()
        } else {
            let n = body.len().saturating_sub(4);
            let mut v = body[..n].to_vec();
            while v.last() == Some(&0) && v.len() > 1 {
                v.pop();
            }
            v
        };
        let addr = self.nodes[from].router.addr;
        self.phone_emit(node, &Event::Call { from: addr, data: text(&data) });
    }

    // ---------- phone uploads ----------

    pub(crate) fn xf_file_begin(&mut self, node: usize, req: u16, dest: usize, size: u32, mime: String, name: String) -> Result<(), (u8, String)> {
        let l = self.xf_limit(node, dest);
        if size > l.max_bytes {
            let m = if self.box_host(node).is_none() {
                "The file is too large (at most 8 KB over LoRa)".to_string()
            } else if l.via_lora && !l.mailbox {
                format!("The file is too large: the recipient is only reached over LoRa, where the limit is {} KB", l.max_bytes / 1024)
            } else {
                format!("The file is too large (at most {} KB)", l.max_bytes / 1024)
            };
            return Err((error_code::FILE_TOO_LARGE, m));
        }
        if !self.knows(node, dest) {
            return Err((error_code::UNKNOWN_DEST, "The recipient has not been heard yet (no announce)".into()));
        }
        self.xf.uploads.insert((node, req), Upload { dest, size, mime, name, data: Vec::with_capacity(size as usize) });
        Ok(())
    }

    pub(crate) fn xf_file_data(&mut self, node: usize, req: u16, offset: u32, data: &[u8]) {
        if let Some(u) = self.xf.uploads.get_mut(&(node, req))
            && offset as usize == u.data.len()
            && u.data.len() + data.len() <= u.size as usize
        {
            u.data.extend_from_slice(data);
        }
    }

    /// `FileEnd`: the upload is complete; send it. Returns (file id, chunks).
    pub(crate) fn xf_file_end(&mut self, node: usize, req: u16) -> Result<(u64, u16), (u8, String)> {
        let Some(u) = self.xf.uploads.remove(&(node, req)) else {
            return Err((error_code::NO_FILE, "No file being uploaded".into()));
        };
        if u.data.len() != u.size as usize {
            return Err((error_code::NO_FILE, "The file was not transferred completely".into()));
        }
        let kind = if u.mime == VOICE_MIME {
            ItemKind::Voice
        } else if u.mime.starts_with("image/") {
            ItemKind::Picture
        } else {
            ItemKind::File
        };
        let k = self.xf_send_file(node, u.dest, kind, u.size, &u.mime, &u.name, Some(u.data))?;
        self.xf.items[k].phone_req = Some(req);
        Ok((self.xf.items[k].file_id, self.xf.items[k].chunks))
    }

    pub(crate) fn xf_by_file_id(&self, file_id: u64) -> Option<usize> {
        self.xf.items.iter().position(|it| it.file_id == file_id && file_id != 0)
    }

    // ---------- reporting ----------

    pub fn item_json(&self, k: usize) -> Value {
        let Some(it) = self.xf.items.get(k) else { return json!({"error": "no such item"}) };
        let mut v = json!({
            "item": k, "kind": it.kind.name(), "from": it.from, "to": it.to, "bytes": it.bytes,
            "chunks": it.chunks, "done": it.done.max(it.ngot), "state": it.state, "why": it.why,
            "route": kind_name(it.route), "created_ms": it.created, "delivered_ms": it.delivered,
            "mailbox_host": if it.boxed_at.is_some() { it.box_host.map(|h| h as i64).unwrap_or(-1) } else { -1 },
            "from_box": it.from_box, "mime": it.mime, "name": it.name, "path": it.path,
            "child": it.child, "file_id": format!("{:016x}", it.file_id),
        });
        if let Some(c) = &it.call {
            let (med, p95) = median_p95(&c.lat);
            v["call"] = json!({"sent": c.sent, "recv": c.recv, "latency_median_ms": med, "latency_p95_ms": p95, "secs": it.secs, "started_ms": c.started});
        }
        if let Some(t) = &it.tun {
            let (med, p95) = median_p95(&t.rtts);
            let secs = t.last_done.unwrap_or(self.now).saturating_sub(t.started).max(1) as f64 / 1000.0;
            let bytes = if t.workload == "bulk" { (it.kb as u64 * 1024).min(t.acked as u64 * tunnel::TUN_MSS as u64) } else { 0 };
            v["tunnel"] = json!({"workload": t.workload, "total": t.total, "answered": t.acked, "rtt_median_ms": med, "rtt_p95_ms": p95,
                "resent": t.retrans, "nacks": t.nacks, "pieces": t.pieces, "bytes_on_air": t.bytes_on_air, "lost_packets": t.lost_pieces,
                "kbps": if t.workload == "bulk" { json!(bytes as f64 / 1024.0 / secs) } else { Value::Null }});
        }
        v
    }

    /// An item with its trace lines (for the follow panel).
    pub fn item_follow(&self, k: usize) -> Value {
        let k1 = format!("\"item\":{k},");
        let lines: Vec<Value> = self.trace.iter().filter(|l| l.contains(&k1)).filter_map(|l| serde_json::from_str(l).ok()).collect();
        let mut v = self.item_json(k);
        v["events"] = json!(lines);
        v
    }

    pub fn items_json(&self, from: usize) -> Value {
        json!({"total": self.xf.items.len(), "items": (from..self.xf.items.len()).map(|k| self.item_json(k)).collect::<Vec<_>>()})
    }
}

#[cfg(test)]
mod tests {
    use crate::create;
    use serde_json::{Value, json};

    fn world(extra: Value) -> crate::engine::Engine {
        let mut sc = json!({"seed": 5, "duration_s": 3600, "nodes": [
            {"kind": "host", "lat": 56.0, "lon": 9.0},
            {"kind": "repeater", "lat": 56.0, "lon": 9.05},
            {"kind": "host", "lat": 56.0, "lon": 9.1},
            {"kind": "handheld", "lat": 56.0, "lon": 9.02},
            {"kind": "handheld", "lat": 56.0, "lon": 9.08}
        ], "links": [{"a": 0, "b": 2, "kind": "tcp"}],
        "traffic": {"direct_per_min": 0, "channel_per_min": 0}});
        if let Value::Object(m) = extra {
            for (k, v) in m {
                sc[k] = v;
            }
        }
        let mut e = create(&sc.to_string()).unwrap();
        e.run_until(30_000, usize::MAX);
        e
    }

    fn run(e: &mut crate::engine::Engine, secs: u64) {
        let t = e.now + secs * 1000;
        e.run_until(t, usize::MAX);
    }

    fn internet(e: &mut crate::engine::Engine, up: bool) {
        let ev: crate::scenario::TimedEvent = serde_json::from_value(json!({"t_s": 0, "type": if up { "internet_up" } else { "internet_down" }})).unwrap();
        e.inject(&ev);
    }

    #[test]
    fn small_file_over_lora_is_delivered() {
        let mut e = world(json!({}));
        let k = e.xf_send_file(3, 4, super::ItemKind::File, 6000, "application/octet-stream", "a.bin", None).unwrap();
        assert_eq!(e.item_json(k)["route"], "lora");
        run(&mut e, 600);
        let v = e.item_json(k);
        assert_eq!(v["state"], "delivered", "{v}");
        assert_eq!(v["done"], v["chunks"]);
    }

    #[test]
    fn firmware_refuses_over_8_kb() {
        let mut e = world(json!({}));
        let r = e.xf_send_file(3, 4, super::ItemKind::File, 9000, "x", "b", None);
        assert!(r.unwrap_err().1.contains("8 KB"));
    }

    #[test]
    fn large_file_over_a_lora_route_is_refused() {
        let mut e = world(json!({}));
        internet(&mut e, false);
        let r = e.xf_send_file(0, 2, super::ItemKind::File, 20_000, "x", "big", None);
        assert_eq!(r.unwrap_err().1, "The file is too large: the recipient is only reached over LoRa, where the limit is 8 KB");
        let l = e.xf_limit(0, 2);
        assert!(l.via_lora && !l.mailbox && l.max_bytes == 8192);
    }

    #[test]
    fn large_file_waits_in_the_mailbox_until_tcp() {
        // Host 2 is out of radio range: without internet it cannot be reached.
        let mut e = world(json!({"nodes": [
            {"kind": "host", "lat": 56.0, "lon": 9.0},
            {"kind": "repeater", "lat": 56.0, "lon": 9.05},
            {"kind": "host", "lat": 56.0, "lon": 11.5}
        ]}));
        internet(&mut e, false);
        let k = e.xf_send_file(0, 2, super::ItemKind::File, 20_000, "x", "big", None).unwrap();
        let v = e.item_json(k);
        assert_eq!(v["route"], "unknown", "{v}");
        assert_eq!(v["state"], "mailbox", "{v}");
        assert_eq!(v["mailbox_host"], 0);
        run(&mut e, 120);
        assert_eq!(e.item_json(k)["state"], "mailbox");
        internet(&mut e, true);
        run(&mut e, 120);
        let v = e.item_json(k);
        assert_eq!(v["state"], "delivered", "{v}");
        assert_eq!(v["route"], "tcp");
        assert_eq!(v["from_box"], true);
    }

    #[test]
    fn voice_message_is_delivered() {
        let mut e = world(json!({}));
        let bytes = 8 * super::VOICE_BYTES_PER_S;
        let k = e.xf_send_file(3, 4, super::ItemKind::Voice, bytes, super::VOICE_MIME, "voice.c2", None).unwrap();
        run(&mut e, 600);
        assert_eq!(e.item_json(k)["state"], "delivered", "{}", e.item_json(k));
    }

    #[test]
    fn page_is_fetched_from_a_host() {
        let mut e = world(json!({}));
        let k = e.xf_page(3, 0, "weather/aarhus", None).unwrap();
        run(&mut e, 300);
        let v = e.item_json(k);
        assert_eq!(v["state"], "delivered", "{v}");
        assert!(v["bytes"].as_u64().unwrap() > 0);
        // A picture copy over LoRa: the greyscale size.
        let k = e.xf_page(3, 0, "_grey/map.jpg", None).unwrap();
        run(&mut e, 600);
        let v = e.item_json(k);
        assert_eq!(v["state"], "delivered", "{v}");
        assert_eq!(v["bytes"], maarse_hostrules::pages::GREY_TARGET as u64);
    }

    #[test]
    fn page_burst_rule_over_lora() {
        let mut e = world(json!({"rules": {"pages_burst_lora": 2}}));
        let ks: Vec<usize> = (0..4).map(|i| e.xf_page(3, 0, &format!("p{i}"), None).unwrap()).collect();
        run(&mut e, 900);
        let states: Vec<String> = ks.iter().map(|&k| e.item_json(k)["state"].as_str().unwrap().to_string()).collect();
        assert_eq!(states, ["delivered", "delivered", "refused", "refused"]);
        assert_eq!(e.item_json(ks[2])["why"], "slow down", "the third gets the Slow down page");
        assert!(e.item_json(ks[3])["why"].as_str().unwrap().contains("quiet"));
    }

    #[test]
    fn call_refused_over_lora_accepted_over_tcp() {
        let mut e = world(json!({}));
        let k = e.xf_call(3, 4, 5);
        let v = e.item_json(k);
        assert_eq!(v["state"], "refused");
        assert_eq!(v["why"], maarse_hostrules::calls::REFUSED_TEXT);
        let k = e.xf_call(0, 2, 5);
        run(&mut e, 30);
        let v = e.item_json(k);
        assert_eq!(v["state"], "done", "{v}");
        assert!(v["call"]["recv"][0].as_u64().unwrap() > 50, "{v}");
    }

    #[test]
    fn lost_device_restored_gets_its_mail() {
        let mut e = world(json!({"mail": "post"}));
        let ev: crate::scenario::TimedEvent = serde_json::from_value(json!({"t_s": 0, "type": "device_lost", "node": 3, "duration_s": 300})).unwrap();
        e.inject(&ev);
        let m = e.send_message(4, 3);
        run(&mut e, 200);
        assert!(e.msgs[m].delivered_ms.is_none(), "the device is gone");
        assert!(e.summary()["mail_deposits"].as_u64().unwrap() > 0, "kept at a post office");
        run(&mut e, 400);
        let s = e.summary();
        assert_eq!(s["restores"], json!(1), "{s}");
        assert!(e.msgs[m].delivered_ms.is_some(), "the restored device gets the mail: {s}");
    }

    #[test]
    fn locked_pages_and_notice_boards() {
        let nodes = json!([
            {"kind": "host", "lat": 56.0, "lon": 9.0},
            {"kind": "repeater", "lat": 56.0, "lon": 9.05},
            {"kind": "host", "lat": 56.0, "lon": 9.1, "site": {"name": "Harbour", "groups": {"crew": [0]}, "pages": {
                "index": "# Harbour\n@board 5\n",
                "crew/index": "@allow group crew\n# Crew only\n",
                "crew/plan": "# Plan\n",
                "secret": "@allow group crew\n@locked hidden\n# Secret\n"
            }}},
            {"kind": "handheld", "lat": 56.0, "lon": 9.02},
            {"kind": "handheld", "lat": 56.0, "lon": 9.08}
        ]);
        let mut e = world(json!({"nodes": nodes}));
        // The crew member reads the locked folder; the lock lines are cut out.
        let m = e.page_markup(2, "crew", 0);
        assert!(m.contains("Crew only") && !m.contains("@allow"), "{m}");
        assert!(e.page_markup(2, "crew/plan", 0).contains("# Plan"));
        // Others get the locked notice, also for a page inside the folder.
        assert!(e.page_markup(2, "crew", 3).contains("This page is locked"));
        assert!(e.page_markup(2, "crew/plan", 3).contains("This page is locked"));
        // A hidden page looks like a missing one.
        assert!(e.page_markup(2, "secret", 3).starts_with("# Not found"));
        // Boards: one note a minute, shown after the board line.
        assert_eq!(e.board_post(2, 3, "index", b"Ferry is late"), Ok(()));
        assert_eq!(e.board_post(2, 3, "index", b"Again"), Err("one note a minute"));
        assert_eq!(e.board_post(2, 3, "crew", b"hi"), Err("this page has no notice board"));
        let m = e.page_markup(2, "index", 4);
        assert!(m.contains("@boardpath index") && m.contains("Ferry is late"), "{m}");
        // Over the network: a note sent from handheld 4 lands on the board.
        let k = e.xf_board(4, 2, "index", "Fish at pier 2").unwrap();
        run(&mut e, 60);
        assert_eq!(e.item_json(k)["state"], "done", "{}", e.item_json(k));
        assert!(e.page_markup(2, "index", 0).contains("Fish at pier 2"));
    }

    #[test]
    fn sequence_gap_between_hosts() {
        let mut e = world(json!({"mail": ""}));
        let (a, b, c) = (e.send_message(0, 2), e.send_message(0, 2), e.send_message(0, 2));
        assert_eq!((e.msgs[a].seq, e.msgs[b].seq, e.msgs[c].seq), (1, 2, 3));
        // Arrivals 1, 3, 2: one gap of one message, then a late one.
        e.seq_on_deliver(a, 2, 1);
        e.seq_on_deliver(c, 2, 1);
        e.seq_on_deliver(b, 2, 1);
        let s = e.summary();
        assert_eq!((s["seq_gaps"].as_u64(), s["seq_missing"].as_u64(), s["seq_late"].as_u64()), (Some(1), Some(1), Some(1)), "{s}");
        // Handhelds do not number (no CAP_SEQ).
        let d = e.send_message(3, 4);
        assert_eq!(e.msgs[d].seq, 0);
    }

    #[test]
    fn hold_direct_keeps_messages_for_an_absent_phone() {
        // The same drop with and without the proposed rule.
        for hold in [false, true] {
            let mut e = world(json!({"mail": "post", "features": {"hold_direct": hold}}));
            e.phone_cmd(3, &[]);
            e.phone_link_drop(3, 180_000);
            let m = e.send_message(4, 3);
            run(&mut e, 400);
            let s = e.summary();
            if hold {
                assert_eq!(s["phone_lost_msgs"], json!(0), "{s}");
                assert!(e.nodes[3].router.stats.held_direct > 0, "{s}");
                assert!(s["mail_deposits"].as_u64().unwrap() > 0, "went to the post office: {s}");
                let t = e.msgs[m].delivered_ms.expect("delivered after the phone came back");
                assert!(t >= e.msgs[m].sent_ms + 180_000, "not before the phone was back");
                assert!(!e.nodes[3].router.hold_direct, "released on reconnect");
            } else {
                assert!(s["phone_lost_msgs"].as_u64().unwrap() >= 1, "{s}");
            }
        }
    }

    #[test]
    fn phone_link_drop_loses_what_arrives_meanwhile() {
        // The firmware before hold_direct (2026-11-19).
        let mut e = world(json!({"features": {"hold_direct": false}}));
        // A phone on handheld 3 (firmware node, Bluetooth to the radio).
        e.phone_cmd(3, &[]);
        e.phone_link_drop(3, 120_000);
        let m = e.send_message(4, 3);
        run(&mut e, 90);
        let s = e.summary();
        assert!(s["phone_lost_msgs"].as_u64().unwrap() >= 1, "{s}");
        // The radio itself got it (and may have acknowledged it).
        assert!(e.msgs[m].delivered_ms.is_some());
        // Back on the link: the phone is told what it missed; new messages arrive.
        run(&mut e, 60);
        let ev = e.phone_events(3);
        assert_eq!(ev["link"], json!(true));
        let before = e.summary()["phone_lost_msgs"].as_u64().unwrap();
        e.send_message(4, 3);
        run(&mut e, 60);
        assert_eq!(e.summary()["phone_lost_msgs"].as_u64().unwrap(), before);
    }

    #[test]
    fn call_test_service_music_echo_speed() {
        let nodes = json!([
            {"kind": "host", "lat": 56.0, "lon": 9.0},
            {"kind": "repeater", "lat": 56.0, "lon": 9.05},
            {"kind": "host", "lat": 56.0, "lon": 9.1, "calltest": true}
        ]);
        let mut e = world(json!({"nodes": nodes}));
        // Music: only the service sends.
        let k = e.xf_call_mode(0, 2, 5, maarse_hostrules::calls::test::MUSIC);
        run(&mut e, 15);
        let v = e.item_json(k);
        assert_eq!(v["state"], "done", "{v}");
        assert_eq!(v["call"]["sent"][0], 0, "{v}");
        assert!(v["call"]["recv"][1].as_u64().unwrap() > 50, "{v}");
        // Echo: what goes out comes back.
        let k = e.xf_call_mode(0, 2, 5, maarse_hostrules::calls::test::ECHO);
        run(&mut e, 15);
        let v = e.item_json(k);
        assert!(v["why"].as_str().unwrap().starts_with("echo test"), "{v}");
        let back = v["call"]["recv"][1].as_u64().unwrap();
        let out = v["call"]["sent"][0].as_u64().unwrap();
        assert!(back > 50 && back <= out, "{v}");
        // Speed: 8 packets per 60 ms for 5 s, then it hangs up.
        let k = e.xf_call_mode(0, 2, 60, maarse_hostrules::calls::test::SPEED);
        run(&mut e, 15);
        let v = e.item_json(k);
        assert!(v["why"].as_str().unwrap().starts_with("speed test"), "{v}");
        let sent = v["call"]["sent"][1].as_u64().unwrap();
        assert!((600..=700).contains(&sent), "5 s / 60 ms * 8 = ~667: {v}");
        // An ordinary host ignores the mode.
        let k = e.xf_call_mode(2, 0, 5, maarse_hostrules::calls::test::SPEED);
        run(&mut e, 15);
        assert!(e.item_json(k)["call"]["sent"][0].as_u64().unwrap() > 50);
    }

    #[test]
    fn ssh_over_tcp_works_over_lora_only_with_tun_lora() {
        let mut e = world(json!({}));
        let k = e.xf_tunnel(0, 2, "ssh", 0, false);
        run(&mut e, 60);
        let v = e.item_json(k);
        assert_eq!(v["state"], "done", "{v}");
        assert_eq!(v["tunnel"]["answered"], 40, "{v}");
        internet(&mut e, false);
        let k = e.xf_tunnel(0, 2, "ssh", 0, false);
        assert_eq!(e.item_json(k)["state"], "refused");
        let k = e.xf_tunnel(0, 2, "ping", 0, true);
        run(&mut e, 120);
        let v = e.item_json(k);
        assert_eq!(v["route"], "lora");
        assert_eq!(v["state"], "done", "{v}");
        assert!(v["tunnel"]["answered"].as_u64().unwrap() > 0, "{v}");
    }

    #[test]
    fn bulk_copy_over_tcp_reports_throughput() {
        let mut e = world(json!({}));
        let k = e.xf_tunnel(0, 2, "bulk", 50, false);
        run(&mut e, 120);
        let v = e.item_json(k);
        assert_eq!(v["state"], "done", "{v}");
        assert!(v["tunnel"]["kbps"].as_f64().unwrap() > 1.0, "{v}");
    }

    #[test]
    fn cancel_by_recipient_stops_the_file() {
        let mut e = world(json!({}));
        let k = e.xf_send_file(3, 4, super::ItemKind::File, 8000, "x", "c", None).unwrap();
        run(&mut e, 5);
        assert!(e.xf_cancel(k, 4));
        run(&mut e, 120);
        assert_eq!(e.item_json(k)["state"], "cancelled");
    }
}
