//! Update check against the GitHub releases of Må:rse, and the signature check of the download.
//! No Windows code here, so it can be tested anywhere.
//!
//! A release carries three Windows files, made by `.github/workflows/release.yml`: the package
//! [`ASSET`], a list of checksums [`SUMS`] and its Ed25519 signature [`SIG`], made with the release key
//! (the same key as the firmware: `maarse-sign sign-release`). The service installs a package only when
//! the signature matches the public key built into it (`MAARSE_RELEASE_PUBKEY`) and the package matches
//! its line in the checksum list. GitHub is only the way the files travel.

use maarse_crypto::ota;
use std::process::Command;

/// The repository the releases come from.
pub const REPO: &str = "trixxor/maarse";
pub const ASSET: &str = "maarse-windows-x64.zip";
pub const SUMS: &str = "SHA256SUMS-windows";
pub const SIG: &str = "SHA256SUMS-windows.sig";

/// What the newest GitHub release offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// Version without the leading `v`.
    pub version: String,
    /// The release notes written on GitHub: the changes and fixes of this version.
    pub notes: String,
    /// The release page.
    pub page: String,
    pub zip_url: String,
    pub sums_url: String,
    pub sig_url: String,
}

/// The public release key built into this program, if there is a valid one. Without it no update is
/// ever installed.
pub fn release_key() -> Option<[u8; 32]> {
    ota::parse_hex32(option_env!("MAARSE_RELEASE_PUBKEY")?).filter(ota::valid_release_key)
}

/// `v0.7.8` or `0.7.8` as numbers; anything after a `-` or `+` is ignored.
pub fn parse_version(text: &str) -> Option<(u32, u32, u32)> {
    let core = text.trim().trim_start_matches('v').split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

pub fn is_newer(current: &str, candidate: &str) -> bool {
    match (parse_version(current), parse_version(candidate)) {
        (Some(c), Some(n)) => n > c,
        _ => false,
    }
}

/// Reads the answer of `GET /repos/<repo>/releases/latest`. `None` when the release lacks one of the three
/// Windows files (a firmware-only release): then there is nothing to update to.
pub fn parse_release(json: &str) -> Option<Release> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let tag = v["tag_name"].as_str()?;
    parse_version(tag)?;
    let assets = v["assets"].as_array()?;
    let url = |name: &str| -> Option<String> {
        let u = assets.iter().find(|a| a["name"].as_str() == Some(name))?["browser_download_url"].as_str()?;
        u.starts_with("https://").then(|| u.to_string())
    };
    Some(Release {
        version: tag.trim_start_matches('v').to_string(),
        notes: v["body"].as_str().unwrap_or("").trim().to_string(),
        page: v["html_url"].as_str().unwrap_or("").to_string(),
        zip_url: url(ASSET)?,
        sums_url: url(SUMS)?,
        sig_url: url(SIG)?,
    })
}

fn curl(args: &[&str]) -> Result<Vec<u8>, String> {
    let out = Command::new("curl.exe")
        .args(["-sSL", "--fail", "-H", "User-Agent: maarse-service"])
        .args(args)
        .output()
        .map_err(|e| format!("curl.exe could not be started: {e}"))?;
    if out.status.success() { Ok(out.stdout) } else { Err(String::from_utf8_lossy(&out.stderr).trim().to_string()) }
}

/// Fetches the newest release. `Ok(None)` = none, or it lacks the Windows files.
pub fn fetch_latest() -> Result<Option<Release>, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let body = curl(&["--max-time", "15", "-H", "Accept: application/vnd.github+json", &url])?;
    Ok(parse_release(&String::from_utf8_lossy(&body)))
}

