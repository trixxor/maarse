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

pub fn spawn(addr: SocketAddr, tx: Sender<In>) -> std::io::Result<()> {
    let listener = TcpListener::bind(addr)?;
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let tx = tx.clone();
            thread::spawn(move || handle(stream, tx));
        }
    });
    Ok(())
}

fn handle(stream: TcpStream, tx: Sender<In>) {
    let mut peek = [0u8; 2048];
    let n = stream.peek(&mut peek).unwrap_or(0);
    let head = String::from_utf8_lossy(&peek[..n]).to_ascii_lowercase();
    if head.contains("upgrade: websocket") {
        websocket(stream, tx);
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
    let (status, ctype, body) = match path {
        "/" | "/index.html" => ("200 OK", "text/html; charset=utf-8", INDEX_HTML),
        _ => ("404 Not Found", "text/plain; charset=utf-8", "Ikke fundet"),
    };
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.write_all(body.as_bytes());
}

/// Kun sider fra samme vært må styre noden. Ellers kunne enhver hjemmeside,
/// du besøger, sende beskeder i dit navn via localhost.
#[allow(clippy::result_large_err)] // signaturen er givet af tungstenite
fn same_origin(req: &Request, resp: Response) -> Result<Response, ErrorResponse> {
    let h = req.headers();
    let host = h.get("host").and_then(|v| v.to_str().ok()).unwrap_or("");
    match h.get("origin").and_then(|v| v.to_str().ok()) {
        None => Ok(resp),
        Some(o) if o == format!("http://{host}") => Ok(resp),
        Some(_) => {
            let mut r = ErrorResponse::new(Some("Forkert origin".into()));
            *r.status_mut() = tungstenite::http::StatusCode::FORBIDDEN;
            Err(r)
        }
    }
}

fn websocket(stream: TcpStream, tx: Sender<In>) {
    let Ok(mut ws) = tungstenite::accept_hdr(stream, same_origin) else {
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
