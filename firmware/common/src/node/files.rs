//! Pictures and files on firmware nodes (up to [`MAX_FILE`] = 8 KB).
//!
//! The same format as maarse-host (file offer as a direct message, chunks as
//! `Fragment` packets with an acknowledgement per chunk), so firmware and host can send
//! files to each other. The state machines are in
//! [`maarse_crypto::files`] and are tested on a PC; here is only the glue to
//! radio, router and client.
//!
//! * **Sending**: the client transfers the file with `FileBegin`/`FileData`/
//!   `FileEnd` into a static buffer (one file at a time). The node sends
//!   the offer, waits for the acknowledgement and then sends the chunks with a window of
//!   [`WINDOW`]. Chunks are only sent while the radio's send queue is short, so a
//!   busy channel or the duty cycle limit slows the transfer down instead of
//!   filling the queue. Chunks without acknowledgement are sent again after 30 s; without
//!   progress for 5 minutes the file is abandoned.
//! * **Receiving**: one file at a time is collected in another static buffer and
//!   checked (SHA-256). Then it is sent to the client as `FileChunk`,
//!   at the pace USB/Bluetooth can take them. If there is no client (the queues
//!   overflow for 30 s), the file stays in RAM and is sent again
//!   when a client connects (`GetInfo`). It is overwritten by the next file
//!   received, and disappears on restart.
//! * **The mailbox** (8 KB flash) is not used for files: one file would fill
//!   it completely and push ordinary messages out, and flash wears from large
//!   writes. A file to a recipient who is offline therefore fails
//!   at the sender (as on maarse-host).

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

/// New chunks are only sent while the radio's send queue is shorter than this.
const TX_QUEUE_LIMIT: u32 = 4;
/// Free space in a client queue before we send another `FileChunk`.
const CLIENT_ROOM: usize = 4;
/// Without room in the client queues for this long, it is assumed that no client is listening.
const NO_CLIENT: Duration = Duration::from_secs(30);

/// A received file that is being sent (or waiting to be sent) to the client.
struct Stream {
    from: Address,
    offer: FileOffer,
    pos: u32,
    last_push: Instant,
    /// No client was listening; sent again on the next `GetInfo`.
    waiting: bool,
}

pub(super) struct Files {
    out_buf: &'static mut [u8; MAX_FILE],
    in_buf: &'static mut [u8; MAX_FILE],
    upload: Option<Upload>,
    out: Option<Outgoing>,
    /// The encoded offer, if it must be sent again.
    offer: heapless::Vec<u8, 160>,
    incoming: Option<Incoming>,
    stream: Option<Stream>,
}

impl Files {
    /// May only be called once (the buffers are static).
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

    /// Lends the two file buffers to a spectrum recording (see `maarse_core::sweeprec::Tables`): the cells in the first, a
    /// work area in the second, where the file is made, the same buffer a file is sent from.
    pub(super) fn rec_tables(&mut self) -> maarse_core::sweeprec::Tables<'_> {
        maarse_core::sweeprec::Tables { cells: &mut self.in_buf[..], work: &mut self.out_buf[..] }
    }

    /// When the files need attention again.
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

