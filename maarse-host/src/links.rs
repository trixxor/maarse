//! Packet links: TCP backbone and serial LoRa modem (serial links, KISS and
//! UDP are in `extra_links`).
//!
//! TCP: every packet is sent as `length (u16 LE) | packet`.
//!
//! Region hello over TCP: right after connecting, each side sends a
//! "packet" `0xFF 'R' 'G' | strict (1) | count (1) | region id (u16 BE) …`
//! with the regions it serves (see `maarse_core::region`). It is never
//! decoded as a Må:rse packet (version 3), so older hosts just count it
//! as invalid. After that we only send floods with a region to peers that
//! serve the region. Peers without a hello get everything (as before).
//!
//! Serial modem mode (requires firmware that supports it):
//! * host -> node: `length | 0x40 | packet`         (send the packet over LoRa)
//! * node -> host: `length | 0xC0 | snr (i8) | packet` (received over LoRa)
//!
//! LoRa modem over Wi-Fi: an ESP32 node in Wi-Fi modem mode connects to the
//! backbone port and sends `MM` first ([`MODEM_HELLO`]). Then:
//! * node -> host: `length (u16 LE) | snr (i8) | packet` (heard over LoRa)
//! * host -> node: `length (u16 LE) | packet`           (to send over LoRa)
//! The link is named `wifi-modem <address>` and, like a serial modem, counts
//! as LoRa (not `tcp …`).
//!
//! Every link is also kept in a small registry for the device list in the
//! web client (`/api/devices`, see [`devices_json`]).

use crate::{In, LinkSender, next_id};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Mutex;
use std::sync::mpsc::{Sender, channel, sync_channel};
use std::thread;
use std::time::{Duration, Instant};
use maarse_core::MAX_PACKET;
use maarse_core::companion::FrameDecoder;

/// SNR we report for packets over TCP (a "perfect" link).
pub(crate) const TCP_SNR: i8 = 10;

/// First two bytes from an ESP32 node in Wi-Fi modem mode.
pub const MODEM_HELLO: [u8; 2] = *b"MM";

/// Start of a firmware-update command to a Wi-Fi modem (see `fwupdate.rs`).
const OTA_MARK: [u8; 3] = [0xFF, b'O', b'T'];

// ---------- device registry ----------

/// One link as shown in the web client's device list.
struct LinkInfo {
    id: u32,
    name: String,
    /// The far end of a TCP link, and a handle to close it (dashboard).
    peer: Option<std::net::SocketAddr>,
    kill: Option<TcpStream>,
    /// `backbone`, `usb-modem`, `wifi-modem`, `serial-link`, `kiss`, `kiss-tcp` or `udp`.
    kind: &'static str,
    since: u64,
    rx: u64,
    tx: u64,
    last_rx: u64,
    last_snr: Option<i8>,
    /// The radio's own counters (`Event::Stats` frames), merged by key.
    stats: Vec<(u8, i32)>,
}

static REGISTRY: Mutex<Vec<LinkInfo>> = Mutex::new(Vec::new());

/// `--radio-quiet`: the radio's buzzer (RAK WisMesh Tag) stays silent for new messages.
pub static RADIO_QUIET: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A serial port the host is told to use, and why it is not open right now (if so).
static PORTS: Mutex<Vec<(String, Option<String>)>> = Mutex::new(Vec::new());

