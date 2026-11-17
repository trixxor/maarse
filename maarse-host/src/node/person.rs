//! Linked devices (one person, several devices): see
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
//!   heard last ("newest announce wins"), encrypted to that device.
//! * **Inbox key**: every device of M holds a weekly hash chain. Contacts get
//!   this week's public key with the certificate. When the chosen device does
//!   not answer, the message is kept by post offices **under M**, encrypted
//!   to the inbox, and handed to whichever device of M turns up. Devices of M
//!   also fetch M's mail from M's replica post offices themselves.
//! * Messages from any device of M are shown as from M.
//! * **Sibling sync**: the device that receives a message for M sends a copy
//!   to each of M's other devices, and so does the device that sends one.
//! * **Removing a device**: the master device revokes it in a new device list
//!   and starts a new inbox chain, which the removed device never gets.

use super::{Node, Pending, err_ev, short, unix_now};
use crate::ClientId;
use maarse_core::Address;
use maarse_core::appdata;
use maarse_core::companion::{Command, Event, PersonInfo, error_code, person_op, person_state, text};
use maarse_core::content::{self, person as op};
use maarse_crypto::person::{
    self as pc, CERT_LEN, DeviceCert, DeviceEntry, DeviceList, INBOX_STATE_LEN, InboxChain, InboxPublic, MAX_DEVICES, link_words,
};
use maarse_crypto::{Identity, recovery};
use rand_core::{OsRng, RngCore};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long a device waits to be linked.
const AWAIT_FOR: Duration = Duration::from_secs(600);
/// How long a message to a person waits for a device's certificate.
const CERT_WAIT: Duration = Duration::from_secs(60);
/// Text per sibling copy piece (FS-encrypted, with the header).
const PIECE: usize = maarse_crypto::ratchet::MAX_FS_TEXT - content::person::SIBLING_HEADER;
/// Weeks of inbox keys kept back (late mail, senders with an old key).
const KEEP_WEEKS: u16 = 2;
/// How often a device fetches its person's mail from the replicas.
const FETCH_EVERY: Duration = Duration::from_secs(600);
/// The backup the master device writes, and the one a client puts here to
/// restore (in the data folder; see `person_op::BACKUP`).
pub(crate) const BACKUP_FILE: &str = "account-backup.maarse";
pub(crate) const RESTORE_FILE: &str = "account-restore.maarse";

fn week_now() -> u16 {
    pc::week_of(unix_now())
}

/// This device as a member of a person.
pub(super) struct Member {
    /// M's secret, on the master device only.
    master: Option<Identity>,
    cert: DeviceCert,
    list: DeviceList,
    /// The inbox chain from the oldest week kept.
    inbox: Option<InboxChain>,
    /// The previous chain after a removal, until its weeks run out.
    old_inbox: Option<InboxChain>,
}

