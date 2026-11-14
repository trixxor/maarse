//! Forward secrecy for samtaler: en lille "double ratchet" til LoRa.
//!
//! **Udkast - skal gennemgås af en ekstern kryptograf før v1.0.**
//!
//! ## Idé
//!
//! Det gamle format ([`crate::seal`], kaldet *statisk*) krypterer hver besked
//! med modtagerens faste nøgle. Stjæles den nøgle, kan alle gamle beskeder
//! læses. Her bygger to kontakter i stedet en *session* op, hvor nøglerne
//! hele tiden skiftes og de brugte slettes:
//!
//! * **Start (init)**: A laver et frisk ratchet-nøglepar `A1` og regner
//!   `es = DH(A1, B)` og `ss = DH(A, B)` med B's faste nøgle - præcis som det
//!   statiske format. Det giver rodnøgle og første kædenøgle. A kan sende med
//!   det samme (ingen ekstra rundtur, virker også via postkasse). Init-beskeder
//!   er kun lige så sikre som det statiske format.
//! * **Ratchet**: Når B svarer, laver B et frisk nøglepar `B1` og blander
//!   `DH(B1, A1)` ind i rodnøglen. Når A svarer igen, `A2` og `DH(A2, B1)`, osv.
//!   Hver tur sletter den gamle hemmelige nøgle. Efter første svar kan hverken
//!   A's eller B's faste nøgle åbne beskederne, og en lækket sessionstilstand
//!   heler, så snart begge parter har taget en tur mere.
//! * **Hash-kæde**: Inden for en tur afledes en ny beskednøgle pr. besked
//!   (`kæde -> (ny kæde, beskednøgle)`), og den brugte slettes.
//! * **Tab og forkert rækkefølge**: Hver besked har et nummer `n` i sin kæde og
//!   `pn` = antal beskeder i afsenderens forrige kæde. Overspringes beskeder,
//!   gemmes deres nøgler i et lille vindue ([`MAX_SKIP`] pr. session). Kommer
//!   en besked senere end vinduet, kan den ikke længere åbnes (den er tabt).
//!
//! ## Format (69 bytes overhead, 5 mere end det statiske)
//!
//! ```text
//! type (1) | ratchet-nøgle (32) | afsender ⊕ maske (16) | n (u16 LE) | pn (u16 LE)
//!   | ChaCha20-Poly1305(beskednøgle, besked, AAD = de 53 header-bytes) | tag (16)
//!
//! type  0xF0 = init (A har ikke hørt fra B i denne session endnu)
//!       0xF1 = ratchet
//! maske = HKDF(DH(ratchet-nøgle, modtagerens faste nøgle))   (skjuler afsenderen)
//! ```
//!
//! * Den første byte i det statiske format er en tilfældig byte af en
//!   offentlig nøgle. Starter en besked med `0xF0`/`0xF1`, prøves det nye
//!   format først og ellers det gamle ([`open`] klarer begge). Nye noder kan
//!   altså stadig læse gamle beskeder. Gamle noder kan ikke læse de nye, så
//!   en node bruger kun det nye format til kontakter, der melder det i deres
//!   announce ([`APP_FS`]) eller allerede har sendt os en FS-besked.
//! * Afsenderen skjules som i det statiske format. Ratchet-nøglen gentages i
//!   alle beskeder i samme tur, så en lytter kan se, at de hører sammen.
//! * Nøglen er ny for hver besked, så noncen er fast (nul).
//!
//! ## Afledning
//!
//! ```text
//! init:   rk0 | ck0  = HKDF(salt "maarse/fs/v1/init", es || ss, info A1 | R | S)
//! rod:    rk' | ck   = HKDF(salt rk, DH(...), info "maarse/fs/v1/rod")
//! kæde:   ck' = HKDF-Expand(ck, "maarse/fs/v1/kaede"),
//!         mk  = HKDF-Expand(ck, "maarse/fs/v1/besked")
//! maske:  HKDF(salt "maarse/fs/v1/afsender", DH(...), info ratchet-nøgle | R)
//! ```
//!
//! ## Samtidig start
//!
//! Starter begge parter en session, før de har hørt fra hinanden, vinder
//! init fra den laveste adresse. Taberens init-beskeder kan stadig læses.
//!
//! ## Kendte huller
//!
//! * Init-beskeder (før første svar) har ingen forward secrecy mod B's faste
//!   nøgle - som i dag. Der er ingen forud-udgivne nøgler (prekeys).
//! * Mister modtageren sin tilstand (fx firmware genstarter), kan beskeder
//!   sendt i den gamle session ikke læses. Noden svarer så med en ny init
//!   (`Content::Resync`), og samtalen fortsætter derfra.
//! * En genafspillet gammel init kan nulstille en session (kun lammelse,
//!   ikke læsning). Det kan også en forfalsket besked, der udløser en resync.
//! * Ingen header-kryptering: n, pn og ratchet-nøglen er synlige.

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
pub const HEADER_LEN: usize = 1 + 32 + 16 + 2 + 2;
pub const FS_OVERHEAD: usize = HEADER_LEN + 16;
/// Største tekst, der kan sendes i begge formater (én pakke, fuld sti).
pub const MAX_FS_TEXT: usize = maarse_core::router::MAX_DIRECT_PAYLOAD - FS_OVERHEAD;
/// Overspringede beskednøgler, der gemmes pr. session.
pub const MAX_SKIP: usize = 8;
/// Så mange beskeder må en kæde højst springe frem ad gangen (mod at en
/// forfalsket besked får os til at regne længe).
pub const MAX_JUMP: u16 = 512;
/// Announce-felt: "jeg kan forward secrecy (v1)". Ingen data.
pub const APP_FS: u8 = 0x02;
/// Gamle ratchet-nøgler fra modparten, vi kan genkende (så en for sen
/// besked ikke tolkes som en ødelagt session).
const OLD_PUBS: usize = 4;

