//! The node: routing, encryption, announces and companion commands.
//!
//! Modem mode: when a maarse-host sends the frame `0x41`, the host takes over
//! routing, and the node is only a radio. `0x40 | packet` is sent over LoRa, and
//! everything received is sent to the host as `0xC0 | snr | packet`.

mod ext;
mod identity;
mod range;

use crate::bus::{
    Frame, Pkt, RADIO_CANCEL, RADIO_CFG, RADIO_RX, RADIO_TX, RSTATS, TxReq, USB_IN, USB_OUT,
};
use core::sync::atomic::Ordering::Relaxed;
use embassy_futures::select::{Either, Either4, select, select4};
use embassy_futures::yield_now;
use embassy_time::{Duration, Instant, Timer};
use heapless::{FnvIndexMap, FnvIndexSet};
use rand_core::{CryptoRng, RngCore};
use maarse_core::companion::{Command, Event, PROTOCOL_VERSION, error_code, text};
use maarse_core::companion::{RadioConfig, Stats, stat};
use maarse_core::content::{self, Content};
use maarse_core::packet::{Header, encode};
use maarse_core::stamp::StampSearch;
use maarse_core::{Action, Address, PacketType, Role, Router};
use maarse_crypto::ratchet::{self, FixedStore, Format, MAX_FS_TEXT};
use maarse_crypto::seal::{self, CryptoError};
use maarse_crypto::{
    Identity, PublicIdentity, channel_address, hashtag_key, public_channel_key, verify_announce,
};

mod files;

const ANNOUNCE_EVERY: Duration = Duration::from_secs(900);
/// Attempts per slice when an announce stamp is computed. 256 SHA-256 blocks
/// take about 20 ms on a 64 MHz Cortex-M4; then the executor gets a turn.
const STAMP_CHUNK: u32 = 256;
/// Forward secrecy sessions in RAM (about 650 bytes each). They are not stored in
/// flash: flash is not erased securely (old keys could be recovered) and
/// wears from frequent writes. After a restart the node starts a new session
/// when it gets a message it cannot read (see `resync`).
const SESSIONS: usize = 8;
/// A new session because of an unreadable message at most this often per contact.
const RESYNC_EVERY: Duration = Duration::from_secs(60);

type Name = heapless::String<40>;
    /// Region names, comma separated (e.g. `dk,nordjylland`).
pub type RegionNames = heapless::Vec<u8, 120>;

/// SNR given to packets from the Wi-Fi backbone link (same as maarse-host's TCP).
const NET_SNR: i8 = 10;

/// Shown in a conversation when a message from that contact could not be
/// decrypted (it arrived too late or the session was lost).
const UNREADABLE_NOTICE: &str =
    "[A message from this contact could not be decrypted. A new encryption session was started; ask them to send it again.]";

/// Copies per message a relay carries (see `apply_role`).
const RELAY_CARRY_COPIES: u8 = 4;

/// Where radio settings are stored (flash on the board).
pub trait Settings {
    fn save_radio(&mut self, cfg: &RadioConfig);
    fn save_admins(&mut self, admins: &[Address]);
    fn load_admins(&mut self) -> heapless::Vec<Address, 4>;
    /// Stores the node's role. Default: not stored.
    fn save_role(&mut self, _role: Role) {}
    /// Stored role; the default (and older flash without a role) is repeater.
    fn load_role(&mut self) -> Role {
        Role::Repeater
    }
    /// Stores the region list and `strict`. Default: not stored.
    fn save_regions(&mut self, _strict: bool, _names: &[u8]) {}
    /// Stored regions; the default (and older flash) is none.
    fn load_regions(&mut self) -> (bool, RegionNames) {
        (false, RegionNames::new())
    }
    /// Replaces the stored identity (64 secret bytes). Returns
    /// `false` if it could not be stored. Default: cannot.
    fn save_identity(&mut self, _secret: &[u8; 64]) -> bool {
        false
    }
    /// Stores sharing settings (position/telemetry). Default: not stored.
    fn save_sharing(&mut self, _s: &maarse_core::companion::Sharing) {}
    /// Stored sharing settings; the default is no sharing.
    fn load_sharing(&mut self) -> maarse_core::companion::Sharing {
        Default::default()
    }
    /// Stores the display name (empty = none). Default: not stored.
    fn save_name(&mut self, _name: &[u8]) {}
    /// Stored display name; the default is none.
    fn load_name(&mut self) -> maarse_core::appdata::Name {
        Default::default()
    }
    /// Saves the Wi-Fi backbone settings. Default: not saved.
    fn save_wifi(&mut self, _w: &maarse_core::companion::WifiSettings) {}
    /// Saved Wi-Fi backbone settings; default is off.
    fn load_wifi(&mut self) -> maarse_core::companion::WifiSettings {
        Default::default()
    }
    /// Stores a range test setting that should survive a restart (mode off =
    /// none). Default: not stored.
    fn save_range(&mut self, _cfg: &maarse_core::rangetest::Config) {}
    /// Stored range test setting; the default is off.
    fn load_range(&mut self) -> maarse_core::rangetest::Config {
        Default::default()
    }

    // ----- mailbox (optional; default: none) -----

    /// Stores an encrypted message for `recipient`. Returns an ID.
    fn mail_add(&mut self, _recipient: &Address, _sealed: &[u8]) -> Option<u32> {
        None
    }
    /// Calls `f(id, message)` for every stored message for `recipient`.
    fn mail_for(&mut self, _recipient: &Address, _f: &mut dyn FnMut(u32, &[u8])) {}
    fn mail_remove(&mut self, _id: u32) {}
    /// (number of recipients, number of messages)
    fn mail_count(&mut self) -> (u32, u32) {
        (0, 0)
    }

