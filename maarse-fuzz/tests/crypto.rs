//! Fuzz-mål for maarse-crypto: verificering af announces og åbning af
//! direkte beskeder, kanalbeskeder og filbidder med vilkårligt input.

use maarse_core::Address;
use maarse_crypto::seal::{self, open_channel, open_chunk, open_direct};
use maarse_crypto::{Identity, PublicIdentity, public_channel_key, verify_announce};
use maarse_fuzz::run;

fn ids() -> (Identity, Identity) {
    // Faste nøgler, så kørslen er deterministisk.
    (Identity::from_secret_bytes(&[1; 64]), Identity::from_secret_bytes(&[2; 64]))
}

#[test]
fn fuzz_verify_announce() {
    let (a, _) = ids();
    let corpus = vec![
        a.announce(7, b"").unwrap().to_vec(),
        a.announce(8, b"app-data her").unwrap().to_vec(),
    ];
    run("announce", &corpus, 270, |b| {
        // Både med adressen afledt af input (når nøglerne kan læses) og en fast.
        if let Some(p) = PublicIdentity::from_bytes(b) {
            let _ = verify_announce(&p.address(), b);
        }
        let _ = verify_announce(&a.address(), b);
    });
}

#[test]
fn fuzz_open_direct() {
    let (a, b) = ids();
    let mut rng = rand_core::OsRng;
    let mut corpus = Vec::new();
    for msg in [&b"hej"[..], &[7u8; 100][..]] {
        let mut out = [0u8; 255];
        let n = seal::seal_direct(&a, &b.public(), &mut rng, msg, &mut out).unwrap();
        corpus.push(out[..n].to_vec());
    }
    let a_pub = a.public();
    run("open-direct", &corpus, 270, |p| {
        let lookup = |s: &Address| (*s == a_pub.address()).then_some(a_pub);
        // Både rigelig og for lille udbuffer.
        let mut out = [0u8; 255];
        let _ = open_direct(&b, p, lookup, &mut out);
        let mut small = [0u8; 8];
        let _ = open_direct(&b, p, lookup, &mut small);
        // Opslag, der altid svarer (med en forkert nøgle).
        let _ = open_direct(&b, p, |_| Some(a_pub), &mut out);
    });
}

#[test]
fn fuzz_open_channel() {
    let (a, _) = ids();
    let key = public_channel_key();
    let mut corpus = Vec::new();
    for msg in [&b"hej kanal"[..], &[1u8; 150][..]] {
        let mut out = [0u8; 255];
        let n = seal::seal_channel(&key, &a.address(), [3; 8], msg, &mut out).unwrap();
        corpus.push(out[..n].to_vec());
    }
    run("open-channel", &corpus, 270, |p| {
        let mut out = [0u8; 255];
        if let Ok((_, n)) = open_channel(&key, p, &mut out) {
            assert!(n <= out.len());
        }
        let mut small = [0u8; 20];
        let _ = open_channel(&key, p, &mut small);
    });
}

#[test]
fn fuzz_open_chunk() {
    let key = [7u8; 32];
    let mut corpus = Vec::new();
    for (i, len) in [(0u16, 10usize), (3, seal::CHUNK_DATA)] {
        let mut out = [0u8; 255];
        let n = seal::seal_chunk(&key, 42, i, &vec![0xab; len], &mut out).unwrap();
        corpus.push(out[..n].to_vec());
    }
    run("open-chunk", &corpus, 270, |p| {
        let _ = seal::chunk_header(p);
        let mut out = [0u8; 255];
        let _ = open_chunk(&key, p, &mut out);
        let mut small = [0u8; 4];
        let _ = open_chunk(&key, p, &mut small);
    });
}

#[test]
fn fuzz_open_ratchet() {
    use maarse_crypto::ratchet::{self, FixedStore, Session, SessionStore};
    let (a, b) = ids();
    let mut rng = rand_core::OsRng;
    let a_pub = a.public();
    let b_pub = b.public();
    // Korpus: init, svar (ratchet) og næste tur fra A.
    let mut sa: FixedStore<2> = FixedStore::new();
    let mut sb: FixedStore<2> = FixedStore::new();
    let mut corpus = Vec::new();
    let mut out = [0u8; 255];
    let mut pt = [0u8; 255];
    let n = ratchet::seal(&a, &b_pub, &mut sa, true, &mut rng, b"hej", &mut out).unwrap().0;
    corpus.push(out[..n].to_vec());
    ratchet::open(&b, &out[..n], &mut sb, |_| Some(a_pub), &mut pt).unwrap();
    let n = ratchet::seal(&b, &a_pub, &mut sb, true, &mut rng, &[9u8; 80], &mut out).unwrap().0;
    corpus.push(out[..n].to_vec());
    ratchet::open(&a, &out[..n], &mut sa, |_| Some(b_pub), &mut pt).unwrap();
    let n = ratchet::seal(&a, &b_pub, &mut sa, true, &mut rng, b"igen", &mut out).unwrap().0;
    corpus.push(out[..n].to_vec());
    let b_state = sb.get_mut(&a_pub.address()).unwrap().clone();
    run("open-ratchet", &corpus, 270, |p| {
        // Hver kørsel på en frisk kopi af B's tilstand.
        let mut store: FixedStore<2> = FixedStore::new();
        store.insert(b_state.clone());
        let mut out = [0u8; 255];
        let _ = ratchet::open(&b, p, &mut store, |_| Some(a_pub), &mut out);
        let mut small = [0u8; 8];
        let _ = ratchet::open(&b, p, &mut store, |_| Some(a_pub), &mut small);
        let _ = Session::from_bytes(p);
    });
}

