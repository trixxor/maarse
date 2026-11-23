//! Firmware update on the node: the commands `OtaBegin`/`OtaChunk`/
//! `OtaEnd`/`OtaStatus` locally (USB/Bluetooth) and via `Remote` (administrators
//! only). The checks themselves (signature, board, version, SHA-256,
//! resuming) are in `maarse_crypto::ota`; the board provides the staging area
//! and installation through [`OtaPort`].
//!
//! The release key is built in with `MAARSE_RELEASE_PUBKEY=<64 hex>` and
//! the version with `MAARSE_FW_VERSION=x.y.z` (otherwise the crate version).
//! OTA is disabled when the release key is absent or invalid.

use embassy_time::{Duration, Instant};
use maarse_core::companion::{Command, Event, OTA_ALLOW_DOWNGRADE, ota_state, text};
use maarse_crypto::ota::{self, Manifest, OtaError, Policy, Receiver, Staging, State};

/// The board's part: staging area (see [`Staging`]) and installation.
pub trait OtaPort: Staging {
    fn board(&self) -> u16;
    /// The size of the staging area in bytes.
    fn capacity(&self) -> u32;
    /// Installs the verified image in the staging area and restarts.
    /// Only returns on error.
    fn install(&mut self, m: &Manifest) -> &'static str;
}

/// The release key the firmware trusts.
fn release_key() -> Option<[u8; 32]> {
    let key = option_env!("MAARSE_RELEASE_PUBKEY").and_then(ota::parse_hex32)?;
    ota::valid_release_key(&key).then_some(key)
}

/// The running firmware's version (major << 16 | minor << 8 | patch).
pub fn fw_version() -> u32 {
    option_env!("MAARSE_FW_VERSION")
        .and_then(ota::parse_version)
        .or_else(|| ota::parse_version(env!("CARGO_PKG_VERSION")))
        .unwrap_or(0)
}

/// Time from `OtaEnd { apply }` to installation, so the reply gets out first.
const INSTALL_DELAY_LOCAL: Duration = Duration::from_secs(2);
const INSTALL_DELAY_REMOTE: Duration = Duration::from_secs(10);

pub struct OtaManager {
    rx: Receiver,
    key: Option<[u8; 32]>,
    install_at: Option<Instant>,
    /// The board ID, learned from the port the first time a command arrives
    /// (0 on a board that cannot be updated).
    board: u16,
}

impl Default for OtaManager {
    fn default() -> Self {
        Self::new()
    }
}

impl OtaManager {
    pub fn new() -> Self {
        OtaManager { rx: Receiver::new(), key: release_key(), install_at: None, board: 0 }
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

    /// The answer to `OtaStatus` that tells a host which board this is (none on a
    /// board that cannot be updated).
    pub fn info(&self) -> Option<Event> {
        (self.board != 0).then_some(Event::OtaInfo { board: self.board })
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

    /// Handles an OTA command and returns the reply. `remote`: came via
    /// radio from an administrator (then "allow older version" is ignored).
    pub fn handle(&mut self, port: Option<&mut dyn OtaPort>, cmd: &Command, remote: bool) -> Event {
        let req = match cmd {
            Command::OtaBegin { req, .. } | Command::OtaChunk { req, .. } | Command::OtaEnd { req, .. } => *req,
            _ => 0,
        };
        let Some(port) = port else {
            return self.state(req, ota_state::UNSUPPORTED, "This board cannot be updated over the air");
        };
        self.board = port.board();
        let Some(key) = self.key else {
            return self.state(req, ota_state::UNSUPPORTED, "OTA is disabled without a valid release key");
        };
        if self.install_at.is_some() && !matches!(cmd, Command::OtaStatus) {
            return self.current(req, "Installing - wait for the restart");
        }
        let res = match cmd {
            Command::OtaBegin { flags, manifest, .. } => {
                let policy = Policy {
                    public_key: key,
                    board: port.board(),
                    current_version: fw_version(),
                    capacity: port.capacity(),
                };
                let downgrade = flags & OTA_ALLOW_DOWNGRADE != 0 && !remote;
                Manifest::parse(manifest)
                    .and_then(|m| self.rx.begin(port, m, &policy, downgrade))
                    .map(|_| "Ready")
            }
            Command::OtaChunk { offset, data, .. } => self.rx.chunk(port, *offset, data).map(|_| ""),
            Command::OtaEnd { apply, .. } => self.rx.end().map(|_| {
                if *apply {
                    let delay = if remote { INSTALL_DELAY_REMOTE } else { INSTALL_DELAY_LOCAL };
                    self.install_at = Some(Instant::now() + delay);
                    "Installing and restarting"
                } else {
                    "Verified; send OtaEnd with apply to install"
                }
            }),
            _ => Ok(""),
        };
        match res {
            Ok(msg) => self.current(req, msg),
            // Wrong offset: the reply tells where the client should continue.
            Err(OtaError::WrongOffset) => self.current(req, OtaError::WrongOffset.text()),
            Err(e) => self.state(req, ota_state::ERROR, e.text()),
        }
    }

    /// Called when [`Self::deadline`] is reached. Returns an error text if
    /// the installation could not start (otherwise the node restarts).
    pub fn on_timer(&mut self, port: Option<&mut dyn OtaPort>) -> Option<&'static str> {
        let at = self.install_at?;
        if Instant::now() < at {
            return None;
        }
        self.install_at = None;
        let Some(port) = port else {
            return Some("Installation aborted: no staging area");
        };
        match self.rx.recheck(port) {
            Ok(m) => Some(port.install(&m)),
            Err(e) => Some(e.text()),
        }
    }
}
