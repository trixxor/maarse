//! Weekly mail keys on a radio: forward secrecy for mail kept by post offices.
//! The same as `maarse-host/src/node/mailkey.rs`, in fixed memory.
//!
//! The node keeps a weekly hash chain (the inbox chain of linked devices, see
//! `maarse_crypto::person`), tells contacts this week's public key over the
//! encrypted session and opens mail sealed to it. Weeks that are too old are
//! overwritten in flash.
//!
//! * A radio has a key only while it knows the time (a phone, a PC or the
//!   hellos of its neighbours, see `maarse_core::clock`) and can save it. A
//!   clock that is far ahead makes it delete old weeks early: mail waiting
//!   for those weeks can then not be opened.
//! * The chain and the keys of [`PEERS`] contacts are stored; with more
//!   contacts, the oldest entry gives way and that mail gets the static seal.

use super::{FwNode, Settings, log};
use embassy_time::{Duration, Instant};
use maarse_core::content::{self, person as op};
use maarse_core::mail::MAX_DEPOSITED;
use maarse_core::Address;
use maarse_crypto::PublicIdentity;
use maarse_crypto::person::{self as pc, INBOX_STATE_LEN, InboxChain, InboxPublic};
use maarse_crypto::ratchet::{Format, Opened};
use maarse_crypto::seal::CryptoError;
use rand_core::{CryptoRng, RngCore};

/// Contacts whose keys are kept.
pub const PEERS: usize = 4;
const PEER_LEN: usize = 16 + 35;
/// The stored form: a flag and our chain, the number of contacts, the contacts.
pub const STORED_LEN: usize = 1 + INBOX_STATE_LEN + 1 + PEERS * PEER_LEN;
/// Weeks of keys kept back after the current one.
const KEEP_WEEKS: u16 = 2;
/// A message may name a week at most this far after the chain we hold.
const MAX_AHEAD: u16 = 8;
/// Wait between tries to store a chain whose old weeks were deleted.
const SAVE_RETRY: Duration = Duration::from_secs(600);
/// A key that was not acknowledged is sent again after this long.
const RETRY: Duration = Duration::from_secs(3600);

struct Told {
    peer: Address,
    week: u16,
    sent: Instant,
    msg_id: u64,
    acked: bool,
}

#[derive(Default)]
pub(super) struct MailKeys {
    chain: Option<InboxChain>,
    peers: heapless::Vec<(Address, InboxPublic), PEERS>,
    told: heapless::Vec<Told, 8>,
    /// Nodes whose announce says they take weekly keys.
    capable: heapless::Vec<Address, 16>,
    /// Contacts to send our key to at the next step.
    due: heapless::Vec<Address, 4>,
    /// Old weeks were deleted but flash still holds them: try again then.
    unsaved: Option<Instant>,
}

impl MailKeys {
    pub(super) fn load(b: &[u8; STORED_LEN]) -> Self {
        let mut m = MailKeys::default();
        if b[0] == 1 {
            m.chain = InboxChain::from_bytes(&b[1..1 + INBOX_STATE_LEN]);
        }
        let count = (b[1 + INBOX_STATE_LEN] as usize).min(PEERS);
        for r in b[2 + INBOX_STATE_LEN..].chunks_exact(PEER_LEN).take(count) {
            if let (Some(a), Some(k)) = (Address::from_slice(&r[..16]), InboxPublic::from_bytes(&r[16..])) {
                let _ = m.peers.push((a, k));
            }
        }
        m
    }

    fn to_bytes(&self) -> [u8; STORED_LEN] {
        let mut b = [0u8; STORED_LEN];
        if let Some(c) = &self.chain {
            b[0] = 1;
            b[1..1 + INBOX_STATE_LEN].copy_from_slice(&c.to_bytes());
        }
        b[1 + INBOX_STATE_LEN] = self.peers.len() as u8;
        for ((a, k), r) in self.peers.iter().zip(b[2 + INBOX_STATE_LEN..].chunks_exact_mut(PEER_LEN)) {
            r[..16].copy_from_slice(&a.0);
            r[16..].copy_from_slice(&k.to_bytes());
        }
        b
    }

    pub(super) fn set_capable(&mut self, peer: Address, capable: bool) {
        let at = self.capable.iter().position(|a| *a == peer);
        match (at, capable) {
            (None, true) => {
                if self.capable.is_full() {
                    self.capable.remove(0);
                }
                let _ = self.capable.push(peer);
            }
            (Some(i), false) => {
                self.capable.swap_remove(i);
            }
            _ => {}
        }
    }

    /// `peer` may need our key: sent at the next step if so.
    pub(super) fn want(&mut self, peer: Address) {
        if self.capable.contains(&peer) && !self.due.contains(&peer) && !self.due.is_full() {
            let _ = self.due.push(peer);
        }
    }

    /// Is `msg_id` one of our key messages? It is then marked as received.
    pub(super) fn acked(&mut self, msg_id: u64) -> bool {
        match self.told.iter_mut().find(|t| t.msg_id == msg_id) {
            Some(t) => {
                t.acked = true;
                true
            }
            None => false,
        }
    }

    fn should_tell(&self, peer: &Address, week: u16) -> bool {
        self.told.iter().find(|t| t.peer == *peer).is_none_or(|t| t.week != week || (!t.acked && t.sent.elapsed() >= RETRY))
    }

