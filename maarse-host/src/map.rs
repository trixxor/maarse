//! The offline map of Denmark.
//!
//! The map is one Protomaps vector tile archive (PMTiles, OpenStreetMap data, ODbL). The project
//! publishes its own extracts as assets on the rolling GitHub release `map-dk` (see
//! `.github/workflows/map.yml` and `docs/map.md`), in two levels:
//!
//! * `overview`: zoom 0-10, about 22 MB, the default;
//! * `detailed`: zoom 0-13, about 185 MB, optional.
//!
//! This module downloads one of them next to the other data (`<data>/map/`), checks the SHA-256 from the
//! manifest, and keeps it as `denmark.pmtiles`. `web.rs` serves that file with HTTP Range requests at
//! `/map/denmark.pmtiles`, and the vendored MapLibre client files, style, fonts and sprites at `/map/*`, so
//! the web client and the Android app draw the map with no internet at all once the file is installed.
//!
//! Layout of `<data>/map/`:
//! * `denmark.pmtiles`: the installed map (served);
//! * `installed.json`: which level it is, its size, hash and build;
//! * `<name>.pmtiles.part`: a download in progress (resumed with a Range request after a restart or a
//!   failed network).
//!
//! Where there is no `curl` (the Android app) the app downloads the file itself to `part_path` (from the
//! status) and calls `POST /api/map/download`, which then only verifies and installs it.

use serde::Deserialize;
use serde_json::json;
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The manifest of the rolling release. The files are next to it (same folder).
pub const DEFAULT_MANIFEST_URL: &str = "https://github.com/trixxor/maarse/releases/download/map-dk/map-manifest.json";
/// Shown on the map and kept in the manifest; ODbL requires the attribution.
pub const ATTRIBUTION: &str = "\u{a9} OpenStreetMap contributors \u{b7} Protomaps";
const ACTIVE: &str = "denmark.pmtiles";
const INSTALLED: &str = "installed.json";
const MAX_MANIFEST: usize = 256 * 1024;

pub fn dir(data_dir: &Path) -> PathBuf {
    data_dir.join("map")
}

// ---------- manifest ----------

#[derive(Clone, Debug, Deserialize)]
pub struct FileEntry {
    pub level: String,
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
    #[serde(default)]
    pub bbox: Vec<f64>,
    #[serde(default)]
    pub minzoom: u8,
    pub maxzoom: u8,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Manifest {
    #[serde(default)]
    pub build: String,
    #[serde(default)]
    pub osm_data: String,
    pub files: Vec<FileEntry>,
}

fn safe_name(name: &str) -> bool {
    name.len() <= 80
        && name.ends_with(".pmtiles")
        && !name.starts_with('.')
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'-' | b'_'))
}

pub fn parse_manifest(bytes: &[u8]) -> Result<Manifest, String> {
    if bytes.len() > MAX_MANIFEST {
        return Err("The map manifest is too large".into());
    }
    let m: Manifest = serde_json::from_slice(bytes).map_err(|e| format!("The map manifest is not valid: {e}"))?;
    if m.files.is_empty() {
        return Err("The map manifest lists no files".into());
    }
    for f in &m.files {
        if !matches!(f.level.as_str(), "overview" | "detailed") {
            return Err(format!("The map manifest has an unknown level: {}", f.level));
        }
        if !safe_name(&f.name) {
            return Err(format!("The map manifest has an unsafe file name: {}", f.name));
        }
        if f.sha256.len() != 64 || !f.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!("The map manifest has a bad hash for {}", f.name));
        }
        if f.bytes == 0 {
            return Err(format!("The map manifest has no size for {}", f.name));
        }
    }
    Ok(m)
}

/// The file `name` in the same folder as the manifest URL.
fn sibling_url(manifest_url: &str, name: &str) -> String {
    let base = manifest_url.split(['?', '#']).next().unwrap_or(manifest_url);
    match base.rfind('/') {
        Some(i) => format!("{}{}", &base[..=i], name),
        None => name.to_string(),
    }
}

// ---------- state ----------

/// Progress of one download, shared with the thread that runs it.
pub struct Progress {
    /// 0 = manifest, 1 = downloading, 2 = verifying.
    phase: AtomicU8,
    received: AtomicU64,
    total: AtomicU64,
    cancel: AtomicBool,
}

impl Progress {
    fn new() -> Self {
        Progress { phase: AtomicU8::new(0), received: AtomicU64::new(0), total: AtomicU64::new(0), cancel: AtomicBool::new(false) }
    }
    fn phase_name(&self) -> &'static str {
        match self.phase.load(Ordering::Relaxed) {
            0 => "manifest",
            1 => "downloading",
            _ => "verifying",
        }
    }
}

