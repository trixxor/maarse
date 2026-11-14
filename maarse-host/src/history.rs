//! Beskedhistorik på hosten: én fælles indbakke, som flere skærme (telefon,
//! bærbar, stationær) kan følge samtidig.
//!
//! # Lagring
//!
//! Alt ligger i `<data>/historik/log.txt`, en append-only tekstlog med én
//! post pr. linje (tabulator-adskilt, binære felter hex-kodet):
//!
//! ```text
//! M <id> <unix> <art> <flag> <tilstand> <hop> <msg_id> <samtale> <fra> <indhold>
//! S <id> <tilstand>                      leveringstilstand ændret
//! R <samtale> <id>                       læst til og med <id>
//! ```
//!
//! * `art`: 1 = direkte besked, 2 = kanalbesked, 3 = fil (kun metadata).
//! * `flag`: bit 0 = udgående (sendt fra en af vores skærme).
//! * `tilstand`: 0 = modtaget, 1 = sendt, 2 = kvitteret, 3 = i postkasse.
//! * `samtale`: modpartens adresse (direkte/fil) eller kanalens adresse.
//! * `indhold`: teksten (UTF-8) eller for filer: størrelse (u32 LE) |
//!   MIME-længde (u8) | MIME | navn. Filernes indhold gemmes ikke.
//!
//! Hele loggen læses ind ved opstart (LoRa-beskeder er små; 90 dage fylder
//! typisk få MB). Ældre poster end `--history-days` fjernes ved opstart og
//! derefter hver time ved at skrive loggen om (ny fil + omdøbning).
//!
//! **Privatliv:** loggen er *ikke* krypteret. Beskederne er ende-til-ende-
//! krypteret på radio og backbone, men hosten dekrypterer dem, og med
//! historik slået til ligger klarteksten på hostens disk. Brug
//! `--no-history` på en delt eller usikker maskine.
//!
//! # Companion-protokol (kun maarse-host; firmware svarer "ugyldig ramme")
//!
//! Kommandoer:
//! * `0x30 GetHistory`: req (u16) | samtale (16, nuller = alle) | før id
//!   (u64, 0 = nyeste) | antal (u8, højst 200). Svar: `HistoryItem`'er
//!   (ældste først) og til sidst `HistoryEnd`.
//! * `0x31 MarkRead`: samtale (16) | til og med id (u64, 0 = alt indtil nu).
//!   Sendes videre til alle skærme som `ReadMark`.
//! * `0x32 GetReadState`: svar: én `ReadMark` pr. samtale.
//!
//! Hændelser:
//! * `0xB0 HistoryItem`: id (u64) | unix (u32) | flag (u8: bit 0 udgående,
//!   bit 1 live = netop sendt fra en anden skærm) | art (u8) | tilstand (u8)
//!   | hop (u8) | msg_id (u64) | samtale (16) | fra (16) | indhold (resten).
//! * `0xB1 HistoryEnd`: req (u16) | samtale (16) | antal (u8) | flere (u8)
//!   | ældste id (u64).
//! * `0xB2 ReadMark`: samtale (16) | til og med id (u64) | ulæste (u16).
//! * `0xB3 HistoryInfo`: slået til (u8) | dage (u16) | antal (u32) |
//!   nyeste id (u64). Sendes ved forbindelse; klienten bruger kun historik,
//!   når den har fået denne.

use maarse_core::Address;
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

pub const CMD_GET_HISTORY: u8 = 0x30;
pub const CMD_MARK_READ: u8 = 0x31;
pub const CMD_GET_READ_STATE: u8 = 0x32;
pub const EV_ITEM: u8 = 0xB0;
pub const EV_END: u8 = 0xB1;
pub const EV_READ_MARK: u8 = 0xB2;
pub const EV_INFO: u8 = 0xB3;

pub const KIND_DIRECT: u8 = 1;
pub const KIND_CHANNEL: u8 = 2;
pub const KIND_FILE: u8 = 3;

pub const ST_RECEIVED: u8 = 0;
pub const ST_SENT: u8 = 1;
pub const ST_ACKED: u8 = 2;
pub const ST_DEPOSITED: u8 = 3;

pub const FLAG_OUTGOING: u8 = 1;
pub const FLAG_LIVE: u8 = 2;

