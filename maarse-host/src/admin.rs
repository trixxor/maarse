//! Admin login for maarse-server's dashboard.
//!
//! * New admin passwords are stored as Argon2id PHC strings in `<data>/admin.hash`.
//!   Existing SHA-256 password records are supported until the password is reset.
//! * At the first start a random password is made and written
//!   to `<data>/admin-password.txt` (delete that file after reading it);
//!   `maarse-server --set-admin-password <password>` sets another.
//! * A login gives a session cookie (HttpOnly, SameSite=Strict, 12 hours).
//! * After 5 wrong passwords from one IP address it is locked out for 15
//!   minutes; every wrong attempt waits 1 s.
//!
//! The dashboard is plain HTTP: over the internet put it behind HTTPS (a
//! reverse proxy such as Caddy or nginx) or reach it through an SSH tunnel,
//! or the password and cookie can be read on the way.

use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use argon2::password_hash::SaltString;
use rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::IpAddr;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const LEGACY_ITERATIONS: u32 = 200_000;
const SESSION: Duration = Duration::from_secs(12 * 3600);
const MAX_FAILS: u32 = 5;
const LOCKOUT: Duration = Duration::from_secs(15 * 60);

static SESSIONS: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);
/// Where sessions are kept across a restart ("Save and restart" in the
/// dashboard): SHA-256 of each token and its Unix expiry, never the token.
static SESSION_FILE: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);

fn token_hash(t: &str) -> String {
    hex(&Sha256::digest(t.as_bytes()))
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Loads the sessions saved before a restart (call once at start).
pub fn load_sessions(data_dir: &Path) {
    let p = data_dir.join("sessions.txt");
    *SESSION_FILE.lock().unwrap_or_else(|e| e.into_inner()) = Some(p.clone());
    let text = std::fs::read_to_string(&p).unwrap_or_default();
    let mut s = SESSIONS.lock().unwrap_or_else(|e| e.into_inner());
    let m = s.get_or_insert_with(HashMap::new);
    let now = unix_now();
    for l in text.lines() {
        if let Some((h, exp)) = l.split_once(' ')
            && let Ok(exp) = exp.trim().parse::<u64>()
            && exp > now
            && valid_token(h)
        {
            // Stored as hash; the age is rebuilt from the expiry.
            let left = Duration::from_secs(exp - now).min(SESSION);
            m.insert(h.to_owned(), Instant::now() - (SESSION - left));
        }
    }
}

fn save_sessions(m: &HashMap<String, Instant>) {
    let Some(p) = SESSION_FILE.lock().unwrap_or_else(|e| e.into_inner()).clone() else { return };
    let now = unix_now();
    let text: String = m.iter().filter_map(|(hash, created)| {
        let remaining = SESSION.checked_sub(created.elapsed())?;
        Some(format!("{hash} {}\n", now + remaining.as_secs()))
    }).collect();
    let _ = std::fs::write(p, text);
}
static FAILS: Mutex<Option<HashMap<IpAddr, (u32, Instant)>>> = Mutex::new(None);
static VERIFY_LOGIN: Mutex<()> = Mutex::new(());

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn derive(salt: &[u8], password: &str, iterations: u32) -> [u8; 32] {
    let mut h: [u8; 32] = Sha256::new().chain_update(salt).chain_update(password.as_bytes()).finalize().into();
    for _ in 1..iterations {
        h = Sha256::new().chain_update(salt).chain_update(h).finalize().into();
    }
    h
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Stores a new admin password and invalidates all existing dashboard sessions.
pub fn set_password(data_dir: &Path, password: &str) -> std::io::Result<()> {
    use std::io::Write;

    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|_| std::io::Error::other("Could not hash admin password"))?;
    std::fs::create_dir_all(data_dir)?;
    let _verification = VERIFY_LOGIN.lock().unwrap_or_else(|error| error.into_inner());
    let mut sessions = SESSIONS.lock().unwrap_or_else(|error| error.into_inner());
    match std::fs::remove_file(data_dir.join("sessions.txt")) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    if let Some(sessions) = sessions.as_mut() { sessions.clear(); }
    let path = data_dir.join("admin.hash");
    let temp = data_dir.join(format!("admin-{}.tmp", salt.as_str().replace('/', "_")));
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)] {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp)?;
    let result = writeln!(file, "{hash}").and_then(|()| file.sync_all());
    drop(file);
    let result = result.and_then(|()| std::fs::rename(&temp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result
}

/// Makes a password at the first start and saves it for the local operator.
pub fn ensure_password(data_dir: &Path) -> std::io::Result<Option<String>> {
    if data_dir.join("admin.hash").exists() {
        return Ok(None);
    }
    const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyzACDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut b = [0u8; 18];
    OsRng.fill_bytes(&mut b);
    let pw: String = b.iter().map(|x| ALPHABET[*x as usize % ALPHABET.len()] as char).collect();
    use std::io::Write;
    std::fs::create_dir_all(data_dir)?;
    let bootstrap = data_dir.join("admin-password.txt");
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)] {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&bootstrap)?;
    let saved = writeln!(file, "{pw}\n(delete this file once you have the password)").and_then(|()| file.sync_all());
    drop(file);
    if let Err(error) = saved.and_then(|()| set_password(data_dir, &pw)) {
        let _ = std::fs::remove_file(bootstrap);
        return Err(error);
    }
    Ok(Some(pw))
}

