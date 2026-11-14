//! Webklient over HTTP og companion-protokollen over WebSocket (samme port).

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

/// Webklienten er bygget ind i programmet, så der kun er én fil at køre.
const INDEX_HTML: &str = include_str!("../../maarse-client/index.html");

/// Adgangsnøglen til fjernadgang: læses fra `<data>/adgangsnoegle.txt`
/// eller laves (32 hextegn) første gang.
pub fn access_token(data_dir: &std::path::Path) -> String {
    let p = data_dir.join("adgangsnoegle.txt");
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

/// Kommer forbindelsen fra denne computer (loopback eller dens egen IP)?
fn is_local(stream: &TcpStream) -> bool {
    match (stream.peer_addr(), stream.local_addr()) {
        (Ok(p), Ok(l)) => p.ip().is_loopback() || p.ip() == l.ip(),
        _ => false,
    }
}

fn handle(stream: TcpStream, token: &str, data_dir: &Path, tx: Sender<In>) {
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
        "/manifest.webmanifest" => ("200 OK", "application/manifest+json", MANIFEST.as_bytes()),
        "/sw.js" => {
            sw = service_worker();
            ("200 OK", "text/javascript; charset=utf-8", sw.as_bytes())
        }
        "/icon.svg" => ("200 OK", "image/svg+xml", ICON_SVG.as_bytes()),
        "/icon-192.png" => ("200 OK", "image/png", ICON_192),
        "/icon-512.png" => ("200 OK", "image/png", ICON_512),
        _ => ("404 Not Found", "text/plain; charset=utf-8", "Ikke fundet".as_bytes()),
    };
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.write_all(body);
}

// ---------- indstillinger (/api/…) ----------

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
        return reply(&mut stream, "403 Forbidden", "text/plain; charset=utf-8", "Adgangsnøgle mangler eller er forkert");
    }
    let restart_soon = |mut stream: TcpStream| {
        reply(&mut stream, "200 OK", "text/plain; charset=utf-8", "ok");
        drop(stream);
        thread::sleep(Duration::from_millis(300));
        crate::settings::restart();
    };
    match (method, path) {
        ("GET", "/api/indstillinger") => {
            let body = crate::settings::status_json(data_dir);
            reply(&mut stream, "200 OK", "application/json; charset=utf-8", &body)
        }
        ("POST", "/api/indstillinger") => {
            let len: usize = header(head, "content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
            if len > 64 * 1024 {
                return reply(&mut stream, "413 Payload Too Large", "text/plain", "for stor");
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
        ("POST", "/api/genstart") => restart_soon(stream),
        ("GET", "/api/enheder") => {
            let body = crate::links::devices_json();
            reply(&mut stream, "200 OK", "application/json; charset=utf-8", &body)
        }
        _ => reply(&mut stream, "404 Not Found", "text/plain; charset=utf-8", "Ikke fundet"),
    }
}

// ---------- PWA: manifest, service worker og ikoner ----------

const MANIFEST: &str = include_str!("../../maarse-client/manifest.webmanifest");
const SW_JS: &str = include_str!("../../maarse-client/sw.js");
const ICON_SVG: &str = include_str!("../../maarse-client/icon.svg");
const ICON_192: &[u8] = include_bytes!("../../maarse-client/icon-192.png");
const ICON_512: &[u8] = include_bytes!("../../maarse-client/icon-512.png");

/// Service workeren får en cache-version ud fra indholdet af de indbyggede filer.
/// Så opdager browseren en ny klient af sig selv, når maarse-host er opdateret.
fn service_worker() -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325; // FNV-1a
    for part in [INDEX_HTML.as_bytes(), MANIFEST.as_bytes(), SW_JS.as_bytes(), ICON_SVG.as_bytes(), ICON_192, ICON_512] {
        for &b in part {
            h = (h ^ b as u64).wrapping_mul(0x0100_0000_01b3);
        }
    }
    SW_JS.replacen("\"maarse-v1\"", &format!("\"maarse-{h:016x}\""), 1)
}

/// Hvem må styre noden over WebSocket?
///
/// * Fra denne computer (loopback eller dens egen IP) og fra en side med samme vært: ja, som
///   før. Origin-tjekket forhindrer, at en hjemmeside, du besøger, sender
///   beskeder i dit navn via localhost.
/// * Alle andre (fx telefonen over VPN, eller appen, der har sin egen
///   origin) skal have adgangsnøglen med: `ws://vært/ws?token=<nøgle>`.
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
    loopback && origin_ok
}

/// Sammenligning i konstant tid (nøglen må ikke kunne gættes tegn for tegn).
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

type Ws = tungstenite::WebSocket<TcpStream>;

/// WebSocket-håndtryk med adgangstjek (se [`allowed`]).
fn accept(stream: TcpStream, token: &str) -> Option<Ws> {
    let peer = stream.peer_addr().map(|a| a.to_string()).unwrap_or_default();
    // Lokal: loopback, eller samme maskine via dens egen netværksadresse
    // (fx en browser på PC'en, der åbner http://<pc-ens-ip>:8080/).
    let loopback = is_local(&stream);
    #[allow(clippy::result_large_err)] // signaturen er givet af tungstenite
    let check = |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
        let origin = req.headers().get("origin").and_then(|v| v.to_str().ok()).unwrap_or("-");
        let h = req.headers();
        let get = |n: &str| h.get(n).and_then(|v| v.to_str().ok());
        if allowed(req.uri().query().unwrap_or(""), get("host"), get("origin"), loopback, token) {
            if !loopback {
                println!("[web] klient forbundet fra {peer} (origin {origin}, {})", req.uri().path());
            }
            Ok(resp)
        } else {
            let has = req.uri().query().is_some_and(|q| q.contains("token="));
            println!("[web] afvist {peer} (origin {origin}, {})", if has { "forkert nøgle" } else { "ingen nøgle" });
            let mut r = ErrorResponse::new(Some("Adgangsnøgle mangler eller er forkert".into()));
            *r.status_mut() = tungstenite::http::StatusCode::FORBIDDEN;
            Err(r)
        }
    };
    let ws = tungstenite::accept_hdr(stream, check).ok()?;
    let _ = ws.get_ref().set_read_timeout(Some(Duration::from_millis(30)));
    Some(ws)
}

/// `/mc`: MeshCore-styring med JSON-tekstbeskeder (se meshcore.rs).
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
                        serde_json::json!({"ev":"err","op":op,"msg":"MeshCore er ikke slået til"}).to_string()
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
