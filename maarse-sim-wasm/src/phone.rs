//! Simulated phones: the real web client talks the companion protocol
//! (`maarse_core::companion`) to a node in the simulated world.
//!
//! The page sends the client's command frames with `phone_cmd` and fetches
//! the node's event frames with `phone_events`. Frames are exactly what the
//! client sends to and gets from a real node or maarse-host (length-prefixed,
//! encoded and decoded by the shared `companion` code).
//!
//! A node gets a phone when a command arrives for it ("attached"); from then
//! on the engine queues events for it: messages delivered to the node, acks,
//! routes, announces heard, channel posts, files (also voice and pictures),
//! pages and call data (see transfer.rs). Everything the simulator cannot do
//! yet is answered with an `Error` event that says so.

use super::*;
use maarse_core::companion::{Command, Event, FrameDecoder, PROTOCOL_VERSION, Stats, error_code, neighbor_events, stat, text};
use std::collections::VecDeque;

/// At most this many unread event frames per phone; older ones are dropped.
const MAX_QUEUED: usize = 2000;
/// Channel posts from phones carry this prefix and the sender's node index.
const PHONE_POST: &[u8] = b"PHC";

#[derive(Default)]
pub struct Phone {
    events: VecDeque<Vec<u8>>,
    decoder: FrameDecoder,
    pub(crate) name: String,
    /// Channels the phone joined: (address, name).
    channels: Vec<(Address, String)>,
    dropped: u64,
    /// The Bluetooth link to the radio is down until then (ms).
    pub(crate) link_down_until: u64,
    /// While the link was down: events and messages the phone never got
    /// (the firmware throws old frames away when a phone connects).
    pub(crate) lost_events: u64,
    pub(crate) lost_msgs: u64,
    pub(crate) link_drops: u64,
    /// Waiting to be linked by a master device until then (ms), 0 = no.
    awaiting_until: u64,
}

/// A person made on a phone (linked devices, maarse-host `node/person.rs`).
pub struct PersonRec {
    /// The person's address, derived from its 24 words (maarse_crypto::recovery).
    pub addr: Address,
    pub master: usize,
    /// (device node, revoked), in the order they were linked.
    pub devices: Vec<(usize, bool)>,
    pub version: u32,
}

#[derive(Default)]
pub struct Phones {
    pub attached: HashMap<usize, Phone>,
    /// Counter appended to phone messages so equal texts get distinct packet ids.
    counter: u64,
    /// Persons made on phones.
    pub persons: Vec<PersonRec>,
    /// Link grants on their way: (message, person, device).
    pending_links: Vec<(usize, usize, usize)>,
}

fn txt(s: &str) -> maarse_core::companion::Text {
    text(s.as_bytes())
}

impl Engine {
    pub(crate) fn phone_emit(&mut self, node: usize, ev: &Event) {
        let now = self.now;
        let host = self.at_host(node);
        let Some(p) = self.phones.attached.get_mut(&node) else { return };
        // Link to the radio down: the firmware queues frames for Bluetooth
        // but empties the queue when a phone connects (firmware ble.rs:
        // "Old frames from before the connection are thrown away"), so
        // nothing that happens meanwhile reaches the phone.
        if now < p.link_down_until && !host {
            p.lost_events += 1;
            if matches!(ev, Event::DirectReceived { .. } | Event::ChannelReceived { .. }) {
                p.lost_msgs += 1;
                self.stats.phone_lost_msgs += 1;
            }
            return;
        }
        if p.events.len() >= MAX_QUEUED {
            p.events.pop_front();
            p.dropped += 1;
        }
        p.events.push_back(ev.encode().to_vec());
    }

    fn phone_err(&mut self, node: usize, req: u16, code: u8, msg: &str) {
        self.phone_emit(node, &Event::Error { req, code, text: txt(msg) });
    }

    /// `phone_cmd {node, frame}`: bytes from the client (one or more frames,
    /// possibly partial). Returns how many commands were handled.
    /// The phone on `node` loses its Bluetooth link to the radio for `ms`.
    /// Phones that run the host themselves (app nodes) have no such link.
    pub fn phone_link_drop(&mut self, node: usize, ms: u64) {
        let now = self.now;
        let Some(p) = self.phones.attached.get_mut(&node) else { return };
        p.link_down_until = now + ms;
        p.link_drops += 1;
        self.stats.phone_link_drops += 1;
        self.tr(format!("{{\"t\":{now},\"ev\":\"event\",\"what\":\"phone_link_drop\",\"n\":{node},\"ms\":{ms}}}"));
        self.schedule_phone_reconnect(node, now + ms);
    }

    /// Back on the link: tell the phone what it missed (the real firmware
    /// does not; this line is the simulator's note).
    pub(crate) fn phone_reconnect(&mut self, node: usize) {
        let Some(p) = self.phones.attached.get_mut(&node) else { return };
        if self.now < p.link_down_until {
            return;
        }
        let (ev, msgs) = (p.lost_events, p.lost_msgs);
        p.lost_events = 0;
        p.lost_msgs = 0;
        let note = format!("Simulator: Bluetooth link to the radio is back. While it was down {ev} events were thrown away, {msgs} of them messages (the firmware drops frames from before a connection; senders may already show them as delivered).");
        self.phone_emit(node, &Event::Log { text: txt(&note) });
    }

