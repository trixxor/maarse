//! maarse-host: gateway og testnode til Må:rse. Kører på Windows og Linux.
//!
//! * Kører en fuld Må:rse-node med egen identitet og kryptering.
//! * Forbinder til andre maarse-host over TCP (backbone).
//! * Kan bruge en LoRa-node over seriel i modem-tilstand (kræver firmware).
//! * Serverer webklienten på http://127.0.0.1:8080, som taler med noden
//!   over WebSocket.
//!
//! Start:  maarse-host
//! Hjælp:  maarse-host --help
#![forbid(unsafe_code)]

mod dht;
#[cfg(test)]
mod fuzz_tests;
mod links;
mod node;
mod sessions;
mod web;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::Sender;

pub type LinkId = u32;
pub type ClientId = u32;

static NEXT_ID: AtomicU32 = AtomicU32::new(1);

pub fn next_id() -> u32 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// Beskeder til nodens hovedløkke.
pub enum In {
    /// Et pakke-link (TCP eller seriel) er oppe. `tx` tager rå Må:rse-pakker.
    LinkUp {
        id: LinkId,
        name: String,
        tx: Sender<Vec<u8>>,
    },
    LinkDown {
        id: LinkId,
    },
    /// En backbone-peer har fortalt, hvilke regioner den betjener.
    LinkRegions {
        id: LinkId,
        strict: bool,
        regions: Vec<u16>,
    },
    Packet {
        link: LinkId,
        data: Vec<u8>,
        snr: i8,
    },
    /// En webklient er forbundet. `tx` tager færdige companion-rammer.
    ClientUp {
        id: ClientId,
        tx: Sender<Vec<u8>>,
    },
    ClientDown {
        id: ClientId,
    },
    ClientFrame {
        id: ClientId,
        frame: Vec<u8>,
    },
    /// En DHT-forespørgsel fra en anden backbone-host; svaret sendes på `reply`.
    DhtRequest {
        data: Vec<u8>,
        reply: Sender<Vec<u8>>,
    },
    /// Et DHT-opslag (kørt i en tråd) er færdigt.
    DhtDone {
        target: maarse_core::Address,
        publish: bool,
        result: dht::Lookup,
    },
}

pub struct Config {
    pub data_dir: PathBuf,
    pub http: Option<SocketAddr>,
    pub listen: Option<SocketAddr>,
    pub peers: Vec<String>,
    pub serial: Option<String>,
    pub baud: u32,
    pub open_browser: bool,
    pub max_file: u32,
    pub postbox: Option<maarse_core::Address>,
    /// Vores offentlige backbone-endpoint ("vært:port"). Gør hosten til en
    /// fuld DHT-deltager, der udgiver de adresser, den kan nå.
    pub public_addr: Option<String>,
    /// DHT-startpunkter (kun opslag, ingen pakkeforbindelse).
    pub dht_seeds: Vec<String>,
    /// Regioner fra kommandolinjen (`--region`); tom = brug gemte.
    pub regions: Vec<String>,
    pub strict_region: bool,
}

/// Offentlige backbone-noder, som `--public` forbinder til. Tom indtil
/// nogen sætter servere op.
pub const PUBLIC_PEERS: &[&str] = &[];

const HELP: &str = "\
maarse-host – gateway og testnode til Må:rse

Brug: maarse-host [valg]

  --data <mappe>        hvor identitet og indstillinger gemmes (standard: maarse-data)
  --http <adr:port>     webklient og WebSocket (standard: 127.0.0.1:8080, 'off' slår fra)
  --listen <adr:port>   modtag backbone-forbindelser over TCP (standard: 0.0.0.0:4242, 'off' slår fra)
  --peer <vært:port>    forbind til en anden maarse-host (kan gentages)
  --serial <port>       LoRa-node i modem-tilstand, fx COM3 eller /dev/ttyACM0
  --baud <hastighed>    baudrate til seriel (standard: 115200)
  --public              forbind også til de offentlige backbone-noder
  --max-file <KB>       største fil, der sendes og modtages (standard: 1024)
  --postkasse <adresse> brug denne node som min postkasse, når jeg er offline
  --public-addr <vært:port>
                        min offentlige backbone-adresse: gør mig til DHT-node, så
                        andre kan slå de adresser op, jeg kan nå
  --dht <vært:port>     DHT-startpunkt (kun opslag, kan gentages). --peer og
                        --public bruges også som startpunkter
  --region <navn>       region, jeg videresender kanalbeskeder for, fx dk eller
                        nordjylland (kan gentages, højst 8). Uden regioner
                        videresendes alt
  --strict-region       uden regioner: videresend kun beskeder uden region
  --no-browser          åbn ikke browseren automatisk
  --help                vis denne tekst

