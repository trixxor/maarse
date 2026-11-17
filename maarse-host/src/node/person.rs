//! Linked devices (one person, several devices), first stage: see
//! `docs/maybe/multi-device.md`.
//!
//! * A person is a master identity M from 24 words. One device (the master
//!   device) keeps M's secret and signs a certificate for each device and a
//!   device list. Every device keeps its own identity for routing and
//!   encryption.
//! * Devices say "I am device i of M" in their announce (`APP_PERSON`). The
//!   claim is trusted only after the device's certificate (signed by M) has
//!   been checked; a device sends it before its first message to a new
//!   contact, and answers when asked.
//! * Writing to M: the message goes to the device of M whose announce we
//!   heard last ("newest announce wins"), encrypted to that device as today.
//! * Messages from any device of M are shown as from M.
//! * **Sibling sync**: the device that receives a message for M sends a copy
//!   to each of M's other devices, and so does the device that sends one. The
//!   copies are ordinary encrypted messages to the sibling, so they go over
//!   TCP or LoRa and wait in post offices like any message.
//!
//! Not yet (next stages): the weekly inbox key and post offices keyed on M,
//! hello time and sequence numbers, backup v2, revocation from the client.

use super::{Node, Pending, err_ev, short, unix_now};
use crate::ClientId;
use maarse_core::Address;
use maarse_core::appdata;
use maarse_core::companion::{Command, Event, PersonInfo, error_code, person_op, person_state, text};
use maarse_core::content::{self, person as op};
use maarse_crypto::person::{CERT_LEN, DeviceCert, DeviceEntry, DeviceList, MAX_DEVICES, link_words};
use maarse_crypto::{Identity, recovery};
use rand_core::{OsRng, RngCore};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long a device waits to be linked.
const AWAIT_FOR: Duration = Duration::from_secs(600);
/// How long a message to a person waits for a device's certificate.
const CERT_WAIT: Duration = Duration::from_secs(60);
/// Text per sibling copy piece (FS-encrypted, with the header).
const PIECE: usize = maarse_crypto::ratchet::MAX_FS_TEXT - content::person::SIBLING_HEADER;

/// This device as a member of a person.
pub(super) struct Member {
    /// M's secret, on the master device only.
    master: Option<Identity>,
    cert: DeviceCert,
    list: DeviceList,
}

impl Member {
    fn person(&self) -> Address {
        self.cert.person()
    }
    fn siblings<'a>(&'a self, me: &'a Address) -> impl Iterator<Item = Address> + 'a {
        self.list.active().filter(move |a| a != me)
    }
}

/// A message to a person, waiting for a device's certificate.
struct Waiting {
    person: Address,
    client: ClientId,
    req: u16,
    text: Vec<u8>,
    since: Instant,
}

pub(super) struct Persons {
    dir: PathBuf,
    pub(super) me: Option<Member>,
    /// Waiting to be linked until then.
    awaiting: Option<Instant>,
    /// A certificate from a master device, waiting for the device list.
    granted: Option<DeviceCert>,
    /// Checked certificates of other devices: device -> certificate.
    certs: HashMap<Address, DeviceCert>,
    /// Device lists we know (our own person's, and others' sent to us).
    lists: HashMap<Address, DeviceList>,
    /// Claims from announces: device -> (person, when heard).
    claims: HashMap<Address, (Address, Instant)>,
    /// Contacts we have shown our certificate to (this run).
    introduced: std::collections::HashSet<Address>,
    /// Devices asked for their certificate, and when.
    asked: HashMap<Address, Instant>,
    waiting: Vec<Waiting>,
    /// Sibling copies arriving in pieces: (sibling, id) -> pieces.
    pieces: HashMap<(Address, u32), (u8, u8, Address, Vec<Option<Vec<u8>>>)>,
}

fn read_member(dir: &Path, me: &Address) -> Option<Member> {
    let b = fs::read(dir.join("person.bin")).ok()?;
    // "MP1" | flags | [master secret (64)] | certificate | device list
    if b.len() < 4 || &b[..3] != b"MP1" {
        return None;
    }
    let mut o = 4;
    let master = if b[3] & 1 != 0 {
        let s: [u8; 64] = b.get(o..o + 64)?.try_into().ok()?;
        o += 64;
        Some(Identity::from_secret_bytes(&s))
    } else {
        None
    };
    let cert = DeviceCert::verify(me, b.get(o..o + CERT_LEN)?).ok()?;
    o += CERT_LEN;
    let list = DeviceList::verify(&cert.master, b.get(o..)?).ok()?;
    Some(Member { master, cert, list })
}

