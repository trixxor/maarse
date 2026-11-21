//! DHT for backbone hosts (only over TCP, never over LoRa).
//!
//! Kademlia-like: every host has an ID = its Må:rse address, distance = XOR.
//! The DHT stores *where* an address can be reached:
//!
//! ```text
//! key:    Må:rse address (16 bytes)
//! value:  the gateway host's ID + TCP endpoint ("host:port") + hops from
//!         the gateway + the signed announce from the address
//! ```
//!
//! The announce is proven by the address's own signature, so nobody can invent
//! addresses. Who *forwards* to the address is not signed by it;
//! a lying gateway can at most drop messages (they are encrypted end-to-end).
//!
//! When a host needs to reach an unknown address, it looks it up in the DHT, connects
//! directly to the gateway as an ordinary TCP peer, and routing does the rest.
//!
//! ## Full replication
//! In a small backbone (at most [`FULL_REPLICA_MAX`] known hosts), records are
//! stored on every known host, not only the K closest. When a backbone link
//! comes up (e.g. after a split heals), the host republishes all its records,
//! so both sides learn where the other side's addresses are again. Records
//! carry the address's own signed announce and expire after [`RECORD_TTL`].
//!
//! Protocol: the client opens a TCP connection to the host's backbone port and
//! sends `D7 D7 | length (u16 LE) | request`; the host answers with
//! `length | response` and closes. Ordinary packet connections never start with
//! D7 D7 (packets are at most 255 bytes).

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};
use maarse_core::Address;
use maarse_crypto::{Identity, PublicIdentity, verify_announce};

/// Contacts per bucket and the number of nodes a record is stored on.
pub const K: usize = 8;
/// Requests per round during a lookup.
pub const ALPHA: usize = 3;
/// A record expires after this time if it is not renewed.
pub const RECORD_TTL: Duration = Duration::from_secs(2 * 3600);
/// How often a gateway republishes its records.
pub const REPUBLISH: Duration = Duration::from_secs(30 * 60);
/// First two bytes on a DHT connection.
pub const MAGIC: [u8; 2] = [0xD7, 0xD7];
pub const AUTH_MAGIC: [u8; 2] = [0xD7, 0xD8];
pub const MUTUAL_MAGIC: [u8; 2] = [0xD7, 0xD9];
const REPLY_PROOF_LEN: usize = 128;
/// Timeout for one request.
pub const RPC_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_EP: usize = 100;
/// Up to this many known DHT contacts, a gateway stores its records on *all*
/// of them instead of only the K closest. With the 20-50 hosts of a national
/// net, every host then holds the whole table, so a lookup is answered
/// locally and each side of a split backbone keeps working with its own
/// copy until the records expire (see "Full replication").
pub const FULL_REPLICA_MAX: usize = 64;
const MAX_MSG: usize = 4096;
const MAX_RECORDS: usize = 8192;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Contact {
    pub id: Address,
    pub ep: String,
}

/// What a host or server offers the backbone, announced over the DHT
/// (`Req::Caps`) and in the backbone link hello. Unknown = as before: post
/// office and relay assumed.
#[derive(Clone, Debug, PartialEq, Eq, Default, serde::Serialize)]
pub struct Caps {
    pub version: String,
    /// maarse-server (a dedicated host).
    pub server: bool,
    /// Has a LoRa modem or trunk up now.
    pub radio: bool,
    /// Regions it forwards channel messages for.
    pub regions: Vec<u16>,
    /// Keeps mail for others (replica post office).
    pub post_office: bool,
    /// Room left for mail, MB (u32::MAX = no limit set).
    pub mail_free_mb: u32,
    /// Relays for hosts behind NAT (publishes the addresses it reaches
    /// through them).
    pub relay: bool,
    /// Answers test calls (music, echo, speed; see node/calltest.rs).
    pub call_test: bool,
    /// Accepts encrypted packet observations on backbone TCP links.
    pub observe: bool,
}

impl Caps {
    /// `version | flags (bit 0 server, 1 radio, 2 post office, 3 relay, 4 call test, 5 observe) |
    /// mail free MB (u32 LE) | n | regions (u16 BE)`.
    pub fn encode(&self, out: &mut Vec<u8>) {
        put_str(out, &self.version);
        out.push(self.server as u8 | (self.radio as u8) << 1 | (self.post_office as u8) << 2 | (self.relay as u8) << 3 | (self.call_test as u8) << 4 | (self.observe as u8) << 5);
        out.extend_from_slice(&self.mail_free_mb.to_le_bytes());
        let n = self.regions.len().min(8);
        out.push(n as u8);
        for r in &self.regions[..n] {
            out.extend_from_slice(&r.to_be_bytes());
        }
    }

    pub fn decode(b: &[u8]) -> Option<Self> {
        let mut rd = Rd(b);
        Self::read(&mut rd)
    }

