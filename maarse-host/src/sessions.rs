//! Forward secrecy sessions (see `maarse_crypto::ratchet`), stored on disk.
//!
//! The file `sessions.bin` is just the sessions one after another (fixed length).
//! It is written atomically (temporary file + rename), readable by the owner
//! only. The file contains secret keys, just like `identity`.
//!
//! A message key must never be used twice, also after a power cut or a failed
//! write. Before a message leaves, [`Sessions::commit_send`] makes sure the
//! file on the disk (synced) has its sending chain at a later message number:
//! it saves the chain up to [`SEND_RESERVE`] messages ahead, so only one
//! message in that many waits for the disk. After a restart the chain goes on
//! from the saved number and the recipient skips the keys in between. If the
//! write fails, the caller must not send.
//!
//! **Putting an old copy of the file back** (a backup, a snapshot) would make
//! the sending chains go back and reuse keys. So the file ends with an epoch
//! number (a trailer older versions ignore) that starts at a random value and
//! goes up with every synced write, and the same number is kept in a small
//! marker file outside the data folder (`%LOCALAPPDATA%\Maarse\state` on
//! Windows, `~/.local/state/maarse` elsewhere, or `MAARSE_STATE_DIR`).
//!
//! * The marker is written first. If it cannot be written, nothing is sent.
//! * On start the file is used only if its epoch is the marker's, or one behind
//!   it (a crash between the two writes; every sending chain then moves
//!   [`RESTORE_SKIP`] messages ahead and the recipient skips the unused keys).
//!   Anything else, also a missing or unreadable marker (a moved or copied data
//!   folder, a new computer), throws the sessions away: every contact gets a
//!   new one.
//! * Without a folder for the marker, sessions are not kept across restarts.
//!
//! A file written by an older version has no epoch and is used if there is no
//! marker. None of this helps when the marker is restored together with the
//! file (a whole-disk snapshot).

use maarse_core::Address;
use maarse_crypto::ratchet::{SESSION_LEN, SESSION_LEN_V1, SESSION_LEN_V2, SID_LEN, Session, SessionStore};
use rand_core::{OsRng, RngCore};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// At most this many sessions (the least used are evicted).
const MAX_SESSIONS: usize = 4096;
/// How far ahead of the used message numbers a sending chain is saved.
pub const SEND_RESERVE: u16 = 32;
/// After a start where the file may be one write old, every sending chain moves
/// this far ahead: more than the two reserves that can lie between two files.
pub const RESTORE_SKIP: u16 = 3 * SEND_RESERVE;
/// The end of the file: magic and epoch (u64 LE). Older versions read whole
/// sessions only and ignore it.
const TRAILER_MAGIC: &[u8; 4] = b"EPO1";
const TRAILER_LEN: usize = 12;

/// Where the epoch marker lives (`None`: no folder outside the data folder
/// can be made). In tests: next to the file.
fn default_marker(sessions: &Path) -> Option<PathBuf> {
    if cfg!(test) {
        return Some(sessions.with_extension("epoch"));
    }
    let base = std::env::var_os("MAARSE_STATE_DIR").map(PathBuf::from).or_else(|| {
        if cfg!(windows) {
            std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("Maarse").join("state"))
        } else {
            std::env::var_os("XDG_STATE_HOME")
                .map(|p| PathBuf::from(p).join("maarse"))
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("state").join("maarse")))
        }
    });
    let dir = base.filter(|d| fs::create_dir_all(d).is_ok())?;
    // One marker per sessions file, so two nodes on one computer do not mix.
    use sha2::{Digest, Sha256};
    let id: String = Sha256::digest(sessions.to_string_lossy().as_bytes()).iter().take(8).map(|b| format!("{b:02x}")).collect();
    Some(dir.join(format!("sessions-{id}.epoch")))
}

fn read_marker(path: &Path) -> Option<u64> {
    fs::read(path).ok().and_then(|b| b.get(..8)?.try_into().ok()).map(u64::from_le_bytes)
}

/// The epoch after `epoch`. The first one is random, so that a file from an
/// earlier series (before the marker was lost) never fits a new marker.
fn next_epoch(epoch: u64) -> u64 {
    if epoch == 0 { (OsRng.next_u64() >> 2) | (1 << 40) } else { epoch + 1 }
}

