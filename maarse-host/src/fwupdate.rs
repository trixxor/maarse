//! Firmware updates of the nodes attached to this host: a USB radio, a radio
//! reached over Bluetooth from the phone, and an ESP32 that joined the host over
//! Wi-Fi. The host is the one that fetches and pushes; the node only ever
//! receives, so it needs no internet and no open port.
//!
//! * Where the firmware comes from: the newest GitHub release (the signed
//!   `maarse-<board>-ota.manifest` and `.bin`), or files the person uploaded
//!   (from their own build). Both are kept under `<data>/firmware/`.
//! * What is trusted: nothing on the way. The node checks the Ed25519 signature
//!   (the release key built into its firmware), the board, the version
//!   (never older) and the SHA-256 before it installs; it keeps the old
//!   firmware and goes back to it if the new one does not start. The host
//!   checks the same signature first so a wrong file is refused early.
//! * How: the companion protocol's `OtaBegin`/`OtaChunk`/`OtaEnd` (see
//!   `maarse_crypto::ota`), one chunk at a time. On a USB or Bluetooth modem
//!   they use the serial stream; on a Wi-Fi modem they are carried on the
//!   existing link (`0xFF 'O' 'T' | frame` to the node, `0x7D | frame` back).
//!
//! ```text
//! GET  /api/firmware                  devices, what is available, the running job
//! POST /api/firmware/check            look for the newest release on GitHub now
//! POST /api/firmware/upload?name=f    a .manifest or .bin file from the person's PC (raw body);
//!                                     `&source=github` when the app downloaded it from the release
//! POST /api/firmware/update           {"id": link, "source": "github" | "local"}
//! POST /api/firmware/stop             stop the running job
//! ```
//! The Android app has no `curl`: it downloads the two files itself and posts
//! them to `upload`, then asks for `"source": "local"`.

use maarse_core::companion::{Command, Event, ota_state, text};
use maarse_crypto::ota::{self, Manifest};
use serde_json::{Value as J, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::Duration;

/// Where the newest release's firmware is published.
pub const LATEST_URL: &str = "https://github.com/trixxor/maarse/releases/latest/download/";
/// Bytes per `OtaChunk` (a multiple of 4; a Wi-Fi frame must stay under 255).
const CHUNK: usize = 200;
/// How long a node may be silent before we ask it where it is.
const REPLY_WAIT: Duration = Duration::from_secs(4);
/// Silent this many times in a row: give up.
const MAX_SILENT: u32 = 8;
const LARGEST_IMAGE: usize = 3 * 1024 * 1024;

/// Sends one encoded companion command (with its length field) to a node.
pub type SendFn = Arc<dyn Fn(Vec<u8>) -> bool + Send + Sync>;

/// What a node last said about firmware.
#[derive(Clone, Default)]
struct Ota {
    state: u8,
    next: u32,
    current: u32,
    text: String,
}

struct Dev {
    id: u32,
    kind: &'static str,
    name: String,
    send: SendFn,
    /// The board, from `OtaInfo` (0: not told, so old firmware).
    board: u16,
    last: Option<Ota>,
    seen: i64,
    waiter: Option<Sender<Ota>>,
}

#[derive(Clone)]
struct Job {
    id: u32,
    name: String,
    /// `running`, `done` or `error`.
    state: &'static str,
    sent: u32,
    total: u32,
    version: u32,
    text: String,
    started: i64,
}

#[derive(Default)]
struct Store {
    dir: Option<PathBuf>,
    checked_at: i64,
    error: String,
    job: Option<Job>,
}

static DEVS: LazyLock<Mutex<Vec<Dev>>> = LazyLock::new(|| Mutex::new(Vec::new()));
static STORE: LazyLock<Mutex<Store>> = LazyLock::new(|| Mutex::new(Store::default()));
static STOP: AtomicBool = AtomicBool::new(false);

fn devs() -> MutexGuard<'static, Vec<Dev>> {
    DEVS.lock().unwrap_or_else(|e| e.into_inner())
}

