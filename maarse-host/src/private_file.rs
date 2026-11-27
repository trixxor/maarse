//! Files that hold secrets or private content: readable by the owner only.
//! On Windows a file takes its folder's permissions (the service restricts the
//! data folder to SYSTEM and administrators).

use std::fs;
use std::io::{self, Write};
use std::path::Path;

/// Replaces `path` atomically with a file only the owner can read. `sync`
/// waits until it is on the disk.
pub(crate) fn write(path: &Path, bytes: &[u8], sync: bool) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    let _ = fs::remove_file(&tmp);
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    file.write_all(bytes)?;
    if sync {
        file.sync_all()?;
    }
    drop(file);
    fs::rename(&tmp, path)?;
    // The new name must be on the disk too (Unix; Windows has no such call).
    #[cfg(unix)]
    if sync && let Some(dir) = path.parent() {
        fs::File::open(dir)?.sync_all()?;
    }
    Ok(())
}

/// Closes the data folder to other users of the computer (Unix). Everything in
/// it is then out of their reach, whatever the single files allow.
pub(crate) fn restrict_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::metadata(dir)?.permissions().mode() & 0o077 != 0 {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }
    }
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_a_file() {
        let dir = std::env::temp_dir().join(format!("maarse-private-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("secret.bin");
        write(&path, b"one", false).unwrap();
        write(&path, b"two", true).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        assert!(!dir.join("secret.tmp").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn only_the_owner_can_read_the_file_and_the_folder() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("maarse-private-mode-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        restrict_dir(&dir).unwrap();
        let path = dir.join("secret.bin");
        write(&path, b"x", true).unwrap();
        assert_eq!(fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        let _ = fs::remove_dir_all(&dir);
    }
}
