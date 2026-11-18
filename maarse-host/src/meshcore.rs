//! Gateway to MeshCore.
//!
//! A MeshCore node with the official *companion_radio_usb* firmware sits
//! on a serial port. We speak MeshCore's companion protocol with it (the same
//! protocol MeshCore apps use over USB) and let the MeshCore node handle
//! all MeshCore cryptography. On the Må:rse side the bridge is just another client of
//! our own node (like the web client), so node.rs does not need to change.
//!
//! The protocol is reimplemented from MeshCore's source code (MIT licence,
//! <https://github.com/meshcore-dev/MeshCore>). The references below are
//! to the `main` branch in November 2026:
//!
//! * `src/helpers/ArduinoSerialInterface.cpp` – framing on serial.
//! * `examples/companion_radio/MyMesh.cpp` – commands, responses and push codes.
//! * `src/helpers/BaseChatMesh.cpp` – the text format of channel messages.
//!
//! Semantics:
//!
//! * MeshCore channel ⇄ Må:rse channel according to `--meshcore-bridge`
//!   (default `public=public`). Text gets the sender prefixed:
//!   `[MC Alice] hi` towards Må:rse, `[Må 1a2b3c4d] hi` towards MeshCore.
//! * Loops: text that has already been bridged (`[MC …` / `[Må …`) is not sent
//!   back, and the same text in the same channel is sent at most once per
//!   [`DEDUP_SECS`].
//! * Direct messages: send a Må:rse DM to the gateway with `@Name text`, and
//!   it goes to the MeshCore contact `Name`. Replies from the contact go back
//!   to the latest Må:rse sender (or to `@<32 hex chars>` in the text).

use crate::{In, next_id};
use maarse_core::Address;
use maarse_core::companion::{Command, Event, FrameDecoder, text};
use sha2::{Digest, Sha256};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ---------- protocol constants (MyMesh.cpp lines 9-140) ----------

const CMD_APP_START: u8 = 1;
const CMD_SEND_TXT_MSG: u8 = 2;
const CMD_SEND_CHANNEL_TXT_MSG: u8 = 3;
const CMD_GET_CONTACTS: u8 = 4;
const CMD_SET_DEVICE_TIME: u8 = 6;
const CMD_SEND_SELF_ADVERT: u8 = 7;
const CMD_SYNC_NEXT_MESSAGE: u8 = 10;
const CMD_REMOVE_CONTACT: u8 = 15;
const CMD_DEVICE_QUERY: u8 = 22;
const CMD_SEND_LOGIN: u8 = 26;
const CMD_SEND_STATUS_REQ: u8 = 27;
const CMD_GET_CHANNEL: u8 = 31;
const CMD_SET_CHANNEL: u8 = 32;
const CMD_SEND_TRACE_PATH: u8 = 36;

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

const PUSH_ADVERT: u8 = 0x80;
const PUSH_MSG_WAITING: u8 = 0x83;
const PUSH_LOGIN_SUCCESS: u8 = 0x85;
const PUSH_LOGIN_FAIL: u8 = 0x86;
const PUSH_STATUS_RESPONSE: u8 = 0x87;
const PUSH_TRACE_DATA: u8 = 0x89;
const PUSH_NEW_ADVERT: u8 = 0x8A;

/// TXT_TYPE_PLAIN (MyMesh.cpp, `CMD_SEND_CHANNEL_TXT_MSG` requires this).
const TXT_PLAIN: u8 = 0;
/// TXT_TYPE_CLI_DATA: admin command to a repeater and its reply
/// (src/helpers/TxtDataHelpers.h line 7).
const TXT_CLI: u8 = 1;
/// OUT_PATH_UNKNOWN (src/helpers/ContactInfo.h line 6); also "not flood" in messages.
const PATH_UNKNOWN: u8 = 0xFF;
/// The protocol version we ask for with CMD_DEVICE_QUERY (≥ 3 gives SNR).
const APP_TARGET_VER: u8 = 3;
/// `MAX_FRAME_SIZE` in MeshCore (BaseSerialInterface.h) is 172; we allow a bit more.
const MAX_MC_FRAME: usize = 255;
/// `MAX_TEXT_LEN` in BaseChatMesh.h (10 AES blocks).
const MC_MAX_TEXT: usize = 160;
/// Channel slots we look at (MeshCore's `MAX_GROUP_CHANNELS` is typically 8-40).
const CHANNEL_SCAN: u8 = 16;
/// `PUBLIC_GROUP_PSK` "izOH6cXN6mrJ5e26oRXNcg==" (MyMesh.cpp line 113), base64-decoded.
pub const PUBLIC_PSK: [u8; 16] = [
    0x8b, 0x33, 0x87, 0xe9, 0xc5, 0xcd, 0xea, 0x6a, 0xc9, 0xe5, 0xed, 0xba, 0xa1, 0x15, 0xcd, 0x72,
];
/// The same text in the same channel is bridged at most once within this period.
pub const DEDUP_SECS: u32 = 300;

/// The key of a MeshCore hashtag channel: the first 16 bytes of
/// SHA-256("#name"). That is the convention in MeshCore apps; the firmware just gets
/// the key via CMD_SET_CHANNEL.
pub fn hashtag_secret(name: &str) -> [u8; 16] {
    let h = Sha256::digest(name.as_bytes());
    let mut k = [0u8; 16];
    k.copy_from_slice(&h[..16]);
    k
}

// ---------- frames ----------

/// Frame from us to the MeshCore node: `'<'` | length (u16 LE) | content
/// (ArduinoSerialInterface.cpp lines 46-57).
pub fn encode_frame(payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(payload.len() + 3);
    v.push(b'<');
    v.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    v.extend_from_slice(payload);
    v
}

/// Collects frames from the MeshCore node: `'>'` | length (u16 LE) | content
/// (ArduinoSerialInterface.cpp lines 31-33). Bytes outside a frame (e.g.
/// debug output) are skipped.
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

// ---------- commands (content without frame header) ----------

/// CMD_DEVICE_QUERY | app_target_ver (MyMesh.cpp lines 1056-1057).
pub fn cmd_device_query() -> Vec<u8> {
    vec![CMD_DEVICE_QUERY, APP_TARGET_VER]
}

/// CMD_APP_START | 7 reserved bytes | app name (MyMesh.cpp lines 1076-1080).
pub fn cmd_app_start(app: &str) -> Vec<u8> {
    let mut v = vec![CMD_APP_START, 0, 0, 0, 0, 0, 0, 0];
    v.extend_from_slice(app.as_bytes());
    v
}

/// CMD_SET_DEVICE_TIME | seconds (u32 LE) (MyMesh.cpp lines 1287-1289).
pub fn cmd_set_time(unix: u32) -> Vec<u8> {
    let mut v = vec![CMD_SET_DEVICE_TIME];
    v.extend_from_slice(&unix.to_le_bytes());
    v
}

/// CMD_GET_CHANNEL | idx (MyMesh.cpp line 1752).
pub fn cmd_get_channel(idx: u8) -> Vec<u8> {
    vec![CMD_GET_CHANNEL, idx]
}

/// CMD_SET_CHANNEL | idx | name (32 bytes, zero-padded) | key (16 bytes)
/// (MyMesh.cpp lines 1769-1774).
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

/// CMD_GET_CONTACTS without 'since' (MyMesh.cpp line 1238).
pub fn cmd_get_contacts() -> Vec<u8> {
    vec![CMD_GET_CONTACTS]
}

/// CMD_SYNC_NEXT_MESSAGE (MyMesh.cpp line 1415).
pub fn cmd_sync_next() -> Vec<u8> {
    vec![CMD_SYNC_NEXT_MESSAGE]
}

/// CMD_SEND_CHANNEL_TXT_MSG | txt_type | idx | timestamp (u32 LE) | text
/// (MyMesh.cpp lines 1178-1185). The firmware itself prefixes "node name: ".
pub fn cmd_send_channel(idx: u8, unix: u32, msg: &str) -> Vec<u8> {
    let mut v = vec![CMD_SEND_CHANNEL_TXT_MSG, TXT_PLAIN, idx];
    v.extend_from_slice(&unix.to_le_bytes());
    v.extend_from_slice(msg.as_bytes());
    v
}

/// CMD_SEND_TXT_MSG | txt_type | attempt | timestamp (u32 LE) |
/// public key prefix (6 bytes) | text (MyMesh.cpp lines 1132-1142).
pub fn cmd_send_txt(prefix: &[u8; 6], unix: u32, msg: &str) -> Vec<u8> {
    cmd_send_typed(TXT_PLAIN, prefix, unix, msg)
}

/// Like [`cmd_send_txt`] with an optional txt_type. With TXT_TYPE_CLI_DATA the
/// text is sent as an admin command (MyMesh.cpp lines 1149-1151; the firmware uses
/// its own clock for the timestamp).
pub fn cmd_send_typed(txt_type: u8, prefix: &[u8; 6], unix: u32, msg: &str) -> Vec<u8> {
    let mut v = vec![CMD_SEND_TXT_MSG, txt_type, 0];
    v.extend_from_slice(&unix.to_le_bytes());
    v.extend_from_slice(prefix);
    v.extend_from_slice(msg.as_bytes());
    v
}

