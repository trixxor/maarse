use super::*;
use crate::seal::{self as st, CryptoError};
use rand_core::{CryptoRng, OsRng, RngCore};
use sha2::Digest;
use std::collections::HashMap;

#[derive(Default, Clone)]
struct Map(HashMap<Address, Session>);
impl SessionStore for Map {
    fn get_mut(&mut self, peer: &Address) -> Option<&mut Session> {
        self.0.get_mut(peer)
    }
    fn insert(&mut self, s: Session) {
        self.0.insert(s.peer(), s);
    }
}

/// Deterministisk "rng" til testvektorer (SHA-256 over en tæller).
struct TestRng(u64);
impl RngCore for TestRng {
    fn next_u32(&mut self) -> u32 {
        self.next_u64() as u32
    }
    fn next_u64(&mut self) -> u64 {
        let mut b = [0u8; 8];
        self.fill_bytes(&mut b);
        u64::from_le_bytes(b)
    }
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        for c in dest.chunks_mut(32) {
            let d = Sha256::digest(self.0.to_le_bytes());
            self.0 += 1;
            c.copy_from_slice(&d[..c.len()]);
        }
    }
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand_core::Error> {
        self.fill_bytes(dest);
        Ok(())
    }
}
impl CryptoRng for TestRng {}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// En part i samtalen.
struct Peer {
    id: Identity,
    store: Map,
    book: HashMap<Address, PublicIdentity>,
}

impl Peer {
    fn new() -> Self {
        Peer { id: Identity::generate(&mut OsRng), store: Map::default(), book: HashMap::new() }
    }
    fn addr(&self) -> Address {
        self.id.address()
    }
    fn knows(&mut self, other: &Peer) {
        self.book.insert(other.addr(), other.id.public());
    }
    fn send(&mut self, to: &Peer, msg: &[u8]) -> (Vec<u8>, Format) {
        let mut out = [0u8; 255];
        let (n, f) =
            seal(&self.id, &to.id.public(), &mut self.store, true, &mut OsRng, msg, &mut out).unwrap();
        (out[..n].to_vec(), f)
    }
    fn recv(&mut self, ct: &[u8]) -> Result<(Vec<u8>, Format), CryptoError> {
        let mut out = [0u8; 255];
        let book = &self.book;
        let o = open(&self.id, ct, &mut self.store, |a| book.get(a).copied(), &mut out)?;
        Ok((out[..o.len].to_vec(), o.format))
    }
    fn session(&self, peer: &Peer) -> Session {
        self.store.0.get(&peer.addr()).unwrap().clone()
    }
}

fn pair() -> (Peer, Peer) {
    let mut a = Peer::new();
    let mut b = Peer::new();
    a.knows(&b);
    b.knows(&a);
    (a, b)
}

/// A starter, B svarer: nu er sessionen etableret begge veje.
fn established() -> (Peer, Peer) {
    let (mut a, mut b) = pair();
    let (c, f) = a.send(&b, b"hej");
    assert_eq!(f, Format::Init);
    assert_eq!(b.recv(&c).unwrap(), (b"hej".to_vec(), Format::Init));
    let (c, f) = b.send(&a, b"hej selv");
    assert_eq!(f, Format::Ratchet);
    assert_eq!(a.recv(&c).unwrap(), (b"hej selv".to_vec(), Format::Ratchet));
    (a, b)
}

#[test]
fn session_setup_and_many_messages_both_ways() {
    let (mut a, mut b) = pair();
    // Flere init-beskeder før svar (fx via postkasse).
    for i in 0..3u8 {
        let (c, f) = a.send(&b, &[i]);
        assert_eq!((c[0], f), (INIT, Format::Init));
        assert_eq!(c.len(), 1 + FS_OVERHEAD);
        assert_eq!(b.recv(&c).unwrap(), (vec![i], Format::Init));
    }
    assert!(!a.session(&b).established());
    let mut turns = 0;
    for round in 0..40u32 {
        // Skiftende antal beskeder pr. tur.
        let (from, to) = if round % 2 == 0 { (&mut b, &mut a) } else { (&mut a, &mut b) };
        for k in 0..(round % 5 + 1) {
            let msg = format!("runde {round} besked {k}");
            let (c, f) = from.send(to, msg.as_bytes());
            assert_eq!((c[0], f), (RATCHET, Format::Ratchet));
            assert_eq!(to.recv(&c).unwrap(), (msg.into_bytes(), Format::Ratchet));
        }
        turns += 1;
    }
    assert_eq!(turns, 40);
    assert!(a.session(&b).established() && b.session(&a).established());
}

