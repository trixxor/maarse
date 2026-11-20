//! Web client over HTTP and the companion protocol over WebSocket (same port).

use crate::{In, next_id};
use std::io::{self, Cursor, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Sender, channel};
use std::thread;
use std::time::{Duration, Instant};
use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
use maarse_core::companion::FrameDecoder;
use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::{Error as WsError, Message};

/// The web client is built into the program, so there is only one file to run.
const INDEX_HTML: &str = include_str!("../../maarse-client/index.html");
/// maarse-server's dashboard: telemetry and everything passing through.
const SERVER_HTML: &str = include_str!("../../maarse-client/server.html");
/// The web flasher: installs firmware over USB (Web Serial) on blank boards.
const FLASH_HTML: &str = include_str!("../../maarse-client/flash.html");

/// The access key for remote access: read from `<data>/access-key.txt`
/// or created (32 hex characters) the first time.
pub fn access_token(data_dir: &std::path::Path) -> String {
    let p = data_dir.join("access-key.txt");
    if let Ok(t) = std::fs::read_to_string(&p) {
        let t = t.trim().to_string();
        if t.len() >= 16 {
            return t;
        }
    }
    use rand_core::RngCore;
    let mut b = [0u8; 16];
    rand_core::OsRng.fill_bytes(&mut b);
    let t: String = b.iter().map(|x| format!("{x:02x}")).collect();
    let _ = std::fs::write(&p, format!("{t}\n"));
    t
}

const MAX_WEB_CONNECTIONS: usize = 64;
const MAX_HTTP_HEAD: usize = 16 * 1024;
const HTTP_TIMEOUT: Duration = Duration::from_secs(5);

struct ConnectionSlot(Arc<AtomicUsize>);

impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

pub fn spawn(addr: SocketAddr, token: String, data_dir: PathBuf, tx: Sender<In>) -> io::Result<()> {
    let listener = crate::settings::bind(addr)?;
    let token = Arc::new(token);
    let data_dir = Arc::new(data_dir);
    let active = Arc::new(AtomicUsize::new(0));
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            if active.load(Ordering::Relaxed) >= MAX_WEB_CONNECTIONS { continue; }
            active.fetch_add(1, Ordering::Relaxed);
            let slot = ConnectionSlot(active.clone());
            let (tx, token, data_dir) = (tx.clone(), token.clone(), data_dir.clone());
            let _ = thread::Builder::new().name("web-client".into()).spawn(move || {
                let _slot = slot;
                handle(stream, &token, &data_dir, tx);
            });
        }
    });
    Ok(())
}

fn local_ip(stream: &TcpStream) -> Option<IpAddr> {
    let local = stream.local_addr().ok()?.ip();
    let peer = stream.peer_addr().ok()?.ip();
    (peer.is_loopback() || peer == local).then_some(local)
}

fn set_deadline(stream: &TcpStream, deadline: Instant) -> io::Result<()> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() { return Err(io::ErrorKind::TimedOut.into()); }
    stream.set_read_timeout(Some(remaining))
}

fn read_head(stream: &mut TcpStream, deadline: Instant) -> io::Result<(Vec<u8>, usize)> {
    let mut request = Vec::with_capacity(1024);
    let mut buffer = [0; 1024];
    while request.len() < MAX_HTTP_HEAD {
        set_deadline(stream, deadline)?;
        let limit = (MAX_HTTP_HEAD - request.len()).min(buffer.len());
        let n = stream.read(&mut buffer[..limit])?;
        if n == 0 { return Err(io::ErrorKind::UnexpectedEof.into()); }
        let start = request.len().saturating_sub(3);
        request.extend_from_slice(&buffer[..n]);
        if let Some(end) = request[start..].windows(4).position(|bytes| bytes == b"\r\n\r\n") {
            return Ok((request, start + end + 4));
        }
    }
    Err(io::ErrorKind::InvalidData.into())
}

// Hand the already-read bytes to tungstenite, including any frame bytes read
// together with the handshake. HTTP handlers reuse the same request buffer.
struct UpgradeStream {
    socket: TcpStream,
    prefix: Cursor<Vec<u8>>,
}

impl Read for UpgradeStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = self.prefix.read(buffer)?;
        if count > 0 { Ok(count) } else { self.socket.read(buffer) }
    }
}

impl Write for UpgradeStream {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> { self.socket.write(bytes) }
    fn flush(&mut self) -> io::Result<()> { self.socket.flush() }
}