fn port_state(name: &str, error: Option<String>) {
    let mut p = PORTS.lock().unwrap_or_else(|e| e.into_inner());
    match p.iter_mut().find(|(n, _)| n == name) {
        Some(e) => e.1 = error,
        None => p.push((name.to_string(), error)),
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn with_registry<T>(f: impl FnOnce(&mut Vec<LinkInfo>) -> T) -> T {
    f(&mut REGISTRY.lock().unwrap_or_else(|e| e.into_inner()))
}

pub(crate) fn register(id: u32, name: &str, kind: &'static str) {
    let now = unix_now();
    with_registry(|r| {
        r.push(LinkInfo { id, name: name.to_string(), peer: None, kill: None, kind, since: now, rx: 0, tx: 0, last_rx: 0, last_snr: None, stats: Vec::new() })
    });
}

/// Registers a TCP link with its far end and a handle to close it.
fn register_tcp(id: u32, name: &str, kind: &'static str, stream: &TcpStream) {
    register(id, name, kind);
    let peer = stream.peer_addr().ok();
    let kill = stream.try_clone().ok();
    with_registry(|r| {
        if let Some(l) = r.iter_mut().find(|l| l.id == id) {
            l.peer = peer;
            l.kill = kill;
        }
    });
}

/// Closes a link (dashboard); its far end's IP address if it was TCP.
pub fn disconnect(id: u32) -> Option<std::net::IpAddr> {
    with_registry(|r| {
        let l = r.iter_mut().find(|l| l.id == id)?;
        if let Some(s) = l.kill.take() {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
        l.peer.map(|p| p.ip())
    })
}

pub(crate) fn unregister(id: u32) {
    with_registry(|r| r.retain(|l| l.id != id));
}

pub(crate) fn note_rx(id: u32, snr: Option<i8>) {
    let now = unix_now();
    with_registry(|r| {
        if let Some(l) = r.iter_mut().find(|l| l.id == id) {
            l.rx += 1;
            l.last_rx = now;
            if snr.is_some() {
                l.last_snr = snr;
            }
        }
    });
}

pub(crate) fn note_tx(id: u32) {
    with_registry(|r| {
        if let Some(l) = r.iter_mut().find(|l| l.id == id) {
            l.tx += 1;
        }
    });
}

/// Keeps the radio's own counters (GPS, Bluetooth, USB, battery ...) from a modem link.
pub(crate) fn note_modem_stats(id: u32, items: &[(u8, i32)]) {
    with_registry(|r| {
        if let Some(l) = r.iter_mut().find(|l| l.id == id) {
            // The firmware sends several frames; a frame with the uptime starts a new round.
            if items.iter().any(|(k, _)| *k == 1) {
                l.stats.retain(|(k, _)| matches!(*k, 60..=70 | 93..));
            }
            for (k, v) in items {
                match l.stats.iter_mut().find(|(x, _)| x == k) {
                    Some(e) => e.1 = *v,
                    None => l.stats.push((*k, *v)),
                }
            }
        }
    });
}

/// What the status page needs about the LoRa radios: how many are connected, how long since
/// one last sent anything, and the first one's own counters (the radio's keys, never the host's).
pub fn radio_summary() -> (usize, Option<u64>, Vec<(u8, i32)>) {
    let now = unix_now();
    with_registry(|r| {
        let radios: Vec<&LinkInfo> = r.iter().filter(|l| matches!(l.kind, "usb-modem" | "wifi-modem" | "ble-modem")).collect();
        let idle = radios.iter().filter(|l| l.last_rx != 0).map(|l| now.saturating_sub(l.last_rx)).min();
        let stats = radios.first().map(|l| l.stats.clone()).unwrap_or_default();
        (radios.len(), idle, stats)
    })
}

/// All links right now, as JSON for the web client.
pub fn devices_json() -> String {
    let now = unix_now();
    let list: Vec<serde_json::Value> = with_registry(|r| {
        r.iter()
            .map(|l| {
                serde_json::json!({
                    "id": l.id,
                    "peer": l.peer.map(|p| p.to_string()),
                    "can_close": l.kill.is_some(),
                    "name": l.name,
                    "kind": l.kind,
                    "up_s": now.saturating_sub(l.since),
                    "rx": l.rx,
                    "tx": l.tx,
                    "last_rx_s": if l.last_rx == 0 { None } else { Some(now.saturating_sub(l.last_rx)) },
                    "snr": l.last_snr,
                })
            })
            .collect()
    });
    // Ports that are set up but not open: the web client says so, with the reason.
    let open: Vec<String> = with_registry(|r| r.iter().map(|l| l.name.clone()).collect());
    let down: Vec<serde_json::Value> = PORTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|(n, _)| !open.contains(n))
        .map(|(n, e)| serde_json::json!({ "name": n, "error": e }))
        .collect();
    serde_json::json!({ "devices": list, "down": down }).to_string()
}

/// Start of a region hello over TCP.
pub const REGION_HELLO: [u8; 3] = [0xFF, b'R', b'G'];

/// Encodes a region hello.
pub fn region_hello(strict: bool, regions: &[u16]) -> Vec<u8> {
    let mut v = REGION_HELLO.to_vec();
    v.push(strict as u8);
    v.push(regions.len() as u8);
    for r in regions {
        v.extend_from_slice(&r.to_be_bytes());
    }
    v
}

/// Decodes a region hello: (strict, regions).
/// A region hello with our capabilities after the regions (older peers
/// ignore the extra bytes).
pub fn region_hello_with_caps(strict: bool, regions: &[u16], caps: &crate::dht::Caps) -> Vec<u8> {
    let mut v = region_hello(strict, regions);
    caps.encode(&mut v);
    v
}

/// The capabilities after a region hello's regions, if the peer sent them.
pub fn parse_hello_caps(data: &[u8]) -> Option<crate::dht::Caps> {
    let rest = data.strip_prefix(&REGION_HELLO)?;
    let n = *rest.get(1)? as usize;
    crate::dht::Caps::decode(rest.get(2 + 2 * n..)?)
}

pub fn parse_region_hello(data: &[u8]) -> Option<(bool, Vec<u16>)> {
    let rest = data.strip_prefix(&REGION_HELLO)?;
    let (&strict, rest) = rest.split_first()?;
    let (&n, rest) = rest.split_first()?;
    let ids = rest.get(..2 * n as usize)?;
    let regions = ids
        .chunks_exact(2)
        .map(|c| u16::from_be_bytes([c[0], c[1]]))
        .take(maarse_core::region::MAX_REGIONS)
        .collect();
    Some((strict != 0, regions))
}

pub fn spawn_tcp_listener(addr: SocketAddr, tx: Sender<In>) -> std::io::Result<()> {
    let listener = crate::settings::bind(addr)?;
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let tx = tx.clone();
            // Limits and the block list (guard.rs) before anything is read.
            let ip = stream.peer_addr().map(|a| a.ip()).ok();
            let ticket = match ip.map(crate::guard::admit) {
                Some(Ok(t)) => Some(t),
                Some(Err(why)) => {
                    println!("[tcp] refused {}: {why}", ip.map(|i| i.to_string()).unwrap_or_default());
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                    continue;
                }
                None => None,
            };
            thread::spawn(move || {
                let _ticket = ticket;
                let name = stream
                    .peer_addr()
                    .map(|a| format!("tcp in {a}"))
                    .unwrap_or_default();
                // The first two bytes decide whether this is a DHT request
                // or an ordinary packet connection.
                let mut first = [0u8; 2];
                let mut s = stream;
                let deadline = Instant::now() + crate::dht::RPC_TIMEOUT;
                if s.set_write_timeout(Some(crate::dht::RPC_TIMEOUT)).is_err()
                    || crate::dht::read_by(&mut s, &mut first, deadline).is_none()
                {
                    return;
                }
                if matches!(first, crate::dht::MAGIC | crate::dht::AUTH_MAGIC | crate::dht::MUTUAL_MAGIC) {
                    if ip.is_some_and(|ip| !crate::guard::dht_allowed(ip)) {
                        return; // too many DHT requests from this address
                    }
                    serve_dht(s, &tx, deadline, first);
                } else if first == MODEM_HELLO {
                    let name = name.replace("tcp in", "wifi-modem");
                    run_tcp_modem(s, name, tx);
                } else if s.set_read_timeout(None).is_ok() {
                    run_tcp(s, name, tx, Some(u16::from_le_bytes(first)), ip);
                }
            });
        }
    });
    Ok(())
}

