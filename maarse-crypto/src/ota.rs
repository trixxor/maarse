//! Signerede firmwareopdateringer: manifest, signatur og modtagelse.
//!
//! En udgivelse består af et firmware-image (rå binær: på nRF52840 appen
//! fra dens startadresse, på ESP32-S3 et ESP-IDF app-image) og et manifest
//! på [`MANIFEST_LEN`] bytes:
//!
//! ```text
//! 0   "MOTA"
//! 4   format (1)
//! 5   flag (0, reserveret)
//! 6   board-ID (u16, se [`board`])
//! 8   version (u32: major << 16 | minor << 8 | patch)
//! 12  størrelse (u32, bytes)
//! 16  SHA-256 af imaget (32)
//! 48  Ed25519-signatur (64) over "maarse/firmware/v1" | byte 0..48
//! ```
//!
//! Noden installerer kun, hvis signaturen passer med udgivelsesnøglen, der er
//! bygget ind i firmwaren, board-ID passer, og versionen er nyere end den
//! kørende (anti-rollback). Kun lokalt (USB/Bluetooth) kan en ældre version
//! tillades med et eksplicit flag.
//!
//! [`Receiver`] tager imod imaget i bidder og skriver det til et
//! mellemlager ([`Staging`]). Fremskridtet gemmes som en bitmap pr.
//! 4 KB-blok på en metaside, så en afbrudt overførsel kan genoptages - også
//! efter genstart - fra første ufærdige blok.

use ed25519_dalek::{Signature, Signer, Verifier, VerifyingKey};
pub use ed25519_dalek::SigningKey;
use sha2::{Digest, Sha256};

pub const MANIFEST_LEN: usize = 112;
const MAGIC: &[u8; 4] = b"MOTA";
const FORMAT: u8 = 1;
const SIGNED_LEN: usize = 48;
const CONTEXT: &[u8] = b"maarse/firmware/v1";
/// Blokstørrelse i mellemlageret (slettesektor på begge platforme).
pub const BLOCK: u32 = 4096;
/// Metasiden: "OTM1" + manifest, derefter ét ord pr. blok (0xFFFFFFFF =
/// ikke skrevet, 0 = færdig; hvert ord skrives højst én gang).
const META_MAGIC: &[u8; 4] = b"OTM1";
const META_BITMAP: u32 = 128;
/// Største antal blokke, metasiden kan holde styr på (ca. 3,9 MB).
pub const MAX_BLOCKS: u32 = (BLOCK - META_BITMAP) / 4;

/// Board-ID'er. Et image til ét board installeres aldrig på et andet.
pub mod board {
    pub const XIAO_NRF52840: u16 = 1;
    pub const WISMESH_TAG: u16 = 2;
    pub const HELTEC_V4: u16 = 3;
    pub const XIAO_ESP32S3: u16 = 4;

    pub fn name(id: u16) -> &'static str {
        match id {
            XIAO_NRF52840 => "xiao-nrf52840",
            WISMESH_TAG => "wismesh-tag",
            HELTEC_V4 => "heltec-v4",
            XIAO_ESP32S3 => "xiao-esp32s3",
            _ => "ukendt",
        }
    }

    pub fn from_name(s: &str) -> Option<u16> {
        [XIAO_NRF52840, WISMESH_TAG, HELTEC_V4, XIAO_ESP32S3]
            .into_iter()
            .find(|&b| name(b) == s)
    }
}

/// Udviklingsnøglens frø: SHA-256 af denne tekst. Den er OFFENTLIG - alle
/// kan signere med den. Firmware bygget uden `MAARSE_RELEASE_PUBKEY` stoler
/// på den og advarer om det.
pub const DEV_SEED_TEXT: &[u8] = b"maarse/dev-release-key/IKKE-TIL-UDGIVELSER";

pub fn dev_signing_key() -> SigningKey {
    SigningKey::from_bytes(&Sha256::digest(DEV_SEED_TEXT).into())
}

pub fn dev_public_key() -> [u8; 32] {
    dev_signing_key().verifying_key().to_bytes()
}