#[test]
fn ratchet_key_changes_every_turn() {
    let (mut a, mut b) = established();
    let mut seen = std::collections::HashSet::new();
    for _ in 0..10 {
        let (c1, _) = a.send(&b, b"x");
        let (c2, _) = a.send(&b, b"y");
        // Samme tur: samme ratchet-nøgle, nye numre.
        assert_eq!(c1[1..33], c2[1..33]);
        assert_eq!(u16::from_le_bytes([c2[49], c2[50]]), 1);
        assert!(seen.insert(c1[1..33].to_vec()), "ratchet-nøglen skal være ny hver tur");
        b.recv(&c1).unwrap();
        b.recv(&c2).unwrap();
        let (c3, _) = b.send(&a, b"z");
        assert!(seen.insert(c3[1..33].to_vec()));
        a.recv(&c3).unwrap();
    }
}

#[test]
fn out_of_order_within_window() {
    let (mut a, mut b) = established();
    let msgs: Vec<_> = (0..MAX_SKIP as u8 + 1).map(|i| a.send(&b, &[i]).0).collect();
    // Omvendt rækkefølge: den sidste først, så de 8 andre fra vinduet.
    for (i, c) in msgs.iter().enumerate().rev() {
        assert_eq!(b.recv(c).unwrap().0, vec![i as u8]);
    }
    // Hen over et ratchet-skridt: gamle beskeder fra forrige kæde (via pn).
    let (old1, _) = a.send(&b, b"gammel 1");
    let (old2, _) = a.send(&b, b"gammel 2");
    let (r, _) = b.send(&a, b"svar"); // B har ikke set old1/old2 endnu
    a.recv(&r).unwrap();
    let (new, _) = a.send(&b, b"ny kaede");
    assert_eq!(b.recv(&new).unwrap().0, b"ny kaede");
    assert_eq!(b.recv(&old2).unwrap().0, b"gammel 2");
    assert_eq!(b.recv(&old1).unwrap().0, b"gammel 1");
}

#[test]
fn late_init_messages_after_ratchet() {
    let (mut a, mut b) = pair();
    let (i0, _) = a.send(&b, b"i0");
    let (i1, _) = a.send(&b, b"i1");
    let (i2, _) = a.send(&b, b"i2");
    b.recv(&i0).unwrap();
    let (r, _) = b.send(&a, b"r");
    a.recv(&r).unwrap();
    let (m, _) = a.send(&b, b"m");
    b.recv(&m).unwrap();
    // i1 og i2 kommer for sent (fx fra en postkasse) - de kan stadig læses.
    assert_eq!(b.recv(&i2).unwrap(), (b"i2".to_vec(), Format::Init));
    assert_eq!(b.recv(&i1).unwrap(), (b"i1".to_vec(), Format::Init));
    // ...men ikke to gange.
    assert!(b.recv(&i1).is_err());
    // Og sessionen er ikke blevet nulstillet.
    let (m2, f) = a.send(&b, b"m2");
    assert_eq!(f, Format::Ratchet);
    assert_eq!(b.recv(&m2).unwrap().0, b"m2");
}

#[test]
fn loss_beyond_window() {
    let (mut a, mut b) = established();
    let msgs: Vec<_> = (0..20u8).map(|i| a.send(&b, &[i]).0).collect();
    assert_eq!(b.recv(&msgs[19]).unwrap().0, vec![19]);
    // De 8 nærmeste kan læses, ældre er tabt (nøglerne er væk).
    for i in (0..19).rev() {
        let r = b.recv(&msgs[i]);
        if i >= 19 - MAX_SKIP {
            assert_eq!(r.unwrap().0, vec![i as u8]);
        } else {
            assert!(r.is_err(), "besked {i} burde være uden for vinduet");
        }
    }
    // Samtalen fortsætter.
    let (c, _) = a.send(&b, b"videre");
    assert_eq!(b.recv(&c).unwrap().0, b"videre");
    let (c, _) = b.send(&a, b"ja");
    assert_eq!(a.recv(&c).unwrap().0, b"ja");
}

