//! Gateway til MeshCore.
//!
//! En MeshCore-node med den officielle *companion_radio_usb*-firmware sidder
//! på en seriel port. Vi taler MeshCores companion-protokol med den (samme
//! protokol som MeshCore-apps bruger over USB) og lader MeshCore-noden klare
//! al MeshCore-kryptografi. På Må:rse-siden er broen bare endnu en klient af
//! vores egen node (som webklienten), så node.rs ikke skal ændres.
//!
//! Protokollen er genimplementeret ud fra MeshCores kildekode (MIT-licens,
//! <https://github.com/meshcore-dev/MeshCore>). Henvisningerne nedenfor er
//! til `main`-grenen i november 2026:
//!
//! * `src/helpers/ArduinoSerialInterface.cpp` – rammer på seriel.
//! * `examples/companion_radio/MyMesh.cpp` – kommandoer, svar og push-koder.
//! * `src/helpers/BaseChatMesh.cpp` – kanalbeskeders tekstformat.
//!
//! Semantik:
//!
//! * MeshCore-kanal ⇄ Må:rse-kanal efter `--meshcore-bridge`
//!   (standard `offentlig=public`). Tekst får afsenderen foran:
//!   `[MC Alice] hej` mod Må:rse, `[Må 1a2b3c4d] hej` mod MeshCore.
//! * Løkker: tekst, der allerede er brobygget (`[MC …` / `[Må …`), sendes
//!   ikke tilbage, og samme tekst i samme kanal sendes højst én gang pr.
//!   [`DEDUP_SECS`].
//! * Direkte beskeder: send en Må:rse-DM til gatewayen med `@Navn tekst`, så
//!   går den til MeshCore-kontakten `Navn`. Svar fra kontakten går tilbage
//!   til den seneste Må:rse-afsender (eller til `@<32 hex-tegn>` i teksten).

use crate::{In, next_id};
use maarse_core::Address;
use maarse_core::companion::{Command, Event, FrameDecoder, text};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::io::{ErrorKind, Read, Write};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ---------- protokolkonstanter (MyMesh.cpp linje 9-131) ----------

const CMD_APP_START: u8 = 1;
const CMD_SEND_TXT_MSG: u8 = 2;
const CMD_SEND_CHANNEL_TXT_MSG: u8 = 3;
const CMD_GET_CONTACTS: u8 = 4;
const CMD_SET_DEVICE_TIME: u8 = 6;
const CMD_SYNC_NEXT_MESSAGE: u8 = 10;
const CMD_DEVICE_QUERY: u8 = 22;
const CMD_GET_CHANNEL: u8 = 31;
const CMD_SET_CHANNEL: u8 = 32;

const RESP_OK: u8 = 0;
const RESP_ERR: u8 = 1;
const RESP_CONTACTS_START: u8 = 2;
const RESP_CONTACT: u8 = 3;
const RESP_END_OF_CONTACTS: u8 = 4;
const RESP_SELF_INFO: u8 = 5;
const RESP_SENT: u8 = 6;
const RESP_CONTACT_MSG: u8 = 7;
const RESP_CHANNEL_MSG: u8 = 8;
const RESP_NO_MORE_MESSAGES: u8 = 10;
const RESP_DEVICE_INFO: u8 = 13;
const RESP_CONTACT_MSG_V3: u8 = 16;
const RESP_CHANNEL_MSG_V3: u8 = 17;
const RESP_CHANNEL_INFO: u8 = 18;

const PUSH_MSG_WAITING: u8 = 0x83;
const PUSH_NEW_ADVERT: u8 = 0x8A;

/// TXT_TYPE_PLAIN (MyMesh.cpp, `CMD_SEND_CHANNEL_TXT_MSG` kræver denne).
const TXT_PLAIN: u8 = 0;
/// Den protokolversion, vi beder om med CMD_DEVICE_QUERY (≥ 3 giver SNR).
const APP_TARGET_VER: u8 = 3;
/// `MAX_FRAME_SIZE` i MeshCore (BaseSerialInterface.h) er 172; vi tillader lidt mere.
const MAX_MC_FRAME: usize = 255;
/// `MAX_TEXT_LEN` i BaseChatMesh.h (10 AES-blokke).
const MC_MAX_TEXT: usize = 160;
/// Kanalpladser, vi kigger på (MeshCores `MAX_GROUP_CHANNELS` er typisk 8-40).
const CHANNEL_SCAN: u8 = 16;
/// `PUBLIC_GROUP_PSK` "izOH6cXN6mrJ5e26oRXNcg==" (MyMesh.cpp linje 113), base64-afkodet.
pub const PUBLIC_PSK: [u8; 16] = [
    0x8b, 0x33, 0x87, 0xe9, 0xc5, 0xcd, 0xea, 0x6a, 0xc9, 0xe5, 0xed, 0xba, 0xa1, 0x15, 0xcd, 0x72,
];
/// Samme tekst i samme kanal brobygges højst én gang inden for dette tidsrum.
pub const DEDUP_SECS: u32 = 300;

/// Nøglen til en MeshCore-hashtagkanal: de første 16 bytes af
/// SHA-256("#navn"). Det er konventionen i MeshCore-apps; firmwaren får bare
/// nøglen via CMD_SET_CHANNEL.
pub fn hashtag_secret(name: &str) -> [u8; 16] {
    let h = Sha256::digest(name.as_bytes());
    let mut k = [0u8; 16];
    k.copy_from_slice(&h[..16]);
    k
}

// ---------- rammer ----------

/// Ramme fra os til MeshCore-noden: `'<'` | længde (u16 LE) | indhold
/// (ArduinoSerialInterface.cpp linje 46-57).
pub fn encode_frame(payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(payload.len() + 3);
    v.push(b'<');
    v.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    v.extend_from_slice(payload);
    v
}

/// Samler rammer fra MeshCore-noden: `'>'` | længde (u16 LE) | indhold
/// (ArduinoSerialInterface.cpp linje 31-33). Bytes uden for en ramme (fx
/// debug-udskrifter) springes over.
#[derive(Default)]
pub struct McDecoder {
    buf: Vec<u8>,
}

impl McDecoder {
    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    pub fn next_frame(&mut self) -> Option<Vec<u8>> {
        loop {
            let start = self.buf.iter().position(|&b| b == b'>');
            match start {
                None => {
                    self.buf.clear();
                    return None;
                }
                Some(s) => {
                    self.buf.drain(..s);
                }
            }
            if self.buf.len() < 3 {
                return None;
            }
            let len = u16::from_le_bytes([self.buf[1], self.buf[2]]) as usize;
            if len == 0 || len > MAX_MC_FRAME {
                self.buf.remove(0);
                continue;
            }
            if self.buf.len() < 3 + len {
                return None;
            }
            let f = self.buf[3..3 + len].to_vec();
            self.buf.drain(..3 + len);
            return Some(f);
        }
    }
}