/// Checks a password against `<data>/admin.hash`.
pub fn check_password(data_dir: &Path, password: &str) -> bool {
    let Ok(s) = std::fs::read_to_string(data_dir.join("admin.hash")) else { return false };
    let record = s.trim();
    if record.starts_with("$argon2id$") {
        return PasswordHash::new(record)
            .is_ok_and(|hash| Argon2::default().verify_password(password.as_bytes(), &hash).is_ok());
    }
    let mut it = record.split(':');
    let (Some(salt), Some(iter), Some(hash)) = (it.next(), it.next(), it.next()) else { return false };
    let unhex = |h: &str| -> Vec<u8> { (0..h.len()).step_by(2).filter_map(|i| h.get(i..i + 2).and_then(|x| u8::from_str_radix(x, 16).ok())).collect() };
    let iterations = iter.parse().unwrap_or(LEGACY_ITERATIONS).clamp(1, 10_000_000);
    ct_eq(&derive(&unhex(salt), password, iterations), &unhex(hash))
}

/// Is this IP locked out after too many wrong passwords?
pub fn locked(ip: IpAddr) -> bool {
    let mut f = FAILS.lock().unwrap_or_else(|e| e.into_inner());
    let m = f.get_or_insert_with(HashMap::new);
    m.retain(|_, (_, t)| t.elapsed() < LOCKOUT);
    m.get(&ip).is_some_and(|(n, _)| *n >= MAX_FAILS)
}

/// Tries a login; a session token on success.
pub fn login(data_dir: &Path, ip: IpAddr, password: &str) -> Option<String> {
    if locked(ip) {
        return None;
    }
    let verification = VERIFY_LOGIN.try_lock().ok()?;
    if locked(ip) { return None; }
    if !check_password(data_dir, password) {
        drop(verification);
        let mut f = FAILS.lock().unwrap_or_else(|e| e.into_inner());
        let e = f.get_or_insert_with(HashMap::new).entry(ip).or_insert((0, Instant::now()));
        e.0 += 1;
        e.1 = Instant::now();
        drop(f);
        std::thread::sleep(Duration::from_secs(1));
        return None;
    }
    if let Some(m) = FAILS.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        m.remove(&ip);
    }
    let mut b = [0u8; 32];
    OsRng.fill_bytes(&mut b);
    let token = hex(&b);
    let mut s = SESSIONS.lock().unwrap_or_else(|e| e.into_inner());
    let m = s.get_or_insert_with(HashMap::new);
    m.retain(|_, t| t.elapsed() < SESSION);
    m.insert(token_hash(&token), Instant::now());
    save_sessions(m);
    drop(verification);
    Some(token)
}

pub fn logout(token: &str) {
    if let Some(m) = SESSIONS.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        m.remove(&token_hash(token));
        save_sessions(m);
    }
}

fn valid_token(token: &str) -> bool {
    token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

/// The session token from a request's Cookie header, if it is valid.
pub fn session(head: &str) -> Option<String> {
    let cookie = head.split("\r\n").skip(1).find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim().eq_ignore_ascii_case("cookie").then(|| v.trim().to_string())
    })?;
    let token = cookie.split(';').find_map(|c| c.trim().strip_prefix("maarse_session="))?.to_string();
    if !valid_token(&token) { return None; }
    let s = SESSIONS.lock().unwrap_or_else(|e| e.into_inner());
    let ok = s
        .as_ref()
        .and_then(|m| m.get(&token_hash(&token)))
        .is_some_and(|t| t.elapsed() < SESSION);
    ok.then_some(token)
}

pub fn cookie(token: &str) -> String {
    format!("maarse_session={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}", SESSION.as_secs())
}

