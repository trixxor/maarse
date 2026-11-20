//! Settings that can be changed from the web client.
//!
//! They are stored in `<data>/settings.txt` as the same options the command line
//! takes – one per line, e.g. `--tun` or `--peer 10.0.0.2:4242`. At startup
//! the file is read first and the command line afterwards, so the latter wins.
//!
//! When the web client saves, the host restarts itself (same program, same
//! command line, same privileges – e.g. administrator for the TUN adapter).

use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub fn file(data_dir: &Path) -> PathBuf {
    data_dir.join("settings.txt")
}

/// Options that must not appear in the file.
const FORBIDDEN: &[&str] = &["--data", "--help", "-h"];

/// Turns text (one option per line) into arguments.
pub fn to_args(text: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (flag, val) = match line.split_once(char::is_whitespace) {
            Some((f, v)) => (f, Some(v.trim())),
            None => (line, None),
        };
        if !flag.starts_with("--") || FORBIDDEN.contains(&flag) {
            return Err(format!("invalid option in settings: {flag}"));
        }
        out.push(flag.to_string());
        if let Some(v) = val.filter(|v| !v.is_empty()) {
            out.push(v.to_string());
        }
    }
    Ok(out)
}

pub fn load(data_dir: &Path) -> String {
    std::fs::read_to_string(file(data_dir)).unwrap_or_default()
}

/// Saves new text if it can be parsed as options.
pub fn save(data_dir: &Path, text: &str) -> Result<(), String> {
    let mut args = to_args(text)?;
    // Checked together with the command line, which also applies (e.g. a
    // server's --public-addr given in its start script).
    args.extend(crate::cli_args().into_iter().filter(|a| !a.starts_with("--set-")));
    crate::parse_from(args)?;
    let body: String = text.lines().map(|l| format!("{}\n", l.trim_end())).collect();
    std::fs::write(file(data_dir), body).map_err(|e| format!("could not save: {e}"))
}

/// Were we started by a restart? Then we wait for the old process to release the ports.
pub fn restarted() -> bool {
    std::env::var_os("MAARSE_RESTART").is_some()
}

pub fn bind(addr: SocketAddr) -> std::io::Result<TcpListener> {
    let mut tries = if restarted() { 100 } else { 1 };
    loop {
        match TcpListener::bind(addr) {
            Ok(l) => return Ok(l),
            Err(e) if tries <= 1 => return Err(e),
            Err(_) => {
                tries -= 1;
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

/// Set by `--service`: a service manager owns the process lifetime.
pub static SERVICE: AtomicBool = AtomicBool::new(false);

/// Exit code used to ask the service manager for a restart (EX_TEMPFAIL).
pub const RESTART_EXIT_CODE: i32 = 75;

/// Starts a new process with the same command line and exits this one.
pub fn restart() -> ! {
    // The MeshCore radio worker is our child: stop it, so the new process starts its own.
    crate::meshcore_gateway::stop_worker();
    if SERVICE.load(std::sync::atomic::Ordering::Relaxed) {
        // A detached child would be killed with our cgroup (systemd) or be
        // invisible to the supervisor, so let the supervisor restart us.
        println!("
[settings] exiting with code {RESTART_EXIT_CODE} so the service manager restarts the host
");
        std::process::exit(RESTART_EXIT_CODE);
    }
    println!("\n[settings] restarting with new settings …\n");
    let exe = std::env::current_exe().expect("current_exe");
    match std::process::Command::new(exe)
        .args(crate::cli_args())
        .env("MAARSE_RESTART", "1")
        .spawn()
    {
        Ok(_) => std::process::exit(0),
        Err(e) => {
            eprintln!("[settings] restart failed: {e}");
            std::process::exit(1);
        }
    }
}

// ---------- status for the web client ----------

pub static TUN_UP: AtomicBool = AtomicBool::new(false);
static STARTUP: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Prints a startup line and remembers it so the web client can show it.
pub fn report(line: impl Into<String>, error: bool) {
    let line = line.into();
    if error {
        eprintln!("{line}");
    } else {
        println!("{line}");
    }
    if let Ok(mut v) = STARTUP.lock() {
        v.push(format!("{}{}", if error { "!" } else { "" }, line.trim()));
    }
}

fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn json_list<'a>(it: impl Iterator<Item = &'a str>) -> String {
    let v: Vec<String> = it.map(json_str).collect();
    format!("[{}]", v.join(","))
}

/// JSON with settings, serial ports and startup status.
pub fn status_json(data_dir: &Path) -> String {
    let ports: Vec<String> = serialport::available_ports()
        .map(|v| v.into_iter().map(|p| p.port_name).collect())
        .unwrap_or_default();
    let startup = STARTUP.lock().map(|v| v.clone()).unwrap_or_default();
    format!(
        "{{\"settings\":{},\"cli\":{},\"ports\":{},\"startup\":{},\"tun_feature\":{},\"tun_up\":{}}}",
        json_str(&load(data_dir)),
        json_list(crate::cli_args().iter().map(|s| s.as_str())),
        json_list(ports.iter().map(|s| s.as_str())),
        json_list(startup.iter().map(|s| s.as_str())),
        cfg!(feature = "tun"),
        TUN_UP.load(Ordering::Relaxed),
    )
}
