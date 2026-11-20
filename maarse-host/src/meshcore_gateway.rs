//! Authenticated TLS transport between the physical gateway and a local radio worker.
//!
//! The server keeps its own certificate authority for the gateway. The
//! dashboard shows the authority's certificate, which the gateway trusts, so
//! no certificate files have to be copied by hand. The server issues and
//! renews its own certificate for the gateway name.

use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};
use rustls::{
    ServerConfig, ServerConnection, StreamOwned,
    pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
};
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
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
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

const AUTHORITY_CERT_FILE: &str = "meshcore-gateway-ca.pem";
const AUTHORITY_KEY_FILE: &str = "meshcore-gateway-ca-key.pem";
const CERT_FILE: &str = "meshcore-gateway-cert.pem";
const KEY_FILE: &str = "meshcore-gateway-key.pem";
const HOST_FILE: &str = "meshcore-gateway-host.txt";
const TOKEN_FILE: &str = "meshcore-gateway-token.txt";
const EXPIRES_FILE: &str = "meshcore-gateway-cert-expires.txt";
const AUTHORITY_LIFETIME: u64 = 10 * 365 * 86400;
const CERT_LIFETIME: u64 = 365 * 86400;
/// The server certificate is replaced this long before it expires.
const RENEW_BEFORE: u64 = 30 * 86400;

static ENABLED: AtomicBool = AtomicBool::new(false);
static GATEWAY_UP: AtomicBool = AtomicBool::new(false);
static WORKER_UP: AtomicBool = AtomicBool::new(false);
static CONNECTED_SINCE: AtomicU64 = AtomicU64::new(0);
static ADDRESS: Mutex<String> = Mutex::new(String::new());

pub fn spawn(address: SocketAddr, worker_address: SocketAddr, data: &Path) -> io::Result<()> {
    if !worker_address.ip().is_loopback() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the MeshCore radio worker must listen on loopback",
        ));
    }
    let gateway = TcpListener::bind(address)?;
    let worker = TcpListener::bind(worker_address)?;
    worker.set_nonblocking(true)?;
    if let Ok(mut shown) = ADDRESS.lock() {
        *shown = address.to_string();
    }
    ENABLED.store(true, Ordering::Relaxed);
    if let Err(error) = gateway_credentials(data) {
        crate::settings::report(format!("MeshCore gateway not ready: {error}"), false);
    }
    let data = data.to_path_buf();
    std::thread::Builder::new()
        .name("meshcore-gateway".into())
        .spawn(move || {
            for socket in gateway.incoming() {
                let Ok(socket) = socket else { break };
                // Read per connection, so dashboard changes apply without a restart.
                match gateway_credentials(&data) {
                    Ok((config, token_hash)) => {
                        let _ = serve(socket, &worker, config, &token_hash);
                    }
                    Err(error) => {
                        crate::settings::report(format!("MeshCore gateway refused: {error}"), true);
                    }
                }
                GATEWAY_UP.store(false, Ordering::Relaxed);
                WORKER_UP.store(false, Ordering::Relaxed);
            }
        })?;
    Ok(())
}

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
    CONNECTED_SINCE.store(unix_now(), Ordering::Relaxed);
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

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// What the dashboard shows and edits: the listen address (kept in settings.txt),
/// the gateway name, the token, and the authority certificate the gateway trusts.
pub fn config_json(data: &Path) -> serde_json::Value {
    let listen = crate::settings::load(data)
        .lines()
        .find_map(|line| line.trim().strip_prefix("--meshcore-gateway").map(|value| value.trim().to_string()))
        .unwrap_or_default();
    serde_json::json!({
        "listen": listen,
        "host": read_trimmed(&data.join(HOST_FILE)),
        "token": read_trimmed(&data.join(TOKEN_FILE)),
        "authorityPem": read_trimmed(&data.join(AUTHORITY_CERT_FILE)),
        "certificateExpires": read_trimmed(&data.join(EXPIRES_FILE)).parse::<u64>().unwrap_or(0),
        "overriddenByCommandLine": crate::cli_args().iter().any(|arg| arg == "--meshcore-gateway"),
    })
}

fn read_trimmed(path: &Path) -> String {
    fs::read_to_string(path).map(|text| text.trim().to_string()).unwrap_or_default()
}

