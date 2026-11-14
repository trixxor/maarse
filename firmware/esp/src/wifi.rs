//! Wi-Fi backbone link (ESP32-S3).
//!
//! When Wi-Fi is enabled in the saved settings, the node joins the network
//! and keeps one TCP connection to a maarse-host (`peer`, `host:port`), framed
//! like the hosts' backbone: `length (u16 LE) | packet`. The node connects
//! out, so it works behind a phone hotspot (NAT); the host needs `--listen`
//! (default `0.0.0.0:4242`) reachable from the network, e.g. over a VPN or a
//! forwarded port.
//!
//! Packets flow to and from the node through `bus::NET_RX` / `bus::NET_TX`;
//! the node sends every packet both over LoRa and over this link, and the
//! router's duplicate filter stops loops. Settings change only at start:
//! `SetWifi` saves them and restarts the board.

use core::sync::atomic::Ordering::Relaxed;
use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_net::dns::DnsQueryType;
use embassy_net::tcp::TcpSocket;
use embassy_net::{IpAddress, Ipv4Address, Runner, Stack, StackResources};
use embassy_time::{Duration, Timer};
use esp_radio::wifi::sta::StationConfig;
use esp_radio::wifi::{AuthenticationMethodConfig, Config, Interface, WifiController};
use maarse_core::MAX_PACKET;
use maarse_core::companion::{WifiSettings, wifi_state};
use maarse_fw_common::bus::{NET_RX, NET_TX, Pkt, WIFI_IP, WIFI_STATE};
use static_cell::StaticCell;

/// Port used when `peer` has none (maarse-host's default `--listen`).
const DEFAULT_PORT: u16 = 4242;
/// Pause before trying again after a failed join or a lost link.
const RETRY: Duration = Duration::from_secs(5);
/// Region greeting a host may send first (see maarse-host links.rs); skipped.
const REGION_HELLO: [u8; 3] = [0xFF, b'R', b'G'];

fn set_state(s: u8) {
    WIFI_STATE.store(s as u32, Relaxed);
}

/// Starts Wi-Fi if it is enabled in `settings`. Returns `false` if it is
/// off or could not be started (the node then runs on LoRa only).
pub fn start(spawner: &Spawner, wifi: esp_hal::peripherals::WIFI<'static>, settings: WifiSettings, seed: u64) -> bool {
    set_state(wifi_state::OFF);
    if !settings.enabled || settings.ssid.is_empty() || settings.peer.is_empty() {
        return false;
    }
    let Some(station) = station_config(&settings) else {
        set_state(wifi_state::BAD_SETTINGS);
        return false;
    };
    // Few buffers: the link carries LoRa-sized packets (< 256 bytes), so
    // throughput does not matter, and RAM is shared with Bluetooth.
    let config = esp_radio::wifi::ControllerConfig::default()
        .with_static_rx_buf_num(4)
        .with_dynamic_rx_buf_num(8)
        .with_dynamic_tx_buf_num(8)
        .with_ampdu_rx_enable(false)
        .with_ampdu_tx_enable(false)
        .with_rx_ba_win(3);
    let Ok(controller) = WifiController::new(wifi, config) else {
        set_state(wifi_state::DRIVER_FAILED);
        return false;
    };
    let iface = Interface::station();
    static RESOURCES: StaticCell<StackResources<3>> = StaticCell::new();
    let (stack, runner) = embassy_net::new(
        iface,
        embassy_net::Config::dhcpv4(Default::default()),
        RESOURCES.init(StackResources::new()),
        seed,
    );
    spawner.spawn(net_task(runner).unwrap());
    spawner.spawn(join_task(controller, station).unwrap());
    spawner.spawn(link_task(stack, settings.peer, settings.modem).unwrap());
    true
}

fn station_config(s: &WifiSettings) -> Option<Config> {
    let ssid = core::str::from_utf8(&s.ssid).ok()?.try_into().ok()?;
    let auth = if s.password.is_empty() {
        AuthenticationMethodConfig::Open
    } else {
        // Accepts WPA and WPA2 personal (phone hotspots use WPA2).
        AuthenticationMethodConfig::WpaWpa2Personal(s.password.as_slice().try_into().ok()?)
    };
    Some(Config::Station(StationConfig::default().with_ssid(ssid).with_authentication(auth)))
}

#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, Interface>) -> ! {
    runner.run().await
}

/// Joins the network and joins again whenever the connection drops.
#[embassy_executor::task]
async fn join_task(mut controller: WifiController<'static>, config: Config) -> ! {
    loop {
        set_state(wifi_state::JOINING);
        WIFI_IP.store(0, Relaxed);
        if controller.set_config(&config).is_ok() && controller.connect_async().await.is_ok() {
            let _ = controller.wait_for_disconnect_async().await;
        }
        Timer::after(RETRY).await;
    }
}

/// Splits `host:port` (port optional).
fn split_peer(peer: &str) -> (&str, u16) {
    match peer.rsplit_once(':') {
        Some((h, p)) => match p.parse() {
            Ok(port) => (h, port),
            Err(_) => (peer, DEFAULT_PORT),
        },
        None => (peer, DEFAULT_PORT),
    }
}

