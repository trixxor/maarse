//! maarse-sign: signs firmware for over-the-air updates.
//!
//! ```text
//! maarse-sign keygen <private.key>
//!     Makes a release key. The public key is printed and built into
//!     the firmware with MAARSE_RELEASE_PUBKEY=<hex>.
//! maarse-sign sign --board <board> --version <x.y.z> (--key <private.key> | --dev-key)
//!                  <firmware.elf|.uf2|.bin> <out>
//!     Writes <out>.bin (the image) and <out>.manifest (112 bytes).
//! maarse-sign verify (--pubkey <hex> | --dev-key) <out.bin> <out.manifest>
//! maarse-sign sign-release --key <private.key> <dir>
//!     Writes <dir>/SHA256SUMS (all files in the folder) and SHA256SUMS.sig
//!     (Ed25519 signature, hex) with the release key.
//! maarse-sign verify-release --pubkey <hex> <dir>
//!     Checks the signature and that all files match SHA256SUMS.
//! ```
//!
//! Boards: xiao-nrf52840, wismesh-tag, heltec-v4, xiao-esp32s3. On the nRF52840
//! the image must start at the app's address (0x27000 on the XIAO, 0x26000 on the
//! WisMesh). On the ESP32-S3 the input is an ESP-IDF app image from
//! `espflash save-image --chip esp32s3 <elf> <bin>`.
#![forbid(unsafe_code)]

use maarse_crypto::ota::{self, Manifest, SigningKey, board};
use rand_core::{OsRng, RngCore};
use std::fs;
use std::process::exit;

