//! Filoverførsel uden heap: tilstandsmaskiner for afsender og modtager.
//!
//! Samme format som maarse-host: et krypteret filtilbud
//! ([`maarse_core::content::FileOffer`]) som direkte besked og derefter
//! bidder ([`crate::seal::seal_chunk`]) som `Fragment`-pakker. Hver bid
//! kvitteres fra ende til ende af routeren.
//!
//! Selve fildata ligger i en buffer, som kalderen ejer (på firmware en
//! statisk buffer på [`MAX_FILE`] bytes). Tilstanden her er lille og kan
//! testes på en PC. Tider er millisekunder fra et vilkårligt nulpunkt.

use crate::seal::{self, CHUNK_DATA};
use maarse_core::Address;
use maarse_core::content::{FileOffer, Small};
use sha2::{Digest, Sha256};

/// Største fil over LoRa (det samme, som klienten skalerer billeder til).
pub const MAX_FILE: usize = 8 * 1024;
/// Højst så mange bidder i én fil (én bit pr. bid i en `u64`).
pub const MAX_CHUNKS: usize = 64;
const _: () = assert!(MAX_FILE.div_ceil(CHUNK_DATA) <= MAX_CHUNKS);

/// Bidder, der må være undervejs uden kvittering.
pub const WINDOW: usize = 4;
/// En bid (eller tilbuddet) uden kvittering sendes igen efter denne tid.
pub const CHUNK_TIMEOUT_MS: u64 = 30_000;
/// Overførslen opgives, hvis der ikke er sket fremskridt i denne tid.
/// Måles fra seneste kvittering, så en langsom kanal (duty cycle) ikke
/// afbryder en overførsel, der går fremad.
pub const STALL_TIMEOUT_MS: u64 = 300_000;

/// Antal bidder i en fil på `size` bytes.
pub fn chunks_for(size: usize) -> u16 {
    size.div_ceil(CHUNK_DATA).max(1) as u16
}

fn all_bits(n: u16) -> u64 {
    if n as usize >= 64 { u64::MAX } else { (1u64 << n) - 1 }
}

fn hash16(data: &[u8]) -> [u8; 16] {
    let d = Sha256::digest(data);
    let mut h = [0u8; 16];
    h.copy_from_slice(&d[..16]);
    h
}

// ---------------------------------------------------------------------------
// Upload fra klienten
// ---------------------------------------------------------------------------

/// En fil, som klienten er ved at overføre til noden (`FileBegin`/`FileData`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Upload {
    pub req: u16,
    pub dest: Address,
    pub size: u32,
    pub mime: Small,
    pub name: Small,
    /// Bytes modtaget indtil videre.
    pub len: u32,
}

impl Upload {
    pub fn new(req: u16, dest: Address, size: u32, mime: &[u8], name: &[u8]) -> Result<Self, &'static str> {
        if size as usize > MAX_FILE {
            return Err("Filen er for stor (højst 8 KB over LoRa)");
        }
        Ok(Upload {
            req,
            dest,
            size,
            mime: Small::from_slice(&mime[..mime.len().min(64)]).unwrap_or_default(),
            name: Small::from_slice(&name[..name.len().min(64)]).unwrap_or_default(),
            len: 0,
        })
    }

    /// Lægger en bid i `buf`. Kun i rækkefølge; andet ignoreres.
    pub fn push(&mut self, offset: u32, data: &[u8], buf: &mut [u8]) -> bool {
        let end = self.len as usize + data.len();
        if offset != self.len || end > self.size as usize || end > buf.len() {
            return false;
        }
        buf[self.len as usize..end].copy_from_slice(data);
        self.len = end as u32;
        true
    }

    pub fn complete(&self) -> bool {
        self.len == self.size
    }
}

// ---------------------------------------------------------------------------
// Afsender
// ---------------------------------------------------------------------------

/// Hvad en sendt pakke var.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    Offer,
    Chunk(u16),
}

/// Svar fra [`Outgoing::on_ack`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AckResult {
    /// Tilbuddet er kvitteret; bidderne kan sendes.
    OfferAcked,
    /// En bid er kvitteret. `done == total` betyder, at filen er leveret.
    Progress { done: u16, total: u16 },
}

/// Svar fra [`Outgoing::tick`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TickResult {
    Idle,
    /// Tilbuddet er ikke kvitteret; send det igen.
    ResendOffer,
    /// Intet fremskridt i [`STALL_TIMEOUT_MS`]; opgiv.
    Failed,
}

