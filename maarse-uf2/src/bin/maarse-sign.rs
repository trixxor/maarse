//! maarse-sign: signerer firmware til opdatering over luften.
//!
//! ```text
//! maarse-sign keygen <privat.key>
//!     Laver en udgivelsesnøgle. Den offentlige nøgle skrives ud og bygges ind
//!     i firmwaren med MAARSE_RELEASE_PUBKEY=<hex>.
//! maarse-sign sign --board <board> --version <x.y.z> (--key <privat.key> | --dev-key)
//!                  <firmware.elf|.uf2|.bin> <ud>
//!     Skriver <ud>.bin (imaget) og <ud>.manifest (112 bytes).
//! maarse-sign verify (--pubkey <hex> | --dev-key) <ud.bin> <ud.manifest>
//! maarse-sign sign-release --key <privat.key> <mappe>
//!     Skriver <mappe>/SHA256SUMS (alle filer i mappen) og SHA256SUMS.sig
//!     (Ed25519-signatur, hex) med udgivelsesnøglen.
//! maarse-sign verify-release --pubkey <hex> <mappe>
//!     Tjekker signaturen og at alle filer passer med SHA256SUMS.
//! ```
//!
//! Boards: xiao-nrf52840, wismesh-tag, heltec-v4, xiao-esp32s3. På nRF52840
//! skal imaget starte ved appens adresse (0x27000 på XIAO, 0x26000 på
//! WisMesh). På ESP32-S3 er input et ESP-IDF app-image fra
//! `espflash save-image --chip esp32s3 <elf> <bin>`.
#![forbid(unsafe_code)]

use maarse_crypto::ota::{self, Manifest, SigningKey, board};
use rand_core::{OsRng, RngCore};
use std::fs;
use std::process::exit;