    fn read(rd: &mut Rd) -> Option<Self> {
        let version = rd.str()?;
        let f = rd.u8()?;
        let mail_free_mb = u32::from_le_bytes(rd.take(4)?.try_into().ok()?);
        let n = rd.u8()?.min(8) as usize;
        let mut regions = Vec::new();
        for _ in 0..n {
            let b = rd.take(2)?;
            regions.push(u16::from_be_bytes([b[0], b[1]]));
        }
        Some(Caps { version, server: f & 1 != 0, radio: f & 2 != 0, post_office: f & 4 != 0, relay: f & 8 != 0, call_test: f & 16 != 0, observe: f & 32 != 0, mail_free_mb, regions })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub dest: Address,
    pub gw: Address,
    pub ep: String,
    /// Hops from the gateway to the address when the announce was heard.
    pub hops: u8,
    /// Age in seconds when the record was sent/received.
    pub age_s: u32,
    pub announce: Vec<u8>,
    proof: Option<RouteProof>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RouteProof {
    identity: PublicIdentity,
    issued: u64,
    signature: [u8; 64],
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

impl Record {
    pub fn signed(gateway: &Identity, dest: Address, ep: String, hops: u8, announce: Vec<u8>) -> Self {
        let issued = unix_now();
        let mut record = Self { dest, gw: gateway.address(), ep, hops, age_s: 0, announce, proof: None };
        record.proof = Some(RouteProof {
            identity: gateway.public(),
            issued,
            signature: gateway.sign_dht_route(&record.route_claim(issued)),
        });
        record
    }

    fn route_claim(&self, issued: u64) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&self.dest.0);
        bytes.extend_from_slice(&self.gw.0);
        bytes.extend_from_slice(&issued.to_le_bytes());
        bytes.push(self.hops);
        bytes.extend_from_slice(&(self.ep.len() as u32).to_le_bytes());
        bytes.extend_from_slice(self.ep.as_bytes());
        bytes.extend_from_slice(&self.announce);
        bytes
    }

    fn proof_is_fresh(proof: &RouteProof) -> bool {
        let now = unix_now();
        proof.issued <= now.saturating_add(120)
            && now.saturating_sub(proof.issued) < RECORD_TTL.as_secs()
    }

    pub fn verified_gateway(&self) -> bool {
        let Some(proof) = &self.proof else { return false };
        Self::proof_is_fresh(proof) && proof.identity.address() == self.gw
            && proof.identity.verify_dht_route(&self.route_claim(proof.issued), &proof.signature)
    }

    /// Is the announce genuine and the record sensible?
    pub fn valid(&self) -> bool {
        !self.ep.is_empty()
            && self.ep.len() <= MAX_EP
            && self.announce.len() <= maarse_core::MAX_PACKET
            && (self.age_s as u64) < RECORD_TTL.as_secs()
            && verify_announce(&self.dest, &self.announce).is_ok()
            && (self.proof.is_none() || self.verified_gateway())
    }
}

pub fn distance(a: &Address, b: &Address) -> [u8; 16] {
    let mut d = [0u8; 16];
    for (o, (x, y)) in d.iter_mut().zip(a.0.iter().zip(b.0.iter())) {
        *o = x ^ y;
    }
    d
}

fn bucket_index(me: &Address, other: &Address) -> Option<usize> {
    let d = distance(me, other);
    let lz = u128::from_be_bytes(d).leading_zeros() as usize;
    (lz < 128).then_some(lz)
}

/// At most this many DHT contacts from one network (see `Table::insert`).
pub const MAX_PER_NET: usize = 4;

/// The network part of an "ip:port" endpoint: IPv4 /24 or IPv6 /48.
fn net_of(ep: &str) -> Option<[u8; 7]> {
    let a: std::net::SocketAddr = ep.parse().ok()?;
    match a.ip() {
        ip if ip.is_loopback() => None,
        std::net::IpAddr::V4(v) => {
            let o = v.octets();
            Some([4, o[0], o[1], o[2], 0, 0, 0])
        }
        std::net::IpAddr::V6(v) => {
            let o = v.octets();
            Some([6, o[0], o[1], o[2], o[3], o[4], o[5]])
        }
    }
}

/// Saves contacts (one "address-hex endpoint" per line).
pub fn save_contacts(path: &std::path::Path, contacts: &[Contact]) {
    let text: String = contacts
        .iter()
        .map(|c| format!("{} {}\n", c.id.0.iter().map(|b| format!("{b:02x}")).collect::<String>(), c.ep))
        .collect();
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

/// Loads contacts saved by [`save_contacts`] (bad lines are skipped).
pub fn load_contacts(path: &std::path::Path) -> Vec<Contact> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    text.lines()
        .filter_map(|l| {
            let (id, ep) = l.trim().split_once(' ')?;
            let b: Vec<u8> = (0..id.len()).step_by(2).filter_map(|i| id.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok())).collect();
            let id = Address::from_slice(&b).filter(|_| id.len() == 32)?;
            let ep = ep.trim();
            (!ep.is_empty() && ep.len() <= MAX_EP).then(|| Contact { id, ep: ep.to_string() })
        })
        .take(512)
        .collect()
}

// ---------------------------------------------------------------------------
// Routing table

pub struct Table {
    me: Address,
    buckets: Vec<Vec<(Contact, Instant)>>,
}

impl Table {
    pub fn new(me: Address) -> Self {
        Self {
            me,
            buckets: vec![Vec::new(); 128],
        }
    }

    /// Adds or updates a contact (most recent last).
    pub fn insert(&mut self, c: Contact) {
        let Some(i) = bucket_index(&self.me, &c.id) else {
            return; // ourselves
        };
        if c.ep.is_empty() || c.ep.len() > MAX_EP {
            return;
        }
        let known = self.buckets[i].iter().find(|(x, _)| x.id == c.id);
        if known.is_some_and(|(existing, _)| existing.ep != c.ep) {
            return;
        }
        if known.is_none() && !self.room_for_net(&c.ep) {
            // Many identities from one network: a cheap way to take over
            // part of the table (Sybil), so at most MAX_PER_NET of them.
            return;
        }
        let b = &mut self.buckets[i];
        if let Some(pos) = b.iter().position(|(x, _)| x.id == c.id) {
            b.remove(pos);
        } else if b.len() >= K {
            // Full bucket: the oldest is evicted if it has not been heard for a long time.
            if b[0].1.elapsed() > Duration::from_secs(15 * 60) {
                b.remove(0);
            } else {
                return;
            }
        }
        b.push((c, Instant::now()));
    }

