//! Noden: routing, kryptering, announces og companion-kommandoer.
//!
//! Modem-tilstand: sender en maarse-host rammen `0x41`, overtager hosten
//! routingen, og noden er kun radio. `0x40 | pakke` sendes over LoRa, og
//! alt modtaget sendes til hosten som `0xC0 | snr | pakke`.

use crate::bus::{
    Frame, Pkt, RADIO_CANCEL, RADIO_CFG, RADIO_RX, RADIO_TX, RSTATS, TxReq, USB_IN, USB_OUT,
};
use core::sync::atomic::Ordering::Relaxed;
use embassy_futures::select::{Either3, select3};
use embassy_futures::yield_now;
use embassy_time::{Duration, Instant, Timer};
use heapless::FnvIndexMap;
use rand_core::{CryptoRng, RngCore};
use maarse_core::companion::{Command, Event, PROTOCOL_VERSION, error_code, text};
use maarse_core::companion::{RadioConfig, Stats, stat};
use maarse_core::content::{self, Content};
use maarse_core::packet::{Header, encode};
use maarse_core::stamp::StampSearch;
use maarse_core::{Action, Address, PacketType, Role, Router};
use maarse_crypto::seal::{self, CryptoError, MAX_DM_TEXT};
use maarse_crypto::{
    Identity, PublicIdentity, channel_address, hashtag_key, public_channel_key, verify_announce,
};

const ANNOUNCE_EVERY: Duration = Duration::from_secs(900);
/// Forsøg pr. bid, når et announce-stempel regnes ud. 256 SHA-256-blokke
/// tager ca. 20 ms på en 64 MHz Cortex-M4; derefter får executoren fri.
const STAMP_CHUNK: u32 = 256;

type Name = heapless::String<24>;

/// Hvor radioindstillinger gemmes (flash på boardet).
pub trait Settings {
    fn save_radio(&mut self, cfg: &RadioConfig);
    fn save_admins(&mut self, admins: &[Address]);
    fn load_admins(&mut self) -> heapless::Vec<Address, 4>;
    /// Gemmer nodens rolle. Standard: gemmes ikke.
    fn save_role(&mut self, _role: Role) {}
    /// Gemt rolle; standard (og ældre flash uden rolle) er repeater.
    fn load_role(&mut self) -> Role {
        Role::Repeater
    }

    // ----- postkasse (valgfri; standard: ingen) -----

    /// Gemmer en krypteret besked til `recipient`. Returnerer et ID.
    fn mail_add(&mut self, _recipient: &Address, _sealed: &[u8]) -> Option<u32> {
        None
    }
    /// Kalder `f(id, besked)` for hver gemt besked til `recipient`.
    fn mail_for(&mut self, _recipient: &Address, _f: &mut dyn FnMut(u32, &[u8])) {}
    fn mail_remove(&mut self, _id: u32) {}
    /// (antal modtagere, antal beskeder)
    fn mail_count(&mut self) -> (u32, u32) {
        (0, 0)
    }
}

