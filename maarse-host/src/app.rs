// The program itself, shared by maarse-host (main.rs) and maarse-server
// (server.rs), which include it; `server_mode()` tells them apart.

// Every printed line is also kept in memory for the web client's Service page.
// These shadow the std macros for the whole crate (textual macro scope).
macro_rules! println {
    () => {{ std::println!(); crate::service::push("", false); }};
    ($($t:tt)*) => {{ let s = format!($($t)*); std::println!("{s}"); crate::service::push(&s, false); }};
}
macro_rules! eprintln {
    () => {{ std::eprintln!(); crate::service::push("", true); }};
    ($($t:tt)*) => {{ let s = format!($($t)*); std::eprintln!("{s}"); crate::service::push(&s, true); }};
}

mod admin;
mod debug;
mod telemetry;
mod dht;
mod guard;
mod extra_links;
mod filebox;
mod framing;
mod history;
#[cfg(test)]
mod fuzz_tests;
mod links;
mod meshcore;
mod mqtt;
mod node;
mod sessions;
mod service;
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
    /// A backbone peer's capabilities (from its link hello).
    LinkCaps {
        id: LinkId,
        caps: crate::dht::Caps,
    },
    /// A DHT node's capabilities (answer to `Req::Caps`).
    DhtCaps {
        id: maarse_core::Address,
        caps: Option<crate::dht::Caps>,
    },
    /// A status frame from a LoRa modem (the firmware's `Stats` or `Radio`
    /// event), polled by maarse-server for telemetry.
    ModemFrame {
        link: LinkId,
        frame: Vec<u8>,
    },
    /// Mail fetched from the replica hosts for `recipient` (post offices).
    MailFetched {
        recipient: maarse_core::Address,
        items: Vec<Vec<u8>>,
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
    /// Host-to-host serial lines (`--serial-link <port>[:baud]`).
    pub serial_links: Vec<(String, u32)>,
    /// KISS TNCs on serial ports (`--kiss <port>[:baud]`) and over TCP (`--kiss-tcp`).
    pub kiss: Vec<(String, u32)>,
    pub kiss_tcp: Vec<String>,
    pub kiss_opts: extra_links::KissOptions,
    /// UDP: our socket (`--udp-listen`), peers (`--udp-peer`) and LAN broadcast port.
    pub udp_listen: Option<SocketAddr>,
    pub udp_peers: Vec<String>,
    pub udp_broadcast: Option<u16>,
    pub open_browser: bool,
    /// `--require-key`: the access key also from 127.0.0.1.
    pub require_key: bool,
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
    /// DNS names that resolve to several backbone nodes (`--seed-dns`),
    /// resolved at start-up into peers.
    pub seed_dns: Vec<String>,
    /// Keep at least this many backbone links open, connecting to nodes
    /// from the DHT when there are fewer (`--min-peers`; backbone 6).
    pub min_peers: usize,
    /// Running as maarse-server (no radio, no web client by default,
    /// periodic status lines).
    pub backbone: bool,
    /// Do not keep mail for others (`--no-post-office`).
    pub no_post_office: bool,
    /// Passphrase Wi-Fi modems must prove (`--modem-key`).
    pub modem_key: Option<String>,
    /// Wi-Fi LoRa modems at fixed addresses to connect to (`--modem-ip`).
    pub modem_ips: Vec<String>,
    /// LoRa modems as a plain byte stream over TCP (`--modem-tcp`).
    pub modem_tcp: Vec<String>,
    /// Block list entries from the command line (`--block`): IP addresses,
    /// networks and node addresses.
    pub block: Vec<String>,
    /// Do not relay for hosts behind NAT (`--no-relay`).
    pub no_relay: bool,
    /// Most mail kept for others, MB (`--mail-quota-mb`; 0 = no limit).
    pub mail_quota_mb: u32,
    /// Regions from the command line (`--region`); empty = use the saved ones.
    pub regions: Vec<String>,
    pub strict_region: bool,
    /// Run as a plain repeater instead of an anchor (`--repeater-role`).
    pub repeater_role: bool,
    /// Long-distance LoRa trunks: LoRa modems with directional antennas
    /// (`--trunk <port>[:baud]`). Used only as a backup when TCP cannot reach
    /// the other end (see node.rs "Trunks").
    pub trunks: Vec<(String, u32)>,
    /// Re-flood every path request from TCP on the radio, as before
    /// (`--no-bordercast`).
    pub no_bordercast: bool,
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
    /// Identities whose signed bulletins we show (`--bulletin-key`).
    pub bulletin_keys: Vec<maarse_crypto::PublicIdentity>,
    /// Root keys whose alert issuer certificates we accept (`--alert-root`).
    pub alert_roots: Vec<maarse_crypto::PublicIdentity>,
    /// Our alert issuer key file (`--issuer-key`).
    pub issuer_key: Option<PathBuf>,
    /// Our issuer certificate file (`--issuer-cert`).
    pub issuer_cert: Option<PathBuf>,
}

