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
pub const MOBILE_ROUTE_MS: u64 = 30_000;
/// Wait before an unanswered path request is repeated (ms).
///
/// The reply can take a long time: over many hops, where every hop can
/// be retransmitted several times with `DEFAULT_HOP_TIMEOUT_MS` in between. If the
/// request is repeated too early, the net is flooded again while the reply is still on
/// its way, and the new flood drowns the reply. In the simulator this was the main cause
/// of lost acknowledgements in large, sparse nets (15 s gave 50 % acknowledged, 45 s
/// gave over 80 %).
pub const PATH_RETRY_MS: u64 = 45_000;
/// Number of repetitions of a path request.
pub const PATH_RETRIES: u8 = 2;
const MAX_FORWARDS: usize = 32;
const MAX_FLOOD_PENDING: usize = 16;
const MAX_QUEUE: usize = 8;
const MAX_AWAIT: usize = 32;
const MAX_CHANNELS: usize = 8;
const MAX_NEIGHBORS: usize = 32;
/// How long role, forwarding and coverage observations stay valid for the
/// edge bridge rule (ms).
pub const EDGE_WINDOW_MS: u64 = 300_000;
/// Messages a node carries at most (see "Carry").
pub const CARRY_MAX: usize = 8;
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
const MAX_TRACES: usize = 4;
/// How many legs an anchor remembers at a time (requests and messages).
const MAX_ANCHORED: usize = 16;
/// How long a leg is remembered by the anchor (ms): long enough for reply and
/// acknowledgement, even over many hops.
const ANCHOR_TTL_MS: u64 = 120_000;
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
    /// When the request is repeated if there is no reply.
    retry_at_ms: u64,
    tries: u8,
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
    reply_at_ms: u64,
}

