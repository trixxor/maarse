//! Fuzz targets for maarse-crypto: verifying announces and opening
//! direct messages, channel messages and file chunks with arbitrary input.

use maarse_core::Address;
use maarse_crypto::seal::{self, open_channel, open_chunk, open_direct};
use maarse_crypto::{Identity, PublicIdentity, public_channel_key, verify_announce};
use maarse_fuzz::run;

fn ids() -> (Identity, Identity) {
    // Fixed keys, so the run is deterministic.
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
        // Both with the address derived from the input (when the keys can be read) and a fixed one.
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
        // Both an ample and a too small output buffer.
        let mut out = [0u8; 255];
        let _ = open_direct(&b, p, lookup, &mut out);
        let mut small = [0u8; 8];
        let _ = open_direct(&b, p, lookup, &mut small);
        // A lookup that always answers (with a wrong key).
        let _ = open_direct(&b, p, |_| Some(a_pub), &mut out);
    });
}

#[test]
fn fuzz_open_channel() {
    let (a, _) = ids();
    let key = public_channel_key();
    let mut corpus = Vec::new();
    for msg in [&b"hi channel"[..], &[1u8; 150][..]] {
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
    // Corpus: init, reply (ratchet) and the next turn from A.
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
    let n = ratchet::seal(&a, &b_pub, &mut sa, true, &mut rng, b"again", &mut out).unwrap().0;
    corpus.push(out[..n].to_vec());
    let b_state = sb.get_mut(&a_pub.address()).unwrap().clone();
    run("open-ratchet", &corpus, 270, |p| {
        // Every run on a fresh copy of B's state.
        let mut store: FixedStore<2> = FixedStore::new();
        store.insert(b_state.clone());
        let mut out = [0u8; 255];
        let _ = ratchet::open(&b, p, &mut store, |_| Some(a_pub), &mut out);
        let mut small = [0u8; 8];
        let _ = ratchet::open(&b, p, &mut store, |_| Some(a_pub), &mut small);
        let _ = Session::from_bytes(p);
    });
}

/// Compact messages (`0xF2`): the parser and the session-id lookup must never
/// panic on any input, and no mutated message may open.
#[test]
fn fuzz_open_compact() {
    use maarse_crypto::ratchet::{self, FixedStore, SessionStore};
    let (a, b) = ids();
    let mut rng = rand_core::OsRng;
    let a_pub = a.public();
    let b_pub = b.public();
    let mut sa: FixedStore<2> = FixedStore::new();
    let mut sb: FixedStore<2> = FixedStore::new();
    let mut out = [0u8; 255];
    let mut pt = [0u8; 255];
    // Build a session where A sends compact messages.
    let mut corpus = Vec::new();
    let n = ratchet::seal_compact(&a, &b_pub, &mut sa, true, true, &mut rng, b"one", &mut out).unwrap().0;
    ratchet::open(&b, &out[..n], &mut sb, |_| Some(a_pub), &mut pt).unwrap();
    let n = ratchet::seal_compact(&b, &a_pub, &mut sb, true, true, &mut rng, b"two", &mut out).unwrap().0;
    ratchet::open(&a, &out[..n], &mut sa, |_| Some(b_pub), &mut pt).unwrap();
    for i in 0..6u8 {
        let n = ratchet::seal_compact(&a, &b_pub, &mut sa, true, true, &mut rng, &[i; 30], &mut out).unwrap().0;
        corpus.push(out[..n].to_vec());
        if i < 5 {
            ratchet::open(&b, &out[..n], &mut sb, |_| Some(a_pub), &mut pt).unwrap();
            // Acknowledged: from here the short form is used.
            let s = sa.get_mut(&b_pub.address()).unwrap();
            let chain = s.send_position().unwrap().0;
            s.confirm(chain);
        }
    }
    // The last one is compact and not yet read: it is the genuine message.
    let genuine = corpus.last().unwrap().clone();
    assert_eq!(genuine[0], ratchet::COMPACT);
    let b_state = sb.get_mut(&a_pub.address()).unwrap().clone();
    run("open-compact", &corpus, 270, |p| {
        let mut store: FixedStore<2> = FixedStore::new();
        store.insert(b_state.clone());
        let mut out = [0u8; 255];
        let r = ratchet::open(&b, p, &mut store, |_| Some(a_pub), &mut out);
        if p != &genuine[..] {
            if let Ok(o) = r {
                // Only a valid ciphertext may open; a changed message must not
                // open as the genuine one.
                assert!(p[0] != ratchet::COMPACT || o.len != 30 || out[..30] != [5u8; 30], "a changed compact message opened");
            }
        }
        let mut small = [0u8; 8];
        let _ = ratchet::open(&b, p, &mut store, |_| Some(a_pub), &mut small);
    });
}

/// The firmware manifest is read from radio/USB before anything else is checked.
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

/// The receiver driven by arbitrary chunks (offset, length) against flash in RAM.
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
    // Started once (the signature check is slow without optimisation), copied per run.
    let mut st0 = Ram(vec![0xFF; 16384], vec![0xFF; BLOCK as usize]);
    let mut r0 = Receiver::new();
    r0.begin(&mut st0, m, &policy, false).unwrap();
    run("ota-receiver", &[vec![0, 128, 0, 0, 128, 0], vec![1, 2, 3, 7, 5, 6]], 64, |ops| {
        let mut st = Ram(st0.0.clone(), st0.1.clone());
        let mut r = r0.clone();
        for op in ops.chunks(3) {
            // Either the right next chunk or an arbitrary offset.
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
                // "Restart": a new receiver resumes from the meta page.
                r = Receiver::new();
                r.begin(&mut st, m, &policy, false).unwrap();
            }
        }
        // If the image is finished, it must be correct.
        if r.end().is_ok() {
            assert_eq!(&st.0[..img.len()], &img[..]);
        }
    });
}
