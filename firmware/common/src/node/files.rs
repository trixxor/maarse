//! Billeder og filer på firmware-noder (op til [`MAX_FILE`] = 8 KB).
//!
//! Samme format som maarse-host (filtilbud som direkte besked, bidder som
//! `Fragment`-pakker med kvittering pr. bid), så firmware og host kan sende
//! filer til hinanden. Tilstandsmaskinerne ligger i
//! [`maarse_crypto::files`] og testes på en PC; her er kun limen til
//! radio, router og klient.
//!
//! * **Afsendelse**: klienten overfører filen med `FileBegin`/`FileData`/
//!   `FileEnd` til en statisk buffer (én fil ad gangen). Noden sender
//!   tilbuddet, venter på kvittering og sender så bidderne med et vindue på
//!   [`WINDOW`]. Bidder sendes kun, mens radioens sendekø er kort, så en
//!   travl kanal eller duty cycle-grænsen bremser overførslen i stedet for at
//!   fylde køen. Bidder uden kvittering sendes igen efter 30 s; uden
//!   fremskridt i 5 minutter opgives filen.
//! * **Modtagelse**: én fil ad gangen samles i en anden statisk buffer og
//!   tjekkes (SHA-256). Derefter sendes den til klienten som `FileChunk`,
//!   i det tempo USB/Bluetooth kan tage dem. Er der ingen klient (køerne
//!   flyder over i 30 s), bliver filen liggende i RAM, og den sendes igen,
//!   når en klient forbinder (`GetInfo`). Den overskrives af den næste fil,
//!   der modtages, og forsvinder ved genstart.
//! * **Postkassen** (8 KB flash) bruges ikke til filer: én fil ville fylde
//!   den helt og skubbe almindelige beskeder ud, og flash slides af store
//!   skrivninger. En fil til en modtager, der er offline, mislykkes derfor
//!   hos afsenderen (som på maarse-host).

use super::{FwNode, Settings, emit, err, log, now_ms};
use crate::bus::{BLE_OUT, RSTATS, USB_OUT};
use core::sync::atomic::Ordering::Relaxed;
use embassy_time::{Duration, Instant};
use maarse_core::Address;
use maarse_core::companion::{Event, MAX_TEXT, error_code, text};
use maarse_core::content::{FileOffer, OFFER_FIXED};
use maarse_crypto::files::{
    AckResult, ChunkResult, Incoming, MAX_FILE, Outgoing, TickResult, Upload, WINDOW,
};
use maarse_crypto::ratchet::MAX_FS_TEXT;
use maarse_crypto::seal::MAX_DM_TEXT;
use rand_core::{CryptoRng, RngCore};
use static_cell::ConstStaticCell;

static OUT_BUF: ConstStaticCell<[u8; MAX_FILE]> = ConstStaticCell::new([0; MAX_FILE]);
static IN_BUF: ConstStaticCell<[u8; MAX_FILE]> = ConstStaticCell::new([0; MAX_FILE]);

/// Nye bidder sendes kun, mens radioens sendekø er kortere end dette.
const TX_QUEUE_LIMIT: u32 = 4;
/// Ledig plads i en klientkø, før vi sender endnu en `FileChunk`.
const CLIENT_ROOM: usize = 4;
/// Uden plads i klientkøerne så længe antages det, at ingen klient lytter.
const NO_CLIENT: Duration = Duration::from_secs(30);

/// En modtaget fil, der sendes (eller venter på at blive sendt) til klienten.
struct Stream {
    from: Address,
    offer: FileOffer,
    pos: u32,
    last_push: Instant,
    /// Ingen klient lyttede; sendes igen ved næste `GetInfo`.
    waiting: bool,
}

pub(super) struct Files {
    out_buf: &'static mut [u8; MAX_FILE],
    in_buf: &'static mut [u8; MAX_FILE],
    upload: Option<Upload>,
    out: Option<Outgoing>,
    /// Det indkodede tilbud, hvis det skal sendes igen.
    offer: heapless::Vec<u8, 160>,
    incoming: Option<Incoming>,
    stream: Option<Stream>,
}

impl Files {
    /// Må kun kaldes én gang (bufferne er statiske).
    pub(super) fn new() -> Self {
        Files {
            out_buf: OUT_BUF.take(),
            in_buf: IN_BUF.take(),
            upload: None,
            out: None,
            offer: heapless::Vec::new(),
            incoming: None,
            stream: None,
        }
    }

    /// Hvornår filerne skal have opmærksomhed igen.
    pub(super) fn deadline(&self) -> Option<Instant> {
        if self.stream.as_ref().is_some_and(|s| !s.waiting) {
            Some(Instant::now() + Duration::from_millis(20))
        } else if self.out.is_some() || self.incoming.is_some() {
            Some(Instant::now() + Duration::from_secs(1))
        } else {
            None
        }
    }
}