/// Læser app-data fra et announce: melder noden forward secrecy?
///
/// Kendte felter: `0x01` postkasse (16 bytes), `0x02` FS (ingen data).
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
    /// Det gamle format (ingen forward secrecy).
    Static,
    /// Første beskeder i en session (som statisk, indtil modparten svarer).
    Init,
    /// Fuld forward secrecy.
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

/// Tilstanden for én samtale (én kontakt). Fast størrelse, ingen heap.
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
    /// Vi startede sessionen og har ikke hørt fra modparten endnu.
    pending: bool,
    /// Næste afsendelse skal tage et nyt ratchet-skridt.
    need_step: bool,
    skipped: [Skipped; MAX_SKIP],
    skip_cursor: u8,
    old_pubs: [[u8; 8]; OLD_PUBS],
}

/// Længden af [`Session::to_bytes`].
pub const SESSION_LEN: usize = 1 + 16 + 32 * 2 + 4 + 32 + 2 + 32 * 2 + 16 + 32 * 2 + 1
    + MAX_SKIP * SKIPPED_LEN
    + 1
    + OLD_PUBS * 8;
const SESSION_VERSION: u8 = 1;

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
        // Aldrig nøgler i logs.
        f.debug_struct("Session")
            .field("peer", &self.peer)
            .field("ns", &self.ns)
            .field("nr", &self.nr)
            .field("pending", &self.pending)
            .finish_non_exhaustive()
    }
}

/// Hvor sessioner gemmes (host: HashMap på disk; firmware: fast antal i RAM).
pub trait SessionStore {
    fn get_mut(&mut self, peer: &Address) -> Option<&mut Session>;
    /// Indsætter eller erstatter sessionen for `s.peer()`. Et fuldt lager
    /// må smide en anden session ud (den falder så tilbage til init).
    fn insert(&mut self, s: Session);
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
    let _ = Hkdf::<Sha256>::new(Some(rk), dh_out).expand(b"maarse/fs/v1/rod", &mut okm);
    let r = split64(&okm);
    okm.zeroize();
    r
}

/// (ny kædenøgle, beskednøgle)
fn kdf_ck(ck: &[u8; 32]) -> ([u8; 32], [u8; 32]) {
    // Kan ikke fejle: PRK er 32 bytes, output 32 bytes.
    let hk = Hkdf::<Sha256>::from_prk(ck).expect("32 bytes PRK");
    let mut next = [0u8; 32];
    let mut mk = [0u8; 32];
    let _ = hk.expand(b"maarse/fs/v1/kaede", &mut next);
    let _ = hk.expand(b"maarse/fs/v1/besked", &mut mk);
    (next, mk)
}