    /// Staging area and installation for firmware updates (see
    /// [`crate::ota`]). Default: not supported.
    fn ota(&mut self) -> Option<&mut dyn crate::ota::OtaPort> {
        None
    }
}

pub struct FwNode<R: RngCore + CryptoRng, S: Settings> {
    /// In its own static (about 53 KB): built inside the node, its temporary
    /// copies overflowed the ESP32-S3's stack at boot.
    router: &'static mut Router<256, 1024>,
    identity: Identity,
    addr: Address,
    known: FnvIndexMap<Address, PublicIdentity, 64>,
    /// Contacts that can do forward secrecy (announce or FS message).
    fs_peers: FnvIndexSet<Address, 64>,
    sessions: FixedStore<SESSIONS>,
    resynced: FnvIndexMap<Address, Instant, 8>,
    channels: heapless::Vec<(Address, [u8; 32], Name), 4>,
    rng: R,
    modem: bool,
    /// LoRa modem for a host over Wi-Fi (fixed at boot; USB commands do not
    /// end it, unlike `modem`).
    wifi_modem: bool,
    radio_ok: bool,
    next_announce: Instant,
    settings: S,
    radio: RadioConfig,
    started: Instant,
    admins: heapless::Vec<Address, 4>,
    control_seen: FnvIndexMap<Address, u32, 4>,
    /// Unix time at start-up, once the client has told us the time.
    boot_unix: Option<u32>,
    /// Mail from the mailbox that has been sent and awaits acknowledgement: msg_id -> mail ID.
    mail_inflight: FnvIndexMap<u64, u32, 16>,
    mail_delivered: u32,
    /// Announce whose proof of work is being computed (see `run`).
    stamping: Option<(maarse_crypto::AnnounceBuf, StampSearch)>,
    region_names: RegionNames,
    /// Pictures and files (see [`files`]).
    files: files::Files,
    /// Diagnostics, battery and sharing (see node/ext.rs).
    ext: ext::Ext,
    ota: crate::ota::OtaManager,
    /// Last counter in an outgoing control command (must increase strictly).
    last_control: u32,
    /// Range test and power accounting (see node/range.rs).
    range: range::Range,
}

fn now_ms() -> u64 {
    Instant::now().as_millis()
}

fn emit(ev: &Event) {
    let f = ev.encode();
    let _ = crate::bus::BLE_OUT.try_send(f.clone());
    let _ = USB_OUT.try_send(f);
}

fn log(msg: &str) {
    emit(&Event::Log {
        text: text(msg.as_bytes()),
    });
}

fn err(req: u16, code: u8, msg: &str) {
    emit(&Event::Error {
        req,
        code,
        text: text(msg.as_bytes()),
    });
}

impl<R: RngCore + CryptoRng, S: Settings> FwNode<R, S> {
    pub fn new(
        identity: Identity,
        rng: R,
        settings: S,
        radio: RadioConfig,
        radio_ok: bool,
    ) -> Self {
        let addr = identity.address();
        crate::bus::MY_SHORT.store(
            u32::from_be_bytes([addr.0[0], addr.0[1], addr.0[2], addr.0[3]]),
            Relaxed,
        );
        // The role is chosen in the app (`SetRole`) and loaded from flash below;
        // the default is repeater (mains power).
        static ROUTER: static_cell::StaticCell<Router<256, 1024>> = static_cell::StaticCell::new();
        let router = ROUTER.init_with(|| Router::new(addr, Role::Repeater));
        let mut n = FwNode {
            router,
            identity,
            addr,
            known: FnvIndexMap::new(),
            fs_peers: FnvIndexSet::new(),
            sessions: FixedStore::new(),
            resynced: FnvIndexMap::new(),
            channels: heapless::Vec::new(),
            rng,
            modem: false,
            wifi_modem: crate::bus::WIFI_MODEM.load(Relaxed),
            radio_ok,
            next_announce: Instant::now() + Duration::from_secs(5),
            settings,
            radio,
            started: Instant::now(),
            admins: heapless::Vec::new(),
            control_seen: FnvIndexMap::new(),
            boot_unix: None,
            mail_inflight: FnvIndexMap::new(),
            mail_delivered: 0,
            stamping: None,
            region_names: RegionNames::new(),
            files: files::Files::new(),
            ext: ext::Ext::new(),
            ota: crate::ota::OtaManager::new(),
            last_control: 0,
            range: range::Range::new(),
        };
        n.ext.sharing = n.settings.load_sharing();
        n.ext.name = n.settings.load_name();
        let (strict, names) = n.settings.load_regions();
        n.apply_regions(strict, names);
        n.admins = n.settings.load_admins();
        let role = n.settings.load_role();
        n.apply_role(role);
        n.join("public");
        n.range_init();
        n
    }

    /// The main loop. If an announce stamp is being computed (about
    /// 4,096 attempts, 0.3 s on an nRF52840), it is done in slices of
    /// [`STAMP_CHUNK`] attempts. Between the slices we give the executor a turn with
    /// `yield_now`, so radio, USB and Bluetooth wait no more than about 20 ms,
    /// and received packets and commands are handled as normal.
    pub async fn run(&mut self) -> ! {
        loop {
            if self.stamping.is_some() {
                self.stamp_step();
                match select4(RADIO_RX.receive(), USB_IN.receive(), select(crate::bus::NET_RX.receive(), crate::bus::TOGGLE_RADIO.wait()), yield_now()).await {
                    Either4::First((pkt, snr)) => self.on_radio(pkt, snr),
                    Either4::Second(frame) => self.on_frame(frame),
                    Either4::Third(Either::First(pkt)) => self.on_net(pkt),
                    Either4::Third(Either::Second(())) => self.toggle_radio(),
                    Either4::Fourth(()) => {
                        if Instant::now() >= self.next_deadline() {
                            self.on_timer();
                        }
                    }
                }
                continue;
            }
            let deadline = self.next_deadline();
            match select4(RADIO_RX.receive(), USB_IN.receive(), select(crate::bus::NET_RX.receive(), crate::bus::TOGGLE_RADIO.wait()), Timer::at(deadline)).await {
                Either4::First((pkt, snr)) => self.on_radio(pkt, snr),
                Either4::Second(frame) => self.on_frame(frame),
                Either4::Third(Either::First(pkt)) => self.on_net(pkt),
                Either4::Third(Either::Second(())) => self.toggle_radio(),
                Either4::Fourth(()) => self.on_timer(),
            }
        }
    }