/// En fil, der sendes. Data ligger i kalderens buffer.
#[derive(Clone, Debug)]
pub struct Outgoing {
    pub file_id: u64,
    pub dest: Address,
    pub key: [u8; 32],
    pub size: u32,
    pub chunks: u16,
    acked: u64,
    pub done: u16,
    offer_acked: bool,
    offer_sent: u64,
    last_progress: u64,
    /// (msg_id, hvad, afsendt)
    in_flight: heapless::Vec<(u64, Slot, u64), { WINDOW + 1 }>,
}

impl Outgoing {
    /// Starter en overførsel af `data` og returnerer tilbuddet (der skal
    /// krypteres og sendes som direkte besked, se [`Outgoing::offer_sent`]).
    pub fn new(
        file_id: u64,
        key: [u8; 32],
        dest: Address,
        data: &[u8],
        mime: &[u8],
        name: &[u8],
        now: u64,
    ) -> Result<(Self, FileOffer), &'static str> {
        if data.len() > MAX_FILE {
            return Err("Filen er for stor (højst 8 KB over LoRa)");
        }
        let chunks = chunks_for(data.len());
        let offer = FileOffer {
            file_id,
            size: data.len() as u32,
            chunks,
            chunk_size: CHUNK_DATA as u8,
            key,
            hash: hash16(data),
            mime: Small::from_slice(&mime[..mime.len().min(64)]).unwrap_or_default(),
            name: Small::from_slice(&name[..name.len().min(64)]).unwrap_or_default(),
        };
        let o = Outgoing {
            file_id,
            dest,
            key,
            size: data.len() as u32,
            chunks,
            acked: 0,
            done: 0,
            offer_acked: false,
            offer_sent: now,
            last_progress: now,
            in_flight: heapless::Vec::new(),
        };
        Ok((o, offer))
    }

    /// Tilbuddet er sendt (eller sendt igen) med `msg_id`.
    pub fn offer_sent(&mut self, msg_id: u64, now: u64) {
        self.in_flight.retain(|(_, s, _)| *s != Slot::Offer);
        let _ = self.in_flight.push((msg_id, Slot::Offer, now));
        self.offer_sent = now;
    }

    fn chunks_in_flight(&self) -> usize {
        self.in_flight.iter().filter(|(_, s, _)| matches!(s, Slot::Chunk(_))).count()
    }

    /// Næste bid, der bør sendes nu (hvis vinduet tillader det).
    pub fn next_chunk(&self) -> Option<u16> {
        if !self.offer_acked || self.chunks_in_flight() >= WINDOW {
            return None;
        }
        (0..self.chunks).find(|i| {
            self.acked & (1 << i) == 0
                && !self.in_flight.iter().any(|(_, s, _)| *s == Slot::Chunk(*i))
        })
    }

    /// Krypterer bid nr. `idx` fra `data` til `out`. Returnerer længden.
    pub fn seal_chunk(&self, idx: u16, data: &[u8], out: &mut [u8]) -> Option<usize> {
        let start = idx as usize * CHUNK_DATA;
        let end = (start + CHUNK_DATA).min(self.size as usize).min(data.len());
        if start > end {
            return None;
        }
        seal::seal_chunk(&self.key, self.file_id, idx, &data[start..end], out).ok()
    }

    /// Bid `idx` er sendt med `msg_id`.
    pub fn chunk_sent(&mut self, idx: u16, msg_id: u64, now: u64) {
        let _ = self.in_flight.push((msg_id, Slot::Chunk(idx), now));
    }

    /// Hører kvitteringen `msg_id` til denne fil?
    pub fn on_ack(&mut self, msg_id: u64, now: u64) -> Option<AckResult> {
        let pos = self.in_flight.iter().position(|(m, _, _)| *m == msg_id)?;
        let (_, slot, _) = self.in_flight.swap_remove(pos);
        self.last_progress = now;
        Some(match slot {
            Slot::Offer => {
                self.offer_acked = true;
                AckResult::OfferAcked
            }
            Slot::Chunk(i) => {
                if self.acked & (1 << i) == 0 {
                    self.acked |= 1 << i;
                    self.done += 1;
                }
                AckResult::Progress { done: self.done, total: self.chunks }
            }
        })
    }

    pub fn finished(&self) -> bool {
        self.acked == all_bits(self.chunks)
    }

    /// Glemmer bidder uden kvittering efter [`CHUNK_TIMEOUT_MS`], så de
    /// sendes igen. Tilbuddet sendes igen, indtil det er kvitteret.
    pub fn tick(&mut self, now: u64) -> TickResult {
        if now.saturating_sub(self.last_progress) > STALL_TIMEOUT_MS {
            return TickResult::Failed;
        }
        self.in_flight.retain(|(_, s, sent)| {
            matches!(s, Slot::Offer) || now.saturating_sub(*sent) < CHUNK_TIMEOUT_MS
        });
        if !self.offer_acked && now.saturating_sub(self.offer_sent) >= CHUNK_TIMEOUT_MS {
            return TickResult::ResendOffer;
        }
        TickResult::Idle
    }
}

