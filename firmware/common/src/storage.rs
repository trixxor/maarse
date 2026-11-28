//! Flash storage shared by all boards (anything that implements `NorFlash`).
//!
//! Two areas:
//!
//! * The settings page (one erase sector, 4 KB):
//!   ```text
//!   0x000: "MRS1" + secret key (64; replaced by `SetIdentity`)
//!   0x080: "RCF1" + radio settings (16)
//!   0x100: "ADM1" + count (1) + administrators (4 × 16)
//!   0x180: "ROL1" + role (1: 0 = client, 1 = repeater, 2 = relay, 3 = anchor, 4 = auto)
//!   0x200: "REG1" + strict (1) + length (1) + region names (≤ 120, comma separated)
//!   0x300: "SHR1" + sharing settings (10, see `companion::Sharing`)
//!   0x320: "NAM1" + length (1) + display name (≤ 24)
//!   0x340: "WIF1" + flags (1: bit 0 enabled, bit 1 modem) + SSID (1 + 32) + password (1 + 64) + peer (1 + 64)
//!   0x400: "RNG1" + range test setting (15, see `rangetest::Config`), only when it should persist
//!   0x420: "MKY1" + Wi-Fi modem key (32, see `maarse_core::modemauth`)
//!   0x500: "CTL1" + count (1) + per-administrator remote counter ceilings (up to 4)
//!   0x560: "BPN1" + own Bluetooth code (u32 LE), only when one has been chosen
//!   0x568: "BBD1" + paired phones (180, see `maarse_core::blepair`)
//!   0x620: "MBX1" + weekly mail keys (241, see `node/mailkey.rs`)
//!   ```
//!   Without "SHR1" neither position nor telemetry is shared (the default).
//!   Only written the first time and when radio/administrators/role/regions change.
//!   Older flash without "ROL1" (erased = 0xFF) gives repeater; without "REG1"
//!   no regions (forward everything).
//!
//! * Sensor recipe and owners (one sector, 4 KB, optional:
//!   [`FlashSettings::with_sensor`]): `"SEN1"` + length + recipe at 0x000,
//!   `"OWN1"` + length + owners at 0x200, `"STA1"` + length + the state a
//!   deep sleep must keep at 0x560 (see `node/sensor.rs`).
//!   From 0x780: packets opened, for the replay guard (`node/guard.rs`): slots
//!   of 16 bytes (packet ID, proof), appended without erasing. When full, the
//!   sector is rewritten with the newest [`REPLAY_KEEP`].
//!
//! * Known contacts (one sector, 4 KB, optional: [`FlashSettings::with_known`]):
//!   `"KNW1"` at 0x000, then 63 slots of 64 bytes from 0x040, each a public
//!   identity (signing key | DH key) or erased (0xFF). A new contact goes into
//!   the first erased slot without erasing the sector; only when it is full is
//!   the sector erased and rewritten with the contacts known now. So a reboot
//!   does not forget who the node can decrypt messages from.
//!
//! * The mailbox (two sectors, 8 KB): entries are written one after another — a
//!   header word `[0xA5, status, length lo, length hi]`, recipient (16) and the
//!   encrypted message, padded to whole words. A delivered entry is marked by
//!   writing status = 0 (the same word is written at most twice, which NOR flash
//!   allows). If the area is full, the active entries are compacted via RAM.
//!
//! All reads and writes are aligned to 4 bytes.

use embedded_storage::nor_flash::NorFlash;
use rand_core::{CryptoRng, RngCore};
use maarse_core::Address;
use maarse_core::blepair::{BONDS_LEN, Bonds, is_own_pin};
use maarse_core::router::Role;
use maarse_core::companion::RadioConfig;
use maarse_crypto::Identity;
use crate::node::MAIL_KEYS_LEN;

const PAGE: u32 = 4096;
const ID_MAGIC: &[u8; 4] = b"MRS1";
const RADIO_MAGIC: &[u8; 4] = b"RCF1";
const RADIO_OFF: usize = 0x80;
const ADMIN_MAGIC: &[u8; 4] = b"ADM1";
const ADMIN_OFF: usize = 0x100;
const ROLE_MAGIC: &[u8; 4] = b"ROL1";
const ROLE_OFF: usize = 0x180;
const REGION_MAGIC: &[u8; 4] = b"REG1";
const REGION_OFF: usize = 0x200;
const REGION_MAX: usize = 120;
const SHARING_MAGIC: &[u8; 4] = b"SHR1";
const SHARING_OFF: usize = 0x300;
const NAME_MAGIC: &[u8; 4] = b"NAM1";
const NAME_OFF: usize = 0x320;
#[cfg(feature = "wifi")]
const WIFI_MAGIC: &[u8; 4] = b"WIF1";
const WIFI_OFF: usize = 0x340;
const WIFI_LEN: usize = 4 + 1 + 33 + 65 + 65;
const _: () = assert!(NAME_OFF + 5 + maarse_core::appdata::MAX_NAME <= WIFI_OFF);
const RANGE_MAGIC: &[u8; 4] = b"RNG1";
const RANGE_OFF: usize = 0x400;
const RANGE_LEN: usize = 4 + maarse_core::rangetest::Config::LEN;
#[cfg(feature = "wifi")]
const MODEM_KEY_MAGIC: &[u8; 4] = b"MKY1";
const MODEM_KEY_OFF: usize = 0x420;
const MODEM_KEY_LEN: usize = 4 + 32;
const _: () = assert!(RANGE_OFF + RANGE_LEN <= MODEM_KEY_OFF);
const SITE_MAGIC: &[u8; 4] = b"SIT1";
const SITE_OFF: usize = 0x450;
/// Magic, length, site bytes.
const SITE_LEN: usize = 4 + 1 + maarse_core::appdata::SITE_FIXED + maarse_core::appdata::MAX_OWNER;
const _: () = assert!(MODEM_KEY_OFF + MODEM_KEY_LEN <= SITE_OFF);
const _: () = assert!(WIFI_OFF + WIFI_LEN <= RANGE_OFF);
/// Used part of the page, rounded up to whole words.
const CONTROL_OFF: usize = 0x500;
const CONTROL_MAGIC: &[u8; 4] = b"CTL1";
const CONTROL_LEN: usize = 5 + 4 * 20;
const BLE_PIN_MAGIC: &[u8; 4] = b"BPN1";
const BLE_PIN_OFF: usize = 0x560;
const BLE_BONDS_MAGIC: &[u8; 4] = b"BBD1";
const BLE_BONDS_OFF: usize = 0x568;
const MAIL_KEYS_MAGIC: &[u8; 4] = b"MBX1";
const MAIL_KEYS_OFF: usize = 0x620;
const USED: usize = (MAIL_KEYS_OFF + 4 + MAIL_KEYS_LEN).div_ceil(4) * 4;
const _: () = assert!(BLE_BONDS_OFF + 4 + BONDS_LEN <= MAIL_KEYS_OFF);
const _: () = assert!(SITE_OFF + SITE_LEN <= CONTROL_OFF && USED <= PAGE as usize);
const _: () = assert!(CONTROL_OFF + CONTROL_LEN <= BLE_PIN_OFF && BLE_PIN_OFF + 8 <= BLE_BONDS_OFF);
const _: () = assert!(SHARING_OFF + 4 + maarse_core::companion::Sharing::LEN <= NAME_OFF);

