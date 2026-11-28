//! Sequence numbers per conversation (`content::SEQ`): the sender numbers
//! its messages to each contact, so a gap is shown ("2 messages missing")
//! instead of passing silently. Only between nodes that announce
//! `appdata::CAP_SEQ` (hosts). Kept in `seq.txt`: `out|in <address> <n>`.

use super::{Node, short};
use maarse_core::Address;
use maarse_core::companion::{Event, text};
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

pub(super) struct Seqs {
    file: PathBuf,
    out: HashMap<Address, u32>,
    inn: HashMap<Address, u32>,
    dirty: bool,
    /// The latest numbered messages shown (sender and a hash of the content,
    /// number included). In memory only.
    shown: VecDeque<(Address, u64)>,
}

/// Numbered messages remembered.
const SHOWN: usize = 512;

impl Seqs {
    pub(super) fn load(dir: &Path) -> Self {
        let mut s = Seqs { file: dir.join("seq.txt"), out: HashMap::new(), inn: HashMap::new(), dirty: false, shown: VecDeque::new() };
        for line in fs::read_to_string(&s.file).unwrap_or_default().lines() {
            let v: Vec<&str> = line.split_whitespace().collect();
            if let [dir, a, n] = v[..]
                && let (Some(a), Ok(n)) = (super::parse_addr(a), n.parse::<u32>())
            {
                if dir == "out" { s.out.insert(a, n) } else { s.inn.insert(a, n) };
            }
        }
        s
    }

    pub(super) fn save(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let mut t = String::new();
        for (d, m) in [("out", &self.out), ("in", &self.inn)] {
            for (a, n) in m {
                t.push_str(&format!("{d} {a} {n}\n"));
            }
        }
        let _ = fs::write(&self.file, t);
    }

    /// `false` for a numbered message from `from` that has been shown lately.
    /// The content counts too: a sender that lost its counter may use a number
    /// again, for another message.
    pub(super) fn first_time(&mut self, from: Address, numbered: &[u8]) -> bool {
        let key = (from, maarse_core::dedup::packet_id(maarse_core::PacketType::Data, &from, numbered));
        if self.shown.contains(&key) {
            return false;
        }
        if self.shown.len() >= SHOWN {
            self.shown.pop_front();
        }
        self.shown.push_back(key);
        true
    }

    /// The next number for a message to `dest`.
    pub(super) fn next(&mut self, dest: &Address) -> u32 {
        let n = self.out.entry(*dest).or_insert(0);
        *n += 1;
        self.dirty = true;
        *n
    }
}

pub(super) use maarse_hostrules::seq::gap_notice;
use maarse_hostrules::seq::{Seen, check};

impl Node {
    /// Message number `n` from `from` (a device). Shows a gap before it.
    pub(super) fn seq_received(&mut self, from: Address, n: u32, hops: u8) {
        let last = self.seqs.inn.get(&from).copied().unwrap_or(0);
        let seen = check(last, n);
        if seen == Seen::LateOrRepeat {
            // Late (it was counted as missing before) or a repeat.
            return;
        }
        self.seqs.inn.insert(from, n);
        self.seqs.dirty = true;
        if let Seen::Gap(missing) = seen {
            self.log(format!("{} message(s) from {} missing before number {n}", missing, short(&from)));
            let peer = self.person_peer(&from);
            self.broadcast(&Event::DirectReceived { from: peer, hops, text: text(gap_notice(missing).as_bytes()) });
        }
    }
}
