//! Forward secrecy for mail kept by post offices, for a node that is not part
//! of an account (accounts have the same thing in `person.rs`).
//!
//! A message that gets no end-to-end acknowledgement is left at a post office
//! sealed in the static format, which anyone who later learns the recipient's
//! long-term key can open. Instead the node keeps a weekly hash chain (the
//! inbox chain of linked devices, see `maarse_crypto::person`), tells its
//! contacts this week's public key over the encrypted session, and opens mail
//! sealed to it. A week at a time the old chain value is deleted, so mail that
//! has been delivered or has expired cannot be opened any more, whatever
//! happens to the long-term key.
//!
//! * The node's own address plays the part of the person.
//! * A sender uses a key that is at most one week old, so mail that waits the
//!   full seven days is still readable when it is fetched. Otherwise (no key,
//!   an old key, a contact that does not know this) the static seal is used.
//! * Radio nodes do the same (`firmware/common/src/node/mailkey.rs`), once
//!   they know the time. A key is sent only to a node that announces
//!   `appdata::CAP2_MAILKEY`.
//! * Contacts' keys are kept in `mailkey-peers.bin`, so a restart of the
//!   sender does not fall back to the static seal. A key we sent that was not
//!   acknowledged is sent again at the next message after [`RETRY`].

use super::{Node, Pending, unix_now};
use maarse_core::Address;
use maarse_core::content::{self, person as op};
use maarse_crypto::person::{self as pc, InboxChain, InboxPublic};
use rand_core::{OsRng, RngCore};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Weeks of keys kept back after the current one.
pub(super) const KEEP_WEEKS: u16 = 2;
const FILE: &str = "mailkey.bin";
const PEERS_FILE: &str = "mailkey-peers.bin";
/// One record in [`PEERS_FILE`]: address and key.
const PEER_RECORD: usize = 16 + 35;
/// How long to wait for the acknowledgement before our key is sent again.
const RETRY: Duration = Duration::from_secs(3600);
/// Contacts remembered (keys received and keys sent).
const MAX_PEERS: usize = 4096;

pub(super) fn week_now() -> u16 {
    pc::week_of(unix_now())
}

pub(super) struct MailKeys {
    path: PathBuf,
    /// The chain from the oldest week kept.
    chain: Option<InboxChain>,
    /// What contacts told us: their key for this week (or a recent one).
    peers: HashMap<Address, InboxPublic>,
    peers_path: PathBuf,
    /// What we last told each contact.
    told: HashMap<Address, Told>,
    /// Nodes whose announce says they take weekly keys.
    capable: HashSet<Address>,
    /// Old weeks were deleted but the file still holds them.
    unsaved: bool,
}

struct Told {
    week: u16,
    sent: Instant,
    acked: bool,
}

impl MailKeys {
    pub(super) fn load(dir: &Path) -> Self {
        let path = dir.join(FILE);
        let chain = fs::read(&path).ok().and_then(|b| InboxChain::from_bytes(&b));
        let peers_path = dir.join(PEERS_FILE);
        let peers = fs::read(&peers_path)
            .unwrap_or_default()
            .chunks_exact(PEER_RECORD)
            .take(MAX_PEERS)
            .filter_map(|r| Some((Address::from_slice(&r[..16])?, InboxPublic::from_bytes(&r[16..])?)))
            .collect();
        MailKeys { path, chain, peers, peers_path, told: HashMap::new(), capable: HashSet::new(), unsaved: false }
    }

    /// What `peer`'s announce says about weekly keys.
    pub(super) fn set_capable(&mut self, peer: Address, capable: bool) {
        if !capable {
            self.capable.remove(&peer);
            return;
        }
        if self.capable.len() >= MAX_PEERS {
            self.capable.clear();
        }
        self.capable.insert(peer);
    }

    /// Contacts' keys are public: a lost write only means the static seal
    /// until they tell us again.
    fn save_peers(&self) {
        let mut b = Vec::with_capacity(self.peers.len() * PEER_RECORD);
        for (a, k) in &self.peers {
            b.extend_from_slice(&a.0);
            b.extend_from_slice(&k.to_bytes());
        }
        let _ = crate::private_file::write(&self.peers_path, &b, false);
    }

    fn save(&self) -> std::io::Result<()> {
        match &self.chain {
            Some(c) => crate::private_file::write(&self.path, &c.to_bytes(), true),
            None => Ok(()),
        }
    }

    /// This week's public key; the chain is made the first time it is needed.
    /// `None` if the chain cannot be saved: a key nobody can read the mail for
    /// after a restart must not be handed out.
    fn public(&mut self) -> Option<InboxPublic> {
        let week = week_now();
        if self.chain.is_none() {
            self.chain = Some(InboxChain::new(&mut OsRng, (OsRng.next_u32() & 0xff) as u8, week));
            if self.save().is_err() {
                self.chain = None;
                return None;
            }
        }
        let c = self.chain.as_ref()?.at(week)?;
        Some(InboxPublic { chain: c.chain, week, key: c.public() })
    }