impl Persons {
    pub(super) fn load(dir: &Path, me: &Address) -> Self {
        let mut p = Persons {
            dir: dir.to_path_buf(),
            me: read_member(dir, me),
            awaiting: None,
            granted: None,
            certs: HashMap::new(),
            lists: HashMap::new(),
            claims: HashMap::new(),
            introduced: Default::default(),
            asked: HashMap::new(),
            waiting: Vec::new(),
            pieces: HashMap::new(),
        };
        // device (16) | certificate, one after the other.
        if let Ok(b) = fs::read(dir.join("person-certs.bin")) {
            for rec in b.chunks_exact(16 + CERT_LEN) {
                if let Some(d) = Address::from_slice(&rec[..16])
                    && let Ok(c) = DeviceCert::verify(&d, &rec[16..])
                {
                    p.certs.insert(d, c);
                }
            }
        }
        p
    }

    fn save_member(&self) {
        let path = self.dir.join("person.bin");
        let Some(m) = &self.me else {
            let _ = fs::remove_file(path);
            return;
        };
        let mut b = b"MP1".to_vec();
        b.push(m.master.is_some() as u8);
        if let Some(s) = &m.master {
            b.extend_from_slice(&s.to_secret_bytes());
        }
        b.extend_from_slice(&m.cert.to_bytes());
        b.extend_from_slice(&m.list.to_bytes());
        let _ = fs::write(path, b);
    }

    fn remember_cert(&mut self, c: DeviceCert) {
        if self.certs.insert(c.device, c).is_none()
            && let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(self.dir.join("person-certs.bin"))
        {
            use std::io::Write;
            let _ = f.write_all(&c.device.0);
            let _ = f.write_all(&c.to_bytes());
        }
    }

    /// The person a device belongs to (checked certificate, not revoked).
    pub(super) fn person_of(&self, device: &Address) -> Option<Address> {
        let p = self.certs.get(device)?.person();
        if self.lists.get(&p).is_some_and(|l| l.is_revoked(device)) {
            return None;
        }
        Some(p)
    }

    /// Is `a` a person we know devices of?
    fn is_person(&self, a: &Address) -> bool {
        self.claims.values().any(|(p, _)| p == a) || self.certs.values().any(|c| c.person() == *a)
    }
}

/// A version that only grows: the time, or one more than before.
fn next_version(old: u32) -> u32 {
    unix_now().max(old + 1)
}

impl Node {
    // ---------- state for clients ----------

    fn person_info(&self) -> PersonInfo {
        let p = &self.persons;
        let mut i = PersonInfo::default();
        if let Some(m) = &p.me {
            i.state = person_state::MEMBER;
            i.master = m.master.is_some();
            i.person = m.person();
            i.index = m.cert.index;
            i.version = m.list.version;
            for d in &m.list.devices {
                let _ = i.devices.push((d.addr, d.revoked));
            }
        } else if p.awaiting.is_some_and(|t| Instant::now() < t) {
            i.state = person_state::AWAITING_LINK;
            i.words = heapless::Vec::from_slice(link_words(&self.identity.public()).join(" ").as_bytes()).unwrap_or_default();
        }
        i
    }

    fn person_changed(&mut self) {
        let ev = Event::Person(self.person_info());
        self.broadcast(&ev);
    }

    /// Our announce field: "device i of M".
    pub(super) fn person_app_data(&self, v: &mut appdata::AppBuf) {
        if let Some(m) = &self.persons.me {
            appdata::push_person(v, &m.person(), m.cert.index);
        }
    }

    /// An announce heard: remember a device's claim to belong to a person.
    pub(super) fn person_on_announce(&mut self, device: Address, app: &[u8]) {
        if let Some((person, _)) = appdata::parse(app).person {
            self.persons.claims.insert(device, (person, Instant::now()));
        }
    }