    /// Is there room for another contact from this endpoint's network (an
    /// IPv4 /24 or an IPv6 /48)? Loopback and names are not limited.
    fn room_for_net(&self, ep: &str) -> bool {
        let Some(net) = net_of(ep) else { return true };
        self.buckets.iter().flatten().filter(|(c, _)| net_of(&c.ep) == Some(net)).count() < MAX_PER_NET
    }

    /// One contact per non-empty bucket (the most recently heard), so a
    /// node keeps links spread over the ID space instead of to its
    /// nearest neighbours only.
    pub fn spread(&self) -> Vec<Contact> {
        self.buckets.iter().rev().filter_map(|b| b.last().map(|(c, _)| c.clone())).collect()
    }

    pub fn remove(&mut self, id: &Address) {
        if let Some(i) = bucket_index(&self.me, id) {
            self.buckets[i].retain(|(c, _)| c.id != *id);
        }
    }

    pub fn closest(&self, target: &Address, n: usize) -> Vec<Contact> {
        let mut all: Vec<Contact> = self
            .buckets
            .iter()
            .flat_map(|b| b.iter().map(|(c, _)| c.clone()))
            .collect();
        all.sort_by_key(|c| distance(&c.id, target));
        all.truncate(n);
        all
    }

    pub fn len(&self) -> usize {
        self.buckets.iter().map(Vec::len).sum()
    }

    /// All contacts.
    pub fn all(&self) -> Vec<Contact> {
        self.buckets.iter().flat_map(|b| b.iter().map(|(c, _)| c.clone())).collect()
    }
}

// ---------------------------------------------------------------------------
// Record store

pub struct Store {
    map: HashMap<Address, (Record, Instant)>,
}

impl Default for Store {
    fn default() -> Self {
        Self::new()
    }
}

impl Store {
    /// Every record kept (for the debug view), with its current age.
    pub fn all(&self) -> Vec<Record> {
        self.map
            .values()
            .map(|(r, t)| Record { age_s: r.age_s.saturating_add(t.elapsed().as_secs() as u32), ..r.clone() })
            .collect()
    }

    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    fn age(r: &Record, at: &Instant) -> u64 {
        r.age_s as u64 + at.elapsed().as_secs()
    }

    /// Stores the record if it is valid and at least as good as the old one
    /// (fewer hops, or the old one is about to go stale).
    pub fn put(&mut self, r: Record) -> bool {
        if !r.valid() {
            return false;
        }
        if let Some((old, at)) = self.map.get(&r.dest) {
            if old.proof.is_some() && r.proof.is_none() && old.proof.as_ref().is_some_and(Record::proof_is_fresh) {
                return false;
            }
            if r.proof.is_some() && old.proof.is_none() {
                self.map.insert(r.dest, (r, Instant::now()));
                return true;
            }
            if let (Some(previous), Some(new)) = (&old.proof, &r.proof)
                && old.gw == r.gw && new.issued < previous.issued
            {
                return false;
            }
            let timestamp = maarse_crypto::announce_timestamp(&r.announce);
            let old_timestamp = maarse_crypto::announce_timestamp(&old.announce);
            if timestamp < old_timestamp {
                return false;
            }
            let old_age = Self::age(old, at);
            let better = r.hops < old.hops
                || (r.hops == old.hops && r.age_s as u64 <= old_age)
                || old_age > 2 * REPUBLISH.as_secs();
            if !better {
                return false;
            }
        }
        if !self.map.contains_key(&r.dest) && self.map.len() >= MAX_RECORDS {
            self.expire();
            if self.map.len() >= MAX_RECORDS {
                return false;
            }
        }
        self.map.insert(r.dest, (r, Instant::now()));
        true
    }

    pub fn get(&self, dest: &Address) -> Option<Record> {
        let (r, at) = self.map.get(dest)?;
        let age = Self::age(r, at);
        (age < RECORD_TTL.as_secs() && (r.proof.as_ref().is_none_or(Record::proof_is_fresh))).then(|| Record {
            age_s: age as u32,
            ..r.clone()
        })
    }

    pub fn expire(&mut self) {
        self.map
            .retain(|_, (r, at)| Self::age(r, at) < RECORD_TTL.as_secs() && r.proof.as_ref().is_none_or(Record::proof_is_fresh));
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }
}

// ---------------------------------------------------------------------------
// Messages

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Req {
    Ping,
    FindNode(Address),
    FindValue(Address),
    Store(Record),
    /// Post offices (see `maarse_core::mail`): keep this copy of a sealed
    /// message for `recipient`.
    MailStore { recipient: Address, sealed: Vec<u8> },
    /// Send me `recipient`'s mail, except these IDs.
    MailFetch { recipient: Address, have: Vec<u32> },
    /// These messages for `recipient` have been delivered: delete them.
    MailDelivered { recipient: Address, ids: Vec<u32> },
    /// What do you offer (post office, relay, radio)?
    Caps,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resp {
    Pong,
    Nodes(Vec<Contact>),
    Value(Record),
    Stored(bool),
    /// Sealed messages (answer to `MailFetch`).
    Mail(Vec<Vec<u8>>),
    /// Answer to `Caps`.
    Caps(Caps),
}

fn put_ids(out: &mut Vec<u8>, ids: &[u32]) {
    let n = ids.len().min(64);
    out.push(n as u8);
    for id in &ids[..n] {
        out.extend_from_slice(&id.to_be_bytes());
    }
}

fn get_ids(rd: &mut Rd) -> Option<Vec<u32>> {
    let n = rd.u8()? as usize;
    (0..n).map(|_| rd.take(4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))).collect()
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    let b = &s.as_bytes()[..s.len().min(255)];
    out.push(b.len() as u8);
    out.extend_from_slice(b);
}

