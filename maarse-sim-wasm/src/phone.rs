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
    /// One event queue per screen (phone window, PC) on this node: maarse-host
    /// serves several clients at once.
    screens: std::collections::BTreeMap<u32, VecDeque<Vec<u8>>>,
    /// Message history on a host (maarse_hostrules::history), shared by the screens.
    hist: Vec<maarse_hostrules::history::Item>,
    hist_read: HashMap<Address, u64>,
    hist_next: u64,
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
    /// Node settings as the firmware keeps them (Get*/Set*).
    pub(crate) radio: Option<maarse_core::companion::RadioConfig>,
    admins: heapless::Vec<Address, 4>,
    regions: (bool, String),
    pub(crate) sharing: Option<maarse_core::companion::Sharing>,
    /// Site of a fixed node (antenna height, gain, owner).
    site: maarse_core::appdata::Site,
    wifi: Option<maarse_core::companion::WifiSettings>,
    /// Trace routes waiting for their answer: (destination, request).
    traces: Vec<(Address, u16)>,
    /// Range test (model): settings, beacon count, received per SF.
    range: Option<maarse_core::rangetest::Config>,
    range_seq: u32,
    range_rx: Vec<(u8, u16, i32, i32)>,
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
    /// The screen whose command is being handled (its replies go only to it).
    cur_screen: Option<u32>,
    /// Link grants on their way: (message, person, device).
    pending_links: Vec<(usize, usize, usize)>,
    /// Remote admin commands and replies on their way: message -> frame.
    remote_out: HashMap<usize, Vec<u8>>,
    remote_back: HashMap<usize, Vec<u8>>,
    /// Account backups: 6-word code -> person address.
    backups: Vec<(String, Address)>,
}

fn txt(s: &str) -> maarse_core::companion::Text {
    text(s.as_bytes())
}