// ---------- kommandoer (indhold uden rammehoved) ----------

/// CMD_DEVICE_QUERY | app_target_ver (MyMesh.cpp linje 1056-1057).
pub fn cmd_device_query() -> Vec<u8> {
    vec![CMD_DEVICE_QUERY, APP_TARGET_VER]
}

/// CMD_APP_START | 7 reserverede bytes | appnavn (MyMesh.cpp linje 1076-1080).
pub fn cmd_app_start(app: &str) -> Vec<u8> {
    let mut v = vec![CMD_APP_START, 0, 0, 0, 0, 0, 0, 0];
    v.extend_from_slice(app.as_bytes());
    v
}

/// CMD_SET_DEVICE_TIME | sekunder (u32 LE) (MyMesh.cpp linje 1287-1289).
pub fn cmd_set_time(unix: u32) -> Vec<u8> {
    let mut v = vec![CMD_SET_DEVICE_TIME];
    v.extend_from_slice(&unix.to_le_bytes());
    v
}

/// CMD_GET_CHANNEL | idx (MyMesh.cpp linje 1752).
pub fn cmd_get_channel(idx: u8) -> Vec<u8> {
    vec![CMD_GET_CHANNEL, idx]
}

/// CMD_SET_CHANNEL | idx | navn (32 bytes, nul-udfyldt) | nøgle (16 bytes)
/// (MyMesh.cpp linje 1769-1774).
pub fn cmd_set_channel(idx: u8, name: &str, secret: &[u8; 16]) -> Vec<u8> {
    let mut v = vec![CMD_SET_CHANNEL, idx];
    let mut n = [0u8; 32];
    let b = name.as_bytes();
    let l = b.len().min(31);
    n[..l].copy_from_slice(&b[..l]);
    v.extend_from_slice(&n);
    v.extend_from_slice(secret);
    v
}

/// CMD_GET_CONTACTS uden 'since' (MyMesh.cpp linje 1238).
pub fn cmd_get_contacts() -> Vec<u8> {
    vec![CMD_GET_CONTACTS]
}

/// CMD_SYNC_NEXT_MESSAGE (MyMesh.cpp linje 1415).
pub fn cmd_sync_next() -> Vec<u8> {
    vec![CMD_SYNC_NEXT_MESSAGE]
}

/// CMD_SEND_CHANNEL_TXT_MSG | txt_type | idx | tidsstempel (u32 LE) | tekst
/// (MyMesh.cpp linje 1178-1185). Firmwaren sætter selv "nodenavn: " foran.
pub fn cmd_send_channel(idx: u8, unix: u32, msg: &str) -> Vec<u8> {
    let mut v = vec![CMD_SEND_CHANNEL_TXT_MSG, TXT_PLAIN, idx];
    v.extend_from_slice(&unix.to_le_bytes());
    v.extend_from_slice(msg.as_bytes());
    v
}

/// CMD_SEND_TXT_MSG | txt_type | forsøg | tidsstempel (u32 LE) |
/// offentlig nøgle-præfiks (6 bytes) | tekst (MyMesh.cpp linje 1132-1142).
pub fn cmd_send_txt(prefix: &[u8; 6], unix: u32, msg: &str) -> Vec<u8> {
    let mut v = vec![CMD_SEND_TXT_MSG, TXT_PLAIN, 0];
    v.extend_from_slice(&unix.to_le_bytes());
    v.extend_from_slice(prefix);
    v.extend_from_slice(msg.as_bytes());
    v
}

// ---------- svar og push ----------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McEvent {
    Ok,
    Err(u8),
    DeviceInfo { fw_ver: u8 },
    SelfInfo { name: String },
    ContactsStart,
    /// RESP_CODE_CONTACT eller PUSH_CODE_NEW_ADVERT.
    Contact { key: [u8; 32], name: String },
    EndOfContacts,
    Sent,
    ChannelMsg { idx: u8, ts: u32, text: String },
    ContactMsg { prefix: [u8; 6], ts: u32, text: String },
    NoMoreMessages,
    ChannelInfo { idx: u8, name: String, secret: [u8; 16] },
    MsgWaiting,
    /// Andre svar (kode < 0x80) og push-beskeder (≥ 0x80), vi ikke bruger.
    Other(u8),
}

fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}

fn u32le(b: &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(..4)?.try_into().ok()?))
}

/// Afkoder en ramme fra MeshCore-noden. Kortere rammer end forventet giver
/// `None`.
pub fn decode_event(f: &[u8]) -> Option<McEvent> {
    let code = *f.first()?;
    Some(match code {
        RESP_OK => McEvent::Ok,
        RESP_ERR => McEvent::Err(f.get(1).copied().unwrap_or(0)),
        RESP_DEVICE_INFO => McEvent::DeviceInfo { fw_ver: *f.get(1)? },
        // code, adv_type, tx_power, max_tx_power, pub_key(32), lat(4), lon(4),
        // multi_acks, loc_policy, telemetry, manual_add, freq(4), bw(4), sf, cr,
        // navn (MyMesh.cpp linje 1085-1116): navnet starter ved byte 58.
        RESP_SELF_INFO => McEvent::SelfInfo {
            name: cstr(f.get(58..).unwrap_or(&[])),
        },
        RESP_CONTACTS_START => McEvent::ContactsStart,
        // code, pub_key(32), type, flags, out_path_len, out_path(64), navn(32), …
        // (writeContactRespFrame, MyMesh.cpp linje 171-191).
        RESP_CONTACT | PUSH_NEW_ADVERT => McEvent::Contact {
            key: f.get(1..33)?.try_into().ok()?,
            name: cstr(f.get(100..132)?),
        },
        RESP_END_OF_CONTACTS => McEvent::EndOfContacts,
        RESP_SENT => McEvent::Sent,
        // v3: code, snr, res1, res2, idx, path_len, txt_type, ts(4), tekst;
        // v<3 mangler de tre første bytes efter koden (MyMesh.cpp linje 559-578).
        RESP_CHANNEL_MSG | RESP_CHANNEL_MSG_V3 => {
            let r = if code == RESP_CHANNEL_MSG_V3 { &f[4..] } else { &f[1..] };
            if r.len() < 7 || r[2] != TXT_PLAIN {
                return Some(McEvent::Other(code));
            }
            McEvent::ChannelMsg {
                idx: r[0],
                ts: u32le(&r[3..])?,
                text: String::from_utf8_lossy(&r[7..]).into_owned(),
            }
        }
        // v3: code, snr, res1, res2, prefix(6), path_len, txt_type, ts(4), tekst
        // (queueMessage, MyMesh.cpp linje 440-459).
        RESP_CONTACT_MSG | RESP_CONTACT_MSG_V3 => {
            let r = if code == RESP_CONTACT_MSG_V3 { f.get(4..)? } else { &f[1..] };
            if r.len() < 12 || r[7] != TXT_PLAIN {
                return Some(McEvent::Other(code));
            }
            McEvent::ContactMsg {
                prefix: r[..6].try_into().ok()?,
                ts: u32le(&r[8..])?,
                text: String::from_utf8_lossy(&r[12..]).into_owned(),
            }
        }
        RESP_NO_MORE_MESSAGES => McEvent::NoMoreMessages,
        // code, idx, navn(32), nøgle(16) (MyMesh.cpp linje 1756-1762).
        RESP_CHANNEL_INFO => McEvent::ChannelInfo {
            idx: *f.get(1)?,
            name: cstr(f.get(2..34)?),
            secret: f.get(34..50)?.try_into().ok()?,
        },
        PUSH_MSG_WAITING => McEvent::MsgWaiting,
        c => McEvent::Other(c),
    })
}