    pub fn phone_cmd(&mut self, node: usize, bytes: &[u8]) -> Value {
        // No link to the radio: what the phone sends never arrives.
        if self.phones.attached.get(&node).is_some_and(|p| self.now < p.link_down_until) && !self.at_host(node) {
            return json!({"handled": 0, "link": false});
        }
        let label = if self.nodes[node].label.is_empty() { format!("Node {node}") } else { self.nodes[node].label.clone() };
        let p = self.phones.attached.entry(node).or_default();
        let fresh = p.name.is_empty();
        if fresh {
            p.name = label;
        }
        if fresh {
            // The public channel is always there, as on a real device.
            let ch = self.public_channel;
            p.channels.push((ch, "public".into()));
            let _ = self.nodes[node].router.subscribe(ch);
            // A phone that starts announces itself, like a device that boots.
            if self.nodes[node].alive() {
                self.announce(node);
            }
        }
        if let Some(p) = self.phones.attached.get_mut(&node) {
            p.decoder.push(bytes);
        }
        let mut frames = Vec::new();
        while let Some(f) = self.phones.attached.get_mut(&node).and_then(|p| p.decoder.next_frame()) {
            frames.push(f);
        }
        let n = frames.len();
        for f in frames {
            match Command::decode(&f) {
                Ok(c) => self.phone_command(node, c),
                Err(maarse_core::companion::FrameError::UnknownType(t)) => {
                    let msg = format!("Not simulated yet: maarse-host command 0x{t:02x} (the phone talks to a simulated node)");
                    self.phone_err(node, 0, error_code::BAD_FRAME, &msg)
                }
                Err(e) => self.phone_err(node, 0, error_code::BAD_FRAME, &format!("Simulator: invalid frame ({e:?})")),
            }
        }
        json!({"ok": true, "commands": n})
    }

    /// `phone_events {node}`: all queued event frames (each length-prefixed).
    pub fn phone_events(&mut self, node: usize) -> Value {
        let Some(p) = self.phones.attached.get_mut(&node) else { return json!({"frames": [], "attached": false}) };
        let frames: Vec<Vec<u8>> = p.events.drain(..).collect();
        let link = self.now >= p.link_down_until;
        json!({"frames": frames, "attached": true, "dropped": p.dropped, "alive": self.nodes[node].alive(), "link": link, "link_drops": p.link_drops})
    }

    pub fn phone_detach(&mut self, node: usize) {
        self.phones.attached.remove(&node);
    }

    fn channel_for(&self, name: &str) -> (Address, String) {
        let (lower, _region) = maarse_core::region::split_channel(name);
        match lower {
            "" | "public" | "#public" => (self.public_channel, "public".into()),
            n => {
                let mut s = String::new();
                if !n.starts_with('#') {
                    s.push('#');
                }
                s.push_str(&n.to_lowercase());
                (maarse_crypto::channel_address(&maarse_crypto::hashtag_key(&s)), s)
            }
        }
    }