/// CMD_SEND_SELF_ADVERT | 1 = flood, 0 = zero-hop (MyMesh.cpp lines 1297-1313).
pub fn cmd_self_advert(flood: bool) -> Vec<u8> {
    vec![CMD_SEND_SELF_ADVERT, flood as u8]
}

/// CMD_REMOVE_CONTACT | public key (32) (MyMesh.cpp lines 1349-1358).
pub fn cmd_remove_contact(key: &[u8; 32]) -> Vec<u8> {
    let mut v = vec![CMD_REMOVE_CONTACT];
    v.extend_from_slice(key);
    v
}

/// CMD_SEND_LOGIN | public key (32) | password (MyMesh.cpp lines 1577-1596).
pub fn cmd_login(key: &[u8; 32], password: &str) -> Vec<u8> {
    let mut v = vec![CMD_SEND_LOGIN];
    v.extend_from_slice(key);
    v.extend_from_slice(clip(password, 15).as_bytes());
    v
}

/// CMD_SEND_STATUS_REQ | public key (32) (MyMesh.cpp lines 1630-1646).
pub fn cmd_status_req(key: &[u8; 32]) -> Vec<u8> {
    let mut v = vec![CMD_SEND_STATUS_REQ];
    v.extend_from_slice(key);
    v
}

/// CMD_SEND_TRACE_PATH | tag (u32 LE) | auth (u32) | flags | path
/// (companion_radio/MyMesh.cpp). flags = 0: 1-byte hash per hop.
pub fn cmd_trace(tag: u32, path: &[u8]) -> Vec<u8> {
    let mut v = vec![CMD_SEND_TRACE_PATH];
    v.extend_from_slice(&tag.to_le_bytes());
    v.extend_from_slice(&[0, 0, 0, 0, 0]);
    v.extend_from_slice(path);
    v
}

/// Path for a MeshCore ping: out along the contact's known route, to the contact itself
/// and the same way back. Only repeaters/rooms forward trace packets.
pub fn ping_path(c: &McContact) -> Option<Vec<u8>> {
    let n = usize::from(hops(c.out_path_len)?);
    let out = c.out_path.get(..n)?;
    let mut p = out.to_vec();
    p.push(c.key[0]);
    p.extend(out.iter().rev());
    Some(p)
}

// ---------- responses and push ----------

/// A MeshCore contact (writeContactRespFrame, MyMesh.cpp lines 171-191).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McContact {
    pub key: [u8; 32],
    /// ADV_TYPE_* (src/helpers/AdvertDataHelpers.h lines 7-11).
    pub kind: u8,
    /// Raw out_path_len: 0xFF = unknown (flood), otherwise hop count in the lower 6 bits.
    pub out_path_len: u8,
    /// Hash bytes for the route (64 bytes in the frame; the first `hops` apply).
    pub out_path: Vec<u8>,
    pub name: String,
    /// The contact's own clock at the latest advert.
    pub last_advert: u32,
    /// Degrees × 1 000 000 (ContactInfo.h line 18).
    pub lat: i32,
    pub lon: i32,
}

/// The node's own information and radio settings (RESP_CODE_SELF_INFO).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct McSelf {
    pub name: String,
    pub key: [u8; 32],
    pub tx_power: u8,
    /// kHz (the firmware sends `freq * 1000` with freq in MHz).
    pub freq_khz: u32,
    /// Hz (the firmware sends `bw * 1000` with bw in kHz).
    pub bw_hz: u32,
    pub sf: u8,
    pub cr: u8,
}

/// Status from a repeater (RepeaterStats, examples/simple_repeater/MyMesh.h
/// lines 44-60). Fields after the end of the frame are `None`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct McStatus {
    pub batt_mv: Option<u16>,
    pub tx_queue: Option<u16>,
    pub noise_floor: Option<i16>,
    pub last_rssi: Option<i16>,
    pub n_recv: Option<u32>,
    pub n_sent: Option<u32>,
    pub airtime_s: Option<u32>,
    pub uptime_s: Option<u32>,
    pub n_sent_flood: Option<u32>,
    pub n_sent_direct: Option<u32>,
    pub n_recv_flood: Option<u32>,
    pub n_recv_direct: Option<u32>,
    pub err_events: Option<u16>,
    /// SNR × 4.
    pub last_snr4: Option<i16>,
}

fn u16le(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32at(b: &[u8], at: usize) -> Option<u32> {
    u32le(b.get(at..)?)
}

impl McStatus {
    pub fn parse(d: &[u8]) -> McStatus {
        McStatus {
            batt_mv: u16le(d, 0),
            tx_queue: u16le(d, 2),
            noise_floor: u16le(d, 4).map(|v| v as i16),
            last_rssi: u16le(d, 6).map(|v| v as i16),
            n_recv: u32at(d, 8),
            n_sent: u32at(d, 12),
            airtime_s: u32at(d, 16),
            uptime_s: u32at(d, 20),
            n_sent_flood: u32at(d, 24),
            n_sent_direct: u32at(d, 28),
            n_recv_flood: u32at(d, 32),
            n_recv_direct: u32at(d, 36),
            err_events: u16le(d, 40),
            last_snr4: u16le(d, 42).map(|v| v as i16),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McEvent {
    Ok,
    Err(u8),
    DeviceInfo { fw_ver: u8 },
    SelfInfo(McSelf),
    ContactsStart,
    /// RESP_CODE_CONTACT (`push` = false) or PUSH_CODE_NEW_ADVERT (`push` = true).
    Contact { c: McContact, push: bool },
    /// PUSH_CODE_ADVERT: a known contact has advertised again.
    Advert { key: [u8; 32] },
    EndOfContacts,
    Sent,
    /// PUSH_CODE_TRACE_DATA: SNR × 4 for each hop and finally for ourselves.
    Trace { tag: u32, snrs: Vec<i8>, last: i8 },
    /// `snr4` is SNR × 4 (v3 only); `path_len` is raw (0xFF = direct route).
    ChannelMsg { idx: u8, ts: u32, text: String, snr4: Option<i8>, path_len: u8 },
    /// `txt_type` is TXT_TYPE_PLAIN or TXT_TYPE_CLI_DATA (others give `Other`).
    ContactMsg { prefix: [u8; 6], ts: u32, text: String, txt_type: u8, snr4: Option<i8>, path_len: u8 },
    NoMoreMessages,
    ChannelInfo { idx: u8, name: String, secret: [u8; 16] },
    MsgWaiting,
    LoginOk { prefix: [u8; 6], admin: bool },
    LoginFail { prefix: [u8; 6] },
    Status { prefix: [u8; 6], st: McStatus },
    /// Other responses (code < 0x80) and push messages (≥ 0x80) that we do not use.
    Other(u8),
}

fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}

fn u32le(b: &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(..4)?.try_into().ok()?))
}

/// Decodes a frame from the MeshCore node. Frames shorter than expected give
/// `None`.
pub fn decode_event(f: &[u8]) -> Option<McEvent> {
    let code = *f.first()?;
    Some(match code {
        RESP_OK => McEvent::Ok,
        RESP_ERR => McEvent::Err(f.get(1).copied().unwrap_or(0)),
        RESP_DEVICE_INFO => McEvent::DeviceInfo { fw_ver: *f.get(1)? },
        // code, adv_type, tx_power, max_tx_power, pub_key(32), lat(4), lon(4),
        // multi_acks, loc_policy, telemetry, manual_add, freq(4), bw(4), sf, cr,
        // name (MyMesh.cpp lines 1085-1116): the name starts at byte 58.
        RESP_SELF_INFO => McEvent::SelfInfo(McSelf {
            tx_power: *f.get(2)?,
            key: f.get(4..36)?.try_into().ok()?,
            freq_khz: u32at(f, 48)?,
            bw_hz: u32at(f, 52)?,
            sf: *f.get(56)?,
            cr: *f.get(57)?,
            name: cstr(f.get(58..).unwrap_or(&[])),
        }),
        RESP_CONTACTS_START => McEvent::ContactsStart,
        // code, pub_key(32), type, flags, out_path_len, out_path(64), name(32),
        // last_advert(4), lat(4), lon(4), lastmod(4)
        // (writeContactRespFrame, MyMesh.cpp lines 171-191).
        RESP_CONTACT | PUSH_NEW_ADVERT => McEvent::Contact {
            c: McContact {
                key: f.get(1..33)?.try_into().ok()?,
                kind: *f.get(33)?,
                out_path_len: *f.get(35)?,
                out_path: f.get(36..100)?.to_vec(),
                name: cstr(f.get(100..132)?),
                last_advert: u32at(f, 132).unwrap_or(0),
                lat: u32at(f, 136).unwrap_or(0) as i32,
                lon: u32at(f, 140).unwrap_or(0) as i32,
            },
            push: code == PUSH_NEW_ADVERT,
        },
        // code, pub_key(32) (onDiscoveredContact, MyMesh.cpp lines 363-365).
        PUSH_ADVERT => McEvent::Advert { key: f.get(1..33)?.try_into().ok()? },
        RESP_END_OF_CONTACTS => McEvent::EndOfContacts,
        RESP_SENT => McEvent::Sent,
        // v3: code, snr*4, res1, res2, idx, path_len, txt_type, ts(4), text;
        // v<3 lacks the first three bytes after the code (MyMesh.cpp lines 558-584).
        RESP_CHANNEL_MSG | RESP_CHANNEL_MSG_V3 => {
            let v3 = code == RESP_CHANNEL_MSG_V3;
            let r = if v3 { f.get(4..)? } else { &f[1..] };
            if r.len() < 7 || r[2] != TXT_PLAIN {
                return Some(McEvent::Other(code));
            }
            McEvent::ChannelMsg {
                idx: r[0],
                ts: u32le(&r[3..])?,
                text: String::from_utf8_lossy(&r[7..]).into_owned(),
                snr4: if v3 { Some(f[1] as i8) } else { None },
                path_len: r[1],
            }
        }
        // v3: code, snr*4, res1, res2, prefix(6), path_len, txt_type, ts(4), text
        // (queueMessage, MyMesh.cpp lines 440-466).
        RESP_CONTACT_MSG | RESP_CONTACT_MSG_V3 => {
            let v3 = code == RESP_CONTACT_MSG_V3;
            let r = if v3 { f.get(4..)? } else { &f[1..] };
            if r.len() < 12 || !matches!(r[7], TXT_PLAIN | TXT_CLI) {
                return Some(McEvent::Other(code));
            }
            McEvent::ContactMsg {
                prefix: r[..6].try_into().ok()?,
                ts: u32le(&r[8..])?,
                text: String::from_utf8_lossy(&r[12..]).into_owned(),
                txt_type: r[7],
                snr4: if v3 { Some(f[1] as i8) } else { None },
                path_len: r[6],
            }
        }
        RESP_NO_MORE_MESSAGES => McEvent::NoMoreMessages,
        // code, idx, name(32), key(16) (MyMesh.cpp lines 1756-1762).
        RESP_CHANNEL_INFO => McEvent::ChannelInfo {
            idx: *f.get(1)?,
            name: cstr(f.get(2..34)?),
            secret: f.get(34..50)?.try_into().ok()?,
        },
        PUSH_MSG_WAITING => McEvent::MsgWaiting,
        // code, permissions (bit 0 = admin), prefix(6), … (onContactResponse,
        // MyMesh.cpp lines 698-716).
        PUSH_LOGIN_SUCCESS => McEvent::LoginOk {
            admin: f.get(1)? & 1 != 0,
            prefix: f.get(2..8)?.try_into().ok()?,
        },
        // code, reserved, prefix(6) (MyMesh.cpp lines 717-721).
        PUSH_LOGIN_FAIL => McEvent::LoginFail { prefix: f.get(2..8)?.try_into().ok()? },
        // code, reserved, prefix(6), RepeaterStats (MyMesh.cpp lines 731-738).
        PUSH_STATUS_RESPONSE => McEvent::Status {
            prefix: f.get(2..8)?.try_into().ok()?,
            st: McStatus::parse(&f[8..]),
        },
        // code, res, path_len, flags, tag(4), auth(4), hashes(path_len),
        // snrs(path_len >> (flags & 3)), last snr (onTraceRecv).
        PUSH_TRACE_DATA => {
            let plen = usize::from(*f.get(2)?);
            let n = plen >> (f.get(3)? & 3);
            let snrs = f.get(12 + plen..12 + plen + n)?;
            McEvent::Trace {
                tag: u32le(f.get(4..)?)?,
                snrs: snrs.iter().map(|&b| b as i8).collect(),
                last: *f.get(12 + plen + n)? as i8,
            }
        }
        c => McEvent::Other(c),
    })
}

// ---------- the bridge (pure logic, no I/O) ----------

/// What the bridge wants sent.
#[derive(Debug, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)] // short-lived values
pub enum Out {
    /// Content for the MeshCore node (without frame header).
    Mc(Vec<u8>),
    /// Companion command to our own Må:rse node.
    Node(Command),
    Log(String),
    /// JSON event to the web clients on `/mc`.
    Ui(String),
}

/// Which MeshCore channel a Må:rse channel is linked to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mapping {
    /// As the Må:rse node calls it: `public` or `#name`.
    pub maarse: String,
    /// `public` or `#name` (lower case).
    pub meshcore: String,
}