// ---------- broen (ren logik, ingen I/O) ----------

/// Hvad broen vil have sendt.
#[derive(Debug, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)] // kortlivede værdier
pub enum Out {
    /// Indhold til MeshCore-noden (uden rammehoved).
    Mc(Vec<u8>),
    /// Companion-kommando til vores egen Må:rse-node.
    Node(Command),
    Log(String),
}

/// Hvilken MeshCore-kanal en Må:rse-kanal kobles til.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mapping {
    /// Som Må:rse-noden kalder den: `offentlig` eller `#navn`.
    pub maarse: String,
    /// `public` eller `#navn` (små bogstaver).
    pub meshcore: String,
}

fn norm_maarse(s: &str) -> String {
    let n = s.trim().to_lowercase();
    match n.as_str() {
        "" | "offentlig" | "#offentlig" | "public" | "#public" => "offentlig".into(),
        _ if n.starts_with('#') => n,
        _ => format!("#{n}"),
    }
}

fn norm_meshcore(s: &str) -> String {
    let n = s.trim().to_lowercase();
    match n.as_str() {
        "" | "public" | "#public" | "offentlig" => "public".into(),
        _ if n.starts_with('#') => n,
        _ => format!("#{n}"),
    }
}

/// Fortolker `--meshcore-bridge <maarse-kanal>=<meshcore-kanal>`. Uden `=`
/// bruges samme navn på begge sider.
pub fn parse_mapping(s: &str) -> Result<Mapping, String> {
    let (a, b) = s.split_once('=').unwrap_or((s, s));
    if a.trim().is_empty() && b.trim().is_empty() {
        return Err(format!("--meshcore-bridge: ugyldig værdi '{s}'"));
    }
    Ok(Mapping { maarse: norm_maarse(a), meshcore: norm_meshcore(b) })
}

#[derive(Clone, Copy, Debug)]
enum Pend {
    Cmd(u8),
    GetChannel,
}

/// Afkorter til højst `max` bytes uden at skære et tegn over.
fn clip(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut e = max;
    while !s.is_char_boundary(e) {
        e -= 1;
    }
    &s[..e]
}

fn short(a: &Address) -> String {
    a.to_string()[..8].to_string()
}

fn parse_addr(hex: &str) -> Option<Address> {
    if hex.len() != 32 || !hex.is_ascii() {
        return None;
    }
    let b: Option<Vec<u8>> = (0..16).map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()).collect();
    Address::from_slice(&b?)
}

pub struct Bridge {
    maps: Vec<Mapping>,
    /// Kommandoer sendt til MeshCore-noden, der venter på svar (i rækkefølge).
    pending: VecDeque<Pend>,
    /// MeshCore-kanalpladser, vi har set: idx -> (navn, nøgle).
    mc_slots: HashMap<u8, (String, [u8; 16])>,
    /// MeshCore-kanal (`public`/`#navn`) -> plads.
    mc_idx: HashMap<String, u8>,
    channels_ready: bool,
    /// Må:rse-kanaladresse -> navn (fra `ChannelJoined`).
    maarse_ch: HashMap<Address, String>,
    /// Vores Må:rse-adresse (fra `Info`).
    my_addr: Option<Address>,
    /// MeshCore-nodens navn (sættes foran kanalbeskeder af firmwaren).
    mc_name: String,
    /// MeshCore-kontakter: nøglepræfiks -> navn.
    contacts: HashMap<[u8; 6], String>,
    /// Hvem på Må:rse skrev senest til en MeshCore-kontakt.
    dm_route: HashMap<[u8; 6], Address>,
    /// (hash, tid) for brobyggede beskeder.
    seen: VecDeque<(u64, u32)>,
    syncing: bool,
}

impl Bridge {
    pub fn new(mut maps: Vec<Mapping>) -> Self {
        if maps.is_empty() {
            maps.push(Mapping { maarse: "offentlig".into(), meshcore: "public".into() });
        }
        Bridge {
            maps,
            pending: VecDeque::new(),
            mc_slots: HashMap::new(),
            mc_idx: HashMap::new(),
            channels_ready: false,
            maarse_ch: HashMap::new(),
            my_addr: None,
            mc_name: String::new(),
            contacts: HashMap::new(),
            dm_route: HashMap::new(),
            seen: VecDeque::new(),
            syncing: false,
        }
    }

    fn mc(&mut self, out: &mut Vec<Out>, p: Pend, payload: Vec<u8>) {
        self.pending.push_back(p);
        out.push(Out::Mc(payload));
    }

    /// Kaldes ved opstart: deltag i Må:rse-kanalerne.
    pub fn node_start(&self) -> Vec<Out> {
        let mut out = vec![Out::Node(Command::GetInfo)];
        for m in &self.maps {
            out.push(Out::Node(Command::JoinChannel { name: text(m.maarse.as_bytes()) }));
        }
        out
    }

    /// Kaldes, hver gang der (gen)forbindes til MeshCore-noden.
    pub fn mc_start(&mut self, now: u32) -> Vec<Out> {
        self.pending.clear();
        self.mc_slots.clear();
        self.mc_idx.clear();
        self.channels_ready = false;
        self.syncing = false;
        let mut out = Vec::new();
        self.mc(&mut out, Pend::Cmd(CMD_DEVICE_QUERY), cmd_device_query());
        self.mc(&mut out, Pend::Cmd(CMD_APP_START), cmd_app_start("maarse-host"));
        self.mc(&mut out, Pend::Cmd(CMD_SET_DEVICE_TIME), cmd_set_time(now));
        self.mc(&mut out, Pend::Cmd(CMD_GET_CONTACTS), cmd_get_contacts());
        for i in 0..CHANNEL_SCAN {
            self.mc(&mut out, Pend::GetChannel, cmd_get_channel(i));
        }
        out
    }