    /// One slice of the stamp search. Sends the announce when the stamp is found.
    fn stamp_step(&mut self) {
        let Some((payload, search)) = self.stamping.as_mut() else {
            return;
        };
        let Some(stamp) = search.step(STAMP_CHUNK) else {
            return;
        };
        let mut payload = payload.clone();
        self.stamping = None;
        if payload.extend_from_slice(&stamp).is_ok()
            && let Ok(pkt) = encode(&Header::new(PacketType::Announce, self.addr), &payload)
        {
            self.transmit(pkt, 0, 0);
        }
    }

    fn next_deadline(&self) -> Instant {
        let mut d = self.next_announce.min(self.ext.next_power);
        if let Some(t) = self.router.next_deadline() {
            let t = Instant::from_millis(t);
            if t < d {
                d = t;
            }
        }
        if let Some(t) = self.files.deadline()
            && t < d
        {
            d = t;
        }
        if let Some(t) = self.ota.deadline() {
            d = d.min(t);
        }
        if let Some(t) = self.range.deadline() {
            d = d.min(t);
        }
        d
    }

    fn on_timer(&mut self) {
        let actions = self.router.tick(now_ms());
        self.apply(actions);
        self.file_tick();
        self.power_tick();
        self.range_tick();
        if Instant::now() >= self.next_announce {
            if !self.modem && !self.wifi_modem {
                self.announce();
            }
            self.next_announce = Instant::now() + ANNOUNCE_EVERY;
        }
        if let Some(e) = self.ota.on_timer(self.settings.ota()) {
            log(e);
        }
    }

    fn transmit(&self, packet: Pkt, delay_ms: u32, id: u64) {
        // The backbone link gets every packet at once; flood delays and
        // cancellation only matter on the shared radio channel.
        if !self.wifi_modem
            && crate::bus::WIFI_STATE.load(Relaxed) == maarse_core::companion::wifi_state::LINKED as u32
        {
            let _ = crate::bus::NET_TX.try_send((packet.clone(), 0));
        }
        let _ = RADIO_TX.try_send(TxReq {
            packet,
            delay_ms,
            id,
            sf: 0,
        });
    }

    fn announce(&mut self) {
        let ts = self.unix_now().unwrap_or((now_ms() / 1000) as u32);
        let app = self.app_data();
        let Some(payload) = self.identity.announce(ts, &app) else {
            return;
        };
        // The stamp is computed in the background by `run` (see `stamp_step`). A new
        // announce replaces one that is not finished yet.
        if let Some(search) = StampSearch::new(PacketType::Announce, &self.addr, &payload) {
            self.stamping = Some((payload, search));
        }
    }

    fn apply(&mut self, actions: maarse_core::router::Actions) {
        for a in actions {
            match a {
                Action::Transmit {
                    packet,
                    delay_ms,
                    id,
                } => self.transmit(packet, delay_ms, id),
                Action::Cancel { id } => {
                    let _ = RADIO_CANCEL.try_send(id);
                }
                Action::Deliver {
                    ptype,
                    dest,
                    payload,
                    hops,
                } => self.on_deliver(ptype, dest, &payload, hops),
                Action::RouteLearned { dest, hops } => {
                    emit(&Event::RouteLearned { dest, hops });
                    self.flush_mail(dest);
                }
                Action::CarryAcked { msg_id } => {
                    // Handled like an acknowledgement, but the app is told
                    // "carried" (only relays carry, see `apply_role`).
                    if let Some(id) = self.mail_inflight.remove(&msg_id) {
                        self.settings.mail_remove(id);
                        self.mail_delivered += 1;
                    } else if !self.file_on_ack(msg_id) {
                        emit(&Event::CarryAcked { msg_id });
                    }
                }
                Action::Acked { msg_id } => {
                    if let Some(id) = self.mail_inflight.remove(&msg_id) {
                        self.settings.mail_remove(id);
                        self.mail_delivered += 1;
                    } else if !self.file_on_ack(msg_id) {
                        emit(&Event::Acked { msg_id });
                    }
                }
            }
        }
    }

    // ---------- radio ----------

    /// A packet from the Wi-Fi backbone link. TCP is treated as a perfect
    /// link, as maarse-host does.
    fn on_net(&mut self, pkt: Pkt) {
        if self.wifi_modem {
            // From the host: send it over LoRa as it is.
            let _ = RADIO_TX.try_send(TxReq { packet: pkt, delay_ms: 0, id: 0, sf: 0 });
            return;
        }
        if self.modem {
            return;
        }
        let actions = self.router.on_receive(&pkt, NET_SNR, now_ms());
        self.apply(actions);
    }