fn norm_maarse(s: &str) -> String {
    let n = s.trim().to_lowercase();
    match n.as_str() {
        "" | "public" | "#public" => "public".into(),
        _ if n.starts_with('#') => n,
        _ => format!("#{n}"),
    }
}

fn norm_meshcore(s: &str) -> String {
    let n = s.trim().to_lowercase();
    match n.as_str() {
        "" | "public" | "#public" => "public".into(),
        _ if n.starts_with('#') => n,
        _ => format!("#{n}"),
    }
}

/// Parses `--meshcore-bridge <maarse-channel>=<meshcore-channel>`. Without `=`
/// the same name is used on both sides.
pub fn parse_mapping(s: &str) -> Result<Mapping, String> {
    let (a, b) = s.split_once('=').unwrap_or((s, s));
    if a.trim().is_empty() && b.trim().is_empty() {
        return Err(format!("--meshcore-bridge: invalid value '{s}'"));
    }
    Ok(Mapping { maarse: norm_maarse(a), meshcore: norm_meshcore(b) })
}

/// Bridge choices from the web client: MeshCore channel -> Må:rse channel (`None` = no bridge).
pub type Overrides = Vec<(String, Option<String>)>;

/// Reads `<data>/meshcore-bridges.txt`: one line per MeshCore channel,
/// `<maarse-channel>=<meshcore-channel>` or `-=<meshcore-channel>` for no bridge.
pub fn parse_overrides(s: &str) -> Overrides {
    let mut v: Overrides = Vec::new();
    for l in s.lines().map(str::trim) {
        let Some((a, b)) = l.split_once('=') else { continue };
        if b.trim().is_empty() {
            continue;
        }
        let mc = norm_meshcore(b);
        let m = if a.trim() == "-" { None } else { Some(norm_maarse(a)) };
        v.retain(|(k, _)| *k != mc);
        v.push((mc, m));
    }
    v
}

pub fn format_overrides(o: &Overrides) -> String {
    o.iter()
        .map(|(mc, m)| format!("{}={mc}\n", m.as_deref().unwrap_or("-")))
        .collect()
}

/// What should happen when a command from the web client succeeds.
#[derive(Clone, Debug)]
enum Effect {
    None,
    Slot(u8, String, [u8; 16]),
    DelContact([u8; 32]),
}

#[derive(Clone, Debug)]
enum Pend {
    Cmd(u8),
    GetChannel,
    /// Command from the web client (`op`).
    Ui(&'static str, Effect),
}

/// Truncates to at most `max` bytes without cutting a character in half.
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

fn unhex(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 || !s.is_ascii() {
        return None;
    }
    (0..s.len() / 2).map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()).collect()
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn parse_addr(hex: &str) -> Option<Address> {
    if hex.len() != 32 {
        return None;
    }
    Address::from_slice(&unhex(hex)?)
}

fn kind_name(k: u8) -> &'static str {
    match k {
        1 => "chat",
        2 => "repeater",
        3 => "room",
        4 => "sensor",
        _ => "?",
    }
}

/// Hop count from a raw path_len (Packet.h lines 79-80), `None` if unknown.
fn hops(p: u8) -> Option<u8> {
    (p != PATH_UNKNOWN).then_some(p & 63)
}

fn err_text(e: u8) -> String {
    // ERR_CODE_* (MyMesh.cpp lines 135-140).
    match e {
        1 => "not supported by the node".into(),
        2 => "not found".into(),
        3 => "the table is full".into(),
        4 => "wrong state".into(),
        5 => "file error on the node".into(),
        6 => "invalid argument".into(),
        _ => format!("error code {e}"),
    }
}

fn ui_err(op: &str, msg: &str) -> Out {
    Out::Ui(json!({"ev":"err","op":op,"msg":msg}).to_string())
}

pub struct Bridge {
    maps: Vec<Mapping>,
    /// Commands sent to the MeshCore node that are waiting for a response (in order).
    pending: VecDeque<Pend>,
    /// MeshCore channel slots we have seen: idx -> (name, key).
    mc_slots: HashMap<u8, (String, [u8; 16])>,
    /// MeshCore channel (`public`/`#name`) -> slot.
    mc_idx: HashMap<String, u8>,
    channels_ready: bool,
    /// Må:rse channel address -> name (from `ChannelJoined`).
    maarse_ch: HashMap<Address, String>,
    /// Display names of Må:rse nodes (from `NodeInfo`).
    names: HashMap<Address, String>,
    /// Our Må:rse address (from `Info`).
    my_addr: Option<Address>,
    /// The MeshCore node's own information (the firmware prefixes the name to channel messages).
    me: McSelf,
    /// MeshCore contacts by public key.
    contacts: HashMap<[u8; 32], McContact>,
    /// Unknown contacts that have advertised (PUSH_ADVERT has only the key);
    /// name and type are sent to the web client once the contact list has been fetched.
    lookup: HashSet<[u8; 32]>,
    /// Outstanding pings: tag -> (contact, sent).
    pings: HashMap<u32, ([u8; 32], std::time::Instant)>,
    /// Who on Må:rse last wrote to a MeshCore contact.
    dm_route: HashMap<[u8; 6], Address>,
    /// Contacts the web client has written to (they get no help text).
    ui_dm: HashSet<[u8; 6]>,
    /// (hash, time) for bridged messages.
    seen: VecDeque<(u64, u32)>,
    syncing: bool,
    connected: bool,
    port: String,
    /// Bridge choices from the web client (stored in `save_to`).
    overrides: Overrides,
    save_to: Option<PathBuf>,
}