fn store() -> MutexGuard<'static, Store> {
    STORE.lock().unwrap_or_else(|e| e.into_inner())
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// The public release key built into this program, if there is a valid one.
fn release_key() -> Option<[u8; 32]> {
    ota::parse_hex32(option_env!("MAARSE_RELEASE_PUBKEY")?).filter(ota::valid_release_key)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn ver(v: u32) -> String {
    let (a, b, c) = ota::version_parts(v);
    format!("{a}.{b}.{c}")
}

/// Opens the firmware folder (once at start-up).
pub fn open(data_dir: &Path) {
    let dir = data_dir.join("firmware");
    let _ = std::fs::create_dir_all(dir.join("local"));
    let _ = std::fs::create_dir_all(dir.join("github"));
    let mut s = store();
    if let Ok(t) = std::fs::read_to_string(dir.join("state.json"))
        && let Ok(j) = serde_json::from_str::<J>(&t)
    {
        s.checked_at = j["checked_at"].as_i64().unwrap_or(0);
    }
    s.dir = Some(dir);
}

// ---------- the nodes ----------

/// A modem link came up: remember how to reach it and ask what it runs.
pub fn link_up(id: u32, kind: &'static str, name: &str, send: SendFn) {
    let ask = Command::OtaStatus.encode().to_vec();
    devs().push(Dev { id, kind, name: name.to_string(), send: send.clone(), board: 0, last: None, seen: 0, waiter: None });
    send(ask);
}

pub fn link_down(id: u32) {
    devs().retain(|d| d.id != id);
}

/// A frame from the node on link `id`; only `OtaState` and `OtaInfo` matter here.
pub fn on_frame(id: u32, frame: &[u8]) {
    let (state, next, current, text) = match Event::decode(frame) {
        Ok(Event::OtaInfo { board }) => {
            if let Some(dev) = devs().iter_mut().find(|d| d.id == id) {
                dev.board = board;
                dev.seen = now();
            }
            return;
        }
        Ok(Event::OtaState { state, next, current, text, .. }) => (state, next, current, text),
        _ => return,
    };
    let o = Ota { state, next, current, text: String::from_utf8_lossy(&text).into_owned() };
    let waiter = {
        let mut d = devs();
        let Some(dev) = d.iter_mut().find(|d| d.id == id) else { return };
        dev.seen = now();
        dev.last = Some(o.clone());
        dev.waiter.clone()
    };
    if let Some(w) = waiter {
        let _ = w.send(o);
    }
}

/// Asks every attached node what it runs (the list refreshes itself).
fn refresh() {
    let list: Vec<SendFn> = devs().iter().map(|d| d.send.clone()).collect();
    for s in list {
        s(Command::OtaStatus.encode().to_vec());
    }
}

// ---------- firmware files ----------

struct Image {
    manifest: Manifest,
    manifest_bytes: Vec<u8>,
    image: Option<PathBuf>,
}

fn folder(source: &str) -> Option<PathBuf> {
    store().dir.as_ref().map(|d| d.join(if source == "local" { "local" } else { "github" }))
}

/// The newest firmware for `board` in a source folder, with the image if it is there.
fn find(source: &str, board: u16) -> Option<Image> {
    let dir = folder(source)?;
    let mut best: Option<Image> = None;
    for e in std::fs::read_dir(&dir).ok()?.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("manifest") {
            continue;
        }
        let Ok(bytes) = std::fs::read(&p) else { continue };
        let Ok(m) = Manifest::parse(&bytes) else { continue };
        if m.board != board || best.as_ref().is_some_and(|b| b.manifest.version >= m.version) {
            continue;
        }
        let img = dir.join(format!("{}.bin", hex(&m.sha256)));
        best = Some(Image { manifest: m, manifest_bytes: bytes, image: img.exists().then_some(img) });
    }
    best
}

/// A manifest is only kept when its signature matches the release key (when
/// this program has one; otherwise the node decides).
fn accept_manifest(bytes: &[u8]) -> Result<Manifest, String> {
    let m = Manifest::parse(bytes).map_err(|e| e.text().to_string())?;
    if let Some(key) = release_key() {
        m.verify_signature(&key).map_err(|_| "The manifest is not signed with the Må:rse release key.".to_string())?;
    }
    Ok(m)
}

fn save_manifest(source: &str, bytes: &[u8]) -> Result<Manifest, String> {
    let m = accept_manifest(bytes)?;
    let dir = folder(source).ok_or("The firmware folder is not open.")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(format!("{}-{:08x}.manifest", ota::board::name(m.board), m.version)), bytes).map_err(|e| e.to_string())?;
    Ok(m)
}