Eksempel med to computere:
  computer A:  maarse-host
  computer B:  maarse-host --peer <A's IP>:4242
";

fn parse_args() -> Result<Config, String> {
    let mut c = Config {
        data_dir: PathBuf::from("maarse-data"),
        http: Some("127.0.0.1:8080".parse().unwrap()),
        listen: Some("0.0.0.0:4242".parse().unwrap()),
        peers: Vec::new(),
        serial: None,
        baud: 115_200,
        open_browser: true,
        max_file: 1024 * 1024,
        postbox: None,
        public_addr: None,
        dht_seeds: Vec::new(),
        regions: Vec::new(),
        strict_region: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut val = |name: &str| args.next().ok_or(format!("{name} mangler en værdi"));
        let addr = |v: String, name: &str| -> Result<Option<SocketAddr>, String> {
            if v == "off" {
                Ok(None)
            } else {
                v.parse()
                    .map(Some)
                    .map_err(|_| format!("{name}: ugyldig adresse '{v}'"))
            }
        };
        match a.as_str() {
            "--data" => c.data_dir = PathBuf::from(val("--data")?),
            "--http" => c.http = addr(val("--http")?, "--http")?,
            "--listen" => c.listen = addr(val("--listen")?, "--listen")?,
            "--peer" => c.peers.push(val("--peer")?),
            "--serial" => c.serial = Some(val("--serial")?),
            "--baud" => c.baud = val("--baud")?.parse().map_err(|_| "--baud: ugyldigt tal")?,
            "--no-browser" => c.open_browser = false,
            "--public-addr" => c.public_addr = Some(val("--public-addr")?),
            "--dht" => c.dht_seeds.push(val("--dht")?),
            "--region" => c.regions.push(val("--region")?.trim().to_lowercase()),
            "--strict-region" => c.strict_region = true,

            "--public" => c.peers.extend(PUBLIC_PEERS.iter().map(|s| s.to_string())),
            "--max-file" => {
                let kb: u32 = val("--max-file")?
                    .parse()
                    .map_err(|_| "--max-file: ugyldigt tal")?;
                c.max_file = kb.saturating_mul(1024);
            }
            "--postkasse" => {
                let v = val("--postkasse")?;
                let b: Vec<u8> = (0..v.len())
                    .step_by(2)
                    .filter_map(|i| v.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok()))
                    .collect();
                c.postbox = Some(
                    maarse_core::Address::from_slice(&b)
                        .filter(|_| v.len() == 32)
                        .ok_or("--postkasse: adressen skal være 32 hex-tegn")?,
                );
            }
            "--help" | "-h" => {
                print!("{HELP}");
                std::process::exit(0);
            }
            other => return Err(format!("ukendt valg: {other} (prøv --help)")),
        }
    }
    Ok(c)
}

fn open_browser(url: &str) {
    #[cfg(target_os = "windows")]
    let r = std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn();
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(url).spawn();
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let r = std::process::Command::new("xdg-open").arg(url).spawn();
    if r.is_err() {
        println!("Åbn {url} i din browser.");
    }
}

fn main() {
    let cfg = match parse_args() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Fejl: {e}");
            std::process::exit(2);
        }
    };
    let (tx, rx) = std::sync::mpsc::channel();
    let node = match node::Node::new(&cfg) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("Fejl ved opstart: {e}");
            std::process::exit(1);
        }
    };
    println!("Må:rse-host v{}", env!("CARGO_PKG_VERSION"));
    println!("  Adresse:  {}", node.address());
    println!("  Data:     {}", cfg.data_dir.display());

    if let Some(addr) = cfg.listen {
        match links::spawn_tcp_listener(addr, tx.clone()) {
            Ok(()) => println!("  Backbone: lytter på {addr}"),
            Err(e) => eprintln!("  Backbone: kunne ikke lytte på {addr}: {e}"),
        }
    }
    for p in &cfg.peers {
        println!("  Backbone: forbinder til {p}");
        links::spawn_tcp_peer(p.clone(), tx.clone());
    }
    if let Some(port) = &cfg.serial {
        println!("  Seriel:   {port} @ {} baud (modem-tilstand)", cfg.baud);
        links::spawn_serial(port.clone(), cfg.baud, tx.clone());
    }
    if let Some(addr) = cfg.http {
        match web::spawn(addr, tx.clone()) {
            Ok(()) => {
                let url = format!("http://{addr}/");
                println!("  Webklient: {url}");
                if cfg.open_browser {
                    open_browser(&url);
                }
            }
            Err(e) => eprintln!("  Webklient: kunne ikke starte på {addr}: {e}"),
        }
    }
    println!("Tryk Ctrl+C for at stoppe.\n");
    if let Some(ep) = &cfg.public_addr {
        println!("  DHT:      offentlig som {ep}");
    }
    node.run(rx, tx);
}