/// The sensor sector: "SEN1" + length (u16) + recipe (at most 256) at 0x000,
/// "OWN1" + length (u16) + owners (at most 816) at 0x200.
const SENSOR_AREA: usize = 0x780;
#[cfg(feature = "sensor")]
const SEN_MAGIC: &[u8; 4] = b"SEN1";
#[cfg(feature = "sensor")]
const SEN_MAX: usize = 256;
#[cfg(feature = "sensor")]
const OWN_OFF: usize = 0x200;
#[cfg(feature = "sensor")]
const OWN_MAGIC: &[u8; 4] = b"OWN1";
#[cfg(feature = "sensor")]
const OWN_MAX: usize = 816;
#[cfg(feature = "sensor")]
const STA_OFF: usize = 0x560;
#[cfg(feature = "sensor")]
const STA_MAGIC: &[u8; 4] = b"STA1";
#[cfg(feature = "sensor")]
const STA_MAX: usize = 512;
#[cfg(feature = "sensor")]
const _: () = assert!(OWN_OFF + 6 + OWN_MAX <= STA_OFF && 6 + SEN_MAX <= OWN_OFF && STA_OFF + 6 + STA_MAX <= SENSOR_AREA);

const REPLAY_SLOT: usize = 16;
const REPLAY_SLOTS: usize = (PAGE as usize - SENSOR_AREA) / REPLAY_SLOT;
/// Newest packets kept when the replay slots are full.
const REPLAY_KEEP: usize = 24;

const KNOWN_MAGIC: &[u8; 4] = b"KNW1";
const KNOWN_FIRST: u32 = 0x40;
const KNOWN_SLOT: u32 = 64;
/// Contacts kept in flash.
pub const KNOWN_SLOTS: u32 = (PAGE - KNOWN_FIRST) / KNOWN_SLOT;

const MAIL_MAGIC: u8 = 0xA5;
/// Largest message in the mailbox.
pub const MAIL_MAX: usize = 256;
/// The compaction buffer must be at least as large as the mailbox area.
pub const MAIL_AREA: usize = 8192;

fn rec_len(msg: usize) -> u32 {
    (4 + 16 + msg).div_ceil(4) as u32 * 4
}

/// Settings and mailbox in flash.
///
/// * `cfg`: the address of the settings page (4 KB, aligned).
/// * `mail`: the start of the mailbox area (8 KB, aligned).
pub struct FlashSettings<F: NorFlash + 'static> {
    pub flash: F,
    cfg: u32,
    mail: u32,
    buf: &'static mut [u8; MAIL_AREA],
    ota: Option<OtaConfig<F>>,
    /// The known contacts sector (4 KB, aligned), if the board has one.
    known: Option<u32>,
    /// The sensor sector (4 KB, aligned), if the board has one.
    sensor: Option<u32>,
}

/// Firmware update: where the staging area and the meta page are (absolute
/// flash addresses, 4 KB aligned) and how the board installs.
pub struct OtaConfig<F> {
    pub board: u16,
    pub staging: u32,
    pub capacity: u32,
    pub meta: u32,
    /// Installs the verified image and restarts; only returns
    /// on error.
    pub install: fn(&mut F, &maarse_crypto::ota::Manifest, &OtaConfig<F>) -> &'static str,
}

impl<F: NorFlash> FlashSettings<F> {
    pub fn new(flash: F, cfg: u32, mail: u32, buf: &'static mut [u8; MAIL_AREA]) -> Self {
        Self {
            flash,
            cfg,
            mail,
            buf,
            ota: None,
            known: None,
            sensor: None,
        }
    }

    /// Keeps the sensor recipe and the owners in the 4 KB sector at `addr`
    /// (see the module docs).
    pub fn with_sensor(mut self, addr: u32) -> Self {
        self.sensor = Some(addr);
        self
    }

    #[cfg(feature = "sensor")]
    /// Reads the used part of the sensor sector into `self.buf`.
    fn sensor_read(&mut self) -> Option<u32> {
        let a = self.sensor?;
        self.flash.read(a, &mut self.buf[..SENSOR_AREA]).ok()?;
        Some(a)
    }

    #[cfg(feature = "sensor")]
    /// The bytes stored at `off` behind `magic` and a length, if any.
    fn sensor_get<const N: usize>(&mut self, off: usize, magic: &[u8; 4]) -> heapless::Vec<u8, N> {
        let mut v = heapless::Vec::new();
        if self.sensor_read().is_none() {
            return v;
        }
        let b = &self.buf[off..];
        if &b[..4] == magic {
            let n = u16::from_le_bytes([b[4], b[5]]) as usize;
            if n <= N && 6 + n <= SENSOR_AREA - off {
                let _ = v.extend_from_slice(&b[6..6 + n]);
            }
        }
        v
    }

