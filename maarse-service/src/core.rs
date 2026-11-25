//! What the service does: runs the host, looks for and installs signed updates, answers the tray app.

use std::fs;
use std::io::Write;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use maarse_service::ipc::{SERVICE_NAME, Status, UpdateInfo};
use maarse_service::update::{self, Release};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const DETACHED_PROCESS: u32 = 0x0000_0008;
const VERSION: &str = match option_env!("MAARSE_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};
/// Only release builds (built with `MAARSE_VERSION`) look for updates; a development build would always find one.
const UPDATES_ENABLED: bool = option_env!("MAARSE_VERSION").is_some();
const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum State {
    Starting = 0,
    Running = 1,
    Stopped = 2,
    Updating = 3,
}

impl State {
    fn name(v: u8) -> &'static str {
        match v {
            1 => "running",
            2 => "stopped",
            3 => "updating",
            _ => "starting",
        }
    }
}

static STATE: AtomicU8 = AtomicU8::new(State::Starting as u8);
/// The host process, so the service can end it.
static CHILD: Mutex<Option<Child>> = Mutex::new(None);
/// Set while the service itself ends the host (stop, update): the supervisor must not start it again.
static STOPPING: AtomicBool = AtomicBool::new(false);
static SUPERVISOR_ACTIVE: AtomicBool = AtomicBool::new(false);
static LATEST: Mutex<Option<Release>> = Mutex::new(None);
static LAST_ERROR: Mutex<String> = Mutex::new(String::new());
/// Set by `maarse-service run`: no access restrictions on the data folder (the user running the test owns it).
pub static CONSOLE: AtomicBool = AtomicBool::new(false);
static STOP: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());

fn set_state(s: State) {
    STATE.store(s as u8, Ordering::SeqCst);
}

// ---- folders ----------------------------------------------------------------

fn install_dir() -> PathBuf {
    std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)).unwrap_or_default()
}

fn root_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("ProgramData").unwrap_or_else(|| r"C:\ProgramData".into())).join("Maarse")
}

fn data_dir() -> PathBuf {
    root_dir().join("data")
}

fn log_dir() -> PathBuf {
    root_dir().join("logs")
}

pub fn log_line(text: &str) {
    let _ = fs::create_dir_all(log_dir());
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(log_dir().join("service.log")) {
        let _ = writeln!(f, "{text}");
    }
}

fn icacls(path: &Path, args: &[&str]) {
    let _ = Command::new("icacls.exe").arg(path).args(args).creation_flags(CREATE_NO_WINDOW).output();
}