/// Afkorter UTF-8 til højst `n` bytes uden at skære et tegn over.
fn clip(b: &[u8], mut n: usize) -> &[u8] {
    if b.len() <= n {
        return b;
    }
    while n > 0 && b[n] & 0xC0 == 0x80 {
        n -= 1;
    }
    &b[..n]
}

impl<R: RngCore + CryptoRng, S: Settings> FwNode<R, S> {
    // ---------- fra klienten ----------

    pub(super) fn file_begin(&mut self, req: u16, dest: Address, size: u32, mime: &[u8], name: &[u8]) {
        if self.files.out.is_some() {
            return err(req, error_code::QUEUE_FULL, "Der sendes allerede en fil; vent til den er færdig");
        }
        if !self.known.contains_key(&dest) {
            return err(req, error_code::UNKNOWN_DEST, "Modtageren er ikke hørt endnu");
        }
        // Tilbuddet skal kunne være i én krypteret besked.
        let room = MAX_FS_TEXT.min(MAX_DM_TEXT) - OFFER_FIXED;
        let mime = clip(mime, 40.min(room));
        let name = clip(name, room - mime.len());
        match Upload::new(req, dest, size, mime, name) {
            Ok(u) => self.files.upload = Some(u),
            Err(e) => err(req, error_code::FILE_TOO_LARGE, e),
        }
    }

    pub(super) fn file_data(&mut self, req: u16, offset: u32, data: &[u8]) {
        let f = &mut self.files;
        if let Some(u) = f.upload.as_mut()
            && u.req == req
        {
            u.push(offset, data, &mut f.out_buf[..]);
        }
    }

    pub(super) fn file_end(&mut self, req: u16) {
        let Some(up) = self.files.upload.take_if(|u| u.req == req) else {
            return err(req, error_code::NO_FILE, "Ingen fil under upload");
        };
        if !up.complete() {
            return err(req, error_code::NO_FILE, "Filen blev ikke overført helt");
        }
        let mut id = [0u8; 8];
        self.rng.fill_bytes(&mut id);
        let mut key = [0u8; 32];
        self.rng.fill_bytes(&mut key);
        let data = &self.files.out_buf[..up.size as usize];
        let (mut out, offer) =
            match Outgoing::new(u64::from_le_bytes(id), key, up.dest, data, &up.mime, &up.name, now_ms()) {
                Ok(x) => x,
                Err(e) => return err(req, error_code::FILE_TOO_LARGE, e),
            };
        let enc = offer.encode();
        match self.send_plain(up.dest, &enc) {
            Ok(msg_id) => {
                out.offer_sent(msg_id, now_ms());
                emit(&Event::FileStarted { req, file_id: out.file_id, chunks: out.chunks });
                log("Sender fil");
                self.files.offer = enc;
                self.files.out = Some(out);
            }
            Err(e) => err(req, error_code::UNKNOWN_DEST, e),
        }
    }

    // ---------- afsendelse ----------

    /// Er kvitteringen til en fil? Så håndteres den her.
    pub(super) fn file_on_ack(&mut self, msg_id: u64) -> bool {
        let Some(out) = self.files.out.as_mut() else {
            return false;
        };
        let file_id = out.file_id;
        match out.on_ack(msg_id, now_ms()) {
            None => return false,
            Some(AckResult::OfferAcked) => {}
            Some(AckResult::Progress { done, total }) => {
                emit(&Event::FileProgress { file_id, done, total, outgoing: true });
                if out.finished() {
                    self.files.out = None;
                    log("Fil leveret");
                    emit(&Event::FileDone { file_id, ok: true, outgoing: true, text: text(b"leveret") });
                    return true;
                }
            }
        }
        self.file_pump();
        true
    }

    /// Sender bidder, så længe vinduet og radioens kø tillader det.
    fn file_pump(&mut self) {
        for _ in 0..WINDOW {
            if RSTATS.tx_queue.load(Relaxed) >= TX_QUEUE_LIMIT {
                return;
            }
            let Some(out) = self.files.out.as_ref() else {
                return;
            };
            let Some(idx) = out.next_chunk() else {
                return;
            };
            let mut ct = [0u8; 255];
            let Some(n) = out.seal_chunk(idx, &self.files.out_buf[..], &mut ct) else {
                return;
            };
            let dest = out.dest;
            let Ok((msg_id, actions)) = self.router.send_fragment(dest, &ct[..n], now_ms()) else {
                return; // Ingen rute lige nu; prøves igen ved næste tick.
            };
            if let Some(out) = self.files.out.as_mut() {
                out.chunk_sent(idx, msg_id, now_ms());
            }
            self.apply(actions);
        }
    }

    // ---------- modtagelse ----------