impl Bridge {
    pub fn new(mut maps: Vec<Mapping>) -> Self {
        if maps.is_empty() {
            maps.push(Mapping { maarse: "public".into(), meshcore: "public".into() });
        }
        Bridge {
            maps,
            pending: VecDeque::new(),
            mc_slots: HashMap::new(),
            mc_idx: HashMap::new(),
            channels_ready: false,
            maarse_ch: HashMap::new(),
            my_addr: None,
            names: HashMap::new(),
            me: McSelf::default(),
            contacts: HashMap::new(),
            lookup: HashSet::new(),
            pings: HashMap::new(),
            dm_route: HashMap::new(),
            ui_dm: HashSet::new(),
            seen: VecDeque::new(),
            syncing: false,
            connected: false,
            port: String::new(),
            overrides: Vec::new(),
            save_to: None,
        }
    }

    /// Applies saved bridge choices on top of `--meshcore-bridge`.
    pub fn apply_overrides(&mut self, o: Overrides) {
        for (mc, m) in &o {
            self.maps.retain(|x| x.meshcore != *mc);
            if let Some(m) = m {
                self.maps.push(Mapping { maarse: m.clone(), meshcore: mc.clone() });
            }
        }
        self.overrides = o;
    }

    /// How a Må:rse sender is shown in MeshCore: the display name if
    /// the node has one (at most 24 bytes, not proven), otherwise 8 hex characters.
    /// Brackets are removed from the name so it cannot look like another prefix.
    fn label(&self, a: &Address) -> String {
        match self.names.get(a) {
            Some(n) => {
                let n: String = n.chars().filter(|c| *c != '[' && *c != ']').collect();
                let n = n.trim();
                if n.is_empty() { short(a) } else { n.to_string() }
            }
            None => short(a),
        }
    }

    fn mc(&mut self, out: &mut Vec<Out>, p: Pend, payload: Vec<u8>) {
        self.pending.push_back(p);
        out.push(Out::Mc(payload));
    }

    fn contact_by_prefix(&self, p: &[u8]) -> Option<&McContact> {
        self.contacts.values().find(|c| c.key.starts_with(p))
    }

    /// Full key in hex if the contact is known, otherwise the prefix.
    fn key_text(&self, p: &[u8; 6]) -> String {
        self.contact_by_prefix(p).map(|c| hex(&c.key)).unwrap_or_else(|| hex(p))
    }

    /// Key (64 hex) or prefix (≥ 12 hex) of a known contact.
    fn find_key(&self, s: &str) -> Option<[u8; 32]> {
        let b = unhex(s)?;
        if let Ok(k) = <[u8; 32]>::try_from(b.as_slice()) {
            return Some(k);
        }
        if b.len() < 6 {
            return None;
        }
        self.contact_by_prefix(&b).map(|c| c.key)
    }

    /// The name the MeshCore channel in the slot has in the bridge (`public`/`#name`).
    fn slot_key(&self, idx: u8) -> Option<String> {
        let (name, secret) = self.mc_slots.get(&idx)?;
        if name.is_empty() && secret.iter().all(|&b| b == 0) {
            return None;
        }
        Some(if *secret == PUBLIC_PSK { "public".into() } else { norm_meshcore(name) })
    }

    /// Builds `mc_idx` from the slots: public and hashtags with the right
    /// key first, then other channels by name.
    fn reindex(&mut self) {
        self.mc_idx.clear();
        let mut slots: Vec<_> = self.mc_slots.iter().map(|(i, v)| (*i, v.clone())).collect();
        slots.sort_by_key(|s| s.0);
        for verified in [true, false] {
            for (idx, (name, secret)) in &slots {
                let Some(key) = self.slot_key(*idx) else { continue };
                let ok = *secret == PUBLIC_PSK || (key.starts_with('#') && hashtag_secret(&key) == *secret);
                if ok == verified {
                    let _ = name;
                    self.mc_idx.entry(key).or_insert(*idx);
                }
            }
        }
    }

    pub fn state_json(&self) -> String {
        let mut contacts: Vec<&McContact> = self.contacts.values().collect();
        contacts.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        let contacts: Vec<Value> = contacts
            .into_iter()
            .map(|c| {
                let mut o = json!({
                    "key": hex(&c.key),
                    "name": c.name,
                    "type": kind_name(c.kind),
                    "last_advert": c.last_advert,
                    "path_len": hops(c.out_path_len).map(i32::from).unwrap_or(-1),
                });
                if c.lat != 0 || c.lon != 0 {
                    o["lat"] = json!(c.lat as f64 / 1e6);
                    o["lon"] = json!(c.lon as f64 / 1e6);
                }
                o
            })
            .collect();
        let mut idxs: Vec<u8> = self.mc_slots.keys().copied().filter(|i| self.slot_key(*i).is_some()).collect();
        idxs.sort();
        let channels: Vec<Value> = idxs
            .into_iter()
            .map(|i| {
                let key = self.slot_key(i).unwrap_or_default();
                let bridged = self.mc_idx.get(&key) == Some(&i);
                let bridge = self.maps.iter().find(|m| bridged && m.meshcore == key).map(|m| m.maarse.clone());
                json!({"idx": i, "name": self.mc_slots[&i].0, "bridge": bridge})
            })
            .collect();
        let mut v = json!({
            "ev": "state",
            "connected": self.connected,
            "port": self.port,
            "contacts": contacts,
            "channels": channels,
        });
        if self.connected && !self.me.name.is_empty() {
            v["self"] = json!({
                "name": self.me.name,
                "key": hex(&self.me.key),
                "freq": self.me.freq_khz as f64 / 1000.0,
                "bw": self.me.bw_hz as f64 / 1000.0,
                "sf": self.me.sf,
                "cr": self.me.cr,
                "tx_power": self.me.tx_power,
            });
        }
        v.to_string()
    }

    /// Called at startup: join the Må:rse channels.
    pub fn node_start(&self) -> Vec<Out> {
        let mut out = vec![Out::Node(Command::GetInfo)];
        for m in &self.maps {
            out.push(Out::Node(Command::JoinChannel { name: text(m.maarse.as_bytes()) }));
        }
        out
    }

    /// Called every time we (re)connect to the MeshCore node.
    pub fn mc_start(&mut self, now: u32) -> Vec<Out> {
        self.pending.clear();
        self.mc_slots.clear();
        self.mc_idx.clear();
        self.channels_ready = false;
        self.syncing = false;
        self.connected = true;
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

    /// Called when the connection to the MeshCore node is lost.
    pub fn mc_lost(&mut self) -> Vec<Out> {
        self.connected = false;
        self.pending.clear();
        vec![Out::Ui(self.state_json())]
    }

    /// Called regularly: fetch messages in case a MSG_WAITING was lost.
    pub fn poll(&mut self) -> Vec<Out> {
        let mut out = Vec::new();
        if !self.syncing && self.channels_ready {
            self.syncing = true;
            self.mc(&mut out, Pend::Cmd(CMD_SYNC_NEXT_MESSAGE), cmd_sync_next());
        }
        out
    }

    /// True if the message has not been bridged recently (and remembers it).
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
        // Known slots: public by key, hashtags by key or name.
        self.reindex();
        let mut free: Vec<u8> = self.mc_slots.keys().copied().filter(|i| self.slot_key(*i).is_none()).collect();
        free.sort();
        let wanted: Vec<String> = self.maps.iter().map(|m| m.meshcore.clone()).collect();
        for w in wanted {
            if self.mc_idx.contains_key(&w) {
                continue;
            }
            if free.is_empty() {
                out.push(Out::Log(format!("MeshCore: no free channel slot for {w}")));
                continue;
            }
            let idx = free.remove(0);
            let (name, secret) = if w == "public" {
                ("Public".to_string(), PUBLIC_PSK)
            } else {
                (w.clone(), hashtag_secret(&w))
            };
            out.push(Out::Log(format!("MeshCore: creating channel {name} in slot {idx}")));
            self.mc(out, Pend::Cmd(CMD_SET_CHANNEL), cmd_set_channel(idx, &name, &secret));
            self.mc_slots.insert(idx, (name, secret));
            self.mc_idx.insert(w, idx);
        }
        self.channels_ready = true;
        let list: Vec<String> = self.maps.iter().map(|m| format!("{} ⇄ {}", m.maarse, m.meshcore)).collect();
        out.push(Out::Log(format!("MeshCore: bridge ready ({})", list.join(", "))));
        // Fetch messages that were waiting while we connected.
        out.extend(self.poll());
    }