    /// Messages from a device of a person are shown as from the person.
    pub(super) fn person_peer(&self, from: &Address) -> Address {
        self.persons.person_of(from).unwrap_or(*from)
    }

    // ---------- commands ----------

    /// Commands for linked devices, and sending to a person. Returns the
    /// command if it does not belong here.
    pub(super) fn person_command(&mut self, id: ClientId, cmd: Command) -> Option<Command> {
        match cmd {
            Command::GetPerson => self.send_client(id, &Event::Person(self.person_info())),
            Command::PersonSetup { req, op, text: t } => self.person_setup(id, req, op, &t),
            Command::LinkDevice { req, device } => self.link_device(id, req, device),
            Command::SendDirect { req, dest, text: msg } if self.persons.is_person(&dest) && !self.is_own_person(&dest) => {
                self.send_to_person(id, req, dest, msg.to_vec());
            }
            Command::SendDirect { req, dest, text: msg } => {
                // An ordinary contact: show who we are first, so the reply
                // and our other devices fit together.
                self.introduce(dest);
                return Some(Command::SendDirect { req, dest, text: msg });
            }
            other => return Some(other),
        }
        None
    }

    fn is_own_person(&self, a: &Address) -> bool {
        self.persons.me.as_ref().is_some_and(|m| m.person() == *a)
    }

    fn person_setup(&mut self, id: ClientId, req: u16, op_: u8, t: &[u8]) {
        let fail = |s: &mut Self, e: &str| s.send_client(id, &err_ev(req, error_code::BAD_FRAME, e));
        match op_ {
            person_op::CREATE | person_op::RESTORE => {
                if self.persons.me.is_some() {
                    return fail(self, "This device already belongs to a person (leave it first)");
                }
                let (phrase, master) = if op_ == person_op::CREATE {
                    let mut e = [0u8; recovery::ENTROPY_LEN];
                    OsRng.fill_bytes(&mut e);
                    let phrase = recovery::entropy_to_phrase(&e).to_string();
                    let Ok(m) = recovery::identity_from_phrase(&phrase, "") else { return fail(self, "Could not make the phrase") };
                    (Some(phrase), m)
                } else {
                    let words = String::from_utf8_lossy(t).split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
                    match recovery::identity_from_phrase(&words, "") {
                        Ok(m) => (None, m),
                        Err(e) => return fail(self, &format!("The phrase is not valid: {e:?}")),
                    }
                };
                let cert = DeviceCert::issue(&master, &self.addr, 0);
                let Ok(list) = DeviceList::issue(&master, next_version(0), &[DeviceEntry { addr: self.addr, revoked: false }]) else {
                    return fail(self, "Could not sign the device list");
                };
                self.persons.lists.insert(master.address(), list.clone());
                self.log(format!("this device is now the master device of {}", short(&master.address())));
                self.persons.me = Some(Member { master: Some(master), cert, list });
                self.persons.awaiting = None;
                self.persons.save_member();
                if let Some(p) = phrase {
                    self.send_client(id, &Event::PersonPhrase { phrase: text(p.as_bytes()) });
                }
                self.person_changed();
                self.announce();
            }
            person_op::AWAIT_LINK => {
                if self.persons.me.is_some() {
                    return fail(self, "This device already belongs to a person (leave it first)");
                }
                self.persons.awaiting = Some(Instant::now() + AWAIT_FOR);
                self.persons.granted = None;
                self.person_changed();
                // The master device needs our keys: tell everyone who we are.
                self.announce();
            }
            person_op::CANCEL => {
                self.persons.awaiting = None;
                self.persons.granted = None;
                self.person_changed();
            }
            person_op::LEAVE => {
                self.persons.me = None;
                self.persons.save_member();
                self.log("this device no longer belongs to a person");
                self.person_changed();
                self.announce();
            }
            _ => fail(self, "Unknown operation"),
        }
    }