fn handle(mut stream: TcpStream, token: &str, data_dir: &Path, tx: Sender<In>) {
    if stream.set_write_timeout(Some(HTTP_TIMEOUT)).is_err() { return; }
    let Ok((request, end)) = read_head(&mut stream, Instant::now() + HTTP_TIMEOUT) else { return };
    let Ok(head) = std::str::from_utf8(&request[..end]) else { return };
    let mut request_line = head.lines().next().unwrap_or("").split_whitespace();
    let (Some(method), Some(target)) = (request_line.next(), request_line.next()) else { return };
    let path = target.split('?').next().unwrap_or("");
    if method == "GET" && path == "/api/public/map" {
        return if PUBLIC_MAP.load(Ordering::Relaxed) {
            reply(&mut stream, "200 OK", "application/json", &crate::debug::public_map())
        } else {
            reply(&mut stream, "404 Not Found", "application/json", "{\"error\":\"this host does not share a public map (--public-map)\"}")
        };
    }
    let upgrade = method == "GET" && header(head, "upgrade").is_some_and(|value| value.eq_ignore_ascii_case("websocket"));
    if crate::server_mode() {
        if upgrade && path == "/meshcore/session" {
            let _ = stream.set_read_timeout(Some(HTTP_TIMEOUT));
            return crate::meshcore_access::session(UpgradeStream { socket: stream, prefix: Cursor::new(request) }, data_dir);
        }
        return server_http(stream, request, end, data_dir);
    }
    if upgrade {
        if path != "/ws" {
            return reply(&mut stream, "404 Not Found", "text/plain; charset=utf-8", "Not found.");
        }
        return websocket(UpgradeStream { socket: stream, prefix: Cursor::new(request) }, token, tx);
    }
    http(stream, request, end, token, data_dir);
}

fn http(mut stream: TcpStream, req: Vec<u8>, head_end: usize, token: &str, data_dir: &Path) {
    let line = String::from_utf8_lossy(&req[..head_end]).into_owned();
    let mut words = line.split_whitespace();
    let method = words.next().unwrap_or("GET").to_string();
    let target = words.next().unwrap_or("/");
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if path.starts_with("/api/") {
        return api(stream, &method, path, query, &line, req, token, data_dir);
    }
    let sw;
    let (status, ctype, body): (&str, &str, &[u8]) = match path {
        "/" | "/index.html" => ("200 OK", "text/html; charset=utf-8", INDEX_HTML.as_bytes()),
        "/flash" | "/flash.html" => ("200 OK", "text/html; charset=utf-8", FLASH_HTML.as_bytes()),
        "/manifest.webmanifest" => ("200 OK", "application/manifest+json", MANIFEST.as_bytes()),
        "/sw.js" => {
            sw = service_worker();
            ("200 OK", "text/javascript; charset=utf-8", sw.as_bytes())
        }
        "/icon.svg" => ("200 OK", "image/svg+xml", ICON_SVG.as_bytes()),
        "/icon-192.png" => ("200 OK", "image/png", ICON_192),
        "/icon-512.png" => ("200 OK", "image/png", ICON_512),
        _ => ("404 Not Found", "text/plain; charset=utf-8", "Not found".as_bytes()),
    };
    send(&mut stream, status, ctype, "", body);
}

// ---------- maarse-server: login and dashboard ----------

fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                match u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn send(stream: &mut TcpStream, status: &str, ctype: &str, extra: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Frame-Options: DENY\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: strict-origin-when-cross-origin\r\n{extra}Connection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
}

fn request_body<'a>(stream: &mut TcpStream, request: &'a mut Vec<u8>, head_end: usize, max: usize, timeout: Duration) -> Result<&'a [u8], &'static str> {
    let head = std::str::from_utf8(&request[..head_end]).map_err(|_| "Invalid headers")?;
    let mut lengths = head.lines().skip(1).filter_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim().eq_ignore_ascii_case("content-length").then_some(value.trim())
    });
    let len = lengths.next().ok_or("Missing Content-Length")?.parse::<usize>().map_err(|_| "Invalid Content-Length")?;
    if lengths.next().is_some() || header(head, "transfer-encoding").is_some() { return Err("Ambiguous request body"); }
    if len > max { return Err("Request body is too large"); }
    let end = head_end + len;
    let deadline = Instant::now() + timeout;
    let mut buffer = [0; 4096];
    while request.len() < end {
        set_deadline(stream, deadline).map_err(|_| "Incomplete request body")?;
        let limit = (end - request.len()).min(buffer.len());
        let count = stream.read(&mut buffer[..limit]).map_err(|_| "Incomplete request body")?;
        if count == 0 { return Err("Incomplete request body"); }
        request.extend_from_slice(&buffer[..count]);
    }
    Ok(&request[head_end..end])
}