/// Connects to a Wi-Fi LoRa modem that listens at a fixed address
/// (`--modem-ip`; the modem's host is set to `listen:<port>`), and again
/// when the connection drops.
pub fn spawn_modem_dial(addr: String, tx: Sender<In>) {
    thread::spawn(move || {
        loop {
            match std::net::TcpStream::connect(&addr) {
                Ok(mut s) => {
                    let mut first = [0u8; 2];
                    let _ = s.set_read_timeout(Some(Duration::from_secs(10)));
                    if s.read_exact(&mut first).is_ok() && first == MODEM_HELLO {
                        let _ = s.set_read_timeout(None);
                        run_tcp_modem(s, format!("wifi-modem {addr}"), tx.clone());
                    } else {
                        println!("[tcp] {addr} is not a Wi-Fi modem (no MM greeting)");
                    }
                }
                Err(e) => println!("[tcp] could not reach the modem at {addr}: {e}"),
            }
            thread::sleep(Duration::from_secs(10));
        }
    });
}

/// Connects to another maarse-host and retries if the connection drops.
/// All addresses behind a seed name ("name" or "name:port", port 4242 by
/// default), as "ip:port" endpoints.
pub fn resolve_seeds(name: &str) -> Vec<String> {
    use std::net::ToSocketAddrs;
    let with_port = if name.rsplit_once(':').is_some_and(|(_, p)| p.parse::<u16>().is_ok()) && !name.ends_with(']') {
        name.to_string()
    } else {
        format!("{name}:4242")
    };
    let mut v: Vec<String> = with_port.to_socket_addrs().map(|it| it.map(|a| a.to_string()).collect()).unwrap_or_default();
    v.sort();
    v.dedup();
    v
}

/// Applies the DHT address policy before connecting to a discovered peer.
pub fn spawn_discovered_tcp_peer(peer: String, configured: Vec<String>, tx: Sender<In>) {
    thread::spawn(move || {
        if let Some(addr) = crate::dht::resolve_peer(&peer, &configured) {
            spawn_tcp_peer(addr.to_string(), tx);
        }
    });
}

pub fn spawn_tcp_peer(peer: String, tx: Sender<In>) {
    thread::spawn(move || {
        loop {
            match TcpStream::connect(&peer) {
                Ok(s) => run_tcp(s, format!("tcp out {peer}"), tx.clone(), None, None),
                Err(e) => eprintln!("[tcp] could not connect to {peer}: {e}"),
            }
            thread::sleep(Duration::from_secs(5));
        }
    });
}