/// SHA-256 af et image.
/// Domæne for udgivelsessignaturer (SHA256SUMS), så en signatur over en
/// udgivelse aldrig kan forveksles med et firmware-manifest.
const RELEASE_DOMAIN: &[u8] = b"maarse-release-v1
";

/// Det, der signeres: domænet efterfulgt af SHA-256 af SHA256SUMS-filen.
fn release_message(sums: &[u8]) -> [u8; 18 + 32] {
    let mut m = [0u8; 18 + 32];
    m[..18].copy_from_slice(RELEASE_DOMAIN);
    m[18..].copy_from_slice(&sha256(sums));
    m
}

/// Signerer en udgivelses SHA256SUMS-fil med udgivelsesnøglen.
pub fn sign_release(key: &SigningKey, sums: &[u8]) -> [u8; 64] {
    key.sign(&release_message(sums)).to_bytes()
}

/// Tjekker en udgivelsessignatur.
pub fn verify_release(public_key: &[u8; 32], sums: &[u8], sig: &[u8; 64]) -> bool {
    let Ok(pk) = VerifyingKey::from_bytes(public_key) else { return false };
    pk.verify(&release_message(sums), &Signature::from_bytes(sig)).is_ok()
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

/// "1.2.3" -> 0x010203. Hver del højst 255 (major højst 65535).
pub fn parse_version(s: &str) -> Option<u32> {
    let mut it = s.trim().split('.');
    let major: u32 = it.next()?.parse().ok()?;
    let minor: u32 = it.next().unwrap_or("0").parse().ok()?;
    let patch: u32 = it.next().unwrap_or("0").split(['-', '+']).next()?.parse().ok()?;
    if it.next().is_some() || major > 0xFFFF || minor > 255 || patch > 255 {
        return None;
    }
    Some(major << 16 | minor << 8 | patch)
}

/// 0x010203 -> (1, 2, 3)
pub fn version_parts(v: u32) -> (u32, u32, u32) {
    (v >> 16, (v >> 8) & 0xFF, v & 0xFF)
}

/// 64 hextegn -> 32 bytes.
pub fn parse_hex32(s: &str) -> Option<[u8; 32]> {
    let s = s.trim().as_bytes();
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, o) in out.iter_mut().enumerate() {
        let h = (s[2 * i] as char).to_digit(16)?;
        let l = (s[2 * i + 1] as char).to_digit(16)?;
        *o = (h << 4 | l) as u8;
    }
    Some(out)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub board: u16,
    pub version: u32,
    pub size: u32,
    pub sha256: [u8; 32],
    pub signature: [u8; 64],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OtaError {
    BadManifest,
    BadSignature,
    WrongBoard,
    /// Versionen er ikke nyere end den kørende.
    NotNewer,
    TooLarge,
    /// Bidden passer ikke med det forventede offset (svaret fortæller hvor).
    WrongOffset,
    BadChunk,
    NotStarted,
    Incomplete,
    HashMismatch,
    Flash,
}

impl OtaError {
    pub fn text(self) -> &'static str {
        match self {
            OtaError::BadManifest => "Ugyldigt manifest",
            OtaError::BadSignature => "Signaturen passer ikke med udgivelsesnøglen",
            OtaError::WrongBoard => "Firmwaren er til et andet board",
            OtaError::NotNewer => "Versionen er ikke nyere end den kørende (rollback afvist)",
            OtaError::TooLarge => "Firmwaren er for stor til mellemlageret",
            OtaError::WrongOffset => "Forkert offset; fortsæt fra det angivne",
            OtaError::BadChunk => "Ugyldig bid (offset/længde skal være delelig med 4)",
            OtaError::NotStarted => "Ingen opdatering i gang",
            OtaError::Incomplete => "Firmwaren er ikke overført helt",
            OtaError::HashMismatch => "SHA-256 passer ikke - overfør igen",
            OtaError::Flash => "Flash-fejl",
        }
    }
}