/// Truncates UTF-8 to at most `n` bytes without cutting a character in half.
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
    // ---------- from the client ----------

    pub(super) fn file_begin(&mut self, req: u16, dest: Address, size: u32, mime: &[u8], name: &[u8]) {
        if self.rec_holds_buffers() {
            return err(req, error_code::QUEUE_FULL, "A spectrum recording is running; wait until it is done");
        }
        if self.files.out.is_some() {
            return err(req, error_code::QUEUE_FULL, "A file is already being sent; wait until it is done");
        }
        if !self.known.contains_key(&dest) {
            return err(req, error_code::UNKNOWN_DEST, "The recipient has not been heard yet");
        }
        // The offer must fit in one encrypted message.
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
            return err(req, error_code::NO_FILE, "No file being uploaded");
        };
        if !up.complete() {
            return err(req, error_code::NO_FILE, "The file was not fully transferred");
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
                log("Sending file");
                self.files.offer = enc;
                self.files.out = Some(out);
            }
            Err(e) => err(req, error_code::UNKNOWN_DEST, e),
        }
    }

    /// No file is being sent, received or handed to the client.
    pub(super) fn files_idle(&self) -> bool {
        self.files.out.is_none() && self.files.upload.is_none() && self.files.incoming.is_none() && self.files.stream.is_none()
    }

    /// Sends the first `len` bytes of the send buffer (a spectrum recording made there) to `dest` over the air.
    pub(super) fn file_send_staged(&mut self, dest: Address, len: usize, mime: &[u8], name: &[u8]) -> Result<(), &'static str> {
        if self.files.out.is_some() || self.files.upload.is_some() {
            return Err("A file is already being sent");
        }
        if !self.known.contains_key(&dest) {
            return Err("The recipient has not been heard yet");
        }
        let mut id = [0u8; 8];
        self.rng.fill_bytes(&mut id);
        let mut key = [0u8; 32];
        self.rng.fill_bytes(&mut key);
        let (mut out, offer) = Outgoing::new(u64::from_le_bytes(id), key, dest, &self.files.out_buf[..len.min(MAX_FILE)], mime, name, now_ms())?;
        let enc = offer.encode();
        let msg_id = self.send_plain(dest, &enc)?;
        out.offer_sent(msg_id, now_ms());
        self.files.offer = enc;
        self.files.out = Some(out);
        log("Sending the spectrum recording");
        Ok(())
    }

    /// Is a file being sent over the air?
    pub(super) fn file_sending(&self) -> bool {
        self.files.out.is_some()
    }

    /// Hands the first `len` bytes of the send buffer (a spectrum recording) to the client on USB or Bluetooth,
    /// as if the file had arrived over the air.
    pub(super) fn file_staged_to_client(&mut self, len: usize, mime: &[u8], name: &[u8]) -> Result<(), &'static str> {
        if self.files.incoming.is_some() || self.files.stream.is_some() {
            return Err("A file is already on its way to the client");
        }
        let n = len.min(MAX_FILE);
        self.files.in_buf[..n].copy_from_slice(&self.files.out_buf[..n]);
        let mut id = [0u8; 8];
        self.rng.fill_bytes(&mut id);
        let file_id = u64::from_le_bytes(id);
        let offer = FileOffer {
            file_id,
            size: n as u32,
            chunks: maarse_crypto::files::chunks_for(n),
            chunk_size: 0,
            key: [0; 32],
            hash: [0; 16],
            mime: maarse_core::content::Small::from_slice(&mime[..mime.len().min(64)]).unwrap_or_default(),
            name: maarse_core::content::Small::from_slice(&name[..name.len().min(64)]).unwrap_or_default(),
        };
        emit(&Event::FileIncoming { file_id, from: self.addr, size: offer.size, mime: text(&offer.mime), name: text(&offer.name) });
        self.files.stream = Some(Stream { from: self.addr, offer, pos: 0, last_push: Instant::now(), waiting: false });
        self.file_stream();
        Ok(())
    }

    // ---------- sending ----------

    /// Is the acknowledgement for a file? Then it is handled here.
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
                log("File delivered");
                emit(&Event::FileDone { file_id, ok: true, outgoing: true, text: text(b"delivered") });
                    return true;
                }
            }
        }
        self.file_pump();
        true
    }

    /// Sends chunks as long as the window and the radio's queue allow it.
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
                return; // No route right now; tried again on the next tick.
            };
            if let Some(out) = self.files.out.as_mut() {
                out.chunk_sent(idx, msg_id, now_ms());
            }
            self.apply(actions);
        }
    }

    // ---------- receiving ----------

    pub(super) fn file_offer(&mut self, from: Address, offer: FileOffer) {
        if self.rec_holds_buffers() {
            return log("File rejected: a spectrum recording is running");
        }
        if let Some(inc) = &self.files.incoming {
            if inc.file_id() == offer.file_id {
                return; // The offer again (the acknowledgement was lost).
            }
            if !inc.stalled(now_ms()) {
                return log("File rejected: the node is already receiving a file");
            }
        }
        let file_id = offer.file_id;
        let (size, mime, name) = (offer.size, text(&offer.mime), text(&offer.name));
        match Incoming::accept(from, offer, MAX_FILE, now_ms()) {
            Ok(inc) => {
                if self.files.stream.take().is_some() {
                    log("An older file that did not reach the client has been overwritten");
                }
                self.files.incoming = Some(inc);
                emit(&Event::FileIncoming { file_id, from, size, mime, name });
                log("File on its way");
            }
            Err(_) if size as usize > MAX_FILE => log("File rejected: larger than 8 KB"),
            Err(_) => log("File rejected: invalid offer"),
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
                log("File received");
                self.file_stream();
            }
            ChunkResult::BadHash => {
                f.incoming = None;
                emit(&Event::FileDone { file_id, ok: false, outgoing: false, text: text(b"checksum does not match") });
            }
        }
    }

    /// Sends the received file to the client at the pace the queues allow.
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
                    log("No client: the file is kept in RAM until a client connects");
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

    /// A client has connected (`GetInfo`): send a waiting file again.
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
                    log("The file was not delivered");
                    emit(&Event::FileDone {
                        file_id,
                        ok: false,
                        outgoing: true,
                        text: text(b"the recipient did not answer"),
                    });
                }
            }
        }
        if let Some(inc) = self.files.incoming.take_if(|i| i.stalled(now)) {
            emit(&Event::FileDone {
                file_id: inc.file_id(),
                ok: false,
                outgoing: false,
                        text: text(b"transfer aborted"),
            });
        }
        self.file_stream();
    }
}