struct State {
    data_dir: PathBuf,
    manifest_url: String,
    manifest: Option<Manifest>,
    checked_at: u64,
    error: String,
    job: Option<(String, Arc<Progress>)>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    let mut g = STATE.lock().unwrap_or_else(|e| e.into_inner());
    g.as_mut().map(f)
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Called once at startup. `manifest_url` is the `--map-url` setting; none = the project's release.
pub fn configure(data_dir: &Path, manifest_url: Option<&str>) {
    let mut g = STATE.lock().unwrap_or_else(|e| e.into_inner());
    *g = Some(State {
        data_dir: data_dir.to_path_buf(),
        manifest_url: manifest_url.filter(|u| !u.is_empty()).unwrap_or(DEFAULT_MANIFEST_URL).to_string(),
        manifest: None,
        checked_at: 0,
        error: String::new(),
        job: None,
    });
}

// ---------- installed map ----------

#[derive(Clone, Debug, Deserialize, serde::Serialize, PartialEq)]
pub struct Installed {
    pub level: String,
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
    pub build: String,
    pub minzoom: u8,
    pub maxzoom: u8,
    pub bbox: Vec<f64>,
    pub installed_at: u64,
}

/// The installed map, if the file is there and has the size that was recorded.
pub fn installed(data_dir: &Path) -> Option<Installed> {
    let d = dir(data_dir);
    let rec: Installed = serde_json::from_slice(&std::fs::read(d.join(INSTALLED)).ok()?).ok()?;
    let len = std::fs::metadata(d.join(ACTIVE)).ok()?.len();
    (len == rec.bytes).then_some(rec)
}

/// The file `web.rs` serves as `/map/denmark.pmtiles`.
pub fn active_file(data_dir: &Path) -> PathBuf {
    dir(data_dir).join(ACTIVE)
}

fn part_path(data_dir: &Path, f: &FileEntry) -> PathBuf {
    dir(data_dir).join(format!("{}.part", f.name))
}

pub fn delete(data_dir: &Path) -> Result<(), String> {
    let d = dir(data_dir);
    let mut first_error = None;
    if let Ok(rd) = std::fs::read_dir(&d) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n == ACTIVE || n == INSTALLED || n.ends_with(".pmtiles.part") {
                if let Err(err) = std::fs::remove_file(e.path()) {
                    first_error.get_or_insert(format!("Could not delete {n}: {err}"));
                }
            }
        }
    }
    first_error.map_or(Ok(()), Err)
}

// ---------- download ----------

fn curl_program() -> &'static str {
    if cfg!(windows) { "curl.exe" } else { "curl" }
}

fn curl_command() -> std::process::Command {
    #[cfg_attr(not(windows), allow(unused_mut))] // Only changed on Windows.
    let mut c = std::process::Command::new(curl_program());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}

/// Is there a `curl` to download with? (Not on Android: the app downloads and the host only installs.)
fn curl_available() -> bool {
    static CACHE: Mutex<Option<bool>> = Mutex::new(None);
    let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    *g.get_or_insert_with(|| {
        curl_command()
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    })
}

fn curl_bytes(url: &str) -> Result<Vec<u8>, String> {
    let out = curl_command()
        .args(["-sSL", "--fail", "--max-time", "30", "--max-filesize", "262144", "-H", "User-Agent: maarse-host", url])
        .output()
        .map_err(|e| format!("{} could not be started: {e}", curl_program()))?;
    if out.status.success() { Ok(out.stdout) } else { Err(String::from_utf8_lossy(&out.stderr).trim().to_string()) }
}

pub fn fetch_manifest(url: &str) -> Result<Manifest, String> {
    parse_manifest(&curl_bytes(url)?)
}

/// Downloads `url` into `part` (resuming what is there), reporting progress, until done, failed or cancelled.
fn curl_download(url: &str, part: &Path, progress: &Progress) -> Result<(), String> {
    let mut child = curl_command()
        .args(["-sSL", "--fail", "-C", "-", "--connect-timeout", "20", "--speed-limit", "1000", "--speed-time", "60", "-H", "User-Agent: maarse-host", "-o"])
        .arg(part)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("{} could not be started: {e}", curl_program()))?;
    loop {
        if progress.cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("cancelled".into());
        }
        if let Ok(m) = std::fs::metadata(part) {
            progress.received.store(m.len(), Ordering::Relaxed);
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                if let Ok(m) = std::fs::metadata(part) {
                    progress.received.store(m.len(), Ordering::Relaxed);
                }
                if status.success() {
                    return Ok(());
                }
                let mut err = String::new();
                if let Some(mut e) = child.stderr.take() {
                    let _ = e.read_to_string(&mut err);
                }
                return Err(format!("Download failed (it continues where it stopped when you try again): {}", err.trim()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(200)),
            Err(e) => return Err(e.to_string()),
        }
    }
}