/// One-shot key tools for alerts; `Some(exit code)` if `argv` asked for one.
fn alert_tools(argv: &[String]) -> Option<i32> {
    use maarse_crypto::{Identity, alert};
    use rand_core::OsRng;
    let run = || -> Result<(), String> {
        match argv.first().map(String::as_str) {
            Some("--new-alert-key") => {
                let path = argv.get(1).ok_or("--new-alert-key <file>")?;
                if std::path::Path::new(path).exists() {
                    return Err(format!("{path} exists; not overwritten"));
                }
                let id = Identity::generate(&mut OsRng);
                std::fs::write(path, id.to_secret_bytes()).map_err(|e| e.to_string())?;
                let hex: String = id.public().to_bytes().iter().map(|b| format!("{b:02x}")).collect();
                println!("Wrote {path}. Keep it offline. Public key:
{hex}");
                Ok(())
            }
            Some("--sign-issuer-cert") => {
                let [root, issuer, name, days, out] = argv.get(1..6).and_then(|v| <&[String; 5]>::try_from(v).ok()).ok_or(
                    "--sign-issuer-cert <root key file> <issuer public key hex> <name> <days> <out file> [events mask] [max severity]",
                )?;
                let k: [u8; 64] = std::fs::read(root).map_err(|e| e.to_string())?.try_into().map_err(|_| "root key: expected 64 bytes")?;
                let root = Identity::from_secret_bytes(&k);
                let days: u32 = days.parse().map_err(|_| "days: invalid number")?;
                let events = match argv.get(6) {
                    Some(m) => u16::from_str_radix(m.trim_start_matches("0x"), 16).map_err(|_| "events mask: hex, e.g. 0x0013")?,
                    None => 0xffff,
                };
                let max_severity = argv.get(7).map_or(Ok(3), |s| s.parse::<u8>()).map_err(|_| "max severity: 0-3")?.min(3);
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as u32);
                let cert = alert::Cert {
                    issuer: parse_identity(issuer)?,
                    not_after: now + days * 86_400,
                    events,
                    max_severity,
                    name: heapless::String::try_from(name.as_str()).map_err(|_| "name: at most 24 bytes")?,
                };
                let mut b = [0u8; 200];
                let n = alert::sign_cert(&root, &cert, &mut b).ok_or("could not sign")?;
                std::fs::write(out, &b[..n]).map_err(|e| e.to_string())?;
                println!("Wrote {out}: \"{name}\", valid {days} days, events {events:#06x}, max severity {max_severity}");
                Ok(())
            }
            _ => Err(String::new()),
        }
    };
    match run() {
        Ok(()) => Some(0),
        Err(e) if e.is_empty() => None,
        Err(e) => {
            eprintln!("{e}");
            Some(2)
        }
    }
}

