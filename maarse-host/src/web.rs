//! Web client over HTTP and the companion protocol over WebSocket (same port).

use crate::{In, next_id};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Sender, channel};
use std::thread;
use std::time::Duration;
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

pub fn spawn(addr: SocketAddr, token: String, data_dir: PathBuf, tx: Sender<In>) -> std::io::Result<()> {
    let listener = crate::settings::bind(addr)?;
    let token = std::sync::Arc::new(token);
    let data_dir = std::sync::Arc::new(data_dir);
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (tx, token, dd) = (tx.clone(), token.clone(), data_dir.clone());
            thread::spawn(move || handle(stream, &token, &dd, tx));
        }
    });
    Ok(())
}

/// Does the connection come from this computer (loopback or its own IP)?
fn is_local(stream: &TcpStream) -> bool {
    match (stream.peer_addr(), stream.local_addr()) {
        (Ok(p), Ok(l)) => p.ip().is_loopback() || p.ip() == l.ip(),
        _ => false,
    }
}

fn handle(mut stream: TcpStream, token: &str, data_dir: &Path, tx: Sender<In>) {
    // The public map (`--public-map`): open to everyone, for map pages on
    // other sites (the Denmark simulator's real-world mode), no login.
    {
        let mut peek = [0u8; 64];
        let n = stream.peek(&mut peek).unwrap_or(0);
        if peek[..n].starts_with(b"GET /api/public/map") {
            let mut sink = [0u8; 4096];
            let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
            let _ = stream.read(&mut sink);
            return if PUBLIC_MAP.load(std::sync::atomic::Ordering::Relaxed) {
                reply(&mut stream, "200 OK", "application/json", &crate::debug::public_map())
            } else {
                reply(&mut stream, "404 Not Found", "application/json", "{\"error\":\"this host does not share a public map (--public-map)\"}")
            };
        }
    }
    if crate::server_mode() {
        // maarse-server: only the admin dashboard, behind a login; no chat
        // client and no WebSocket.
        return server_http(stream, data_dir);
    }
    let mut peek = [0u8; 2048];
    let n = stream.peek(&mut peek).unwrap_or(0);
    let head = String::from_utf8_lossy(&peek[..n]).to_ascii_lowercase();
    if head.contains("upgrade: websocket") {
        let path = head.split_whitespace().nth(1).unwrap_or("");
        if path == "/mc" || path.starts_with("/mc?") {
            mc_socket(stream, token);
        } else {
            websocket(stream, token, tx);
        }
    } else {
        http(stream, token, data_dir);
    }
}

fn http(mut stream: TcpStream, token: &str, data_dir: &Path) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut req = Vec::new();
    let mut buf = [0u8; 1024];
    while !req.windows(4).any(|w| w == b"\r\n\r\n") && req.len() < 16 * 1024 {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => req.extend_from_slice(&buf[..n]),
        }
    }
    let line = String::from_utf8_lossy(&req).into_owned();
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
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.write_all(body);
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

fn server_http(mut stream: TcpStream, data_dir: &Path) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut req = Vec::new();
    let mut buf = [0u8; 2048];
    while !req.windows(4).any(|w| w == b"\r\n\r\n") && req.len() < 16 * 1024 {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => req.extend_from_slice(&buf[..n]),
        }
    }
    let head_end = req.windows(4).position(|w| w == b"\r\n\r\n").map_or(req.len(), |p| p + 4);
    let head = String::from_utf8_lossy(&req[..head_end]).into_owned();
    let mut words = head.split_whitespace();
    let method = words.next().unwrap_or("GET").to_string();
    let target = words.next().unwrap_or("/").to_string();
    let (path, query) = target.split_once('?').unwrap_or((target.as_str(), ""));
    let ip = stream.peer_addr().map(|a| a.ip()).unwrap_or(std::net::IpAddr::from([0, 0, 0, 0]));
    let html = "text/html; charset=utf-8";
    let jsn = "application/json; charset=utf-8";
    if path == "/login" && method == "POST" {
        let len: usize = header(&head, "content-length").and_then(|v| v.parse().ok()).unwrap_or(0).min(4096);
        let mut body = req[head_end.min(req.len())..].to_vec();
        while body.len() < len {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => body.extend_from_slice(&buf[..n]),
            }
        }
        let text = String::from_utf8_lossy(&body[..len.min(body.len())]).into_owned();
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
                    let len: usize = header(&head, "content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
                    if len > crate::pagefiles::MAX_FILE {
                        return send(&mut stream, "413 Payload Too Large", "text/plain; charset=utf-8", "", b"larger than 2 MB");
                    }
                    let mut body = req[head_end.min(req.len())..].to_vec();
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
                    while body.len() < len {
                        match stream.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => body.extend_from_slice(&buf[..n]),
                        }
                    }
                    if body.len() < len {
                        Err("the upload was cut short".to_string())
                    } else {
                        crate::pagefiles::write(data_dir, &rel, &body[..len])
                    }
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
            let len: usize = header(&head, "content-length").and_then(|v| v.parse().ok()).unwrap_or(0).min(4096);
            let mut body = req[head_end.min(req.len())..].to_vec();
            while body.len() < len {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => body.extend_from_slice(&buf[..n]),
                }
            }
            let v: serde_json::Value = serde_json::from_slice(&body[..len.min(body.len())]).unwrap_or_default();
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
            let len: usize = header(&head, "content-length").and_then(|v| v.parse().ok()).unwrap_or(0).min(64 * 1024);
            let mut body = req[head_end.min(req.len())..].to_vec();
            while body.len() < len {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => body.extend_from_slice(&buf[..n]),
                }
            }
            let text = String::from_utf8_lossy(&body[..len.min(body.len())]).into_owned();
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
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.flush();
}

