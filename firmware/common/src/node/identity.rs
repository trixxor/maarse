//! Udskiftning af nodens identitet (`Command::SetIdentity`), typisk fra en
//! gendannelsesfrase, som webklienten har afledt. Kun lokalt (USB/BLE):
//! kontrolkommandoer via radio afvises i `on_control`.

use super::{FwNode, Settings, emit, err, log};
use core::sync::atomic::Ordering::Relaxed;
use maarse_core::companion::{Event, error_code};
use maarse_crypto::Identity;
use maarse_crypto::ratchet::FixedStore;
use rand_core::{CryptoRng, RngCore};

impl<R: RngCore + CryptoRng, S: Settings> FwNode<R, S> {
    pub(super) fn set_identity(&mut self, confirm: bool, secret: &[u8; 64]) {
        if !confirm {
            return err(0, error_code::BAD_FRAME, "Udskiftning af identitet skal bekræftes");
        }
        let identity = Identity::from_secret_bytes(secret);
        let addr = identity.address();
        if addr != self.addr {
            if !self.settings.save_identity(secret) {
                return err(0, error_code::BAD_FRAME, "Kunne ikke gemme identiteten");
            }
            self.identity = identity;
            self.addr = addr;
            crate::bus::MY_SHORT.store(u32::from_be_bytes([addr.0[0], addr.0[1], addr.0[2], addr.0[3]]), Relaxed);
            // Ruter og forward secrecy-sessioner hører til den gamle identitet.
            self.router.set_address(addr);
            self.sessions = FixedStore::new();
            self.resynced.clear();
            self.mail_inflight.clear();
            self.stamping = None;
            log("Ny identitet gemt");
        }
        emit(&Event::IdentityChanged { addr, identity: self.identity.public().to_bytes() });
        self.announce();
    }
}
