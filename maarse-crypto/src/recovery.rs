//! Gendannelsesfrase (24 ord, BIP39) og afledning af master-identiteten.
//!
//! ```text
//! entropi (32 bytes, tilfældig)
//!   -> 24 ord fra BIP39's engelske ordliste (256 bit + 8 bit tjeksum)
//!   -> seed = PBKDF2-HMAC-SHA512(frase, "mnemonic" + adgangsord, 2048)  (64 bytes, som BIP39)
//!   -> master = HKDF-SHA256(salt = "maarse/master/v1", ikm = seed, info = "")  (64 bytes)
//!   -> Identity::from_secret_bytes(master)  (Ed25519-frø | X25519-nøgle)
//! ```
//!
//! **Overgangsregel:** indtil videre ER nodens identitet master-identiteten,
//! så frasen genskaber samme adresse og sikkerhedsnummer. Senere (tilknyttede
//! enheder) bliver master kun brugt til at underskrive hver enheds egen nøgle;
//! samme afledning bevares, så nutidens fraser stadig gælder.
//!
//! Frasen skal være i BIP39's kanoniske form: små bogstaver, ét mellemrum
//! mellem ordene. Adgangsordet (valgfrit) skal være NFKD-normaliseret, som
//! BIP39 kræver; ren ASCII er altid i orden. Webklienten normaliserer selv.

use hmac::{Hmac, Mac};
use sha2::{Sha256, Sha512};

use crate::Identity;

/// BIP39's officielle engelske ordliste (2048 ord, ét pr. linje).
/// SHA-256: 2f5eed53a4727b4bf8880d8f3f199efc90e58503646d9ff8eff3a2ed3b24dbda.
pub const WORDLIST: &str = include_str!("bip39_english.txt");
pub const WORDS: usize = 24;
pub const ENTROPY_LEN: usize = 32;
/// Salt til HKDF fra BIP39-seed til master-hemmeligheden.
pub const MASTER_SALT: &[u8] = b"maarse/master/v1";
/// Plads til en kanonisk frase (højst 8 bogstaver pr. ord).
pub type Phrase = heapless::String<{ WORDS * 9 }>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhraseError {
    /// Ikke præcis 24 ord.
    WordCount,
    /// Ord nummer `n` (fra 0) findes ikke i ordlisten.
    UnknownWord(usize),
    /// Tjeksummen passer ikke (skrivefejl eller ombyttede ord).
    Checksum,
}

/// Ord nummer `i` (0..2048).
pub fn word(i: u16) -> &'static str {
    WORDLIST.lines().nth(i as usize).unwrap_or("")
}

/// Ordets nummer. Ordlisten er sorteret, men vi søger lineært — 2048
/// sammenligninger er ingenting, og det virker uden ekstra tabel i flash.
pub fn word_index(w: &str) -> Option<u16> {
    WORDLIST.lines().position(|x| x == w).map(|i| i as u16)
}

/// 24 ordnumre for 32 bytes entropi (sidste ord bærer 8 bit tjeksum).
pub fn entropy_to_indices(entropy: &[u8; ENTROPY_LEN]) -> [u16; WORDS] {
    let mut bits = [0u8; ENTROPY_LEN + 1];
    bits[..ENTROPY_LEN].copy_from_slice(entropy);
    bits[ENTROPY_LEN] = sha256(entropy)[0];
    let mut out = [0u16; WORDS];
    for (i, o) in out.iter_mut().enumerate() {
        let mut v = 0u16;
        for b in 0..11 {
            let pos = i * 11 + b;
            let bit = (bits[pos / 8] >> (7 - pos % 8)) & 1;
            v = (v << 1) | bit as u16;
        }
        *o = v;
    }
    out
}

/// Den kanoniske frase for 32 bytes entropi.
pub fn entropy_to_phrase(entropy: &[u8; ENTROPY_LEN]) -> Phrase {
    let mut s = Phrase::new();
    for (i, idx) in entropy_to_indices(entropy).iter().enumerate() {
        if i > 0 {
            let _ = s.push(' ');
        }
        let _ = s.push_str(word(*idx));
    }
    s
}

