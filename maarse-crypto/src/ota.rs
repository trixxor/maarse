//! Signed firmware updates: manifest, signature and reception.
//!
//! A release consists of a firmware image (raw binary: on the nRF52840 the app
//! from its start address, on the ESP32-S3 an ESP-IDF app image) and a manifest
//! of [`MANIFEST_LEN`] bytes:
//!
//! ```text
//! 0   "MOTA"
//! 4   format (1)
//! 5   flags (0, reserved)
//! 6   board ID (u16, see [`board`])
//! 8   version (u32: major << 16 | minor << 8 | patch)
//! 12  size (u32, bytes)
//! 16  SHA-256 of the image (32)
//! 48  Ed25519 signature (64) over "maarse/firmware/v1" | bytes 0..48
//! ```
//!
//! The node only installs if the signature matches the release key built
//! into the firmware, the board ID matches, and the version is newer than the
//! running one (anti-rollback). Only locally (USB/Bluetooth) can an older version
//! be allowed with an explicit flag.
//!
//! [`Receiver`] receives the image in chunks and writes it to a
//! staging area ([`Staging`]). Progress is stored as a bitmap per
//! 4 KB block on a meta page, so an interrupted transfer can be resumed - even
//! after a restart - from the first unfinished block.

use ed25519_dalek::{Signature, Signer, VerifyingKey};
pub use ed25519_dalek::SigningKey;
use sha2::{Digest, Sha256};

pub const MANIFEST_LEN: usize = 112;
const MAGIC: &[u8; 4] = b"MOTA";
const FORMAT: u8 = 1;
const SIGNED_LEN: usize = 48;
const CONTEXT: &[u8] = b"maarse/firmware/v1";
/// Block size in the staging area (erase sector on both platforms).
pub const BLOCK: u32 = 4096;
/// The meta page: "OTM1" + manifest, then one word per block (0xFFFFFFFF =
/// not written, 0 = done; each word is written at most once).
const META_MAGIC: &[u8; 4] = b"OTM1";
const META_BITMAP: u32 = 128;
/// Largest number of blocks the meta page can keep track of (about 3.9 MB).
pub const MAX_BLOCKS: u32 = (BLOCK - META_BITMAP) / 4;

/// Board IDs. An image for one board is never installed on another.
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
        _ => "unknown",
        }
    }

    pub fn from_name(s: &str) -> Option<u16> {
        [XIAO_NRF52840, WISMESH_TAG, HELTEC_V4, XIAO_ESP32S3]
            .into_iter()
            .find(|&b| name(b) == s)
    }
}

/// The development key's seed: SHA-256 of this text. It is PUBLIC - anyone
/// can sign with it. Use it only in test tooling and test vectors.
pub const DEV_SEED_TEXT: &[u8] = b"maarse/dev-release-key/NOT-FOR-RELEASES";

pub fn dev_signing_key() -> SigningKey {
    SigningKey::from_bytes(&Sha256::digest(DEV_SEED_TEXT).into())
}

/// Checks that the configured Ed25519 public key decodes and is not weak.
pub fn valid_release_key(key: &[u8; 32]) -> bool {
    *key != dev_public_key() && VerifyingKey::from_bytes(key).is_ok_and(|key| !key.is_weak())
}

pub fn dev_public_key() -> [u8; 32] {
    dev_signing_key().verifying_key().to_bytes()
}

/// SHA-256 of an image.
/// Domain for release signatures (SHA256SUMS), so a signature over a
/// release can never be confused with a firmware manifest.
const RELEASE_DOMAIN: &[u8] = b"maarse-release-v1
";

/// What is signed: the domain followed by SHA-256 of the SHA256SUMS file.
fn release_message(sums: &[u8]) -> [u8; 18 + 32] {
    let mut m = [0u8; 18 + 32];
    m[..18].copy_from_slice(RELEASE_DOMAIN);
    m[18..].copy_from_slice(&sha256(sums));
    m
}

/// Signs a release's SHA256SUMS file with the release key.
pub fn sign_release(key: &SigningKey, sums: &[u8]) -> [u8; 64] {
    key.sign(&release_message(sums)).to_bytes()
}

