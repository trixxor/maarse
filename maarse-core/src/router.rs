//! The routing engine.
//!
//! `Router` has no I/O and no clock. It takes bytes and timestamps in and
//! returns [`Action`]s: packets to send (with a delay), messages to deliver
//! to the app, and events like "route found".
//! That makes it easy to test and to run the same way in firmware and simulator.
//!
//! ## How a route is found
//! 1. The sender floods a `PathRequest` with a random reply token.
//! 2. Every repeater appends its path ID and forwards after a
//!    delay: the better the SNR, the longer the wait. So the repeater
//!    furthest away wins, and routes get fewer hops. Repeaters with
//!    low battery wait extra and are therefore chosen less often.
//! 3. The destination answers with a `PathResponse` to the token along the
//!    reverse path. The sender's address is never in clear text.
//! 4. The sender stores the path and sends the message as a source path.
//! 5. The recipient acknowledges with an `Ack` to a token derived from the message ID.
//!
//! ## Paths over 12 hops: anchor repeaters
//! A source path can carry at most [`MAX_PATH`] (12) hops. When a path request
//! reaches a repeater with the role [`Role::Anchor`], the anchor remembers the first
//! leg (the way back to the sender) and forwards
//! the request with an empty trail. The target answers along the second leg
//! to the anchor, which learns the onward route to the target and sends the reply back along
//! the first leg, with the first leg as the route. The sender then sends
//! its messages to the anchor; the anchor sees that the path ends at it, but that
//! the message is for someone else, and forwards it along its own route.
//! Acknowledgements follow the same way back. The packet format is unchanged, and several
//! anchors in a row work by themselves (up to `MAX_HOPS` in total).

//! ## Local repair
//! A repeater that sends a source-routed packet listens for the next hop to
//! forward it, and retransmits up to [`HOP_RETRIES`] times. If the next hop
//! never answers, the repeater looks for a node further along the path that
//! it has heard in the last two minutes (or the destination itself) and
//! sends the packet straight there, with the dead hop cut out of the path.
//! The dead hop is remembered for five minutes (or until it is heard again),
//! so later packets skip it at once. This saves the sender's end-to-end
//! timeout and a new path request flooded from the start. The packet format
//! is unchanged, so older nodes on the path work as before.

//! ## Carry (store-carry-forward)
//! With `carry_copies > 0`, a direct message the router gives up on (no
//! route found, or no acknowledgement after the retries) is not dropped but
//! carried: the node keeps it with `carry_copies` tokens ("binary spray and
//! wait"). Every few seconds the router looks at the neighbours it has heard
//! recently: if one is the destination, the message is handed to it; if not,
//! a holder with more than one token gives half of them to a neighbour that
//! has none. People move, so islands of a group meet again, and the message
//! gets across. Copies expire after [`CARRY_TTL_MS`].
//!
//! On the air this is a [`PacketType::Carry`] packet with a one-element path
//! naming the receiving neighbour and the payload
//! `tokens (1) | ttl left in s (u16 BE) | from (PathId, u16 BE) | message`.
//! The destination delivers the message as an ordinary `Data` message (it is
//! still end-to-end encrypted, so carriers cannot read it).
//!
//! ## Delivered notice
//! When the destination gets a carried message (also a repeated copy), it
//! sends a "delivered" notice: a [`PacketType::Carry`] packet *without* a
//! path, addressed to [`ack_token`] of the message id, with no payload
//! (18 bytes on the air). Nodes from before the notice drop Carry packets
//! without a path, so the change is backward tolerant. A carrier that hears
//! the notice deletes its copy and stops handing out copies, passes the
//! notice on once, and remembers it for the rest of the copy's lifetime.
//! It repeats it when it meets a neighbour it gave a copy to (or got its
//! copy from), and answers a handoff of a delivered message with the
//! notice instead of storing it. So the notice walks back along the spray
//! tree to the sender, which reports [`Action::CarryAcked`] ("delivered
//! (carried)"). A sender that gets an ordinary acknowledgement for a
//! message it carries does the same, so its carriers stop too.
//!
//! ## Delivery proof
//!
//! An acknowledgement is addressed to [`ack_token`], which anyone who heard
//! the message can compute. So the recipient adds a proof of [`PROOF_LEN`]
//! bytes that only the sender and the recipient can compute (see
//! `maarse_crypto::receipt_proof`): as the payload of the `Ack`, after the
//! path in a `PathResponse` that answers a request carrying the message, and
//! as the payload of the delivered notice. A sender that has said what to
//! expect ([`Router::expect_proof`]) counts nothing else as delivered.
//! Repeaters and anchors pass the bytes on without reading them.
//!
//! With [`Router::app_acks`] the recipient's router does not acknowledge a
//! direct message by itself: the app calls [`Router::acknowledge`] once it
//! has opened it. A message nobody could open is then not reported as
//! delivered, also not to senders that expect no proof.

use crate::addr::{Address, PathId};
use crate::dedup::{DedupCache, packet_id};
use crate::packet::{Header, Packet, PacketType, Path, encode};
use crate::routes::{Route, RouteTable};
use crate::stamp;
use crate::diag;
use crate::{LOCAL_SCOPE, MAX_HOPS, MAX_PACKET, MAX_PATH};

pub type Buf = heapless::Vec<u8, MAX_PACKET>;
pub type Actions = heapless::Vec<Action, 8>;

/// Delay before a repeater forwards a source-routed packet (ms).
pub const SOURCE_ROUTE_DELAY_MS: u32 = 50;
/// Largest payload in a direct message (room for a full path of 12 hops).
pub const MAX_DIRECT_PAYLOAD: usize = MAX_PACKET - 18 - 1 - 2 * MAX_PATH;

/// Default time slot for the flood wait (ms). Should be about the time on air
/// of a short packet at the chosen spreading factor.
pub const DEFAULT_SLOT_MS: u32 = 200;
/// How long we wait to hear the next repeater forward (ms).
pub const DEFAULT_HOP_TIMEOUT_MS: u32 = 3000;
/// Number of times a hop is retransmitted if the next repeater is not heard.
pub const HOP_RETRIES: u8 = 4;

/// Default hop limit for relays in group mode.
pub const DEFAULT_RELAY_HOPS: u8 = 4;
/// The most hops a relay may forward a flood to, whatever `relay_hops` says.
pub const MAX_RELAY_HOPS: u8 = 12;

const MAX_PENDING: usize = 8;
/// The destination waits this many time slots before it answers a
/// path request (the flood must have time to die out around it).
pub const RESPONSE_WAIT_SLOTS: u32 = 10;
/// Without an acknowledgement within this time the route is assumed broken: it
/// is forgotten, and the message is sent again with a new path request (ms).
pub const DEFAULT_ACK_TIMEOUT_MS: u32 = 20_000;
/// Number of times the sender sends a message again.
pub const E2E_RETRIES: u8 = 2;
/// A mobile node does not trust routes older than this (ms).
/// Routes to or from a moving node are looked up afresh after this long.
/// maarse-sim (E45 car, 8 seeds): 30 s gave 71 % delivered (a path request
/// for nearly every message), no refresh 80 %, 2 min 84 %, 5 min 93 %.
pub const MOBILE_ROUTE_MS: u64 = 300_000;
/// Wait before an unanswered path request is repeated (ms).
///
/// The reply can take a long time: over many hops, where every hop can
/// be retransmitted several times with `DEFAULT_HOP_TIMEOUT_MS` in between. If the
/// request is repeated too early, the net is flooded again while the reply is still on
/// its way, and the new flood drowns the reply. In the simulator this was the main cause
/// of lost acknowledgements in large, sparse nets (15 s gave 50 % acknowledged, 45 s
/// gave over 80 %).
pub const PATH_RETRY_MS: u64 = 20_000;
/// Default share of path requests that are forwarded (per mille).
pub const REQUEST_FWD_PERMILLE: u16 = 800;
/// ... and only by a node that has at least this many neighbours (see `request_fwd_permille`).
pub const REQUEST_FWD_MIN_NEIGHBORS: usize = 3;
/// Number of repetitions of a path request.
pub const PATH_RETRIES: u8 = 2;
const MAX_FORWARDS: usize = 32;
const MAX_FLOOD_PENDING: usize = 16;
const MAX_QUEUE: usize = 8;
const MAX_AWAIT: usize = 32;
const MAX_CHANNELS: usize = 8;
const MAX_NEIGHBORS: usize = 32;
/// Destinations without coverage an edge bridge remembers (boundary mode).
const MAX_UNCOVERED: usize = 64;
/// Edge bridges a node remembers from their announces.
const MAX_BRIDGES: usize = 8;
/// Trickle timer for hellos (RFC 6206): the shortest interval, used right
/// after something changed, and the longest per kind of node. Repeaters stay
/// within `EDGE_WINDOW_MS` so "hears a repeater" stays true between hellos;
/// covered clients, most nodes in a town, say hello least often.
pub const HELLO_MIN_MS: u64 = 10_000;
pub const HELLO_MAX_REPEATER_MS: u64 = 4 * 60_000;
pub const HELLO_MAX_MS: u64 = 10 * 60_000;
pub const HELLO_MAX_CLIENT_MS: u64 = 30 * 60_000;
/// How long a neighbor's hello information (role, bridge choice) counts
/// for the bridge rules: a little over the longest hello interval.
pub const INFO_WINDOW_MS: u64 = 35 * 60_000;
/// Mobile destinations a node remembers (routes to them go stale fast).
const MAX_MOBILE: usize = 16;
/// How long a dropped inward path request is remembered: a second request
/// for the same destination within this time is let through.
const INWARD_REPEAT_MS: u64 = 120_000;
/// How long a bridge's announce keeps it a candidate for our choice (ms);
/// bridges announce every 10 minutes (see `BRIDGE_ANNOUNCE_S`).
pub const BRIDGE_WINDOW_MS: u64 = 30 * 60_000;
/// Announce interval of a node while it is an edge bridge (s).
pub const BRIDGE_ANNOUNCE_S: u32 = 600;
/// How long role, forwarding and coverage observations stay valid for the
/// edge bridge rule (ms).
pub const EDGE_WINDOW_MS: u64 = 300_000;
/// Messages a node carries at most (see "Carry").
pub const CARRY_MAX: usize = 8;
/// Room for carried messages (`carry_max` is the limit in use).
#[cfg(not(feature = "big-carry"))]
pub const CARRY_CAP: usize = CARRY_MAX;
/// Room for carried messages (`carry_max` is the limit in use).
#[cfg(feature = "big-carry")]
pub const CARRY_CAP: usize = 128;
/// How long a carried copy lives.
pub const CARRY_TTL_MS: u64 = 30 * 60 * 1000;
/// How often the router looks for neighbours to hand carried messages to.
const CARRY_EVERY_MS: u64 = 5_000;
/// A neighbour heard this recently counts as "here" for carrying.
const CARRY_FRESH_MS: u64 = 30_000;
/// Wait this long before handing the same message to the destination again.
const CARRY_RETRY_MS: u64 = 60_000;
/// Delivered notices a node remembers (see "Delivered notice").
const CARRY_DONE_MAX: usize = 16;

/// A carried message we know has been delivered.
#[derive(Clone)]
struct CarryDone {
    id: u64,
    until_ms: u64,
    /// Neighbours that may still hold a copy (we gave them one, or got ours
    /// from them): they get the notice when we meet them.
    holders: heapless::Vec<PathId, 8>,
    /// Last time we sent the notice.
    sent_ms: u64,
    /// The recipient's proof, passed on with the notice.
    proof: Option<Proof>,
}

/// A message we carry for someone else (or for ourselves, as its sender).
#[derive(Clone)]
struct Carried {
    dest: Address,
    payload: Buf,
    id: u64,
    tokens: u8,
    until_ms: u64,
    /// Neighbours that got a copy from us (or gave us ours).
    given: heapless::Vec<PathId, 8>,
    /// Last time we handed it to the destination (0 = never).
    tried_ms: u64,
    /// Last time we tried ordinary routing again near a repeater (0 = never).
    routed_ms: u64,
    /// We sent this message ourselves (only the sender tries routing again).
    own: bool,
}

/// A carrier near a repeater tries ordinary routing again this often: the
/// destination may be reachable through the repeater network by now (a car
/// out of a coverage gap, a base station waiting for it).
const CARRY_REROUTE_MS: u64 = 300_000;
/// Smart carry: a moving carrier tries routing a message again at most this often.
const CARRY_ARRIVE_MS: u64 = 600_000;
const MAX_TRACES: usize = 4;
/// How many legs an anchor remembers at a time (requests and messages).
const MAX_ANCHORED: usize = 16;
/// How long a leg is remembered by the anchor (ms): long enough for reply and
/// acknowledgement, even over many hops.
pub const ANCHOR_TTL_MS: u64 = 120_000;
/// How fresh a route's last acknowledgement must be for an anchor to answer
/// a path request with it.
pub const PROXY_ROUTE_MS: u64 = 300_000;
/// How far announces are forwarded by default. Nodes further away get a key
/// with a key request (answered by hosts' caches too) when they need it.
/// maarse-sim (regression suite): against 8 hops, 20 % less airtime in the
/// national scenario and 22 % on the outage journey, 5-8 points more
/// delivered in the town and on the journey, the car unchanged (8 seeds).
/// Announces from mobile nodes keep `LOCAL_SCOPE`.
pub const ANNOUNCE_SCOPE: u8 = 3;
/// Expanding ring search: the first path request goes at most `ring` hops
/// (marked in its reply token: byte 12 = ring, bytes 13-14 = "RG"; older
/// nodes ignore it) and is repeated without a limit after this long.
pub const RING_RETRY_MS: u64 = 10_000;
const RING_MARK: [u8; 2] = *b"RG";
/// Next hops we remember as dead (see "Local repair").
const MAX_DEAD_HOPS: usize = 8;
/// How long a next hop counts as dead, unless we hear it again.
const DEAD_HOP_MS: u64 = 300_000;
/// A node further along the path must have been heard this recently to be
/// used for local repair.
const BYPASS_FRESH_MS: u64 = 120_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Mobile node, or node on battery without charging. Does not forward.
    Client,
    /// Stationary with a positive energy budget (mains power or solar).
    Repeater,
    /// Handheld node in group mode ("forest mode") when there are no
    /// repeaters. Forwards like a repeater, but waits longer (so
    /// real repeaters win) and only within `relay_hops`.
    Relay,
    /// Fixed repeater that also splits paths over [`MAX_PATH`] hops into
    /// legs (see the module documentation). Used e.g. along motorways.
    Anchor,
}

impl Role {
    /// The role as a number on the wire/in flash (0 = client, 1 = repeater, 2 = relay,
    /// 3 = anchor).
    pub fn as_u8(self) -> u8 {
        match self {
            Role::Client => 0,
            Role::Repeater => 1,
            Role::Relay => 2,
            Role::Anchor => 3,
        }
    }

    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Role::Client),
            1 => Some(Role::Repeater),
            2 => Some(Role::Relay),
            3 => Some(Role::Anchor),
            _ => None,
        }
    }

    /// Does the node forward for others (repeater or relay)?
    pub fn forwards(self) -> bool {
        self != Role::Client
    }
}

/// Energy status, announced in announces and affecting routing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Energy {
    /// Forwards nothing; the node sleeps until it is charged.
    Critical = 0,
    /// Only forwards direct messages and path requests.
    Low = 1,
    Ok = 2,
    Full = 3,
}