// ---------------------------------------------------------------------------
// Modtager
// ---------------------------------------------------------------------------

/// Svar fra [`Incoming::on_chunk`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChunkResult {
    /// Ikke vores, dublet eller ugyldig.
    Ignored,
    Progress { got: u16, total: u16 },
    /// Alle bidder er her, og kontrolsummen passer.
    Complete,
    /// Alle bidder er her, men kontrolsummen passer ikke.
    BadHash,
}

/// En fil, der modtages. Data samles i kalderens buffer.
#[derive(Clone, Debug)]
pub struct Incoming {
    pub from: Address,
    pub offer: FileOffer,
    got_bits: u64,
    pub got: u16,
    pub last_progress: u64,
}

impl Incoming {
    /// Tjekker et filtilbud. `max` er den største fil, vi tager imod.
    pub fn accept(from: Address, offer: FileOffer, max: usize, now: u64) -> Result<Self, &'static str> {
        let cs = offer.chunk_size as usize;
        if offer.size as usize > max {
            return Err("for stor");
        }
        if cs == 0
            || cs > CHUNK_DATA
            || offer.chunks as usize > MAX_CHUNKS
            || offer.chunks != (offer.size as usize).div_ceil(cs).max(1) as u16
        {
            return Err("ugyldigt tilbud");
        }
        Ok(Incoming { from, offer, got_bits: 0, got: 0, last_progress: now })
    }

    pub fn file_id(&self) -> u64 {
        self.offer.file_id
    }

    /// Hvor mange bytes bid `idx` skal indeholde.
    fn chunk_len(&self, idx: u16) -> usize {
        let cs = self.offer.chunk_size as usize;
        (self.offer.size as usize).saturating_sub(idx as usize * cs).min(cs)
    }

    /// En `Fragment`-payload. Dekrypteres direkte ind i `buf`.
    pub fn on_chunk(&mut self, payload: &[u8], buf: &mut [u8], now: u64) -> ChunkResult {
        let Some((id, idx)) = seal::chunk_header(payload) else {
            return ChunkResult::Ignored;
        };
        if id != self.offer.file_id || idx >= self.offer.chunks || self.got_bits & (1 << idx) != 0 {
            return ChunkResult::Ignored;
        }
        let want = self.chunk_len(idx);
        let start = idx as usize * self.offer.chunk_size as usize;
        if payload.len() != want + seal::CHUNK_OVERHEAD || start + want > buf.len() {
            return ChunkResult::Ignored;
        }
        if seal::open_chunk(&self.offer.key, payload, &mut buf[start..start + want]).is_err() {
            return ChunkResult::Ignored;
        }
        self.got_bits |= 1 << idx;
        self.got += 1;
        self.last_progress = now;
        if self.got_bits != all_bits(self.offer.chunks) {
            return ChunkResult::Progress { got: self.got, total: self.offer.chunks };
        }
        if hash16(&buf[..self.offer.size as usize]) == self.offer.hash {
            ChunkResult::Complete
        } else {
            ChunkResult::BadHash
        }
    }

    pub fn stalled(&self, now: u64) -> bool {
        now.saturating_sub(self.last_progress) > STALL_TIMEOUT_MS
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maarse_core::content::{self, Content};

    fn data(n: usize) -> heapless::Vec<u8, MAX_FILE> {
        (0..n).map(|i| (i * 31 % 251) as u8).collect()
    }

    /// Sender en hel fil fra `Outgoing` til `Incoming`; `drop` vælger
    /// bidder, der går tabt første gang.
    fn transfer(n: usize, drop: impl Fn(u16) -> bool) -> (ChunkResult, u16) {
        let src = data(n);
        let (mut out, offer) =
            Outgoing::new(9, [3; 32], Address([1; 16]), &src, b"image/webp", b"a.webp", 0).unwrap();
        // Tilbuddet går gennem samme indkodning som over radio.
        let enc = offer.encode();
        let Some(Content::FileOffer(parsed)) = content::parse(&enc) else { panic!() };
        let mut inc = Incoming::accept(Address([2; 16]), parsed, MAX_FILE, 0).unwrap();
        let mut buf = [0u8; MAX_FILE];
        let mut now = 0;
        let mut msg = 100;
        out.offer_sent(msg, now);
        assert_eq!(out.next_chunk(), None, "ingen bidder før tilbuddet er kvitteret");
        assert_eq!(out.on_ack(msg, now), Some(AckResult::OfferAcked));
        let mut lost = 0u64;
        let mut last = ChunkResult::Ignored;
        let mut rounds = 0;
        while !out.finished() {
            rounds += 1;
            assert!(rounds < 1000);
            while let Some(i) = out.next_chunk() {
                msg += 1;
                let mut ct = [0u8; 255];
                let len = out.seal_chunk(i, &src, &mut ct).unwrap();
                out.chunk_sent(i, msg, now);
                if drop(i) && lost & (1 << i) == 0 {
                    lost |= 1 << i;
                    continue;
                }
                last = inc.on_chunk(&ct[..len], &mut buf, now);
                // Dubletter ignoreres.
                assert_eq!(inc.on_chunk(&ct[..len], &mut buf, now), ChunkResult::Ignored);
                assert!(matches!(out.on_ack(msg, now), Some(AckResult::Progress { .. })));
            }
            now += CHUNK_TIMEOUT_MS;
            assert_ne!(out.tick(now), TickResult::Failed);
        }
        assert_eq!(&buf[..n], &src[..]);
        (last, out.chunks)
    }

    #[test]
    fn full_transfer_with_losses() {
        assert_eq!(transfer(MAX_FILE, |i| i % 5 == 2), (ChunkResult::Complete, 45));
        assert_eq!(transfer(1, |_| false), (ChunkResult::Complete, 1));
        assert_eq!(transfer(CHUNK_DATA, |_| true), (ChunkResult::Complete, 1));
        assert_eq!(transfer(CHUNK_DATA + 1, |_| false).1, 2);
    }

    #[test]
    fn window_and_retry() {
        let src = data(2000);
        let (mut out, _) = Outgoing::new(1, [0; 32], Address([1; 16]), &src, b"", b"", 0).unwrap();
        out.offer_sent(1, 0);
        assert_eq!(out.tick(CHUNK_TIMEOUT_MS), TickResult::ResendOffer);
        out.offer_sent(2, CHUNK_TIMEOUT_MS);
        assert_eq!(out.on_ack(1, 0), None, "gammelt tilbud er glemt");
        assert_eq!(out.on_ack(2, CHUNK_TIMEOUT_MS), Some(AckResult::OfferAcked));
        for m in 0..WINDOW as u64 {
            let i = out.next_chunk().unwrap();
            out.chunk_sent(i, 10 + m, CHUNK_TIMEOUT_MS);
        }
        assert_eq!(out.next_chunk(), None, "vinduet er fuldt");
        assert_eq!(out.tick(2 * CHUNK_TIMEOUT_MS), TickResult::Idle);
        assert_eq!(out.next_chunk(), Some(0), "bid 0 sendes igen");
        assert_eq!(out.tick(CHUNK_TIMEOUT_MS + STALL_TIMEOUT_MS + 1), TickResult::Failed);
    }

    #[test]
    fn rejects_bad_offers_and_tampering() {
        let src = data(500);
        let (_, offer) = Outgoing::new(5, [7; 32], Address([1; 16]), &src, b"", b"", 0).unwrap();
        let from = Address([2; 16]);
        assert!(Incoming::accept(from, offer.clone(), 100, 0).is_err());
        let mut bad = offer.clone();
        bad.chunks = 1;
        assert!(Incoming::accept(from, bad, MAX_FILE, 0).is_err());
        let mut bad = offer.clone();
        bad.chunk_size = 0;
        assert!(Incoming::accept(from, bad, MAX_FILE, 0).is_err());
        let mut bad = offer.clone();
        bad.hash = [0; 16];
        let mut inc = Incoming::accept(from, bad, MAX_FILE, 0).unwrap();
        let mut buf = [0u8; MAX_FILE];
        let mut res = ChunkResult::Ignored;
        for i in 0..3u16 {
            let start = i as usize * CHUNK_DATA;
            let end = (start + CHUNK_DATA).min(500);
            let mut ct = [0u8; 255];
            let n = seal::seal_chunk(&[7; 32], 5, i, &src[start..end], &mut ct).unwrap();
            let mut t = ct;
            t[12] ^= 1;
            assert_eq!(inc.on_chunk(&t[..n], &mut buf, 0), ChunkResult::Ignored);
            res = inc.on_chunk(&ct[..n], &mut buf, 0);
        }
        assert_eq!(res, ChunkResult::BadHash);
        assert!(Upload::new(1, from, MAX_FILE as u32 + 1, b"", b"").is_err());
    }

    #[test]
    fn upload_in_order() {
        let mut buf = [0u8; MAX_FILE];
        let mut u = Upload::new(1, Address([1; 16]), 5, b"text/plain", b"x.txt").unwrap();
        assert!(!u.push(1, b"x", &mut buf));
        assert!(u.push(0, b"hej", &mut buf));
        assert!(!u.push(3, b"for lang", &mut buf));
        assert!(u.push(3, b"!!", &mut buf));
        assert!(u.complete());
        assert_eq!(&buf[..5], b"hej!!");
    }
}
