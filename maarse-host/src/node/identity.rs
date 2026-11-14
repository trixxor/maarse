//! Udskiftning af nodens identitet (`Command::SetIdentity`), typisk fra en
//! gendannelsesfrase, som webklienten har afledt (se
//! `maarse_crypto::recovery`). Kun lokalt: kontrolkommandoer via `Remote`
//! afvises før de når hertil.

use super::{Node, err_ev, short};
use crate::ClientId;
use maarse_core::companion::{Event, error_code};
use maarse_crypto::Identity;
use std::fs;
use std::path::Path;

/// Skriver nøglefilen atomisk (midlertidig fil + omdøbning). Den gamle
/// nøgle gemmes som `identity.old`, så en fejltagelse kan rulles tilbage.
pub(super) fn write_identity(dir: &Path, secret: &[u8; 64]) -> std::io::Result<()> {
    let path = dir.join("identity.key");
    if path.exists() {
        fs::copy(&path, dir.join("identity.old"))?;
    }
    let tmp = dir.join("identity.tmp");
    fs::write(&tmp, secret)?;
    fs::rename(&tmp, &path)
}

impl Node {
    pub(super) fn set_identity(&mut self, id: ClientId, confirm: bool, secret: [u8; 64]) {
        if !confirm {
            let e = err_ev(0, error_code::BAD_FRAME, "Udskiftning af identitet skal bekræftes");
            return self.send_client(id, &e);
        }
        let identity = Identity::from_secret_bytes(&secret);
        let addr = identity.address();
        if addr == self.addr {
            // Samme identitet (fx gendannet fra samme frase): intet at gøre.
            let ev = Event::IdentityChanged { addr, identity: identity.public().to_bytes() };
            return self.send_client(id, &ev);
        }
        if let Err(e) = write_identity(&self.data_dir, &secret) {
            let m = format!("Kunne ikke gemme identiteten: {e}");
            return self.send_client(id, &err_ev(0, error_code::BAD_FRAME, &m));
        }
        let old = self.addr;
        self.identity = identity;
        self.addr = addr;
        // Ruter og forward secrecy-sessioner hører til den gamle identitet.
        self.router.set_address(addr);
        self.sessions.clear();
        self.resynced.clear();
        self.awaiting.clear();
        self.outbox.clear();
        self.dht.heard.remove(&old);
        self.dht.table = crate::dht::Table::new(addr);
        self.dht.last_bootstrap = None;
        self.log(format!("ny identitet: {} (før {})", short(&addr), short(&old)));
        let ev = Event::IdentityChanged { addr, identity: self.identity.public().to_bytes() };
        self.broadcast(&ev);
        let info = self.info();
        self.broadcast(&info);
        self.announce();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_keeps_old_key() {
        let dir = std::env::temp_dir().join(format!("maarse-id-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        write_identity(&dir, &[1; 64]).unwrap();
        assert!(!dir.join("identity.old").exists());
        write_identity(&dir, &[2; 64]).unwrap();
        assert_eq!(fs::read(dir.join("identity.key")).unwrap(), vec![2; 64]);
        assert_eq!(fs::read(dir.join("identity.old")).unwrap(), vec![1; 64]);
        assert!(!dir.join("identity.tmp").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