/// Keeps the TCP backbone link to the peer up.
#[embassy_executor::task]
async fn link_task(stack: Stack<'static>, peer: heapless::Vec<u8, 64>, modem: bool) -> ! {
    static RX: StaticCell<[u8; 2048]> = StaticCell::new();
    static TX: StaticCell<[u8; 2048]> = StaticCell::new();
    let rx = RX.init([0; 2048]);
    let tx = TX.init([0; 2048]);
    let peer = core::str::from_utf8(&peer).unwrap_or("");
    let (host, port) = split_peer(peer.trim());
    loop {
        stack.wait_config_up().await;
        if let Some(cfg) = stack.config_v4() {
            WIFI_IP.store(u32::from_be_bytes(cfg.address.address().octets()), Relaxed);
        }
        set_state(wifi_state::ONLINE);
        if let Some(ip) = resolve(stack, host).await {
            let mut socket = TcpSocket::new(stack, rx, tx);
            socket.set_timeout(Some(Duration::from_secs(180)));
            socket.set_keep_alive(Some(Duration::from_secs(30)));
            if socket.connect((ip, port)).await.is_ok() {
                set_state(wifi_state::LINKED);
                run_link(&mut socket, modem).await;
                socket.abort();
                let _ = socket.flush().await;
            }
        }
        // Packets queued while the link was down are stale.
        while NET_TX.try_receive().is_ok() {}
        if WIFI_STATE.load(Relaxed) == wifi_state::LINKED as u32 {
            set_state(wifi_state::ONLINE);
        }
        Timer::after(RETRY).await;
    }
}

async fn resolve(stack: Stack<'static>, host: &str) -> Option<IpAddress> {
    if let Ok(ip) = host.parse::<Ipv4Address>() {
        return Some(IpAddress::Ipv4(ip));
    }
    stack.dns_query(host, DnsQueryType::A).await.ok()?.first().copied()
}

/// Greeting that tells the host this connection is a LoRa modem, not a
/// backbone peer (as a length it would be 19 789, larger than any packet).
const MODEM_HELLO: [u8; 2] = *b"MM";

/// Moves packets both ways until the connection fails.
///
/// Backbone (node mode): `length (u16 LE) | packet` both ways.
/// Modem mode: after `MM`, to the host `length | snr (i8) | packet` (heard
/// over LoRa), from the host `length | packet` (to send over LoRa).
async fn run_link(socket: &mut TcpSocket<'_>, modem: bool) {
    let (mut reader, mut writer) = socket.split();
    // Say something at once: the host decides what a connection is from its
    // first two bytes, and a node with nothing to send would otherwise not
    // show up there until its next announce. As a backbone peer we send an
    // empty region greeting (`0xFF 'R' 'G' | strict | count`: no regions, so
    // the host sends us everything).
    let hello: &[u8] = if modem { &MODEM_HELLO } else { &[5, 0, 0xFF, b'R', b'G', 0, 0] };
    if write_all(&mut writer, hello).await.is_none() {
        return;
    }
    let rx = async {
        let mut len = [0u8; 2];
        let mut buf = [0u8; MAX_PACKET];
        loop {
            if read_exact(&mut reader, &mut len).await.is_none() {
                return;
            }
            let n = u16::from_le_bytes(len) as usize;
            if n == 0 || n > MAX_PACKET {
                return; // Not a backbone stream.
            }
            if read_exact(&mut reader, &mut buf[..n]).await.is_none() {
                return;
            }
            if buf[..n].starts_with(&REGION_HELLO) {
                continue;
            }
            if let Ok(p) = Pkt::from_slice(&buf[..n]) {
                NET_RX.send(p).await;
            }
        }
    };
    let tx = async {
        loop {
            let (pkt, snr) = NET_TX.receive().await;
            let ok = if modem {
                let len = (pkt.len() as u16 + 1).to_le_bytes();
                write_all(&mut writer, &len).await.is_some()
                    && write_all(&mut writer, &[snr as u8]).await.is_some()
                    && write_all(&mut writer, &pkt).await.is_some()
            } else {
                let len = (pkt.len() as u16).to_le_bytes();
                write_all(&mut writer, &len).await.is_some() && write_all(&mut writer, &pkt).await.is_some()
            };
            if !ok {
                return;
            }
        }
    };
    match select(rx, tx).await {
        Either::First(()) | Either::Second(()) => {}
    }
}

async fn read_exact(r: &mut embassy_net::tcp::TcpReader<'_>, mut buf: &mut [u8]) -> Option<()> {
    while !buf.is_empty() {
        match r.read(buf).await {
            Ok(0) | Err(_) => return None,
            Ok(n) => buf = &mut buf[n..],
        }
    }
    Some(())
}

async fn write_all(w: &mut embassy_net::tcp::TcpWriter<'_>, mut buf: &[u8]) -> Option<()> {
    while !buf.is_empty() {
        match w.write(buf).await {
            Ok(0) | Err(_) => return None,
            Ok(n) => buf = &buf[n..],
        }
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::split_peer;

    #[test]
    fn peers() {
        assert_eq!(split_peer("10.8.0.2:4242"), ("10.8.0.2", 4242));
        assert_eq!(split_peer("example.org"), ("example.org", 4242));
        assert_eq!(split_peer("example.org:9"), ("example.org", 9));
    }
}
