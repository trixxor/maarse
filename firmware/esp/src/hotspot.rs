//! Wi-Fi hotspot: the node opens its own Wi-Fi network, phones join it and use the node as a public repeater.
//!
//! Every phone keeps its own identity and runs its own host, which connects to the node as one more backbone
//! peer (the framing of `wifi.rs`: `length (u16 LE) | packet`, port [`hotspot::PORT`]). The node routes as it
//! always does:
//!
//! * what a phone sends goes into the node's router like a packet heard over a perfect radio link, and the node
//!   forwards it over LoRa when its role and the route say so;
//! * every packet the node transmits is also sent to every connected phone, so phones hear what the node forwards
//!   and what is addressed to them, and phones on the same hotspot reach each other without any LoRa airtime.
//!
//! Rules (decided 2026-11-27): phones are clients only. A packet a phone has passed on for someone else
//! (`hops > 0`) is dropped, and the radio's duty cycle is shared fairly: every connected phone fills a bucket of
//! airtime at its share of the duty cycle (see `maarse_core::hotspot`), and a phone that has used its share is not
//! read from until it has more (the TCP window holds the rest back). The Wi-Fi network is WPA2 protected.

extern crate alloc;

use alloc::boxed::Box;
use core::cell::RefCell;
use core::sync::atomic::{AtomicU8, Ordering::Relaxed};
use embassy_executor::Spawner;
use embassy_futures::select::{Either, Either3, select, select3};
use embassy_net::tcp::TcpSocket;
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_net::{IpAddress, IpEndpoint, Ipv4Address, Ipv4Cidr, Runner, Stack, StackResources, StaticConfigV4};
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Instant, Timer};
use esp_radio::wifi::ap::AccessPointConfig;
use esp_radio::wifi::{AuthenticationMethodConfig, Config, Interface, WifiController};
use maarse_core::MAX_PACKET;
use maarse_core::companion::{WifiSettings, wifi_state};
use maarse_core::hotspot::{self, Leases, MAX_CLIENTS, Shares};
use maarse_fw_common::bus::{NET_EVT, NET_RX, NET_STATS, NET_TX, Pkt, WIFI_IP, WIFI_STATE, radio_air};

/// Region greeting a host may send first (see maarse-host links.rs); skipped.
const REGION_HELLO: [u8; 3] = [0xFF, b'R', b'G'];

/// TCP buffer per direction for one phone (packets are at most 255 bytes).
const SOCKET_BUF: usize = 768;

/// What goes to one phone. The queues are made on the heap when the hotspot starts, not as statics: static memory
/// takes the main stack's room, and the Wi-Fi driver needs nearly all of that when it starts.
type Out = Channel<CriticalSectionRawMutex, Pkt, 2>;
static OUT_REFS: embassy_sync::once_lock::OnceLock<[&'static Out; MAX_CLIENTS]> = embassy_sync::once_lock::OnceLock::new();

/// Each phone's share of the airtime, kept when it connects again (on the
/// heap, like the queues).
type SharedAir = Mutex<CriticalSectionRawMutex, RefCell<Shares>>;
static SHARES: embassy_sync::once_lock::OnceLock<&'static SharedAir> = embassy_sync::once_lock::OnceLock::new();

/// Takes `cost_ms` of airtime for the phone at `host`, or says how long to wait.
fn take_air(host: u8, cost_ms: u32) -> Result<(), u32> {
    let (_, duty) = radio_air();
    let c = clients();
    SHARES.try_get().expect("hotspot shares").lock(|s| {
        let mut s = s.borrow_mut();
        if s.take(host, now_ms(), c, duty, cost_ms) { Ok(()) } else { Err(s.wait_ms(host, now_ms(), c, duty, cost_ms)) }
    })
}

fn out(i: usize) -> &'static Out {
    OUT_REFS.try_get().expect("hotspot queues")[i]
}
/// Bit `i` is set while phone slot `i` is connected.
static ACTIVE: AtomicU8 = AtomicU8::new(0);

fn clients() -> u32 {
    ACTIVE.load(Relaxed).count_ones()
}

fn refresh_state() {
    WIFI_STATE.store(if clients() > 0 { wifi_state::HOTSPOT_CLIENTS } else { wifi_state::HOTSPOT_OPEN } as u32, Relaxed);
}

fn now_ms() -> u64 {
    Instant::now().as_millis()
}

