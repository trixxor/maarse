//! Files and routes on the host: how large a file may be along the current route,
//! and a file mailbox for recipients who are offline.
//!
//! # Route type
//!
//! The host sends packets out on all its links (TCP backbone and an optional
//! serial LoRa modem). Which way a recipient is reached, we therefore only know
//! from where we last heard it: the link its announce came in on, and how
//! many hops away it was. [`classify`] says:
//!
//! - [`PathKind::Tcp`]: the recipient was heard over a TCP link, and both it and
//!   all intermediate hops on the known route are hosts we know are on the backbone
//!   (direct TCP neighbours, heard with 0 hops, or contacts in the DHT). A
//!   web client at a host has the host's address and therefore counts too.
//! - [`PathKind::Lora`]: the recipient was heard over serial/LoRa, or it was
//!   heard over TCP, but through a hop we do not know is a host (e.g. a
//!   radio node behind another host's modem).
//! - [`PathKind::Unknown`]: no knowledge (never heard, or the link it was
//!   heard on has gone down).
//!
//! Assumption: two hosts that are both on the backbone are connected over TCP.
//! When in doubt, the answer is never `Tcp`, and then the 8 KB limit applies.
//!
//! # Policy
//!
//! - `Tcp`: up to `--max-file` (default 1 MB).
//! - `Lora`: at most [`LORA_MAX_FILE`] (8 KB, same as the firmware).
//! - `Unknown`: the recipient cannot be reached now. With the file mailbox enabled the
//!   file may be up to 1 MB: it is put in the mailbox and delivered when the
//!   recipient shows up again. Files over 8 KB are never sent over a route
//!   that is not `Tcp` (the chunks are held back), so they wait in the mailbox
//!   for a TCP route or expire after 7 days.
//!
//! # The file mailbox
//!
//! Only on the host, never in radio nodes' flash. The file is stored encrypted end
//! to end: the sealed offer (as it was sent to the recipient, with the
//! file key inside) and the sealed chunks. The host cannot read it.
//! One file per entry in `file-mailbox/<recipient>-<file-id>.file`:
//!
//! ```text
//! "MFB1" | recipient (16) | file id (u64) | stored (u64, Unix sec.) | size (u32)
//! | offer length (u16) | offer | chunk count (u16) | (length (u8) | chunk)*
//! ```

use maarse_core::Address;
use maarse_core::airtime::LoraParams;
use maarse_core::companion::RadioConfig;
use maarse_crypto::seal::CHUNK_DATA;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Largest file over LoRa (same as the firmware's file buffer).
pub const LORA_MAX_FILE: u32 = maarse_crypto::files::MAX_FILE as u32;
/// Files in the mailbox expire after the same time as messages.
pub const FILEBOX_TTL: Duration = Duration::from_secs(7 * 24 * 3600);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathKind {
    Tcp,
    Lora,
    Unknown,
}

/// What the host knows about the way to a recipient.
pub struct PathFacts<'a> {
    /// The link the recipient's announce last came in on: `Some(true)` for TCP,
    /// `Some(false)` for serial/LoRa, `None` if unknown.
    pub heard_tcp: Option<bool>,
    /// Intermediate hops (path ids) on the known route, if the router has one.
    pub route: Option<&'a [u16]>,
    /// Is the recipient itself a known backbone host?
    pub dest_is_host: bool,
    /// Path ids of known backbone hosts.
    pub host_ids: &'a HashSet<u16>,
}

pub fn classify(f: &PathFacts) -> PathKind {
    match f.heard_tcp {
        None => PathKind::Unknown,
        Some(false) => PathKind::Lora,
        Some(true) => {
            let hops_ok = f.route.is_none_or(|p| p.iter().all(|id| f.host_ids.contains(id)));
            if f.dest_is_host && hops_ok {
                PathKind::Tcp
            } else {
                PathKind::Lora
            }
        }
    }
}