fn sha256_file(path: &Path, progress: &Progress) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut done = 0u64;
    loop {
        if progress.cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        done += n as u64;
        progress.received.store(done, Ordering::Relaxed);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Downloads (or resumes), verifies and installs one level. Runs in its own thread in normal use.
pub fn run_download(data_dir: &Path, manifest: &Manifest, manifest_url: &str, level: &str, progress: &Progress) -> Result<Installed, String> {
    let f = manifest.files.iter().find(|f| f.level == level).ok_or_else(|| format!("The map manifest has no {level} level"))?;
    let d = dir(data_dir);
    std::fs::create_dir_all(&d).map_err(|e| format!("Could not create {}: {e}", d.display()))?;
    let part = part_path(data_dir, f);
    progress.total.store(f.bytes, Ordering::Relaxed);
    progress.phase.store(1, Ordering::Relaxed);
    // A part that is too large belongs to something else: start again.
    if std::fs::metadata(&part).is_ok_and(|m| m.len() > f.bytes) {
        let _ = std::fs::remove_file(&part);
    }
    let have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    if have < f.bytes {
        if !curl_available() {
            return Err("This device has no curl: download the file to the .part path and start again".into());
        }
        curl_download(&sibling_url(manifest_url, &f.name), &part, progress)?;
    }
    progress.phase.store(2, Ordering::Relaxed);
    progress.received.store(0, Ordering::Relaxed);
    let len = std::fs::metadata(&part).map(|m| m.len()).map_err(|e| e.to_string())?;
    if len != f.bytes {
        let _ = std::fs::remove_file(&part);
        return Err(format!("The downloaded map has the wrong size ({len} bytes, expected {}); it was deleted", f.bytes));
    }
    let hash = sha256_file(&part, progress)?;
    if !hash.eq_ignore_ascii_case(&f.sha256) {
        let _ = std::fs::remove_file(&part);
        return Err("The downloaded map does not match its checksum; it was deleted".into());
    }
    let rec = Installed {
        level: f.level.clone(),
        name: f.name.clone(),
        bytes: f.bytes,
        sha256: f.sha256.to_ascii_lowercase(),
        build: manifest.build.clone(),
        minzoom: f.minzoom,
        maxzoom: f.maxzoom,
        bbox: f.bbox.clone(),
        installed_at: now(),
    };
    // The old record goes first, so a crash between the two steps leaves "not installed", never a wrong record.
    let _ = std::fs::remove_file(d.join(INSTALLED));
    std::fs::rename(&part, d.join(ACTIVE)).map_err(|e| format!("Could not install the map: {e}"))?;
    let body = serde_json::to_vec_pretty(&rec).map_err(|e| e.to_string())?;
    let tmp = d.join("installed.json.tmp");
    std::fs::write(&tmp, body).and_then(|_| std::fs::rename(&tmp, d.join(INSTALLED))).map_err(|e| e.to_string())?;
    Ok(rec)
}

fn start_download(level: &str) -> Result<(), (&'static str, String)> {
    let level = level.to_string();
    if !matches!(level.as_str(), "overview" | "detailed") {
        return Err(("400 Bad Request", "level must be overview or detailed".into()));
    }
    let progress = Arc::new(Progress::new());
    let (data_dir, url, cached) = with(|s| {
        if s.job.is_some() {
            return None;
        }
        s.job = Some((level.clone(), progress.clone()));
        s.error.clear();
        Some((s.data_dir.clone(), s.manifest_url.clone(), s.manifest.clone()))
    })
    .ok_or(("500 Internal Server Error", "The map is not set up".to_string()))?
    .ok_or(("409 Conflict", "A map download is already running".to_string()))?;
    std::thread::Builder::new()
        .name("map-download".into())
        .spawn(move || {
            let result = (|| {
                let manifest = match cached {
                    Some(m) => m,
                    None => {
                        let m = fetch_manifest(&url)?;
                        with(|s| {
                            s.manifest = Some(m.clone());
                            s.checked_at = now();
                        });
                        m
                    }
                };
                run_download(&data_dir, &manifest, &url, &level, &progress)
            })();
            with(|s| {
                s.job = None;
                s.error = match &result {
                    Ok(_) => String::new(),
                    Err(e) if e == "cancelled" => String::new(),
                    Err(e) => e.clone(),
                };
            });
        })
        .map(|_| ())
        .map_err(|e| {
            with(|s| s.job = None);
            ("500 Internal Server Error", e.to_string())
        })
}

// ---------- status and API ----------

pub fn status_json() -> String {
    with(|s| {
        let inst = installed(&s.data_dir);
        let available: Vec<_> = s
            .manifest
            .iter()
            .flat_map(|m| m.files.iter())
            .map(|f| {
                json!({
                    "level": f.level, "name": f.name, "bytes": f.bytes, "minzoom": f.minzoom, "maxzoom": f.maxzoom,
                    "bbox": f.bbox, "part_path": part_path(&s.data_dir, f).to_string_lossy(),
                    "part_bytes": std::fs::metadata(part_path(&s.data_dir, f)).map(|m| m.len()).unwrap_or(0),
                })
            })
            .collect();
        let download = s.job.as_ref().map(|(level, p)| {
            json!({"level": level, "phase": p.phase_name(), "received": p.received.load(Ordering::Relaxed), "total": p.total.load(Ordering::Relaxed)})
        });
        json!({
            "enabled": true,
            "attribution": ATTRIBUTION,
            "manifest_url": s.manifest_url,
            "can_download": curl_available(),
            "tiles_url": inst.as_ref().map(|_| "/map/denmark.pmtiles"),
            "installed": inst,
            "available": available,
            "build": s.manifest.as_ref().map(|m| m.build.as_str()),
            "osm_data": s.manifest.as_ref().map(|m| m.osm_data.as_str()),
            "checked_at": s.checked_at,
            "download": download,
            "error": s.error,
            "dir": dir(&s.data_dir).to_string_lossy(),
        })
        .to_string()
    })
    .unwrap_or_else(|| json!({"enabled": false}).to_string())
}

fn query_value<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))
}

/// The `/api/map/*` routes. The caller has already checked the access key. `None` = not a map route.
pub fn api(method: &str, path: &str, query: &str, body: &[u8]) -> Option<(&'static str, String)> {
    let err = |status: &'static str, msg: String| Some((status, json!({"error": msg}).to_string()));
    match (method, path) {
        ("GET", "/api/map/status") => Some(("200 OK", status_json())),
        ("POST", "/api/map/check") => {
            let url = with(|s| s.manifest_url.clone())?;
            match fetch_manifest(&url) {
                Ok(m) => {
                    with(|s| {
                        s.manifest = Some(m);
                        s.checked_at = now();
                        s.error.clear();
                    });
                    Some(("200 OK", status_json()))
                }
                Err(e) => {
                    with(|s| s.error = e.clone());
                    err("502 Bad Gateway", e)
                }
            }
        }
        // For devices without curl: the app fetched the manifest itself.
        ("POST", "/api/map/manifest") => match parse_manifest(body) {
            Ok(m) => {
                with(|s| {
                    s.manifest = Some(m);
                    s.checked_at = now();
                    s.error.clear();
                });
                Some(("200 OK", status_json()))
            }
            Err(e) => err("400 Bad Request", e),
        },
        ("POST", "/api/map/download") => match start_download(query_value(query, "level").unwrap_or("overview")) {
            Ok(()) => Some(("200 OK", status_json())),
            Err((status, msg)) => err(status, msg),
        },
        ("POST", "/api/map/cancel") => {
            with(|s| {
                if let Some((_, p)) = &s.job {
                    p.cancel.store(true, Ordering::Relaxed);
                }
            });
            Some(("200 OK", status_json()))
        }
        ("POST", "/api/map/delete") => {
            let busy = with(|s| s.job.is_some()).unwrap_or(false);
            if busy {
                return err("409 Conflict", "A map download is running; cancel it first".into());
            }
            let dd = with(|s| s.data_dir.clone())?;
            match delete(&dd) {
                Ok(()) => Some(("200 OK", status_json())),
                Err(e) => err("500 Internal Server Error", e),
            }
        }
        (_, p) if p.starts_with("/api/map/") => err("404 Not Found", "Not found".into()),
        _ => None,
    }
}

