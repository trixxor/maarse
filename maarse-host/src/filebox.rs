//! Filer og ruter på hosten: hvor stor en fil må være ad den aktuelle rute,
//! og en filpostkasse til modtagere, der er offline.
//!
//! # Rutens type
//!
//! Hosten sender pakker ud på alle sine links (TCP-backbone og et evt.
//! serielt LoRa-modem). Hvilken vej en modtager nås, ved vi derfor kun ud
//! fra, hvor vi sidst hørte den: linket, dens announce kom ind på, og hvor
//! mange hop den var væk. [`classify`] siger:
//!
//! - [`PathKind::Tcp`]: modtageren er hørt over et TCP-link, og både den og
//!   alle mellemled på den kendte rute er hosts, vi ved er på backbone
//!   (direkte TCP-naboer, hørt med 0 hop, eller kontakter i DHT'en). En
//!   webklient hos en host har hostens adresse og tæller derfor med.
//! - [`PathKind::Lora`]: modtageren er hørt over seriel/LoRa, eller den er
//!   hørt over TCP, men gennem et led, vi ikke ved er en host (fx en
//!   radionode bag en anden hosts modem).
//! - [`PathKind::Unknown`]: ingen viden (aldrig hørt, eller linket, den blev
//!   hørt på, er gået ned).
//!
//! Antagelse: to hosts, der begge er på backbone, er forbundet over TCP.
//! Er man i tvivl, er svaret aldrig `Tcp`, og så gælder 8 KB-grænsen.
//!
//! # Politik
//!
//! - `Tcp`: op til `--max-file` (standard 1 MB).
//! - `Lora`: højst [`LORA_MAX_FILE`] (8 KB, samme som firmwaren).
//! - `Unknown`: modtageren er ikke at nå nu. Med filpostkassen slået til må
//!   filen være op til 1 MB: den lægges i postkassen og leveres, når
//!   modtageren dukker op igen. Filer over 8 KB sendes aldrig over en rute,
//!   der ikke er `Tcp` (bidderne holdes tilbage), så de venter i postkassen
//!   på en TCP-rute eller udløber efter 7 dage.
//!
//! # Filpostkassen
//!
//! Kun på hosten, aldrig i radionoders flash. Filen gemmes krypteret ende
//! til ende: det forseglede tilbud (som det blev sendt til modtageren, med
//! filnøglen inden i) og de forseglede bidder. Hosten kan ikke læse den.
//! Én fil pr. post i `filpostkasse/<modtager>-<fil-id>.fil`:
//!
//! ```text
//! "MFB1" | modtager (16) | fil-id (u64) | gemt (u64, Unix-sek.) | størrelse (u32)
//! | tilbud-længde (u16) | tilbud | antal bidder (u16) | (længde (u8) | bid)*
//! ```

use maarse_core::Address;
use maarse_core::airtime::LoraParams;
use maarse_core::companion::RadioConfig;
use maarse_crypto::seal::CHUNK_DATA;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Største fil over LoRa (samme som firmwarens filbuffer).
pub const LORA_MAX_FILE: u32 = maarse_crypto::files::MAX_FILE as u32;
/// Filer i postkassen udløber efter samme tid som beskeder.
pub const FILEBOX_TTL: Duration = Duration::from_secs(7 * 24 * 3600);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathKind {
    Tcp,
    Lora,
    Unknown,
}

/// Hvad hosten ved om vejen til en modtager.
pub struct PathFacts<'a> {
    /// Linket, modtagerens announce sidst kom ind på: `Some(true)` for TCP,
    /// `Some(false)` for seriel/LoRa, `None` hvis ukendt.
    pub heard_tcp: Option<bool>,
    /// Mellemled (path-id'er) på den kendte rute, hvis routeren har en.
    pub route: Option<&'a [u16]>,
    /// Er modtageren selv en kendt backbone-host?
    pub dest_is_host: bool,
    /// Path-id'er for kendte backbone-hosts.
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

/// Grænse for en fil ad en rute af typen `kind`.
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

/// Anslået tid for at sende `size` bytes som filbidder over LoRa med
/// radioindstillingerne `radio`. Hver bid regnes som en fuld pakke på 255
/// bytes (plus én til tilbuddet), og hver bid kvitteres med en kort pakke
/// (ca. 40 bytes), som også optager kanalen. Duty cycle måles over en time
/// (se `maarse_core::duty`): kan afsenderens sendetid være inden for timens
/// budget, er det sendetiden; ellers er det sendetid / duty cycle.
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

/// En fil i postkassen (alt er krypteret ende til ende).
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
    pub fn danish(self) -> &'static str {
        match self {
            DepositError::Disabled => "filpostkassen er slået fra",
            DepositError::TooLarge => "filen er for stor til postkassen",
            DepositError::RecipientFull => "modtagerens filpostkasse er fuld",
            DepositError::Full => "filpostkassen er fuld",
            DepositError::Io => "kunne ikke gemme filen på disken",
        }
    }
}

struct Meta {
    recipient: Address,
    stored: u64,
    bytes: u64,
}

/// Filpostkassen på disken. I hukommelsen holdes kun et indeks; selve
/// filerne læses, når de skal leveres.
pub struct FileBox {
    dir: PathBuf,
    /// Største fil (bytes klartekst).
    pub max_file: u32,
    /// Kvote pr. modtager og i alt (bytes på disken). 0 = slået fra.
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

    fn path(&self, recipient: &Address, file_id: u64) -> PathBuf {
        self.dir.join(format!("{recipient}-{file_id:016x}.fil"))
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

    /// Fil-id'er, der venter på `r`.
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

    /// Sletter filer ældre end [`FILEBOX_TTL`]. Returnerer de slettede.
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
        // Direkte TCP-nabo (eller en webklient hos den).
        assert_eq!(f(Some(true), None, true), PathKind::Tcp);
        assert_eq!(f(Some(true), Some(&[]), true), PathKind::Tcp);
        // Via kendte hosts på backbone.
        assert_eq!(f(Some(true), Some(&[0x1111, 0x2222]), true), PathKind::Tcp);
        // Et ukendt mellemled (fx en LoRa-repeater bag en host).
        assert_eq!(f(Some(true), Some(&[0x1111, 0x3333]), true), PathKind::Lora);
        // Radionode bag en anden hosts modem.
        assert_eq!(f(Some(true), None, false), PathKind::Lora);
        // Hørt over det serielle modem.
        assert_eq!(f(Some(false), None, true), PathKind::Lora);
        assert_eq!(f(Some(false), Some(&[]), false), PathKind::Lora);
        // Ingen viden.
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
        // En lille --max-file vinder over 8 KB.
        assert_eq!(limit(PathKind::Lora, 4096, true, &r).max_bytes, 4096);
    }

    #[test]
    fn airtime_grows_with_size_and_sf() {
        let r = RadioConfig::EU_DEFAULT;
        assert!(est_airtime_s(8192, &r) > est_airtime_s(1000, &r));
        let sf12 = RadioConfig { sf: 12, ..r };
        assert!(est_airtime_s(8192, &sf12) > est_airtime_s(8192, &r));
        // 1 MB over LoRa tager timer.
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
        // Overlever genstart.
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
