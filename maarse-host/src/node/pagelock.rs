//! Locked pages and notice boards for `maarse://` pages.
//!
//! Both are written in the page's own `.mu` file; the host keeps them.
//!
//! **Locks.** `@allow <who> …` lines say who may read a page:
//!
//! - a full address (32 hex characters: the fingerprint of the reader's
//!   keys, so it cannot be borrowed; a page request is a sealed message
//!   only that key can have made),
//! - `admins`: the host's administrators,
//! - `group <name>`: a group in `pages/.groups` (one line per group:
//!   `name address address …`; files starting with `.` are never served).
//!
//! `@locked hidden` answers others with the ordinary "Not found", so they do
//! not learn that the page exists; `@locked until 2026-12-24` opens the page
//! for everyone on that date. The lock of a folder's `index.mu` covers every
//! page and picture in that folder (not the top folder: locking the front
//! page does not lock the whole site). The owner always reads. Lock lines
//! are cut out before a page is sent, so the list never leaves the host; a
//! reader on the list gets `@lockinfo <n>` (how many may read it).
//!
//! **Notice boards.** A line `@board [n] [style]` marks a board. Readers
//! write notes ([`maarse_core::content::BOARD_POST`], at most 100 bytes);
//! the host keeps the newest (`--board-keep`, 50) in `<page>.board` beside the
//! page, at most one note a minute from each reader, and serves the page
//! with the newest notes after the `@board` line as
//! `@note <unix time>\t<name>\t<short address>\t<text>`.

use super::{Node, short};
use maarse_core::Address;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

// The text rules (lock lines, boards, notes, dates, preview words) live in
// maarse-hostrules (shared with the simulator).
pub(super) use maarse_hostrules::pagelock::{outside_code, strip_locks, with_previews};
use maarse_hostrules::pagelock::{Lock, board_line, clean_note, has_board, hex, is_note, lock_folders, lock_of, may_note, note_line, parse_hex};

pub(super) fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// The `.mu` files whose locks apply to `path` (a page path or the path of
/// a picture, both relative to `pages/`): the page itself, and the
/// `index.mu` of every folder it is in (not the top folder).
pub(super) fn lock_files(root: &Path, path: &str, page: Option<&Path>) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = page.map(Path::to_path_buf).into_iter().collect();
    for folder in lock_folders(path) {
        let idx = root.join(folder).join("index.mu");
        if idx.is_file() && !v.contains(&idx) {
            v.push(idx);
        }
    }
    // A folder's own front page (`crew` -> crew/index.mu) is the page itself.
    v
}

/// Where a page's notes are kept: `<page>.board` beside the `.mu` file.
pub(super) fn board_file(page: &Path) -> PathBuf {
    page.with_extension("board")
}

/// The note lines kept for a board, newest last.
pub(super) fn read_notes(file: &Path) -> Vec<String> {
    fs::read_to_string(file).unwrap_or_default().lines().filter(|l| is_note(l)).map(str::to_string).collect()
}

/// What a reader gets for a locked page.
pub(super) enum Access {
    /// Not locked.
    Open,
    /// Locked and this reader is on the list (`n` may read it).
    Member(usize),
    /// Locked; the reader is not on the list.
    Locked,
    /// Locked with `@locked hidden`.
    Hidden,
}

impl Node {
    /// Members of `pages/.groups` group `name`.
    fn group(&self, root: &Path, name: &str) -> Vec<Address> {
        let text = fs::read_to_string(root.join(".groups")).unwrap_or_default();
        text.lines()
            .filter_map(|l| {
                let mut w = l.split_whitespace();
                (w.next() == Some(name)).then(|| w.filter_map(parse_hex).collect::<Vec<_>>())
            })
            .flatten()
            .collect()
    }

    /// The readers a lock lets in.
    fn members(&self, root: &Path, lock: &Lock) -> Vec<Address> {
        let mut v = Vec::new();
        for a in &lock.allow {
            if a == "admins" {
                v.extend(self.admins.iter().copied());
            } else if let Some(g) = a.strip_prefix("group:") {
                v.extend(self.group(root, g));
            } else if let Some(x) = parse_hex(a) {
                v.push(x);
            }
        }
        v.sort_by_key(|a| a.0);
        v.dedup();
        v
    }