    /// Response to a command from the web client.
    fn ui_reply(&mut self, op: &'static str, eff: Effect, ev: &McEvent, out: &mut Vec<Out>) {
        match ev {
            McEvent::Ok | McEvent::Sent => {
                let changed = !matches!(eff, Effect::None);
                match eff {
                    Effect::None => {}
                    Effect::Slot(idx, name, secret) => {
                        self.mc_slots.insert(idx, (name, secret));
                        self.reindex();
                    }
                    Effect::DelContact(k) => {
                        self.contacts.remove(&k);
                    }
                }
                let ev = if matches!(op, "send" | "chan_send") { "sent" } else { "ok" };
                out.push(Out::Ui(json!({"ev":ev,"op":op}).to_string()));
                if changed {
                    out.push(Out::Ui(self.state_json()));
                }
            }
            McEvent::Err(e) => out.push(ui_err(op, &format!("MeshCore: {}", err_text(*e)))),
            _ => out.push(ui_err(op, "unexpected response from the MeshCore node")),
        }
    }

    pub fn on_mc(&mut self, frame: &[u8], now: u32) -> Vec<Out> {
        let mut out = Vec::new();
        let Some(ev) = decode_event(frame) else { return out };
        let code = frame[0];
        // Responses (code < 0x80) belong to the oldest waiting command; contacts
        // come as START, CONTACT … END, and only END completes.
        let pend = if code < 0x80 && !matches!(code, RESP_CONTACTS_START | RESP_CONTACT) {
            self.pending.pop_front()
        } else {
            None
        };
        if let Some(Pend::Ui(op, eff)) = &pend {
            self.ui_reply(op, eff.clone(), &ev, &mut out);
            return out;
        }
        match ev {
            McEvent::ChannelInfo { idx, name, secret } => {
                self.mc_slots.insert(idx, (name, secret));
            }
            McEvent::SelfInfo(me) => {
                out.push(Out::Log(format!("MeshCore: connected to '{}'", me.name)));
                self.me = me;
            }
            McEvent::DeviceInfo { fw_ver } if fw_ver < 3 => {
                out.push(Out::Log(format!("MeshCore: old firmware (v{fw_ver}), update companion_radio")));
            }
            McEvent::Contact { c, push } => {
                if self.lookup.remove(&c.key) || push {
                    out.push(Out::Ui(
                        json!({"ev":"advert","key":hex(&c.key),"name":c.name,"type":kind_name(c.kind),"ts":c.last_advert})
                            .to_string(),
                    ));
                }
                self.contacts.insert(c.key, c);
            }
            McEvent::EndOfContacts => self.lookup.clear(),
            McEvent::Trace { tag, snrs, last } => {
                if let Some((key, sent)) = self.pings.remove(&tag) {
                    let db = |s: i8| f64::from(s) / 4.0;
                    out.push(Out::Ui(
                        json!({"ev":"pong","key":hex(&key),"ms":sent.elapsed().as_millis() as u64,
                               "snrs":snrs.iter().map(|&s| db(s)).collect::<Vec<_>>(),"last":db(last)})
                        .to_string(),
                    ));
                }
            }
            McEvent::Advert { key } => {
                let mut ev = json!({"ev":"advert","key":hex(&key),"ts":now});
                if let Some(c) = self.contacts.get_mut(&key) {
                    c.last_advert = now;
                    ev["name"] = json!(c.name);
                    ev["type"] = json!(kind_name(c.kind));
                } else if self.lookup.insert(key) && self.lookup.len() == 1 {
                    // The firmware added the contact itself; fetch it for name and type.
                    self.mc(&mut out, Pend::Cmd(CMD_GET_CONTACTS), cmd_get_contacts());
                }
                out.push(Out::Ui(ev.to_string()));
            }
            McEvent::Err(e) => {
                if let Some(Pend::Cmd(c)) = pend
                    && c != CMD_SET_DEVICE_TIME
                {
                    out.push(Out::Log(format!("MeshCore: command {c} failed (code {e})")));
                }
                if matches!(pend, Some(Pend::Cmd(CMD_SYNC_NEXT_MESSAGE))) {
                    self.syncing = false;
                }
            }
            McEvent::MsgWaiting => out.extend(self.poll()),
            McEvent::NoMoreMessages => self.syncing = false,
            McEvent::ChannelMsg { idx, ts, text: t, snr4, path_len } => {
                self.syncing = false;
                out.extend(self.poll());
                let chan = self.mc_slots.get(&idx).map(|s| s.0.clone());
                out.push(Out::Ui(
                    json!({"ev":"msg","kind":"chan","idx":idx,"chan":chan,"text":t,"ts":ts,
                           "snr":snr4.map(|s| s as f64 / 4.0),"path_len":hops(path_len)})
                    .to_string(),
                ));
                self.mc_channel_msg(idx, &t, now, &mut out);
            }
            McEvent::ContactMsg { prefix, ts, text: t, txt_type, snr4, path_len } => {
                self.syncing = false;
                out.extend(self.poll());
                let key = self.key_text(&prefix);
                if txt_type == TXT_CLI {
                    out.push(Out::Ui(json!({"ev":"cli","key":key,"text":t}).to_string()));
                } else {
                    let name = self.contact_by_prefix(&prefix).map(|c| c.name.clone());
                    out.push(Out::Ui(
                        json!({"ev":"msg","kind":"dm","key":key,"name":name,"text":t,"ts":ts,
                               "snr":snr4.map(|s| s as f64 / 4.0),"path_len":hops(path_len)})
                        .to_string(),
                    ));
                    self.mc_direct_msg(prefix, &t, &mut out);
                }
            }
            McEvent::LoginOk { prefix, admin } => {
                out.push(Out::Ui(json!({"ev":"login","key":self.key_text(&prefix),"ok":true,"admin":admin}).to_string()));
            }
            McEvent::LoginFail { prefix } => {
                out.push(Out::Ui(json!({"ev":"login","key":self.key_text(&prefix),"ok":false,"admin":false}).to_string()));
            }
            McEvent::Status { prefix, st } => {
                let mut f = serde_json::Map::new();
                let mut put = |k: &str, v: Option<Value>| {
                    if let Some(v) = v {
                        f.insert(k.into(), v);
                    }
                };
                put("batt_mv", st.batt_mv.map(Value::from));
                put("tx_queue", st.tx_queue.map(Value::from));
                put("noise_floor", st.noise_floor.map(Value::from));
                put("last_rssi", st.last_rssi.map(Value::from));
                put("n_recv", st.n_recv.map(Value::from));
                put("n_sent", st.n_sent.map(Value::from));
                put("airtime_s", st.airtime_s.map(Value::from));
                put("uptime_s", st.uptime_s.map(Value::from));
                put("n_sent_flood", st.n_sent_flood.map(Value::from));
                put("n_sent_direct", st.n_sent_direct.map(Value::from));
                put("n_recv_flood", st.n_recv_flood.map(Value::from));
                put("n_recv_direct", st.n_recv_direct.map(Value::from));
                put("err_events", st.err_events.map(Value::from));
                put("last_snr", st.last_snr4.map(|s| Value::from(s as f64 / 4.0)));
                out.push(Out::Ui(json!({"ev":"status","key":self.key_text(&prefix),"fields":f}).to_string()));
            }
            McEvent::Other(RESP_CONTACT_MSG | RESP_CONTACT_MSG_V3 | RESP_CHANNEL_MSG | RESP_CHANNEL_MSG_V3) => {
                // Non-text (e.g. signed): skip, fetch the next.
                self.syncing = false;
                out.extend(self.poll());
            }
            _ => {}
        }
        if matches!(pend, Some(Pend::GetChannel)) && !self.pending.iter().any(|p| matches!(p, Pend::GetChannel)) {
            if self.channels_ready {
                self.reindex();
            } else {
                self.resolve_channels(&mut out);
            }
            out.push(Out::Ui(self.state_json()));
        }
        out
    }

