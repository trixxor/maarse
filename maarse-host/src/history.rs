//! Message history on the host: one shared inbox that several screens (phone,
//! laptop, desktop) can follow at the same time.
//!
//! # Storage
//!
//! Everything is in `<data>/history/log.txt`, an append-only text log with one
//! record per line (tab-separated, binary fields hex-encoded):
//!
//! ```text
//! M <id> <unix> <kind> <flag> <state> <hops> <msg_id> <conv> <from> <content>
//! S <id> <state>                         delivery state changed
//! R <conv> <id>                          read up to and including <id>
//! D <id>                                 deleted by the user
//! ```
//!
//! * `kind`: 1 = direct message, 2 = channel message, 3 = file (metadata only).
//! * `flag`: bit 0 = outgoing (sent from one of our screens).
//! * `state`: 0 = received, 1 = sent, 2 = acknowledged, 3 = in mailbox.
//! * `conv`: the peer's address (direct/file) or the channel's address.
//! * `content`: the text (UTF-8) or for files: size (u32 LE) |
//!   MIME length (u8) | MIME | name. File contents are not stored.
//!
//! The whole log is read at startup (LoRa messages are small; 90 days
//! typically take a few MB). Records older than `--history-days` are removed at
//! startup and then every hour by rewriting the log (new file + rename).
//!
//! **Privacy:** the log is *not* encrypted. Messages are end-to-end
//! encrypted on radio and backbone, but the host decrypts them, and with
//! history enabled the plaintext lies on the host's disk. Use
//! `--no-history` on a shared or untrusted machine.
//!
//! # Companion protocol (maarse-host only; firmware answers "invalid frame")
//!
//! Commands:
//! * `0x30 GetHistory`: req (u16) | conv (16, zeros = all) | before id
//!   (u64, 0 = newest) | count (u8, at most 200). Answer: `HistoryItem`s
//!   (oldest first) and finally `HistoryEnd`.
//! * `0x31 MarkRead`: conv (16) | up to and including id (u64, 0 = everything so far).
//!   Forwarded to all screens as `ReadMark`.
//! * `0x32 GetReadState`: answer: one `ReadMark` per conversation.
//! * `0x33 Delete`: conv (16) | flags (u8: bit 0 outgoing, bit 1 the whole
//!   conversation) | unix (u32) | content (the rest). Deletes the message
//!   with that text and direction within two minutes of that time (or the
//!   whole conversation). Forwarded to all screens as `Deleted`.
//!
//! Events:
//! * `0xB0 HistoryItem`: id (u64) | unix (u32) | flag (u8: bit 0 outgoing,
//!   bit 1 live = just sent from another screen) | kind (u8) | state (u8)
//!   | hops (u8) | msg_id (u64) | conv (16) | from (16) | content (the rest).
//! * `0xB1 HistoryEnd`: req (u16) | conv (16) | count (u8) | more (u8)
//!   | oldest id (u64).
//! * `0xB2 ReadMark`: conv (16) | up to and including id (u64) | unread (u16).
//! * `0xB9 Deleted`: the same fields as `Delete`.
//! * `0xB3 HistoryInfo`: enabled (u8) | days (u16) | count (u32) |
//!   newest id (u64). Sent on connect; the client only uses history
//!   once it has received this.

use maarse_core::Address;
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

pub const CMD_GET_HISTORY: u8 = 0x30;
pub const CMD_MARK_READ: u8 = 0x31;
pub const CMD_GET_READ_STATE: u8 = 0x32;
pub const CMD_DELETE: u8 = 0x33;
pub const EV_DELETED: u8 = 0xB9;
pub const DEL_OUTGOING: u8 = 1;
pub const DEL_CONVERSATION: u8 = 2;
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
/// Delivered by carrying (store-carry-forward).
pub const ST_CARRIED: u8 = 4;

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

/// Commands handled by the history (types 0x30-0x32).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistCmd {
    Get { req: u16, conv: Option<Address>, before: u64, limit: u8 },
    MarkRead { conv: Address, up_to: u64 },
    GetReadState,
    Delete { conv: Address, flags: u8, ts: u32, content: Vec<u8> },
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
        CMD_DELETE if b.len() >= 21 => Some(HistCmd::Delete {
            conv: Address::from_slice(&b[..16])?,
            flags: b[16],
            ts: u32::from_le_bytes(b[17..21].try_into().ok()?),
            content: b[21..].to_vec(),
        }),
        _ => None,
    }
}