fn die(msg: impl std::fmt::Display) -> ! {
    eprintln!("Error: {msg}");
    exit(1)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn usage() -> ! {
    eprintln!(
        "Usage:\n  maarse-sign keygen <private.key>\n  maarse-sign check-release-key --pubkey <hex> [--key <file>]\n  maarse-sign sign --board <board> --version <x.y.z> (--key <file> | --dev-key) <firmware.elf|.uf2|.bin> <out>\n  maarse-sign verify (--pubkey <hex> | --dev-key) <out.bin> <out.manifest>\n  maarse-sign sign-release --key <file> <dir>\n  maarse-sign verify-release --pubkey <hex> <dir>\nBoards: xiao-nrf52840, wismesh-tag, heltec-v4, xiao-esp32s3"
    );
    exit(2)
}

/// The app's start address on nRF52840 boards (see firmware/nrf/build.rs).
fn nrf_origin(b: u16) -> Option<u32> {
    match b {
        board::XIAO_NRF52840 => Some(0x27000),
        board::WISMESH_TAG => Some(0x26000),
        _ => None,
    }
}

fn read_key(path: &str) -> SigningKey {
    let s = fs::read_to_string(path).unwrap_or_else(|e| die(format!("{path}: {e}")));
    let seed = ota::parse_hex32(&s).unwrap_or_else(|| die("the key file must contain 64 hex characters"));
    SigningKey::from_bytes(&seed)
}

/// Reads the firmware as the canonical image (see `maarse_uf2::flatten`).
fn load_image(path: &str, b: u16) -> Vec<u8> {
    let data = fs::read(path).unwrap_or_else(|e| die(format!("{path}: {e}")));
    let flat = if data.starts_with(b"\x7fELF") {
        let segs = maarse_uf2::load_segments(&data).unwrap_or_else(|e| die(e));
        Some(maarse_uf2::flatten(&maarse_uf2::pages(&segs)).unwrap_or_else(|| die("empty ELF")))
    } else if path.to_ascii_lowercase().ends_with(".uf2") {
        let pages = maarse_uf2::uf2_pages(&data).unwrap_or_else(|e| die(e));
        Some(maarse_uf2::flatten(&pages).unwrap_or_else(|| die("empty UF2")))
    } else {
        None
    };
    match (flat, nrf_origin(b)) {
        (Some((start, img)), Some(origin)) => {
            if start != origin {
                die(format!(
                    "the image starts at 0x{start:X}, but {} expects 0x{origin:X} (wrong board?)",
                    board::name(b)
                ));
            }
            img
        }
        (Some(_), None) => die("ESP32-S3 needs an app image (.bin from espflash save-image)"),
        (None, Some(_)) => {
            eprintln!("Note: the .bin is assumed to start at the app's address");
            data
        }
        (None, None) => {
            if data.first() != Some(&0xE9) {
                die("not an ESP-IDF app image (the first byte must be 0xE9)");
            }
            data
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut pos = Vec::new();
    let (mut board_id, mut version, mut key, mut pubkey, mut dev) = (None, None, None, None, false);
    let mut it = args.iter().skip(1);
    while let Some(a) = it.next() {
        let mut val = || it.next().cloned().unwrap_or_else(|| usage());
        match a.as_str() {
            "--board" => {
                let v = val();
                board_id = Some(board::from_name(&v).unwrap_or_else(|| die(format!("unknown board: {v}"))));
            }
            "--version" => {
                let v = val();
                version = Some(ota::parse_version(&v).unwrap_or_else(|| die(format!("invalid version: {v}"))));
            }
            "--key" => key = Some(val()),
            "--pubkey" => pubkey = Some(val()),
            "--dev-key" => dev = true,
            _ => pos.push(a.clone()),
        }
    }
    match args.first().map(String::as_str) {
        Some("check-release-key") => {
            if dev || !pos.is_empty() {
                usage();
            }
            let public = pubkey.as_deref().and_then(ota::parse_hex32)
                .filter(ota::valid_release_key)
                .unwrap_or_else(|| die("configure a valid non-development MAARSE_RELEASE_PUBKEY"));
            if let Some(path) = key.as_deref()
                && read_key(path).verifying_key().to_bytes() != public
            {
                die("the signing key does not match MAARSE_RELEASE_PUBKEY");
            }
            println!("Release key configuration verified");
        }
        Some("keygen") => {
            let [path] = pos.as_slice() else { usage() };
            let mut seed = [0u8; 32];
            OsRng.fill_bytes(&mut seed);
            if fs::metadata(path).is_ok() {
                die(format!("{path} already exists; not overwriting a key"));
            }
            fs::write(path, hex(&seed) + "\n").unwrap_or_else(|e| die(e));
            let public = SigningKey::from_bytes(&seed).verifying_key().to_bytes();
            println!("Private key saved in {path} (keep it secret, make a backup).");
            println!("Public key - build the firmware with:");
            println!("  MAARSE_RELEASE_PUBKEY={}", hex(&public));
        }
        Some("sign") => {
            let [input, out] = pos.as_slice() else { usage() };
            let b = board_id.unwrap_or_else(|| usage());
            let v = version.unwrap_or_else(|| usage());
            let sk = match (key, dev) {
                (Some(k), false) => read_key(&k),
                (None, true) => {
                    eprintln!("WARNING: signing with the public development key - for testing only!");
                    ota::dev_signing_key()
                }
                _ => die("give either --key <file> or --dev-key"),
            };
            let img = load_image(input, b);
            let m = Manifest::sign(&sk, b, v, &img);
            fs::write(format!("{out}.bin"), &img).unwrap_or_else(|e| die(e));
            fs::write(format!("{out}.manifest"), m.to_bytes()).unwrap_or_else(|e| die(e));
            let (ma, mi, pa) = ota::version_parts(v);
            println!(
                "{out}.bin + {out}.manifest: {} version {ma}.{mi}.{pa}, {} bytes, sha256 {}",
                board::name(b),
                img.len(),
                hex(&m.sha256)
            );
            println!("Public key: {}", hex(&sk.verifying_key().to_bytes()));
        }
        Some("verify") => {
            let [bin, man] = pos.as_slice() else { usage() };
            let pk = match (pubkey, dev) {
                (Some(p), false) => ota::parse_hex32(&p).unwrap_or_else(|| die("--pubkey must be 64 hex characters")),
                (None, true) => ota::dev_public_key(),
                _ => die("give either --pubkey <hex> or --dev-key"),
            };
            let img = fs::read(bin).unwrap_or_else(|e| die(e));
            let m = Manifest::parse(&fs::read(man).unwrap_or_else(|e| die(e))).unwrap_or_else(|e| die(e.text()));
            m.verify_signature(&pk).unwrap_or_else(|e| die(e.text()));
            let ok = m.size as usize == img.len() && ota::sha256(&img) == m.sha256;
            if !ok {
                die("the image does not match the manifest (size/SHA-256)");
            }
            let (ma, mi, pa) = ota::version_parts(m.version);
            println!("OK: {} version {ma}.{mi}.{pa}, {} bytes", board::name(m.board), m.size);
        }
        Some("sign-release") => {
            let [dir] = pos.as_slice() else { usage() };
            let sk = read_key(&key.unwrap_or_else(|| die("give --key <file>")));
            let sums = release_sums(dir);
            fs::write(format!("{dir}/SHA256SUMS"), &sums).unwrap_or_else(|e| die(e));
            let sig = ota::sign_release(&sk, sums.as_bytes());
            fs::write(format!("{dir}/SHA256SUMS.sig"), hex(&sig) + "\n").unwrap_or_else(|e| die(e));
            print!("{sums}");
            println!("Signed with {}", hex(&sk.verifying_key().to_bytes()));
        }
        Some("verify-release") => {
            let [dir] = pos.as_slice() else { usage() };
            let pk = ota::parse_hex32(&pubkey.unwrap_or_else(|| die("angiv --pubkey <hex>")))
                .unwrap_or_else(|| die("--pubkey must be 64 hex characters"));
            let sums = fs::read_to_string(format!("{dir}/SHA256SUMS")).unwrap_or_else(|e| die(e));
            let sig_hex = fs::read_to_string(format!("{dir}/SHA256SUMS.sig")).unwrap_or_else(|e| die(e));
            let t = sig_hex.trim();
            let sig: Vec<u8> = (0..t.len() / 2).filter_map(|i| u8::from_str_radix(&t[i * 2..i * 2 + 2], 16).ok()).collect();
            let sig: [u8; 64] = sig.try_into().unwrap_or_else(|_| die("SHA256SUMS.sig is invalid"));
            if !ota::verify_release(&pk, sums.as_bytes(), &sig) {
                die("the signature does not match (wrong key or changed SHA256SUMS)");
            }
            if sums != release_sums(dir) {
                die("the files do not match SHA256SUMS");
            }
            println!("OK: {} files, signed with {}", sums.lines().count(), hex(&pk));
        }
        _ => usage(),
    }
}

/// SHA256SUMS content for all files in the folder (sorted, without SHA256SUMS*).
fn release_sums(dir: &str) -> String {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap_or_else(|e| die(e))
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with("SHA256SUMS"))
        .collect();
    names.sort();
    names
        .iter()
        .map(|n| {
            let data = fs::read(format!("{dir}/{n}")).unwrap_or_else(|e| die(e));
            format!("{}  {n}\n", hex(&ota::sha256(&data)))
        })
        .collect()
}