    /// Kaldes jævnligt: hent beskeder, hvis en MSG_WAITING er gået tabt.
    pub fn poll(&mut self) -> Vec<Out> {
        let mut out = Vec::new();
        if !self.syncing && self.channels_ready {
            self.syncing = true;
            self.mc(&mut out, Pend::Cmd(CMD_SYNC_NEXT_MESSAGE), cmd_sync_next());
        }
        out
    }

    /// Sand, hvis beskeden ikke er brobygget for nylig (og husker den).
    fn fresh(&mut self, chan: &str, body: &str, now: u32) -> bool {
        while self.seen.front().is_some_and(|&(_, t)| now.saturating_sub(t) > DEDUP_SECS) {
            self.seen.pop_front();
        }
        let mut h = Sha256::new();
        h.update(chan.as_bytes());
        h.update([0]);
        h.update(body.trim().as_bytes());
        let d = h.finalize();
        let key = u64::from_le_bytes(d[..8].try_into().unwrap());
        if self.seen.iter().any(|&(k, _)| k == key) {
            return false;
        }
        if self.seen.len() >= 512 {
            self.seen.pop_front();
        }
        self.seen.push_back((key, now));
        true
    }

    fn resolve_channels(&mut self, out: &mut Vec<Out>) {
        // Kendte pladser: offentlig efter nøgle, hashtags efter nøgle eller navn.
        let mut free: Vec<u8> = Vec::new();
        let mut slots: Vec<_> = self.mc_slots.iter().map(|(i, v)| (*i, v.clone())).collect();
        slots.sort_by_key(|s| s.0);
        for (idx, (name, secret)) in &slots {
            if name.is_empty() && secret.iter().all(|&b| b == 0) {
                free.push(*idx);
                continue;
            }
            let key = if *secret == PUBLIC_PSK {
                "public".to_string()
            } else {
                let n = norm_meshcore(name);
                if !(n.starts_with('#') && hashtag_secret(&n) == *secret) {
                    continue;
                }
                n
            };
            self.mc_idx.entry(key).or_insert(*idx);
        }
        let wanted: Vec<String> = self.maps.iter().map(|m| m.meshcore.clone()).collect();
        for w in wanted {
            if self.mc_idx.contains_key(&w) {
                continue;
            }
            if free.is_empty() {
                out.push(Out::Log(format!("MeshCore: ingen ledig kanalplads til {w}")));
                continue;
            }
            let idx = free.remove(0);
            let (name, secret) = if w == "public" {
                ("Public".to_string(), PUBLIC_PSK)
            } else {
                (w.clone(), hashtag_secret(&w))
            };
            out.push(Out::Log(format!("MeshCore: opretter kanal {name} på plads {idx}")));
            self.mc(out, Pend::Cmd(CMD_SET_CHANNEL), cmd_set_channel(idx, &name, &secret));
            self.mc_idx.insert(w, idx);
        }
        self.channels_ready = true;
        let list: Vec<String> = self.maps.iter().map(|m| format!("{} ⇄ {}", m.maarse, m.meshcore)).collect();
        out.push(Out::Log(format!("MeshCore: bro klar ({})", list.join(", "))));
        // Hent beskeder, der ventede, mens vi forbandt.
        out.extend(self.poll());
    }

    pub fn on_mc(&mut self, frame: &[u8], now: u32) -> Vec<Out> {
        let mut out = Vec::new();
        let Some(ev) = decode_event(frame) else { return out };
        let code = frame[0];
        // Svar (kode < 0x80) hører til den ældste ventende kommando; kontakter
        // kommer som START, CONTACT … END, og kun END afslutter.
        let pend = if code < 0x80 && !matches!(code, RESP_CONTACTS_START | RESP_CONTACT) {
            self.pending.pop_front()
        } else {
            None
        };
        match ev {
            McEvent::ChannelInfo { idx, name, secret } => {
                self.mc_slots.insert(idx, (name, secret));
            }
            McEvent::SelfInfo { name } => {
                out.push(Out::Log(format!("MeshCore: forbundet til '{name}'")));
                self.mc_name = name;
            }
            McEvent::DeviceInfo { fw_ver } if fw_ver < 3 => {
                out.push(Out::Log(format!("MeshCore: gammel firmware (v{fw_ver}), opdatér companion_radio")));
            }
            McEvent::Contact { key, name } => {
                let p: [u8; 6] = key[..6].try_into().unwrap();
                self.contacts.insert(p, name);
            }
            McEvent::Err(e) => {
                if let Some(Pend::Cmd(c)) = pend
                    && c != CMD_SET_DEVICE_TIME
                {
                    out.push(Out::Log(format!("MeshCore: kommando {c} fejlede (kode {e})")));
                }
                if matches!(pend, Some(Pend::Cmd(CMD_SYNC_NEXT_MESSAGE))) {
                    self.syncing = false;
                }
            }
            McEvent::MsgWaiting => out.extend(self.poll()),
            McEvent::NoMoreMessages => self.syncing = false,
            McEvent::ChannelMsg { idx, ts: _, text: t } => {
                self.syncing = false;
                out.extend(self.poll());
                self.mc_channel_msg(idx, &t, now, &mut out);
            }
            McEvent::ContactMsg { prefix, ts: _, text: t } => {
                self.syncing = false;
                out.extend(self.poll());
                self.mc_direct_msg(prefix, &t, &mut out);
            }
            McEvent::Other(RESP_CONTACT_MSG | RESP_CONTACT_MSG_V3 | RESP_CHANNEL_MSG | RESP_CHANNEL_MSG_V3) => {
                // Ikke-tekst (fx signeret eller CLI): spring over, hent næste.
                self.syncing = false;
                out.extend(self.poll());
            }
            _ => {}
        }
        if matches!(pend, Some(Pend::GetChannel))
            && !self.channels_ready
            && !self.pending.iter().any(|p| matches!(p, Pend::GetChannel))
        {
            self.resolve_channels(&mut out);
        }
        out
    }

    fn mc_channel_msg(&mut self, idx: u8, t: &str, now: u32, out: &mut Vec<Out>) {
        let Some(mc) = self.mc_idx.iter().find(|(_, i)| **i == idx).map(|(n, _)| n.clone()) else {
            return;
        };
        // Kanaltekst er "<afsender>: <besked>" (BaseChatMesh.cpp linje 523).
        let (from, body) = t.split_once(": ").unwrap_or(("?", t));
        if body.starts_with("[Må ") || body.starts_with("[MC ") || from == self.mc_name {
            return;
        }
        let targets: Vec<String> =
            self.maps.iter().filter(|m| m.meshcore == mc).map(|m| m.maarse.clone()).collect();
        for m in targets {
            let Some(ch) = self.maarse_ch.iter().find(|(_, n)| **n == m).map(|(a, _)| *a) else {
                continue;
            };
            if !self.fresh(&m, body, now) {
                continue;
            }
            let msg = format!("[MC {from}] {body}");
            out.push(Out::Node(Command::SendChannel {
                req: 0,
                channel: ch,
                text: text(clip(&msg, maarse_core::companion::MAX_TEXT).as_bytes()),
            }));
        }
    }

