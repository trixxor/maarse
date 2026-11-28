//! Forward secrecy for conversations: a small "double ratchet" for LoRa.
//!
//! **Draft - must be reviewed by an external cryptographer before v1.0.**
//!
//! ## Idea
//!
//! The old format ([`crate::seal`], called *static*) encrypts every message
//! with the recipient's static key. If that key is stolen, all old messages can be
//! read. Here two contacts instead build up a *session*, where the keys
//! change all the time and the used ones are deleted:
//!
//! * **Start (init)**: A makes a fresh ratchet key pair `A1` and computes
//!   `es = DH(A1, B)` and `ss = DH(A, B)` with B's static key - exactly like the
//!   static format. That gives the root key and the first chain key. A can send right
//!   away (no extra round trip, also works via a mailbox). Init messages
//!   are only as secure as the static format.
//! * **Ratchet**: When B answers, B makes a fresh key pair `B1` and mixes
//!   `DH(B1, A1)` into the root key. When A answers again, `A2` and `DH(A2, B1)`, etc.
//!   Every turn deletes the old secret key. After the first reply neither
//!   A's nor B's static key can open the messages, and a leaked session state
//!   heals as soon as both parties have taken one more turn.
//! * **Hash chain**: Within a turn a new message key is derived per message
//!   (`chain -> (new chain, message key)`), and the used one is deleted.
//! * **Loss and wrong order**: Every message has a number `n` in its chain and
//!   `pn` = number of messages in the sender's previous chain. If messages are skipped,
//!   their keys are stored in a small window ([`MAX_SKIP`] per session). If
//!   a message arrives later than the window, it can no longer be opened (it is lost).
//!
//! ## Format (69 bytes overhead, 5 more than the static one)
//!
//! ```text
//! type (1) | ratchet key (32) | sender ⊕ mask (16) | n (u16 LE) | pn (u16 LE)
//!   | ChaCha20-Poly1305(message key, message, AAD = the 53 header bytes) | tag (16)
//!
//! type  0xF0 = init (A has not heard from B in this session yet)
//!       0xF1 = ratchet
//! mask = HKDF(DH(ratchet key, the recipient's static key))   (hides the sender)
//! ```
//!
//! * The first byte in the static format is a random byte of a
//!   public key. If a message starts with `0xF0`/`0xF1`, the new
//!   format is tried first and otherwise the old one ([`open`] handles both). New nodes can
//!   therefore still read old messages. Old nodes cannot read the new ones, so
//!   a node only uses the new format for contacts that announce it in their
//!   announce ([`APP_FS`]) or have already sent us an FS message.
//! * The sender is hidden as in the static format. The ratchet key is repeated in
//!   all messages of the same turn, so a listener can see that they belong together.
//! * The key is new for every message, so the nonce is fixed (zero).
//!
//! ## Compact messages (22 bytes overhead instead of 69)
//!
//! Once a session has taken a ratchet step, a sender that knows the other
//! party can read them ([`CAP_COMPACT`](maarse_core::appdata::CAP_COMPACT)) sends
//! most messages in a short form:
//!
//! ```text
//! type 0xF2 (1) | session id (3) | n (u16 LE)
//!   | ChaCha20-Poly1305(message key, message, AAD = these 6 bytes) | tag (16)
//!
//! session id = HKDF-Expand(chain key at the start of the chain, "maarse/fs/v2/sid")[..3]
//! ```
//!
//! * The tag is the full 16 bytes and the message key is new for every
//!   message, exactly as in the long form; only the repeated ratchet key and
//!   the masked sender are left out. The recipient finds the session from the
//!   session id and the sender from the session; a message that does not open
//!   under any session with that id is rejected.
//! * The first [`FULL_FIRST`] messages after a step still carry the full
//!   ratchet key (so one lost message does not break the chain), and a sender
//!   that has heard a new key from the other party takes its next step after
//!   [`STEP_EVERY`] messages instead of at once. So the keys heal after at
//!   most that many messages in each direction (not every turn). No step is
//!   delayed before the first one (the init chain is only as strong as the
//!   static format, so it is left at once, as before).
//! * The session id and `n` are visible and repeat within a chain, so a
//!   listener can tell that messages belong together (the ratchet key already
//!   does that in the long form). The sender stays hidden.
//!
//! ## Derivation
//!
//! ```text
//! init:   rk0 | ck0  = HKDF(salt "maarse/fs/v1/init", es || ss, info A1 | R | S)
//! root:   rk' | ck   = HKDF(salt rk, DH(...), info "maarse/fs/v1/root")
//! chain:  ck' = HKDF-Expand(ck, "maarse/fs/v1/chain"),
//!         mk  = HKDF-Expand(ck, "maarse/fs/v1/message")
//! mask:   HKDF(salt "maarse/fs/v1/sender", DH(...), info ratchet key | R)
//! ```
//!
//! ## Simultaneous start
//!
//! If both parties start a session before they have heard from each other, the
//! init from the lowest address wins. The loser's init messages can still be read.
//!
//! ## Known gaps
//!
//! * Init messages (before the first reply) have no forward secrecy against B's static
//!   key - as today. There are no pre-published keys (prekeys).
//! * If the recipient loses its state (e.g. the firmware restarts), messages
//!   sent in the old session cannot be read. The node then answers with a new init
//!   (`Content::Resync`), and the conversation continues from there.
//! * A replayed old init that a node has forgotten can reset a session (only
//!   denial of service, not reading). A forged message cannot: the nodes test
//!   the session before replacing it (see `docs/protocol-spec.md`, 6.3).
//! * No header encryption: n, pn and the ratchet key are visible.