fn server_http(mut stream: TcpStream, mut req: Vec<u8>, head_end: usize, data_dir: &Path) {
    let head = String::from_utf8_lossy(&req[..head_end]).into_owned();
    let mut words = head.split_whitespace();
    let method = words.next().unwrap_or("GET").to_string();
    let target = words.next().unwrap_or("/").to_string();
    let (path, query) = target.split_once('?').unwrap_or((target.as_str(), ""));
    let ip = stream.peer_addr().map(|a| a.ip()).unwrap_or(std::net::IpAddr::from([0, 0, 0, 0]));
    let html = "text/html; charset=utf-8";
    let jsn = "application/json; charset=utf-8";
    if path == "/login" && method == "POST" {
        let body = match request_body(&mut stream, &mut req, head_end, 4096, HTTP_TIMEOUT) {
            Ok(body) => body,
            Err(error) => return reply(&mut stream, "400 Bad Request", "text/plain; charset=utf-8", error),
        };
        let text = String::from_utf8_lossy(body);
        let pw = text.split('&').find_map(|kv| kv.strip_prefix("password=")).map(url_decode).unwrap_or_default();
        return match crate::admin::login(data_dir, ip, &pw) {
            Some(tok) => {
                crate::debug::push("admin", serde_json::json!({"op": "login", "ip": ip.to_string()}));
                send(&mut stream, "303 See Other", html, &format!("Location: /\r\nSet-Cookie: {}\r\n", crate::admin::cookie(&tok)), b"")
            }
            None => {
                crate::debug::push("admin", serde_json::json!({"op": "failed login", "ip": ip.to_string()}));
                let msg = if crate::admin::locked(ip) { "Too many wrong passwords: locked for 15 minutes." } else { "Wrong password." };
                let page = crate::admin::LOGIN_HTML.replace("<!--ERR-->", &format!("<div class=\"err\">{msg}</div>"));
                send(&mut stream, "401 Unauthorized", html, "", page.as_bytes())
            }
        };
    }
    if path == "/login" {
        return send(&mut stream, "200 OK", html, "", crate::admin::LOGIN_HTML.as_bytes());
    }
    let Some(session) = crate::admin::session(&head) else {
        if path.starts_with("/api/") {
            return send(&mut stream, "401 Unauthorized", jsn, "", b"{\"error\":\"login required\"}");
        }
        return send(&mut stream, "303 See Other", html, "Location: /login\r\n", b"");
    };
    match (method.as_str(), path) {
        ("GET", "/logout") => {
            crate::admin::logout(&session);
            send(&mut stream, "303 See Other", html, "Location: /login\r\nSet-Cookie: maarse_session=; Max-Age=0; Path=/\r\n", b"")
        }
        ("GET", "/") | ("GET", "/index.html") => send(&mut stream, "200 OK", html, "", SERVER_HTML.as_bytes()),
        ("GET", "/icon.svg") => send(&mut stream, "200 OK", "image/svg+xml", "", ICON_SVG.as_bytes()),
        ("GET", "/api/debug/snapshot") => send(&mut stream, "200 OK", jsn, "", crate::debug::snapshot().as_bytes()),
        ("GET", "/api/telemetry") => send(&mut stream, "200 OK", jsn, "", crate::debug::telemetry().as_bytes()),
        ("GET", "/api/debug/events") => {
            let since = query.split('&').find_map(|kv| kv.strip_prefix("since=")).and_then(|v| v.parse().ok()).unwrap_or(0);
            send(&mut stream, "200 OK", jsn, "", crate::debug::events_json(since).as_bytes())
        }
        ("GET", "/api/service/log") => {
            let since = query.split('&').find_map(|kv| kv.strip_prefix("since=")).and_then(|v| v.parse().ok()).unwrap_or(0);
            send(&mut stream, "200 OK", jsn, "", crate::service::log_json(since).as_bytes())
        }
        ("GET", "/api/service/status") => send(&mut stream, "200 OK", jsn, "", crate::service::status_json(data_dir).as_bytes()),
        // Replaced by the Connections page of the dashboard.
        ("GET", "/meshcore-access") => send(&mut stream, "303 See Other", html, "Location: /\r\n", b""),
        ("GET", "/api/meshcore") => {
            let grants = match crate::meshcore_access::grants(data_dir) {
                Ok(grants) => grants,
                Err(_) => return reply(&mut stream, "500 Internal Server Error", jsn, "{\"error\":\"Could not read MeshCore grants.\"}"),
            };
            let body = serde_json::json!({"gateway": crate::meshcore_gateway::status(), "grants": grants});
            reply(&mut stream, "200 OK", "application/json; charset=utf-8", &body.to_string())
        }
        ("POST", "/api/meshcore/grant") => {
            let host = header(&head, "host").unwrap_or("");
            if header(&head, "origin").is_some_and(|origin| origin != format!("http://{host}") && origin != format!("https://{host}")) {
                return reply(&mut stream, "403 Forbidden", "application/json; charset=utf-8", "{\"error\":\"Wrong origin.\"}");
            }
            let body = match request_body(&mut stream, &mut req, head_end, 512, HTTP_TIMEOUT) {
                Ok(body) => body,
                Err(error) => return reply(&mut stream, "400 Bad Request", "application/json; charset=utf-8", &serde_json::json!({"error": error}).to_string()),
            };
            let Ok(request) = serde_json::from_slice::<serde_json::Value>(body) else {
                return reply(&mut stream, "400 Bad Request", "application/json; charset=utf-8", "{\"error\":\"Send JSON with person and allowed.\"}");
            };
            let (Some(person), Some(allowed)) = (request["person"].as_str(), request["allowed"].as_bool()) else {
                return reply(&mut stream, "400 Bad Request", "application/json; charset=utf-8", "{\"error\":\"Send JSON with person and allowed.\"}");
            };
            match crate::meshcore_access::set_grant(data_dir, person, allowed) {
                Ok(()) => {
                    let grants = crate::meshcore_access::grants(data_dir).unwrap_or_default();
                    reply(&mut stream, "200 OK", "application/json; charset=utf-8", &serde_json::json!({"grants": grants}).to_string())
                }
                Err(err) if err.kind() == std::io::ErrorKind::InvalidInput => {
                    reply(&mut stream, "400 Bad Request", "application/json; charset=utf-8", &serde_json::json!({"error": err.to_string()}).to_string())
                }
                Err(_) => reply(&mut stream, "500 Internal Server Error", jsn, "{\"error\":\"Could not save MeshCore grants.\"}"),
            }
        }
        ("GET", "/api/settings") => send(&mut stream, "200 OK", jsn, "", crate::settings::status_json(data_dir).as_bytes()),
        // The page server's files (the dashboard's Pages tab).
        ("GET", "/api/pages/files") => send(&mut stream, "200 OK", jsn, "", crate::pagefiles::list(data_dir).to_string().as_bytes()),
        ("GET", "/api/pages/file") => {
            let rel = url_decode(query.split('&').find_map(|kv| kv.strip_prefix("path=")).unwrap_or(""));
            match crate::pagefiles::read(data_dir, &rel) {
                Ok(d) => send(&mut stream, "200 OK", "application/octet-stream", "", &d),
                Err(e) => send(&mut stream, "404 Not Found", "text/plain; charset=utf-8", "", e.as_bytes()),
            }
        }
        ("PUT", "/api/pages/file") | ("DELETE", "/api/pages/file") | ("POST", "/api/pages/mkdir") => {
            let host = header(&head, "host").unwrap_or("");
            if header(&head, "origin").is_some_and(|o| o != format!("http://{host}") && o != format!("https://{host}")) {
                return send(&mut stream, "403 Forbidden", "text/plain; charset=utf-8", "", b"wrong origin");
            }
            let rel = url_decode(query.split('&').find_map(|kv| kv.strip_prefix("path=")).unwrap_or(""));
            let result = match method.as_str() {
                "PUT" => {
                    let body = match request_body(&mut stream, &mut req, head_end, crate::pagefiles::MAX_FILE, Duration::from_secs(30)) {
                        Ok(body) => body,
                        Err(error) => return reply(&mut stream, "400 Bad Request", "text/plain; charset=utf-8", error),
                    };
                    crate::pagefiles::write(data_dir, &rel, body)
                }
                "DELETE" => crate::pagefiles::delete(data_dir, &rel),
                _ => crate::pagefiles::mkdir(data_dir, &rel),
            };
            match result {
                Ok(()) => {
                    crate::debug::push("admin", serde_json::json!({"op": format!("pages: {method} {rel}"), "ip": ip.to_string()}));
                    send(&mut stream, "200 OK", "text/plain; charset=utf-8", "", b"ok")
                }
                Err(e) => send(&mut stream, "400 Bad Request", "text/plain; charset=utf-8", "", e.as_bytes()),
            }
        }
        ("GET", "/api/guard") => {
            let devices: serde_json::Value = serde_json::from_str(&crate::links::devices_json()).unwrap_or_default();
            let body = serde_json::json!({"guard": crate::guard::json(), "links": devices["devices"]});
            send(&mut stream, "200 OK", jsn, "", body.to_string().as_bytes())
        }
        ("POST", "/api/guard") => {
            let host = header(&head, "host").unwrap_or("");
            if header(&head, "origin").is_some_and(|o| o != format!("http://{host}") && o != format!("https://{host}")) {
                return send(&mut stream, "403 Forbidden", "text/plain; charset=utf-8", "", b"wrong origin");
            }
            let body = match request_body(&mut stream, &mut req, head_end, 4096, HTTP_TIMEOUT) {
                Ok(body) => body,
                Err(error) => return reply(&mut stream, "400 Bad Request", "text/plain; charset=utf-8", error),
            };
            let v: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
            let target = v["target"].as_str().unwrap_or("").trim().to_string();
            let result: Result<String, String> = match v["op"].as_str().unwrap_or("") {
                "block" => crate::guard::block(&target).map(|_| format!("{target} blocked")),
                "unblock" => Ok(if crate::guard::unblock(&target) { format!("{target} unblocked") } else { format!("{target} was not blocked") }),
                op @ ("disconnect" | "disconnect_block") => match v["id"].as_u64().and_then(|i| crate::links::disconnect(i as u32).map(|ip| (i, ip))) {
                    Some((i, ip)) if op == "disconnect_block" => crate::guard::block(&ip.to_string()).map(|_| format!("link {i} closed and {ip} blocked")),
                    Some((i, ip)) => Ok(format!("link {i} to {ip} closed")),
                    None => Err("no such TCP link (or it cannot be closed)".into()),
                },
                _ => Err("unknown operation".into()),
            };
            match result {
                Ok(m) => {
                    crate::debug::push("admin", serde_json::json!({"op": m, "ip": ip.to_string()}));
                    send(&mut stream, "200 OK", "text/plain; charset=utf-8", "", m.as_bytes())
                }
                Err(e) => send(&mut stream, "400 Bad Request", "text/plain; charset=utf-8", "", e.as_bytes()),
            }
        }
        ("POST", "/api/settings") | ("POST", "/api/admin/password") => {
            // Same-origin only (the session cookie is SameSite=Strict too).
            let host = header(&head, "host").unwrap_or("");
            if header(&head, "origin").is_some_and(|o| o != format!("http://{host}") && o != format!("https://{host}")) {
                return send(&mut stream, "403 Forbidden", "text/plain; charset=utf-8", "", b"wrong origin");
            }
            let body = match request_body(&mut stream, &mut req, head_end, 64 * 1024, HTTP_TIMEOUT) {
                Ok(body) => body,
                Err(error) => return reply(&mut stream, "400 Bad Request", "text/plain; charset=utf-8", error),
            };
            let text = String::from_utf8_lossy(body);
            if path == "/api/admin/password" {
                let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
                let (old, new) = (v["old"].as_str().unwrap_or(""), v["new"].as_str().unwrap_or(""));
                if !crate::admin::check_password(data_dir, old) {
                    std::thread::sleep(Duration::from_secs(1));
                    return send(&mut stream, "403 Forbidden", "text/plain; charset=utf-8", "", b"The current password is wrong.");
                }
                if new.chars().count() < 10 {
                    return send(&mut stream, "400 Bad Request", "text/plain; charset=utf-8", "", b"The new password needs at least 10 characters.");
                }
                return match crate::admin::set_password(data_dir, new) {
                    Ok(()) => {
                        let _ = std::fs::remove_file(data_dir.join("admin-password.txt"));
                        crate::debug::push("admin", serde_json::json!({"op": "password changed", "ip": ip.to_string()}));
                        send(&mut stream, "200 OK", "text/plain; charset=utf-8", "", b"Password changed.")
                    }
                    Err(e) => send(&mut stream, "500 Internal Server Error", "text/plain; charset=utf-8", "", e.to_string().as_bytes()),
                };
            }
            match crate::settings::save(data_dir, &text) {
                Ok(()) => {
                    crate::debug::push("admin", serde_json::json!({"op": "settings saved, restarting", "ip": ip.to_string()}));
                    send(&mut stream, "200 OK", "text/plain; charset=utf-8", "", b"Saved. Restarting ...");
                    drop(stream);
                    thread::sleep(Duration::from_millis(400));
                    crate::settings::restart();
                }
                Err(e) => send(&mut stream, "400 Bad Request", "text/plain; charset=utf-8", "", e.as_bytes()),
            }
        }
        ("GET", "/api/meshcore/gateway") => {
            send(&mut stream, "200 OK", jsn, "", crate::meshcore_gateway::config_json(data_dir).to_string().as_bytes())
        }
        ("POST", "/api/meshcore/gateway/token") => {
            let host = header(&head, "host").unwrap_or("");
            if header(&head, "origin").is_some_and(|o| o != format!("http://{host}") && o != format!("https://{host}")) {
                return send(&mut stream, "403 Forbidden", jsn, "", b"{\"error\":\"Wrong origin.\"}");
            }
            match crate::meshcore_gateway::create_token(data_dir) {
                Ok(token) => send(&mut stream, "200 OK", jsn, "", serde_json::json!({"token": token}).to_string().as_bytes()),
                Err(error) => send(&mut stream, "500 Internal Server Error", jsn, "", serde_json::json!({"error": error}).to_string().as_bytes()),
            }
        }
        ("POST", "/api/meshcore/gateway") => {
            let host = header(&head, "host").unwrap_or("");
            if header(&head, "origin").is_some_and(|o| o != format!("http://{host}") && o != format!("https://{host}")) {
                return send(&mut stream, "403 Forbidden", jsn, "", b"{\"error\":\"Wrong origin.\"}");
            }
            let body = match request_body(&mut stream, &mut req, head_end, 64 * 1024, HTTP_TIMEOUT) {
                Ok(body) => body,
                Err(error) => return send(&mut stream, "400 Bad Request", jsn, "", serde_json::json!({"error": error}).to_string().as_bytes()),
            };
            let Ok(update) = serde_json::from_slice::<serde_json::Value>(body) else {
                return send(&mut stream, "400 Bad Request", jsn, "", b"{\"error\":\"Send the gateway settings as JSON.\"}");
            };
            match crate::meshcore_gateway::save_config(data_dir, &update) {
                Ok(()) => {
                    crate::debug::push("admin", serde_json::json!({"op": "gateway settings saved, restarting", "ip": ip.to_string()}));
                    send(&mut stream, "200 OK", "text/plain; charset=utf-8", "", b"Saved. Restarting ...");
                    drop(stream);
                    thread::sleep(Duration::from_millis(400));
                    crate::settings::restart();
                }
                Err(error) => send(&mut stream, "400 Bad Request", jsn, "", serde_json::json!({"error": error}).to_string().as_bytes()),
            }
        }
        _ => send(&mut stream, "404 Not Found", "text/plain; charset=utf-8", "", b"Not found"),
    }
}

