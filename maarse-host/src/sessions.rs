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
//! write fails, the caller must not send. Putting an old copy of the file back
//! (a backup) is not covered: delete `sessions.bin` after a restore.

use maarse_core::Address;
use maarse_crypto::ratchet::{SESSION_LEN, SESSION_LEN_V1, SID_LEN, Session, SessionStore};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::PathBuf;

/// At most this many sessions (the least used are evicted).
const MAX_SESSIONS: usize = 4096;
/// How far ahead of the used message numbers a sending chain is saved.
pub const SEND_RESERVE: u16 = 32;

/// A sending chain and a message number in it.
type Position = ([u8; 8], u16);

pub struct Sessions {
    map: HashMap<Address, (u64, Session)>,
    clock: u64,
    path: PathBuf,
    dirty: bool,
    /// Per peer: the sending chain that is safely on the disk and the first
    /// message number that is not covered by it.
    saved: HashMap<Address, Position>,
}

impl Sessions {
    pub fn load(path: PathBuf) -> Self {
        let mut map = HashMap::new();
        if let Ok(b) = fs::read(&path) {
            // Version 1 files (before compact messages) have shorter sessions.
            let size = if b.first() == Some(&1) { SESSION_LEN_V1 } else { SESSION_LEN };
            for (i, chunk) in b.chunks_exact(size).enumerate() {
                if let Some(s) = Session::from_bytes(chunk) {
                    map.insert(s.peer(), (i as u64, s));
                }
            }
        }
        let clock = map.len() as u64;
        // What was read is on the disk: nothing at or after these numbers has
        // been used.
        let saved = map.iter().filter_map(|(a, (_, s))| s.send_position().map(|p| (*a, p))).collect();
        Sessions { map, clock, path, dirty: false, saved }
    }

    pub fn contains(&self, peer: &Address) -> bool {
        self.map.contains_key(peer)
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Forgets all sessions (new identity) and writes that to disk.
    pub fn clear(&mut self) {
        self.map.clear();
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
        crate::private_file::write(&self.path, &b, reserve.is_some())?;
        if reserve.is_some() {
            self.saved = saved;
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

    fn insert(&mut self, s: Session) {
        self.clock += 1;
        if self.map.len() >= MAX_SESSIONS && !self.map.contains_key(&s.peer())
            && let Some(old) = self.map.iter().min_by_key(|(_, (t, _))| *t).map(|(a, _)| *a)
        {
            self.map.remove(&old);
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
    use rand_core::OsRng;

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