use crate::seal::CryptoError;
use crate::{Identity, PublicIdentity};
use chacha20poly1305::aead::AeadInPlace;
use chacha20poly1305::{ChaCha20Poly1305, Key, KeyInit, Nonce, Tag};
use hkdf::Hkdf;
use maarse_core::Address;
use rand_core::CryptoRngCore;
use sha2::Sha256;
use x25519_dalek::{PublicKey as XPublic, StaticSecret};
use zeroize::Zeroize;

pub const INIT: u8 = 0xF0;
pub const RATCHET: u8 = 0xF1;
/// A compact message of an established session (see the module documentation).
pub const COMPACT: u8 = 0xF2;
pub const HEADER_LEN: usize = 1 + 32 + 16 + 2 + 2;
pub const FS_OVERHEAD: usize = HEADER_LEN + 16;
pub const SID_LEN: usize = 3;
pub const COMPACT_HEADER_LEN: usize = 1 + SID_LEN + 2;
/// Overhead of a compact message (the long form is [`FS_OVERHEAD`]).
pub const COMPACT_OVERHEAD: usize = COMPACT_HEADER_LEN + 16;
/// Messages on one sending key before a sender that could step does so, when
/// it sends compact messages.
pub const STEP_EVERY: u16 = 8;
/// The first messages after a step always carry the full ratchet key.
pub const FULL_FIRST: u16 = 2;
/// Largest text that can be sent in both formats (one packet, full path).
pub const MAX_FS_TEXT: usize = maarse_core::router::MAX_DIRECT_PAYLOAD - FS_OVERHEAD;
/// Skipped message keys stored per session.
pub const MAX_SKIP: usize = 8;
/// A chain may skip at most this many messages ahead at a time (so that a
/// forged message cannot make us compute for a long time).
pub const MAX_JUMP: u16 = 512;
/// Announce field: "I can do forward secrecy (v1)". No data.
pub const APP_FS: u8 = 0x02;
/// Old ratchet keys from the other party that we can recognise (so a late
/// message is not taken for a broken session).
const OLD_PUBS: usize = 4;