    #[cfg(feature = "sensor")]
    /// Stores `data` at `off` behind `magic` and a length; rewrites the sector.
    fn sensor_put(&mut self, off: usize, magic: &[u8; 4], max: usize, data: &[u8]) -> bool {
        if data.len() > max {
            return false;
        }
        let Some(a) = self.sensor else { return false };
        if self.flash.read(a, &mut self.buf[..PAGE as usize]).is_err() {
            return false;
        }
        let b = &mut self.buf[..SENSOR_AREA];
        b[off..off + 4].copy_from_slice(magic);
        b[off + 4..off + 6].copy_from_slice(&(data.len() as u16).to_le_bytes());
        b[off + 6..off + 6 + data.len()].copy_from_slice(data);
        self.sector_rewrite(a)
    }

    /// Erases the sector at `a` and writes the first page of `self.buf` to it,
    /// without the erased end (those words stay free for appending).
    fn sector_rewrite(&mut self, a: u32) -> bool {
        let used = self.buf[..PAGE as usize].iter().rposition(|&b| b != 0xFF).map_or(0, |i| i + 1).div_ceil(4) * 4;
        if self.flash.erase(a, a + PAGE).is_err() {
            return false;
        }
        used == 0 || self.flash.write(a, &self.buf[..used]).is_ok()
    }

    /// Keeps known contacts in the 4 KB sector at `addr` (see the module docs).
    pub fn with_known(mut self, addr: u32) -> Self {
        self.known = Some(addr);
        self
    }

    fn known_start(&mut self) -> Option<u32> {
        let a = self.known?;
        let mut m = [0u8; 4];
        if self.flash.read(a, &mut m).is_err() {
            return None;
        }
        if &m != KNOWN_MAGIC {
            let _ = self.flash.erase(a, a + PAGE);
            self.flash.write(a, KNOWN_MAGIC).ok()?;
        }
        Some(a)
    }

    /// Enables firmware updates (see [`OtaConfig`]).
    pub fn with_ota(mut self, ota: OtaConfig<F>) -> Self {
        self.ota = Some(ota);
        self
    }

    fn mail_end(&self) -> u32 {
        self.mail + MAIL_AREA as u32
    }

    fn read_page(&mut self) -> [u8; USED] {
        let mut buf = [0xFFu8; USED];
        if self.flash.read(self.cfg, &mut buf).is_err() {
            buf.fill(0xFF);
        }
        buf
    }

    fn write_page(&mut self, buf: &[u8; USED]) {
        let _ = self.flash.erase(self.cfg, self.cfg + PAGE);
        let _ = self.flash.write(self.cfg, buf);
    }

    pub fn load_or_create_identity(&mut self, rng: &mut (impl RngCore + CryptoRng)) -> Identity {
        let mut buf = self.read_page();
        if &buf[..4] == ID_MAGIC {
            let mut key = [0u8; 64];
            key.copy_from_slice(&buf[4..68]);
            return Identity::from_secret_bytes(&key);
        }
        let id = Identity::generate(rng);
        // Without an identity nothing in the page is ours. What another firmware left there is not kept: in the
        // place of the remote command counters it would block every remote change.
        buf.fill(0xFF);
        buf[..4].copy_from_slice(ID_MAGIC);
        buf[4..68].copy_from_slice(&id.to_secret_bytes());
        self.write_page(&buf);
        id
    }

    /// Stored radio settings, otherwise `default`.
    pub fn load_radio(&mut self, default: RadioConfig) -> RadioConfig {
        let buf = self.read_page();
        if &buf[RADIO_OFF..RADIO_OFF + 4] == RADIO_MAGIC
            && let Some(c) = RadioConfig::from_bytes(&buf[RADIO_OFF + 4..])
            && c.validate().is_ok()
        {
            return c;
        }
        default
    }

    fn scan(&mut self, f: impl FnMut(&mut F, u32, bool, Address, usize)) -> u32 {
        let end = self.mail_end();
        scan(&mut self.flash, self.mail, end, f)
    }

    /// Moves the active entries together at the start of the area.
    fn compact(&mut self) {
        let (start, end) = (self.mail, self.mail_end());
        let Self { flash, buf, .. } = self;
        buf.fill(0xFF);
        let mut used = 0usize;
        scan(flash, start, end, |fl, a, active, _, len| {
            let n = rec_len(len) as usize;
            if active && used + n <= buf.len() {
                let _ = fl.read(a, &mut buf[used..used + n]);
                used += n;
            }
        });
        let _ = flash.erase(start, end);
        if used > 0 {
            let _ = flash.write(start, &buf[..used]);
        }
    }
}

/// Walks through the entries: (flash, address, active, recipient, message length).
/// Returns the address right after the last entry.
fn scan<F: NorFlash>(
    flash: &mut F,
    start: u32,
    end: u32,
    mut f: impl FnMut(&mut F, u32, bool, Address, usize),
) -> u32 {
    let mut a = start;
    while a + 20 <= end {
        let mut h = [0u8; 20];
        if flash.read(a, &mut h).is_err() || h[0] != MAIL_MAGIC {
            break;
        }
        let len = u16::from_le_bytes([h[2], h[3]]) as usize;
        if len > MAIL_MAX || a + rec_len(len) > end {
            break; // corrupt entry: the rest is ignored (compacted away later)
        }
        let r = Address::from_slice(&h[4..20]).unwrap_or_default();
        f(flash, a, h[1] == 0xFF, r, len);
        a += rec_len(len);
    }
    a
}