/// Firmware-manifestet læses fra radio/USB, før noget andet tjekkes.
#[test]
fn fuzz_ota_manifest() {
    use maarse_crypto::ota::{self, Manifest, Policy};
    let key = ota::dev_signing_key();
    let corpus = vec![
        Manifest::sign(&key, ota::board::HELTEC_V4, 0x000200, b"firmware").to_bytes().to_vec(),
        Manifest::sign(&key, ota::board::XIAO_NRF52840, 0x010000, &[0; 300]).to_bytes().to_vec(),
    ];
    let policy = Policy {
        public_key: ota::dev_public_key(),
        board: ota::board::HELTEC_V4,
        current_version: 0x000100,
        capacity: 1 << 20,
    };
    run("ota-manifest", &corpus, 130, |b| {
        if let Ok(m) = Manifest::parse(b) {
            assert_eq!(&m.to_bytes()[..], b);
            let _ = m.check(&policy, false);
            let _ = m.check(&policy, true);
        }
    });
}

/// Modtageren styret af vilkårlige bidder (offset, længde) mod flash i RAM.
#[test]
fn fuzz_ota_receiver() {
    use maarse_crypto::ota::{self, BLOCK, Manifest, Policy, Receiver, Staging};
    struct Ram(Vec<u8>, Vec<u8>);
    fn nor(mem: &mut [u8], o: u32, d: &[u8]) -> Result<(), ()> {
        let t = mem.get_mut(o as usize..o as usize + d.len()).ok_or(())?;
        t.iter_mut().zip(d).for_each(|(a, b)| *a &= b);
        Ok(())
    }
    impl Staging for Ram {
        fn erase_block(&mut self, o: u32) -> Result<(), ()> {
            self.0.get_mut(o as usize..(o + BLOCK) as usize).ok_or(())?.fill(0xFF);
            Ok(())
        }
        fn write(&mut self, o: u32, d: &[u8]) -> Result<(), ()> {
            nor(&mut self.0, o, d)
        }
        fn read(&mut self, o: u32, b: &mut [u8]) -> Result<(), ()> {
            b.copy_from_slice(self.0.get(o as usize..o as usize + b.len()).ok_or(())?);
            Ok(())
        }
        fn erase_meta(&mut self) -> Result<(), ()> {
            self.1.fill(0xFF);
            Ok(())
        }
        fn write_meta(&mut self, o: u32, d: &[u8]) -> Result<(), ()> {
            nor(&mut self.1, o, d)
        }
        fn read_meta(&mut self, o: u32, b: &mut [u8]) -> Result<(), ()> {
            b.copy_from_slice(self.1.get(o as usize..o as usize + b.len()).ok_or(())?);
            Ok(())
        }
    }
    let img: Vec<u8> = (0..9000u32).map(|i| (i * 31) as u8).collect();
    let m = Manifest::sign(&ota::dev_signing_key(), 1, 2, &img);
    let policy = Policy { public_key: ota::dev_public_key(), board: 1, current_version: 1, capacity: 16384 };
    // Startet én gang (signaturtjek er langsomt uden optimering), kopieres pr. kørsel.
    let mut st0 = Ram(vec![0xFF; 16384], vec![0xFF; BLOCK as usize]);
    let mut r0 = Receiver::new();
    r0.begin(&mut st0, m, &policy, false).unwrap();
    run("ota-receiver", &[vec![0, 128, 0, 0, 128, 0], vec![1, 2, 3, 7, 5, 6]], 64, |ops| {
        let mut st = Ram(st0.0.clone(), st0.1.clone());
        let mut r = r0.clone();
        for op in ops.chunks(3) {
            // Enten den rigtige næste bid eller et vilkårligt offset.
            let len = *op.get(1).unwrap_or(&0) as usize * 4;
            let off = if op[0] & 1 == 0 {
                r.next
            } else {
                u32::from(op[0]) * 4 + u32::from(*op.get(2).unwrap_or(&0))
            };
            let end = (off as usize + len).min(img.len());
            let data = img.get(off as usize..end).unwrap_or(&[]);
            let _ = r.chunk(&mut st, off, data);
            if op[0] == 7 {
                // "Genstart": en ny modtager genoptager fra metasiden.
                r = Receiver::new();
                r.begin(&mut st, m, &policy, false).unwrap();
            }
        }
        // Bliver imaget færdigt, skal det være korrekt.
        if r.end().is_ok() {
            assert_eq!(&st.0[..img.len()], &img[..]);
        }
    });
}