/// Reads app data from an announce: does the node announce forward secrecy?
///
/// Known fields: `0x01` mailbox (16 bytes), `0x02` FS (no data).
pub fn announces_fs(app_data: &[u8]) -> bool {
    let mut i = 0;
    while i < app_data.len() {
        match app_data[i] {
            0x01 => i += 17,
            APP_FS => return true,
            _ => return false,
        }
    }
    false
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// The old format (no forward secrecy).
    Static,
    /// First messages in a session (like static, until the other party answers).
    Init,
    /// Full forward secrecy.
    Ratchet,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Opened {
    pub from: Address,
    pub len: usize,
    pub format: Format,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct Skipped {
    used: bool,
    fp: [u8; 8],
    n: u16,
    mk: [u8; 32],
}
const SKIPPED_LEN: usize = 1 + 8 + 2 + 32;

/// The state of one conversation (one contact). Fixed size, no heap.
#[derive(Clone, PartialEq, Eq)]
pub struct Session {
    peer: Address,
    rk: [u8; 32],
    cks: [u8; 32],
    ns: u16,
    pn: u16,
    ckr: [u8; 32],
    nr: u16,
    dhs: [u8; 32],
    dhs_pub: [u8; 32],
    mask: [u8; 16],
    dhr: [u8; 32],
    init_pub: [u8; 32],
    has_cks: bool,
    has_ckr: bool,
    has_dhs: bool,
    /// We started the session and have not heard from the other party yet.
    pending: bool,
    /// The next send must take a new ratchet step.
    need_step: bool,
    skipped: [Skipped; MAX_SKIP],
    skip_cursor: u8,
    old_pubs: [[u8; 8]; OLD_PUBS],
    /// Session ids of the sending and the receiving chain (see the module
    /// documentation) and whether they are known.
    sid_s: [u8; SID_LEN],
    sid_r: [u8; SID_LEN],
    has_sid_s: bool,
    has_sid_r: bool,
    /// We have taken at least one ratchet step (our sending chain no longer
    /// depends on the init keys).
    stepped: bool,
    /// Messages sent on the current sending key.
    sent_in_chain: u16,
    /// The next message is sent in the long form: the other party may have
    /// lost its session (it sent us a static message or announced itself), and
    /// only the long form lets it find the sender and start a new session.
    force_full: bool,
}

/// The length of the version 1 form (before compact messages).
pub const SESSION_LEN_V1: usize = 1 + 16 + 32 * 2 + 4 + 32 + 2 + 32 * 2 + 16 + 32 * 2 + 1
    + MAX_SKIP * SKIPPED_LEN
    + 1
    + OLD_PUBS * 8;
/// The length of [`Session::to_bytes`].
pub const SESSION_LEN: usize = SESSION_LEN_V1 + SID_LEN * 2 + 2 + 1;
const SESSION_VERSION: u8 = 2;

impl Drop for Session {
    fn drop(&mut self) {
        self.rk.zeroize();
        self.cks.zeroize();
        self.ckr.zeroize();
        self.dhs.zeroize();
        for s in self.skipped.iter_mut() {
            s.mk.zeroize();
        }
    }
}

impl core::fmt::Debug for Session {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Never keys in logs.
        f.debug_struct("Session")
            .field("peer", &self.peer)
            .field("ns", &self.ns)
            .field("nr", &self.nr)
            .field("pending", &self.pending)
            .finish_non_exhaustive()
    }
}

/// Where sessions are stored (host: HashMap on disk; firmware: a fixed number in RAM).
pub trait SessionStore {
    fn get_mut(&mut self, peer: &Address) -> Option<&mut Session>;
    /// Inserts or replaces the session for `s.peer()`. A full store
    /// may evict another session (it then falls back to init).
    fn insert(&mut self, s: Session);
    /// The `skip`-th session whose receive chain has session id `sid`
    /// (ids can collide, so the caller tries them one by one).
    fn find_sid(&mut self, sid: &[u8; SID_LEN], skip: usize) -> Option<&mut Session>;
}

fn dh(secret: &[u8; 32], public: &[u8; 32]) -> Result<[u8; 32], CryptoError> {
    let s = StaticSecret::from(*secret);
    let out = s.diffie_hellman(&XPublic::from(*public));
    if !out.was_contributory() {
        return Err(CryptoError::Decrypt);
    }
    Ok(*out.as_bytes())
}

fn public_of(secret: &[u8; 32]) -> [u8; 32] {
    XPublic::from(&StaticSecret::from(*secret)).to_bytes()
}

fn split64(okm: &[u8; 64]) -> ([u8; 32], [u8; 32]) {
    let mut a = [0u8; 32];
    let mut b = [0u8; 32];
    a.copy_from_slice(&okm[..32]);
    b.copy_from_slice(&okm[32..]);
    (a, b)
}

fn kdf_init(
    es: &[u8; 32],
    ss: &[u8; 32],
    init_pub: &[u8; 32],
    recipient: &Address,
    sender: &Address,
) -> ([u8; 32], [u8; 32]) {
    let mut ikm = [0u8; 64];
    ikm[..32].copy_from_slice(es);
    ikm[32..].copy_from_slice(ss);
    let mut info = [0u8; 64];
    info[..32].copy_from_slice(init_pub);
    info[32..48].copy_from_slice(&recipient.0);
    info[48..].copy_from_slice(&sender.0);
    let mut okm = [0u8; 64];
    let _ = Hkdf::<Sha256>::new(Some(b"maarse/fs/v1/init"), &ikm).expand(&info, &mut okm);
    ikm.zeroize();
    let r = split64(&okm);
    okm.zeroize();
    r
}

fn kdf_rk(rk: &[u8; 32], dh_out: &[u8; 32]) -> ([u8; 32], [u8; 32]) {
    let mut okm = [0u8; 64];
    let _ = Hkdf::<Sha256>::new(Some(rk), dh_out).expand(b"maarse/fs/v1/root", &mut okm);
    let r = split64(&okm);
    okm.zeroize();
    r
}

/// (new chain key, message key)
fn kdf_ck(ck: &[u8; 32]) -> ([u8; 32], [u8; 32]) {
    // Cannot fail: the PRK is 32 bytes, output 32 bytes.
    let hk = Hkdf::<Sha256>::from_prk(ck).expect("32 bytes PRK");
    let mut next = [0u8; 32];
    let mut mk = [0u8; 32];
    let _ = hk.expand(b"maarse/fs/v1/chain", &mut next);
    let _ = hk.expand(b"maarse/fs/v1/message", &mut mk);
    (next, mk)
}

fn mask_key(dh_out: &[u8; 32], ratchet_pub: &[u8; 32], recipient: &Address) -> [u8; 16] {
    let mut info = [0u8; 48];
    info[..32].copy_from_slice(ratchet_pub);
    info[32..].copy_from_slice(&recipient.0);
    let mut k = [0u8; 16];
    let _ = Hkdf::<Sha256>::new(Some(b"maarse/fs/v1/sender"), dh_out).expand(&info, &mut k);
    k
}

fn fp(public: &[u8; 32]) -> [u8; 8] {
    let mut f = [0u8; 8];
    f.copy_from_slice(&public[..8]);
    f
}

/// Session id of a chain, from its first chain key.
fn sid_of(ck: &[u8; 32]) -> [u8; SID_LEN] {
    let hk = Hkdf::<Sha256>::from_prk(ck).expect("32 bytes PRK");
    let mut sid = [0u8; SID_LEN];
    let _ = hk.expand(b"maarse/fs/v2/sid", &mut sid);
    sid
}

fn aead(mk: &[u8; 32]) -> ChaCha20Poly1305 {
    ChaCha20Poly1305::new(Key::from_slice(mk))
}

impl Session {
    fn blank(peer: Address) -> Self {
        Session {
            peer,
            rk: [0; 32],
            cks: [0; 32],
            ns: 0,
            pn: 0,
            ckr: [0; 32],
            nr: 0,
            dhs: [0; 32],
            dhs_pub: [0; 32],
            mask: [0; 16],
            dhr: [0; 32],
            init_pub: [0; 32],
            has_cks: false,
            has_ckr: false,
            has_dhs: false,
            pending: false,
            need_step: false,
            skipped: [Skipped::default(); MAX_SKIP],
            skip_cursor: 0,
            old_pubs: [[0; 8]; OLD_PUBS],
            sid_s: [0; SID_LEN],
            sid_r: [0; SID_LEN],
            has_sid_s: false,
            has_sid_r: false,
            stepped: false,
            sent_in_chain: 0,
            force_full: false,
        }
    }

    /// Starts a new session to `peer` (we are the initiator).
    pub fn initiate(
        me: &Identity,
        peer: &PublicIdentity,
        rng: &mut impl CryptoRngCore,
    ) -> Result<Self, CryptoError> {
        let mut a1 = [0u8; 32];
        rng.fill_bytes(&mut a1);
        let a1_pub = public_of(&a1);
        let peer_addr = peer.address();
        let es = dh(&a1, &peer.dh).map_err(|_| CryptoError::Malformed)?;
        let ss = me.dh.diffie_hellman(&XPublic::from(peer.dh));
        if !ss.was_contributory() {
            return Err(CryptoError::Malformed);
        }
        let (rk, ck) = kdf_init(&es, ss.as_bytes(), &a1_pub, &peer_addr, &me.address());
        let mut s = Session::blank(peer_addr);
        s.rk = rk;
        s.cks = ck;
        s.sid_s = sid_of(&ck);
        s.has_sid_s = true;
        s.has_cks = true;
        s.dhs = a1;
        s.dhs_pub = a1_pub;
        s.has_dhs = true;
        s.mask = mask_key(&es, &a1_pub, &peer_addr);
        s.dhr = peer.dh;
        s.init_pub = a1_pub;
        s.pending = true;
        a1.zeroize();
        Ok(s)
    }

    pub fn peer(&self) -> Address {
        self.peer
    }

    /// Has the other party answered in this session (messages have full FS)?
    pub fn established(&self) -> bool {
        !self.pending
    }

    /// The other party may have lost its session state: the next message is
    /// sent in the long form (see [`Session::seal`]). Call it when we hear a
    /// sign of that: an announce from the peer or a static message.
    pub fn force_full(&mut self) {
        self.force_full = true;
    }

    /// The session id of the chain we receive on, if known.
    pub fn receive_sid(&self) -> Option<[u8; SID_LEN]> {
        self.has_sid_r.then_some(self.sid_r)
    }

    /// The sending chain (an id for it) and the number of the next message,
    /// if we have one. A store on disk uses this to never save a state that
    /// could produce an already used message key again.
    pub fn send_position(&self) -> Option<([u8; 8], u16)> {
        self.has_cks.then(|| (fp(&self.dhs_pub), self.ns))
    }

    /// Moves the sending chain forward to message number `to` without sending:
    /// the keys in between are never used. The recipient skips them.
    pub fn advance_send(&mut self, to: u16) {
        while self.has_cks && self.ns < to {
            let (next, mut mk) = kdf_ck(&self.cks);
            mk.zeroize();
            self.cks = next;
            self.ns += 1;
        }
    }

    /// Encrypts `msg` to `peer` (must be the session's contact). Returns
    /// the length (`msg.len()` plus [`FS_OVERHEAD`], or [`COMPACT_OVERHEAD`]
    /// for a compact message). On error the session is unchanged.
    ///
    /// `compact_ok`: the other party reads compact messages. Without it
    /// nothing changes from the long form (a step at every turn).
    pub fn seal(
        &mut self,
        me: &Identity,
        peer: &PublicIdentity,
        rng: &mut impl CryptoRngCore,
        compact_ok: bool,
        msg: &[u8],
        out: &mut [u8],
    ) -> Result<usize, CryptoError> {
        if peer.address() != self.peer || self.ns == u16::MAX {
            return Err(CryptoError::Malformed);
        }
        // Delay a step only when we can send compact messages and our sending
        // chain is already a ratcheted one.
        let lazy = compact_ok && self.stepped && !self.pending && self.has_sid_s && !self.force_full;
        let step = self.need_step && (!lazy || self.sent_in_chain >= STEP_EVERY);
        let compact = lazy && !step && self.sent_in_chain >= FULL_FIRST;
        let total = msg.len() + if compact { COMPACT_OVERHEAD } else { FS_OVERHEAD };
        if out.len() < total || total > maarse_core::router::MAX_DIRECT_PAYLOAD {
            return Err(CryptoError::BufferTooSmall);
        }
        if step {
            let mut s = [0u8; 32];
            rng.fill_bytes(&mut s);
            let s_pub = public_of(&s);
            let d = dh(&s, &self.dhr).map_err(|_| CryptoError::Malformed)?;
            let m = dh(&s, &peer.dh).map_err(|_| CryptoError::Malformed)?;
            let (rk, ck) = kdf_rk(&self.rk, &d);
            self.rk = rk;
            self.cks = ck;
            self.sid_s = sid_of(&ck);
            self.has_sid_s = true;
            self.has_cks = true;
            self.pn = self.ns;
            self.ns = 0;
            self.dhs = s;
            self.dhs_pub = s_pub;
            self.has_dhs = true;
            self.mask = mask_key(&m, &s_pub, &self.peer);
            self.need_step = false;
            self.stepped = true;
            self.sent_in_chain = 0;
            s.zeroize();
        }
        if !self.has_cks {
            return Err(CryptoError::Malformed);
        }
        let (next, mut mk) = kdf_ck(&self.cks);
        let head_len;
        if compact {
            head_len = COMPACT_HEADER_LEN;
            out[0] = COMPACT;
            out[1..1 + SID_LEN].copy_from_slice(&self.sid_s);
            out[1 + SID_LEN..COMPACT_HEADER_LEN].copy_from_slice(&self.ns.to_le_bytes());
        } else {
            head_len = HEADER_LEN;
            let me_addr = me.address();
            out[0] = if self.pending { INIT } else { RATCHET };
            out[1..33].copy_from_slice(&self.dhs_pub);
            for i in 0..16 {
                out[33 + i] = me_addr.0[i] ^ self.mask[i];
            }
            out[49..51].copy_from_slice(&self.ns.to_le_bytes());
            out[51..53].copy_from_slice(&self.pn.to_le_bytes());
        }
        let (head, rest) = out.split_at_mut(head_len);
        rest[..msg.len()].copy_from_slice(msg);
        let tag = aead(&mk)
            .encrypt_in_place_detached(Nonce::from_slice(&[0u8; 12]), head, &mut rest[..msg.len()])
            .map_err(|_| CryptoError::Malformed)?;
        rest[msg.len()..msg.len() + 16].copy_from_slice(&tag);
        mk.zeroize();
        self.cks = next;
        self.ns += 1;
        self.sent_in_chain = self.sent_in_chain.saturating_add(1);
        self.force_full = false;
        Ok(total)
    }

    fn store_skipped(&mut self, fpr: [u8; 8], n: u16, mk: [u8; 32]) {
        let slot = match self.skipped.iter().position(|s| !s.used) {
            Some(i) => i,
            None => {
                let i = self.skip_cursor as usize % MAX_SKIP;
                self.skip_cursor = ((i + 1) % MAX_SKIP) as u8;
                i
            }
        };
        self.skipped[slot] = Skipped { used: true, fp: fpr, n, mk };
    }

    /// Advances the receive chain to number `target` and stores the last
    /// [`MAX_SKIP`] skipped keys.
    fn skip_to(&mut self, target: u16) -> Result<(), CryptoError> {
        if target <= self.nr {
            return Ok(());
        }
        if target - self.nr > MAX_JUMP {
            return Err(CryptoError::Decrypt);
        }
        let f = fp(&self.dhr);
        while self.nr < target {
            let (next, mk) = kdf_ck(&self.ckr);
            if target - self.nr <= MAX_SKIP as u16 {
                self.store_skipped(f, self.nr, mk);
            }
            self.ckr = next;
            self.nr += 1;
        }
        Ok(())
    }

    /// Decrypts one long-form message. Called on a copy; the copy is only stored on success.
    fn decrypt(
        &mut self,
        rpub: &[u8; 32],
        n: u16,
        pn: u16,
        allow_step: bool,
        payload: &[u8],
        out: &mut [u8],
    ) -> Result<usize, CryptoError> {
        let len = payload.len() - FS_OVERHEAD;
        self.decrypt_parts(
            rpub,
            n,
            pn,
            allow_step,
            &payload[..HEADER_LEN],
            &payload[HEADER_LEN..HEADER_LEN + len],
            &payload[HEADER_LEN + len..],
            out,
        )
    }

    /// Decrypts a message split into the authenticated header, the
    /// ciphertext and the tag.
    #[allow(clippy::too_many_arguments)]
    fn decrypt_parts(
        &mut self,
        rpub: &[u8; 32],
        n: u16,
        pn: u16,
        allow_step: bool,
        head: &[u8],
        body: &[u8],
        tag: &[u8],
        out: &mut [u8],
    ) -> Result<usize, CryptoError> {
        let len = body.len();
        let tag = Tag::from_slice(tag);
        let f = fp(rpub);
        let mut mk = if let Some(i) = self.skipped.iter().position(|s| s.used && s.fp == f && s.n == n) {
            let mk = self.skipped[i].mk;
            self.skipped[i] = Skipped::default();
            mk
        } else {
            if !(self.has_ckr && *rpub == self.dhr) {
                // New ratchet key from the other party.
                if !allow_step || !self.has_dhs || self.old_pubs.contains(&f) {
                    return Err(CryptoError::Decrypt);
                }
                if self.has_ckr {
                    self.skip_to(pn)?;
                    self.old_pubs.rotate_right(1);
                    self.old_pubs[0] = fp(&self.dhr);
                }
                let d = dh(&self.dhs, rpub)?;
                let (rk, ck) = kdf_rk(&self.rk, &d);
                self.rk = rk;
                self.ckr = ck;
                self.sid_r = sid_of(&ck);
                self.has_sid_r = true;
                self.dhr = *rpub;
                self.nr = 0;
                self.has_ckr = true;
                self.need_step = true;
            } else if n < self.nr {
                // Already used (and deleted) or too old.
                return Err(CryptoError::Decrypt);
            }
            self.skip_to(n)?;
            if self.nr == u16::MAX {
                return Err(CryptoError::Decrypt);
            }
            let (next, mk) = kdf_ck(&self.ckr);
            self.ckr = next;
            self.nr += 1;
            mk
        };
        out[..len].copy_from_slice(body);
        let r = aead(&mk).decrypt_in_place_detached(
            Nonce::from_slice(&[0u8; 12]),
            head,
            &mut out[..len],
            tag,
        );
        mk.zeroize();
        r.map_err(|_| CryptoError::Decrypt)?;
        Ok(len)
    }

    /// Is `rpub` one of the other party's old ratchet keys (late message)?
    fn knows_old(&self, rpub: &[u8; 32]) -> bool {
        let f = fp(rpub);
        self.old_pubs.contains(&f) || f == fp(&self.init_pub)
    }

    pub fn to_bytes(&self) -> [u8; SESSION_LEN] {
        let mut b = [0u8; SESSION_LEN];
        let mut w = Writer { b: &mut b, i: 0 };
        w.put(&[SESSION_VERSION]);
        w.put(&self.peer.0);
        w.put(&self.rk);
        w.put(&self.cks);
        w.put(&self.ns.to_le_bytes());
        w.put(&self.pn.to_le_bytes());
        w.put(&self.ckr);
        w.put(&self.nr.to_le_bytes());
        w.put(&self.dhs);
        w.put(&self.dhs_pub);
        w.put(&self.mask);
        w.put(&self.dhr);
        w.put(&self.init_pub);
        let flags = self.has_cks as u8
            | (self.has_ckr as u8) << 1
            | (self.has_dhs as u8) << 2
            | (self.pending as u8) << 3
            | (self.need_step as u8) << 4;
        w.put(&[flags]);
        for s in &self.skipped {
            w.put(&[s.used as u8]);
            w.put(&s.fp);
            w.put(&s.n.to_le_bytes());
            w.put(&s.mk);
        }
        w.put(&[self.skip_cursor]);
        for p in &self.old_pubs {
            w.put(p);
        }
        w.put(&self.sid_s);
        w.put(&self.sid_r);
        w.put(&self.sent_in_chain.to_le_bytes());
        w.put(&[
            self.has_sid_s as u8 | (self.has_sid_r as u8) << 1 | (self.stepped as u8) << 2 | (self.force_full as u8) << 3,
        ]);
        debug_assert_eq!(w.i, SESSION_LEN);
        b
    }

    /// Reads the current form, or the version 1 form (from before compact
    /// messages, without session ids: it sends long messages until its next step).
    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        let v1 = b.len() == SESSION_LEN_V1 && b.first() == Some(&1);
        if !v1 && (b.len() != SESSION_LEN || b[0] != SESSION_VERSION) {
            return None;
        }
        let mut r = Reader { b, i: 1 };
        let mut s = Session::blank(Address(r.arr()));
        s.rk = r.arr();
        s.cks = r.arr();
        s.ns = u16::from_le_bytes(r.arr());
        s.pn = u16::from_le_bytes(r.arr());
        s.ckr = r.arr();
        s.nr = u16::from_le_bytes(r.arr());
        s.dhs = r.arr();
        s.dhs_pub = r.arr();
        s.mask = r.arr();
        s.dhr = r.arr();
        s.init_pub = r.arr();
        let [flags] = r.arr();
        s.has_cks = flags & 1 != 0;
        s.has_ckr = flags & 2 != 0;
        s.has_dhs = flags & 4 != 0;
        s.pending = flags & 8 != 0;
        s.need_step = flags & 16 != 0;
        for k in s.skipped.iter_mut() {
            let [u] = r.arr();
            k.used = u != 0;
            k.fp = r.arr();
            k.n = u16::from_le_bytes(r.arr());
            k.mk = r.arr();
        }
        let [c] = r.arr();
        s.skip_cursor = c % MAX_SKIP as u8;
        for p in s.old_pubs.iter_mut() {
            *p = r.arr();
        }
        if !v1 {
            s.sid_s = r.arr();
            s.sid_r = r.arr();
            s.sent_in_chain = u16::from_le_bytes(r.arr());
            let [f2] = r.arr();
            s.has_sid_s = f2 & 1 != 0;
            s.has_sid_r = f2 & 2 != 0;
            s.stepped = f2 & 4 != 0;
            s.force_full = f2 & 8 != 0;
        }
        Some(s)
    }
}