/// The sessions part of a file and its epoch, if it has one.
fn split_trailer(b: &[u8]) -> (&[u8], Option<u64>) {
    if b.len() >= TRAILER_LEN {
        let (body, t) = b.split_at(b.len() - TRAILER_LEN);
        if &t[..4] == TRAILER_MAGIC {
            return (body, Some(u64::from_le_bytes(t[4..].try_into().unwrap_or([0; 8]))));
        }
    }
    (b, None)
}

/// A sending chain and a message number in it.
type Position = ([u8; 8], u16);

pub struct Sessions {
    map: HashMap<Address, (u64, Session)>,
    /// Replaced sessions not yet given up (`ratchet`, "Replacing a session").
    /// In memory only.
    backups: HashMap<Address, Session>,
    clock: u64,
    path: PathBuf,
    dirty: bool,
    /// Per peer: the sending chain that is safely on the disk and the first
    /// message number that is not covered by it.
    saved: HashMap<Address, Position>,
    /// Number of the last synced write (see the module comment).
    epoch: u64,
    /// The marker file that keeps the epoch outside the data folder.
    marker: Option<PathBuf>,
    /// The file on disk does not fit the marker (an old copy, or the marker is
    /// gone) and was thrown away (set at start).
    pub restored_old_file: bool,
    /// There is no place for a marker: sessions last until the next restart.
    pub unprotected: bool,
}

impl Sessions {
    pub fn load(path: PathBuf) -> Self {
        let marker = default_marker(&path);
        Self::load_with_marker(path, marker)
    }

    pub fn load_with_marker(path: PathBuf, marker: Option<PathBuf>) -> Self {
        let mut map = HashMap::new();
        let unprotected = marker.is_none();
        let marked = marker.as_deref().and_then(read_marker);
        let mut epoch = marked.unwrap_or(0);
        let mut restored_old_file = false;
        let mut one_behind = false;
        let mut dropped = false;
        if let Ok(b) = fs::read(&path) {
            let (body, file_epoch) = split_trailer(&b);
            // Files from older versions have shorter sessions.
            let size = match body.first() {
                Some(1) => SESSION_LEN_V1,
                Some(2) => SESSION_LEN_V2,
                _ => SESSION_LEN,
            };
            // A file that does not fit the marker may hold sending chains
            // that are behind what has been used.
            let stale = match (file_epoch, marked) {
                (Some(f), Some(m)) => f != m && f.checked_add(1) != Some(m),
                (Some(f), None) => f > 0,
                (None, m) => m.is_some(),
            };
            if (stale || unprotected) && body.len() >= size {
                dropped = true;
                restored_old_file = !unprotected;
                if unprotected {
                    eprintln!("no folder for the restore protection file (set MAARSE_STATE_DIR): encryption sessions are not kept across restarts");
                } else {
                    eprintln!("sessions.bin does not fit what this computer has written (a restored backup or a moved data folder?): all encryption sessions are started again");
                }
            } else if !stale {
                one_behind = file_epoch.is_some_and(|f| f.checked_add(1) == marked);
                epoch = marked.or(file_epoch).unwrap_or(0);
                for (i, chunk) in body.chunks_exact(size).enumerate() {
                    if let Some(s) = Session::from_bytes(chunk) {
                        map.insert(s.peer(), (i as u64, s));
                    }
                }
            }
        }
        // After a crash between the marker and the file the file may be one
        // write old: move every sending chain safely past what it can have used.
        if one_behind {
            for (_, s) in map.values_mut() {
                if let Some((_, next)) = s.send_position() {
                    s.advance_send(next.saturating_add(RESTORE_SKIP));
                }
            }
        }
        let clock = map.len() as u64;
        // What was read is on the disk: nothing at or after these numbers has
        // been used.
        let saved = map.iter().filter_map(|(a, (_, s))| s.send_position().map(|p| (*a, p))).collect();
        Sessions { map, backups: HashMap::new(), clock, path, dirty: dropped || one_behind, saved, epoch, marker, restored_old_file, unprotected }
    }

