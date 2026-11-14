//! Lille, afhængighedsfri fuzz-motor: deterministisk PRNG, mutator og en
//! løkke, der kører en målfunktion mod tilfældige og muterede input.
//!
//! Antal iterationer pr. mål styres af `MAARSE_FUZZ_ITERS` (standard
//! 100 000 i release, 5 000 i debug) og frøet af `MAARSE_FUZZ_SEED`. Ved panik skrives frø,
//! iteration og input i hex, så fejlen kan genskabes.
#![forbid(unsafe_code)]

use std::panic::{AssertUnwindSafe, catch_unwind};

/// xorshift64* – hurtig og deterministisk; ikke kryptografisk.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng((seed ^ 0x9e37_79b9_7f4a_7c15) | 1)
    }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next_u64() % n as u64) as usize }
    }
    pub fn byte(&mut self) -> u8 {
        self.next_u64() as u8
    }
    pub fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.byte()).collect()
    }
}

/// Interessante værdier til længde- og tællerfelter.
const MAGIC: [u8; 10] = [0x00, 0x01, 0x02, 0x7f, 0x80, 0xfe, 0xff, 0x0c, 0x0d, 0x40];

/// Muterer `v` med 1-4 tilfældige operationer.
pub fn mutate(rng: &mut Rng, v: &mut Vec<u8>, max_len: usize) {
    for _ in 0..1 + rng.below(4) {
        match rng.below(8) {
            0 if !v.is_empty() => {
                let i = rng.below(v.len());
                v[i] ^= 1 << rng.below(8);
            }
            1 if !v.is_empty() => {
                let i = rng.below(v.len());
                v[i] = rng.byte();
            }
            2 if !v.is_empty() => {
                let i = rng.below(v.len());
                v[i] = MAGIC[rng.below(MAGIC.len())];
            }
            3 => {
                let i = rng.below(v.len() + 1);
                v.insert(i, rng.byte());
            }
            4 if !v.is_empty() => {
                let i = rng.below(v.len());
                v.remove(i);
            }
            5 => {
                let n = rng.below(v.len() + 1);
                v.truncate(n);
            }
            6 if !v.is_empty() => {
                // Gentag et stykke (giver lange/indlejrede strukturer).
                let a = rng.below(v.len());
                let b = a + rng.below(v.len() - a) + 1;
                let piece = v[a..b].to_vec();
                let at = rng.below(v.len() + 1);
                v.splice(at..at, piece);
            }
            _ => {
                let n = rng.below(16);
                v.extend(rng.bytes(n));
            }
        }
    }
    v.truncate(max_len);
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name).ok().and_then(|s| s.parse().ok()).unwrap_or(default)
}

/// Antal iterationer pr. mål (`MAARSE_FUZZ_ITERS`). Standard er 100 000 i
/// `--release` og 5 000 i debug, så `cargo test --workspace` forbliver hurtig.
pub fn iterations() -> u64 {
    env_u64("MAARSE_FUZZ_ITERS", if cfg!(debug_assertions) { 5_000 } else { 100_000 })
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Kører `target` mod `iterations()` input: halvdelen helt tilfældige
/// (længde 0..=`max_len`), halvdelen mutationer af korpus-elementer.
pub fn run(name: &str, corpus: &[Vec<u8>], max_len: usize, mut target: impl FnMut(&[u8])) {
    let seed = env_u64("MAARSE_FUZZ_SEED", 0x6d61_6172_7365);
    let mut rng = Rng::new(seed ^ name.bytes().fold(0u64, |h, b| h.rotate_left(5) ^ b as u64));
    let n = iterations();
    for c in corpus.iter().chain(std::iter::once(&Vec::new())) {
        target(c);
    }
    let start = std::time::Instant::now();
    for i in 0..n {
        let input = if corpus.is_empty() || i % 2 == 0 {
            let len = rng.below(max_len + 1);
            rng.bytes(len)
        } else {
            let mut v = corpus[rng.below(corpus.len())].clone();
            mutate(&mut rng, &mut v, max_len);
            v
        };
        if catch_unwind(AssertUnwindSafe(|| target(&input))).is_err() {
            panic!("fuzz-mål {name}: panik ved frø {seed:#x}, iteration {i}, input {}", hex(&input));
        }
    }
    eprintln!("fuzz {name}: {n} iterationer på {:.1?}", start.elapsed());
}