struct Rd<'a>(&'a [u8]);

impl<'a> Rd<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.0.len() < n {
            return None;
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Some(a)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn addr(&mut self) -> Option<Address> {
        Address::from_slice(self.take(16)?)
    }
    fn str(&mut self) -> Option<String> {
        let n = self.u8()? as usize;
        String::from_utf8(self.take(n)?.to_vec()).ok()
    }
}

fn put_record(out: &mut Vec<u8>, r: &Record) {
    out.extend_from_slice(&r.dest.0);
    out.extend_from_slice(&r.gw.0);
    put_str(out, &r.ep);
    out.push(r.hops);
    out.extend_from_slice(&r.age_s.to_le_bytes());
    out.extend_from_slice(&(r.announce.len() as u16).to_le_bytes());
    out.extend_from_slice(&r.announce);
    if let Some(proof) = &r.proof {
        out.extend_from_slice(&proof.issued.to_le_bytes());
        out.extend_from_slice(&proof.identity.to_bytes());
        out.extend_from_slice(&proof.signature);
    }
}

fn get_record(rd: &mut Rd) -> Option<Record> {
    let dest = rd.addr()?;
    let gw = rd.addr()?;
    let ep = rd.str()?;
    let hops = rd.u8()?;
    let age_s = u32::from_le_bytes(rd.take(4)?.try_into().ok()?);
    let n = u16::from_le_bytes(rd.take(2)?.try_into().ok()?) as usize;
    let announce = rd.take(n)?.to_vec();
    let proof = if rd.0.is_empty() {
        None
    } else {
        let bytes = rd.take(136)?;
        if !rd.0.is_empty() { return None }
        Some(RouteProof {
            issued: u64::from_le_bytes(bytes[..8].try_into().ok()?),
            identity: PublicIdentity::from_bytes(&bytes[8..72])?,
            signature: bytes[72..].try_into().ok()?,
        })
    };
    Some(Record {
        dest,
        gw,
        ep,
        hops,
        age_s,
        announce,
        proof,
    })
}

/// Request: `type | sender ID | sender's endpoint ("" = none) | body`.
pub fn encode_req(from: &Address, from_ep: Option<&str>, req: &Req) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(match req {
        Req::Ping => 1,
        Req::FindNode(_) => 2,
        Req::FindValue(_) => 3,
        Req::Store(_) => 4,
        Req::MailStore { .. } => 5,
        Req::MailFetch { .. } => 6,
        Req::MailDelivered { .. } => 7,
        Req::Caps => 8,
    });
    out.extend_from_slice(&from.0);
    put_str(&mut out, from_ep.unwrap_or(""));
    match req {
        Req::Ping | Req::Caps => {}
        Req::FindNode(a) | Req::FindValue(a) => out.extend_from_slice(&a.0),
        Req::Store(r) => put_record(&mut out, r),
        Req::MailStore { recipient, sealed } => {
            out.extend_from_slice(&recipient.0);
            out.extend_from_slice(&(sealed.len().min(1024) as u16).to_le_bytes());
            out.extend_from_slice(&sealed[..sealed.len().min(1024)]);
        }
        Req::MailFetch { recipient, have } => {
            out.extend_from_slice(&recipient.0);
            put_ids(&mut out, have);
        }
        Req::MailDelivered { recipient, ids } => {
            out.extend_from_slice(&recipient.0);
            put_ids(&mut out, ids);
        }
    }
    out
}

pub fn decode_req(b: &[u8]) -> Option<(Address, Option<String>, Req)> {
    let mut rd = Rd(b);
    let kind = rd.u8()?;
    let from = rd.addr()?;
    let ep = rd.str()?;
    let req = match kind {
        1 => Req::Ping,
        2 => Req::FindNode(rd.addr()?),
        3 => Req::FindValue(rd.addr()?),
        4 => Req::Store(get_record(&mut rd)?),
        5 => {
            let recipient = rd.addr()?;
            let n = u16::from_le_bytes(rd.take(2)?.try_into().ok()?) as usize;
            Req::MailStore { recipient, sealed: rd.take(n)?.to_vec() }
        }
        6 => Req::MailFetch { recipient: rd.addr()?, have: get_ids(&mut rd)? },
        7 => Req::MailDelivered { recipient: rd.addr()?, ids: get_ids(&mut rd)? },
        8 => Req::Caps,
        _ => return None,
    };
    Some((from, (!ep.is_empty()).then_some(ep), req))
}

/// Response: `type | responder's ID | body`.
pub fn encode_resp(me: &Address, resp: &Resp) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(match resp {
        Resp::Pong => 0x81,
        Resp::Nodes(_) => 0x82,
        Resp::Value(_) => 0x83,
        Resp::Stored(_) => 0x84,
        Resp::Mail(_) => 0x85,
        Resp::Caps(_) => 0x86,
    });
    out.extend_from_slice(&me.0);
    match resp {
        Resp::Pong => {}
        Resp::Nodes(v) => {
            out.push(v.len().min(K * 2) as u8);
            for c in v.iter().take(K * 2) {
                out.extend_from_slice(&c.id.0);
                put_str(&mut out, &c.ep);
            }
        }
        Resp::Value(r) => put_record(&mut out, r),
        Resp::Stored(ok) => out.push(*ok as u8),
        Resp::Caps(c) => c.encode(&mut out),
        Resp::Mail(items) => {
            // As many as fit in one response.
            let mut v: Vec<&Vec<u8>> = Vec::new();
            let mut size = 0;
            for m in items {
                if size + 2 + m.len() > MAX_MSG - 64 || v.len() == 255 {
                    break;
                }
                size += 2 + m.len();
                v.push(m);
            }
            out.push(v.len() as u8);
            for m in v {
                out.extend_from_slice(&(m.len() as u16).to_le_bytes());
                out.extend_from_slice(m);
            }
        }
    }
    out
}

