//! maarse-host: gateway and test node for Må:rse. Runs on Windows and Linux.
//!
//! * Runs a full Må:rse node with its own identity and encryption.
//! * Connects to other maarse-host instances over TCP (backbone).
//! * Can use a LoRa node over serial in modem mode (requires firmware).
//! * Serves the web client at http://127.0.0.1:8080, which talks to the node
//!   over WebSocket.
//!
//! Start:  maarse-host
//! Help:   maarse-host --help
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
mod settings;
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

/// Messages for the node's main loop.
pub enum In {
    /// A packet link (TCP or serial) is up. `tx` takes raw Må:rse packets.
    LinkUp {
        id: LinkId,
        name: String,
        tx: Sender<Vec<u8>>,
    },
    LinkDown {
        id: LinkId,
    },
    /// A backbone peer has told us which regions it serves.
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
    /// A web client has connected. `tx` takes complete companion frames.
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
    /// A DHT request from another backbone host; the answer is sent on `reply`.
    DhtRequest {
        data: Vec<u8>,
        reply: Sender<Vec<u8>>,
    },
    /// An IP packet from the TUN adapter (`--tun`).
    #[cfg_attr(not(feature = "tun"), allow(dead_code))]
    TunPacket(Vec<u8>),
    /// A DHT lookup (run in a thread) has finished.
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
    /// The file mailbox quota per recipient and in total (bytes; 0 disables it).
    pub filebox_per_recipient: u64,
    pub filebox_total: u64,
    pub postbox: Option<maarse_core::Address>,
    /// Our public backbone endpoint ("host:port"). Makes the host a
    /// full DHT participant that publishes the addresses it can reach.
    pub public_addr: Option<String>,
    /// DHT seeds (lookups only, no packet connection).
    pub dht_seeds: Vec<String>,
    /// Regions from the command line (`--region`); empty = use the saved ones.
    pub regions: Vec<String>,
    pub strict_region: bool,
    /// MeshCore gateway: serial port to a companion_radio_usb node.
    pub meshcore: Option<String>,
    pub meshcore_baud: u32,
    pub meshcore_bridge: Vec<meshcore::Mapping>,
    /// MQTT bridge (`--mqtt`).
    pub mqtt: Option<mqtt::MqttConfig>,
    /// Message history: number of days to keep (`None` = `--no-history`).
    pub history_days: Option<u32>,
    /// IP tunnel: create a TUN adapter (`--tun`), optionally with a path to wintun.dll.
    pub tun: bool,
    pub wintun: Option<String>,
    /// Allow the IP tunnel over LoRa (`--tun-lora`); otherwise only over TCP.
    pub tun_lora: bool,
    /// Run under a service manager (`--service`): no browser, and a settings
    /// restart exits with RESTART_EXIT_CODE so the supervisor starts us again.
    pub service: bool,
}

/// Public backbone nodes that `--public` connects to. Empty until
/// someone sets up servers.
pub const PUBLIC_PEERS: &[&str] = &[];

const HELP: &str = "\
maarse-host – gateway and test node for Må:rse

Usage: maarse-host [options]

  --data <folder>       where identity and settings are stored (default: maarse-data)
  --http <addr:port>    web client and WebSocket (default: 127.0.0.1:8080, 'off' disables)
  --listen <addr:port>  accept backbone connections over TCP (default: 0.0.0.0:4242, 'off' disables)
  --peer <host:port>    connect to another maarse-host (can be repeated)
  --serial <port>       LoRa node in modem mode, e.g. COM3 or /dev/ttyACM0
  --baud <rate>         serial baud rate (default: 115200)
  --public              also connect to the public backbone nodes
  --max-file <KB>       largest file sent and received (default: 1024)
  --history-days <N>    keep message history for N days, so several screens see
                        the whole conversation (default: 90). NOTE: stored
                        unencrypted in <data>/history
  --no-history          keep no message history on disk
  --mailbox <address>   use this node as my mailbox while I am offline
  --file-mailbox-mb <MB>
                        space for files to recipients who are offline (default:
                        200, 0 disables the file mailbox). Files expire after 7 days
  --file-mailbox-recipient-mb <MB>
                        of which at most per recipient (default: 20)
  --public-addr <host:port>
                        my public backbone address: makes me a DHT node, so
                        others can look up the addresses I can reach
  --dht <host:port>     DHT seed (lookups only, can be repeated). --peer and
                        --public are also used as seeds
  --region <name>       region I forward channel messages for, e.g. dk or
                        nordjylland (can be repeated, at most 8). Without regions
                        everything is forwarded
  --strict-region       without regions: forward only messages without a region
  --meshcore <port>     gateway to MeshCore: a MeshCore node running the
                        companion_radio_usb firmware on this port, e.g. COM5
  --meshcore-baud <rate>
                        baud rate for the MeshCore node (default: 115200)
  --meshcore-bridge <maarse-channel>=<meshcore-channel>
                        link two channels, e.g. public=public or
                        #aarhus=#aarhus (can be repeated; default: public=public)
  --mqtt <host:port>    MQTT bridge: publish received telemetry, positions and
                        channel messages as JSON, with Home Assistant discovery
  --mqtt-prefix <topic> topic prefix (default: maarse)
  --mqtt-user <name>    user name for the broker
  --mqtt-pass <pass>    password for the broker
  --no-browser          do not open the browser automatically
  --service             run under a service manager (systemd, Task Scheduler):
                        implies --no-browser; a settings restart exits with
                        code 75 and leaves the restart to the supervisor
  --help                show this text