/// Tjekker en frase (ord adskilt af mellemrum/linjeskift; store bogstaver
/// tilladt) og returnerer entropien.
pub fn phrase_to_entropy(phrase: &str) -> Result<[u8; ENTROPY_LEN], PhraseError> {
    let mut bits = [0u8; ENTROPY_LEN + 1];
    let mut n = 0usize;
    for w in phrase.split_whitespace() {
        if n >= WORDS {
            return Err(PhraseError::WordCount);
        }
        let mut lower = heapless::String::<16>::new();
        for c in w.chars() {
            if lower.push(c.to_ascii_lowercase()).is_err() {
                return Err(PhraseError::UnknownWord(n));
            }
        }
        let idx = word_index(&lower).ok_or(PhraseError::UnknownWord(n))?;
        for b in 0..11 {
            if (idx >> (10 - b)) & 1 == 1 {
                let pos = n * 11 + b;
                bits[pos / 8] |= 1 << (7 - pos % 8);
            }
        }
        n += 1;
    }
    if n != WORDS {
        return Err(PhraseError::WordCount);
    }
    let mut e = [0u8; ENTROPY_LEN];
    e.copy_from_slice(&bits[..ENTROPY_LEN]);
    if sha256(&e)[0] != bits[ENTROPY_LEN] {
        return Err(PhraseError::Checksum);
    }
    Ok(e)
}

/// BIP39-seed: PBKDF2-HMAC-SHA512(frase, "mnemonic" + adgangsord, 2048).
/// `phrase` skal være kanonisk (se modulets dokumentation).
pub fn phrase_to_seed(phrase: &str, passphrase: &str) -> [u8; 64] {
    let mac = Hmac::<Sha512>::new_from_slice(phrase.as_bytes()).expect("HMAC tager alle nøglelængder");
    // Første (og eneste) blok: U1 = HMAC(P, salt || 00 00 00 01).
    let mut m = mac.clone();
    m.update(b"mnemonic");
    m.update(passphrase.as_bytes());
    m.update(&1u32.to_be_bytes());
    let mut u: [u8; 64] = m.finalize().into_bytes().into();
    let mut t = u;
    for _ in 1..2048 {
        let mut m = mac.clone();
        m.update(&u);
        u = m.finalize().into_bytes().into();
        for (a, b) in t.iter_mut().zip(u.iter()) {
            *a ^= b;
        }
    }
    t
}

/// Master-hemmeligheden (64 bytes til [`Identity::from_secret_bytes`]).
pub fn seed_to_master_secret(seed: &[u8; 64]) -> [u8; 64] {
    let hk = hkdf::Hkdf::<Sha256>::new(Some(MASTER_SALT), seed);
    let mut out = [0u8; 64];
    hk.expand(b"", &mut out).expect("64 bytes er inden for HKDF's grænse");
    out
}

/// Hele vejen fra frase (+ valgfrit adgangsord) til identitet. Frasen
/// tjekkes og gøres kanonisk først.
pub fn identity_from_phrase(phrase: &str, passphrase: &str) -> Result<Identity, PhraseError> {
    let canon = entropy_to_phrase(&phrase_to_entropy(phrase)?);
    let seed = phrase_to_seed(&canon, passphrase);
    Ok(Identity::from_secret_bytes(&seed_to_master_secret(&seed)))
}