    fn link_device(&mut self, id: ClientId, req: u16, device: Address) {
        let fail = |s: &mut Self, e: &str| s.send_client(id, &err_ev(req, error_code::UNKNOWN_DEST, e));
        let Some(m) = self.persons.me.as_ref() else { return fail(self, "This device does not belong to a person") };
        let Some(master) = m.master.as_ref() else {
            return fail(self, "Only the master device can link devices");
        };
        let Some(pubkey) = self.known.get(&device).copied() else {
            return fail(self, "The new device has not been heard yet: choose \"Link to an account\" on it and keep it open");
        };
        let mut devices: Vec<DeviceEntry> = m.list.devices.iter().copied().filter(|d| d.addr != device).collect();
        if devices.len() >= MAX_DEVICES {
            return fail(self, "The person already has 4 devices");
        }
        let index = (0..=255u8).find(|i| !devices.iter().any(|d| self.persons.certs.get(&d.addr).is_some_and(|c| c.index == *i)) && *i != 0).unwrap_or(1);
        devices.push(DeviceEntry { addr: device, revoked: false });
        let cert = DeviceCert::issue(master, &device, index);
        let Ok(list) = DeviceList::issue(master, next_version(m.list.version), &devices) else {
            return fail(self, "Could not sign the device list");
        };
        let person = m.person();
        self.persons.remember_cert(cert);
        if let Some(me) = self.persons.me.as_mut() {
            me.list = list.clone();
        }
        self.persons.lists.insert(person, list.clone());
        self.persons.save_member();
        // The grant to the new device, then the list to every device.
        let mut g = vec![content::PERSON, op::LINK_GRANT];
        g.extend_from_slice(&cert.to_bytes());
        let _ = self.send_sealed(device, &g, Pending::Control);
        self.send_list_to_all(&list);
        let words = link_words(&pubkey).join(" ");
        self.log(format!("linked {} as device {index} ({words})", short(&device)));
        self.broadcast(&Event::DeviceLinked { device, index, words: heapless::Vec::from_slice(words.as_bytes()).unwrap_or_default() });
        self.person_changed();
    }

    fn send_list_to_all(&mut self, list: &DeviceList) {
        let mut l = vec![content::PERSON, op::LIST];
        l.extend_from_slice(&list.to_bytes());
        let me = self.addr;
        for d in list.active().filter(|d| *d != me).collect::<Vec<_>>() {
            let _ = self.send_sealed(d, &l, Pending::Control);
        }
    }

    // ---------- sending to a person ----------

    /// Our certificate to a contact, once per run.
    fn introduce(&mut self, peer: Address) {
        let Some(m) = &self.persons.me else { return };
        if !self.known.contains_key(&peer) || !self.persons.introduced.insert(peer) {
            return;
        }
        let mut c = vec![content::PERSON, op::CERT];
        c.extend_from_slice(&m.cert.to_bytes());
        let _ = self.send_sealed(peer, &c, Pending::Control);
    }

    /// The device of `person` to write to: checked certificate, newest announce.
    fn device_for(&self, person: &Address) -> Option<Address> {
        let p = &self.persons;
        p.claims
            .iter()
            .filter(|(d, (pp, _))| pp == person && p.person_of(d) == Some(*person) && self.known.contains_key(d))
            .max_by_key(|(_, (_, t))| *t)
            .map(|(d, _)| *d)
            .or_else(|| {
                p.certs.values().filter(|c| c.person() == *person && p.person_of(&c.device).is_some()).map(|c| c.device).find(|d| self.known.contains_key(d))
            })
    }

    fn send_to_person(&mut self, id: ClientId, req: u16, person: Address, msg: Vec<u8>) {
        if msg.len() > maarse_crypto::ratchet::MAX_FS_TEXT - 1 {
            return self.send_client(id, &err_ev(req, error_code::TOO_LARGE, "The message is too long"));
        }
        if let Some(device) = self.device_for(&person) {
            return self.person_deliver(id, req, person, device, &msg);
        }
        // Only unchecked claims: ask the newest device for its certificate.
        let newest = self.persons.claims.iter().filter(|(_, (p, _))| *p == person).max_by_key(|(_, (_, t))| *t).map(|(d, _)| *d);
        let Some(device) = newest else {
            return self.send_client(id, &err_ev(req, error_code::UNKNOWN_DEST, "No device of this person has been heard"));
        };
        self.ask_cert(device);
        self.persons.waiting.push(Waiting { person, client: id, req, text: msg, since: Instant::now() });
        let t = format!("Checking that {} belongs to {} …", short(&device), short(&person));
        self.send_client(id, &Event::Log { text: text(t.as_bytes()) });
    }