// ---------- serving /map/* ----------

const MAPLIBRE_JS: &[u8] = include_bytes!("../../maarse-client/map/maplibre-gl.js");
const MAPLIBRE_CSS: &[u8] = include_bytes!("../../maarse-client/map/maplibre-gl.css");
const PMTILES_JS: &[u8] = include_bytes!("../../maarse-client/map/pmtiles.js");
const STYLE_LIGHT: &str = include_str!("../../maarse-client/map/style-light.json");
const STYLE_DARK: &str = include_str!("../../maarse-client/map/style-dark.json");

macro_rules! font {
    ($name:literal, $range:literal) => {
        ($name, $range, include_bytes!(concat!("../../maarse-client/map/fonts/", $name, "/", $range, ".pbf")) as &[u8])
    };
}
macro_rules! sprite {
    ($file:literal) => {
        ($file, include_bytes!(concat!("../../maarse-client/map/sprite/", $file)) as &[u8])
    };
}

/// Latin and general punctuation glyph ranges: enough for Danish, Swedish, Norwegian and German names.
const FONTS: &[(&str, &str, &[u8])] = &[
    font!("Noto Sans Regular", "0-255"),
    font!("Noto Sans Regular", "256-511"),
    font!("Noto Sans Regular", "8192-8447"),
    font!("Noto Sans Medium", "0-255"),
    font!("Noto Sans Medium", "256-511"),
    font!("Noto Sans Medium", "8192-8447"),
    font!("Noto Sans Italic", "0-255"),
    font!("Noto Sans Italic", "256-511"),
    font!("Noto Sans Italic", "8192-8447"),
];

const SPRITES: &[(&str, &[u8])] = &[
    sprite!("light.json"),
    sprite!("light.png"),
    sprite!("light@2x.json"),
    sprite!("light@2x.png"),
    sprite!("dark.json"),
    sprite!("dark.png"),
    sprite!("dark@2x.json"),
    sprite!("dark@2x.png"),
];

/// Is this a request for the map (`/map/...`)?
pub fn is_map_path(path: &str) -> bool {
    path.starts_with("/map/")
}

fn ctype_of(name: &str) -> &'static str {
    if name.ends_with(".png") {
        "image/png"
    } else if name.ends_with(".json") {
        "application/json"
    } else {
        "application/octet-stream"
    }
}

/// Only plain host names, addresses and ports go into the style (it is built from the Host header).
fn origin_from(host: Option<&str>) -> String {
    let h = host.map(str::trim).filter(|h| !h.is_empty() && h.len() <= 100 && h.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':' | b'[' | b']')));
    format!("http://{}", h.unwrap_or("127.0.0.1:8080"))
}

fn head_line(status: &str, ctype: &str, len: u64, extra: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {len}\r\nAccept-Ranges: bytes\r\nX-Content-Type-Options: nosniff\r\nCross-Origin-Resource-Policy: same-origin\r\n{extra}Connection: close\r\n\r\n"
    )
}

fn text(stream: &mut TcpStream, status: &str, msg: &str) {
    let _ = stream.write_all(head_line(status, "text/plain; charset=utf-8", msg.len() as u64, "Cache-Control: no-store\r\n").as_bytes());
    let _ = stream.write_all(msg.as_bytes());
}

enum Range {
    Whole,
    Part(u64, u64),
    Unsatisfiable,
}

/// A single `bytes=` range. Several ranges are refused (nothing here needs them); a range that is not
/// understood is ignored, as the standard says.
fn parse_range(value: Option<&str>, len: u64) -> Range {
    let Some(spec) = value.and_then(|v| v.trim().strip_prefix("bytes=")) else { return Range::Whole };
    if spec.contains(',') {
        return Range::Unsatisfiable;
    }
    let Some((a, b)) = spec.trim().split_once('-') else { return Range::Whole };
    let (a, b) = (a.trim(), b.trim());
    if len == 0 {
        return Range::Unsatisfiable;
    }
    if a.is_empty() {
        // The last n bytes.
        return match b.parse::<u64>() {
            Ok(n) if n > 0 => Range::Part(len.saturating_sub(n), len - 1),
            Ok(_) => Range::Unsatisfiable,
            Err(_) => Range::Whole,
        };
    }
    let Ok(start) = a.parse::<u64>() else { return Range::Whole };
    let end = if b.is_empty() {
        len - 1
    } else {
        match b.parse::<u64>() {
            Ok(e) => e.min(len - 1),
            Err(_) => return Range::Whole,
        }
    };
    if start >= len || start > end { Range::Unsatisfiable } else { Range::Part(start, end) }
}

/// The ETag, and whether `If-None-Match` already has it.
fn not_modified(header_value: Option<&str>, etag: &str) -> bool {
    header_value.is_some_and(|v| v.split(',').any(|t| t.trim() == "*" || t.trim().trim_start_matches("W/") == etag))
}

/// Serves a fixed in-memory file with a cache validator.
fn serve_bytes(stream: &mut TcpStream, method: &str, if_none_match: Option<&str>, ctype: &str, body: &[u8]) {
    let etag = format!("\"{}-{}\"", env!("CARGO_PKG_VERSION"), body.len());
    let extra = format!("ETag: {etag}\r\nCache-Control: public, max-age=3600\r\n");
    if not_modified(if_none_match, &etag) {
        let _ = stream.write_all(head_line("304 Not Modified", ctype, 0, &extra).replace("Content-Length: 0\r\n", "").as_bytes());
        return;
    }
    let _ = stream.write_all(head_line("200 OK", ctype, body.len() as u64, &extra).as_bytes());
    if method != "HEAD" {
        let _ = stream.write_all(body);
    }
}

