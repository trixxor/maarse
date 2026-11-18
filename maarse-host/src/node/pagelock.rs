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


/// The lock lines of one page.
#[derive(Default, Debug, PartialEq)]
pub(super) struct Lock {
    /// Who may read: addresses (hex), `admins`, `group:<name>`.
    pub allow: Vec<String>,
    pub hidden: bool,
    /// Open for everyone from this time (unix seconds).
    pub until: Option<u64>,
}

impl Lock {
    pub fn locked(&self) -> bool {
        !self.allow.is_empty() || self.until.is_some()
    }
}

/// The lines of a page outside code blocks (an example in a code block
/// is text to read, never a setting).
pub(super) fn outside_code(markup: &str) -> impl Iterator<Item = &str> {
    let mut fence = false;
    markup.lines().filter(move |l| {
        if l.trim_start().starts_with("```") {
            fence = !fence;
            return false;
        }
        !fence
    })
}

/// Reads the lock lines of a page.
pub(super) fn lock_of(markup: &str) -> Lock {
    let mut l = Lock::default();
    for line in outside_code(markup) {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("@allow ") {
            let mut words = rest.split_whitespace();
            while let Some(w) = words.next() {
                match w {
                    "group" => {
                        if let Some(g) = words.next() {
                            l.allow.push(format!("group:{g}"));
                        }
                    }
                    w => l.allow.push(w.to_ascii_lowercase()),
                }
            }
        } else if let Some(rest) = line.strip_prefix("@locked ") {
            let mut words = rest.split_whitespace();
            while let Some(w) = words.next() {
                match w {
                    "hidden" => l.hidden = true,
                    "until" => l.until = words.next().and_then(parse_date),
                    _ => {}
                }
            }
        }
    }
    l
}