/// Limit for a file along a route of type `kind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limit {
    pub max_bytes: u32,
    pub via_lora: bool,
    pub mailbox: bool,
    pub est_airtime_s: u32,
}

pub fn limit(kind: PathKind, max_file: u32, filebox_on: bool, radio: &RadioConfig) -> Limit {
    let lora = |mailbox| {
        let max = LORA_MAX_FILE.min(max_file);
        Limit { max_bytes: max, via_lora: true, mailbox, est_airtime_s: est_airtime_s(max, radio) }
    };
    match kind {
        PathKind::Tcp => Limit { max_bytes: max_file, via_lora: false, mailbox: false, est_airtime_s: 0 },
        PathKind::Lora => lora(false),
        PathKind::Unknown if filebox_on => Limit {
            max_bytes: max_file,
            via_lora: true,
            mailbox: true,
            est_airtime_s: est_airtime_s(LORA_MAX_FILE.min(max_file), radio),
        },
        PathKind::Unknown => lora(false),
    }
}

/// Estimated time to send `size` bytes as file chunks over LoRa with
/// the radio settings `radio`. Each chunk counts as a full packet of 255
/// bytes (plus one for the offer), and each chunk is acknowledged with a short packet
/// (about 40 bytes), which also occupies the channel. Duty cycle is measured over an hour
/// (see `maarse_core::duty`): if the sender's airtime fits within the hour's
/// budget, it is the airtime; otherwise it is airtime / duty cycle.
pub fn est_airtime_s(size: u32, radio: &RadioConfig) -> u32 {
    let p = LoraParams {
        sf: radio.sf,
        bw_hz: radio.bw_hz,
        cr: radio.cr.saturating_sub(4).max(1),
        preamble: radio.preamble,
    };
    let packets = (size as usize).div_ceil(CHUNK_DATA) as u64 + 1;
    let tx_ms = packets * p.airtime_ms(maarse_core::MAX_PACKET) as u64;
    let ack_ms = packets * p.airtime_ms(40) as u64;
    let duty = radio.duty_permille.clamp(1, 1000) as u64;
    let budget_ms = maarse_core::duty::WINDOW_MS * duty / 1000;
    let ms = if tx_ms <= budget_ms { tx_ms + ack_ms } else { tx_ms * 1000 / duty };
    ms.div_ceil(1000) as u32
}

/// A file in the mailbox (everything is encrypted end to end).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Boxed {
    pub recipient: Address,
    pub file_id: u64,
    pub stored: u64,
    pub size: u32,
    pub offer: Vec<u8>,
    pub chunks: Vec<Vec<u8>>,
}

impl Boxed {
    fn encode(&self) -> Vec<u8> {
        let mut b = b"MFB1".to_vec();
        b.extend_from_slice(&self.recipient.0);
        b.extend_from_slice(&self.file_id.to_le_bytes());
        b.extend_from_slice(&self.stored.to_le_bytes());
        b.extend_from_slice(&self.size.to_le_bytes());
        b.extend_from_slice(&(self.offer.len() as u16).to_le_bytes());
        b.extend_from_slice(&self.offer);
        b.extend_from_slice(&(self.chunks.len() as u16).to_le_bytes());
        for c in &self.chunks {
            b.push(c.len() as u8);
            b.extend_from_slice(c);
        }
        b
    }

    fn decode(b: &[u8]) -> Option<Boxed> {
        let mut i = 0;
        let mut take = |n: usize| {
            let s = b.get(i..i + n)?;
            i += n;
            Some(s)
        };
        if take(4)? != b"MFB1" {
            return None;
        }
        let recipient = Address::from_slice(take(16)?)?;
        let file_id = u64::from_le_bytes(take(8)?.try_into().ok()?);
        let stored = u64::from_le_bytes(take(8)?.try_into().ok()?);
        let size = u32::from_le_bytes(take(4)?.try_into().ok()?);
        let n = u16::from_le_bytes(take(2)?.try_into().ok()?) as usize;
        let offer = take(n)?.to_vec();
        let count = u16::from_le_bytes(take(2)?.try_into().ok()?) as usize;
        let mut chunks = Vec::with_capacity(count);
        for _ in 0..count {
            let n = take(1)?[0] as usize;
            chunks.push(take(n)?.to_vec());
        }
        Some(Boxed { recipient, file_id, stored, size, offer, chunks })
    }