    fn phone_command(&mut self, node: usize, c: Command) {
        let not_yet = |what: &str| format!("Not simulated yet: {what} (Denmark simulator)");
        match c {
            Command::GetInfo => {
                let max_file = if self.nodes[node].kind == Kind::Host || !self.nodes[node].radio { self.sc.rules.max_file } else { maarse_hostrules::filebox::LORA_MAX_FILE };
                let rt = &self.nodes[node].router;
                let info = Event::Info {
                    addr: rt.addr,
                    repeater: rt.role.forwards(),
                    version: PROTOCOL_VERSION,
                    max_text: (maarse_core::companion::MAX_TEXT - 1) as u8,
                    max_file,
                    identity: None,
                };
                self.phone_emit(node, &info);
                let chans: Vec<(Address, String)> = self.phones.attached[&node].channels.clone();
                for (ch, name) in chans {
                    self.phone_emit(node, &Event::ChannelJoined { channel: ch, name: txt(&name) });
                }
                // Contacts: the other phones' nodes this node knows.
                let others: Vec<usize> = self.phones.attached.keys().copied().filter(|&o| o != node).collect();
                for o in others {
                    if self.knows(node, o) {
                        let addr = self.nodes[o].router.addr;
                        self.phone_emit(node, &Event::Heard { addr, hops: 0, identity: None });
                    }
                }
                if !self.nodes[node].alive() {
                    self.phone_emit(node, &Event::Log { text: txt("Simulator: this node is down (power or failure)") });
                }
            }
            Command::GetName => {
                let name = self.phones.attached[&node].name.clone();
                self.phone_emit(node, &Event::Name { name: txt(&name) });
            }
            Command::SetName { name } => {
                let s = String::from_utf8_lossy(&name).into_owned();
                if let Some(p) = self.phones.attached.get_mut(&node) {
                    p.name = s.clone();
                }
                self.phone_emit(node, &Event::Name { name: txt(&s) });
            }
            Command::SetTime { .. } => {}
            Command::AnnounceNow => {
                self.announce(node);
                self.phone_emit(node, &Event::Log { text: txt("Announce sent") });
            }
            Command::JoinChannel { name } => {
                let (ch, label) = self.channel_for(&String::from_utf8_lossy(&name));
                let _ = self.nodes[node].router.subscribe(ch);
                if let Some(p) = self.phones.attached.get_mut(&node)
                    && !p.channels.iter().any(|(a, _)| *a == ch)
                {
                    p.channels.push((ch, label.clone()));
                }
                self.phone_emit(node, &Event::ChannelJoined { channel: ch, name: txt(&label) });
            }
            Command::SendDirect { req, dest, text: t } => {
                // An account address: send to one of the person's devices (the
                // engine routes to the best one and the others get copies).
                let dest = self.person_device_addr(&dest).unwrap_or(dest);
                let Some(&to) = self.addr_idx.get(&dest) else {
                    return self.phone_err(node, req, error_code::UNKNOWN_DEST, "Simulator: no node with this address in this world");
                };
                if !self.nodes[node].alive() {
                    return self.phone_err(node, req, error_code::QUEUE_FULL, "Simulator: this node is down; nothing was sent");
                }
                // Text, a zero byte and a counter keep packet ids unique.
                self.phones.counter += 1;
                let mut payload = t.to_vec();
                payload.push(0);
                payload.extend_from_slice(&self.phones.counter.to_be_bytes());
                let m = self.send_payload(node, to, payload);
                let msg_id = self.msgs[m].id;
                self.phone_emit(node, &Event::Sent { req, msg_id });
                if self.msgs[m].unsent {
                    self.phone_err(node, req, error_code::UNKNOWN_DEST, "Simulator: the recipient's keys are unknown to this node");
                }
            }
            Command::SendChannel { req, channel, text: t, .. } => {
                if !self.nodes[node].alive() {
                    return self.phone_err(node, req, error_code::QUEUE_FULL, "Simulator: this node is down; nothing was sent");
                }
                let mut payload = PHONE_POST.to_vec();
                payload.extend_from_slice(&(node as u32).to_be_bytes());
                payload.extend_from_slice(&t);
                match self.nodes[node].router.send_channel(channel, &payload) {
                    Ok(actions) => {
                        let msg_id = packet_id(PacketType::Channel, &channel, &payload);
                        self.apply(node, actions.into_iter().collect(), None);
                        self.phone_emit(node, &Event::Sent { req, msg_id });
                    }
                    Err(e) => self.phone_err(node, req, error_code::NOT_IN_CHANNEL, &format!("Simulator: channel send failed ({e:?})")),
                }
            }
            Command::GetStats => {
                let rt = &self.nodes[node].router;
                let mut items = Stats::new();
                let routes = rt.routes().len() as i32;
                let _ = items.push((stat::UPTIME_S, (self.now / 1000) as i32));
                let _ = items.push((stat::KNOWN_NODES, self.heard_from[node].len() as i32));
                let _ = items.push((stat::ROUTES, routes));
                let _ = items.push((stat::CHANNELS, self.phones.attached[&node].channels.len() as i32));
                let _ = items.push((stat::LINKS, self.nodes[node].links.iter().filter(|&&k| self.link_ok(k)).count() as i32));
                let _ = items.push((stat::CLIENTS, 1));
                let _ = items.push((stat::ROLE, rt.role.as_u8() as i32));
                self.phone_emit(node, &Event::Stats { items });
            }
            Command::GetNeighbors => {
                for ev in neighbor_events(&self.nodes[node].router.neighbors, self.now) {
                    self.phone_emit(node, &ev);
                }
            }
            Command::GetSharing | Command::GetRadio | Command::GetAdmins | Command::GetRegions | Command::GetWifi | Command::GetRangeTest | Command::OtaStatus => {
                self.phone_err(node, 0, error_code::BAD_FRAME, &not_yet("node settings (radio, sharing, admins, regions, Wi-Fi, range test, updates)"));
            }
            Command::TraceRoute { req, .. } => self.phone_err(node, req, error_code::BAD_FRAME, &not_yet("trace route")),
            Command::FileBegin { req, dest, size, mime, name } => {
                let Some(&to) = self.addr_idx.get(&dest) else {
                    return self.phone_err(node, req, error_code::UNKNOWN_DEST, "Simulator: no node with this address in this world");
                };
                let (mime, name) = (String::from_utf8_lossy(&mime).into_owned(), String::from_utf8_lossy(&name).into_owned());
                if let Err((code, m)) = self.xf_file_begin(node, req, to, size, mime, name) {
                    self.phone_err(node, req, code, &m);
                }
            }
            Command::FileData { req, offset, data } => self.xf_file_data(node, req, offset, &data),
            Command::FileEnd { req } => match self.xf_file_end(node, req) {
                Ok((file_id, chunks)) => self.phone_emit(node, &Event::FileStarted { req, file_id, chunks }),
                Err((code, m)) => self.phone_err(node, req, code, &m),
            },
            Command::GetFileLimit { dest } => {
                let Some(&to) = self.addr_idx.get(&dest) else {
                    return self.phone_err(node, 0, error_code::UNKNOWN_DEST, "Simulator: no node with this address in this world");
                };
                let l = self.xf_limit(node, to);
                self.phone_emit(node, &Event::FileLimit { dest, max_bytes: l.max_bytes, via_lora: l.via_lora, mailbox: l.mailbox, est_airtime_s: l.est_airtime_s });
            }
            Command::CancelFile { file_id } => {
                if let Some(k) = self.xf_by_file_id(file_id) {
                    self.xf_cancel(k, node);
                }
            }
            Command::SendCall { dest, data } => {
                let Some(&to) = self.addr_idx.get(&dest) else {
                    return self.phone_err(node, 0, error_code::UNKNOWN_DEST, "Simulator: no node with this address in this world");
                };
                if let Some((code, why)) = self.xf_phone_call(node, to, &data) {
                    self.phone_err(node, 0, code, why);
                }
            }
            Command::RequestPage { req, dest, path } => {
                let Some(&host) = self.addr_idx.get(&dest) else {
                    return self.phone_err(node, req, error_code::UNKNOWN_DEST, "Simulator: no node with this address in this world");
                };
                if host == node {
                    return self.phone_err(node, req, error_code::BAD_FRAME, &not_yet("this node's own pages (ask another host)"));
                }
                match self.xf_page(node, host, &String::from_utf8_lossy(&path), None) {
                    Ok(k) => {
                        let msg_id = k as u64 | 0x5000_0000_0000_0000;
                        self.phone_emit(node, &Event::Sent { req, msg_id });
                    }
                    Err((code, m)) => self.phone_err(node, req, code, &m),
                }
            }
            Command::PostBoard { req, dest, path, text: note } => {
                let Some(&host) = self.addr_idx.get(&dest) else {
                    return self.phone_err(node, req, error_code::UNKNOWN_DEST, "Simulator: no node with this address in this world");
                };
                match self.xf_board(node, host, &String::from_utf8_lossy(&path), &String::from_utf8_lossy(&note)) {
                    Ok(k) => {
                        let msg_id = k as u64 | 0x5000_0000_0000_0000;
                        self.phone_emit(node, &Event::Sent { req, msg_id });
                    }
                    Err((code, m)) => self.phone_err(node, req, code, &m),
                }
            }
            Command::Remote { req, .. } => self.phone_err(node, req, error_code::BAD_FRAME, &not_yet("remote admin")),
            Command::SendAlert { req, .. } => self.phone_err(node, req, error_code::BAD_FRAME, &not_yet("alerts from a phone (use the Alert tool on the map)")),
            Command::GetPerson => {
                let info = self.person_info(node);
                self.phone_emit(node, &Event::Person(info));
            }
            Command::PersonSetup { req, op, text: t } => self.person_setup(node, req, op, &t),
            Command::LinkDevice { req, device } => self.link_device(node, req, device),
            Command::JoinPrivateChannel { .. } => self.phone_err(node, 0, error_code::BAD_FRAME, &not_yet("private channels")),
            other => self.phone_err(node, 0, error_code::BAD_FRAME, &not_yet(&format!("command {:?}", core::mem::discriminant(&other)))),
        }
    }