impl Engine {
    /// The node's site as the firmware reports it: the stored one, with the
    /// radio's transmit power.
    fn phone_site(&self, node: usize) -> maarse_core::appdata::Site {
        let p = &self.phones.attached[&node];
        let tx = p.radio.map(|r| r.tx_power_dbm).unwrap_or(self.sc.tx_dbm as i8);
        maarse_core::appdata::Site { tx_dbm: tx, ..p.site.clone() }
    }

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
        // Replies go to the screen that asked; what happens on the node goes
        // to every screen (maarse-host send_client / broadcast).
        let broadcast = matches!(
            ev,
            Event::DirectReceived { .. } | Event::ChannelReceived { .. } | Event::Acked { .. } | Event::CarryAcked { .. } | Event::Deposited { .. }
                | Event::Heard { .. } | Event::NodeInfo { .. } | Event::RouteLearned { .. } | Event::FileIncoming { .. } | Event::FileChunk { .. }
                | Event::FileDone { .. } | Event::Call { .. } | Event::Person(_) | Event::DeviceLinked { .. } | Event::ChannelJoined { .. }
        );
        let only = if broadcast { None } else { self.phones.cur_screen };
        let f = ev.encode().to_vec();
        if p.screens.is_empty() {
            p.screens.insert(0, VecDeque::new());
        }
        for (id, q) in p.screens.iter_mut() {
            if only.is_some_and(|o| o != *id) {
                continue;
            }
            if q.len() >= MAX_QUEUED {
                q.pop_front();
                p.dropped += 1;
            }
            q.push_back(f.clone());
        }
        if matches!(ev, Event::DirectReceived { .. } | Event::ChannelReceived { .. } | Event::Acked { .. } | Event::Deposited { .. } | Event::CarryAcked { .. }) {
            self.hist_note(node, ev);
        }
    }

    /// Raw frames (history events) to one screen or all.
    fn phone_raw(&mut self, node: usize, frame: Vec<u8>, only: Option<u32>, except: Option<u32>) {
        let Some(p) = self.phones.attached.get_mut(&node) else { return };
        for (id, q) in p.screens.iter_mut() {
            if only.is_some_and(|o| o != *id) || except == Some(*id) {
                continue;
            }
            q.push_back(frame.clone());
        }
    }

    /// The host's history follows what happens (maarse-host node/hist.rs).
    fn hist_note(&mut self, node: usize, ev: &Event) {
        use maarse_hostrules::history::*;
        if !self.at_host(node) {
            return;
        }
        let ts = (1_700_000_000 + self.now / 1000) as u32;
        let Some(p) = self.phones.attached.get_mut(&node) else { return };
        let mut add = |kind: u8, conv: Address, from: Address, hops: u8, content: &[u8]| {
            p.hist_next += 1;
            p.hist.push(Item { id: p.hist_next, ts, kind, outgoing: false, state: ST_RECEIVED, hops, msg_id: 0, conv, from, content: content.to_vec() });
        };
        match ev {
            Event::DirectReceived { from, hops, text } => add(KIND_DIRECT, *from, *from, *hops, text),
            Event::ChannelReceived { channel, from, hops, text, .. } => add(KIND_CHANNEL, *channel, *from, *hops, text),
            Event::Acked { msg_id } | Event::Deposited { msg_id } | Event::CarryAcked { msg_id } => {
                let st = match ev {
                    Event::Acked { .. } => ST_ACKED,
                    Event::Deposited { .. } => ST_DEPOSITED,
                    _ => ST_CARRIED,
                };
                if let Some(it) = p.hist.iter_mut().rev().find(|i| i.msg_id == *msg_id && i.outgoing) {
                    it.state = st;
                }
            }
            _ => {}
        }
    }

    /// A message sent from one screen: in the history, and live to the others.
    fn hist_sent(&mut self, node: usize, kind: u8, conv: Address, msg_id: u64, text: &[u8]) {
        use maarse_hostrules::history::*;
        if !self.at_host(node) {
            return;
        }
        let ts = (1_700_000_000 + self.now / 1000) as u32;
        let me = self.nodes[node].router.addr;
        let screen = self.phones.cur_screen;
        let Some(p) = self.phones.attached.get_mut(&node) else { return };
        p.hist_next += 1;
        let it = Item { id: p.hist_next, ts, kind, outgoing: true, state: ST_SENT, hops: 0, msg_id, conv, from: me, content: text.to_vec() };
        p.hist.push(it.clone());
        self.phone_raw(node, item_frame(&it, true), None, screen);
    }

    /// History commands 0x30-0x33 (maarse-host only; firmware nodes answer
    /// "invalid frame" as the firmware does).
    fn hist_command(&mut self, node: usize, bytes: &[u8]) {
        use maarse_hostrules::history::*;
        let screen = self.phones.cur_screen;
        if !self.at_host(node) {
            return self.phone_err(node, 0, error_code::BAD_FRAME, "invalid frame");
        }
        let Some(cmd) = parse_command(bytes) else { return };
        match cmd {
            HistCmd::Get { req, conv, before, limit } => {
                let p = &self.phones.attached[&node];
                let mut v: Vec<&Item> = p.hist.iter().filter(|i| conv.is_none_or(|c| i.conv == c) && (before == 0 || i.id < before)).collect();
                let more = v.len() > limit as usize;
                let skip = v.len().saturating_sub((limit as usize).min(MAX_PAGE));
                v.drain(..skip);
                let frames: Vec<Vec<u8>> = v.iter().map(|i| item_frame(i, false)).collect();
                let oldest = v.first().map_or(0, |i| i.id);
                let n = frames.len() as u8;
                for f in frames {
                    self.phone_raw(node, f, screen, None);
                }
                self.phone_raw(node, end_frame(req, conv, n, more, oldest), screen, None);
            }
            HistCmd::MarkRead { conv, up_to } => {
                let p = self.phones.attached.get_mut(&node).unwrap();
                let mark = if up_to == 0 { p.hist_next } else { up_to };
                p.hist_read.insert(conv, mark);
                let unread = p.hist.iter().filter(|i| i.conv == conv && i.id > mark && !i.outgoing).count();
                self.phone_raw(node, read_mark_frame(conv, mark, unread), None, None);
            }
            HistCmd::GetReadState => {
                let p = &self.phones.attached[&node];
                let mut convs: Vec<Address> = p.hist.iter().map(|i| i.conv).collect();
                convs.sort_by_key(|a| a.0);
                convs.dedup();
                let frames: Vec<Vec<u8>> = convs
                    .into_iter()
                    .map(|c| {
                        let mark = p.hist_read.get(&c).copied().unwrap_or(0);
                        let unread = p.hist.iter().filter(|i| i.conv == c && i.id > mark && !i.outgoing).count();
                        read_mark_frame(c, mark, unread)
                    })
                    .collect();
                for f in frames {
                    self.phone_raw(node, f, screen, None);
                }
            }
            HistCmd::Delete { conv, flags, ts, content } => {
                let p = self.phones.attached.get_mut(&node).unwrap();
                if flags & DEL_CONVERSATION != 0 {
                    p.hist.retain(|i| i.conv != conv);
                } else {
                    let out = flags & DEL_OUTGOING != 0;
                    p.hist.retain(|i| !(i.conv == conv && i.outgoing == out && i.content == content && i.ts.abs_diff(ts) <= 120));
                }
                let mut b = conv.0.to_vec();
                b.push(flags);
                b.extend_from_slice(&ts.to_le_bytes());
                b.extend_from_slice(&content);
                self.phone_raw(node, frame(EV_DELETED, &b), None, None);
            }
        }
    }

    /// HistoryInfo, sent on connect so the client uses the history.
    fn hist_info(&mut self, node: usize) {
        use maarse_hostrules::history::*;
        if !self.at_host(node) {
            return;
        }
        let screen = self.phones.cur_screen;
        let p = &self.phones.attached[&node];
        let mut b = vec![1u8];
        b.extend_from_slice(&90u16.to_le_bytes());
        b.extend_from_slice(&(p.hist.len() as u32).to_le_bytes());
        b.extend_from_slice(&p.hist_next.to_le_bytes());
        self.phone_raw(node, frame(EV_INFO, &b), screen, None);
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
        // The proposed firmware rule: a companion node without its phone does
        // not acknowledge direct messages (hosts read their own).
        if self.sc.features.hold_direct && !self.at_host(node) {
            self.nodes[node].router.hold_direct = true;
        }
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
        let held = std::mem::take(&mut self.nodes[node].router.hold_direct);
        if held && self.sc.mail == "post" && self.nodes[node].alive() {
            // Back: ask the post offices for what senders left there meanwhile.
            self.mail_sync(node);
        }
        let Some(p) = self.phones.attached.get_mut(&node) else { return };
        let (ev, msgs) = (p.lost_events, p.lost_msgs);
        p.lost_events = 0;
        p.lost_msgs = 0;
        let note = format!("Simulator: Bluetooth link to the radio is back. While it was down {ev} events were thrown away, {msgs} of them messages (the firmware drops frames from before a connection; senders may already show them as delivered).");
        self.phone_emit(node, &Event::Log { text: txt(&note) });
    }

    pub fn phone_cmd(&mut self, node: usize, bytes: &[u8]) -> Value {
        self.phone_cmd_on(node, 0, bytes)
    }

    /// `phone_cmd {node, screen, frame}`: bytes from one screen.
    pub fn phone_cmd_on(&mut self, node: usize, screen: u32, bytes: &[u8]) -> Value {
        self.phones.attached.entry(node).or_default().screens.entry(screen).or_default();
        self.phones.cur_screen = Some(screen);
        let r = self.phone_cmd_inner(node, bytes);
        self.phones.cur_screen = None;
        r
    }

    fn phone_cmd_inner(&mut self, node: usize, bytes: &[u8]) -> Value {
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
            if maarse_hostrules::history::is_history_command(&f) {
                self.hist_command(node, &f);
                continue;
            }
            if f.first() == Some(&0x39) {
                self.backbones(node);
                continue;
            }
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

    /// `0x39 GetBackbones` as maarse-host answers it (node/backbone.rs): one
    /// `0xBA Backbone` per working TCP, Wi-Fi or LoRa trunk link of this
    /// node, then link 0 as the end of the list. Firmware nodes have none.
    fn backbones(&mut self, node: usize) {
        let screen = self.phones.cur_screen;
        let mut out = Vec::new();
        if self.at_host(node) {
            for &k in &self.nodes[node].links {
                if !self.link_ok(k) {
                    continue;
                }
                let l = &self.links[k];
                let peer = if l.a == node { l.b } else { l.a };
                let p = &self.nodes[peer];
                let kind = if l.lora { "lora" } else if l.wifi { "wifi" } else { "tcp" };
                let name = format!("{kind} {}", if p.label.is_empty() { format!("node {peer}") } else { p.label.clone() });
                let server = p.kind == Kind::Host && !p.radio;
                let flags = server as u8 | (p.radio as u8) << 1 | ((p.kind == Kind::Host) as u8) << 2;
                let mut b = ((k + 1) as u16).to_le_bytes().to_vec();
                b.push(flags);
                b.extend_from_slice(&((self.now / 1000) as u32).to_le_bytes());
                b.extend_from_slice(&p.router.addr.0);
                for s in ["sim", name.as_str()] {
                    let s = &s.as_bytes()[..s.len().min(64)];
                    b.push(s.len() as u8);
                    b.extend_from_slice(s);
                }
                b.push(0);
                out.push(b);
            }
        }
        let mut end = 0u16.to_le_bytes().to_vec();
        end.extend_from_slice(&[0; 1 + 4 + 16]);
        end.extend_from_slice(&[0, 0, 0]);
        out.push(end);
        for body in out {
            let mut f = ((body.len() + 1) as u16).to_le_bytes().to_vec();
            f.push(0xBA);
            f.extend_from_slice(&body);
            self.phone_raw(node, f, screen, None);
        }
    }

    /// `phone_events {node}`: all queued event frames (each length-prefixed).
    pub fn phone_events(&mut self, node: usize) -> Value {
        self.phone_events_on(node, 0)
    }

    /// `phone_events {node, screen}`: the queued frames for one screen.
    pub fn phone_events_on(&mut self, node: usize, screen: u32) -> Value {
        let Some(p) = self.phones.attached.get_mut(&node) else { return json!({"frames": [], "attached": false}) };
        let frames: Vec<Vec<u8>> = p.screens.entry(screen).or_default().drain(..).collect();
        let link = self.now >= p.link_down_until;
        json!({"frames": frames, "attached": true, "dropped": p.dropped, "alive": self.nodes[node].alive(), "link": link, "link_drops": p.link_drops})
    }

    pub fn phone_detach(&mut self, node: usize) {
        self.phones.attached.remove(&node);
    }

    /// One screen goes; the node keeps its phone state while others remain.
    pub fn phone_detach_screen(&mut self, node: usize, screen: u32) {
        let gone = self.phones.attached.get_mut(&node).is_some_and(|p| {
            p.screens.remove(&screen);
            p.screens.is_empty()
        });
        if gone {
            self.phones.attached.remove(&node);
        }
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
                    identity: Some(self.node_key(node)),
                };
                self.phone_emit(node, &info);
                self.hist_info(node);
                let chans: Vec<(Address, String)> = self.phones.attached[&node].channels.clone();
                for (ch, name) in chans {
                    self.phone_emit(node, &Event::ChannelJoined { channel: ch, name: txt(&name) });
                }
                // Contacts: the other phones' nodes this node knows.
                let others: Vec<usize> = self.phones.attached.keys().copied().filter(|&o| o != node).collect();
                for o in others {
                    if self.knows(node, o) {
                        let addr = self.nodes[o].router.addr;
                        self.phone_emit(node, &Event::Heard { addr, hops: 0, identity: Some(self.node_key(o)) });
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
            Command::GetSite => {
                let site = self.phone_site(node);
                self.phone_emit(node, &Event::Site(site));
            }
            Command::SetSite(site) => {
                // The antenna height is the simulated node's own: links change.
                if site.is_set() {
                    self.nodes[node].h = site.height_dm as f64 / 10.0;
                    self.relink(node);
                }
                if let Some(p) = self.phones.attached.get_mut(&node) {
                    p.site = site;
                }
                let site = self.phone_site(node);
                self.phone_emit(node, &Event::Site(site));
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
                self.hist_sent(node, maarse_hostrules::history::KIND_DIRECT, dest, msg_id, &t);
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
                        self.hist_sent(node, maarse_hostrules::history::KIND_CHANNEL, channel, msg_id, &t);
                    }
                    Err(e) => self.phone_err(node, req, error_code::NOT_IN_CHANNEL, &format!("Simulator: channel send failed ({e:?})")),
                }
            }
            Command::GetStats => {
                let items = self.node_stats(node);
                self.phone_emit(node, &Event::Stats { items });
            }
            Command::Remote { req, dest, frame } => {
                let Some(&to) = self.addr_idx.get(&dest) else {
                    return self.phone_err(node, req, error_code::UNKNOWN_DEST, "Simulator: no node with this address in this world");
                };
                // A sealed control message (content::CONTROL) travels to the node.
                // A counter keeps equal commands apart (sealed messages differ anyway).
                self.phones.counter += 1;
                let m = self.send_payload(node, to, [b"RADM".as_slice(), &self.phones.counter.to_be_bytes(), &frame].concat());
                self.phones.remote_out.insert(m, frame.to_vec());
                let msg_id = self.msgs[m].id;
                self.phone_emit(node, &Event::Sent { req, msg_id });
            }
            Command::SendAlert { req, meta, text: t } => {
                // maarse-host: only a host with an issuer key issues alerts.
                // In the simulator every host (and app at a host) has one: the
                // authority key the nodes trust.
                if !self.at_host(node) {
                    return self.phone_err(node, req, error_code::BAD_FRAME, "Only a host with an issuer key sends alerts (firmware nodes cannot)");
                }
                let kind = maarse_crypto::alert::event::NAMES.get(meta.event as usize).copied().unwrap_or("info");
                let area = match meta.area {
                    Some((la, lo, r)) => crate::scenario::Area { circle: Some([la as f64 / 1e5, lo as f64 / 1e5, r as f64 * 100.0]), ..Default::default() },
                    None => crate::scenario::Area::default(),
                };
                let text_s = String::from_utf8_lossy(&t).into_owned();
                match self.send_alert_v2(node, kind, &area, &text_s, meta.expires_min, meta.severity, "real") {
                    Some(idx) => self.phone_emit(node, &Event::Sent { req, msg_id: idx as u64 + 1 }),
                    None => self.phone_err(node, req, error_code::BAD_FRAME, "Could not sign the alert"),
                }
            }
            Command::GetRangeTest => {
                let st = self.range_state(node);
                self.phone_emit(node, &Event::RangeState(st));
            }
            Command::SetRangeTest(cfg) => {
                let cfg = match cfg.sanitized() {
                    Ok(c) => c,
                    Err(e) => return self.phone_err(node, 0, error_code::BAD_FRAME, e),
                };
                if let Some(p) = self.phones.attached.get_mut(&node) {
                    p.range = (cfg.mode != maarse_core::rangetest::mode::OFF).then_some(cfg);
                    p.range_seq = 0;
                    p.range_rx.clear();
                }
                if cfg.mode == maarse_core::rangetest::mode::BEACON {
                    self.schedule_range_beacon(node, self.now + 1000);
                }
                let st = self.range_state(node);
                self.phone_emit(node, &Event::RangeState(st));
                self.phone_emit(node, &Event::Log { text: txt("Simulator: the range test is a model: beacons are not put on the air; receivers get the radio model's RSSI and SNR") });
            }
            Command::GetNeighbors => {
                for ev in neighbor_events(&self.nodes[node].router.neighbors, self.now) {
                    self.phone_emit(node, &ev);
                }
            }
            Command::GetRadio => {
                let r = self.phone_radio(node);
                self.phone_emit(node, &Event::Radio(r));
            }
            Command::SetRadio(r) => {
                // The node's spreading factor follows; frequency, bandwidth
                // and power are one setting for the whole simulated country
                // (Break > Radio settings), as the radio model is shared.
                if (7..=12).contains(&r.sf) {
                    self.nodes[node].sf = r.sf;
                }
                if let Some(p) = self.phones.attached.get_mut(&node) {
                    p.radio = Some(r);
                }
                self.phone_emit(node, &Event::Radio(r));
                self.build_links();
                self.phone_emit(node, &Event::Log { text: txt("Radio saved. Simulator: this node's spreading factor changed; frequency, bandwidth and power are set for all nodes in Break > Radio settings") });
            }
            Command::GetAdmins => {
                let admins = self.phones.attached[&node].admins.clone();
                self.phone_emit(node, &Event::Admins { admins });
            }
            Command::SetAdmins { admins } => {
                if let Some(p) = self.phones.attached.get_mut(&node) {
                    p.admins = admins.clone();
                }
                self.phone_emit(node, &Event::Admins { admins });
            }
            Command::GetRegions => {
                let (strict, names) = self.phones.attached[&node].regions.clone();
                self.phone_emit(node, &Event::Regions { strict, names: txt(&names) });
            }
            Command::SetRegions { strict, names } => {
                let names = String::from_utf8_lossy(&names).into_owned();
                let mut r = maarse_core::region::Regions::new();
                for n in names.split([',', ' ']).filter(|n| !n.is_empty()) {
                    let _ = r.push(maarse_core::region::region_id(n));
                }
                self.nodes[node].router.regions = r;
                self.nodes[node].router.strict_region = strict;
                if let Some(p) = self.phones.attached.get_mut(&node) {
                    p.regions = (strict, names.clone());
                }
                self.phone_emit(node, &Event::Regions { strict, names: txt(&names) });
            }
            Command::SetRole { role } => {
                if let Some(r) = maarse_core::Role::from_u8(role) {
                    self.nodes[node].router.set_role(r);
                    self.phone_emit(node, &Event::Log { text: txt("Role saved") });
                }
            }
            Command::GetSharing => {
                let sh = self.phone_sharing(node);
                self.phone_emit(node, &Event::Sharing(sh));
            }
            Command::SetSharing(sh) => {
                if let Some(p) = self.phones.attached.get_mut(&node) {
                    p.sharing = Some(sh);
                }
                self.phone_emit(node, &Event::Sharing(sh));
                self.phone_emit(node, &Event::Log { text: txt("Sharing saved; shown in the next announce") });
            }
            Command::GetWifi => {
                let w = self.phones.attached[&node].wifi.clone().unwrap_or_default();
                self.phone_emit(node, &Event::Wifi { enabled: w.enabled, modem: w.modem, state: 0, ip: [0; 4], ssid: w.ssid.clone(), peer: w.peer.clone() });
            }
            Command::SetWifi(w) => {
                if let Some(p) = self.phones.attached.get_mut(&node) {
                    p.wifi = Some(w.clone());
                }
                self.phone_emit(node, &Event::Wifi { enabled: w.enabled, modem: w.modem, state: 0, ip: [0; 4], ssid: w.ssid.clone(), peer: w.peer.clone() });
                self.phone_emit(node, &Event::Log { text: txt("Wi-Fi saved. Simulator: a node's own Wi-Fi link is drawn as a link on the map (Build), not from this setting") });
            }
            Command::OtaStatus => {
                self.phone_emit(node, &Event::OtaState { req: 0, state: 0, next: 0, size: 0, version: 0, current: 0, text: txt("Simulator: no firmware updates in the simulated world") });
            }
            Command::TraceRoute { req, dest, .. } => {
                let dest = self.person_device_addr(&dest).unwrap_or(dest);
                if let Some(p) = self.phones.attached.get_mut(&node) {
                    p.traces.retain(|t| t.0 != dest);
                    p.traces.push((dest, req));
                }
                let mut token = [0u8; 16];
                self.rng.fill(&mut token[..]);
                let now = self.now;
                let actions = self.nodes[node].router.send_trace(dest, token, now);
                self.apply(node, actions.into_iter().collect(), None);
                self.phone_emit(node, &Event::Sent { req, msg_id: 0 });
            }
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
                    // Our own pages (maarse-host own_page): straight to the screen.
                    return self.own_page(node, req, &String::from_utf8_lossy(&path));
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
            Command::GetPerson => {
                let info = self.person_info(node);
                self.phone_emit(node, &Event::Person(info));
            }
            Command::PersonSetup { req, op, text: t } => self.person_setup(node, req, op, &t),
            Command::LinkDevice { req, device } => self.link_device(node, req, device),
            Command::JoinPrivateChannel { key, name } => {
                let ch = maarse_crypto::channel_address(&key);
                let label = String::from_utf8_lossy(&name).into_owned();
                let _ = self.nodes[node].router.subscribe(ch);
                if let Some(p) = self.phones.attached.get_mut(&node)
                    && !p.channels.iter().any(|(a, _)| *a == ch)
                {
                    p.channels.push((ch, label.clone()));
                }
                self.phone_emit(node, &Event::ChannelJoined { channel: ch, name: txt(&label) });
            }
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
            PacketType::Probe => {
                // A trace route answer (firmware ext.rs on_trace).
                let Some(r) = maarse_core::diag::parse_response(payload) else { return };
                let Some(p) = self.phones.attached.get_mut(&node) else { return };
                let Some(i) = p.traces.iter().position(|t| t.0 == dest) else { return };
                let (_, req) = p.traces.remove(i);
                self.phone_emit(node, &Event::TraceResult { req, dest, path: r.path.iter().copied().collect(), out: r.out.iter().copied().collect(), back: r.back.iter().copied().collect() });
            }
            PacketType::Announce => {
                if let Some(&who) = self.addr_idx.get(&dest)
                    && who != node
                    && self.nodes[who].kind.is_user()
                {
                    self.phone_emit(node, &Event::Heard { addr: dest, hops, identity: Some(self.node_key(who)) });
                    // The announce's app data (name, capabilities such as
                    // pages), as the firmware passes it on: keys (64) |
                    // timestamp (4) | app data length | app data | ...
                    if let Some(&len) = payload.get(68)
                        && let Some(raw) = payload.get(69..69 + len as usize)
                    {
                        let a = maarse_core::appdata::parse(raw);
                        if a.name.is_some() || a.caps.is_some() || a.position.is_some() || a.role.is_some() || a.person.is_some() {
                            self.phone_emit(node, &Event::NodeInfo { addr: dest, hops, position: a.position, telemetry: a.telemetry, name: a.name, caps: a.caps, role: a.role, person: a.person });
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
        if let Some(frame) = self.phones.remote_out.remove(&m) {
            return self.remote_arrived(m, node, &frame);
        }
        if let Some(frame) = self.phones.remote_back.remove(&m) {
            let from = self.nodes[self.msgs[m].from].router.addr;
            return self.phone_emit(node, &Event::RemoteReply { from, frame: text(&frame) });
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

    /// A node's public key (64 bytes) for safety numbers. The simulator's
    /// nodes route with random addresses; this key is derived from the seed
    /// and the node number with the real key code (maarse_crypto::Identity),
    /// so both ends compute the same safety number.
    pub(crate) fn node_key(&self, node: usize) -> [u8; 64] {
        use sha2::{Digest, Sha512};
        let mut h = Sha512::new();
        h.update(b"maarse/sim/node-key");
        h.update(self.sc.seed.to_le_bytes());
        h.update((node as u64).to_le_bytes());
        let secret: [u8; 64] = h.finalize().into();
        maarse_crypto::Identity::from_secret_bytes(&secret).public().to_bytes()
    }

    /// The radio settings a phone sees: what it saved, else the firmware
    /// default with this node's spreading factor and the scenario's band.
    fn phone_radio(&self, node: usize) -> maarse_core::companion::RadioConfig {
        if let Some(r) = self.phones.attached.get(&node).and_then(|p| p.radio) {
            return r;
        }
        let mut r = maarse_core::companion::RadioConfig::EU_DEFAULT;
        r.sf = self.nodes[node].sf;
        if self.sc.freq_mhz > 0.0 {
            r.freq_hz = (self.sc.freq_mhz * 1e6) as u32;
        }
        if self.sc.bw_hz > 0 {
            r.bw_hz = self.sc.bw_hz;
        }
        r.tx_power_dbm = self.sc.tx_dbm as i8;
        r.duty_permille = crate::engine::effective_duty(&self.sc) as u16;
        r
    }

    /// What a phone shares in its announces (default: nothing, the node's
    /// place pre-filled for when it is switched on).
    pub(crate) fn phone_sharing(&self, node: usize) -> maarse_core::companion::Sharing {
        if let Some(s) = self.phones.attached.get(&node).and_then(|p| p.sharing) {
            return s;
        }
        let n = &self.sc.nodes[node];
        maarse_core::companion::Sharing { precision: 0, lat_e7: (n.lat * 1e7) as i32, lon_e7: (n.lon * 1e7) as i32, telemetry: false }
    }

    /// The status numbers a node reports (GetStats).
    pub(crate) fn node_stats(&self, node: usize) -> Stats {
        let rt = &self.nodes[node].router;
        let mut items = Stats::new();
        let _ = items.push((stat::UPTIME_S, (self.now / 1000) as i32));
        let _ = items.push((stat::KNOWN_NODES, self.heard_from[node].len() as i32));
        let _ = items.push((stat::ROUTES, rt.routes().len() as i32));
        let _ = items.push((stat::CHANNELS, self.phones.attached.get(&node).map_or(0, |p| p.channels.len()) as i32));
        let _ = items.push((stat::LINKS, self.nodes[node].links.iter().filter(|&&k| self.link_ok(k)).count() as i32));
        let _ = items.push((stat::CLIENTS, self.phones.attached.contains_key(&node) as i32));
        let _ = items.push((stat::ROLE, rt.role.as_u8() as i32));
        items
    }

    /// Who may control `node` remotely: its phone's admin list, or the
    /// scenario's `admins` (node numbers).
    fn admins_of(&self, node: usize) -> Vec<Address> {
        let mut v: Vec<Address> = self.phones.attached.get(&node).map(|p| p.admins.to_vec()).unwrap_or_default();
        for &i in self.sc.nodes.get(node).map(|n| n.admins.as_slice()).unwrap_or(&[]) {
            if i < self.nodes.len() {
                v.push(self.nodes[i].router.addr);
            }
        }
        v
    }

    /// A remote admin command reached `node` (firmware `on_control`): only
    /// admins, and only stats, radio, role and update status; the replies
    /// travel back as messages.
    fn remote_arrived(&mut self, m: usize, node: usize, frame: &[u8]) {
        let from = self.msgs[m].from;
        let deny = |msg: &str| Event::Error { req: 0, code: error_code::BAD_FRAME, text: txt(msg) };
        let from_addr = self.nodes[from].router.addr;
        let mut replies: Vec<Event> = Vec::new();
        if !self.admins_of(node).contains(&from_addr) {
            replies.push(deny("You are not an administrator of this node"));
        } else {
            match Command::decode(frame) {
                Ok(Command::GetStats) => replies.push(Event::Stats { items: self.node_stats(node) }),
                Ok(Command::GetRadio) => replies.push(Event::Radio(self.phone_radio(node))),
                Ok(Command::SetRadio(r)) => match r.validate() {
                    Ok(()) => {
                        replies.push(Event::Radio(r));
                        if (7..=12).contains(&r.sf) {
                            self.nodes[node].sf = r.sf;
                            self.build_links();
                        }
                    }
                    Err(e) => replies.push(deny(e)),
                },
                Ok(Command::SetRole { role }) => match maarse_core::Role::from_u8(role) {
                    Some(r) => {
                        self.nodes[node].router.set_role(r);
                        replies.push(Event::Stats { items: self.node_stats(node) });
                    }
                    None => replies.push(deny("Unknown role")),
                },
                Ok(Command::OtaStatus) => replies.push(Event::OtaState { req: 0, state: 0, next: 0, size: 0, version: 0, current: 0, text: txt("Simulator: no firmware updates in the simulated world") }),
                _ => replies.push(deny("The command cannot be controlled remotely")),
            }
        }
        for ev in replies {
            let f = ev.encode();
            let body = f[2..].to_vec();
            self.phones.counter += 1;
            let r = self.send_payload(node, from, [b"RREP".as_slice(), &self.phones.counter.to_be_bytes(), &body].concat());
            self.phones.remote_back.insert(r, body);
        }
    }

    /// Range test (model) state for a phone.
    fn range_state(&self, node: usize) -> maarse_core::companion::RangeState {
        use maarse_core::companion::{RangeSfStat, RangeState};
        let Some(p) = self.phones.attached.get(&node) else { return RangeState::default() };
        let mut st = RangeState { config: p.range.unwrap_or_default(), sf: self.nodes[node].sf, synced: !p.range_rx.is_empty(), count: p.range_rx.len() as u32, missed: 0, seq: p.range_seq, per_sf: heapless::Vec::new() };
        for sf in 7..=12u8 {
            let v: Vec<&(u8, u16, i32, i32)> = p.range_rx.iter().filter(|r| r.0 == sf).collect();
            if v.is_empty() {
                continue;
            }
            let n = v.len() as i32;
            let _ = st.per_sf.push(RangeSfStat {
                sf,
                rx: n as u16,
                lost: 0,
                rssi_avg: (v.iter().map(|r| r.2).sum::<i32>() / n) as i16,
                rssi_min: v.iter().map(|r| r.2).min().unwrap_or(0) as i16,
                snr_avg_x4: (v.iter().map(|r| r.3 * 4).sum::<i32>() / n) as i16,
                snr_min: v.iter().map(|r| r.3).min().unwrap_or(0) as i8,
            });
        }
        st
    }

    /// A beacon from `node`: every phone in receiver mode that the radio
    /// model says hears it gets RangeRx (SF steps as the firmware: the SF
    /// mask, `per_step` beacons each).
    pub(crate) fn range_beacon(&mut self, node: usize) {
        use maarse_core::companion::{RangeRx, RangeTx};
        let Some(cfg) = self.phones.attached.get(&node).and_then(|p| p.range) else { return };
        if cfg.mode != maarse_core::rangetest::mode::BEACON || !self.nodes[node].alive() {
            return;
        }
        let seq = self.phones.attached.get(&node).map_or(0, |p| p.range_seq);
        let sfs = cfg.sfs(self.nodes[node].sf);
        let step = (seq / cfg.per_step.max(1) as u32) as usize % sfs.len().max(1);
        let sf = sfs.get(step).copied().unwrap_or(self.nodes[node].sf);
        if let Some(p) = self.phones.attached.get_mut(&node) {
            p.range_seq += 1;
        }
        let air = maarse_core::airtime::LoraParams::new(sf).airtime_us(maarse_core::rangetest::FIXED_LEN + cfg.pad as usize) / 1000;
        self.phone_emit(node, &Event::RangeTx(RangeTx { uptime_ms: self.now as u32, seq, sf, slot: step as u8, interval_s: cfg.interval_s, airtime_ms: air as u16, charge_ma_ms: 0, skipped: 0 }));
        let short: [u8; 4] = self.nodes[node].router.addr.0[..4].try_into().unwrap_or([0; 4]);
        let receivers: Vec<usize> = self.phones.attached.iter().filter(|(k, p)| **k != node && p.range.is_some_and(|c| c.mode == maarse_core::rangetest::mode::RECEIVER)).map(|(k, _)| *k).collect();
        for r in receivers {
            let Some((rssi, snr)) = self.range_rx(node, r, sf) else { continue };
            if let Some(p) = self.phones.attached.get_mut(&r) {
                p.range_rx.push((sf, seq as u16, rssi as i32, snr as i32));
            }
            let (la, lo) = crate::engine::to_ll(self.nodes[node].x, self.nodes[node].y);
            self.phone_emit(r, &Event::RangeRx(RangeRx { uptime_ms: self.now as u32, sender: short, seq, sf, rssi: rssi as i16, snr: snr as i8, len: (maarse_core::rangetest::FIXED_LEN + cfg.pad as usize) as u8, lost: 0, skipped: 0, tx_power_dbm: self.sc.tx_dbm as i8, battery_mv: 4000, position: Some(((la * 1e7) as i32, (lo * 1e7) as i32)) }));
        }
        let next = self.now + cfg.interval_s.max(1) as u64 * 1000;
        self.schedule_range_beacon(node, next);
    }

    /// A phone asks for a page on its own node: no network, the file events
    /// come at once (a firmware node has no pages).
    fn own_page(&mut self, node: usize, req: u16, path: &str) {
        if self.nodes[node].kind != Kind::Host {
            return self.phone_err(node, req, error_code::BAD_FRAME, "This node has no pages (only hosts share pages)");
        }
        let markup = self.page_markup(node, path, node);
        let data = miniz_oxide::deflate::compress_to_vec(markup.as_bytes(), 9);
        let file_id: u64 = self.rng.r#gen();
        let me = self.nodes[node].router.addr;
        let name = path.trim_matches('/');
        let name = if name.is_empty() { "index" } else { name };
        self.phone_emit(node, &Event::Sent { req, msg_id: 0 });
        self.phone_emit(node, &Event::FileIncoming { file_id, from: me, size: data.len() as u32, mime: text(maarse_core::content::PAGE_MIME), name: txt(name) });
        for (i, part) in data.chunks(maarse_core::companion::MAX_TEXT).enumerate() {
            self.phone_emit(node, &Event::FileChunk { file_id, offset: (i * maarse_core::companion::MAX_TEXT) as u32, data: text(part) });
        }
        self.phone_emit(node, &Event::FileDone { file_id, ok: true, outgoing: false, text: txt(name) });
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
            person_op::BACKUP => {
                let Some(k) = self.person_of_node(node) else { return fail(self, "This device does not belong to a person") };
                if self.phones.persons[k].master != node {
                    return fail(self, "Only the master device makes backups");
                }
                // The 6-word code that opens the backup file (the file itself
                // stays in the simulator).
                let code: Vec<&str> = (0..6).map(|_| maarse_crypto::recovery::word(self.rng.gen_range(0..2048u16))).collect();
                let code = code.join(" ");
                let person = self.phones.persons[k].addr;
                self.phones.backups.push((code.clone(), person));
                self.phone_emit(node, &Event::PersonBackup { code: txt(&code) });
            }
            person_op::RESTORE_BACKUP => {
                if self.person_of_node(node).is_some() {
                    return fail(self, "This device already belongs to a person (leave it first)");
                }
                let code = String::from_utf8_lossy(t).split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
                let Some(&(_, person)) = self.phones.backups.iter().find(|(c, _)| *c == code) else {
                    return fail(self, "The code does not open a backup (or no backup was made in this world)");
                };
                let Some(k) = self.phones.persons.iter().position(|p| p.addr == person) else { return fail(self, "That person is gone") };
                // The old master device is removed; this one becomes the master.
                let old = self.phones.persons[k].master;
                for d in self.phones.persons[k].devices.iter_mut() {
                    if d.0 == old {
                        d.1 = true;
                    }
                }
                self.phones.persons[k].devices.push((node, false));
                self.phones.persons[k].master = node;
                self.person_changed(k);
                self.announce(node);
            }
            _ => fail(self, "Unknown operation"),
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
        self.phones.counter += 1;
        let m = self.send_payload(node, d, [b"PLINK".as_slice(), &self.phones.counter.to_be_bytes()].concat());
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
            .filter_map(|f| {
                let b: Vec<u8> = f.as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u8).collect();
                // History frames (0xB0-0xB9) are maarse-host's own, not companion events.
                if maarse_hostrules::history::EV_ITEM <= b[2] && b[2] <= maarse_hostrules::history::EV_DELETED && !matches!(b[2], 0xb4..=0xb8) {
                    return None;
                }
                Some(Event::decode(&b[2..]).unwrap())
            })
            .collect()
    }

    /// `0x39 GetBackbones` on a host lists its working links as `0xBA`
    /// frames, then link 0; a firmware node answers with the end only.
    #[test]
    fn backbones_list_the_hosts_links() {
        let sc = json!({"seed": 4, "duration_s": 600, "nodes": [
            {"kind": "host", "lat": 56.0, "lon": 9.0, "label": "Aarhus host"},
            {"kind": "host", "lat": 55.7, "lon": 12.5, "label": "Copenhagen host"},
            {"kind": "handheld", "lat": 56.0, "lon": 9.01}
        ], "links": [{"a": 0, "b": 1, "kind": "tcp", "up": true}],
           "traffic": {"direct_per_min": 0, "channel_per_min": 0}});
        let mut e = create(&sc.to_string()).unwrap();
        e.run_until(5_000, usize::MAX);
        let ba = |v: &Value| -> Vec<Vec<u8>> {
            v["frames"].as_array().unwrap().iter()
                .map(|f| f.as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u8).collect::<Vec<u8>>())
                .filter(|b| b[2] == 0xBA)
                .collect()
        };
        e.phone_cmd(0, &[1, 0, 0x39]);
        let got = ba(&e.phone_events(0));
        assert_eq!(got.len(), 2, "{got:?}");
        assert_ne!(u16::from_le_bytes([got[0][3], got[0][4]]), 0);
        assert!(String::from_utf8_lossy(&got[0]).contains("tcp Copenhagen host"), "{got:?}");
        assert_eq!(u16::from_le_bytes([got[1][3], got[1][4]]), 0);
        e.phone_cmd(2, &[1, 0, 0x39]);
        let got = ba(&e.phone_events(2));
        assert_eq!(got.len(), 1, "{got:?}");
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
        // Trace route: the answer comes back with the path and the SNRs.
        e.phone_cmd(0, &Command::TraceRoute { req: 9, dest: b_addr, via: 0 }.encode());
        e.run_until(260_000, usize::MAX);
        let a = frames(&e.phone_events(0));
        assert!(a.iter().any(|ev| matches!(ev, Event::TraceResult { req: 9, dest, out, .. } if *dest == b_addr && !out.is_empty())), "{a:?}");
        // Settings: regions, sharing, admins and a private channel.
        e.phone_cmd(0, &Command::SetRegions { strict: false, names: text(b"midtjylland") }.encode());
        e.phone_cmd(0, &Command::SetSharing(maarse_core::companion::Sharing { precision: 2, lat_e7: 0, lon_e7: 0, telemetry: true }).encode());
        e.phone_cmd(0, &Command::JoinPrivateChannel { key: [7; 32], name: text(b"family") }.encode());
        e.phone_cmd(0, &Command::GetRadio.encode());
        let a = frames(&e.phone_events(0));
        assert!(a.iter().any(|ev| matches!(ev, Event::Regions { names, .. } if &names[..] == b"midtjylland")), "{a:?}");
        assert!(a.iter().any(|ev| matches!(ev, Event::ChannelJoined { channel, .. } if *channel == maarse_crypto::channel_address(&[7; 32]))), "{a:?}");
        assert!(a.iter().any(|ev| matches!(ev, Event::Radio(r) if r.sf == 9)), "{a:?}");
        assert!(!a.iter().any(|ev| matches!(ev, Event::Error { .. })), "{a:?}");
        // B hears A's announce with the position and name it now shares.
        e.phone_cmd(0, &Command::AnnounceNow.encode());
        e.run_until(300_000, usize::MAX);
        let b = frames(&e.phone_events(2));
        assert!(b.iter().any(|ev| matches!(ev, Event::NodeInfo { addr, position: Some(_), telemetry: Some(_), .. } if *addr == a_addr)), "{b:?}");
        // Keys for safety numbers: the same key for A in A's Info and in B's Heard.
        assert!(b.iter().any(|ev| matches!(ev, Event::Heard { addr, identity: Some(k), .. } if *addr == a_addr && *k == e.node_key(0))), "{b:?}");
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

    /// Two screens on one host share its history: a message sent from screen
    /// 1 shows live on screen 2, both get what arrives, and the history can be
    /// read back.
    #[test]
    fn two_screens_share_the_hosts_history() {
        use maarse_hostrules::history::{EV_END, EV_INFO, EV_ITEM};
        let sc = json!({"seed": 5, "duration_s": 600, "nodes": [
            {"kind": "host", "lat": 56.0, "lon": 9.0},
            {"kind": "repeater", "lat": 56.0, "lon": 9.05},
            {"kind": "handheld", "lat": 56.0, "lon": 9.08}
        ], "traffic": {"direct_per_min": 0, "channel_per_min": 0}});
        let mut e = create(&sc.to_string()).unwrap();
        e.run_until(20_000, usize::MAX);
        let raw = |v: &Value| -> Vec<Vec<u8>> { v["frames"].as_array().unwrap().iter().map(|f| f.as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u8).collect::<Vec<u8>>()[2..].to_vec()).collect() };
        for s in [1, 2] {
            e.phone_cmd_on(0, s, &Command::GetInfo.encode());
        }
        e.phone_cmd_on(2, 1, &Command::GetInfo.encode());
        let f1 = raw(&e.phone_events_on(0, 1));
        assert!(f1.iter().any(|f| f[0] == EV_INFO), "history info on connect");
        raw(&e.phone_events_on(0, 2));
        let h = e.nodes[2].router.addr;
        e.phone_cmd_on(0, 1, &Command::SendDirect { req: 1, dest: h, text: text(b"from screen 1") }.encode());
        let f2 = raw(&e.phone_events_on(0, 2));
        assert!(f2.iter().any(|f| f[0] == EV_ITEM && f.ends_with(b"from screen 1")), "live on the other screen");
        assert!(!f2.iter().any(|f| matches!(Event::decode(f), Ok(Event::Sent { .. }))), "the reply only to the screen that sent");
        // An answer from the handheld reaches both screens.
        let a = e.nodes[0].router.addr;
        e.phone_cmd_on(2, 1, &Command::SendDirect { req: 2, dest: a, text: text(b"hello host") }.encode());
        e.run_until(200_000, usize::MAX);
        for s in [1, 2] {
            let f = frames(&e.phone_events_on(0, s));
            assert!(f.iter().any(|ev| matches!(ev, Event::DirectReceived { text: t, .. } if &t[..] == b"hello host")), "screen {s}");
        }
        // The history: both messages, then the end frame.
        let mut get = vec![0x30u8, 9, 0];
        get.extend_from_slice(&[0; 16]);
        get.extend_from_slice(&0u64.to_le_bytes());
        get.push(50);
        let mut fr = ((get.len()) as u16).to_le_bytes().to_vec();
        fr.extend_from_slice(&get);
        e.phone_cmd_on(0, 2, &fr);
        let f = raw(&e.phone_events_on(0, 2));
        assert_eq!(f.iter().filter(|x| x[0] == EV_ITEM).count(), 2, "{f:?}");
        assert_eq!(f.last().map(|x| x[0]), Some(EV_END));
    }

    /// Remote admin of a repeater, an alert from a phone at a host, an
    /// account backup restored on another phone, and a range test.
    #[test]
    fn phone_remote_admin_alert_backup_range() {
        use maarse_core::companion::person_op;
        let sc = json!({"seed": 4, "duration_s": 900, "mail": "post", "nodes": [
            {"kind": "handheld", "lat": 56.0, "lon": 9.0},
            {"kind": "repeater", "lat": 56.0, "lon": 9.05, "admins": [0]},
            {"kind": "handheld", "lat": 56.0, "lon": 9.1},
            {"kind": "host", "lat": 56.0, "lon": 9.06}
        ], "traffic": {"direct_per_min": 0, "channel_per_min": 0}});
        let mut e = create(&sc.to_string()).unwrap();
        e.run_until(20_000, usize::MAX);
        for n in [0, 2, 3] {
            e.phone_cmd(n, &Command::GetInfo.encode());
            frames(&e.phone_events(n));
        }
        let rep = e.nodes[1].router.addr;
        // 0 is an admin of the repeater; 2 is not.
        e.phone_cmd(0, &Command::Remote { req: 1, dest: rep, frame: text(&Command::GetStats.encode()[2..]) }.encode());
        e.run_until(60_000, usize::MAX);
        e.phone_cmd(2, &Command::Remote { req: 2, dest: rep, frame: text(&Command::GetStats.encode()[2..]) }.encode());
        e.run_until(200_000, usize::MAX);
        let a = frames(&e.phone_events(0));
        let b = frames(&e.phone_events(2));
        let stats = |f: &[u8]| matches!(Event::decode(f), Ok(Event::Stats { .. }));
        assert!(a.iter().any(|ev| matches!(ev, Event::RemoteReply { from, frame } if *from == rep && stats(frame))), "{a:?}");
        assert!(b.iter().any(|ev| matches!(ev, Event::RemoteReply { frame, .. } if matches!(Event::decode(frame), Ok(Event::Error { .. })))), "{b:?}");
        // An alert from the phone at the host goes out; from a handheld it is refused.
        let meta = maarse_core::companion::AlertMeta { event: 3, severity: 2, expires_min: 30, area: Some((5_600_000, 905_000, 100)), ..Default::default() };
        e.phone_cmd(3, &Command::SendAlert { req: 3, meta, text: text(b"Fire") }.encode());
        e.phone_cmd(0, &Command::SendAlert { req: 4, meta, text: text(b"Fire") }.encode());
        e.run_until(260_000, usize::MAX);
        assert!(frames(&e.phone_events(3)).iter().any(|ev| matches!(ev, Event::Sent { req: 3, .. })));
        assert!(frames(&e.phone_events(0)).iter().any(|ev| matches!(ev, Event::Error { req: 4, .. })));
        assert!(e.alert_summary(0, false)["inside_received"].as_u64().unwrap() > 0);
        // Backup: made on 0, restored on 2 (2 becomes the master, 0 removed).
        e.phone_cmd(0, &Command::PersonSetup { req: 5, op: person_op::CREATE, text: text(b"") }.encode());
        e.phone_cmd(0, &Command::PersonSetup { req: 6, op: person_op::BACKUP, text: text(b"") }.encode());
        let a = frames(&e.phone_events(0));
        let code = a.iter().find_map(|ev| if let Event::PersonBackup { code } = ev { Some(code.clone()) } else { None }).expect("backup code");
        e.phone_cmd(2, &Command::PersonSetup { req: 7, op: person_op::RESTORE_BACKUP, text: code }.encode());
        let b = frames(&e.phone_events(2));
        assert!(b.iter().any(|ev| matches!(ev, Event::Person(p) if p.master)), "{b:?}");
        // Range test: 0 sends beacons, 2 listens.
        let cfg = |mode| maarse_core::rangetest::Config { mode, interval_s: 10, per_step: 1, ..Default::default() };
        e.phone_cmd(2, &Command::SetRangeTest(cfg(maarse_core::rangetest::mode::RECEIVER)).encode());
        e.phone_cmd(0, &Command::SetRangeTest(cfg(maarse_core::rangetest::mode::BEACON)).encode());
        e.run_until(400_000, usize::MAX);
        let b = frames(&e.phone_events(2));
        assert!(b.iter().filter(|ev| matches!(ev, Event::RangeRx(_))).count() >= 5, "{b:?}");
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
