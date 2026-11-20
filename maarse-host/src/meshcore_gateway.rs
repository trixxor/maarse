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
/// First line of a pairing exchange: `MAARSE-PAIR/1 <secret>`. The server answers
/// `PAIRED <token>` and a second line with the authority certificate (the base64
/// body), or `ERR`, and closes the connection.
const PAIR_HELLO: &[u8] = b"MAARSE-PAIR/1 ";
/// Longest pairing code the dashboard hands out. It has to be easy to copy.
const PAIRING_CODE_MAX: usize = 120;
/// A pairing code works for this long, and once.
const PAIRING_LIFETIME: u64 = 10 * 60;
/// Wrong secrets tolerated before the pending code is cancelled.
const PAIRING_MAX_FAILURES: u8 = 5;
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
static PAIRED_AT: AtomicU64 = AtomicU64::new(0);

/// A pairing code that has been created and not yet used or expired.
struct Pairing {
    secret_hash: [u8; 32],
    expires: u64,
    failures: u8,
}

static PAIRING: Mutex<Option<Pairing>> = Mutex::new(None);

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
                        let _ = serve(socket, &worker, config, &token_hash, &data);
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
    let pairing_expires = PAIRING
        .lock()
        .map(|pending| pending.as_ref().map(|p| p.expires).filter(|&expires| expires > unix_now()).unwrap_or(0))
        .unwrap_or(0);
    serde_json::json!({
        "pairingExpires": pairing_expires,
        "pairedAt": PAIRED_AT.load(Ordering::Relaxed),
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
    data: &Path,
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
    if let Some(secret) = hello.strip_prefix(PAIR_HELLO) {
        return pair(&mut radio, secret, data);
    }
    let Some(token) = hello.strip_prefix(HELLO) else {
        return Ok(());
    };
    if !valid_token(token) || !bool::from(Sha256::digest(token).as_slice().ct_eq(token_hash)) {
        report_often(&LAST_TOKEN_REFUSED, "MeshCore gateway refused: it sent a token this server does not know. Pair it again.");
        refuse(&mut radio, "token");
        return Ok(());
    }
    let mut worker = match accept_worker(worker) {
        Ok(worker) => worker,
        Err(error) => {
            refuse(&mut radio, "worker");
            return Err(error);
        }
    };
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

static LAST_TOKEN_REFUSED: AtomicU64 = AtomicU64::new(0);
static LAST_NO_WORKER: AtomicU64 = AtomicU64::new(0);

/// Logs a problem at most once a minute per kind: a gateway that cannot connect
/// tries again every few seconds.
fn report_often(last: &AtomicU64, text: &str) {
    let now = unix_now();
    if now.saturating_sub(last.load(Ordering::Relaxed)) >= 60 {
        last.store(now, Ordering::Relaxed);
        crate::settings::report(text, true);
    }
}

/// Tells the gateway why it is not let in: `ERR token` (unknown token) or
/// `ERR worker` (no radio worker is connected). It then shows the reason.
fn refuse(radio: &mut StreamOwned<ServerConnection, GatewaySocket>, reason: &str) {
    let _ = radio.write_all(format!("ERR {reason}\n").as_bytes());
    let _ = radio.flush();
    radio.conn.send_close_notify();
    let _ = radio.flush();
}

/// Answers a pairing request: a valid, unused, unexpired secret gets a new
/// gateway token (which replaces the old one); anything else gets `ERR`.
fn pair(
    radio: &mut StreamOwned<ServerConnection, GatewaySocket>,
    secret: &[u8],
    data: &Path,
) -> io::Result<()> {
    let reply = if take_pairing(secret) {
        match create_token(data).and_then(|token| authority_body(data).map(|authority| (token, authority))) {
            Ok((token, authority)) => {
                PAIRED_AT.store(unix_now(), Ordering::Relaxed);
                crate::settings::report("MeshCore gateway paired; it has a new token", false);
                format!("PAIRED {token}\n{authority}\n")
            }
            Err(error) => {
                crate::settings::report(format!("MeshCore gateway pairing failed: {error}"), true);
                "ERR\n".to_string()
            }
        }
    } else {
        "ERR\n".to_string()
    };
    radio.write_all(reply.as_bytes())?;
    radio.flush()?;
    radio.conn.send_close_notify();
    radio.flush()
}

/// Checks a secret against the pending code. A match uses the code up; too many
/// wrong secrets cancel it.
fn take_pairing(secret: &[u8]) -> bool {
    let mut pending = PAIRING.lock().unwrap_or_else(|error| error.into_inner());
    let Some(code) = pending.as_mut() else { return false };
    if unix_now() >= code.expires {
        *pending = None;
        return false;
    }
    if bool::from(Sha256::digest(secret).as_slice().ct_eq(&code.secret_hash)) {
        *pending = None;
        return true;
    }
    code.failures += 1;
    if code.failures >= PAIRING_MAX_FAILURES {
        *pending = None;
        crate::settings::report("MeshCore pairing code cancelled after too many wrong attempts", true);
    }
    false
}

/// The base64 body of the authority certificate: one line, without the PEM markers.
fn authority_body(data: &Path) -> Result<String, String> {
    let body: String = read_trimmed(&data.join(AUTHORITY_CERT_FILE))
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect();
    if body.is_empty() {
        return Err("The gateway authority certificate is missing.".into());
    }
    Ok(body)
}

/// The first 16 bytes of the SHA-256 of the server certificate, in hex. The
/// gateway checks this against the certificate it is shown, before it sends
/// the pairing secret, so a server it has never met can be recognised.
fn certificate_pin(data: &Path) -> Result<String, String> {
    let pem = fs::read(data.join(CERT_FILE)).map_err(|e| e.to_string())?;
    let der = CertificateDer::pem_slice_iter(&pem)
        .next()
        .ok_or_else(|| "The gateway certificate is missing.".to_string())?
        .map_err(|_| "The gateway certificate is not valid PEM.".to_string())?;
    Ok(Sha256::digest(der.as_ref())[..16].iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Creates a pairing code for a new gateway, replacing any pending one:
/// `MP1;<name>;<port>;<secret>;<pin>`, at most 120 characters. The secret works
/// once, for ten minutes. The pin identifies this server's certificate. The
/// gateway checks it, sends the secret, and gets its own token and the authority
/// certificate over that connection.
pub fn create_pairing(data: &Path) -> Result<(String, u64), String> {
    let host = read_trimmed(&data.join(HOST_FILE));
    if !valid_host(&host) {
        return Err("Save the gateway name and port first.".into());
    }
    let port = config_listen(data)
        .parse::<SocketAddr>()
        .map_err(|_| "Save the gateway name and port first.".to_string())?
        .port();
    ensure_certificates(data, &host)?;
    authority_body(data)?;
    let pin = certificate_pin(data)?;
    // 80 bits: the code works once, for ten minutes, and is cancelled after five wrong guesses.
    let mut bytes = [0u8; 10];
    rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut bytes);
    let secret: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    let code = format!("MP1;{host};{port};{secret};{pin}");
    if code.len() > PAIRING_CODE_MAX {
        return Err(format!("The gateway name is too long for a pairing code of at most {PAIRING_CODE_MAX} characters. Use a shorter name."));
    }
    let expires = unix_now() + PAIRING_LIFETIME;
    let mut secret_hash = [0u8; 32];
    secret_hash.copy_from_slice(&Sha256::digest(secret.as_bytes()));
    *PAIRING.lock().unwrap_or_else(|error| error.into_inner()) = Some(Pairing { secret_hash, expires, failures: 0 });
    Ok((code, expires))
}

fn accept_worker(listener: &TcpListener) -> io::Result<TcpStream> {
    let deadline = Instant::now() + WORKER_WAIT;
    loop {
        match listener.accept() {
            Ok((socket, _)) => return Ok(socket),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    report_often(
                        &LAST_NO_WORKER,
                        "MeshCore gateway authenticated, but no radio worker is connected to this server",
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
    serde_json::json!({
        "listen": config_listen(data),
        "host": read_trimmed(&data.join(HOST_FILE)),
        "token": read_trimmed(&data.join(TOKEN_FILE)),
        "authorityPem": read_trimmed(&data.join(AUTHORITY_CERT_FILE)),
        "certificateExpires": read_trimmed(&data.join(EXPIRES_FILE)).parse::<u64>().unwrap_or(0),
        "overriddenByCommandLine": crate::cli_args().iter().any(|arg| arg == "--meshcore-gateway"),
    })
}

/// The saved gateway listen address ("0.0.0.0:4343"), or empty.
fn config_listen(data: &Path) -> String {
    crate::settings::load(data)
        .lines()
        .find_map(|line| line.trim().strip_prefix("--meshcore-gateway").map(|value| value.trim().to_string()))
        .unwrap_or_default()
}

fn read_trimmed(path: &Path) -> String {
    fs::read_to_string(path).map(|text| text.trim().to_string()).unwrap_or_default()
}

/// Saves the gateway settings and makes sure the certificates match the name.
/// A token is created when none exists. Use `create_token` to replace it. The caller
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
    if !valid_token(read_trimmed(&data.join(TOKEN_FILE)).as_bytes()) {
        create_token(data)?;
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

/// Creates a new gateway token and returns it. The gateway must be given the
/// new token before it can connect again.
pub fn create_token(data: &Path) -> Result<String, String> {
    let mut bytes = [0u8; 32];
    rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut bytes);
    let token: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    write_file(&data.join(TOKEN_FILE), &token, true)?;
    Ok(token)
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
    // Without a token the hash is empty: no gateway can connect with a token yet,
    // but a gateway can still pair and get one.
    let token = read_trimmed(&data.join(TOKEN_FILE));
    let token_hash = if valid_token(token.as_bytes()) { Sha256::digest(token.as_bytes()).to_vec() } else { Vec::new() };
    ensure_certificates(data, &host)?;
    let certificate = fs::read(data.join(CERT_FILE)).map_err(|e| e.to_string())?;
    let key = fs::read(data.join(KEY_FILE)).map_err(|e| e.to_string())?;
    Ok((tls_config(&certificate, &key)?, token_hash))
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
    fn create_token_writes_a_valid_token() {
        let dir = std::env::temp_dir().join(format!("maarse-token-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let token = create_token(&dir).unwrap();
        assert!(valid_token(token.as_bytes()));
        assert_eq!(read_trimmed(&dir.join(TOKEN_FILE)), token);
        assert_ne!(create_token(&dir).unwrap(), token);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn save_config_rejects_bad_input_before_writing() {
        let dir = std::env::temp_dir().join(format!("maarse-save-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        for update in [
            serde_json::json!({"listen": "nonsense", "host": "backbone.example.org"}),
            serde_json::json!({"listen": "0.0.0.0:0", "host": "backbone.example.org"}),
            serde_json::json!({"listen": "0.0.0.0:4343", "host": "bad host"}),
            serde_json::json!({"listen": "0.0.0.0:4343"}),
        ] {
            assert!(save_config(&dir, &update).is_err(), "{update}");
        }
        assert!(!dir.join(TOKEN_FILE).exists());
        assert!(!dir.join(AUTHORITY_CERT_FILE).exists());
        fs::remove_dir_all(&dir).ok();
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

    /// One test function, because the pending pairing code is process-wide state.
    #[test]
    fn pairing_gives_a_new_token_once() {
        let dir = std::env::temp_dir().join(format!("maarse-pairing-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(crate::settings::file(&dir), "--meshcore-gateway 0.0.0.0:4343\n").unwrap();
        fs::write(dir.join(HOST_FILE), "backbone.example.org").unwrap();
        let old_token = create_token(&dir).unwrap();

        // The code carries the name, the port, a secret and a pin of the server certificate.
        let (code, expires) = create_pairing(&dir).unwrap();
        assert!(expires > unix_now());
        let fields: Vec<&str> = code.split(';').collect();
        assert_eq!(fields.len(), 5, "{code}");
        assert_eq!((fields[0], fields[1], fields[2]), ("MP1", "backbone.example.org", "4343"));
        // The gateway firmware expects a secret of 20 and a pin of 32 hex characters.
        assert_eq!(fields[3].len(), 20);
        assert_eq!(fields[4].len(), 32);
        assert!(fields[3..].iter().all(|field| field.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))));
        assert!(code.len() <= PAIRING_CODE_MAX, "{}", code.len());
        println!("pairing code length: {}", code.len());
        // The pin is what a gateway computes from the certificate it is shown.
        let shown = CertificateDer::pem_slice_iter(&fs::read(dir.join(CERT_FILE)).unwrap()).next().unwrap().unwrap();
        let expected: String = Sha256::digest(shown.as_ref())[..16].iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(fields[4], expected);
        let authority = fs::read_to_string(dir.join(AUTHORITY_CERT_FILE)).unwrap();
        let authority_line: String = authority.lines().filter(|line| !line.starts_with("-----")).collect();
        let secret = fields[3].to_string();

        // A wrong secret is refused and does not use the code up.
        assert_eq!(pair_over_tls(&dir, &authority, &"0".repeat(20)), "ERR");
        // The right secret gives a new token and the authority, and replaces the old token.
        let reply = pair_over_tls(&dir, &authority, &secret);
        let mut lines = reply.lines();
        let token = lines.next().unwrap().strip_prefix("PAIRED ").unwrap_or_else(|| panic!("{reply}"));
        assert!(valid_token(token.as_bytes()));
        assert_ne!(token, old_token);
        assert_eq!(read_trimmed(&dir.join(TOKEN_FILE)), token);
        assert_eq!(lines.next(), Some(authority_line.as_str()));
        // A code works once.
        assert_eq!(pair_over_tls(&dir, &authority, &secret), "ERR");

        // An expired code is refused, and five wrong secrets cancel a code.
        let (code, _) = create_pairing(&dir).unwrap();
        let secret = code.split(';').nth(3).unwrap().to_string();
        PAIRING.lock().unwrap().as_mut().unwrap().expires = unix_now() - 1;
        assert_eq!(pair_over_tls(&dir, &authority, &secret), "ERR");
        let (code, _) = create_pairing(&dir).unwrap();
        let secret = code.split(';').nth(3).unwrap().to_string();
        for _ in 0..PAIRING_MAX_FAILURES {
            assert_eq!(pair_over_tls(&dir, &authority, &"1".repeat(32)), "ERR");
        }
        assert_eq!(pair_over_tls(&dir, &authority, &secret), "ERR");

        // A gateway name that makes the code longer than the limit is refused.
        fs::write(dir.join(HOST_FILE), format!("{}.example.org", "a".repeat(60))).unwrap();
        assert!(create_pairing(&dir).unwrap_err().contains("too long"));
        fs::remove_dir_all(&dir).ok();
    }

    /// A gateway that trusts only `authority` sends a pairing request.
    fn pair_over_tls(dir: &Path, authority: &str, secret: &str) -> String {
        hello_over_tls(dir, authority, &format!("MAARSE-PAIR/1 {secret}"))
    }

    #[test]
    fn gateway_is_told_why_it_is_refused() {
        let dir = std::env::temp_dir().join(format!("maarse-refuse-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(HOST_FILE), "backbone.example.org").unwrap();
        let token = create_token(&dir).unwrap();
        ensure_certificates(&dir, "backbone.example.org").unwrap();
        let authority = fs::read_to_string(dir.join(AUTHORITY_CERT_FILE)).unwrap();
        // A token the server does not know.
        let unknown = "MAARSE-KISS/1 not-a-token-the-server-knows-at-all-1234";
        assert_eq!(hello_over_tls(&dir, &authority, unknown), "ERR token");
        // The right token, but no radio worker has connected to the server.
        assert_eq!(hello_over_tls(&dir, &authority, &format!("MAARSE-KISS/1 {token}")), "ERR worker");
        fs::remove_dir_all(&dir).ok();
    }

    /// Connects as a gateway that trusts only `authority`, sends `hello` as its
    /// first line and returns what the server answers.
    fn hello_over_tls(dir: &Path, authority: &str, hello: &str) -> String {
        let (config, token_hash) = gateway_credentials(dir).unwrap();
        let mut roots = RootCertStore::empty();
        for certificate in CertificateDer::pem_slice_iter(authority.as_bytes()) {
            roots.add(certificate.unwrap()).unwrap();
        }
        let client = ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = TcpListener::bind("127.0.0.1:0").unwrap();
        worker.set_nonblocking(true).unwrap();
        let data = dir.to_path_buf();
        let gateway = std::thread::spawn(move || {
            let (socket, _) = listener.accept().unwrap();
            let _ = serve(socket, &worker, config, &token_hash, &data);
        });
        let reply = {
            let socket = TcpStream::connect(address).unwrap();
            let name = ServerName::try_from("backbone.example.org").unwrap();
            let connection = ClientConnection::new(Arc::new(client), name).unwrap();
            let mut stream = StreamOwned::new(connection, socket);
            stream.write_all(format!("{hello}\n").as_bytes()).unwrap();
            let mut reply = String::new();
            let _ = stream.read_to_string(&mut reply);
            reply
        };
        gateway.join().unwrap();
        reply.trim().to_string()
    }

    fn gateway_credentials_for_test(dir: &Path) -> (Arc<ServerConfig>, Vec<u8>) {
        gateway_credentials_with(dir, "backbone.example.org")
    }

    fn gateway_credentials_with(dir: &Path, host: &str) -> (Arc<ServerConfig>, Vec<u8>) {
        fs::write(dir.join(HOST_FILE), host).unwrap();
        create_token(dir).unwrap();
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