    // ---------- hooks called by the engine ----------

    /// A packet was delivered to `node` (called from `on_deliver`).
    pub(crate) fn phone_on_deliver(&mut self, node: usize, ptype: PacketType, dest: Address, payload: &[u8], hops: u8) {
        if !self.phones.attached.contains_key(&node) {
            return;
        }
        match ptype {
            PacketType::Announce => {
                if let Some(&who) = self.addr_idx.get(&dest)
                    && who != node
                    && self.nodes[who].kind.is_user()
                {
                    self.phone_emit(node, &Event::Heard { addr: dest, hops, identity: None });
                    // The announce's app data (name, capabilities such as
                    // pages), as the firmware passes it on: keys (64) |
                    // timestamp (4) | app data length | app data | ...
                    if let Some(&len) = payload.get(68)
                        && let Some(raw) = payload.get(69..69 + len as usize)
                    {
                        let a = maarse_core::appdata::parse(raw);
                        if a.name.is_some() || a.caps.is_some() || a.position.is_some() {
                            self.phone_emit(node, &Event::NodeInfo { addr: dest, hops, position: a.position, telemetry: a.telemetry, name: a.name, caps: a.caps });
                        }
                    }
                }
            }
            PacketType::Channel if payload.starts_with(PHONE_POST) && payload.len() >= 7 => {
                let from = u32::from_be_bytes([payload[3], payload[4], payload[5], payload[6]]) as usize;
                if from == node || from >= self.nodes.len() {
                    return;
                }
                let from = self.nodes[from].router.addr;
                self.phone_emit(node, &Event::ChannelReceived { channel: dest, from, hops, text: text(&payload[7..]) });
            }
            _ => {}
        }
    }