    fn mc_direct_msg(&mut self, prefix: [u8; 6], t: &str, out: &mut Vec<Out>) {
        let name = self.contacts.get(&prefix).cloned().unwrap_or_else(|| {
            prefix.iter().map(|b| format!("{b:02x}")).collect()
        });
        let t = t.trim();
        let (dest, body) = match t.strip_prefix('@').and_then(|r| {
            let (a, b) = r.split_once(' ').unwrap_or((r, ""));
            parse_addr(a).map(|a| (a, b))
        }) {
            Some((a, b)) => (Some(a), b),
            None => (self.dm_route.get(&prefix).copied(), t),
        };
        match dest {
            Some(d) => {
                self.dm_route.insert(prefix, d);
                let msg = format!("[MC {name}] {body}");
                out.push(Out::Node(Command::SendDirect {
                    req: 0,
                    dest: d,
                    text: text(clip(&msg, 200).as_bytes()),
                }));
            }
            None => {
                let me = self.my_addr.map(|a| a.to_string()).unwrap_or_default();
                let help = format!("Må:rse-gateway {me}. Skriv: @<Må:rse-adresse, 32 hex-tegn> besked");
                let now = unix_now();
                out.push(Out::Mc(cmd_send_txt(&prefix, now, clip(&help, MC_MAX_TEXT - 10))));
                self.pending.push_back(Pend::Cmd(CMD_SEND_TXT_MSG));
            }
        }
    }

    pub fn on_node(&mut self, ev: Event, now: u32) -> Vec<Out> {
        let mut out = Vec::new();
        match ev {
            Event::Info { addr, .. } => self.my_addr = Some(addr),
            Event::ChannelJoined { channel, name } => {
                self.maarse_ch.insert(channel, String::from_utf8_lossy(&name).into_owned());
            }
            Event::ChannelReceived { channel, from, text: t, .. } => {
                let (Some(name), true) = (self.maarse_ch.get(&channel).cloned(), self.channels_ready) else {
                    return out;
                };
                let body = String::from_utf8_lossy(&t).into_owned();
                if body.starts_with("[MC ") || body.starts_with("[Må ") || Some(from) == self.my_addr {
                    return out;
                }
                let targets: Vec<String> =
                    self.maps.iter().filter(|m| m.maarse == name).map(|m| m.meshcore.clone()).collect();
                for mc in targets {
                    let Some(&idx) = self.mc_idx.get(&mc) else { continue };
                    if !self.fresh(&mc, &body, now) {
                        continue;
                    }
                    // Firmwaren sætter "<nodenavn>: " foran (BaseChatMesh.cpp linje 523).
                    let room = MC_MAX_TEXT.saturating_sub(self.mc_name.len() + 2);
                    let msg = format!("[Må {}] {body}", short(&from));
                    self.mc(&mut out, Pend::Cmd(CMD_SEND_CHANNEL_TXT_MSG), cmd_send_channel(idx, now, clip(&msg, room)));
                }
            }
            Event::DirectReceived { from, text: t, .. } => {
                // Kun "@Navn tekst" er til broen; alt andet er til hostens egen bruger.
                let s = String::from_utf8_lossy(&t).into_owned();
                let Some(rest) = s.trim().strip_prefix('@') else { return out };
                let lower = rest.to_lowercase();
                // Længste kontaktnavn, teksten starter med (navne kan have mellemrum).
                let hit = self
                    .contacts
                    .iter()
                    .filter(|(_, n)| {
                        let n = n.to_lowercase();
                        !n.is_empty()
                            && lower.starts_with(&n)
                            && lower[n.len()..].chars().next().is_none_or(|c| c == ' ')
                    })
                    .max_by_key(|(_, n)| n.len())
                    .map(|(p, n)| (*p, n.clone()));
                match hit {
                    Some((p, n)) => {
                        let body = rest[n.len()..].trim();
                        let msg = format!("[Må {}] {body}", short(&from));
                        self.dm_route.insert(p, from);
                        self.mc(&mut out, Pend::Cmd(CMD_SEND_TXT_MSG), cmd_send_txt(&p, now, clip(&msg, MC_MAX_TEXT)));
                    }
                    None => {
                        let first = rest.split(' ').next().unwrap_or("");
                        let m = format!("MeshCore-gateway: kender ingen kontakt '{first}'");
                        out.push(Out::Node(Command::SendDirect { req: 0, dest: from, text: text(m.as_bytes()) }));
                    }
                }
            }
            _ => {}
        }
        out
    }
}

// ---------- I/O ----------

fn unix_now() -> u32 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as u32).unwrap_or(0)
}

/// Hvor ofte vi spørger efter beskeder, hvis en MSG_WAITING er gået tabt.
const POLL_EVERY: Duration = Duration::from_secs(15);

struct Ctx<'a> {
    tx: &'a Sender<In>,
    id: u32,
}

impl Ctx<'_> {
    fn dispatch(&self, port: Option<&mut dyn Write>, outs: Vec<Out>) -> std::io::Result<()> {
        let mut port = port;
        for o in outs {
            match o {
                Out::Mc(p) => {
                    if let Some(w) = port.as_deref_mut() {
                        w.write_all(&encode_frame(&p))?;
                    }
                }
                Out::Node(c) => {
                    let f = c.encode();
                    let _ = self.tx.send(In::ClientFrame { id: self.id, frame: f[2..].to_vec() });
                }
                Out::Log(s) => println!("  {s}"),
            }
        }
        Ok(())
    }
}

fn node_events(crx: &Receiver<Vec<u8>>, dec: &mut FrameDecoder) -> Vec<Event> {
    let mut evs = Vec::new();
    while let Ok(b) = crx.try_recv() {
        dec.push(&b);
        while let Some(f) = dec.next_frame() {
            if let Ok(e) = Event::decode(&f) {
                evs.push(e);
            }
        }
    }
    evs
}