impl Manifest {
    pub fn parse(b: &[u8]) -> Result<Self, OtaError> {
        if b.len() != MANIFEST_LEN || &b[..4] != MAGIC || b[4] != FORMAT || b[5] != 0 {
            return Err(OtaError::BadManifest);
        }
        let u32_at = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        let mut sha256 = [0u8; 32];
        sha256.copy_from_slice(&b[16..48]);
        let mut signature = [0u8; 64];
        signature.copy_from_slice(&b[48..112]);
        let m = Manifest {
            board: u16::from_le_bytes([b[6], b[7]]),
            version: u32_at(8),
            size: u32_at(12),
            sha256,
            signature,
        };
        if m.size == 0 {
            return Err(OtaError::BadManifest);
        }
        Ok(m)
    }

    fn signed_part(&self) -> [u8; SIGNED_LEN] {
        let mut b = [0u8; SIGNED_LEN];
        b[..4].copy_from_slice(MAGIC);
        b[4] = FORMAT;
        b[6..8].copy_from_slice(&self.board.to_le_bytes());
        b[8..12].copy_from_slice(&self.version.to_le_bytes());
        b[12..16].copy_from_slice(&self.size.to_le_bytes());
        b[16..48].copy_from_slice(&self.sha256);
        b
    }

    fn message(&self) -> [u8; CONTEXT.len() + SIGNED_LEN] {
        let mut m = [0u8; CONTEXT.len() + SIGNED_LEN];
        m[..CONTEXT.len()].copy_from_slice(CONTEXT);
        m[CONTEXT.len()..].copy_from_slice(&self.signed_part());
        m
    }

    pub fn to_bytes(&self) -> [u8; MANIFEST_LEN] {
        let mut b = [0u8; MANIFEST_LEN];
        b[..SIGNED_LEN].copy_from_slice(&self.signed_part());
        b[SIGNED_LEN..].copy_from_slice(&self.signature);
        b
    }

    /// Laver et signeret manifest for `image` (bruges af `maarse-sign`).
    pub fn sign(key: &SigningKey, board: u16, version: u32, image: &[u8]) -> Self {
        let mut m = Manifest {
            board,
            version,
            size: image.len() as u32,
            sha256: Sha256::digest(image).into(),
            signature: [0; 64],
        };
        m.signature = key.sign(&m.message()).to_bytes();
        m
    }

    pub fn verify_signature(&self, public_key: &[u8; 32]) -> Result<(), OtaError> {
        let key = VerifyingKey::from_bytes(public_key).map_err(|_| OtaError::BadSignature)?;
        key.verify(&self.message(), &Signature::from_bytes(&self.signature))
            .map_err(|_| OtaError::BadSignature)
    }

    /// Alle kontroller før overførslen: signatur, board, version, størrelse.
    pub fn check(&self, policy: &Policy, allow_downgrade: bool) -> Result<(), OtaError> {
        self.verify_signature(&policy.public_key)?;
        if self.board != policy.board {
            return Err(OtaError::WrongBoard);
        }
        if self.version <= policy.current_version && !allow_downgrade {
            return Err(OtaError::NotNewer);
        }
        if self.size > policy.capacity || self.size.div_ceil(BLOCK) > MAX_BLOCKS {
            return Err(OtaError::TooLarge);
        }
        Ok(())
    }
}

/// Hvad noden accepterer.
#[derive(Clone, Copy, Debug)]
pub struct Policy {
    pub public_key: [u8; 32],
    pub board: u16,
    pub current_version: u32,
    /// Mellemlagerets størrelse i bytes.
    pub capacity: u32,
}

/// Mellemlageret og dets metaside. Offsets er relative til mellemlageret;
/// metasiden (én blok) adresseres for sig. Skrivninger er ord-justerede.
pub trait Staging {
    fn erase_block(&mut self, offset: u32) -> Result<(), ()>;
    fn write(&mut self, offset: u32, data: &[u8]) -> Result<(), ()>;
    fn read(&mut self, offset: u32, buf: &mut [u8]) -> Result<(), ()>;
    fn erase_meta(&mut self) -> Result<(), ()>;
    fn write_meta(&mut self, offset: u32, data: &[u8]) -> Result<(), ()>;
    fn read_meta(&mut self, offset: u32, buf: &mut [u8]) -> Result<(), ()>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Idle,
    Receiving,
    /// Hele imaget er modtaget og SHA-256 passer.
    Verified,
}