fn mask_key(dh_out: &[u8; 32], ratchet_pub: &[u8; 32], recipient: &Address) -> [u8; 16] {
    let mut info = [0u8; 48];
    info[..32].copy_from_slice(ratchet_pub);
    info[32..].copy_from_slice(&recipient.0);
    let mut k = [0u8; 16];
    let _ = Hkdf::<Sha256>::new(Some(b"maarse/fs/v1/afsender"), dh_out).expand(&info, &mut k);
    k
}

fn fp(public: &[u8; 32]) -> [u8; 8] {
    let mut f = [0u8; 8];
    f.copy_from_slice(&public[..8]);
    f
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
        }
    }

    /// Starter en ny session til `peer` (vi er initiativtager).
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

    /// Har modparten svaret i denne session (beskeder har fuld FS)?
    pub fn established(&self) -> bool {
        !self.pending
    }

    /// Krypterer `msg` til `peer` (skal være sessionens kontakt). Returnerer
    /// længden (`msg.len() + FS_OVERHEAD`). Ved fejl er sessionen uændret.
    pub fn seal(
        &mut self,
        me: &Identity,
        peer: &PublicIdentity,
        rng: &mut impl CryptoRngCore,
        msg: &[u8],
        out: &mut [u8],
    ) -> Result<usize, CryptoError> {
        let total = msg.len() + FS_OVERHEAD;
        if out.len() < total || total > maarse_core::router::MAX_DIRECT_PAYLOAD {
            return Err(CryptoError::BufferTooSmall);
        }
        if peer.address() != self.peer || self.ns == u16::MAX {
            return Err(CryptoError::Malformed);
        }
        if self.need_step {
            let mut s = [0u8; 32];
            rng.fill_bytes(&mut s);
            let s_pub = public_of(&s);
            let d = dh(&s, &self.dhr).map_err(|_| CryptoError::Malformed)?;
            let m = dh(&s, &peer.dh).map_err(|_| CryptoError::Malformed)?;
            let (rk, ck) = kdf_rk(&self.rk, &d);
            self.rk = rk;
            self.cks = ck;
            self.has_cks = true;
            self.pn = self.ns;
            self.ns = 0;
            self.dhs = s;
            self.dhs_pub = s_pub;
            self.has_dhs = true;
            self.mask = mask_key(&m, &s_pub, &self.peer);
            self.need_step = false;
            s.zeroize();
        }
        if !self.has_cks {
            return Err(CryptoError::Malformed);
        }
        let (next, mut mk) = kdf_ck(&self.cks);
        let me_addr = me.address();
        out[0] = if self.pending { INIT } else { RATCHET };
        out[1..33].copy_from_slice(&self.dhs_pub);
        for i in 0..16 {
            out[33 + i] = me_addr.0[i] ^ self.mask[i];
        }
        out[49..51].copy_from_slice(&self.ns.to_le_bytes());
        out[51..53].copy_from_slice(&self.pn.to_le_bytes());
        let (head, rest) = out.split_at_mut(HEADER_LEN);
        rest[..msg.len()].copy_from_slice(msg);
        let tag = aead(&mk)
            .encrypt_in_place_detached(Nonce::from_slice(&[0u8; 12]), head, &mut rest[..msg.len()])
            .map_err(|_| CryptoError::Malformed)?;
        rest[msg.len()..msg.len() + 16].copy_from_slice(&tag);
        mk.zeroize();
        self.cks = next;
        self.ns += 1;
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

    /// Går modtagekæden frem til nummer `target` og gemmer de sidste
    /// [`MAX_SKIP`] overspringede nøgler.
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

    /// Dekrypterer én besked. Kaldes på en kopi; kopien gemmes kun ved succes.
    #[allow(clippy::too_many_arguments)]
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
        let head = &payload[..HEADER_LEN];
        let tag = Tag::from_slice(&payload[HEADER_LEN + len..]);
        let f = fp(rpub);
        let mut mk = if let Some(i) = self.skipped.iter().position(|s| s.used && s.fp == f && s.n == n) {
            let mk = self.skipped[i].mk;
            self.skipped[i] = Skipped::default();
            mk
        } else {
            if !(self.has_ckr && *rpub == self.dhr) {
                // Ny ratchet-nøgle fra modparten.
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
                self.dhr = *rpub;
                self.nr = 0;
                self.has_ckr = true;
                self.need_step = true;
            } else if n < self.nr {
                // Allerede brugt (og slettet) eller for gammel.
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
        out[..len].copy_from_slice(&payload[HEADER_LEN..HEADER_LEN + len]);
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

    /// Er `rpub` en af modpartens gamle ratchet-nøgler (for sen besked)?
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
        debug_assert_eq!(w.i, SESSION_LEN);
        b
    }

    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        if b.len() != SESSION_LEN || b[0] != SESSION_VERSION {
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

/// Krypterer en direkte besked. Bruger forward secrecy, hvis `fs_ok`
/// (modparten kan det), og ellers - eller hvis det fejler - det statiske
/// format. Returnerer længde og det brugte format.
pub fn seal(
    me: &Identity,
    peer: &PublicIdentity,
    store: &mut impl SessionStore,
    fs_ok: bool,
    rng: &mut impl CryptoRngCore,
    msg: &[u8],
    out: &mut [u8],
) -> Result<(usize, Format), CryptoError> {
    if fs_ok {
        let addr = peer.address();
        let r = match store.get_mut(&addr) {
            Some(s) => s.seal(me, peer, rng, msg, out).map(|n| (n, s.pending)),
            None => Session::initiate(me, peer, rng).and_then(|mut s| {
                let n = s.seal(me, peer, rng, msg, out)?;
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

/// Starter forfra med `peer`: en ny init-session erstatter den gamle.
pub fn reset(
    me: &Identity,
    peer: &PublicIdentity,
    store: &mut impl SessionStore,
    rng: &mut impl CryptoRngCore,
) -> Result<(), CryptoError> {
    store.insert(Session::initiate(me, peer, rng)?);
    Ok(())
}

/// Dekrypterer en direkte besked i begge formater.
///
/// Fejl ud over [`crate::seal::open_direct`]s:
/// * [`CryptoError::NoSession`]: ratchet-besked fra en kontakt, vi ikke har
///   en session med (fx efter genstart). Svar med en ny init.
/// * [`CryptoError::Desync`]: vi har en session, men beskeden passer ikke
///   (sessionerne er kommet ud af trit - eller beskeden er forfalsket).
pub fn open(
    me: &Identity,
    payload: &[u8],
    store: &mut impl SessionStore,
    lookup: impl Fn(&Address) -> Option<PublicIdentity>,
    out: &mut [u8],
) -> Result<Opened, CryptoError> {
    if payload.len() >= FS_OVERHEAD && matches!(payload[0], INIT | RATCHET) {
        match open_fs(me, payload, store, &lookup, out) {
            Ok(o) => return Ok(o),
            Err(e) => {
                // 1/128 af de gamle beskeder starter tilfældigvis med 0xF0/0xF1.
                return match crate::seal::open_direct(me, payload, &lookup, out) {
                    Ok((from, len)) => Ok(Opened { from, len, format: Format::Static }),
                    Err(_) => Err(e),
                };
            }
        }
    }
    let (from, len) = crate::seal::open_direct(me, payload, lookup, out)?;
    Ok(Opened { from, len, format: Format::Static })
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
        // En init fra den session, vi allerede har: kun kæde eller vindue.
        if let Some(sess) = store.get_mut(&from)
            && sess.init_pub == rpub
        {
            let mut c = sess.clone();
            let n = c.decrypt(&rpub, n, pn, false, payload, out)?;
            *sess = c;
            return Ok(Opened { from, len: n, format: Format::Init });
        }
        let ss = me.dh.diffie_hellman(&XPublic::from(peer.dh));
        let (rk, ck) = kdf_init(es.as_bytes(), ss.as_bytes(), &rpub, &my_addr, &from);
        let mut fresh = Session::blank(from);
        fresh.rk = rk;
        fresh.ckr = ck;
        fresh.has_ckr = true;
        fresh.dhr = rpub;
        fresh.init_pub = rpub;
        fresh.need_step = true;
        let n = fresh.decrypt(&rpub, n, pn, false, payload, out)?;
        let adopt = match store.get_mut(&from) {
            None => true,
            // Begge startede: init fra den laveste adresse vinder.
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

/// Et simpelt lager med plads til `N` sessioner, uden heap (til firmware).
/// Er det fuldt, smides den mindst brugte ud.
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