/// Saves the gateway settings and makes sure the certificates match the name.
/// A new token is created when asked for or when none exists. The caller
/// restarts the server afterwards so the listener uses the new port.
pub fn save_config(data: &Path, update: &serde_json::Value) -> Result<(), String> {
    let listen: SocketAddr = update["listen"]
        .as_str()
        .unwrap_or("")
        .trim()
        .parse()
        .map_err(|_| "The listen address must look like 0.0.0.0:4343.".to_string())?;
    if listen.port() == 0 {
        return Err("Choose a port from 1 to 65535.".into());
    }
    let host = update["host"].as_str().unwrap_or("").trim().to_ascii_lowercase();
    if !valid_host(&host) {
        return Err("Enter the DNS name the gateway connects to, for example backbone.example.org.".into());
    }
    let token_path = data.join(TOKEN_FILE);
    if update["newToken"].as_bool().unwrap_or(false) || !valid_token(read_trimmed(&token_path).as_bytes()) {
        write_file(&token_path, &new_token(), true)?;
    }
    ensure_certificates(data, &host)?;
    let mut lines: Vec<String> = crate::settings::load(data)
        .lines()
        .filter(|line| !line.trim().starts_with("--meshcore-gateway"))
        .map(str::to_string)
        .collect();
    lines.push(format!("--meshcore-gateway {listen}"));
    crate::settings::save(data, &lines.join("\n"))
}

fn new_token() -> String {
    let mut bytes = [0u8; 32];
    rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Creates the gateway authority on first use. Creates a server certificate
/// for `host` when none exists, when the name changed, or when the current one
/// is close to expiry. The authority is kept, so the gateway needs its
/// certificate only once.
pub fn ensure_certificates(data: &Path, host: &str) -> Result<(), String> {
    let now = unix_now();
    let (authority_key, created) = match (
        fs::read_to_string(data.join(AUTHORITY_CERT_FILE)),
        fs::read_to_string(data.join(AUTHORITY_KEY_FILE)),
    ) {
        (Ok(_), Ok(key)) => (KeyPair::from_pem(&key).map_err(|e| e.to_string())?, false),
        _ => (create_authority(data, now)?, true),
    };
    let expires = read_trimmed(&data.join(EXPIRES_FILE)).parse::<u64>().unwrap_or(0);
    let current = !created
        && read_trimmed(&data.join(HOST_FILE)) == host
        && data.join(CERT_FILE).is_file()
        && data.join(KEY_FILE).is_file()
        && expires > now + RENEW_BEFORE;
    if current {
        return Ok(());
    }
    let authority = ca_params(now)?;
    let issuer = Issuer::from_params(&authority, authority_key);
    let leaf_key = KeyPair::generate().map_err(|e| e.to_string())?;
    let leaf = leaf_params(host, now)?
        .signed_by(&leaf_key, &issuer)
        .map_err(|e| e.to_string())?;
    write_file(&data.join(CERT_FILE), &leaf.pem(), false)?;
    write_file(&data.join(KEY_FILE), &leaf_key.serialize_pem(), true)?;
    write_file(&data.join(HOST_FILE), host, false)?;
    write_file(&data.join(EXPIRES_FILE), &(now + CERT_LIFETIME).to_string(), false)?;
    Ok(())
}

fn create_authority(data: &Path, now: u64) -> Result<KeyPair, String> {
    let key = KeyPair::generate().map_err(|e| e.to_string())?;
    let certificate = ca_params(now)?.self_signed(&key).map_err(|e| e.to_string())?;
    write_file(&data.join(AUTHORITY_CERT_FILE), &certificate.pem(), false)?;
    write_file(&data.join(AUTHORITY_KEY_FILE), &key.serialize_pem(), true)?;
    Ok(key)
}

fn ca_params(now: u64) -> Result<CertificateParams, String> {
    let mut params = CertificateParams::default();
    params.distinguished_name.push(DnType::CommonName, "Maarse MeshCore gateway CA");
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    set_validity(&mut params, now, AUTHORITY_LIFETIME)?;
    Ok(params)
}

fn leaf_params(host: &str, now: u64) -> Result<CertificateParams, String> {
    let mut params = CertificateParams::new(vec![host.to_string()]).map_err(|e| e.to_string())?;
    params.distinguished_name.push(DnType::CommonName, host);
    params.is_ca = IsCa::NoCa;
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    set_validity(&mut params, now, CERT_LIFETIME)?;
    Ok(params)
}

fn set_validity(params: &mut CertificateParams, now: u64, lifetime: u64) -> Result<(), String> {
    // One day of slack for clock differences between the server and the gateway.
    params.not_before = time::OffsetDateTime::from_unix_timestamp((now - 86400) as i64).map_err(|e| e.to_string())?;
    params.not_after = time::OffsetDateTime::from_unix_timestamp((now + lifetime) as i64).map_err(|e| e.to_string())?;
    Ok(())
}

/// Builds the TLS configuration from PEM text, as the listener uses it.
fn tls_config(certificate: &[u8], key: &[u8]) -> Result<Arc<ServerConfig>, String> {
    let certs = CertificateDer::pem_slice_iter(certificate)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "the gateway certificate is not valid PEM".to_string())?;
    if certs.is_empty() {
        return Err("the gateway certificate file is empty".into());
    }
    let key = PrivateKeyDer::from_pem_slice(key).map_err(|_| "the gateway key is not valid PEM".to_string())?;
    ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map(Arc::new)
        .map_err(|e| e.to_string())
}