fn save_image(source: &str, bytes: &[u8]) -> Result<String, String> {
    if bytes.len() > LARGEST_IMAGE {
        return Err("The file is too large for a firmware image.".into());
    }
    let sha = hex(&ota::sha256(bytes));
    let dir = folder(source).ok_or("The firmware folder is not open.")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(format!("{sha}.bin")), bytes).map_err(|e| e.to_string())?;
    Ok(sha)
}

fn curl(url: &str, secs: &str) -> Result<Vec<u8>, String> {
    let program = if cfg!(windows) { "curl.exe" } else { "curl" };
    let out = std::process::Command::new(program)
        .args(["-sSL", "--fail", "--max-time", secs, "-H", "User-Agent: maarse-host", url])
        .output()
        .map_err(|e| format!("{program} could not be started: {e}"))?;
    if out.status.success() { Ok(out.stdout) } else { Err(String::from_utf8_lossy(&out.stderr).trim().to_string()) }
}

/// Looks for the newest release on GitHub: the small signed manifests only.
pub fn check_now() -> Result<String, String> {
    let mut found = 0;
    let mut last_error = String::new();
    for b in [ota::board::XIAO_NRF52840, ota::board::WISMESH_TAG, ota::board::HELTEC_V4, ota::board::XIAO_ESP32S3] {
        match curl(&format!("{LATEST_URL}maarse-{}-ota.manifest", ota::board::name(b)), "30") {
            Ok(bytes) => match save_manifest("github", &bytes) {
                Ok(_) => found += 1,
                Err(e) => last_error = e,
            },
            Err(e) => last_error = e,
        }
    }
    let mut s = store();
    s.checked_at = now();
    if found == 0 {
        s.error = if last_error.is_empty() { "Nothing found on GitHub.".into() } else { last_error };
        return Err(s.error.clone());
    }
    s.error.clear();
    if let Some(d) = &s.dir {
        let _ = std::fs::write(d.join("state.json"), json!({"checked_at": s.checked_at}).to_string());
    }
    Ok(format!("Found firmware for {found} board(s) on GitHub."))
}

/// Makes sure the image for a manifest from GitHub is on disk and intact.
fn fetch_image(m: &Manifest) -> Result<PathBuf, String> {
    let dir = folder("github").ok_or("The firmware folder is not open.")?;
    let path = dir.join(format!("{}.bin", hex(&m.sha256)));
    if path.exists() {
        return Ok(path);
    }
    let bytes = curl(&format!("{LATEST_URL}maarse-{}-ota.bin", ota::board::name(m.board)), "300")?;
    if bytes.len() != m.size as usize || ota::sha256(&bytes) != m.sha256 {
        return Err("The download does not match its signed manifest (a newer release may have come out; check again).".into());
    }
    save_image("github", &bytes)?;
    Ok(path)
}

// ---------- the update itself ----------

fn set_job(f: impl FnOnce(&mut Job)) {
    if let Some(j) = store().job.as_mut() {
        f(j);
    }
}