    /// May `reader` (None: the owner) read `path`? `page` is the page's
    /// `.mu` file when `path` is a page (None for a picture).
    pub(super) fn access(&self, reader: Option<Address>, path: &str, page: Option<&Path>) -> Access {
        let root = self.pages_dir();
        let mut result = Access::Open;
        let now = unix_now();
        for f in lock_files(&root, path, page) {
            let lock = lock_of(&fs::read_to_string(&f).unwrap_or_default());
            if !lock.applies(now) {
                continue;
            }
            let members = self.members(&root, &lock);
            let ok = match reader {
                None => true,
                Some(r) => r == self.addr || members.contains(&r) || members.contains(&self.person_peer(&r)),
            };
            if !ok {
                return if lock.hidden { Access::Hidden } else { Access::Locked };
            }
            result = Access::Member(members.len() + 1);
        }
        result
    }

    /// The page's markup with the newest notes after each `@board` line.
    pub(super) fn with_notes(&self, markup: &str, page: &Path, path: &str) -> String {
        if !outside_code(markup).any(|l| l.trim_start().starts_with("@board")) {
            return markup.to_string();
        }
        let notes = read_notes(&board_file(page));
        let mut out = String::with_capacity(markup.len() + notes.len() * 80);
        let mut fence = false;
        for line in markup.lines() {
            out.push_str(line);
            out.push('\n');
            let t = line.trim_start();
            // An example in a code block is text, never a board.
            if t.starts_with("```") {
                fence = !fence;
                continue;
            }
            if fence {
                continue;
            }
            // `@board [n] [style]`: n newest (all kept ones without n).
            if let Some(n) = board_line(t, self.pages.board_keep) {
                out.push_str(&format!("@boardpath {path}\n"));
                for note in notes.iter().rev().take(n) {
                    out.push_str(&note_line(note));
                }
            }
        }
        out
    }

    /// A reader writes a note on a page's board.
    pub(super) fn on_board_post(&mut self, from: Address, path: &str, text: &[u8]) -> Result<(), &'static str> {
        let root = self.pages_dir();
        let page = super::pages::find_page(&root, path).ok_or("no such page")?;
        let markup = fs::read_to_string(&page).map_err(|_| "no such page")?;
        if !has_board(&markup) {
            return Err("this page has no notice board");
        }
        let reader = (from != self.addr).then_some(from);
        if matches!(self.access(reader, path, Some(&page)), Access::Locked | Access::Hidden) {
            return Err("locked");
        }
        let now = unix_now();
        if !may_note(self.pages.noted.get(&from).copied(), now, self.pages.board_every) {
            return Err("one note a minute");
        }
        let text = clean_note(text).ok_or("empty note")?;
        let name = self.ext.names.get(&from).map(|(n, _)| String::from_utf8_lossy(n).replace(['\t', '\n'], " ")).unwrap_or_default();
        let name = if from == self.addr && name.is_empty() { self.ext.name.replace(['\t', '\n'], " ") } else { name };
        let file = board_file(&page);
        let mut notes = read_notes(&file);
        notes.push(format!("{now}\t{}\t{name}\t{text}", hex(&from)));
        let skip = notes.len().saturating_sub(self.pages.board_keep);
        let body = notes[skip..].join("\n") + "\n";
        fs::write(&file, body).map_err(|_| "could not save the note")?;
        self.pages.noted.insert(from, now);
        if let Some(t) = self.tele.as_mut() {
            t.page_note(path);
        }
        self.log(format!("note on '{path}' from {}", short(&from)));
        Ok(())
    }
}

// ---------- picture previews ----------
//
// Every picture line a page is sent with gets a preview in its style words:
// `~<width>x<height>:<24 characters>`, the picture's size (so the reader's
// frame has the right shape before anything comes) and 4×3 colours (12 bits
// each, base64url), drawn blurred until a real copy arrives. About 35 bytes
// per picture, made once per file and kept in memory.

/// The preview token of a picture file, e.g. `~1200x800:Aa0…`.
pub(crate) fn preview_token(data: &[u8]) -> Option<String> {
    let img = image::load_from_memory(data).ok()?;
    let (w, h) = (img.width(), img.height());
    let small = img.resize_exact(4, 3, image::imageops::FilterType::Triangle).to_rgb8();
    Some(maarse_hostrules::pagelock::preview_from_pixels(w, h, small.as_raw()))
}

#[cfg(test)]
mod preview_tests {
    use super::*;

    #[test]
    fn preview_of_a_picture() {
        let img = image::RgbImage::from_fn(120, 80, |x, _| image::Rgb([(x * 2) as u8, 0, 200]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(img).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let t = preview_token(&png).unwrap();
        assert!(t.starts_with("~120x80:") && t.len() == "~120x80:".len() + 24, "{t}");
    }
}