/// `2026-12-24` as unix seconds (midnight UTC).
pub(super) fn parse_date(s: &str) -> Option<u64> {
    let mut it = s.split('-');
    let (y, m, d): (i64, i64, i64) = (it.next()?.parse().ok()?, it.next()?.parse().ok()?, it.next()?.parse().ok()?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    // Days from civil (Howard Hinnant).
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    (days >= 0).then_some(days as u64 * 86400)
}

pub(super) fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// The markup sent to a reader: lock lines cut out.
pub(super) fn strip_locks(markup: &str) -> String {
    let mut out = String::with_capacity(markup.len());
    let mut fence = false;
    for line in markup.lines() {
        let t = line.trim_start();
        if t.starts_with("```") {
            fence = !fence;
        } else if !fence && (t.starts_with("@allow ") || t.starts_with("@locked ")) {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// The `.mu` files whose locks apply to `path` (a page path or the path of
/// a picture, both relative to `pages/`): the page itself, and the
/// `index.mu` of every folder it is in (not the top folder).
pub(super) fn lock_files(root: &Path, path: &str, page: Option<&Path>) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = page.map(Path::to_path_buf).into_iter().collect();
    let segs: Vec<&str> = path.split('/').collect();
    for i in 1..segs.len() {
        let idx = root.join(segs[..i].join("/")).join("index.mu");
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
    fs::read_to_string(file).unwrap_or_default().lines().filter(|l| l.split('\t').count() >= 4).map(str::to_string).collect()
}

/// Cleans a note: one line, no tabs, no control characters, trimmed.
pub(super) fn clean_note(text: &[u8]) -> Option<String> {
    let s = std::str::from_utf8(text).ok()?;
    let s: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    (!s.is_empty()).then_some(s)
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
            if !lock.locked() || lock.until.is_some_and(|t| now >= t) {
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
            if let Some(rest) = t.strip_prefix("@board") {
                if !(rest.is_empty() || rest.starts_with(' ')) {
                    continue;
                }
                // `@board [n] [style]`: n newest (all kept ones without n).
                let n = rest.split_whitespace().find_map(|w| w.parse::<usize>().ok()).unwrap_or(self.pages.board_keep);
                out.push_str(&format!("@boardpath {path}\n"));
                for note in notes.iter().rev().take(n) {
                    let mut f = note.splitn(4, '\t');
                    let (ts, addr, name, text) = (f.next().unwrap_or(""), f.next().unwrap_or(""), f.next().unwrap_or(""), f.next().unwrap_or(""));
                    out.push_str(&format!("@note {ts}\t{name}\t{}\t{text}\n", &addr[..addr.len().min(8)]));
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
        if !outside_code(&markup).any(|l| l.trim_start() == "@board" || l.trim_start().starts_with("@board ")) {
            return Err("this page has no notice board");
        }
        let reader = (from != self.addr).then_some(from);
        if matches!(self.access(reader, path, Some(&page)), Access::Locked | Access::Hidden) {
            return Err("locked");
        }
        let now = unix_now();
        if let Some(t) = self.pages.noted.get(&from)
            && now.saturating_sub(*t) < self.pages.board_every
        {
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

fn hex(a: &Address) -> String {
    a.0.iter().map(|b| format!("{b:02x}")).collect()
}

fn parse_hex(s: &str) -> Option<Address> {
    let s = s.trim();
    if s.len() != 32 {
        return None;
    }
    let b: Vec<u8> = (0..32).step_by(2).filter_map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect();
    Address::from_slice(&b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_lock_lines() {
        let l = lock_of("@title x\n@allow 0123456789abcdef0123456789abcdef admins\n@allow group crew\n@locked hidden until 2026-12-24\n# x\n");
        assert_eq!(l.allow, vec!["0123456789abcdef0123456789abcdef", "admins", "group:crew"]);
        assert!(l.hidden);
        assert_eq!(l.until, Some(1_798_070_400));
        assert!(!lock_of("# open page\n").locked());
    }

    #[test]
    fn examples_in_code_blocks_are_not_settings() {
        let m = "# Help
```
@allow 0123456789abcdef0123456789abcdef
@board 3
```
";
        assert!(!lock_of(m).locked());
        assert_eq!(strip_locks(m), m);
        assert!(!outside_code(m).any(|l| l.starts_with("@board")), "a board in a code block");
    }

    #[test]
    fn strips_lock_lines() {
        assert_eq!(strip_locks("@allow admins\n# Hi\n@locked hidden\ntext\n"), "# Hi\ntext\n");
    }

    #[test]
    fn dates() {
        assert_eq!(parse_date("1970-01-01"), Some(0));
        assert_eq!(parse_date("2000-03-01"), Some(951_868_800));
        assert_eq!(parse_date("2026-13-01"), None);
    }

    #[test]
    fn notes_are_one_clean_line() {
        assert_eq!(clean_note(b"  hello\tthere\nyou  ").as_deref(), Some("hello there you"));
        assert_eq!(clean_note(b"   "), None);
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
    let nibbles: Vec<u8> = small.pixels().flat_map(|p| [p[0] >> 4, p[1] >> 4, p[2] >> 4]).collect();
    let bytes: Vec<u8> = nibbles.chunks(2).map(|c| c[0] << 4 | c.get(1).copied().unwrap_or(0)).collect();
    Some(format!("~{w}x{h}:{}", b64url(&bytes)))
}

fn b64url(b: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        let k = c.len() + 1;
        for i in 0..k {
            out.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

/// Adds the preview token to every picture line outside code blocks.
/// `token` gives the token for a picture path (relative to `pages/`).
pub(super) fn with_previews(markup: &str, token: &mut dyn FnMut(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(markup.len() + 64);
    let mut fence = false;
    for line in markup.lines() {
        let t = line.trim();
        if t.starts_with("```") {
            fence = !fence;
        }
        let pic = (!fence && t.starts_with("![") && t.ends_with(')')).then(|| t.find("](")).flatten();
        if let Some(i) = pic {
            let inner = &t[i + 2..t.len() - 1];
            let (src, words) = match inner.find(char::is_whitespace) {
                Some(k) => (&inner[..k], inner[k..].trim()),
                None => (inner, ""),
            };
            let words = words.trim_matches('"');
            if !words.contains('~')
                && let Some(tok) = token(src.trim_start_matches('/'))
            {
                let w = if words.is_empty() { tok } else { format!("{words} {tok}") };
                out.push_str(&format!("{}({src} \"{w}\")\n", &t[..i + 1]));
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod preview_tests {
    use super::*;

    #[test]
    fn picture_lines_get_a_preview() {
        let m = "# Hi\n![Cabin](demo/a.png \"dots wide\")\n![b](b.png)\n```\n![x](a.png)\n```\n";
        let out = with_previews(m, &mut |p| Some(format!("~4x3:{p}")));
        assert!(out.contains("![Cabin](demo/a.png \"dots wide ~4x3:demo/a.png\")"), "{out}");
        assert!(out.contains("![b](b.png \"~4x3:b.png\")"), "{out}");
        assert!(out.contains("```\n![x](a.png)\n```"), "an example in a code block was changed: {out}");
    }

    #[test]
    fn preview_of_a_picture() {
        let img = image::RgbImage::from_fn(120, 80, |x, _| image::Rgb([(x * 2) as u8, 0, 200]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(img).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let t = preview_token(&png).unwrap();
        assert!(t.starts_with("~120x80:") && t.len() == "~120x80:".len() + 24, "{t}");
    }
}