    pub fn contains(&self, peer: &Address) -> bool {
        self.map.contains_key(peer)
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// The chain `peer`'s session sends on.
    pub fn send_chain(&self, peer: &Address) -> Option<[u8; 8]> {
        self.map.get(peer).and_then(|(_, s)| s.send_position()).map(|(chain, _)| chain)
    }

    /// `peer` has acknowledged a message of the chain `chain`.
    pub fn confirm(&mut self, peer: &Address, chain: [u8; 8]) {
        if let Some((_, s)) = self.map.get_mut(peer) {
            s.confirm(chain);
            self.dirty = true;
        }
    }

    /// Forgets all sessions (new identity) and writes that to disk.
    pub fn clear(&mut self) {
        self.map.clear();
        self.backups.clear();
        self.saved.clear();
        self.dirty = true;
        self.save();
    }

    /// Writes to disk if anything has changed since last time. Used after
    /// receiving; a failure is tried again at the next change.
    pub fn save(&mut self) {
        if self.dirty && self.write(None).is_ok() {
            self.dirty = false;
        }
    }

    /// Call after sealing a message to `peer` and before it is sent. On an
    /// error the message must not be sent.
    pub fn commit_send(&mut self, peer: &Address) -> io::Result<()> {
        let Some((chain, next)) = self.map.get(peer).and_then(|(_, s)| s.send_position()) else {
            return Ok(());
        };
        if self.saved.get(peer).is_some_and(|(c, limit)| *c == chain && next <= *limit) {
            self.save();
            return Ok(());
        }
        self.write(Some((*peer, (chain, next.saturating_add(SEND_RESERVE)))))?;
        self.dirty = false;
        Ok(())
    }

    /// Writes the file, with every sending chain at least as far as it has
    /// been saved before. With `reserve` the write is synced to the disk.
    fn write(&mut self, reserve: Option<(Address, Position)>) -> io::Result<()> {
        let mut saved = self.saved.clone();
        if let Some((peer, position)) = reserve {
            saved.insert(peer, position);
            // The synced write also covers chains that are new since the last one.
            for (a, (_, s)) in &self.map {
                if let Some(p) = s.send_position()
                    && saved.get(a).is_none_or(|(c, _)| *c != p.0)
                {
                    saved.insert(*a, p);
                }
            }
        }
        let mut b = Vec::with_capacity(self.map.len() * SESSION_LEN);
        for (a, (_, s)) in &self.map {
            match (saved.get(a), s.send_position()) {
                (Some((c, limit)), Some((chain, next))) if *c == chain && *limit > next => {
                    let mut ahead = s.clone();
                    ahead.advance_send(*limit);
                    b.extend_from_slice(&ahead.to_bytes());
                }
                _ => b.extend_from_slice(&s.to_bytes()),
            }
        }
        // A synced write is a new epoch. The marker goes first: a file that is
        // one epoch behind it is accepted at start, a file ahead of it is fine.
        let epoch = if reserve.is_some() { next_epoch(self.epoch) } else { self.epoch };
        if reserve.is_some()
            && let Some(marker) = &self.marker
        {
            // The data folder is not made here: a missing one is an error to report.
            if let Some(dir) = marker.parent().filter(|d| Some(*d) != self.path.parent()) {
                let _ = fs::create_dir_all(dir);
            }
            // Without a current marker an old copy of the file could be used
            // later, so nothing may be sent.
            crate::private_file::write(marker, &epoch.to_le_bytes(), true).map_err(|e| {
                io::Error::new(e.kind(), format!("the restore protection file {} cannot be written: {e}", marker.display()))
            })?;
        }
        b.extend_from_slice(TRAILER_MAGIC);
        b.extend_from_slice(&epoch.to_le_bytes());
        crate::private_file::write(&self.path, &b, reserve.is_some())?;
        if reserve.is_some() {
            self.saved = saved;
            self.epoch = epoch;
        }
        Ok(())
    }
}

impl SessionStore for Sessions {
    fn get_mut(&mut self, peer: &Address) -> Option<&mut Session> {
        self.clock += 1;
        let clock = self.clock;
        // Only called when the session is about to be used (and typically changed).
        self.dirty = true;
        self.map.get_mut(peer).map(|(t, s)| {
            *t = clock;
            s
        })
    }

    fn find_sid(&mut self, sid: &[u8; SID_LEN], skip: usize) -> Option<&mut Session> {
        self.clock += 1;
        let clock = self.clock;
        self.dirty = true;
        self.map.values_mut().filter(|(_, s)| s.receive_sid() == Some(*sid)).nth(skip).map(|(t, s)| {
            *t = clock;
            s
        })
    }