    /// Command from a web client (JSON from `/mc`).
    pub fn on_ui(&mut self, v: &Value, now: u32) -> Vec<Out> {
        let mut out = Vec::new();
        let op = v["op"].as_str().unwrap_or("");
        let s = |k: &str| v[k].as_str().unwrap_or("");
        let idx = v["idx"].as_u64().and_then(|i| u8::try_from(i).ok());
        // The command's name, as 'static for Pend::Ui.
        const OPS: [&str; 10] =
            ["advert", "send", "chan_send", "chan_set", "chan_del", "contact_del", "login", "status", "cli", "ping"];
        let sop = OPS.iter().copied().find(|o| *o == op);
        match op {
            "state" => out.push(Out::Ui(self.state_json())),
            "bridge" => {
                let Some(key) = idx.and_then(|i| self.slot_key(i)) else {
                    out.push(ui_err(op, "unknown channel slot"));
                    return out;
                };
                let maarse = v["maarse"].as_str().filter(|m| !m.trim().is_empty()).map(norm_maarse);
                self.maps.retain(|m| m.meshcore != key);
                if let Some(m) = &maarse {
                    if !self.maarse_ch.values().any(|n| n == m) {
                        out.push(Out::Node(Command::JoinChannel { name: text(m.as_bytes()) }));
                    }
                    self.maps.push(Mapping { maarse: m.clone(), meshcore: key.clone() });
                    if let Some(i) = idx {
                        self.mc_idx.insert(key.clone(), i);
                    }
                }
                out.push(Out::Log(format!(
                    "MeshCore: bridge {key} ⇄ {}",
                    maarse.as_deref().unwrap_or("(none)")
                )));
                self.overrides.retain(|(k, _)| *k != key);
                self.overrides.push((key, maarse));
                if let Some(p) = &self.save_to
                    && let Err(e) = std::fs::write(p, format_overrides(&self.overrides))
                {
                    out.push(Out::Log(format!("MeshCore: could not save {}: {e}", p.display())));
                }
                out.push(Out::Ui(json!({"ev":"ok","op":"bridge"}).to_string()));
                out.push(Out::Ui(self.state_json()));
            }
            _ if !self.connected => out.push(ui_err(op, "The MeshCore node is not connected")),
            "refresh" => {
                self.mc_slots.clear();
                self.contacts.clear();
                self.mc(&mut out, Pend::Cmd(CMD_GET_CONTACTS), cmd_get_contacts());
                for i in 0..CHANNEL_SCAN {
                    self.mc(&mut out, Pend::GetChannel, cmd_get_channel(i));
                }
            }
            "advert" => {
                let flood = v["flood"].as_bool().unwrap_or(false);
                self.mc(&mut out, Pend::Ui("advert", Effect::None), cmd_self_advert(flood));
            }
            "send" | "cli" => {
                let Some(k) = self.find_key(s("to")).or_else(|| self.find_key(s("key"))) else {
                    out.push(ui_err(op, "unknown contact"));
                    return out;
                };
                let p: [u8; 6] = k[..6].try_into().unwrap();
                if op == "send" {
                    self.dm_route.remove(&p);
                    self.ui_dm.insert(p);
                    self.mc(&mut out, Pend::Ui("send", Effect::None), cmd_send_txt(&p, now, clip(s("text"), MC_MAX_TEXT)));
                } else {
                    self.mc(&mut out, Pend::Ui("cli", Effect::None), cmd_send_typed(TXT_CLI, &p, now, clip(s("cmd"), MC_MAX_TEXT)));
                }
            }
            "chan_send" => {
                let Some(i) = idx.filter(|i| self.slot_key(*i).is_some()) else {
                    out.push(ui_err(op, "unknown channel slot"));
                    return out;
                };
                // The firmware prefixes "<node name>: " (BaseChatMesh.cpp line 523).
                let room = MC_MAX_TEXT.saturating_sub(self.me.name.len() + 2);
                self.mc(&mut out, Pend::Ui("chan_send", Effect::None), cmd_send_channel(i, now, clip(s("text"), room)));
            }
            "chan_set" => {
                let name = s("name").trim().to_string();
                let Some(i) = idx.filter(|i| *i < CHANNEL_SCAN) else {
                    out.push(ui_err(op, "invalid channel slot"));
                    return out;
                };
                let secret: Option<[u8; 16]> = match v["secret"].as_str().filter(|x| !x.is_empty()) {
                    Some(h) => match unhex(h).and_then(|b| b.try_into().ok()) {
                        Some(k) => Some(k),
                        None => {
                            out.push(ui_err(op, "the key must be 32 hex characters"));
                            return out;
                        }
                    },
                    None if name.starts_with('#') => Some(hashtag_secret(&norm_meshcore(&name))),
                    None if name.eq_ignore_ascii_case("public") => Some(PUBLIC_PSK),
                    None => None,
                };
                let (Some(secret), false) = (secret, name.is_empty()) else {
                    out.push(ui_err(op, "give a name and a key (or a name with #)"));
                    return out;
                };
                self.mc(&mut out, Pend::Ui("chan_set", Effect::Slot(i, name.clone(), secret)), cmd_set_channel(i, &name, &secret));
            }
            "chan_del" => {
                let Some(i) = idx.filter(|i| *i < CHANNEL_SCAN) else {
                    out.push(ui_err(op, "invalid channel slot"));
                    return out;
                };
                self.mc(&mut out, Pend::Ui("chan_del", Effect::Slot(i, String::new(), [0; 16])), cmd_set_channel(i, "", &[0; 16]));
            }
            "contact_del" | "login" | "status" => {
                let Some(k) = self.find_key(s("key")) else {
                    out.push(ui_err(op, "unknown contact"));
                    return out;
                };
                let (eff, cmd) = match op {
                    "contact_del" => (Effect::DelContact(k), cmd_remove_contact(&k)),
                    "login" => (Effect::None, cmd_login(&k, s("password"))),
                    _ => (Effect::None, cmd_status_req(&k)),
                };
                self.mc(&mut out, Pend::Ui(sop.unwrap_or("?"), eff), cmd);
            }
            "ping" => {
                let Some(c) = self.find_key(s("key")).and_then(|k| self.contacts.get(&k)) else {
                    out.push(ui_err(op, "unknown contact"));
                    return out;
                };
                let Some(path) = ping_path(c) else {
                    out.push(ui_err(op, "no known route to the contact yet (wait for an advert or a reply)"));
                    return out;
                };
                let key = c.key;
                let tag = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH))
                    .map(|d| d.subsec_nanos() ^ d.as_secs() as u32)
                    .unwrap_or(0);
                self.pings.retain(|_, (_, t)| t.elapsed().as_secs() < 120);
                self.pings.insert(tag, (key, std::time::Instant::now()));
                self.mc(&mut out, Pend::Ui("ping", Effect::None), cmd_trace(tag, &path));
            }
            _ => out.push(ui_err(op, "unknown command")),
        }
        out
    }

    fn mc_channel_msg(&mut self, idx: u8, t: &str, now: u32, out: &mut Vec<Out>) {
        let Some(mc) = self.mc_idx.iter().find(|(_, i)| **i == idx).map(|(n, _)| n.clone()) else {
            return;
        };
        // Channel text is "<sender>: <message>" (BaseChatMesh.cpp line 523).
        let (from, body) = t.split_once(": ").unwrap_or(("?", t));
        if body.starts_with("[Må ") || body.starts_with("[MC ") || from == self.me.name {
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
        let name = self.contact_by_prefix(&prefix).map(|c| c.name.clone()).unwrap_or_else(|| hex(&prefix));
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
            // The web client is having the conversation with the contact: no help text.
            None if self.ui_dm.contains(&prefix) => {}
            None => {
                let me = self.my_addr.map(|a| a.to_string()).unwrap_or_default();
                let help = format!("Må:rse gateway {me}. Write: @<Må:rse address, 32 hex chars> message");
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
            Event::NodeInfo { addr, name, .. } => match name {
                Some(n) => {
                    self.names.insert(addr, String::from_utf8_lossy(&n).into_owned());
                }
                None => {
                    self.names.remove(&addr);
                }
            },
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
                    // The firmware prefixes "<node name>: " (BaseChatMesh.cpp line 523).
                    let room = MC_MAX_TEXT.saturating_sub(self.me.name.len() + 2);
                    let msg = format!("[Må {}] {body}", self.label(&from));
                    self.mc(&mut out, Pend::Cmd(CMD_SEND_CHANNEL_TXT_MSG), cmd_send_channel(idx, now, clip(&msg, room)));
                }
            }
            Event::DirectReceived { from, text: t, .. } => {
                // Only "@Name text" is for the bridge; everything else is for the host's own user.
                let s = String::from_utf8_lossy(&t).into_owned();
                let Some(rest) = s.trim().strip_prefix('@') else { return out };
                let lower = rest.to_lowercase();
                // The longest contact name the text starts with (names can contain spaces).
                let hit = self
                    .contacts
                    .values()
                    .filter(|c| {
                        let n = c.name.to_lowercase();
                        !n.is_empty()
                            && lower.starts_with(&n)
                            && lower[n.len()..].chars().next().is_none_or(|c| c == ' ')
                    })
                    .max_by_key(|c| c.name.len())
                    .map(|c| (<[u8; 6]>::try_from(&c.key[..6]).unwrap(), c.name.clone()));
                match hit {
                    Some((p, n)) => {
                        let body = rest[n.len()..].trim();
                        let msg = format!("[Må {}] {body}", self.label(&from));
                        self.dm_route.insert(p, from);
                        self.mc(&mut out, Pend::Cmd(CMD_SEND_TXT_MSG), cmd_send_txt(&p, now, clip(&msg, MC_MAX_TEXT)));
                    }
                    None => {
                        let first = rest.split(' ').next().unwrap_or("");
                        let m = format!("MeshCore gateway: no known contact '{first}'");
                        out.push(Out::Node(Command::SendDirect { req: 0, dest: from, text: text(m.as_bytes()) }));
                    }
                }
            }
            _ => {}
        }
        out
    }
}

// ---------- web clients (/mc) ----------

/// The connection between the MeshCore thread and the web clients on `/mc`.
struct Hub {
    /// One sender per connected web client (JSON text).
    subs: Mutex<Vec<Sender<String>>>,
    /// Commands for the MeshCore thread; `None` without `--meshcore`.
    cmd: Mutex<Option<Sender<String>>>,
}

static HUB: Hub = Hub { subs: Mutex::new(Vec::new()), cmd: Mutex::new(None) };

/// New web client: receives all events from now on.
pub fn subscribe() -> Receiver<String> {
    let (tx, rx) = channel();
    HUB.subs.lock().unwrap_or_else(|e| e.into_inner()).push(tx);
    rx
}

/// Sends a JSON command to the MeshCore thread. False without `--meshcore`.
pub fn command(json: String) -> bool {
    HUB.cmd.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|t| t.send(json).is_ok())
}

