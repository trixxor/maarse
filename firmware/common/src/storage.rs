//! Flash-lager fælles for alle boards (alt der implementerer `NorFlash`).
//!
//! To områder:
//!
//! * Indstillingssiden (én slettesektor, 4 KB):
//!   ```text
//!   0x000: "MRS1" + hemmelig nøgle (64; erstattes af `SetIdentity`)
//!   0x080: "RCF1" + radioindstillinger (16)
//!   0x100: "ADM1" + antal (1) + administratorer (4 × 16)
//!   0x180: "ROL1" + rolle (1: 0 = klient, 1 = repeater, 2 = relay)
//!   0x200: "REG1" + strict (1) + længde (1) + regionsnavne (≤ 120, kommasepareret)
//!   0x300: "SHR1" + delingsindstillinger (10, se `companion::Sharing`)
//!   ```
//!   Uden "SHR1" deles hverken position eller telemetri (standard).
//!   Skrives kun første gang og når radio/administratorer/rolle/regioner ændres.
//!   Ældre flash uden "ROL1" (slettet = 0xFF) giver repeater; uden "REG1"
//!   ingen regioner (videresend alt).
//!
//! * Postkassen (to sektorer, 8 KB): poster skrives efter hinanden — et
//!   hovedord `[0xA5, status, længde lo, længde hi]`, modtager (16) og den
//!   krypterede besked, fyldt op til hele ord. En leveret post markeres ved at
//!   skrive status = 0 (samme ord skrives højst to gange, hvilket NOR-flash
//!   tillader). Er området fuldt, pakkes de aktive poster sammen via RAM.
//!
//! Alle læsninger og skrivninger er justeret til 4 bytes.

use embedded_storage::nor_flash::NorFlash;
use rand_core::{CryptoRng, RngCore};
use maarse_core::Address;
use maarse_core::router::Role;
use maarse_core::companion::RadioConfig;
use maarse_crypto::Identity;

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
/// Brugt del af siden, rundet op til hele ord.
const USED: usize = (SHARING_OFF + 4 + maarse_core::companion::Sharing::LEN).div_ceil(4) * 4;

const MAIL_MAGIC: u8 = 0xA5;
/// Største besked i postkassen.
pub const MAIL_MAX: usize = 256;
/// Bufferen til sammenpakning skal være mindst så stor som postkasseområdet.
pub const MAIL_AREA: usize = 8192;

fn rec_len(msg: usize) -> u32 {
    (4 + 16 + msg).div_ceil(4) as u32 * 4
}

/// Indstillinger og postkasse i flash.
///
/// * `cfg`: adressen på indstillingssiden (4 KB, justeret).
/// * `mail`: starten af postkasseområdet (8 KB, justeret).
pub struct FlashSettings<F: NorFlash + 'static> {
    pub flash: F,
    cfg: u32,
    mail: u32,
    buf: &'static mut [u8; MAIL_AREA],
    ota: Option<OtaConfig<F>>,
}

/// Firmwareopdatering: hvor mellemlageret og metasiden ligger (absolutte
/// flash-adresser, 4 KB-justeret) og hvordan boardet installerer.
pub struct OtaConfig<F> {
    pub board: u16,
    pub staging: u32,
    pub capacity: u32,
    pub meta: u32,
    /// Installerer det verificerede image og genstarter; returnerer kun
    /// ved fejl.
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
        }
    }

    /// Slår firmwareopdatering til (se [`OtaConfig`]).
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
        buf[..4].copy_from_slice(ID_MAGIC);
        buf[4..68].copy_from_slice(&id.to_secret_bytes());
        self.write_page(&buf);
        id
    }

    /// Gemte radioindstillinger, ellers `default`.
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

    /// Flytter de aktive poster sammen i starten af området.
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

/// Gennemløber posterne: (flash, adresse, aktiv, modtager, beskedlængde).
/// Returnerer adressen lige efter sidste post.
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
            break; // ødelagt post: resten ignoreres (pakkes væk senere)
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
            return "Opdatering er ikke slået til";
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
        // Læs tilbage: siden slettes og skrives, så tjek at det lykkedes.
        let back = self.read_page();
        back[..4] == *ID_MAGIC && back[4..68] == secret[..]
    }

    fn save_radio(&mut self, cfg: &RadioConfig) {
        let mut buf = self.read_page();
        buf[RADIO_OFF..RADIO_OFF + 4].copy_from_slice(RADIO_MAGIC);
        buf[RADIO_OFF + 4..RADIO_OFF + 4 + RadioConfig::LEN].copy_from_slice(&cfg.to_bytes());
        self.write_page(&buf);
    }

    fn save_role(&mut self, role: Role) {
        let mut buf = self.read_page();
        buf[ROLE_OFF..ROLE_OFF + 4].copy_from_slice(ROLE_MAGIC);
        buf[ROLE_OFF + 4] = role.as_u8();
        self.write_page(&buf);
    }

    fn load_role(&mut self) -> Role {
        let buf = self.read_page();
        if &buf[ROLE_OFF..ROLE_OFF + 4] == ROLE_MAGIC
            && let Some(r) = Role::from_u8(buf[ROLE_OFF + 4])
        {
            return r;
        }
        Role::Repeater
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
