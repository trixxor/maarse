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
use maarse_core::appdata;
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

/// Contacts kept in RAM, and how many of them are pinned (the people one
/// writes to or hears from: never evicted, the only ones kept in flash).
/// In maarse-sim a national net shows 1-2 % of messages with 64 contacts
/// evicting the oldest, 93-95 % with pinning (docs/denmark/README.md).
const PINNED_MAX: usize = 32;
/// Post offices (see `maarse_core::mail`): unacknowledged messages kept for
/// a deposit, how long one waits for a post office, and how often we ask
/// for mail anyway.
const OUTBOX: usize = 4;
const OUTBOX_GIVE_UP: Duration = Duration::from_secs(600);
const MAIL_SYNC_EVERY: Duration = Duration::from_secs(6 * 3600);
const MAIL_SYNC_MIN: Duration = Duration::from_secs(60);

/// How long a message waits for its recipient's key (key request).
const KEY_WAIT: Duration = Duration::from_secs(60);
/// Mobile mode: the best repeater changed this many times within
/// `MOBILE_WINDOW` (a car passing repeaters); off again after `MOBILE_CALM`
/// without a change.
const MOBILE_HANDOVERS: usize = 3;
const MOBILE_WINDOW: Duration = Duration::from_secs(600);
const MOBILE_CALM: Duration = Duration::from_secs(900);

/// Stored role value for automatic forest mode (beyond the router's roles 0-3).
pub const ROLE_AUTO: u8 = 4;
/// Automatic forest mode: relay after this long without a repeater or anchor
/// in range, client again after hearing one this long without a break. In
/// maarse-sim 2 min beat 10 min in the forest and in a town (see
/// docs/denmark/README.md, "Automatic forest mode").
const AUTO_SWITCH: Duration = Duration::from_secs(120);

/// Copies per message a relay carries (see `apply_role`).
const RELAY_CARRY_COPIES: u8 = 4;

/// Where radio settings are stored (flash on the board).
pub trait Settings {
    fn save_radio(&mut self, cfg: &RadioConfig);
    fn save_admins(&mut self, admins: &[Address]);
    fn load_admins(&mut self) -> heapless::Vec<Address, 4>;
    /// Stores the node's role. Default: not stored.
    fn save_role(&mut self, _role: u8) {}
    /// Stored role; the default (and older flash without a role) is repeater.
    fn load_role(&mut self) -> u8 {
        Role::Repeater.as_u8()
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
    /// Stores the Wi-Fi modem key (`None` = none). Default: not stored.
    fn save_modem_key(&mut self, _key: Option<[u8; 32]>) {}
    /// The stored Wi-Fi modem key.
    fn load_modem_key(&mut self) -> Option<[u8; 32]> {
        None
    }
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

    /// Known contacts (public identities) kept across reboots. Default: not kept.
    fn known_load(&mut self, _f: &mut dyn FnMut(&[u8; 64])) {}
    /// Adds one contact; `false` if there is no room (then `known_reset`).
    fn known_add(&mut self, _id: &[u8; 64]) -> bool {
        false
    }
    /// Replaces the stored contacts with these.
    fn known_reset(&mut self, _ids: &mut dyn Iterator<Item = [u8; 64]>) {}

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
    known: FnvIndexMap<Address, PublicIdentity, 128>,
    /// Contacts we have written to or heard from (kept in flash, never
    /// evicted from `known`).
    pinned: heapless::Vec<Address, PINNED_MAX>,
    /// Messages waiting for their recipient's key: (request, recipient,
    /// plaintext, since).
    await_key: heapless::Vec<(u16, Address, heapless::Vec<u8, 200>, Instant), 4>,
    /// Last key request (whom, when) and last answer to one.
    key_asked: Option<(Address, Instant)>,
    key_answered: Option<Instant>,
    /// Mobile mode: best repeater now, and recent changes of it.
    best_repeater: Option<u16>,
    handovers: heapless::Vec<Instant, MOBILE_HANDOVERS>,
    next_mobile_check: Instant,
    /// Sent messages not acknowledged yet: (msg ID, recipient, the message
    /// sealed for a post office (static seal), sent, deposited).
    outbox: heapless::Vec<(u64, Address, heapless::Vec<u8, 255>, Instant, bool), OUTBOX>,
    /// What other nodes can read (`appdata::CAP_*`), from their announces.
    caps: heapless::FnvIndexMap<Address, u8, 64>,
    /// Mail IDs of messages we received lately (sent along in a MailSync).
    got_mail: heapless::Vec<u32, 8>,
    /// Our home post office (path ID, address) and the last mail sync.
    home_po: Option<(u16, Option<Address>)>,
    last_sync: Option<Instant>,
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
    /// Next time the Wi-Fi modem sends its counters to the host.
    next_net_stats: Instant,
    /// Wi-Fi modem whose link has been down for `MODEM_FALLBACK_AFTER`:
    /// it works as an ordinary node (routing, announces, hellos) until the
    /// link is back, so the LoRa group around it is not left without it.
    modem_fallback: bool,
    link_down_since: Option<Instant>,
    radio_ok: bool,
    next_announce: Instant,
    /// A repeater or anchor has been heard since the start: announce at the
    /// configured interval (`RadioConfig::announce_interval_s`, default 12 h)
    /// instead of every 15 minutes.
    announce_settled: bool,
    /// The first announce since the start has been sent (it is never put
    /// off by settling, so a new node is known at once).
    announced: bool,
    /// Automatic forest mode (role "auto"): the router is a client while a
    /// repeater or anchor is in range and a relay when none has been for a while.
    auto_role: bool,
    /// Last time a repeater was in range, and since when without a break.
    rep_last: Instant,
    rep_since: Option<Instant>,
    next_auto_check: Instant,
    /// The edge bridge choice our last announce carried (or will carry).
    told_choice: Option<u16>,
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
    /// Direct messages from senders we do not know yet (no announce heard,
    /// e.g. right after a reboot without stored contacts): opened when the
    /// sender's announce arrives. (payload, hops)
    pending_rx: heapless::Vec<(heapless::Vec<u8, 256>, u8), PENDING_RX>,
}

/// Messages from unknown senders kept until their announce.
const PENDING_RX: usize = 4;

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

/// A Wi-Fi modem works as an ordinary node after its link has been down this long.
const MODEM_FALLBACK_AFTER: Duration = Duration::from_secs(60);

impl<R: RngCore + CryptoRng, S: Settings> FwNode<R, S> {
    /// A node returned by value (it is built in place and then moved; see
    /// [`FwNode::new_in`] where stack is short).
    pub fn new(identity: Identity, rng: R, settings: S, radio: RadioConfig, radio_ok: bool) -> Self {
        let mut slot = core::mem::MaybeUninit::uninit();
        Self::new_in(&mut slot, identity, rng, settings, radio, radio_ok);
        // SAFETY: new_in has written the node into the slot.
        unsafe { slot.assume_init() }
    }