// ---------- settings (/api/…) ----------

fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.split("\r\n").skip(1).find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim().eq_ignore_ascii_case(name).then(|| v.trim())
    })
}

fn reply(stream: &mut TcpStream, status: &str, ctype: &str, body: &str) {
    send(stream, status, ctype,
        "Access-Control-Allow-Origin: *\r\nAccess-Control-Allow-Headers: Authorization, Content-Type\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\n",
        body.as_bytes());
}

#[allow(clippy::too_many_arguments)]
fn api(mut stream: TcpStream, method: &str, path: &str, query: &str, head: &str, mut req: Vec<u8>, token: &str, data_dir: &Path) {
    let head_end = head.len();
    let provided = bearer(header(head, "authorization")).or_else(|| legacy_token(query));
    let ok = allowed(provided, header(head, "host"), header(head, "origin"), local_ip(&stream), token);
    if method == "OPTIONS" {
        return reply(&mut stream, "204 No Content", "text/plain", "");
    }
    if !ok {
        return reply(&mut stream, "403 Forbidden", "text/plain; charset=utf-8", "Access key missing or wrong");
    }
    let restart_soon = |mut stream: TcpStream| {
        reply(&mut stream, "200 OK", "text/plain; charset=utf-8", "ok");
        drop(stream);
        thread::sleep(Duration::from_millis(300));
        crate::settings::restart();
    };
    match (method, path) {
        ("GET", "/api/settings") => {
            let body = crate::settings::status_json(data_dir);
            reply(&mut stream, "200 OK", "application/json; charset=utf-8", &body)
        }
        ("POST", "/api/settings") => {
            let body = match request_body(&mut stream, &mut req, head_end, 64 * 1024, HTTP_TIMEOUT) {
                Ok(body) => body,
                Err(error) => return reply(&mut stream, "400 Bad Request", "text/plain; charset=utf-8", error),
            };
            let text = String::from_utf8_lossy(body);
            match crate::settings::save(data_dir, &text) {
                Ok(()) => restart_soon(stream),
                Err(e) => reply(&mut stream, "400 Bad Request", "text/plain; charset=utf-8", &e),
            }
        }
        ("POST", "/api/restart") => restart_soon(stream),
        ("GET", "/api/service/status") => {
            let body = crate::service::status_json(data_dir);
            reply(&mut stream, "200 OK", "application/json; charset=utf-8", &body)
        }
        ("GET", "/api/service/log") => {
            let since = query
                .split('&')
                .find_map(|kv| kv.strip_prefix("since="))
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            let body = crate::service::log_json(since);
            reply(&mut stream, "200 OK", "application/json; charset=utf-8", &body)
        }
        ("POST", "/api/service/restart") => restart_soon(stream),
        ("POST", "/api/service/stop") => {
            reply(&mut stream, "200 OK", "text/plain; charset=utf-8", "ok");
            drop(stream);
            thread::sleep(Duration::from_millis(300));
            crate::service::stop();
        }
        // Linked devices: the master's encrypted backup (node/person.rs), and
        // a backup to restore from (the code goes over the WebSocket).
        ("GET", "/api/account-backup") => match std::fs::read(data_dir.join(crate::node::BACKUP_FILE)) {
            Ok(b) => send(&mut stream, "200 OK", "application/octet-stream", "Content-Disposition: attachment; filename=\"maarse-account.maarse\"\r\n", &b),
            Err(_) => reply(&mut stream, "404 Not Found", "text/plain; charset=utf-8", "No backup has been made"),
        },
        ("POST", "/api/account-restore") => {
            let body = match request_body(&mut stream, &mut req, head_end, 4096, HTTP_TIMEOUT) {
                Ok(body) => body,
                Err(error) => return reply(&mut stream, "400 Bad Request", "text/plain; charset=utf-8", error),
            };
            match std::fs::write(data_dir.join(crate::node::RESTORE_FILE), body) {
                Ok(()) => reply(&mut stream, "200 OK", "text/plain; charset=utf-8", "ok"),
                Err(e) => reply(&mut stream, "500 Internal Server Error", "text/plain; charset=utf-8", &e.to_string()),
            }
        }
        ("GET", "/api/devices") => {
            let body = crate::links::devices_json();
            reply(&mut stream, "200 OK", "application/json; charset=utf-8", &body)
        }
        _ => reply(&mut stream, "404 Not Found", "text/plain; charset=utf-8", "Not found"),
    }
}