    /// The button asks to switch between Bluetooth and Wi-Fi: flip the saved
    /// mode and restart (only one radio runs at a time).
    fn toggle_radio(&mut self) {
        use crate::bus::{NOTE_ONE_RADIO, NOTE_TO_BLE, NOTE_TO_WIFI, NOTE_WIFI_NOT_SET_UP, note};
        if !crate::bus::WIFI_SUPPORTED.load(Relaxed) {
            note(NOTE_ONE_RADIO, 4);
            return;
        }
        let mut w = self.settings.load_wifi();
        if !w.enabled && (w.ssid.is_empty() || w.peer.is_empty()) {
            note(NOTE_WIFI_NOT_SET_UP, 5);
            log("Wi-Fi is not set up; set it up over USB first");
            return;
        }
        w.enabled = !w.enabled;
        self.settings.save_wifi(&w);
        note(if w.enabled { NOTE_TO_WIFI } else { NOTE_TO_BLE }, 5);
        log(if w.enabled { "Button: switching to Wi-Fi" } else { "Button: switching to Bluetooth" });
        let ev = self.wifi_event();
        emit(&ev);
        crate::bus::RESTART.signal(());
    }

    fn wifi_event(&mut self) -> Event {
        let w = self.settings.load_wifi();
        let ip = crate::bus::WIFI_IP.load(Relaxed).to_be_bytes();
        Event::Wifi {
            enabled: w.enabled,
            modem: w.modem,
            state: crate::bus::WIFI_STATE.load(Relaxed) as u8,
            ip,
            ssid: w.ssid,
            peer: w.peer,
        }
    }

    fn on_radio(&mut self, pkt: Pkt, snr: i8) {
        if self.wifi_modem {
            // To the host over Wi-Fi; dropped while the link is down.
            if crate::bus::WIFI_STATE.load(Relaxed) == maarse_core::companion::wifi_state::LINKED as u32 {
                let _ = crate::bus::NET_TX.try_send((pkt, snr));
            }
            return;
        }
        if self.modem {
            let mut f = maarse_core::companion::FrameBuf::new();
            let len = (pkt.len() + 2) as u16;
            let _ = f.extend_from_slice(&len.to_le_bytes());
            let _ = f.push(0xC0);
            let _ = f.push(snr as u8);
            let _ = f.extend_from_slice(&pkt);
            let _ = USB_OUT.try_send(f);
            return;
        }
        if self.range_on_radio(&pkt, snr) {
            return;
        }
        self.router.rx_rssi = RSTATS.last_rssi.load(Relaxed) as i16;
        let actions = self.router.on_receive(&pkt, snr, now_ms());
        self.apply(actions);
    }

    fn on_deliver(&mut self, ptype: PacketType, dest: Address, payload: &[u8], hops: u8) {
        match ptype {
            PacketType::Announce if dest != self.addr => {
                if let Ok(a) = verify_announce(&dest, payload) {
                    if self.known.insert(dest, a.identity).is_err() {
                        // Full: forget the oldest.
                        if let Some(k) = self.known.keys().next().copied() {
                            self.known.remove(&k);
                        }
                        let _ = self.known.insert(dest, a.identity);
                    }
                    if ratchet::announces_fs(a.app_data) {
                        self.mark_fs(dest);
                    }
                    self.on_app_data(dest, hops, a.app_data);
                    crate::bus::KNOWN_NODES.store(self.known.len() as u32, Relaxed);
                    emit(&Event::Heard { addr: dest, hops, identity: Some(a.identity.to_bytes()) });
                    self.flush_mail(dest);
                }
            }
            PacketType::Data => {
                let mut out = [0u8; 256];
                let known = &self.known;
                let res = ratchet::open(
                    &self.identity,
                    payload,
                    &mut self.sessions,
                    |a| known.get(a).copied(),
                    &mut out,
                );
                if let Ok(o) = &res
                    && o.format != Format::Static
                {
                    self.mark_fs(o.from);
                }
                match res.map(|o| (o.from, o.len)) {
                    Ok((from, n)) => match content::parse(&out[..n]) {
                        Some(Content::Resync) => log("The contact started a new encryption session"),
                        Some(Content::Text(t)) => {
                            crate::bus::note_message(&from, t);
                            emit(&Event::DirectReceived {
                                from,
                                hops,
                                text: text(t),
                            })
                        }
                        Some(Content::FileOffer(offer)) => self.file_offer(from, offer),
                        Some(Content::Control(counter, frame)) => {
                            let mut f: heapless::Vec<u8, 256> = heapless::Vec::new();
                            let _ = f.extend_from_slice(frame);
                            self.on_control(from, counter, &f);
                        }
                        Some(Content::ControlReply(_, frame)) => {
                            emit(&Event::RemoteReply { from, frame: text(frame) })
                        }
                        Some(Content::PageRequest(_)) => log("Page request: this node has no pages"),
                        _ => {}
                    },
                    Err(CryptoError::UnknownSender(_)) => {
                        log("Message from an unknown sender (no announce yet)")
                    }
                    Err(CryptoError::NoSession(a) | CryptoError::Desync(a)) => {
                        log("Message could not be read (the encryption session is lost)");
                        // Show it in the conversation too, so the loss is not silent.
                        emit(&Event::DirectReceived { from: a, hops, text: text(UNREADABLE_NOTICE.as_bytes()) });
                        self.resync(a);
                    }
                    Err(_) => {}
                }
            }
            PacketType::Fragment => self.file_chunk(payload),
            PacketType::Probe => self.on_trace(dest, payload),
            PacketType::Deposit => {
                // We are a mailbox for someone else: store the message for the recipient.
                if let Some(recipient) = Address::from_slice(payload) {
                    if self.settings.mail_add(&recipient, &payload[16..]).is_some() {
                        self.flush_mail(recipient);
                    } else {
                        log("The mailbox is full");
                    }
                }
            }
            PacketType::Channel => {
                let Some((_, key, _)) = self.channels.iter().find(|(a, _, _)| *a == dest) else {
                    return;
                };
                let mut out = [0u8; 256];
                if let Ok((from, n)) = seal::open_channel(key, payload, &mut out)
                    && from != self.addr
                {
                    crate::bus::note_message(&from, &out[..n]);
                    emit(&Event::ChannelReceived {
                        channel: dest,
                        from,
                        hops,
                        text: text(&out[..n]),
                    });
                }
            }
            _ => {}
        }
    }