    /// Message `m` reached `node` (called where the engine marks it delivered).
    pub(crate) fn phone_on_message(&mut self, m: usize, node: usize, hops: u8) {
        if self.msgs[m].payload.starts_with(b"PLINK") && self.link_arrived(m, node) {
            return;
        }
        if !self.phones.attached.contains_key(&node) {
            return;
        }
        // A copy from a sibling device carries the original message.
        let m = self.msgs[m].sync_of.unwrap_or(m);
        let p = &self.msgs[m].payload;
        let body = match p.iter().position(|&b| b == 0) {
            Some(k) => &p[..k],
            None => {
                let end = p.iter().rposition(|&b| b != b'.').map_or(0, |k| k + 1);
                &p[..end]
            }
        };
        let sender = self.msgs[m].from;
        // From a device of a person: shown as the person (maarse-host person_peer).
        let from = self.person_of_node(sender).map_or(self.nodes[sender].router.addr, |k| self.phones.persons[k].addr);
        self.phone_emit(node, &Event::DirectReceived { from, hops, text: text(body) });
    }

    /// The person (index in `phones.persons`) `node` is a live device of.
    fn person_of_node(&self, node: usize) -> Option<usize> {
        self.phones.persons.iter().position(|p| p.devices.iter().any(|&(d, rev)| d == node && !rev))
    }

    /// A device address for an account address (the master, or the first
    /// device that is alive).
    fn person_device_addr(&self, a: &Address) -> Option<Address> {
        let p = self.phones.persons.iter().find(|p| p.addr == *a)?;
        let live: Vec<usize> = p.devices.iter().filter(|(_, r)| !r).map(|(d, _)| *d).collect();
        let d = live.iter().copied().find(|&d| self.nodes[d].alive()).or(live.first().copied())?;
        Some(self.nodes[d].router.addr)
    }

    /// The words both devices show while linking (maarse_crypto::person::
    /// link_words takes the device's public key; the simulator's nodes have
    /// no full identity, so the same word list is used on the address).
    fn link_words(&self, node: usize) -> String {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(b"maarse/link/v1");
        h.update(self.nodes[node].router.addr.0);
        let d = h.finalize();
        (0..4).map(|i| maarse_crypto::recovery::word(u16::from_be_bytes([d[2 * i], d[2 * i + 1]]) & 0x7ff)).collect::<Vec<_>>().join(" ")
    }

    fn person_info(&self, node: usize) -> maarse_core::companion::PersonInfo {
        use maarse_core::companion::{PersonInfo, person_state};
        let mut i = PersonInfo::default();
        if let Some(k) = self.person_of_node(node) {
            let p = &self.phones.persons[k];
            i.state = person_state::MEMBER;
            i.master = p.master == node;
            i.person = p.addr;
            i.index = p.devices.iter().position(|&(d, _)| d == node).unwrap_or(0) as u8;
            i.version = p.version;
            for &(d, rev) in &p.devices {
                let _ = i.devices.push((self.nodes[d].router.addr, rev));
            }
        } else if self.phones.attached.get(&node).is_some_and(|p| self.now < p.awaiting_until) {
            i.state = person_state::AWAITING_LINK;
            i.words = heapless::Vec::from_slice(self.link_words(node).as_bytes()).unwrap_or_default();
        }
        i
    }

    /// Puts the person's live devices into the engine's device groups
    /// (master first: the person's mailbox key), replacing its old group.
    fn person_groups(&mut self, k: usize) {
        let p = &self.phones.persons[k];
        let all: Vec<usize> = p.devices.iter().map(|(d, _)| *d).collect();
        let mut live: Vec<usize> = p.devices.iter().filter(|(_, r)| !r).map(|(d, _)| *d).collect();
        if let Some(pos) = live.iter().position(|&d| d == p.master) {
            live.swap(0, pos);
        }
        self.sc.device_groups.retain(|g| !g.iter().any(|d| all.contains(d)));
        if live.len() >= 2 {
            self.sc.device_groups.push(live);
            // maarse-host always copies the person's messages to its other devices.
            self.sc.sibling_sync = true;
        }
    }

    /// Every live device of person `k` hears what changed.
    fn person_changed(&mut self, k: usize) {
        self.phones.persons[k].version += 1;
        self.person_groups(k);
        let devs: Vec<usize> = self.phones.persons[k].devices.iter().map(|(d, _)| *d).collect();
        for d in devs {
            let info = self.person_info(d);
            self.phone_emit(d, &Event::Person(info));
        }
    }