/// Starts updating the node on link `id` from `source` (`github` or `local`).
pub fn start(id: u32, source: &str) -> Result<String, String> {
    if store().job.as_ref().is_some_and(|j| j.state == "running") {
        return Err("Another update is running.".into());
    }
    let (name, send, last, board) = {
        let d = devs();
        let dev = d.iter().find(|d| d.id == id).ok_or("That device is not connected any more.")?;
        (dev.name.clone(), dev.send.clone(), dev.last.clone(), dev.board)
    };
    let last = last.ok_or("The device has not said what it runs yet. Wait a moment and try again.")?;
    if last.state == ota_state::UNSUPPORTED {
        return Err(format!("This device cannot be updated over the air: {}", last.text));
    }
    if board == 0 {
        return Err("This device runs old firmware that cannot tell its board. Update it once over USB (web flasher).".into());
    }
    if source == "github" && find("github", board).is_none() {
        check_now()?;
    }
    let img = find(if source == "local" { "local" } else { "github" }, board)
        .ok_or_else(|| format!("No firmware for {} found.", ota::board::name(board)))?;
    if img.manifest.version <= last.current {
        return Err(format!("Already up to date (running {}, offered {}).", ver(last.current), ver(img.manifest.version)));
    }
    let path = match img.image {
        Some(p) => p,
        None if source == "local" => return Err("The image file for that manifest has not been uploaded.".into()),
        None => fetch_image(&img.manifest)?,
    };
    let image = std::fs::read(&path).map_err(|e| e.to_string())?;
    if image.len() != img.manifest.size as usize || ota::sha256(&image) != img.manifest.sha256 {
        return Err("The image does not match its manifest.".into());
    }
    let (tx, rx) = channel::<Ota>();
    if let Some(d) = devs().iter_mut().find(|d| d.id == id) {
        d.waiter = Some(tx);
    }
    STOP.store(false, Ordering::Relaxed);
    let job = Job { id, name: name.clone(), state: "running", sent: 0, total: image.len() as u32, version: img.manifest.version, text: "Starting".into(), started: now() };
    store().job = Some(job);
    let manifest_bytes = img.manifest_bytes;
    std::thread::Builder::new()
        .name("firmware-update".into())
        .spawn(move || {
            let result = run_job(&send, &rx, &manifest_bytes, &image);
            if let Some(d) = devs().iter_mut().find(|d| d.id == id) {
                d.waiter = None;
            }
            set_job(|j| match result {
                Ok(t) => {
                    j.state = "done";
                    j.text = t;
                    j.sent = j.total;
                }
                Err(t) => {
                    j.state = "error";
                    j.text = t;
                }
            });
        })
        .map_err(|e| e.to_string())?;
    Ok(format!("Updating {name} to {}.", ver(img.manifest.version)))
}

fn command(send: &SendFn, c: Command) -> Result<(), String> {
    if send(c.encode().to_vec()) { Ok(()) } else { Err("The connection to the device was lost.".into()) }
}

fn run_job(send: &SendFn, rx: &Receiver<Ota>, manifest: &[u8], image: &[u8]) -> Result<String, String> {
    let begin = || command(send, Command::OtaBegin { req: 0, flags: 0, manifest: text(manifest) });
    while rx.try_recv().is_ok() {} // Old answers.
    begin()?;
    let mut silent = 0;
    // The offset whose acknowledgement we wait for; `None` before the first chunk and after a silence.
    let mut expect: Option<u32> = None;
    let mut resync = true;
    loop {
        if STOP.load(Ordering::Relaxed) {
            return Err("Stopped. The node keeps what it has and can continue later.".into());
        }
        let ev = match rx.recv_timeout(REPLY_WAIT) {
            Ok(ev) => ev,
            Err(RecvTimeoutError::Timeout) => {
                silent += 1;
                if silent >= MAX_SILENT {
                    return Err("The node does not answer. Check its power and Wi-Fi, then continue.".into());
                }
                resync = true;
                command(send, Command::OtaStatus)?;
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => return Err("The connection to the device was lost.".into()),
        };
        silent = 0;
        match ev.state {
            ota_state::RECEIVING => {
                // Only the acknowledgement of the last chunk moves us on, unless we asked where the node is.
                if !resync && expect != Some(ev.next) {
                    continue;
                }
                resync = false;
                let next = ev.next as usize;
                set_job(|j| {
                    j.sent = next as u32;
                    j.text = "Sending".into();
                });
                if next >= image.len() {
                    continue; // The node is about to answer "verified".
                }
                let end = (next + CHUNK).min(image.len());
                expect = Some(end as u32);
                command(send, Command::OtaChunk { req: 0, offset: next as u32, data: text(&image[next..end]) })?;
            }
            ota_state::VERIFIED => {
                set_job(|j| j.text = "Checked; installing".into());
                command(send, Command::OtaEnd { req: 0, apply: true })?;
            }
            ota_state::INSTALLING => {
                return Ok("Installed. The node restarts and comes back in about a minute.".into());
            }
            ota_state::IDLE if resync => {
                resync = false;
                begin()?;
            }
            ota_state::IDLE => {}
            _ => return Err(if ev.text.is_empty() { "The node refused the update.".into() } else { ev.text }),
        }
    }
}

pub fn stop() {
    STOP.store(true, Ordering::Relaxed);
}

// ---------- the web API ----------

fn board_json(source: &str) -> Vec<J> {
    [ota::board::XIAO_NRF52840, ota::board::WISMESH_TAG, ota::board::HELTEC_V4, ota::board::XIAO_ESP32S3]
        .iter()
        .filter_map(|&b| {
            let i = find(source, b)?;
            Some(json!({"board": b, "name": ota::board::name(b), "version": i.manifest.version, "version_text": ver(i.manifest.version),
                        "size": i.manifest.size, "image": i.image.is_some()}))
        })
        .collect()
}

pub fn status_json() -> J {
    let list: Vec<J> = devs()
        .iter()
        .map(|d| {
            let board = d.board;
            let (current, state, note) = d.last.as_ref().map_or((0, 255, String::new()), |l| (l.current, l.state, l.text.clone()));
            let newer = |src: &str| find(src, board).filter(|i| i.manifest.version > current).map(|i| i.manifest.version);
            json!({
                "id": d.id, "name": d.name, "kind": d.kind, "seen_s": if d.seen == 0 { None } else { Some(now() - d.seen) },
                "board": board, "board_name": if board == 0 { "" } else { ota::board::name(board) },
                "current": current, "current_text": if current == 0 { String::new() } else { ver(current) },
                // 255: has not answered yet; 5: this board cannot be updated over the air.
                "state": state, "note": note,
                "can_update": board != 0 && state != ota_state::UNSUPPORTED && state != 255,
                "github_newer": if board == 0 { None } else { newer("github").map(ver) },
                "local_newer": if board == 0 { None } else { newer("local").map(ver) },
            })
        })
        .collect();
    // `board_json` takes the store lock itself, so it runs before we hold it.
    let (github, local) = (board_json("github"), board_json("local"));
    let s = store();
    json!({
        "devices": list,
        "github": {"checked_at": s.checked_at, "error": s.error, "boards": github, "source": LATEST_URL},
        "local": local,
        "can_verify": release_key().is_some(),
        "job": s.job.as_ref().map(|j| json!({"id": j.id, "name": j.name, "state": j.state, "sent": j.sent, "total": j.total,
                                              "version": ver(j.version), "text": j.text, "started": j.started})),
    })
}

fn with_message(mut j: J, ok: bool, m: &str) -> (&'static str, String) {
    j["message"] = json!(m);
    (if ok { "200 OK" } else { "400 Bad Request" }, j.to_string())
}

fn query_value<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|p| p.strip_prefix(key)?.strip_prefix('='))
}