pub struct FwNode<R: RngCore + CryptoRng, S: Settings> {
    router: Router<256, 1024>,
    identity: Identity,
    addr: Address,
    known: FnvIndexMap<Address, PublicIdentity, 64>,
    channels: heapless::Vec<(Address, [u8; 32], Name), 4>,
    rng: R,
    modem: bool,
    radio_ok: bool,
    next_announce: Instant,
    settings: S,
    radio: RadioConfig,
    started: Instant,
    admins: heapless::Vec<Address, 4>,
    control_seen: FnvIndexMap<Address, u32, 4>,
    /// Unix-tid ved opstart, når klienten har fortalt os klokken.
    boot_unix: Option<u32>,
    /// Post fra postkassen, der er sendt og venter på kvittering: msg_id -> post-ID.
    mail_inflight: FnvIndexMap<u64, u32, 16>,
    mail_delivered: u32,
    /// Announce, hvis arbejdsbevis er ved at blive regnet ud (se `run`).
    stamping: Option<(maarse_crypto::AnnounceBuf, StampSearch)>,
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
        // Rollen vælges i appen (`SetRole`) og hentes fra flash nedenfor;
        // standard er repeater (fast strøm).
        let router = Router::new(addr, Role::Repeater);
        let mut n = FwNode {
            router,
            identity,
            addr,
            known: FnvIndexMap::new(),
            channels: heapless::Vec::new(),
            rng,
            modem: false,
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
        };
        n.admins = n.settings.load_admins();
        let role = n.settings.load_role();
        n.router.set_role(role);
        n.join("offentlig");
        n
    }

    /// Hovedløkken. Er et announce-stempel ved at blive regnet ud (ca.
    /// 4.096 forsøg, 0,3 s på en nRF52840), tages det i bidder på
    /// [`STAMP_CHUNK`] forsøg. Mellem bidderne giver vi executoren fri med
    /// `yield_now`, så radio, USB og Bluetooth ikke venter mere end ca. 20 ms,
    /// og modtagne pakker og kommandoer behandles som normalt.
    pub async fn run(&mut self) -> ! {
        loop {
            if self.stamping.is_some() {
                self.stamp_step();
                match select3(RADIO_RX.receive(), USB_IN.receive(), yield_now()).await {
                    Either3::First((pkt, snr)) => self.on_radio(pkt, snr),
                    Either3::Second(frame) => self.on_frame(frame),
                    Either3::Third(()) => {
                        if Instant::now() >= self.next_deadline() {
                            self.on_timer();
                        }
                    }
                }
                continue;
            }
            let deadline = self.next_deadline();
            match select3(RADIO_RX.receive(), USB_IN.receive(), Timer::at(deadline)).await {
                Either3::First((pkt, snr)) => self.on_radio(pkt, snr),
                Either3::Second(frame) => self.on_frame(frame),
                Either3::Third(()) => self.on_timer(),
            }
        }
    }

    /// Ét bid af stempel-søgningen. Sender announcet, når stemplet er fundet.
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
        let mut d = self.next_announce;
        if let Some(t) = self.router.next_deadline() {
            let t = Instant::from_millis(t);
            if t < d {
                d = t;
            }
        }
        d
    }

    fn on_timer(&mut self) {
        let actions = self.router.tick(now_ms());
        self.apply(actions);
        if Instant::now() >= self.next_announce {
            if !self.modem {
                self.announce();
            }
            self.next_announce = Instant::now() + ANNOUNCE_EVERY;
        }
    }

    fn transmit(&self, packet: Pkt, delay_ms: u32, id: u64) {
        let _ = RADIO_TX.try_send(TxReq {
            packet,
            delay_ms,
            id,
        });
    }

    fn announce(&mut self) {
        let ts = self.unix_now().unwrap_or((now_ms() / 1000) as u32);
        let Some(payload) = self.identity.announce(ts, &[]) else {
            return;
        };
        // Stemplet regnes ud i baggrunden af `run` (se `stamp_step`). Et nyt
        // announce erstatter et, der endnu ikke er færdigt.
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
                Action::Acked { msg_id } => {
                    if let Some(id) = self.mail_inflight.remove(&msg_id) {
                        self.settings.mail_remove(id);
                        self.mail_delivered += 1;
                    } else {
                        emit(&Event::Acked { msg_id });
                    }
                }
            }
        }
    }

    // ---------- radio ----------

    fn on_radio(&mut self, pkt: Pkt, snr: i8) {
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
        let actions = self.router.on_receive(&pkt, snr, now_ms());
        self.apply(actions);
    }

    fn on_deliver(&mut self, ptype: PacketType, dest: Address, payload: &[u8], hops: u8) {
        match ptype {
            PacketType::Announce if dest != self.addr => {
                if let Ok(a) = verify_announce(&dest, payload) {
                    if self.known.insert(dest, a.identity).is_err() {
                        // Fuld: glem den ældste.
                        if let Some(k) = self.known.keys().next().copied() {
                            self.known.remove(&k);
                        }
                        let _ = self.known.insert(dest, a.identity);
                    }
                    crate::bus::KNOWN_NODES.store(self.known.len() as u32, Relaxed);
                    emit(&Event::Heard { addr: dest, hops });
                    self.flush_mail(dest);
                }
            }
            PacketType::Data => {
                let mut out = [0u8; 256];
                let known = &self.known;
                match seal::open_direct(
                    &self.identity,
                    payload,
                    |a| known.get(a).copied(),
                    &mut out,
                ) {
                    Ok((from, n)) => match content::parse(&out[..n]) {
                        Some(Content::Text(t)) => {
                            crate::bus::note_message(&from, t);
                            emit(&Event::DirectReceived {
                                from,
                                hops,
                                text: text(t),
                            })
                        }
                        Some(Content::FileOffer(_)) => {
                            log("Filer understøttes endnu ikke af denne node")
                        }
                        Some(Content::Control(counter, frame)) => {
                            let mut f: heapless::Vec<u8, 256> = heapless::Vec::new();
                            let _ = f.extend_from_slice(frame);
                            self.on_control(from, counter, &f);
                        }
                        Some(Content::ControlReply(_, frame)) => {
                            emit(&Event::RemoteReply { from, frame: text(frame) })
                        }
                        _ => {}
                    },
                    Err(CryptoError::UnknownSender(_)) => {
                        log("Besked fra ukendt afsender (intet announce endnu)")
                    }
                    Err(_) => {}
                }
            }
            PacketType::Deposit => {
                // Vi er postkasse for en anden: gem beskeden til modtageren.
                if let Some(recipient) = Address::from_slice(payload) {
                    if self.settings.mail_add(&recipient, &payload[16..]).is_some() {
                        self.flush_mail(recipient);
                    } else {
                        log("Postkassen er fuld");
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
            return err(0, error_code::BAD_FRAME, "ugyldig ramme");
        };
        // Status og radioindstillinger må hentes uden at forlade modem-tilstand.
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
                    emit(&Event::Radio(cfg));
                    log("Radioindstillinger gemt");
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
                    max_text: (MAX_DM_TEXT - 1) as u8,
                    max_file: 0,
                });
                for (ch, _, name) in &self.channels {
                    emit(&Event::ChannelJoined {
                        channel: *ch,
                        name: text(name.as_bytes()),
                    });
                }
                for a in self.known.keys() {
                    emit(&Event::Heard { addr: *a, hops: 0 });
                }
                if !self.radio_ok {
                    log("Radioen svarer ikke - tjek board-valget");
                }
            }
            Command::AnnounceNow => {
                self.announce();
                log("Announce sendes, når arbejdsbeviset er regnet ud");
            }
            Command::JoinChannel { name } => {
                let name = core::str::from_utf8(&name).unwrap_or("");
                self.join(name);
            }
            Command::SendDirect {
                req,
                dest,
                text: msg,
            } => {
                let Some(recipient) = self.known.get(&dest).copied() else {
                    return err(
                        req,
                        error_code::UNKNOWN_DEST,
                        "Modtageren er ikke hørt endnu",
                    );
                };
                let plain = content::text(&msg);
                let mut ct = [0u8; 255];
                let Ok(n) =
                    seal::seal_direct(&self.identity, &recipient, &mut self.rng, &plain, &mut ct)
                else {
                    return err(req, error_code::TOO_LARGE, "Beskeden er for lang");
                };
                let mut token = [0u8; 16];
                self.rng.fill_bytes(&mut token);
                match self.router.send_direct(dest, &ct[..n], token, now_ms()) {
                    Ok((msg_id, actions)) => {
                        emit(&Event::Sent { req, msg_id });
                        self.apply(actions);
                    }
                    Err(_) => err(req, error_code::QUEUE_FULL, "Køen er fuld"),
                }
            }
            Command::SendChannel {
                req,
                channel,
                text: msg,
            } => {
                let Some((_, key, _)) = self.channels.iter().find(|(a, _, _)| *a == channel) else {
                    return err(req, error_code::NOT_IN_CHANNEL, "Ikke med i kanalen");
                };
                let key = *key;
                let mut nonce = [0u8; 8];
                self.rng.fill_bytes(&mut nonce);
                let mut ct = [0u8; 255];
                let res = seal::seal_channel(&key, &self.addr, nonce, &msg, &mut ct)
                    .ok()
                    .and_then(|n| self.router.send_channel(channel, &ct[..n]).ok());
                match res {
                    Some(actions) => {
                        emit(&Event::Sent { req, msg_id: 0 });
                        self.apply(actions);
                    }
                    None => err(req, error_code::TOO_LARGE, "Beskeden er for lang"),
                }
            }
            Command::Remote { req, dest, frame } => {
                if !matches!(
                    Command::decode(&frame),
                    Ok(Command::GetStats | Command::GetRadio | Command::SetRadio(_) | Command::SetRole { .. })
                ) {
                    return err(req, error_code::BAD_FRAME, "Kun status, radio og rolle kan fjernstyres");
                }
                let Some(counter) = self.unix_now() else {
                    return err(req, error_code::BAD_FRAME, "Klokken er ikke sat (forbind igen)");
                };
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
            Command::SetTime { unix } => {
                self.boot_unix = Some(unix.saturating_sub(self.started.elapsed().as_secs() as u32));
            }
            Command::FileBegin { req, .. }
            | Command::FileData { req, .. }
            | Command::FileEnd { req } => {
                err(
                    req,
                    error_code::FILE_TOO_LARGE,
                    "Filer understøttes endnu ikke af denne node",
                );
            }
        }
    }

    /// Prøver at levere post til `recipient` (hvis vi har nogen).
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

    /// Krypterer og sender en direkte besked.
    fn send_plain(&mut self, dest: Address, plain: &[u8]) -> Result<u64, &'static str> {
        let recipient = *self.known.get(&dest).ok_or("Modtageren er ikke hørt endnu")?;
        let mut ct = [0u8; 255];
        let n = seal::seal_direct(&self.identity, &recipient, &mut self.rng, plain, &mut ct)
            .map_err(|_| "Beskeden er for lang")?;
        let mut token = [0u8; 16];
        self.rng.fill_bytes(&mut token);
        let (msg_id, actions) = self
            .router
            .send_direct(dest, &ct[..n], token, now_ms())
            .map_err(|_| "Køen er fuld")?;
        self.apply(actions);
        Ok(msg_id)
    }

    /// En kontrolkommando over radio. Afsenderen er bevist af krypteringen
    /// og skal stå på listen over administratorer.
    fn on_control(&mut self, from: Address, counter: u32, frame: &[u8]) {
        let reply = |node: &mut Self, ev: Event| {
            let f = ev.encode();
            let plain = content::control(content::CONTROL_REPLY, counter, &f[2..]);
            let _ = node.send_plain(from, &plain);
        };
        let deny = |msg: &str| Event::Error { req: 0, code: error_code::BAD_FRAME, text: text(msg.as_bytes()) };
        if !self.admins.contains(&from) {
            return reply(self, deny("Du er ikke administrator for denne node"));
        }
        // Tælleren (afsenderens ur) skal stige; kender vi klokken, må den
        // højst være 10 minutter fra vores.
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
                    // Svar først med de gamle indstillinger aktive, så svaret
                    // kan nå frem; skift derefter.
                    reply(self, Event::Radio(cfg));
                    self.radio = cfg;
                    self.settings.save_radio(&cfg);
                    RADIO_CFG.signal(cfg);
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
            _ => reply(self, deny("Kommandoen kan ikke fjernstyres")),
        }
    }

    /// Skifter rolle og gemmer den i flash. Returnerer en logtekst.
    fn set_role(&mut self, role: u8) -> Result<&'static str, &'static str> {
        let r = Role::from_u8(role).ok_or("Ukendt rolle (0 = klient, 1 = repeater, 2 = relay)")?;
        self.router.set_role(r);
        self.settings.save_role(r);
        Ok(match r {
            Role::Client => "Rolle gemt: klient",
            Role::Repeater => "Rolle gemt: repeater",
            Role::Relay => "Rolle gemt: relay",
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
        items
    }

    fn mail_stats(&mut self, items: &mut Stats) {
        let (r, m) = self.settings.mail_count();
        let _ = items.push((stat::MAILBOX_RECIPIENTS, r as i32));
        let _ = items.push((stat::MAILBOX_MESSAGES, m as i32));
        let _ = items.push((stat::MAIL_DELIVERED, self.mail_delivered as i32));
    }

    fn join(&mut self, name: &str) {
        let lower = name.trim();
        let (label, key): (&str, [u8; 32]) = match lower {
            "" | "offentlig" | "#offentlig" | "public" | "#public" => {
                ("offentlig", public_channel_key())
            }
            n => (n, {
                // Hashtag-kanaler er altid små bogstaver og med '#'.
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
        let addr = channel_address(&key);
        if !self.channels.iter().any(|(a, _, _)| *a == addr) {
            let mut nm = Name::new();
            if label != "offentlig" && !label.starts_with('#') {
                let _ = nm.push('#');
            }
            for c in label.chars() {
                for l in c.to_lowercase() {
                    let _ = nm.push(l);
                }
            }
            if self.channels.push((addr, key, nm)).is_err() {
                return log("Højst 4 kanaler på denne node");
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