#[test]
fn too_large_jump_is_rejected_without_state_change() {
    let (mut a, mut b) = established();
    let mut msgs = Vec::new();
    for _ in 0..=MAX_JUMP as usize + 1 {
        msgs.push(a.send(&b, b"x").0);
    }
    let before = b.session(&a).to_bytes();
    assert!(b.recv(msgs.last().unwrap()).is_err());
    assert_eq!(b.session(&a).to_bytes(), before);
    assert!(b.recv(&msgs[0]).is_ok());
}

#[test]
fn used_keys_are_deleted() {
    let (mut a, mut b) = established();
    let (c1, _) = a.send(&b, b"en");
    let (c2, _) = a.send(&b, b"to");
    b.recv(&c2).unwrap();
    b.recv(&c1).unwrap(); // fra vinduet
    // Genafspilning virker ikke: nøglerne er slettet, både fra kæden og vinduet.
    assert!(b.recv(&c1).is_err());
    assert!(b.recv(&c2).is_err());
    // Selv en kopi af hele B's tilstand nu kan ikke genskabe dem.
    let snapshot = b.store.clone();
    let mut thief = Peer { id: Identity::from_secret_bytes(&b.id.to_secret_bytes()), store: snapshot, book: b.book.clone() };
    assert!(thief.recv(&c1).is_err());
    assert!(thief.recv(&c2).is_err());
    // Vinduet er tomt igen.
    assert!(b.session(&a).skipped.iter().all(|s| !s.used));
}

#[test]
fn dh_step_gives_forward_secrecy_against_static_keys() {
    let (mut a, mut b) = pair();
    let (init, _) = a.send(&b, b"init");
    b.recv(&init).unwrap();
    let (r, _) = b.send(&a, b"svar");
    a.recv(&r).unwrap();
    let (m, f) = a.send(&b, b"hemmelig");
    assert_eq!(f, Format::Ratchet);
    assert_ne!(m[1..33], init[1..33], "A har taget et nyt ratchet-skridt");
    b.recv(&m).unwrap();

    // En tyv med begge faste nøgler (men ingen sessionstilstand):
    let mut thief_b = Peer {
        id: Identity::from_secret_bytes(&b.id.to_secret_bytes()),
        store: Map::default(),
        book: b.book.clone(),
    };
    // Init-beskeden kan læses (kendt hul, som det statiske format)...
    assert_eq!(thief_b.recv(&init).unwrap().0, b"init");
    // ...men ikke beskeder efter første svar.
    assert!(thief_b.recv(&m).is_err());
    let mut thief_a = Peer {
        id: Identity::from_secret_bytes(&a.id.to_secret_bytes()),
        store: Map::default(),
        book: a.book.clone(),
    };
    assert!(thief_a.recv(&r).is_err());
}

#[test]
fn compromise_then_heal() {
    let (mut a, mut b) = established();
    let (before, _) = a.send(&b, b"foer");
    b.recv(&before).unwrap();
    // Angriberen kopierer hele B's tilstand (og faste nøgle) her.
    let mut eve = Peer {
        id: Identity::from_secret_bytes(&b.id.to_secret_bytes()),
        store: b.store.clone(),
        book: b.book.clone(),
    };
    // Tidligere beskeder kan ikke læses med den stjålne tilstand.
    assert!(eve.recv(&before).is_err());
    // Resten af den igangværende tur kan (forventet).
    let (same_turn, _) = a.send(&b, b"samme tur");
    b.recv(&same_turn).unwrap();
    assert!(eve.recv(&same_turn).is_ok());
    // B svarer med en frisk nøgle (Eve ser kun det offentlige)...
    let (r, _) = b.send(&a, b"svar");
    a.recv(&r).unwrap();
    // ...og A's næste tur bruger DH med B's nye nøgle: Eve er ude.
    let (after, _) = a.send(&b, b"efter");
    assert_eq!(b.recv(&after).unwrap().0, b"efter");
    assert!(eve.recv(&after).is_err());
    for i in 0..5u8 {
        let (c, _) = b.send(&a, &[i]);
        a.recv(&c).unwrap();
        let (c, _) = a.send(&b, &[i]);
        b.recv(&c).unwrap();
        assert!(eve.recv(&c).is_err());
    }
}