/// Content for a file: size | MIME length | MIME | name.
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
    /// Opens (or creates) `<data>/history` and removes old records.
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
                ["D", id] => {
                    if let Ok(id) = id.parse::<u64>() {
                        self.items.retain(|i| i.id != id);
                        by_id.clear();
                        for (n, it) in self.items.iter().enumerate() {
                            by_id.insert(it.id, n);
                        }
                    }
                }
                ["R", conv, id] => {
                    if let (Some(c), Ok(id)) = (addr(conv), id.parse::<u64>()) {
                        let e = self.read.entry(c).or_insert(0);
                        *e = (*e).max(id);
                    }
                }
                _ => {} // half a line after a crash or similar
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

    /// Removes records older than `days` days and rewrites the log.
    pub fn prune(&mut self, now: u32) -> std::io::Result<usize> {
        let cutoff = now.saturating_sub(self.days.saturating_mul(86_400));
        let before = self.items.len();
        self.items.retain(|i| i.ts >= cutoff);
        let removed = before - self.items.len();
        let oldest = self.items.first().map_or(u64::MAX, |i| i.id);
        let latest = self.items.last().map_or(0, |i| i.id);
        // Read marks for conversations without records do not matter.
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

    /// Stores a new record and gives it an id.
    pub fn add(&mut self, mut it: Item) -> Item {
        it.id = self.next_id;
        self.next_id += 1;
        it.content.truncate(MAX_CONTENT);
        self.append(&Self::line(&it));
        self.items.push(it.clone());
        it
    }

    /// Updates the delivery state of an outgoing message (`msg_id`).
    pub fn set_state(&mut self, msg_id: u64, state: u8) -> Option<u64> {
        if msg_id == 0 {
            return None;
        }
        let it = self.items.iter_mut().rev().find(|i| i.outgoing && i.msg_id == msg_id)?;
        if it.state == ST_ACKED || it.state == ST_CARRIED {
            return None; // an acknowledgement beats the mailbox
        }
        it.state = state;
        let id = it.id;
        self.append(&format!("S\t{id}\t{state}\n"));
        Some(id)
    }

    /// Marks `conv` as read up to and including `up_to` (0 = latest). Returns
    /// (read mark, unread).
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

    /// (conversation, read mark, unread) for all conversations with records.
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

    /// A page: at most `limit` records before `before` (0 = newest), oldest first.
    /// Also returns whether there are older records.
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

    /// Deletes a message (same conversation, direction and text within two
    /// minutes of `ts`) or, with `DEL_CONVERSATION`, the whole conversation.
    /// Returns how many records went.
    pub fn delete(&mut self, conv: Address, flags: u8, ts: u32, content: &[u8]) -> usize {
        let all = flags & DEL_CONVERSATION != 0;
        let out = flags & DEL_OUTGOING != 0;
        let ids: Vec<u64> = self
            .items
            .iter()
            .filter(|i| i.conv == conv && (all || (i.outgoing == out && i.content == content && i.ts.abs_diff(ts) <= 120)))
            .map(|i| i.id)
            .collect();
        let ids = if all { ids } else { ids.into_iter().take(1).collect() };
        for id in &ids {
            self.append(&format!("D\t{id}\n"));
        }
        self.items.retain(|i| !ids.contains(&i.id));
        ids.len()
    }

    /// The `Deleted` event.
    pub fn deleted_frame(conv: Address, flags: u8, ts: u32, content: &[u8]) -> Vec<u8> {
        let mut b = conv.0.to_vec();
        b.push(flags);
        b.extend_from_slice(&ts.to_le_bytes());
        b.extend_from_slice(content);
        frame(EV_DELETED, &b)
    }

    /// Direct messages (sent and received) newer than `ts`, oldest first.
    pub fn since(&self, ts: u32, limit: usize) -> Vec<&Item> {
        self.items.iter().filter(|i| i.ts > ts && i.kind == KIND_DIRECT).take(limit).collect()
    }

    /// Is there a direct message like this already (same conversation,
    /// direction and text, within two minutes)?
    pub fn has_like(&self, conv: Address, outgoing: bool, content: &[u8], ts: u32) -> bool {
        self.items.iter().rev().take(2000).any(|i| {
            i.conv == conv && i.outgoing == outgoing && i.content == content && i.ts.abs_diff(ts) <= 120
        })
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
    fn delete_survives_a_restart() {
        let d = dir("delete");
        let now = 1_800_000_000;
        let (c1, c2) = (Address([1; 16]), Address([2; 16]));
        {
            let mut h = History::open(d.clone(), 90, now).unwrap();
            h.add(msg(1, now, false, "keep"));
            h.add(msg(1, now + 5, true, "remove me"));
            h.add(msg(2, now, false, "a"));
            h.add(msg(2, now, true, "b"));
            // Wrong direction: nothing; right one, a minute off: deleted.
            assert_eq!(h.delete(c1, 0, now + 5, b"remove me"), 0);
            assert_eq!(h.delete(c1, DEL_OUTGOING, now + 65, b"remove me"), 1);
            assert_eq!(h.delete(c2, DEL_CONVERSATION, 0, b""), 2);
        }
        let h = History::open(d.clone(), 90, now + 10).unwrap();
        assert_eq!(h.len(), 1);
        assert_eq!(h.page(Some(c1), 0, 10).0[0].content, b"keep");
    }

    #[test]
    fn append_and_reload() {
        let d = dir("reload");
        let now = 1_800_000_000;
        {
            let mut h = History::open(d.clone(), 90, now).unwrap();
            let a = h.add(msg(1, now, false, "hello æøå"));
            let b = h.add(msg(1, now + 1, true, "reply"));
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
        assert_eq!(p[0].content, "hello æøå".as_bytes());
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
        h.add(msg(1, now - 5 * day, false, "old"));
        h.add(msg(2, now - 3 * day, false, "old 2"));
        h.add(msg(1, now - day, false, "new"));
        h.mark_read(Address([2; 16]), 0);
        assert_eq!(h.prune(now).unwrap(), 2);
        assert_eq!(h.len(), 1);
        let n = h.add(msg(1, now, false, "newer"));
        assert_eq!(n.id, 4, "ids are not reused");
        let h = History::open(d.clone(), 2, now).unwrap();
        assert_eq!(h.len(), 2);
        assert_eq!(h.read_state().len(), 1);
        let text = fs::read_to_string(d.join("log.txt")).unwrap();
        assert!(!text.contains(&hex(b"old")));
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
