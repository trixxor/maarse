//! The page server's files, for the server dashboard's file manager: list,
//! read, write (upload or edit) and delete files in `<data>/pages/`.
//!
//! Only an admin who is logged in reaches these (the dashboard checks the
//! session and the origin). Paths are relative to `pages/`, made of
//! `a-z A-Z 0-9 - _ .` segments, never `..`, never starting with `.` except
//! the groups file `.groups`. Only page files, pictures and `.groups` can be
//! written; a file is at most 2 MB.

use serde_json::{Value, json};
use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

/// Largest file that can be uploaded.
pub const MAX_FILE: usize = 2 * 1024 * 1024;
const WRITABLE: [&str; 7] = ["mu", "png", "jpg", "jpeg", "webp", "gif", "txt"];

fn root(data_dir: &Path) -> PathBuf {
    data_dir.join("pages")
}

fn linked(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)] {
        use std::os::windows::fs::MetadataExt;
        // Includes directory junctions as well as symbolic links.
        if metadata.file_attributes() & 0x400 != 0 { return true; }
    }
    metadata.file_type().is_symlink()
}

/// Resolve within an operator-owned page tree without following links.
pub(crate) fn checked_path(base: &Path, path: &Path) -> io::Result<()> {
    let relative = path.strip_prefix(base).map_err(io::Error::other)?;
    let mut current = base.to_path_buf();
    let check = |path: &Path| match fs::symlink_metadata(path) {
        Ok(metadata) if linked(&metadata) || !(metadata.is_file() || metadata.is_dir()) => Err(io::Error::new(io::ErrorKind::InvalidInput, "linked or special file in page path")),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    };
    check(&current)?;
    for component in relative.components() {
        let Component::Normal(name) = component else { return Err(io::ErrorKind::InvalidInput.into()) };
        #[cfg(windows)] {
            let name = name.to_string_lossy();
            let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
            if name.contains(':') || name.ends_with(['.', ' ']) || matches!(stem.as_str(), "CON" | "AUX" | "NUL" | "PRN")
                || (stem.starts_with("COM") || stem.starts_with("LPT")) && matches!(stem.get(3..), Some("1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "\u{00b9}" | "\u{00b2}" | "\u{00b3}")) {
                return Err(io::ErrorKind::InvalidInput.into());
            }
        }
        current.push(name);
        check(&current)?;
    }
    Ok(())
}

pub(crate) fn read_bounded(base: &Path, path: &Path) -> io::Result<Vec<u8>> {
    checked_path(base, path)?;
    let file = fs::File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_FILE as u64 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid or oversized page file"));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_FILE as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_FILE { return Err(io::ErrorKind::InvalidData.into()); }
    Ok(bytes)
}

pub(crate) fn read_markup(base: &Path, path: &Path) -> io::Result<String> {
    String::from_utf8(read_bounded(base, path)?).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// The full path of `rel` in `pages/`, or why it is not allowed.
pub fn resolve(data_dir: &Path, rel: &str) -> Result<PathBuf, &'static str> {
    let rel = rel.trim().trim_matches('/');
    if rel.is_empty() || rel.len() > 200 {
        return Err("no file name");
    }
    if rel == ".groups" {
        let base = root(data_dir);
        let path = base.join(".groups");
        checked_path(&base, &path).map_err(|_| "unsafe page path")?;
        return Ok(path);
    }
    for seg in rel.split('/') {
        let ok = !seg.is_empty() && !seg.starts_with('.') && seg.bytes().all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c));
        if !ok {
            return Err("names may only hold letters, digits, - _ and . (and not start with a dot)");
        }
    }
    let base = root(data_dir);
    let path = base.join(rel);
    checked_path(&base, &path).map_err(|_| "unsafe page path")?;
    Ok(path)
}

fn writable(rel: &str) -> bool {
    rel == ".groups" || rel.rsplit_once('.').is_some_and(|(_, e)| WRITABLE.contains(&e.to_ascii_lowercase().as_str()))
}