/// The firmware routes; `None` when the path is not one of them.
pub fn api(method: &str, path: &str, query: &str, body: &[u8]) -> Option<(&'static str, String)> {
    Some(match (method, path) {
        ("GET", "/api/firmware") => {
            refresh();
            ("200 OK", status_json().to_string())
        }
        ("POST", "/api/firmware/check") => {
            let r = check_now();
            with_message(status_json(), r.is_ok(), &r.unwrap_or_else(|e| e))
        }
        ("POST", "/api/firmware/upload") => {
            // `source=github`: the Android app (no curl here) downloaded the release files itself.
            let source = if query_value(query, "source") == Some("github") { "github" } else { "local" };
            let r = if body.len() == ota::MANIFEST_LEN && body.starts_with(b"MOTA") {
                save_manifest(source, body).map(|m| format!("Manifest saved: {} {}.", ota::board::name(m.board), ver(m.version)))
            } else if body.first() == Some(&0xE9) || query_value(query, "name").is_some_and(|n| n.ends_with(".bin")) {
                save_image(source, body).map(|_| "Image saved.".to_string())
            } else {
                Err("Upload the .manifest and the .bin file (from maarse-sign or a release).".into())
            };
            if source == "github" && r.is_ok() {
                store().checked_at = now();
            }
            with_message(status_json(), r.is_ok(), &r.unwrap_or_else(|e| e))
        }
        ("POST", "/api/firmware/update") => {
            let j: J = serde_json::from_slice(body).unwrap_or(J::Null);
            let (Some(id), source) = (j["id"].as_u64(), j["source"].as_str().unwrap_or("github")) else {
                return Some(("400 Bad Request", json!({"message": "Say which device."}).to_string()));
            };
            let r = start(id as u32, source);
            with_message(status_json(), r.is_ok(), &r.unwrap_or_else(|e| e))
        }
        ("POST", "/api/firmware/stop") => {
            stop();
            with_message(status_json(), true, "Stopping.")
        }
        _ => return None,
    })
}