/// Én forbindelse til MeshCore-noden. Returnerer, når porten fejler.
fn session<P: Read + Write>(
    port: &mut P,
    bridge: &mut Bridge,
    crx: &Receiver<Vec<u8>>,
    ndec: &mut FrameDecoder,
    ctx: &Ctx,
) -> std::io::Result<()> {
    let mut dec = McDecoder::default();
    let mut buf = [0u8; 256];
    let start = bridge.mc_start(unix_now());
    ctx.dispatch(Some(port), start)?;
    let mut last_poll = Instant::now();
    loop {
        match port.read(&mut buf) {
            Ok(0) => return Err(ErrorKind::UnexpectedEof.into()),
            Ok(n) => {
                dec.push(&buf[..n]);
                while let Some(f) = dec.next_frame() {
                    let outs = bridge.on_mc(&f, unix_now());
                    ctx.dispatch(Some(port), outs)?;
                }
            }
            Err(e) if matches!(e.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock | ErrorKind::Interrupted) => {}
            Err(e) => return Err(e),
        }
        for ev in node_events(crx, ndec) {
            let outs = bridge.on_node(ev, unix_now());
            ctx.dispatch(Some(port), outs)?;
        }
        if last_poll.elapsed() >= POLL_EVERY {
            last_poll = Instant::now();
            let outs = bridge.poll();
            ctx.dispatch(Some(port), outs)?;
        }
    }
}

