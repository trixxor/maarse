//! Keeps the sensor catalogue up to date from GitHub, without a new app or a new
//! release of the host.
//!
//! GitHub Actions (`.github/workflows/sensors.yml`) merges the files in
//! `sensors/`, signs them with the release key (the same key as the firmware and
//! the Windows updates) and publishes three files on the rolling `sensors`
//! release: `catalog.json`, `SHA256SUMS-sensors` and `SHA256SUMS-sensors.sig`.
//! A host accepts a catalogue only when
//!
//! * the signature of the checksum list matches the release key built into this
//!   program (`MAARSE_RELEASE_PUBKEY`; a build without it never updates),
//! * the file matches its line in the list,
//! * its format is one this host reads, every description fits together and
//!   every test from a data sheet gives its number, and
//! * its version is newer than the one in use (no going back).
//!
//! The accepted files are kept in `<data>/sensor-catalog/` and checked again
//! at every start, so a changed file on disk is ignored. The host looks about
//! once a day (it can be switched off, or asked at once); the Android app, which
//! has no `curl`, downloads the three files itself and posts them to
//! `POST /api/sensors/catalog/update`, which does the same checks.

use super::catalog::{self, Catalog};
use maarse_crypto::ota;
use serde_json::{Value as J, json};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

/// Where the files are published.
pub const BASE_URL: &str = "https://github.com/trixxor/maarse/releases/download/sensors/";
pub const FILE: &str = "catalog.json";
pub const SUMS: &str = "SHA256SUMS-sensors";
pub const SIG: &str = "SHA256SUMS-sensors.sig";
/// Seconds between two looks on GitHub.
const EVERY_S: i64 = 24 * 3600;

#[derive(Default)]
struct State {
    dir: Option<PathBuf>,
    auto: bool,
    checked_at: i64,
    updated_at: i64,
    error: String,
    started: bool,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| Mutex::new(State { auto: true, ..State::default() }));

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The public release key built into this program, if there is a valid one.
pub fn release_key() -> Option<[u8; 32]> {
    ota::parse_hex32(option_env!("MAARSE_RELEASE_PUBKEY")?).filter(ota::valid_release_key)
}

