//! maarse-lab: test lab in the browser.
//!
//! Starts a local web server (default http://127.0.0.1:8090) with a page
//! that shows all test suites, fuzzing, simulator scenarios and benchmarks.
//! Runs are subprocesses; their output is streamed live over WebSocket.
//! The simulator can also be shown live on a map (via `maarse-sim --trace-json -`).
#![forbid(unsafe_code)]

mod hw;
mod jobs;

use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, OnceLock, atomic::{AtomicUsize, Ordering}};
use std::thread;
use std::time::Duration;
use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::{Error as WsError, Message};

const INDEX_HTML: &str = include_str!("index.html");

pub struct Cfg {
    pub root: PathBuf,
    pub data: PathBuf,
    pub ports: Vec<String>,
}

static CFG: OnceLock<Cfg> = OnceLock::new();
pub fn cfg() -> &'static Cfg {
    CFG.get().expect("cfg")
}

static CLIENTS: Mutex<Vec<Sender<String>>> = Mutex::new(Vec::new());

/// Send a message to all open pages.
pub fn broadcast(v: &Value) {
    broadcast_raw(v.to_string());
}

pub fn broadcast_raw(s: String) {
    CLIENTS
        .lock()
        .unwrap()
        .retain(|c| c.send(s.clone()).is_ok());
}

fn usage() -> ! {
    println!(
        "maarse-lab [--addr 127.0.0.1:8090] [--no-browser] [--root <repo>] [--port COM6 --port COM7]\n\n\
         Test lab for Må:rse. Opens a page in the browser where all tests,\n\
         fuzzing, simulator scenarios and benchmarks can be run and watched live.\n\
         --port opens real boards (the companion protocol) and shows their status live;\n\
         without --port no serial ports are touched."
    );
    std::process::exit(0)
}

fn find_root() -> PathBuf {
    let ok = |p: &PathBuf| p.join("maarse-core").is_dir() && p.join("Cargo.toml").is_file();
    let mut cands = vec![std::env::current_dir().unwrap_or_default()];
    if let Ok(exe) = std::env::current_exe() {
        cands.extend(exe.ancestors().map(|p| p.to_path_buf()));
    }
    for c in cands {
        let mut p = Some(c.as_path());
        while let Some(d) = p {
            let d2 = d.to_path_buf();
            if ok(&d2) {
                return d2;
            }
            p = d.parent();
        }
    }
    std::env::current_dir().unwrap_or_default()
}

fn main() {
    let mut addr: SocketAddr = "127.0.0.1:8090".parse().unwrap();
    let mut browser = true;
    let mut root = None;
    let mut ports = Vec::new();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        let v = args.get(i + 1).cloned().unwrap_or_default();
        match args[i].as_str() {
            "--addr" => {
                addr = v.parse().expect("--addr ip:port");
                i += 1;
            }
            "--root" => {
                root = Some(PathBuf::from(v));
                i += 1;
            }
            "--port" => {
                ports.push(v);
                i += 1;
            }
            "--no-browser" => browser = false,
            "--help" | "-h" => usage(),
            o => {
                eprintln!("unknown argument: {o}");
                usage()
            }
        }
        i += 1;
    }
    if !addr.ip().is_loopback() {
        eprintln!("The test lab must listen on a loopback address");
        std::process::exit(2);
    }
    let root = root.unwrap_or_else(find_root);
    let data = root.join("maarse-lab-data");
    let _ = std::fs::create_dir_all(&data);
    let _ = CFG.set(Cfg { root, data, ports });

    jobs::init();
    hw::spawn_all();

    let listener = TcpListener::bind(addr).unwrap_or_else(|e| {
        eprintln!("cannot listen on {addr}: {e}");
        std::process::exit(1)
    });
    let url = format!("http://{addr}/");
    println!("Må:rse test lab: {url}");
    println!("Repo: {}", cfg().root.display());
    if browser {
        open_browser(&url);
    }
    let active = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming().flatten() {
        if active.load(Ordering::Relaxed) >= 16 { continue; }
        active.fetch_add(1, Ordering::Relaxed);
        let slot = LabConnection(active.clone());
        let _ = thread::Builder::new().name("lab-client".into()).spawn(move || {
            let _slot = slot;
            handle(stream);
        });
    }
}

struct LabConnection(Arc<AtomicUsize>);

impl Drop for LabConnection {
    fn drop(&mut self) { self.0.fetch_sub(1, Ordering::Relaxed); }
}