pub fn decode_resp(b: &[u8]) -> Option<(Address, Resp)> {
    let mut rd = Rd(b);
    let kind = rd.u8()?;
    let from = rd.addr()?;
    let resp = match kind {
        0x81 => Resp::Pong,
        0x82 => {
            let n = rd.u8()? as usize;
            let mut v = Vec::with_capacity(n);
            for _ in 0..n {
                let id = rd.addr()?;
                let ep = rd.str()?;
                v.push(Contact { id, ep });
            }
            Resp::Nodes(v)
        }
        0x83 => Resp::Value(get_record(&mut rd)?),
        0x84 => Resp::Stored(rd.u8()? != 0),
        0x85 => {
            let n = rd.u8()? as usize;
            let mut items = Vec::with_capacity(n);
            for _ in 0..n {
                let len = u16::from_le_bytes(rd.take(2)?.try_into().ok()?) as usize;
                items.push(rd.take(len)?.to_vec());
            }
            Resp::Mail(items)
        }
        0x86 => Resp::Caps(Caps::read(&mut rd)?),
        _ => return None,
    };
    Some((from, resp))
}

// ---------------------------------------------------------------------------
// Network (blocking; run in separate threads)

/// Resolves an endpoint once and applies policy to the address we will dial.
pub fn resolve_peer(ep: &str, configured: &[String]) -> Option<SocketAddr> {
    let explicit = configured.iter().any(|allowed| allowed == ep);
    ep.to_socket_addrs().ok()?.find(|addr| addr.port() != 0 && (explicit || public_ip(addr.ip())))
}

fn public_ip(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !ip.is_private()
                && !ip.is_loopback()
                && !ip.is_link_local()
                && !ip.is_documentation()
                && a != 0
                && a < 224
                && !(a == 100 && (64..=127).contains(&b))
                && !(a == 192 && b == 0 && (c == 0 || c == 2))
                && !(a == 198 && (b == 18 || b == 19))
        }
        IpAddr::V6(ip) => {
            let [a, b, ..] = ip.segments();
            // IPv6 global unicast, excluding IETF special use, 6to4 and documentation.
            (0x2000..=0x3fff).contains(&a)
                && !(a == 0x2001 && (b < 0x200 || b == 0xdb8))
                && a != 0x2002
                && a != 0x3fff
        }
    }
}

/// One request to an eligible endpoint.
pub fn rpc(peer: &Contact, configured: &[String], identity: &Identity, my_ep: Option<&str>, req: &Req, allow_legacy: bool) -> Option<(Address, Resp)> {
    use rand_core::{OsRng, RngCore};

    let addr = resolve_peer(&peer.ep, configured)?;
    let body = encode_req(&identity.address(), my_ep, req);
    if body.len() > MAX_MSG { return None; }
    let mut challenge = [0u8; 32];
    OsRng.fill_bytes(&mut challenge);
    let (id, response) = match exchange(addr, &body, MUTUAL_MAGIC, &challenge, identity) {
        Some(wire) => decode_signed_reply(&challenge, &body, &wire)?,
        None if allow_legacy && configured.contains(&peer.ep) => {
            match exchange(addr, &body, AUTH_MAGIC, &challenge, identity) {
                Some(wire) => decode_signed_reply(&challenge, &body, &wire)?,
                None => decode_resp(&exchange(addr, &body, MAGIC, &challenge, identity)?)?,
            }
        }
        None => return None,
    };
    (peer.id == Address::default() || peer.id == id).then_some((id, response))
}

fn exchange(addr: SocketAddr, body: &[u8], protocol: [u8; 2], challenge: &[u8; 32], identity: &Identity) -> Option<Vec<u8>> {
    let mut stream = TcpStream::connect_timeout(&addr, RPC_TIMEOUT).ok()?;
    stream.set_write_timeout(Some(RPC_TIMEOUT)).ok()?;
    stream.write_all(&protocol).ok()?;
    if protocol != MAGIC {
        stream.write_all(challenge).ok()?;
    }
    let deadline = Instant::now() + RPC_TIMEOUT;
    let mut request = Vec::with_capacity(body.len() + 2 + REPLY_PROOF_LEN);
    request.extend_from_slice(&(body.len() as u16).to_le_bytes());
    request.extend_from_slice(body);
    if protocol == MUTUAL_MAGIC {
        let mut server_challenge = [0; 32];
        read_by(&mut stream, &mut server_challenge, deadline)?;
        request.extend_from_slice(&identity.public().to_bytes());
        request.extend_from_slice(&identity.sign_dht_request(&server_challenge, challenge, body));
    }
    stream.write_all(&request).ok()?;
    let limit = MAX_MSG + if protocol != MAGIC { REPLY_PROOF_LEN } else { 0 };
    read_frame(&mut stream, deadline, limit)
}

pub fn verify_request_proof(server_challenge: &[u8; 32], client_challenge: &[u8; 32], data: &[u8], proof: &[u8; 128]) -> Option<Address> {
    let identity = PublicIdentity::from_bytes(&proof[..64])?;
    let signature = proof[64..].try_into().ok()?;
    let (claimed_sender, _, _) = decode_req(data)?;
    (identity.address() == claimed_sender
        && identity.verify_dht_request(server_challenge, client_challenge, data, signature))
        .then_some(claimed_sender)
}