impl<F: NorFlash> maarse_crypto::ota::Staging for FlashSettings<F> {
    fn erase_block(&mut self, offset: u32) -> Result<(), ()> {
        let o = self.ota.as_ref().ok_or(())?;
        if offset + PAGE > o.capacity {
            return Err(());
        }
        let a = o.staging + offset;
        self.flash.erase(a, a + PAGE).map_err(|_| ())
    }
    fn write(&mut self, offset: u32, data: &[u8]) -> Result<(), ()> {
        let o = self.ota.as_ref().ok_or(())?;
        if offset + data.len() as u32 > o.capacity {
            return Err(());
        }
        let a = o.staging + offset;
        self.flash.write(a, data).map_err(|_| ())
    }
    fn read(&mut self, offset: u32, buf: &mut [u8]) -> Result<(), ()> {
        let o = self.ota.as_ref().ok_or(())?;
        if offset + buf.len() as u32 > o.capacity {
            return Err(());
        }
        let a = o.staging + offset;
        self.flash.read(a, buf).map_err(|_| ())
    }
    fn erase_meta(&mut self) -> Result<(), ()> {
        let a = self.ota.as_ref().ok_or(())?.meta;
        self.flash.erase(a, a + PAGE).map_err(|_| ())
    }
    fn write_meta(&mut self, offset: u32, data: &[u8]) -> Result<(), ()> {
        let a = self.ota.as_ref().ok_or(())?.meta;
        if offset + data.len() as u32 > PAGE {
            return Err(());
        }
        self.flash.write(a + offset, data).map_err(|_| ())
    }
    fn read_meta(&mut self, offset: u32, buf: &mut [u8]) -> Result<(), ()> {
        let a = self.ota.as_ref().ok_or(())?.meta;
        if offset + buf.len() as u32 > PAGE {
            return Err(());
        }
        self.flash.read(a + offset, buf).map_err(|_| ())
    }
}

impl<F: NorFlash> crate::ota::OtaPort for FlashSettings<F> {
    fn board(&self) -> u16 {
        self.ota.as_ref().map_or(0, |o| o.board)
    }
    fn capacity(&self) -> u32 {
        self.ota.as_ref().map_or(0, |o| o.capacity)
    }
    fn install(&mut self, m: &maarse_crypto::ota::Manifest) -> &'static str {
        let Some(o) = self.ota.take() else {
            return "Updates are not enabled";
        };
        let err = (o.install)(&mut self.flash, m, &o);
        self.ota = Some(o);
        err
    }
}

impl<F: NorFlash> crate::node::Settings for FlashSettings<F> {
    fn ota(&mut self) -> Option<&mut dyn crate::ota::OtaPort> {
        if self.ota.is_some() { Some(self) } else { None }
    }

    fn save_identity(&mut self, secret: &[u8; 64]) -> bool {
        let mut buf = self.read_page();
        buf[..4].copy_from_slice(ID_MAGIC);
        buf[4..68].copy_from_slice(secret);
        self.write_page(&buf);
        // Read back: the page is erased and written, so check that it succeeded.
        let back = self.read_page();
        back[..4] == *ID_MAGIC && back[4..68] == secret[..]
    }

    fn save_radio(&mut self, cfg: &RadioConfig) {
        let mut buf = self.read_page();
        buf[RADIO_OFF..RADIO_OFF + 4].copy_from_slice(RADIO_MAGIC);
        buf[RADIO_OFF + 4..RADIO_OFF + 4 + RadioConfig::LEN].copy_from_slice(&cfg.to_bytes());
        self.write_page(&buf);
    }

    fn save_role(&mut self, role: u8) {
        let mut buf = self.read_page();
        buf[ROLE_OFF..ROLE_OFF + 4].copy_from_slice(ROLE_MAGIC);
        buf[ROLE_OFF + 4] = role;
        self.write_page(&buf);
    }

    fn load_role(&mut self) -> u8 {
        let buf = self.read_page();
        if &buf[ROLE_OFF..ROLE_OFF + 4] == ROLE_MAGIC && buf[ROLE_OFF + 4] <= crate::node::ROLE_AUTO {
            return buf[ROLE_OFF + 4];
        }
        Role::Repeater.as_u8()
    }

    fn save_regions(&mut self, strict: bool, names: &[u8]) {
        let mut buf = self.read_page();
        let n = names.len().min(REGION_MAX);
        buf[REGION_OFF..REGION_OFF + 4].copy_from_slice(REGION_MAGIC);
        buf[REGION_OFF + 4] = strict as u8;
        buf[REGION_OFF + 5] = n as u8;
        buf[REGION_OFF + 6..REGION_OFF + 6 + n].copy_from_slice(&names[..n]);
        self.write_page(&buf);
    }

    fn load_regions(&mut self) -> (bool, crate::node::RegionNames) {
        let buf = self.read_page();
        if &buf[REGION_OFF..REGION_OFF + 4] != REGION_MAGIC {
            return (false, crate::node::RegionNames::new());
        }
        let n = (buf[REGION_OFF + 5] as usize).min(REGION_MAX);
        let names = crate::node::RegionNames::from_slice(&buf[REGION_OFF + 6..REGION_OFF + 6 + n])
            .unwrap_or_default();
        (buf[REGION_OFF + 4] == 1, names)
    }

    fn save_range(&mut self, cfg: &maarse_core::rangetest::Config) {
        let mut buf = self.read_page();
        buf[RANGE_OFF..RANGE_OFF + 4].copy_from_slice(RANGE_MAGIC);
        buf[RANGE_OFF + 4..RANGE_OFF + RANGE_LEN].copy_from_slice(&cfg.to_bytes());
        self.write_page(&buf);
    }

    fn load_range(&mut self) -> maarse_core::rangetest::Config {
        let buf = self.read_page();
        if &buf[RANGE_OFF..RANGE_OFF + 4] != RANGE_MAGIC {
            return Default::default();
        }
        maarse_core::rangetest::Config::from_bytes(&buf[RANGE_OFF + 4..RANGE_OFF + RANGE_LEN])
            .and_then(|c| c.sanitized().ok())
            .unwrap_or_default()
    }

    fn save_sharing(&mut self, s: &maarse_core::companion::Sharing) {
        let mut buf = self.read_page();
        buf[SHARING_OFF..SHARING_OFF + 4].copy_from_slice(SHARING_MAGIC);
        buf[SHARING_OFF + 4..SHARING_OFF + 14].copy_from_slice(&s.to_bytes());
        self.write_page(&buf);
    }

