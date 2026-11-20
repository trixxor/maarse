//! Replacing the node's identity (`Command::SetIdentity`), typically from a
//! recovery phrase that the web client has derived (see
//! `maarse_crypto::recovery`). Local only: control commands via `Remote`
//! are rejected before they get here.

use super::{Node, err_ev, short};
use crate::ClientId;
use maarse_core::companion::{Event, error_code};
use maarse_crypto::Identity;
use std::fs;
use std::io::{self, Write};
use std::path::Path;

pub(super) fn read_identity(dir: &Path) -> io::Result<Option<Identity>> {
    let path = dir.join("identity.key");
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !metadata.is_file() || metadata.len() != 64 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "identity.key must be a regular 64-byte key file"));
    }
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        let backup = dir.join("identity.old");
        for key in [&path, &backup] {
            if let Ok(info) = fs::symlink_metadata(key)
                && info.is_file() && info.permissions().mode() & 0o077 != 0 {
                fs::set_permissions(key, fs::Permissions::from_mode(0o600))?;
            }
        }
    }
    let bytes: [u8; 64] = fs::read(path)?.try_into().map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "identity.key changed while reading"))?;
    Ok(Some(Identity::from_secret_bytes(&bytes)))
}

fn write_key(dir: &Path, target: &Path, secret: &[u8]) -> io::Result<()> {
    use rand_core::RngCore;
    let tmp = dir.join(format!("identity-{:016x}.tmp", rand_core::OsRng.next_u64()));
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)] {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    let result = file.write_all(secret).and_then(|()| file.sync_all());
    drop(file);
    let result = result.and_then(|()| fs::rename(&tmp, target));
    if result.is_err() { let _ = fs::remove_file(tmp); }
    result
}

/// Replaces the key atomically and keeps a private copy of the old key.
pub(super) fn write_identity(dir: &Path, secret: &[u8; 64]) -> io::Result<()> {
    let path = dir.join("identity.key");
    if let Some(previous) = read_identity(dir)? {
        write_key(dir, &dir.join("identity.old"), &previous.to_secret_bytes())?;
    }
    write_key(dir, &path, secret)
}

impl Node {
    pub(super) fn set_identity(&mut self, id: ClientId, confirm: bool, secret: [u8; 64]) {
        if !confirm {
            let e = err_ev(0, error_code::BAD_FRAME, "Replacing the identity must be confirmed");
            return self.send_client(id, &e);
        }
        let identity = Identity::from_secret_bytes(&secret);
        let addr = identity.address();
        if addr == self.addr {
            // Same identity (e.g. restored from the same phrase): nothing to do.
            let ev = Event::IdentityChanged { addr, identity: identity.public().to_bytes() };
            return self.send_client(id, &ev);
        }
        if let Err(e) = write_identity(&self.data_dir, &secret) {
            let m = format!("Could not save the identity: {e}");
            return self.send_client(id, &err_ev(0, error_code::BAD_FRAME, &m));
        }
        let old = self.addr;
        self.identity = identity.into();
        self.addr = addr;
        // Routes and forward secrecy sessions belong to the old identity.
        self.router.set_address(addr);
        self.sessions.clear();
        self.resynced.clear();
        self.awaiting.clear();
        self.outbox.clear();
        self.dht.heard.remove(&old);
        self.dht.table = crate::dht::Table::new(addr);
        self.dht.last_bootstrap = None;
        self.log(format!("new identity: {} (was {})", short(&addr), short(&old)));
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
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 2);
        fs::write(dir.join("identity.key"), [3; 63]).unwrap();
        assert!(read_identity(&dir).is_err());
        assert!(write_identity(&dir, &[4; 64]).is_err());
        assert_eq!(fs::read(dir.join("identity.key")).unwrap(), vec![3; 63]);
        let _ = fs::remove_dir_all(&dir);
    }
}