struct Writer<'a> {
    b: &'a mut [u8],
    i: usize,
}
impl Writer<'_> {
    fn put(&mut self, x: &[u8]) {
        self.b[self.i..self.i + x.len()].copy_from_slice(x);
        self.i += x.len();
    }
}
struct Reader<'a> {
    b: &'a [u8],
    i: usize,
}
impl Reader<'_> {
    fn arr<const N: usize>(&mut self) -> [u8; N] {
        let mut a = [0u8; N];
        a.copy_from_slice(&self.b[self.i..self.i + N]);
        self.i += N;
        a
    }
}

/// Encrypts a direct message. Uses forward secrecy if `fs_ok`
/// (the other party can do it), and otherwise - or if it fails - the static
/// format. Returns the length and the format used.
pub fn seal(
    me: &Identity,
    peer: &PublicIdentity,
    store: &mut impl SessionStore,
    fs_ok: bool,
    rng: &mut impl CryptoRngCore,
    msg: &[u8],
    out: &mut [u8],
) -> Result<(usize, Format), CryptoError> {
    seal_compact(me, peer, store, fs_ok, false, rng, msg, out)
}

/// Like [`seal`], and with `compact_ok` (the other party reads compact
/// messages, [`CAP_COMPACT`](maarse_core::appdata::CAP_COMPACT)) the session
/// sends the short form.
#[allow(clippy::too_many_arguments)]
pub fn seal_compact(
    me: &Identity,
    peer: &PublicIdentity,
    store: &mut impl SessionStore,
    fs_ok: bool,
    compact_ok: bool,
    rng: &mut impl CryptoRngCore,
    msg: &[u8],
    out: &mut [u8],
) -> Result<(usize, Format), CryptoError> {
    if fs_ok {
        let addr = peer.address();
        let r = match store.get_mut(&addr) {
            Some(s) => s.seal(me, peer, rng, compact_ok, msg, out).map(|n| (n, s.pending)),
            None => Session::initiate(me, peer, rng).and_then(|mut s| {
                let n = s.seal(me, peer, rng, compact_ok, msg, out)?;
                store.insert(s);
                Ok((n, true))
            }),
        };
        if let Ok((n, pending)) = r {
            return Ok((n, if pending { Format::Init } else { Format::Ratchet }));
        }
    }
    crate::seal::seal_direct(me, peer, rng, msg, out).map(|n| (n, Format::Static))
}