/// Starter broen i en tråd. Forbinder igen, hvis porten forsvinder.
pub fn spawn(port: String, baud: u32, maps: Vec<Mapping>, tx: Sender<In>) {
    std::thread::spawn(move || {
        let id = next_id();
        let (ctx_tx, crx) = channel();
        if tx.send(In::ClientUp { id, tx: ctx_tx }).is_err() {
            return;
        }
        let ctx = Ctx { tx: &tx, id };
        let mut bridge = Bridge::new(maps);
        let mut ndec = FrameDecoder::new();
        let _ = ctx.dispatch(None, bridge.node_start());
        loop {
            match serialport::new(&port, baud).timeout(Duration::from_millis(50)).open() {
                Ok(mut sp) => {
                    let _ = sp.write_data_terminal_ready(true);
                    let r = session(&mut sp, &mut bridge, &crx, &mut ndec, &ctx);
                    eprintln!("  MeshCore: {port} mistet ({r:?}), prøver igen");
                }
                Err(e) => eprintln!("  MeshCore: kan ikke åbne {port}: {e}"),
            }
            // Følg med i Må:rse-kanaler, mens vi venter.
            let until = Instant::now() + Duration::from_secs(5);
            while Instant::now() < until {
                for ev in node_events(&crx, &mut ndec) {
                    let _ = bridge.on_node(ev, unix_now());
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(b: u8) -> Address {
        Address([b; 16])
    }

    /// RESP_CODE_CHANNEL_MSG_RECV_V3-ramme som i onChannelMessageRecv
    /// (MyMesh.cpp linje 559-578): kode, snr*4, 0, 0, idx, path_len, txt_type, ts, tekst.
    fn chan_frame(idx: u8, ts: u32, t: &str) -> Vec<u8> {
        let mut f = vec![RESP_CHANNEL_MSG_V3, 0x28, 0, 0, idx, 2, TXT_PLAIN];
        f.extend_from_slice(&ts.to_le_bytes());
        f.extend_from_slice(t.as_bytes());
        f
    }

    /// RESP_CODE_CHANNEL_INFO (MyMesh.cpp linje 1756-1762).
    fn info_frame(idx: u8, name: &str, secret: &[u8; 16]) -> Vec<u8> {
        let mut f = vec![RESP_CHANNEL_INFO, idx];
        let mut n = [0u8; 32];
        n[..name.len()].copy_from_slice(name.as_bytes());
        f.extend_from_slice(&n);
        f.extend_from_slice(secret);
        f
    }

    #[test]
    fn frame_header_matches_arduino_serial_interface() {
        // '<' (0x3c), længde LSB, MSB (ArduinoSerialInterface.cpp linje 46-57).
        assert_eq!(encode_frame(&[0x16, 0x03]), vec![0x3c, 0x02, 0x00, 0x16, 0x03]);
        // '>' (0x3e) fra noden (linje 31-33); skrald før rammen ignoreres.
        let mut d = McDecoder::default();
        d.push(b"debug\r\n>\x01");
        assert_eq!(d.next_frame(), None);
        d.push(&[0x00, 0x83, 0x3e, 0x02, 0x00, 0x01, 0x02]);
        assert_eq!(d.next_frame(), Some(vec![0x83]));
        assert_eq!(d.next_frame(), Some(vec![0x01, 0x02]));
        assert_eq!(d.next_frame(), None);
    }

    #[test]
    fn command_bytes() {
        assert_eq!(cmd_device_query(), vec![22, 3]);
        assert_eq!(cmd_app_start("ab"), vec![1, 0, 0, 0, 0, 0, 0, 0, b'a', b'b']);
        assert_eq!(cmd_set_time(0x01020304), vec![6, 4, 3, 2, 1]);
        assert_eq!(cmd_get_channel(2), vec![31, 2]);
        assert_eq!(cmd_sync_next(), vec![10]);
        assert_eq!(
            cmd_send_channel(1, 0x6553f100, "hej"),
            vec![3, 0, 1, 0x00, 0xf1, 0x53, 0x65, b'h', b'e', b'j']
        );
        let p = [1, 2, 3, 4, 5, 6];
        assert_eq!(cmd_send_txt(&p, 1, "x"), vec![2, 0, 0, 1, 0, 0, 0, 1, 2, 3, 4, 5, 6, b'x']);
        let s = cmd_set_channel(3, "#test", &hashtag_secret("#test"));
        assert_eq!(s.len(), 2 + 32 + 16); // MyMesh.cpp linje 1769
        assert_eq!(&s[2..7], b"#test");
        assert_eq!(s[7], 0);
        assert_eq!(s[34..36], [0x9c, 0xd8]);
    }

    #[test]
    fn public_psk_is_base64_of_firmware_constant() {
        // "izOH6cXN6mrJ5e26oRXNcg==" afkodet med base64.
        const B64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let s = b"izOH6cXN6mrJ5e26oRXNcg";
        let mut bits = 0u32;
        let mut n = 0;
        let mut out = Vec::new();
        for c in s {
            bits = (bits << 6) | B64.iter().position(|x| x == c).unwrap() as u32;
            n += 6;
            if n >= 8 {
                n -= 8;
                out.push((bits >> n) as u8);
            }
        }
        assert_eq!(out, PUBLIC_PSK);
    }

    #[test]
    fn decode_responses() {
        assert_eq!(
            decode_event(&chan_frame(0, 0x11223344, "Alice: hej")),
            Some(McEvent::ChannelMsg { idx: 0, ts: 0x11223344, text: "Alice: hej".into() })
        );
        // v<3-variant (RESP_CODE_CHANNEL_MSG_RECV): ingen snr/reserverede bytes.
        let mut old = vec![RESP_CHANNEL_MSG, 1, 0xff, TXT_PLAIN, 1, 0, 0, 0];
        old.extend_from_slice(b"Bo: hi");
        assert_eq!(decode_event(&old), Some(McEvent::ChannelMsg { idx: 1, ts: 1, text: "Bo: hi".into() }));
        // RESP_CODE_CONTACT_MSG_RECV_V3 (MyMesh.cpp linje 440-459).
        let mut dm = vec![RESP_CONTACT_MSG_V3, 0, 0, 0, 9, 8, 7, 6, 5, 4, 0xff, TXT_PLAIN, 5, 0, 0, 0];
        dm.extend_from_slice(b"yo");
        assert_eq!(
            decode_event(&dm),
            Some(McEvent::ContactMsg { prefix: [9, 8, 7, 6, 5, 4], ts: 5, text: "yo".into() })
        );
        assert_eq!(
            decode_event(&info_frame(2, "#test", &[7; 16])),
            Some(McEvent::ChannelInfo { idx: 2, name: "#test".into(), secret: [7; 16] })
        );
        // writeContactRespFrame: 1+32+1+1+1+64+32+4*4 = 148 bytes.
        let mut c = vec![RESP_CONTACT];
        c.extend_from_slice(&[0xaa; 32]);
        c.extend_from_slice(&[1, 0, 0xff]);
        c.extend_from_slice(&[0; 64]);
        let mut name = [0u8; 32];
        name[..5].copy_from_slice(b"Alice");
        c.extend_from_slice(&name);
        c.extend_from_slice(&[0; 16]);
        assert_eq!(c.len(), 148);
        assert_eq!(decode_event(&c), Some(McEvent::Contact { key: [0xaa; 32], name: "Alice".into() }));
        // RESP_CODE_SELF_INFO: navnet starter ved byte 58.
        let mut si = vec![RESP_SELF_INFO];
        si.extend_from_slice(&[0; 57]);
        si.extend_from_slice(b"Gw");
        assert_eq!(decode_event(&si), Some(McEvent::SelfInfo { name: "Gw".into() }));
        assert_eq!(decode_event(&[PUSH_MSG_WAITING]), Some(McEvent::MsgWaiting));
        assert_eq!(decode_event(&[RESP_ERR, 2]), Some(McEvent::Err(2)));
        assert_eq!(decode_event(&[]), None);
        assert_eq!(decode_event(&[RESP_CHANNEL_INFO, 1]), None);
    }

    #[test]
    fn mapping_parse() {
        assert_eq!(parse_mapping("offentlig=public").unwrap(), Mapping { maarse: "offentlig".into(), meshcore: "public".into() });
        assert_eq!(parse_mapping("Aarhus").unwrap(), Mapping { maarse: "#aarhus".into(), meshcore: "#aarhus".into() });
        assert_eq!(parse_mapping("#dk=#Denmark").unwrap(), Mapping { maarse: "#dk".into(), meshcore: "#denmark".into() });
        assert!(parse_mapping("=").is_err());
    }

    /// En broen klar med offentlig ⇄ public (plads 0) og #test ⇄ #test, som
    /// skal oprettes på den ledige plads 1.
    fn ready_bridge() -> (Bridge, Vec<Out>) {
        let mut b = Bridge::new(vec![parse_mapping("offentlig=public").unwrap(), parse_mapping("#test").unwrap()]);
        b.on_node(Event::Info { addr: addr(9), repeater: false, version: 2, max_text: 240, max_file: 0, identity: None }, 0);
        b.on_node(Event::ChannelJoined { channel: addr(1), name: text(b"offentlig") }, 0);
        b.on_node(Event::ChannelJoined { channel: addr(2), name: text(b"#test") }, 0);
        b.mc_start(100);
        let mut si = vec![RESP_DEVICE_INFO, 8];
        si.extend_from_slice(&[0; 10]);
        b.on_mc(&si, 100);
        let mut si = vec![RESP_SELF_INFO];
        si.extend_from_slice(&[0; 57]);
        si.extend_from_slice(b"Gw");
        b.on_mc(&si, 100);
        b.on_mc(&[RESP_ERR, 6], 100); // tid
        b.on_mc(&[RESP_CONTACTS_START, 1, 0, 0, 0], 100);
        let mut c = vec![RESP_CONTACT];
        c.extend_from_slice(&[0xaa; 32]);
        c.extend_from_slice(&[1, 0, 0xff]);
        c.extend_from_slice(&[0; 64]);
        let mut name = [0u8; 32];
        name[..9].copy_from_slice(b"Alice Mob");
        c.extend_from_slice(&name);
        c.extend_from_slice(&[0; 16]);
        b.on_mc(&c, 100);
        b.on_mc(&[RESP_END_OF_CONTACTS, 0, 0, 0, 0], 100);
        b.on_mc(&info_frame(0, "Public", &PUBLIC_PSK), 100);
        b.on_mc(&info_frame(1, "", &[0; 16]), 100);
        let mut last = Vec::new();
        for i in 2..CHANNEL_SCAN {
            last = b.on_mc(&[RESP_ERR, 2], 100);
            let _ = i;
        }
        (b, last)
    }

    #[test]
    fn channels_are_discovered_and_created() {
        let (b, out) = ready_bridge();
        assert!(b.channels_ready);
        assert_eq!(b.mc_idx.get("public"), Some(&0));
        assert_eq!(b.mc_idx.get("#test"), Some(&1));
        assert!(out.contains(&Out::Mc(cmd_set_channel(1, "#test", &hashtag_secret("#test")))));
        assert!(out.contains(&Out::Mc(cmd_sync_next())));
    }

    fn sent_texts(out: &[Out]) -> Vec<String> {
        out.iter()
            .filter_map(|o| match o {
                Out::Node(Command::SendChannel { text, .. }) => Some(String::from_utf8_lossy(text).into_owned()),
                Out::Mc(p) if p[0] == CMD_SEND_CHANNEL_TXT_MSG => Some(String::from_utf8_lossy(&p[7..]).into_owned()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn bridges_both_ways_with_prefix_and_no_loops() {
        let (mut b, _) = ready_bridge();
        // MeshCore -> Må:rse.
        let out = b.on_mc(&chan_frame(0, 1, "Alice: hej"), 200);
        assert_eq!(sent_texts(&out), vec!["[MC Alice] hej"]);
        assert!(matches!(&out[..], [Out::Mc(_), Out::Node(Command::SendChannel { channel, .. })] if *channel == addr(1)));
        // Samme besked igen (fx via en anden gateway) brobygges ikke.
        assert!(sent_texts(&b.on_mc(&chan_frame(0, 2, "Alice: hej"), 210)).is_empty());
        // Allerede brobygget tekst sendes ikke tilbage.
        assert!(sent_texts(&b.on_mc(&chan_frame(0, 3, "Gw2: [Må 01020304] x"), 220)).is_empty());
        // Ukendt kanalplads ignoreres.
        assert!(sent_texts(&b.on_mc(&chan_frame(5, 3, "Bo: y"), 220)).is_empty());
        // #test-kanalen på plads 1 går til Må:rse #test.
        let out = b.on_mc(&chan_frame(1, 4, "Bo: tjek"), 230);
        assert!(out.iter().any(|o| matches!(o, Out::Node(Command::SendChannel { channel, .. }) if *channel == addr(2))));

        // Må:rse -> MeshCore.
        let ev = |t: &str, from: Address| Event::ChannelReceived { channel: addr(1), from, hops: 1, text: text(t.as_bytes()) };
        let out = b.on_node(ev("hej igen", addr(0x1a)), 300);
        assert_eq!(sent_texts(&out), vec!["[Må 1a1a1a1a] hej igen"]);
        assert!(matches!(&out[0], Out::Mc(p) if p[2] == 0 && p[3..7] == 300u32.to_le_bytes()));
        // Vores egen brobyggede besked (fra en anden gateway) går ikke tilbage.
        assert!(b.on_node(ev("[MC Alice] hej", addr(0x1b)), 310).is_empty());
        // Gentagelse inden for DEDUP_SECS droppes, efter er den ok.
        assert!(b.on_node(ev("hej igen", addr(0x1a)), 320).is_empty());
        assert_eq!(sent_texts(&b.on_node(ev("hej igen", addr(0x1a)), 300 + DEDUP_SECS + 1)).len(), 1);
        // Lang tekst afkortes, så "Gw: " + tekst er højst MC_MAX_TEXT.
        let long = "æ".repeat(200);
        let out = b.on_node(ev(&long, addr(0x1a)), 1000);
        let s = &sent_texts(&out)[0];
        assert!(s.len() + "Gw: ".len() <= MC_MAX_TEXT);
    }

    #[test]
    fn direct_messages() {
        let (mut b, _) = ready_bridge();
        // Må:rse-bruger skriver "@Alice Mob hej" til gatewayen.
        let from = addr(0x33);
        let out = b.on_node(Event::DirectReceived { from, hops: 1, text: text(b"@alice mob hej") }, 50);
        assert_eq!(out, vec![Out::Mc(cmd_send_txt(&[0xaa; 6], 50, "[Må 33333333] hej"))]);
        // Svaret går tilbage til samme Må:rse-bruger.
        let mut dm = vec![RESP_CONTACT_MSG_V3, 0, 0, 0, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xff, TXT_PLAIN, 5, 0, 0, 0];
        dm.extend_from_slice(b"svar");
        let out = b.on_mc(&dm, 60);
        assert!(out.contains(&Out::Node(Command::SendDirect { req: 0, dest: from, text: text(b"[MC Alice Mob] svar") })));
        // Ukendt kontakt giver en fejl tilbage til afsenderen.
        let out = b.on_node(Event::DirectReceived { from, hops: 1, text: text(b"@Ukendt hej") }, 70);
        assert!(matches!(&out[..], [Out::Node(Command::SendDirect { dest, .. })] if *dest == from));
        // Almindelige DM'er til hosten røres ikke.
        assert!(b.on_node(Event::DirectReceived { from, hops: 1, text: text(b"hej") }, 80).is_empty());
    }

    /// Falsk seriel port: giver de forberedte bytes og gemmer det skrevne.
    struct MockPort {
        input: VecDeque<Vec<u8>>,
        written: Vec<u8>,
    }

    impl Read for MockPort {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            match self.input.pop_front() {
                Some(c) => {
                    buf[..c.len()].copy_from_slice(&c);
                    Ok(c.len())
                }
                None => Err(ErrorKind::BrokenPipe.into()),
            }
        }
    }

    impl Write for MockPort {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.written.extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn session_over_mock_serial() {
        let (tx, rx) = channel::<In>();
        let (ctx_tx, crx) = channel::<Vec<u8>>();
        let mut bridge = Bridge::new(vec![]);
        // Noden fortæller om kanalen "offentlig".
        ctx_tx.send(Event::ChannelJoined { channel: addr(1), name: text(b"offentlig") }.encode().to_vec()).unwrap();
        let mut ndec = FrameDecoder::new();
        for e in node_events(&crx, &mut ndec) {
            bridge.on_node(e, 0);
        }
        // Svar fra MeshCore: device info, self info, ok (tid), kontakter, kanaler, besked.
        let mut frames: Vec<Vec<u8>> = vec![vec![RESP_DEVICE_INFO, 8], vec![RESP_OK]];
        frames.insert(1, {
            let mut s = vec![RESP_SELF_INFO];
            s.extend_from_slice(&[0; 57]);
            s.extend_from_slice(b"Gw");
            s
        });
        frames.push(vec![RESP_CONTACTS_START, 0, 0, 0, 0]);
        frames.push(vec![RESP_END_OF_CONTACTS, 0, 0, 0, 0]);
        frames.push(info_frame(0, "Public", &PUBLIC_PSK));
        for _ in 1..CHANNEL_SCAN {
            frames.push(vec![RESP_ERR, 2]);
        }
        frames.push(vec![PUSH_MSG_WAITING]);
        frames.push(chan_frame(0, 1, "Alice: hej fra MeshCore"));
        frames.push(vec![RESP_NO_MORE_MESSAGES]);
        let mut wire = Vec::new();
        for f in &frames {
            wire.push(b'>');
            wire.extend_from_slice(&(f.len() as u16).to_le_bytes());
            wire.extend_from_slice(f);
        }
        // Del strømmen i små stykker, som en rigtig seriel port.
        let mut port = MockPort { input: wire.chunks(7).map(|c| c.to_vec()).collect(), written: Vec::new() };
        let ctx = Ctx { tx: &tx, id: 42 };
        let r = session(&mut port, &mut bridge, &crx, &mut ndec, &ctx);
        assert_eq!(r.unwrap_err().kind(), ErrorKind::BrokenPipe);
        // Det første, der skrives, er CMD_DEVICE_QUERY i en '<'-ramme.
        assert_eq!(port.written[..5], [b'<', 2, 0, CMD_DEVICE_QUERY, APP_TARGET_VER]);
        // Beskeden er sendt til Må:rse-kanalen med præfiks.
        let sent: Vec<Command> = rx
            .try_iter()
            .filter_map(|m| match m {
                In::ClientFrame { id: 42, frame } => Command::decode(&frame).ok(),
                _ => None,
            })
            .collect();
        assert!(sent.contains(&Command::SendChannel { req: 0, channel: addr(1), text: text(b"[MC Alice] hej fra MeshCore") }));
        // Og vi har bedt om næste besked (CMD_SYNC_NEXT_MESSAGE).
        assert!(port.written.windows(4).any(|w| w == [b'<', 1, 0, CMD_SYNC_NEXT_MESSAGE]));
    }
}
