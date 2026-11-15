//! Indstillinger, der kan ændres fra webklienten.
//!
//! De gemmes i `<data>/indstillinger.txt` som de samme valg, kommandolinjen
//! tager – ét pr. linje, fx `--tun` eller `--peer 10.0.0.2:4242`. Ved opstart
//! læses filen først og kommandolinjen bagefter, så den vinder.
//!
//! Når webklienten gemmer, genstarter hosten sig selv (samme program, samme
//! kommandolinje, samme rettigheder – fx administrator til TUN-adapteren).

use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub fn file(data_dir: &Path) -> PathBuf {
    data_dir.join("indstillinger.txt")
}

/// Valg, der ikke må stå i filen.
const FORBIDDEN: &[&str] = &["--data", "--help", "-h"];

/// Laver tekst (ét valg pr. linje) om til argumenter.
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
            return Err(format!("ugyldigt valg i indstillinger: {flag}"));
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

/// Gemmer ny tekst, hvis den kan læses som valg.
pub fn save(data_dir: &Path, text: &str) -> Result<(), String> {
    let args = to_args(text)?;
    crate::parse_from(args)?;
    let body: String = text.lines().map(|l| format!("{}\n", l.trim_end())).collect();
    std::fs::write(file(data_dir), body).map_err(|e| format!("kunne ikke gemme: {e}"))
}

/// Startes vi af en genstart? Så venter vi på, at den gamle proces slipper portene.
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

/// Starter en ny proces med samme kommandolinje og afslutter denne.
pub fn restart() -> ! {
    if SERVICE.load(std::sync::atomic::Ordering::Relaxed) {
        // A detached child would be killed with our cgroup (systemd) or be
        // invisible to the supervisor, so let the supervisor restart us.
        println!("
[settings] exiting with code {RESTART_EXIT_CODE} so the service manager restarts the host
");
        std::process::exit(RESTART_EXIT_CODE);
    }
    println!("\n[indstillinger] genstarter med nye indstillinger …\n");
    let exe = std::env::current_exe().expect("current_exe");
    match std::process::Command::new(exe)
        .args(std::env::args().skip(1))
        .env("MAARSE_RESTART", "1")
        .spawn()
    {
        Ok(_) => std::process::exit(0),
        Err(e) => {
            eprintln!("[indstillinger] genstart fejlede: {e}");
            std::process::exit(1);
        }
    }
}

// ---------- status til webklienten ----------

pub static TUN_UP: AtomicBool = AtomicBool::new(false);
static STARTUP: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Skriver en opstartslinje og husker den, så webklienten kan vise den.
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

/// JSON med indstillinger, seriel-porte og opstartsstatus.
pub fn status_json(data_dir: &Path) -> String {
    let ports: Vec<String> = serialport::available_ports()
        .map(|v| v.into_iter().map(|p| p.port_name).collect())
        .unwrap_or_default();
    let startup = STARTUP.lock().map(|v| v.clone()).unwrap_or_default();
    format!(
        "{{\"settings\":{},\"cli\":{},\"ports\":{},\"startup\":{},\"tun_feature\":{},\"tun_up\":{}}}",
        json_str(&load(data_dir)),
        json_list(std::env::args().skip(1).collect::<Vec<_>>().iter().map(|s| s.as_str())),
        json_list(ports.iter().map(|s| s.as_str())),
        json_list(startup.iter().map(|s| s.as_str())),
        cfg!(feature = "tun"),
        TUN_UP.load(Ordering::Relaxed),
    )
}