    /// The chain id (the part of a key that tells chains apart).
    pub(super) fn chain_id(&self) -> Option<u8> {
        self.chain.as_ref().map(|c| c.chain)
    }

    /// The secret for a message's chain and week, if we still hold it.
    pub(super) fn secret(&self, chain: u8, week: u16) -> Option<pc::InboxSecret> {
        if week > week_now().saturating_add(1) {
            return None;
        }
        let c = self.chain.as_ref().filter(|c| c.chain == chain)?;
        c.at(week).map(|c| c.secret())
    }

    /// Deletes the weeks that are too old. This is the forward secrecy: the
    /// old chain value is overwritten on the disk. A write that fails is tried
    /// again at every call; the error is returned the first time.
    fn rotate(&mut self) -> Option<std::io::Error> {
        let keep_from = week_now().saturating_sub(KEEP_WEEKS);
        let mut moved = false;
        if let Some(c) = &self.chain
            && c.week < keep_from
        {
            self.chain = c.at(keep_from);
            moved = true;
        }
        if !moved && !self.unsaved {
            return None;
        }
        let first = !self.unsaved;
        match self.save() {
            Ok(()) => {
                self.unsaved = false;
                None
            }
            Err(e) => {
                self.unsaved = true;
                first.then_some(e)
            }
        }
    }

    /// The contact's key, if it is recent enough for mail that waits a week.
    pub(super) fn usable(&self, peer: &Address) -> Option<InboxPublic> {
        self.peers.get(peer).copied().filter(|k| k.week.saturating_add(1) >= week_now())
    }

    pub(super) fn note_peer(&mut self, peer: Address, key: InboxPublic) {
        let newer = self.peers.get(&peer).is_none_or(|o| o.chain != key.chain || key.week >= o.week);
        if newer && self.peers.get(&peer).is_none_or(|o| o.to_bytes() != key.to_bytes()) {
            if self.peers.len() >= MAX_PEERS && !self.peers.contains_key(&peer) {
                self.peers.clear();
            }
            self.peers.insert(peer, key);
            self.save_peers();
        }
    }

    /// Is it time to send `peer` this week's key?
    fn should_tell(&self, peer: &Address, week: u16) -> bool {
        self.told.get(peer).is_none_or(|t| t.week != week || (!t.acked && t.sent.elapsed() >= RETRY))
    }

    /// `peer` has acknowledged our key.
    pub(super) fn acked(&mut self, peer: &Address) {
        if let Some(t) = self.told.get_mut(peer) {
            t.acked = true;
        }
    }

    /// Forget that `peer` has our key (it used an old one).
    pub(super) fn forget_told(&mut self, peer: &Address) {
        self.told.remove(peer);
    }
}

impl Node {
    /// Tells `peer` this week's key, once a week. Not for a node that is part
    /// of an account: the account's key goes with its certificate.
    pub(super) fn mailkey_introduce(&mut self, peer: Address) {
        if self.persons.me.is_some() || peer == self.addr || !self.known.contains_key(&peer) || !self.mailkeys.capable.contains(&peer) {
            return;
        }
        let week = week_now();
        if !self.mailkeys.should_tell(&peer, week) {
            return;
        }
        let Some(key) = self.mailkeys.public() else { return };
        if self.mailkeys.told.len() >= MAX_PEERS {
            self.mailkeys.told.clear();
        }
        self.mailkeys.told.insert(peer, Told { week, sent: Instant::now(), acked: false });
        let mut body = vec![content::PERSON, op::DEVICE_INBOX];
        body.extend_from_slice(&key.to_bytes());
        let _ = self.send_sealed(peer, &body, Pending::MailKey(peer));
    }

    /// A key from a contact, in a message sealed by it (so it is theirs).
    pub(super) fn on_device_inbox(&mut self, from: Address, data: &[u8]) {
        if let Some(key) = InboxPublic::from_bytes(data) {
            self.mailkeys.note_peer(from, key);
        }
    }

    /// The sealed form of `plain` for a post office to keep for `dest`:
    /// to its weekly key if we have a recent one.
    pub(super) fn mail_seal_to_key(&mut self, dest: &Address, plain: &[u8]) -> Option<Vec<u8>> {
        let key = self.mailkeys.usable(dest)?;
        let mut out = [0u8; 255];
        let n = pc::seal_inbox(&self.identity, dest, &key, &mut OsRng, plain, &mut out).ok()?;
        // This form is a little longer than the static one: a message that
        // only fits the static form must still reach the post office.
        Node::fits_deposit(n).then(|| out[..n].to_vec())
    }