    fn ask_cert(&mut self, device: Address) {
        if self.persons.asked.get(&device).is_some_and(|t| t.elapsed() < Duration::from_secs(20)) {
            return;
        }
        self.persons.asked.insert(device, Instant::now());
        let _ = self.send_sealed(device, &[content::PERSON, op::CERT_REQUEST], Pending::Control);
    }

    fn person_deliver(&mut self, id: ClientId, req: u16, person: Address, device: Address, msg: &[u8]) {
        self.introduce(device);
        match self.send_sealed(device, &content::text(msg), Pending::Message) {
            Ok(msg_id) => {
                self.log(format!("message to {} via device {}", short(&person), short(&device)));
                self.send_client(id, &Event::Sent { req, msg_id });
                self.hist_sent(id, crate::history::KIND_DIRECT, person, msg_id, msg);
                self.person_sent(person, msg);
            }
            Err(e) => self.send_client(id, &err_ev(req, error_code::UNKNOWN_DEST, e)),
        }
    }

    /// Messages to a person whose certificate has now been checked, or that
    /// waited too long.
    fn person_flush(&mut self) {
        let waiting = std::mem::take(&mut self.persons.waiting);
        for w in waiting {
            if let Some(device) = self.device_for(&w.person) {
                self.person_deliver(w.client, w.req, w.person, device, &w.text);
            } else if w.since.elapsed() > CERT_WAIT {
                self.send_client(w.client, &err_ev(w.req, error_code::UNKNOWN_DEST, "No device of this person could be checked"));
            } else {
                self.persons.waiting.push(w);
            }
        }
    }

    /// Housekeeping: messages waiting for a certificate time out.
    pub(super) fn person_maintain(&mut self) {
        if !self.persons.waiting.is_empty() {
            self.person_flush();
        }
        if self.persons.awaiting.is_some_and(|t| Instant::now() >= t) {
            self.persons.awaiting = None;
            self.persons.granted = None;
            self.person_changed();
        }
    }

    // ---------- sibling sync ----------

    /// We sent `msg` to `peer`: a copy for our other devices.
    pub(super) fn person_sent(&mut self, peer: Address, msg: &[u8]) {
        self.sibling_copy(op::SIBLING_SENT, peer, msg);
    }

    /// We received `msg` from `peer` (a person or a device): a copy for our
    /// other devices.
    pub(super) fn person_received(&mut self, peer: Address, msg: &[u8]) {
        self.sibling_copy(op::SIBLING_RECEIVED, peer, msg);
    }

    fn sibling_copy(&mut self, kind: u8, peer: Address, msg: &[u8]) {
        let Some(m) = &self.persons.me else { return };
        let siblings: Vec<Address> = m.siblings(&self.addr).collect();
        if siblings.is_empty() {
            return;
        }
        let id = OsRng.next_u32();
        let pieces: Vec<&[u8]> = if msg.is_empty() { vec![&[][..]] } else { msg.chunks(PIECE).collect() };
        let count = pieces.len().min(15) as u8;
        for s in siblings {
            for (i, piece) in pieces.iter().take(15).enumerate() {
                let mut c = vec![content::PERSON, op::SIBLING, kind];
                c.extend_from_slice(&peer.0);
                c.extend_from_slice(&id.to_le_bytes());
                c.push(((i as u8) << 4) | count);
                c.extend_from_slice(piece);
                // As a message: if the sibling is away it waits in a post office.
                let _ = self.send_sealed(s, &c, Pending::Message);
            }
        }
    }

    // ---------- content from other devices ----------