pub const MAX_PAGE: usize = 200;
const MAX_CONTENT: usize = 240;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub id: u64,
    pub ts: u32,
    pub kind: u8,
    pub outgoing: bool,
    pub state: u8,
    pub hops: u8,
    pub msg_id: u64,
    pub conv: Address,
    pub from: Address,
    pub content: Vec<u8>,
}

/// Kommandoer, som historikken håndterer (typerne 0x30-0x32).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistCmd {
    Get { req: u16, conv: Option<Address>, before: u64, limit: u8 },
    MarkRead { conv: Address, up_to: u64 },
    GetReadState,
}

pub fn is_history_command(frame: &[u8]) -> bool {
    matches!(frame.first(), Some(0x30..=0x33))
}

pub fn parse_command(frame: &[u8]) -> Option<HistCmd> {
    let (&t, b) = frame.split_first()?;
    match t {
        CMD_GET_HISTORY if b.len() >= 27 => {
            let conv = Address::from_slice(&b[2..18])?;
            Some(HistCmd::Get {
                req: u16::from_le_bytes([b[0], b[1]]),
                conv: (conv != Address::default()).then_some(conv),
                before: u64::from_le_bytes(b[18..26].try_into().ok()?),
                limit: b[26],
            })
        }
        CMD_MARK_READ if b.len() >= 24 => Some(HistCmd::MarkRead {
            conv: Address::from_slice(&b[..16])?,
            up_to: u64::from_le_bytes(b[16..24].try_into().ok()?),
        }),
        CMD_GET_READ_STATE => Some(HistCmd::GetReadState),
        _ => None,
    }
}

/// Indhold for en fil: størrelse | MIME-længde | MIME | navn.
pub fn file_content(size: u32, mime: &[u8], name: &[u8]) -> Vec<u8> {
    let mime = &mime[..mime.len().min(64)];
    let mut v = size.to_le_bytes().to_vec();
    v.push(mime.len() as u8);
    v.extend_from_slice(mime);
    v.extend_from_slice(name);
    v.truncate(MAX_CONTENT);
    v
}

fn frame(kind: u8, body: &[u8]) -> Vec<u8> {
    let len = (body.len() + 1) as u16;
    let mut v = Vec::with_capacity(body.len() + 3);
    v.extend_from_slice(&len.to_le_bytes());
    v.push(kind);
    v.extend_from_slice(body);
    v
}

pub fn item_frame(it: &Item, live: bool) -> Vec<u8> {
    let mut b = Vec::with_capacity(56 + it.content.len());
    b.extend_from_slice(&it.id.to_le_bytes());
    b.extend_from_slice(&it.ts.to_le_bytes());
    let flags = if it.outgoing { FLAG_OUTGOING } else { 0 } | if live { FLAG_LIVE } else { 0 };
    b.extend_from_slice(&[flags, it.kind, it.state, it.hops]);
    b.extend_from_slice(&it.msg_id.to_le_bytes());
    b.extend_from_slice(&it.conv.0);
    b.extend_from_slice(&it.from.0);
    b.extend_from_slice(&it.content[..it.content.len().min(MAX_CONTENT)]);
    frame(EV_ITEM, &b)
}

pub fn end_frame(req: u16, conv: Option<Address>, count: u8, more: bool, oldest: u64) -> Vec<u8> {
    let mut b = req.to_le_bytes().to_vec();
    b.extend_from_slice(&conv.unwrap_or_default().0);
    b.extend_from_slice(&[count, more as u8]);
    b.extend_from_slice(&oldest.to_le_bytes());
    frame(EV_END, &b)
}