/// Starts over with `peer`: a new init session replaces the old one.
pub fn reset(
    me: &Identity,
    peer: &PublicIdentity,
    store: &mut impl SessionStore,
    rng: &mut impl CryptoRngCore,
) -> Result<(), CryptoError> {
    store.insert(Session::initiate(me, peer, rng)?);
    Ok(())
}

/// For tests in other crates: makes a packet in the ratchet or init format look
/// as if it came from `claimed`, the way anyone can (the hidden sender is only
/// masked with a key the forger can work out). It never opens as a message; it
/// only gets as far as telling the recipient it has no matching session.
/// `recipient` is used for the mask, as the forger's own ephemeral key would be.
#[doc(hidden)]
pub fn forge_sender_for_test(recipient: &Identity, packet: &mut [u8], claimed: &Address) {
    let mut rpub = [0u8; 32];
    rpub.copy_from_slice(&packet[1..33]);
    let es = recipient.dh.diffie_hellman(&XPublic::from(rpub));
    let mask = mask_key(es.as_bytes(), &rpub, &recipient.address());
    for i in 0..16 {
        packet[33 + i] = claimed.0[i] ^ mask[i];
    }
    packet[0] = RATCHET;
}

/// Decrypts a direct message in both formats.
///
/// Errors besides those of [`crate::seal::open_direct`]:
/// * [`CryptoError::NoSession`]: ratchet message from a contact we do not have
///   a session with (e.g. after a restart). Answer with a new init.
/// * [`CryptoError::Desync`]: we have a session, but the message does not fit
///   (the sessions have got out of step - or the message is forged).
pub fn open(
    me: &Identity,
    payload: &[u8],
    store: &mut impl SessionStore,
    lookup: impl Fn(&Address) -> Option<PublicIdentity>,
    out: &mut [u8],
) -> Result<Opened, CryptoError> {
    if payload.len() >= COMPACT_OVERHEAD && payload[0] == COMPACT {
        return match open_compact(payload, store, out) {
            Ok(o) => Ok(o),
            // 1/256 of the old messages happen to start with 0xF2.
            Err(e) => match crate::seal::open_direct(me, payload, &lookup, out) {
                Ok((from, len)) => {
                    note_static(store, &from);
                    Ok(Opened { from, len, format: Format::Static })
                }
                Err(_) => Err(e),
            },
        };
    }
    if payload.len() >= FS_OVERHEAD && matches!(payload[0], INIT | RATCHET) {
        match open_fs(me, payload, store, &lookup, out) {
            Ok(o) => return Ok(o),
            Err(e) => {
                // 1/128 of the old messages happen to start with 0xF0/0xF1.
                return match crate::seal::open_direct(me, payload, &lookup, out) {
                    Ok((from, len)) => {
                        note_static(store, &from);
                        Ok(Opened { from, len, format: Format::Static })
                    }
                    Err(_) => Err(e),
                };
            }
        }
    }
    let (from, len) = crate::seal::open_direct(me, payload, lookup, out)?;
    note_static(store, &from);
    Ok(Opened { from, len, format: Format::Static })
}