#[derive(Clone)]
/// Modtager et image i rækkefølge (stop-and-wait: klienten sender næste
/// bid, når forrige er kvitteret med næste offset).
pub struct Receiver {
    pub manifest: Option<Manifest>,
    pub state: State,
    /// Næste forventede offset.
    pub next: u32,
    /// Blokke under `erased` er slettet i denne omgang og må skrives.
    erased: u32,
}

impl Default for Receiver {
    fn default() -> Self {
        Self::new()
    }
}

impl Receiver {
    pub const fn new() -> Self {
        Receiver { manifest: None, state: State::Idle, next: 0, erased: 0 }
    }

    /// Starter (eller genoptager) en overførsel. Er det samme manifest
    /// allerede gemt på metasiden, fortsættes fra første ufærdige blok.
    /// Returnerer det offset, klienten skal sende fra.
    pub fn begin(
        &mut self,
        st: &mut (impl Staging + ?Sized),
        m: Manifest,
        policy: &Policy,
        allow_downgrade: bool,
    ) -> Result<u32, OtaError> {
        m.check(policy, allow_downgrade)?;
        let blocks = m.size.div_ceil(BLOCK);
        let mut head = [0u8; 4 + MANIFEST_LEN];
        st.read_meta(0, &mut head).map_err(|_| OtaError::Flash)?;
        let same = &head[..4] == META_MAGIC && head[4..] == m.to_bytes();
        let mut done = 0;
        if same {
            while done < blocks {
                let mut w = [0u8; 4];
                st.read_meta(META_BITMAP + 4 * done, &mut w).map_err(|_| OtaError::Flash)?;
                if w != [0; 4] {
                    break;
                }
                done += 1;
            }
        } else {
            st.erase_meta().map_err(|_| OtaError::Flash)?;
            head[..4].copy_from_slice(META_MAGIC);
            head[4..].copy_from_slice(&m.to_bytes());
            st.write_meta(0, &head).map_err(|_| OtaError::Flash)?;
        }
        self.manifest = Some(m);
        self.next = (done * BLOCK).min(m.size);
        self.erased = done;
        self.state = State::Receiving;
        if self.next >= m.size {
            self.finish(st)?;
        }
        Ok(self.next)
    }

    /// Skriver en bid. `offset` skal være lig `next` (ellers `WrongOffset`,
    /// og klienten fortsætter fra `next`). Længden skal være delelig med 4,
    /// undtagen sidste bid. Returnerer næste offset.
    pub fn chunk(&mut self, st: &mut (impl Staging + ?Sized), offset: u32, data: &[u8]) -> Result<u32, OtaError> {
        let m = self.manifest.ok_or(OtaError::NotStarted)?;
        if self.state == State::Verified && offset >= m.size {
            return Ok(self.next);
        }
        if self.state != State::Receiving {
            return Err(OtaError::NotStarted);
        }
        if offset != self.next {
            return Err(OtaError::WrongOffset);
        }
        let end = offset.checked_add(data.len() as u32).ok_or(OtaError::BadChunk)?;
        if data.is_empty() || offset % 4 != 0 || end > m.size || (data.len() % 4 != 0 && end != m.size) {
            return Err(OtaError::BadChunk);
        }
        // Slet nye blokke, før der skrives i dem.
        while self.erased * BLOCK < end {
            st.erase_block(self.erased * BLOCK).map_err(|_| OtaError::Flash)?;
            self.erased += 1;
        }
        let whole = data.len() & !3;
        if whole > 0 {
            st.write(offset, &data[..whole]).map_err(|_| OtaError::Flash)?;
        }
        if whole < data.len() {
            let mut last = [0xFFu8; 4];
            last[..data.len() - whole].copy_from_slice(&data[whole..]);
            st.write(offset + whole as u32, &last).map_err(|_| OtaError::Flash)?;
        }
        // Markér blokke, der nu er hele, som færdige.
        let first_open = offset / BLOCK;
        let complete = if end == m.size { end.div_ceil(BLOCK) } else { end / BLOCK };
        for b in first_open..complete {
            st.write_meta(META_BITMAP + 4 * b, &[0; 4]).map_err(|_| OtaError::Flash)?;
        }
        self.next = end;
        if end == m.size {
            self.finish(st)?;
        }
        Ok(self.next)
    }