/// Public backbone servers that `--public` connects to (maarse-server
/// instances run by the project). More are found through them (DHT).
pub const PUBLIC_PEERS: &[&str] = &["nj01.dk.eu.maarse.dk:4242"];

/// Parses a public identity given as 128 hex digits (signing key, then DH key).
fn parse_identity(s: &str) -> Result<maarse_crypto::PublicIdentity, String> {
    let s = s.trim();
    let bytes: Option<Vec<u8>> = (0..s.len())
        .step_by(2)
        .map(|i| s.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok()))
        .collect();
    match bytes {
        Some(b) if b.len() == 64 => {
            maarse_crypto::PublicIdentity::from_bytes(&b).ok_or_else(|| "--bulletin-key: invalid key".to_string())
        }
        _ => Err(format!("--bulletin-key: expected 128 hex digits, got '{s}'")),
    }
}

const HELP: &str = "\
maarse-host – gateway and test node for Må:rse

Usage: maarse-host [options]

  --data <folder>       where identity and settings are stored (default: maarse-data)
  --http <addr:port>    web client and WebSocket (default: 127.0.0.1:8080, 'off' disables)
  --listen <addr:port>  accept backbone connections over TCP (default: 0.0.0.0:4242, 'off' disables)
  --peer <host:port>    connect to another maarse-host (can be repeated)
  --serial <port>       LoRa node in modem mode, e.g. COM3 or /dev/ttyACM0
  --baud <rate>         serial baud rate (default: 115200)
  --serial-link <port>[:baud]
                        direct link to another maarse-host over a serial line
                        (null-modem cable, USB-serial, transparent radio modem);
                        default 115200 baud. At 230400 baud or more it counts
                        as backbone (like TCP), below that as a slow radio link
                        (can be repeated)
  --kiss <port>[:baud]  KISS TNC on a serial port, e.g. a packet radio TNC or
                        an RNode-like KISS device (default 9600 baud; can be
                        repeated). Counts as a slow radio link
  --kiss-tcp <host:port>
                        KISS TNC over TCP, e.g. Direwolf on port 8001
                        (reconnects every 5 s; can be repeated)
  --kiss-txdelay <N>    KISS TXDELAY sent to the TNC, in 10 ms units
  --kiss-persist <N>    KISS persistence (0-255)
  --kiss-slottime <N>   KISS slot time, in 10 ms units
  --kiss-callsign <CALL[-SSID]>
                        wrap KISS packets in AX.25 UI frames from this
                        callsign (to QST), for amateur-radio identification
  --udp-listen <addr:port>
                        UDP socket for backbone peers, e.g. 0.0.0.0:4243;
                        senders become peers and are forgotten after 3 minutes
                        of silence
  --udp-peer <host:port>
                        send to this UDP peer (can be repeated)
  --udp-broadcast <port>
                        find peers on the LAN: broadcast to this port and accept
                        packets from any sender on it
  --public              also connect to the public backbone servers
                        (nj01.dk.eu.maarse.dk:4242; more are found from there)
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
  --seed-dns <name[:port]>
                        a DNS name with several backbone nodes behind it;
                        all of them become peers (can be repeated)
  --set-admin-password <password>
                        maarse-server: set the dashboard's admin password and exit
  --no-post-office      do not keep mail for others (told to the backbone,
                        so no one picks this host as a replica)
  --modem-key <passphrase>
                        Wi-Fi LoRa modems must prove this key (set the same
                        passphrase in the modem); maarse-server accepts no
                        Wi-Fi modem without it (at least 12 characters)
  --modem-tcp <host:port> a LoRa modem as a plain byte stream over TCP (the
                        serial modem protocol; the Android app bridges it to
                        a radio over Bluetooth). Key in modem-tcp-key.txt.
  --modem-ip <host:port> connect to a Wi-Fi LoRa modem that listens at this
                        address (its host set to listen:<port>); can be repeated
  --block <ip|net|node> never accept this IP address, network (a.b.c.d/24) or
                        node address (32 hex); can be repeated; the dashboard
                        keeps its own list in blocked.txt
  --no-relay            do not relay for hosts behind NAT (only addresses
                        heard over our own radio are published)
  --mail-quota-mb <N>   keep at most N MB of mail for others (0 = no limit)
  --min-peers <N>       keep at least N backbone links, connecting to nodes
                        from the DHT when there are fewer (default 0;
                        maarse-server 6)
  --region <name>       region I forward channel messages for, e.g. dk or
                        nordjylland (can be repeated, at most 8). Without regions
                        everything is forwarded
  --strict-region       without regions: forward only messages without a region
  --trunk <port>[:baud] long-distance LoRa trunk: a LoRa node in modem mode with
                        a directional antenna; used only when TCP cannot reach
                        the other end, and only for what has to cross
  --repeater-role       route as a repeater, not an anchor (anchors split long
                        routes at the host, so a route may be 12 hops per side)
  --no-bordercast       put every path request from TCP on the radio (default:
                        only for nodes heard over our own radio, or on a retry)
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
  --bulletin-key <hex>  trust signed bulletins from this identity (128 hex
                        digits, printed by the sender's host at start-up; can
                        be repeated). Joins the #bulletin channel, where only
                        verified bulletins are shown
  --alert-root <hex>    accept alert issuer certificates signed by this root
                        key (128 hex digits; can be repeated)
  --issuer-key <file>   issue alerts with this key (kept apart from the node
                        identity; create one with --new-alert-key)
  --issuer-cert <file>  our issuer certificate, sent before our alerts
                        (from --sign-issuer-cert)
  --new-alert-key <file>
                        create an alert root or issuer key and exit
  --sign-issuer-cert <root key> <issuer hex> <name> <days> <out> [events] [max severity]
                        sign an issuer certificate with a root key and exit
  --no-browser          do not open the browser automatically
  --require-key         ask for the access key from this machine too (for
                        phones and shared computers, where other programs
                        can reach 127.0.0.1)
  --service             run under a service manager (systemd, Task Scheduler):
                        implies --no-browser; a settings restart exits with
                        code 75 and leaves the restart to the supervisor
  --help                show this text

