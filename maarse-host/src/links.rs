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

use crate::{In, next_id};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Mutex;
use std::sync::mpsc::{Sender, channel};
use std::thread;
use std::time::Duration;
use maarse_core::MAX_PACKET;
use maarse_core::companion::FrameDecoder;

/// SNR we report for packets over TCP (a "perfect" link).
pub(crate) const TCP_SNR: i8 = 10;

/// First two bytes from an ESP32 node in Wi-Fi modem mode.
pub const MODEM_HELLO: [u8; 2] = *b"MM";

// ---------- device registry ----------

/// One link as shown in the web client's device list.
struct LinkInfo {
    id: u32,
    name: String,
    /// `backbone`, `usb-modem`, `wifi-modem`, `serial-link`, `kiss`, `kiss-tcp` or `udp`.
    kind: &'static str,
    since: u64,
    rx: u64,
    tx: u64,
    last_rx: u64,
    last_snr: Option<i8>,
}

static REGISTRY: Mutex<Vec<LinkInfo>> = Mutex::new(Vec::new());

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn with_registry<T>(f: impl FnOnce(&mut Vec<LinkInfo>) -> T) -> T {
    f(&mut REGISTRY.lock().unwrap_or_else(|e| e.into_inner()))
}

pub(crate) fn register(id: u32, name: &str, kind: &'static str) {
    let now = unix_now();
    with_registry(|r| {
        r.push(LinkInfo { id, name: name.to_string(), kind, since: now, rx: 0, tx: 0, last_rx: 0, last_snr: None })
    });
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

/// All links right now, as JSON for the web client.
pub fn devices_json() -> String {
    let now = unix_now();
    let list: Vec<serde_json::Value> = with_registry(|r| {
        r.iter()
            .map(|l| {
                serde_json::json!({
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
    serde_json::json!({ "devices": list }).to_string()
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
            thread::spawn(move || {
                let name = stream
                    .peer_addr()
                    .map(|a| format!("tcp in {a}"))
                    .unwrap_or_default();
                // The first two bytes decide whether this is a DHT request
                // or an ordinary packet connection.
                let mut first = [0u8; 2];
                let mut s = stream;
                if s.read_exact(&mut first).is_err() {
                    return;
                }
                if first == crate::dht::MAGIC {
                    serve_dht(s, &tx);
                } else if first == MODEM_HELLO {
                    let name = name.replace("tcp in", "wifi-modem");
                    run_tcp_modem(s, name, tx);
                } else {
                    run_tcp(s, name, tx, Some(u16::from_le_bytes(first)));
                }
            });
        }
    });
    Ok(())
}

/// Connects to another maarse-host and retries if the connection drops.
pub fn spawn_tcp_peer(peer: String, tx: Sender<In>) {
    thread::spawn(move || {
        loop {
            match TcpStream::connect(&peer) {
                Ok(s) => run_tcp(s, format!("tcp out {peer}"), tx.clone(), None),
                Err(e) => eprintln!("[tcp] could not connect to {peer}: {e}"),
            }
            thread::sleep(Duration::from_secs(5));
        }
    });
}

/// Answers one DHT request via the node's main loop.
fn serve_dht(mut s: TcpStream, tx: &Sender<In>) {
    let Some(data) = crate::dht::read_request(&mut s) else {
        return;
    };
    let (rtx, rrx) = channel();
    if tx.send(In::DhtRequest { data, reply: rtx }).is_err() {
        return;
    }
    if let Ok(resp) = rrx.recv_timeout(crate::dht::RPC_TIMEOUT) {
        crate::dht::write_response(&mut s, &resp);
    }
}

/// Keepalive probes after this much silence, then every `KEEPALIVE_EVERY`.
const KEEPALIVE_IDLE: Duration = Duration::from_secs(30);
const KEEPALIVE_EVERY: Duration = Duration::from_secs(10);

fn run_tcp(stream: TcpStream, name: String, tx: Sender<In>, first_len: Option<u16>) {
    let _ = stream.set_nodelay(true);
    // Without keepalive a peer that disappears without closing (a node whose
    // hotspot dropped, a restart) leaves a dead link we keep writing to.
    let ka = socket2::TcpKeepalive::new().with_time(KEEPALIVE_IDLE).with_interval(KEEPALIVE_EVERY);
    let _ = socket2::SockRef::from(&stream).set_tcp_keepalive(&ka);
    let id = next_id();
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    let (wtx, wrx) = channel::<Vec<u8>>();
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
    register(id, &name, "backbone");
    if tx.send(In::LinkUp { id, name, tx: wtx }).is_err() {
        unregister(id);
        return;
    }
    let mut reader = stream;
    let mut len = [0u8; 2];
    let mut first = first_len;
    loop {
        if let Some(f) = first.take() {
            len = f.to_le_bytes();
        } else if reader.read_exact(&mut len).is_err() {
            break;
        }
        let n = u16::from_le_bytes(len) as usize;
        if n == 0 || n > MAX_PACKET {
            break; // Invalid stream: close.
        }
        let mut data = vec![0u8; n];
        if reader.read_exact(&mut data).is_err() {
            break;
        }
        if let Some((strict, regions)) = parse_region_hello(&data) {
            if tx.send(In::LinkRegions { id, strict, regions }).is_err() {
                break;
            }
            continue;
        }
        note_rx(id, None);
        if tx
            .send(In::Packet {
                link: id,
                data,
                snr: TCP_SNR,
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
fn run_tcp_modem(stream: TcpStream, name: String, tx: Sender<In>) {
    let _ = stream.set_nodelay(true);
    let ka = socket2::TcpKeepalive::new().with_time(KEEPALIVE_IDLE).with_interval(KEEPALIVE_EVERY);
    let _ = socket2::SockRef::from(&stream).set_tcp_keepalive(&ka);
    let id = next_id();
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    let (wtx, wrx) = channel::<Vec<u8>>();
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
    register(id, &name, "wifi-modem");
    if tx.send(In::LinkUp { id, name, tx: wtx }).is_err() {
        unregister(id);
        return;
    }
    let mut reader = stream;
    let mut len = [0u8; 2];
    loop {
        if reader.read_exact(&mut len).is_err() {
            break;
        }
        let n = u16::from_le_bytes(len) as usize;
        if n < 2 || n > MAX_PACKET + 1 {
            break; // Not a modem stream: close.
        }
        let mut data = vec![0u8; n];
        if reader.read_exact(&mut data).is_err() {
            break;
        }
        let snr = data[0] as i8;
        note_rx(id, Some(snr));
        if tx.send(In::Packet { link: id, data: data[1..].to_vec(), snr }).is_err() {
            break;
        }
    }
    let _ = reader.shutdown(std::net::Shutdown::Both);
    unregister(id);
    let _ = tx.send(In::LinkDown { id });
}

#[cfg(test)]
mod tests {
    use super::*;

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
                Ok(p) => run_serial(p, name.clone(), &tx),
                Err(e) => eprintln!("[serial] could not open {port}: {e}"),
            }
            thread::sleep(Duration::from_secs(3));
        }
    });
}

/// Reads and writes in the same thread. The port is deliberately not cloned: on Windows
/// `try_clone` creates a handle that can be inherited (DuplicateHandle with
/// bInheritHandle = TRUE), and then the process from a restart would keep the port
/// locked forever ("Access denied").
fn run_serial(mut port: Box<dyn serialport::SerialPort>, name: String, tx: &Sender<In>) {
    let id = next_id();
    let (wtx, wrx) = channel::<Vec<u8>>();
    // Put the node in modem mode: the host does the routing.
    if port.write_all(&[1, 0, 0x41]).is_err() {
        return;
    }
    register(id, &name, "usb-modem");
    if tx.send(In::LinkUp { id, name, tx: wtx }).is_err() {
        unregister(id);
        return;
    }
    let mut dec = FrameDecoder::new();
    let mut buf = [0u8; 256];
    'link: loop {
        match port.read(&mut buf) {
            Ok(0) => break,
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
                        });
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => break,
        }
        while let Ok(pkt) = wrx.try_recv() {
            let mut out = Vec::with_capacity(pkt.len() + 3);
            out.extend_from_slice(&((pkt.len() + 1) as u16).to_le_bytes());
            out.push(0x40);
            out.extend_from_slice(&pkt);
            if port.write_all(&out).is_err() {
                break 'link;
            }
            note_tx(id);
        }
    }
    unregister(id);
    let _ = tx.send(In::LinkDown { id });
}