/// A static message from a peer we have a session with: it has lost its
/// session (or never had one), so our next message goes in the long form.
fn note_static(store: &mut impl SessionStore, from: &Address) {
    if let Some(s) = store.get_mut(from) {
        s.force_full();
    }
}

/// Opens a compact message: finds the session by its session id and tries
/// every session with that id (ids can collide).
fn open_compact(
    payload: &[u8],
    store: &mut impl SessionStore,
    out: &mut [u8],
) -> Result<Opened, CryptoError> {
    let len = payload.len() - COMPACT_OVERHEAD;
    if out.len() < len {
        return Err(CryptoError::BufferTooSmall);
    }
    let mut sid = [0u8; SID_LEN];
    sid.copy_from_slice(&payload[1..1 + SID_LEN]);
    let n = u16::from_le_bytes([payload[1 + SID_LEN], payload[2 + SID_LEN]]);
    let mut skip = 0;
    while let Some(sess) = store.find_sid(&sid, skip) {
        let mut c = sess.clone();
        let rpub = c.dhr;
        if let Ok(len) = c.decrypt_parts(
            &rpub,
            n,
            0,
            false,
            &payload[..COMPACT_HEADER_LEN],
            &payload[COMPACT_HEADER_LEN..COMPACT_HEADER_LEN + len],
            &payload[COMPACT_HEADER_LEN + len..],
            out,
        ) {
            let from = c.peer;
            *sess = c;
            return Ok(Opened { from, len, format: Format::Ratchet });
        }
        skip += 1;
    }
    Err(CryptoError::Decrypt)
}