impl Energy {
    /// Extra wait in floods, so routes avoid repeaters with low battery.
    fn penalty_ms(self) -> u32 {
        match self {
            Energy::Full => 0,
            Energy::Ok => 150,
            Energy::Low => 800,
            Energy::Critical => 3000,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Send the packet after `delay_ms`. The layer below handles duty cycle and CAD.
    /// `id` is the packet's ID, so a later `Cancel` can find it.
    Transmit { packet: Buf, delay_ms: u32, id: u64 },
    /// Cancel a planned forward of a flood because the neighbours
    /// have already covered the area. If it has been sent, this is ignored.
    Cancel { id: u64 },
    /// A packet for us or for a channel we listen on.
    Deliver {
        ptype: PacketType,
        dest: Address,
        payload: Buf,
        hops: u8,
    },
    /// We have found a route.
    RouteLearned { dest: Address, hops: u8 },
    /// A direct message has been acknowledged by the recipient.
    Acked { msg_id: u64 },
    /// A message we carried has reached its destination ("delivered
    /// (carried)"): the delivered notice came back to us. `msg_id` is the
    /// id `send_direct` returned.
    CarryAcked { msg_id: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendError {
    TooLarge,
    /// No known route (send a direct message first).
    NoRoute,
    QueueFull,
    TooManyChannels,
}

struct PendingRequest {
    token: Address,
    /// Tokens from earlier attempts: a late reply to them still counts.
    old_tokens: heapless::Vec<Address, 2>,
    target: Address,
    /// Message that was sent along in the request (like MeshCore: the first
    /// message is flooded, and the reply is both route and acknowledgement).
    carried: Option<u64>,
    /// This attempt was a ring-limited search (the next is not, and does not
    /// count as a retry).
    ringed: bool,
    /// When the request is repeated if there is no reply.
    retry_at_ms: u64,
    tries: u8,
    /// False while the request waits for the path request quota.
    sent: bool,
}

struct Queued {
    ptype: PacketType,
    dest: Address,
    payload: Buf,
}

/// A reply to a path request that waits for more copies to
/// arrive, so the best trail can be chosen.
struct PendingReply {
    token: Address,
    best: Path,
    best_snr: i8,
    /// Hops of the best copy (with weak-hop penalties).
    best_hops: u8,
    reply_at_ms: u64,
    /// The message in the request that the app must open before the reply.
    message: Option<u64>,
}

/// A leg an anchor remembers: replies to `key` (reply token or
/// ack token) are sent back along `back`. `target` is the target of a
/// path request (otherwise unused).
struct Anchored {
    key: Address,
    target: Address,
    back: Path,
    until_ms: u64,
    /// A message we forwarded along our own route (not a request leg): if
    /// no ack comes back before `until_ms`, that route is dropped.
    forward: bool,
}

struct Owed {
    id: u64,
    /// The way back, for a message that came along a source route.
    back: Option<Path>,
    /// It was handed over by a carrier, which waits for the delivered notice.
    carried: bool,
    /// The app has opened it (or asked for a plain acknowledgement).
    answered: bool,
    proof: Option<Proof>,
}

struct AwaitAck {
    token: Address,
    msg_id: u64,
    ptype: PacketType,
    dest: Address,
    payload: Buf,
    sent_ms: u64,
    tries: u8,
}

/// A source-routed packet we have sent, where we listen for the next
/// repeater to forward it (implicit per-hop acknowledgement).
struct Forward {
    id: u64,
    hops: u8,
    packet: Buf,
    deadline_ms: u64,
    tries: u8,
}

/// Limit for a weak link at a given spreading factor: the receiver's
/// sensitivity limit plus a 5 dB margin.
pub fn weak_snr_for_sf(sf: u8) -> i8 {
    let floor = match sf {
        5 | 6 => -5,
        7 => -8,
        8 => -10,
        9 => -13,
        10 => -15,
        11 => -18,
        _ => -20,
    };
    floor + 5
}

/// Length of a delivery proof (see "Delivery proof").
pub const PROOF_LEN: usize = 8;
/// A delivery proof.
pub type Proof = [u8; PROOF_LEN];
/// Messages we know the proof for and still wait for.
const MAX_EXPECT: usize = 16;
/// Messages to us that the app has not answered, or may be asked about again.
const MAX_OWED: usize = 8;

/// The token the recipient sends the acknowledgement to.
pub fn ack_token(msg_id: u64) -> Address {
    let mut b = [0u8; 16];
    b[..8].copy_from_slice(&msg_id.to_le_bytes());
    b[8..].copy_from_slice(&(!msg_id).to_le_bytes());
    Address(b)
}

fn reversed(path: &Path) -> Path {
    let mut p = Path::new();
    for id in path.iter().rev() {
        let _ = p.push(*id);
    }
    p
}

/// Counters for the status page.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RouterStats {
    /// Valid packets received.
    pub rx_packets: u32,
    /// Packets that could not be decoded.
    pub rx_invalid: u32,
    /// Copies of packets we had already seen.
    pub rx_duplicates: u32,
    /// Packets we have forwarded for others.
    pub forwarded: u32,
    /// Packets we have originated ourselves.
    pub originated: u32,
    /// Packets delivered to us (or a channel we listen on).
    pub delivered: u32,
    /// Acknowledgements received for our messages.
    pub acked: u32,
    pub routes_learned: u32,
    pub path_requests: u32,
    /// Hops retransmitted because the next repeater was not heard.
    pub hop_retries: u32,
    /// Carry packets sent and messages delivered to us by carrying.
    pub carry_handoffs: u32,
    pub carry_delivered: u32,
    /// Direct messages left unacknowledged because of `hold_direct`.
    pub held_direct: u32,
    /// Smart carry: carried messages a moving carrier tried to route on arrival.
    pub carry_rerouted: u32,
    /// Delivered notices sent (see "Delivered notice").
    pub carry_notices: u32,
    /// Flood transmissions cancelled because neighbours had covered the area.
    pub floods_cancelled: u32,
    /// Announces and path requests dropped because the proof of work
    /// (the stamp) was missing or wrong (see [`crate::stamp`]).
    pub bad_stamps: u32,
    /// Source-routed packets sent past a next hop that stopped answering
    /// (see "Local repair").
    pub local_repairs: u32,
    /// Path requests we did not forward because the quota was used up.
    pub quota_dropped: u32,
    /// Own path requests held back to the next minute by the quota.
    pub quota_held: u32,
    /// Hellos sent.
    pub hellos: u32,
    /// Key requests sent.
    pub key_requests: u32,
    /// Path requests an anchor answered for the target (`proxy_reply`).
    pub proxy_replies: u32,
}

/// Routing engine with `R` routes and a duplicate filter of `D` packets.
pub struct Router<const R: usize, const D: usize> {
    pub addr: Address,
    pub id: PathId,
    pub role: Role,
    pub energy: Energy,
    routes: RouteTable<R>,
    dedup: DedupCache<D>,
    pending: heapless::Vec<PendingRequest, MAX_PENDING>,
    queue: heapless::Vec<Queued, MAX_QUEUE>,
    awaiting: heapless::Vec<AwaitAck, MAX_AWAIT>,
    /// Proofs expected for messages we sent (oldest first).
    expected: heapless::Vec<(u64, Proof), MAX_EXPECT>,
    /// Messages to us waiting for the app (see [`Router::app_acks`]).
    owed: heapless::Vec<Owed, MAX_OWED>,
    /// The app acknowledges direct messages with [`Router::acknowledge`].
    pub app_acks: bool,
    channels: heapless::Vec<Address, MAX_CHANNELS>,
    forwards: heapless::Vec<Forward, MAX_FORWARDS>,
    replies: heapless::Vec<PendingReply, 4>,
    /// Floods we have planned to forward, and how many copies we have heard.
    flood_pending: heapless::Vec<(u64, u8), MAX_FLOOD_PENDING>,
    /// Tidsslot for flood-ventetid (ms).
    pub slot_ms: u32,
    /// Wait for the implicit per-hop acknowledgement (ms).
    pub hop_timeout_ms: u32,
    /// Cancel our own forward of a flood after this many heard copies
    /// with good SNR (see `on_duplicate_flood`).
    pub suppress_after: u8,
    /// Extra wait before we forward a flood (ms). Used by a bridge,
    /// so a repeater's copy arrives first if the repeater sends itself.
    pub extra_flood_delay_ms: u32,
    /// Copies per message for store-carry-forward; 0 = off (see "Carry").
    pub carry_copies: u8,
    carried: heapless::Vec<Carried, CARRY_CAP>,
    /// Messages carried at most (≤ [`CARRY_CAP`]; default [`CARRY_MAX`]).
    pub carry_max: usize,
    /// Lifetime of a copy we start carrying (default [`CARRY_TTL_MS`]).
    pub carry_ttl_ms: u64,
    /// Smart carry (experimental, off by default): copies go only to
    /// neighbours that move; a carrier that moves tries ordinary routing
    /// when it meets new neighbours (at most every [`CARRY_ARRIVE_MS`] per
    /// message); the sender re-routes its own carried messages only when it
    /// regains a repeater, not every [`CARRY_REROUTE_MS`].
    pub carry_smart: bool,
    next_carry_ms: u64,
    /// Neighbours seen at the previous carry step: handoffs only go to new
    /// encounters, not to the same neighbours again and again.
    carry_seen: heapless::Vec<PathId, 32>,
    /// New neighbours at the last carry step (smart carry: "arrived").
    carry_seen_new: usize,
    /// We heard a repeater at the previous carry step.
    carry_near_repeater: bool,
    /// Carried messages known to be delivered.
    carry_done: heapless::Vec<CarryDone, CARRY_DONE_MAX>,
    /// Largest number of hops a relay forwards (default 4, at most 7).
    pub relay_hops: u8,
    /// Boundary forwarding for edge bridges (after Reticulum's boundary
    /// interfaces): a relay that hears a repeater forwards a path request
    /// only when it comes from a neighbor without coverage or from a
    /// repeater, i.e. across the edge of coverage, not around the covered
    /// side. Off by default; automatic forest mode turns it on.
    pub boundary: bool,
    /// How old a route to or from a moving node may be before a fresh one is
    /// looked for (`MOBILE_ROUTE_MS` by default).
    pub mobile_route_ms: u64,
    /// Nodes that announced they hear no repeater (role flags), newest
    /// last: in boundary mode a bridge forwards a path request from the
    /// repeater side into the uncovered side only for one of these, as
    /// Reticulum forwards onto a boundary interface only what belongs there.
    uncovered: heapless::Vec<Address, MAX_UNCOVERED>,
    /// Edge bridges heard announcing (ID, last heard, hops), for our own
    /// choice.
    bridges_known: heapless::Vec<(PathId, u64, u8), MAX_BRIDGES>,
    /// Send hellos (Trickle timer). Off by default.
    pub hello: bool,
    hello_iv_ms: u64,
    hello_at_ms: u64,
    hello_sig: u32,
    hello_seed: u32,
    /// Destinations that said they move (hello or announce flag), with when.
    mobile_dests: heapless::Vec<(Address, u64), MAX_MOBILE>,
    /// Boundary mode: inward path requests dropped for an unknown
    /// destination; a repeat within `INWARD_REPEAT_MS` goes through.
    inward_dropped: heapless::Vec<(Address, u64), 8>,
    key_nonce: u16,
    /// Our post office class (role flags bits 4-5; 0 = not a post office).
    pub mail_class: u8,
    /// Post office: recipients (path IDs) with mail waiting, told in our
    /// hello so they ask for it ("mail waiting", like a Wi-Fi beacon's TIM).
    pub mail_waiting: heapless::Vec<PathId, 4>,
    /// The time, told and learned in hellos (see [`crate::clock`]).
    pub clock: crate::clock::Clock,
    /// A post office's hello named us: we have mail waiting.
    mail_hint: bool,
    /// Wait for the end-to-end acknowledgement before the route is repaired (ms).
    pub ack_timeout_ms: u32,
    /// The node moves (e.g. in a car): routes go stale quickly.
    pub mobile: bool,
    /// A last hop with SNR below this counts as weak. Use
    /// [`weak_snr_for_sf`] for the chosen spreading factor.
    pub weak_snr_db: i8,
    /// Regions the node forwards floods for (see [`crate::region`]).
    pub regions: crate::region::Regions,
    /// With an empty region list: only forward floods without a region.
    pub strict_region: bool,
    pub stats: RouterStats,
    /// Direct neighbours (see [`crate::diag`]).
    pub neighbors: crate::diag::NeighborTable<MAX_NEIGHBORS>,
    /// RSSI (dBm) of the packet being handled now; set by the radio layer before
    /// `on_receive` (0 = unknown).
    pub rx_rssi: i16,
    /// Trace routes: (target, token, sent). Waits for a route until it is sent.
    traces: heapless::Vec<(Address, Address, bool), MAX_TRACES>,
    /// Legs we remember as an anchor (see [`Role::Anchor`]).
    anchored: heapless::Vec<Anchored, MAX_ANCHORED>,
    /// Next hops that stopped answering: (id, until). Packets for them are
    /// sent past them at once (see "Local repair").
    dead_hops: heapless::Vec<(PathId, u64), MAX_DEAD_HOPS>,
    /// Send past next hops that stop answering (see "Local repair"). On by
    /// default; the simulator turns it off for comparison.
    pub local_repair: bool,
    /// Wait before an unanswered path request is repeated (ms).
    pub path_retry_ms: u32,
    /// How long an anchor remembers a leg (default `ANCHOR_TTL_MS`). Hosts
    /// use longer: their legs may cross LoRa trunks with queues.
    pub anchor_ttl_ms: u64,
    /// An anchor answers a path request itself when it has a fresh route to
    /// the target (younger than `PROXY_ROUTE_MS`), instead of flooding it on
    /// (as AODV lets an intermediate node with a fresh route reply). The
    /// requester then sends to us and we forward along our route. Off by
    /// default; hosts at trunk ends use it, so a request does not cross the
    /// water again for a node we reached a moment ago.
    pub proxy_reply: bool,
    /// Expanding ring search (as in AODV): the first path request for a
    /// target goes at most this many hops; 0 = off.
    pub ring: u8,
    /// How far announces are forwarded (hops). Nodes further away ask for a
    /// key with a key request when they need it.
    pub announce_scope: u8,
    /// Share of path requests a forwarder passes on, in per mille (1000 or
    /// 0 = all), decided from the packet id and our id. 80 % cut airtime by
    /// 1-39 % in the simulator with the same or better delivery (docs/maybe/airtime-optimisation.md).
    pub request_fwd_permille: u16,
    /// A hop heard with a weak signal counts as two when the destination
    /// picks among trails (a cheap stand-in for ETX: weak links cost
    /// retransmissions and lost acks).
    pub weak_hop_penalty: bool,
    /// How far away nodes were heard lately (announces, hellos): the ring
    /// search is used only for a target heard within the ring, the way AODV
    /// starts from the last known hop count.
    heard_hops: heapless::Vec<(Address, u8, u64), 64>,
    /// An anchor drops its route to a target when a message it forwarded
    /// there was not acknowledged (the target moved or went away).
    pub drop_unacked_routes: bool,
    /// Load control: path request quota and longer flood delays when the
    /// measured channel load is high (see [`crate::qos::ChannelLoad`]).
    pub load_control: bool,
    /// Nobody reads our messages right now (a companion node whose phone is
    /// away): direct messages to us are neither delivered nor acknowledged,
    /// so the sender keeps them (retries, post office, carry) instead of
    /// seeing "delivered" for a message the phone never gets.
    pub hold_direct: bool,
    load: crate::qos::ChannelLoad,
    load_high: bool,
    path_quota: crate::qos::PathQuota,
}

impl<const R: usize, const D: usize> Router<R, D> {
    /// Changes role at run time (e.g. after `SetRole` from the app).
    pub fn set_role(&mut self, role: Role) {
        self.role = role;
    }

    /// Changes the node's address (new identity, see `Command::SetIdentity`).
    /// Routes, pending sends and acknowledgements belonged to the old
    /// address and are forgotten; channels, regions, role and neighbours are kept.
    pub fn set_address(&mut self, addr: Address) {
        self.addr = addr;
        self.id = addr.path_id();
        self.routes = RouteTable::new();
        self.pending.clear();
        self.queue.clear();
        self.awaiting.clear();
        self.forwards.clear();
        self.replies.clear();
        self.flood_pending.clear();
        self.traces.clear();
    }

    pub fn new(addr: Address, role: Role) -> Self {
        Router {
            addr,
            id: addr.path_id(),
            role,
            energy: Energy::Full,
            routes: RouteTable::new(),
            dedup: DedupCache::new(),
            pending: heapless::Vec::new(),
            queue: heapless::Vec::new(),
            awaiting: heapless::Vec::new(),
            expected: heapless::Vec::new(),
            owed: heapless::Vec::new(),
            app_acks: false,
            channels: heapless::Vec::new(),
            forwards: heapless::Vec::new(),
            replies: heapless::Vec::new(),
            flood_pending: heapless::Vec::new(),
            slot_ms: DEFAULT_SLOT_MS,
            hop_timeout_ms: DEFAULT_HOP_TIMEOUT_MS,
            suppress_after: 1,
            extra_flood_delay_ms: 0,
            carry_copies: 0,
            carry_max: CARRY_MAX,
            carry_ttl_ms: CARRY_TTL_MS,
            carry_smart: false,
            carried: heapless::Vec::new(),
            next_carry_ms: 0,
            carry_seen: heapless::Vec::new(),
            carry_seen_new: 0,
            carry_near_repeater: false,
            carry_done: heapless::Vec::new(),
            relay_hops: DEFAULT_RELAY_HOPS,
            boundary: false,
            mobile_route_ms: MOBILE_ROUTE_MS,
            uncovered: heapless::Vec::new(),
            bridges_known: heapless::Vec::new(),
            hello: false,
            hello_iv_ms: HELLO_MIN_MS,
            hello_at_ms: 0,
            hello_sig: u32::MAX,
            hello_seed: 0,
            mobile_dests: heapless::Vec::new(),
            inward_dropped: heapless::Vec::new(),
            key_nonce: 0,
            mail_class: 0,
            mail_waiting: heapless::Vec::new(),
            clock: crate::clock::Clock::default(),
            mail_hint: false,
            ack_timeout_ms: DEFAULT_ACK_TIMEOUT_MS,
            mobile: false,
            weak_snr_db: weak_snr_for_sf(9),
            regions: crate::region::Regions::new(),
            strict_region: false,
            stats: RouterStats::default(),
            neighbors: crate::diag::NeighborTable::new(),
            dead_hops: heapless::Vec::new(),
            local_repair: true,
            path_retry_ms: PATH_RETRY_MS as u32,
            anchor_ttl_ms: ANCHOR_TTL_MS,
            proxy_reply: false,
            ring: 0,
            announce_scope: ANNOUNCE_SCOPE,
            request_fwd_permille: REQUEST_FWD_PERMILLE,
            weak_hop_penalty: false,
            heard_hops: heapless::Vec::new(),
            drop_unacked_routes: false,
            load_control: false,
            hold_direct: false,
            load: crate::qos::ChannelLoad::new(),
            load_high: false,
            path_quota: crate::qos::PathQuota::new(),
            rx_rssi: 0,
            traces: heapless::Vec::new(),
            anchored: heapless::Vec::new(),
        }
    }

    pub fn routes(&self) -> &RouteTable<R> {
        &self.routes
    }

    pub fn subscribe(&mut self, channel: Address) -> Result<(), SendError> {
        if self.channels.contains(&channel) {
            return Ok(());
        }
        self.channels
            .push(channel)
            .map_err(|_| SendError::TooManyChannels)
    }

    /// Forgets a route, e.g. after several lost acknowledgements.
    pub fn forget_route(&mut self, dest: &Address) {
        self.routes.remove(dest);
    }

    /// Wait before a repeater forwards a flood.
    ///
    /// High SNR (close by) gives a long wait, so distant repeaters send first.
    ///
    /// SNR is split into 8 steps of 5 dB; each step is one time slot. The layer below should
    /// add a random wait of 0..slot on top, so repeaters in the same
    /// step do not send at the same time.
    pub fn flood_delay_ms(&self, snr_db: i8) -> u32 {
        let bucket = ((snr_db as i32).clamp(-20, 19) + 20) / 5; // 0..=7
        let relay = if self.role == Role::Relay {
            2 * self.slot_ms
        } else {
            0
        };
        let base = bucket as u32 * self.slot_ms + self.energy.penalty_ms() + relay + self.extra_flood_delay_ms;
        // Under high load floods wait twice as long, so more copies are heard
        // (and suppressed) before we send.
        if self.load_control && self.load_high { base * 2 } else { base }
    }

    /// Sets the flood slot; with `scale_timeouts` the ack timeout, the path
    /// request retry interval and the hop timeout grow with it
    /// (`base * slot / 200`, never below today's values).
    pub fn set_slot(&mut self, slot_ms: u32, scale_timeouts: bool) {
        self.slot_ms = slot_ms;
        let s = if scale_timeouts { slot_ms } else { crate::qos::BASE_SLOT_MS };
        self.ack_timeout_ms = crate::qos::scale_timeout(DEFAULT_ACK_TIMEOUT_MS, s);
        self.path_retry_ms = crate::qos::scale_timeout(PATH_RETRY_MS as u32, s);
        self.hop_timeout_ms = crate::qos::scale_timeout(DEFAULT_HOP_TIMEOUT_MS, s);
    }

    /// Records channel use (a packet sent or heard) for load control.
    pub fn note_airtime(&mut self, now_ms: u64, airtime_ms: u32) {
        self.load.add(now_ms, airtime_ms);
        self.refresh_load(now_ms);
    }

    /// Measured channel load over the last minute (per mille).
    pub fn load_permille(&mut self, now_ms: u64) -> u16 {
        self.load.permille(now_ms)
    }

    fn refresh_load(&mut self, now_ms: u64) {
        self.load_high = self.load.permille(now_ms) >= crate::qos::LOAD_HIGH_PERMILLE;
    }

    /// May we forward a flood in `region` (0 = none)?
    pub fn forwards_region(&self, region: u16) -> bool {
        crate::region::forwards(&self.regions, self.strict_region, region)
    }

    /// May we forward a packet that has taken `hops` hops?
    fn may_forward(&self, hops: u8) -> bool {
        match self.role {
            Role::Client => false,
            Role::Repeater | Role::Anchor => self.energy != Energy::Critical,
            Role::Relay => self.energy != Energy::Critical && hops < self.relay_hops.min(MAX_RELAY_HOPS),
        }
    }

    fn emit(&mut self, header: &Header, payload: &[u8], delay_ms: u32, out: &mut Actions) {
        let Ok(packet) = encode(header, payload) else {
            return;
        };
        let id = packet_id(header.ptype, &header.dest, payload);
        // Our own packets must not be handled again when neighbours repeat them.
        self.dedup.insert(id);
        let _ = out.push(Action::Transmit {
            packet,
            delay_ms,
            id,
        });
    }

    /// Sends a source-routed packet and remembers it, so it can be retransmitted if
    /// we do not hear the next repeater forward it.
    fn emit_routed(
        &mut self,
        header: &Header,
        payload: &[u8],
        delay_ms: u32,
        now_ms: u64,
        out: &mut Actions,
    ) {
        self.emit(header, payload, delay_ms, out);
        let next_exists = header
            .path
            .as_ref()
            .is_some_and(|p| (header.hops as usize) < p.len());
        if !next_exists {
            return; // Last hop: covered by the end-to-end acknowledgement.
        }
        let Ok(packet) = encode(header, payload) else {
            return;
        };
        if self.forwards.is_full() {
            self.forwards.remove(0);
        }
        let _ = self.forwards.push(Forward {
            id: packet_id(header.ptype, &header.dest, payload),
            hops: header.hops,
            packet,
            deadline_ms: now_ms + delay_ms as u64 + self.hop_timeout_ms as u64,
            tries: 0,
        });
    }

    /// Next hops remembered as dead (see "Local repair"): (id, until ms).
    /// For diagnostics (the simulator's node inspector).
    pub fn dead_hops(&self) -> &[(PathId, u64)] {
        &self.dead_hops
    }

    /// Is the next hop of this (outgoing) header one we remember as dead?
    fn is_dead_hop(&self, header: &Header, now_ms: u64) -> bool {
        let Some(next) = header.path.as_ref().and_then(|p| p.get(header.hops as usize)) else {
            return false;
        };
        self.dead_hops.iter().any(|(d, until)| d == next && *until > now_ms)
    }

    /// A header that skips the next hop: the path continues at the farthest
    /// node on it we have heard recently, or ends here if we hear the
    /// destination itself. `None` if no such node is in range.
    fn bypass_header(&self, header: &Header, now_ms: u64) -> Option<Header> {
        if !self.local_repair {
            return None;
        }
        let path = header.path.as_ref()?;
        let idx = header.hops as usize;
        if idx >= path.len() {
            return None;
        }
        let fresh = |n: &crate::diag::Neighbor| n.last_ms + BYPASS_FRESH_MS > now_ms;
        let mut new_path = Path::new();
        // The destination itself is in range: no more repeaters needed.
        let dest_near = self.neighbors.iter().any(|n| n.addr == Some(header.dest) && fresh(n));
        let skip_to = if dest_near {
            path.len()
        } else {
            (idx + 1..path.len())
                .rev()
                .find(|&j| path[j] != self.id && self.neighbors.get(path[j]).is_some_and(fresh))?
        };
        for (j, id) in path.iter().enumerate() {
            if j < idx || j >= skip_to {
                let _ = new_path.push(*id);
            }
        }
        let mut h = header.clone();
        h.path = Some(new_path);
        Some(h)
    }

    /// Local repair: the next hop did not forward a source-routed packet after
    /// all retries. Instead of leaving it to the sender (end-to-end timeout and
    /// a new path request flooded from the start), send it past the dead hop
    /// to a node further along the path that we can hear. The dead hop is
    /// remembered for a while, so later packets skip it at once.
    fn local_repair(&mut self, packet: &[u8], now_ms: u64, out: &mut Actions) {
        let Ok(pkt) = Packet::decode(packet) else {
            return;
        };
        let header = pkt.header.clone();
        if let Some(dead) = header.path.as_ref().and_then(|p| p.get(header.hops as usize)).copied() {
            self.dead_hops.retain(|(d, until)| *d != dead && *until > now_ms);
            if self.dead_hops.is_full() {
                self.dead_hops.remove(0);
            }
            let _ = self.dead_hops.push((dead, now_ms + DEAD_HOP_MS));
        }
        let Some(bypass) = self.bypass_header(&header, now_ms) else {
            return;
        };
        self.stats.local_repairs += 1;
        let payload: Buf = Buf::from_slice(pkt.payload).unwrap_or_default();
        self.emit_routed(&bypass, &payload, 0, now_ms, out);
    }

    /// Forwards a flood later and remembers it, so we can cancel.
    fn emit_flood(&mut self, header: &Header, payload: &[u8], delay_ms: u32, out: &mut Actions) {
        self.emit(header, payload, delay_ms, out);
        let id = packet_id(header.ptype, &header.dest, payload);
        if self.flood_pending.is_full() {
            self.flood_pending.remove(0);
        }
        let _ = self.flood_pending.push((id, 0));
    }

    /// A copy of a flood we have already seen. If we planned to
    /// forward it ourselves, and neighbours have already done so, ours is cancelled.
    ///
    /// Only copies heard with good SNR count: they come from a nearby neighbour whose
    /// transmission covers roughly the same area as ours. A weak copy
    /// comes from far away and covers a different area, so it is no reason
    /// to stay quiet. That is why one strong copy is enough (`suppress_after` = 1). Counting
    /// all copies, the flood would die out on long chains (motorway), and
    /// the routes found would consist only of weak links.
    fn on_duplicate_flood(&mut self, id: u64, snr_db: i8, prev: Option<PathId>, now_ms: u64, out: &mut Actions) {
        if snr_db < self.weak_snr_db {
            return;
        }
        // Edge bridge: a copy from a non-repeater does not prove that the
        // repeater side is covered, so it is no reason to stay silent.
        if self.is_edge_bridge(now_ms) && !prev.is_some_and(|p| self.is_repeater_neighbor(p, now_ms)) {
            return;
        }
        if let Some(i) = self.flood_pending.iter().position(|(x, _)| *x == id) {
            self.flood_pending[i].1 += 1;
            if self.flood_pending[i].1 >= self.suppress_after {
                self.flood_pending.remove(i);
                let _ = out.push(Action::Cancel { id });
            }
        }
    }

    /// Records a neighbor's role from its announce (role field in app-data).
    /// Only direct (0-hop) announces should be passed in. The router has
    /// normally recorded the neighbor already (with SNR) when the announce
    /// arrived; otherwise it is added with the given values.
    pub fn note_role(
        &mut self,
        id: PathId,
        role: Role,
        hears_repeater: Option<bool>,
        rssi: i16,
        snr_db: i8,
        now_ms: u64,
    ) {
        if id == self.id {
            return;
        }
        if self.neighbors.get(id).is_none() {
            self.neighbors.heard(id, None, snr_db, rssi, now_ms);
        }
        self.neighbors.set_role(id, role, hears_repeater);
    }

    /// Do we hear a repeater or anchor directly? Sent in our announces
    /// (role field flag) so neighbors can apply the edge bridge rule.
    pub fn hears_repeater(&self, now_ms: u64) -> bool {
        self.neighbors.iter().any(|n| self.is_repeater_neighbor(n.id, now_ms))
    }

    fn is_repeater_neighbor(&self, id: PathId, now_ms: u64) -> bool {
        self.neighbors.get(id).is_some_and(|n| {
            matches!(n.role, Some(Role::Repeater | Role::Anchor)) && now_ms.saturating_sub(n.last_ms) < EDGE_WINDOW_MS
        })
    }

    /// Edge bridge: we are a relay, we hear a repeater directly, and at
    /// least one recent neighbor does not. Then we are the group's way into
    /// the repeater network and must not be silenced by the group's own
    /// copies.
    ///
    /// A neighbor that reports the role flag is believed. For one that does
    /// not (older firmware), we guess: it forwards floods, and no repeater
    /// has been heard echoing it or being echoed by it.
    pub fn is_edge_bridge(&self, now_ms: u64) -> bool {
        self.role == Role::Relay && self.hears_repeater(now_ms) && self.has_uncovered_neighbor(now_ms)
    }

    /// Is a recent neighbor already an edge bridge (a relay that reports it
    /// hears a repeater)? Returns the lowest such neighbor ID, so automatic
    /// forest mode can leave the bridging to one node per stretch of the
    /// coverage edge.
    pub fn bridge_neighbor(&self, now_ms: u64) -> Option<PathId> {
        self.neighbors
            .iter()
            .filter(|n| n.role == Some(Role::Relay) && n.hears_repeater == Some(true) && now_ms.saturating_sub(n.last_ms) < INFO_WINDOW_MS)
            .map(|n| n.id)
            .min()
    }

    /// An announce said whether its sender hears a repeater (role flags):
    /// remember the ones without coverage for boundary mode.
    pub fn note_coverage(&mut self, addr: Address, hears_repeater: bool) {
        if let Some(i) = self.uncovered.iter().position(|a| *a == addr) {
            self.uncovered.remove(i);
        }
        if !hears_repeater {
            if self.uncovered.is_full() {
                self.uncovered.remove(0);
            }
            let _ = self.uncovered.push(addr);
        }
    }

    /// Our hello payload: role, flags, chosen bridge (0xFFFF = none). The
    /// bridge count (flag bits 1-2) is left out: it changes whenever a
    /// bridge nearby comes or goes and would keep resetting every
    /// neighbor's Trickle timer, and the named-bridge rule does not use it.
    pub fn hello_payload(&self, now_ms: u64) -> heapless::Vec<u8, 17> {
        self.hello_payload_with(now_ms, true)
    }

    /// The hello, with or without the time (the time is left out of the
    /// Trickle signature, or every second would count as a change).
    fn hello_payload_with(&self, now_ms: u64, time: bool) -> heapless::Vec<u8, 17> {
        let clock = time.then(|| self.clock.now(now_ms)).flatten().filter(|(_, s)| *s < crate::clock::MAX_STRATUM);
        let flags = crate::appdata::role_flags(self.hears_repeater(now_ms), 0)
            | if self.mobile { crate::appdata::ROLE_MOBILE } else { 0 }
            | if clock.is_some() { crate::appdata::HELLO_TIME } else { 0 }
            | ((self.mail_class.min(3) << crate::appdata::ROLE_MAIL_SHIFT) & crate::appdata::ROLE_MAIL_MASK);
        let [hi, lo] = self.chosen_bridge(now_ms).unwrap_or(0xFFFF).to_be_bytes();
        let mut v: heapless::Vec<u8, 17> = heapless::Vec::new();
        let _ = v.extend_from_slice(&[self.role.as_u8(), flags, hi, lo]);
        if let Some((unix, stratum)) = clock {
            let _ = v.extend_from_slice(&unix.to_be_bytes());
            let _ = v.push(stratum);
        }
        // A post office names up to 4 recipients with mail waiting.
        for id in &self.mail_waiting {
            let _ = v.extend_from_slice(&id.to_be_bytes());
        }
        v
    }

    /// What our hello says, in one number: a change resets the Trickle
    /// timer. A mobile node also counts the repeater it hears, so moving
    /// into another repeater's area is told at once (a Mobile IP binding
    /// update, in effect).
    fn hello_signature(&self, now_ms: u64) -> u32 {
        let p = self.hello_payload_with(now_ms, false);
        let p = crate::dedup::packet_id(PacketType::Hello, &self.addr, &p).to_be_bytes();
        let p = [p[0], p[1], p[2], p[3]];
        let rep = if self.mobile {
            self.neighbors.iter().filter(|n| self.is_repeater_neighbor(n.id, now_ms)).map(|n| n.id).min().unwrap_or(0)
        } else {
            0
        };
        u32::from_be_bytes(p) ^ ((rep as u32) << 7)
    }

    fn hello_max_ms(&self, now_ms: u64) -> u64 {
        match self.role {
            Role::Repeater | Role::Anchor => HELLO_MAX_REPEATER_MS,
            Role::Relay => HELLO_MAX_MS,
            Role::Client if !self.hears_repeater(now_ms) || self.mobile => HELLO_MAX_MS,
            Role::Client => HELLO_MAX_CLIENT_MS,
        }
    }

    /// Random point in [iv/2, iv) (a small LCG; good enough for spreading).
    fn hello_jitter(&mut self, iv: u64) -> u64 {
        self.hello_seed = self.hello_seed.wrapping_mul(1_103_515_245).wrapping_add(12_345 ^ self.id as u32);
        iv / 2 + (self.hello_seed >> 8) as u64 % (iv / 2).max(1)
    }

    /// Trickle: reset to the shortest interval when what we would say has
    /// changed; send when the timer is due, then double the interval.
    fn hello_step(&mut self, now_ms: u64, out: &mut Actions) {
        let sig = self.hello_signature(now_ms);
        if sig != self.hello_sig {
            self.hello_sig = sig;
            self.hello_iv_ms = HELLO_MIN_MS;
            let at = now_ms + self.hello_jitter(HELLO_MIN_MS);
            if self.hello_at_ms == 0 || self.hello_at_ms > at {
                self.hello_at_ms = at;
            }
        }
        if now_ms < self.hello_at_ms {
            return;
        }
        let payload = self.hello_payload(now_ms);
        let header = Header::new(PacketType::Hello, self.addr);
        if let Ok(packet) = encode(&header, &payload) {
            let id = packet_id(PacketType::Hello, &self.addr, &payload) ^ now_ms;
            self.stats.hellos += 1;
            let _ = out.push(Action::Transmit { packet, delay_ms: 0, id });
        }
        self.hello_iv_ms = (self.hello_iv_ms * 2).min(self.hello_max_ms(now_ms));
        let j = self.hello_jitter(self.hello_iv_ms);
        self.hello_at_ms = now_ms + j;
    }

    /// A neighbor's hello: its role, coverage, bridge choice and motion.
    fn on_hello(&mut self, pkt: &Packet, snr_db: i8, now_ms: u64) {
        let h = &pkt.header;
        let p = pkt.payload;
        if h.hops != 0 || p.len() < 4 || h.dest == self.addr {
            return;
        }
        let id = h.dest.path_id();
        self.neighbors.heard(id, Some(h.dest), snr_db, self.rx_rssi, now_ms);
        self.dead_hops.retain(|(d, _)| *d != id);
        let Some(role) = Role::from_u8(p[0]) else { return };
        let hears = p[1] & crate::appdata::ROLE_HEARS_REPEATER != 0;
        self.neighbors.set_role(id, role, Some(hears));
        self.neighbors.set_bridges(id, (p[1] & crate::appdata::ROLE_BRIDGES_MASK) >> crate::appdata::ROLE_BRIDGES_SHIFT);
        let choice = u16::from_be_bytes([p[2], p[3]]);
        self.neighbors.set_bridge_choice(id, (choice != 0xFFFF).then_some(choice));
        self.note_coverage(h.dest, hears);
        self.note_heard_hops(h.dest, 0, now_ms);
        self.note_mobile(h.dest, p[1] & crate::appdata::ROLE_MOBILE != 0, now_ms);
        self.neighbors.set_mail_class(id, (p[1] & crate::appdata::ROLE_MAIL_MASK) >> crate::appdata::ROLE_MAIL_SHIFT);
        let mut rest = &p[4..];
        if p[1] & crate::appdata::HELLO_TIME != 0 && rest.len() >= 5 {
            let unix = u32::from_be_bytes([rest[0], rest[1], rest[2], rest[3]]);
            self.clock.offer(id, unix, rest[4], now_ms);
            rest = &rest[5..];
        }
        if rest.chunks_exact(2).any(|c| u16::from_be_bytes([c[0], c[1]]) == self.id) {
            self.mail_hint = true;
        }
        if hears && role == Role::Relay {
            self.note_bridge(id, 0, now_ms);
        }
    }

    /// The best post office among our neighbors: highest class, then best
    /// SNR (its path ID, address if known, class).
    pub fn best_post_office(&self, now_ms: u64) -> Option<(PathId, Option<Address>, u8)> {
        self.neighbors
            .iter()
            .filter(|n| n.mail_class > 0 && now_ms.saturating_sub(n.last_ms) < INFO_WINDOW_MS)
            .max_by_key(|n| (n.mail_class, n.snr))
            .map(|n| (n.id, n.addr, n.mail_class))
    }

    /// Did a post office's hello name us since the last call?
    pub fn take_mail_hint(&mut self) -> bool {
        core::mem::take(&mut self.mail_hint)
    }

    /// "I am here; do you have mail for me?" (a `MailSync`, 2-hop flood)
    /// naming our home post office (it fetches our copies) and the IDs of
    /// messages we already have.
    pub fn request_mail(&mut self, home: Option<PathId>, have: &[u32]) -> Actions {
        let mut out = Actions::new();
        self.key_nonce = self.key_nonce.wrapping_add(1);
        let mut p: heapless::Vec<u8, 40> = heapless::Vec::new();
        let _ = p.extend_from_slice(&home.unwrap_or(0xFFFF).to_be_bytes());
        for id in have.iter().take(8) {
            let _ = p.extend_from_slice(&id.to_be_bytes());
        }
        let _ = p.extend_from_slice(&self.key_nonce.to_be_bytes());
        let mut p = Buf::from_slice(&p).unwrap_or_default();
        stamp::append(PacketType::MailSync, &self.addr, &mut p);
        let header = Header::new(PacketType::MailSync, self.addr);
        self.emit(&header, &p, 0, &mut out);
        out
    }

    /// `addr` was heard `hops` away (announce or hello).
    fn note_heard_hops(&mut self, addr: Address, hops: u8, now_ms: u64) {
        if let Some(e) = self.heard_hops.iter_mut().find(|e| e.0 == addr) {
            e.1 = hops;
            e.2 = now_ms;
            return;
        }
        if self.heard_hops.is_full() {
            let oldest = (0..self.heard_hops.len()).min_by_key(|&i| self.heard_hops[i].2).unwrap_or(0);
            self.heard_hops.remove(oldest);
        }
        let _ = self.heard_hops.push((addr, hops, now_ms));
    }

    /// A destination said whether it moves (hello or announce flag).
    pub fn note_mobile(&mut self, addr: Address, mobile: bool, now_ms: u64) {
        if let Some(i) = self.mobile_dests.iter().position(|(a, _)| *a == addr) {
            self.mobile_dests.remove(i);
        }
        if mobile {
            if self.mobile_dests.is_full() {
                self.mobile_dests.remove(0);
            }
            let _ = self.mobile_dests.push((addr, now_ms));
        }
    }

    /// A neighbour (by path ID) said it moves.
    fn is_mobile_neighbor(&self, id: PathId, now_ms: u64) -> bool {
        self.mobile_dests.iter().any(|(a, t)| a.path_id() == id && now_ms.saturating_sub(*t) < INFO_WINDOW_MS)
    }

    fn is_mobile_dest(&self, addr: &Address, now_ms: u64) -> bool {
        self.mobile_dests.iter().any(|(a, t)| a == addr && now_ms.saturating_sub(*t) < INFO_WINDOW_MS)
    }

    /// Asks "who is `wanted`?" (a key request, local flood). The answer is
    /// an announce, handled like any other.
    pub fn request_key(&mut self, wanted: Address) -> Actions {
        let mut out = Actions::new();
        self.key_nonce = self.key_nonce.wrapping_add(1);
        let [a, b] = self.id.to_be_bytes();
        let [c, d] = self.key_nonce.to_be_bytes();
        let header = Header::new(PacketType::KeyRequest, wanted);
        self.stats.key_requests += 1;
        let mut p = Buf::from_slice(&[a, b, c, d]).unwrap_or_default();
        stamp::append(PacketType::KeyRequest, &wanted, &mut p);
        self.emit(&header, &p, 0, &mut out);
        out
    }

    /// An edge bridge (a relay that hears a repeater) announced itself,
    /// heard `hops` away. Only bridges within the relay hop limit count:
    /// one further away serves another group.
    pub fn note_bridge(&mut self, id: PathId, hops: u8, now_ms: u64) {
        if id == self.id || hops > self.relay_hops.min(MAX_RELAY_HOPS) {
            return;
        }
        if let Some(b) = self.bridges_known.iter_mut().find(|b| b.0 == id) {
            b.1 = now_ms;
            b.2 = hops;
            return;
        }
        if self.bridges_known.is_full() {
            let oldest = (0..self.bridges_known.len()).min_by_key(|&i| self.bridges_known[i].1).unwrap_or(0);
            self.bridges_known.remove(oldest);
        }
        let _ = self.bridges_known.push((id, now_ms, hops));
    }

    /// The edge bridge this node relies on: the nearest bridge heard
    /// announcing recently, the lowest ID among equally near ones, so
    /// neighbors in a group without coverage tend to pick the same one.
    /// Sent in the role field; `None` if we hear a repeater.
    pub fn chosen_bridge(&self, now_ms: u64) -> Option<PathId> {
        if self.hears_repeater(now_ms) {
            return None;
        }
        self.bridges_known.iter().filter(|b| now_ms.saturating_sub(b.1) < BRIDGE_WINDOW_MS).min_by_key(|b| (b.2, b.0)).map(|b| b.0)
    }

    /// Recent neighbors (not repeaters or anchors) that hear a repeater
    /// themselves: the other candidates for edge bridge around us.
    pub fn covered_neighbors(&self, now_ms: u64) -> usize {
        self.neighbors
            .iter()
            .filter(|n| !matches!(n.role, Some(Role::Repeater | Role::Anchor)) && n.hears_repeater == Some(true) && now_ms.saturating_sub(n.last_ms) < INFO_WINDOW_MS)
            .count()
    }

    /// Has a recent neighbor chosen us as its edge bridge?
    pub fn bridge_named(&self, now_ms: u64) -> bool {
        self.neighbors.iter().any(|n| n.bridge_choice == Some(self.id) && now_ms.saturating_sub(n.last_ms) < INFO_WINDOW_MS)
    }

    /// Is there a recent neighbor without coverage that has no bridge yet?
    pub fn unserved_neighbor(&self, now_ms: u64) -> bool {
        self.neighbors.iter().any(|n| {
            !matches!(n.role, Some(Role::Repeater | Role::Anchor))
                && now_ms.saturating_sub(n.last_ms) < INFO_WINDOW_MS
                && n.hears_repeater == Some(false)
                && n.bridge_choice.is_none()
        })
    }

    /// How many edge bridges (relays that report hearing a repeater) we hear,
    /// sent in our role flags.
    pub fn bridges_heard(&self, now_ms: u64) -> u8 {
        self.neighbors
            .iter()
            .filter(|n| n.role == Some(Role::Relay) && n.hears_repeater == Some(true) && now_ms.saturating_sub(n.last_ms) < INFO_WINDOW_MS)
            .count()
            .min(3) as u8
    }

    /// Does a neighbor without coverage need us as its bridge? With
    /// `counted`, only a neighbor that reports fewer than `enough` bridges
    /// (a client asks for 1: none yet; a bridge for 2: it is the only one).
    pub fn needs_bridge(&self, now_ms: u64, enough: u8) -> bool {
        let fresh = |t: u64| t != 0 && now_ms.saturating_sub(t) < INFO_WINDOW_MS;
        self.neighbors.iter().any(|n| {
            !matches!(n.role, Some(Role::Repeater | Role::Anchor))
                && fresh(n.last_ms)
                && n.hears_repeater == Some(false)
                && n.bridges.is_none_or(|b| b < enough)
        })
    }

    /// Recent neighbors that forward (relays, repeaters, anchors): the
    /// density check for automatic forest mode (after Thread's
    /// router-eligible end devices, which become routers only where few
    /// routers are around).
    pub fn forwarding_neighbors(&self, now_ms: u64) -> usize {
        self.neighbors
            .iter()
            .filter(|n| matches!(n.role, Some(Role::Relay | Role::Repeater | Role::Anchor)) && now_ms.saturating_sub(n.last_ms) < EDGE_WINDOW_MS)
            .count()
    }

    /// Is there a recent neighbor (not a repeater or anchor) that does not
    /// hear a repeater itself? Independent of our own role, so automatic
    /// forest mode can keep a node at the edge of coverage a relay.
    pub fn has_uncovered_neighbor(&self, now_ms: u64) -> bool {
        let fresh = |t: u64| t != 0 && now_ms.saturating_sub(t) < EDGE_WINDOW_MS;
        self.neighbors.iter().any(|n| {
            if matches!(n.role, Some(Role::Repeater | Role::Anchor)) || !fresh(n.last_ms) {
                return false;
            }
            match n.hears_repeater {
                Some(h) => !h,
                None => fresh(n.forwarded_ms) && !fresh(n.covered_ms),
            }
        })
    }

    /// Have we already seen this packet (same ID)? Changes nothing.
    pub fn seen(&self, buf: &[u8]) -> bool {
        Packet::decode(buf).is_ok_and(|p| self.dedup.contains(packet_id(p.header.ptype, &p.header.dest, p.payload)))
    }

    /// Time of the next retransmission, if any.
    pub fn next_deadline(&self) -> Option<u64> {
        let fwd = self.forwards.iter().map(|f| f.deadline_ms);
        let req = self.pending.iter().map(|p| p.retry_at_ms);
        let t = self.ack_timeout_ms as u64;
        let ack = self.awaiting.iter().map(|a| a.sent_ms + t);
        let rep = self.replies.iter().map(|r| r.reply_at_ms);
        let carry = (!self.carried.is_empty() || !self.carry_done.is_empty()).then_some(self.next_carry_ms);
        let hello = self.hello.then_some(self.hello_at_ms);
        fwd.chain(req).chain(ack).chain(rep).chain(carry).chain(hello).min()
    }

    /// Called when `next_deadline` is reached: retransmits hops that have not
    /// been forwarded, and gives up on them after `HOP_RETRIES` attempts.
    pub fn tick(&mut self, now_ms: u64) -> Actions {
        let mut out = Actions::new();
        if self.hello {
            self.hello_step(now_ms, &mut out);
        }
        if (!self.carried.is_empty() || !self.carry_done.is_empty()) && now_ms >= self.next_carry_ms {
            self.next_carry_ms = now_ms + CARRY_EVERY_MS;
            self.carry_step(now_ms, &mut out);
        }
        // Answer path requests along the best trail we have heard.
        while let Some(i) = self.replies.iter().position(|r| r.reply_at_ms <= now_ms) {
            let r = self.replies.swap_remove(i);
            let owed = r.message.and_then(|id| self.owed.iter().find(|o| o.id == id));
            if owed.is_some_and(|o| !o.answered) {
                // The app could not open the message in the request: a reply
                // would tell an older sender that it was delivered.
                continue;
            }
            let proof = owed.and_then(|o| o.proof);
            let mut payload = Buf::new();
            let _ = payload.push(r.best.len() as u8);
            for id in &r.best {
                let _ = payload.extend_from_slice(&id.to_be_bytes());
            }
            if let Some(p) = proof {
                let _ = payload.extend_from_slice(&p);
            }
            let header = Header::new(PacketType::PathResponse, r.token).with_path(reversed(&r.best));
            self.emit_routed(&header, &payload, 0, now_ms, &mut out);
        }
        // Messages without acknowledgement: the route is probably broken (e.g. because the recipient
        // has moved). Forget it and send again with a new path request.
        let timeout = self.ack_timeout_ms as u64;
        let mut i = 0;
        while i < self.awaiting.len() {
            if self.awaiting[i].sent_ms + timeout > now_ms {
                i += 1;
                continue;
            }
            if self.awaiting[i].tries >= E2E_RETRIES {
                let a = self.awaiting.remove(i);
                if a.ptype == PacketType::Data {
                    self.start_carry(a.dest, &a.payload, now_ms);
                }
                continue;
            }
            let a = &mut self.awaiting[i];
            a.tries += 1;
            a.sent_ms = now_ms;
            let (ptype, dest, payload, msg_id, tries) =
                (a.ptype, a.dest, a.payload.clone(), a.msg_id, a.tries);
            self.routes.remove(&dest);
            self.pending.retain(|p| p.target != dest);
            let mut token = [0u8; 16];
            token[..8].copy_from_slice(&(msg_id ^ now_ms.rotate_left(17)).to_le_bytes());
            token[8] = tries;
            if let Ok((_, more)) = self.send_typed(ptype, dest, &payload, token, now_ms) {
                for m in more {
                    let _ = out.push(m);
                }
            }
            i += 1;
        }
        // Unanswered path requests are repeated with a new token, so
        // the repeaters' duplicate filter does not stop them.
        let mut i = 0;
        while i < self.pending.len() {
            if self.pending[i].retry_at_ms > now_ms {
                i += 1;
                continue;
            }
            let p = self.pending.swap_remove(i);
            if !p.sent {
                // Held back by the quota: send it now with the same token.
                self.emit_request(p.target, p.token, p.old_tokens.clone(), now_ms, p.tries, &mut out);
                continue;
            }
            if p.tries >= PATH_RETRIES {
                // Give up: the recipient cannot be found now. Messages queued for it
                // are dropped, so the queue does not fill up (the app can use the mailbox).
                // With carrying on, direct messages are carried instead.
                let mut carry: heapless::Vec<Buf, 4> = heapless::Vec::new();
                for q in self.queue.iter().filter(|q| q.dest == p.target && q.ptype == PacketType::Data) {
                    let _ = carry.push(q.payload.clone());
                }
                self.queue.retain(|q| q.dest != p.target);
                self.awaiting.retain(|a| a.dest != p.target);
                for m in carry {
                    self.start_carry(p.target, &m, now_ms);
                }
                continue;
            }
            let mut token = p.token;
            token.0[15] = token.0[15].wrapping_add(1);
            let mut old = p.old_tokens.clone();
            if old.is_full() {
                old.remove(0);
            }
            let _ = old.push(p.token);
            // A ring search that found nothing: now everywhere, and it does
            // not count as one of the retries.
            let tries = if p.ringed { p.tries } else { p.tries + 1 };
            self.emit_request(p.target, token, old, now_ms, tries, &mut out);
        }
        let mut i = 0;
        while i < self.forwards.len() {
            if self.forwards[i].deadline_ms > now_ms {
                i += 1;
                continue;
            }
            if self.forwards[i].tries >= HOP_RETRIES {
                let f = self.forwards.swap_remove(i);
                self.local_repair(&f.packet, now_ms, &mut out);
                continue;
            }
            let f = &mut self.forwards[i];
            f.tries += 1;
            self.stats.hop_retries += 1;
            // A slightly different wait for each packet and each attempt, so two
            // repeaters do not keep colliding in the same rhythm.
            let jitter = (f.id.rotate_left(f.tries as u32 * 7) % 1000) * self.hop_timeout_ms as u64 / 1000;
            f.deadline_ms = now_ms + self.hop_timeout_ms as u64 + jitter;
            let _ = out.push(Action::Transmit {
                packet: f.packet.clone(),
                delay_ms: 0,
                id: f.id,
            });
            i += 1;
        }
        out
    }

    // ---------- sending ----------

    /// Sends a direct message. If no route is known, the message is queued and
    /// a path request is sent with `req_token` (16 random bytes).
    ///
    /// Returns the message ID, which comes back later in `Action::Acked`.
    pub fn send_direct(
        &mut self,
        dest: Address,
        payload: &[u8],
        req_token: [u8; 16],
        now_ms: u64,
    ) -> Result<(u64, Actions), SendError> {
        self.send_typed(PacketType::Data, dest, payload, req_token, now_ms)
    }

    /// Like `send_direct`, but with an optional packet type (`Data`, `Fragment`,
    /// `Deposit`). Acknowledged end to end. Only `Data` can travel along in
    /// the path request.
    pub fn send_typed(
        &mut self,
        ptype: PacketType,
        dest: Address,
        payload: &[u8],
        req_token: [u8; 16],
        now_ms: u64,
    ) -> Result<(u64, Actions), SendError> {
        // Deposits carry a proof of work (post offices must not be filled for
        // free); the stamp is part of the payload the ack is computed over.
        let mut stamped = Buf::new();
        let payload = if stamp::required(ptype) {
            stamped.extend_from_slice(payload).map_err(|_| SendError::TooLarge)?;
            if !stamp::append(ptype, &dest, &mut stamped) {
                return Err(SendError::TooLarge);
            }
            &stamped[..]
        } else {
            payload
        };
        if payload.len() > MAX_DIRECT_PAYLOAD {
            return Err(SendError::TooLarge);
        }
        let msg_id = packet_id(ptype, &dest, payload);
        let mut out = Actions::new();
        if (self.mobile || self.is_mobile_dest(&dest, now_ms))
            && self
                .routes
                .get(&dest)
                .is_some_and(|r| r.learned_ms + self.mobile_route_ms < now_ms)
        {
            // We or the destination move: find a fresh route.
            self.routes.remove(&dest);
        }
        if let Some(route) = self.routes.get(&dest) {
            let path = route.path.clone();
            self.routes.touch(&dest, now_ms);
            self.send_routed_on(ptype, dest, payload, path, now_ms, &mut out);
        } else {
            let payload = Buf::from_slice(payload).map_err(|_| SendError::TooLarge)?;
            let dup = self
                .queue
                .iter()
                .any(|q| q.dest == dest && q.ptype == ptype && q.payload == payload);
            if !dup {
                self.queue
                    .push(Queued {
                        ptype,
                        dest,
                        payload,
                    })
                    .map_err(|_| SendError::QueueFull)?;
            }
            if !self.pending.iter().any(|p| p.target == dest) {
                self.request_path(dest, req_token, now_ms, &mut out);
            }
        }
        Ok((msg_id, out))
    }

    /// Sends a fragment (e.g. a chunk of a file) along a known route. It is
    /// acknowledged end to end just like a direct message.
    pub fn send_fragment(
        &mut self,
        dest: Address,
        payload: &[u8],
        now_ms: u64,
    ) -> Result<(u64, Actions), SendError> {
        if payload.len() > MAX_DIRECT_PAYLOAD {
            return Err(SendError::TooLarge);
        }
        let route = self.routes.get(&dest).ok_or(SendError::NoRoute)?;
        let path = route.path.clone();
        self.routes.touch(&dest, now_ms);
        let mut out = Actions::new();
        self.send_routed_on(PacketType::Fragment, dest, payload, path, now_ms, &mut out);
        Ok((packet_id(PacketType::Fragment, &dest, payload), out))
    }

    /// Sends a trace route to `dest` (see [`crate::diag`]). If no route is known,
    /// it is found first. The reply comes as `Action::Deliver` with
    /// `ptype = Probe`, `dest` = the target and the payload as trace reply.
    pub fn send_trace(&mut self, dest: Address, token: [u8; 16], now_ms: u64) -> Actions {
        let mut out = Actions::new();
        let token = Address(token);
        if self.traces.is_full() {
            self.traces.remove(0);
        }
        if let Some(route) = self.routes.get(&dest) {
            let path = route.path.clone();
            let _ = self.traces.push((dest, token, true));
            self.emit_trace(dest, token, path, now_ms, &mut out);
        } else {
            let _ = self.traces.push((dest, token, false));
            if !self.pending.iter().any(|p| p.target == dest) {
                self.request_path(dest, token.0, now_ms, &mut out);
            }
        }
        out
    }

    fn emit_trace(&mut self, dest: Address, token: Address, path: Path, now_ms: u64, out: &mut Actions) {
        self.stats.originated += 1;
        let header = Header::new(PacketType::Probe, dest).with_path(path);
        self.emit_routed(&header, &diag::trace_request(&token), 0, now_ms, out);
    }

    /// Remembers the neighbour that sent the packet, if that can be seen: the previous hop in
    /// a source path or a trail, or the sender of an announce with no hops.
    fn note_neighbor(&mut self, pkt: &Packet, snr_db: i8, now_ms: u64) {
        let h = &pkt.header;
        let from = match (&h.ptype, &h.path) {
            (PacketType::PathRequest, Some(p)) => p.last().map(|id| (*id, None)),
            (PacketType::PathRequest, None) => None,
            (_, Some(p)) => (h.hops as usize)
                .checked_sub(1)
                .and_then(|i| p.get(i))
                .map(|id| (*id, None)),
            (PacketType::Announce, None) if h.hops == 0 => Some((h.dest.path_id(), Some(h.dest))),
            _ => None,
        };
        if let Some((id, addr)) = from
            && id != self.id
        {
            self.neighbors.heard(id, addr, snr_db, self.rx_rssi, now_ms);
            // Heard again: no longer a dead hop.
            self.dead_hops.retain(|(d, _)| *d != id);
        }
        // Flood forwarding and coverage: the last hop forwarded this request;
        // if it is a repeater, it heard the hop before it.
        if let (PacketType::PathRequest, Some(p)) = (&h.ptype, &h.path)
            && let Some(&last) = p.last()
        {
            self.neighbors.mark_forwarded(last, now_ms);
            if p.len() >= 2 {
                let before = p[p.len() - 2];
                if self.is_repeater_neighbor(last, now_ms) {
                    // A repeater forwarded what `before` sent: it hears `before`.
                    self.neighbors.mark_covered(before, now_ms);
                } else if self.is_repeater_neighbor(before, now_ms) {
                    // `last` forwarded what a repeater sent: it hears the
                    // repeater (links are assumed to be roughly symmetric).
                    self.neighbors.mark_covered(last, now_ms);
                }
            }
        }
    }

    /// Messages we carry right now.
    pub fn carried_count(&self) -> usize {
        self.carried.len()
    }

    /// Starts carrying a direct message we could not deliver (see "Carry").
    fn start_carry(&mut self, dest: Address, payload: &[u8], now_ms: u64) {
        if self.carry_copies == 0 || dest == self.addr {
            return;
        }
        self.add_carried(dest, payload, self.carry_copies, self.carry_ttl_ms, None, now_ms);
    }

    /// Carries a message for someone else, e.g. mail a post office cannot
    /// deliver (see "Carry"): `tokens` copies, living `ttl_ms`. Returns
    /// false when it was not taken (no room, already carried, no tokens).
    pub fn carry_for(&mut self, dest: Address, payload: &[u8], tokens: u8, ttl_ms: u64, now_ms: u64) -> bool {
        let id = packet_id(PacketType::Data, &dest, payload);
        if self.carried.iter().any(|c| c.id == id) || self.carry_done.iter().any(|d| d.id == id) {
            return false;
        }
        self.add_carried(dest, payload, tokens, ttl_ms, None, now_ms);
        self.carried.iter().any(|c| c.id == id)
    }

    fn add_carried(
        &mut self,
        dest: Address,
        payload: &[u8],
        tokens: u8,
        ttl_ms: u64,
        from: Option<PathId>,
        now_ms: u64,
    ) {
        let id = packet_id(PacketType::Data, &dest, payload);
        if self.carried.iter().any(|c| c.id == id) || tokens == 0 {
            return;
        }
        let Ok(payload) = Buf::from_slice(payload) else { return };
        if (self.carried.is_full() || self.carried.len() >= self.carry_max.clamp(1, CARRY_CAP))
            && let Some(i) = self.carried.iter().enumerate().min_by_key(|(_, c)| c.until_ms).map(|(i, _)| i)
        {
            self.carried.swap_remove(i);
        }
        let mut given = heapless::Vec::new();
        if let Some(f) = from {
            let _ = given.push(f);
        }
        let _ = self.carried.push(Carried { dest, payload, id, tokens, until_ms: now_ms + ttl_ms, given, tried_ms: 0, routed_ms: 0, own: from.is_none() });
    }

    /// Hands carried messages to neighbours heard recently.
    fn carry_step(&mut self, now_ms: u64, out: &mut Actions) {
        self.carried.retain(|c| c.until_ms > now_ms);
        self.carry_done.retain(|d| d.until_ms > now_ms);
        let here: heapless::Vec<PathId, 32> = self
            .neighbors
            .iter()
            .filter(|n| now_ms.saturating_sub(n.last_ms) < CARRY_FRESH_MS)
            .map(|n| n.id)
            .collect();
        // Near a repeater: try ordinary routing again (the outbox that waits
        // for coverage). The message stays carried until it expires; the
        // destination's duplicate filter drops copies.
        let near_repeater = here.iter().any(|&n| self.is_repeater_neighbor(n, now_ms));
        let regained = near_repeater && !self.carry_near_repeater;
        self.carry_near_repeater = near_repeater;
        if near_repeater {
            let due: heapless::Vec<(Address, Buf, u64), CARRY_CAP> = self
                .carried
                .iter()
                .filter(|c| c.own && (regained || !self.carry_smart && (c.routed_ms == 0 || now_ms.saturating_sub(c.routed_ms) >= CARRY_REROUTE_MS)))
                .map(|c| (c.dest, c.payload.clone(), c.id))
                .collect();
            for (dest, payload, id) in due {
                if let Some(c) = self.carried.iter_mut().find(|c| c.id == id) {
                    c.routed_ms = now_ms;
                }
                let mut token = [0u8; 16];
                token[..8].copy_from_slice(&(id ^ now_ms.rotate_left(29)).to_le_bytes());
                token[8] = 0xCA;
                if let Ok((_, more)) = self.send_typed(PacketType::Data, dest, &payload, token, now_ms) {
                    for m in more {
                        let _ = out.push(m);
                    }
                }
            }
        }
        self.carry_seen_new = here.iter().filter(|n| !self.carry_seen.contains(n)).count();
        let fresh: heapless::Vec<PathId, 32> = here
            .iter()
            .copied()
            .filter(|n| !self.carry_seen.contains(n))
            .filter(|n| !self.neighbors.get(*n).is_some_and(|x| matches!(x.role, Some(Role::Repeater | Role::Anchor))))
            .collect();
        self.carry_seen = here.clone();
        // Smart carry: only neighbours that move take copies.
        let fresh: heapless::Vec<PathId, 32> =
            if self.carry_smart { fresh.iter().copied().filter(|&n| self.is_mobile_neighbor(n, now_ms)).collect() } else { fresh };
        // Smart carry: a moving carrier that meets new neighbours has arrived
        // somewhere: try ordinary routing from here.
        if self.carry_smart && self.mobile && self.carry_seen_new >= 2 {
            let due: heapless::Vec<(Address, Buf, u64), CARRY_CAP> = self
                .carried
                .iter()
                .filter(|c| c.routed_ms == 0 || now_ms.saturating_sub(c.routed_ms) >= CARRY_ARRIVE_MS)
                .take(2)
                .map(|c| (c.dest, c.payload.clone(), c.id))
                .collect();
            for (dest, payload, id) in due {
                if let Some(c) = self.carried.iter_mut().find(|c| c.id == id) {
                    c.routed_ms = now_ms;
                }
                let mut token = [0u8; 16];
                token[..8].copy_from_slice(&(id ^ now_ms.rotate_left(31)).to_le_bytes());
                token[8] = 0xCB;
                self.stats.carry_rerouted += 1;
                if let Ok((_, more)) = self.send_typed(PacketType::Data, dest, &payload, token, now_ms) {
                    for m in more {
                        let _ = out.push(m);
                    }
                }
            }
        }
        let mut sent = 0;
        // Tell new encounters that may hold a copy of a delivered message.
        for i in 0..self.carry_done.len() {
            if sent >= 2 {
                break;
            }
            let d = &self.carry_done[i];
            if d.sent_ms != 0 && now_ms.saturating_sub(d.sent_ms) < CARRY_RETRY_MS {
                continue;
            }
            if fresh.iter().any(|n| d.holders.contains(n)) {
                let id = d.id;
                self.emit_carry_notice(id, None, now_ms, out);
                sent += 1;
            }
        }
        for i in 0..self.carried.len() {
            if sent >= 2 {
                break; // a few per step: the channel is shared
            }
            let c = &self.carried[i];
            let dest_id = c.dest.path_id();
            let ttl_s = (c.until_ms.saturating_sub(now_ms) / 1000).min(u16::MAX as u64) as u16;
            if here.contains(&dest_id) {
                if c.tried_ms != 0 && now_ms.saturating_sub(c.tried_ms) < CARRY_RETRY_MS {
                    continue;
                }
                let (dest, payload) = (c.dest, c.payload.clone());
                self.carried[i].tried_ms = now_ms;
                self.emit_carry(dest, dest_id, 0, ttl_s, &payload, now_ms, out);
                sent += 1;
                continue;
            }
            if c.tokens > 1
                && let Some(&nb) = fresh.iter().find(|n| !c.given.contains(n) && **n != dest_id)
            {
                let give = c.tokens / 2;
                let (dest, payload) = (c.dest, c.payload.clone());
                let c = &mut self.carried[i];
                c.tokens -= give;
                let _ = c.given.push(nb);
                self.emit_carry(dest, nb, give, ttl_s, &payload, now_ms, out);
                sent += 1;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_carry(&mut self, dest: Address, to: PathId, tokens: u8, ttl_s: u16, msg: &[u8], now_ms: u64, out: &mut Actions) {
        let mut payload = Buf::new();
        let _ = payload.push(tokens);
        let _ = payload.extend_from_slice(&ttl_s.to_be_bytes());
        let _ = payload.extend_from_slice(&self.id.to_be_bytes());
        if payload.extend_from_slice(msg).is_err() {
            return;
        }
        let Ok(path) = Path::from_slice(&[to]) else { return };
        let header = Header::new(PacketType::Carry, dest).with_path(path);
        self.stats.carry_handoffs += 1;
        let _ = now_ms;
        self.emit(&header, &payload, SOURCE_ROUTE_DELAY_MS, out);
    }

    /// A carried message handed to us.
    fn on_carry(&mut self, pkt: &Packet, now_ms: u64, out: &mut Actions) {
        let h = &pkt.header;
        let Some(path) = &h.path else {
            self.on_carry_notice(pkt, now_ms, out);
            return;
        };
        if path.first() != Some(&self.id) || pkt.payload.len() < 5 {
            return;
        }
        let tokens = pkt.payload[0];
        let ttl_s = u16::from_be_bytes([pkt.payload[1], pkt.payload[2]]);
        let from = u16::from_be_bytes([pkt.payload[3], pkt.payload[4]]);
        let msg = &pkt.payload[5..];
        let id = packet_id(PacketType::Data, &h.dest, msg);
        if h.dest == self.addr && self.hold_direct {
            // The carrier keeps it until our phone is back.
            self.stats.held_direct += 1;
            return;
        }
        if h.dest == self.addr {
            let new = self.dedup.insert(id);
            if new {
                self.stats.carry_delivered += 1;
                let _ = out.push(Action::Deliver {
                    ptype: PacketType::Data,
                    dest: self.addr,
                    payload: Buf::from_slice(msg).unwrap_or_default(),
                    hops: 0,
                });
            }
            // Delivered (again): tell the carrier, so it stops. With app acks
            // only once the app has opened it.
            if let Some(o) = self.owed.iter_mut().find(|o| o.id == id) {
                o.carried = true;
                let (answered, proof) = (o.answered, o.proof);
                if answered {
                    self.emit_delivered(id, proof, now_ms, out);
                }
            } else if self.app_acks && new {
                self.owe(id, None, true);
            } else {
                self.emit_carry_notice(id, None, now_ms, out);
            }
            return;
        }
        if self.carry_copies == 0 {
            return;
        }
        if self.carry_done.iter().any(|d| d.id == id) {
            // Already delivered: answer with the notice instead of storing it.
            self.emit_carry_notice(id, None, now_ms, out);
            return;
        }
        if self.dedup.contains(id) {
            return; // already delivered through us
        }
        self.add_carried(h.dest, msg, tokens, ttl_s as u64 * 1000, Some(from), now_ms);
    }

    /// Sends the delivered notice for carried message `id` (see "Delivered
    /// notice"): a Carry packet without a path, addressed to `ack_token(id)`,
    /// with the delivery proof as payload if we have it.
    fn emit_carry_notice(&mut self, id: u64, proof: Option<Proof>, now_ms: u64, out: &mut Actions) {
        let mut proof = proof;
        if let Some(d) = self.carry_done.iter_mut().find(|d| d.id == id) {
            d.sent_ms = now_ms;
            proof = proof.or(d.proof);
        }
        let header = Header::new(PacketType::Carry, ack_token(id));
        let payload = proof.as_ref().map_or(&[][..], |p| &p[..]);
        let Ok(packet) = encode(&header, payload) else { return };
        self.stats.carry_notices += 1;
        // Not through `emit`: the notice is repeated on purpose, so it is
        // kept out of the duplicate filter.
        let tx_id = packet_id(PacketType::Carry, &header.dest, payload) ^ now_ms;
        let _ = out.push(Action::Transmit { packet, delay_ms: SOURCE_ROUTE_DELAY_MS, id: tx_id });
    }

    /// A delivered notice heard from a neighbour.
    fn on_carry_notice(&mut self, pkt: &Packet, now_ms: u64, out: &mut Actions) {
        let b = pkt.header.dest.0;
        let mut lo = [0u8; 8];
        let mut hi = [0u8; 8];
        lo.copy_from_slice(&b[..8]);
        hi.copy_from_slice(&b[8..]);
        let id = u64::from_le_bytes(lo);
        if u64::from_le_bytes(hi) != !id || !matches!(pkt.payload.len(), 0 | PROOF_LEN) {
            return; // not a notice
        }
        self.carry_delivered_notice(id, true, Some(pkt.payload), now_ms, out);
    }

    /// Carried message `id` is known to be delivered: drop our copy, tell
    /// the app if it is ours, and pass the notice on once. `relay` is false
    /// when the sender learned it from an ordinary acknowledgement (the app
    /// already got `Acked`).
    fn carry_delivered_notice(&mut self, id: u64, relay: bool, proof: Option<&[u8]>, now_ms: u64, out: &mut Actions) {
        let got = proof.unwrap_or_default();
        let proof: Option<Proof> = got.try_into().ok();
        if let Some(d) = self.carry_done.iter_mut().find(|d| d.id == id) {
            // The proof after a notice without it: pass it on too.
            if d.proof.is_none() && proof.is_some() {
                d.proof = proof;
                if !d.holders.is_empty() {
                    self.emit_carry_notice(id, proof, now_ms, out);
                }
            }
            return;
        }
        let Some(i) = self.carried.iter().position(|c| c.id == id) else {
            return; // we never carried it: nothing to stop
        };
        // Our own message: only the expected proof stops it.
        if self.carried[i].own && relay && !self.proven(id, got) {
            return;
        }
        let c = self.carried.swap_remove(i);
        if c.own && relay {
            let _ = out.push(Action::CarryAcked { msg_id: id });
        }
        if self.carry_done.is_full() {
            self.carry_done.remove(0);
        }
        let _ = self.carry_done.push(CarryDone { id, until_ms: c.until_ms, holders: c.given.clone(), sent_ms: 0, proof });
        // Pass it on only if someone else may hold a copy.
        if !c.given.is_empty() {
            self.emit_carry_notice(id, proof, now_ms, out);
        }
    }

    /// Message `msg_id` (from [`Router::send_direct`]) counts as delivered
    /// only with `proof`. Call it right after sending.
    pub fn expect_proof(&mut self, msg_id: u64, proof: Proof) {
        self.expected.retain(|(id, _)| *id != msg_id);
        if self.expected.is_full() {
            self.expected.remove(0);
        }
        let _ = self.expected.push((msg_id, proof));
    }

    /// Does an acknowledgement of `msg_id` with the bytes `got` count?
    fn proven(&mut self, msg_id: u64, got: &[u8]) -> bool {
        let Some(i) = self.expected.iter().position(|(id, _)| *id == msg_id) else {
            return true;
        };
        if self.expected[i].1 != got {
            return false;
        }
        self.expected.remove(i);
        true
    }

    /// The app has opened the direct message `id` (its packet ID): tell the
    /// sender, with `proof` if the app can make one. Does nothing without
    /// [`Router::app_acks`] or for an unknown message.
    pub fn acknowledge(&mut self, id: u64, proof: Option<Proof>, now_ms: u64) -> Actions {
        let mut out = Actions::new();
        let Some(o) = self.owed.iter_mut().find(|o| o.id == id) else {
            return out;
        };
        o.answered = true;
        o.proof = proof.or(o.proof);
        let (back, carried, proof) = (o.back.clone(), o.carried, o.proof);
        if carried {
            self.emit_delivered(id, proof, now_ms, &mut out);
        }
        if let Some(back) = back {
            self.send_ack(id, back, proof, now_ms, &mut out);
        }
        out
    }

    /// Our own delivered notice: with the proof, and then without it for
    /// carriers from before proofs (they drop a notice with a payload).
    fn emit_delivered(&mut self, id: u64, proof: Option<Proof>, now_ms: u64, out: &mut Actions) {
        self.emit_carry_notice(id, proof, now_ms, out);
        if proof.is_some() {
            self.emit_carry_notice(id, None, now_ms, out);
        }
    }

    fn owe(&mut self, id: u64, back: Option<Path>, carried: bool) {
        if self.owed.iter().any(|o| o.id == id) {
            return;
        }
        if self.owed.is_full() {
            // An answered one goes first.
            let i = self.owed.iter().position(|o| o.answered).unwrap_or(0);
            self.owed.remove(i);
        }
        let _ = self.owed.push(Owed { id, back, carried, answered: false, proof: None });
    }

    fn send_ack(&mut self, id: u64, back: Path, proof: Option<Proof>, now_ms: u64, out: &mut Actions) {
        let header = Header::new(PacketType::Ack, ack_token(id)).with_path(back);
        let payload = proof.as_ref().map_or(&[][..], |p| &p[..]);
        self.emit_routed(&header, payload, SOURCE_ROUTE_DELAY_MS, now_ms, out);
    }

    /// Do we know a route to `dest`?
    pub fn has_route(&self, dest: &Address) -> bool {
        self.routes.get(dest).is_some()
    }

    /// Sends a new path request, e.g. if the first one was not answered.
    pub fn retry_path(&mut self, dest: Address, req_token: [u8; 16], now_ms: u64) -> Actions {
        let mut out = Actions::new();
        self.pending.retain(|p| p.target != dest);
        self.request_path(dest, req_token, now_ms, &mut out);
        out
    }

    fn request_path(&mut self, dest: Address, token: [u8; 16], now_ms: u64, out: &mut Actions) {
        if self.pending.is_full() {
            self.pending.remove(0);
        }
        self.emit_request(dest, Address(token), heapless::Vec::new(), now_ms, 0, out);
    }

    fn emit_request(
        &mut self,
        dest: Address,
        token: Address,
        old_tokens: heapless::Vec<Address, 2>,
        now_ms: u64,
        tries: u8,
        out: &mut Actions,
    ) {
        self.refresh_load(now_ms);
        if self.load_control && !self.path_quota.take(now_ms, self.load_high) {
            // Over the quota: wait for the next minute instead of flooding now.
            self.stats.quota_held += 1;
            let _ = self.pending.push(PendingRequest {
                token,
                old_tokens,
                target: dest,
                carried: None,
                ringed: false,
                retry_at_ms: (now_ms / 60_000 + 1) * 60_000,
                tries,
                sent: false,
            });
            return;
        }
        // Expanding ring: the very first attempt only goes a few hops, when
        // the target was heard within the ring lately (else everywhere).
        let mut token = token;
        let near = self
            .heard_hops
            .iter()
            .find(|(a, _, t)| *a == dest && now_ms.saturating_sub(*t) < 600_000)
            .map(|(_, h, _)| *h)
            .filter(|&h| h < self.ring && !self.mobile && !self.is_mobile_dest(&dest, now_ms));
        let ringed = tries == 0 && self.ring > 0 && old_tokens.is_empty() && near.is_some();
        if ringed {
            // One hop of margin beyond where it was heard.
            token.0[12] = (near.unwrap_or(0) + 2).min(self.ring);
            token.0[13..15].copy_from_slice(&RING_MARK);
        } else if token.0[13..15] == RING_MARK {
            token.0[13] = 0;
        }
        self.stats.path_requests += 1;
        self.stats.originated += 1;
        // The first message in the queue is sent along if it fits when
        // the path is full (12 hops).
        let mut payload = Buf::new();
        let _ = payload.extend_from_slice(&token.0);
        let mut carried = None;
        if let Some(q) = self
            .queue
            .iter()
            .find(|q| q.dest == dest && q.ptype == PacketType::Data)
            && 16 + q.payload.len() + stamp::STAMP_LEN <= MAX_DIRECT_PAYLOAD
            && payload.extend_from_slice(&q.payload).is_ok()
        {
            carried = Some(packet_id(PacketType::Data, &dest, &q.payload));
        }
        let wait = if ringed { RING_RETRY_MS } else { self.path_retry_ms as u64 * if self.load_control && self.load_high { 2 } else { 1 } };
        let _ = self.pending.push(PendingRequest {
            token,
            old_tokens,
            target: dest,
            carried,
            ringed,
            retry_at_ms: now_ms + wait,
            tries,
            sent: true,
        });
        // Proof of work bound to the target and the token (about 256 attempts on average).
        stamp::append(PacketType::PathRequest, &dest, &mut payload);
        let header = Header::new(PacketType::PathRequest, dest).with_path(Path::new());
        self.emit(&header, &payload, 0, out);
    }

    fn send_routed_on(
        &mut self,
        ptype: PacketType,
        dest: Address,
        payload: &[u8],
        path: Path,
        now_ms: u64,
        out: &mut Actions,
    ) {
        self.stats.originated += 1;
        let msg_id = packet_id(ptype, &dest, payload);
        if ptype == PacketType::Datagram {
            // No end-to-end acknowledgement: the tunnel handles loss itself (NACK).
        } else if let Some(a) = self.awaiting.iter_mut().find(|a| a.msg_id == msg_id) {
            a.sent_ms = now_ms;
        } else {
            if self.awaiting.is_full() {
                self.awaiting.remove(0);
            }
            let _ = self.awaiting.push(AwaitAck {
                token: ack_token(msg_id),
                msg_id,
                ptype,
                dest,
                payload: Buf::from_slice(payload).unwrap_or_default(),
                sent_ms: now_ms,
                tries: 0,
            });
        }
        let header = Header::new(ptype, dest).with_path(path);
        self.emit_routed(&header, payload, 0, now_ms, out);
    }

    /// Sends a message to a channel (flooded locally).
    pub fn send_channel(&mut self, channel: Address, payload: &[u8]) -> Result<Actions, SendError> {
        self.send_channel_in(channel, 0, payload)
    }

    /// Sends a channel message tagged with a region (0 = none).
    pub fn send_channel_in(
        &mut self,
        channel: Address,
        region: u16,
        payload: &[u8],
    ) -> Result<Actions, SendError> {
        self.stats.originated += 1;
        let header = Header::new(PacketType::Channel, channel).with_region(region);
        if payload.len() > header.max_payload() {
            return Err(SendError::TooLarge);
        }
        let mut out = Actions::new();
        self.emit(&header, payload, 0, &mut out);
        Ok(out)
    }

    // ---------- receiving ----------

    /// Handles a received packet.
    pub fn on_receive(&mut self, buf: &[u8], snr_db: i8, now_ms: u64) -> Actions {
        let mut out = Actions::new();
        let Ok(pkt) = Packet::decode(buf) else {
            self.stats.rx_invalid += 1;
            return out;
        };
        // Announces and path requests without a valid proof of work are not handled
        // and not forwarded. Checking costs one hash.
        if stamp::required(pkt.header.ptype)
            && stamp::verify(pkt.header.ptype, &pkt.header.dest, pkt.payload).is_err()
        {
            self.stats.bad_stamps += 1;
            return out;
        }
        self.stats.rx_packets += 1;
        self.note_neighbor(&pkt, snr_db, now_ms);
        if pkt.header.ptype == PacketType::Carry {
            self.on_carry(&pkt, now_ms, &mut out);
            return out;
        }
        if pkt.header.ptype == PacketType::Summary {
            if pkt.header.hops == 0 {
                deliver(&pkt, &mut out);
            }
            return out;
        }
        if pkt.header.ptype == PacketType::Hello {
            self.on_hello(&pkt, snr_db, now_ms);
            if self.hello {
                self.hello_step(now_ms, &mut out);
            }
            return out;
        }
        let id = packet_id(pkt.header.ptype, &pkt.header.dest, pkt.payload);
        match (&pkt.header.ptype, &pkt.header.path) {
            (PacketType::PathRequest, _) => {
                if self.dedup.insert(id) {
                    self.on_path_request(&pkt, snr_db, now_ms, &mut out);
                } else {
                    self.stats.rx_duplicates += 1;
                    if pkt.header.dest == self.addr {
                        self.consider_reply(&pkt, snr_db);
                    }
                    let prev = pkt.header.path.as_ref().and_then(|p| p.last().copied());
                    self.on_duplicate_flood(id, snr_db, prev, now_ms, &mut out);
                }
            }
            (_, Some(path)) => {
                // If we hear the next repeater forward our packet, the hop succeeded.
                let hops = pkt.header.hops;
                self.forwards.retain(|f| !(f.id == id && hops > f.hops));
                let path = path.clone();
                self.on_source_routed(&pkt, &path, id, snr_db, now_ms, &mut out);
            }
            (_, None) => {
                if self.dedup.insert(id) {
                    self.on_flood(&pkt, snr_db, now_ms, &mut out);
                } else {
                    self.stats.rx_duplicates += 1;
                    self.on_duplicate_flood(id, snr_db, None, now_ms, &mut out);
                }
            }
        }
        for a in &out {
            match a {
                Action::Deliver { .. } => self.stats.delivered += 1,
                Action::Acked { .. } | Action::CarryAcked { .. } => self.stats.acked += 1,
                Action::RouteLearned { .. } => self.stats.routes_learned += 1,
                Action::Cancel { .. } => self.stats.floods_cancelled += 1,
                Action::Transmit { .. } => {}
            }
        }
        out
    }

    /// One more copy of a path request to us. If its trail is better than the
    /// best so far, it is remembered instead.
    ///
    /// First weak last hops are avoided (they won because they reached furthest, but
    /// the reply back is often lost). Then fewest hops and best SNR.
    fn consider_reply(&mut self, pkt: &Packet, snr_db: i8) {
        let Some(token) = Address::from_slice(pkt.payload) else { return };
        let weak = self.weak_snr_db;
        let Some(r) = self.replies.iter_mut().find(|r| r.token == token) else { return };
        let path = pkt.header.path.clone().unwrap_or_default();
        let score = |snr: i8, hops: usize| ((snr < weak) as u8, hops, -(snr as i16));
        // With weak-hop penalties the header's hop count carries them.
        let (mine, theirs) = if self.weak_hop_penalty { (pkt.header.hops as usize, r.best_hops as usize) } else { (path.len(), r.best.len()) };
        if score(snr_db, mine) < score(r.best_snr, theirs) {
            r.best_hops = pkt.header.hops;
            r.best = path;
            r.best_snr = snr_db;
        }
    }

    fn on_path_request(&mut self, pkt: &Packet, snr_db: i8, now_ms: u64, out: &mut Actions) {
        let h = &pkt.header;
        let record = h.path.clone().unwrap_or_default();
        if h.dest == self.addr {
            // We are the target: answer the token along the reverse path, and send
            // the path found along in the payload.
            let Some(token) = Address::from_slice(pkt.payload) else {
                return;
            };
            // If the request contains a message, it is delivered now. The reply
            // then also works as an acknowledgement.
            let data = &stamp::strip(PacketType::PathRequest, pkt.payload)[16..];
            if !data.is_empty() && self.hold_direct {
                // The reply would acknowledge the message: stay quiet.
                self.stats.held_direct += 1;
                return;
            }
            let message = (!data.is_empty() && self.app_acks).then(|| packet_id(PacketType::Data, &self.addr, data));
            if !data.is_empty()
                && self
                    .dedup
                    .insert(packet_id(PacketType::Data, &self.addr, data))
            {
                if let Some(id) = message {
                    self.owe(id, None, false);
                }
                let _ = out.push(Action::Deliver {
                    ptype: PacketType::Data,
                    dest: self.addr,
                    payload: Buf::from_slice(data).unwrap_or_default(),
                    hops: h.hops,
                });
            }
            // Do not answer right away: wait for more copies along other routes and
            // pick the best (see `consider_reply`).
            if self.replies.iter().any(|r| r.token == token) {
                return;
            }
            if self.replies.is_full() {
                self.replies.remove(0);
            }
            let _ = self.replies.push(PendingReply {
                token,
                best: record,
                best_snr: snr_db,
                best_hops: h.hops,
                reply_at_ms: now_ms + (RESPONSE_WAIT_SLOTS * self.slot_ms) as u64,
                message,
            });
            return;
        }
        if !self.may_forward(h.hops) || h.hops >= MAX_HOPS {
            return;
        }
        if let Some(t) = pkt.payload.get(..16)
            && t[13..15] == RING_MARK
            && t[12] > 0
            && h.hops + 1 >= t[12]
        {
            // Ring search: nodes `ring` hops away hear it; it goes no further.
            return;
        }
        if self.proxy_reply
            && self.role == Role::Anchor
            && h.dest != self.addr
            && self.routes.get(&h.dest).is_some_and(|r| r.confirmed_ms > 0 && now_ms.saturating_sub(r.confirmed_ms) < PROXY_ROUTE_MS)
            && let Some(token) = Address::from_slice(pkt.payload)
        {
            // Answer for the target: the requester's route ends at us, and
            // we forward along our own route (anchor_forward).
            let back = reversed(&record);
            let mut payload = Buf::new();
            let _ = payload.push(record.len() as u8);
            for pid in &record {
                let _ = payload.extend_from_slice(&pid.to_be_bytes());
            }
            let header = Header::new(PacketType::PathResponse, token).with_path(back);
            self.stats.proxy_replies += 1;
            self.emit_routed(&header, &payload, SOURCE_ROUTE_DELAY_MS, now_ms, out);
            return;
        }
        if self.boundary
            && self.role == Role::Relay
            && self.hears_repeater(now_ms)
            && let Some(&prev) = record.last()
        {
            if self.is_repeater_neighbor(prev, now_ms) {
                // From the repeater side: into the uncovered side only for
                // a destination known to be there, or on the second request
                // for it (an Ethernet bridge floods unknown destinations;
                // we do so only when the first try found nothing).
                if !self.uncovered.contains(&h.dest) {
                    self.inward_dropped.retain(|(_, t)| now_ms.saturating_sub(*t) < INWARD_REPEAT_MS);
                    if let Some(i) = self.inward_dropped.iter().position(|(a, _)| *a == h.dest) {
                        self.inward_dropped.remove(i);
                    } else {
                        if self.inward_dropped.is_full() {
                            self.inward_dropped.remove(0);
                        }
                        let _ = self.inward_dropped.push((h.dest, now_ms));
                        return;
                    }
                }
            } else if self.neighbors.get(prev).is_some_and(|n| n.hears_repeater == Some(true)) {
                // From a covered neighbor: the repeater side has it already.
                return;
            }
        }
        let mut record = record;
        if self.role == Role::Anchor && !record.is_empty() {
            // An anchor splits the path: it remembers the way back and forwards
            // with an empty trail. Then an anchor at least every 12th hop is enough.
            let Some(token) = Address::from_slice(pkt.payload) else { return };
            self.remember_anchor(token, h.dest, reversed(&record), now_ms);
            record = Path::new();
        } else if record.push(self.id).is_err() {
            return; // Longer than a source path can carry, and we are not an anchor.
        }
        self.refresh_load(now_ms);
        if self.load_control && !self.path_quota.take(now_ms, self.load_high) {
            self.stats.quota_dropped += 1;
            return;
        }
        let mut header = Header::new(PacketType::PathRequest, h.dest).with_path(record);
        header.hops = h.hops + 1 + (self.weak_hop_penalty && snr_db < self.weak_snr_db) as u8;
        let delay = self.flood_delay_ms(snr_db);
        // Only where others can pass the request on: a node with few neighbours may be the one
        // way through (a chain, a host between two networks), and then a dropped request costs
        // a whole retry for nothing.
        if self.request_fwd_permille > 0 && self.request_fwd_permille < 1000 && self.neighbors.len() >= REQUEST_FWD_MIN_NEIGHBORS {
            let mix = packet_id(h.ptype, &h.dest, pkt.payload) ^ (self.id as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
            let mix = mix ^ (mix >> 29);
            if (mix % 1000) as u16 >= self.request_fwd_permille {
                return;
            }
        }
        self.stats.forwarded += 1;
        self.emit_flood(&header, pkt.payload, delay, out);
    }

    fn on_source_routed(
        &mut self,
        pkt: &Packet,
        path: &Path,
        id: u64,
        snr_db: i8,
        now_ms: u64,
        out: &mut Actions,
    ) {
        let h = &pkt.header;
        let idx = h.hops as usize;
        if idx < path.len() {
            // Are we the next repeater on the path? A copy we have seen before is
            // forwarded too: the previous hop only retransmits when it did not hear us.
            if path[idx] != self.id || !self.may_forward(h.hops) {
                return;
            }
            self.dedup.insert(id);
            let mut header = h.clone();
            header.hops += 1;
            self.stats.forwarded += 1;
            // The next hop is known to be dead: go past it at once.
            if self.is_dead_hop(&header, now_ms)
                && let Some(bypass) = self.bypass_header(&header, now_ms)
            {
                self.stats.local_repairs += 1;
                header = bypass;
            }
            if h.ptype == PacketType::Probe && diag::trace_fixed_len(pkt.payload).is_some() {
                // Trace: append the SNR we heard the packet with.
                let mut p = diag::TraceBuf::from_slice(pkt.payload).unwrap_or_default();
                diag::trace_append(&mut p, snr_db);
                self.emit_routed(&header, &p, SOURCE_ROUTE_DELAY_MS, now_ms, out);
            } else {
                self.emit_routed(&header, pkt.payload, SOURCE_ROUTE_DELAY_MS, now_ms, out);
            }
            return;
        }
        if idx != path.len() {
            return;
        }
        // Last stretch: is the packet for us?
        match h.ptype {
            PacketType::PathResponse => {
                let Some(i) = self
                    .pending
                    .iter()
                    .position(|p| p.token == h.dest || p.old_tokens.contains(&h.dest))
                else {
                    self.anchor_path_response(pkt, id, now_ms, out);
                    return;
                };
                if !self.dedup.insert(id) {
                    return;
                }
                let Some(forward) = parse_path(pkt.payload) else {
                    return;
                };
                let req = self.pending.swap_remove(i);
                // Without the expected proof the message stays queued and goes
                // along the route just learned.
                if let Some(msg_id) = req.carried
                    && self.proven(msg_id, pkt.payload.get(1 + 2 * forward.len()..).unwrap_or_default())
                {
                    self.awaiting.retain(|a| a.msg_id != msg_id);
                    // The message arrived with the request: remove it from the queue.
                    let target = req.target;
                    self.queue
                        .retain(|q| packet_id(q.ptype, &target, &q.payload) != msg_id);
                    let _ = out.push(Action::Acked { msg_id });
                    self.carry_delivered_notice(msg_id, false, pkt.payload.get(1 + 2 * forward.len()..), now_ms, out);
                }
                self.learn_route(req.target, forward, now_ms, out);
            }
            PacketType::Ack => {
                let Some(i) = self.awaiting.iter().position(|a| a.token == h.dest) else {
                    self.anchor_relay_back(pkt, id, now_ms, out);
                    return;
                };
                let msg_id = self.awaiting[i].msg_id;
                if self.dedup.contains(id) || !self.proven(msg_id, pkt.payload) {
                    return;
                }
                self.dedup.insert(id);
                let a = self.awaiting.remove(i);
                self.routes.confirm(&a.dest, now_ms);
                let _ = out.push(Action::Acked { msg_id });
                self.carry_delivered_notice(msg_id, false, Some(pkt.payload), now_ms, out);
            }
            PacketType::Probe if h.dest == self.addr => {
                // Trace to us: answer along the reverse path to the token.
                let Some((token, snrs)) = diag::parse_request(pkt.payload) else { return };
                if !self.dedup.insert(id) {
                    return;
                }
                let mut out_snrs: heapless::Vec<u8, { diag::MAX_SNRS }> = heapless::Vec::new();
                let _ = out_snrs.extend_from_slice(&snrs[..snrs.len().min(diag::MAX_SNRS - 1)]);
                let _ = out_snrs.push(snr_db as u8);
                let resp = diag::trace_response(path, &out_snrs);
                let header = Header::new(PacketType::Probe, token).with_path(reversed(path));
                self.emit_routed(&header, &resp, SOURCE_ROUTE_DELAY_MS, now_ms, out);
            }
            PacketType::Probe => {
                // A reply to our own trace?
                let Some(i) = self.traces.iter().position(|t| t.1 == h.dest) else { return };
                if !self.dedup.insert(id) {
                    return;
                }
                let (dest, _, _) = self.traces.remove(i);
                let mut p = diag::TraceBuf::from_slice(pkt.payload).unwrap_or_default();
                diag::trace_append(&mut p, snr_db);
                let _ = out.push(Action::Deliver {
                    ptype: PacketType::Probe,
                    dest,
                    payload: Buf::from_slice(&p).unwrap_or_default(),
                    hops: h.hops,
                });
            }
            _ if h.dest == self.addr && self.hold_direct && matches!(h.ptype, PacketType::Data | PacketType::Fragment) => {
                // Nobody to hand it to: no delivery, no acknowledgement.
                self.stats.held_direct += 1;
            }
            _ if h.dest == self.addr => {
                // A repeated message is not delivered again, but acknowledged again,
                // because the first acknowledgement may have been lost.
                let new = self.dedup.insert(id);
                if new {
                    deliver(pkt, out);
                }
                if !matches!(
                    h.ptype,
                    PacketType::Data | PacketType::Fragment | PacketType::Deposit
                ) {
                    return;
                }
                let back = reversed(path);
                if !(self.app_acks && h.ptype == PacketType::Data) {
                    self.send_ack(id, back, None, now_ms, out);
                } else if let Some(o) = self.owed.iter_mut().find(|o| o.id == id) {
                    // A copy: the way back may have changed.
                    o.back = Some(back.clone());
                    let (answered, proof) = (o.answered, o.proof);
                    if answered {
                        self.send_ack(id, back, proof, now_ms, out);
                    }
                } else if new {
                    // The app answers (see `acknowledge`).
                    self.owe(id, Some(back), false);
                } else {
                    self.send_ack(id, back, None, now_ms, out);
                }
            }
            PacketType::Data | PacketType::Fragment | PacketType::Deposit | PacketType::Datagram
                if self.role == Role::Anchor =>
            {
                self.anchor_forward(pkt, path, id, now_ms, out);
            }
            _ => {}
        }
    }

    fn remember_anchor(&mut self, key: Address, target: Address, back: Path, now_ms: u64) {
        self.remember_anchor_as(key, target, back, now_ms, false);
    }

    fn remember_anchor_as(&mut self, key: Address, target: Address, back: Path, now_ms: u64, forward: bool) {
        // Forwards that were never acknowledged: our route to their target
        // is suspect (it is looked for again next time).
        // Only with `drop_unacked_routes` (or when we answer from routes).
        let dead: heapless::Vec<Address, MAX_ANCHORED> =
            if self.proxy_reply || self.drop_unacked_routes { self.anchored.iter().filter(|a| a.forward && a.until_ms <= now_ms).map(|a| a.target).collect() } else { heapless::Vec::new() };
        for t in dead {
            if self.routes.get(&t).is_some_and(|r| r.confirmed_ms < now_ms.saturating_sub(self.anchor_ttl_ms)) {
                self.routes.remove(&t);
            }
        }
        self.anchored.retain(|a| a.until_ms > now_ms && a.key != key);
        if self.anchored.is_full() {
            self.anchored.remove(0);
        }
        let _ = self.anchored.push(Anchored { key, target, back, until_ms: now_ms + self.anchor_ttl_ms, forward });
    }

    /// Reply to a request we split: learn the onward route to the target and send
    /// the reply back along the first leg with that leg as the sender's route.
    fn anchor_path_response(&mut self, pkt: &Packet, id: u64, now_ms: u64, out: &mut Actions) {
        let h = &pkt.header;
        let Some(i) = self.anchored.iter().position(|a| a.key == h.dest && a.until_ms > now_ms) else {
            return;
        };
        if !self.dedup.insert(id) {
            return;
        }
        let Some(onward) = parse_path(pkt.payload) else { return };
        let a = self.anchored.swap_remove(i);
        self.routes.upsert(Route {
            dest: a.target,
            cost: onward.len() as u16,
            path: onward,
            last_seen_ms: now_ms,
            learned_ms: now_ms,
            confirmed_ms: 0,
        });
        let first = reversed(&a.back);
        let mut payload = Buf::new();
        let _ = payload.push(first.len() as u8);
        for pid in &first {
            let _ = payload.extend_from_slice(&pid.to_be_bytes());
        }
        // What follows the path (a delivery proof) goes on unchanged.
        let _ = payload.extend_from_slice(pkt.payload.get(1 + 2 * usize::from(pkt.payload[0])..).unwrap_or_default());
        let header = Header::new(PacketType::PathResponse, a.key).with_path(a.back);
        self.stats.forwarded += 1;
        self.emit_routed(&header, &payload, SOURCE_ROUTE_DELAY_MS, now_ms, out);
    }

    /// Acknowledgement for a message we forwarded as an anchor: send it back.
    fn anchor_relay_back(&mut self, pkt: &Packet, id: u64, now_ms: u64, out: &mut Actions) {
        let h = &pkt.header;
        let Some(i) = self.anchored.iter().position(|a| a.key == h.dest && a.until_ms > now_ms) else {
            return;
        };
        if !self.dedup.insert(id) {
            return;
        }
        let a = self.anchored.swap_remove(i);
        if a.forward {
            // The message we forwarded got through: the route works.
            self.routes.confirm(&a.target, now_ms);
        }
        let header = Header::new(h.ptype, a.key).with_path(a.back);
        self.stats.forwarded += 1;
        self.emit_routed(&header, pkt.payload, SOURCE_ROUTE_DELAY_MS, now_ms, out);
    }

    /// A message whose path ends at us, but which is for someone else: forward it
    /// along our own route and remember the way back for the acknowledgement.
    fn anchor_forward(&mut self, pkt: &Packet, path: &Path, id: u64, now_ms: u64, out: &mut Actions) {
        let h = &pkt.header;
        if h.dest == self.addr || self.energy == Energy::Critical {
            return;
        }
        let Some(route) = self.routes.get(&h.dest).map(|r| r.path.clone()) else { return };
        if !self.dedup.insert(id) {
            return;
        }
        self.remember_anchor_as(ack_token(id), h.dest, reversed(path), now_ms, true);
        let header = Header::new(h.ptype, h.dest).with_path(route);
        self.stats.forwarded += 1;
        self.emit_routed(&header, pkt.payload, SOURCE_ROUTE_DELAY_MS, now_ms, out);
    }

    /// Adds a route learned some other way than a path request, e.g. from a
    /// location service that knows the fixed infrastructure (the simulator's
    /// home routing model). It is used and repaired like any learned route.
    pub fn add_route(&mut self, target: Address, path: Path, now_ms: u64) {
        self.routes.upsert(Route {
            dest: target,
            cost: path.len() as u16,
            path,
            last_seen_ms: now_ms,
            learned_ms: now_ms,
            confirmed_ms: 0,
        });
    }

    fn learn_route(&mut self, target: Address, path: Path, now_ms: u64, out: &mut Actions) {
        let hops = path.len() as u8;
        self.routes.upsert(Route {
            dest: target,
            cost: hops as u16,
            path: path.clone(),
            last_seen_ms: now_ms,
            learned_ms: now_ms,
            confirmed_ms: 0,
        });
        let _ = out.push(Action::RouteLearned { dest: target, hops });
        // Trace routes that were waiting for the route.
        for i in 0..self.traces.len() {
            let (dest, token, sent) = self.traces[i];
            if dest == target && !sent {
                self.traces[i].2 = true;
                self.emit_trace(dest, token, path.clone(), now_ms, out);
            }
        }
        // Send messages that were waiting for the route.
        let mut i = 0;
        while i < self.queue.len() {
            if self.queue[i].dest == target {
                let q = self.queue.swap_remove(i);
                self.send_routed_on(q.ptype, q.dest, &q.payload, path.clone(), now_ms, out);
            } else {
                i += 1;
            }
        }
    }

    fn on_flood(&mut self, pkt: &Packet, snr_db: i8, now_ms: u64, out: &mut Actions) {
        let h = &pkt.header;
        // Announces always go to the app, so it can verify and remember the key.
        if h.ptype == PacketType::Announce {
            self.note_heard_hops(h.dest, h.hops, now_ms);
        }
        // Key requests too: the app knows whether it can answer.
        if h.dest == self.addr
            || self.channels.contains(&h.dest)
            || matches!(h.ptype, PacketType::Announce | PacketType::KeyRequest | PacketType::MailSync)
        {
            deliver(pkt, out);
        }
        if !self.may_forward(h.hops) || h.dest == self.addr {
            return;
        }
        let local = matches!(
            h.ptype,
            PacketType::Channel | PacketType::Announce | PacketType::Probe | PacketType::KeyRequest | PacketType::MailSync
        );
        // On low battery only the most important things are forwarded.
        let allowed = match self.energy {
            Energy::Full | Energy::Ok => true,
            Energy::Low => !local,
            Energy::Critical => false,
        };
        let scope = if h.ptype == PacketType::MailSync {
            crate::mail::MAIL_SCOPE
        } else if h.ptype == PacketType::Announce {
            // A moving node's key is needed far ahead of it: its announces
            // keep the full local scope.
            if announce_is_mobile(pkt.payload) { LOCAL_SCOPE } else { self.announce_scope }
        } else if local {
            LOCAL_SCOPE
        } else {
            MAX_HOPS
        };
        if !allowed || h.hops >= scope || !self.forwards_region(h.region) {
            return;
        }
        let mut header = h.clone();
        header.hops += 1;
        let delay = self.flood_delay_ms(snr_db);
        self.stats.forwarded += 1;
        self.emit_flood(&header, pkt.payload, delay, out);
    }
}

/// Does an announce say its sender is mobile (role flags in the app data,
/// which starts at byte 69: keys (64) | timestamp (4) | length (1))?
fn announce_is_mobile(payload: &[u8]) -> bool {
    let Some(&n) = payload.get(68) else { return false };
    payload
        .get(69..69 + n as usize)
        .and_then(|d| crate::appdata::parse(d).role_flags)
        .is_some_and(|f| f & crate::appdata::ROLE_MOBILE != 0)
}

/// Delivers the packet to the app. A stamp is removed first, so the app sees the same
/// payload the sender made (e.g. a signed announce).
fn deliver(pkt: &Packet, out: &mut Actions) {
    let payload = stamp::strip(pkt.header.ptype, pkt.payload);
    let _ = out.push(Action::Deliver {
        ptype: pkt.header.ptype,
        dest: pkt.header.dest,
        payload: Buf::from_slice(payload).unwrap_or_default(),
        hops: pkt.header.hops,
    });
}

fn parse_path(payload: &[u8]) -> Option<Path> {
    let n = *payload.first()? as usize;
    if n > MAX_PATH || payload.len() < 1 + 2 * n {
        return None;
    }
    let mut p = Path::new();
    for c in payload[1..1 + 2 * n].chunks_exact(2) {
        let _ = p.push(u16::from_be_bytes([c[0], c[1]]));
    }
    Some(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    type R = Router<32, 64>;

    fn node(n: u8, role: Role) -> R {
        let mut a = [n; 16];
        a[0] = 0;
        a[1] = n; // path ID = n
        let mut r = R::new(Address(a), role);
        // The tests build fixed topologies where every repeater must forward;
        // the default drops one path request in five.
        r.request_fwd_permille = 1000;
        r
    }

    /// How many of 400 different path requests a repeater with `neighbours` neighbours forwards.
    fn forwarded_of_400(neighbours: u16) -> usize {
        let mut fwd = 0;
        for i in 0..400u32 {
            let mut r = R::new(Address([7; 16]), Role::Repeater);
            assert_eq!(r.request_fwd_permille, REQUEST_FWD_PERMILLE);
            for n in 0..neighbours {
                r.neighbors.heard(100 + n, None, 5, -80, 1000);
            }
            let dest = Address([9; 16]);
            let mut payload = Buf::new();
            let _ = payload.extend_from_slice(&[i as u8, (i >> 8) as u8, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);
            assert!(stamp::append(PacketType::PathRequest, &dest, &mut payload));
            let h = Header::new(PacketType::PathRequest, dest).with_path(Path::new());
            let pkt = encode(&h, &payload).unwrap();
            let out = r.on_receive(&pkt, -5, 1000);
            if out.iter().any(|a| matches!(a, Action::Transmit { .. })) {
                fwd += 1;
            }
        }
        fwd
    }

    #[test]
    fn about_four_fifths_of_path_requests_are_forwarded_where_there_are_other_neighbours() {
        let fwd = forwarded_of_400(REQUEST_FWD_MIN_NEIGHBORS as u16);
        assert!((280..=360).contains(&fwd), "forwarded {fwd} of 400");
    }

    #[test]
    fn a_node_with_few_neighbours_forwards_every_path_request() {
        // It may be the only way through (a chain, a host between two networks).
        assert_eq!(forwarded_of_400(0), 400);
        assert_eq!(forwarded_of_400(REQUEST_FWD_MIN_NEIGHBORS as u16 - 1), 400);
    }

    /// Lets `n` wait out the time slot in which it collects copies of a
    /// path request, and returns what it then sends.
    fn reply(n: &mut R, now: u64) -> Vec<Buf> {
        let d = n.next_deadline().expect("waiting to reply");
        assert!(d >= now);
        transmits(&n.tick(d))
    }

    fn transmits(actions: &Actions) -> Vec<Buf> {
        actions
            .iter()
            .filter_map(|a| match a {
                Action::Transmit { packet, .. } => Some(packet.clone()),
                _ => None,
            })
            .collect()
    }

    /// Simulates a chain: every node only hears its two neighbours. Runs until
    /// nothing more happens, and returns all events (node, action).
    fn run_line(nodes: &mut [R], start: Vec<(usize, Buf)>, now: &mut u64) -> Vec<(usize, Action)> {
        let mut queue: std::collections::VecDeque<(usize, Buf)> = start.into();
        let mut events = Vec::new();
        for _ in 0..20_000 {
            if let Some((from, pkt)) = queue.pop_front() {
                for j in [from.wrapping_sub(1), from + 1] {
                    if j >= nodes.len() {
                        continue;
                    }
                    for a in nodes[j].on_receive(&pkt, 5, *now) {
                        match a {
                            Action::Transmit { packet, .. } => queue.push_back((j, packet)),
                            other => events.push((j, other)),
                        }
                    }
                }
                continue;
            }
            let Some(next) = nodes.iter().filter_map(|n| n.next_deadline()).min() else { break };
            if next > *now + 300_000 {
                break;
            }
            *now = next.max(*now);
            for (i, n) in nodes.iter_mut().enumerate() {
                for a in n.tick(*now) {
                    match a {
                        Action::Transmit { packet, .. } => queue.push_back((i, packet)),
                        other => events.push((i, other)),
                    }
                }
            }
        }
        events
    }

    fn line(len: u8, anchors: &[u8]) -> Vec<R> {
        (0..=len)
            .map(|n| {
                let role = if n == 0 || n == len {
                    Role::Client
                } else if anchors.contains(&n) {
                    Role::Anchor
                } else {
                    Role::Repeater
                };
                node(n + 1, role)
            })
            .collect()
    }

    #[test]
    fn anchors_carry_messages_beyond_twelve_hops() {
        // A, 22 repeaters with anchors at 8 and 16, B: 22 hops in total.
        let mut nodes = line(23, &[8, 16]);
        let b = nodes[23].addr;
        let mut now = 0;
        let (first, out) = nodes[0].send_direct(b, "far away".as_bytes(), [7; 16], now).unwrap();
        let ev = run_line(&mut nodes, transmits(&out).into_iter().map(|p| (0, p)).collect(), &mut now);
        assert!(ev.iter().any(|(i, a)| *i == 23 && matches!(a,
            Action::Deliver { ptype: PacketType::Data, payload, .. } if payload.as_slice() == "far away".as_bytes())));
        assert!(ev.contains(&(0, Action::Acked { msg_id: first })), "first message acknowledged via the reply");
        // The sender's route ends at the first anchor.
        let route = nodes[0].routes().get(&b).unwrap().path.clone();
        assert_eq!(route.as_slice(), &(2..=8).collect::<Vec<u16>>()[..]);

        // The next message goes along the route through both anchors and is acknowledged all the way.
        let (msg, out) = nodes[0].send_direct(b, b"again", [8; 16], now).unwrap();
        assert_eq!(Packet::decode(&transmits(&out)[0]).unwrap().header.ptype, PacketType::Data);
        let ev = run_line(&mut nodes, transmits(&out).into_iter().map(|p| (0, p)).collect(), &mut now);
        assert!(ev.iter().any(|(i, a)| *i == 23 && matches!(a,
            Action::Deliver { ptype: PacketType::Data, payload, .. } if payload.as_slice() == b"again")));
        assert!(ev.contains(&(0, Action::Acked { msg_id: msg })));
    }

    #[test]
    fn without_anchors_twenty_two_hops_is_too_far() {
        let mut nodes = line(23, &[]);
        let b = nodes[23].addr;
        let mut now = 0;
        let (_, out) = nodes[0].send_direct(b, b"far", [7; 16], now).unwrap();
        let ev = run_line(&mut nodes, transmits(&out).into_iter().map(|p| (0, p)).collect(), &mut now);
        assert!(!ev.iter().any(|(i, a)| *i == 23 && matches!(a, Action::Deliver { .. })));
    }

    #[test]
    fn drops_packets_without_valid_stamp() {
        let mut r = node(2, Role::Repeater);
        let dest = Address([5; 16]);
        // Path request without a stamp.
        let h = Header::new(PacketType::PathRequest, dest).with_path(Path::new());
        let out = r.on_receive(&encode(&h, &[9u8; 16]).unwrap(), 0, 0);
        assert!(out.is_empty());
        // Announce with a wrong stamp (a real stamp, but a different timestamp).
        let mut body: Buf = Buf::from_slice(&[1u8; 69]).unwrap();
        assert!(stamp::append(PacketType::Announce, &dest, &mut body));
        body[67] ^= 0xff;
        let h = Header::new(PacketType::Announce, dest);
        let out = r.on_receive(&encode(&h, &body).unwrap(), 0, 0);
        assert!(out.is_empty());
        assert_eq!(r.stats.bad_stamps, 2);
        assert_eq!(r.stats.rx_packets, 0);
        assert_eq!(r.stats.forwarded, 0);
    }

    #[test]
    fn stamped_announce_is_delivered_without_stamp_and_forwarded_with_it() {
        let mut r = node(2, Role::Repeater);
        let dest = Address([5; 16]);
        let body: Buf = Buf::from_slice(&[1u8; 140]).unwrap();
        let mut stamped = body.clone();
        assert!(stamp::append(PacketType::Announce, &dest, &mut stamped));
        let pkt = encode(&Header::new(PacketType::Announce, dest), &stamped).unwrap();
        let out = r.on_receive(&pkt, 0, 0);
        assert!(out.iter().any(
            |x| matches!(x, Action::Deliver { payload, .. } if payload.as_slice() == body.as_slice())
        ));
        let fwd = transmits(&out);
        assert_eq!(fwd.len(), 1);
        assert_eq!(Packet::decode(&fwd[0]).unwrap().payload, stamped.as_slice());
        assert_eq!(r.stats.bad_stamps, 0);
    }

    /// Chain: A (client) - R1 - R2 - B (client). Each only hears its neighbours.
    #[test]
    fn finds_route_delivers_and_acks_over_two_repeaters() {
        let mut a = node(1, Role::Client);
        let mut r1 = node(2, Role::Repeater);
        let mut r2 = node(3, Role::Repeater);
        let mut b = node(4, Role::Client);

        let (first_id, out) = a.send_direct(b.addr, b"hej B", [7; 16], 0).unwrap();
        let req = transmits(&out);
        assert_eq!(req.len(), 1);

        // The request (with the first message) is flooded A -> R1 -> R2 -> B.
        let f1 = transmits(&r1.on_receive(&req[0], 5, 0));
        let f2 = transmits(&r2.on_receive(&f1[0], 5, 0));
        let at_b = b.on_receive(&f2[0], 5, 0);
        assert!(at_b.iter().any(|x| matches!(x,
            Action::Deliver { ptype: PacketType::Data, payload, hops: 2, .. } if payload.as_slice() == b"hej B")));
        let resp = reply(&mut b, 0);

        // The reply goes B -> R2 -> R1 -> A as a source path and is also the acknowledgement.
        let s2 = transmits(&r2.on_receive(&resp[0], 5, 0));
        let s1 = transmits(&r1.on_receive(&s2[0], 5, 0));
        let got = a.on_receive(&s1[0], 5, 0);
        assert!(got.contains(&Action::RouteLearned {
            dest: b.addr,
            hops: 2
        }));
        assert!(got.contains(&Action::Acked { msg_id: first_id }));
        assert_eq!(a.routes().get(&b.addr).unwrap().path.as_slice(), &[2, 3]);

        // The next message uses the route: A -> R1 -> R2 -> B.
        let (msg_id, out) = a.send_direct(b.addr, b"again", [8; 16], 10).unwrap();
        let data = transmits(&out);
        assert_eq!(
            Packet::decode(&data[0]).unwrap().header.ptype,
            PacketType::Data
        );
        let d1 = transmits(&r1.on_receive(&data[0], 5, 0));
        let d2 = transmits(&r2.on_receive(&d1[0], 5, 0));
        let at_b = b.on_receive(&d2[0], 5, 0);
        assert!(at_b.iter().any(|x| matches!(x,
            Action::Deliver { ptype: PacketType::Data, payload, .. } if payload.as_slice() == b"again")));

        // Acknowledgement B -> R2 -> R1 -> A.
        let ack = transmits(&at_b);
        let k2 = transmits(&r2.on_receive(&ack[0], 5, 0));
        let k1 = transmits(&r1.on_receive(&k2[0], 5, 0));
        assert!(
            a.on_receive(&k1[0], 5, 0)
                .contains(&Action::Acked { msg_id })
        );
    }

    #[test]
    fn trace_route_records_snr_both_ways_and_neighbors() {
        let mut a = node(1, Role::Client);
        let mut r1 = node(2, Role::Repeater);
        let mut r2 = node(3, Role::Repeater);
        let mut b = node(4, Role::Client);
        // No route yet: the trace waits for the path request.
        let req = transmits(&a.send_trace(b.addr, [9; 16], 0));
        assert_eq!(Packet::decode(&req[0]).unwrap().header.ptype, PacketType::PathRequest);
        let f1 = transmits(&r1.on_receive(&req[0], 5, 0));
        let f2 = transmits(&r2.on_receive(&f1[0], 5, 0));
        b.on_receive(&f2[0], 5, 0);
        let resp = reply(&mut b, 0);
        let s2 = transmits(&r2.on_receive(&resp[0], 5, 0));
        let s1 = transmits(&r1.on_receive(&s2[0], 5, 0));
        let got = a.on_receive(&s1[0], 5, 0);
        // The route is learned, and the trace is sent now: A -> R1 -> R2 -> B.
        let t = transmits(&got);
        assert_eq!(Packet::decode(&t[0]).unwrap().header.ptype, PacketType::Probe);
        let t1 = transmits(&r1.on_receive(&t[0], 7, 0));
        let t2 = transmits(&r2.on_receive(&t1[0], -3, 0));
        let back = transmits(&b.on_receive(&t2[0], 4, 0));
        let b2 = transmits(&r2.on_receive(&back[0], 6, 0));
        let b1 = transmits(&r1.on_receive(&b2[0], 2, 0));
        let done = a.on_receive(&b1[0], 8, 0);
        let payload = done
            .iter()
            .find_map(|x| match x {
                Action::Deliver { ptype: PacketType::Probe, dest, payload, .. } if *dest == b.addr => {
                    Some(payload.clone())
                }
                _ => None,
            })
            .expect("trace reply");
        let r = diag::parse_response(&payload).unwrap();
        assert_eq!(&r.path[..], &[2, 3]);
        assert_eq!(&r.out[..], &[7, -3, 4]);
        assert_eq!(&r.back[..], &[6, 2, 8]);
        // Hop retransmission still works even though the payload grows: R1 has
        // heard R2 forward and forgets its copy.
        assert!(!r1.forwards.is_empty());
        r1.on_receive(&t2[0], 5, 0);
        assert!(r1.forwards.is_empty());
        // Neighbours: R1 has heard R2 (path ID 3) directly, A has heard R1.
        assert!(r1.neighbors.sorted().iter().any(|n| n.id == 3));
        assert!(a.neighbors.sorted().iter().any(|n| n.id == 2 && n.snr == 8));
        assert!(!a.neighbors.sorted().iter().any(|n| n.id == 3));
    }

    #[test]
    fn first_message_rides_along_with_the_path_request() {
        let mut a = node(1, Role::Client);
        let mut r = node(2, Role::Repeater);
        let mut b = node(4, Role::Client);
        let (msg_id, out) = a.send_direct(b.addr, b"right away", [7; 16], 0).unwrap();
        let req = transmits(&r.on_receive(&transmits(&out)[0], 0, 0));
        let at_b = b.on_receive(&req[0], 0, 0);
        assert!(at_b.iter().any(|x| matches!(x,
            Action::Deliver { ptype: PacketType::Data, payload, .. } if payload.as_slice() == b"right away")));
        let resp = transmits(&r.on_receive(&reply(&mut b, 0)[0], 0, 0));
        let got = a.on_receive(&resp[0], 0, 0);
        assert!(got.contains(&Action::Acked { msg_id }));
        assert!(transmits(&got).is_empty(), "the message is not sent again");
    }

    #[test]
    fn clients_do_not_forward() {
        let mut a = node(1, Role::Client);
        let mut c = node(2, Role::Client);
        let out = a.send_channel(Address([9; 16]), b"x").unwrap();
        assert!(transmits(&c.on_receive(&transmits(&out)[0], 0, 0)).is_empty());
        let (_, out) = a.send_direct(Address([5; 16]), b"x", [1; 16], 0).unwrap();
        assert!(transmits(&c.on_receive(&transmits(&out)[0], 0, 0)).is_empty());
    }

    /// The router's size in the firmware (the nRF52840 has 256 KB RAM). Run with
    /// `--nocapture` to see the number. Not with `big-carry` (the simulator's
    /// bigger carry store, never in firmware), which a workspace build turns on.
    #[test]
    #[cfg(not(feature = "big-carry"))]
    fn router_size_in_firmware() {
        let size = core::mem::size_of::<Router<256, 1024>>();
        std::println!("size_of::<Router<256, 1024>>() = {size} bytes");
        assert!(size < 96 * 1024);
    }

    #[test]
    fn time_is_learned_from_a_neighbours_hello() {
        let mut a = node(1, Role::Repeater);
        let mut b = node(2, Role::Client);
        let t = 1_800_000_000;
        a.clock.set(t, 0, 1);
        a.mail_waiting.push(b.addr.path_id()).unwrap();
        let pkt = encode(&Header::new(PacketType::Hello, a.addr), &a.hello_payload(5_000)).unwrap();
        b.on_receive(&pkt, 0, 5_000);
        assert_eq!(b.clock.now(5_000), Some((t + 5, 2)));
        // The "mail waiting" ID after the time is still read.
        assert!(b.take_mail_hint());
        // The time does not change the Trickle signature.
        assert_eq!(a.hello_signature(1_000), a.hello_signature(60_000));
    }

    #[test]
    fn duplicates_are_dropped() {
        let mut a = node(1, Role::Client);
        let mut r = node(2, Role::Repeater);
        let out = a.send_channel(Address([9; 16]), b"x").unwrap();
        let pkt = &transmits(&out)[0];
        assert_eq!(transmits(&r.on_receive(pkt, 0, 0)).len(), 1);
        // A weak copy is not handled again (and cancels nothing).
        assert!(r.on_receive(pkt, -15, 0).is_empty());
        // A strong copy is not handled again either; it only cancels.
        assert!(transmits(&r.on_receive(pkt, 0, 0)).is_empty());
    }

    #[test]
    fn channel_delivered_to_subscribers_and_limited_in_scope() {
        let ch = Address([9; 16]);
        let mut a = node(1, Role::Client);
        let mut r = node(2, Role::Repeater);
        r.subscribe(ch).unwrap();
        let out = a.send_channel(ch, b"hi channel").unwrap();
        let mut pkt = transmits(&out)[0].clone();
        let got = r.on_receive(&pkt, 0, 0);
        assert!(got.iter().any(|x| matches!(x, Action::Deliver { .. })));
        assert_eq!(transmits(&got).len(), 1);

        // At the limit there is no forwarding.
        let mut r2 = node(3, Role::Repeater);
        pkt[1] = LOCAL_SCOPE;
        assert!(transmits(&r2.on_receive(&pkt, 0, 0)).is_empty());
    }

    #[test]
    fn scoped_channel_forwarded_only_inside_region() {
        use crate::region::region_id;
        let (dk, nj) = (region_id("dk"), region_id("nordjylland"));
        let ch = Address([9; 16]);
        let mut a = node(1, Role::Client);
        let pkt = transmits(&a.send_channel_in(ch, nj, b"hej").unwrap())[0].clone();
        assert_eq!(Packet::decode(&pkt).unwrap().header.region, nj);

        // Aalborg: dk + nordjylland -> forwards, the region is kept.
        let mut aalborg = node(2, Role::Repeater);
        aalborg.regions = crate::region::Regions::from_slice(&[dk, nj]).unwrap();
        let fwd = transmits(&aalborg.on_receive(&pkt, 0, 0));
        assert_eq!(fwd.len(), 1);
        assert_eq!(Packet::decode(&fwd[0]).unwrap().header.region, nj);

        // Aarhus: only dk -> does not forward, but delivers to its own user.
        let mut aarhus = node(3, Role::Repeater);
        aarhus.regions = crate::region::Regions::from_slice(&[dk]).unwrap();
        aarhus.subscribe(ch).unwrap();
        let got = aarhus.on_receive(&pkt, 0, 0);
        assert!(transmits(&got).is_empty());
        assert!(got.iter().any(|x| matches!(x, Action::Deliver { .. })));
        // ... but a dk message and a global message are forwarded.
        let dkpkt = transmits(&a.send_channel_in(ch, dk, b"x").unwrap())[0].clone();
        assert_eq!(transmits(&aarhus.on_receive(&dkpkt, 0, 0)).len(), 1);
        let global = transmits(&a.send_channel(ch, b"y").unwrap())[0].clone();
        assert_eq!(transmits(&aarhus.on_receive(&global, 0, 0)).len(), 1);

        // Empty list: everything is forwarded; strict: only global.
        let mut open = node(4, Role::Repeater);
        assert_eq!(transmits(&open.on_receive(&pkt, 0, 0)).len(), 1);
        let mut strict = node(5, Role::Repeater);
        strict.strict_region = true;
        assert!(transmits(&strict.on_receive(&pkt, 0, 0)).is_empty());
        let global2 = transmits(&a.send_channel(ch, b"z").unwrap())[0].clone();
        assert_eq!(transmits(&strict.on_receive(&global2, 0, 0)).len(), 1);
    }

    #[test]
    fn far_repeaters_forward_first_and_low_battery_waits() {
        let r = node(2, Role::Repeater);
        assert!(r.flood_delay_ms(-15) < r.flood_delay_ms(10));
        let mut low = node(3, Role::Repeater);
        low.energy = Energy::Low;
        assert!(low.flood_delay_ms(-15) > r.flood_delay_ms(-15));
    }

    #[test]
    fn low_energy_skips_channels_critical_skips_all() {
        let mut a = node(1, Role::Client);
        let mut r = node(2, Role::Repeater);
        r.energy = Energy::Low;
        let ch = transmits(&a.send_channel(Address([9; 16]), b"x").unwrap())[0].clone();
        assert!(transmits(&r.on_receive(&ch, 0, 0)).is_empty());
        let (_, out) = a.send_direct(Address([5; 16]), b"x", [1; 16], 0).unwrap();
        let req = transmits(&out)[0].clone();
        assert_eq!(transmits(&r.on_receive(&req, 0, 0)).len(), 1);

        let mut dead = node(3, Role::Repeater);
        dead.energy = Energy::Critical;
        assert!(transmits(&dead.on_receive(&req, 0, 0)).is_empty());
    }

    #[test]
    fn only_the_named_repeater_forwards_source_routed() {
        let mut r = node(2, Role::Repeater);
        let path = Path::from_slice(&[9, 2]).unwrap();
        let header = Header::new(PacketType::Data, Address([5; 16])).with_path(path);
        let pkt = encode(&header, b"x").unwrap();
        assert!(r.on_receive(&pkt, 0, 0).is_empty(), "hop 0 is repeater 9");
        let mut h2 = header.clone();
        h2.hops = 1;
        let pkt = encode(&h2, b"x").unwrap();
        let fwd = transmits(&r.on_receive(&pkt, 0, 0));
        assert_eq!(Packet::decode(&fwd[0]).unwrap().header.hops, 2);
    }

    #[test]
    fn destination_overhearing_early_still_gets_final_copy() {
        let mut b = node(4, Role::Client);
        let path = Path::from_slice(&[2, 3]).unwrap();
        let mut h = Header::new(PacketType::Data, b.addr).with_path(path);
        h.hops = 1; // B hears it while it is on its way to R2
        assert!(b.on_receive(&encode(&h, b"x").unwrap(), 0, 0).is_empty());
        h.hops = 2;
        let got = b.on_receive(&encode(&h, b"x").unwrap(), 0, 0);
        assert!(got.iter().any(|x| matches!(x, Action::Deliver { .. })));
    }

    #[test]
    fn flood_is_cancelled_after_hearing_enough_copies() {
        let mut a = node(1, Role::Client);
        let mut r = node(2, Role::Repeater);
        let pkt = transmits(&a.send_channel(Address([9; 16]), b"x").unwrap())[0].clone();
        let out = r.on_receive(&pkt, 0, 0);
        let (fwd, id) = match &out[0] {
            Action::Transmit { packet, id, .. } => (packet.clone(), *id),
            _ => panic!("forventede videresendelse"),
        };
        // A weak copy comes from far away: it does not cover our area.
        assert!(r.on_receive(&fwd, -15, 0).is_empty());
        // A strong copy from a nearby neighbour: the area is covered, cancel.
        assert_eq!(
            r.on_receive(&fwd, 0, 0).as_slice(),
            &[Action::Cancel { id }]
        );
    }

    #[test]
    fn hop_is_retransmitted_until_next_repeater_is_heard() {
        let mut r = node(2, Role::Repeater);
        let header = Header::new(PacketType::Data, Address([5; 16]))
            .with_path(Path::from_slice(&[2, 3]).unwrap());
        let pkt = encode(&header, b"x").unwrap();
        assert_eq!(transmits(&r.on_receive(&pkt, 0, 0)).len(), 1);
        let d = r.next_deadline().expect("waiting for the next hop");
        assert!(r.tick(d - 1).is_empty());
        assert_eq!(transmits(&r.tick(d)).len(), 1, "gensendes");
        // Now we hear repeater 3 forward.
        let mut h2 = header.clone();
        h2.hops = 2;
        r.on_receive(&encode(&h2, b"x").unwrap(), 0, d + 10);
        assert!(r.next_deadline().is_none());
    }

    #[test]
    fn hop_retries_give_up() {
        let mut r = node(2, Role::Repeater);
        let header = Header::new(PacketType::Data, Address([5; 16]))
            .with_path(Path::from_slice(&[2, 3]).unwrap());
        r.on_receive(&encode(&header, b"x").unwrap(), 0, 0);
        let mut sent = 0;
        while let Some(d) = r.next_deadline() {
            sent += transmits(&r.tick(d)).len();
        }
        assert_eq!(sent, HOP_RETRIES as usize);
    }

    #[test]
    fn dead_next_hop_is_bypassed() {
        let mut r = node(2, Role::Repeater);
        // We hear repeater 4, which comes after 3 on the path.
        r.neighbors.heard(4, None, 5, 0, 0);
        let header = Header::new(PacketType::Data, Address([5; 16]))
            .with_path(Path::from_slice(&[2, 3, 4]).unwrap());
        r.on_receive(&encode(&header, b"x").unwrap(), 0, 0);
        // Repeater 3 never forwards: after the retries the packet goes to 4.
        let mut last = Vec::new();
        while let Some(d) = r.next_deadline() {
            if d > 60_000 {
                break;
            }
            last = transmits(&r.tick(d));
            if last.iter().any(|p| Packet::decode(p).unwrap().header.path.as_ref().unwrap().len() == 2) {
                break;
            }
        }
        let p = Packet::decode(&last[0]).unwrap();
        assert_eq!(p.header.path.as_ref().unwrap().as_slice(), &[2, 4]);
        assert_eq!(p.header.hops, 1);
        assert_eq!(r.stats.local_repairs, 1);
        // The next packet skips 3 at once.
        let out = transmits(&r.on_receive(&encode(&header, b"y").unwrap(), 0, 30_000));
        let p = Packet::decode(&out[0]).unwrap();
        assert_eq!(p.header.path.as_ref().unwrap().as_slice(), &[2, 4]);
        // Hearing 3 again makes it usable again.
        let mut h3 = header.clone();
        h3.hops = 2;
        r.on_receive(&encode(&h3, b"z").unwrap(), 0, 31_000);
        let out = transmits(&r.on_receive(&encode(&header, b"w").unwrap(), 0, 32_000));
        assert_eq!(Packet::decode(&out[0]).unwrap().header.path.as_ref().unwrap().as_slice(), &[2, 3, 4]);
    }

    #[test]
    fn dead_last_repeater_is_bypassed_to_the_destination() {
        let mut r = node(2, Role::Repeater);
        let dest = Address([5; 16]);
        r.neighbors.heard(dest.path_id(), Some(dest), 5, 0, 0);
        let header = Header::new(PacketType::Data, dest).with_path(Path::from_slice(&[2, 3]).unwrap());
        r.on_receive(&encode(&header, b"x").unwrap(), 0, 0);
        let mut found = false;
        while let Some(d) = r.next_deadline() {
            for p in transmits(&r.tick(d)) {
                let p = Packet::decode(&p).unwrap();
                if p.header.path.as_ref().unwrap().as_slice() == [2] && p.header.hops == 1 {
                    found = true;
                }
            }
        }
        assert!(found, "sent straight to the destination");
    }

    #[test]
    fn no_bypass_without_a_node_in_range() {
        let mut r = node(2, Role::Repeater);
        let header = Header::new(PacketType::Data, Address([5; 16]))
            .with_path(Path::from_slice(&[2, 3, 4]).unwrap());
        r.on_receive(&encode(&header, b"x").unwrap(), 0, 0);
        while let Some(d) = r.next_deadline() {
            r.tick(d);
        }
        assert_eq!(r.stats.local_repairs, 0);
    }

    #[test]
    fn last_hop_is_not_tracked() {
        let mut r = node(3, Role::Repeater);
        let mut header = Header::new(PacketType::Data, Address([5; 16]))
            .with_path(Path::from_slice(&[2, 3]).unwrap());
        header.hops = 1;
        r.on_receive(&encode(&header, b"x").unwrap(), 0, 0);
        assert!(r.next_deadline().is_none());
    }

    #[test]
    fn repeated_message_is_acked_again_but_delivered_once() {
        let mut b = node(4, Role::Client);
        let header = Header::new(PacketType::Data, b.addr).with_path(Path::new());
        let pkt = encode(&header, b"x").unwrap();
        let first = b.on_receive(&pkt, 0, 0);
        let second = b.on_receive(&pkt, 0, 0);
        assert!(first.iter().any(|x| matches!(x, Action::Deliver { .. })));
        assert!(!second.iter().any(|x| matches!(x, Action::Deliver { .. })));
        assert_eq!(transmits(&second).len(), 1, "new acknowledgement");
    }

    #[test]
    fn relay_forwards_within_its_limit_and_waits_longer() {
        let mut a = node(1, Role::Client);
        let mut relay = node(2, Role::Relay);
        let rep = node(3, Role::Repeater);
        assert!(
            relay.flood_delay_ms(0) > rep.flood_delay_ms(0),
            "rigtige repeatere vinder"
        );
        let mut pkt = transmits(&a.send_channel(Address([9; 16]), b"x").unwrap())[0].clone();
        assert_eq!(transmits(&relay.on_receive(&pkt, 0, 0)).len(), 1);
        let mut relay2 = node(4, Role::Relay);
        pkt[1] = DEFAULT_RELAY_HOPS; // already 4 hops
        assert!(transmits(&relay2.on_receive(&pkt, 0, 0)).is_empty());
    }

    #[test]
    fn fragments_need_a_route_and_are_acked() {
        let mut a = node(1, Role::Client);
        let mut b = node(4, Role::Client);
        assert_eq!(
            a.send_fragment(b.addr, b"bid", 0).err(),
            Some(SendError::NoRoute)
        );
        // Learn a direct route (neighbours, empty path).
        let (_, out) = a.send_direct(b.addr, b"offer", [1; 16], 0).unwrap();
        b.on_receive(&transmits(&out)[0], 0, 0);
        let resp = reply(&mut b, 0);
        a.on_receive(&resp[0], 0, 0);
        assert!(a.has_route(&b.addr));
        let (id, out) = a.send_fragment(b.addr, b"bid 0", 0).unwrap();
        let at_b = b.on_receive(&transmits(&out)[0], 0, 0);
        assert!(at_b.iter().any(|x| matches!(
            x,
            Action::Deliver {
                ptype: PacketType::Fragment,
                ..
            }
        )));
        let ack = transmits(&at_b);
        assert!(
            a.on_receive(&ack[0], 0, 0)
                .contains(&Action::Acked { msg_id: id })
        );
    }

    #[test]
    fn set_slot_scales_timeouts_only_when_asked() {
        let mut a = node(1, Role::Client);
        a.set_slot(1_000, false);
        assert_eq!(a.ack_timeout_ms, DEFAULT_ACK_TIMEOUT_MS);
        a.set_slot(1_000, true);
        assert_eq!(a.ack_timeout_ms, 5 * DEFAULT_ACK_TIMEOUT_MS);
        assert_eq!(a.path_retry_ms as u64, 5 * PATH_RETRY_MS);
        assert_eq!(a.hop_timeout_ms, 5 * DEFAULT_HOP_TIMEOUT_MS);
        a.set_slot(100, true);
        assert_eq!(a.hop_timeout_ms, DEFAULT_HOP_TIMEOUT_MS, "never below today");
        let (_, out) = a.send_direct(Address([5; 16]), b"x", [1; 16], 0).unwrap();
        assert_eq!(transmits(&out).len(), 1);
        assert_eq!(a.next_deadline(), Some(PATH_RETRY_MS));
    }

    #[test]
    fn path_request_quota_under_high_load() {
        let mut a = node(1, Role::Client);
        a.load_control = true;
        // A busy channel: 40 s of airtime in the last minute.
        for t in 0..40u64 {
            a.note_airtime(t * 1_000, 1_000);
        }
        let now = 40_000;
        let normal = node(1, Role::Client).flood_delay_ms(0);
        assert_eq!(a.flood_delay_ms(0), 2 * normal);
        let mut sent = 0;
        for k in 0..(crate::qos::PATH_QUOTA_PER_MIN as u8 + 2) {
            let (_, out) = a.send_direct(Address([10 + k; 16]), b"x", [k; 16], now).unwrap();
            sent += transmits(&out).len();
        }
        assert_eq!(sent, crate::qos::PATH_QUOTA_PER_MIN as usize);
        assert_eq!(a.stats.quota_held, 2);
        // The held requests go out at the next minute.
        let out = a.tick(60_000);
        assert_eq!(transmits(&out).len(), 2);
    }

    #[test]
    fn unanswered_path_request_is_retried_with_new_token() {
        let mut a = node(1, Role::Client);
        let (_, out) = a.send_direct(Address([5; 16]), b"x", [1; 16], 0).unwrap();
        let first = transmits(&out)[0].clone();
        let d = a.next_deadline().unwrap();
        assert_eq!(d, PATH_RETRY_MS);
        let again = transmits(&a.tick(d));
        assert_eq!(again.len(), 1);
        assert_ne!(again[0], first, "new token");
        let d2 = a.next_deadline().unwrap();
        assert_eq!(transmits(&a.tick(d2)).len(), 1);
        let d3 = a.next_deadline().unwrap();
        assert!(transmits(&a.tick(d3)).is_empty());
        assert!(
            a.next_deadline().is_none(),
            "gives up after {PATH_RETRIES} repetitions"
        );
        // The same message again: no duplicate in the queue, but a new request.
        let (_, out) = a
            .send_direct(Address([5; 16]), b"x", [2; 16], 100_000)
            .unwrap();
        assert_eq!(transmits(&out).len(), 1);
    }

    #[test]
    fn statistics_are_counted() {
        let mut a = node(1, Role::Client);
        let mut r = node(2, Role::Repeater);
        let out = a.send_channel(Address([9; 16]), b"x").unwrap();
        let pkt = transmits(&out)[0].clone();
        r.on_receive(&pkt, 0, 0);
        r.on_receive(&pkt, 0, 0);
        r.on_receive(&[1, 2, 3], 0, 0);
        assert_eq!(a.stats.originated, 1);
        assert_eq!(r.stats.rx_packets, 2);
        assert_eq!(r.stats.rx_duplicates, 1);
        assert_eq!(r.stats.rx_invalid, 1);
        assert_eq!(r.stats.forwarded, 1);
    }

    #[test]
    fn missing_ack_repairs_route_and_resends() {
        let mut a = node(1, Role::Client);
        let mut b = node(4, Role::Client);
        // Learn a route (direct neighbours) with the first message.
        let (_, out) = a.send_direct(b.addr, b"1", [1; 16], 0).unwrap();
        b.on_receive(&transmits(&out)[0], 0, 0);
        let resp = reply(&mut b, 0);
        a.on_receive(&resp[0], 0, 0);
        assert!(a.has_route(&b.addr));
        // The next message is sent along the route, but B has moved and does not answer.
        let (_, out) = a.send_direct(b.addr, b"2", [2; 16], 1000).unwrap();
        assert_eq!(
            Packet::decode(&transmits(&out)[0]).unwrap().header.ptype,
            PacketType::Data
        );
        let t = 1000 + DEFAULT_ACK_TIMEOUT_MS as u64;
        let again = transmits(&a.tick(t));
        assert!(!a.has_route(&b.addr), "the route is forgotten");
        let req = again
            .iter()
            .find(|p| Packet::decode(p).unwrap().header.ptype == PacketType::PathRequest);
        let req = req.expect("new path request with the message");
        // B (now somewhere else) receives the request, gets the message and answers.
        let at_b = b.on_receive(req, 0, t);
        assert!(
            at_b.iter().any(
                |x| matches!(x, Action::Deliver { payload, .. } if payload.as_slice() == b"2")
            )
        );
        let got = a.on_receive(&reply(&mut b, t)[0], 0, t);
        assert!(got.iter().any(|x| matches!(x, Action::Acked { .. })));
        assert!(a.next_deadline().is_none(), "nothing more to wait for");
    }

    #[test]
    fn mobile_node_refreshes_old_routes() {
        let mut a = node(1, Role::Client);
        a.mobile = true;
        let mut b = node(4, Role::Client);
        let (_, out) = a.send_direct(b.addr, b"1", [1; 16], 0).unwrap();
        b.on_receive(&transmits(&out)[0], 0, 0);
        let resp = reply(&mut b, 0);
        a.on_receive(&resp[0], 0, 0);
        let (_, out) = a.send_direct(b.addr, b"2", [2; 16], 10_000).unwrap();
        assert_eq!(
            Packet::decode(&transmits(&out)[0]).unwrap().header.ptype,
            PacketType::Data
        );
        let (_, out) = a
            .send_direct(b.addr, b"3", [3; 16], MOBILE_ROUTE_MS + 1)
            .unwrap();
        assert_eq!(
            Packet::decode(&transmits(&out)[0]).unwrap().header.ptype,
            PacketType::PathRequest
        );
    }

    #[test]
    fn destination_prefers_a_strong_last_hop() {
        let mut b = node(4, Role::Client);
        let token = [9u8; 16];
        let dest = b.addr;
        let mk = |path: &[u16]| {
            let mut h = Header::new(PacketType::PathRequest, dest).with_path(Path::from_slice(path).unwrap());
            h.hops = path.len() as u8;
            let mut p: Buf = Buf::from_slice(&token).unwrap();
            assert!(stamp::append(PacketType::PathRequest, &dest, &mut p));
            encode(&h, &p).unwrap()
        };
        // First copy: 1 hop, but almost at the noise limit.
        b.on_receive(&mk(&[7]), -11, 0);
        // Second copy: 2 hops with a strong last hop.
        b.on_receive(&mk(&[5, 6]), 5, 10);
        let resp = reply(&mut b, 10);
        let p = Packet::decode(&resp[0]).unwrap();
        assert_eq!(p.header.path.unwrap().as_slice(), &[6, 5]);
    }

    /// A path request to someone else, as forwarded along `path`.
    fn forwarded_request(token: u8, path: &[u16]) -> Buf {
        let dest = Address([0x55; 16]);
        let mut h = Header::new(PacketType::PathRequest, dest).with_path(Path::from_slice(path).unwrap());
        h.hops = path.len() as u8;
        let mut p: Buf = Buf::from_slice(&[token; 16]).unwrap();
        assert!(stamp::append(PacketType::PathRequest, &dest, &mut p));
        encode(&h, &p).unwrap()
    }

    const REP: u16 = 0x0303;

    #[test]
    fn hello_tells_neighbors_role_coverage_and_bridge_choice() {
        let mut a = node(2, Role::Relay);
        a.hello = true;
        a.mobile = true;
        a.note_role(REP, Role::Repeater, None, 0, 5, 0);
        // The first tick plans the first hello within the shortest interval.
        let _ = a.tick(0);
        let first = a.next_deadline().unwrap();
        assert!(first <= HELLO_MIN_MS);
        let out = a.tick(first);
        let sent = transmits(&out);
        assert_eq!(sent.len(), 1, "one hello");
        let mut b = node(3, Role::Client);
        b.on_receive(&sent[0], 5, 100);
        let n = b.neighbors.get(2).expect("a is a neighbor now");
        assert_eq!(n.role, Some(Role::Relay));
        assert_eq!(n.hears_repeater, Some(true));
        assert!(b.bridges_heard(100) == 1, "a relay that hears a repeater is a bridge");
        // Hellos are never forwarded.
        let mut r = node(4, Role::Repeater);
        assert!(transmits(&r.on_receive(&sent[0], 5, 100)).is_empty());
    }

    #[test]
    fn hello_trickle_doubles_and_resets_on_change() {
        let mut a = node(2, Role::Client);
        a.hello = true;
        let mut t = 0;
        let mut gaps = Vec::new();
        let mut last = 0;
        for _ in 0..6 {
            t = a.next_deadline().unwrap().max(t);
            if !transmits(&a.tick(t)).is_empty() {
                gaps.push(t - last);
                last = t;
            }
        }
        assert!(gaps.len() >= 4);
        assert!(gaps[3] > gaps[1], "intervals grow: {gaps:?}");
        // A role change brings the next hello forward to within HELLO_MIN_MS.
        a.set_role(Role::Relay);
        let _ = a.tick(t + 1);
        assert!(a.next_deadline().unwrap() <= t + 1 + HELLO_MIN_MS);
    }

    #[test]
    fn ring_search_stops_at_the_ring_and_the_retry_goes_everywhere() {
        let mut a = node(2, Role::Client);
        a.ring = 3;
        // Heard right next to us: ring = 0 + 2 hops of margin = 2.
        a.note_heard_hops(Address([0x66; 16]), 0, 0);
        let out = a.send_direct(Address([0x66; 16]), b"hi", [5; 16], 0).unwrap().1;
        let first = transmits(&out);
        assert_eq!(first.len(), 1);
        // Hop 1 forwards (hops 0 -> 1), hop 2 must not.
        let mut r1 = node(3, Role::Repeater);
        let f1 = transmits(&r1.on_receive(&first[0], 5, 10));
        assert_eq!(f1.len(), 1, "hop 1 forwards");
        let mut r1b = node(7, Role::Repeater);
        assert!(transmits(&r1b.on_receive(&f1[0], 5, 20)).is_empty(), "ring 2: hop 2 is the edge");
        // A target never heard: no ring at all.
        let mut c = node(8, Role::Client);
        c.ring = 3;
        let far = transmits(&c.send_direct(Address([0x67; 16]), b"x", [6; 16], 0).unwrap().1);
        let mut q = node(9, Role::Repeater);
        let q1 = transmits(&q.on_receive(&far[0], 5, 10));
        let mut q2 = node(10, Role::Repeater);
        assert_eq!(transmits(&q2.on_receive(&q1[0], 5, 20)).len(), 1, "unknown target: no ring");
        // After RING_RETRY_MS the request is repeated without a limit.
        let retry = transmits(&a.tick(RING_RETRY_MS + 1));
        assert_eq!(retry.len(), 1);
        let mut r2 = node(4, Role::Repeater);
        assert_eq!(transmits(&r2.on_receive(&retry[0], 5, RING_RETRY_MS + 10)).len(), 1, "the retry is not ring-limited");
    }

    #[test]
    fn mail_sync_travels_two_hops_and_hello_names_recipients() {
        let mut a = node(2, Role::Client);
        let sent = transmits(&a.request_mail(Some(6), &[1, 2]));
        let mut r1 = node(3, Role::Repeater);
        let out = r1.on_receive(&sent[0], 5, 10);
        assert!(out.iter().any(|x| matches!(x, Action::Deliver { ptype: PacketType::MailSync, .. })));
        let fwd = transmits(&out);
        assert_eq!(fwd.len(), 1, "first hop forwards");
        let mut r2 = node(4, Role::Repeater);
        let out2 = r2.on_receive(&fwd[0], 5, 20);
        let fwd2 = transmits(&out2);
        assert_eq!(fwd2.len(), 1, "second hop forwards");
        let mut r3 = node(5, Role::Repeater);
        assert!(transmits(&r3.on_receive(&fwd2[0], 5, 30)).is_empty(), "not past MAIL_SCOPE");
        // A post office names node 2 in its hello: node 2 gets the hint.
        let mut po = node(6, Role::Repeater);
        po.hello = true;
        po.mail_class = crate::mail::CLASS_LARGE;
        let _ = po.mail_waiting.push(2);
        let _ = po.tick(0);
        let t = po.next_deadline().unwrap();
        let hello = transmits(&po.tick(t));
        assert_eq!(hello.len(), 1);
        a.on_receive(&hello[0], 5, t + 1);
        assert!(a.take_mail_hint());
        assert!(!a.take_mail_hint(), "taken once");
        assert_eq!(a.best_post_office(t + 1).map(|(id, _, c)| (id, c)), Some((6, crate::mail::CLASS_LARGE)));
    }

    #[test]
    fn key_request_is_delivered_and_forwarded_locally() {
        let mut a = node(2, Role::Client);
        let want = Address([0x77; 16]);
        let out = a.request_key(want);
        let sent = transmits(&out);
        assert_eq!(sent.len(), 1);
        let mut r = node(3, Role::Repeater);
        let out = r.on_receive(&sent[0], 5, 10);
        assert!(out.iter().any(|x| matches!(x, Action::Deliver { ptype: PacketType::KeyRequest, .. })));
        assert_eq!(transmits(&out).len(), 1, "a repeater forwards it");
    }

    #[test]
    fn boundary_lets_a_repeated_inward_request_through() {
        let mut b = node(2, Role::Relay);
        b.boundary = true;
        b.note_role(REP, Role::Repeater, None, 0, 5, 0);
        // First inward request for an unknown destination: dropped.
        assert!(transmits(&b.on_receive(&forwarded_request(1, &[REP]), 5, 10)).is_empty());
        // The originator tries again (new token): let through.
        assert_eq!(transmits(&b.on_receive(&forwarded_request(2, &[REP]), 5, 20)).len(), 1);
    }

    #[test]
    fn edge_bridge_needs_a_repeater_and_an_uncovered_forwarder() {
        let mut relay = node(2, Role::Relay);
        assert!(!relay.is_edge_bridge(0));
        relay.note_role(REP, Role::Repeater, None, 0, 5, 0);
        assert!(!relay.is_edge_bridge(0), "no group neighbors yet");
        relay.on_receive(&forwarded_request(1, &[0x0707]), 5, 10);
        assert!(relay.is_edge_bridge(10), "0x0707 forwards and no repeater has echoed it");
        // The repeater re-forwards a flood from 0x0707: the repeater hears it.
        relay.on_receive(&forwarded_request(2, &[0x0707, REP]), 5, 20);
        assert!(!relay.is_edge_bridge(20), "the group is covered by the repeater");
        // Observations expire.
        assert!(!relay.is_edge_bridge(20 + EDGE_WINDOW_MS));
        // Repeaters and clients are never edge bridges.
        let mut rep = node(3, Role::Repeater);
        rep.note_role(0x0909, Role::Repeater, None, 0, 5, 0);
        rep.on_receive(&forwarded_request(3, &[0x0707]), 5, 10);
        assert!(!rep.is_edge_bridge(10));
    }

    #[test]
    fn edge_bridge_ignores_group_copies_but_not_repeater_copies() {
        let mut relay = node(2, Role::Relay);
        relay.note_role(REP, Role::Repeater, None, 0, 5, 0);
        // Becomes an edge bridge and schedules forwarding of flood 7.
        assert_eq!(transmits(&relay.on_receive(&forwarded_request(7, &[0x0707]), 5, 10)).len(), 1);
        assert!(relay.is_edge_bridge(10));
        // A strong copy from another group member does not silence it ...
        let out = relay.on_receive(&forwarded_request(7, &[0x0808]), 10, 20);
        assert!(!out.iter().any(|a| matches!(a, Action::Cancel { .. })));
        // ... but a copy from the repeater does: the repeater side is covered.
        let out = relay.on_receive(&forwarded_request(7, &[0x0808, REP]), 10, 30);
        assert!(out.iter().any(|a| matches!(a, Action::Cancel { .. })));
        // Without the role information it behaves as before.
        let mut plain = node(4, Role::Relay);
        plain.on_receive(&forwarded_request(7, &[0x0707]), 5, 10);
        let out = plain.on_receive(&forwarded_request(7, &[0x0808]), 10, 20);
        assert!(out.iter().any(|a| matches!(a, Action::Cancel { .. })));
    }

    #[test]
    fn carry_hands_off_and_delivers() {
        let mut a = node(1, Role::Relay);
        let mut b = node(4, Role::Relay);
        let mut dest = node(5, Role::Client);
        a.carry_copies = 4;
        b.carry_copies = 4;
        // A gives up on a message to `dest` and starts carrying it.
        a.start_carry(dest.addr, b"sealed", 0);
        assert_eq!(a.carried_count(), 1);
        // A meets B (heard recently): half the tokens go to B.
        a.neighbors.heard(b.id, None, 5, 0, 1_000);
        let out = a.tick(10_000);
        let pkt = transmits(&out)[0].clone();
        assert_eq!(Packet::decode(&pkt).unwrap().header.ptype, PacketType::Carry);
        assert!(b.on_receive(&pkt, 5, 10_100).is_empty());
        assert_eq!(b.carried_count(), 1, "B carries a copy");
        // B meets the destination and hands it over; it is delivered as Data.
        b.neighbors.heard(dest.id, None, 5, 0, 20_000);
        let out = b.tick(20_000);
        let pkt = transmits(&out)[0].clone();
        let got = dest.on_receive(&pkt, 5, 20_100);
        assert!(got.iter().any(|x| matches!(x, Action::Deliver { ptype: PacketType::Data, payload, .. } if payload.as_slice() == b"sealed")));
        // A second copy is not delivered again (but the notice is repeated).
        assert!(!dest.on_receive(&pkt, 5, 20_200).iter().any(|x| matches!(x, Action::Deliver { .. })));
        // Copies expire.
        let _ = b.tick(20_000 + CARRY_TTL_MS + 1);
        assert_eq!(b.carried_count(), 0);
        // Off by default: a node without carrying ignores carried messages.
        let mut off = node(6, Role::Relay);
        let p = {
            let mut c = node(7, Role::Relay);
            c.carry_copies = 4;
            c.start_carry(dest.addr, b"x", 0);
            c.neighbors.heard(off.id, None, 5, 0, 1_000);
            transmits(&c.tick(5_000))[0].clone()
        };
        let _ = off.on_receive(&p, 5, 5_100);
        assert_eq!(off.carried_count(), 0);
    }

    #[test]
    fn delivered_notice_stops_carriers_and_acks_the_sender() {
        let mut a = node(1, Role::Relay);
        let mut b = node(4, Role::Relay);
        let mut dest = node(5, Role::Client);
        a.carry_copies = 4;
        b.carry_copies = 4;
        let msg_id = packet_id(PacketType::Data, &dest.addr, b"sealed");
        a.start_carry(dest.addr, b"sealed", 0);
        a.neighbors.heard(b.id, None, 5, 0, 1_000);
        let pkt = transmits(&a.tick(10_000))[0].clone();
        b.on_receive(&pkt, 5, 10_100);
        assert_eq!((a.carried_count(), b.carried_count()), (1, 1));
        // B hands the message to the destination, which answers with a notice.
        b.neighbors.heard(dest.id, None, 5, 0, 20_000);
        let pkt = transmits(&b.tick(20_000))[0].clone();
        let got = dest.on_receive(&pkt, 5, 20_100);
        let notice = transmits(&got)[0].clone();
        assert_eq!(notice.len(), crate::packet::FIXED_HEADER, "the notice is a bare header");
        assert_eq!(dest.stats.carry_notices, 1);
        // B drops its copy and passes the notice on (A gave it its copy).
        let out = b.on_receive(&notice, 5, 20_200);
        assert_eq!(b.carried_count(), 0);
        let relayed = transmits(&out)[0].clone();
        // A, the sender, gets "delivered (carried)" and stops.
        let out = a.on_receive(&relayed, 5, 20_300);
        assert!(out.contains(&Action::CarryAcked { msg_id }));
        assert_eq!(a.carried_count(), 0);
        // A later handoff of the same message to B is answered with the notice.
        let mut c = node(6, Role::Relay);
        c.carry_copies = 4;
        c.start_carry(dest.addr, b"sealed", 0);
        c.neighbors.heard(b.id, None, 5, 0, 30_000);
        let pkt = transmits(&c.tick(30_000))[0].clone();
        let out = b.on_receive(&pkt, 5, 30_100);
        assert_eq!(b.carried_count(), 0, "B does not store a delivered message");
        let back = transmits(&out)[0].clone();
        assert!(c.on_receive(&back, 5, 30_200).contains(&Action::CarryAcked { msg_id }));
        assert_eq!(c.carried_count(), 0);
        // Old nodes (and nodes with carrying off) ignore the notice.
        let mut off = node(7, Role::Relay);
        assert!(off.on_receive(&notice, 5, 40_000).is_empty());
    }

    #[test]
    fn edge_bridge_trusts_the_neighbors_own_report() {
        let mut relay = node(2, Role::Relay);
        relay.note_role(REP, Role::Repeater, None, 0, 5, 0);
        assert!(relay.hears_repeater(0));
        // A neighbor that says it hears a repeater: not an edge.
        relay.note_role(0x0707, Role::Relay, Some(true), 0, 5, 0);
        relay.on_receive(&forwarded_request(1, &[0x0707]), 5, 10);
        assert!(!relay.is_edge_bridge(10));
        // One that says it does not: edge, even before it forwards anything.
        relay.note_role(0x0808, Role::Client, Some(false), 0, 5, 20);
        assert!(relay.is_edge_bridge(20));
    }

    // ----- delivery proof -----

    const PROOF: Proof = [7, 1, 8, 2, 8, 1, 8, 2];

    fn acked(events: &[(usize, Action)], msg_id: u64) -> bool {
        events.iter().any(|(i, a)| *i == 0 && *a == Action::Acked { msg_id })
    }

    /// As `run_line`, with an app on the last node that answers every message
    /// at once with `proof` (`None`: it cannot open it and says nothing).
    fn run_with_app(nodes: &mut [R], start: Vec<(usize, Buf)>, now: &mut u64, proof: Option<Proof>) -> Vec<(usize, Action)> {
        let last = nodes.len() - 1;
        let mut queue: std::collections::VecDeque<(usize, Buf)> = start.into();
        let mut events = Vec::new();
        for _ in 0..20_000 {
            let actions: Vec<(usize, Action)> = if let Some((from, pkt)) = queue.pop_front() {
                let mut got = Vec::new();
                for j in [from.wrapping_sub(1), from + 1] {
                    if j < nodes.len() {
                        got.extend(nodes[j].on_receive(&pkt, 5, *now).into_iter().map(|a| (j, a)));
                    }
                }
                got
            } else {
                let Some(next) = nodes.iter().filter_map(|n| n.next_deadline()).min() else { break };
                if next > *now + 300_000 {
                    break;
                }
                *now = next.max(*now);
                let mut got = Vec::new();
                for (i, n) in nodes.iter_mut().enumerate() {
                    got.extend(n.tick(*now).into_iter().map(|a| (i, a)));
                }
                got
            };
            for (j, a) in actions {
                match a {
                    Action::Transmit { packet, .. } => queue.push_back((j, packet)),
                    other => {
                        if j == last
                            && let Action::Deliver { ptype: PacketType::Data, payload, .. } = &other
                            && let Some(p) = proof
                        {
                            let id = packet_id(PacketType::Data, &nodes[last].addr, payload);
                            let out = nodes[last].acknowledge(id, Some(p), *now);
                            queue.extend(transmits(&out).into_iter().map(|p| (last, p)));
                        }
                        events.push((j, other));
                    }
                }
            }
        }
        events
    }

    fn proving_line(len: u8, anchors: &[u8]) -> Vec<R> {
        let mut nodes = line(len, anchors);
        nodes.last_mut().unwrap().app_acks = true;
        nodes
    }

    /// Sends one message over the line, with the sender expecting `PROOF`.
    fn send_expecting(nodes: &mut [R], text: &[u8], token: u8, now: &mut u64, app: Option<Proof>) -> (u64, Vec<(usize, Action)>) {
        let dest = nodes.last().unwrap().addr;
        let (msg, out) = nodes[0].send_direct(dest, text, [token; 16], *now).unwrap();
        nodes[0].expect_proof(msg, PROOF);
        let ev = run_with_app(nodes, transmits(&out).into_iter().map(|p| (0, p)).collect(), now, app);
        (msg, ev)
    }

    #[test]
    fn a_message_with_proof_is_acknowledged_on_the_first_and_later_sends() {
        let mut nodes = proving_line(3, &[]);
        let mut now = 0;
        // The first message travels in the path request; the proof comes back in the reply.
        let (first, ev) = send_expecting(&mut nodes, b"one", 1, &mut now, Some(PROOF));
        assert!(acked(&ev, first), "{ev:?}");
        assert_eq!(nodes[0].stats.originated, 1, "one message, not sent again");
        // The next goes along the route; the proof comes back in the ack.
        let (second, ev) = send_expecting(&mut nodes, b"two", 2, &mut now, Some(PROOF));
        assert!(acked(&ev, second), "{ev:?}");
    }

    #[test]
    fn nothing_but_the_proof_counts_as_delivered() {
        let mut nodes = proving_line(2, &[]);
        let mut now = 0;
        send_expecting(&mut nodes, b"hello", 1, &mut now, Some(PROOF));
        let dest = nodes[2].addr;
        let (msg, _) = nodes[0].send_direct(dest, b"secret", [2; 16], now).unwrap();
        nodes[0].expect_proof(msg, PROOF);
        for forged in [&[][..], &[0; PROOF_LEN], &PROOF[..4]] {
            let header = Header::new(PacketType::Ack, ack_token(msg)).with_path(Path::new());
            let packet = encode(&header, forged).unwrap();
            assert!(!nodes[0].on_receive(&packet, 5, now + 1).contains(&Action::Acked { msg_id: msg }), "{forged:?}");
        }
        let header = Header::new(PacketType::Ack, ack_token(msg)).with_path(Path::new());
        let packet = encode(&header, &PROOF).unwrap();
        assert!(nodes[0].on_receive(&packet, 5, now + 2).contains(&Action::Acked { msg_id: msg }));
    }

    #[test]
    fn a_message_the_app_cannot_open_is_never_acknowledged() {
        let mut nodes = proving_line(2, &[]);
        let mut now = 0;
        let (msg, ev) = send_expecting(&mut nodes, b"unreadable", 1, &mut now, None);
        assert!(!acked(&ev, msg), "path reply: {ev:?}");
        // Also for a sender from before proofs, once the route is there.
        let dest = nodes[2].addr;
        let (old, out) = nodes[0].send_direct(dest, b"unreadable too", [2; 16], now).unwrap();
        let ev = run_with_app(&mut nodes, transmits(&out).into_iter().map(|p| (0, p)).collect(), &mut now, None);
        assert!(!acked(&ev, old), "{ev:?}");
    }

    #[test]
    fn an_old_sender_is_acknowledged_by_a_recipient_with_app_acks() {
        let mut nodes = proving_line(2, &[]);
        let mut now = 0;
        send_expecting(&mut nodes, b"route", 1, &mut now, Some(PROOF));
        let dest = nodes[2].addr;
        let (msg, out) = nodes[0].send_direct(dest, b"no proof expected", [2; 16], now).unwrap();
        let ev = run_with_app(&mut nodes, transmits(&out).into_iter().map(|p| (0, p)).collect(), &mut now, Some(PROOF));
        assert!(acked(&ev, msg), "{ev:?}");
    }

    #[test]
    fn the_proof_comes_through_anchors() {
        let mut nodes = proving_line(23, &[8, 16]);
        let mut now = 0;
        let (first, ev) = send_expecting(&mut nodes, b"far away", 7, &mut now, Some(PROOF));
        assert!(acked(&ev, first), "reply through anchors");
        let (second, ev) = send_expecting(&mut nodes, b"again", 8, &mut now, Some(PROOF));
        assert!(acked(&ev, second), "ack through anchors");
    }

    #[test]
    fn a_late_copy_gets_the_same_answer_after_the_duplicate_filter_forgot_it() {
        let mut dest = node(5, Role::Client);
        dest.app_acks = true;
        let header = Header::new(PacketType::Data, dest.addr).with_path(Path::new());
        let packet = encode(&header, b"sealed").unwrap();
        let id = packet_id(PacketType::Data, &dest.addr, b"sealed");
        dest.on_receive(&packet, 5, 100);
        dest.acknowledge(id, Some(PROOF), 100);
        dest.dedup = DedupCache::new();
        let again = transmits(&dest.on_receive(&packet, 5, 200_000));
        assert_eq!(again.len(), 1, "the answer is repeated");
        assert_eq!(Packet::decode(&again[0]).unwrap().payload, &PROOF);
    }

    #[test]
    fn a_forged_delivered_notice_does_not_stop_the_sender() {
        let mut a = node(1, Role::Relay);
        let mut dest = node(5, Role::Client);
        a.carry_copies = 4;
        dest.app_acks = true;
        let msg_id = packet_id(PacketType::Data, &dest.addr, b"sealed");
        a.expect_proof(msg_id, PROOF);
        a.start_carry(dest.addr, b"sealed", 0);
        let forged = encode(&Header::new(PacketType::Carry, ack_token(msg_id)), &[]).unwrap();
        assert!(!a.on_receive(&forged, 5, 100).contains(&Action::CarryAcked { msg_id }));
        assert_eq!(a.carried_count(), 1, "still carried");
        // The destination gets it and answers only when its app has opened it.
        a.neighbors.heard(dest.id, None, 5, 0, 10_000);
        let handoff = transmits(&a.tick(10_000))[0].clone();
        let got = dest.on_receive(&handoff, 5, 10_100);
        assert!(transmits(&got).is_empty(), "no notice before the app has opened it");
        let notices = transmits(&dest.acknowledge(msg_id, Some(PROOF), 10_200));
        // With the proof, and an empty one for carriers from before proofs.
        assert_eq!(notices.iter().map(|n| Packet::decode(n).unwrap().payload.len()).collect::<Vec<_>>(), [PROOF_LEN, 0]);
        let notice = notices[0].clone();
        assert!(a.on_receive(&notice, 5, 10_300).contains(&Action::CarryAcked { msg_id }));
        assert_eq!(a.carried_count(), 0);
    }

    #[test]
    fn rejects_oversized_direct_message() {
        let mut a = node(1, Role::Client);
        let big = [0u8; MAX_DIRECT_PAYLOAD + 1];
        assert_eq!(
            a.send_direct(Address([5; 16]), &big, [0; 16], 0).err(),
            Some(SendError::TooLarge)
        );
    }
}