    /// maarse-host `person_setup` on a phone.
    fn person_setup(&mut self, node: usize, req: u16, op: u8, t: &[u8]) {
        use maarse_core::companion::person_op;
        let fail = |e: &mut Self, m: &str| e.phone_err(node, req, error_code::BAD_FRAME, m);
        match op {
            person_op::CREATE | person_op::RESTORE => {
                if self.person_of_node(node).is_some() {
                    return fail(self, "This device already belongs to a person (leave it first)");
                }
                let (phrase, words) = if op == person_op::CREATE {
                    let mut ent = [0u8; maarse_crypto::recovery::ENTROPY_LEN];
                    self.rng.fill(&mut ent[..]);
                    let ph = maarse_crypto::recovery::entropy_to_phrase(&ent).to_string();
                    (Some(ph.clone()), ph)
                } else {
                    (None, String::from_utf8_lossy(t).split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase())
                };
                let master = match maarse_crypto::recovery::identity_from_phrase(&words, "") {
                    Ok(m) => m.address(),
                    Err(e) => return fail(self, &format!("The phrase is not valid: {e:?}")),
                };
                // Restoring a person that already exists here: this device
                // becomes its master and the old devices must be linked again.
                if let Some(k) = self.phones.persons.iter().position(|p| p.addr == master) {
                    self.phones.persons[k].master = node;
                    self.phones.persons[k].devices = vec![(node, false)];
                    self.person_changed(k);
                } else {
                    self.phones.persons.push(PersonRec { addr: master, master: node, devices: vec![(node, false)], version: 0 });
                    let k = self.phones.persons.len() - 1;
                    self.person_changed(k);
                }
                if let Some(p) = self.phones.attached.get_mut(&node) {
                    p.awaiting_until = 0;
                }
                if let Some(ph) = phrase {
                    self.phone_emit(node, &Event::PersonPhrase { phrase: txt(&ph) });
                }
                self.announce(node);
            }
            person_op::AWAIT_LINK => {
                if self.person_of_node(node).is_some() {
                    return fail(self, "This device already belongs to a person (leave it first)");
                }
                let until = self.now + 600_000;
                if let Some(p) = self.phones.attached.get_mut(&node) {
                    p.awaiting_until = until;
                }
                let info = self.person_info(node);
                self.phone_emit(node, &Event::Person(info));
                // The master device needs our keys: tell everyone who we are.
                self.announce(node);
            }
            person_op::CANCEL => {
                if let Some(p) = self.phones.attached.get_mut(&node) {
                    p.awaiting_until = 0;
                }
                let info = self.person_info(node);
                self.phone_emit(node, &Event::Person(info));
            }
            person_op::LEAVE => {
                if let Some(k) = self.person_of_node(node) {
                    self.phones.persons[k].devices.retain(|&(d, _)| d != node);
                    self.sc.device_groups.retain(|g| !g.contains(&node));
                    if self.phones.persons[k].devices.is_empty() {
                        self.phones.persons.remove(k);
                    } else {
                        self.person_changed(k);
                    }
                }
                let info = self.person_info(node);
                self.phone_emit(node, &Event::Person(info));
                self.announce(node);
            }
            person_op::REMOVE => {
                let Some(k) = self.person_of_node(node) else { return fail(self, "This device does not belong to a person") };
                if self.phones.persons[k].master != node {
                    return fail(self, "Only the master device can remove devices");
                }
                let hex = String::from_utf8_lossy(t).trim().to_lowercase();
                let Some(d) = maarse_hostrules::pagelock::parse_hex(&hex).and_then(|a| self.addr_idx.get(&a).copied()) else {
                    return fail(self, "Give the device's address (32 hex characters)");
                };
                let Some(e) = self.phones.persons[k].devices.iter_mut().find(|(x, _)| *x == d) else {
                    return fail(self, "That device does not belong to this person");
                };
                e.1 = true;
                self.person_changed(k);
                let info = self.person_info(d);
                self.phone_emit(d, &Event::Person(info));
            }
            person_op::SYNC => {
                if self.person_of_node(node).is_none() {
                    return fail(self, "This device does not belong to a person");
                }
                self.person_sync(node);
                self.phone_emit(node, &Event::Log { text: txt("Syncing with the other devices and the post offices") });
            }
            _ => fail(self, "Not simulated yet: account backups (use the 24 words) (Denmark simulator)"),
        }
    }

    /// maarse-host `link_device`: the master device links a waiting device
    /// it has heard. The grant travels to the device as a sealed message.
    fn link_device(&mut self, node: usize, req: u16, device: Address) {
        let fail = |e: &mut Self, m: &str| e.phone_err(node, req, error_code::UNKNOWN_DEST, m);
        let Some(k) = self.person_of_node(node) else { return fail(self, "This device does not belong to a person") };
        if self.phones.persons[k].master != node {
            return fail(self, "Only the master device can link devices");
        }
        let Some(&d) = self.addr_idx.get(&device) else {
            return fail(self, "The new device has not been heard yet: choose \"Link to an account\" on it and keep it open");
        };
        if d == node || !self.knows(node, d) {
            return fail(self, "The new device has not been heard yet: choose \"Link to an account\" on it and keep it open");
        }
        if self.phones.persons[k].devices.iter().filter(|(x, _)| *x != d).count() >= 4 {
            return fail(self, "The person already has 4 devices (removed ones count too)");
        }
        // The grant goes over the network like any sealed message; the link
        // takes effect when it arrives (checked in phone_on_message).
        let m = self.send_payload(node, d, b"PLINK".to_vec());
        self.phones.pending_links.push((m, k, d));
        let id = self.msgs[m].id;
        self.phone_emit(node, &Event::Sent { req, msg_id: id });
    }

