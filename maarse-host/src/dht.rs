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
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};
use maarse_core::Address;
use maarse_crypto::verify_announce;

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

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Contact {
    pub id: Address,
    pub ep: String,
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
}

impl Record {
    /// Is the announce genuine and the record sensible?
    pub fn valid(&self) -> bool {
        !self.ep.is_empty()
            && self.ep.len() <= MAX_EP
            && self.announce.len() <= maarse_core::MAX_PACKET
            && (self.age_s as u64) < RECORD_TTL.as_secs()
            && verify_announce(&self.dest, &self.announce).is_ok()
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
            let old_age = Self::age(old, at);
            let better = r.hops < old.hops
                || (r.hops == old.hops && r.age_s as u64 <= old_age)
                || old_age > 2 * REPUBLISH.as_secs();
            if !better {
                return false;
            }
        }
        self.map.insert(r.dest, (r, Instant::now()));
        true
    }

    pub fn get(&self, dest: &Address) -> Option<Record> {
        let (r, at) = self.map.get(dest)?;
        let age = Self::age(r, at);
        (age < RECORD_TTL.as_secs()).then(|| Record {
            age_s: age as u32,
            ..r.clone()
        })
    }

    pub fn expire(&mut self) {
        self.map
            .retain(|_, (r, at)| Self::age(r, at) < RECORD_TTL.as_secs());
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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resp {
    Pong,
    Nodes(Vec<Contact>),
    Value(Record),
    Stored(bool),
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
}

fn get_record(rd: &mut Rd) -> Option<Record> {
    let dest = rd.addr()?;
    let gw = rd.addr()?;
    let ep = rd.str()?;
    let hops = rd.u8()?;
    let age_s = u32::from_le_bytes(rd.take(4)?.try_into().ok()?);
    let n = u16::from_le_bytes(rd.take(2)?.try_into().ok()?) as usize;
    let announce = rd.take(n)?.to_vec();
    Some(Record {
        dest,
        gw,
        ep,
        hops,
        age_s,
        announce,
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
    });
    out.extend_from_slice(&from.0);
    put_str(&mut out, from_ep.unwrap_or(""));
    match req {
        Req::Ping => {}
        Req::FindNode(a) | Req::FindValue(a) => out.extend_from_slice(&a.0),
        Req::Store(r) => put_record(&mut out, r),
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
        _ => return None,
    };
    Some((from, resp))
}

// ---------------------------------------------------------------------------
// Network (blocking; run in separate threads)

/// One request to `ep`. Only answers if everything succeeds within the time limit.
pub fn rpc(ep: &str, me: &Address, my_ep: Option<&str>, req: &Req) -> Option<(Address, Resp)> {
    let addr = ep.to_socket_addrs().ok()?.next()?;
    let mut s = TcpStream::connect_timeout(&addr, RPC_TIMEOUT).ok()?;
    s.set_read_timeout(Some(RPC_TIMEOUT)).ok()?;
    s.set_write_timeout(Some(RPC_TIMEOUT)).ok()?;
    let body = encode_req(me, my_ep, req);
    let mut buf = Vec::with_capacity(body.len() + 4);
    buf.extend_from_slice(&MAGIC);
    buf.extend_from_slice(&(body.len() as u16).to_le_bytes());
    buf.extend_from_slice(&body);
    s.write_all(&buf).ok()?;
    let mut len = [0u8; 2];
    s.read_exact(&mut len).ok()?;
    let n = u16::from_le_bytes(len) as usize;
    if n > MAX_MSG {
        return None;
    }
    let mut resp = vec![0u8; n];
    s.read_exact(&mut resp).ok()?;
    decode_resp(&resp)
}

/// Reads one request from a connection where MAGIC has already been read.
pub fn read_request(s: &mut TcpStream) -> Option<Vec<u8>> {
    s.set_read_timeout(Some(RPC_TIMEOUT)).ok()?;
    let mut len = [0u8; 2];
    s.read_exact(&mut len).ok()?;
    let n = u16::from_le_bytes(len) as usize;
    if n > MAX_MSG {
        return None;
    }
    let mut b = vec![0u8; n];
    s.read_exact(&mut b).ok()?;
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
    use super::*;
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
        }
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