/// `state` when the host runs without `--meshcore`.
pub fn disabled_state() -> String {
    json!({"ev":"state","connected":false,"enabled":false,"contacts":[],"channels":[]}).to_string()
}

fn broadcast(s: &str) {
    HUB.subs.lock().unwrap_or_else(|e| e.into_inner()).retain(|t| t.send(s.to_string()).is_ok());
}

// ---------- I/O ----------

fn unix_now() -> u32 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as u32).unwrap_or(0)
}

/// How often we ask for messages in case a MSG_WAITING was lost.
const POLL_EVERY: Duration = Duration::from_secs(15);

struct Ctx<'a> {
    tx: &'a Sender<In>,
    id: u32,
    ui: &'a Receiver<String>,
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
                Out::Log(s) => {
                    println!("  {s}");
                    broadcast(&json!({"ev":"log","text":s}).to_string());
                }
                Out::Ui(s) => broadcast(&s),
            }
        }
        Ok(())
    }

    /// Commands from the web clients.
    fn ui_commands(&self, bridge: &mut Bridge) -> Vec<Out> {
        let mut outs = Vec::new();
        while let Ok(s) = self.ui.try_recv() {
            match serde_json::from_str::<Value>(&s) {
                Ok(v) => outs.extend(bridge.on_ui(&v, unix_now())),
                Err(_) => outs.push(ui_err("?", "invalid JSON")),
            }
        }
        outs
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

/// One connection to the MeshCore node. Returns when the port fails.
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
        let outs = ctx.ui_commands(bridge);
        ctx.dispatch(Some(port), outs)?;
        if last_poll.elapsed() >= POLL_EVERY {
            last_poll = Instant::now();
            let outs = bridge.poll();
            ctx.dispatch(Some(port), outs)?;
        }
    }
}

/// The file with bridge choices from the web client.
pub fn overrides_file(data_dir: &Path) -> PathBuf {
    data_dir.join("meshcore-bridges.txt")
}

