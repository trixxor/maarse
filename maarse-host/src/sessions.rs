//! Forward secrecy-sessioner (se `maarse_crypto::ratchet`), gemt på disk.
//!
//! Filen `sessioner.bin` er bare sessionerne efter hinanden (fast længde).
//! Den skrives atomisk (midlertidig fil + omdøbning) efter hver ændring - på
//! en PC er det billigt, og en mistet tilstand betyder tabte beskeder.
//! Filen indeholder hemmelige nøgler, ligesom `identitet`.

use maarse_core::Address;
use maarse_crypto::ratchet::{SESSION_LEN, Session, SessionStore};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// Højst så mange sessioner (de mindst brugte smides ud).
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
            for (i, chunk) in b.chunks_exact(SESSION_LEN).enumerate() {
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

    /// Skriver til disk, hvis noget er ændret siden sidst.
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
        // Kaldes kun, når sessionen skal bruges (og typisk ændres).
        self.dirty = true;
        self.map.get_mut(peer).map(|(t, s)| {
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
        let path = dir.join("sessioner.bin");
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