// ---------- PWA: manifest, service worker and icons ----------

const MANIFEST: &str = include_str!("../../maarse-client/manifest.webmanifest");
const SW_JS: &str = include_str!("../../maarse-client/sw.js");
const ICON_SVG: &str = include_str!("../../maarse-client/icon.svg");
const ICON_192: &[u8] = include_bytes!("../../maarse-client/icon-192.png");
const ICON_512: &[u8] = include_bytes!("../../maarse-client/icon-512.png");

/// The service worker gets a cache version derived from the contents of the built-in files.
/// That way the browser discovers a new client by itself when maarse-host is updated.
fn service_worker() -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325; // FNV-1a
    for part in [INDEX_HTML.as_bytes(), MANIFEST.as_bytes(), SW_JS.as_bytes(), ICON_SVG.as_bytes(), ICON_192, ICON_512] {
        for &b in part {
            h = (h ^ b as u64).wrapping_mul(0x0100_0000_01b3);
        }
    }
    SW_JS.replacen("\"maarse-v1\"", &format!("\"maarse-{h:016x}\""), 1)
}

/// Who may control the node over WebSocket?
///
/// * From this computer (loopback or its own IP) and from a page with the same host: yes, as
///   before. The Origin check prevents a web site you visit from sending
///   messages in your name via localhost.
/// * Remote clients must supply the access key. Native clients use a bearer
///   header and browsers offer a credential with the WebSocket subprotocol.
///
/// Backbone mode and `--require-key` apply the same policy to loopback.
pub static REQUIRE_KEY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Temporary migration switch for clients that send the key in the request URL.
pub static ALLOW_LEGACY_WEB_TOKEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// `--public-map`: serve `/api/public/map` to everyone.
pub static PUBLIC_MAP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn bearer(header: Option<&str>) -> Option<&str> {
    header?.strip_prefix("Bearer ")
}