fn die(msg: impl std::fmt::Display) -> ! {
    eprintln!("Fejl: {msg}");
    exit(1)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn usage() -> ! {
    eprintln!(
        "Brug:\n  maarse-sign keygen <privat.key>\n  maarse-sign sign --board <board> --version <x.y.z> (--key <fil> | --dev-key) <firmware.elf|.uf2|.bin> <ud>\n  maarse-sign verify (--pubkey <hex> | --dev-key) <ud.bin> <ud.manifest>\n  maarse-sign sign-release --key <fil> <mappe>\n  maarse-sign verify-release --pubkey <hex> <mappe>\nBoards: xiao-nrf52840, wismesh-tag, heltec-v4, xiao-esp32s3"
    );
    exit(2)
}

/// Appens startadresse på nRF52840-boards (se firmware/nrf/build.rs).
fn nrf_origin(b: u16) -> Option<u32> {
    match b {
        board::XIAO_NRF52840 => Some(0x27000),
        board::WISMESH_TAG => Some(0x26000),
        _ => None,
    }
}

fn read_key(path: &str) -> SigningKey {
    let s = fs::read_to_string(path).unwrap_or_else(|e| die(format!("{path}: {e}")));
    let seed = ota::parse_hex32(&s).unwrap_or_else(|| die("nøglefilen skal indeholde 64 hextegn"));
    SigningKey::from_bytes(&seed)
}

/// Læser firmwaren som det kanoniske image (se `maarse_uf2::flatten`).
fn load_image(path: &str, b: u16) -> Vec<u8> {
    let data = fs::read(path).unwrap_or_else(|e| die(format!("{path}: {e}")));
    let flat = if data.starts_with(b"\x7fELF") {
        let segs = maarse_uf2::load_segments(&data).unwrap_or_else(|e| die(e));
        Some(maarse_uf2::flatten(&maarse_uf2::pages(&segs)).unwrap_or_else(|| die("tom ELF")))
    } else if path.to_ascii_lowercase().ends_with(".uf2") {
        let pages = maarse_uf2::uf2_pages(&data).unwrap_or_else(|e| die(e));
        Some(maarse_uf2::flatten(&pages).unwrap_or_else(|| die("tom UF2")))
    } else {
        None
    };
    match (flat, nrf_origin(b)) {
        (Some((start, img)), Some(origin)) => {
            if start != origin {
                die(format!(
                    "imaget starter ved 0x{start:X}, men {} forventer 0x{origin:X} (forkert board?)",
                    board::name(b)
                ));
            }
            img
        }
        (Some(_), None) => die("ESP32-S3 skal have et app-image (.bin fra espflash save-image)"),
        (None, Some(_)) => {
            eprintln!("Bemærk: .bin antages at starte ved appens adresse");
            data
        }
        (None, None) => {
            if data.first() != Some(&0xE9) {
                die("ikke et ESP-IDF app-image (første byte skal være 0xE9)");
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
                board_id = Some(board::from_name(&v).unwrap_or_else(|| die(format!("ukendt board: {v}"))));
            }
            "--version" => {
                let v = val();
                version = Some(ota::parse_version(&v).unwrap_or_else(|| die(format!("ugyldig version: {v}"))));
            }
            "--key" => key = Some(val()),
            "--pubkey" => pubkey = Some(val()),
            "--dev-key" => dev = true,
            _ => pos.push(a.clone()),
        }
    }
    match args.first().map(String::as_str) {
        Some("keygen") => {
            let [path] = pos.as_slice() else { usage() };
            let mut seed = [0u8; 32];
            OsRng.fill_bytes(&mut seed);
            if fs::metadata(path).is_ok() {
                die(format!("{path} findes allerede; overskriver ikke en nøgle"));
            }
            fs::write(path, hex(&seed) + "\n").unwrap_or_else(|e| die(e));
            let public = SigningKey::from_bytes(&seed).verifying_key().to_bytes();
            println!("Privat nøgle gemt i {path} (hold den hemmelig, tag backup).");
            println!("Offentlig nøgle - byg firmwaren med:");
            println!("  MAARSE_RELEASE_PUBKEY={}", hex(&public));
        }
        Some("sign") => {
            let [input, out] = pos.as_slice() else { usage() };
            let b = board_id.unwrap_or_else(|| usage());
            let v = version.unwrap_or_else(|| usage());
            let sk = match (key, dev) {
                (Some(k), false) => read_key(&k),
                (None, true) => {
                    eprintln!("ADVARSEL: signerer med den offentlige udviklingsnøgle - kun til test!");
                    ota::dev_signing_key()
                }
                _ => die("angiv enten --key <fil> eller --dev-key"),
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
            println!("Offentlig nøgle: {}", hex(&sk.verifying_key().to_bytes()));
        }
        Some("verify") => {
            let [bin, man] = pos.as_slice() else { usage() };
            let pk = match (pubkey, dev) {
                (Some(p), false) => ota::parse_hex32(&p).unwrap_or_else(|| die("--pubkey skal være 64 hextegn")),
                (None, true) => ota::dev_public_key(),
                _ => die("angiv enten --pubkey <hex> eller --dev-key"),
            };
            let img = fs::read(bin).unwrap_or_else(|e| die(e));
            let m = Manifest::parse(&fs::read(man).unwrap_or_else(|e| die(e))).unwrap_or_else(|e| die(e.text()));
            m.verify_signature(&pk).unwrap_or_else(|e| die(e.text()));
            let ok = m.size as usize == img.len() && ota::sha256(&img) == m.sha256;
            if !ok {
                die("imaget passer ikke med manifestet (størrelse/SHA-256)");
            }
            let (ma, mi, pa) = ota::version_parts(m.version);
            println!("OK: {} version {ma}.{mi}.{pa}, {} bytes", board::name(m.board), m.size);
        }
        Some("sign-release") => {
            let [dir] = pos.as_slice() else { usage() };
            let sk = read_key(&key.unwrap_or_else(|| die("angiv --key <fil>")));
            let sums = release_sums(dir);
            fs::write(format!("{dir}/SHA256SUMS"), &sums).unwrap_or_else(|e| die(e));
            let sig = ota::sign_release(&sk, sums.as_bytes());
            fs::write(format!("{dir}/SHA256SUMS.sig"), hex(&sig) + "\n").unwrap_or_else(|e| die(e));
            print!("{sums}");
            println!("Signeret med {}", hex(&sk.verifying_key().to_bytes()));
        }
        Some("verify-release") => {
            let [dir] = pos.as_slice() else { usage() };
            let pk = ota::parse_hex32(&pubkey.unwrap_or_else(|| die("angiv --pubkey <hex>")))
                .unwrap_or_else(|| die("--pubkey skal være 64 hextegn"));
            let sums = fs::read_to_string(format!("{dir}/SHA256SUMS")).unwrap_or_else(|e| die(e));
            let sig_hex = fs::read_to_string(format!("{dir}/SHA256SUMS.sig")).unwrap_or_else(|e| die(e));
            let t = sig_hex.trim();
            let sig: Vec<u8> = (0..t.len() / 2).filter_map(|i| u8::from_str_radix(&t[i * 2..i * 2 + 2], 16).ok()).collect();
            let sig: [u8; 64] = sig.try_into().unwrap_or_else(|_| die("SHA256SUMS.sig er ugyldig"));
            if !ota::verify_release(&pk, sums.as_bytes(), &sig) {
                die("signaturen passer ikke (forkert nøgle eller ændret SHA256SUMS)");
            }
            if sums != release_sums(dir) {
                die("filerne passer ikke med SHA256SUMS");
            }
            println!("OK: {} filer, signeret med {}", sums.lines().count(), hex(&pk));
        }
        _ => usage(),
    }
}

/// SHA256SUMS-indhold for alle filer i mappen (sorteret, uden SHA256SUMS*).
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
