//! More link types ("interfaces"), next to TCP and the LoRa modem in `links`:
//!
//! * `--serial-link`: host-to-host over a serial line, KISS-style stuffing
//!   plus CRC-16 (see `framing`).
//! * `--kiss` / `--kiss-tcp`: a KISS TNC on a serial port or over TCP.
//! * `--udp-listen` / `--udp-peer` / `--udp-broadcast`: one packet per
//!   datagram with a short prefix. Every remote address is its own link
//!   (`udp <addr>`); peers learned from received datagrams are forgotten
//!   after [`UDP_PEER_TIMEOUT`] of silence.
//!
//! Which links count as backbone (fast) is decided by [`is_backbone`].

use crate::framing::*;
use crate::links::{TCP_SNR, note_rx, note_tx, parse_region_hello, register, unregister};
use crate::{In, next_id};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::mpsc::{Sender, TryRecvError, channel};
use std::thread;
use std::time::{Duration, Instant};

/// A `--serial-link` at this baud rate or faster counts as backbone (like
/// TCP) for file limits and path choice; slower lines count as radio.
pub const FAST_SERIAL_BAUD: u32 = 230_400;

/// True for links that count as backbone: TCP peers, UDP peers and fast
/// serial links. Everything else (LoRa modems, KISS TNCs, slow serial lines)
/// is treated as a slow radio link.
pub fn is_backbone(name: &str) -> bool {
    name.starts_with("tcp") || name.starts_with("udp ") || name.starts_with("serial-link fast ")
}

/// Link name for a `--serial-link`; see [`FAST_SERIAL_BAUD`].
pub fn serial_link_name(port: &str, baud: u32) -> String {
    if baud >= FAST_SERIAL_BAUD { format!("serial-link fast {port}") } else { format!("serial-link {port}") }
}

/// KISS settings from the command line, shared by `--kiss` and `--kiss-tcp`.
#[derive(Clone, Debug, Default)]
pub struct KissOptions {
    /// TXDELAY in 10 ms units.
    pub txdelay: Option<u8>,
    /// p-persistence, 0-255.
    pub persist: Option<u8>,
    /// Slot time in 10 ms units.
    pub slottime: Option<u8>,
    /// Wrap packets in AX.25 UI frames from this callsign.
    pub callsign: Option<Callsign>,
}

impl KissOptions {
    fn framing(&self) -> Framing {
        Framing::Kiss { callsign: self.callsign.clone() }
    }

    /// Parameter frames sent to the TNC when the link opens.
    fn init(&self) -> Vec<u8> {
        let mut v = Vec::new();
        for (cmd, val) in [(KISS_TXDELAY, self.txdelay), (KISS_PERSIST, self.persist), (KISS_SLOTTIME, self.slottime)] {
            if let Some(val) = val {
                v.extend(kiss_param(cmd, val));
            }
        }
        v
    }
}

/// Parses `<port>[:baud]` (a trailing `:number` is the baud rate).
pub fn parse_port_baud(s: &str, default: u32) -> Result<(String, u32), String> {
    match s.rsplit_once(':') {
        Some((p, b)) if !p.is_empty() && !b.is_empty() && b.bytes().all(|c| c.is_ascii_digit()) => {
            Ok((p.to_string(), b.parse().map_err(|_| format!("invalid baud rate in '{s}'"))?))
        }
        _ => Ok((s.to_string(), default)),
    }
}

fn spawn_serial_framed(port: String, baud: u32, name: String, kind: &'static str, framing: Framing, init: Vec<u8>, tx: Sender<In>) {
    thread::spawn(move || {
        loop {
            match serialport::new(&port, baud).timeout(Duration::from_millis(10)).open() {
                Ok(p) => run_framed(p, name.clone(), kind, &framing, &init, &tx),
                Err(e) => eprintln!("[{kind}] could not open {port}: {e}"),
            }
            thread::sleep(Duration::from_secs(3));
        }
    });
}

