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
use std::path::{Path, PathBuf};

/// Largest file that can be uploaded.
pub const MAX_FILE: usize = 2 * 1024 * 1024;
const WRITABLE: [&str; 7] = ["mu", "png", "jpg", "jpeg", "webp", "gif", "txt"];

fn root(data_dir: &Path) -> PathBuf {
    data_dir.join("pages")
}

/// The full path of `rel` in `pages/`, or why it is not allowed.
pub fn resolve(data_dir: &Path, rel: &str) -> Result<PathBuf, &'static str> {
    let rel = rel.trim().trim_matches('/');
    if rel.is_empty() || rel.len() > 200 {
        return Err("no file name");
    }
    if rel == ".groups" {
        return Ok(root(data_dir).join(".groups"));
    }
    for seg in rel.split('/') {
        let ok = !seg.is_empty() && !seg.starts_with('.') && seg.bytes().all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c));
        if !ok {
            return Err("names may only hold letters, digits, - _ and . (and not start with a dot)");
        }
    }
    Ok(root(data_dir).join(rel))
}

fn writable(rel: &str) -> bool {
    rel == ".groups" || rel.rsplit_once('.').is_some_and(|(_, e)| WRITABLE.contains(&e.to_ascii_lowercase().as_str()))
}

/// Every file in `pages/`, with size and time (unix s), folders first.
pub fn list(data_dir: &Path) -> Value {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<Value>) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            let rel = p.strip_prefix(base).unwrap_or(&p).to_string_lossy().replace('\\', "/");
            let Ok(m) = e.metadata() else { continue };
            let t = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
            if m.is_dir() {
                out.push(json!({"path": rel, "dir": true, "time": t}));
                walk(base, &p, out);
            } else {
                out.push(json!({"path": rel, "size": m.len(), "time": t}));
            }
        }
    }
    let base = root(data_dir);
    let mut out = Vec::new();
    walk(&base, &base, &mut out);
    json!({"files": out})
}

pub fn read(data_dir: &Path, rel: &str) -> Result<Vec<u8>, &'static str> {
    fs::read(resolve(data_dir, rel)?).map_err(|_| "no such file")
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
        let _ = fs::remove_dir_all(d);
    }
}