    pub(super) fn on_person(&mut self, from: Address, o: u8, data: &[u8]) {
        match o {
            op::CERT => match DeviceCert::verify(&from, data) {
                Ok(c) => {
                    let new = !self.persons.certs.contains_key(&from);
                    self.persons.claims.entry(from).or_insert((c.person(), std::time::Instant::now()));
                    self.persons.remember_cert(c);
                    if new {
                        self.log(format!("{} is a device of {}", short(&from), short(&c.person())));
                    }
                    self.person_flush();
                }
                Err(e) => self.log(format!("certificate from {} refused: {e:?}", short(&from))),
            },
            op::CERT_REQUEST => {
                self.persons.introduced.remove(&from);
                self.introduce(from);
            }
            op::LINK_GRANT => {
                if !self.persons.awaiting.is_some_and(|t| Instant::now() < t) {
                    return self.log(format!("link from {} ignored: this device is not waiting to be linked", short(&from)));
                }
                match DeviceCert::verify(&self.addr, data) {
                    Ok(c) => {
                        self.log(format!("{} offers to link us to {}", short(&from), short(&c.person())));
                        self.persons.granted = Some(c);
                    }
                    Err(e) => self.log(format!("link from {} refused: {e:?}", short(&from))),
                }
            }
            op::LIST => self.on_list(from, data),
            op::SIBLING => self.on_sibling(from, data),
            _ => {}
        }
    }

    fn on_list(&mut self, from: Address, data: &[u8]) {
        // A list for the person we are joining, or already belong to.
        let master = match (&self.persons.me, &self.persons.granted) {
            (Some(m), _) => m.cert.master,
            (None, Some(g)) => g.master,
            _ => return,
        };
        let Ok(list) = DeviceList::verify(&master, data) else {
            return self.log(format!("device list from {} refused (bad signature)", short(&from)));
        };
        let person = master.address();
        if let Some(g) = self.persons.granted.take() {
            if !list.active().any(|d| d == self.addr) {
                return self.log("the device list does not hold this device");
            }
            self.persons.awaiting = None;
            self.persons.me = Some(Member { master: None, cert: g, list: list.clone() });
            self.persons.lists.insert(person, list);
            self.persons.save_member();
            self.log(format!("linked to {} as device {}", short(&person), g.index));
            let words = link_words(&self.identity.public()).join(" ");
            self.broadcast(&Event::DeviceLinked { device: self.addr, index: g.index, words: heapless::Vec::from_slice(words.as_bytes()).unwrap_or_default() });
            self.person_changed();
            self.announce();
            return;
        }
        let Some(m) = self.persons.me.as_mut() else { return };
        if list.version <= m.list.version {
            return;
        }
        let revoked = list.is_revoked(&self.addr) || !list.active().any(|d| d == self.addr);
        m.list = list.clone();
        self.persons.lists.insert(person, list);
        if revoked {
            self.log("this device was removed from the person");
            self.persons.me = None;
        }
        self.persons.save_member();
        self.person_changed();
        if revoked {
            self.announce();
        }
    }

    fn on_sibling(&mut self, from: Address, d: &[u8]) {
        let Some(m) = &self.persons.me else { return };
        if !m.list.active().any(|a| a == from) || from == self.addr {
            return self.log(format!("sibling copy from {} ignored: not one of our devices", short(&from)));
        }
        if d.len() < 1 + 16 + 4 + 1 {
            return;
        }
        let kind = d[0];
        let Some(peer) = Address::from_slice(&d[1..17]) else { return };
        let id = u32::from_le_bytes([d[17], d[18], d[19], d[20]]);
        let (i, count) = (d[21] >> 4, (d[21] & 0x0f).max(1));
        let piece = d[22..].to_vec();
        let entry = self.persons.pieces.entry((from, id)).or_insert_with(|| (kind, count, peer, vec![None; count as usize]));
        if let Some(slot) = entry.3.get_mut(i as usize) {
            *slot = Some(piece);
        }
        if entry.3.iter().any(Option::is_none) {
            return;
        }
        let (kind, _, peer, parts) = self.persons.pieces.remove(&(from, id)).unwrap_or_default();
        let msg: Vec<u8> = parts.into_iter().flatten().flatten().collect();
        match kind {
            op::SIBLING_RECEIVED => {
                self.log(format!("copy from our device {}: message from {}", short(&from), short(&peer)));
                self.broadcast(&Event::DirectReceived { from: peer, hops: 0, text: text(&msg) });
            }
            op::SIBLING_SENT => {
                self.log(format!("copy from our device {}: message to {}", short(&from), short(&peer)));
                self.hist_sent(0, crate::history::KIND_DIRECT, peer, 0, &msg);
            }
            _ => {}
        }
    }
}