/// Answers one DHT request via the node's main loop.
fn serve_dht(mut s: TcpStream, tx: &Sender<In>, deadline: Instant, protocol: [u8; 2]) {
    use rand_core::{OsRng, RngCore};

    let challenge = if protocol != crate::dht::MAGIC {
        let mut nonce = [0u8; 32];
        if crate::dht::read_by(&mut s, &mut nonce, deadline).is_none() { return; }
        Some(nonce)
    } else {
        None
    };
    let server_challenge = if protocol == crate::dht::MUTUAL_MAGIC {
        let mut nonce = [0; 32];
        OsRng.fill_bytes(&mut nonce);
        if s.write_all(&nonce).is_err() { return; }
        Some(nonce)
    } else {
        None
    };
    let Some(data) = crate::dht::read_request(&mut s, deadline) else {
        return;
    };
    if let (Some(server_challenge), Some(client_challenge)) = (server_challenge, challenge) {
        let mut proof = [0; 128];
        if crate::dht::read_by(&mut s, &mut proof, deadline).is_none()
            || crate::dht::verify_request_proof(&server_challenge, &client_challenge, &data, &proof).is_none()
        {
            return;
        }
    }
    let (rtx, rrx) = channel();
    if tx.send(In::DhtRequest { data, challenge, reply: rtx }).is_err() {
        return;
    }
    if let Ok(resp) = rrx.recv_timeout(crate::dht::RPC_TIMEOUT) {
        crate::dht::write_response(&mut s, &resp);
    }
}

/// Keepalive probes after this much silence, then every `KEEPALIVE_EVERY`.
const KEEPALIVE_IDLE: Duration = Duration::from_secs(30);
const KEEPALIVE_EVERY: Duration = Duration::from_secs(10);

fn run_tcp(stream: TcpStream, name: String, tx: Sender<In>, first_len: Option<u16>, ip: Option<std::net::IpAddr>) {
    let _ = stream.set_nodelay(true);
    // Without keepalive a peer that disappears without closing (a node whose
    // hotspot dropped, a restart) leaves a dead link we keep writing to.
    let ka = socket2::TcpKeepalive::new().with_time(KEEPALIVE_IDLE).with_interval(KEEPALIVE_EVERY);
    let _ = socket2::SockRef::from(&stream).set_tcp_keepalive(&ka);
    let id = next_id();
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    if writer.set_write_timeout(Some(crate::dht::RPC_TIMEOUT)).is_err() {
        return;
    }
    let (wtx, wrx) = sync_channel::<(Vec<u8>, Option<Vec<u8>>)>(256);
    let (report_tx, report_rx) = sync_channel::<Vec<u8>>(2);
    thread::spawn(move || {
        use std::sync::mpsc::{RecvTimeoutError, TryRecvError};
        loop {
            let (pkt, descriptor) = match wrx.try_recv() {
                Ok(pkt) => pkt,
                Err(TryRecvError::Disconnected) => break,
                Err(TryRecvError::Empty) => match report_rx.try_recv() {
                    Ok(report) => (report, None),
                    Err(_) => match wrx.recv_timeout(Duration::from_millis(100)) {
                        Ok(pkt) => pkt,
                        Err(RecvTimeoutError::Timeout) => continue,
                        Err(RecvTimeoutError::Disconnected) => break,
                    },
                },
            };
            let mut buf = Vec::with_capacity(pkt.len() + 2 + descriptor.as_ref().map_or(0, |name| name.len() + 2));
            if let Some(name) = descriptor {
                buf.extend_from_slice(&(name.len() as u16).to_le_bytes());
                buf.extend_from_slice(&name);
            }
            buf.extend_from_slice(&(pkt.len() as u16).to_le_bytes());
            buf.extend_from_slice(&pkt);
            if writer.write_all(&buf).is_err() {
                break;
            }
            note_tx(id);
        }
        let _ = writer.shutdown(std::net::Shutdown::Both);
    });
    register_tcp(id, &name, "backbone", &stream);
    if tx.send(In::LinkUp { id, name, tx: LinkSender::Backbone { packets: wtx, reports: report_tx } }).is_err() {
        unregister(id);
        return;
    }
    let mut reader = stream;
    let mut len = [0u8; 2];
    let mut first = first_len;
    // Incoming links are rate limited; outgoing ones (we chose the peer) not.
    let mut rate = ip.map(|_| crate::guard::Rate::default());
    let mut report_window = Instant::now();
    let mut reports = 0;
    let mut pending_channel = None;
    loop {
        let length_already_read = first.is_some();
        if let Some(f) = first.take() {
            len = f.to_le_bytes();
        } else if reader.set_read_timeout(None).is_err() || reader.read_exact(&mut len[..1]).is_err() {
            break;
        }
        let deadline = Instant::now() + crate::dht::RPC_TIMEOUT;
        if !length_already_read {
            if crate::dht::read_by(&mut reader, &mut len[1..], deadline).is_none() {
                break;
            }
        }
        let n = u16::from_le_bytes(len) as usize;
        if n == 0 || n > MAX_PACKET {
            break; // Invalid stream: close.
        }
        let mut data = vec![0u8; n];
        if crate::dht::read_by(&mut reader, &mut data, deadline).is_none() {
            break;
        }
        let open_channel = pending_channel.take();
        if let (Some(r), Some(ip)) = (rate.as_mut(), ip) {
            match r.on_packet(data.len()) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(b) => {
                    crate::guard::ban(ip, std::time::Duration::from_millis(b.ms), b.why);
                    break;
                }
            }
        }
        if let Some((strict, regions)) = parse_region_hello(&data) {
            if let Some(caps) = parse_hello_caps(&data) {
                let _ = tx.send(In::LinkCaps { id, caps });
            }
            if tx.send(In::LinkRegions { id, strict, regions }).is_err() {
                break;
            }
            continue;
        }
        if data.starts_with(crate::observe::PREFIX) {
            if report_window.elapsed() >= Duration::from_secs(1) {
                report_window = Instant::now();
                reports = 0;
            }
            if reports < 8 {
                reports += 1;
                if tx.send(In::ObservationReport(data)).is_err() { break; }
            }
            continue;
        }
        if data.starts_with(crate::observe::CHANNEL_PREFIX) {
            pending_channel = crate::observe::PublicChannel::from_frame(&data);
            continue;
        }
        if let (Some(r), Some(ip)) = (rate.as_mut(), ip) {
            if maarse_core::Packet::decode(&data).is_err() {
                if let Some(b) = r.on_invalid() {
                    crate::guard::ban(ip, std::time::Duration::from_millis(b.ms), b.why);
                    break;
                }
                continue;
            }
        }
        note_rx(id, None);
        if tx
            .send(In::Packet {
                link: id,
                data,
                snr: TCP_SNR,
                open_channel,
            })
            .is_err()
        {
            break;
        }
    }
    let _ = reader.shutdown(std::net::Shutdown::Both);
    unregister(id);
    let _ = tx.send(In::LinkDown { id });
}

