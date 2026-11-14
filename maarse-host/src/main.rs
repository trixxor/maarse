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
mod filebox;
mod history;
#[cfg(test)]
mod fuzz_tests;
mod links;
mod meshcore;
mod mqtt;
mod node;
mod sessions;
mod tunnel;
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
    /// En IP-pakke fra TUN-adapteren (`--tun`).
    #[cfg_attr(not(feature = "tun"), allow(dead_code))]
    TunPacket(Vec<u8>),
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
    /// Filpostkassens kvote pr. modtager og i alt (bytes; 0 slår den fra).
    pub filebox_per_recipient: u64,
    pub filebox_total: u64,
    pub postbox: Option<maarse_core::Address>,
    /// Vores offentlige backbone-endpoint ("vært:port"). Gør hosten til en
    /// fuld DHT-deltager, der udgiver de adresser, den kan nå.
    pub public_addr: Option<String>,
    /// DHT-startpunkter (kun opslag, ingen pakkeforbindelse).
    pub dht_seeds: Vec<String>,
    /// Regioner fra kommandolinjen (`--region`); tom = brug gemte.
    pub regions: Vec<String>,
    pub strict_region: bool,
    /// MeshCore-gateway: seriel port til en companion_radio_usb-node.
    pub meshcore: Option<String>,
    pub meshcore_baud: u32,
    pub meshcore_bridge: Vec<meshcore::Mapping>,
    /// MQTT-bro (`--mqtt`).
    pub mqtt: Option<mqtt::MqttConfig>,
    /// Beskedhistorik: antal dage, der gemmes (`None` = `--no-history`).
    pub history_days: Option<u32>,
    /// IP-tunnel: opret en TUN-adapter (`--tun`), evt. med sti til wintun.dll.
    pub tun: bool,
    pub wintun: Option<String>,
    /// Tillad IP-tunnel over LoRa (`--tun-lora`); ellers kun over TCP.
    pub tun_lora: bool,
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
  --history-days <N>    gem beskedhistorik i N dage, så flere skærme ser hele
                        samtalen (standard: 90). OBS: gemmes ukrypteret i
                        <data>/historik
  --no-history          gem ingen beskedhistorik på disken
  --postkasse <adresse> brug denne node som min postkasse, når jeg er offline
  --filpostkasse-mb <MB>
                        plads til filer til modtagere, der er offline (standard:
                        200, 0 slår filpostkassen fra). Filer udløber efter 7 dage
  --filpostkasse-modtager-mb <MB>
                        heraf højst pr. modtager (standard: 20)
  --public-addr <vært:port>
                        min offentlige backbone-adresse: gør mig til DHT-node, så
                        andre kan slå de adresser op, jeg kan nå
  --dht <vært:port>     DHT-startpunkt (kun opslag, kan gentages). --peer og
                        --public bruges også som startpunkter
  --region <navn>       region, jeg videresender kanalbeskeder for, fx dk eller
                        nordjylland (kan gentages, højst 8). Uden regioner
                        videresendes alt
  --strict-region       uden regioner: videresend kun beskeder uden region
  --meshcore <port>     gateway til MeshCore: en MeshCore-node med firmwaren
                        companion_radio_usb på denne port, fx COM5
  --meshcore-baud <hastighed>
                        baudrate til MeshCore-noden (standard: 115200)
  --meshcore-bridge <maarse-kanal>=<meshcore-kanal>
                        kobl to kanaler sammen, fx offentlig=public eller
                        #aarhus=#aarhus (kan gentages; standard: offentlig=public)
  --mqtt <vært:port>    MQTT-bro: send modtaget telemetri, positioner og
                        kanalbeskeder som JSON, med Home Assistant discovery
  --mqtt-prefix <emne>  emne-præfiks (standard: maarse)
  --mqtt-user <navn>    brugernavn til brokeren
  --mqtt-pass <kode>    adgangskode til brokeren
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
        filebox_per_recipient: 20 << 20,
        filebox_total: 200 << 20,
        postbox: None,
        public_addr: None,
        dht_seeds: Vec::new(),
        regions: Vec::new(),
        strict_region: false,
        meshcore: None,
        meshcore_baud: 115_200,
        meshcore_bridge: Vec::new(),
        mqtt: None,
        history_days: Some(90),
        tun: false,
        wintun: None,
        tun_lora: false,
    };
    let mut mqtt_prefix = "maarse".to_string();
    let (mut mqtt_user, mut mqtt_pass) = (None, None);
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
            "--no-history" => c.history_days = None,
            "--history-days" => {
                let n: u32 = val("--history-days")?
                    .parse()
                    .map_err(|_| "--history-days: ugyldigt tal")?;
                c.history_days = Some(n.max(1));
            }
            "--public-addr" => c.public_addr = Some(val("--public-addr")?),
            "--dht" => c.dht_seeds.push(val("--dht")?),
            "--region" => c.regions.push(val("--region")?.trim().to_lowercase()),
            "--strict-region" => c.strict_region = true,
            "--meshcore" => c.meshcore = Some(val("--meshcore")?),
            "--meshcore-baud" => {
                c.meshcore_baud = val("--meshcore-baud")?
                    .parse()
                    .map_err(|_| "--meshcore-baud: ugyldigt tal")?
            }
            "--meshcore-bridge" => c
                .meshcore_bridge
                .push(meshcore::parse_mapping(&val("--meshcore-bridge")?)?),

            "--mqtt" => {
                c.mqtt = Some(mqtt::MqttConfig {
                    addr: val("--mqtt")?,
                    prefix: String::new(),
                    user: None,
                    pass: None,
                })
            }
            "--tun" => c.tun = true,
            "--tun-lora" => c.tun_lora = true,
            "--wintun" => {
                c.tun = true;
                c.wintun = Some(val("--wintun")?);
            }
            "--mqtt-prefix" => mqtt_prefix = val("--mqtt-prefix")?.trim_end_matches('/').to_string(),
            "--mqtt-user" => mqtt_user = Some(val("--mqtt-user")?),
            "--mqtt-pass" => mqtt_pass = Some(val("--mqtt-pass")?),
            "--public" => c.peers.extend(PUBLIC_PEERS.iter().map(|s| s.to_string())),
            "--max-file" => {
                let kb: u32 = val("--max-file")?
                    .parse()
                    .map_err(|_| "--max-file: ugyldigt tal")?;
                c.max_file = kb.saturating_mul(1024);
            }
            "--filpostkasse-mb" => {
                let mb: u64 = val("--filpostkasse-mb")?
                    .parse()
                    .map_err(|_| "--filpostkasse-mb: ugyldigt tal")?;
                c.filebox_total = mb << 20;
            }
            "--filpostkasse-modtager-mb" => {
                let mb: u64 = val("--filpostkasse-modtager-mb")?
                    .parse()
                    .map_err(|_| "--filpostkasse-modtager-mb: ugyldigt tal")?;
                c.filebox_per_recipient = mb << 20;
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
    if let Some(m) = c.mqtt.as_mut() {
        m.prefix = mqtt_prefix;
        m.user = mqtt_user;
        m.pass = mqtt_pass;
    } else if mqtt_user.is_some() || mqtt_pass.is_some() {
        return Err("--mqtt-user/--mqtt-pass kræver --mqtt".into());
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
    #[allow(unused_mut)]
    let mut node = match node::Node::new(&cfg) {
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
    if let Some(port) = &cfg.meshcore {
        println!("  MeshCore: {port} @ {} baud (companion_radio_usb)", cfg.meshcore_baud);
        meshcore::spawn(port.clone(), cfg.meshcore_baud, cfg.meshcore_bridge.clone(), tx.clone());
    }
    if let Some(m) = &cfg.mqtt {
        println!("  MQTT:     {} (emner under {}/)", m.addr, m.prefix);
    }
    if let Some(addr) = cfg.http {
        let token = web::access_token(&cfg.data_dir);
        match web::spawn(addr, token.clone(), tx.clone()) {
            Ok(()) => {
                let url = format!("http://{addr}/");
                println!("  Webklient: {url}");
                if !addr.ip().is_loopback() {
                    let shown = if addr.ip().is_unspecified() { format!("<denne-pc>:{}", addr.port()) } else { addr.to_string() };
                    println!("  Fjernadgang: kræver adgangsnøglen (se {})", cfg.data_dir.join("adgangsnoegle.txt").display());
                    println!("    Browser: http://{shown}/#token={token}");
                    println!("    App:     adresse {shown}, nøgle {token}");
                }
                if cfg.open_browser {
                    open_browser(&url);
                }
            }
            Err(e) => eprintln!("  Webklient: kunne ikke starte på {addr}: {e}"),
        }
    }
    println!(
        "  IP:       {} (IP-tunnel{})",
        tunnel::ip_text(&node.my_ip()),
        if cfg.tun_lora { ", også over LoRa" } else { ", kun over TCP" }
    );
    if cfg.tun {
        #[cfg(feature = "tun")]
        match tunnel::spawn(node.my_ip(), cfg.wintun.clone(), tx.clone()) {
            Ok(t) => {
                node.set_tun(t);
                println!("  TUN:      adapter 'maarse' oprettet (fd00::/8)");
            }
            Err(e) => eprintln!("  TUN:      kunne ikke oprette adapteren ({e}); kræver administrator/root og wintun.dll på Windows"),
        }
        #[cfg(not(feature = "tun"))]
        eprintln!("  TUN:      bygget uden featuren 'tun' (cargo build --features tun)");
    }
    println!("Tryk Ctrl+C for at stoppe.\n");
    if let Some(ep) = &cfg.public_addr {
        println!("  DHT:      offentlig som {ep}");
    }
    node.run(rx, tx);
}