pub fn read_mark_frame(conv: Address, up_to: u64, unread: usize) -> Vec<u8> {
    let mut b = conv.0.to_vec();
    b.extend_from_slice(&up_to.to_le_bytes());
    b.extend_from_slice(&(unread.min(u16::MAX as usize) as u16).to_le_bytes());
    frame(EV_READ_MARK, &b)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

fn addr(s: &str) -> Option<Address> {
    Address::from_slice(&unhex(s)?)
}

pub struct History {
    dir: PathBuf,
    days: u32,
    items: Vec<Item>,
    read: HashMap<Address, u64>,
    next_id: u64,
    file: Option<File>,
}

impl History {
    /// Åbner (eller opretter) `<data>/historik` og fjerner gamle poster.
    pub fn open(dir: PathBuf, days: u32, now: u32) -> std::io::Result<Self> {
        fs::create_dir_all(&dir)?;
        let mut h = History { dir, days, items: Vec::new(), read: HashMap::new(), next_id: 1, file: None };
        h.load();
        h.prune(now)?;
        Ok(h)
    }

    fn log_path(&self) -> PathBuf {
        self.dir.join("log.txt")
    }

    fn load(&mut self) {
        let Ok(f) = File::open(self.log_path()) else { return };
        let mut by_id: HashMap<u64, usize> = HashMap::new();
        for line in BufReader::new(f).lines().map_while(Result::ok) {
            let p: Vec<&str> = line.split('\t').collect();
            match p.as_slice() {
                ["M", id, ts, kind, flags, st, hops, mid, conv, from, content] => {
                    let it = (|| {
                        Some(Item {
                            id: id.parse().ok()?,
                            ts: ts.parse().ok()?,
                            kind: kind.parse().ok()?,
                            outgoing: flags.parse::<u8>().ok()? & FLAG_OUTGOING != 0,
                            state: st.parse().ok()?,
                            hops: hops.parse().ok()?,
                            msg_id: mid.parse().ok()?,
                            conv: addr(conv)?,
                            from: addr(from)?,
                            content: unhex(content)?,
                        })
                    })();
                    if let Some(it) = it {
                        self.next_id = self.next_id.max(it.id + 1);
                        by_id.insert(it.id, self.items.len());
                        self.items.push(it);
                    }
                }
                ["S", id, st] => {
                    if let (Some(&i), Ok(st)) = (id.parse().ok().and_then(|id: u64| by_id.get(&id)), st.parse()) {
                        self.items[i].state = st;
                    }
                }
                ["R", conv, id] => {
                    if let (Some(c), Ok(id)) = (addr(conv), id.parse::<u64>()) {
                        let e = self.read.entry(c).or_insert(0);
                        *e = (*e).max(id);
                    }
                }
                _ => {} // halv linje efter et nedbrud o.l.
            }
        }
        self.items.sort_by_key(|i| i.id);
    }

    fn line(it: &Item) -> String {
        format!(
            "M\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            it.id,
            it.ts,
            it.kind,
            if it.outgoing { FLAG_OUTGOING } else { 0 },
            it.state,
            it.hops,
            it.msg_id,
            hex(&it.conv.0),
            hex(&it.from.0),
            hex(&it.content)
        )
    }

    fn append(&mut self, s: &str) {
        if self.file.is_none() {
            self.file = OpenOptions::new().create(true).append(true).open(self.log_path()).ok();
        }
        if let Some(f) = self.file.as_mut()
            && f.write_all(s.as_bytes()).is_err()
        {
            self.file = None;
        }
    }

    /// Fjerner poster ældre end `days` dage og skriver loggen om.
    pub fn prune(&mut self, now: u32) -> std::io::Result<usize> {
        let cutoff = now.saturating_sub(self.days.saturating_mul(86_400));
        let before = self.items.len();
        self.items.retain(|i| i.ts >= cutoff);
        let removed = before - self.items.len();
        let oldest = self.items.first().map_or(u64::MAX, |i| i.id);
        let latest = self.items.last().map_or(0, |i| i.id);
        // Læsemærker for samtaler uden poster er ligegyldige.
        self.read.retain(|c, _| self.items.iter().any(|i| i.conv == *c));
        if removed == 0 && oldest != u64::MAX && self.log_path().exists() {
            return Ok(0);
        }
        let mut out = String::new();
        for it in &self.items {
            out.push_str(&Self::line(it));
        }
        for (c, id) in &self.read {
            out.push_str(&format!("R\t{}\t{}\n", hex(&c.0), id));
        }
        let tmp = self.dir.join("log.tmp");
        fs::write(&tmp, out)?;
        self.file = None;
        fs::rename(&tmp, self.log_path())?;
        self.next_id = self.next_id.max(latest + 1);
        Ok(removed)
    }

    /// Gemmer en ny post og giver den et id.
    pub fn add(&mut self, mut it: Item) -> Item {
        it.id = self.next_id;
        self.next_id += 1;
        it.content.truncate(MAX_CONTENT);
        self.append(&Self::line(&it));
        self.items.push(it.clone());
        it
    }

    /// Opdaterer leveringstilstanden for en udgående besked (`msg_id`).
    pub fn set_state(&mut self, msg_id: u64, state: u8) -> Option<u64> {
        if msg_id == 0 {
            return None;
        }
        let it = self.items.iter_mut().rev().find(|i| i.outgoing && i.msg_id == msg_id)?;
        if it.state == ST_ACKED {
            return None; // kvittering slår postkasse
        }
        it.state = state;
        let id = it.id;
        self.append(&format!("S\t{id}\t{state}\n"));
        Some(id)
    }

    /// Markerer `conv` som læst til og med `up_to` (0 = seneste). Giver
    /// (læsemærke, ulæste).
    pub fn mark_read(&mut self, conv: Address, up_to: u64) -> (u64, usize) {
        let newest = self.items.iter().rev().find(|i| i.conv == conv).map_or(0, |i| i.id);
        let target = if up_to == 0 { newest } else { up_to.min(newest) };
        let cur = self.read.get(&conv).copied().unwrap_or(0);
        if target > cur {
            self.read.insert(conv, target);
            self.append(&format!("R\t{}\t{}\n", hex(&conv.0), target));
        }
        let mark = target.max(cur);
        (mark, self.unread(conv))
    }

    pub fn unread(&self, conv: Address) -> usize {
        let mark = self.read.get(&conv).copied().unwrap_or(0);
        self.items.iter().filter(|i| i.conv == conv && !i.outgoing && i.id > mark).count()
    }

    /// (samtale, læsemærke, ulæste) for alle samtaler med poster.
    pub fn read_state(&self) -> Vec<(Address, u64, usize)> {
        let mut convs: Vec<Address> = Vec::new();
        for i in &self.items {
            if !convs.contains(&i.conv) {
                convs.push(i.conv);
            }
        }
        convs
            .into_iter()
            .map(|c| (c, self.read.get(&c).copied().unwrap_or(0), self.unread(c)))
            .collect()
    }

    /// En side: højst `limit` poster før `before` (0 = nyeste), ældste først.
    /// Giver også, om der er ældre poster.
    pub fn page(&self, conv: Option<Address>, before: u64, limit: usize) -> (Vec<&Item>, bool) {
        let limit = limit.clamp(1, MAX_PAGE);
        let mut out: Vec<&Item> = self
            .items
            .iter()
            .rev()
            .filter(|i| (before == 0 || i.id < before) && conv.is_none_or(|c| i.conv == c))
            .take(limit + 1)
            .collect();
        let more = out.len() > limit;
        out.truncate(limit);
        out.reverse();
        (out, more)
    }

    pub fn info_frame(&self) -> Vec<u8> {
        let mut b = vec![1];
        b.extend_from_slice(&(self.days.min(u16::MAX as u32) as u16).to_le_bytes());
        b.extend_from_slice(&(self.items.len() as u32).to_le_bytes());
        b.extend_from_slice(&self.items.last().map_or(0, |i| i.id).to_le_bytes());
        frame(EV_INFO, &b)
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.items.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("maarse-hist-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    fn msg(conv: u8, ts: u32, out: bool, text: &str) -> Item {
        Item {
            id: 0,
            ts,
            kind: KIND_DIRECT,
            outgoing: out,
            state: if out { ST_SENT } else { ST_RECEIVED },
            hops: 1,
            msg_id: if out { 1000 + ts as u64 } else { 0 },
            conv: Address([conv; 16]),
            from: Address([if out { 0xee } else { conv }; 16]),
            content: text.as_bytes().to_vec(),
        }
    }

    #[test]
    fn append_and_reload() {
        let d = dir("reload");
        let now = 1_800_000_000;
        {
            let mut h = History::open(d.clone(), 90, now).unwrap();
            let a = h.add(msg(1, now, false, "hej æøå"));
            let b = h.add(msg(1, now + 1, true, "svar"));
            assert_eq!((a.id, b.id), (1, 2));
            assert_eq!(h.set_state(b.msg_id, ST_DEPOSITED), Some(2));
            assert_eq!(h.set_state(b.msg_id, ST_ACKED), Some(2));
            assert_eq!(h.set_state(b.msg_id, ST_DEPOSITED), None);
            let mut f = h.add(msg(2, now, false, ""));
            f.kind = KIND_FILE;
            h.add(Item { kind: KIND_FILE, content: file_content(10, b"image/png", b"x.png"), ..f });
            assert_eq!(h.unread(Address([1; 16])), 1);
            assert_eq!(h.mark_read(Address([1; 16]), 0), (2, 0));
        }
        let h = History::open(d.clone(), 90, now + 10).unwrap();
        assert_eq!(h.len(), 4);
        let (p, more) = h.page(Some(Address([1; 16])), 0, 10);
        assert!(!more);
        assert_eq!(p[0].content, "hej æøå".as_bytes());
        assert_eq!(p[1].state, ST_ACKED);
        assert!(p[1].outgoing);
        assert_eq!(h.unread(Address([1; 16])), 0);
        assert_eq!(h.unread(Address([2; 16])), 2);
        let (p, _) = h.page(Some(Address([2; 16])), 0, 10);
        assert_eq!(p[1].content, file_content(10, b"image/png", b"x.png"));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn paging() {
        let d = dir("paging");
        let now = 1_800_000_000;
        let mut h = History::open(d.clone(), 90, now).unwrap();
        for i in 0..25 {
            h.add(msg(if i % 2 == 0 { 1 } else { 2 }, now + i, false, &format!("m{i}")));
        }
        let (p, more) = h.page(None, 0, 10);
        assert!(more);
        assert_eq!(p.iter().map(|i| i.id).collect::<Vec<_>>(), (16..=25).collect::<Vec<_>>());
        let (p, more) = h.page(None, 16, 10);
        assert!(more);
        assert_eq!(p.first().unwrap().id, 6);
        let (p, more) = h.page(None, 6, 10);
        assert!(!more);
        assert_eq!(p.len(), 5);
        let (p, more) = h.page(Some(Address([1; 16])), 0, 5);
        assert!(more);
        assert!(p.iter().all(|i| i.conv == Address([1; 16])));
        assert_eq!(p.last().unwrap().id, 25);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn pruning() {
        let d = dir("prune");
        let day = 86_400;
        let now = 1_800_000_000;
        let mut h = History::open(d.clone(), 2, now).unwrap();
        h.add(msg(1, now - 5 * day, false, "gammel"));
        h.add(msg(2, now - 3 * day, false, "gammel 2"));
        h.add(msg(1, now - day, false, "ny"));
        h.mark_read(Address([2; 16]), 0);
        assert_eq!(h.prune(now).unwrap(), 2);
        assert_eq!(h.len(), 1);
        let n = h.add(msg(1, now, false, "nyere"));
        assert_eq!(n.id, 4, "id'er genbruges ikke");
        let h = History::open(d.clone(), 2, now).unwrap();
        assert_eq!(h.len(), 2);
        assert_eq!(h.read_state().len(), 1);
        let text = fs::read_to_string(d.join("log.txt")).unwrap();
        assert!(!text.contains(&hex(b"gammel")));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn commands_and_frames() {
        let mut f = vec![CMD_GET_HISTORY, 7, 0];
        f.extend_from_slice(&[0; 16]);
        f.extend_from_slice(&5u64.to_le_bytes());
        f.push(20);
        assert_eq!(parse_command(&f), Some(HistCmd::Get { req: 7, conv: None, before: 5, limit: 20 }));
        let mut m = vec![CMD_MARK_READ];
        m.extend_from_slice(&[3; 16]);
        m.extend_from_slice(&0u64.to_le_bytes());
        assert_eq!(parse_command(&m), Some(HistCmd::MarkRead { conv: Address([3; 16]), up_to: 0 }));
        assert_eq!(parse_command(&[CMD_MARK_READ, 1]), None);
        let it = Item { id: 9, ..msg(1, 5, true, &"x".repeat(300)) };
        let fr = item_frame(&it, true);
        assert!(fr.len() - 2 <= maarse_core::companion::MAX_FRAME);
        assert_eq!(fr[2], EV_ITEM);
        assert_eq!(fr[3 + 12], FLAG_OUTGOING | FLAG_LIVE);
    }
}
