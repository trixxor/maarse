//! Packet observations over an existing backbone link and the public site feed.

use maarse_core::packet::PacketType;
use maarse_core::{Address, MAX_PACKET, Packet};
use maarse_crypto::{
    Identity, PublicIdentity, channel_address, hashtag_key, public_channel_key, seal,
};
use rand_core::{OsRng, RngCore};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const PREFIX: &[u8] = b"\xffOB";
pub const CHANNEL_PREFIX: &[u8] = b"\xffOC";
pub const FLUSH: Duration = Duration::from_millis(250);
const CONTEXT: &[u8] = b"OBS1";
const HEADER: usize = CONTEXT.len() + 8 + 4 + 8;
const ROW: usize = 14;
const BATCH: usize = (MAX_PACKET - PREFIX.len() - seal::DM_OVERHEAD - HEADER) / ROW;
const MAX_AGE_MS: u64 = 30_000;
const PUBLIC_PACKET_AGE_MS: u64 = 120_000;
const PUBLIC_CHANNEL_AGE_MS: u64 = 24 * 60 * 60 * 1000;
const KEEP_OBSERVATIONS: usize = 4096;
const KEEP_MESSAGES: usize = 200;
const MAX_DAY_PACKETS: usize = 500_000;
const DAY_MS: u64 = 86_400_000;

static ENABLED: AtomicBool = AtomicBool::new(false);
static GROUP_SALT: LazyLock<[u8; 32]> = LazyLock::new(|| {
    let mut salt = [0; 32];
    OsRng.fill_bytes(&mut salt);
    salt
});
static FEED: Mutex<Feed> = Mutex::new(Feed {
    sequence: 0,
    observations: VecDeque::new(),
    messages: VecDeque::new(),
    channels: Vec::new(),
    radio_day: None,
});

pub fn enable_public_feed(enabled: bool, auto_channels: bool, channels: &[PublicChannel]) {
    let mut feed = FEED.lock().unwrap_or_else(|err| err.into_inner());
    feed.channels = channels
        .iter()
        .map(|channel| channel.name.clone())
        .collect();
    if auto_channels && !feed.channels.iter().any(|name| name == "public") {
        feed.channels.insert(0, "public".into());
    }
    ENABLED.store(enabled, Ordering::Relaxed);
}

pub fn public_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

#[derive(Clone, Debug)]
pub struct Target {
    pub link: String,
    identity: PublicIdentity,
}