Example with two computers:
  computer A:  maarse-host
  computer B:  maarse-host --peer <A's IP>:4242
";

/// Are we maarse-server? Decided by the program's name, so one code base
/// gives two programs with their own defaults.
pub fn server_mode() -> bool {
    option_env!("CARGO_BIN_NAME") == Some("maarse-server") || SERVER_MODE.load(Ordering::Relaxed)
}

/// Server mode chosen at run time, for the library build (the Android app),
/// where there is no program name to go by.
static SERVER_MODE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Runs the program as maarse-server instead of maarse-host (library use).
pub fn set_server_mode(on: bool) {
    SERVER_MODE.store(on, Ordering::Relaxed);
}

/// Default data folder for this program.
pub fn default_data_dir() -> PathBuf {
    PathBuf::from(if server_mode() { "maarse-server-data" } else { "maarse-data" })
}

pub fn parse_from(argv: Vec<String>) -> Result<Config, String> {
    let backbone = server_mode();
    let mut c = Config {
        data_dir: default_data_dir(),
        // The server's dashboard is reached from elsewhere, behind its login.
        http: Some(if backbone { "0.0.0.0:8080" } else { "127.0.0.1:8080" }.parse().unwrap()),
        listen: Some("0.0.0.0:4242".parse().unwrap()),
        peers: Vec::new(),
        serial: None,
        baud: 115_200,
        serial_links: Vec::new(),
        kiss: Vec::new(),
        kiss_tcp: Vec::new(),
        kiss_opts: Default::default(),
        udp_listen: None,
        udp_peers: Vec::new(),
        udp_broadcast: None,
        open_browser: true,
        require_key: false,
        max_file: 1024 * 1024,
        filebox_per_recipient: 20 << 20,
        filebox_total: 200 << 20,
        postbox: None,
        public_addr: None,
        dht_seeds: Vec::new(),
        seed_dns: Vec::new(),
        min_peers: if backbone { 6 } else { 0 },
        backbone,
        no_post_office: false,
        no_relay: false,
        mail_quota_mb: 0,
        modem_key: None,
        modem_ips: Vec::new(),
        modem_tcp: Vec::new(),
        block: Vec::new(),
        regions: Vec::new(),
        strict_region: false,
        repeater_role: false,
        trunks: Vec::new(),
        no_bordercast: false,
        meshcore: None,
        meshcore_baud: 115_200,
        meshcore_bridge: Vec::new(),
        mqtt: None,
        history_days: Some(90),
        tun: false,
        wintun: None,
        tun_lora: false,
        service: false,
        bulletin_keys: Vec::new(),
        alert_roots: Vec::new(),
        issuer_key: None,
        issuer_cert: None,
    };
    let mut mqtt_prefix = "maarse".to_string();
    let (mut mqtt_user, mut mqtt_pass) = (None, None);
    let mut args = argv.into_iter();
    while let Some(a) = args.next() {
        // A value never starts with "--": "--peer --other" is a missing
        // value, not a peer called "--other".
        let mut val = |name: &str| match args.next() {
            Some(v) if !v.starts_with("--") => Ok(v),
            _ => Err(format!("{name} is missing a value")),
        };
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
            "--serial-link" => c.serial_links.push(extra_links::parse_port_baud(&val("--serial-link")?, 115_200)?),
            "--kiss" => c.kiss.push(extra_links::parse_port_baud(&val("--kiss")?, 9600)?),
            "--kiss-tcp" => c.kiss_tcp.push(val("--kiss-tcp")?),
            "--kiss-txdelay" => c.kiss_opts.txdelay = Some(val("--kiss-txdelay")?.parse().map_err(|_| "--kiss-txdelay: expected 0-255")?),
            "--kiss-persist" => c.kiss_opts.persist = Some(val("--kiss-persist")?.parse().map_err(|_| "--kiss-persist: expected 0-255")?),
            "--kiss-slottime" => c.kiss_opts.slottime = Some(val("--kiss-slottime")?.parse().map_err(|_| "--kiss-slottime: expected 0-255")?),
            "--kiss-callsign" => {
                c.kiss_opts.callsign = Some(framing::Callsign::parse(&val("--kiss-callsign")?).map_err(|e| format!("--kiss-callsign: {e}"))?)
            }
            "--udp-listen" => c.udp_listen = addr(val("--udp-listen")?, "--udp-listen")?,
            "--udp-peer" => c.udp_peers.push(val("--udp-peer")?),
            "--udp-broadcast" => c.udp_broadcast = Some(val("--udp-broadcast")?.parse().map_err(|_| "--udp-broadcast: invalid port")?),
            "--no-browser" => c.open_browser = false,
            "--require-key" => c.require_key = true,
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
            "--bulletin-key" => c.bulletin_keys.push(parse_identity(&val("--bulletin-key")?)?),
            "--alert-root" => c.alert_roots.push(parse_identity(&val("--alert-root")?)?),
            "--issuer-key" => c.issuer_key = Some(PathBuf::from(val("--issuer-key")?)),
            "--issuer-cert" => c.issuer_cert = Some(PathBuf::from(val("--issuer-cert")?)),
            "--dht" => c.dht_seeds.push(val("--dht")?),
            "--seed-dns" => c.seed_dns.push(val("--seed-dns")?),
            "--no-post-office" => c.no_post_office = true,
            "--no-relay" => c.no_relay = true,
            "--modem-tcp" => c.modem_tcp.push(val("--modem-tcp")?),
            "--modem-ip" => {
                let v = val("--modem-ip")?;
                if !v.contains(':') {
                    return Err("--modem-ip: host:port, e.g. 192.168.1.50:4243".into());
                }
                c.modem_ips.push(v);
            }
            "--block" => {
                let v = val("--block")?;
                if guard::Net::parse(&v).is_none() && !(v.len() == 32 && v.chars().all(|c| c.is_ascii_hexdigit())) {
                    return Err(format!("--block: not an IP address, network (a.b.c.d/24) or node address: {v}"));
                }
                c.block.push(v);
            }
            "--modem-key" => {
                let k = val("--modem-key")?;
                if k.len() < 12 {
                    return Err("--modem-key: at least 12 characters".into());
                }
                c.modem_key = Some(k);
            }
            "--mail-quota-mb" => c.mail_quota_mb = val("--mail-quota-mb")?.parse().map_err(|_| "--mail-quota-mb: invalid number")?,
            "--min-peers" => c.min_peers = val("--min-peers")?.parse().map_err(|_| "--min-peers: invalid number")?,
            "--region" => c.regions.push(val("--region")?.trim().to_lowercase()),
            "--strict-region" => c.strict_region = true,
            "--repeater-role" => c.repeater_role = true,
            "--trunk" => {
                let v = val("--trunk")?;
                let (port, baud) = match v.rsplit_once(':') {
                    Some((p, b)) if b.chars().all(|c| c.is_ascii_digit()) && !b.is_empty() => (p.to_string(), b.parse().unwrap_or(115200)),
                    _ => (v, 115200),
                };
                c.trunks.push((port, baud));
            }
            "--no-bordercast" => c.no_bordercast = true,
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
    if c.backbone {
        // A backbone has no browser to open and needs an address others can
        // reach, or it can neither be found nor relay.
        c.open_browser = false;
        if c.public_addr.is_none() {
            return Err("maarse-server needs --public-addr <host:port>: the address and port (default 4242) others reach it on".into());
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

/// The command line given to [`run`]. In the Android app the process's own
/// arguments are the Java runtime's, so they cannot be read back from std.
static CLI: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();

pub fn cli_args() -> Vec<String> {
    CLI.get().cloned().unwrap_or_else(|| std::env::args().skip(1).collect())
}

#[allow(dead_code)] // not used by the library build
fn main() {
    run(std::env::args().skip(1).collect());
}

/// The whole program with these arguments (without the program name). Never
/// returns: it runs the node until the process ends. Used by `main` and by
/// the Android app, which runs it in a process of its own.
pub fn run(cli: Vec<String>) {
    let _ = CLI.set(cli.clone());
    service::mark_start();
    // Settings from the web client (<data>/settings.txt) first, then the
    // command line, which therefore wins.
    if let Some(code) = alert_tools(&cli) {
        std::process::exit(code);
    }
    if server_mode() && cli.first().map(String::as_str) == Some("--set-admin-password") {
        let dir = cli.iter().position(|a| a == "--data").and_then(|i| cli.get(i + 1)).map(PathBuf::from).unwrap_or_else(default_data_dir);
        let Some(pw) = cli.get(1).filter(|p| p.len() >= 8 && !p.starts_with("--")) else {
            eprintln!("--set-admin-password <password> [--data <folder>]: at least 8 characters");
            std::process::exit(2);
        };
        match admin::set_password(&dir, pw) {
            Ok(()) => {
                let _ = std::fs::remove_file(dir.join("admin-password.txt"));
                println!("Admin password set in {}", dir.join("admin.hash").display());
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("Could not save the password: {e}");
                std::process::exit(1);
            }
        }
    }
    let data_dir = cli
        .iter()
        .position(|a| a == "--data")
        .and_then(|i| cli.get(i + 1))
        .map(PathBuf::from)
        .unwrap_or_else(default_data_dir);
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
    let mut cfg = match parsed {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(2);
        }
    };
    // DNS seeds: one name, several backbone nodes (as Bitcoin's DNS seeds).
    for name in cfg.seed_dns.clone() {
        let found = links::resolve_seeds(&name);
        if found.is_empty() {
            eprintln!("Warning: --seed-dns {name}: no addresses found");
        }
        for ep in found {
            if !cfg.peers.contains(&ep) && cfg.public_addr.as_deref() != Some(ep.as_str()) {
                cfg.peers.push(ep);
            }
        }
    }
    settings::SERVICE.store(cfg.service, std::sync::atomic::Ordering::Relaxed);
    if cfg.backbone {
        debug::enable();
        admin::load_sessions(&cfg.data_dir);
        if let Some(pw) = admin::ensure_password(&cfg.data_dir) {
            println!("  Admin password (first start; also in {}): {pw}", cfg.data_dir.join("admin-password.txt").display());
        }
    }
    let (tx, rx) = std::sync::mpsc::channel();
    #[allow(unused_mut)]
    let mut node = match node::Node::new(&cfg) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("Startup error: {e}");
            std::process::exit(1);
        }
    };
    println!("{} v{}", if cfg.backbone { "Må:rse-server" } else { "Må:rse-host" }, env!("CARGO_PKG_VERSION"));
    settings::report(format!("  Address:  {}", node.address()), false);
    // A LoRa modem shows the address of the host it works for.
    let _ = links::HOST_ADDR.set(node.address().0);
    settings::report(format!("  Data:     {}", cfg.data_dir.display()), false);
    let key: String = node.public_identity().to_bytes().iter().map(|b| format!("{b:02x}")).collect();
    settings::report(format!("  Bulletin key (for --bulletin-key on receivers): {key}"), false);
    if !cfg.bulletin_keys.is_empty() {
        settings::report(format!("  Bulletins: trusting {} key(s) on #bulletin", cfg.bulletin_keys.len()), false);
    }
    if !cfg.alert_roots.is_empty() {
        settings::report(format!("  Alerts: accepting certificates from {} root key(s)", cfg.alert_roots.len()), false);
    }
    if let Some(p) = node.issuer_public() {
        let hex: String = p.to_bytes().iter().map(|b| format!("{b:02x}")).collect();
        settings::report(format!("  Alert issuer key: {hex}"), false);
    }

    guard::init(&cfg.data_dir, &cfg.block);
    // Wi-Fi modems must prove the key; a server takes none without one.
    links::set_modem_auth(cfg.backbone, cfg.modem_key.as_deref().map(|k| maarse_core::modemauth::key(k.as_bytes())));
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
    for m in &cfg.modem_tcp {
        settings::report(format!("  LoRa modem: {m} (byte stream, e.g. the app's Bluetooth bridge)"), false);
        let key = std::fs::read_to_string(cfg.data_dir.join("modem-tcp-key.txt")).ok().map(|k| k.trim().to_string()).filter(|k| !k.is_empty());
        links::spawn_modem_tcp(m.clone(), key, tx.clone());
    }
    for m in &cfg.modem_ips {
        settings::report(format!("  Wi-Fi modem: connecting to {m}"), false);
        links::spawn_modem_dial(m.clone(), tx.clone());
    }
    if let Some(port) = &cfg.serial {
        settings::report(format!("  Serial:   {port} @ {} baud (modem mode)", cfg.baud), false);
        links::spawn_serial(port.clone(), cfg.baud, tx.clone());
    }
    for (port, baud) in &cfg.trunks {
        settings::report(format!("  Trunk:    {port} @ {baud} baud (LoRa modem, backup only)"), false);
        links::spawn_serial_named(port.clone(), *baud, format!("trunk {port}"), tx.clone());
    }
    for (port, baud) in &cfg.serial_links {
        let class = if *baud >= extra_links::FAST_SERIAL_BAUD { "backbone" } else { "slow link" };
        settings::report(format!("  Serial link: {port} @ {baud} baud ({class})"), false);
        extra_links::spawn_serial_link(port.clone(), *baud, tx.clone());
    }
    for (port, baud) in &cfg.kiss {
        settings::report(format!("  KISS TNC: {port} @ {baud} baud"), false);
        extra_links::spawn_kiss(port.clone(), *baud, cfg.kiss_opts.clone(), tx.clone());
    }
    for addr in &cfg.kiss_tcp {
        settings::report(format!("  KISS TNC: tcp {addr}"), false);
        extra_links::spawn_kiss_tcp(addr.clone(), cfg.kiss_opts.clone(), tx.clone());
    }
    let udp_target = |a: &String| extra_links::UdpTarget { name: format!("udp {a}"), addr: a.clone() };
    if let Some(port) = cfg.udp_broadcast {
        let bind: SocketAddr = ([0, 0, 0, 0], port).into();
        let target = extra_links::UdpTarget { name: format!("udp broadcast {port}"), addr: format!("255.255.255.255:{port}") };
        // The peers ride on the broadcast socket unless --udp-listen gives them their own.
        let mut targets = vec![target];
        if cfg.udp_listen.is_none() {
            targets.extend(cfg.udp_peers.iter().map(udp_target));
        }
        match extra_links::spawn_udp(bind, true, targets, tx.clone()) {
            Ok(()) => settings::report(format!("  UDP:      LAN broadcast on port {port}"), false),
            Err(e) => settings::report(format!("  UDP:      could not use port {port}: {e}"), true),
        }
    }
    if cfg.udp_listen.is_some() || (cfg.udp_broadcast.is_none() && !cfg.udp_peers.is_empty()) {
        let bind = cfg.udp_listen.unwrap_or_else(|| ([0, 0, 0, 0], 0).into());
        match extra_links::spawn_udp(bind, false, cfg.udp_peers.iter().map(udp_target).collect(), tx.clone()) {
            Ok(()) => settings::report(format!("  UDP:      {bind}, {} peer(s)", cfg.udp_peers.len()), false),
            Err(e) => settings::report(format!("  UDP:      could not bind {bind}: {e}"), true),
        }
    }
    if let Some(port) = &cfg.meshcore {
        settings::report(format!("  MeshCore: {port} @ {} baud (companion_radio_usb)", cfg.meshcore_baud), false);
        meshcore::spawn(port.clone(), cfg.meshcore_baud, cfg.meshcore_bridge.clone(), cfg.data_dir.clone(), tx.clone());
    }
    if let Some(m) = &cfg.mqtt {
        settings::report(format!("  MQTT:     {} (topics under {}/)", m.addr, m.prefix), false);
    }
    web::REQUIRE_KEY.store(cfg.require_key, std::sync::atomic::Ordering::Relaxed);
    if let Some(addr) = cfg.http {
        let token = web::access_token(&cfg.data_dir);
        match web::spawn(addr, token.clone(), cfg.data_dir.clone(), tx.clone()) {
            Ok(()) => {
                let url = format!("http://{addr}/");
                if cfg.backbone {
                    settings::report(format!("  Dashboard: {url} (admin login)"), false);
                } else {
                    settings::report(format!("  Web client: {url}"), false);
                }
                if !addr.ip().is_loopback() && !cfg.backbone {
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
    fn a_missing_value_is_not_the_next_flag() {
        assert!(crate::parse_from(vec!["--peer".into(), "--no-relay".into()]).is_err());
        assert!(crate::parse_from(vec!["--peer".into(), "a.example:4242".into(), "--no-relay".into()]).is_ok());
    }

    #[test]
    fn service_flag_implies_no_browser() {
        let c = crate::parse_from(vec!["--service".into()]).unwrap();
        assert!(c.service);
        assert!(!c.open_browser);
        let d = crate::parse_from(Vec::new()).unwrap();
        assert!(!d.service && d.open_browser);
    }
}