    /// Housekeeping: delete weeks that are too old.
    pub(super) fn mailkey_maintain(&mut self) {
        if let Some(e) = self.mailkeys.rotate() {
            self.log(format!("old mail keys could not be erased from the disk (trying again): {e}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maarse_crypto::Identity;

    #[test]
    fn a_week_that_has_been_rotated_away_cannot_be_opened() {
        let dir = std::env::temp_dir().join(format!("maarse-mailkey-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let sender = Identity::generate(&mut OsRng);
        let receiver = Identity::generate(&mut OsRng);
        let mut keys = MailKeys::load(&dir);
        let key = keys.public().unwrap();
        let mut out = [0u8; 255];
        let n = pc::seal_inbox(&sender, &receiver.address(), &key, &mut OsRng, b"for later", &mut out).unwrap();
        let sealed = out[..n].to_vec();
        let sp = sender.public();
        let open = |keys: &MailKeys| {
            let (chain, week) = pc::inbox_header(&sealed).unwrap();
            let secret = keys.secret(chain, week)?;
            let mut pt = [0u8; 256];
            pc::open_inbox(&secret, &receiver.address(), &sealed, |a| (*a == sp.address()).then_some(sp), &mut pt).ok().map(|(_, n)| pt[..n].to_vec())
        };
        assert_eq!(open(&keys).as_deref(), Some(&b"for later"[..]));
        // Survives a restart.
        let mut again = MailKeys::load(&dir);
        assert_eq!(open(&again).as_deref(), Some(&b"for later"[..]));
        // A chain that has moved on three weeks no longer holds that week.
        let c = again.chain.take().unwrap();
        again.chain = c.at(c.week + KEEP_WEEKS + 1);
        assert_eq!(open(&again), None);
        // And the later weeks are still derived from the kept value.
        assert!(again.chain.as_ref().unwrap().at(c.week + KEEP_WEEKS + 3).is_some());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_rotation_that_cannot_be_saved_is_tried_again() {
        let dir = std::env::temp_dir().join(format!("maarse-mailkey-save-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut keys = MailKeys::load(&dir);
        let old = week_now() - KEEP_WEEKS - 3;
        keys.chain = Some(InboxChain::new(&mut OsRng, 1, old));
        keys.save().unwrap();
        // A folder where the temporary file goes makes the write fail.
        fs::create_dir_all(dir.join("mailkey.tmp")).unwrap();
        assert!(keys.rotate().is_some(), "the failure is reported");
        assert!(keys.rotate().is_none(), "and only once");
        assert_eq!(MailKeys::load(&dir).chain.unwrap().week, old, "still on the disk");
        fs::remove_dir_all(dir.join("mailkey.tmp")).unwrap();
        assert!(keys.rotate().is_none());
        let kept = MailKeys::load(&dir).chain.unwrap();
        assert_eq!(kept.week, week_now() - KEEP_WEEKS, "the old weeks are gone from the disk");
        assert!(kept.at(old).is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn contacts_keys_survive_a_restart() {
        let dir = std::env::temp_dir().join(format!("maarse-mailkey-peers-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let peer = Address([7; 16]);
        let key = InboxPublic { chain: 3, week: week_now(), key: [2; 32] };
        MailKeys::load(&dir).note_peer(peer, key);
        let again = MailKeys::load(&dir).usable(&peer).expect("the key was not kept");
        assert_eq!(again.to_bytes(), key.to_bytes());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_key_that_was_not_acknowledged_is_sent_again() {
        let dir = std::env::temp_dir().join(format!("maarse-mailkey-retry-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let mut keys = MailKeys::load(&dir);
        let peer = Address([8; 16]);
        let week = week_now();
        assert!(keys.should_tell(&peer, week));
        keys.told.insert(peer, Told { week, sent: Instant::now(), acked: false });
        assert!(!keys.should_tell(&peer, week), "not again straight away");
        // Not possible on a computer that has run for less than RETRY.
        if let Some(long_ago) = Instant::now().checked_sub(RETRY) {
            keys.told.insert(peer, Told { week, sent: long_ago, acked: false });
            assert!(keys.should_tell(&peer, week), "lost: send it again");
        }
        keys.acked(&peer);
        assert!(!keys.should_tell(&peer, week));
        assert!(keys.should_tell(&peer, week + 1), "a new week");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_a_recent_key_is_used() {
        let dir = std::env::temp_dir().join(format!("maarse-mailkey-use-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let mut keys = MailKeys::load(&dir);
        let peer = Address([9; 16]);
        let now = week_now();
        keys.note_peer(peer, InboxPublic { chain: 1, week: now, key: [1; 32] });
        assert!(keys.usable(&peer).is_some());
        keys.peers.insert(peer, InboxPublic { chain: 1, week: now.saturating_sub(1), key: [1; 32] });
        assert!(keys.usable(&peer).is_some());
        keys.peers.insert(peer, InboxPublic { chain: 1, week: now.saturating_sub(2), key: [1; 32] });
        assert!(keys.usable(&peer).is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}