/// Creates the folders. The data folder (identity, messages, access key) is only for SYSTEM and
/// administrators; the log folder can also be read by users, for "Open log folder" in the tray app.
/// On the first start the host becomes a client: it forwards nothing for others until the user
/// chooses another role under Settings. The marker keeps a later choice from being overwritten.
fn init_first_run() -> Result<(), String> {
    let first = !root_dir().exists();
    for d in [data_dir(), log_dir()] {
        fs::create_dir_all(&d).map_err(|e| format!("Could not create {}: {e}", d.display()))?;
    }
    if first && !CONSOLE.load(Ordering::SeqCst) {
        icacls(&root_dir(), &["/inheritance:r", "/grant:r", "*S-1-5-18:(OI)(CI)F", "*S-1-5-32-544:(OI)(CI)F"]);
        icacls(&log_dir(), &["/grant:r", "*S-1-5-32-545:(OI)(CI)RX"]);
    }
    let settings = data_dir().join("settings.txt");
    let marker = data_dir().join(".service-initialised");
    if !marker.exists() {
        let old = fs::read_to_string(&settings).unwrap_or_default();
        if !old.lines().any(|l| l.trim().starts_with("--role")) {
            let mut text = old;
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str("--role client\n");
            fs::write(&settings, text).map_err(|e| format!("Could not write the settings: {e}"))?;
        }
        fs::write(&marker, "role client chosen at first start\n").map_err(|e| e.to_string())?;
    }
    // Every client joins the open backbone (nj01) unless the person chose otherwise. Done once, also for
    // installs that already started: removing `--public` later in Settings sticks.
    let public = data_dir().join(".public-default");
    if !public.exists() {
        let old = fs::read_to_string(&settings).unwrap_or_default();
        let chosen = old.lines().any(|l| {
            let l = l.trim();
            l.starts_with("--public ") || l == "--public" || l.starts_with("--peer")
        });
        if !chosen {
            let mut text = old;
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str("--public\n");
            fs::write(&settings, text).map_err(|e| format!("Could not write the settings: {e}"))?;
        }
        fs::write(&public, "the open backbone is on by default\n").map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Removes the `.old` files an update leaves behind.
fn clean_old_files() {
    if let Ok(rd) = fs::read_dir(install_dir()) {
        for e in rd.flatten() {
            if e.path().extension().is_some_and(|x| x == "old") {
                let _ = fs::remove_file(e.path());
            }
        }
    }
}

// ---- the web client address -------------------------------------------------

fn http_addr() -> String {
    let text = fs::read_to_string(data_dir().join("settings.txt")).unwrap_or_default();
    let mut addr = "127.0.0.1:8080".to_string();
    for line in text.lines() {
        if let Some(v) = line.trim().strip_prefix("--http") {
            if !v.trim().is_empty() {
                addr = v.trim().to_string();
            }
        }
    }
    if let Some(port) = addr.strip_prefix("0.0.0.0:") {
        addr = format!("127.0.0.1:{port}");
    } else if let Some(port) = addr.strip_prefix("[::]:") {
        addr = format!("[::1]:{port}");
    }
    addr
}

fn client_url() -> String {
    let mut url = format!("http://{}/", http_addr());
    if let Ok(t) = fs::read_to_string(data_dir().join("access-key.txt")) {
        let t = t.trim();
        if !t.is_empty() {
            url.push_str(&format!("#token={t}"));
        }
    }
    url
}

fn web_up() -> bool {
    use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
    let addrs: Vec<SocketAddr> = http_addr().to_socket_addrs().map(|a| a.collect()).unwrap_or_default();
    addrs.iter().any(|a| TcpStream::connect_timeout(a, Duration::from_millis(500)).is_ok())
}

// ---- host supervisor --------------------------------------------------------

fn spawn_host() -> Result<Child, String> {
    let exe = install_dir().join("maarse-host.exe");
    let log_path = log_dir().join("host.log");
    if fs::metadata(&log_path).is_ok_and(|m| m.len() > 10_000_000) {
        let _ = fs::rename(&log_path, log_dir().join("host.old.log"));
    }
    let log = fs::OpenOptions::new().create(true).append(true).open(&log_path).map_err(|e| e.to_string())?;
    let log2 = log.try_clone().map_err(|e| e.to_string())?;
    Command::new(&exe)
        .arg("--service")
        .arg("--data")
        .arg(data_dir())
        .env("MAARSE_LAUNCHER", "1")
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(log2)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("Could not start {}: {e}", exe.display()))
}

/// Runs the host and starts it again when it stops, until the service ends it on purpose.
/// Exit code 75 = new settings, 76 = the web client asked for an update, 0 = Stop in the web client.
fn start_supervisor() {
    if SUPERVISOR_ACTIVE.swap(true, Ordering::SeqCst) {
        return;
    }
    STOPPING.store(false, Ordering::SeqCst);
    std::thread::spawn(|| {
        supervise();
        SUPERVISOR_ACTIVE.store(false, Ordering::SeqCst);
    });
}

fn supervise() {
    let mut crashes: Vec<Instant> = Vec::new();
    while !STOPPING.load(Ordering::SeqCst) {
        set_state(State::Starting);
        match spawn_host() {
            Ok(child) => *CHILD.lock().unwrap() = Some(child),
            Err(e) => {
                log_line(&e);
                set_state(State::Stopped);
                return;
            }
        }
        let code = loop {
            std::thread::sleep(Duration::from_millis(500));
            if STOPPING.load(Ordering::SeqCst) {
                return;
            }
            let mut guard = CHILD.lock().unwrap();
            let Some(child) = guard.as_mut() else { return };
            if let Ok(Some(status)) = child.try_wait() {
                *guard = None;
                break status.code().unwrap_or(-1);
            }
            drop(guard);
            if STATE.load(Ordering::SeqCst) == State::Starting as u8 && web_up() {
                set_state(State::Running);
            }
        };
        match code {
            0 => {
                // Stop in the web client: the host stays stopped until the tray app starts it.
                set_state(State::Stopped);
                return;
            }
            75 => continue,
            76 => {
                begin_update();
                return;
            }
            other => {
                log_line(&format!("maarse-host exited with code {other}, starting it again"));
                let now = Instant::now();
                crashes.retain(|t| now.duration_since(*t) < Duration::from_secs(60));
                crashes.push(now);
                if crashes.len() >= 5 {
                    set_state(State::Stopped);
                    return;
                }
                std::thread::sleep(Duration::from_secs(3));
            }
        }
    }
}

fn stop_host() {
    STOPPING.store(true, Ordering::SeqCst);
    if let Some(mut c) = CHILD.lock().unwrap().take() {
        let _ = c.kill();
        let _ = c.wait();
    }
}

/// Stops the host and waits (a few seconds at most) until the supervisor has ended.
fn stop_host_and_wait() {
    if matches!(STATE.load(Ordering::SeqCst), x if x == State::Updating as u8) {
        return;
    }
    stop_host();
    for _ in 0..50 {
        if !SUPERVISOR_ACTIVE.load(Ordering::SeqCst) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    set_state(State::Stopped);
    log_line("the host was stopped from the tray");
}

// ---- update -----------------------------------------------------------------

fn check_for_update() {
    if !UPDATES_ENABLED || update::release_key().is_none() {
        return;
    }
    match update::fetch_latest() {
        Ok(Some(r)) if update::is_newer(VERSION, &r.version) => *LATEST.lock().unwrap() = Some(r),
        Ok(_) => *LATEST.lock().unwrap() = None,
        Err(e) => log_line(&format!("update check failed: {e}")),
    }
}

/// Downloads the package, checks the signature, replaces the program files and starts the service again.
/// Files in use are renamed to `.old` first (Windows allows that for a running program).
fn apply_update(release: &Release) -> Result<(), String> {
    let key = update::release_key().ok_or("This build has no release key, so it cannot install updates.")?;
    let zip_bytes = update::download_verified(release, &key)?;

    let work = root_dir().join("update");
    let _ = fs::remove_dir_all(&work);
    let staging = work.join("files");
    fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    let zip = work.join(update::ASSET);
    fs::write(&zip, &zip_bytes).map_err(|e| e.to_string())?;
    let out = Command::new("tar.exe")
        .arg("-xf")
        .arg(&zip)
        .arg("-C")
        .arg(&staging)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("Unpacking failed: {e}"))?;
    if !out.status.success() {
        return Err(format!("Unpacking failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    for need in ["maarse.exe", "maarse-host.exe", "maarse-service.exe"] {
        if !staging.join(need).is_file() {
            return Err(format!("The package has no {need}."));
        }
    }

    stop_host();
    swap_files(&staging, &install_dir())?;
    let _ = fs::remove_dir_all(&work);
    Ok(())
}

/// Copies every file of the package into the install folder; undoes it all if one step fails.
fn swap_files(staging: &Path, dir: &Path) -> Result<(), String> {
    let mut done: Vec<(PathBuf, PathBuf, bool)> = Vec::new();
    let undo = |done: &[(PathBuf, PathBuf, bool)]| {
        for (target, old, had) in done.iter().rev() {
            let _ = fs::remove_file(target);
            if *had {
                let _ = fs::rename(old, target);
            }
        }
    };
    let entries = fs::read_dir(staging).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        if !entry.path().is_file() {
            continue;
        }
        let name = entry.file_name();
        let target = dir.join(&name);
        let mut old_name = name.clone();
        old_name.push(".old");
        let old = dir.join(old_name);
        let _ = fs::remove_file(&old);
        let had = target.exists();
        if had {
            if let Err(e) = fs::rename(&target, &old) {
                undo(&done);
                return Err(format!("Could not replace {}: {e}", target.display()));
            }
        }
        done.push((target.clone(), old, had));
        if let Err(e) = fs::copy(entry.path(), &target) {
            undo(&done);
            return Err(format!("Could not write {}: {e}", target.display()));
        }
    }
    Ok(())
}

/// Starts the update on a background thread. When it has worked, the service starts itself again from the
/// new files: a detached helper waits a few seconds and runs `sc start`, then this service stops.
fn begin_update() -> bool {
    if STATE.load(Ordering::SeqCst) == State::Updating as u8 {
        return true;
    }
    let release = {
        if LATEST.lock().unwrap().is_none() {
            check_for_update();
        }
        LATEST.lock().unwrap().clone()
    };
    let Some(release) = release else {
        *LAST_ERROR.lock().unwrap() = "No update was found.".into();
        if !STOPPING.load(Ordering::SeqCst) {
            start_supervisor();
        }
        return false;
    };
    set_state(State::Updating);
    LAST_ERROR.lock().unwrap().clear();
    std::thread::spawn(move || match apply_update(&release) {
        Ok(()) => {
            log_line(&format!("updated to {}, restarting the service", release.version));
            let _ = Command::new("cmd.exe")
                .args(["/c", &format!("ping -n 4 127.0.0.1 >nul & sc start {SERVICE_NAME}")])
                .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
                .spawn();
            request_stop();
        }
        Err(e) => {
            log_line(&format!("update failed: {e}"));
            *LAST_ERROR.lock().unwrap() = e;
            set_state(State::Starting);
            start_supervisor();
        }
    });
    true
}

// ---- life cycle and commands --------------------------------------------------

pub fn start() -> Result<(), String> {
    init_first_run()?;
    clean_old_files();
    if !install_dir().join("maarse-host.exe").is_file() {
        return Err(format!("maarse-host.exe was not found next to the service ({}).", install_dir().display()));
    }
    crate::pipe::spawn_server();
    crate::tray_guard::spawn(install_dir(), log_line, || STOPPING.load(Ordering::SeqCst));
    start_supervisor();
    std::thread::spawn(|| {
        loop {
            check_for_update();
            std::thread::sleep(CHECK_EVERY);
        }
    });
    Ok(())
}

pub fn request_stop() {
    *STOP.0.lock().unwrap() = true;
    STOP.1.notify_all();
}

pub fn wait_for_stop() {
    let mut stopped = STOP.0.lock().unwrap();
    while !*stopped {
        stopped = STOP.1.wait(stopped).unwrap();
    }
}

pub fn shutdown() {
    stop_host();
}

fn status() -> Status {
    Status {
        version: VERSION.to_string(),
        state: State::name(STATE.load(Ordering::SeqCst)).to_string(),
        url: client_url(),
        error: LAST_ERROR.lock().unwrap().clone(),
        update: LATEST
            .lock()
            .unwrap()
            .as_ref()
            .map(|r| UpdateInfo { version: r.version.clone(), notes: r.notes.clone(), page: r.page.clone() }),
    }
}

/// Answers one command from the pipe.
pub fn handle(command: &str) -> Status {
    match command.trim() {
        "check" => check_for_update(),
        "update" => {
            begin_update();
        }
        "start" => {
            if STATE.load(Ordering::SeqCst) == State::Stopped as u8 {
                start_supervisor();
            }
        }
        // Stops the host (and with it the serial ports it holds, so a radio can be flashed).
        // The service itself keeps running, so "start" brings the host back.
        "stop" => stop_host_and_wait(),
        "restart" => {
            stop_host_and_wait();
            start_supervisor();
        }
        _ => {}
    }
    status()
}