    /// A link grant reached the waiting device `d`.
    pub(crate) fn link_arrived(&mut self, m: usize, node: usize) -> bool {
        let Some(pos) = self.phones.pending_links.iter().position(|&(mm, _, d)| mm == m && d == node) else { return false };
        let (_, k, d) = self.phones.pending_links.remove(pos);
        let waiting = self.phones.attached.get(&d).is_some_and(|p| self.now < p.awaiting_until);
        if !waiting || k >= self.phones.persons.len() {
            return true;
        }
        if let Some(p) = self.phones.attached.get_mut(&d) {
            p.awaiting_until = 0;
        }
        let devs = &mut self.phones.persons[k].devices;
        devs.retain(|(x, _)| *x != d);
        devs.push((d, false));
        let index = (devs.len() - 1) as u8;
        self.person_changed(k);
        let words = self.link_words(d);
        let all: Vec<usize> = self.phones.persons[k].devices.iter().filter(|(_, r)| !r).map(|(x, _)| *x).collect();
        let dev_addr = self.nodes[d].router.addr;
        for x in all {
            self.phone_emit(x, &Event::DeviceLinked { device: dev_addr, index, words: heapless::Vec::from_slice(words.as_bytes()).unwrap_or_default() });
        }
        self.announce(d);
        true
    }

    /// The router of `node` got an ack for `msg_id`.
    pub(crate) fn phone_on_ack(&mut self, node: usize, msg_id: u64, carry: bool) {
        if self.phones.attached.contains_key(&node) {
            let ev = if carry { Event::CarryAcked { msg_id } } else { Event::Acked { msg_id } };
            self.phone_emit(node, &ev);
        }
    }

    pub(crate) fn phone_on_route(&mut self, node: usize, dest: Address, hops: u8) {
        if self.phones.attached.contains_key(&node) {
            self.phone_emit(node, &Event::RouteLearned { dest, hops });
        }
    }