    fn load_sharing(&mut self) -> maarse_core::companion::Sharing {
        let buf = self.read_page();
        if &buf[SHARING_OFF..SHARING_OFF + 4] == SHARING_MAGIC {
            maarse_core::companion::Sharing::from_bytes(&buf[SHARING_OFF + 4..]).unwrap_or_default()
        } else {
            Default::default()
        }
    }

    fn save_name(&mut self, name: &[u8]) {
        let mut buf = self.read_page();
        let n = name.len().min(maarse_core::appdata::MAX_NAME);
        buf[NAME_OFF..NAME_OFF + 4].copy_from_slice(NAME_MAGIC);
        buf[NAME_OFF + 4] = n as u8;
        buf[NAME_OFF + 5..NAME_OFF + 5 + n].copy_from_slice(&name[..n]);
        self.write_page(&buf);
    }

    #[cfg(feature = "wifi")]
    fn save_wifi(&mut self, w: &maarse_core::companion::WifiSettings) {
        let mut buf = self.read_page();
        let b = &mut buf[WIFI_OFF..WIFI_OFF + WIFI_LEN];
        b.fill(0);
        b[..4].copy_from_slice(WIFI_MAGIC);
        b[4] = w.enabled as u8 | (w.modem as u8) << 1 | (w.hotspot as u8) << 2;
        let mut put = |at: usize, s: &[u8]| {
            b[at] = s.len() as u8;
            b[at + 1..at + 1 + s.len()].copy_from_slice(s);
        };
        put(5, &w.ssid);
        put(5 + 33, &w.password);
        put(5 + 33 + 65, &w.peer);
        self.write_page(&buf);
    }
    #[cfg(feature = "wifi")]
    fn load_wifi(&mut self) -> maarse_core::companion::WifiSettings {
        let buf = self.read_page();
        let b = &buf[WIFI_OFF..WIFI_OFF + WIFI_LEN];
        if &b[..4] != WIFI_MAGIC {
            return Default::default();
        }
        fn get<const N: usize>(b: &[u8], at: usize) -> heapless::Vec<u8, N> {
            let n = (b[at] as usize).min(N);
            heapless::Vec::from_slice(&b[at + 1..at + 1 + n]).unwrap_or_default()
        }
        maarse_core::companion::WifiSettings {
            enabled: b[4] & 1 != 0,
            modem: b[4] & 2 != 0,
            hotspot: b[4] & 4 != 0,
            ssid: get(b, 5),
            password: get(b, 5 + 33),
            peer: get(b, 5 + 33 + 65),
            modem_key: self.load_modem_key(),
        }
    }
    fn save_ble_pin(&mut self, pin: Option<u32>) {
        let mut buf = self.read_page();
        let b = &mut buf[BLE_PIN_OFF..BLE_PIN_OFF + 8];
        b.fill(0xFF);
        if let Some(p) = pin {
            b[..4].copy_from_slice(BLE_PIN_MAGIC);
            b[4..].copy_from_slice(&p.to_le_bytes());
        }
        self.write_page(&buf);
    }
    fn load_ble_pin(&mut self) -> Option<u32> {
        let buf = self.read_page();
        let b = &buf[BLE_PIN_OFF..BLE_PIN_OFF + 8];
        (&b[..4] == BLE_PIN_MAGIC).then(|| u32::from_le_bytes([b[4], b[5], b[6], b[7]])).filter(|p| is_own_pin(*p))
    }
    fn save_ble_bonds(&mut self, bonds: &Bonds) {
        let mut buf = self.read_page();
        buf[BLE_BONDS_OFF..BLE_BONDS_OFF + 4].copy_from_slice(BLE_BONDS_MAGIC);
        buf[BLE_BONDS_OFF + 4..BLE_BONDS_OFF + 4 + BONDS_LEN].copy_from_slice(&bonds.to_bytes());
        self.write_page(&buf);
    }
    fn load_ble_bonds(&mut self) -> Bonds {
        let buf = self.read_page();
        if &buf[BLE_BONDS_OFF..BLE_BONDS_OFF + 4] != BLE_BONDS_MAGIC {
            return Bonds::new();
        }
        Bonds::from_bytes(&buf[BLE_BONDS_OFF + 4..BLE_BONDS_OFF + 4 + BONDS_LEN])
    }
    #[cfg(feature = "wifi")]
    fn save_modem_key(&mut self, key: Option<[u8; 32]>) {
        let mut buf = self.read_page();
        let b = &mut buf[MODEM_KEY_OFF..MODEM_KEY_OFF + MODEM_KEY_LEN];
        b.fill(0xFF);
        if let Some(k) = key {
            b[..4].copy_from_slice(MODEM_KEY_MAGIC);
            b[4..].copy_from_slice(&k);
        }
        self.write_page(&buf);
    }
    #[cfg(feature = "wifi")]
    fn load_modem_key(&mut self) -> Option<[u8; 32]> {
        let buf = self.read_page();
        let b = &buf[MODEM_KEY_OFF..MODEM_KEY_OFF + MODEM_KEY_LEN];
        (&b[..4] == MODEM_KEY_MAGIC).then(|| b[4..].try_into().unwrap())
    }
    fn save_site(&mut self, site: &maarse_core::appdata::Site) {
        let mut buf = self.read_page();
        let b = &mut buf[SITE_OFF..SITE_OFF + SITE_LEN];
        b.fill(0);
        b[..4].copy_from_slice(SITE_MAGIC);
        let w = site.to_bytes();
        b[4] = w.len() as u8;
        b[5..5 + w.len()].copy_from_slice(&w);
        self.write_page(&buf);
    }
    fn load_site(&mut self) -> maarse_core::appdata::Site {
        let buf = self.read_page();
        let b = &buf[SITE_OFF..SITE_OFF + SITE_LEN];
        if &b[..4] != SITE_MAGIC {
            return Default::default();
        }
        let n = (b[4] as usize).min(SITE_LEN - 5);
        maarse_core::appdata::Site::from_bytes(&b[5..5 + n]).unwrap_or_default()
    }
    fn load_name(&mut self) -> maarse_core::appdata::Name {
        let buf = self.read_page();
        if &buf[NAME_OFF..NAME_OFF + 4] != NAME_MAGIC {
            return Default::default();
        }
        let n = (buf[NAME_OFF + 4] as usize).min(maarse_core::appdata::MAX_NAME);
        maarse_core::appdata::clean_name(&buf[NAME_OFF + 5..NAME_OFF + 5 + n]).unwrap_or_default()
    }