    // ---------- USB ----------

    fn on_frame(&mut self, frame: Frame) {
        match frame.first() {
            Some(0x41) => {
                self.modem = true;
                return;
            }
            Some(0x40) => {
                if let Ok(pkt) = Pkt::from_slice(&frame[1..]) {
                    self.transmit(pkt, 0, 0);
                }
                return;
            }
            _ => {}
        }
        let Ok(cmd) = Command::decode(&frame) else {
            return err(0, error_code::BAD_FRAME, "invalid frame");
        };
        let Some(cmd) = self.ext_command(cmd).and_then(|c| self.range_command(c)) else {
            self.modem = false;
            return;
        };
        // Status and radio settings may be fetched without leaving modem mode.
        if !matches!(
            cmd,
            Command::GetStats | Command::GetRadio | Command::SetRadio(_) | Command::SetRole { .. }
        ) {
            self.modem = false;
        }
        match cmd {
            Command::GetStats => {
                let mut items = self.stats();
                self.mail_stats(&mut items);
                emit(&Event::Stats { items });
            }
            Command::GetRadio => emit(&Event::Radio(self.radio)),
            Command::SetRadio(cfg) => match cfg.validate() {
                Ok(()) => {
                    self.radio = cfg;
                    self.settings.save_radio(&cfg);
                    RADIO_CFG.signal(cfg);
                    self.range_radio_changed();
                    emit(&Event::Radio(cfg));
                    log("Radio settings saved");
                }
                Err(e) => err(0, error_code::BAD_FRAME, e),
            },
            Command::SetRole { role } => match self.set_role(role) {
                Ok(name) => {
                    let mut items = self.stats();
                    self.mail_stats(&mut items);
                    emit(&Event::Stats { items });
                    log(name);
                }
                Err(e) => err(0, error_code::BAD_FRAME, e),
            },
            Command::GetInfo => {
                emit(&Event::Info {
                    addr: self.addr,
                    repeater: self.router.role.forwards(),
                    version: PROTOCOL_VERSION,
                    max_text: (MAX_FS_TEXT - 1) as u8,
                    max_file: maarse_crypto::files::MAX_FILE as u32,
                    identity: Some(self.identity.public().to_bytes()),
                });
                for (ch, _, name) in &self.channels {
                    emit(&Event::ChannelJoined {
                        channel: *ch,
                        name: text(name.as_bytes()),
                    });
                }
                for (a, id) in self.known.iter() {
                    emit(&Event::Heard { addr: *a, hops: 0, identity: Some(id.to_bytes()) });
                }
                if !self.radio_ok {
                    log("The radio does not answer - check the board choice");
                }
                self.file_client_connected();
            }
            Command::AnnounceNow => {
                self.announce();
                log("The announce is sent when the proof of work has been computed");
            }
            Command::JoinChannel { name } => {
                let name = core::str::from_utf8(&name).unwrap_or("");
                self.join(name);
            }
            Command::JoinPrivateChannel { key, name } => {
                let name = core::str::from_utf8(&name).unwrap_or("");
                self.join_private(name, key);
            }
            Command::SendDirect {
                req,
                dest,
                text: msg,
            } => self.send_content(req, dest, &content::text(&msg)),
            Command::SendCall { .. } => err(0, error_code::NOT_OVER_TCP, "Calls need maarse-host (TCP)"),
            Command::RequestPage { req, dest, path } => match content::page_request(&path) {
                Some(plain) => self.send_content(req, dest, &plain),
                None => err(req, error_code::TOO_LARGE, "Invalid path"),
            },
            Command::SendChannel {
                req,
                channel,
                text: msg,
            } => {
                let Some((_, key, nm)) = self.channels.iter().find(|(a, _, _)| *a == channel) else {
                    return err(req, error_code::NOT_IN_CHANNEL, "Not a member of the channel");
                };
                let key = *key;
                let region = maarse_core::region::region_id(maarse_core::region::split_channel(nm).1);
                let mut nonce = [0u8; 8];
                self.rng.fill_bytes(&mut nonce);
                let mut ct = [0u8; 255];
                let res = seal::seal_channel(&key, &self.addr, nonce, &msg, &mut ct)
                    .ok()
                    .and_then(|n| self.router.send_channel_in(channel, region, &ct[..n]).ok());
                match res {
                    Some(actions) => {
                        emit(&Event::Sent { req, msg_id: 0 });
                        self.apply(actions);
                    }
                    None => err(req, error_code::TOO_LARGE, "The message is too long"),
                }
            }
            Command::Remote { req, dest, frame } => {
                if !matches!(
                    Command::decode(&frame),
                    Ok(Command::GetStats
                        | Command::GetRadio
                        | Command::SetRadio(_)
                        | Command::SetRole { .. }
                        | Command::OtaBegin { .. }
                        | Command::OtaChunk { .. }
                        | Command::OtaEnd { .. }
                        | Command::OtaStatus)
                ) {
                    return err(req, error_code::BAD_FRAME, "Only status, radio, role and firmware can be controlled remotely");
                }
                let Some(now) = self.unix_now() else {
                    return err(req, error_code::BAD_FRAME, "The clock is not set (connect again)");
                };
                // Must increase strictly: firmware chunks can come several per second.
                let counter = now.max(self.last_control.wrapping_add(1));
                self.last_control = counter;
                let plain = content::control(content::CONTROL, counter, &frame);
                match self.send_plain(dest, &plain) {
                    Ok(msg_id) => emit(&Event::Sent { req, msg_id }),
                    Err(e) => err(req, error_code::UNKNOWN_DEST, e),
                }
            }
            Command::SetAdmins { admins } => {
                self.admins = admins;
                self.settings.save_admins(&self.admins);
                emit(&Event::Admins { admins: self.admins.clone() });
            }
            Command::GetAdmins => emit(&Event::Admins { admins: self.admins.clone() }),
            Command::SetRegions { strict, names } => {
                let names = RegionNames::from_slice(&names[..names.len().min(120)]).unwrap_or_default();
                self.apply_regions(strict, names);
                self.settings.save_regions(strict, &self.region_names);
                emit(&self.regions_event());
                log("Regions saved");
            }
            Command::GetRegions => emit(&self.regions_event()),
            Command::GetWifi => {
                let ev = self.wifi_event();
                emit(&ev);
            }
            Command::SetWifi(w) => {
                if !crate::bus::WIFI_SUPPORTED.load(Relaxed) {
                    return err(0, error_code::BAD_FRAME, "This board has no Wi-Fi");
                }
                // An empty password for the same network keeps the saved one,
                // so the host can be changed without typing it again.
                let mut w = w;
                let old = self.settings.load_wifi();
                if w.password.is_empty() && w.ssid == old.ssid {
                    w.password = old.password;
                }
                self.settings.save_wifi(&w);
                let ev = self.wifi_event();
                emit(&ev);
                log("Wi-Fi saved; restarting to apply");
                crate::bus::RESTART.signal(());
            }
            // Only the host knows the route well enough to answer; the client falls
            // back to `Info.max_file` (8 KB on firmware).
            Command::GetFileLimit { .. } => err(0, error_code::BAD_FRAME, "unknown command"),
            Command::SetTime { unix } => {
                self.boot_unix = Some(unix.saturating_sub(self.started.elapsed().as_secs() as u32));
            }
            Command::OtaBegin { .. } | Command::OtaChunk { .. } | Command::OtaEnd { .. } | Command::OtaStatus => {
                let ev = self.ota.handle(self.settings.ota(), &cmd, false);
                emit(&ev);
            }
            // Handled in node/ext.rs.
            Command::TraceRoute { .. }
            | Command::GetNeighbors
            | Command::SetSharing(_)
            | Command::GetSharing
            | Command::SetName { .. }
            | Command::GetName
            | Command::SetRangeTest(_)
            | Command::GetRangeTest => {}
            Command::FileBegin { req, dest, size, mime, name } => {
                self.file_begin(req, dest, size, &mime, &name)
            }
            Command::FileData { req, offset, data } => self.file_data(req, offset, &data),
            Command::FileEnd { req } => self.file_end(req),
            Command::SetIdentity { confirm, secret } => self.set_identity(confirm, &secret),
        }
    }