    /// Tjekker SHA-256 over hele mellemlageret.
    fn finish(&mut self, st: &mut (impl Staging + ?Sized)) -> Result<(), OtaError> {
        let m = self.manifest.ok_or(OtaError::NotStarted)?;
        let mut h = Sha256::new();
        let mut buf = [0u8; 256];
        let mut o = 0;
        while o < m.size {
            let n = (m.size - o).min(256) as usize;
            st.read(o, &mut buf[..n.div_ceil(4) * 4]).map_err(|_| OtaError::Flash)?;
            h.update(&buf[..n]);
            o += n as u32;
        }
        if <[u8; 32]>::from(h.finalize()) != m.sha256 {
            // Start forfra: glem fremskridtet.
            let _ = st.erase_meta();
            self.state = State::Idle;
            self.manifest = None;
            self.next = 0;
            return Err(OtaError::HashMismatch);
        }
        self.state = State::Verified;
        Ok(())
    }

    /// Klar til installation? (hele imaget er modtaget og verificeret)
    pub fn end(&self) -> Result<Manifest, OtaError> {
        match (self.state, self.manifest) {
            (State::Verified, Some(m)) => Ok(m),
            (State::Receiving, _) => Err(OtaError::Incomplete),
            _ => Err(OtaError::NotStarted),
        }
    }

    /// Læser mellemlageret igen og tjekker SHA-256 lige før installation.
    pub fn recheck(&mut self, st: &mut (impl Staging + ?Sized)) -> Result<Manifest, OtaError> {
        self.end()?;
        self.finish(st)?;
        self.end()
    }