/// A leg an anchor remembers: replies to `key` (reply token or
/// ack token) are sent back along `back`. `target` is the target of a
/// path request (otherwise unused).
struct Anchored {
    key: Address,
    target: Address,
    back: Path,
    until_ms: u64,
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
    carried: heapless::Vec<Carried, CARRY_MAX>,
    next_carry_ms: u64,
    /// Neighbours seen at the previous carry step: handoffs only go to new
    /// encounters, not to the same neighbours again and again.
    carry_seen: heapless::Vec<PathId, 32>,
    /// We heard a repeater at the previous carry step.
    carry_near_repeater: bool,
    /// Carried messages known to be delivered.
    carry_done: heapless::Vec<CarryDone, CARRY_DONE_MAX>,
    /// Largest number of hops a relay forwards (default 4, at most 7).
    pub relay_hops: u8,
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
            channels: heapless::Vec::new(),
            forwards: heapless::Vec::new(),
            replies: heapless::Vec::new(),
            flood_pending: heapless::Vec::new(),
            slot_ms: DEFAULT_SLOT_MS,
            hop_timeout_ms: DEFAULT_HOP_TIMEOUT_MS,
            suppress_after: 1,
            extra_flood_delay_ms: 0,
            carry_copies: 0,
            carried: heapless::Vec::new(),
            next_carry_ms: 0,
            carry_seen: heapless::Vec::new(),
            carry_near_repeater: false,
            carry_done: heapless::Vec::new(),
            relay_hops: DEFAULT_RELAY_HOPS,
            ack_timeout_ms: DEFAULT_ACK_TIMEOUT_MS,
            mobile: false,
            weak_snr_db: weak_snr_for_sf(9),
            regions: crate::region::Regions::new(),
            strict_region: false,
            stats: RouterStats::default(),
            neighbors: crate::diag::NeighborTable::new(),
            dead_hops: heapless::Vec::new(),
            local_repair: true,
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
        bucket as u32 * self.slot_ms + self.energy.penalty_ms() + relay + self.extra_flood_delay_ms
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
            Role::Relay => self.energy != Energy::Critical && hops < self.relay_hops.min(7),
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
        if self.role != Role::Relay || !self.hears_repeater(now_ms) {
            return false;
        }
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
        fwd.chain(req).chain(ack).chain(rep).chain(carry).min()
    }

    /// Called when `next_deadline` is reached: retransmits hops that have not
    /// been forwarded, and gives up on them after `HOP_RETRIES` attempts.
    pub fn tick(&mut self, now_ms: u64) -> Actions {
        let mut out = Actions::new();
        if (!self.carried.is_empty() || !self.carry_done.is_empty()) && now_ms >= self.next_carry_ms {
            self.next_carry_ms = now_ms + CARRY_EVERY_MS;
            self.carry_step(now_ms, &mut out);
        }
        // Answer path requests along the best trail we have heard.
        while let Some(i) = self.replies.iter().position(|r| r.reply_at_ms <= now_ms) {
            let r = self.replies.swap_remove(i);
            let mut payload = Buf::new();
            let _ = payload.push(r.best.len() as u8);
            for id in &r.best {
                let _ = payload.extend_from_slice(&id.to_be_bytes());
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
            self.emit_request(p.target, token, old, now_ms, p.tries + 1, &mut out);
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
        if payload.len() > MAX_DIRECT_PAYLOAD {
            return Err(SendError::TooLarge);
        }
        let msg_id = packet_id(ptype, &dest, payload);
        let mut out = Actions::new();
        if self.mobile
            && self
                .routes
                .get(&dest)
                .is_some_and(|r| r.learned_ms + MOBILE_ROUTE_MS < now_ms)
        {
            // A mobile node has moved: find a fresh route.
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
        self.add_carried(dest, payload, self.carry_copies, CARRY_TTL_MS, None, now_ms);
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
        if self.carried.is_full()
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
            let due: heapless::Vec<(Address, Buf, u64), CARRY_MAX> = self
                .carried
                .iter()
                .filter(|c| c.own && (regained || c.routed_ms == 0 || now_ms.saturating_sub(c.routed_ms) >= CARRY_REROUTE_MS))
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
        let fresh: heapless::Vec<PathId, 32> = here
            .iter()
            .copied()
            .filter(|n| !self.carry_seen.contains(n))
            .filter(|n| !self.neighbors.get(*n).is_some_and(|x| matches!(x.role, Some(Role::Repeater | Role::Anchor))))
            .collect();
        self.carry_seen = here.clone();
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
                self.emit_carry_notice(id, now_ms, out);
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
        if h.dest == self.addr {
            if self.dedup.insert(id) {
                self.stats.carry_delivered += 1;
                let _ = out.push(Action::Deliver {
                    ptype: PacketType::Data,
                    dest: self.addr,
                    payload: Buf::from_slice(msg).unwrap_or_default(),
                    hops: 0,
                });
            }
            // Delivered (again): tell the carrier, so it stops.
            self.emit_carry_notice(id, now_ms, out);
            return;
        }
        if self.carry_copies == 0 {
            return;
        }
        if self.carry_done.iter().any(|d| d.id == id) {
            // Already delivered: answer with the notice instead of storing it.
            self.emit_carry_notice(id, now_ms, out);
            return;
        }
        if self.dedup.contains(id) {
            return; // already delivered through us
        }
        self.add_carried(h.dest, msg, tokens, ttl_s as u64 * 1000, Some(from), now_ms);
    }

    /// Sends the delivered notice for carried message `id` (see "Delivered
    /// notice"): a Carry packet without a path, addressed to `ack_token(id)`.
    fn emit_carry_notice(&mut self, id: u64, now_ms: u64, out: &mut Actions) {
        if let Some(d) = self.carry_done.iter_mut().find(|d| d.id == id) {
            d.sent_ms = now_ms;
        }
        let header = Header::new(PacketType::Carry, ack_token(id));
        let Ok(packet) = encode(&header, &[]) else { return };
        self.stats.carry_notices += 1;
        // Not through `emit`: the notice is repeated on purpose, so it is
        // kept out of the duplicate filter.
        let tx_id = packet_id(PacketType::Carry, &header.dest, &[]) ^ now_ms;
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
        if u64::from_le_bytes(hi) != !id || !pkt.payload.is_empty() {
            return; // not a notice
        }
        self.carry_delivered_notice(id, true, now_ms, out);
    }

    /// Carried message `id` is known to be delivered: drop our copy, tell
    /// the app if it is ours, and pass the notice on once. `relay` is false
    /// when the sender learned it from an ordinary acknowledgement (the app
    /// already got `Acked`).
    fn carry_delivered_notice(&mut self, id: u64, relay: bool, now_ms: u64, out: &mut Actions) {
        if self.carry_done.iter().any(|d| d.id == id) {
            return;
        }
        let Some(i) = self.carried.iter().position(|c| c.id == id) else {
            return; // we never carried it: nothing to stop
        };
        let c = self.carried.swap_remove(i);
        if c.own && relay {
            let _ = out.push(Action::CarryAcked { msg_id: id });
        }
        if self.carry_done.is_full() {
            self.carry_done.remove(0);
        }
        let _ = self.carry_done.push(CarryDone { id, until_ms: c.until_ms, holders: c.given.clone(), sent_ms: 0 });
        // Pass it on only if someone else may hold a copy.
        if !c.given.is_empty() {
            self.emit_carry_notice(id, now_ms, out);
        }
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
        let _ = self.pending.push(PendingRequest {
            token,
            old_tokens,
            target: dest,
            carried,
            retry_at_ms: now_ms + PATH_RETRY_MS,
            tries,
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
                    self.on_flood(&pkt, snr_db, &mut out);
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
        if score(snr_db, path.len()) < score(r.best_snr, r.best.len()) {
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
            if !data.is_empty()
                && self
                    .dedup
                    .insert(packet_id(PacketType::Data, &self.addr, data))
            {
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
                reply_at_ms: now_ms + (RESPONSE_WAIT_SLOTS * self.slot_ms) as u64,
            });
            return;
        }
        if !self.may_forward(h.hops) || h.hops >= MAX_HOPS {
            return;
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
        let mut header = Header::new(PacketType::PathRequest, h.dest).with_path(record);
        header.hops = h.hops + 1;
        let delay = self.flood_delay_ms(snr_db);
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
                if let Some(msg_id) = req.carried {
                    self.awaiting.retain(|a| a.msg_id != msg_id);
                    // The message arrived with the request: remove it from the queue.
                    let target = req.target;
                    self.queue
                        .retain(|q| packet_id(q.ptype, &target, &q.payload) != msg_id);
                    let _ = out.push(Action::Acked { msg_id });
                    self.carry_delivered_notice(msg_id, false, now_ms, out);
                }
                self.learn_route(req.target, forward, now_ms, out);
            }
            PacketType::Ack => {
                let Some(i) = self.awaiting.iter().position(|a| a.token == h.dest) else {
                    self.anchor_relay_back(pkt, id, now_ms, out);
                    return;
                };
                if !self.dedup.insert(id) {
                    return;
                }
                let msg_id = self.awaiting.remove(i).msg_id;
                let _ = out.push(Action::Acked { msg_id });
                self.carry_delivered_notice(msg_id, false, now_ms, out);
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
            _ if h.dest == self.addr => {
                // A repeated message is not delivered again, but acknowledged again,
                // because the first acknowledgement may have been lost.
                if self.dedup.insert(id) {
                    deliver(pkt, out);
                }
                if matches!(
                    h.ptype,
                    PacketType::Data | PacketType::Fragment | PacketType::Deposit
                ) {
                    let header =
                        Header::new(PacketType::Ack, ack_token(id)).with_path(reversed(path));
                    self.emit_routed(&header, &[], SOURCE_ROUTE_DELAY_MS, now_ms, out);
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
        self.anchored.retain(|a| a.until_ms > now_ms && a.key != key);
        if self.anchored.is_full() {
            self.anchored.remove(0);
        }
        let _ = self.anchored.push(Anchored { key, target, back, until_ms: now_ms + ANCHOR_TTL_MS });
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
        });
        let first = reversed(&a.back);
        let mut payload = Buf::new();
        let _ = payload.push(first.len() as u8);
        for pid in &first {
            let _ = payload.extend_from_slice(&pid.to_be_bytes());
        }
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
        self.remember_anchor(ack_token(id), h.dest, reversed(path), now_ms);
        let header = Header::new(h.ptype, h.dest).with_path(route);
        self.stats.forwarded += 1;
        self.emit_routed(&header, pkt.payload, SOURCE_ROUTE_DELAY_MS, now_ms, out);
    }

    fn learn_route(&mut self, target: Address, path: Path, now_ms: u64, out: &mut Actions) {
        let hops = path.len() as u8;
        self.routes.upsert(Route {
            dest: target,
            cost: hops as u16,
            path: path.clone(),
            last_seen_ms: now_ms,
            learned_ms: now_ms,
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

    fn on_flood(&mut self, pkt: &Packet, snr_db: i8, out: &mut Actions) {
        let h = &pkt.header;
        // Announces always go to the app, so it can verify and remember the key.
        if h.dest == self.addr || self.channels.contains(&h.dest) || h.ptype == PacketType::Announce
        {
            deliver(pkt, out);
        }
        if !self.may_forward(h.hops) || h.dest == self.addr {
            return;
        }
        let local = matches!(
            h.ptype,
            PacketType::Channel | PacketType::Announce | PacketType::Probe
        );
        // On low battery only the most important things are forwarded.
        let allowed = match self.energy {
            Energy::Full | Energy::Ok => true,
            Energy::Low => !local,
            Energy::Critical => false,
        };
        let scope = if local { LOCAL_SCOPE } else { MAX_HOPS };
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
        R::new(Address(a), role)
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
    /// `--nocapture` to see the number.
    #[test]
    fn router_size_in_firmware() {
        let size = core::mem::size_of::<Router<256, 1024>>();
        std::println!("size_of::<Router<256, 1024>>() = {size} bytes");
        assert!(size < 96 * 1024);
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
