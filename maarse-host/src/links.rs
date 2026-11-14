//! Pakke-links: TCP-backbone og seriel LoRa-modem.
//!
//! TCP: hver pakke sendes som `længde (u16 LE) | pakke`.
//!
//! Regionshilsen over TCP: lige efter forbindelsen sender hver side en
//! "pakke" `0xFF 'R' 'G' | strict (1) | antal (1) | region-id (u16 BE) …`
//! med de regioner, den betjener (se `maarse_core::region`). Den afkodes
//! aldrig som en Må:rse-pakke (version 3), så ældre hosts tæller den blot
//! som ugyldig. Herefter sender vi kun floods med region til peers, der
//! betjener regionen. Peers uden hilsen får alt (som før).
//!
//! Seriel modem-tilstand (kræver firmware, der understøtter den):
//! * host -> node: `længde | 0x40 | pakke`         (send pakken over LoRa)
//! * node -> host: `længde | 0xC0 | snr (i8) | pakke` (modtaget over LoRa)

use crate::{In, next_id};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc::{Sender, channel};
use std::thread;
use std::time::Duration;
use maarse_core::MAX_PACKET;
use maarse_core::companion::FrameDecoder;

/// SNR, vi angiver for pakker over TCP (et "perfekt" link).
const TCP_SNR: i8 = 10;

/// Start på en regionshilsen over TCP.
pub const REGION_HELLO: [u8; 3] = [0xFF, b'R', b'G'];

/// Koder en regionshilsen.
pub fn region_hello(strict: bool, regions: &[u16]) -> Vec<u8> {
    let mut v = REGION_HELLO.to_vec();
    v.push(strict as u8);
    v.push(regions.len() as u8);
    for r in regions {
        v.extend_from_slice(&r.to_be_bytes());
    }
    v
}

/// Afkoder en regionshilsen: (strict, regioner).
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
    let listener = TcpListener::bind(addr)?;
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let tx = tx.clone();
            thread::spawn(move || {
                let name = stream
                    .peer_addr()
                    .map(|a| format!("tcp ind {a}"))
                    .unwrap_or_default();
                // De første to bytes afgør, om det er en DHT-forespørgsel
                // eller en almindelig pakkeforbindelse.
                let mut first = [0u8; 2];
                let mut s = stream;
                if s.read_exact(&mut first).is_err() {
                    return;
                }
                if first == crate::dht::MAGIC {
                    serve_dht(s, &tx);
                } else {
                    run_tcp(s, name, tx, Some(u16::from_le_bytes(first)));
                }
            });
        }
    });
    Ok(())
}

/// Forbinder til en anden maarse-host og prøver igen, hvis forbindelsen går.
pub fn spawn_tcp_peer(peer: String, tx: Sender<In>) {
    thread::spawn(move || {
        loop {
            match TcpStream::connect(&peer) {
                Ok(s) => run_tcp(s, format!("tcp ud {peer}"), tx.clone(), None),
                Err(e) => eprintln!("[tcp] kunne ikke forbinde til {peer}: {e}"),
            }
            thread::sleep(Duration::from_secs(5));
        }
    });
}

/// Besvarer én DHT-forespørgsel via nodens hovedløkke.
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

fn run_tcp(stream: TcpStream, name: String, tx: Sender<In>, first_len: Option<u16>) {
    let _ = stream.set_nodelay(true);
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
        }
        let _ = writer.shutdown(std::net::Shutdown::Both);
    });
    if tx.send(In::LinkUp { id, name, tx: wtx }).is_err() {
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
            break; // Ugyldig strøm: luk.
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
        // Hilsenen kan aldrig forveksles med en Må:rse-pakke.
        assert!(maarse_core::Packet::decode(&h).is_err());
    }
}

pub fn spawn_serial(
port: String, baud: u32, tx: Sender<In>) {
    thread::spawn(move || {
        loop {
            match serialport::new(&port, baud)
                .timeout(Duration::from_millis(100))
                .open()
            {
                Ok(p) => run_serial(p, format!("seriel {port}"), &tx),
                Err(e) => eprintln!("[seriel] kunne ikke åbne {port}: {e}"),
            }
            thread::sleep(Duration::from_secs(3));
        }
    });
}

fn run_serial(port: Box<dyn serialport::SerialPort>, name: String, tx: &Sender<In>) {
    let id = next_id();
    let Ok(mut writer) = port.try_clone() else {
        return;
    };
    let (wtx, wrx) = channel::<Vec<u8>>();
    thread::spawn(move || {
        // Sæt noden i modem-tilstand: hosten står for routingen.
        let _ = writer.write_all(&[1, 0, 0x41]);
        for pkt in wrx {
            let mut buf = Vec::with_capacity(pkt.len() + 3);
            buf.extend_from_slice(&((pkt.len() + 1) as u16).to_le_bytes());
            buf.push(0x40);
            buf.extend_from_slice(&pkt);
            if writer.write_all(&buf).is_err() {
                break;
            }
        }
    });
    if tx.send(In::LinkUp { id, name, tx: wtx }).is_err() {
        return;
    }
    let mut reader = port;
    let mut dec = FrameDecoder::new();
    let mut buf = [0u8; 256];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                dec.push(&buf[..n]);
                while let Some(f) = dec.next_frame() {
                    if f.len() >= 2 && f[0] == 0xC0 {
                        let snr = f[1] as i8;
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
    }
    let _ = tx.send(In::LinkDown { id });
}