/// Downloads the package and its signature files and checks them. Returns the package bytes only when
/// they are signed by `key`.
pub fn download_verified(release: &Release, key: &[u8; 32]) -> Result<Vec<u8>, String> {
    let sums = curl(&["--max-time", "30", &release.sums_url]).map_err(|e| format!("Download of {SUMS} failed: {e}"))?;
    let sig = curl(&["--max-time", "30", &release.sig_url]).map_err(|e| format!("Download of {SIG} failed: {e}"))?;
    let zip = curl(&["--max-time", "600", &release.zip_url]).map_err(|e| format!("Download of {ASSET} failed: {e}"))?;
    verify_package(key, &String::from_utf8_lossy(&sums), &String::from_utf8_lossy(&sig), &zip)?;
    Ok(zip)
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

/// The signature must match the checksum list, and the package must match its line in that list.
pub fn verify_package(key: &[u8; 32], sums: &str, sig_hex: &str, zip: &[u8]) -> Result<(), String> {
    let sig = parse_sig(sig_hex).ok_or("The signature file is not valid.")?;
    if !ota::verify_release(key, sums.as_bytes(), &sig) {
        return Err("The signature does not match the Må:rse release key. Nothing was installed.".into());
    }
    let hash = hex(&ota::sha256(zip));
    let listed = sums.lines().find_map(|l| {
        let (h, name) = l.split_once("  ")?;
        (name.trim() == ASSET).then(|| h.trim().to_ascii_lowercase())
    });
    match listed {
        Some(h) if h == hash => Ok(()),
        Some(_) => Err("The downloaded package does not match the signed checksum. Nothing was installed.".into()),
        None => Err(format!("{ASSET} is not in the signed checksum list.")),
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use maarse_crypto::ota::SigningKey;

    #[test]
    fn versions_compare_as_numbers() {
        assert_eq!(parse_version("v0.7.8"), Some((0, 7, 8)));
        assert_eq!(parse_version("0.8.0-rc1"), Some((0, 8, 0)));
        assert_eq!(parse_version("abc"), None);
        assert!(is_newer("0.7.8", "v0.7.10"));
        assert!(!is_newer("0.7.8", "0.7.8"));
        assert!(!is_newer("0.8.0", "0.7.9"));
        assert!(!is_newer("0.7.8", "garbage"));
    }

    #[test]
    fn release_needs_all_three_windows_files() {
        let asset = |n: &str| format!(r#"{{"name":"{n}","browser_download_url":"https://x/{n}"}}"#);
        let json = |names: &[&str]| {
            format!(
                r#"{{"tag_name":"v0.8.0","body":"Fixes","html_url":"https://x/r","assets":[{}]}}"#,
                names.iter().map(|n| asset(n)).collect::<Vec<_>>().join(",")
            )
        };
        let r = parse_release(&json(&[ASSET, SUMS, SIG, "maarse.apk"])).unwrap();
        assert_eq!((r.version.as_str(), r.notes.as_str()), ("0.8.0", "Fixes"));
        assert!(parse_release(&json(&[ASSET, SUMS])).is_none(), "no signature, no update");
        assert!(parse_release(&json(&["maarse.apk"])).is_none());
        let plain = json(&[ASSET, SUMS, SIG]).replace("https://x/maarse-windows", "http://x/maarse-windows");
        assert!(parse_release(&plain).is_none(), "only https downloads");
    }

    fn signed(sk: &SigningKey, zip: &[u8]) -> (String, String) {
        let sums = format!("{}  {ASSET}\n{}  other.exe\n", hex(&ota::sha256(zip)), hex(&ota::sha256(b"x")));
        let sig = hex(&ota::sign_release(sk, sums.as_bytes()));
        (sums, sig)
    }

    #[test]
    fn only_a_correctly_signed_package_passes() {
        let sk = SigningKey::from_bytes(&[7; 32]);
        let key = sk.verifying_key().to_bytes();
        let zip = b"package bytes";
        let (sums, sig) = signed(&sk, zip);
        assert!(verify_package(&key, &sums, &sig, zip).is_ok());
        // Changed package.
        assert!(verify_package(&key, &sums, &sig, b"other bytes").is_err());
        // Signed by somebody else.
        let (sums2, sig2) = signed(&SigningKey::from_bytes(&[8; 32]), zip);
        assert!(verify_package(&key, &sums2, &sig2, zip).is_err());
        // Changed checksum list under the old signature.
        let forged = sums.replace(&hex(&ota::sha256(zip)), &hex(&ota::sha256(b"evil")));
        assert!(verify_package(&key, &forged, &sig, b"evil").is_err());
        // Garbage signature.
        assert!(verify_package(&key, &sums, "zz", zip).is_err());
    }
}