    fn set_backup(&mut self, s: Session) {
        // As many as there can be sessions; anyone may start one, so bounded.
        if self.backups.len() >= MAX_SESSIONS && !self.backups.contains_key(&s.peer()) {
            self.backups.clear();
        }
        self.backups.insert(s.peer(), s);
    }

    fn backup_mut(&mut self, peer: &Address) -> Option<&mut Session> {
        self.backups.get_mut(peer)
    }

    fn take_backup(&mut self, peer: &Address) -> Option<Session> {
        self.backups.remove(peer)
    }

    fn backup_by_sid(&mut self, sid: &[u8; SID_LEN]) -> Option<&mut Session> {
        self.backups.values_mut().find(|s| s.receive_sid() == Some(*sid))
    }

    fn insert(&mut self, s: Session) {
        self.clock += 1;
        if self.map.len() >= MAX_SESSIONS && !self.map.contains_key(&s.peer())
            && let Some(old) = self.map.iter().min_by_key(|(_, (t, _))| *t).map(|(a, _)| *a)
        {
            self.map.remove(&old);
            self.backups.remove(&old);
            self.saved.remove(&old);
        }
        self.map.insert(s.peer(), (self.clock, s));
        self.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maarse_crypto::Identity;
    use maarse_crypto::ratchet::{self, FixedStore};
    use maarse_crypto::seal::CryptoError;

    /// Two identities with an established session on the host side `store`.
    fn established(path: &std::path::Path) -> (Identity, Identity, Sessions, FixedStore<2>) {
        let (a, b) = (Identity::generate(&mut OsRng), Identity::generate(&mut OsRng));
        let (mut sa, mut sb) = (Sessions::load(path.to_path_buf()), FixedStore::<2>::new());
        for _ in 0..2 {
            let m = host_send(&a, &b, &mut sa, b"ping").unwrap();
            open(&b, &a, &mut sb, &m).unwrap();
            let mut out = [0u8; 255];
            let (n, _) = ratchet::seal(&b, &a.public(), &mut sb, true, &mut OsRng, b"pong", &mut out).unwrap();
            open(&a, &b, &mut sa, &out[..n]).unwrap();
        }
        // The first message after a turn takes the ratchet step.
        let m = host_send(&a, &b, &mut sa, b"step").unwrap();
        open(&b, &a, &mut sb, &m).unwrap();
        (a, b, sa, sb)
    }

    /// What the host does for a message: seal, make the state safe, then send.
    fn host_send(a: &Identity, b: &Identity, s: &mut Sessions, text: &[u8]) -> std::io::Result<Vec<u8>> {
        let mut out = [0u8; 255];
        let (n, _) = ratchet::seal(a, &b.public(), s, true, &mut OsRng, text, &mut out).unwrap();
        s.commit_send(&b.address())?;
        Ok(out[..n].to_vec())
    }

    fn open(me: &Identity, from: &Identity, s: &mut impl SessionStore, m: &[u8]) -> Result<(), CryptoError> {
        let p = from.public();
        let mut out = [0u8; 256];
        ratchet::open(me, m, s, |a| (*a == p.address()).then_some(p), &mut out).map(|_| ())
    }

    fn number(message: &[u8]) -> u16 {
        u16::from_le_bytes([message[49], message[50]])
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("maarse-sess-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_restart_never_uses_a_message_key_again() {
        let dir = temp_dir("restart");
        let path = dir.join("sessions.bin");
        let (a, b, mut sa, mut sb) = established(&path);
        let first = host_send(&a, &b, &mut sa, b"attack at dawn!!").unwrap();
        // The state on disk right after the last durable write: a power cut
        // loses everything written after it.
        let durable = fs::read(&path).unwrap();
        let mut used = vec![number(&first)];
        for _ in 0..5 {
            let m = host_send(&a, &b, &mut sa, b"more").unwrap();
            open(&b, &a, &mut sb, &m).unwrap();
            used.push(number(&m));
        }
        drop(sa);
        fs::write(&path, durable).unwrap();
        let mut after = Sessions::load(path);
        let m = host_send(&a, &b, &mut after, b"retreat at noon!").unwrap();
        assert!(number(&m) > *used.iter().max().unwrap(), "{} after {used:?}", number(&m));
        // The other side still reads it.
        open(&b, &a, &mut sb, &m).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_old_copy_of_the_file_is_not_used() {
        let dir = temp_dir("rollback");
        let path = dir.join("sessions.bin");
        let (a, b, mut sa, mut sb) = established(&path);
        let old = fs::read(&path).unwrap();
        for _ in 0..100 {
            let m = host_send(&a, &b, &mut sa, b"x").unwrap();
            open(&b, &a, &mut sb, &m).unwrap();
        }
        drop(sa);
        // A backup is put back.
        fs::write(&path, old).unwrap();
        let mut s = Sessions::load(path.clone());
        assert!(s.restored_old_file);
        assert_eq!(s.len(), 0, "no session from the old file may be used");
        // The empty state is written; the next start is normal.
        s.save();
        drop(s);
        let s = Sessions::load(path);
        assert!(!s.restored_old_file);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_one_write_behind_is_used_with_the_chains_moved_on() {
        let dir = temp_dir("one-behind");
        let path = dir.join("sessions.bin");
        let (a, b, mut sa, mut sb) = established(&path);
        let mut used = 0;
        for _ in 0..3 {
            let m = host_send(&a, &b, &mut sa, b"x").unwrap();
            open(&b, &a, &mut sb, &m).unwrap();
            used = used.max(number(&m));
        }
        drop(sa);
        // A crash after the marker and before the file: the marker is one ahead.
        let epoch = split_trailer(&fs::read(&path).unwrap()).1.unwrap();
        fs::write(path.with_extension("epoch"), (epoch + 1).to_le_bytes()).unwrap();
        let mut s = Sessions::load(path);
        assert!(!s.restored_old_file);
        assert_eq!(s.len(), 1);
        let m = host_send(&a, &b, &mut s, b"again").unwrap();
        assert!(number(&m) >= used + RESTORE_SKIP, "{} after {used}", number(&m));
        open(&b, &a, &mut sb, &m).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_old_file_without_epoch_is_read_the_first_time() {
        let dir = temp_dir("legacy");
        let path = dir.join("sessions.bin");
        let (a, b, mut sa, _sb) = established(&path);
        let _ = host_send(&a, &b, &mut sa, b"x").unwrap();
        drop(sa);
        // What an older version wrote: no trailer, and no marker yet.
        let bytes = fs::read(&path).unwrap();
        fs::write(&path, split_trailer(&bytes).0).unwrap();
        let _ = fs::remove_file(path.with_extension("epoch"));
        let s = Sessions::load(path);
        assert!(!s.restored_old_file);
        assert_eq!(s.len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn nothing_is_sent_when_the_marker_cannot_be_written() {
        let dir = temp_dir("marker-write");
        let path = dir.join("sessions.bin");
        let (a, b, sa, mut sb) = established(&path);
        drop(sa);
        let mut s = Sessions::load(path.clone());
        // A file where the marker's folder should be makes its write fail.
        fs::write(dir.join("blocked"), b"a file, not a folder").unwrap();
        s.marker = Some(dir.join("blocked").join("marker"));
        let before = fs::read(&path).unwrap();
        let error = host_send(&a, &b, &mut s, b"x").unwrap_err();
        assert!(error.to_string().contains("restore protection"), "{error}");
        assert_eq!(fs::read(&path).unwrap(), before, "the state moved without the marker");
        // Works again when the marker can be written.
        s.marker = Some(path.with_extension("epoch"));
        let m = host_send(&a, &b, &mut s, b"y").unwrap();
        open(&b, &a, &mut sb, &m).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_without_its_marker_is_not_used() {
        let dir = temp_dir("marker-gone");
        let path = dir.join("sessions.bin");
        let (a, b, sa, _sb) = established(&path);
        drop(sa);
        let file = fs::read(&path).unwrap();
        // A copy on a new computer, and a marker that cannot be read.
        for marker in [None, Some(&b"torn"[..])] {
            fs::write(&path, &file).unwrap();
            let _ = fs::remove_file(path.with_extension("epoch"));
            if let Some(m) = marker {
                fs::write(path.with_extension("epoch"), m).unwrap();
            }
            let mut s = Sessions::load(path.clone());
            assert!(s.restored_old_file, "{marker:?}");
            assert_eq!(s.len(), 0);
            // It goes on with new sessions, in a new series of epochs.
            s.insert(Session::initiate(&a, &b.public(), &mut OsRng).unwrap());
            host_send(&a, &b, &mut s, b"new").unwrap();
            drop(s);
            let s = Sessions::load(path.clone());
            assert!(!s.restored_old_file && s.len() == 1);
            // The earlier copy does not fit the new marker.
            drop(s);
            fs::write(&path, &file).unwrap();
            assert!(Sessions::load(path.clone()).restored_old_file);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_marker_that_fell_behind_is_not_trusted() {
        let dir = temp_dir("marker-behind");
        let path = dir.join("sessions.bin");
        let (_a, b, sa, _sb) = established(&path);
        drop(sa);
        let epoch = split_trailer(&fs::read(&path).unwrap()).1.unwrap();
        fs::write(path.with_extension("epoch"), (epoch - 1).to_le_bytes()).unwrap();
        let s = Sessions::load(path);
        assert!(s.restored_old_file && !s.contains(&b.address()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn without_a_place_for_the_marker_sessions_do_not_survive_a_restart() {
        let dir = temp_dir("unprotected");
        let path = dir.join("sessions.bin");
        let (a, b) = (Identity::generate(&mut OsRng), Identity::generate(&mut OsRng));
        let mut s = Sessions::load_with_marker(path.clone(), None);
        s.insert(Session::initiate(&a, &b.public(), &mut OsRng).unwrap());
        host_send(&a, &b, &mut s, b"x").unwrap();
        drop(s);
        let s = Sessions::load_with_marker(path, None);
        assert!(s.unprotected && s.len() == 0);
        let _ = fs::remove_dir_all(&dir);
    }

    /// What 0.9.13 wrote (shorter sessions, with an epoch) is read.
    #[test]
    fn a_file_with_version_2_sessions_is_read() {
        let dir = temp_dir("version2");
        let path = dir.join("sessions.bin");
        let (_a, b, sa, _sb) = established(&path);
        drop(sa);
        let bytes = fs::read(&path).unwrap();
        let (body, epoch) = split_trailer(&bytes);
        let mut old = body[..SESSION_LEN_V2].to_vec();
        old[0] = 2;
        old[SESSION_LEN_V2 - 1] &= 0x0f;
        old.extend_from_slice(TRAILER_MAGIC);
        old.extend_from_slice(&epoch.unwrap().to_le_bytes());
        fs::write(&path, old).unwrap();
        let s = Sessions::load(path);
        assert!(!s.restored_old_file);
        assert!(s.contains(&b.address()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn restarts_without_sending_do_not_move_the_chain() {
        let dir = temp_dir("idle");
        let path = dir.join("sessions.bin");
        let (a, b, mut sa, mut sb) = established(&path);
        let before = number(&host_send(&a, &b, &mut sa, b"x").unwrap());
        drop(sa);
        for _ in 0..40 {
            let mut s = Sessions::load(path.clone());
            let _ = s.get_mut(&b.address());
            s.save();
        }
        let mut s = Sessions::load(path);
        let m = host_send(&a, &b, &mut s, b"y").unwrap();
        assert!(number(&m) <= before + SEND_RESERVE + 1, "{} after {before}", number(&m));
        open(&b, &a, &mut sb, &m).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_write_is_reported_and_tried_again() {
        let dir = temp_dir("fail");
        let (a, b) = (Identity::generate(&mut OsRng), Identity::generate(&mut OsRng));
        let missing = dir.join("not-there");
        let mut s = Sessions::load(missing.join("sessions.bin"));
        assert!(host_send(&a, &b, &mut s, b"x").is_err());
        fs::create_dir_all(&missing).unwrap();
        assert!(host_send(&a, &b, &mut s, b"x").is_ok());
        assert_eq!(Sessions::load(missing.join("sessions.bin")).len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_only_for_the_owner() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("mode");
        let path = dir.join("sessions.bin");
        let _ = established(&path);
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn saves_and_loads() {
        let dir = std::env::temp_dir().join(format!("maarse-sess-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sessions.bin");
        let me = Identity::generate(&mut OsRng);
        let peer = Identity::generate(&mut OsRng).public();
        let mut s = Sessions::load(path.clone());
        s.insert(Session::initiate(&me, &peer, &mut OsRng).unwrap());
        s.save();
        let mut t = Sessions::load(path);
        assert_eq!(t.len(), 1);
        assert!(t.get_mut(&peer.address()).is_some());
        let _ = fs::remove_dir_all(&dir);
    }
}