/// The TLS configuration and the token hash for one gateway connection.
fn gateway_credentials(data: &Path) -> Result<(Arc<ServerConfig>, Vec<u8>), String> {
    let host = read_trimmed(&data.join(HOST_FILE));
    if !valid_host(&host) {
        return Err("no gateway name is set in the dashboard".into());
    }
    let token = read_trimmed(&data.join(TOKEN_FILE));
    if !valid_token(token.as_bytes()) {
        return Err("no gateway token is set in the dashboard".into());
    }
    ensure_certificates(data, &host)?;
    let certificate = fs::read(data.join(CERT_FILE)).map_err(|e| e.to_string())?;
    let key = fs::read(data.join(KEY_FILE)).map_err(|e| e.to_string())?;
    Ok((tls_config(&certificate, &key)?, Sha256::digest(token.as_bytes()).to_vec()))
}

fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

/// Writes a file through a temporary name. Private files are readable by the
/// owner only on Unix.
fn write_file(path: &Path, text: &str, private: bool) -> Result<(), String> {
    let pending = path.with_extension("tmp");
    fs::write(&pending, text).map_err(|e| format!("could not write {}: {e}", path.display()))?;
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&pending, fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
    }
    #[cfg(not(unix))]
    let _ = private;
    fs::rename(&pending, path).map_err(|e| format!("could not save {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::{
        ClientConfig, ClientConnection, RootCertStore,
        pki_types::{CertificateDer, ServerName, pem::PemObject},
    };

    #[test]
    fn valid_host_names() {
        assert!(valid_host("backbone.example.org"));
        assert!(valid_host("a-b.c1"));
        assert!(!valid_host(""));
        assert!(!valid_host("-bad.example"));
        assert!(!valid_host("bad-.example"));
        assert!(!valid_host("two..dots"));
        assert!(!valid_host("with space.example"));
        assert!(!valid_host("under_score.example"));
    }

    #[test]
    fn tls_config_rejects_garbage() {
        assert!(tls_config(b"not pem", b"not pem").is_err());
    }

    /// Issues certificates into a temporary folder and checks that a gateway
    /// trusting only the dashboard's authority accepts the name and nothing else.
    #[test]
    fn gateway_verifies_the_issued_certificate() {
        let dir = std::env::temp_dir().join(format!("maarse-gateway-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        ensure_certificates(&dir, "backbone.example.org").unwrap();
        let (config, _) = gateway_credentials_for_test(&dir);
        let authority = fs::read(dir.join(AUTHORITY_CERT_FILE)).unwrap();
        assert!(handshake(config.clone(), &authority, "backbone.example.org").is_ok());
        assert!(handshake(config, &authority, "other.example.org").is_err());
        fs::remove_dir_all(&dir).ok();
    }

    fn gateway_credentials_for_test(dir: &Path) -> (Arc<ServerConfig>, Vec<u8>) {
        gateway_credentials_with(dir, "backbone.example.org")
    }

    fn gateway_credentials_with(dir: &Path, host: &str) -> (Arc<ServerConfig>, Vec<u8>) {
        fs::write(dir.join(HOST_FILE), host).unwrap();
        fs::write(dir.join(TOKEN_FILE), new_token()).unwrap();
        gateway_credentials(dir).unwrap()
    }

    fn handshake(server: Arc<ServerConfig>, authority: &[u8], name: &str) -> Result<(), String> {
        let mut roots = RootCertStore::empty();
        for certificate in CertificateDer::pem_slice_iter(authority) {
            roots.add(certificate.map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        }
        let client = ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .with_root_certificates(roots)
            .with_no_client_auth();
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        let address = listener.local_addr().map_err(|e| e.to_string())?;
        let gateway = std::thread::spawn(move || -> Result<(), String> {
            let (mut socket, _) = listener.accept().map_err(|e| e.to_string())?;
            let mut connection = ServerConnection::new(server).map_err(|e| e.to_string())?;
            while connection.is_handshaking() {
                connection.complete_io(&mut socket).map_err(|e| e.to_string())?;
            }
            Ok(())
        });
        // The client socket is dropped before joining, so a refused handshake
        // cannot leave the gateway thread waiting for more bytes.
        let client_result = {
            let mut socket = TcpStream::connect(address).map_err(|e| e.to_string())?;
            let server_name = ServerName::try_from(name.to_string()).map_err(|e| e.to_string())?;
            let mut connection = ClientConnection::new(Arc::new(client), server_name).map_err(|e| e.to_string())?;
            (|| -> Result<(), String> {
                while connection.is_handshaking() {
                    connection.complete_io(&mut socket).map_err(|e| e.to_string())?;
                }
                Ok(())
            })()
        };
        let _ = gateway.join();
        client_result
    }
}