pub fn sign_reply(identity: &Identity, challenge: &[u8; 32], request: &[u8], mut response: Vec<u8>) -> Vec<u8> {
    let signature = identity.sign_dht_reply(challenge, request, &response);
    response.extend_from_slice(&identity.public().to_bytes());
    response.extend_from_slice(&signature);
    response
}

fn decode_signed_reply(challenge: &[u8; 32], request: &[u8], wire: &[u8]) -> Option<(Address, Resp)> {
    let (response, proof) = wire.split_at_checked(wire.len().checked_sub(REPLY_PROOF_LEN)?)?;
    let identity = PublicIdentity::from_bytes(proof.get(..64)?)?;
    let signature = proof.get(64..)?.try_into().ok()?;
    let result = decode_resp(response)?;
    (identity.address() == result.0 && identity.verify_dht_reply(challenge, request, response, signature)).then_some(result)
}

/// Reads a complete frame with an absolute deadline, even if bytes arrive slowly.
pub fn read_by(s: &mut TcpStream, mut buf: &mut [u8], deadline: Instant) -> Option<()> {
    while !buf.is_empty() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return None;
        }
        s.set_read_timeout(Some(remaining)).ok()?;
        match s.read(buf) {
            Ok(0) => return None,
            Ok(n) => buf = &mut buf[n..],
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
    Some(())
}

/// Reads one request from a connection where MAGIC has already been read.
pub fn read_request(s: &mut TcpStream, deadline: Instant) -> Option<Vec<u8>> {
    read_frame(s, deadline, MAX_MSG)
}

fn read_frame(s: &mut TcpStream, deadline: Instant, limit: usize) -> Option<Vec<u8>> {
    let mut len = [0u8; 2];
    read_by(s, &mut len, deadline)?;
    let n = u16::from_le_bytes(len) as usize;
    if n > limit {
        return None;
    }
    let mut b = vec![0u8; n];
    read_by(s, &mut b, deadline)?;
    Some(b)
}

pub fn write_response(s: &mut TcpStream, resp: &[u8]) {
    let mut buf = Vec::with_capacity(resp.len() + 2);
    buf.extend_from_slice(&(resp.len() as u16).to_le_bytes());
    buf.extend_from_slice(resp);
    let _ = s.write_all(&buf);
}

/// The result of a lookup.
#[derive(Default, Debug)]
pub struct Lookup {
    /// Record found (only for `FindValue`).
    pub record: Option<Record>,
    /// Contacts that answered (put in the routing table).
    pub alive: Vec<Contact>,
    /// Contacts that did not answer (removed from the routing table).
    pub dead: Vec<Address>,
    /// The K closest live contacts to the target.
    pub closest: Vec<Contact>,
}

/// Iterative Kademlia lookup. `ask` performs one request (replaceable,
/// so the logic can be tested without a network).
pub fn lookup(
    me: &Address,
    start: Vec<Contact>,
    target: &Address,
    want_value: bool,
    mut ask: impl FnMut(&Contact, &Req) -> Option<(Address, Resp)>,
) -> Lookup {
    let mut res = Lookup::default();
    let mut short: Vec<Contact> = start;
    // Endpoints we have asked (seeds are only known by endpoint, ID = 0).
    let mut asked: HashSet<String> = HashSet::new();
    let mut alive: HashSet<Address> = HashSet::new();
    let req = if want_value {
        Req::FindValue(*target)
    } else {
        Req::FindNode(*target)
    };
    for _round in 0..20 {
        short.retain(|c| c.id != *me);
        short.sort_by_key(|c| distance(&c.id, target));
        short.dedup_by(|a, b| a.id == b.id);
        let todo: Vec<Contact> = short
            .iter()
            .filter(|c| !asked.contains(&c.ep))
            .take(K)
            .take(ALPHA)
            .cloned()
            .collect();
        if todo.is_empty() {
            break;
        }
        for c in todo {
            asked.insert(c.ep.clone());
            match ask(&c, &req) {
                Some((id, resp)) if id == c.id || c.id == Address::default() => {
                    let c = Contact { id, ep: c.ep };
                    if alive.insert(id) {
                        res.alive.push(c.clone());
                    }
                    match resp {
                        Resp::Value(r) if want_value && r.dest == *target && r.valid() => {
                            res.record = Some(r);
                        }
                        Resp::Nodes(v) => short.extend(v),
                        _ => {}
                    }
                    // The contact may have been known under a temporary ID.
                    short.retain(|x| x.id != Address::default() || x.ep != c.ep);
                    short.push(c);
                }
                _ => {
                    if c.id != Address::default() {
                        res.dead.push(c.id);
                    }
                }
            }
            if res.record.is_some() {
                break;
            }
        }
        if res.record.is_some() {
            break;
        }
    }
    short.retain(|c| alive.contains(&c.id));
    short.sort_by_key(|c| distance(&c.id, target));
    short.dedup_by(|a, b| a.id == b.id);
    short.truncate(K);
    res.closest = short;
    res
}

#[cfg(test)]
mod tests {
    #[test]
    fn mail_requests_roundtrip() {
        let me = Address([1; 16]);
        let r = Address([2; 16]);
        for req in [
            Req::MailStore { recipient: r, sealed: vec![9; 100] },
            Req::MailFetch { recipient: r, have: vec![1, 2, 3] },
            Req::MailDelivered { recipient: r, ids: vec![7] },
        ] {
            let b = encode_req(&me, Some("h:1"), &req);
            assert_eq!(decode_req(&b), Some((me, Some("h:1".to_string()), req)));
        }
        let resp = Resp::Mail(vec![vec![1; 50], vec![2; 60]]);
        assert_eq!(decode_resp(&encode_resp(&me, &resp)), Some((me, resp)));
    }
    use super::*;

