//! Firmwareopdatering på noden: kommandoerne `OtaBegin`/`OtaChunk`/
//! `OtaEnd`/`OtaStatus` lokalt (USB/Bluetooth) og via `Remote` (kun
//! administratorer). Selve kontrollen (signatur, board, version, SHA-256,
//! genoptagelse) ligger i `maarse_crypto::ota`; boardet leverer mellemlager
//! og installation gennem [`OtaPort`].
//!
//! Udgivelsesnøglen bygges ind med `MAARSE_RELEASE_PUBKEY=<64 hex>` og
//! versionen med `MAARSE_FW_VERSION=x.y.z` (ellers crate-versionen). Uden
//! nøgle bruges den offentlige udviklingsnøgle - alle kan så signere
//! firmware til noden. Byg aldrig udgivelser sådan.

use embassy_time::{Duration, Instant};
use maarse_core::companion::{Command, Event, OTA_ALLOW_DOWNGRADE, ota_state, text};
use maarse_crypto::ota::{self, Manifest, OtaError, Policy, Receiver, Staging, State};

/// Boardets del: mellemlager (se [`Staging`]) og installation.
pub trait OtaPort: Staging {
    fn board(&self) -> u16;
    /// Mellemlagerets størrelse i bytes.
    fn capacity(&self) -> u32;
    /// Installerer det verificerede image i mellemlageret og genstarter.
    /// Vender kun tilbage ved fejl.
    fn install(&mut self, m: &Manifest) -> &'static str;
}

/// Udgivelsesnøglen, firmwaren stoler på.
pub fn release_key() -> ([u8; 32], bool) {
    match option_env!("MAARSE_RELEASE_PUBKEY").and_then(ota::parse_hex32) {
        Some(k) => (k, false),
        None => (ota::dev_public_key(), true),
    }
}

/// Den kørende firmwares version (major << 16 | minor << 8 | patch).
pub fn fw_version() -> u32 {
    option_env!("MAARSE_FW_VERSION")
        .and_then(ota::parse_version)
        .or_else(|| ota::parse_version(env!("CARGO_PKG_VERSION")))
        .unwrap_or(0)
}

/// Tid fra `OtaEnd { apply }` til installation, så svaret når ud først.
const INSTALL_DELAY_LOCAL: Duration = Duration::from_secs(2);
const INSTALL_DELAY_REMOTE: Duration = Duration::from_secs(10);

pub struct OtaManager {
    rx: Receiver,
    install_at: Option<Instant>,
}

impl Default for OtaManager {
    fn default() -> Self {
        Self::new()
    }
}

impl OtaManager {
    pub const fn new() -> Self {
        OtaManager { rx: Receiver::new(), install_at: None }
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.install_at
    }

    fn state(&self, req: u16, code: u8, msg: &str) -> Event {
        let m = self.rx.manifest;
        Event::OtaState {
            req,
            state: code,
            next: self.rx.next,
            size: m.map_or(0, |m| m.size),
            version: m.map_or(0, |m| m.version),
            current: fw_version(),
            text: text(msg.as_bytes()),
        }
    }

    fn current(&self, req: u16, msg: &str) -> Event {
        let code = if self.install_at.is_some() {
            ota_state::INSTALLING
        } else {
            match self.rx.state {
                State::Idle => ota_state::IDLE,
                State::Receiving => ota_state::RECEIVING,
                State::Verified => ota_state::VERIFIED,
            }
        };
        self.state(req, code, msg)
    }

    /// Behandler en OTA-kommando og returnerer svaret. `remote`: kom via
    /// radio fra en administrator (så ignoreres "tillad ældre version").
    pub fn handle(&mut self, port: Option<&mut dyn OtaPort>, cmd: &Command, remote: bool) -> Event {
        let req = match cmd {
            Command::OtaBegin { req, .. } | Command::OtaChunk { req, .. } | Command::OtaEnd { req, .. } => *req,
            _ => 0,
        };
        let Some(port) = port else {
            return self.state(req, ota_state::UNSUPPORTED, "Dette board kan ikke opdateres over luften");
        };
        if self.install_at.is_some() && !matches!(cmd, Command::OtaStatus) {
            return self.current(req, "Installerer - vent på genstart");
        }
        let res = match cmd {
            Command::OtaBegin { flags, manifest, .. } => {
                let (key, dev) = release_key();
                let policy = Policy {
                    public_key: key,
                    board: port.board(),
                    current_version: fw_version(),
                    capacity: port.capacity(),
                };
                let downgrade = flags & OTA_ALLOW_DOWNGRADE != 0 && !remote;
                Manifest::parse(manifest)
                    .and_then(|m| self.rx.begin(port, m, &policy, downgrade))
                    .map(|_| if dev { "ADVARSEL: noden stoler på udviklingsnøglen" } else { "Klar" })
            }
            Command::OtaChunk { offset, data, .. } => self.rx.chunk(port, *offset, data).map(|_| ""),
            Command::OtaEnd { apply, .. } => self.rx.end().map(|_| {
                if *apply {
                    let delay = if remote { INSTALL_DELAY_REMOTE } else { INSTALL_DELAY_LOCAL };
                    self.install_at = Some(Instant::now() + delay);
                    "Installerer og genstarter"
                } else {
                    "Verificeret; send OtaEnd med apply for at installere"
                }
            }),
            _ => Ok(""),
        };
        match res {
            Ok(msg) => self.current(req, msg),
            // Forkert offset: svaret fortæller, hvor klienten skal fortsætte.
            Err(OtaError::WrongOffset) => self.current(req, OtaError::WrongOffset.text()),
            Err(e) => self.state(req, ota_state::ERROR, e.text()),
        }
    }

    /// Kaldes, når [`Self::deadline`] er nået. Returnerer en fejltekst, hvis
    /// installationen ikke kunne starte (ellers genstarter noden).
    pub fn on_timer(&mut self, port: Option<&mut dyn OtaPort>) -> Option<&'static str> {
        let at = self.install_at?;
        if Instant::now() < at {
            return None;
        }
        self.install_at = None;
        let Some(port) = port else {
            return Some("Installation afbrudt: intet mellemlager");
        };
        match self.rx.recheck(port) {
            Ok(m) => Some(port.install(&m)),
            Err(e) => Some(e.text()),
        }
    }
}