/// Every file in `pages/`, with size and time (unix s), folders first.
pub fn list(data_dir: &Path) -> Value {
    fn walk(base: &Path, dir: &Path, depth: usize, out: &mut Vec<Value>) {
        if depth > 24 || out.len() >= 10_000 { return; }
        let Ok(rd) = fs::read_dir(dir) else { return };
        let mut entries: Vec<_> = rd.flatten().take(10_000 - out.len()).collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            if out.len() >= 10_000 { break; }
            let p = e.path();
            let rel = p.strip_prefix(base).unwrap_or(&p).to_string_lossy().replace('\\', "/");
            let Ok(m) = fs::symlink_metadata(&p) else { continue };
            if linked(&m) || !(m.is_file() || m.is_dir()) { continue; }
            let t = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
            if m.is_dir() {
                out.push(json!({"path": rel, "dir": true, "time": t}));
                walk(base, &p, depth + 1, out);
            } else {
                out.push(json!({"path": rel, "size": m.len(), "time": t}));
            }
        }
    }
    let base = root(data_dir);
    let mut out = Vec::new();
    if checked_path(&base, &base).is_ok() { walk(&base, &base, 0, &mut out); }
    json!({"files": out})
}

pub fn read(data_dir: &Path, rel: &str) -> Result<Vec<u8>, &'static str> {
    read_bounded(&root(data_dir), &resolve(data_dir, rel)?).map_err(|_| "no such or oversized file")
}

pub fn write(data_dir: &Path, rel: &str, data: &[u8]) -> Result<(), String> {
    let path = resolve(data_dir, rel)?;
    if !writable(rel) {
        return Err(format!("only {} files and .groups can be put here", WRITABLE.join(", ")));
    }
    if data.len() > MAX_FILE {
        return Err("larger than 2 MB".into());
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    fs::write(&path, data).map_err(|e| e.to_string())
}

/// Deletes a file, or a folder that is empty.
pub fn delete(data_dir: &Path, rel: &str) -> Result<(), String> {
    let path = resolve(data_dir, rel)?;
    if path.is_dir() {
        fs::remove_dir(&path).map_err(|_| "the folder is not empty".to_string())
    } else {
        fs::remove_file(&path).map_err(|_| "no such file".to_string())
    }
}

/// Makes a folder.
pub fn mkdir(data_dir: &Path, rel: &str) -> Result<(), String> {
    fs::create_dir_all(resolve(data_dir, rel)?).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_stay_in_pages() {
        let d = std::env::temp_dir().join(format!("maarse-pagefiles-{}", std::process::id()));
        assert!(resolve(&d, "../x.mu").is_err());
        assert!(resolve(&d, "demo/../../x").is_err());
        assert!(resolve(&d, ".hidden").is_err());
        assert!(resolve(&d, ".groups").is_ok());
        assert!(write(&d, "demo/a.mu", b"# Hi").is_ok());
        assert!(write(&d, "demo/run.sh", b"x").is_err());
        assert_eq!(read(&d, "demo/a.mu").unwrap(), b"# Hi");
        assert!(list(&d)["files"].as_array().unwrap().iter().any(|f| f["path"] == "demo/a.mu"));
        assert!(delete(&d, "demo").is_err(), "a folder with files is kept");
        assert!(delete(&d, "demo/a.mu").is_ok());
        let oversized = root(&d).join("big.mu");
        fs::File::create(oversized).unwrap().set_len(MAX_FILE as u64 + 1).unwrap();
        assert!(read(&d, "big.mu").is_err());
        #[cfg(unix)] {
            let outside = d.join("outside.mu");
            fs::write(&outside, b"outside").unwrap();
            std::os::unix::fs::symlink(&outside, root(&d).join("linked.mu")).unwrap();
            assert!(read(&d, "linked.mu").is_err());
            assert!(!list(&d)["files"].as_array().unwrap().iter().any(|f| f["path"] == "linked.mu"));
        }
        #[cfg(windows)] {
            assert!(resolve(&d, "nul.mu").is_err());
            assert!(resolve(&d, "folder./a.mu").is_err());
        }
        let _ = fs::remove_dir_all(d);
    }
}