fn parse_sig(hex_text: &str) -> Option<[u8; 64]> {
    let t = hex_text.trim().as_bytes();
    if t.len() != 128 {
        return None;
    }
    let mut out = [0u8; 64];
    for (i, o) in out.iter_mut().enumerate() {
        let h = (t[2 * i] as char).to_digit(16)?;
        let l = (t[2 * i + 1] as char).to_digit(16)?;
        *o = (h << 4 | l) as u8;
    }
    Some(out)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Checks the signature, the checksum and the content of a downloaded catalogue.
pub fn verify(key: &[u8; 32], catalog_bytes: &[u8], sums: &str, sig_hex: &str) -> Result<Catalog, String> {
    let sig = parse_sig(sig_hex).ok_or("The signature file is not valid.")?;
    if !ota::verify_release(key, sums.as_bytes(), &sig) {
        return Err("The signature does not match the Må:rse release key. The sensor list was not changed.".into());
    }
    let hash = hex(&ota::sha256(catalog_bytes));
    let listed = sums.lines().find_map(|l| {
        let (h, name) = l.split_once("  ")?;
        (name.trim() == FILE).then(|| h.trim().to_ascii_lowercase())
    });
    match listed {
        Some(h) if h == hash => {}
        Some(_) => return Err("The catalogue does not match its signed checksum. The sensor list was not changed.".into()),
        None => return Err(format!("{FILE} is not in the signed checksum list.")),
    }
    let text = std::str::from_utf8(catalog_bytes).map_err(|_| "The catalogue is not text.")?;
    Catalog::parse(text)
}

fn now() -> i64 {
    super::unix_now()
}

fn save_state(s: &State) {
    if let Some(dir) = &s.dir {
        let j = json!({"auto": s.auto, "checked_at": s.checked_at, "updated_at": s.updated_at});
        let _ = std::fs::write(dir.join("state.json"), j.to_string());
    }
}

/// Checks and installs a signed catalogue (from GitHub, or posted by the
/// Android app) when it is newer than the one in use. Says what happened.
pub fn install_signed(catalog_bytes: &[u8], sums: &str, sig_hex: &str) -> Result<String, String> {
    let key = release_key().ok_or("This build has no release key, so it cannot check a signed sensor list.")?;
    install_with_key(&key, catalog_bytes, sums, sig_hex)
}

fn install_with_key(key: &[u8; 32], catalog_bytes: &[u8], sums: &str, sig_hex: &str) -> Result<String, String> {
    let cat = verify(key, catalog_bytes, sums, sig_hex)?;
    let have = catalog::current().version;
    if cat.version <= have {
        return Ok(format!("The sensor list is up to date (version {have})."));
    }
    let version = cat.version;
    let mut s = state();
    if let Some(dir) = &s.dir {
        let _ = std::fs::create_dir_all(dir);
        let ok = std::fs::write(dir.join(FILE), catalog_bytes).is_ok() && std::fs::write(dir.join(SUMS), sums).is_ok() && std::fs::write(dir.join(SIG), sig_hex).is_ok();
        if !ok {
            return Err("The new sensor list could not be saved.".into());
        }
    }
    catalog::install(cat);
    s.updated_at = now();
    s.error.clear();
    save_state(&s);
    Ok(format!("The sensor list was updated to version {version}."))
}

/// Opens the folder, reads the saved choice and the saved catalogue (checked
/// again) and starts the daily look.
pub fn open(data_dir: &Path) {
    let dir = data_dir.join("sensor-catalog");
    {
        let mut s = state();
        s.dir = Some(dir.clone());
        if let Ok(t) = std::fs::read_to_string(dir.join("state.json"))
            && let Ok(j) = serde_json::from_str::<J>(&t)
        {
            s.auto = j["auto"].as_bool().unwrap_or(true);
            s.checked_at = j["checked_at"].as_i64().unwrap_or(0);
            s.updated_at = j["updated_at"].as_i64().unwrap_or(0);
        }
    }
    if let Some(key) = release_key()
        && let (Ok(c), Ok(sums), Ok(sig)) = (std::fs::read(dir.join(FILE)), std::fs::read_to_string(dir.join(SUMS)), std::fs::read_to_string(dir.join(SIG)))
    {
        match verify(&key, &c, &sums, &sig) {
            Ok(cat) if cat.version > catalog::current().version => catalog::install(cat),
            Ok(_) => {}
            Err(e) => state().error = format!("The saved sensor list was ignored: {e}"),
        }
    }
    start_daily();
}

fn curl(url: &str) -> Result<Vec<u8>, String> {
    let program = if cfg!(windows) { "curl.exe" } else { "curl" };
    let out = std::process::Command::new(program)
        .args(["-sSL", "--fail", "--max-time", "30", "-H", "User-Agent: maarse-host", url])
        .output()
        .map_err(|e| format!("{program} could not be started: {e}"))?;
    if out.status.success() { Ok(out.stdout) } else { Err(String::from_utf8_lossy(&out.stderr).trim().to_string()) }
}

/// Looks on GitHub now. Where there is no `curl` (Android) the app downloads and posts instead.
pub fn check_now() -> Result<String, String> {
    if release_key().is_none() {
        return Err("This build has no release key, so it cannot check a signed sensor list.".into());
    }
    let result = (|| {
        let c = curl(&format!("{BASE_URL}{FILE}"))?;
        let sums = curl(&format!("{BASE_URL}{SUMS}"))?;
        let sig = curl(&format!("{BASE_URL}{SIG}"))?;
        install_signed(&c, &String::from_utf8_lossy(&sums), &String::from_utf8_lossy(&sig))
    })();
    let mut s = state();
    s.checked_at = now();
    match &result {
        Ok(_) => s.error.clear(),
        Err(e) => s.error = e.clone(),
    }
    save_state(&s);
    result
}

fn start_daily() {
    {
        let mut s = state();
        if s.started {
            return;
        }
        s.started = true;
    }
    std::thread::Builder::new()
        .name("sensor-catalog".into())
        .spawn(|| {
            std::thread::sleep(Duration::from_secs(90));
            loop {
                let (auto, last) = {
                    let s = state();
                    (s.auto, s.checked_at)
                };
                if auto && now() - last >= EVERY_S {
                    let _ = check_now();
                }
                std::thread::sleep(Duration::from_secs(3600));
            }
        })
        .ok();
}

pub fn set_auto(on: bool) {
    let mut s = state();
    s.auto = on;
    save_state(&s);
}

pub fn status_json() -> J {
    let s = state();
    json!({"version": catalog::current().version, "built_in_version": catalog::embedded_version(), "auto": s.auto, "checked_at": s.checked_at,
           "updated_at": s.updated_at, "error": s.error, "can_update": release_key().is_some(), "source": BASE_URL})
}

#[cfg(test)]
mod tests {
    use super::*;
    use maarse_crypto::ota::SigningKey;

    fn signed(key: &SigningKey, body: &str) -> (String, String) {
        let sums = format!("{}  {FILE}\n", hex(&ota::sha256(body.as_bytes())));
        (sums.clone(), hex(&ota::sign_release(key, sums.as_bytes())))
    }

    fn bundle(version: u64) -> String {
        let mut v: J = serde_json::from_str(include_str!("../../../sensors/catalog.json")).unwrap();
        v["version"] = json!(version);
        v.to_string()
    }

    #[test]
    fn only_a_catalogue_signed_with_the_release_key_is_accepted() {
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let public = key.verifying_key().to_bytes();
        let body = bundle(5);
        let (sums, sig) = signed(&key, &body);
        assert_eq!(verify(&public, body.as_bytes(), &sums, &sig).unwrap().version, 5);
        // Another key, a changed file, a changed list, garbage.
        let other = SigningKey::from_bytes(&[8u8; 32]);
        let (sums2, sig2) = signed(&other, &body);
        assert!(verify(&public, body.as_bytes(), &sums2, &sig2).unwrap_err().contains("signature"));
        assert!(verify(&public, bundle(6).as_bytes(), &sums, &sig).unwrap_err().contains("checksum"));
        assert!(verify(&public, body.as_bytes(), &sums.replace(FILE, "other.json"), &sig).is_err());
        assert!(verify(&public, body.as_bytes(), &sums, "zz").unwrap_err().contains("not valid"));
        // Signed, but the content is wrong (a data sheet test fails): refused too.
        let mut v: J = serde_json::from_str(&body).unwrap();
        v["sensors"].as_array_mut().unwrap().iter_mut().find(|s| s["id"] == "sht3x").unwrap()["tests"][0]["value"] = json!(99.0);
        let bad = v.to_string();
        let (s3, g3) = signed(&key, &bad);
        assert!(verify(&public, bad.as_bytes(), &s3, &g3).unwrap_err().contains("data sheet"));
    }

    #[test]
    fn an_older_or_equal_version_does_not_replace_the_one_in_use() {
        let key = SigningKey::from_bytes(&[9u8; 32]);
        let public = key.verifying_key().to_bytes();
        let have = catalog::current().version;
        let old = bundle(have);
        let (sums, sig) = signed(&key, &old);
        assert!(install_with_key(&public, old.as_bytes(), &sums, &sig).unwrap().contains("up to date"));
        let older = bundle(have.saturating_sub(1));
        let (sums, sig) = signed(&key, &older);
        assert!(install_with_key(&public, older.as_bytes(), &sums, &sig).unwrap().contains("up to date"));
        assert_eq!(catalog::current().version, have);
    }

    #[test]
    fn the_status_says_whether_this_build_can_update() {
        let s = status_json();
        assert!(s["version"].as_u64().unwrap() >= s["built_in_version"].as_u64().unwrap());
        assert_eq!(s["can_update"], json!(release_key().is_some()));
    }
}