fn sha256(b: &[u8]) -> [u8; 32] {
    use sha2::Digest;
    Sha256::digest(b).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }
    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn wordlist_is_official() {
        assert_eq!(
            hex(&sha256(WORDLIST.as_bytes())),
            "2f5eed53a4727b4bf8880d8f3f199efc90e58503646d9ff8eff3a2ed3b24dbda"
        );
        assert_eq!(WORDLIST.lines().count(), 2048);
        assert_eq!(word(0), "abandon");
        assert_eq!(word(2047), "zoo");
        assert_eq!(word_index("zoo"), Some(2047));
    }

    /// De officielle BIP39-testvektorer (24 ord, adgangsord "TREZOR").
    const TREZOR: &[(&str, &str, &str)] = &[
        (
            "0000000000000000000000000000000000000000000000000000000000000000",
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art",
            "bda85446c68413707090a52022edd26a1c9462295029f2e60cd7c4f2bbd3097170af7a4d73245cafa9c3cca8d561a7c3de6f5d4a10be8ed2a5e608d68f92fcc8",
        ),
        (
            "7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f",
            "legal winner thank year wave sausage worth useful legal winner thank year wave sausage worth useful legal winner thank year wave sausage worth title",
            "bc09fca1804f7e69da93c2f2028eb238c227f2e9dda30cd63699232578480a4021b146ad717fbb7e451ce9eb835f43620bf5c514db0f8add49f5d121449d3e87",
        ),
        (
            "8080808080808080808080808080808080808080808080808080808080808080",
            "letter advice cage absurd amount doctor acoustic avoid letter advice cage absurd amount doctor acoustic avoid letter advice cage absurd amount doctor acoustic bless",
            "c0c519bd0e91a2ed54357d9d1ebef6f5af218a153624cf4f2da911a0ed8f7a09e2ef61af0aca007096df430022f7a2b6fb91661a9589097069720d015e4e982f",
        ),
        (
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
            "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo vote",
            "dd48c104698c30cfe2b6142103248622fb7bb0ff692eebb00089b32d22484e1613912f0a5b694407be899ffd31ed3992c456cdf60f5d4564b8ba3f05a69890ad",
        ),
        (
            "68a79eaca2324873eacc50cb9c6eca8cc68ea5d936f98787c60c7ebc74e6ce7c",
            "hamster diagram private dutch cause delay private meat slide toddler razor book happy fancy gospel tennis maple dilemma loan word shrug inflict delay length",
            "64c87cde7e12ecf6704ab95bb1408bef047c22db4cc7491c4271d170a1b213d20b385bc1588d9c7b38f1b39d415665b8a9030c9ec653d75e65f847d8fc1fc440",
        ),
    ];

    #[test]
    fn bip39_vectors() {
        for (e, phrase, seed) in TREZOR {
            let e: [u8; 32] = unhex(e).try_into().unwrap();
            assert_eq!(entropy_to_phrase(&e).as_str(), *phrase);
            assert_eq!(phrase_to_entropy(phrase), Ok(e));
            assert_eq!(hex(&phrase_to_seed(phrase, "TREZOR")), *seed);
        }
    }

    /// Vores egen vektor: "abandon … art" uden adgangsord. Samme tal står i
    /// tools/recovery_phrase_test.mjs (webklienten).
    #[test]
    fn maarse_vector() {
        let phrase = TREZOR[0].1;
        let seed = phrase_to_seed(phrase, "");
        assert_eq!(
            hex(&seed),
            "408b285c123836004f4b8842c89324c1f01382450c0d439af345ba7fc49acf705489c6fc77dbd4e3dc1dd8cc6bc9f043db8ada1e243c4a0eafb290d399480840"
        );
        let master = seed_to_master_secret(&seed);
        assert_eq!(
            hex(&master),
            "743d6ccda8a1826db3ef1df490b9d6d82f4b3920c76335447f21b261e4bf72525485ca54b88ea0c0358d84254ee7c92855aa5ec5fec21de960bf6f00996c80a4"
        );
        let id = identity_from_phrase(phrase, "").unwrap();
        assert_eq!(id.to_secret_bytes(), master);
        assert_eq!(hex(&id.public().to_bytes()), MAARSE_PUBLIC);
        assert_eq!(hex(&id.address().0), MAARSE_ADDRESS);
        // Store bogstaver og ekstra mellemrum er ligegyldige.
        let messy = phrase.to_uppercase().replace(' ', "  \n");
        assert_eq!(identity_from_phrase(&messy, "").unwrap().address(), id.address());
        // Adgangsordet giver en anden identitet.
        assert_ne!(identity_from_phrase(phrase, "x").unwrap().address(), id.address());
    }

    const MAARSE_PUBLIC: &str =
        "04c10d960686af455a9cb72273370a85d566605017f1cc37d41abde3b1e5b53facc9b97b5a899e4332874acc655e5f792e806d6743b26049c90f868a3484e112";
    const MAARSE_ADDRESS: &str = "6ef182b74279e05122977ced600ae3e6";

    #[test]
    fn rejects_bad_phrases() {
        let ok = TREZOR[1].1;
        assert_eq!(phrase_to_entropy("abandon"), Err(PhraseError::WordCount));
        assert_eq!(phrase_to_entropy(&format!("{ok} zoo")), Err(PhraseError::WordCount));
        assert_eq!(phrase_to_entropy(&ok.replace("thank", "thnk")), Err(PhraseError::UnknownWord(2)));
        // Sidste ord byttet: tjeksummen fejler.
        assert_eq!(phrase_to_entropy(&ok.replace("title", "zoo")), Err(PhraseError::Checksum));
        assert_eq!(phrase_to_entropy(&TREZOR[0].1.replace("art", "abandon")), Err(PhraseError::Checksum));
    }
}