impl Member {
    fn person(&self) -> Address {
        self.cert.person()
    }
    fn siblings<'a>(&'a self, me: &'a Address) -> impl Iterator<Item = Address> + 'a {
        self.list.active().filter(move |a| a != me)
    }
    fn inbox_public(&self) -> Option<InboxPublic> {
        let w = week_now();
        let c = self.inbox.as_ref()?.at(w)?;
        Some(InboxPublic { chain: c.chain, week: w, key: c.public() })
    }
    /// The secret for an inbox message's chain and week.
    fn inbox_secret(&self, chain: u8, week: u16) -> Option<pc::InboxSecret> {
        if week > week_now() + 1 {
            return None;
        }
        [&self.inbox, &self.old_inbox].into_iter().flatten().find(|c| c.chain == chain).and_then(|c| c.at(week)).map(|c| c.secret())
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
    /// An inbox chain that came before the device list (while linking).
    granted_inbox: Option<InboxChain>,
    /// Checked certificates of other devices: device -> certificate.
    certs: HashMap<Address, DeviceCert>,
    /// Device lists we know (our own person's, and others' sent to us).
    lists: HashMap<Address, DeviceList>,
    /// Claims from announces: device -> (person, when heard).
    claims: HashMap<Address, (Address, Instant)>,
    /// Persons' inbox keys this week, from their devices: person -> key.
    inboxes: HashMap<Address, InboxPublic>,
    /// An inbox key from a device whose certificate has not come yet.
    early_inbox: HashMap<Address, InboxPublic>,
    /// Contacts we have shown our certificate to (this run).
    introduced: HashSet<Address>,
    /// Devices asked for their certificate, and when.
    asked: HashMap<Address, Instant>,
    waiting: Vec<Waiting>,
    /// Sibling copies arriving in pieces: (sibling, id) -> pieces.
    pieces: HashMap<(Address, u32), (u8, u8, Address, Vec<Option<Vec<u8>>>)>,
    /// Mail IDs of inbox messages already opened (mail may come twice: from
    /// a post office and from the replicas).
    seen_mail: HashSet<u32>,
    last_fetch: Option<Instant>,
    /// When the user last sent from a sibling (its sent copies tell us).
    sibling_active: HashMap<Address, Instant>,
    /// When the user last sent from this device.
    my_active: Option<Instant>,
    /// Contacts told to write to a sibling, and when (at most every 10 min).
    hinted: HashMap<Address, Instant>,
    /// Sync: copies asked from siblings up to this time (Unix s), and when
    /// the first automatic sync is due.
    sync_since: u32,
    auto_sync_at: Option<Instant>,
}

fn read_member(dir: &Path, me: &Address) -> Option<Member> {
    let b = fs::read(dir.join("person.bin")).ok()?;
    // "MP2" | flags (1 master, 2 inbox, 4 old inbox) | [master secret (64)]
    // | certificate | list length (u16) | list | [inbox (35)] | [old inbox (35)]
    if b.len() < 4 || &b[..3] != b"MP2" {
        return None;
    }
    let flags = b[3];
    let mut o = 4;
    let master = if flags & 1 != 0 {
        let s: [u8; 64] = b.get(o..o + 64)?.try_into().ok()?;
        o += 64;
        Some(Identity::from_secret_bytes(&s))
    } else {
        None
    };
    let cert = DeviceCert::verify(me, b.get(o..o + CERT_LEN)?).ok()?;
    o += CERT_LEN;
    let n = u16::from_le_bytes([*b.get(o)?, *b.get(o + 1)?]) as usize;
    o += 2;
    let list = DeviceList::verify(&cert.master, b.get(o..o + n)?).ok()?;
    o += n;
    let mut chain = |on: bool| -> Option<InboxChain> {
        if !on {
            return None;
        }
        let c = InboxChain::from_bytes(b.get(o..o + INBOX_STATE_LEN)?);
        o += INBOX_STATE_LEN;
        c
    };
    let inbox = chain(flags & 2 != 0);
    let old_inbox = chain(flags & 4 != 0);
    Some(Member { master, cert, list, inbox, old_inbox })
}

impl Persons {
    pub(super) fn load(dir: &Path, me: &Address) -> Self {
        let mut p = Persons {
            dir: dir.to_path_buf(),
            me: read_member(dir, me),
            awaiting: None,
            granted: None,
            granted_inbox: None,
            certs: HashMap::new(),
            lists: HashMap::new(),
            claims: HashMap::new(),
            inboxes: HashMap::new(),
            early_inbox: HashMap::new(),
            introduced: HashSet::new(),
            asked: HashMap::new(),
            waiting: Vec::new(),
            pieces: HashMap::new(),
            seen_mail: HashSet::new(),
            last_fetch: None,
            sibling_active: HashMap::new(),
            my_active: None,
            hinted: HashMap::new(),
            sync_since: fs::read_to_string(dir.join("person-sync.txt")).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0),
            // Catch up a little after start-up, when the links are up.
            auto_sync_at: Some(Instant::now() + Duration::from_secs(20)),
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
        if let Some(m) = &p.me {
            p.lists.insert(m.person(), m.list.clone());
        }
        p
    }