/// Streams a range of the installed map from disk.
fn serve_tiles(stream: &mut TcpStream, method: &str, head_value: &dyn Fn(&str) -> Option<String>, data_dir: &Path) {
    let path = active_file(data_dir);
    let Ok(mut file) = std::fs::File::open(&path) else {
        return text(stream, "404 Not Found", "No map is installed");
    };
    let Ok(meta) = file.metadata() else { return text(stream, "404 Not Found", "No map is installed") };
    let len = meta.len();
    let mtime = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    let etag = format!("\"{len:x}-{mtime:x}\"");
    // The archive changes when a new map is installed, so a browser asks again before reuse.
    let common = format!("ETag: {etag}\r\nCache-Control: no-cache\r\n");
    let ctype = "application/octet-stream";
    if not_modified(head_value("if-none-match").as_deref(), &etag) {
        let _ = stream.write_all(head_line("304 Not Modified", ctype, 0, &common).replace("Content-Length: 0\r\n", "").as_bytes());
        return;
    }
    // A long download of a large range must not die on the 5 s default.
    let _ = stream.set_write_timeout(Some(Duration::from_secs(60)));
    let (status, start, end, extra) = match parse_range(head_value("range").as_deref(), len) {
        Range::Whole => ("200 OK", 0, len.saturating_sub(1), common),
        Range::Part(a, b) => ("206 Partial Content", a, b, format!("{common}Content-Range: bytes {a}-{b}/{len}\r\n")),
        Range::Unsatisfiable => {
            let _ = stream.write_all(head_line("416 Range Not Satisfiable", "text/plain; charset=utf-8", 0, &format!("Content-Range: bytes */{len}\r\n")).as_bytes());
            return;
        }
    };
    let count = if len == 0 { 0 } else { end - start + 1 };
    if stream.write_all(head_line(status, ctype, count, &extra).as_bytes()).is_err() || method == "HEAD" || count == 0 {
        return;
    }
    if file.seek(SeekFrom::Start(start)).is_err() {
        return;
    }
    let mut left = count;
    let mut buf = vec![0u8; 64 * 1024];
    while left > 0 {
        let want = left.min(buf.len() as u64) as usize;
        match file.read(&mut buf[..want]) {
            Ok(0) | Err(_) => return,
            Ok(n) => {
                if stream.write_all(&buf[..n]).is_err() {
                    return;
                }
                left -= n as u64;
            }
        }
    }
}

fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Answers a GET or HEAD for `/map/...`. Public read-only data: it needs no access key (see docs/map.md).
/// Only the fixed names below are served; no part of the request is ever used as a file name.
pub fn serve(stream: &mut TcpStream, method: &str, path: &str, query: &str, head: &str, data_dir: &Path) {
    if method != "GET" && method != "HEAD" {
        return text(stream, "405 Method Not Allowed", "Method not allowed");
    }
    let hv = |name: &str| -> Option<String> {
        head.split("\r\n").skip(1).find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim().eq_ignore_ascii_case(name).then(|| v.trim().to_string())
        })
    };
    let inm = hv("if-none-match");
    let rest = &path["/map/".len()..];
    match rest {
        "denmark.pmtiles" => serve_tiles(stream, method, &hv, data_dir),
        "maplibre-gl.js" => serve_bytes(stream, method, inm.as_deref(), "text/javascript; charset=utf-8", MAPLIBRE_JS),
        "maplibre-gl.css" => serve_bytes(stream, method, inm.as_deref(), "text/css; charset=utf-8", MAPLIBRE_CSS),
        "pmtiles.js" => serve_bytes(stream, method, inm.as_deref(), "text/javascript; charset=utf-8", PMTILES_JS),
        "style.json" => {
            let template = if query_value(query, "flavor") == Some("dark") { STYLE_DARK } else { STYLE_LIGHT };
            let body = template.replace("{{ORIGIN}}", &origin_from(hv("host").as_deref()));
            let _ = stream.write_all(head_line("200 OK", "application/json", body.len() as u64, "Cache-Control: no-cache\r\n").as_bytes());
            if method != "HEAD" {
                let _ = stream.write_all(body.as_bytes());
            }
        }
        _ => {
            if let Some(file) = rest.strip_prefix("sprite/") {
                if let Some((name, bytes)) = SPRITES.iter().find(|(n, _)| *n == file) {
                    return serve_bytes(stream, method, inm.as_deref(), ctype_of(name), bytes);
                }
            } else if let Some(r) = rest.strip_prefix("fonts/") {
                if let Some((stack, file)) = r.split_once('/') {
                    let (stack, file) = (url_decode(stack), url_decode(file));
                    let range = file.strip_suffix(".pbf").unwrap_or("");
                    // A stack may list several fonts ("A,B"): the first one that is here.
                    let hit = stack.split(',').find_map(|name| FONTS.iter().find(|(n, r, _)| *n == name.trim() && *r == range));
                    if let Some((_, _, bytes)) = hit {
                        return serve_bytes(stream, method, inm.as_deref(), "application/x-protobuf", bytes);
                    }
                }
            }
            text(stream, "404 Not Found", "Not found")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::atomic::AtomicUsize;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("maarse-map-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn sample(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i.wrapping_mul(31) % 251) as u8).collect()
    }

    fn sha(bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Sends `raw` to `serve` over a real socket and returns the whole answer.
    fn request(data_dir: &Path, raw: &str) -> Vec<u8> {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let raw = raw.to_string();
        let client = std::thread::spawn(move || {
            let mut s = TcpStream::connect(address).unwrap();
            s.write_all(raw.as_bytes()).unwrap();
            let mut out = Vec::new();
            s.read_to_end(&mut out).unwrap();
            out
        });
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = vec![0u8; 4096];
        let n = stream.read(&mut buf).unwrap();
        let head = String::from_utf8_lossy(&buf[..n]).into_owned();
        let mut words = head.split_whitespace();
        let method = words.next().unwrap().to_string();
        let target = words.next().unwrap().to_string();
        let (path, query) = target.split_once('?').unwrap_or((&target, ""));
        serve(&mut stream, &method, path, query, &head, data_dir);
        drop(stream);
        client.join().unwrap()
    }

    fn split(answer: &[u8]) -> (String, Vec<u8>) {
        let i = answer.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        (String::from_utf8_lossy(&answer[..i]).into_owned(), answer[i + 4..].to_vec())
    }

    fn installed_dir(name: &str, bytes: &[u8]) -> PathBuf {
        let d = temp(name);
        std::fs::create_dir_all(dir(&d)).unwrap();
        std::fs::write(active_file(&d), bytes).unwrap();
        d
    }

    #[test]
    fn range_requests_are_answered_like_a_file_server() {
        let data = sample(1000);
        let d = installed_dir("range", &data);
        let get = |extra: &str| request(&d, &format!("GET /map/denmark.pmtiles HTTP/1.1\r\nHost: x\r\n{extra}\r\n"));

        let (head, body) = split(&get(""));
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        assert!(head.contains("Accept-Ranges: bytes") && head.contains("Content-Length: 1000") && head.contains("ETag:"), "{head}");
        assert!(head.contains("Cache-Control: no-cache"), "{head}");
        assert_eq!(body, data);

        let (head, body) = split(&get("Range: bytes=10-19\r\n"));
        assert!(head.starts_with("HTTP/1.1 206"), "{head}");
        assert!(head.contains("Content-Range: bytes 10-19/1000") && head.contains("Content-Length: 10"), "{head}");
        assert_eq!(body, &data[10..20]);

        let (head, body) = split(&get("Range: bytes=990-\r\n"));
        assert!(head.contains("Content-Range: bytes 990-999/1000"), "{head}");
        assert_eq!(body, &data[990..]);

        let (head, body) = split(&get("Range: bytes=-5\r\n"));
        assert!(head.contains("Content-Range: bytes 995-999/1000"), "{head}");
        assert_eq!(body, &data[995..]);

        // An end past the file is cut to the file.
        let (head, _) = split(&get("Range: bytes=900-99999\r\n"));
        assert!(head.contains("Content-Range: bytes 900-999/1000"), "{head}");

        for bad in ["bytes=1000-", "bytes=5000-6000", "bytes=20-10", "bytes=0-1,5-6"] {
            let (head, body) = split(&get(&format!("Range: {bad}\r\n")));
            assert!(head.starts_with("HTTP/1.1 416"), "{bad}: {head}");
            assert!(head.contains("Content-Range: bytes */1000"), "{head}");
            assert!(body.is_empty());
        }

        // HEAD: the same headers, no body.
        let (head, body) = split(&request(&d, "HEAD /map/denmark.pmtiles HTTP/1.1\r\nHost: x\r\n\r\n"));
        assert!(head.starts_with("HTTP/1.1 200") && head.contains("Content-Length: 1000"), "{head}");
        assert!(body.is_empty());

        // The ETag lets a client keep its copy.
        let etag = head.lines().find_map(|l| l.strip_prefix("ETag: ")).unwrap().to_string();
        let (head, body) = split(&get(&format!("If-None-Match: {etag}\r\n")));
        assert!(head.starts_with("HTTP/1.1 304"), "{head}");
        assert!(body.is_empty());

        // Read-only.
        let (head, _) = split(&request(&d, "POST /map/denmark.pmtiles HTTP/1.1\r\nHost: x\r\n\r\n"));
        assert!(head.starts_with("HTTP/1.1 405"), "{head}");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn missing_map_and_unknown_or_traversing_paths_are_404() {
        let d = temp("missing");
        std::fs::write(d.join("settings.txt"), "secret").unwrap();
        let (head, _) = split(&request(&d, "GET /map/denmark.pmtiles HTTP/1.1\r\nHost: x\r\n\r\n"));
        assert!(head.starts_with("HTTP/1.1 404"), "{head}");
        let d2 = installed_dir("traversal", b"tiles");
        std::fs::write(d2.join("settings.txt"), "secret").unwrap();
        for p in [
            "/map/../settings.txt", "/map/%2e%2e/settings.txt", "/map/..%2fsettings.txt", "/map/fonts/../../settings.txt",
            "/map/fonts/Noto%20Sans%20Regular/../../../settings.txt", "/map/fonts/..%2f..%2fsettings.txt/0-255.pbf",
            "/map/sprite/../../settings.txt", "/map/denmark.pmtiles/../settings.txt", "/map/installed.json", "/map/denmark.pmtiles.part",
            "/map/fonts/Nope/0-255.pbf", "/map/fonts/Noto%20Sans%20Regular/9999-10000.pbf", "/map/sprite/nope.png", "/map/",
        ] {
            let (head, body) = split(&request(&d2, &format!("GET {p} HTTP/1.1\r\nHost: x\r\n\r\n")));
            assert!(head.starts_with("HTTP/1.1 404"), "{p}: {head}");
            assert!(!String::from_utf8_lossy(&body).contains("secret"), "{p}");
        }
        std::fs::remove_dir_all(&d).ok();
        std::fs::remove_dir_all(&d2).ok();
    }

    #[test]
    fn style_fonts_sprites_and_libraries_are_served() {
        let d = temp("static");
        let (head, body) = split(&request(&d, "GET /map/style.json HTTP/1.1\r\nHost: 192.168.1.5:8080\r\n\r\n"));
        assert!(head.starts_with("HTTP/1.1 200") && head.contains("application/json"), "{head}");
        let style: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(style["sprite"], "http://192.168.1.5:8080/map/sprite/light");
        assert_eq!(style["glyphs"], "http://192.168.1.5:8080/map/fonts/{fontstack}/{range}.pbf");
        assert_eq!(style["sources"]["protomaps"]["url"], "pmtiles://http://192.168.1.5:8080/map/denmark.pmtiles");
        let (_, body) = split(&request(&d, "GET /map/style.json?flavor=dark HTTP/1.1\r\nHost: localhost:8080\r\n\r\n"));
        let dark: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(dark["sprite"], "http://localhost:8080/map/sprite/dark");
        // A Host header that is not a plain authority never reaches the style.
        let (_, body) = split(&request(&d, "GET /map/style.json HTTP/1.1\r\nHost: a\"b<script>\r\n\r\n"));
        assert!(String::from_utf8_lossy(&body).contains("http://127.0.0.1:8080/map/"));
        for (p, ct) in [
            ("/map/maplibre-gl.js", "text/javascript"), ("/map/maplibre-gl.css", "text/css"), ("/map/pmtiles.js", "text/javascript"),
            ("/map/fonts/Noto%20Sans%20Regular/0-255.pbf", "application/x-protobuf"), ("/map/fonts/Noto%20Sans%20Medium/256-511.pbf", "application/x-protobuf"),
            ("/map/fonts/Noto%20Sans%20Regular,Noto%20Sans%20Devanagari%20Regular%20v1/0-255.pbf", "application/x-protobuf"),
            ("/map/sprite/light.json", "application/json"), ("/map/sprite/dark@2x.png", "image/png"),
        ] {
            let (head, body) = split(&request(&d, &format!("GET {p} HTTP/1.1\r\nHost: x\r\n\r\n")));
            assert!(head.starts_with("HTTP/1.1 200") && head.contains(ct), "{p}: {head}");
            assert!(!body.is_empty(), "{p}");
        }
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn range_parser_edge_cases() {
        assert!(matches!(parse_range(None, 10), Range::Whole));
        assert!(matches!(parse_range(Some("items=1-2"), 10), Range::Whole));
        assert!(matches!(parse_range(Some("bytes=abc-"), 10), Range::Whole));
        assert!(matches!(parse_range(Some("bytes=0-0"), 10), Range::Part(0, 0)));
        assert!(matches!(parse_range(Some("bytes=-100"), 10), Range::Part(0, 9)));
        assert!(matches!(parse_range(Some("bytes=-0"), 10), Range::Unsatisfiable));
        assert!(matches!(parse_range(Some("bytes=0-1"), 0), Range::Unsatisfiable));
    }

    #[test]
    fn manifest_must_be_sane() {
        let ok = |name: &str, sha: &str, level: &str| {
            parse_manifest(format!(r#"{{"build":"1","files":[{{"level":"{level}","name":"{name}","bytes":5,"sha256":"{sha}","maxzoom":10}}]}}"#).as_bytes())
        };
        let h = "a".repeat(64);
        assert!(ok("denmark-overview.pmtiles", &h, "overview").is_ok());
        assert!(ok("../x.pmtiles", &h, "overview").is_err());
        assert!(ok("a/b.pmtiles", &h, "overview").is_err());
        assert!(ok("a.exe", &h, "overview").is_err());
        assert!(ok("a.pmtiles", "abc", "overview").is_err());
        assert!(ok("a.pmtiles", &h, "huge").is_err());
        assert!(parse_manifest(b"[]").is_err());
        assert_eq!(sibling_url("https://h/x/y/map-manifest.json?a=b", "f.pmtiles"), "https://h/x/y/f.pmtiles");
    }

    /// A small file server for the download tests: serves the manifest and the file, honours
    /// `Range: bytes=N-`, and records which ranges were asked for.
    struct Server {
        base: String,
        ranges: Arc<Mutex<Vec<String>>>,
        hits: Arc<AtomicUsize>,
    }

    fn serve_files(file: Vec<u8>, manifest: String) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        let ranges = Arc::new(Mutex::new(Vec::new()));
        let hits = Arc::new(AtomicUsize::new(0));
        let (r, h) = (ranges.clone(), hits.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (file, manifest, r, h) = (file.clone(), manifest.clone(), r.clone(), h.clone());
                std::thread::spawn(move || {
                    let mut stream = stream;
                    let mut buf = vec![0u8; 4096];
                    let Ok(n) = stream.read(&mut buf) else { return };
                    let head = String::from_utf8_lossy(&buf[..n]).into_owned();
                    let path = head.split_whitespace().nth(1).unwrap_or("").to_string();
                    h.fetch_add(1, Ordering::Relaxed);
                    let (body, ranged): (Vec<u8>, Option<u64>) = if path.ends_with("map-manifest.json") {
                        (manifest.into_bytes(), None)
                    } else if path.ends_with(".pmtiles") {
                        let start = head.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("range: bytes=").map(|v| v.trim_end_matches('-').to_string()));
                        if let Some(s) = &start {
                            r.lock().unwrap().push(s.clone());
                        }
                        let start = start.and_then(|s| s.parse::<u64>().ok());
                        (file[start.unwrap_or(0) as usize..].to_vec(), start)
                    } else {
                        let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                        return;
                    };
                    let head = match ranged {
                        Some(s) => format!("HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {s}-{}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", file.len() - 1, file.len(), body.len()),
                        None => format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()),
                    };
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(&body);
                });
            }
        });
        Server { base, ranges, hits }
    }

    fn manifest_for(file: &[u8], sha256: &str) -> String {
        format!(
            r#"{{"schema":1,"build":"20261123","osm_data":"2026-11-23","attribution":"x","files":[{{"level":"overview","name":"denmark-overview.pmtiles","bytes":{},"sha256":"{sha256}","bbox":[7.9,54.5,15.3,57.8],"minzoom":0,"maxzoom":10}}]}}"#,
            file.len()
        )
    }

    #[test]
    fn download_verifies_installs_and_deletes() {
        if !curl_available() {
            eprintln!("no curl here: skipped");
            return;
        }
        let file = sample(300_000);
        let server = serve_files(file.clone(), manifest_for(&file, &sha(&file)));
        let url = format!("{}map-manifest.json", server.base);
        let d = temp("download");
        let manifest = fetch_manifest(&url).unwrap();
        assert_eq!(manifest.build, "20261123");
        let progress = Progress::new();
        let rec = run_download(&d, &manifest, &url, "overview", &progress).unwrap();
        assert_eq!(rec.bytes, 300_000);
        assert_eq!(std::fs::read(active_file(&d)).unwrap(), file);
        assert!(!dir(&d).join("denmark-overview.pmtiles.part").exists());
        assert_eq!(installed(&d), Some(rec));
        assert!(run_download(&d, &manifest, &url, "detailed", &progress).is_err());
        delete(&d).unwrap();
        assert!(installed(&d).is_none());
        assert!(!active_file(&d).exists());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn download_resumes_a_part_with_a_range_request() {
        if !curl_available() {
            return;
        }
        let file = sample(300_000);
        let server = serve_files(file.clone(), manifest_for(&file, &sha(&file)));
        let url = format!("{}map-manifest.json", server.base);
        let d = temp("resume");
        std::fs::create_dir_all(dir(&d)).unwrap();
        std::fs::write(dir(&d).join("denmark-overview.pmtiles.part"), &file[..123_456]).unwrap();
        let manifest = fetch_manifest(&url).unwrap();
        run_download(&d, &manifest, &url, "overview", &Progress::new()).unwrap();
        assert_eq!(std::fs::read(active_file(&d)).unwrap(), file);
        assert_eq!(server.ranges.lock().unwrap().as_slice(), ["123456"], "only the missing part is asked for");
        // A complete .part (placed by the Android app) is only verified, nothing is fetched.
        let before = server.hits.load(Ordering::Relaxed);
        std::fs::write(dir(&d).join("denmark-overview.pmtiles.part"), &file).unwrap();
        run_download(&d, &manifest, &url, "overview", &Progress::new()).unwrap();
        assert_eq!(server.hits.load(Ordering::Relaxed), before);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn download_rejects_a_wrong_hash_and_keeps_the_old_map() {
        if !curl_available() {
            return;
        }
        let file = sample(50_000);
        let server = serve_files(file.clone(), manifest_for(&file, &"0".repeat(64)));
        let url = format!("{}map-manifest.json", server.base);
        let d = installed_dir("badhash", b"old map");
        let manifest = fetch_manifest(&url).unwrap();
        let e = run_download(&d, &manifest, &url, "overview", &Progress::new()).unwrap_err();
        assert!(e.contains("checksum"), "{e}");
        assert!(!dir(&d).join("denmark-overview.pmtiles.part").exists(), "the bad file is deleted");
        assert_eq!(std::fs::read(active_file(&d)).unwrap(), b"old map");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn status_and_api_json() {
        let d = temp("status");
        let file = sample(10_000);
        configure(&d, Some("http://127.0.0.1:9/map-manifest.json"));
        let s: serde_json::Value = serde_json::from_str(&status_json()).unwrap();
        assert_eq!(s["enabled"], true);
        assert!(s["installed"].is_null() && s["download"].is_null() && s["tiles_url"].is_null());
        assert_eq!(s["attribution"], ATTRIBUTION);
        assert_eq!(s["manifest_url"], "http://127.0.0.1:9/map-manifest.json");
        // The manifest posted by a device that has no curl.
        let (status, body) = api("POST", "/api/map/manifest", "", manifest_for(&file, &sha(&file)).as_bytes()).unwrap();
        assert_eq!(status, "200 OK");
        let s: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(s["available"][0]["level"], "overview");
        assert_eq!(s["available"][0]["bytes"], 10_000);
        assert!(s["available"][0]["part_path"].as_str().unwrap().ends_with("denmark-overview.pmtiles.part"));
        assert_eq!(api("POST", "/api/map/manifest", "", b"{}").unwrap().0, "400 Bad Request");
        assert_eq!(api("POST", "/api/map/download", "level=nope", b"").unwrap().0, "400 Bad Request");
        assert_eq!(api("GET", "/api/map/other", "", b"").unwrap().0, "404 Not Found");
        assert!(api("GET", "/api/other", "", b"").is_none());
        // The app put the whole file at part_path: download installs it with no curl and no server.
        std::fs::create_dir_all(dir(&d)).unwrap();
        std::fs::write(dir(&d).join("denmark-overview.pmtiles.part"), &file).unwrap();
        assert_eq!(api("POST", "/api/map/download", "level=overview", b"").unwrap().0, "200 OK");
        for _ in 0..100 {
            if installed(&d).is_some() && with(|s| s.job.is_none()) == Some(true) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let s: serde_json::Value = serde_json::from_str(&status_json()).unwrap();
        assert_eq!(s["installed"]["level"], "overview", "{s}");
        assert_eq!(s["tiles_url"], "/map/denmark.pmtiles");
        assert_eq!(s["error"], "");
        assert_eq!(api("POST", "/api/map/delete", "", b"").unwrap().0, "200 OK");
        let s: serde_json::Value = serde_json::from_str(&status_json()).unwrap();
        assert!(s["installed"].is_null());
        std::fs::remove_dir_all(&d).ok();
    }
}