/// Starts the hotspot. Returns `false` if the settings are not usable or the driver fails.
pub fn start(spawner: &Spawner, wifi: esp_hal::peripherals::WIFI<'static>, settings: &WifiSettings, seed: u64) -> bool {
    let Some(ap) = access_point_config(settings) else {
        WIFI_STATE.store(wifi_state::BAD_SETTINGS as u32, Relaxed);
        return false;
    };
    let config = esp_radio::wifi::ControllerConfig::default()
        .with_static_rx_buf_num(3)
        .with_dynamic_rx_buf_num(4)
        .with_dynamic_tx_buf_num(4)
        .with_ampdu_rx_enable(false)
        .with_ampdu_tx_enable(false)
        .with_rx_ba_win(3);
    let Ok(controller) = WifiController::new(wifi, config) else {
        WIFI_STATE.store(wifi_state::DRIVER_FAILED as u32, Relaxed);
        return false;
    };
    let iface = Interface::access_point();
    let ip = hotspot::SERVER_IP;
    let net = embassy_net::Config::ipv4_static(StaticConfigV4 {
        address: Ipv4Cidr::new(Ipv4Address::new(ip[0], ip[1], ip[2], ip[3]), 24),
        gateway: None,
        dns_servers: Default::default(),
    });
    // Phones (MAX_CLIENTS sockets), the DHCP server, and a little room.
    let (stack, runner) = embassy_net::new(iface, net, crate::wifi::NET_RESOURCES.init(StackResources::new()), seed);
    // The queues to the phones live on the heap too (see below).
    let outs: [&'static Out; MAX_CLIENTS] = core::array::from_fn(|_| &*Box::leak(Box::new(Channel::new())));
    let _ = OUT_REFS.init(outs);
    let _ = SHARES.init(&*Box::leak(Box::new(Mutex::new(RefCell::new(Shares::new())))));
    WIFI_IP.store(u32::from_be_bytes(ip), Relaxed);
    WIFI_STATE.store(wifi_state::HOTSPOT_OPEN as u32, Relaxed);
    spawner.spawn(net_task(runner).unwrap());
    spawner.spawn(ap_task(controller, ap).unwrap());
    // The buffers live on the heap, not in static memory: the main stack is what is left of RAM after the statics, and
    // the Wi-Fi driver needs a good deal of it when it starts (with 27 KB of statics for the phones it crashed there).
    let dhcp_buf: &'static mut [u8] = Box::leak(alloc::vec![0u8; 2400].into_boxed_slice());
    spawner.spawn(dhcp_task(stack, dhcp_buf).unwrap());
    spawner.spawn(distribute_task().unwrap());
    for slot in 0..MAX_CLIENTS {
        let buf: &'static mut [u8] = Box::leak(alloc::vec![0u8; 2 * SOCKET_BUF].into_boxed_slice());
        spawner.spawn(client_task(stack, slot, buf).unwrap());
    }
    true
}

fn access_point_config(s: &WifiSettings) -> Option<Config> {
    let ssid = core::str::from_utf8(&s.ssid).ok()?.try_into().ok()?;
    // WPA2 needs 8 to 63 characters; an open hotspot is not offered.
    if !(8..=63).contains(&s.password.len()) {
        return None;
    }
    let auth = AuthenticationMethodConfig::Wpa2Personal(s.password.as_slice().try_into().ok()?);
    let ap = AccessPointConfig::default()
        .with_ssid(ssid)
        .with_authentication(auth)
        .with_max_connections(MAX_CLIENTS as u16);
    Some(Config::AccessPoint(ap))
}

#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, Interface>) -> ! {
    runner.run().await
}

/// Switches the access point on and keeps the driver alive.
#[embassy_executor::task]
async fn ap_task(mut controller: WifiController<'static>, config: Config) -> ! {
    if controller.set_config(&config).is_err() {
        WIFI_STATE.store(wifi_state::DRIVER_FAILED as u32, Relaxed);
    }
    core::future::pending().await
}