/// An ESP32 node that is our LoRa modem over Wi-Fi (see the module docs).
/// Wi-Fi modems: whether a key is required, and the key (see
/// `maarse_core::modemauth`). maarse-server always requires one.
static MODEM_AUTH: std::sync::OnceLock<(bool, Option<[u8; 32]>)> = std::sync::OnceLock::new();

pub fn set_modem_auth(required: bool, key: Option<[u8; 32]>) {
    let _ = MODEM_AUTH.set((required || key.is_some(), key));
}

/// Asks a new Wi-Fi modem to prove the key; `Err` says why it is refused.
fn modem_challenge(s: &mut TcpStream) -> Result<(), &'static str> {
    use maarse_core::modemauth::{CHALLENGE, NONCE_LEN, RESPONSE, RESPONSE_MARK, check};
    let (required, key) = MODEM_AUTH.get().copied().unwrap_or((false, None));
    if !required {
        return Ok(());
    }
    let Some(key) = key else {
        return Err("no modem key is set on this host (--modem-key)");
    };
    let mut nonce = [0u8; NONCE_LEN];
    rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut nonce);
    let mut f = Vec::with_capacity(2 + 3 + NONCE_LEN);
    f.extend_from_slice(&((3 + NONCE_LEN) as u16).to_le_bytes());
    f.extend_from_slice(&CHALLENGE);
    f.extend_from_slice(&nonce);
    s.set_write_timeout(Some(crate::dht::RPC_TIMEOUT)).map_err(|_| "could not configure the modem socket")?;
    s.write_all(&f).map_err(|_| "could not send the challenge")?;
    let deadline = Instant::now() + crate::dht::RPC_TIMEOUT;
    // The modem may send a few LoRa packets before its answer.
    for _ in 0..16 {
        let mut len = [0u8; 2];
        crate::dht::read_by(s, &mut len, deadline).ok_or("no answer to the modem key challenge (old firmware or no key)")?;
        let n = u16::from_le_bytes(len) as usize;
        if n == 0 || n > MAX_PACKET + 1 {
            return Err("not a modem stream");
        }
        let mut d = vec![0u8; n];
        crate::dht::read_by(s, &mut d, deadline).ok_or("connection lost during the challenge")?;
        if d[0] == RESPONSE_MARK && d[1..].starts_with(&RESPONSE) {
            let _ = s.set_read_timeout(None);
            return if check(&key, &nonce, &d[4..]) { Ok(()) } else { Err("wrong modem key") };
        }
    }
    Err("no answer to the modem key challenge")
}