impl Target {
    pub fn parse(value: &str) -> Result<Self, String> {
        let Some((peer, key)) = value.rsplit_once('@') else {
            return Err("--observe-to expects host:port@128-hex collector identity".into());
        };
        if !peer.rsplit_once(':').is_some_and(|(host, port)| {
            !host.is_empty()
                && !host.contains(char::is_whitespace)
                && port.parse::<u16>().is_ok_and(|port| port > 0)
        }) {
            return Err("--observe-to requires the host:port of an existing TCP peer".into());
        }
        let identity = crate::parse_identity(key)
            .map_err(|_| "--observe-to requires a 128-hex collector identity")?;
        Ok(Self {
            link: format!("tcp out {peer}"),
            identity,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicChannel {
    pub name: String,
    pub address: Address,
    region: u16,
    key: [u8; 32],
}

impl PublicChannel {
    pub fn parse(value: &str) -> Result<Self, String> {
        fn valid(name: &str) -> bool {
            !name.is_empty() && !name.contains('@') && !name.chars().any(char::is_control)
        }
        let invalid = || {
            "--public-channel expects public or #name, optionally followed by @region".to_string()
        };
        let value = value.trim().to_lowercase();
        let (name, region) = maarse_core::region::split_channel(&value);
        if value.len() > maarse_core::companion::MAX_TEXT || (value.contains('@') && !valid(region)) {
            return Err(invalid());
        }
        let (name, key) = match name {
            "public" | "#public" => ("public", public_channel_key()),
            name if name.starts_with('#') && name != "#bulletin" && valid(&name[1..]) => {
                (name, hashtag_key(name))
            }
            _ => return Err(invalid()),
        };
        Ok(Self {
            name: if region.is_empty() {
                name.to_string()
            } else {
                format!("{name}@{region}")
            },
            address: channel_address(&key),
            region: maarse_core::region::region_id(region),
            key,
        })
    }

    pub fn from_frame(frame: &[u8]) -> Option<Self> {
        let name = std::str::from_utf8(frame.strip_prefix(CHANNEL_PREFIX)?).ok()?;
        Self::parse(name).ok()
    }

    pub fn frame(&self) -> Vec<u8> {
        [CHANNEL_PREFIX, self.name.as_bytes()].concat()
    }

    pub fn route(&self) -> (Address, u16) {
        (self.address, self.region)
    }

    pub fn is_for(&self, header: &maarse_core::Header) -> bool {
        header.ptype == PacketType::Channel
            && header.path.is_none()
            && self.route() == (header.dest, header.region)
    }

    pub fn from_local(name: &str, key: &[u8; 32], header: &maarse_core::Header) -> Option<Self> {
        let channel = Self::parse(name).ok()?;
        (channel.key == *key && channel.is_for(header)).then_some(channel)
    }

    pub fn builtin(header: &maarse_core::Header) -> Option<Self> {
        let key = public_channel_key();
        let address = channel_address(&key);
        if header.ptype != PacketType::Channel || header.path.is_some() || header.dest != address {
            return None;
        }
        let region = header.region;
        let name = if region == 0 { "public".into() } else { format!("public@0x{region:04x}") };
        Some(Self { name, address, region, key })
    }

    /// Only a matching deterministic open key may decrypt a web message.
    pub fn record(&self, packet: &Packet<'_>) -> bool {
        if !self.is_for(&packet.header) {
            return false;
        }
        let mut text = [0; MAX_PACKET];
        let Ok((sender, len)) = seal::open_channel(&self.key, packet.payload, &mut text) else {
            return false;
        };
        let Ok(text) = std::str::from_utf8(&text[..len]) else {
            return false;
        };
        if text.trim().is_empty() {
            return false;
        }
        let pid = packet_id(packet);
        let time = now_ms();
        let mut feed = FEED.lock().unwrap_or_else(|err| err.into_inner());
        if feed
            .messages
            .iter()
            .any(|m| m.pid == pid && m.region == self.region)
        {
            return true;
        }
        if feed.messages.len() >= KEEP_MESSAGES {
            feed.messages.pop_front();
        }
        feed.messages.push_back(ChannelMessage {
            pid,
            region: self.region,
            time,
            channel: self.name.clone(),
            sender: crate::debug::hex(&sender.0[..4]),
            text: text.to_string(),
        });
        crate::public_history::message(time, &self.name, &crate::debug::hex(&sender.0[..4]), text);
        true
    }
}

#[derive(Clone, Copy)]
pub struct Sample {
    pid: u64,
    time: u64,
    kind: PacketType,
    hops: u8,
    transport: u8,
    tx: bool,
    snr: Option<i8>,
    subject: Option<Address>,
}

fn packet_id(packet: &Packet<'_>) -> u64 {
    maarse_core::dedup::packet_id(packet.header.ptype, &packet.header.dest, packet.payload)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

impl Sample {
    pub fn new(packet: &Packet<'_>, transport: &str, tx: bool, snr: Option<i8>) -> Self {
        let transport = match transport {
            "trunk" => 1,
            "backbone" => 2,
            _ => 0,
        };
        Self {
            pid: packet_id(packet),
            time: now_ms(),
            kind: packet.header.ptype,
            hops: packet.header.hops,
            transport,
            tx,
            snr: snr.filter(|_| transport != 2 && !tx),
            subject: None,
        }
    }
}

pub struct Observers {
    pub target: Option<Target>,
    allowed: HashMap<Address, PublicIdentity>,
    pending: VecDeque<Sample>,
    session: u64,
    sequence: u32,
    seen: HashMap<(Address, u64), (u32, u64)>,
}

impl Observers {
    pub fn new(target: Option<Target>, allowed: &[PublicIdentity]) -> Self {
        Self {
            target,
            allowed: allowed.iter().map(|key| (key.address(), *key)).collect(),
            pending: VecDeque::new(),
            session: OsRng.next_u64(),
            sequence: 0,
            seen: HashMap::new(),
        }
    }

    pub fn collecting(&self) -> bool {
        !self.allowed.is_empty()
    }
    pub fn enabled(&self) -> bool {
        self.target.is_some() || self.collecting()
    }

    pub fn record(&mut self, observer: Address, sample: Sample, publish_locally: bool) {
        if publish_locally {
            publish(observer, sample);
        }
        if self.target.is_some() {
            if self.pending.len() >= BATCH * 2 {
                self.pending.pop_front();
            }
            self.pending.push_back(sample);
        }
    }

    pub fn publish_announce(&self, observer: Address, mut sample: Sample, subject: Option<Address>) {
        sample.subject = subject;
        if let Some(addr) = subject {
            crate::public_history::announce(&crate::debug::hex(&addr.0));
        }
        publish(observer, sample);
    }

    pub fn report(&mut self, me: &Identity) -> Option<Vec<u8>> {
        let target = self.target.as_ref()?;
        let time = now_ms();
        self.pending
            .retain(|sample| time.saturating_sub(sample.time) <= 2000 && sample.time <= time);
        if self.pending.is_empty() {
            return None;
        }
        self.sequence = self.sequence.wrapping_add(1);
        let mut body = Vec::with_capacity(HEADER + BATCH * ROW);
        body.extend_from_slice(CONTEXT);
        body.extend_from_slice(&self.session.to_le_bytes());
        body.extend_from_slice(&self.sequence.to_le_bytes());
        body.extend_from_slice(&time.to_le_bytes());
        for sample in self.pending.drain(..BATCH.min(self.pending.len())) {
            body.extend_from_slice(&sample.pid.to_le_bytes());
            body.extend_from_slice(&((time - sample.time) as u16).to_le_bytes());
            body.push(sample.kind as u8);
            body.push(sample.hops);
            body.push(
                sample.transport | ((sample.tx as u8) << 2) | ((sample.snr.is_some() as u8) << 3),
            );
            body.push(sample.snr.unwrap_or_default() as u8);
        }
        let mut report = vec![0; PREFIX.len() + body.len() + seal::DM_OVERHEAD];
        report[..PREFIX.len()].copy_from_slice(PREFIX);
        seal::seal_direct(
            me,
            &target.identity,
            &mut OsRng,
            &body,
            &mut report[PREFIX.len()..],
        )
        .ok()?;
        Some(report)
    }

    pub fn receive(&mut self, me: &Identity, report: &[u8]) {
        if !self.collecting() || report.len() > MAX_PACKET {
            return;
        }
        let Some(encrypted) = report.strip_prefix(PREFIX) else {
            return;
        };
        let mut plain = [0; MAX_PACKET];
        let Ok((observer, len)) = seal::open_direct(
            me,
            encrypted,
            |id| self.allowed.get(id).copied(),
            &mut plain,
        ) else {
            return;
        };
        let Some((session, seq, time, samples)) = decode(&plain[..len]) else {
            return;
        };
        let now = now_ms();
        if time.abs_diff(now) > MAX_AGE_MS {
            return;
        }
        self.seen
            .retain(|_, (_, time)| now.saturating_sub(*time) <= MAX_AGE_MS * 2);
        let key = (observer, session);
        if self.seen.get(&key).is_some_and(|(last, _)| seq <= *last)
            || (!self.seen.contains_key(&key) && self.seen.len() >= 256)
        {
            return;
        }
        self.seen.insert(key, (seq, now));
        for sample in samples {
            publish(observer, sample);
        }
    }
}

fn decode(plain: &[u8]) -> Option<(u64, u32, u64, Vec<Sample>)> {
    if !plain.starts_with(CONTEXT)
        || plain.len() < HEADER + ROW
        || plain.len() > HEADER + BATCH * ROW
    {
        return None;
    }
    let session = u64::from_le_bytes(plain[4..12].try_into().ok()?);
    let seq = u32::from_le_bytes(plain[12..16].try_into().ok()?);
    let time = u64::from_le_bytes(plain[16..24].try_into().ok()?);
    let rows = plain[HEADER..].chunks_exact(ROW);
    if !rows.remainder().is_empty() {
        return None;
    }
    let mut samples = Vec::with_capacity(rows.len());
    for row in rows {
        let flags = row[12];
        let transport = flags & 3;
        let tx = flags & 4 != 0;
        let snr = (flags & 8 != 0).then_some(row[13] as i8);
        let age = u16::from_le_bytes(row[8..10].try_into().ok()?) as u64;
        if flags > 15 || transport > 2 || (snr.is_some() && (tx || transport == 2)) || age > 2000 {
            return None;
        }
        samples.push(Sample {
            pid: u64::from_le_bytes(row[..8].try_into().ok()?),
            time: time.checked_sub(age)?,
            kind: PacketType::try_from(row[10]).ok()?,
            hops: row[11],
            transport,
            tx,
            snr,
            subject: None,
        });
    }
    Some((session, seq, time, samples))
}

struct Feed {
    sequence: u64,
    observations: VecDeque<Observation>,
    messages: VecDeque<ChannelMessage>,
    channels: Vec<String>,
    radio_day: Option<RadioDay>,
}

struct RadioDay {
    utc_day: u64,
    packets: HashSet<u64>,
    sites: HashSet<Address>,
    observations: u64,
    capped: bool,
}

impl RadioDay {
    fn new(utc_day: u64) -> Self {
        Self {
            utc_day,
            packets: HashSet::new(),
            sites: HashSet::new(),
            observations: 0,
            capped: false,
        }
    }

    fn add(&mut self, observer: Address, pid: u64) {
        self.observations += 1;
        self.sites.insert(observer);
        if self.packets.len() < MAX_DAY_PACKETS {
            self.packets.insert(pid);
        } else if !self.capped && !self.packets.contains(&pid) {
            self.capped = true;
        }
    }
}

#[derive(Clone, Serialize)]
struct Observation {
    #[serde(skip)]
    pid: u64,
    id: u64,
    observer: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    subject: Option<String>,
    time: u64,
    kind: String,
    transport: &'static str,
    direction: &'static str,
    hops: u8,
    snr: Option<i8>,
}

#[derive(Clone, Serialize)]
struct ChannelMessage {
    #[serde(skip)]
    pid: u64,
    #[serde(skip)]
    region: u16,
    time: u64,
    channel: String,
    sender: String,
    text: String,
}

fn publish(observer: Address, sample: Sample) {
    let mut feed = FEED.lock().unwrap_or_else(|err| err.into_inner());
    if sample.transport != 2 && !sample.tx {
        let day = now_ms() / DAY_MS;
        if feed
            .radio_day
            .as_ref()
            .is_none_or(|stats| stats.utc_day != day)
        {
            feed.radio_day = Some(RadioDay::new(day));
        }
        if sample.time / DAY_MS == day
            && let Some(stats) = feed.radio_day.as_mut()
        {
            stats.add(observer, sample.pid);
        }
    }
    if feed.observations.len() >= KEEP_OBSERVATIONS {
        feed.observations.pop_front();
    }
    feed.sequence += 1;
    let id = feed.sequence;
    feed.observations.push_back(Observation {
        pid: sample.pid,
        id,
        observer: crate::debug::hex(&observer.0),
        subject: sample.subject.map(|address| crate::debug::hex(&address.0)),
        time: sample.time,
        kind: format!("{:?}", sample.kind),
        hops: sample.hops,
        snr: sample.snr,
        transport: ["radio", "trunk", "backbone"][sample.transport as usize],
        direction: if sample.tx { "tx" } else { "rx" },
    });
}

#[derive(Serialize)]
struct Group<'a> {
    id: String,
    kind: &'a str,
    observations: Vec<&'a Observation>,
    count: usize,
    observer_count: usize,
    #[serde(skip)]
    observers: HashSet<&'a str>,
}

pub fn public_feed() -> String {
    let now = now_ms();
    let (observations, messages, channels, radio_today) = {
        let feed = FEED.lock().unwrap_or_else(|err| err.into_inner());
        let observations = feed
            .observations
            .iter()
            .rev()
            .filter(|obs| now.saturating_sub(obs.time) <= PUBLIC_PACKET_AGE_MS)
            .cloned()
            .collect::<Vec<_>>();
        let messages = feed
            .messages
            .iter()
            .rev()
            .filter(|msg| now.saturating_sub(msg.time) <= PUBLIC_CHANNEL_AGE_MS)
            .cloned()
            .collect::<Vec<_>>();
        let stats = feed
            .radio_day
            .as_ref()
            .filter(|stats| stats.utc_day == now / DAY_MS);
        let radio_today = serde_json::json!({
            "unique_packets": stats.map_or(0, |s| s.packets.len()),
            "observations": stats.map_or(0, |s| s.observations),
            "sites": stats.map_or(0, |s| s.sites.len()),
            "capped": stats.is_some_and(|s| s.capped),
        });
        let mut channels = feed.channels.clone();
        for msg in &messages {
            if !channels.contains(&msg.channel) {
                channels.push(msg.channel.clone());
            }
        }
        (observations, messages, channels, radio_today)
    };
    let mut groups: Vec<Group<'_>> = Vec::new();
    let mut indices = HashMap::new();
    for obs in &observations {
        let i = if let Some(i) = indices.get(&obs.pid) {
            *i
        } else if groups.len() < 100 {
            let i = groups.len();
            let digest = Sha256::new()
                .chain_update(*GROUP_SALT)
                .chain_update(obs.pid.to_le_bytes())
                .finalize();
            groups.push(Group {
                id: crate::debug::hex(&digest[..12]),
                kind: &obs.kind,
                observations: Vec::new(),
                count: 0,
                observer_count: 0,
                observers: HashSet::new(),
            });
            indices.insert(obs.pid, i);
            i
        } else {
            continue;
        };
        let group = &mut groups[i];
        group.count += 1;
        if group.observers.insert(&obs.observer) {
            group.observer_count += 1;
        }
        if group.observations.len() < 32 {
            group.observations.push(obs);
        }
    }
    serde_json::json!({
        "version": 1, "packets": groups, "channels": channels, "messages": messages, "radio_today": radio_today,
    }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_authorized_fresh_reports_once() {
        let collector = Identity::generate(&mut OsRng);
        let approved = Identity::generate(&mut OsRng);
        let other = Identity::generate(&mut OsRng);
        let target = Target {
            link: "tcp out localhost:4242".into(),
            identity: collector.public(),
        };
        let mut source = Observers::new(Some(target.clone()), &[]);
        let mut unauthorised = Observers::new(Some(target), &[]);
        let mut sink = Observers::new(None, &[approved.public()]);
        let sample = Sample {
            pid: 12,
            time: now_ms(),
            kind: PacketType::Hello,
            hops: 0,
            transport: 0,
            tx: false,
            snr: Some(7),
            subject: None,
        };
        source.record(approved.address(), sample, false);
        unauthorised.record(other.address(), sample, false);
        sink.receive(&collector, &unauthorised.report(&other).unwrap());
        let report = source.report(&approved).unwrap();
        let mut tampered = report.clone();
        *tampered.last_mut().unwrap() ^= 1;
        sink.receive(&collector, &tampered);
        assert!(sink.seen.is_empty());
        sink.receive(&collector, &report);
        sink.receive(&collector, &report);
        let feed = FEED.lock().unwrap();
        assert_eq!(feed.observations.len(), 1);
        assert_eq!(feed.observations[0].snr, Some(7));
    }
}