/// Checks a release signature.
pub fn verify_release(public_key: &[u8; 32], sums: &[u8], sig: &[u8; 64]) -> bool {
    let Ok(pk) = VerifyingKey::from_bytes(public_key) else { return false };
    pk.verify_strict(&release_message(sums), &Signature::from_bytes(sig)).is_ok()
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

/// "1.2.3" -> 0x010203. Each part at most 255 (major at most 65535).
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

/// 64 hex characters -> 32 bytes.
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
    /// The version is not newer than the running one.
    NotNewer,
    TooLarge,
    /// The chunk does not match the expected offset (the reply says where).
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
            OtaError::BadManifest => "Invalid manifest",
            OtaError::BadSignature => "The signature does not match the release key",
            OtaError::WrongBoard => "The firmware is for a different board",
            OtaError::NotNewer => "The version is not newer than the running one (rollback rejected)",
            OtaError::TooLarge => "The firmware is too large for the staging area",
            OtaError::WrongOffset => "Wrong offset; continue from the given one",
            OtaError::BadChunk => "Invalid chunk (offset/length must be a multiple of 4)",
            OtaError::NotStarted => "No update in progress",
            OtaError::Incomplete => "The firmware has not been fully transferred",
            OtaError::HashMismatch => "SHA-256 does not match - transfer again",
            OtaError::Flash => "Flash error",
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

    /// Makes a signed manifest for `image` (used by `maarse-sign`).
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
        key.verify_strict(&self.message(), &Signature::from_bytes(&self.signature))
            .map_err(|_| OtaError::BadSignature)
    }

    /// All checks before the transfer: signature, board, version, size.
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

/// What the node accepts.
#[derive(Clone, Copy, Debug)]
pub struct Policy {
    pub public_key: [u8; 32],
    pub board: u16,
    pub current_version: u32,
    /// The size of the staging area in bytes.
    pub capacity: u32,
}

/// The staging area and its meta page. Offsets are relative to the staging area;
/// the meta page (one block) is addressed separately. Writes are word aligned.
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
    /// The whole image has been received and SHA-256 matches.
    Verified,
}

#[derive(Clone)]
/// Receives an image in order (stop-and-wait: the client sends the next
/// chunk when the previous one is acknowledged with the next offset).
pub struct Receiver {
    pub manifest: Option<Manifest>,
    pub state: State,
    /// The next expected offset.
    pub next: u32,
    /// Blocks below `erased` have been erased in this round and may be written.
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

    /// Starts (or resumes) a transfer. If the same manifest is
    /// already stored on the meta page, it continues from the first unfinished block.
    /// Returns the offset the client should send from.
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

    /// Writes a chunk. `offset` must equal `next` (otherwise `WrongOffset`,
    /// and the client continues from `next`). The length must be a multiple of 4,
    /// except for the last chunk. Returns the next offset.
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
        // Erase new blocks before writing to them.
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
        // Mark blocks that are now complete as done.
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
        // Start over: forget the progress.
            let _ = st.erase_meta();
            self.state = State::Idle;
            self.manifest = None;
            self.next = 0;
            return Err(OtaError::HashMismatch);
        }
        self.state = State::Verified;
        Ok(())
    }

    /// Ready to install? (the whole image is received and verified)
    pub fn end(&self) -> Result<Manifest, OtaError> {
        match (self.state, self.manifest) {
            (State::Verified, Some(m)) => Ok(m),
            (State::Receiving, _) => Err(OtaError::Incomplete),
            _ => Err(OtaError::NotStarted),
        }
    }

    /// Reads the staging area again and checks SHA-256 right before installation.
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

    /// Flash in RAM that behaves like NOR: only 1 -> 0 without erasing, and
    /// each word on the meta page may be written at most once after an erase.
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
        // Any changed byte in the signed part or the signature is rejected.
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
        // A wrong offset is rejected without breaking anything.
        assert_eq!(r.chunk(&mut f, 128, &img[128..256]), Err(OtaError::WrongOffset));
        assert_eq!(r.chunk(&mut f, 0, &img[0..3]), Err(OtaError::BadChunk));
        assert_eq!(send_all(&mut r, &mut f, &img, 0, 128), Ok(10_001));
        assert_eq!(r.end(), Ok(m));
        assert_eq!(&f.data[..img.len()], &img[..]);
        // A repeated last chunk (lost acknowledgement) is harmless.
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
        // Interrupted in the middle of block 2 (power loss: the RAM state is gone).
        let mut o = 0;
        while o < 2 * 4096 + 1000 {
            o = r.chunk(&mut f, o, &img[o as usize..o as usize + 200]).unwrap();
        }
        let mut r = Receiver::new();
        assert_eq!(r.begin(&mut f, m, &pol, false), Ok(2 * 4096));
        assert_eq!(send_all(&mut r, &mut f, &img, 2 * 4096, 240), Ok(img.len() as u32));
        assert!(r.end().is_ok());
        assert_eq!(&f.data[..img.len()], &img[..]);
        // Begin again after a finished transfer: already verified.
        let mut r = Receiver::new();
        assert_eq!(r.begin(&mut f, m, &pol, false), Ok(img.len() as u32));
        assert!(r.end().is_ok());
        // A new manifest starts over.
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
        // Over again with correct data.
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
    fn release_policy_refuses_public_development_and_weak_keys() {
        assert!(!valid_release_key(&dev_public_key()));
        assert!(!valid_release_key(&[0; 32]));
        let private = SigningKey::from_bytes(&[42; 32]);
        assert!(valid_release_key(&private.verifying_key().to_bytes()));
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
", &sig), "changed file");
        assert!(!verify_release(&SigningKey::from_bytes(&[4; 32]).verifying_key().to_bytes(), sums, &sig));
    }
}