/// Direct host-to-host link over a serial line (`--serial-link`).
pub fn spawn_serial_link(port: String, baud: u32, tx: Sender<In>) {
    let name = serial_link_name(&port, baud);
    spawn_serial_framed(port, baud, name, "serial-link", Framing::SerialLink, Vec::new(), tx);
}

/// A KISS TNC on a serial port (`--kiss`).
pub fn spawn_kiss(port: String, baud: u32, opts: KissOptions, tx: Sender<In>) {
    let name = format!("kiss {port}");
    spawn_serial_framed(port, baud, name, "kiss", opts.framing(), opts.init(), tx);
}

/// A KISS TNC over TCP, e.g. Direwolf on port 8001 (`--kiss-tcp`).
/// Reconnects every 5 s like a backbone peer.
pub fn spawn_kiss_tcp(addr: String, opts: KissOptions, tx: Sender<In>) {
    thread::spawn(move || {
        loop {
            match TcpStream::connect(&addr) {
                Ok(s) => {
                    let _ = s.set_nodelay(true);
                    let _ = s.set_read_timeout(Some(Duration::from_millis(10)));
                    run_framed(s, format!("kiss-tcp {addr}"), "kiss-tcp", &opts.framing(), &opts.init(), &tx);
                }
                Err(e) => eprintln!("[kiss-tcp] could not connect to {addr}: {e}"),
            }
            thread::sleep(Duration::from_secs(5));
        }
    });
}

/// Turns a decoded packet into a message for the node (a region hello is
/// link metadata, not a packet).
fn incoming(id: u32, data: Vec<u8>) -> In {
    match parse_region_hello(&data) {
        Some((strict, regions)) => In::LinkRegions { id, strict, regions },
        None => {
            note_rx(id, None);
            In::Packet { link: id, data, snr: TCP_SNR }
        }
    }
}

/// Runs one byte-stream link with KISS-style framing until the stream
/// fails. Reads and writes in one thread (the serial port is deliberately
/// not cloned, see `links::run_serial`); `io` must have a short read timeout.
fn run_framed<S: Read + Write>(mut io: S, name: String, kind: &'static str, framing: &Framing, init: &[u8], tx: &Sender<In>) {
    if !init.is_empty() && io.write_all(init).is_err() {
        return;
    }
    let id = next_id();
    let (wtx, wrx) = channel::<Vec<u8>>();
    register(id, &name, kind);
    if tx.send(In::LinkUp { id, name, tx: wtx }).is_err() {
        unregister(id);
        return;
    }
    let mut dec = Unstuffer::new();
    let mut buf = [0u8; 512];
    'link: loop {
        match io.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                dec.push(&buf[..n]);
                while let Some(body) = dec.next_body() {
                    // Damaged frames and frames not for us are dropped.
                    if let Some(data) = framing.decode(&body)
                        && tx.send(incoming(id, data)).is_err()
                    {
                        break 'link;
                    }
                }
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) => {}
            Err(_) => break,
        }
        loop {
            match wrx.try_recv() {
                Ok(pkt) => {
                    if io.write_all(&framing.encode(&pkt)).is_err() {
                        break 'link;
                    }
                    note_tx(id);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break 'link,
            }
        }
    }
    unregister(id);
    let _ = tx.send(In::LinkDown { id });
}

// ---------- UDP ----------

/// A UDP peer learned from received datagrams is dropped after this much silence.
pub const UDP_PEER_TIMEOUT: Duration = Duration::from_secs(180);

/// One configured UDP destination: its link name and how to resolve it.
pub struct UdpTarget {
    pub name: String,
    pub addr: String,
}

struct UdpPeer {
    id: u32,
    last: Instant,
    fixed: bool,
}