    /// Tries to deliver mail to `recipient` (if we have any).
    fn flush_mail(&mut self, recipient: Address) {
        let mut items: heapless::Vec<(u32, heapless::Vec<u8, 255>), 4> = heapless::Vec::new();
        self.settings.mail_for(&recipient, &mut |id, sealed| {
            let _ = items.push((id, heapless::Vec::from_slice(sealed).unwrap_or_default()));
        });
        for (id, sealed) in items {
            if self.mail_inflight.values().any(|v| *v == id) {
                continue;
            }
            let mut token = [0u8; 16];
            self.rng.fill_bytes(&mut token);
            if let Ok((msg_id, actions)) = self.router.send_direct(recipient, &sealed, token, now_ms()) {
                let _ = self.mail_inflight.insert(msg_id, id);
                self.apply(actions);
            }
        }
    }

    fn unix_now(&self) -> Option<u32> {
        self.boot_unix.map(|b| b + self.started.elapsed().as_secs() as u32)
    }

    /// Encrypts with forward secrecy if the other party can, otherwise static.
    /// Encrypts and sends a direct message (text or page request).
    fn send_content(&mut self, req: u16, dest: Address, plain: &[u8]) {
        let Some(recipient) = self.known.get(&dest).copied() else {
            return err(req, error_code::UNKNOWN_DEST, "The recipient has not been heard yet");
        };
        let mut ct = [0u8; 255];
        let Ok(n) = self.seal(&recipient, plain, &mut ct) else {
            return err(req, error_code::TOO_LARGE, "The message is too long");
        };
        let mut token = [0u8; 16];
        self.rng.fill_bytes(&mut token);
        match self.router.send_direct(dest, &ct[..n], token, now_ms()) {
            Ok((msg_id, actions)) => {
                emit(&Event::Sent { req, msg_id });
                self.apply(actions);
            }
            Err(_) => err(req, error_code::QUEUE_FULL, "The queue is full"),
        }
    }

    fn seal(
        &mut self,
        recipient: &PublicIdentity,
        plain: &[u8],
        out: &mut [u8],
    ) -> Result<usize, CryptoError> {
        let dest = recipient.address();
        let fs_ok = self.fs_peers.contains(&dest) || self.sessions.contains(&dest);
        ratchet::seal(&self.identity, recipient, &mut self.sessions, fs_ok, &mut self.rng, plain, out)
            .map(|(n, _)| n)
    }

    fn mark_fs(&mut self, a: Address) {
        if self.fs_peers.insert(a).is_err() {
            self.fs_peers.clear();
            let _ = self.fs_peers.insert(a);
        }
    }

    /// A new session with `peer` and an empty resync message, so the other party switches.
    fn resync(&mut self, peer: Address) {
        let Some(id) = self.known.get(&peer).copied() else {
            return;
        };
        if self.resynced.get(&peer).is_some_and(|t| t.elapsed() < RESYNC_EVERY) {
            return;
        }
        if self.resynced.insert(peer, Instant::now()).is_err() {
            self.resynced.clear();
            let _ = self.resynced.insert(peer, Instant::now());
        }
        if ratchet::reset(&self.identity, &id, &mut self.sessions, &mut self.rng).is_ok() {
            self.mark_fs(peer);
            let _ = self.send_plain(peer, &[content::RESYNC]);
        }
    }