fn legacy_token(query: &str) -> Option<&str> {
    if !ALLOW_LEGACY_WEB_TOKEN.load(std::sync::atomic::Ordering::Relaxed) {
        return None;
    }
    query.split('&').find_map(|parameter| parameter.strip_prefix("token="))
}

fn allowed(given: Option<&str>, host: Option<&str>, origin: Option<&str>, local: Option<IpAddr>, token: &str) -> bool {
    if !token.is_empty() && given.is_some_and(|given| ct_eq(given.as_bytes(), token.as_bytes())) {
        return true;
    }
    let Some(local) = local else { return false };
    let Some(host) = host else { return false };
    local_authority(host, local) && origin.is_none_or(|origin| origin == format!("http://{host}"))
        && !REQUIRE_KEY.load(Ordering::Relaxed)
}

/// Constant-time comparison (the key must not be guessable character by character).
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn local_authority(host: &str, local: IpAddr) -> bool {
    let Ok(authority) = host.parse::<tungstenite::http::uri::Authority>() else { return false };
    let name = authority.host();
    (local.is_loopback() && name.eq_ignore_ascii_case("localhost"))
        || name.trim_matches(['[', ']']).parse::<IpAddr>().is_ok_and(|ip| ip == local)
}

type Ws = tungstenite::WebSocket<UpgradeStream>;