/// Binds a UDP socket and runs it in a thread. `targets` get a link at once
/// and are never forgotten; other senders become links when they speak.
pub fn spawn_udp(bind: SocketAddr, broadcast: bool, targets: Vec<UdpTarget>, tx: Sender<In>) -> std::io::Result<()> {
    let sock = UdpSocket::bind(bind)?;
    if broadcast {
        sock.set_broadcast(true)?;
    }
    thread::spawn(move || run_udp(sock, targets, tx));
    Ok(())
}

fn open_udp_link(sock: &UdpSocket, instance: u32, addr: SocketAddr, name: String, tx: &Sender<In>) -> Option<u32> {
    let id = next_id();
    let w = sock.try_clone().ok()?;
    let (wtx, wrx) = channel::<Vec<u8>>();
    thread::spawn(move || {
        for pkt in wrx {
            if w.send_to(&udp_encode(instance, &pkt), addr).is_ok() {
                note_tx(id);
            }
        }
    });
    register(id, &name, "udp");
    tx.send(In::LinkUp { id, name, tx: wtx }).ok()?;
    Some(id)
}

fn close_udp_link(id: u32, tx: &Sender<In>) {
    unregister(id);
    let _ = tx.send(In::LinkDown { id });
}

fn run_udp(sock: UdpSocket, mut targets: Vec<UdpTarget>, tx: Sender<In>) {
    use rand_core::RngCore;
    let instance = rand_core::OsRng.next_u32();
    let _ = sock.set_read_timeout(Some(Duration::from_millis(500)));
    let mut peers: HashMap<SocketAddr, UdpPeer> = HashMap::new();
    let mut last_resolve: Option<Instant> = None;
    let mut buf = [0u8; 2048];
    loop {
        // Configured targets: resolve (retrying every 30 s if DNS fails).
        if !targets.is_empty() && last_resolve.is_none_or(|t| t.elapsed() > Duration::from_secs(30)) {
            last_resolve = Some(Instant::now());
            targets.retain(|t| match t.addr.to_socket_addrs().ok().and_then(|mut a| a.next()) {
                Some(addr) => {
                    if let Some(old) = peers.remove(&addr) {
                        close_udp_link(old.id, &tx);
                    }
                    match open_udp_link(&sock, instance, addr, t.name.clone(), &tx) {
                        Some(id) => {
                            peers.insert(addr, UdpPeer { id, last: Instant::now(), fixed: true });
                        }
                        None => return true,
                    }
                    false
                }
                None => {
                    eprintln!("[udp] could not resolve {}", t.addr);
                    true
                }
            });
        }
        match sock.recv_from(&mut buf) {
            Ok((n, from)) => {
                let Some((inst, pkt)) = udp_decode(&buf[..n]) else {
                    continue; // Stray datagram.
                };
                if inst == instance {
                    continue; // Our own broadcast.
                }
                let id = match peers.get_mut(&from) {
                    Some(p) => {
                        p.last = Instant::now();
                        p.id
                    }
                    None => {
                        let Some(id) = open_udp_link(&sock, instance, from, format!("udp {from}"), &tx) else {
                            return;
                        };
                        peers.insert(from, UdpPeer { id, last: Instant::now(), fixed: false });
                        id
                    }
                };
                if tx.send(incoming(id, pkt.to_vec())).is_err() {
                    return;
                }
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) => {}
            // Windows reports ICMP "port unreachable" from an earlier send as a
            // receive error (WSAECONNRESET); it says nothing about this socket.
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
            Err(e) => {
                eprintln!("[udp] receive error: {e}");
                thread::sleep(Duration::from_secs(1));
            }
        }
        peers.retain(|_, p| {
            let keep = p.fixed || p.last.elapsed() < UDP_PEER_TIMEOUT;
            if !keep {
                close_udp_link(p.id, &tx);
            }
            keep
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::links::region_hello;
    use std::collections::VecDeque;
    use std::net::TcpListener;
    use std::sync::mpsc::Receiver;
    use std::sync::{Arc, Mutex};

    type Pipe = Arc<Mutex<VecDeque<u8>>>;

    /// One end of an in-memory full-duplex serial line.
    struct End {
        rx: Pipe,
        tx: Pipe,
        /// Noise injected before each write.
        noise: Vec<u8>,
    }

    impl Read for End {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            let mut q = self.rx.lock().unwrap();
            if q.is_empty() {
                drop(q);
                thread::sleep(Duration::from_millis(2));
                return Err(std::io::ErrorKind::TimedOut.into());
            }
            // Short reads on purpose, so frames are split across reads.
            let n = out.len().min(q.len()).min(7);
            for b in out.iter_mut().take(n) {
                *b = q.pop_front().unwrap();
            }
            Ok(n)
        }
    }

    impl Write for End {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            let mut q = self.tx.lock().unwrap();
            q.extend(self.noise.iter().copied());
            q.extend(b.iter().copied());
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn pair(noise: Vec<u8>) -> (End, End) {
        let a: Pipe = Default::default();
        let b: Pipe = Default::default();
        (End { rx: a.clone(), tx: b.clone(), noise: noise.clone() }, End { rx: b, tx: a, noise })
    }

    fn up(rx: &Receiver<In>) -> (String, Sender<Vec<u8>>) {
        match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            In::LinkUp { name, tx, .. } => (name, tx),
            _ => panic!("expected LinkUp"),
        }
    }

    fn packet(rx: &Receiver<In>) -> Vec<u8> {
        loop {
            match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
                In::Packet { data, .. } => return data,
                In::LinkUp { .. } | In::LinkRegions { .. } => continue,
                _ => panic!("expected Packet"),
            }
        }
    }

    fn regions(rx: &Receiver<In>) -> (bool, Vec<u16>) {
        match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            In::LinkRegions { strict, regions, .. } => (strict, regions),
            _ => panic!("expected LinkRegions"),
        }
    }

    fn run_pair(framing: Framing, noise: Vec<u8>) {
        let (a, b) = pair(noise);
        let (atx, arx) = channel();
        let (btx, brx) = channel();
        let fa = framing.clone();
        thread::spawn(move || run_framed(a, "serial-link A".into(), "serial-link", &fa, &[], &atx));
        thread::spawn(move || run_framed(b, "serial-link B".into(), "serial-link", &framing, &[], &btx));
        let (_, to_b) = up(&arx);
        let (_, to_a) = up(&brx);
        let p1: Vec<u8> = (0..200u8).collect(); // includes FEND and FESC
        to_b.send(p1.clone()).unwrap();
        assert_eq!(packet(&brx), p1);
        to_a.send(vec![FEND, FESC, 1]).unwrap();
        assert_eq!(packet(&arx), vec![FEND, FESC, 1]);
        to_b.send(region_hello(true, &[7])).unwrap();
        assert_eq!(regions(&brx), (true, vec![7]));
    }

    #[test]
    fn serial_link_between_two_hosts() {
        run_pair(Framing::SerialLink, Vec::new());
    }

    #[test]
    fn serial_link_survives_line_noise() {
        run_pair(Framing::SerialLink, vec![0x13, FESC, 0x00, 0x42, FEND, 0x99, 0x98]);
    }

    #[test]
    fn kiss_between_two_hosts_with_callsign() {
        run_pair(Framing::Kiss { callsign: Some(Callsign::parse("N0CALL-1").unwrap()) }, Vec::new());
    }

    /// A fake KISS-over-TCP TNC (like Direwolf's port 8001) that records the
    /// frames it gets and echoes data frames back.
    #[test]
    fn kiss_tcp_against_loopback_tnc() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let (seen_tx, seen_rx) = channel::<Vec<u8>>();
        thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut dec = Unstuffer::new();
            let mut buf = [0u8; 256];
            while let Ok(n @ 1..) = s.read(&mut buf) {
                dec.push(&buf[..n]);
                while let Some(body) = dec.next_body() {
                    let _ = seen_tx.send(body.clone());
                    if body[0] == KISS_DATA {
                        let _ = s.write_all(&stuff(&body));
                    }
                }
            }
        });
        let opts = KissOptions { txdelay: Some(30), persist: Some(63), slottime: Some(10), callsign: None };
        let (tx, rx) = channel();
        spawn_kiss_tcp(addr, opts, tx);
        let (name, out) = up(&rx);
        assert!(name.starts_with("kiss-tcp ") && !is_backbone(&name));
        let seen = || seen_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(seen(), vec![KISS_TXDELAY, 30]);
        assert_eq!(seen(), vec![KISS_PERSIST, 63]);
        assert_eq!(seen(), vec![KISS_SLOTTIME, 10]);
        out.send(vec![0x31, FEND, 2]).unwrap();
        assert_eq!(seen(), vec![0x00, 0x31, FEND, 2]);
        assert_eq!(packet(&rx), vec![0x31, FEND, 2]);
    }

    /// Two hosts over UDP on 127.0.0.1: B knows A (`--udp-peer`), A learns B.
    #[test]
    fn udp_between_two_hosts() {
        let a = UdpSocket::bind("127.0.0.1:0").unwrap();
        let a_addr = a.local_addr().unwrap();
        let (atx, arx) = channel();
        thread::spawn(move || run_udp(a, Vec::new(), atx));
        let b = UdpSocket::bind("127.0.0.1:0").unwrap();
        let b_addr = b.local_addr().unwrap();
        let (btx, brx) = channel();
        let target = UdpTarget { name: format!("udp {a_addr}"), addr: a_addr.to_string() };
        thread::spawn(move || run_udp(b, vec![target], btx));
        let (name, to_a) = up(&brx);
        assert!(is_backbone(&name));
        // Stray datagrams are ignored.
        UdpSocket::bind("127.0.0.1:0").unwrap().send_to(b"hello", a_addr).unwrap();
        to_a.send(vec![0x30, 1, 2, 3]).unwrap();
        let (learned, to_b) = up(&arx);
        assert_eq!(learned, format!("udp {b_addr}"));
        assert_eq!(packet(&arx), vec![0x30, 1, 2, 3]);
        to_b.send(region_hello(false, &[3])).unwrap();
        assert_eq!(regions(&brx), (false, vec![3]));
        to_b.send(vec![9; 255]).unwrap();
        assert_eq!(packet(&brx), vec![9; 255]);
    }

    /// A host's own datagrams (as with a broadcast) are dropped.
    #[test]
    fn udp_drops_own_datagrams() {
        let a = UdpSocket::bind("127.0.0.1:0").unwrap();
        let a_addr = a.local_addr().unwrap();
        let (atx, arx) = channel();
        let target = UdpTarget { name: "udp self".into(), addr: a_addr.to_string() };
        thread::spawn(move || run_udp(a, vec![target], atx));
        let (_, to_self) = up(&arx);
        to_self.send(vec![1, 2, 3]).unwrap();
        assert!(arx.recv_timeout(Duration::from_millis(800)).is_err());
    }

    #[test]
    fn port_baud_and_link_classes() {
        assert_eq!(parse_port_baud("COM5", 115_200).unwrap(), ("COM5".into(), 115_200));
        assert_eq!(parse_port_baud("COM5:9600", 115_200).unwrap(), ("COM5".into(), 9600));
        assert_eq!(parse_port_baud("/dev/ttyUSB0:921600", 1).unwrap(), ("/dev/ttyUSB0".into(), 921_600));
        assert!(!is_backbone(&serial_link_name("COM5", 115_200)));
        assert!(is_backbone(&serial_link_name("COM5", 921_600)));
        assert!(is_backbone("tcp out x:1") && is_backbone("udp 1.2.3.4:5"));
        assert!(!is_backbone("serial COM3") && !is_backbone("kiss COM4") && !is_backbone("wifi-modem x"));
    }
}