    pub fn abort(&mut self, st: &mut (impl Staging + ?Sized)) {
        let _ = st.erase_meta();
        *self = Receiver::new();
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    extern crate std;
    use std::vec;
    use std::vec::Vec;

    /// Flash i RAM, der opfører sig som NOR: kun 1 -> 0 uden sletning, og
    /// hvert ord på metasiden må højst skrives én gang efter sletning.
    pub struct MockFlash {
        pub data: Vec<u8>,
        pub meta: Vec<u8>,
        pub meta_writes: Vec<u8>,
        pub fail_after: Option<usize>,
        pub writes: usize,
    }

    impl MockFlash {
        pub fn new(cap: usize) -> Self {
            MockFlash {
                data: vec![0xFF; cap],
                meta: vec![0xFF; BLOCK as usize],
                meta_writes: vec![0; BLOCK as usize / 4],
                fail_after: None,
                writes: 0,
            }
        }
    }

    fn nor_write(mem: &mut [u8], off: u32, data: &[u8]) -> Result<(), ()> {
        if off % 4 != 0 || data.len() % 4 != 0 || off as usize + data.len() > mem.len() {
            return Err(());
        }
        for (i, b) in data.iter().enumerate() {
            mem[off as usize + i] &= *b;
        }
        Ok(())
    }

    impl Staging for MockFlash {
        fn erase_block(&mut self, offset: u32) -> Result<(), ()> {
            assert_eq!(offset % BLOCK, 0);
            let o = offset as usize;
            self.data.get_mut(o..o + BLOCK as usize).ok_or(())?.fill(0xFF);
            Ok(())
        }
        fn write(&mut self, offset: u32, data: &[u8]) -> Result<(), ()> {
            self.writes += 1;
            if self.fail_after.is_some_and(|n| self.writes > n) {
                return Err(());
            }
            nor_write(&mut self.data, offset, data)
        }
        fn read(&mut self, offset: u32, buf: &mut [u8]) -> Result<(), ()> {
            let o = offset as usize;
            buf.copy_from_slice(self.data.get(o..o + buf.len()).ok_or(())?);
            Ok(())
        }
        fn erase_meta(&mut self) -> Result<(), ()> {
            self.meta.fill(0xFF);
            self.meta_writes.fill(0);
            Ok(())
        }
        fn write_meta(&mut self, offset: u32, data: &[u8]) -> Result<(), ()> {
            for w in (offset / 4)..(offset + data.len() as u32).div_ceil(4) {
                self.meta_writes[w as usize] += 1;
                assert!(self.meta_writes[w as usize] <= 1, "metaord skrevet to gange");
            }
            nor_write(&mut self.meta, offset, data)
        }
        fn read_meta(&mut self, offset: u32, buf: &mut [u8]) -> Result<(), ()> {
            let o = offset as usize;
            buf.copy_from_slice(&self.meta[o..o + buf.len()]);
            Ok(())
        }
    }

    fn image(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i * 7 + i / 251) as u8).collect()
    }

    fn policy(cap: u32) -> Policy {
        Policy { public_key: dev_public_key(), board: board::XIAO_NRF52840, current_version: 0x000100, capacity: cap }
    }

    fn send_all(r: &mut Receiver, f: &mut MockFlash, img: &[u8], from: u32, chunk: usize) -> Result<u32, OtaError> {
        let mut o = from;
        while (o as usize) < img.len() {
            let end = (o as usize + chunk).min(img.len());
            o = r.chunk(f, o, &img[o as usize..end])?;
        }
        Ok(o)
    }

    #[test]
    fn manifest_roundtrip_and_signature() {
        let img = image(1000);
        let m = Manifest::sign(&dev_signing_key(), board::HELTEC_V4, 0x010203, &img);
        let b = m.to_bytes();
        let p = Manifest::parse(&b).unwrap();
        assert_eq!(p, m);
        assert!(p.verify_signature(&dev_public_key()).is_ok());
        // Enhver ændret byte i den signerede del eller signaturen afvises.
        for i in 0..MANIFEST_LEN {
            let mut bad = b;
            bad[i] ^= 1;
            if let Ok(q) = Manifest::parse(&bad) {
                assert!(q.verify_signature(&dev_public_key()).is_err(), "byte {i}");
            }
        }
        let other = SigningKey::from_bytes(&[9; 32]).verifying_key().to_bytes();
        assert_eq!(p.verify_signature(&other), Err(OtaError::BadSignature));
        assert!(Manifest::parse(&b[..111]).is_err());
        assert_eq!(parse_version("1.2.3"), Some(0x010203));
        assert_eq!(parse_version("0.1.0-dev"), Some(0x000100));
        assert_eq!(parse_version("1.256.0"), None);
        assert_eq!(parse_hex32(&"ab".repeat(32)), Some([0xAB; 32]));
    }

    #[test]
    fn policy_rejects_board_rollback_and_size() {
        let img = image(5000);
        let key = dev_signing_key();
        let pol = policy(64 * 1024);
        let ok = Manifest::sign(&key, board::XIAO_NRF52840, 0x000200, &img);
        assert!(ok.check(&pol, false).is_ok());
        let wrong = Manifest::sign(&key, board::WISMESH_TAG, 0x000200, &img);
        assert_eq!(wrong.check(&pol, false), Err(OtaError::WrongBoard));
        let same = Manifest::sign(&key, board::XIAO_NRF52840, 0x000100, &img);
        assert_eq!(same.check(&pol, false), Err(OtaError::NotNewer));
        let older = Manifest::sign(&key, board::XIAO_NRF52840, 0x000009, &img);
        assert_eq!(older.check(&pol, false), Err(OtaError::NotNewer));
        assert!(older.check(&pol, true).is_ok());
        let big = Manifest::sign(&key, board::XIAO_NRF52840, 0x000200, &vec![0; 70 * 1024]);
        assert_eq!(big.check(&pol, false), Err(OtaError::TooLarge));
        let foreign = Manifest::sign(&SigningKey::from_bytes(&[1; 32]), board::XIAO_NRF52840, 0x000200, &img);
        assert_eq!(foreign.check(&pol, false), Err(OtaError::BadSignature));
    }

    #[test]
    fn receive_whole_image() {
        let img = image(10_001);
        let m = Manifest::sign(&dev_signing_key(), board::XIAO_NRF52840, 0x000200, &img);
        let mut f = MockFlash::new(64 * 1024);
        let mut r = Receiver::new();
        assert_eq!(r.begin(&mut f, m, &policy(64 * 1024), false), Ok(0));
        assert_eq!(r.end(), Err(OtaError::Incomplete));
        // Forkert offset afvises uden at ødelægge noget.
        assert_eq!(r.chunk(&mut f, 128, &img[128..256]), Err(OtaError::WrongOffset));
        assert_eq!(r.chunk(&mut f, 0, &img[0..3]), Err(OtaError::BadChunk));
        assert_eq!(send_all(&mut r, &mut f, &img, 0, 128), Ok(10_001));
        assert_eq!(r.end(), Ok(m));
        assert_eq!(&f.data[..img.len()], &img[..]);
        // En gentaget sidste bid (tabt kvittering) er ufarlig.
        assert_eq!(r.chunk(&mut f, 10_001, &[]), Ok(10_001));
    }

    #[test]
    fn resume_after_reboot_and_interruption() {
        let img = image(3 * 4096 + 500);
        let m = Manifest::sign(&dev_signing_key(), board::XIAO_NRF52840, 0x000200, &img);
        let pol = policy(64 * 1024);
        let mut f = MockFlash::new(64 * 1024);
        let mut r = Receiver::new();
        r.begin(&mut f, m, &pol, false).unwrap();
        // Afbrudt midt i blok 2 (strømsvigt: RAM-tilstanden er væk).
        let mut o = 0;
        while o < 2 * 4096 + 1000 {
            o = r.chunk(&mut f, o, &img[o as usize..o as usize + 200]).unwrap();
        }
        let mut r = Receiver::new();
        assert_eq!(r.begin(&mut f, m, &pol, false), Ok(2 * 4096));
        assert_eq!(send_all(&mut r, &mut f, &img, 2 * 4096, 240), Ok(img.len() as u32));
        assert!(r.end().is_ok());
        assert_eq!(&f.data[..img.len()], &img[..]);
        // Begin igen efter færdig overførsel: allerede verificeret.
        let mut r = Receiver::new();
        assert_eq!(r.begin(&mut f, m, &pol, false), Ok(img.len() as u32));
        assert!(r.end().is_ok());
        // Et nyt manifest starter forfra.
        let img2 = image(5000);
        let m2 = Manifest::sign(&dev_signing_key(), board::XIAO_NRF52840, 0x000300, &img2);
        assert_eq!(r.begin(&mut f, m2, &pol, false), Ok(0));
    }

    #[test]
    fn corrupt_staging_is_detected() {
        let img = image(6000);
        let m = Manifest::sign(&dev_signing_key(), board::XIAO_NRF52840, 0x000200, &img);
        let mut f = MockFlash::new(64 * 1024);
        let mut r = Receiver::new();
        r.begin(&mut f, m, &policy(64 * 1024), false).unwrap();
        let mut bad = img.clone();
        bad[4000] ^= 0x10;
        assert_eq!(send_all(&mut r, &mut f, &bad, 0, 240), Err(OtaError::HashMismatch));
        assert_eq!(r.end(), Err(OtaError::NotStarted));
        // Forfra med korrekte data.
        assert_eq!(r.begin(&mut f, m, &policy(64 * 1024), false), Ok(0));
        assert!(send_all(&mut r, &mut f, &img, 0, 240).is_ok());
    }

    #[test]
    fn flash_error_keeps_offset() {
        let img = image(2000);
        let m = Manifest::sign(&dev_signing_key(), board::XIAO_NRF52840, 0x000200, &img);
        let mut f = MockFlash::new(64 * 1024);
        f.fail_after = Some(3);
        let mut r = Receiver::new();
        r.begin(&mut f, m, &policy(64 * 1024), false).unwrap();
        assert_eq!(send_all(&mut r, &mut f, &img, 0, 128), Err(OtaError::Flash));
        assert_eq!(r.next, 3 * 128);
    }

    #[test]
    fn release_signature() {
        let k = SigningKey::from_bytes(&[3; 32]);
        let pk = k.verifying_key().to_bytes();
        let sums = b"abc  maarse.apk
";
        let sig = sign_release(&k, sums);
        assert!(verify_release(&pk, sums, &sig));
        assert!(!verify_release(&pk, b"abd  maarse.apk
", &sig), "ændret fil");
        assert!(!verify_release(&SigningKey::from_bytes(&[4; 32]).verifying_key().to_bytes(), sums, &sig));
    }
}