/// Starts the bridge in a thread. Reconnects if the port disappears.
pub fn spawn(port: String, baud: u32, maps: Vec<Mapping>, data_dir: PathBuf, tx: Sender<In>) {
    let (ui_tx, ui_rx) = channel();
    *HUB.cmd.lock().unwrap_or_else(|e| e.into_inner()) = Some(ui_tx);
    std::thread::spawn(move || {
        let id = next_id();
        let (ctx_tx, crx) = channel();
        if tx.send(In::ClientUp { id, tx: ctx_tx }).is_err() {
            return;
        }
        let ctx = Ctx { tx: &tx, id, ui: &ui_rx };
        let mut bridge = Bridge::new(maps);
        bridge.port = port.clone();
        let file = overrides_file(&data_dir);
        if let Ok(s) = std::fs::read_to_string(&file) {
            bridge.apply_overrides(parse_overrides(&s));
        }
        bridge.save_to = Some(file);
        let mut ndec = FrameDecoder::new();
        let _ = ctx.dispatch(None, bridge.node_start());
        loop {
            match serialport::new(&port, baud).timeout(Duration::from_millis(50)).open() {
                Ok(mut sp) => {
                    let _ = sp.write_data_terminal_ready(true);
                    let r = session(&mut sp, &mut bridge, &crx, &mut ndec, &ctx);
                    eprintln!("  MeshCore: {port} lost ({r:?}), retrying");
                }
                Err(e) => eprintln!("  MeshCore: cannot open {port}: {e}"),
            }
            let _ = ctx.dispatch(None, bridge.mc_lost());
            // Keep up with Må:rse channels and web clients while we wait.
            let until = Instant::now() + Duration::from_secs(5);
            while Instant::now() < until {
                for ev in node_events(&crx, &mut ndec) {
                    let _ = bridge.on_node(ev, unix_now());
                }
                let outs = ctx.ui_commands(&mut bridge);
                let _ = ctx.dispatch(None, outs);
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

    /// RESP_CODE_CHANNEL_MSG_RECV_V3 frame as in onChannelMessageRecv
    /// (MyMesh.cpp lines 559-578): code, snr*4, 0, 0, idx, path_len, txt_type, ts, text.
    fn chan_frame(idx: u8, ts: u32, t: &str) -> Vec<u8> {
        let mut f = vec![RESP_CHANNEL_MSG_V3, 0x28, 0, 0, idx, 2, TXT_PLAIN];
        f.extend_from_slice(&ts.to_le_bytes());
        f.extend_from_slice(t.as_bytes());
        f
    }

    /// RESP_CODE_CHANNEL_INFO (MyMesh.cpp lines 1756-1762).
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
        // '<' (0x3c), length LSB, MSB (ArduinoSerialInterface.cpp lines 46-57).
        assert_eq!(encode_frame(&[0x16, 0x03]), vec![0x3c, 0x02, 0x00, 0x16, 0x03]);
        // '>' (0x3e) from the node (lines 31-33); garbage before the frame is ignored.
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
            cmd_send_channel(1, 0x6553f100, "hi!"),
            vec![3, 0, 1, 0x00, 0xf1, 0x53, 0x65, b'h', b'i', b'!']
        );
        let p = [1, 2, 3, 4, 5, 6];
        assert_eq!(cmd_send_txt(&p, 1, "x"), vec![2, 0, 0, 1, 0, 0, 0, 1, 2, 3, 4, 5, 6, b'x']);
        let s = cmd_set_channel(3, "#test", &hashtag_secret("#test"));
        assert_eq!(s.len(), 2 + 32 + 16); // MyMesh.cpp line 1769
        assert_eq!(&s[2..7], b"#test");
        assert_eq!(s[7], 0);
        assert_eq!(s[34..36], [0x9c, 0xd8]);
    }

    #[test]
    fn public_psk_is_base64_of_firmware_constant() {
        // "izOH6cXN6mrJ5e26oRXNcg==" decoded with base64.
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
        assert!(matches!(
            decode_event(&chan_frame(0, 0x11223344, "Alice: hi!")),
            Some(McEvent::ChannelMsg { idx: 0, ts: 0x11223344, ref text, .. }) if text == "Alice: hi!"
        ));
        // v<3 variant (RESP_CODE_CHANNEL_MSG_RECV): no snr/reserved bytes.
        let mut old = vec![RESP_CHANNEL_MSG, 1, 0xff, TXT_PLAIN, 1, 0, 0, 0];
        old.extend_from_slice(b"Bo: hi");
        assert_eq!(
            decode_event(&old),
            Some(McEvent::ChannelMsg { idx: 1, ts: 1, text: "Bo: hi".into(), snr4: None, path_len: 0xff })
        );
        // RESP_CODE_CONTACT_MSG_RECV_V3 (MyMesh.cpp lines 440-459).
        let mut dm = vec![RESP_CONTACT_MSG_V3, 0, 0, 0, 9, 8, 7, 6, 5, 4, 0xff, TXT_PLAIN, 5, 0, 0, 0];
        dm.extend_from_slice(b"yo");
        assert_eq!(
            decode_event(&dm),
            Some(McEvent::ContactMsg {
                prefix: [9, 8, 7, 6, 5, 4],
                ts: 5,
                text: "yo".into(),
                txt_type: TXT_PLAIN,
                snr4: Some(0),
                path_len: 0xff
            })
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
        assert!(matches!(
            decode_event(&c),
            Some(McEvent::Contact { ref c, push: false }) if c.key == [0xaa; 32] && c.name == "Alice"
        ));
        // RESP_CODE_SELF_INFO: the name starts at byte 58.
        let mut si = vec![RESP_SELF_INFO];
        si.extend_from_slice(&[0; 57]);
        si.extend_from_slice(b"Gw");
        assert!(matches!(decode_event(&si), Some(McEvent::SelfInfo(ref m)) if m.name == "Gw"));
        assert_eq!(decode_event(&[PUSH_MSG_WAITING]), Some(McEvent::MsgWaiting));
        assert_eq!(decode_event(&[RESP_ERR, 2]), Some(McEvent::Err(2)));
        assert_eq!(decode_event(&[]), None);
        assert_eq!(decode_event(&[RESP_CHANNEL_INFO, 1]), None);
    }

    #[test]
    fn mapping_parse() {
        assert_eq!(parse_mapping("public=public").unwrap(), Mapping { maarse: "public".into(), meshcore: "public".into() });
        assert_eq!(parse_mapping("Aarhus").unwrap(), Mapping { maarse: "#aarhus".into(), meshcore: "#aarhus".into() });
        assert_eq!(parse_mapping("#dk=#Denmark").unwrap(), Mapping { maarse: "#dk".into(), meshcore: "#denmark".into() });
        assert!(parse_mapping("=").is_err());
    }

    /// A bridge ready with public ⇄ public (slot 0) and #test ⇄ #test, which
    /// must be created in the free slot 1.
    fn ready_bridge() -> (Bridge, Vec<Out>) {
        let mut b = Bridge::new(vec![parse_mapping("public=public").unwrap(), parse_mapping("#test").unwrap()]);
        b.on_node(Event::Info { addr: addr(9), repeater: false, version: 2, max_text: 240, max_file: 0, identity: None }, 0);
        b.on_node(Event::ChannelJoined { channel: addr(1), name: text(b"public") }, 0);
        b.on_node(Event::ChannelJoined { channel: addr(2), name: text(b"#test") }, 0);
        b.mc_start(100);
        let mut si = vec![RESP_DEVICE_INFO, 8];
        si.extend_from_slice(&[0; 10]);
        b.on_mc(&si, 100);
        let mut si = vec![RESP_SELF_INFO];
        si.extend_from_slice(&[0; 57]);
        si.extend_from_slice(b"Gw");
        b.on_mc(&si, 100);
        b.on_mc(&[RESP_ERR, 6], 100); // time
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
        let out = b.on_mc(&chan_frame(0, 1, "Alice: hi!"), 200);
        assert_eq!(sent_texts(&out), vec!["[MC Alice] hi!"]);
        let wire: Vec<&Out> = out.iter().filter(|o| !matches!(o, Out::Ui(_))).collect();
        assert!(matches!(&wire[..], [Out::Mc(_), Out::Node(Command::SendChannel { channel, .. })] if *channel == addr(1)));
        // The same message again (e.g. via another gateway) is not bridged.
        assert!(sent_texts(&b.on_mc(&chan_frame(0, 2, "Alice: hi!"), 210)).is_empty());
        // Already bridged text is not sent back.
        assert!(sent_texts(&b.on_mc(&chan_frame(0, 3, "Gw2: [Må 01020304] x"), 220)).is_empty());
        // Unknown channel slot is ignored.
        assert!(sent_texts(&b.on_mc(&chan_frame(5, 3, "Bo: y"), 220)).is_empty());
        // The #test channel in slot 1 goes to Må:rse #test.
        let out = b.on_mc(&chan_frame(1, 4, "Bo: check"), 230);
        assert!(out.iter().any(|o| matches!(o, Out::Node(Command::SendChannel { channel, .. }) if *channel == addr(2))));

        // Må:rse -> MeshCore.
        let ev = |t: &str, from: Address| Event::ChannelReceived { channel: addr(1), from, hops: 1, text: text(t.as_bytes()) };
        let out = b.on_node(ev("hi again", addr(0x1a)), 300);
        assert_eq!(sent_texts(&out), vec!["[Må 1a1a1a1a] hi again"]);
        assert!(matches!(&out[0], Out::Mc(p) if p[2] == 0 && p[3..7] == 300u32.to_le_bytes()));
        // Our own bridged message (from another gateway) does not go back.
        assert!(b.on_node(ev("[MC Alice] hi!", addr(0x1b)), 310).is_empty());
        // A repeat within DEDUP_SECS is dropped, after that it is ok.
        assert!(b.on_node(ev("hi again", addr(0x1a)), 320).is_empty());
        assert_eq!(sent_texts(&b.on_node(ev("hi again", addr(0x1a)), 300 + DEDUP_SECS + 1)).len(), 1);
        // Long text is truncated so "Gw: " + text is at most MC_MAX_TEXT.
        let long = "æ".repeat(200);
        let out = b.on_node(ev(&long, addr(0x1a)), 1000);
        let s = &sent_texts(&out)[0];
        assert!(s.len() + "Gw: ".len() <= MC_MAX_TEXT);
    }

    #[test]
    fn display_names() {
        let (mut b, _) = ready_bridge();
        let ev = |t: &str, from: Address| Event::ChannelReceived { channel: addr(1), from, hops: 1, text: text(t.as_bytes()) };
        let named = |n: Option<&str>| Event::NodeInfo {
            addr: addr(0x1a),
            hops: 1,
            position: None,
            telemetry: None,
            name: n.and_then(|n| maarse_core::appdata::clean_name(n.as_bytes())),
            caps: None,
        };
        b.on_node(named(Some("Dennis")), 100);
        assert_eq!(sent_texts(&b.on_node(ev("hi!", addr(0x1a)), 110)), vec!["[Må Dennis] hi!"]);
        // Brackets in the name are removed so it cannot imitate a prefix.
        b.on_node(named(Some("[MC x]")), 120);
        assert_eq!(sent_texts(&b.on_node(ev("two", addr(0x1a)), 130)), vec!["[Må MC x] two"]);
        // Name removed: back to the address.
        b.on_node(named(None), 140);
        assert_eq!(sent_texts(&b.on_node(ev("three", addr(0x1a)), 150)), vec!["[Må 1a1a1a1a] three"]);
    }

    #[test]
    fn direct_messages() {
        let (mut b, _) = ready_bridge();
        // A Må:rse user writes "@Alice Mob hi!" to the gateway.
        let from = addr(0x33);
        let out = b.on_node(Event::DirectReceived { from, hops: 1, text: text(b"@alice mob hi!") }, 50);
        assert_eq!(out, vec![Out::Mc(cmd_send_txt(&[0xaa; 6], 50, "[Må 33333333] hi!"))]);
        // The reply goes back to the same Må:rse user.
        let mut dm = vec![RESP_CONTACT_MSG_V3, 0, 0, 0, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xff, TXT_PLAIN, 5, 0, 0, 0];
        dm.extend_from_slice(b"reply");
        let out = b.on_mc(&dm, 60);
        assert!(out.contains(&Out::Node(Command::SendDirect { req: 0, dest: from, text: text(b"[MC Alice Mob] reply") })));
        // An unknown contact gives an error back to the sender.
        let out = b.on_node(Event::DirectReceived { from, hops: 1, text: text(b"@Unknown hi!") }, 70);
        assert!(matches!(&out[..], [Out::Node(Command::SendDirect { dest, .. })] if *dest == from));
        // Ordinary DMs to the host are not touched.
        assert!(b.on_node(Event::DirectReceived { from, hops: 1, text: text(b"hi!") }, 80).is_empty());
    }

    /// Fake serial port: yields the prepared bytes and stores what is written.
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
        // The node tells us about the channel "public".
        ctx_tx.send(Event::ChannelJoined { channel: addr(1), name: text(b"public") }.encode().to_vec()).unwrap();
        let mut ndec = FrameDecoder::new();
        for e in node_events(&crx, &mut ndec) {
            bridge.on_node(e, 0);
        }
        // Responses from MeshCore: device info, self info, ok (time), contacts, channels, message.
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
        frames.push(chan_frame(0, 1, "Alice: hi! from MeshCore"));
        frames.push(vec![RESP_NO_MORE_MESSAGES]);
        let mut wire = Vec::new();
        for f in &frames {
            wire.push(b'>');
            wire.extend_from_slice(&(f.len() as u16).to_le_bytes());
            wire.extend_from_slice(f);
        }
        // Split the stream into small pieces, like a real serial port.
        let mut port = MockPort { input: wire.chunks(7).map(|c| c.to_vec()).collect(), written: Vec::new() };
        let (_ui_tx, ui_rx) = channel::<String>();
        let ctx = Ctx { tx: &tx, id: 42, ui: &ui_rx };
        let r = session(&mut port, &mut bridge, &crx, &mut ndec, &ctx);
        assert_eq!(r.unwrap_err().kind(), ErrorKind::BrokenPipe);
        // The first thing written is CMD_DEVICE_QUERY in a '<' frame.
        assert_eq!(port.written[..5], [b'<', 2, 0, CMD_DEVICE_QUERY, APP_TARGET_VER]);
        // The message has been sent to the Må:rse channel with a prefix.
        let sent: Vec<Command> = rx
            .try_iter()
            .filter_map(|m| match m {
                In::ClientFrame { id: 42, frame } => Command::decode(&frame).ok(),
                _ => None,
            })
            .collect();
        assert!(sent.contains(&Command::SendChannel { req: 0, channel: addr(1), text: text(b"[MC Alice] hi! from MeshCore") }));
        // And we have asked for the next message (CMD_SYNC_NEXT_MESSAGE).
        assert!(port.written.windows(4).any(|w| w == [b'<', 1, 0, CMD_SYNC_NEXT_MESSAGE]));
    }
}

#[cfg(test)]
mod ping_tests {
    use super::*;

    #[test]
    fn trace_frame_og_sti() {
        // 2 hops with 1-byte hash: [0x89, res, plen=2, flags=0, tag, auth, AA BB, snr1 snr2, last]
        let mut f = vec![PUSH_TRACE_DATA, 0, 2, 0];
        f.extend_from_slice(&0x1234_5678u32.to_le_bytes());
        f.extend_from_slice(&[0; 4]);
        f.extend_from_slice(&[0xAA, 0xBB, 40, (-8i8) as u8, 22]);
        assert_eq!(decode_event(&f), Some(McEvent::Trace { tag: 0x1234_5678, snrs: vec![40, -8], last: 22 }));

        let mut c = McContact {
            key: [0x7E; 32],
            kind: 2,
            out_path_len: 2,
            out_path: vec![0x11, 0x22],
            name: "r".into(),
            last_advert: 0,
            lat: 0,
            lon: 0,
        };
        assert_eq!(ping_path(&c), Some(vec![0x11, 0x22, 0x7E, 0x22, 0x11]));
        c.out_path_len = 0;
        assert_eq!(ping_path(&c), Some(vec![0x7E]));
        c.out_path_len = PATH_UNKNOWN;
        assert_eq!(ping_path(&c), None);
        assert_eq!(&cmd_trace(1, &[0x7E])[..], &[CMD_SEND_TRACE_PATH, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0x7E]);
    }
}