fn run_tcp_modem(mut stream: TcpStream, name: String, tx: Sender<In>) {
    if let Err(why) = modem_challenge(&mut stream) {
        println!("[tcp] refused a Wi-Fi modem from {}: {why}", name.trim_start_matches("wifi-modem "));
        let _ = stream.shutdown(std::net::Shutdown::Both);
        return;
    }
    let _ = stream.set_nodelay(true);
    let ka = socket2::TcpKeepalive::new().with_time(KEEPALIVE_IDLE).with_interval(KEEPALIVE_EVERY);
    let _ = socket2::SockRef::from(&stream).set_tcp_keepalive(&ka);
    let id = next_id();
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    if writer.set_write_timeout(Some(crate::dht::RPC_TIMEOUT)).is_err() { return; }
    let (wtx, wrx) = sync_channel::<Vec<u8>>(256);
    thread::spawn(move || {
        for pkt in wrx {
            let mut buf = Vec::with_capacity(pkt.len() + 2);
            buf.extend_from_slice(&(pkt.len() as u16).to_le_bytes());
            buf.extend_from_slice(&pkt);
            if writer.write_all(&buf).is_err() {
                break;
            }
            note_tx(id);
        }
        let _ = writer.shutdown(std::net::Shutdown::Both);
    });
    register_tcp(id, &name, "wifi-modem", &stream);
    // Firmware updates ride on this link: `0xFF 'O' 'T' | frame` to the node.
    let ota_tx = wtx.clone();
    crate::fwupdate::link_up(
        id,
        "wifi-modem",
        &name,
        std::sync::Arc::new(move |frame: Vec<u8>| {
            let mut p = OTA_MARK.to_vec();
            p.extend_from_slice(&frame[2..]);
            ota_tx.try_send(p).is_ok()
        }),
    );
    if tx.send(In::LinkUp { id, name, tx: LinkSender::Bounded(wtx) }).is_err() {
        crate::fwupdate::link_down(id);
        unregister(id);
        return;
    }
    let mut reader = stream;
    let mut len = [0u8; 2];
    loop {
        if reader.set_read_timeout(None).is_err() || reader.read_exact(&mut len[..1]).is_err() { break; }
        let deadline = Instant::now() + crate::dht::RPC_TIMEOUT;
        if crate::dht::read_by(&mut reader, &mut len[1..], deadline).is_none() { break; }
        let n = u16::from_le_bytes(len) as usize;
        if n < 2 || n > MAX_PACKET + 1 {
            break; // Not a modem stream: close.
        }
        let mut data = vec![0u8; n];
        if crate::dht::read_by(&mut reader, &mut data, deadline).is_none() {
            break;
        }
        // A reply to a firmware update (marker 0x7D): the companion event.
        if data[0] == 0x7D {
            crate::fwupdate::on_frame(id, &data[1..]);
            continue;
        }
        // The modem's own counters (marker 0x7E): telemetry, not a packet.
        if data[0] == 0x7E {
            if tx.send(In::ModemFrame { link: id, frame: data[1..].to_vec() }).is_err() {
                break;
            }
            continue;
        }
        let snr = data[0] as i8;
        note_rx(id, Some(snr));
        if tx.send(In::Packet { link: id, data: data[1..].to_vec(), snr, open_channel: None }).is_err() {
            break;
        }
    }
    let _ = reader.shutdown(std::net::Shutdown::Both);
    crate::fwupdate::link_down(id);
    unregister(id);
    let _ = tx.send(In::LinkDown { id });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Wi-Fi modem is updated over its own link: the commands go out as `0xFF 'O' 'T' | frame`, the
    /// replies come back as `0x7D | frame`, and the node runs the firmware's own receiver.
    #[test]
    fn a_wifi_modem_is_updated_over_its_link() {
        use crate::fwupdate::testkit::NodeSim;
        use crate::fwupdate::tests::{ONE_AT_A_TIME, temp, test_image};
        use maarse_crypto::ota::{self, Manifest, SigningKey};
        use std::net::TcpListener;
        let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        let dir = temp();
        crate::fwupdate::open(&dir);
        let key = SigningKey::from_bytes(&[21u8; 32]);
        let image = test_image(9000);
        let m = Manifest::sign(&key, ota::board::HELTEC_V4, 0x000300, &image);
        assert_eq!(crate::fwupdate::api("POST", "/api/firmware/upload", "name=m.manifest", &m.to_bytes()).unwrap().0, "200 OK");
        assert_eq!(crate::fwupdate::api("POST", "/api/firmware/upload", "name=i.bin", &image).unwrap().0, "200 OK");

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, _rx) = channel::<In>();
        let host = thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            run_tcp_modem(s, "wifi-modem 127.0.0.1".into(), tx);
        });
        // The node: reads `length | packet`, answers firmware commands like the firmware does.
        let mut node = TcpStream::connect(addr).unwrap();
        node.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut sim = NodeSim::new(key.verifying_key().to_bytes(), ota::board::HELTEC_V4, 0x000100);
        let mut finished = None;
        let mut commands = 0;
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(20) {
            let mut len = [0u8; 2];
            if node.read_exact(&mut len).is_err() {
                continue;
            }
            let mut p = vec![0u8; u16::from_le_bytes(len) as usize];
            node.read_exact(&mut p).unwrap();
            if !p.starts_with(&OTA_MARK) {
                continue;
            }
            commands += 1;
            for reply in sim.handle(&p[3..]) {
                let mut out = ((reply.len() + 1) as u16).to_le_bytes().to_vec();
                out.push(0x7D);
                out.extend_from_slice(&reply);
                node.write_all(&out).unwrap();
            }
            if commands == 1 {
                // The host asked what we run; once it has the answer the update can start.
                std::thread::sleep(Duration::from_millis(200));
                let id = crate::fwupdate::status_json()["devices"][0]["id"].as_u64().unwrap();
                let r = crate::fwupdate::api("POST", "/api/firmware/update", "", format!("{{\"id\":{id},\"source\":\"local\"}}").as_bytes()).unwrap();
                assert_eq!(r.0, "200 OK", "{}", r.1);
            }
            if crate::fwupdate::status_json()["job"]["state"] == serde_json::json!("done") {
                finished = Some(sim.installed.clone());
                break;
            }
        }
        assert_eq!(finished.as_deref(), Some(&image[..]), "{}", crate::fwupdate::status_json()["job"]);
        drop(node);
        let _ = host.join();
        let _ = std::fs::remove_dir_all(&dir);
    }


    #[test]
    fn hello_carries_caps_and_old_parsers_still_read_it() {
        let c = crate::dht::Caps { version: "0.1.0".into(), radio: true, post_office: true, mail_free_mb: 9, ..Default::default() };
        let h = region_hello_with_caps(false, &[5], &c);
        assert_eq!(parse_region_hello(&h), Some((false, vec![5])));
        assert_eq!(parse_hello_caps(&h), Some(c));
        assert_eq!(parse_hello_caps(&region_hello(false, &[5])), None);
    }

    #[test]
    fn region_hello_roundtrip() {
        let h = region_hello(true, &[1, 0xabcd]);
        assert_eq!(parse_region_hello(&h), Some((true, vec![1, 0xabcd])));
        assert_eq!(parse_region_hello(&region_hello(false, &[])), Some((false, vec![])));
        assert_eq!(parse_region_hello(&h[..h.len() - 1]), None);
        // The hello can never be mistaken for a Må:rse packet.
        assert!(maarse_core::Packet::decode(&h).is_err());
    }
}