#[test]
fn static_fallback_and_compatibility() {
    let (mut a, mut b) = pair();
    // Modparten kan ikke FS: statisk format.
    let mut out = [0u8; 255];
    let (n, f) = seal(&a.id, &b.id.public(), &mut a.store, false, &mut OsRng, b"gammel", &mut out).unwrap();
    assert_eq!((f, n), (Format::Static, 6 + st::DM_OVERHEAD));
    assert!(a.store.0.is_empty());
    assert_eq!(b.recv(&out[..n]).unwrap(), (b"gammel".to_vec(), Format::Static));
    // En besked fra en gammel node (seal_direct) kan læses af den nye kode.
    let n = st::seal_direct(&a.id, &b.id.public(), &mut OsRng, b"v0", &mut out).unwrap();
    assert_eq!(b.recv(&out[..n]).unwrap(), (b"v0".to_vec(), Format::Static));
    // En gammel node kan ikke læse FS-beskeder (accepteret; den melder ikke APP_FS).
    let (c, _) = a.send(&b, b"ny");
    let a_pub = a.id.public();
    assert!(st::open_direct(&b.id, &c, |x| (*x == a_pub.address()).then_some(a_pub), &mut out).is_err());
}

#[test]
fn old_format_starting_with_marker_byte_still_opens() {
    let (a, mut b) = pair();
    let mut out = [0u8; 255];
    let mut hits = 0;
    for _ in 0..2000 {
        let n = st::seal_direct(&a.id, &b.id.public(), &mut OsRng, b"tilfaeldig", &mut out).unwrap();
        if matches!(out[0], INIT | RATCHET) {
            hits += 1;
            assert_eq!(b.recv(&out[..n]).unwrap(), (b"tilfaeldig".to_vec(), Format::Static));
        }
    }
    assert!(hits > 0);
}

#[test]
fn too_long_for_fs_falls_back_to_static() {
    let (mut a, mut b) = pair();
    let mut out = [0u8; 255];
    let (n, f) = seal(&a.id, &b.id.public(), &mut a.store, true, &mut OsRng, &[b'x'; MAX_FS_TEXT], &mut out).unwrap();
    assert_eq!((n, f), (maarse_core::router::MAX_DIRECT_PAYLOAD, Format::Init));
    b.recv(&out[..n]).unwrap();
    let before = a.session(&b).to_bytes();
    let (n, f) = seal(&a.id, &b.id.public(), &mut a.store, true, &mut OsRng, &[b'x'; MAX_FS_TEXT + 1], &mut out).unwrap();
    assert_eq!(f, Format::Static);
    assert_eq!(a.session(&b).to_bytes(), before, "mislykket FS må ikke ændre sessionen");
    assert_eq!(b.recv(&out[..n]).unwrap().1, Format::Static);
}

#[test]
fn tampering_fails_and_leaves_state_untouched() {
    let (mut a, mut b) = established();
    let (c, _) = a.send(&b, b"hemmelig");
    let before = b.session(&a).to_bytes();
    for i in [0, 1, 20, 33, 40, 49, 51, HEADER_LEN, c.len() - 1] {
        let mut bad = c.clone();
        bad[i] ^= 1;
        assert!(b.recv(&bad).is_err(), "byte {i}");
        assert_eq!(b.session(&a).to_bytes(), before, "byte {i}");
    }
    assert_eq!(b.recv(&c).unwrap().0, b"hemmelig");
}