    fn save_mail_keys(&mut self, keys: &[u8; MAIL_KEYS_LEN]) -> bool {
        let mut buf = self.read_page();
        buf[MAIL_KEYS_OFF..MAIL_KEYS_OFF + 4].copy_from_slice(MAIL_KEYS_MAGIC);
        buf[MAIL_KEYS_OFF + 4..MAIL_KEYS_OFF + 4 + MAIL_KEYS_LEN].copy_from_slice(keys);
        self.write_page(&buf);
        self.read_page() == buf
    }

    fn load_mail_keys(&mut self) -> [u8; MAIL_KEYS_LEN] {
        let buf = self.read_page();
        let mut keys = [0u8; MAIL_KEYS_LEN];
        if &buf[MAIL_KEYS_OFF..MAIL_KEYS_OFF + 4] == MAIL_KEYS_MAGIC {
            keys.copy_from_slice(&buf[MAIL_KEYS_OFF + 4..MAIL_KEYS_OFF + 4 + MAIL_KEYS_LEN]);
        }
        keys
    }

    fn replay_add(&mut self, id: u64, proof: &[u8; 8]) {
        let Some(a) = self.sensor else { return };
        let mut entry = [0u8; REPLAY_SLOT];
        entry[..8].copy_from_slice(&id.to_le_bytes());
        entry[8..].copy_from_slice(proof);
        for k in 0..REPLAY_SLOTS {
            let at = a + (SENSOR_AREA + k * REPLAY_SLOT) as u32;
            let mut slot = [0u8; REPLAY_SLOT];
            if self.flash.read(at, &mut slot).is_err() {
                return;
            }
            if slot == [0xFF; REPLAY_SLOT] {
                let _ = self.flash.write(at, &entry);
                return;
            }
        }
        // Full: keep the newest and the new one, erase the rest.
        if self.flash.read(a, &mut self.buf[..PAGE as usize]).is_err() {
            return;
        }
        let end = SENSOR_AREA + REPLAY_SLOTS * REPLAY_SLOT;
        let keep = (REPLAY_KEEP - 1) * REPLAY_SLOT;
        self.buf.copy_within(end - keep..end, SENSOR_AREA);
        self.buf[SENSOR_AREA + keep..SENSOR_AREA + keep + REPLAY_SLOT].copy_from_slice(&entry);
        self.buf[SENSOR_AREA + keep + REPLAY_SLOT..PAGE as usize].fill(0xFF);
        self.sector_rewrite(a);
    }

    fn replay_load(&mut self, f: &mut dyn FnMut(u64, [u8; 8])) {
        let Some(a) = self.sensor else { return };
        for k in 0..REPLAY_SLOTS {
            let mut slot = [0u8; REPLAY_SLOT];
            if self.flash.read(a + (SENSOR_AREA + k * REPLAY_SLOT) as u32, &mut slot).is_err() || slot == [0xFF; REPLAY_SLOT] {
                return;
            }
            let mut id = [0u8; 8];
            id.copy_from_slice(&slot[..8]);
            let mut proof = [0u8; 8];
            proof.copy_from_slice(&slot[8..]);
            f(u64::from_le_bytes(id), proof);
        }
    }

    fn save_admins(&mut self, admins: &[Address]) {

        let mut buf = self.read_page();
        buf[ADMIN_OFF..ADMIN_OFF + 4].copy_from_slice(ADMIN_MAGIC);
        let n = admins.len().min(4);
        buf[ADMIN_OFF + 4] = n as u8;
        for (i, a) in admins.iter().take(4).enumerate() {
            let o = ADMIN_OFF + 5 + 16 * i;
            buf[o..o + 16].copy_from_slice(&a.0);
        }
        self.write_page(&buf);
    }

    fn load_admins(&mut self) -> heapless::Vec<Address, 4> {
        let buf = self.read_page();
        let mut v = heapless::Vec::new();
        if &buf[ADMIN_OFF..ADMIN_OFF + 4] == ADMIN_MAGIC {
            for i in 0..(buf[ADMIN_OFF + 4] as usize).min(4) {
                let o = ADMIN_OFF + 5 + 16 * i;
                if let Some(a) = Address::from_slice(&buf[o..o + 16]) {
                    let _ = v.push(a);
                }
            }
        }
        v
    }

    #[cfg(feature = "sensor")]
    fn save_sensor_recipe(&mut self, recipe: &[u8]) -> bool {
        self.sensor_put(0, SEN_MAGIC, SEN_MAX, recipe)
    }

    #[cfg(feature = "sensor")]
    fn load_sensor_recipe(&mut self) -> heapless::Vec<u8, 256> {
        self.sensor_get(0, SEN_MAGIC)
    }

    #[cfg(feature = "sensor")]
    fn save_sensor_state(&mut self, state: &[u8]) -> bool {
        self.sensor_put(STA_OFF, STA_MAGIC, STA_MAX, state)
    }

    #[cfg(feature = "sensor")]
    fn load_sensor_state(&mut self) -> heapless::Vec<u8, 512> {
        self.sensor_get(STA_OFF, STA_MAGIC)
    }

    #[cfg(feature = "sensor")]
    fn save_owners(&mut self, owners: &[u8]) -> bool {
        self.sensor_put(OWN_OFF, OWN_MAGIC, OWN_MAX, owners)
    }

    #[cfg(feature = "sensor")]
    fn load_owners(&mut self) -> heapless::Vec<u8, 816> {
        self.sensor_get(OWN_OFF, OWN_MAGIC)
    }

