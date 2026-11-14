//! Webklient over HTTP og companion-protokollen over WebSocket (samme port).

use crate::{In, next_id};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
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

pub fn spawn(addr: SocketAddr, token: String, tx: Sender<In>) -> std::io::Result<()> {
    let listener = TcpListener::bind(addr)?;
    let token = std::sync::Arc::new(token);
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (tx, token) = (tx.clone(), token.clone());
            thread::spawn(move || handle(stream, &token, tx));
        }
    });
    Ok(())
}

fn handle(stream: TcpStream, token: &str, tx: Sender<In>) {
    let mut peek = [0u8; 2048];
    let n = stream.peek(&mut peek).unwrap_or(0);
    let head = String::from_utf8_lossy(&peek[..n]).to_ascii_lowercase();
    if head.contains("upgrade: websocket") {
        websocket(stream, token, tx);
    } else {
        http(stream);
    }
}

fn http(mut stream: TcpStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut req = Vec::new();
    let mut buf = [0u8; 1024];
    while !req.windows(4).any(|w| w == b"\r\n\r\n") && req.len() < 16 * 1024 {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => req.extend_from_slice(&buf[..n]),
        }
    }
    let line = String::from_utf8_lossy(&req);
    let path = line.split_whitespace().nth(1).unwrap_or("/");
    let path = path.split('?').next().unwrap_or("/");
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
/// * Fra denne computer (loopback) og fra en side med samme vært: ja, som
///   før. Origin-tjekket forhindrer, at en hjemmeside, du besøger, sender
///   beskeder i dit navn via localhost.
/// * Alle andre (fx telefonen over VPN, eller appen, der har sin egen
///   origin) skal have adgangsnøglen med: `ws://vært/ws?token=<nøgle>`.
fn allowed(req: &Request, loopback: bool, token: &str) -> bool {
    let given = req
        .uri()
        .query()
        .unwrap_or("")
        .split('&')
        .find_map(|kv| kv.strip_prefix("token="))
        .unwrap_or("");
    if !token.is_empty() && ct_eq(given.as_bytes(), token.as_bytes()) {
        return true;
    }
    let h = req.headers();
    let host = h.get("host").and_then(|v| v.to_str().ok()).unwrap_or("");
    let origin_ok = match h.get("origin").and_then(|v| v.to_str().ok()) {
        None => true,
        Some(o) => o == format!("http://{host}"),
    };
    loopback && origin_ok
}

/// Sammenligning i konstant tid (nøglen må ikke kunne gættes tegn for tegn).
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn websocket(stream: TcpStream, token: &str, tx: Sender<In>) {
    let loopback = stream.peer_addr().map(|a| a.ip().is_loopback()).unwrap_or(false);
    #[allow(clippy::result_large_err)] // signaturen er givet af tungstenite
    let check = |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
        if allowed(req, loopback, token) {
            Ok(resp)
        } else {
            let mut r = ErrorResponse::new(Some("Adgangsnøgle mangler eller er forkert".into()));
            *r.status_mut() = tungstenite::http::StatusCode::FORBIDDEN;
            Err(r)
        }
    };
    let Ok(mut ws) = tungstenite::accept_hdr(stream, check) else {
        return;
    };
    let _ = ws
        .get_ref()
        .set_read_timeout(Some(Duration::from_millis(30)));
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