#[test]
fn forged_sender_cannot_inject() {
    // C kender A's og B's offentlige nøgler og påstår at være A.
    let (mut a, mut b) = established();
    let mut c = Peer::new();
    c.knows(&b);
    let (fake_init, _) = c.send(&b, b"jeg er A");
    // Ombyt den skjulte afsender til A's adresse.
    let mut forged = fake_init.clone();
    let rpub: [u8; 32] = forged[1..33].try_into().unwrap();
    let es = b.id.dh.diffie_hellman(&XPublic::from(rpub));
    let mask = mask_key(es.as_bytes(), &rpub, &b.addr());
    for i in 0..16 {
        forged[33 + i] = a.addr().0[i] ^ mask[i];
    }
    let before = b.session(&a).to_bytes();
    assert!(b.recv(&forged).is_err());
    forged[0] = RATCHET;
    assert!(matches!(b.recv(&forged), Err(CryptoError::Desync(x)) if x == a.addr()));
    assert_eq!(b.session(&a).to_bytes(), before);
    let (m, _) = a.send(&b, "ægte".as_bytes());
    assert_eq!(b.recv(&m).unwrap().0, "ægte".as_bytes());
}

#[test]
fn simultaneous_start() {
    let (mut a, mut b) = pair();
    let (ca, _) = a.send(&b, b"fra A");
    let (cb, _) = b.send(&a, b"fra B");
    // Begge kan læse den andens init.
    assert_eq!(b.recv(&ca).unwrap().0, b"fra A");
    assert_eq!(a.recv(&cb).unwrap().0, b"fra B");
    // Begge er enige om én session og kan fortsætte.
    for i in 0..4u8 {
        let (c, _) = a.send(&b, &[i]);
        assert_eq!(b.recv(&c).unwrap().0, vec![i]);
        let (c, _) = b.send(&a, &[i]);
        assert_eq!(a.recv(&c).unwrap().0, vec![i]);
    }
    assert_eq!(a.send(&b, b"z").1, Format::Ratchet);
}

#[test]
fn lost_state_resyncs() {
    let (mut a, mut b) = established();
    // B genstarter og mister alt.
    b.store = Map::default();
    let (lost, _) = a.send(&b, b"tabt");
    assert!(matches!(b.recv(&lost), Err(CryptoError::NoSession(x)) if x == a.addr()));
    // B svarer med en ny init (Content::Resync i noderne).
    reset(&b.id, &a.id.public(), &mut b.store, &mut OsRng).unwrap();
    let (resync, f) = b.send(&a, b"");
    assert_eq!(f, Format::Init);
    assert_eq!(a.recv(&resync).unwrap().1, Format::Init);
    // A har overtaget B's nye session.
    let (c, f) = a.send(&b, b"igen");
    assert_eq!(f, Format::Ratchet);
    assert_eq!(b.recv(&c).unwrap().0, b"igen");
    let (c, _) = b.send(&a, b"ok");
    assert_eq!(a.recv(&c).unwrap(), (b"ok".to_vec(), Format::Ratchet));
}

#[test]
fn late_message_from_old_chain_is_not_desync() {
    let (mut a, mut b) = established();
    let msgs: Vec<_> = (0..12u8).map(|i| a.send(&b, &[i]).0).collect();
    b.recv(&msgs[11]).unwrap();
    let (r, _) = b.send(&a, b"r");
    a.recv(&r).unwrap();
    let (m, _) = a.send(&b, b"m");
    b.recv(&m).unwrap();
    // Uden for vinduet og fra en gammel kæde: almindelig fejl, ikke Desync.
    assert_eq!(b.recv(&msgs[0]), Err(CryptoError::Decrypt));
}

#[test]
fn session_survives_serialization() {
    let (mut a, mut b) = established();
    let (c1, _) = a.send(&b, b"1");
    let (c2, _) = a.send(&b, b"2");
    b.recv(&c2).unwrap();
    // Gem og hent begge sider (som hosten gør på disk).
    for p in [&mut a, &mut b] {
        let bytes: Vec<_> = p.store.0.values().map(|s| s.to_bytes()).collect();
        p.store = Map::default();
        for x in bytes {
            assert_eq!(x.len(), SESSION_LEN);
            p.store.insert(Session::from_bytes(&x).unwrap());
        }
    }
    assert_eq!(b.recv(&c1).unwrap().0, b"1");
    let (c, _) = b.send(&a, b"3");
    assert_eq!(a.recv(&c).unwrap().0, b"3");
    assert!(Session::from_bytes(&[0u8; SESSION_LEN]).is_none());
    assert!(Session::from_bytes(&[1u8; 10]).is_none());
}