#[allow(clippy::too_many_arguments)]
fn api(mut stream: TcpStream, method: &str, path: &str, query: &str, head: &str, mut req: Vec<u8>, token: &str, data_dir: &Path) {
    let head_end = head.find("\r\n\r\n").unwrap_or(head.len());
    let head = &head[..head_end];
    let ok = allowed(query, header(head, "host"), header(head, "origin"), is_local(&stream), token);
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
            let len: usize = header(head, "content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
            if len > 64 * 1024 {
                return reply(&mut stream, "413 Payload Too Large", "text/plain", "too large");
            }
            let mut body = req.split_off((head_end + 4).min(req.len()));
            let mut buf = [0u8; 4096];
            while body.len() < len {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => body.extend_from_slice(&buf[..n]),
                }
            }
            let text = String::from_utf8_lossy(&body[..len.min(body.len())]).into_owned();
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
            let len: usize = header(head, "content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
            if len > 4096 {
                return reply(&mut stream, "413 Payload Too Large", "text/plain", "too large");
            }
            let mut body = req.split_off((head_end + 4).min(req.len()));
            let mut buf = [0u8; 4096];
            while body.len() < len {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => body.extend_from_slice(&buf[..n]),
                }
            }
            match std::fs::write(data_dir.join(crate::node::RESTORE_FILE), &body[..len.min(body.len())]) {
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
/// * Everyone else (e.g. the phone over VPN, or the app, which has its own
///   origin) must bring the access key: `ws://host/ws?token=<key>`.
/// `--require-key`: no free pass for 127.0.0.1. On a phone every other app
/// can reach the loopback address, so there the key is always needed.
pub static REQUIRE_KEY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// `--public-map`: serve `/api/public/map` to everyone.
pub static PUBLIC_MAP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn allowed(query: &str, host: Option<&str>, origin: Option<&str>, loopback: bool, token: &str) -> bool {
    let given = query
        .split('&')
        .find_map(|kv| kv.strip_prefix("token="))
        .unwrap_or("");
    if !token.is_empty() && ct_eq(given.as_bytes(), token.as_bytes()) {
        return true;
    }
    let host = host.unwrap_or("");
    let origin_ok = match origin {
        None => true,
        Some(o) => o == format!("http://{host}"),
    };
    loopback && origin_ok && !REQUIRE_KEY.load(std::sync::atomic::Ordering::Relaxed)
}

/// Constant-time comparison (the key must not be guessable character by character).
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

type Ws = tungstenite::WebSocket<TcpStream>;

/// WebSocket handshake with access check (see [`allowed`]).
fn accept(stream: TcpStream, token: &str) -> Option<Ws> {
    let peer = stream.peer_addr().map(|a| a.to_string()).unwrap_or_default();
    // Local: loopback, or the same machine via its own network address
    // (e.g. a browser on the PC opening http://<the-pc's-ip>:8080/).
    let loopback = is_local(&stream);
    #[allow(clippy::result_large_err)] // the signature is given by tungstenite
    let check = |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
        let origin = req.headers().get("origin").and_then(|v| v.to_str().ok()).unwrap_or("-");
        let h = req.headers();
        let get = |n: &str| h.get(n).and_then(|v| v.to_str().ok());
        if allowed(req.uri().query().unwrap_or(""), get("host"), get("origin"), loopback, token) {
            if !loopback {
                println!("[web] client connected from {peer} (origin {origin}, {})", req.uri().path());
            }
            Ok(resp)
        } else {
            let has = req.uri().query().is_some_and(|q| q.contains("token="));
            println!("[web] rejected {peer} (origin {origin}, {})", if has { "wrong key" } else { "no key" });
            let mut r = ErrorResponse::new(Some("Access key missing or wrong".into()));
            *r.status_mut() = tungstenite::http::StatusCode::FORBIDDEN;
            Err(r)
        }
    };
    let ws = tungstenite::accept_hdr(stream, check).ok()?;
    let _ = ws.get_ref().set_read_timeout(Some(Duration::from_millis(30)));
    Some(ws)
}

/// `/mc`: MeshCore control with JSON text messages (see meshcore.rs).
fn mc_socket(stream: TcpStream, token: &str) {
    let Some(mut ws) = accept(stream, token) else { return };
    let rx = crate::meshcore::subscribe();
    let enabled = crate::meshcore::command(r#"{"op":"state"}"#.into());
    if !enabled && ws.send(Message::Text(crate::meshcore::disabled_state())).is_err() {
        return;
    }
    loop {
        match ws.read() {
            Ok(Message::Text(t)) => {
                if !crate::meshcore::command(t.clone()) {
                    let op = serde_json::from_str::<serde_json::Value>(&t)
                        .ok()
                        .and_then(|v| v["op"].as_str().map(str::to_string))
                        .unwrap_or_default();
                    let reply = if op == "state" {
                        crate::meshcore::disabled_state()
                    } else {
                        serde_json::json!({"ev":"err","op":op,"msg":"MeshCore is not enabled"}).to_string()
                    };
                    if ws.send(Message::Text(reply)).is_err() {
                        break;
                    }
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(WsError::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(_) => break,
        }
        while let Ok(s) = rx.try_recv() {
            if ws.send(Message::Text(s)).is_err() {
                return;
            }
        }
    }
}

fn websocket(stream: TcpStream, token: &str, tx: Sender<In>) {
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