    #[test]
    fn caps_roundtrip() {
        let c = Caps { version: "0.1.0".into(), server: true, radio: false, regions: vec![0x1234, 7], post_office: false, call_test: true, observe: true, mail_free_mb: 512, relay: true };
        let me = Address([1; 16]);
        let b = encode_resp(&me, &Resp::Caps(c.clone()));
        assert_eq!(decode_resp(&b), Some((me, Resp::Caps(c.clone()))));
        assert_eq!(decode_req(&encode_req(&me, None, &Req::Caps)), Some((me, None, Req::Caps)));
        let mut raw = Vec::new();
        c.encode(&mut raw);
        assert_eq!(Caps::decode(&raw), Some(c));
    }

    #[test]
    fn one_network_cannot_fill_the_table() {
        let mut t = Table::new(Address([0; 16]));
        for i in 0..20u8 {
            let mut id = [0xff; 16];
            id[15] = i;
            id[0] = 0x80 | i;
            t.insert(Contact { id: Address(id), ep: format!("203.0.113.{i}:4242") });
        }
        assert_eq!(t.len(), MAX_PER_NET);
        t.insert(Contact { id: Address([0x40; 16]), ep: "198.51.100.7:4242".into() });
        t.insert(Contact { id: Address([0x20; 16]), ep: "127.0.0.1:4243".into() });
        t.insert(Contact { id: Address([0x10; 16]), ep: "127.0.0.1:4244".into() });
        assert_eq!(t.len(), MAX_PER_NET + 3, "other networks and loopback are not limited");
        assert_eq!(t.spread().len(), 4, "one contact per non-empty bucket");
    }

