//! Authenticated TLS transport between the physical gateway and a local radio worker.

use rustls::{ServerConfig, ServerConnection, StreamOwned, pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject}};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;

const AUTH_TIMEOUT: Duration = Duration::from_secs(5);
const READ_INTERVAL: Duration = Duration::from_millis(20);
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);
const HELLO: &[u8] = b"MAARSE-KISS/1 ";
// The gateway firmware waits two seconds for "OK". The worker has to be
// connected inside that window, so a worker that starts a moment late is
// given a short grace period instead of an immediate refusal.
const WORKER_WAIT: Duration = Duration::from_secs(1);

pub fn spawn(address: SocketAddr, worker_address: SocketAddr, data: &Path) -> io::Result<()> {
    if !worker_address.ip().is_loopback() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the MeshCore radio worker must listen on loopback",
        ));
    }
    let token = fs::read_to_string(data.join("meshcore-gateway-token.txt"))?;
    if !valid_token(token.trim().as_bytes()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid MeshCore gateway token",
        ));
    }
    let token_hash = Sha256::digest(token.trim().as_bytes());
    let cert = fs::File::open(data.join("meshcore-gateway-cert.pem"))?;
    let certs = CertificateDer::pem_reader_iter(cert)
        .collect::<Result<Vec<_>, _>>().map_err(io::Error::other)?;
    let key = PrivateKeyDer::from_pem_reader(fs::File::open(data.join("meshcore-gateway-key.pem"))?)
        .map_err(io::Error::other)?;
    let config = Arc::new(
        ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(io::Error::other)?
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .map_err(io::Error::other)?,
    );
    let gateway = TcpListener::bind(address)?;
    let worker = TcpListener::bind(worker_address)?;
    worker.set_nonblocking(true)?;
    if let Ok(mut shown) = ADDRESS.lock() {
        *shown = address.to_string();
    }
    ENABLED.store(true, Ordering::Relaxed);
    std::thread::Builder::new()
        .name("meshcore-gateway".into())
        .spawn(move || {
            for socket in gateway.incoming() {
                let Ok(socket) = socket else { break };
                let _ = serve(socket, &worker, config.clone(), token_hash.as_slice());
                GATEWAY_UP.store(false, Ordering::Relaxed);
                WORKER_UP.store(false, Ordering::Relaxed);
            }
        })?;
    Ok(())
}

static ENABLED: AtomicBool = AtomicBool::new(false);
static GATEWAY_UP: AtomicBool = AtomicBool::new(false);
static WORKER_UP: AtomicBool = AtomicBool::new(false);
static CONNECTED_SINCE: AtomicU64 = AtomicU64::new(0);
static ADDRESS: Mutex<String> = Mutex::new(String::new());

/// Gateway state for the backbone dashboard.
pub fn status() -> serde_json::Value {
    let gateway_up = GATEWAY_UP.load(Ordering::Relaxed);
    serde_json::json!({
        "enabled": ENABLED.load(Ordering::Relaxed),
        "address": ADDRESS.lock().map(|shown| shown.clone()).unwrap_or_default(),
        "gatewayConnected": gateway_up,
        "workerConnected": WORKER_UP.load(Ordering::Relaxed),
        "connectedSince": if gateway_up { CONNECTED_SINCE.load(Ordering::Relaxed) } else { 0 },
    })
}

fn valid_token(token: &[u8]) -> bool {
    (32..=128).contains(&token.len())
        && token
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

struct GatewaySocket {
    socket: TcpStream,
    deadline: Option<Instant>,
}

impl Read for GatewaySocket {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if let Some(deadline) = self.deadline {
            self.socket.set_read_timeout(Some(time_left(deadline)?))?;
        }
        self.socket.read(buffer)
    }
}

impl Write for GatewaySocket {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> { self.socket.write(buffer) }
    fn flush(&mut self) -> io::Result<()> { self.socket.flush() }
}

fn serve(
    socket: TcpStream,
    worker: &TcpListener,
    config: Arc<ServerConfig>,
    token_hash: &[u8],
) -> io::Result<()> {
    socket.set_write_timeout(Some(WRITE_TIMEOUT))?;
    socket.set_nodelay(true)?;
    let deadline = Instant::now() + AUTH_TIMEOUT;
    let mut radio = StreamOwned::new(
        ServerConnection::new(config).map_err(io::Error::other)?,
        GatewaySocket { socket, deadline: Some(deadline) },
    );
    while radio.conn.is_handshaking() {
        radio.conn.complete_io(&mut radio.sock)?;
    }
    let mut hello = Vec::with_capacity(HELLO.len() + 128);
    loop {
        let mut byte = [0];
        radio.read_exact(&mut byte)?;
        if byte[0] == b'\n' {
            break;
        }
        if hello.len() == HELLO.len() + 128 {
            return Ok(());
        }
        hello.push(byte[0]);
    }
    let Some(token) = hello.strip_prefix(HELLO) else {
        return Ok(());
    };
    if !valid_token(token) || !bool::from(Sha256::digest(token).as_slice().ct_eq(token_hash)) {
        return Ok(());
    }
    let mut worker = accept_worker(worker)?;
    WORKER_UP.store(true, Ordering::Relaxed);
    worker.set_nonblocking(false)?;
    worker.set_nodelay(true)?;
    radio.sock.deadline = None;
    for socket in [&radio.sock.socket, &worker] {
        socket.set_read_timeout(Some(READ_INTERVAL))?;
        socket.set_write_timeout(Some(WRITE_TIMEOUT))?;
    }
    radio.write_all(b"OK\n")?;
    CONNECTED_SINCE.store(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or(0),
        Ordering::Relaxed,
    );
    GATEWAY_UP.store(true, Ordering::Relaxed);
    let mut buffer = [0; 4096];
    while relay(&mut worker, &mut radio, &mut buffer)?
        && relay(&mut radio, &mut worker, &mut buffer)?
    {}
    Ok(())
}

fn accept_worker(listener: &TcpListener) -> io::Result<TcpStream> {
    let deadline = Instant::now() + WORKER_WAIT;
    loop {
        match listener.accept() {
            Ok((socket, _)) => return Ok(socket),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    crate::settings::report(
                        "MeshCore gateway authenticated, but no radio worker is connected",
                        true,
                    );
                    return Err(io::Error::new(
                        io::ErrorKind::NotConnected,
                        "MeshCore radio worker is not connected",
                    ));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => return Err(error),
        }
    }
}

fn time_left(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|left| !left.is_zero())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                "MeshCore gateway authentication timed out",
            )
        })
}

fn relay(
    source: &mut impl Read,
    destination: &mut impl Write,
    buffer: &mut [u8],
) -> io::Result<bool> {
    match source.read(buffer) {
        Ok(0) => Ok(false),
        Ok(count) => {
            destination.write_all(&buffer[..count])?;
            Ok(true)
        }
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
            ) =>
        {
            Ok(true)
        }
        Err(error) => Err(error),
    }
}
