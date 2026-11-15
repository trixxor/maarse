//! Replacing the node's identity (`Command::SetIdentity`), typically from a
//! recovery phrase the web client has derived. Local only (USB/BLE):
//! control commands via radio are rejected in `on_control`.

use super::{FwNode, Settings, emit, err, log};
use core::sync::atomic::Ordering::Relaxed;
use maarse_core::companion::{Event, error_code};
use maarse_crypto::Identity;
use maarse_crypto::ratchet::FixedStore;
use rand_core::{CryptoRng, RngCore};

impl<R: RngCore + CryptoRng, S: Settings> FwNode<R, S> {
    pub(super) fn set_identity(&mut self, confirm: bool, secret: &[u8; 64]) {
        if !confirm {
            return err(0, error_code::BAD_FRAME, "Replacing the identity must be confirmed");
        }
        let identity = Identity::from_secret_bytes(secret);
        let addr = identity.address();
        if addr != self.addr {
            if !self.settings.save_identity(secret) {
                return err(0, error_code::BAD_FRAME, "Could not save the identity");
            }
            self.identity = identity;
            self.addr = addr;
            crate::bus::MY_SHORT.store(u32::from_be_bytes([addr.0[0], addr.0[1], addr.0[2], addr.0[3]]), Relaxed);
            // Routes and forward secrecy sessions belong to the old identity.
            self.router.set_address(addr);
            self.sessions = FixedStore::new();
            self.resynced.clear();
            self.mail_inflight.clear();
            self.stamping = None;
            log("New identity saved");
        }
        emit(&Event::IdentityChanged { addr, identity: self.identity.public().to_bytes() });
        self.announce();
    }
}