/// A node for the tests: the firmware's own receiver (`maarse_crypto::ota`) on a flash in memory,
/// so the host's stop-and-wait is tested against the same rules the firmware uses.
#[cfg(test)]
pub(crate) mod testkit {
    use super::*;
    use ota::{Policy, Receiver as Rx, Staging};

    struct Flash(Vec<u8>, Vec<u8>);
    impl Staging for Flash {
        fn erase_block(&mut self, o: u32) -> Result<(), ()> {
            self.0[o as usize..(o + ota::BLOCK) as usize].fill(0xff);
            Ok(())
        }
        fn write(&mut self, o: u32, d: &[u8]) -> Result<(), ()> {
            self.0[o as usize..o as usize + d.len()].copy_from_slice(d);
            Ok(())
        }
        fn read(&mut self, o: u32, b: &mut [u8]) -> Result<(), ()> {
            b.copy_from_slice(&self.0[o as usize..o as usize + b.len()]);
            Ok(())
        }
        fn erase_meta(&mut self) -> Result<(), ()> {
            self.1.fill(0xff);
            Ok(())
        }
        fn write_meta(&mut self, o: u32, d: &[u8]) -> Result<(), ()> {
            self.1[o as usize..o as usize + d.len()].copy_from_slice(d);
            Ok(())
        }
        fn read_meta(&mut self, o: u32, b: &mut [u8]) -> Result<(), ()> {
            b.copy_from_slice(&self.1[o as usize..o as usize + b.len()]);
            Ok(())
        }
    }

    pub struct NodeSim {
        flash: Flash,
        rx: Rx,
        policy: Policy,
        board: u16,
        running: u32,
        /// What the node would boot after the update.
        pub installed: Vec<u8>,
    }

    impl NodeSim {
        pub fn new(key: [u8; 32], board: u16, running: u32) -> Self {
            NodeSim {
                flash: Flash(vec![0xff; 0x40000], vec![0xff; ota::BLOCK as usize]),
                rx: Rx::new(),
                policy: Policy { public_key: key, board, current_version: running, capacity: 0x30000 },
                board,
                running,
                installed: Vec::new(),
            }
        }

