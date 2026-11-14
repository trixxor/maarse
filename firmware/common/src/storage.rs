//! Flash-lager fælles for alle boards (alt der implementerer `NorFlash`).
//!
//! To områder:
//!
//! * Indstillingssiden (én slettesektor, 4 KB):
//!   ```text
//!   0x000: "MRS1" + hemmelig nøgle (64)
//!   0x080: "RCF1" + radioindstillinger (16)
//!   0x100: "ADM1" + antal (1) + administratorer (4 × 16)
//!   ```
//!   Skrives kun første gang og når radio/administratorer ændres.
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
use maarse_core::companion::RadioConfig;
use maarse_crypto::Identity;

const PAGE: u32 = 4096;
const ID_MAGIC: &[u8; 4] = b"MRS1";
const RADIO_MAGIC: &[u8; 4] = b"RCF1";
const RADIO_OFF: usize = 0x80;
const ADMIN_MAGIC: &[u8; 4] = b"ADM1";
const ADMIN_OFF: usize = 0x100;
/// Brugt del af siden, rundet op til hele ord.
const USED: usize = (ADMIN_OFF + 4 + 1 + 4 * 16).div_ceil(4) * 4;

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
}

impl<F: NorFlash> FlashSettings<F> {
    pub fn new(flash: F, cfg: u32, mail: u32, buf: &'static mut [u8; MAIL_AREA]) -> Self {
        Self {
            flash,
            cfg,
            mail,
            buf,
        }
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

impl<F: NorFlash> crate::node::Settings for FlashSettings<F> {
    fn save_radio(&mut self, cfg: &RadioConfig) {
        let mut buf = self.read_page();
        buf[RADIO_OFF..RADIO_OFF + 4].copy_from_slice(RADIO_MAGIC);
        buf[RADIO_OFF + 4..RADIO_OFF + 4 + RadioConfig::LEN].copy_from_slice(&cfg.to_bytes());
        self.write_page(&buf);
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