fn open_browser(url: &str) {
    let r = if cfg!(windows) {
        std::process::Command::new("cmd").args(["/C", "start", "", url]).spawn()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(url).spawn()
    };
    if r.is_err() {
        println!("Open {url} in your browser.");
    }
}

fn handle(stream: TcpStream) {
    let timeout = Some(Duration::from_secs(5));
    if stream.set_read_timeout(timeout).is_err() || stream.set_write_timeout(timeout).is_err() { return; }
    let mut peek = [0u8; 2048];
    let n = stream.peek(&mut peek).unwrap_or(0);
    let head = String::from_utf8_lossy(&peek[..n]).to_ascii_lowercase();
    if head.contains("upgrade: websocket") {
        websocket(stream);
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
        _ => ("404 Not Found", "text/plain; charset=utf-8", "Not found"),
    };
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.write_all(body.as_bytes());
}

fn local_browser(host: &str, origin: Option<&str>, listener: SocketAddr) -> bool {
    let Ok(authority) = host.parse::<tungstenite::http::uri::Authority>() else { return false };
    let name = authority.host();
    let local = name.eq_ignore_ascii_case("localhost")
        || name.trim_matches(['[', ']']).parse::<IpAddr>().is_ok_and(|ip| ip == listener.ip());
    listener.ip().is_loopback() && local && (authority.port_u16().unwrap_or(80) == listener.port())
        && origin.is_some_and(|origin| origin == format!("http://{host}"))
}

/// Only pages from the actual loopback listener may control the lab.
#[allow(clippy::result_large_err)]
fn same_origin(req: &Request, resp: Response, listener: SocketAddr) -> Result<Response, ErrorResponse> {
    let h = req.headers();
    let host = h.get("host").and_then(|value| value.to_str().ok()).unwrap_or("");
    let origin = h.get("origin").and_then(|value| value.to_str().ok());
    if local_browser(host, origin, listener) { return Ok(resp); }
    let mut response = ErrorResponse::new(Some("A local lab page is required".into()));
    *response.status_mut() = tungstenite::http::StatusCode::FORBIDDEN;
    Err(response)
}

fn websocket(stream: TcpStream) {
    let Ok(listener) = stream.local_addr() else { return };
    let config = tungstenite::protocol::WebSocketConfig {
        max_frame_size: Some(16 * 1024), max_message_size: Some(16 * 1024), ..Default::default()
    };
    let Ok(mut ws) = tungstenite::accept_hdr_with_config(stream, |request: &Request, response: Response| same_origin(request, response, listener), Some(config)) else {
        return;
    };
    let _ = ws.get_ref().set_read_timeout(Some(Duration::from_millis(20)));
    let (tx, rx): (Sender<String>, Receiver<String>) = channel();
    // First message: catalogue, test list, history and hardware status.
    let _ = tx.send(jobs::hello().to_string());
    for v in hw::snapshot() {
        let _ = tx.send(v.to_string());
    }
    CLIENTS.lock().unwrap().push(tx);
    'outer: loop {
        match ws.read() {
            Ok(Message::Text(t)) => {
                if let Ok(v) = serde_json::from_str::<Value>(&t) {
                    command(&v);
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
        let mut n = 0;
        while let Ok(m) = rx.try_recv() {
            if ws.write(Message::Text(m)).is_err() {
                break 'outer;
            }
            n += 1;
            if n > 200 {
                break;
            }
        }
        if n > 0 && ws.flush().is_err() {
            break;
        }
    }
}

fn command(v: &Value) {
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    match s("cmd").as_str() {
        "run" => jobs::enqueue(&s("suite")),
        "runall" => jobs::enqueue_all(),
        "stop" => jobs::stop(),
        "discover" => jobs::discover(),
        "sim" => {
            let args: Vec<String> = v
                .get("args")
                .and_then(|a| a.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                .unwrap_or_default();
            jobs::sim_live(&s("tag"), args);
        }
        "simstop" => jobs::sim_stop(),
        "clearhist" => jobs::clear_history(),
        _ => broadcast(&json!({"type":"error","text":"unknown command"})),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_loopback_browser_can_control_the_lab() {
        let address = "127.0.0.1:8090".parse().unwrap();
        assert!(local_browser("127.0.0.1:8090", Some("http://127.0.0.1:8090"), address));
        assert!(local_browser("localhost:8090", Some("http://localhost:8090"), address));
        assert!(!local_browser("example.test:8090", Some("http://example.test:8090"), address));
        assert!(!local_browser("localhost:8090", None, address));
    }
}