    fn disk_bytes(&self) -> u64 {
        self.encode().len() as u64
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DepositError {
    Disabled,
    TooLarge,
    RecipientFull,
    Full,
    Io,
}

impl DepositError {
    pub fn message(self) -> &'static str {
        match self {
            DepositError::Disabled => "the file mailbox is disabled",
            DepositError::TooLarge => "the file is too large for the mailbox",
            DepositError::RecipientFull => "the recipient's file mailbox is full",
            DepositError::Full => "the file mailbox is full",
            DepositError::Io => "could not save the file to disk",
        }
    }
}

struct Meta {
    recipient: Address,
    stored: u64,
    bytes: u64,
}

/// The file mailbox on disk. Only an index is kept in memory; the
/// files themselves are read when they are to be delivered.
pub struct FileBox {
    dir: PathBuf,
    /// Largest file (plaintext bytes).
    pub max_file: u32,
    /// Quota per recipient and in total (bytes on disk). 0 = disabled.
    pub per_recipient: u64,
    pub total: u64,
    index: HashMap<u64, Meta>,
}

pub fn unix_secs(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl FileBox {
    pub fn open(dir: PathBuf, max_file: u32, per_recipient: u64, total: u64) -> Self {
        let mut fb = FileBox { dir, max_file, per_recipient, total, index: HashMap::new() };
        if let Ok(rd) = fs::read_dir(&fb.dir) {
            for e in rd.flatten() {
                if let Some(b) = fs::read(e.path()).ok().and_then(|b| Boxed::decode(&b)) {
                    let bytes = b.disk_bytes();
                    fb.index.insert(b.file_id, Meta { recipient: b.recipient, stored: b.stored, bytes });
                } else {
                    let _ = fs::remove_file(e.path());
                }
            }
        }
        fb
    }

    pub fn enabled(&self) -> bool {
        self.total > 0 && self.per_recipient > 0
    }

    /// The recipient of a file in the mailbox.
    pub fn recipient(&self, file_id: u64) -> Option<Address> {
        self.index.get(&file_id).map(|m| m.recipient)
    }

    fn path(&self, recipient: &Address, file_id: u64) -> PathBuf {
        self.dir.join(format!("{recipient}-{file_id:016x}.file"))
    }

    pub fn used(&self) -> u64 {
        self.index.values().map(|m| m.bytes).sum()
    }

    pub fn used_by(&self, r: &Address) -> u64 {
        self.index.values().filter(|m| m.recipient == *r).map(|m| m.bytes).sum()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn deposit(&mut self, b: &Boxed) -> Result<(), DepositError> {
        if !self.enabled() {
            return Err(DepositError::Disabled);
        }
        if b.size > self.max_file {
            return Err(DepositError::TooLarge);
        }
        let old = self.index.get(&b.file_id).map_or(0, |m| m.bytes);
        let enc = b.encode();
        let n = enc.len() as u64;
        if self.used_by(&b.recipient) - old + n > self.per_recipient {
            return Err(DepositError::RecipientFull);
        }
        if self.used() - old + n > self.total {
            return Err(DepositError::Full);
        }
        fs::create_dir_all(&self.dir).map_err(|_| DepositError::Io)?;
        fs::write(self.path(&b.recipient, b.file_id), &enc).map_err(|_| DepositError::Io)?;
        self.index.insert(b.file_id, Meta { recipient: b.recipient, stored: b.stored, bytes: n });
        Ok(())
    }

    /// File ids waiting for `r`.
    pub fn waiting_for(&self, r: &Address) -> Vec<u64> {
        let mut v: Vec<(u64, u64)> =
            self.index.iter().filter(|(_, m)| m.recipient == *r).map(|(id, m)| (m.stored, *id)).collect();
        v.sort();
        v.into_iter().map(|(_, id)| id).collect()
    }

    pub fn load(&self, file_id: u64) -> Option<Boxed> {
        let m = self.index.get(&file_id)?;
        fs::read(self.path(&m.recipient, file_id)).ok().and_then(|b| Boxed::decode(&b))
    }

    pub fn remove(&mut self, file_id: u64) -> bool {
        match self.index.remove(&file_id) {
            Some(m) => {
                let _ = fs::remove_file(self.path(&m.recipient, file_id));
                true
            }
            None => false,
        }
    }

    /// Deletes files older than [`FILEBOX_TTL`]. Returns the deleted ones.
    pub fn expire(&mut self, now: SystemTime) -> Vec<(Address, u64)> {
        let now = unix_secs(now);
        let old: Vec<(Address, u64)> = self
            .index
            .iter()
            .filter(|(_, m)| now.saturating_sub(m.stored) >= FILEBOX_TTL.as_secs())
            .map(|(id, m)| (m.recipient, *id))
            .collect();
        for (_, id) in &old {
            self.remove(*id);
        }
        old
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[u16]) -> HashSet<u16> {
        v.iter().copied().collect()
    }

    #[test]
    fn classify_routes() {
        let hosts = ids(&[0x1111, 0x2222]);
        let f = |heard_tcp, route: Option<&'static [u16]>, dest_is_host| {
            classify(&PathFacts { heard_tcp, route, dest_is_host, host_ids: &hosts })
        };
        // Direct TCP neighbour (or a web client at it).
        assert_eq!(f(Some(true), None, true), PathKind::Tcp);
        assert_eq!(f(Some(true), Some(&[]), true), PathKind::Tcp);
        // Via known hosts on the backbone.
        assert_eq!(f(Some(true), Some(&[0x1111, 0x2222]), true), PathKind::Tcp);
        // An unknown intermediate hop (e.g. a LoRa repeater behind a host).
        assert_eq!(f(Some(true), Some(&[0x1111, 0x3333]), true), PathKind::Lora);
        // Radio node behind another host's modem.
        assert_eq!(f(Some(true), None, false), PathKind::Lora);
        // Heard over the serial modem.
        assert_eq!(f(Some(false), None, true), PathKind::Lora);
        assert_eq!(f(Some(false), Some(&[]), false), PathKind::Lora);
        // No knowledge.
        assert_eq!(f(None, None, true), PathKind::Unknown);
        assert_eq!(f(None, Some(&[]), true), PathKind::Unknown);
    }

    #[test]
    fn limits_by_path() {
        let r = RadioConfig::EU_DEFAULT;
        let mb = 1024 * 1024;
        let t = limit(PathKind::Tcp, mb, true, &r);
        assert_eq!((t.max_bytes, t.via_lora, t.mailbox, t.est_airtime_s), (mb, false, false, 0));
        let l = limit(PathKind::Lora, mb, true, &r);
        assert_eq!((l.max_bytes, l.via_lora, l.mailbox), (8192, true, false));
        assert!(l.est_airtime_s > 30 && l.est_airtime_s < 300, "{}", l.est_airtime_s);
        let u = limit(PathKind::Unknown, mb, true, &r);
        assert_eq!((u.max_bytes, u.via_lora, u.mailbox), (mb, true, true));
        let u = limit(PathKind::Unknown, mb, false, &r);
        assert_eq!((u.max_bytes, u.mailbox), (8192, false));
        // A small --max-file beats 8 KB.
        assert_eq!(limit(PathKind::Lora, 4096, true, &r).max_bytes, 4096);
    }

    #[test]
    fn airtime_grows_with_size_and_sf() {
        let r = RadioConfig::EU_DEFAULT;
        assert!(est_airtime_s(8192, &r) > est_airtime_s(1000, &r));
        let sf12 = RadioConfig { sf: 12, ..r };
        assert!(est_airtime_s(8192, &sf12) > est_airtime_s(8192, &r));
        // 1 MB over LoRa takes hours.
        assert!(est_airtime_s(1 << 20, &r) > 3600 * 3);
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("maarse-filebox-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    fn boxed(r: u8, id: u64, size: u32, stored: u64) -> Boxed {
        let n = (size as usize).div_ceil(CHUNK_DATA);
        Boxed {
            recipient: Address([r; 16]),
            file_id: id,
            stored,
            size,
            offer: vec![7; 120],
            chunks: (0..n).map(|i| vec![i as u8; CHUNK_DATA + 30]).collect(),
        }
    }

    #[test]
    fn deposit_load_remove_and_reload() {
        let d = tmp("rt");
        let mut fb = FileBox::open(d.clone(), 1 << 20, 20 << 20, 200 << 20);
        let b = boxed(1, 42, 20_000, unix_secs(SystemTime::now()));
        fb.deposit(&b).unwrap();
        assert_eq!(fb.waiting_for(&Address([1; 16])), vec![42]);
        assert!(fb.waiting_for(&Address([2; 16])).is_empty());
        assert_eq!(fb.load(42), Some(b.clone()));
        // Survives a restart.
        let mut fb = FileBox::open(d.clone(), 1 << 20, 20 << 20, 200 << 20);
        assert_eq!(fb.load(42), Some(b));
        assert!(fb.remove(42));
        assert_eq!(fb.len(), 0);
        assert_eq!(FileBox::open(d.clone(), 1 << 20, 20 << 20, 200 << 20).len(), 0);
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn quotas() {
        let d = tmp("quota");
        let now = unix_secs(SystemTime::now());
        let one = boxed(1, 1, 20_000, now).disk_bytes();
        let mut fb = FileBox::open(d.clone(), 1 << 20, one * 2, one * 3);
        fb.deposit(&boxed(1, 1, 20_000, now)).unwrap();
        fb.deposit(&boxed(1, 2, 20_000, now)).unwrap();
        assert_eq!(fb.deposit(&boxed(1, 3, 20_000, now)), Err(DepositError::RecipientFull));
        fb.deposit(&boxed(2, 4, 20_000, now)).unwrap();
        assert_eq!(fb.deposit(&boxed(3, 5, 20_000, now)), Err(DepositError::Full));
        assert_eq!(fb.deposit(&boxed(3, 6, (1 << 20) + 1, now)), Err(DepositError::TooLarge));
        let mut off = FileBox::open(tmp("off"), 1 << 20, 0, 0);
        assert_eq!(off.deposit(&boxed(3, 7, 10, now)), Err(DepositError::Disabled));
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn expires_after_seven_days() {
        let d = tmp("ttl");
        let now = SystemTime::now();
        let mut fb = FileBox::open(d.clone(), 1 << 20, 20 << 20, 200 << 20);
        let t = unix_secs(now);
        fb.deposit(&boxed(1, 1, 100, t - FILEBOX_TTL.as_secs() - 1)).unwrap();
        fb.deposit(&boxed(1, 2, 100, t - 3600)).unwrap();
        assert_eq!(fb.expire(now), vec![(Address([1; 16]), 1)]);
        assert_eq!(fb.waiting_for(&Address([1; 16])), vec![2]);
        assert!(fb.expire(now + Duration::from_secs(7 * 24 * 3600)).len() == 1);
        assert_eq!(fb.len(), 0);
        let _ = fs::remove_dir_all(d);
    }
}