Example with two computers:
  computer A:  maarse-host
  computer B:  maarse-host --peer <A's IP>:4242
";

pub fn parse_from(argv: Vec<String>) -> Result<Config, String> {
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
        service: false,
    };
    let mut mqtt_prefix = "maarse".to_string();
    let (mut mqtt_user, mut mqtt_pass) = (None, None);
    let mut args = argv.into_iter();
    while let Some(a) = args.next() {
        let mut val = |name: &str| args.next().ok_or(format!("{name} is missing a value"));
        let addr = |v: String, name: &str| -> Result<Option<SocketAddr>, String> {
            if v == "off" {
                Ok(None)
            } else {
                v.parse()
                    .map(Some)
                    .map_err(|_| format!("{name}: invalid address '{v}'"))
            }
        };
        match a.as_str() {
            "--data" => c.data_dir = PathBuf::from(val("--data")?),
            "--http" => c.http = addr(val("--http")?, "--http")?,
            "--listen" => c.listen = addr(val("--listen")?, "--listen")?,
            "--peer" => c.peers.push(val("--peer")?),
            "--serial" => c.serial = Some(val("--serial")?),
            "--baud" => c.baud = val("--baud")?.parse().map_err(|_| "--baud: invalid number")?,
            "--no-browser" => c.open_browser = false,
            "--service" => {
                c.service = true;
                c.open_browser = false;
            }
            "--no-history" => c.history_days = None,
            "--history-days" => {
                let n: u32 = val("--history-days")?
                    .parse()
                    .map_err(|_| "--history-days: invalid number")?;
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
                    .map_err(|_| "--meshcore-baud: invalid number")?
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
                    .map_err(|_| "--max-file: invalid number")?;
                c.max_file = kb.saturating_mul(1024);
            }
            "--file-mailbox-mb" => {
                let mb: u64 = val("--file-mailbox-mb")?
                    .parse()
                    .map_err(|_| "--file-mailbox-mb: invalid number")?;
                c.filebox_total = mb << 20;
            }
            "--file-mailbox-recipient-mb" => {
                let mb: u64 = val("--file-mailbox-recipient-mb")?
                    .parse()
                    .map_err(|_| "--file-mailbox-recipient-mb: invalid number")?;
                c.filebox_per_recipient = mb << 20;
            }
            "--mailbox" => {
                let v = val("--mailbox")?;
                let b: Vec<u8> = (0..v.len())
                    .step_by(2)
                    .filter_map(|i| v.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok()))
                    .collect();
                c.postbox = Some(
                    maarse_core::Address::from_slice(&b)
                        .filter(|_| v.len() == 32)
                        .ok_or("--mailbox: the address must be 32 hex characters")?,
                );
            }
            "--help" | "-h" => {
                print!("{HELP}");
                std::process::exit(0);
            }
            other => return Err(format!("unknown option: {other} (try --help)")),
        }
    }
    if let Some(m) = c.mqtt.as_mut() {
        m.prefix = mqtt_prefix;
        m.user = mqtt_user;
        m.pass = mqtt_pass;
    } else if mqtt_user.is_some() || mqtt_pass.is_some() {
        return Err("--mqtt-user/--mqtt-pass requires --mqtt".into());
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
        println!("Open {url} in your browser.");
    }
}