/// A small DHCP server: one address per phone, no router and no DNS (the hotspot has no internet).
#[embassy_executor::task]
async fn dhcp_task(stack: Stack<'static>, buf: &'static mut [u8]) -> ! {
    let mut rx_meta = [PacketMetadata::EMPTY; 2];
    let mut tx_meta = [PacketMetadata::EMPTY; 2];
    let (rx, tx) = buf.split_at_mut(1200);
    let mut socket = UdpSocket::new(stack, &mut rx_meta, rx, &mut tx_meta, tx);
    if socket.bind(67).is_err() {
        core::future::pending::<()>().await;
    }
    let mut leases = Leases::new();
    let mut req = [0u8; 600];
    let mut out = [0u8; 300];
    loop {
        let Ok((n, _)) = socket.recv_from(&mut req).await else { continue };
        if let Some(len) = hotspot::dhcp_reply(&req[..n], &mut leases, &mut out) {
            // A phone without an address cannot be reached any other way than by broadcast.
            let to = IpEndpoint::new(IpAddress::Ipv4(Ipv4Address::new(255, 255, 255, 255)), 68);
            let _ = socket.send_to(&out[..len], to).await;
        }
    }
}

/// Sends everything the node transmits to every connected phone; throws away what only a host would want
/// (counters and firmware-update replies).
#[embassy_executor::task]
async fn distribute_task() -> ! {
    loop {
        match select3(NET_TX.receive(), NET_STATS.receive(), NET_EVT.receive()).await {
            Either3::First((pkt, _snr)) => {
                let active = ACTIVE.load(Relaxed);
                for i in 0..MAX_CLIENTS {
                    if active & (1 << i) != 0 {
                        let _ = out(i).try_send(pkt.clone());
                    }
                }
            }
            Either3::Second(_) | Either3::Third(_) => {}
        }
    }
}

/// One phone slot: waits for a phone, then moves packets both ways until it goes away.
#[embassy_executor::task(pool_size = 4)]
async fn client_task(stack: Stack<'static>, slot: usize, buf: &'static mut [u8]) -> ! {
    let (rx_buf, tx_buf) = buf.split_at_mut(SOCKET_BUF);
    loop {
        let mut socket = TcpSocket::new(stack, &mut *rx_buf, &mut *tx_buf);
        socket.set_timeout(Some(Duration::from_secs(180)));
        socket.set_keep_alive(Some(Duration::from_secs(30)));
        if socket.accept(hotspot::PORT).await.is_ok() {
            ACTIVE.fetch_or(1 << slot, Relaxed);
            while out(slot).try_receive().is_ok() {}
            refresh_state();
            // The share follows the phone's address, not the connection.
            let host = socket.remote_endpoint().map_or(0, |e| match e.addr {
                IpAddress::Ipv4(a) => a.octets()[3],
                #[allow(unreachable_patterns)]
                _ => 0,
            });
            run_client(&mut socket, slot, host).await;
            ACTIVE.fetch_and(!(1 << slot), Relaxed);
            refresh_state();
            socket.abort();
            let _ = socket.flush().await;
        }
        Timer::after(Duration::from_millis(200)).await;
    }
}

async fn run_client(socket: &mut TcpSocket<'_>, slot: usize, host: u8) {
    let (mut reader, mut writer) = socket.split();
    // The host decides what a connection is from its first bytes: an empty region greeting says "a backbone peer,
    // send me everything".
    if write_all(&mut writer, &[5, 0, 0xFF, b'R', b'G', 0, 0]).await.is_none() {
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
            // Clients only: nothing a phone has passed on for others.
            if !hotspot::from_client_ok(&buf[..n]) {
                continue;
            }
            // A fair share of the radio: wait until this phone's bucket holds the packet's airtime.
            let cost = hotspot::estimate_air_ms(n, &radio_air().0);
            while let Err(wait) = take_air(host, cost) {
                Timer::after(Duration::from_millis(wait.clamp(50, 5_000) as u64)).await;
            }
            let Ok(p) = Pkt::from_slice(&buf[..n]) else { continue };
            // The other phones hear it at once, as on a radio; the node's router takes it from here.
            let active = ACTIVE.load(Relaxed);
            for i in 0..MAX_CLIENTS {
                if i != slot && active & (1 << i) != 0 {
                    let _ = out(i).try_send(p.clone());
                }
            }
            NET_RX.send(p).await;
        }
    };
    let tx = async {
        loop {
            let pkt = out(slot).receive().await;
            let len = (pkt.len() as u16).to_le_bytes();
            if write_all(&mut writer, &len).await.is_none() || write_all(&mut writer, &pkt).await.is_none() {
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