/// WebSocket handshake with access check (see [`allowed`]).
fn accept(stream: UpgradeStream, token: &str) -> Option<Ws> {
    let peer = stream.socket.peer_addr().map(|a| a.to_string()).unwrap_or_default();
    // Local: loopback, or the same machine via its own network address
    // (e.g. a browser on the PC opening http://<the-pc's-ip>:8080/).
    let local = local_ip(&stream.socket);
    #[allow(clippy::result_large_err)] // the signature is given by tungstenite
    let check = |req: &Request, mut resp: Response| -> Result<Response, ErrorResponse> {
        let origin = req.headers().get("origin").and_then(|v| v.to_str().ok()).unwrap_or("-");
        let h = req.headers();
        let get = |n: &str| h.get(n).and_then(|v| v.to_str().ok());
        let protocols = get("sec-websocket-protocol").unwrap_or("");
        let offered = |name| protocols.split(',').any(|p| p.trim() == name);
        let app_protocol = offered("maarse");
        let provided = bearer(get("authorization"))
            .or_else(|| {
                if app_protocol {
                    protocols.split(',').find_map(|p| p.trim().strip_prefix("maarse-key."))
                } else {
                    None
                }
            })
            .or_else(|| legacy_token(req.uri().query().unwrap_or("")));
        if allowed(provided, get("host"), get("origin"), local, token) {
            if app_protocol {
                resp.headers_mut().insert("sec-websocket-protocol", tungstenite::http::HeaderValue::from_static("maarse"));
            }
            if local.is_none() {
                println!("[web] client connected from {peer} (origin {origin}, {})", req.uri().path());
            }
            Ok(resp)
        } else {
            let has = provided.is_some();
            println!("[web] rejected {peer} (origin {origin}, {})", if has { "wrong key" } else { "no key" });
            let mut r = ErrorResponse::new(Some("Access key missing or wrong".into()));
            *r.status_mut() = tungstenite::http::StatusCode::FORBIDDEN;
            Err(r)
        }
    };
    let config = tungstenite::protocol::WebSocketConfig {
        max_message_size: Some(64 * 1024), max_frame_size: Some(64 * 1024),
        ..Default::default()
    };
    let ws = tungstenite::accept_hdr_with_config(stream, check, Some(config)).ok()?;
    let _ = ws.get_ref().socket.set_read_timeout(Some(Duration::from_millis(30)));
    Some(ws)
}