fn open_fs(
    me: &Identity,
    payload: &[u8],
    store: &mut impl SessionStore,
    lookup: &impl Fn(&Address) -> Option<PublicIdentity>,
    out: &mut [u8],
) -> Result<Opened, CryptoError> {
    let len = payload.len() - FS_OVERHEAD;
    if out.len() < len {
        return Err(CryptoError::BufferTooSmall);
    }
    let mut rpub = [0u8; 32];
    rpub.copy_from_slice(&payload[1..33]);
    let my_addr = me.address();
    let es = me.dh.diffie_hellman(&XPublic::from(rpub));
    if !es.was_contributory() {
        return Err(CryptoError::Decrypt);
    }
    let mask = mask_key(es.as_bytes(), &rpub, &my_addr);
    let mut s = [0u8; 16];
    for i in 0..16 {
        s[i] = payload[33 + i] ^ mask[i];
    }
    let from = Address(s);
    let n = u16::from_le_bytes([payload[49], payload[50]]);
    let pn = u16::from_le_bytes([payload[51], payload[52]]);

    if payload[0] == INIT {
        let peer = lookup(&from).ok_or(CryptoError::UnknownSender(from))?;
        if peer.address() != from {
            return Err(CryptoError::Decrypt);
        }
        // An init from the session we already have: only chain or window.
        if let Some(sess) = store.get_mut(&from)
            && sess.init_pub == rpub
        {
            let mut c = sess.clone();
            let n = c.decrypt(&rpub, n, pn, false, payload, out)?;
            *sess = c;
            return Ok(Opened { from, len: n, format: Format::Init });
        }
        let ss = me.dh.diffie_hellman(&XPublic::from(peer.dh));
        if !ss.was_contributory() {
            return Err(CryptoError::Decrypt);
        }
        let (rk, ck) = kdf_init(es.as_bytes(), ss.as_bytes(), &rpub, &my_addr, &from);
        let mut fresh = Session::blank(from);
        fresh.rk = rk;
        fresh.ckr = ck;
        fresh.sid_r = sid_of(&ck);
        fresh.has_sid_r = true;
        fresh.has_ckr = true;
        fresh.dhr = rpub;
        fresh.init_pub = rpub;
        fresh.need_step = true;
        let n = fresh.decrypt(&rpub, n, pn, false, payload, out)?;
        let adopt = match store.get_mut(&from) {
            None => true,
            // Both started: the init from the lowest address wins.
            Some(old) => !old.pending || from.0 < my_addr.0,
        };
        if adopt {
            store.insert(fresh);
        }
        return Ok(Opened { from, len: n, format: Format::Init });
    }

    let sess = store.get_mut(&from).ok_or(CryptoError::NoSession(from))?;
    let mut c = sess.clone();
    match c.decrypt(&rpub, n, pn, true, payload, out) {
        Ok(n) => {
            c.pending = false;
            *sess = c;
            Ok(Opened { from, len: n, format: Format::Ratchet })
        }
        Err(CryptoError::Decrypt) if !sess.knows_old(&rpub) && rpub != sess.dhr => {
            Err(CryptoError::Desync(from))
        }
        Err(e) => Err(e),
    }
}

