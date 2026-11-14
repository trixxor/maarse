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