    /// Builds the node directly in `dest` (e.g. a `StaticCell`'s memory)
    /// and loads its settings and contacts there. Built on the stack and
    /// then copied, the node (about 27 KB) took about 60 KB of stack on the
    /// ESP32-S3 at start-up and left 4.5 KB; the PHY calibration interrupt
    /// that came then ran past the end of the stack and crashed the board.
    pub fn new_in<'a>(
        dest: &'a mut core::mem::MaybeUninit<Self>,
        identity: Identity,
        rng: R,
        settings: S,
        radio: RadioConfig,
        radio_ok: bool,
    ) -> &'a mut Self {
        let addr = identity.address();
        crate::bus::MY_SHORT.store(
            u32::from_be_bytes([addr.0[0], addr.0[1], addr.0[2], addr.0[3]]),
            Relaxed,
        );
        // The role is chosen in the app (`SetRole`) and loaded from flash below;
        // the default is repeater (mains power).
        static ROUTER: static_cell::StaticCell<Router<256, 1024>> = static_cell::StaticCell::new();
        let router = ROUTER.init_with(|| Router::new(addr, Role::Repeater));
        let n = dest.write(FwNode {
            router,
            identity,
            addr,
            known: FnvIndexMap::new(),
            pinned: heapless::Vec::new(),
            await_key: heapless::Vec::new(),
            key_asked: None,
            key_answered: None,
            best_repeater: None,
            handovers: heapless::Vec::new(),
            next_mobile_check: Instant::now() + Duration::from_secs(15),
            outbox: heapless::Vec::new(),
            got_mail: heapless::Vec::new(),
            caps: heapless::FnvIndexMap::new(),
            home_po: None,
            last_sync: None,
            fs_peers: FnvIndexSet::new(),
            sessions: FixedStore::new(),
            resynced: FnvIndexMap::new(),
            channels: heapless::Vec::new(),
            rng,
            modem: false,
            wifi_modem: crate::bus::WIFI_MODEM.load(Relaxed),
            next_net_stats: Instant::now() + Duration::from_secs(20),
            modem_fallback: false,
            link_down_since: None,
            radio_ok,
            next_announce: Instant::now() + Duration::from_secs(5),
            announce_settled: false,
            announced: false,
            auto_role: false,
            rep_last: Instant::now(),
            rep_since: None,
            next_auto_check: Instant::now() + Duration::from_secs(15),
            told_choice: None,
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
            pending_rx: heapless::Vec::new(),
        });
        // The first announce 5-35 s after the start, at random: after a power
        // cut many nodes start at the same moment and would collide.
        n.next_announce = Instant::now() + Duration::from_millis(5000 + n.rng.next_u64() % 30_000);
        // Contacts stored in flash: a reboot does not forget them.
        {
            let FwNode { settings, known, pinned, .. } = &mut *n;
            settings.known_load(&mut |b| {
                if let Some(p) = PublicIdentity::from_bytes(b) {
                    let _ = known.insert(p.address(), p);
                    if !pinned.contains(&p.address()) {
                        let _ = pinned.push(p.address());
                    }
                }
            });
        }
        // Neighbor hellos (role, coverage, bridge choice, motion) on a
        // Trickle timer: neighbors know within seconds what announces 12 h
        // apart would tell them only much later.
        n.router.hello = true;
        if n.wifi_modem {
            // The host's antenna: the host routes and says hello, not us.
            n.router.hello = false;
        }
        // Expanding ring search: a node heard within 4 hops lately is looked
        // for there first (maarse-sim: 16 % less airtime nationally).
        n.router.ring = 4;
        {
        }
        crate::bus::KNOWN_NODES.store(n.known.len() as u32, Relaxed);
        n.ext.sharing = n.settings.load_sharing();
        n.ext.name = n.settings.load_name();
        let (strict, names) = n.settings.load_regions();
        n.apply_regions(strict, names);
        n.admins = n.settings.load_admins();
        let role = n.settings.load_role();
        n.apply_role_value(role);
        n.apply_radio_timing();
        n.join("public");
        n.range_init();
        n
    }

    /// Sizes the flood slot from the radio settings and scales the ack,
    /// path retry and hop timeouts with it (`Router::set_slot`). In
    /// maarse-sim this beat the fixed 200 ms slot at SF11/SF12 and was equal
    /// at SF9; the SF slot without scaled timeouts was worse.
    fn apply_radio_timing(&mut self) {
        let slot = maarse_core::qos::slot_ms(&self.radio.airtime());
        self.router.set_slot(slot, true);
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
        if self.acting_modem() {
            // Only our own timers: the router is not used as the host's modem.
            let mut d = self.next_mobile_check.min(self.next_net_stats).min(self.ext.next_power);
            if let Some(t) = self.ota.deadline() {
                d = d.min(t);
            }
            return d;
        }
        let mut d = self.next_announce.min(self.ext.next_power);
        if self.auto_role {
            d = d.min(self.next_auto_check);
        }
        d = d.min(self.next_mobile_check);
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

    /// Working as the host's Wi-Fi modem right now (not fallen back).
    fn acting_modem(&self) -> bool {
        self.wifi_modem && !self.modem_fallback
    }

    /// A Wi-Fi modem whose link has been down a while works as an ordinary
    /// node; when the link is back, it is the host's modem again.
    fn modem_fallback_step(&mut self) {
        let linked = crate::bus::WIFI_STATE.load(Relaxed) == maarse_core::companion::wifi_state::LINKED as u32;
        if linked {
            self.link_down_since = None;
            if self.modem_fallback {
                self.modem_fallback = false;
                self.router.hello = false;
                crate::bus::MODEM_FALLBACK.store(false, Relaxed);
                log("Wi-Fi link back: the host's modem again");
            }
            return;
        }
        let since = *self.link_down_since.get_or_insert_with(Instant::now);
        if !self.modem_fallback && since.elapsed() >= MODEM_FALLBACK_AFTER {
            self.modem_fallback = true;
            self.router.hello = true;
            crate::bus::MODEM_FALLBACK.store(true, Relaxed);
            // Tell the neighbours soon who we are (5-35 s, as after a start).
            self.next_announce = Instant::now() + Duration::from_millis(5000 + self.rng.next_u64() % 30_000);
            log("Wi-Fi link down for 60 s: working as an ordinary node until it is back");
        }
    }

    fn on_timer(&mut self) {
        if self.wifi_modem {
            self.modem_fallback_step();
        }
        if self.acting_modem() {
            // The host routes: only our counters, power and updates here.
            self.push_net_stats();
            self.power_tick();
            if let Some(e) = self.ota.on_timer(self.settings.ota()) {
                log(e);
            }
            self.next_mobile_check = Instant::now() + Duration::from_secs(5);
            return;
        }
        let actions = self.router.tick(now_ms());
        self.apply(actions);
        self.file_tick();
        self.power_tick();
        self.range_tick();
        self.check_settled();
        if Instant::now() >= self.next_mobile_check {
            self.next_mobile_check = Instant::now() + Duration::from_secs(15);
            self.mobile_step();
            self.mail_step();
            if let Some(a) = self.await_key.first().map(|e| e.1) {
                // Also expires messages whose key never came.
                self.release_await_key_expired(a);
            }
        }
        if self.auto_role && Instant::now() >= self.next_auto_check {
            self.next_auto_check = Instant::now() + Duration::from_secs(15);
            self.auto_role_step();
        }
        if Instant::now() >= self.next_announce {
            if !self.modem && !self.acting_modem() {
                self.announce();
            }
            self.announced = true;
            let bridge = self.router.role == Role::Relay && self.router.hears_repeater(now_ms());
            let every = if bridge {
                // Nodes without coverage choose among the bridges they hear.
                maarse_core::router::BRIDGE_ANNOUNCE_S.min(self.radio.announce_interval_s())
            } else if self.announce_settled {
                self.radio.announce_interval_s()
            } else {
                RadioConfig::BOOT_ANNOUNCE_S
            };
            self.next_announce = Instant::now() + self.jittered(every);
        }
        if let Some(e) = self.ota.on_timer(self.settings.ota()) {
            log(e);
        }
    }

    /// Wi-Fi modem: the host gets our counters every 30 s, as over USB.
    fn push_net_stats(&mut self) {
        if Instant::now() < self.next_net_stats {
            return;
        }
        self.next_net_stats = Instant::now() + Duration::from_secs(30);
        let frames = [Event::Stats { items: self.stats() }.encode(), Event::Stats { items: self.more_stats() }.encode(), Event::Radio(self.radio).encode()];
        for f in frames {
            if let Ok(v) = heapless::Vec::from_slice(&f[2..]) {
                let _ = crate::bus::NET_STATS.try_send(v);
            }
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
                    self.outbox.retain(|e| e.0 != msg_id);
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
        if self.acting_modem() {
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
        let bad = self.router.stats.bad_stamps;
        let actions = self.router.on_receive(&pkt, snr, now_ms());
        if self.router.stats.bad_stamps != bad {
            log_bad_stamp(&pkt, snr);
        }
        // After apply: an announce's role field reaches the router there.
        self.apply(actions);
        self.check_settled();
    }

    /// Once a repeater or anchor is in range after the start, announce at the
    /// long interval from then on.
    fn check_settled(&mut self) {
        let choice = self.router.chosen_bridge(now_ms());
        if !self.announce_settled && (self.router.hears_repeater(now_ms()) || choice.is_some()) {
            self.announce_settled = true;
            if self.announced {
                self.reschedule_announce();
            }
            log(if choice.is_some() {
                "An edge bridge connects us to the network: announcing at the long interval from now on"
            } else {
                "A repeater is in range: announcing at the long interval from now on"
            });
        }
        if choice != self.told_choice {
            // The chosen bridge learns from our announce that it is needed.
            self.told_choice = choice;
            if self.announced {
                let soon = Instant::now() + Duration::from_millis(5000 + self.rng.next_u64() % 30_000);
                self.next_announce = self.next_announce.min(soon);
            }
        }
    }

    fn on_deliver(&mut self, ptype: PacketType, dest: Address, payload: &[u8], hops: u8) {
        match ptype {
            PacketType::KeyRequest if dest == self.addr => {
                // Someone needs our key: announce soon (at most every 30 s).
                if self.key_answered.is_none_or(|t| t.elapsed() >= Duration::from_secs(30)) {
                    self.key_answered = Some(Instant::now());
                    let soon = Instant::now() + Duration::from_millis(500 + self.rng.next_u64() % 2500);
                    self.next_announce = self.next_announce.min(soon);
                    log("Asked for our key: announcing");
                }
            }
            PacketType::Announce if dest != self.addr => {
                if let Ok(a) = verify_announce(&dest, payload) {
                    let new = !self.known.contains_key(&dest);
                    if self.known.insert(dest, a.identity).is_err() {
                        // Full: forget the oldest contact that is not pinned
                        // (strangers go first; the people we talk to stay).
                        let pinned = &self.pinned;
                        if let Some(k) = self.known.keys().find(|k| !pinned.contains(k)).copied() {
                            self.known.remove(&k);
                        }
                        let _ = self.known.insert(dest, a.identity);
                    }
                    if new {
                        self.open_pending();
                    }
                    self.release_await_key(dest);
                    if ratchet::announces_fs(a.app_data) {
                        self.mark_fs(dest);
                    }
                    self.on_app_data(dest, hops, a.app_data);
                    crate::bus::KNOWN_NODES.store(self.known.len() as u32, Relaxed);
                    emit(&Event::Heard { addr: dest, hops, identity: Some(a.identity.to_bytes()) });
                    self.flush_mail(dest);
                }
            }
            PacketType::Data => self.on_data(payload, hops),
            PacketType::Fragment => self.file_chunk(payload),
            PacketType::Probe => self.on_trace(dest, payload),
            PacketType::Deposit => {
                // We are a post office for someone else. Radio nodes do not
                // offer it (mail class 0: hosts keep mail, repeaters only
                // carry it), but a STORE sent to us is kept in the small
                // flash mailbox, and SYNC delivers it.
                use maarse_core::mail::{self, Deposit};
                match Deposit::parse(payload) {
                    Some(d) if d.op == mail::OP_STORE => {
                        if self.settings.mail_add(&d.recipient, d.body).is_some() {
                            self.flush_mail(d.recipient);
                        } else {
                            log("The mailbox is full");
                        }
                    }
                    Some(d) if d.op == mail::OP_SYNC => self.flush_mail(d.recipient),
                    Some(d) if d.op == mail::OP_BUNDLE && d.recipient == self.addr => {
                        // Several messages for us in one packet.
                        let items: heapless::Vec<heapless::Vec<u8, 255>, 4> =
                            d.bundle().filter_map(|m| heapless::Vec::from_slice(m).ok()).take(4).collect();
                        for m in items {
                            self.on_data(&m, hops);
                        }
                    }
                    Some(_) => {}
                    None => {
                        // Older senders: recipient | sealed.
                        if let Some(recipient) = Address::from_slice(payload) {
                            if self.settings.mail_add(&recipient, &payload[16..]).is_some() {
                                self.flush_mail(recipient);
                            } else {
                                log("The mailbox is full");
                            }
                        }
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
                emit(&Event::Stats { items: self.stats() });
                emit(&Event::Stats { items: self.more_stats() });
            }
            Command::GetRadio => emit(&Event::Radio(self.radio)),
            Command::SetRadio(cfg) => match cfg.validate() {
                Ok(()) => {
                    self.radio = cfg;
                    if self.announce_settled && self.announced {
                        self.reschedule_announce();
                    }
                    self.apply_radio_timing();
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
                    emit(&Event::Stats { items: self.stats() });
                    emit(&Event::Stats { items: self.more_stats() });
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
            // Linked devices are built in maarse-host first.
            Command::LinkDevice { req, .. } | Command::PersonSetup { req, .. } => {
                err(req, error_code::NOT_OVER_TCP, "Linked devices need maarse-host (phone as node or a PC)")
            }
            Command::GetPerson => err(0, error_code::NOT_OVER_TCP, "Linked devices need maarse-host"),
            Command::SendAlert { req, .. } => err(req, error_code::NOT_OVER_TCP, "Alerts are issued by maarse-host with an issuer key"),
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
            Command::SetModemKey { passphrase } => {
                if !crate::bus::WIFI_SUPPORTED.load(Relaxed) {
                    return err(0, error_code::BAD_FRAME, "This board has no Wi-Fi");
                }
                let key = (!passphrase.is_empty()).then(|| maarse_core::modemauth::key(&passphrase));
                self.settings.save_modem_key(key);
                log(if key.is_some() { "Modem key saved; used after the next restart" } else { "Modem key removed" });
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
        // Text is compressed for a node that says it can read it (about 40 %
        // shorter for short chat messages).
        let z;
        let plain = if self.caps.get(&dest).is_some_and(|c| c & appdata::CAP_TEXTZ != 0) {
            z = content::compress(plain);
            &z[..]
        } else {
            plain
        };
        let Some(recipient) = self.known.get(&dest).copied() else {
            // Ask the network for the key; the message goes when it comes.
            let Ok(p) = heapless::Vec::from_slice(plain) else {
                return err(req, error_code::TOO_LARGE, "The message is too long");
            };
            if self.await_key.is_full() {
                let (r, ..) = self.await_key.remove(0);
                err(r, error_code::UNKNOWN_DEST, "The recipient's key did not come");
            }
            let _ = self.await_key.push((req, dest, p, Instant::now()));
            self.ask_key(dest);
            return log("The recipient has not been heard yet: asking the network for its key");
        };
        self.pin(dest);
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
                // A copy for a post office, sealed without forward secrecy
                // (a ratchet message delivered days later may not open).
                let mut st = [0u8; 255];
                if let Ok((m, _)) = ratchet::seal(&self.identity, &recipient, &mut self.sessions, false, &mut self.rng, plain, &mut st) {
                    if self.outbox.is_full() {
                        self.outbox.remove(0);
                    }
                    let _ = self.outbox.push((msg_id, dest, heapless::Vec::from_slice(&st[..m]).unwrap_or_default(), Instant::now(), false));
                }
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

    /// `secs` with ±10 % randomness, so nodes do not announce in step.
    fn jittered(&mut self, secs: u32) -> Duration {
        let ms = secs as u64 * 1000;
        let span = (ms / 5).max(1);
        Duration::from_millis(ms - ms / 10 + self.rng.next_u64() % span)
    }

    /// After settling or a new interval: the next announce at a random moment
    /// within the interval, so many nodes that settle together spread out.
    fn reschedule_announce(&mut self) {
        let ms = self.radio.announce_interval_s() as u64 * 1000;
        let wait = ms / 10 + self.rng.next_u64() % (ms - ms / 10).max(1);
        self.next_announce = Instant::now() + Duration::from_millis(wait);
    }

    /// Opens kept messages from senders that were unknown (a new contact has
    /// just been heard). Those still unknown are kept again.
    fn open_pending(&mut self) {
        if self.pending_rx.is_empty() {
            return;
        }
        let kept = core::mem::take(&mut self.pending_rx);
        let addr = self.addr;
        for (p, hops) in kept {
            self.on_deliver(PacketType::Data, addr, &p, hops);
        }
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
                let items = self.stats();
                let more = self.more_stats();
                for chunk in items.chunks(25).chain(more.chunks(25)) {
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
                    if self.announce_settled && self.announced {
                        self.reschedule_announce();
                    }
                    self.apply_radio_timing();
                    self.settings.save_radio(&cfg);
                    RADIO_CFG.signal(cfg);
                    self.range_radio_changed();
                }
                Err(e) => reply(self, deny(e)),
            },
            Ok(Command::SetRole { role }) => match self.set_role(role) {
                Ok(_) => {
                    let items = self.stats();
                    let more = self.more_stats();
                    for chunk in items.chunks(25).chain(more.chunks(25)) {
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
    /// clients do not carry. Smart carry (copies only to neighbours that
    /// move, routing on arrival, no 5-minute re-route loop): in the crisis
    /// simulation the same delivery for 31 % less airtime
    /// (docs/maybe/crisis-carry.md).
    fn apply_role(&mut self, role: Role) {
        self.router.set_role(role);
        self.router.carry_copies = if role == Role::Relay { RELAY_CARRY_COPIES } else { 0 };
        self.router.carry_smart = true;
    }

    /// Changes role and stores it in flash. Returns a log text.
    /// A direct message (or one taken out of a mail bundle): open, expand
    /// compressed text, and hand it to the app.
    fn on_data(&mut self, payload: &[u8], hops: u8) {
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
                if res.is_ok() {
                    let id = maarse_core::mail::mail_id(&self.addr, payload);
                    if self.got_mail.is_full() {
                        self.got_mail.remove(0);
                    }
                    let _ = self.got_mail.push(id);
                }
                // Compressed text is expanded before anything else looks at it.
                let res = match res {
                    Ok(mut o) if out.first() == Some(&content::TEXT_Z) => {
                        if let Some(v) = content::expand(&out[..o.len]) {
                            let m = v.len().min(out.len());
                            out[..m].copy_from_slice(&v[..m]);
                            o.len = m;
                        }
                        Ok(o)
                    }
                    other => other,
                };
                match res.map(|o| (o.from, o.len)) {
                    Ok((from, n)) => match content::parse(&out[..n]) {
                        Some(Content::Resync) => log("The contact started a new encryption session"),
                        Some(Content::Text(t)) => {
                            self.pin(from);
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
                    Err(CryptoError::UnknownSender(sender)) => {
                        // Keep it until the sender's announce arrives (the
                        // router has already acknowledged it), and ask for it.
                        if self.pending_rx.is_full() {
                            self.pending_rx.remove(0);
                        }
                        if let Ok(p) = heapless::Vec::from_slice(payload) {
                            let _ = self.pending_rx.push((p, hops));
                        }
                        self.ask_key(sender);
                        log("Message from an unknown sender: kept, asking for its key")
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

    /// Pins a contact we write to or hear from: never evicted, kept in flash
    /// (the only contacts written there, so the flash hardly wears).
    fn pin(&mut self, a: Address) {
        if self.pinned.contains(&a) {
            return;
        }
        let Some(id) = self.known.get(&a).copied() else { return };
        if self.pinned.is_full() {
            // The oldest pin makes room; rewrite the sector with the rest.
            self.pinned.remove(0);
            let _ = self.pinned.push(a);
            let FwNode { settings, known, pinned, .. } = self;
            settings.known_reset(&mut pinned.iter().filter_map(|p| known.get(p)).map(|p| p.to_bytes()));
            return;
        }
        let _ = self.pinned.push(a);
        if !self.settings.known_add(&id.to_bytes()) {
            let FwNode { settings, known, pinned, .. } = self;
            settings.known_reset(&mut pinned.iter().filter_map(|p| known.get(p)).map(|p| p.to_bytes()));
        }
    }

    /// Asks the network who `a` is (key request), at most every 30 s for
    /// the same contact.
    fn ask_key(&mut self, a: Address) {
        if self.key_asked.is_some_and(|(w, t)| w == a && t.elapsed() < Duration::from_secs(30)) {
            return;
        }
        self.key_asked = Some((a, Instant::now()));
        let actions = self.router.request_key(a);
        self.apply(actions);
    }

    /// Gives up on messages whose key did not come in time.
    fn release_await_key_expired(&mut self, _a: Address) {
        let mut i = 0;
        while i < self.await_key.len() {
            if self.await_key[i].3.elapsed() >= KEY_WAIT {
                let (req, ..) = self.await_key.remove(i);
                err(req, error_code::UNKNOWN_DEST, "The recipient's key did not come");
            } else {
                i += 1;
            }
        }
    }

    /// `a`'s key has come: send what waited for it; give up on old ones.
    fn release_await_key(&mut self, a: Address) {
        let mut i = 0;
        while i < self.await_key.len() {
            if self.await_key[i].1 == a {
                let (req, dest, plain, _) = self.await_key.remove(i);
                self.send_content(req, dest, &plain);
            } else if self.await_key[i].3.elapsed() >= KEY_WAIT {
                let (req, ..) = self.await_key.remove(i);
                err(req, error_code::UNKNOWN_DEST, "The recipient's key did not come");
            } else {
                i += 1;
            }
        }
    }

    /// Post offices, the client side (see `maarse_core::mail`): keep our home
    /// post office up to date (the best one our neighbors' hellos name),
    /// drop unacknowledged messages there after 45 s, and ask for our mail
    /// when we start, when the home office changes (then also ask the
    /// previous one), when its hello says mail is waiting, and every 6 hours.
    fn mail_step(&mut self) {
        let now = now_ms();
        let best = self.router.best_post_office(now).map(|(id, a, _)| (id, a));
        if best.is_some() && best.map(|b| b.0) != self.home_po.map(|h| h.0) {
            let prev = self.home_po.and_then(|h| h.1);
            self.home_po = best;
            log("New home post office");
            self.mail_sync();
            if let Some(p) = prev
                && Some(p) != best.and_then(|b| b.1)
            {
                // Moved: the previous office delivers what it holds, over
                // whatever route reaches us now.
                let mut body: heapless::Vec<u8, 32> = heapless::Vec::new();
                maarse_core::mail::id_body(&mut body, self.got_mail.iter().copied(), 8);
                self.send_deposit(p, self.addr, maarse_core::mail::OP_SYNC, &body);
            }
        }
        let since = self.last_sync.map(|t| t.elapsed());
        let hint = self.router.take_mail_hint();
        if self.home_po.is_some()
            && (since.is_none() && self.started.elapsed() > Duration::from_secs(40)
                || hint && since.is_none_or(|s| s >= MAIL_SYNC_MIN)
                || since.is_some_and(|s| s >= MAIL_SYNC_EVERY))
        {
            self.mail_sync();
        }
        // Unacknowledged messages go to the post office.
        let home = self.home_po.and_then(|h| h.1);
        let mut i = 0;
        while i < self.outbox.len() {
            let (_, dest, _, sent, deposited) = &self.outbox[i];
            let age = sent.elapsed();
            if age >= OUTBOX_GIVE_UP {
                self.outbox.remove(i);
                continue;
            }
            if !*deposited
                && age >= Duration::from_millis(maarse_core::mail::DEPOSIT_AFTER_MS)
                && let Some(po) = home
                && po != *dest
            {
                let (dest, sealed) = (*dest, self.outbox[i].2.clone());
                self.outbox[i].4 = true;
                self.send_deposit(po, dest, maarse_core::mail::OP_STORE, &sealed);
                log("Recipient not answering: message handed to our post office");
            }
            i += 1;
        }
    }

    /// "Do you have mail for me?" to the post offices within 2 hops.
    fn mail_sync(&mut self) {
        self.last_sync = Some(Instant::now());
        let ids: heapless::Vec<u32, 8> = self.got_mail.clone();
        let actions = self.router.request_mail(self.home_po.map(|h| h.0), &ids);
        self.apply(actions);
    }

    /// Sends a `Deposit` (`recipient | op | body`) to post office `po`.
    fn send_deposit(&mut self, po: Address, recipient: Address, op: u8, body: &[u8]) {
        let mut p: heapless::Vec<u8, 300> = heapless::Vec::new();
        if !maarse_core::mail::build(&mut p, &recipient, op, body) {
            return;
        }
        let mut token = [0u8; 16];
        self.rng.fill_bytes(&mut token);
        if let Ok((_, actions)) = self.router.send_typed(PacketType::Deposit, po, &p, token, now_ms()) {
            self.apply(actions);
        }
    }

    /// Mobile mode: a node whose best repeater keeps changing is moving (a
    /// car). It says so in hellos and announces, so routes to it go stale
    /// fast, refreshes its own routes, and never acts as an edge bridge.
    fn mobile_step(&mut self) {
        let now_ms = now_ms();
        let best = self
            .router
            .neighbors
            .iter()
            .filter(|n| matches!(n.role, Some(Role::Repeater | Role::Anchor)) && now_ms.saturating_sub(n.last_ms) < maarse_core::router::EDGE_WINDOW_MS)
            .max_by_key(|n| n.snr)
            .map(|n| n.id);
        if best.is_some() && best != self.best_repeater {
            if self.best_repeater.is_some() {
                if self.handovers.is_full() {
                    self.handovers.remove(0);
                }
                let _ = self.handovers.push(Instant::now());
            }
            self.best_repeater = best;
        }
        let recent = self.handovers.iter().filter(|t| t.elapsed() < MOBILE_WINDOW).count();
        let calm = self.handovers.last().is_none_or(|t| t.elapsed() >= MOBILE_CALM);
        if !self.router.mobile && recent >= MOBILE_HANDOVERS {
            self.router.mobile = true;
            log("Passing repeaters quickly: mobile mode on");
        } else if self.router.mobile && calm {
            self.router.mobile = false;
            log("Staying put: mobile mode off");
        }
    }

    fn set_role(&mut self, role: u8) -> Result<&'static str, &'static str> {
        if role != ROLE_AUTO && Role::from_u8(role).is_none() {
            return Err("Unknown role (0 = client, 1 = repeater, 2 = relay, 3 = anchor, 4 = auto)");
        }
        self.apply_role_value(role);
        self.settings.save_role(role);
        Ok(match Role::from_u8(role) {
            Some(Role::Client) => "Role saved: client",
            Some(Role::Repeater) => "Role saved: repeater",
            Some(Role::Relay) => "Role saved: relay",
            Some(Role::Anchor) => "Role saved: anchor",
            None => "Role saved: auto (relays when no repeater is in range)",
        })
    }

    /// A stored role value: a router role, or [`ROLE_AUTO`].
    fn apply_role_value(&mut self, role: u8) {
        self.auto_role = role == ROLE_AUTO;
        // Edge bridges forward path requests only across the edge.
        self.router.boundary = self.auto_role;
        if self.auto_role {
            // Start as a client; forest mode only if no repeater shows up.
            self.rep_last = Instant::now();
            self.rep_since = None;
            self.next_auto_check = Instant::now() + Duration::from_secs(15);
            self.apply_role(Role::Client);
        } else {
            self.apply_role(Role::from_u8(role).unwrap_or(Role::Repeater));
        }
    }

    /// Automatic forest mode: relay after [`AUTO_SWITCH`] without a repeater
    /// or anchor in range, client again after hearing one that long.
    fn auto_role_step(&mut self) {
        let now = Instant::now();
        let hears = self.router.hears_repeater(now_ms());
        if hears {
            self.rep_last = now;
            if self.rep_since.is_none() {
                self.rep_since = Some(now);
            }
        } else {
            self.rep_since = None;
        }
        // Edge bridges, after OLSR multipoint relays: nodes without coverage
        // choose a bridge (the nearest one heard announcing, lowest ID) and
        // name it in their announces. We stay a bridge while a neighbor names
        // us, and step in for a neighbor without coverage that has no bridge
        // yet, once we have had steady coverage for 2 min (after a mass
        // restart every neighbor first looks uncovered). A client steps in
        // with chance 1 / (1 + neighbors that hear a repeater too), at least
        // 1/4 per check, so the person on a hill takes the job at once while
        // nodes along a town's coverage edge leave room for the first one.
        // maarse-sim (canopy model, 8 seeds): forest 97 % delivered and 96 %
        // of messages into and out of the forest (49 % without bridges);
        // town 93 % into and out of the uncovered part (78 %) at 32 % more
        // airtime (see docs/denmark/README.md, "Edge bridges").
        // A moving node (car) never bridges: it would leave the group soon.
        let steady = !self.router.mobile && (self.router.role == Role::Relay || self.rep_since.is_some_and(|s| now - s >= AUTO_SWITCH));
        let mut edge = false;
        if hears && steady {
            edge = self.router.unserved_neighbor(now_ms()) || (self.router.role == Role::Relay && self.router.bridge_named(now_ms()));
            if edge && self.router.role == Role::Client {
                let rivals = self.router.covered_neighbors(now_ms()) as u32;
                // Chance max(1 / (1 + rivals), 1/4).
                edge = rivals <= 3 || self.rng.next_u32() % 4 == 0;
                if edge && rivals > 0 && rivals <= 3 {
                    edge = self.rng.next_u32() % (1 + rivals) == 0;
                }
            }
        }
        if edge {
            self.rep_since = Some(now);
        }
        let switched = match self.router.role {
            Role::Client if edge => {
                self.apply_role(Role::Relay);
                log("A neighbor without coverage has no bridge: relaying for it (edge bridge)");
                true
            }
            Role::Client if !hears && now - self.rep_last >= AUTO_SWITCH => {
                self.apply_role(Role::Relay);
                log("No repeater in range: forest mode on (this node relays)");
                true
            }
            Role::Relay if hears && !edge && self.rep_since.is_some_and(|s| now - s >= AUTO_SWITCH) => {
                self.apply_role(Role::Client);
                log("A repeater is in range and no neighbor needs us as a bridge: forest mode off");
                true
            }
            _ => false,
        };
        if switched {
            // Neighbors learn our role (and whether we bridge) only from
            // announces, which are 12 h apart once settled: tell them soon.
            let soon = Instant::now() + Duration::from_millis(5000 + self.rng.next_u64() % 30_000);
            self.next_announce = self.next_announce.min(soon);
        }
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
        put(stat::ROLE, if self.auto_role { ROLE_AUTO as i32 } else { self.router.role.as_u8() as i32 });
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

    /// The second stats frame (the first one is full): post office and the
    /// router counters added since.
    fn more_stats(&mut self) -> Stats {
        let mut items = Stats::new();
        let (r, m) = self.settings.mail_count();
        let st = self.router.stats;
        for (k, v) in [
            (stat::MAILBOX_RECIPIENTS, r as i32),
            (stat::MAILBOX_MESSAGES, m as i32),
            (stat::MAIL_DELIVERED, self.mail_delivered as i32),
            (stat::HELLOS, st.hellos as i32),
            (stat::KEY_REQUESTS, st.key_requests as i32),
            (stat::LOCAL_REPAIRS, st.local_repairs as i32),
            (stat::CARRIED, self.router.carried_count() as i32),
            (stat::PINNED, self.pinned.len() as i32),
            (stat::MOBILE, self.router.mobile as i32),
            (stat::QUOTA_DROPPED, st.quota_dropped as i32),
            (stat::AWAIT_KEY, self.await_key.len() as i32),
            (stat::OUTBOX, self.outbox.len() as i32),
            (stat::MODEM_FALLBACK, self.modem_fallback as i32),
        ] {
            let _ = items.push((k, v));
        }
        items
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

/// Logs a packet the router dropped for a bad proof-of-work stamp, with what
/// is needed to tell a wrong stamp from a damaged or padded packet.
fn log_bad_stamp(pkt: &[u8], snr: i8) {
    use core::fmt::Write as _;
    let mut s: heapless::String<160> = heapless::String::new();
    let ptype = pkt.first().map_or(0xFF, |f| (f >> 2) & 0x0F);
    let _ = write!(s, "Dropped a packet with a bad stamp: type {ptype}, {} bytes, SNR {snr}, ends", pkt.len());
    for b in &pkt[pkt.len().saturating_sub(8)..] {
        let _ = write!(s, " {b:02x}");
    }
    log(&s);
}