fn websocket(stream: UpgradeStream, token: &str, tx: Sender<In>) {
    let Some(mut ws) = accept(stream, token) else { return };
    let id = next_id();
    let (ctx, crx) = channel::<Vec<u8>>();
    if tx.send(In::ClientUp { id, tx: ctx }).is_err() {
        return;
    }
    let mut dec = FrameDecoder::new();
    'outer: loop {
        match ws.read() {
            Ok(Message::Binary(b)) => {
                dec.push(&b);
                while let Some(f) = dec.next_frame() {
                    if tx
                        .send(In::ClientFrame {
                            id,
                            frame: f.to_vec(),
                        })
                        .is_err()
                    {
                        break 'outer;
                    }
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(WsError::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_) => break,
        }
        while let Ok(frame) = crx.try_recv() {
            if ws.send(Message::Binary(frame)).is_err() {
                break 'outer;
            }
        }
    }
    let _ = tx.send(In::ClientDown { id });
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// The gateway routes are used by the server dashboard, so they must be
    /// served by `server_http` behind the admin login. They were once placed in
    /// the PC host's `api` only, where the server never routes them.
    #[test]
    fn server_dashboard_serves_gateway_routes() {
        let dir = std::env::temp_dir().join(format!("maarse-web-gateway-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        crate::admin::set_password(&dir, "a long test password").unwrap();
        let session = crate::admin::login(&dir, IpAddr::from([127, 0, 0, 1]), "a long test password").unwrap();
        let cookie = format!("Cookie: maarse_session={session}\r\n");

        let config = dashboard_request(&dir, &format!("GET /api/meshcore/gateway HTTP/1.1\r\nHost: localhost\r\n{cookie}\r\n"));
        assert!(config.starts_with("HTTP/1.1 200"), "{config}");
        assert!(config.contains("\"listen\""), "{config}");

        let token = dashboard_request(&dir, &format!(
            "POST /api/meshcore/gateway/token HTTP/1.1\r\nHost: localhost\r\nOrigin: http://localhost\r\n{cookie}Content-Length: 0\r\n\r\n"
        ));
        assert!(token.starts_with("HTTP/1.1 200"), "{token}");
        assert!(token.contains("\"token\""), "{token}");

        let other_origin = dashboard_request(&dir, &format!(
            "POST /api/meshcore/gateway/token HTTP/1.1\r\nHost: localhost\r\nOrigin: http://evil.example\r\n{cookie}Content-Length: 0\r\n\r\n"
        ));
        assert!(other_origin.starts_with("HTTP/1.1 403"), "{other_origin}");

        let no_login = dashboard_request(&dir, "GET /api/meshcore/gateway HTTP/1.1\r\nHost: localhost\r\n\r\n");
        assert!(no_login.starts_with("HTTP/1.1 401"), "{no_login}");
        std::fs::remove_dir_all(&dir).ok();
    }

    fn dashboard_request(dir: &Path, request: &str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let request = request.to_string();
        let client = thread::spawn(move || {
            let mut stream = TcpStream::connect(address).unwrap();
            stream.write_all(request.as_bytes()).unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            response
        });
        let (mut stream, _) = listener.accept().unwrap();
        let (request, head_end) = read_head(&mut stream, Instant::now() + HTTP_TIMEOUT).unwrap();
        server_http(stream, request, head_end, dir);
        client.join().unwrap()
    }

    #[test]
    fn tokenless_authorities_identify_the_local_listener() {
        let v4 = "127.0.0.1".parse().unwrap();
        let v6 = "::1".parse().unwrap();
        let lan = "192.0.2.17".parse().unwrap();
        assert!(local_authority("localhost:8080", v4));
        assert!(local_authority("[::1]:8080", v6));
        assert!(local_authority("192.0.2.17:8080", lan));
        assert!(!local_authority("host.example:8080", v4));
        assert!(!local_authority("localhost:8080", lan));
        assert!(!local_authority("192.0.2.18:8080", lan));
    }

    #[test]
    fn request_parsing_preserves_body_and_handles_incomplete_uploads() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let client = thread::spawn(move || {
            let mut stream = TcpStream::connect(address).unwrap();
            stream.write_all(b"PUT /api/pages/file HTTP/1.1\r\nContent-Length: 5\r\n").unwrap();
            stream.write_all(b"\r\nabc").unwrap();
        });
        let (mut stream, _) = listener.accept().unwrap();
        let (mut request, head_end) = read_head(&mut stream, Instant::now() + HTTP_TIMEOUT).unwrap();
        assert_eq!(request_body(&mut stream, &mut request, head_end, 64, HTTP_TIMEOUT), Err("Incomplete request body"));
        client.join().unwrap();
    }
}