#[test]
fn fixed_store_evicts_least_recently_used() {
    let me = Identity::generate(&mut OsRng);
    let ids: Vec<_> = (0..4).map(|_| Identity::generate(&mut OsRng).public()).collect();
    let mut st: FixedStore<3> = FixedStore::new();
    for p in &ids[..3] {
        st.insert(Session::initiate(&me, p, &mut OsRng).unwrap());
    }
    assert!(st.get_mut(&ids[0].address()).is_some()); // 0 er nu nyligt brugt
    st.insert(Session::initiate(&me, &ids[3], &mut OsRng).unwrap());
    assert_eq!(st.len(), 3);
    assert!(st.contains(&ids[0].address()));
    assert!(!st.contains(&ids[1].address()));
    assert!(st.contains(&ids[3].address()));
    st.remove(&ids[0].address());
    assert!(!st.contains(&ids[0].address()));
}

#[test]
fn announce_flag() {
    assert!(announces_fs(&[APP_FS]));
    let mut pb = vec![0x01];
    pb.extend_from_slice(&[0x02; 16]); // postkasse-adresse med 0x02-bytes
    assert!(!announces_fs(&pb));
    pb.push(APP_FS);
    assert!(announces_fs(&pb));
    assert!(!announces_fs(&[]));
    assert!(!announces_fs(&[0x7f, APP_FS]));
}

/// Testvektorer for afledningerne og et helt deterministisk forløb.
#[test]
fn vectors() {
    let (ck, mk) = kdf_ck(&[1u8; 32]);
    assert_eq!(hex(&ck), VEC_CK);
    assert_eq!(hex(&mk), VEC_MK);
    let (rk, ck) = kdf_rk(&[2u8; 32], &[3u8; 32]);
    assert_eq!(hex(&rk), VEC_RK);
    assert_eq!(hex(&ck), VEC_RK_CK);

    let a = Identity::from_secret_bytes(&[1u8; 64]);
    let b = Identity::from_secret_bytes(&[2u8; 64]);
    let mut rng = TestRng(0);
    let mut sa = Map::default();
    let mut out = [0u8; 255];
    let (n, f) = seal(&a, &b.public(), &mut sa, true, &mut rng, b"hej", &mut out).unwrap();
    assert_eq!(f, Format::Init);
    assert_eq!(hex(&out[..n]), VEC_FIRST);
    let mut sb = Map::default();
    let a_pub = a.public();
    let mut pt = [0u8; 255];
    let o = open(&b, &out[..n], &mut sb, |x| (*x == a_pub.address()).then_some(a_pub), &mut pt).unwrap();
    assert_eq!(&pt[..o.len], b"hej");
    let (n, f) = seal(&b, &a_pub, &mut sb, true, &mut rng, b"svar", &mut out).unwrap();
    assert_eq!(f, Format::Ratchet);
    assert_eq!(hex(&out[..n]), VEC_REPLY);
}

const VEC_CK: &str = "1d2a0c4600e74fa6d82220f51a3a3cafc9e7142293a07589ccb3cb6ad386865a";
const VEC_MK: &str = "c09ff15bb7d8b4575a1d4b802d118d22dd36907ebe099e110e42bf3400817cc9";
const VEC_RK: &str = "24b0c25edab84bbf706e478596a55e7fb0a17bf34f3a340ccd65c69226d67334";
const VEC_RK_CK: &str = "1f811c59db8712d024bcaac9d50565d701007be68c4480c93cc8808d115ed28c";
const VEC_FIRST: &str = "f027b284f2a3ac7edc0860504d85386d5d9e32fbfd2423af930e47466bf9c4df4b69f37484d4ddbac46388ce9cb54818700000000027ba4a5de4c227fb2827434420850e6ea7e3a6";
const VEC_REPLY: &str = "f18d06fac5c8eafee9f706edd6211ad5de9e3794b795966ae5b9a22efd8871c8477c5bf4bad2678ff039fbabfe57524c3f00000000031072a316f3c5a843ab5471eae08b7fca905450";