        /// One companion command (no length field) in, the node's reply frames (no length field) out.
        pub fn handle(&mut self, frame: &[u8]) -> Vec<Vec<u8>> {
            let cmd = Command::decode(frame).unwrap();
            let is_status = matches!(cmd, Command::OtaStatus);
            let (state, msg) = match cmd {
                Command::OtaBegin { manifest, .. } => match Manifest::parse(&manifest).and_then(|m| self.rx.begin(&mut self.flash, m, &self.policy, false)) {
                    Ok(_) => (ota_state::RECEIVING, String::new()),
                    Err(e) => (ota_state::ERROR, e.text().to_string()),
                },
                Command::OtaChunk { offset, data, .. } => match self.rx.chunk(&mut self.flash, offset, &data) {
                    Ok(_) if self.rx.end().is_ok() => (ota_state::VERIFIED, String::new()),
                    Ok(_) => (ota_state::RECEIVING, String::new()),
                    Err(_) => (ota_state::RECEIVING, "wrong offset".into()),
                },
                Command::OtaEnd { .. } => {
                    self.installed = self.flash.0[..self.rx.manifest.unwrap().size as usize].to_vec();
                    (ota_state::INSTALLING, String::new())
                }
                _ => (ota_state::IDLE, String::new()),
            };
            let ev = Event::OtaState { req: 0, state, next: self.rx.next, size: 0, version: 0, current: self.running, text: text(msg.as_bytes()) };
            let mut out = vec![ev.encode()[2..].to_vec()];
            if is_status {
                out.push(Event::OtaInfo { board: self.board }.encode()[2..].to_vec());
            }
            out
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::testkit::NodeSim;
    use super::*;
    use maarse_crypto::ota::SigningKey;
    use std::sync::atomic::AtomicU32;

    static N: AtomicU32 = AtomicU32::new(0);
    /// The tests share the global device list and folder, so they run one at a time.
    pub(crate) static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

    pub(crate) fn temp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("maarse-fw-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    /// A node behind a `SendFn`; its replies come back through `on_frame` as they would from a link.
    fn fake_node(link: u32, key: [u8; 32], board: u16, running: u32) -> (SendFn, Arc<Mutex<NodeSim>>) {
        let sim = Arc::new(Mutex::new(NodeSim::new(key, board, running)));
        let s2 = sim.clone();
        let send: SendFn = Arc::new(move |frame: Vec<u8>| {
            let replies = s2.lock().unwrap().handle(&frame[2..]);
            std::thread::spawn(move || {
                for f in replies {
                    on_frame(link, &f);
                }
            });
            true
        });
        (send, sim)
    }

    pub(crate) fn test_image(len: usize) -> Vec<u8> {
        let mut image: Vec<u8> = (0..len as u32).map(|i| (i * 7) as u8).collect();
        image[0] = 0xE9;
        image
    }

    #[test]
    fn an_update_goes_through_with_the_firmware_s_own_rules() {
        let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        let dir = temp();
        open(&dir);
        let key = SigningKey::from_bytes(&[11u8; 32]);
        let image = test_image(5004);
        let m = Manifest::sign(&key, ota::board::HELTEC_V4, 0x000200, &image);
        save_manifest("local", &m.to_bytes()).unwrap();
        save_image("local", &image).unwrap();
        let (send, sim) = fake_node(900, key.verifying_key().to_bytes(), ota::board::HELTEC_V4, 0x000100);
        link_up(900, "wifi-modem", "wifi-modem 10.0.0.9", send);
        std::thread::sleep(Duration::from_millis(100));
        assert!(status_json()["devices"][0]["can_update"].as_bool().unwrap(), "{}", status_json());
        assert_eq!(status_json()["devices"][0]["local_newer"], json!("0.2.0"));
        assert!(start(900, "local").is_ok());
        for _ in 0..200 {
            std::thread::sleep(Duration::from_millis(50));
            if status_json()["job"]["state"] != json!("running") {
                break;
            }
        }
        assert_eq!(status_json()["job"]["state"], json!("done"), "{}", status_json()["job"]);
        assert_eq!(sim.lock().unwrap().installed, image);
        link_down(900);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_node_that_already_runs_the_version_is_left_alone() {
        let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        let dir = temp();
        open(&dir);
        let key = SigningKey::from_bytes(&[12u8; 32]);
        let image = test_image(4096);
        let m = Manifest::sign(&key, ota::board::HELTEC_V4, 0x000200, &image);
        save_manifest("local", &m.to_bytes()).unwrap();
        save_image("local", &image).unwrap();
        let (send, _) = fake_node(901, key.verifying_key().to_bytes(), ota::board::HELTEC_V4, 0x000200);
        link_up(901, "usb-modem", "COM6", send);
        std::thread::sleep(Duration::from_millis(100));
        assert!(start(901, "local").unwrap_err().contains("up to date"));
        link_down(901);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_node_that_does_not_tell_its_board_is_sent_to_usb() {
        let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        let dir = temp();
        open(&dir);
        // Old firmware: it answers `OtaStatus` with `OtaState` only, never `OtaInfo`.
        let send: SendFn = Arc::new(|_| {
            let ev = Event::OtaState { req: 0, state: ota_state::IDLE, next: 0, size: 0, version: 0, current: 0x000100, text: text(b"") };
            let f = ev.encode()[2..].to_vec();
            std::thread::spawn(move || on_frame(902, &f));
            true
        });
        link_up(902, "usb-modem", "COM9", send);
        std::thread::sleep(Duration::from_millis(100));
        let d = status_json()["devices"][0].clone();
        assert_eq!(d["can_update"], json!(false));
        assert!(start(902, "local").unwrap_err().contains("USB"));
        link_down(902);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn uploads_are_recognised_and_a_wrong_file_is_refused() {
        let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        let dir = temp();
        open(&dir);
        assert_eq!(api("POST", "/api/firmware/upload", "name=x.txt", b"hello").unwrap().0, "400 Bad Request");
        let (code, _) = api("POST", "/api/firmware/upload", "name=a.bin", &[0xE9, 1, 2, 3]).unwrap();
        assert_eq!(code, "200 OK");
        assert!(api("GET", "/api/other", "", b"").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