    fn load_control_limits(&mut self) -> Option<heapless::Vec<(Address, u32), 4>> {
        let mut buf = [0xff; CONTROL_LEN.div_ceil(4) * 4];
        self.flash.read(self.cfg + CONTROL_OFF as u32, &mut buf).ok()?;
        let mut limits = heapless::Vec::new();
        if &buf[..4] == CONTROL_MAGIC {
            let count = usize::from(buf[4]);
            if count > 4 { return None; }
            for record in buf[5..5 + count * 20].chunks_exact(20) {
                let address = Address::from_slice(&record[..16]).unwrap();
                let counter = u32::from_le_bytes(record[16..20].try_into().unwrap());
                let _ = limits.push((address, counter));
            }
        } else if buf.iter().any(|byte| *byte != 0xff) {
            return None;
        }
        Some(limits)
    }

    fn save_control_limits(&mut self, limits: &[(Address, u32)]) -> bool {
        if limits.len() > 4 {
            return false;
        }
        let mut buf = [0xff; USED];
        if self.flash.read(self.cfg, &mut buf).is_err() {
            return false;
        }
        let stored = &mut buf[CONTROL_OFF..CONTROL_OFF + CONTROL_LEN];
        stored.fill(0xff);
        stored[..4].copy_from_slice(CONTROL_MAGIC);
        stored[4] = limits.len() as u8;
        for ((address, counter), record) in limits.iter().zip(stored[5..].chunks_exact_mut(20)) {
            record[..16].copy_from_slice(&address.0);
            record[16..20].copy_from_slice(&counter.to_le_bytes());
        }
        self.flash.erase(self.cfg, self.cfg + PAGE).is_ok()
            && self.flash.write(self.cfg, &buf).is_ok()
            && self.read_page() == buf
    }

    fn known_load(&mut self, f: &mut dyn FnMut(&[u8; 64])) {
        let Some(a) = self.known else { return };
        let mut m = [0u8; 4];
        if self.flash.read(a, &mut m).is_err() || &m != KNOWN_MAGIC {
            return;
        }
        let mut slot = [0u8; 64];
        for k in 0..KNOWN_SLOTS {
            if self.flash.read(a + KNOWN_FIRST + k * KNOWN_SLOT, &mut slot).is_ok() && slot.iter().any(|&b| b != 0xFF) {
                f(&slot);
            }
        }
    }

    fn known_add(&mut self, id: &[u8; 64]) -> bool {
        let Some(a) = self.known_start() else { return false };
        let mut slot = [0u8; 64];
        for k in 0..KNOWN_SLOTS {
            let at = a + KNOWN_FIRST + k * KNOWN_SLOT;
            if self.flash.read(at, &mut slot).is_err() {
                return false;
            }
            if slot == *id {
                return true;
            }
            if slot.iter().all(|&b| b == 0xFF) {
                return self.flash.write(at, id).is_ok();
            }
        }
        false
    }

    fn known_reset(&mut self, ids: &mut dyn Iterator<Item = [u8; 64]>) {
        let Some(a) = self.known else { return };
        if self.flash.erase(a, a + PAGE).is_err() || self.flash.write(a, KNOWN_MAGIC).is_err() {
            return;
        }
        for (k, id) in ids.take(KNOWN_SLOTS as usize).enumerate() {
            let _ = self.flash.write(a + KNOWN_FIRST + k as u32 * KNOWN_SLOT, &id);
        }
    }

    fn mail_add(&mut self, recipient: &Address, sealed: &[u8]) -> Option<u32> {
        if sealed.len() > MAIL_MAX {
            return None;
        }
        let n = rec_len(sealed.len());
        let mut end = self.scan(|_, _, _, _, _| {});
        if end + n > self.mail_end() {
            self.compact();
            end = self.scan(|_, _, _, _, _| {});
            if end + n > self.mail_end() {
                return None;
            }
        }
        let mut rec = [0xFFu8; 4 + 16 + MAIL_MAX];
        rec[0] = MAIL_MAGIC;
        rec[1] = 0xFF;
        rec[2..4].copy_from_slice(&(sealed.len() as u16).to_le_bytes());
        rec[4..20].copy_from_slice(&recipient.0);
        rec[20..20 + sealed.len()].copy_from_slice(sealed);
        self.flash.write(end, &rec[..n as usize]).ok()?;
        Some(end)
    }

    fn mail_for(&mut self, recipient: &Address, f: &mut dyn FnMut(u32, &[u8])) {
        let mut buf = [0u8; MAIL_MAX];
        let want = *recipient;
        self.scan(|fl, a, active, r, len| {
            if active && r == want {
                let _ = fl.read(a + 20, &mut buf[..len.div_ceil(4) * 4]);
                f(a, &buf[..len]);
            }
        });
    }

    fn mail_remove(&mut self, id: u32) {
        if id < self.mail || id >= self.mail_end() {
            return;
        }
        let mut h = [0u8; 4];
        if self.flash.read(id, &mut h).is_ok() && h[0] == MAIL_MAGIC && h[1] == 0xFF {
            h[1] = 0;
            let _ = self.flash.write(id, &h);
        }
    }

    fn mail_count(&mut self) -> (u32, u32) {
        let mut seen: heapless::Vec<Address, 32> = heapless::Vec::new();
        let mut msgs = 0;
        self.scan(|_, _, active, r, _| {
            if active {
                msgs += 1;
                if !seen.contains(&r) {
                    let _ = seen.push(r);
                }
            }
        });
        (seen.len() as u32, msgs)
    }
}