pub const LOGIN_HTML: &str = r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Server login</title>
<style>
:root{--bg:#eef2f0;--panel:#fbfcfb;--ink:#14201b;--muted:#5d6d66;--line:#d8e0dc;--accent:#1f6f5c;--bad:#b4472a}
@media (prefers-color-scheme:dark){:root{--bg:#0f1513;--panel:#151d1a;--ink:#e3ebe7;--muted:#8fa199;--line:#26332e;--accent:#5cc0a0;--bad:#ec8e70}}
body{margin:0;min-height:100vh;display:grid;place-items:center;background:var(--bg);color:var(--ink);font:15px/1.5 system-ui,sans-serif;padding:16px}
form{background:var(--panel);border:1px solid var(--line);border-radius:12px;padding:24px;width:min(360px,100%)}
h1{font-size:18px;margin:0 0 4px}p{color:var(--muted);margin:0 0 16px;font-size:13px}
input{width:100%;box-sizing:border-box;font:inherit;padding:8px 10px;border-radius:8px;border:1px solid var(--line);background:var(--bg);color:var(--ink)}
button{margin-top:12px;width:100%;font:inherit;font-weight:600;padding:8px;border-radius:8px;border:0;background:var(--accent);color:var(--panel);cursor:pointer}
.err{color:var(--bad);font-size:13px;margin-top:10px}
</style></head><body>
<form method="post" action="/login">
<h1>Må:rse server</h1><p>Administrator login for the telemetry dashboard.</p>
<input type="password" name="password" placeholder="Password" autofocus autocomplete="current-password">
<button type="submit">Log in</button>
<!--ERR-->
</form></body></html>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_login_and_lockout() {
        let dir = std::env::temp_dir().join(format!("maarse-admin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let pw = ensure_password(&dir).unwrap().expect("a first password");
        assert!(ensure_password(&dir).unwrap().is_none(), "only once");
        assert!(std::fs::read_to_string(dir.join("admin.hash")).unwrap().starts_with("$argon2id$"));
        assert!(check_password(&dir, &pw));
        assert!(!check_password(&dir, "wrong"));
        let ip: IpAddr = "203.0.113.9".parse().unwrap();
        let tok = login(&dir, ip, &pw).expect("session");
        let head = format!("GET / HTTP/1.1\r\nHost: x\r\nCookie: a=b; maarse_session={tok}\r\n\r\n");
        assert_eq!(session(&head), Some(tok.clone()));
        // A restart keeps the session (only its hash is on disk).
        load_sessions(&dir);
        let tok2 = login(&dir, ip, &pw).unwrap();
        SESSIONS.lock().unwrap().as_mut().unwrap().clear();
        load_sessions(&dir);
        let head2 = format!("GET / HTTP/1.1\r\nCookie: maarse_session={tok2}\r\n\r\n");
        assert_eq!(session(&head2), Some(tok2.clone()), "session survives a restart");
        assert!(!std::fs::read_to_string(dir.join("sessions.txt")).unwrap().contains(&tok2), "only the hash is stored");
        let stored_hash = token_hash(&tok2);
        let stored_as_cookie = format!("GET / HTTP/1.1\r\nCookie: maarse_session={stored_hash}\r\n");
        assert!(session(&stored_as_cookie).is_none(), "the on-disk hash is not a session credential");
        logout(&tok2);
        assert_eq!(session(&head2), None);
        let tok3 = login(&dir, ip, &pw).unwrap();
        let head3 = format!("GET / HTTP/1.1\r\nCookie: maarse_session={tok3}\r\n");
        assert_eq!(session(&head3), Some(tok3));
        set_password(&dir, "a new password for this test").unwrap();
        assert!(session(&head3).is_none(), "password rotation invalidates active sessions");
        load_sessions(&dir);
        assert!(session(&head3).is_none(), "revoked sessions stay revoked after restart");
        assert!(check_password(&dir, "a new password for this test"));
        logout(&tok);
        assert_eq!(session(&head), None);
        set_password(&dir, "new one").unwrap();
        assert!(check_password(&dir, "new one") && !check_password(&dir, &pw));
        // Lockout (without the 1 s waits: set the counter directly).
        let bad: IpAddr = "198.51.100.1".parse().unwrap();
        FAILS.lock().unwrap().get_or_insert_with(HashMap::new).insert(bad, (MAX_FAILS, Instant::now()));
        assert!(locked(bad));
        assert!(login(&dir, bad, "new one").is_none(), "locked out even with the right password");
    }
}