/// A simple store with room for `N` sessions, without heap (for firmware).
/// When it is full, the least used one is evicted.
pub struct FixedStore<const N: usize> {
    slots: [Option<(u32, Session)>; N],
    clock: u32,
}

impl<const N: usize> Default for FixedStore<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> FixedStore<N> {
    pub const fn new() -> Self {
        FixedStore { slots: [const { None }; N], clock: 0 }
    }

    pub fn len(&self) -> usize {
        self.slots.iter().filter(|s| s.is_some()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn contains(&self, peer: &Address) -> bool {
        self.slots.iter().flatten().any(|(_, s)| s.peer == *peer)
    }

    pub fn remove(&mut self, peer: &Address) {
        for slot in self.slots.iter_mut() {
            if slot.as_ref().is_some_and(|(_, s)| s.peer == *peer) {
                *slot = None;
            }
        }
    }
}

impl<const N: usize> SessionStore for FixedStore<N> {
    fn get_mut(&mut self, peer: &Address) -> Option<&mut Session> {
        self.clock = self.clock.wrapping_add(1);
        let clock = self.clock;
        self.slots.iter_mut().flatten().find(|(_, s)| s.peer == *peer).map(|(t, s)| {
            *t = clock;
            s
        })
    }

    fn find_sid(&mut self, sid: &[u8; SID_LEN], skip: usize) -> Option<&mut Session> {
        self.slots
            .iter_mut()
            .flatten()
            .filter(|(_, s)| s.has_sid_r && s.sid_r == *sid)
            .nth(skip)
            .map(|(_, s)| s)
    }

    fn insert(&mut self, s: Session) {
        self.clock = self.clock.wrapping_add(1);
        let i = self
            .slots
            .iter()
            .position(|x| x.as_ref().is_some_and(|(_, o)| o.peer == s.peer))
            .or_else(|| self.slots.iter().position(|x| x.is_none()))
            .unwrap_or_else(|| {
                (0..N)
                    .min_by_key(|&i| self.slots[i].as_ref().map_or(0, |(t, _)| *t))
                    .unwrap_or(0)
            });
        if N > 0 {
            self.slots[i] = Some((self.clock, s));
        }
    }
}

#[cfg(test)]
mod tests;