    #[test]
    fn contacts_are_saved_and_loaded() {
        let dir = std::env::temp_dir().join(format!("maarse-dht-contacts-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("c.txt");
        let cs = vec![Contact { id: Address([7; 16]), ep: "a.example:4242".into() }, Contact { id: Address([9; 16]), ep: "[2001:db8::1]:4242".into() }];
        save_contacts(&p, &cs);
        std::fs::write(dir.join("junk.txt"), "zz nothing\n").unwrap();
        assert_eq!(load_contacts(&p), cs);
        assert!(load_contacts(&dir.join("junk.txt")).is_empty());
    }
    use rand_core::OsRng;
    use maarse_crypto::Identity;

    fn a(n: u8) -> Address {
        let mut x = [0u8; 16];
        x[0] = n;
        Address(x)
    }

    fn record(id: &Identity, gw: Address, hops: u8) -> Record {
        Record {
            dest: id.address(),
            gw,
            ep: "127.0.0.1:1".into(),
            hops,
            age_s: 0,
            announce: id.announce(1, &[]).unwrap().to_vec(),
            proof: None,
        }
    }

    #[test]
    fn request_proof_requires_the_claimed_identity_and_both_challenges() {
        let identity = Identity::generate(&mut OsRng);
        let request = encode_req(&identity.address(), None, &Req::Caps);
        let client = [23; 32];
        let server = [17; 32];
        let mut proof = [0; 128];
        proof[..64].copy_from_slice(&identity.public().to_bytes());
        proof[64..].copy_from_slice(&identity.sign_dht_request(&server, &client, &request));
        assert_eq!(verify_request_proof(&server, &client, &request, &proof), Some(identity.address()));
        assert!(verify_request_proof(&[18; 32], &client, &request, &proof).is_none());
        assert!(verify_request_proof(&server, &[24; 32], &request, &proof).is_none());
        let wrong_sender = encode_req(&a(1), None, &Req::Caps);
        proof[64..].copy_from_slice(&identity.sign_dht_request(&server, &client, &wrong_sender));
        assert!(verify_request_proof(&server, &client, &wrong_sender, &proof).is_none());
    }

    #[test]
    fn dht_response_proof_is_bound_to_the_request_and_challenge() {
        let responder = Identity::generate(&mut OsRng);
        let nonce = [17; 32];
        let request = encode_req(&a(1), None, &Req::Ping);
        let response = encode_resp(&responder.address(), &Resp::Pong);
        let signed = sign_reply(&responder, &nonce, &request, response);
        assert_eq!(decode_signed_reply(&nonce, &request, &signed), Some((responder.address(), Resp::Pong)));
        assert!(decode_signed_reply(&[18; 32], &request, &signed).is_none());
        assert!(decode_signed_reply(&nonce, &encode_req(&a(1), None, &Req::Caps), &signed).is_none());
        assert!(decode_signed_reply(&nonce, &request, &signed[..signed.len()-1]).is_none());
    }

    #[test]
    fn signed_gateway_proof_survives_wire_roundtrip_and_binds_route() {
        let dest = Identity::generate(&mut OsRng);
        let gateway = Identity::generate(&mut OsRng);
        let route = Record::signed(&gateway, dest.address(), "gateway.example:4242".into(), 2,
            dest.announce(1, &[]).unwrap().to_vec());
        let request = Req::Store(route.clone());
        let encoded = encode_req(&gateway.address(), None, &request);
        assert_eq!(decode_req(&encoded), Some((gateway.address(), None, request)));
        assert!(route.verified_gateway());
        let mut store = Store::new();
        assert!(store.put(route.clone()));
        let mut altered = route;
        altered.hops += 1;
        assert!(!altered.valid());
        altered.hops -= 1;
        altered.ep = "other.example:4242".into();
        assert!(!altered.valid());
        assert!(!store.put(record(&dest, a(4), 0)));
    }

    #[test]
    fn wire_roundtrip() {
        let id = Identity::generate(&mut OsRng);
        let r = record(&id, a(9), 2);
        for req in [
            Req::Ping,
            Req::FindNode(a(3)),
            Req::FindValue(a(4)),
            Req::Store(r.clone()),
        ] {
            let b = encode_req(&a(1), Some("h:1"), &req);
            assert_eq!(decode_req(&b), Some((a(1), Some("h:1".into()), req)));
        }
        for resp in [
            Resp::Pong,
            Resp::Nodes(vec![Contact {
                id: a(5),
                ep: "x:2".into(),
            }]),
            Resp::Value(r),
            Resp::Stored(true),
        ] {
            let b = encode_resp(&a(2), &resp);
            assert_eq!(decode_resp(&b), Some((a(2), resp)));
        }
        assert_eq!(decode_req(&[2, 1, 2]), None);
    }

    #[test]
    fn discovered_endpoints_require_a_public_address() {
        for endpoint in ["127.0.0.1:4242", "10.0.0.1:4242", "169.254.169.254:4242",
            "[::ffff:127.0.0.1]:4242", "[fe80::1]:4242", "8.8.8.8:0"] {
            assert!(resolve_peer(endpoint, &[]).is_none(), "{endpoint}");
        }
        assert!(resolve_peer("8.8.8.8:4242", &[]).is_some());
        let configured = vec!["127.0.0.1:4242".into()];
        assert!(resolve_peer("127.0.0.1:4242", &configured).is_some());
        assert!(resolve_peer("127.0.0.1:4243", &configured).is_none());
    }

    #[test]
    fn an_unverified_contact_cannot_replace_an_existing_endpoint() {
        let mut table = Table::new(a(1));
        table.insert(Contact { id: a(2), ep: "one.example:4242".into() });
        table.insert(Contact { id: a(2), ep: "two.example:4242".into() });
        assert_eq!(table.all()[0].ep, "one.example:4242");
    }

    #[test]
    fn store_checks_signature_and_prefers_fewer_hops() {
        let id = Identity::generate(&mut OsRng);
        let mut s = Store::new();
        let mut bad = record(&id, a(1), 1);
        bad.announce[40] ^= 1;
        assert!(!s.put(bad));
        assert!(s.put(record(&id, a(1), 3)));
        assert!(s.put(record(&id, a(2), 1)));
        assert!(!s.put(record(&id, a(3), 2)));
        assert_eq!(s.get(&id.address()).unwrap().gw, a(2));
    }

    #[test]
    fn table_orders_by_xor_distance() {
        let mut t = Table::new(a(0));
        for n in [0x80, 0x40, 0x41, 0x01, 0x02] {
            t.insert(Contact {
                id: a(n),
                ep: format!("h:{n}"),
            });
        }
        t.insert(Contact {
            id: a(0),
            ep: "self:1".into(),
        });
        assert_eq!(t.len(), 5);
        let c = t.closest(&a(0x43), 2);
        assert_eq!(c[0].id, a(0x41));
        assert_eq!(c[1].id, a(0x40));
    }

    /// 200 simulated hosts, each knows only 3 random others: the lookup
    /// must still find the record, which is stored at the K closest.
    #[test]
    fn lookup_finds_value_in_simulated_network() {
        let n = 200;
        let ids: Vec<Address> = (0..n)
            .map(|_| Identity::generate(&mut OsRng).address())
            .collect();
        let mut tables: Vec<Table> = ids.iter().map(|i| Table::new(*i)).collect();
        let mut rng = 12345u64;
        let mut next = || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng as usize
        };
        for (i, t) in tables.iter_mut().enumerate() {
            for _ in 0..3 {
                let j = next() % n;
                t.insert(Contact {
                    id: ids[j],
                    ep: j.to_string(),
                });
            }
            let j = (i + 1) % n; // a ring, so the network is connected
            t.insert(Contact {
                id: ids[j],
                ep: j.to_string(),
            });
        }
        // Let every host do a lookup for itself (bootstrap), so the
        // tables fill up as in reality.
        for i in 0..n {
            let start = tables[i].closest(&ids[i], K);
            let res = lookup(&ids[i], start, &ids[i], false, |c, req| {
                let j: usize = c.ep.parse().unwrap();
                let (from, from_ep) = (ids[i], i.to_string());
                tables[j].insert(Contact {
                    id: from,
                    ep: from_ep,
                });
                match req {
                    Req::FindNode(t) => Some((ids[j], Resp::Nodes(tables[j].closest(t, K)))),
                    _ => None,
                }
            });
            for c in res.alive {
                tables[i].insert(c);
            }
        }
        // Store a record at the K closest to the target.
        let dest_id = Identity::generate(&mut OsRng);
        let dest = dest_id.address();
        let mut stores: Vec<Store> = (0..n).map(|_| Store::new()).collect();
        let res = lookup(&ids[0], tables[0].closest(&dest, K), &dest, false, |c, req| {
            let j: usize = c.ep.parse().unwrap();
            match req {
                Req::FindNode(t) => Some((ids[j], Resp::Nodes(tables[j].closest(t, K)))),
                _ => None,
            }
        });
        assert!(res.closest.len() >= K / 2);
        for c in &res.closest {
            let j: usize = c.ep.parse().unwrap();
            assert!(stores[j].put(record(&dest_id, ids[0], 1)));
        }
        // Another host finds it.
        let asker = 137;
        let res = lookup(
            &ids[asker],
            tables[asker].closest(&dest, K),
            &dest,
            true,
            |c, req| {
                let j: usize = c.ep.parse().unwrap();
                match req {
                    Req::FindValue(t) => Some((
                        ids[j],
                        match stores[j].get(t) {
                            Some(r) => Resp::Value(r),
                            None => Resp::Nodes(tables[j].closest(t, K)),
                        },
                    )),
                    _ => None,
                }
            },
        );
        assert_eq!(res.record.map(|r| r.gw), Some(ids[0]));
    }
}
