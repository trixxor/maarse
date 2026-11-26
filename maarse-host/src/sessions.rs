//! Forward secrecy sessions (see `maarse_crypto::ratchet`), stored on disk.
//!
//! The file `sessions.bin` is just the sessions one after another (fixed length).
//! It is written atomically (temporary file + rename) after every change - on
//! a PC that is cheap, and a lost state means lost messages.
//! The file contains secret keys, just like `identity`.

use maarse_core::Address;
use maarse_crypto::ratchet::{SESSION_LEN, SESSION_LEN_V1, SID_LEN, Session, SessionStore};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// At most this many sessions (the least used are evicted).
const MAX_SESSIONS: usize = 4096;

pub struct Sessions {
    map: HashMap<Address, (u64, Session)>,
    clock: u64,
    path: PathBuf,
    dirty: bool,
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
        Sessions { map, clock, path, dirty: false }
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
        self.dirty = true;
        self.save();
    }

    /// Writes to disk if anything has changed since last time.
    pub fn save(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let mut b = Vec::with_capacity(self.map.len() * SESSION_LEN);
        for (_, s) in self.map.values() {
            b.extend_from_slice(&s.to_bytes());
        }
        let tmp = self.path.with_extension("tmp");
        if fs::write(&tmp, &b).is_ok() {
            let _ = fs::rename(&tmp, &self.path);
        }
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
        }
        self.map.insert(s.peer(), (self.clock, s));
        self.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maarse_crypto::Identity;
    use rand_core::OsRng;

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