// Run on a PC (the radio driver patch is the one the firmware builds use):
//   cargo test --lib --config 'patch.crates-io.lora-phy.path="../vendor/lora-phy"'
#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use crate::node::Settings as _;
    use embedded_storage::nor_flash::{ErrorType, NorFlashError, NorFlashErrorKind, ReadNorFlash};

    const SIZE: usize = PAGE as usize + MAIL_AREA;

    struct Ram(std::boxed::Box<[u8; SIZE]>);

    #[derive(Debug)]
    struct Refused;
    impl NorFlashError for Refused {
        fn kind(&self) -> NorFlashErrorKind {
            NorFlashErrorKind::Other
        }
    }
    impl ErrorType for Ram {
        type Error = Refused;
    }
    impl ReadNorFlash for Ram {
        const READ_SIZE: usize = 1;
        fn read(&mut self, at: u32, out: &mut [u8]) -> Result<(), Refused> {
            out.copy_from_slice(&self.0[at as usize..at as usize + out.len()]);
            Ok(())
        }
        fn capacity(&self) -> usize {
            SIZE
        }
    }
    impl NorFlash for Ram {
        const WRITE_SIZE: usize = 4;
        const ERASE_SIZE: usize = PAGE as usize;
        fn erase(&mut self, from: u32, to: u32) -> Result<(), Refused> {
            self.0[from as usize..to as usize].fill(0xFF);
            Ok(())
        }
        fn write(&mut self, at: u32, data: &[u8]) -> Result<(), Refused> {
            for (cell, byte) in self.0[at as usize..].iter_mut().zip(data) {
                *cell &= byte;
            }
            Ok(())
        }
    }

    struct Counting(u8);
    impl RngCore for Counting {
        fn next_u32(&mut self) -> u32 {
            rand_core::impls::next_u32_via_fill(self)
        }
        fn next_u64(&mut self) -> u64 {
            rand_core::impls::next_u64_via_fill(self)
        }
        fn fill_bytes(&mut self, out: &mut [u8]) {
            for b in out {
                self.0 = self.0.wrapping_add(1);
                *b = self.0;
            }
        }
        fn try_fill_bytes(&mut self, out: &mut [u8]) -> Result<(), rand_core::Error> {
            self.fill_bytes(out);
            Ok(())
        }
    }
    impl CryptoRng for Counting {}

    /// A board whose settings page holds `leftover` everywhere, as after another firmware.
    fn board_after_other_firmware(leftover: u8) -> FlashSettings<Ram> {
        let mut ram = Ram(std::boxed::Box::new([0xFF; SIZE]));
        ram.0[..PAGE as usize].fill(leftover);
        FlashSettings::new(ram, 0, PAGE, std::boxed::Box::leak(std::boxed::Box::new([0xFF; MAIL_AREA])))
    }

    #[test]
    fn a_new_identity_does_not_keep_what_another_firmware_left_in_the_page() {
        let mut flash = board_after_other_firmware(0x5A);
        let id = flash.load_or_create_identity(&mut Counting(0));
        assert_eq!(flash.load_control_limits().map(|l| l.len()), Some(0), "remote commands can be used");
        assert!(flash.flash.0[68..PAGE as usize].iter().all(|&b| b == 0xFF));
        assert_eq!(flash.load_or_create_identity(&mut Counting(9)).address(), id.address(), "kept after a restart");
    }

    fn board_with_sensor_sector() -> FlashSettings<Ram> {
        // The settings page at 0 and the sensor sector right after it (the
        // mailbox area is not used here).
        board_after_other_firmware(0xFF).with_sensor(PAGE)
    }

    fn replays(flash: &mut FlashSettings<Ram>) -> std::vec::Vec<u64> {
        let mut v = std::vec::Vec::new();
        flash.replay_load(&mut |id, _| v.push(id));
        v
    }

    #[test]
    fn mail_keys_are_stored_next_to_the_other_settings() {
        let mut flash = board_with_sensor_sector();
        let id = flash.load_or_create_identity(&mut Counting(0));
        assert_eq!(flash.load_mail_keys(), [0; MAIL_KEYS_LEN]);
        let keys = [7u8; MAIL_KEYS_LEN];
        assert!(flash.save_mail_keys(&keys));
        assert!(flash.save_control_limits(&[]));
        flash.save_role(2);
        assert_eq!(flash.load_mail_keys(), keys);
        assert_eq!(flash.load_or_create_identity(&mut Counting(9)).address(), id.address());
    }

    #[test]
    fn replays_are_appended_kept_and_survive_sensor_writes() {
        let mut flash = board_with_sensor_sector();
        assert!(replays(&mut flash).is_empty());
        for i in 1..=3u64 {
            flash.replay_add(i, &[i as u8; 8]);
        }
        #[cfg(feature = "sensor")]
        assert!(flash.save_sensor_recipe(b"recipe"));
        assert_eq!(replays(&mut flash), [1, 2, 3]);
        flash.replay_add(4, &[4; 8]);
        assert_eq!(replays(&mut flash), [1, 2, 3, 4]);
        // Full: the newest are kept, and the sensor settings too.
        let total = REPLAY_SLOTS as u64 + 5;
        for i in 5..=total {
            flash.replay_add(i, &[1; 8]);
        }
        let v = replays(&mut flash);
        assert!(v.len() >= REPLAY_KEEP && v.len() < REPLAY_SLOTS, "{}", v.len());
        assert_eq!(v.last(), Some(&total));
        assert!(v.windows(2).all(|w| w[1] == w[0] + 1), "{v:?}");
        #[cfg(feature = "sensor")]
        assert_eq!(&flash.load_sensor_recipe()[..], b"recipe");
    }

    #[test]
    fn leftover_remote_counters_can_be_replaced_and_nothing_else_is_lost() {
        // A board that already has its identity, with leftovers where the counters belong.
        let mut flash = board_after_other_firmware(0xFF);
        let id = flash.load_or_create_identity(&mut Counting(0));
        flash.flash.0[CONTROL_OFF..CONTROL_OFF + CONTROL_LEN].fill(0x5A);
        assert!(flash.load_control_limits().is_none(), "unreadable counters are not trusted");
        assert!(flash.save_control_limits(&[]));
        assert_eq!(flash.load_control_limits().map(|l| l.len()), Some(0));
        assert_eq!(flash.load_or_create_identity(&mut Counting(9)).address(), id.address());
    }
}
