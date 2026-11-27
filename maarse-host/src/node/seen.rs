//! Messages already received, so a recording sent again later is ignored.
//!
//! The router forgets a packet after a while and at a restart. A direct
//! message in the static format and a channel message carry nothing that
//! says "already read", and an old first message of a session would replace
//! the current session. So the host remembers an ID (a hash of the sealed
//! packet) and the sender of the last [`CAP`] messages it accepted, in
//! `seen.bin` (records of ID (8, LE) | sender (16)). The same sealed bytes
//! are never a new message: every sealing uses fresh keys or nonces.

use maarse_core::Address;
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// How many received messages are remembered.
const CAP: usize = 20_000;
const RECORD: usize = 8 + 16;

pub(super) struct Seen {
    path: PathBuf,
    senders: HashMap<u64, Address>,
    order: VecDeque<u64>,
    /// Records in the file (rewritten when it has twice what is remembered).
    in_file: usize,
}

impl Seen {
    pub(super) fn load(dir: &Path) -> Self {
        let path = dir.join("seen.bin");
        let data = fs::read(&path).unwrap_or_default();
        let mut s = Seen { path, senders: HashMap::new(), order: VecDeque::new(), in_file: data.len() / RECORD };
        for r in data.chunks_exact(RECORD) {
            if let (Ok(id), Some(a)) = (r[..8].try_into(), Address::from_slice(&r[8..])) {
                s.remember(u64::from_le_bytes(id), a);
            }
        }
        // A torn record at the end would shift all that follow.
        if data.len() % RECORD != 0 {
            s.rewrite();
        }
        s
    }

    /// Who sent the message with this ID, if we have received it.
    pub(super) fn sender(&self, id: u64) -> Option<Address> {
        self.senders.get(&id).copied()
    }

    pub(super) fn add(&mut self, id: u64, sender: Address) {
        if !self.remember(id, sender) {
            return;
        }
        if self.in_file >= CAP * 2 {
            return self.rewrite();
        }
        if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&self.path) {
            let mut r = id.to_le_bytes().to_vec();
            r.extend_from_slice(&sender.0);
            if f.write_all(&r).is_ok() {
                self.in_file += 1;
            }
        }
    }

    fn remember(&mut self, id: u64, sender: Address) -> bool {
        if self.senders.insert(id, sender).is_some() {
            return false;
        }
        self.order.push_back(id);
        if self.order.len() > CAP
            && let Some(old) = self.order.pop_front()
        {
            self.senders.remove(&old);
        }
        true
    }

    fn rewrite(&mut self) {
        let mut b = Vec::with_capacity(self.order.len() * RECORD);
        for id in &self.order {
            b.extend_from_slice(&id.to_le_bytes());
            b.extend_from_slice(&self.senders[id].0);
        }
        if crate::private_file::write(&self.path, &b, false).is_ok() {
            self.in_file = self.order.len();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_across_a_restart_and_forgets_the_oldest() {
        let dir = std::env::temp_dir().join(format!("maarse-seen-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut s = Seen::load(&dir);
        for i in 0..CAP as u64 + 1 {
            s.add(i, Address([1; 16]));
        }
        let s = Seen::load(&dir);
        assert_eq!(s.sender(0), None);
        assert_eq!(s.sender(CAP as u64), Some(Address([1; 16])));
        // A torn record at the end is cut away.
        fs::OpenOptions::new().append(true).open(dir.join("seen.bin")).unwrap().write_all(b"half").unwrap();
        let mut s = Seen::load(&dir);
        s.add(u64::MAX, Address([2; 16]));
        assert_eq!(Seen::load(&dir).sender(u64::MAX), Some(Address([2; 16])));
        let _ = fs::remove_dir_all(&dir);
    }
}
