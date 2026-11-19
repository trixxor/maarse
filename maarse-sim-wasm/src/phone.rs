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
}

#[derive(Default)]
pub struct Phones {
    pub attached: HashMap<usize, Phone>,
    /// Counter appended to phone messages so equal texts get distinct packet ids.
    counter: u64,
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
            Command::PostBoard { req, .. } => self.phone_err(node, req, error_code::BAD_FRAME, &not_yet("page boards")),
            Command::Remote { req, .. } => self.phone_err(node, req, error_code::BAD_FRAME, &not_yet("remote admin")),
            Command::SendAlert { req, .. } => self.phone_err(node, req, error_code::BAD_FRAME, &not_yet("alerts from a phone (use the Alert tool on the map)")),
            Command::LinkDevice { req, .. } | Command::PersonSetup { req, .. } => self.phone_err(node, req, error_code::BAD_FRAME, &not_yet("linked devices")),
            Command::GetPerson => self.phone_err(node, 0, error_code::BAD_FRAME, &not_yet("linked devices")),
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
        if !self.phones.attached.contains_key(&node) {
            return;
        }
        let p = &self.msgs[m].payload;
        let body = match p.iter().position(|&b| b == 0) {
            Some(k) => &p[..k],
            None => {
                let end = p.iter().rposition(|&b| b != b'.').map_or(0, |k| k + 1);
                &p[..end]
            }
        };
        let from = self.nodes[self.msgs[m].from].router.addr;
        self.phone_emit(node, &Event::DirectReceived { from, hops, text: text(body) });
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