pub fn spawn_serial(
port: String, baud: u32, tx: Sender<In>) {
    let name = format!("serial {port}");
    spawn_serial_named(port, baud, name, tx);
}

/// Like [`spawn_serial`], with the link's name (`trunk <port>` for a trunk).
pub fn spawn_serial_named(port: String, baud: u32, name: String, tx: Sender<In>) {
    thread::spawn(move || {
        loop {
            match serialport::new(&port, baud)
                .timeout(Duration::from_millis(10))
                .open()
            {
                Ok(p) => {
                    port_state(&name, None);
                    run_serial(p, name.clone(), &tx);
                    port_state(&name, Some("the connection was lost".into()));
                }
                Err(e) => {
                    eprintln!("[serial] could not open {port}: {e}");
                    port_state(&name, Some(e.to_string()));
                }
            }
            thread::sleep(Duration::from_secs(3));
        }
    });
}

/// Reads and writes in the same thread. The port is deliberately not cloned: on Windows
/// `try_clone` creates a handle that can be inherited (DuplicateHandle with
/// bInheritHandle = TRUE), and then the process from a restart would keep the port
/// locked forever ("Access denied").
fn run_serial(port: Box<dyn serialport::SerialPort>, name: String, tx: &Sender<In>) {
    run_modem(port, name, "usb-modem", tx);
}

/// This host's address, sent to a LoRa modem with the modem command so its
/// display can show the ID in use (set once at startup).
pub static HOST_ADDR: std::sync::OnceLock<[u8; 16]> = std::sync::OnceLock::new();

/// The modem command: `0x41`, followed by our address when known.
fn modem_command() -> Vec<u8> {
    let mut f = vec![0x41];
    if let Some(a) = HOST_ADDR.get() {
        f.extend_from_slice(a);
    }
    let mut out = (f.len() as u16).to_le_bytes().to_vec();
    out.extend_from_slice(&f);
    out
}

/// How often the modem command is repeated: a radio that fell out of modem
/// mode (e.g. after a quick Bluetooth reconnect) comes back by itself.
const MODEM_REPEAT: Duration = Duration::from_secs(20);