fn main() {
    // Settings from the web client (<data>/settings.txt) first, then the
    // command line, which therefore wins.
    let cli: Vec<String> = std::env::args().skip(1).collect();
    let data_dir = cli
        .iter()
        .position(|a| a == "--data")
        .and_then(|i| cli.get(i + 1))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("maarse-data"));
    let saved = match settings::to_args(&settings::load(&data_dir)) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("Warning: {e} – ignoring {}", settings::file(&data_dir).display());
            Vec::new()
        }
    };
    let mut all = saved.clone();
    all.extend(cli.iter().cloned());
    let parsed = match parse_from(all) {
        Ok(c) => Ok(c),
        Err(e) if !saved.is_empty() => {
            eprintln!("Warning: the settings could not be used ({e}); starting without them");
            parse_from(cli)
        }
        Err(e) => Err(e),
    };
    let cfg = match parsed {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(2);
        }
    };
    settings::SERVICE.store(cfg.service, std::sync::atomic::Ordering::Relaxed);
    let (tx, rx) = std::sync::mpsc::channel();
    #[allow(unused_mut)]
    let mut node = match node::Node::new(&cfg) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("Startup error: {e}");
            std::process::exit(1);
        }
    };
    println!("Må:rse-host v{}", env!("CARGO_PKG_VERSION"));
    settings::report(format!("  Address:  {}", node.address()), false);
    settings::report(format!("  Data:     {}", cfg.data_dir.display()), false);

    if let Some(addr) = cfg.listen {
        match links::spawn_tcp_listener(addr, tx.clone()) {
            Ok(()) => settings::report(format!("  Backbone: listening on {addr}"), false),
            Err(e) => settings::report(format!("  Backbone: could not listen on {addr}: {e}"), true),
        }
    }
    for p in &cfg.peers {
        settings::report(format!("  Backbone: connecting to {p}"), false);
        links::spawn_tcp_peer(p.clone(), tx.clone());
    }
    if let Some(port) = &cfg.serial {
        settings::report(format!("  Serial:   {port} @ {} baud (modem mode)", cfg.baud), false);
        links::spawn_serial(port.clone(), cfg.baud, tx.clone());
    }
    if let Some(port) = &cfg.meshcore {
        settings::report(format!("  MeshCore: {port} @ {} baud (companion_radio_usb)", cfg.meshcore_baud), false);
        meshcore::spawn(port.clone(), cfg.meshcore_baud, cfg.meshcore_bridge.clone(), cfg.data_dir.clone(), tx.clone());
    }
    if let Some(m) = &cfg.mqtt {
        settings::report(format!("  MQTT:     {} (topics under {}/)", m.addr, m.prefix), false);
    }
    if let Some(addr) = cfg.http {
        let token = web::access_token(&cfg.data_dir);
        match web::spawn(addr, token.clone(), cfg.data_dir.clone(), tx.clone()) {
            Ok(()) => {
                let url = format!("http://{addr}/");
                settings::report(format!("  Web client: {url}"), false);
                if !addr.ip().is_loopback() {
                    let shown = if addr.ip().is_unspecified() { format!("<this-pc>:{}", addr.port()) } else { addr.to_string() };
                    println!("  Remote access: requires the access key (see {})", cfg.data_dir.join("access-key.txt").display());
                    println!("    Browser: http://{shown}/#token={token}");
                    println!("    App:     address {shown}, key {token}");
                }
                if cfg.open_browser && !settings::restarted() {
                    open_browser(&url);
                }
            }
            Err(e) => settings::report(format!("  Web client: could not start on {addr}: {e}"), true),
        }
    }
    settings::report(format!(
        "  IP:       {} (IP-tunnel{})",
        tunnel::ip_text(&node.my_ip()),
        if cfg.tun_lora { ", also over LoRa" } else { ", only over TCP" }
    ), false);
    if cfg.tun {
        #[cfg(feature = "tun")]
        match tunnel::spawn(node.my_ip(), cfg.wintun.clone(), tx.clone()) {
            Ok(t) => {
                node.set_tun(t);
                settings::TUN_UP.store(true, std::sync::atomic::Ordering::Relaxed);
                settings::report("  TUN:      adapter 'maarse' created (fd00::/8)", false);
            }
            Err(e) => settings::report(format!("  TUN:      could not create the adapter ({e}); requires administrator/root and wintun.dll on Windows"), true),
        }
        #[cfg(not(feature = "tun"))]
        settings::report("  TUN:      built without the 'tun' feature (cargo build --features tun)", true);
    }
    println!("Press Ctrl+C to stop.\n");
    if let Some(ep) = &cfg.public_addr {
        settings::report(format!("  DHT:      public as {ep}"), false);
    }
    node.run(rx, tx);
}

#[cfg(test)]
mod service_tests {
    #[test]
    fn service_flag_implies_no_browser() {
        let c = crate::parse_from(vec!["--service".into()]).unwrap();
        assert!(c.service);
        assert!(!c.open_browser);
        let d = crate::parse_from(Vec::new()).unwrap();
        assert!(!d.service && d.open_browser);
    }
}