    pub(super) fn file_offer(&mut self, from: Address, offer: FileOffer) {
        if let Some(inc) = &self.files.incoming {
            if inc.file_id() == offer.file_id {
                return; // Tilbuddet igen (kvitteringen gik tabt).
            }
            if !inc.stalled(now_ms()) {
                return log("Fil afvist: noden modtager allerede en fil");
            }
        }
        let file_id = offer.file_id;
        let (size, mime, name) = (offer.size, text(&offer.mime), text(&offer.name));
        match Incoming::accept(from, offer, MAX_FILE, now_ms()) {
            Ok(inc) => {
                if self.files.stream.take().is_some() {
                    log("En ældre fil, som ikke nåede klienten, er overskrevet");
                }
                self.files.incoming = Some(inc);
                emit(&Event::FileIncoming { file_id, from, size, mime, name });
                log("Fil på vej");
            }
            Err(_) if size as usize > MAX_FILE => log("Fil afvist: større end 8 KB"),
            Err(_) => log("Fil afvist: ugyldigt tilbud"),
        }
    }

    pub(super) fn file_chunk(&mut self, payload: &[u8]) {
        let f = &mut self.files;
        let Some(inc) = f.incoming.as_mut() else {
            return;
        };
        let file_id = inc.file_id();
        match inc.on_chunk(payload, &mut f.in_buf[..], now_ms()) {
            ChunkResult::Ignored => {}
            ChunkResult::Progress { got, total } => {
                emit(&Event::FileProgress { file_id, done: got, total, outgoing: false });
            }
            ChunkResult::Complete => {
                let total = inc.offer.chunks;
                emit(&Event::FileProgress { file_id, done: total, total, outgoing: false });
                if let Some(inc) = f.incoming.take() {
                    f.stream = Some(Stream {
                        from: inc.from,
                        offer: inc.offer,
                        pos: 0,
                        last_push: Instant::now(),
                        waiting: false,
                    });
                }
                log("Fil modtaget");
                self.file_stream();
            }
            ChunkResult::BadHash => {
                f.incoming = None;
                emit(&Event::FileDone { file_id, ok: false, outgoing: false, text: text(b"kontrolsum passer ikke") });
            }
        }
    }

    /// Sender den modtagne fil til klienten i det tempo, køerne tillader.
    fn file_stream(&mut self) {
        let f = &mut self.files;
        let Some(s) = f.stream.as_mut() else {
            return;
        };
        if s.waiting {
            return;
        }
        let size = s.offer.size;
        let file_id = s.offer.file_id;
        while s.pos < size {
            if USB_OUT.free_capacity() < CLIENT_ROOM && BLE_OUT.free_capacity() < CLIENT_ROOM {
                if s.last_push.elapsed() > NO_CLIENT {
                    s.waiting = true;
                    log("Ingen klient: filen gemmes i RAM, til en klient forbinder");
                }
                return;
            }
            let end = (s.pos as usize + MAX_TEXT).min(size as usize);
            emit(&Event::FileChunk { file_id, offset: s.pos, data: text(&f.in_buf[s.pos as usize..end]) });
            s.pos = end as u32;
            s.last_push = Instant::now();
        }
        emit(&Event::FileDone { file_id, ok: true, outgoing: false, text: text(&s.offer.name) });
        f.stream = None;
    }

    /// En klient har forbundet (`GetInfo`): send en ventende fil igen.
    pub(super) fn file_client_connected(&mut self) {
        if let Some(s) = self.files.stream.as_mut()
            && s.waiting
        {
            s.waiting = false;
            s.pos = 0;
            s.last_push = Instant::now();
            emit(&Event::FileIncoming {
                file_id: s.offer.file_id,
                from: s.from,
                size: s.offer.size,
                mime: text(&s.offer.mime),
                name: text(&s.offer.name),
            });
        }
    }

    // ---------- timer ----------

    pub(super) fn file_tick(&mut self) {
        let now = now_ms();
        if let Some(out) = self.files.out.as_mut() {
            let file_id = out.file_id;
            let dest = out.dest;
            match out.tick(now) {
                TickResult::Idle => self.file_pump(),
                TickResult::ResendOffer => {
                    let offer = self.files.offer.clone();
                    if let Ok(msg_id) = self.send_plain(dest, &offer)
                        && let Some(out) = self.files.out.as_mut()
                    {
                        out.offer_sent(msg_id, now);
                    }
                }
                TickResult::Failed => {
                    self.files.out = None;
                    log("Filen blev ikke leveret");
                    emit(&Event::FileDone {
                        file_id,
                        ok: false,
                        outgoing: true,
                        text: text(b"modtageren svarede ikke"),
                    });
                }
            }
        }
        if let Some(inc) = self.files.incoming.take_if(|i| i.stalled(now)) {
            emit(&Event::FileDone {
                file_id: inc.file_id(),
                ok: false,
                outgoing: false,
                text: text(b"overforsel afbrudt"),
            });
        }
        self.file_stream();
    }
}