/// A LoRa modem over a TCP byte stream with the serial modem protocol
/// (`--modem-tcp`): the Android app bridges it to a radio over Bluetooth.
/// If `<data>/modem-tcp-key.txt` exists, its key is sent first (a line
/// `KEY <hex>`).
pub fn spawn_modem_tcp(addr: String, key: Option<String>, tx: Sender<In>) {
    thread::spawn(move || {
        loop {
            match TcpStream::connect(&addr) {
                Ok(mut s) => {
                    let _ = s.set_nodelay(true);
                    let _ = s.set_read_timeout(Some(Duration::from_millis(10)));
                    let ok = key.as_ref().is_none_or(|k| s.write_all(format!("KEY {k}\n").as_bytes()).is_ok());
                    if ok {
                        run_modem(s, format!("modem {addr}"), "ble-modem", &tx);
                    }
                }
                Err(_) => {}
            }
            thread::sleep(Duration::from_secs(3));
        }
    });
}

fn run_modem<P: std::io::Read + std::io::Write>(mut port: P, name: String, kind: &'static str, tx: &Sender<In>) {
    let id = next_id();
    let (wtx, wrx) = channel::<Vec<u8>>();
    // Firmware updates: encoded companion commands, written to the port as they are.
    let (ctl_tx, ctl_rx) = channel::<Vec<u8>>();
    // Put the node in modem mode: the host does the routing.
    if port.write_all(&modem_command()).is_err() {
        return;
    }
    let mut last_modem_cmd = std::time::Instant::now();
    register(id, &name, kind);
    crate::fwupdate::link_up(id, kind, &name, {
        let ctl = std::sync::Mutex::new(ctl_tx);
        std::sync::Arc::new(move |frame: Vec<u8>| ctl.lock().is_ok_and(|c| c.send(frame).is_ok()))
    });
    if tx.send(In::LinkUp { id, name, tx: wtx.into() }).is_err() {
        crate::fwupdate::link_down(id);
        unregister(id);
        return;
    }
    let mut dec = FrameDecoder::new();
    let mut buf = [0u8; 256];
    let mut last_poll = std::time::Instant::now() - std::time::Duration::from_secs(10);
    'link: loop {
        match port.read(&mut buf) {
            Ok(0) => {
                eprintln!("[modem] {kind}: the other side closed the connection");
                break;
            }
            Ok(n) => {
                dec.push(&buf[..n]);
                while let Some(f) = dec.next_frame() {
                    if f.len() >= 2 && f[0] == 0xC0 {
                        let snr = f[1] as i8;
                        note_rx(id, Some(snr));
                        let _ = tx.send(In::Packet {
                            link: id,
                            data: f[2..].to_vec(),
                            snr,
                            open_channel: None,
                        });
                    } else if matches!(f.first(), Some(0x91 | 0x92 | 0xbe | 0xbf)) {
                        if let Ok(maarse_core::companion::Event::Stats { items }) = maarse_core::companion::Event::decode(&f) {
                            let items: Vec<(u8, i32)> = items.iter().map(|(k, v)| (*k, *v)).collect();
                            note_modem_stats(id, &items);
                        }
                        let _ = tx.send(In::ModemFrame { link: id, frame: f.to_vec() });
                    } else if matches!(f.first(), Some(0x9a | 0x9b)) {
                        crate::fwupdate::on_frame(id, &f);
                    }
                }
            }
            // Interrupted: a signal hit the read (common on Android, where
            // the app's runtime sends them); nothing is wrong, read again.
            // Ending the link on it dropped the phone's Bluetooth modem
            // every few seconds.
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) => {}
            Err(e) => {
                eprintln!("[modem] {kind}: read error: {e}");
                break;
            }
        }
        // maarse-server: ask the modem for its own counters every 30 s (it
        // answers in modem mode without leaving it).
        if last_poll.elapsed() >= std::time::Duration::from_secs(10) {
            last_poll = std::time::Instant::now();
            use maarse_core::companion::Command;
            if port.write_all(&Command::GetStats.encode()).is_err() || port.write_all(&Command::GetRadio.encode()).is_err() {
                break 'link;
            }
        }
        if last_modem_cmd.elapsed() >= MODEM_REPEAT {
            last_modem_cmd = std::time::Instant::now();
            if let Err(e) = port.write_all(&modem_command()) {
                eprintln!("[modem] {kind}: write error: {e}");
                break 'link;
            }
        }
        while let Ok(frame) = ctl_rx.try_recv() {
            if let Err(e) = port.write_all(&frame) {
                eprintln!("[modem] {kind}: write error: {e}");
                break 'link;
            }
        }
        while let Ok(pkt) = wrx.try_recv() {
            let mut out = Vec::with_capacity(pkt.len() + 3);
            out.extend_from_slice(&((pkt.len() + 1) as u16).to_le_bytes());
            out.push(0x40);
            out.extend_from_slice(&pkt);
            if let Err(e) = port.write_all(&out) {
                eprintln!("[modem] {kind}: write error: {e}");
                break 'link;
            }
            note_tx(id);
        }
    }
    crate::fwupdate::link_down(id);
    unregister(id);
    let _ = tx.send(In::LinkDown { id });
}