    /// Encrypts and sends a direct message.
    fn send_plain(&mut self, dest: Address, plain: &[u8]) -> Result<u64, &'static str> {
        let recipient = *self.known.get(&dest).ok_or("The recipient has not been heard yet")?;
        let mut ct = [0u8; 255];
        let n = self.seal(&recipient, plain, &mut ct).map_err(|_| "The message is too long")?;
        let mut token = [0u8; 16];
        self.rng.fill_bytes(&mut token);
        let (msg_id, actions) = self
            .router
            .send_direct(dest, &ct[..n], token, now_ms())
            .map_err(|_| "The queue is full")?;
        self.apply(actions);
        Ok(msg_id)
    }

    /// A control command over radio. The sender is proven by the encryption
    /// and must be on the list of administrators.
    fn on_control(&mut self, from: Address, counter: u32, frame: &[u8]) {
        let reply = |node: &mut Self, ev: Event| {
            let f = ev.encode();
            let plain = content::control(content::CONTROL_REPLY, counter, &f[2..]);
            let _ = node.send_plain(from, &plain);
        };
        let deny = |msg: &str| Event::Error { req: 0, code: error_code::BAD_FRAME, text: text(msg.as_bytes()) };
        if !self.admins.contains(&from) {
            return reply(self, deny("You are not an administrator of this node"));
        }
        // The counter (the sender's clock) must increase; if we know the time, it may
        // be at most 10 minutes from ours.
        let last = self.control_seen.get(&from).copied().unwrap_or(0);
        let fresh = self.unix_now().is_none_or(|now| counter.abs_diff(now) <= 600);
        if counter <= last || !fresh {
            return;
        }
        let _ = self.control_seen.insert(from, counter);
        match Command::decode(frame) {
            Ok(Command::GetStats) => {
                let mut items = self.stats();
                self.mail_stats(&mut items);
                for chunk in items.chunks(25) {
                    reply(self, Event::Stats { items: chunk.iter().copied().collect() });
                }
            }
            Ok(Command::GetRadio) => reply(self, Event::Radio(self.radio)),
            Ok(Command::SetRadio(cfg)) => match cfg.validate() {
                Ok(()) => {
                    // Reply first with the old settings active, so the reply
                    // can arrive; then switch.
                    reply(self, Event::Radio(cfg));
                    self.radio = cfg;
                    self.settings.save_radio(&cfg);
                    RADIO_CFG.signal(cfg);
                    self.range_radio_changed();
                }
                Err(e) => reply(self, deny(e)),
            },
            Ok(Command::SetRole { role }) => match self.set_role(role) {
                Ok(_) => {
                    let mut items = self.stats();
                    self.mail_stats(&mut items);
                    for chunk in items.chunks(25) {
                        reply(self, Event::Stats { items: chunk.iter().copied().collect() });
                    }
                }
                Err(e) => reply(self, deny(e)),
            },
            Ok(
                c @ (Command::OtaBegin { .. }
                | Command::OtaChunk { .. }
                | Command::OtaEnd { .. }
                | Command::OtaStatus),
            ) => {
                let ev = self.ota.handle(self.settings.ota(), &c, true);
                reply(self, ev);
            }
            _ => reply(self, deny("The command cannot be controlled remotely")),
        }
    }

    /// Sets the router's regions from a comma separated list of names.
    fn apply_regions(&mut self, strict: bool, names: RegionNames) {
        let s = core::str::from_utf8(&names).unwrap_or("");
        self.router.regions = maarse_core::region::parse_list(s);
        self.router.strict_region = strict;
        // Store normalised: lowercase, without spaces, at most 8.
        let mut norm = RegionNames::new();
        for n in maarse_core::region::names(s).take(maarse_core::region::MAX_REGIONS) {
            if !norm.is_empty() {
                let _ = norm.push(b',');
            }
            for c in n.chars().flat_map(char::to_lowercase) {
                let mut b = [0u8; 4];
                let _ = norm.extend_from_slice(c.encode_utf8(&mut b).as_bytes());
            }
        }
        self.region_names = norm;
    }

    fn regions_event(&self) -> Event {
        Event::Regions {
            strict: self.router.strict_region,
            names: text(&self.region_names),
        }
    }

    /// Sets the router's role. Relays (forest mode, handhelds on the move)
    /// also carry messages they cannot deliver: in the simulator, 4 copies
    /// gave +14-16 points delivered for about +10 % airtime. Repeaters and
    /// clients do not carry.
    fn apply_role(&mut self, role: Role) {
        self.router.set_role(role);
        self.router.carry_copies = if role == Role::Relay { RELAY_CARRY_COPIES } else { 0 };
    }

    /// Changes role and stores it in flash. Returns a log text.
    fn set_role(&mut self, role: u8) -> Result<&'static str, &'static str> {
        let r = Role::from_u8(role).ok_or("Unknown role (0 = client, 1 = repeater, 2 = relay)")?;
        self.apply_role(r);
        self.settings.save_role(r);
        Ok(match r {
            Role::Client => "Role saved: client",
            Role::Repeater => "Role saved: repeater",
            Role::Relay => "Role saved: relay",
            Role::Anchor => "Role saved: anchor",
        })
    }

    fn stats(&self) -> Stats {
        let r = self.router.stats;
        let rs = &RSTATS;
        let u = |a: &core::sync::atomic::AtomicU32| a.load(Relaxed).min(i32::MAX as u32) as i32;
        let mut items = Stats::new();
        let mut put = |k: u8, v: i32| {
            let _ = items.push((k, v));
        };
        put(stat::UPTIME_S, self.started.elapsed().as_secs() as i32);
        put(stat::ROLE, self.router.role.as_u8() as i32);
        put(stat::KNOWN_NODES, self.known.len() as i32);
        put(stat::ROUTES, self.router.routes().len() as i32);
        put(stat::CHANNELS, self.channels.len() as i32);
        put(stat::RX_PACKETS, r.rx_packets as i32);
        put(stat::RX_INVALID, r.rx_invalid as i32);
        put(stat::RX_DUPLICATES, r.rx_duplicates as i32);
        put(stat::FORWARDED, r.forwarded as i32);
        put(stat::ORIGINATED, r.originated as i32);
        put(stat::DELIVERED, r.delivered as i32);
        put(stat::ACKED, r.acked as i32);
        put(stat::ROUTES_LEARNED, r.routes_learned as i32);
        put(stat::PATH_REQUESTS, r.path_requests as i32);
        put(stat::HOP_RETRIES, r.hop_retries as i32);
        put(stat::FLOODS_CANCELLED, r.floods_cancelled as i32);
        put(stat::BAD_STAMPS, r.bad_stamps as i32);
        put(stat::RADIO_RX_OK, u(&rs.rx_ok));
        put(stat::RADIO_RX_ERR, u(&rs.rx_err));
        put(stat::RADIO_TX_OK, u(&rs.tx_ok));
        put(stat::RADIO_TX_ERR, u(&rs.tx_err));
        put(stat::RADIO_DROPPED, u(&rs.dropped));
        if rs.rx_ok.load(Relaxed) > 0 {
            put(stat::LAST_RSSI_DBM, rs.last_rssi.load(Relaxed));
            put(stat::LAST_SNR_DB, rs.last_snr.load(Relaxed));
            put(stat::AVG_RSSI_DBM, rs.avg_rssi_x16.load(Relaxed) / 16);
            put(stat::AVG_SNR_DB, rs.avg_snr_x16.load(Relaxed) / 16);
            put(stat::MIN_RSSI_DBM, rs.min_rssi.load(Relaxed));
            put(stat::MAX_RSSI_DBM, rs.max_rssi.load(Relaxed));
        }
        put(stat::CAD_BUSY, u(&rs.cad_busy));
        put(stat::DUTY_DEFERRED, u(&rs.duty_deferred));
        put(stat::AIRTIME_MS, u(&rs.airtime_ms));
        put(stat::DUTY_USED_PERMILLE, u(&rs.duty_used_permille));
        put(stat::RADIO_BYTES_RX, u(&rs.bytes_rx));
        put(stat::RADIO_BYTES_TX, u(&rs.bytes_tx));
        put(stat::TX_QUEUE, u(&rs.tx_queue));
        put(stat::RADIO_STAGE, u(&rs.stage));
        put(stat::RADIO_LOOPS, u(&rs.loops));
        for (i, d) in crate::bus::RADIO_DIAG.iter().enumerate() {
            put(55 + i as u8, d.load(Relaxed) as i32);
        }
        put(stat::FREQ_KHZ, (self.radio.freq_hz / 1000) as i32);
        put(stat::SF, self.radio.sf as i32);
        put(stat::BW_KHZ, (self.radio.bw_hz / 1000) as i32);
        put(stat::TX_POWER_DBM, self.radio.tx_power_dbm as i32);
        self.ext_stats(&mut items);
        items
    }

    fn mail_stats(&mut self, items: &mut Stats) {
        let (r, m) = self.settings.mail_count();
        let _ = items.push((stat::MAILBOX_RECIPIENTS, r as i32));
        let _ = items.push((stat::MAILBOX_MESSAGES, m as i32));
        let _ = items.push((stat::MAIL_DELIVERED, self.mail_delivered as i32));
    }

    fn join(&mut self, name: &str) {
        // `name@region`: the region tags messages we send to the channel.
        let (lower, region) = maarse_core::region::split_channel(name);
        let (label, key): (&str, [u8; 32]) = match lower {
            "" | "public" | "#public" => {
                ("public", public_channel_key())
            }
            n => (n, {
                // Hashtag channels are always lowercase and with '#'.
                let mut s: heapless::String<24> = heapless::String::new();
                if !n.starts_with('#') {
                    let _ = s.push('#');
                }
                for c in n.chars() {
                    for l in c.to_lowercase() {
                        let _ = s.push(l);
                    }
                }
                hashtag_key(&s)
            }),
        };
        let mut nm = Name::new();
        if label != "public" && !label.starts_with('#') {
            let _ = nm.push('#');
        }
        let region_part = if region.is_empty() { None } else { Some(region) };
        for c in label.chars().chain(region_part.into_iter().flat_map(|r| core::iter::once('@').chain(r.chars()))) {
            for l in c.to_lowercase() {
                let _ = nm.push(l);
            }
        }
        self.join_key(nm, key);
    }

    /// Private channel: the key is random and comes from an invitation. The name
    /// is only for display (truncated to 24 bytes).
    fn join_private(&mut self, name: &str, key: [u8; 32]) {
        let mut nm = Name::new();
        for c in name.trim().chars() {
            if nm.push(c).is_err() {
                break;
            }
        }
        if nm.is_empty() {
            let _ = nm.push_str("privat");
        }
        self.join_key(nm, key);
    }

    fn join_key(&mut self, nm: Name, key: [u8; 32]) {
        let addr = channel_address(&key);
        if let Some(ch) = self.channels.iter_mut().find(|(a, _, _)| *a == addr) {
            // Already a member: update the name (and with it the region).
            ch.2 = nm;
        } else {
            if self.channels.push((addr, key, nm)).is_err() {

                return log("At most 4 channels on this node");
            }
            let _ = self.router.subscribe(addr);
        }
        if let Some((_, _, nm)) = self.channels.iter().find(|(a, _, _)| *a == addr) {
            emit(&Event::ChannelJoined {
                channel: addr,
                name: text(nm.as_bytes()),
            });
        }
    }
}
