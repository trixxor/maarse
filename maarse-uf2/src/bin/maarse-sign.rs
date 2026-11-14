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
        "Brug:\n  maarse-sign keygen <privat.key>\n  maarse-sign sign --board <board> --version <x.y.z> (--key <fil> | --dev-key) <firmware.elf|.uf2|.bin> <ud>\n  maarse-sign verify (--pubkey <hex> | --dev-key) <ud.bin> <ud.manifest>\nBoards: xiao-nrf52840, wismesh-tag, heltec-v4, xiao-esp32s3"
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
        _ => usage(),
    }
}