    /// Remembers `peer`'s key. `true` if it changed and must be stored.
    fn note_peer(&mut self, peer: Address, key: InboxPublic) -> bool {
        if let Some(e) = self.peers.iter_mut().find(|e| e.0 == peer) {
            if (e.1.chain == key.chain && key.week < e.1.week) || e.1 == key {
                return false;
            }
            e.1 = key;
        } else {
            if self.peers.is_full() {
                self.peers.remove(0);
            }
            let _ = self.peers.push((peer, key));
        }
        true
    }
}

impl<R: RngCore + CryptoRng, S: Settings> FwNode<R, S> {
    fn mail_week(&self) -> Option<u16> {
        self.unix_now().map(pc::week_of)
    }

    fn save_mail_keys(&mut self) -> bool {
        let b = self.mailkeys.to_bytes();
        self.settings.save_mail_keys(&b)
    }

    /// Periodic: delete old weeks and send our key to those who need it.
    pub(super) fn mailkey_step(&mut self) {
        let Some(week) = self.mail_week() else { return };
        let keep_from = week.saturating_sub(KEEP_WEEKS);
        if let Some(c) = &self.mailkeys.chain
            && c.week < keep_from
        {
            self.mailkeys.chain = c.at(keep_from);
            self.mailkeys.unsaved = Some(Instant::now());
        }
        // The old value is overwritten in flash: the forward secrecy. Tried
        // until it works.
        if self.mailkeys.unsaved.is_some_and(|at| Instant::now() >= at) {
            if self.save_mail_keys() {
                self.mailkeys.unsaved = None;
            } else {
                self.mailkeys.unsaved = Some(Instant::now() + SAVE_RETRY);
                log("Old mail keys could not be erased: trying again");
            }
        }
        let due = core::mem::take(&mut self.mailkeys.due);
        for peer in due {
            self.mailkey_tell(peer, week);
        }
    }

    #[inline(never)]
    fn mailkey_tell(&mut self, peer: Address, week: u16) {
        if self.modem || !self.known.contains_key(&peer) || !self.mailkeys.should_tell(&peer, week) {
            return;
        }
        if self.mailkeys.chain.is_none() {
            let id = (self.rng.next_u32() & 0xff) as u8;
            self.mailkeys.chain = Some(InboxChain::new(&mut self.rng, id, week));
            // A key nobody can read the mail for after a restart is not handed out.
            if !self.save_mail_keys() {
                self.mailkeys.chain = None;
                return;
            }
        }
        let Some(c) = self.mailkeys.chain.as_ref().and_then(|c| c.at(week)) else { return };
        let key = InboxPublic { chain: c.chain, week, key: c.public() };
        let mut body = [0u8; 37];
        body[0] = content::PERSON;
        body[1] = op::DEVICE_INBOX;
        body[2..].copy_from_slice(&key.to_bytes());
        let Ok(msg_id) = self.send_plain(peer, &body) else { return };
        self.mailkeys.told.retain(|t| t.peer != peer);
        if self.mailkeys.told.is_full() {
            self.mailkeys.told.remove(0);
        }
        let _ = self.mailkeys.told.push(Told { peer, week, sent: Instant::now(), msg_id, acked: false });
    }

    /// A contact's key, in a message sealed by it.
    pub(super) fn on_device_inbox(&mut self, from: Address, data: &[u8]) {
        if let Some(key) = InboxPublic::from_bytes(data)
            && self.mailkeys.note_peer(from, key)
        {
            self.save_mail_keys();
            log("A contact's mail key saved");
        }
    }

    /// Seals a post office copy to `dest`'s weekly key, if we have a recent
    /// one and the longer form still fits in a deposit.
    #[inline(never)]
    pub(super) fn mail_seal_to_key(&mut self, dest: &Address, plain: &[u8], out: &mut [u8]) -> Option<usize> {
        let week = self.mail_week()?;
        let key = self.mailkeys.peers.iter().find(|e| e.0 == *dest).map(|e| e.1)?;
        // At most one week old: mail may wait a week and must still open.
        if key.week.saturating_add(1) < week || plain.len() + pc::INBOX_OVERHEAD > MAX_DEPOSITED {
            return None;
        }
        pc::seal_inbox(&self.identity, dest, &key, &mut self.rng, plain, out).ok()
    }

    /// Opens mail sealed to our weekly key. `None`: not such a message (the
    /// ordinary formats are tried).
    #[inline(never)]
    pub(super) fn mail_open(&mut self, payload: &[u8], out: &mut [u8]) -> Option<Result<Opened, CryptoError>> {
        let (chain, week) = pc::inbox_header(payload)?;
        let latest = match self.mail_week() {
            Some(now) => now.saturating_add(1),
            None => self.mailkeys.chain.as_ref()?.week.saturating_add(MAX_AHEAD),
        };
        if week > latest {
            return None;
        }
        let secret = self.mailkeys.chain.as_ref().filter(|c| c.chain == chain)?.at(week)?.secret();
        let known = &self.known;
        let lookup = |a: &Address| -> Option<PublicIdentity> { known.get(a).map(|(id, _)| *id) };
        match pc::open_inbox(&secret, &self.addr, payload, lookup, out) {
            Ok((from, len)) => Some(Ok(Opened { from, len, format: Format::Static })),
            // The sender is hidden with this key: a real message from a
            // node whose key we lack.
            Err(e @ CryptoError::UnknownSender(_)) => Some(Err(e)),
            Err(_) => None,
        }
    }
}