    /// Message `m` from a phone's node was deposited at a post office.
    pub(crate) fn phone_on_deposit(&mut self, m: usize) {
        let (from, msg_id) = (self.msgs[m].from, self.msgs[m].id);
        if self.phones.attached.contains_key(&from) {
            self.phone_emit(from, &Event::Deposited { msg_id });
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::create;
    use maarse_core::companion::{Command, Event, text};
    use serde_json::{Value, json};

    fn frames(v: &Value) -> Vec<Event> {
        v["frames"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| {
                let b: Vec<u8> = f.as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u8).collect();
                Event::decode(&b[2..]).unwrap()
            })
            .collect()
    }

    /// Phone A (node 0) sends a direct message through a repeater to phone B
    /// (node 2); B gets DirectReceived with the text, A gets Sent and Acked.
    #[test]
    fn phone_send_direct_reaches_the_other_phone() {
        let sc = json!({"seed": 3, "duration_s": 600, "nodes": [
            {"kind": "handheld", "lat": 56.0, "lon": 9.0},
            {"kind": "repeater", "lat": 56.0, "lon": 9.05},
            {"kind": "handheld", "lat": 56.0, "lon": 9.1}
        ], "traffic": {"direct_per_min": 0, "channel_per_min": 0}});
        let mut e = create(&sc.to_string()).unwrap();
        e.run_until(20_000, usize::MAX);
        for n in [0, 2] {
            e.phone_cmd(n, &Command::GetInfo.encode());
        }
        let b = frames(&e.phone_events(2));
        let Some(Event::Info { addr: b_addr, .. }) = b.first().cloned() else { panic!("{b:?}") };
        frames(&e.phone_events(0));
        let cmd = Command::SendDirect { req: 7, dest: b_addr, text: text(b"hello from A") };
        e.phone_cmd(0, &cmd.encode());
        e.run_until(200_000, usize::MAX);
        let a = frames(&e.phone_events(0));
        let b = frames(&e.phone_events(2));
        let a_addr = e.nodes[0].router.addr;
        assert!(
            b.iter().any(|ev| matches!(ev, Event::DirectReceived { from, text: t, .. } if *from == a_addr && &t[..] == b"hello from A")),
            "{b:?}"
        );
        let sent = a.iter().find_map(|ev| if let Event::Sent { req: 7, msg_id } = ev { Some(*msg_id) } else { None }).expect("no Sent");
        assert!(a.iter().any(|ev| matches!(ev, Event::Acked { msg_id } if *msg_id == sent)), "{a:?}");
        // What is not simulated says so.
        e.phone_cmd(0, &Command::TraceRoute { req: 9, dest: b_addr }.encode());
        let a = frames(&e.phone_events(0));
        assert!(matches!(&a[..], [Event::Error { req: 9, .. }]), "{a:?}");
    }

    /// Linked devices from phones: A creates an account, B waits and is
    /// linked by A; a message from C to the account reaches both.
    #[test]
    fn phones_link_devices_and_share_the_account() {
        use maarse_core::companion::{person_op, person_state};
        let sc = json!({"seed": 3, "duration_s": 900, "mail": "post", "nodes": [
            {"kind": "handheld", "lat": 56.0, "lon": 9.0},
            {"kind": "repeater", "lat": 56.0, "lon": 9.05},
            {"kind": "handheld", "lat": 56.0, "lon": 9.1},
            {"kind": "handheld", "lat": 56.0, "lon": 9.06}
        ], "traffic": {"direct_per_min": 0, "channel_per_min": 0}});
        let mut e = create(&sc.to_string()).unwrap();
        e.run_until(20_000, usize::MAX);
        for n in [0, 2, 3] {
            e.phone_cmd(n, &Command::GetInfo.encode());
            frames(&e.phone_events(n));
        }
        e.phone_cmd(0, &Command::PersonSetup { req: 1, op: person_op::CREATE, text: text(b"") }.encode());
        let a = frames(&e.phone_events(0));
        assert!(a.iter().any(|ev| matches!(ev, Event::PersonPhrase { phrase } if phrase.split(|&b| b == b' ').count() == 24)), "{a:?}");
        let person = a.iter().find_map(|ev| if let Event::Person(p) = ev { Some(p.person) } else { None }).unwrap();
        e.phone_cmd(2, &Command::PersonSetup { req: 2, op: person_op::AWAIT_LINK, text: text(b"") }.encode());
        let b = frames(&e.phone_events(2));
        assert!(b.iter().any(|ev| matches!(ev, Event::Person(p) if p.state == person_state::AWAITING_LINK && !p.words.is_empty())), "{b:?}");
        e.run_until(60_000, usize::MAX);
        let b_addr = e.nodes[2].router.addr;
        e.phone_cmd(0, &Command::LinkDevice { req: 3, device: b_addr }.encode());
        e.run_until(150_000, usize::MAX);
        let b = frames(&e.phone_events(2));
        assert!(b.iter().any(|ev| matches!(ev, Event::Person(p) if p.state == person_state::MEMBER && p.person == person && p.devices.len() == 2)), "{b:?}");
        assert!(b.iter().any(|ev| matches!(ev, Event::DeviceLinked { device, .. } if *device == b_addr)), "{b:?}");
        // C writes to the account address: both devices get it.
        e.phone_cmd(3, &Command::SendDirect { req: 4, dest: person, text: text(b"to the account") }.encode());
        e.run_until(400_000, usize::MAX);
        for n in [0, 2] {
            let f = frames(&e.phone_events(n));
            assert!(f.iter().any(|ev| matches!(ev, Event::DirectReceived { text: t, .. } if &t[..] == b"to the account")), "node {n}: {f:?}");
        }
    }

    /// A phone sends a small file through the companion commands; the other
    /// phone gets it with the same bytes. A call over LoRa is refused with
    /// the host's text.
    #[test]
    fn phone_file_and_call_refusal() {
        let sc = json!({"seed": 3, "duration_s": 600, "nodes": [
            {"kind": "handheld", "lat": 56.0, "lon": 9.0},
            {"kind": "repeater", "lat": 56.0, "lon": 9.05},
            {"kind": "handheld", "lat": 56.0, "lon": 9.1}
        ], "traffic": {"direct_per_min": 0, "channel_per_min": 0}});
        let mut e = create(&sc.to_string()).unwrap();
        e.run_until(20_000, usize::MAX);
        let b_addr = e.nodes[2].router.addr;
        e.phone_cmd(2, &Command::GetInfo.encode());
        frames(&e.phone_events(2));
        e.phone_cmd(0, &Command::GetFileLimit { dest: b_addr }.encode());
        let a = frames(&e.phone_events(0));
        assert!(matches!(&a[..], [Event::FileLimit { max_bytes: 8192, via_lora: true, .. }]), "{a:?}");
        let data: Vec<u8> = (0..1000u32).map(|i| (i * 7) as u8).collect();
        e.phone_cmd(0, &Command::FileBegin { req: 3, dest: b_addr, size: 1000, mime: text(b"text/plain"), name: text(b"a.txt") }.encode());
        for (k, part) in data.chunks(200).enumerate() {
            e.phone_cmd(0, &Command::FileData { req: 3, offset: (k * 200) as u32, data: text(part) }.encode());
        }
        e.phone_cmd(0, &Command::FileEnd { req: 3 }.encode());
        e.run_until(400_000, usize::MAX);
        let a = frames(&e.phone_events(0));
        assert!(a.iter().any(|ev| matches!(ev, Event::FileStarted { req: 3, .. })), "{a:?}");
        assert!(a.iter().any(|ev| matches!(ev, Event::FileDone { ok: true, outgoing: true, .. })), "{a:?}");
        let b = frames(&e.phone_events(2));
        assert!(b.iter().any(|ev| matches!(ev, Event::FileIncoming { size: 1000, .. })), "{b:?}");
        let got: Vec<u8> = b.iter().filter_map(|ev| if let Event::FileChunk { data, .. } = ev { Some(data.to_vec()) } else { None }).flatten().collect();
        assert_eq!(got, data);
        e.phone_cmd(0, &Command::SendCall { dest: b_addr, data: text(&[1]) }.encode());
        let a = frames(&e.phone_events(0));
        assert!(matches!(&a[..], [Event::Error { text: t, .. }] if &t[..] == maarse_hostrules::calls::REFUSED_TEXT.as_bytes()), "{a:?}");
    }
}