    fn save_member(&self) {
        let path = self.dir.join("person.bin");
        let Some(m) = &self.me else {
            let _ = fs::remove_file(path);
            return;
        };
        let mut b = b"MP2".to_vec();
        b.push(m.master.is_some() as u8 | (m.inbox.is_some() as u8) << 1 | (m.old_inbox.is_some() as u8) << 2);
        if let Some(s) = &m.master {
            b.extend_from_slice(&s.to_secret_bytes());
        }
        b.extend_from_slice(&m.cert.to_bytes());
        let l = m.list.to_bytes();
        b.extend_from_slice(&(l.len() as u16).to_le_bytes());
        b.extend_from_slice(&l);
        for c in [&m.inbox, &m.old_inbox].into_iter().flatten() {
            b.extend_from_slice(&c.to_bytes());
        }
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

fn parse_hex_addr(t: &[u8]) -> Option<Address> {
    let s = String::from_utf8_lossy(t).trim().to_lowercase();
    if s.len() != 32 {
        return None;
    }
    let b: Vec<u8> = (0..32).step_by(2).filter_map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect();
    Address::from_slice(&b)
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

    /// An announce heard: remember a device's claim to belong to a person,
    /// and hand it mail we keep for the person.
    pub(super) fn person_on_announce(&mut self, device: Address, app: &[u8]) {
        let Some((person, _)) = appdata::parse(app).person else { return };
        self.persons.claims.insert(device, (person, Instant::now()));
        self.person_mail_turned_up(person, device);
    }

    /// A device of `person` is here: deliver the person's mail to it (after
    /// its certificate has been checked).
    fn person_mail_turned_up(&mut self, person: Address, device: Address) {
        if self.is_own_person(&person) || !self.mailbox.get(&person).is_some_and(|m| !m.is_empty()) {
            return;
        }
        if self.persons.person_of(&device) == Some(person) {
            self.flush_person_mail(person, Some(device));
        } else {
            self.ask_cert(device);
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
            // Files, file limits and calls to a person go to its active device.
            Command::FileBegin { req, dest, size, mime, name } if self.persons.is_person(&dest) && !self.is_own_person(&dest) => match self.device_for(&dest) {
                Some(d) => { self.introduce(d); return Some(Command::FileBegin { req, dest: d, size, mime, name }) }
                None => self.send_client(id, &err_ev(req, error_code::UNKNOWN_DEST, "No device of this person has been checked yet; send a message first")),
            },
            Command::GetFileLimit { dest } if self.persons.is_person(&dest) => {
                return Some(Command::GetFileLimit { dest: self.device_for(&dest).unwrap_or(dest) });
            }
            Command::SendCall { dest, data } if self.persons.is_person(&dest) && !self.is_own_person(&dest) => {
                return Some(Command::SendCall { dest: self.device_for(&dest).unwrap_or(dest), data });
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

    pub(super) fn is_own_person(&self, a: &Address) -> bool {
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
                // A restored person starts a new chain: the old devices are
                // linked again (or removed) from here.
                let inbox = InboxChain::new(&mut OsRng, (OsRng.next_u32() & 0xff) as u8, week_now());
                self.persons.lists.insert(master.address(), list.clone());
                self.log(format!("this device is now the master device of {}", short(&master.address())));
                self.persons.me = Some(Member { master: Some(master), cert, list, inbox: Some(inbox), old_inbox: None });
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
                self.persons.granted_inbox = None;
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
            person_op::BACKUP => self.person_backup(id, req),
            person_op::SYNC => {
                if self.persons.me.is_none() {
                    return fail(self, "This device does not belong to a person");
                }
                self.person_sync();
                self.send_client(id, &Event::Log { text: text(b"Syncing with the other devices and the post offices") });
            }
            person_op::RESTORE_BACKUP => self.person_restore_backup(id, req, t),
            person_op::REMOVE => match parse_hex_addr(t) {
                Some(d) => self.remove_device(id, req, d),
                None => fail(self, "Give the device's address (32 hex characters)"),
            },
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
            return fail(self, "The person already has 4 devices (removed ones count too)");
        }
        let used: Vec<u8> = devices.iter().filter_map(|d| self.persons.certs.get(&d.addr).map(|c| c.index)).collect();
        let index = (1..=255u8).find(|i| !used.contains(i)).unwrap_or(1);
        devices.push(DeviceEntry { addr: device, revoked: false });
        let cert = DeviceCert::issue(master, &device, index);
        let Ok(list) = DeviceList::issue(master, next_version(m.list.version), &devices) else {
            return fail(self, "Could not sign the device list");
        };
        let person = m.person();
        let inbox = m.inbox.clone();
        self.persons.remember_cert(cert);
        if let Some(me) = self.persons.me.as_mut() {
            me.list = list.clone();
        }
        self.persons.lists.insert(person, list.clone());
        self.persons.save_member();
        // To the new device: the grant, the inbox chain, then the list (to every device).
        let mut g = vec![content::PERSON, op::LINK_GRANT];
        g.extend_from_slice(&cert.to_bytes());
        let _ = self.send_sealed(device, &g, Pending::Control);
        if let Some(c) = inbox {
            self.send_inbox_state(device, &c);
        }
        self.send_list_to_all(&list);
        let words = link_words(&pubkey).join(" ");
        self.log(format!("linked {} as device {index} ({words})", short(&device)));
        self.broadcast(&Event::DeviceLinked { device, index, words: heapless::Vec::from_slice(words.as_bytes()).unwrap_or_default() });
        self.person_changed();
    }

    fn remove_device(&mut self, id: ClientId, req: u16, device: Address) {
        let fail = |s: &mut Self, e: &str| s.send_client(id, &err_ev(req, error_code::UNKNOWN_DEST, e));
        let me = self.addr;
        let Some(m) = self.persons.me.as_ref() else { return fail(self, "This device does not belong to a person") };
        let Some(master) = m.master.as_ref() else { return fail(self, "Only the master device can remove devices") };
        if device == me {
            return fail(self, "The master device cannot remove itself (use \"Remove this device\")");
        }
        if !m.list.devices.iter().any(|d| d.addr == device && !d.revoked) {
            return fail(self, "That device is not one of the person's devices");
        }
        let devices: Vec<DeviceEntry> =
            m.list.devices.iter().map(|d| DeviceEntry { addr: d.addr, revoked: d.revoked || d.addr == device }).collect();
        let Ok(list) = DeviceList::issue(master, next_version(m.list.version), &devices) else {
            return fail(self, "Could not sign the device list");
        };
        let person = m.person();
        let old = m.inbox.clone();
        let chain = old.as_ref().map_or(1, |c| c.chain.wrapping_add(1));
        let fresh = InboxChain::new(&mut OsRng, chain, week_now());
        if let Some(me) = self.persons.me.as_mut() {
            me.list = list.clone();
            me.old_inbox = old.and_then(|c| c.at(week_now().saturating_sub(KEEP_WEEKS).max(c.week)));
            me.inbox = Some(fresh.clone());
        }
        self.persons.lists.insert(person, list.clone());
        self.persons.save_member();
        // The new chain to the remaining devices; the list to all, including
        // the removed one, so it knows.
        for d in list.active().filter(|d| *d != me).collect::<Vec<_>>() {
            self.send_inbox_state(d, &fresh);
        }
        self.send_list_to_all(&list);
        let mut l = vec![content::PERSON, op::LIST];
        l.extend_from_slice(&list.to_bytes());
        let _ = self.send_sealed(device, &l, Pending::Control);
        // Contacts get the new list and inbox key with our next message.
        self.persons.introduced.clear();
        self.log(format!("removed device {} from {}; new inbox chain {chain}", short(&device), short(&person)));
        self.person_changed();
    }

    fn send_inbox_state(&mut self, device: Address, c: &InboxChain) {
        let mut b = vec![content::PERSON, op::INBOX_STATE];
        b.extend_from_slice(&c.to_bytes());
        // As a message: a sibling that is away gets it from a post office.
        let _ = self.send_sealed(device, &b, Pending::Message);
    }

    fn send_list_to_all(&mut self, list: &DeviceList) {
        let mut l = vec![content::PERSON, op::LIST];
        l.extend_from_slice(&list.to_bytes());
        let me = self.addr;
        for d in list.active().filter(|d| *d != me).collect::<Vec<_>>() {
            let _ = self.send_sealed(d, &l, Pending::Control);
        }
    }


    // ---------- backup of the master (v2) ----------

    /// Writes `account-backup.maarse`: the master secret, the inbox chain and
    /// the device list, encrypted with a fresh 6-word code (sent once).
    fn person_backup(&mut self, id: ClientId, req: u16) {
        let fail = |s: &mut Self, e: &str| s.send_client(id, &err_ev(req, error_code::BAD_FRAME, e));
        let Some(m) = &self.persons.me else { return fail(self, "This device does not belong to a person") };
        let Some(master) = &m.master else { return fail(self, "Only the master device can make a backup") };
        // version (1) | master device (16) | master secret (64) | inbox (35) | list length (u16) | list
        let mut plain = vec![1u8];
        plain.extend_from_slice(&self.addr.0);
        plain.extend_from_slice(&master.to_secret_bytes());
        plain.extend_from_slice(&m.inbox.as_ref().map(|c| c.to_bytes()).unwrap_or([0; INBOX_STATE_LEN]));
        let l = m.list.to_bytes();
        plain.extend_from_slice(&(l.len() as u16).to_le_bytes());
        plain.extend_from_slice(&l);
        let code = pc::backup_code(&mut OsRng);
        let mut out = vec![0u8; plain.len() + pc::BACKUP_OVERHEAD];
        let Some(n) = pc::backup_seal(&code, &mut OsRng, &plain, &mut out) else { return fail(self, "Could not encrypt the backup") };
        if let Err(e) = fs::write(self.data_dir.join(BACKUP_FILE), &out[..n]) {
            return fail(self, &format!("Could not write the backup: {e}"));
        }
        self.log(format!("account backup written to {BACKUP_FILE}"));
        self.send_client(id, &Event::PersonBackup { code: text(code.as_bytes()) });
    }

    /// Becomes the master device from `account-restore.maarse` and the code.
    fn person_restore_backup(&mut self, id: ClientId, req: u16, code: &[u8]) {
        let fail = |s: &mut Self, e: &str| s.send_client(id, &err_ev(req, error_code::BAD_FRAME, e));
        if self.persons.me.as_ref().is_some_and(|m| m.master.is_some()) {
            return fail(self, "This device is already a master device");
        }
        let path = self.data_dir.join(RESTORE_FILE);
        let Ok(file) = fs::read(&path) else { return fail(self, "No backup file was given") };
        let mut plain = vec![0u8; file.len()];
        let Some(n) = pc::backup_open(&String::from_utf8_lossy(code), &file, &mut plain) else {
            return fail(self, "Wrong code, or the file is damaged");
        };
        let p = &plain[..n];
        if p.len() < 1 + 16 + 64 + INBOX_STATE_LEN + 2 || p[0] != 1 {
            return fail(self, "Unknown backup format");
        }
        let old_master = Address::from_slice(&p[1..17]).unwrap_or_default();
        let secret: [u8; 64] = p[17..81].try_into().unwrap_or([0; 64]);
        let master = Identity::from_secret_bytes(&secret);
        let old_inbox = InboxChain::from_bytes(&p[81..81 + INBOX_STATE_LEN]);
        let o = 81 + INBOX_STATE_LEN;
        let ln = u16::from_le_bytes([p[o], p[o + 1]]) as usize;
        let Some(old_list) = p.get(o + 2..o + 2 + ln).and_then(|b| DeviceList::verify(&master.public(), b).ok()) else {
            return fail(self, "The device list in the backup is not valid");
        };
        let _ = fs::remove_file(&path);
        // The old master device is removed (it may be lost or stolen); the
        // other devices stay. Removed ones are dropped to make room.
        let me = self.addr;
        let mut devices: Vec<DeviceEntry> = old_list
            .devices
            .iter()
            .filter(|d| !d.revoked || d.addr == old_master)
            .map(|d| DeviceEntry { addr: d.addr, revoked: d.revoked || (d.addr == old_master && d.addr != me) })
            .filter(|d| d.addr != me)
            .collect();
        devices.push(DeviceEntry { addr: me, revoked: false });
        while devices.len() > MAX_DEVICES {
            match devices.iter().position(|d| d.revoked) {
                Some(i) => {
                    devices.remove(i);
                }
                None => return fail(self, "The backup's device list is full"),
            }
        }
        let used: Vec<u8> = devices.iter().filter_map(|d| self.persons.certs.get(&d.addr).map(|c| c.index)).collect();
        let index = (0..=255u8).find(|i| !used.contains(i)).unwrap_or(0);
        let cert = DeviceCert::issue(&master, &me, index);
        let Ok(list) = DeviceList::issue(&master, next_version(old_list.version), &devices) else {
            return fail(self, "Could not sign the device list");
        };
        let chain = old_inbox.as_ref().map_or(1, |c| c.chain.wrapping_add(1));
        let fresh = InboxChain::new(&mut OsRng, chain, week_now());
        let person = master.address();
        self.persons.lists.insert(person, list.clone());
        self.persons.me = Some(Member {
            master: Some(master),
            cert,
            list: list.clone(),
            inbox: Some(fresh.clone()),
            old_inbox: old_inbox.and_then(|c| c.at(week_now().saturating_sub(KEEP_WEEKS).max(c.week))),
        });
        self.persons.awaiting = None;
        self.persons.save_member();
        // The others learn the new list first, then the new chain.
        self.send_list_to_all(&list);
        for d in list.active().filter(|d| *d != me).collect::<Vec<_>>() {
            self.send_inbox_state(d, &fresh);
        }
        if old_master != me {
            let mut l = vec![content::PERSON, op::LIST];
            l.extend_from_slice(&list.to_bytes());
            let _ = self.send_sealed(old_master, &l, Pending::Control);
        }
        self.persons.introduced.clear();
        self.log(format!("restored {} from a backup; this is the master device now", short(&person)));
        self.person_changed();
        self.announce();
    }

    // ---------- sending to a person ----------

    /// Our certificate, inbox key and (after a removal) device list to a
    /// contact, once per run.
    fn introduce(&mut self, peer: Address) {
        let Some(m) = &self.persons.me else { return };
        if !self.known.contains_key(&peer) || !self.persons.introduced.insert(peer) {
            return;
        }
        let mut c = vec![content::PERSON, op::CERT];
        c.extend_from_slice(&m.cert.to_bytes());
        let inbox = m.inbox_public();
        let list = m.list.devices.iter().any(|d| d.revoked).then(|| m.list.to_bytes());
        let _ = self.send_sealed(peer, &c, Pending::Control);
        if let Some(ip) = inbox {
            let mut b = vec![content::PERSON, op::INBOX_PUB];
            b.extend_from_slice(&ip.to_bytes());
            let _ = self.send_sealed(peer, &b, Pending::Control);
        }
        if let Some(l) = list {
            let mut b = vec![content::PERSON, op::LIST];
            b.extend_from_slice(&l);
            let _ = self.send_sealed(peer, &b, Pending::Control);
        }
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
        if !self.known.contains_key(&device) || self.persons.asked.get(&device).is_some_and(|t| t.elapsed() < Duration::from_secs(20)) {
            return;
        }
        self.persons.asked.insert(device, Instant::now());
        let _ = self.send_sealed(device, &[content::PERSON, op::CERT_REQUEST], Pending::Control);
    }

    fn person_deliver(&mut self, id: ClientId, req: u16, person: Address, device: Address, msg: &[u8]) {
        self.introduce(device);
        let plain = content::text(msg);
        match self.send_sealed(device, &plain, Pending::Message) {
            Ok(msg_id) => {
                // If the device does not answer, the post offices keep the
                // message under the person, for whichever device turns up.
                if let Some(ip) = self.persons.inboxes.get(&person).copied()
                    && ip.week + KEEP_WEEKS >= week_now()
                {
                    let mut out = [0u8; 255];
                    if let Ok(n) = pc::seal_inbox(&self.identity, &person, &ip, &mut OsRng, &plain, &mut out)
                        && let Some(o) = self.outbox.get_mut(&msg_id)
                    {
                        o.dest = person;
                        o.mail_sealed = out[..n].to_vec();
                    }
                }
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

    /// Housekeeping: messages waiting for a certificate time out, old inbox
    /// weeks are forgotten, and our person's mail is fetched now and then.
    pub(super) fn person_maintain(&mut self) {
        if !self.persons.waiting.is_empty() {
            self.person_flush();
        }
        if self.persons.awaiting.is_some_and(|t| Instant::now() >= t) {
            self.persons.awaiting = None;
            self.persons.granted = None;
            self.person_changed();
        }
        if self.persons.me.is_some() && self.persons.auto_sync_at.is_some_and(|t| Instant::now() >= t) {
            self.persons.auto_sync_at = None;
            self.person_sync();
        }
        let keep_from = week_now().saturating_sub(KEEP_WEEKS);
        let mut changed = false;
        if let Some(m) = self.persons.me.as_mut() {
            if let Some(c) = &m.inbox
                && c.week < keep_from
            {
                // Forward secrecy, a week at a time: the old K(n) is gone.
                m.inbox = c.at(keep_from);
                changed = true;
            }
            if let Some(c) = &m.old_inbox
                && c.week + KEEP_WEEKS < week_now()
            {
                m.old_inbox = None;
                changed = true;
            }
        }
        if changed {
            self.persons.save_member();
        }
        if let Some(person) = self.persons.me.as_ref().map(|m| m.person())
            && self.persons.last_fetch.is_none_or(|t| t.elapsed() >= FETCH_EVERY)
            && self.tx.is_some()
        {
            self.persons.last_fetch = Some(Instant::now());
            self.fetch_mail(person, Vec::new());
        }
    }

    // ---------- mail for a person ----------

    /// Mail for `person` that we keep: hand it to a checked device of the
    /// person (the one that turned up, or the newest). Delivery is
    /// acknowledged by that device; the mail ID stays the person's.
    pub(super) fn flush_person_mail(&mut self, person: Address, device: Option<Address>) -> bool {
        let Some(device) = device.filter(|d| self.persons.person_of(d) == Some(person)).or_else(|| self.device_for(&person)) else {
            return false;
        };
        if !self.router.has_route(&device) && !self.known.contains_key(&device) {
            return false;
        }
        let items: Vec<(u64, Vec<u8>)> = self.mailbox.get(&person).map(|m| m.iter().map(|(id, m)| (*id, m.sealed.clone())).collect()).unwrap_or_default();
        if items.is_empty() {
            return true;
        }
        self.log(format!("mailbox: {} message(s) for {} to its device {}", items.len(), short(&person), short(&device)));
        for (stored, sealed) in items {
            let now = self.now_ms();
            if let Ok((msg_id, actions)) = self.router.send_direct(device, &sealed, super::token(), now) {
                self.awaiting.insert(msg_id, Pending::MailDelivery(person));
                self.mail_alias.insert(msg_id, stored);
                self.apply(actions.into_iter().collect());
            }
        }
        true
    }

    /// A `MailSync` from a device that says it belongs to a person.
    pub(super) fn person_mail_sync(&mut self, asker: Address, named: bool) {
        let Some((person, _)) = self.persons.claims.get(&asker).copied() else { return };
        if named && !self.is_own_person(&person) {
            self.fetch_mail(person, Vec::new());
        }
        self.person_mail_turned_up(person, asker);
    }

    /// Our person's mail, fetched from its replicas: open it here and tell
    /// the replicas it is delivered. Returns false if it is not ours.
    pub(super) fn person_mail_fetched(&mut self, recipient: Address, items: &[Vec<u8>]) -> bool {
        if !self.is_own_person(&recipient) {
            return false;
        }
        let mut ids = Vec::new();
        for sealed in items {
            ids.push(maarse_core::mail::mail_id(&recipient, sealed));
            self.open_direct(sealed.clone(), 0, true);
        }
        if !ids.is_empty() {
            self.log(format!("fetched {} message(s) for our person from its replicas", ids.len()));
            let replicas = self.mail_replicas(&recipient);
            let (me, ep) = (self.addr, self.dht.public_ep.clone());
            std::thread::spawn(move || {
                for c in replicas {
                    let _ = crate::dht::rpc(&c.ep, &me, ep.as_deref(), &crate::dht::Req::MailDelivered { recipient, ids: ids.clone() });
                }
            });
        }
        true
    }

    /// An inbox message (first byte `0xF2`) for our person. `None` if it is
    /// not one (the caller tries the other formats).
    pub(super) fn try_open_inbox(&mut self, payload: &[u8], hops: u8) -> Option<Result<(), maarse_crypto::seal::CryptoError>> {
        let (chain, week) = pc::inbox_header(payload)?;
        let m = self.persons.me.as_ref()?;
        let person = m.person();
        let secret = m.inbox_secret(chain, week)?;
        let current = m.inbox.as_ref().map(|c| c.chain);
        let mut out = [0u8; 256];
        let known = &self.known;
        let (from, n) = match pc::open_inbox(&secret, &person, payload, |a| known.get(a).copied(), &mut out) {
            Ok(r) => r,
            Err(e) => return Some(Err(e)),
        };
        if !self.persons.seen_mail.insert(maarse_core::mail::mail_id(&person, payload)) {
            return Some(Ok(()));
        }
        if self.persons.seen_mail.len() > 4096 {
            self.persons.seen_mail.clear();
        }
        if current != Some(chain) {
            // The sender has an old inbox key: send it the new one.
            self.persons.introduced.remove(&from);
            self.introduce(from);
        }
        self.on_content(from, &out[..n], hops, maarse_crypto::ratchet::Format::Static);
        Some(Ok(()))
    }

    // ---------- sibling sync ----------

    /// We sent `msg` to `peer`: a copy for our other devices. The user is
    /// on this device.
    pub(super) fn person_sent(&mut self, peer: Address, msg: &[u8]) {
        self.persons.my_active = Some(Instant::now());
        self.sibling_copy(op::SIBLING_SENT, peer, msg);
    }

    /// We received `msg` from `peer` (a person or a device) through the
    /// contact's device `from`: a copy for our other devices, and a hint to
    /// write to the sibling the user is on.
    pub(super) fn person_received(&mut self, peer: Address, from: Address, msg: &[u8]) {
        self.sibling_copy(op::SIBLING_RECEIVED, peer, msg);
        self.redirect_hint(from);
    }

    /// The user sent from a sibling within the last hour, and later than
    /// from here: tell the contact's device to write there next time, so
    /// the message does not cross the network twice.
    fn redirect_hint(&mut self, contact_device: Address) {
        if self.persons.me.is_none() || self.persons.hinted.get(&contact_device).is_some_and(|t| t.elapsed() < Duration::from_secs(600)) {
            return;
        }
        let mine = self.persons.my_active;
        let Some((s, _)) = self
            .persons
            .sibling_active
            .iter()
            .filter(|(_, t)| t.elapsed() < Duration::from_secs(3600) && mine.is_none_or(|m| **t > m))
            .max_by_key(|(_, t)| **t)
            .map(|(a, t)| (*a, *t))
        else {
            return;
        };
        self.persons.hinted.insert(contact_device, Instant::now());
        let mut c = vec![content::PERSON, op::REDIRECT];
        c.extend_from_slice(&s.0);
        let _ = self.send_sealed(contact_device, &c, Pending::Control);
        self.log(format!("told {} to write to our device {} next time", short(&contact_device), short(&s)));
    }

    /// Sync now: our person's mail from the replicas, and copies of what the
    /// siblings have after the last sync.
    pub(super) fn person_sync(&mut self) {
        self.persons.last_fetch = None;
        let since = if self.persons.sync_since > 0 { self.persons.sync_since } else { unix_now().saturating_sub(7 * 86_400) };
        let siblings: Vec<Address> = self.persons.me.as_ref().map(|m| m.siblings(&self.addr).collect()).unwrap_or_default();
        for s in siblings {
            let mut c = vec![content::PERSON, op::SYNC_REQUEST];
            c.extend_from_slice(&since.to_le_bytes());
            let _ = self.send_sealed(s, &c, Pending::Control);
        }
    }

    /// A sibling asks for copies after `since`: at most 6, then where to go on.
    fn on_sync_request(&mut self, from: Address, since: u32) {
        const BATCH: usize = 6;
        let items: Vec<(u8, Address, u32, Vec<u8>)> = self
            .hist
            .store
            .as_ref()
            .map(|h| {
                h.since(since, BATCH)
                    .into_iter()
                    .map(|i| (if i.outgoing { op::SIBLING_OLD_SENT } else { op::SIBLING_OLD_RECEIVED }, i.conv, i.ts, i.content.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let next = items.last().map_or(since, |i| i.2);
        let more = items.len() == BATCH;
        for (kind, peer, ts, msg) in &items {
            self.sibling_copy_to(from, *kind, *peer, *ts, msg);
        }
        let mut c = vec![content::PERSON, op::SYNC_DONE];
        c.extend_from_slice(&next.to_le_bytes());
        c.push(more as u8);
        let _ = self.send_sealed(from, &c, Pending::Control);
        if !items.is_empty() {
            self.log(format!("sync: {} copies to our device {}", items.len(), short(&from)));
        }
    }

    fn sibling_copy(&mut self, kind: u8, peer: Address, msg: &[u8]) {
        let Some(m) = &self.persons.me else { return };
        let siblings: Vec<Address> = m.siblings(&self.addr).collect();
        if siblings.is_empty() {
            return;
        }
        let id = OsRng.next_u32();
        for s in siblings {
            self.sibling_copy_to(s, kind, peer, id, msg);
        }
    }

    /// One copy, in pieces if needed. For history copies `id` is the time.
    fn sibling_copy_to(&mut self, sibling: Address, kind: u8, peer: Address, id: u32, msg: &[u8]) {
        let pieces: Vec<&[u8]> = if msg.is_empty() { vec![&[][..]] } else { msg.chunks(PIECE).collect() };
        let count = pieces.len().min(15) as u8;
        for (i, piece) in pieces.iter().take(15).enumerate() {
            let mut c = vec![content::PERSON, op::SIBLING, kind];
            c.extend_from_slice(&peer.0);
            c.extend_from_slice(&id.to_le_bytes());
            c.push(((i as u8) << 4) | count);
            c.extend_from_slice(piece);
            // As a message: if the sibling is away it waits in a post office.
            let _ = self.send_sealed(sibling, &c, Pending::Message);
        }
    }

    // ---------- content from other devices ----------

    pub(super) fn on_person(&mut self, from: Address, o: u8, data: &[u8]) {
        match o {
            op::CERT => match DeviceCert::verify(&from, data) {
                Ok(c) => {
                    let new = !self.persons.certs.contains_key(&from);
                    let person = c.person();
                    self.persons.claims.entry(from).or_insert((person, Instant::now()));
                    self.persons.remember_cert(c);
                    if new {
                        self.log(format!("{} is a device of {}", short(&from), short(&person)));
                    }
                    if let Some(ip) = self.persons.early_inbox.remove(&from) {
                        self.note_inbox(person, ip);
                    }
                    self.person_flush();
                    if self.mailbox.get(&person).is_some_and(|m| !m.is_empty()) {
                        self.flush_person_mail(person, Some(from));
                    }
                }
                Err(e) => self.log(format!("certificate from {} refused: {e:?}", short(&from))),
            },
            op::CERT_REQUEST => {
                self.persons.introduced.remove(&from);
                self.introduce(from);
            }
            op::INBOX_PUB => {
                let Some(ip) = InboxPublic::from_bytes(data) else { return };
                match self.persons.person_of(&from) {
                    Some(person) => self.note_inbox(person, ip),
                    None => {
                        self.persons.early_inbox.insert(from, ip);
                    }
                }
            }
            op::INBOX_STATE => self.on_inbox_state(from, data),
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
            op::SYNC_REQUEST if data.len() >= 4 => {
                if self.persons.me.as_ref().is_some_and(|m| m.list.active().any(|a| a == from) && from != self.addr) {
                    self.on_sync_request(from, u32::from_le_bytes([data[0], data[1], data[2], data[3]]));
                }
            }
            op::SYNC_DONE if data.len() >= 5 => {
                if !self.persons.me.as_ref().is_some_and(|m| m.list.active().any(|a| a == from)) {
                    return;
                }
                let next = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
                if next > self.persons.sync_since {
                    self.persons.sync_since = next;
                    let _ = fs::write(self.persons.dir.join("person-sync.txt"), next.to_string());
                }
                if data[4] != 0 {
                    let mut c = vec![content::PERSON, op::SYNC_REQUEST];
                    c.extend_from_slice(&next.to_le_bytes());
                    let _ = self.send_sealed(from, &c, Pending::Control);
                }
            }
            op::REDIRECT if data.len() >= 16 => {
                // From a device of a person: its user is on another device.
                let (Some(person), Some(s)) = (self.persons.person_of(&from), Address::from_slice(&data[..16])) else { return };
                if s == from {
                    return;
                }
                self.persons.claims.insert(s, (person, Instant::now()));
                if self.persons.person_of(&s) != Some(person) {
                    self.ask_cert(s);
                }
                self.log(format!("{} says: write to {} for {} next time", short(&from), short(&s), short(&person)));
            }
            _ => {}
        }
    }

    fn note_inbox(&mut self, person: Address, ip: InboxPublic) {
        let newer = self.persons.inboxes.get(&person).is_none_or(|o| o.chain != ip.chain || ip.week >= o.week);
        if newer {
            self.persons.inboxes.insert(person, ip);
        }
    }

    /// The inbox chain from a sibling (when linked, or after a removal).
    fn on_inbox_state(&mut self, from: Address, data: &[u8]) {
        let Some(c) = InboxChain::from_bytes(data) else { return };
        // While linking, the list may come after the chain.
        if self.persons.me.is_none() {
            if self.persons.granted.is_some() {
                self.persons.granted_inbox = Some(c);
            }
            return;
        }
        let me = self.addr;
        let Some(m) = self.persons.me.as_mut() else { return };
        if from == me || !m.list.active().any(|a| a == from) {
            return;
        }
        if m.inbox.as_ref().is_some_and(|o| o.chain == c.chain) {
            return;
        }
        m.old_inbox = m.inbox.take();
        m.inbox = Some(c);
        self.persons.save_member();
        self.persons.introduced.clear();
        self.log(format!("new inbox key from our device {}", short(&from)));
    }

    fn on_list(&mut self, from: Address, data: &[u8]) {
        // Our own person's list, or the one we are joining; or a contact's
        // list (it tells us which of their devices are removed).
        // The signature decides, not who brought it: our own person's key
        // first (a new master device after a restore is not known yet).
        let own = self.persons.me.as_ref().map(|m| m.cert.master).or(self.persons.granted.map(|g| g.master));
        let contact = self.persons.certs.get(&from).map(|c| c.master);
        let Some((master, list)) = [own, contact].into_iter().flatten().find_map(|k| DeviceList::verify(&k, data).ok().map(|l| (k, l))) else {
            return self.log(format!("device list from {} refused (bad signature)", short(&from)));
        };
        let person = master.address();
        if self.persons.me.is_none() && let Some(g) = self.persons.granted.take() {
            if !list.active().any(|d| d == self.addr) {
                return self.log("the device list does not hold this device");
            }
            self.persons.awaiting = None;
            let inbox = self.persons.granted_inbox.take();
            self.persons.me = Some(Member { master: None, cert: g, list: list.clone(), inbox, old_inbox: None });
            self.persons.lists.insert(person, list);
            self.persons.save_member();
            self.log(format!("linked to {} as device {}", short(&person), g.index));
            let words = link_words(&self.identity.public()).join(" ");
            self.broadcast(&Event::DeviceLinked { device: self.addr, index: g.index, words: heapless::Vec::from_slice(words.as_bytes()).unwrap_or_default() });
            self.person_changed();
            self.announce();
            return;
        }
        if !self.is_own_person(&person) {
            // A contact's list: keep the newest.
            if self.persons.lists.get(&person).is_none_or(|l| list.version > l.version) {
                self.persons.lists.insert(person, list);
            }
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
        if kind == op::SIBLING_SENT {
            self.persons.sibling_active.insert(from, Instant::now());
        }
        if count == 1 {
            return self.sibling_done(from, kind, peer, id, piece);
        }
        let entry = self.persons.pieces.entry((from, id)).or_insert_with(|| (kind, count, peer, vec![None; count as usize]));
        if let Some(slot) = entry.3.get_mut(i as usize) {
            *slot = Some(piece);
        }
        if entry.3.iter().any(Option::is_none) {
            return;
        }
        let (kind, _, peer, parts) = self.persons.pieces.remove(&(from, id)).unwrap_or_default();
        let msg: Vec<u8> = parts.into_iter().flatten().flatten().collect();
        self.sibling_done(from, kind, peer, id, msg);
    }

    fn sibling_done(&mut self, from: Address, kind: u8, peer: Address, id: u32, msg: Vec<u8>) {
        match kind {
            op::SIBLING_RECEIVED => {
                self.log(format!("copy from our device {}: message from {}", short(&from), short(&peer)));
                self.broadcast(&Event::DirectReceived { from: peer, hops: 0, text: text(&msg) });
            }
            op::SIBLING_SENT => {
                self.log(format!("copy from our device {}: message to {}", short(&from), short(&peer)));
                self.hist_sent(0, crate::history::KIND_DIRECT, peer, 0, &msg);
            }
            // From a sync: into the history at its own time, without a
            // notification, unless we have it already.
            op::SIBLING_OLD_RECEIVED | op::SIBLING_OLD_SENT => {
                self.hist_import(peer, kind == op::SIBLING_OLD_SENT, id, &msg);
            }
            _ => {}
        }
    }
}
