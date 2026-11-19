//! Locked pages and notice boards: the text rules. Reading `@allow` and
//! `@locked` lines, cutting them out, `@board` lines, cleaning notes, dates
//! and the preview words on picture lines. Files, groups and the lock check
//! against a reader stay in `maarse-host`.
//!
//! **Locks.** `@allow <who> …` lines say who may read a page: a full
//! address (32 hex characters), `admins`, or `group <name>`.
//! `@locked hidden` answers others with the ordinary "Not found";
//! `@locked until 2026-12-24` opens the page for everyone on that date.
//!
//! **Notice boards.** A line `@board [n] [style]` marks a board; the host
//! serves the page with the newest notes after it as
//! `@note <unix time>\t<name>\t<short address>\t<text>`.

use maarse_core::Address;

/// The lock lines of one page.
#[derive(Default, Debug, PartialEq)]
pub struct Lock {
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

    /// Does the lock apply at `now_s` (unix seconds)?
    pub fn applies(&self, now_s: u64) -> bool {
        self.locked() && !self.until.is_some_and(|t| now_s >= t)
    }
}

/// The lines of a page outside code blocks (an example in a code block
/// is text to read, never a setting).
pub fn outside_code(markup: &str) -> impl Iterator<Item = &str> {
    let mut fence = false;
    markup.lines().filter(move |l| {
        if l.trim_start().starts_with("```") {
            fence = !fence;
            return false;
        }
        !fence
    })
}

/// Notes kept per board (`--board-keep`).
pub const BOARD_KEEP: usize = 50;
/// A reader may write one note per this many seconds (`--board-every`).
pub const BOARD_EVERY_S: u64 = 60;

/// What a reader who may not open `path` gets: "Not found" for a hidden
/// page, otherwise the locked notice with the reader's address (hex).
pub fn refused_page(path: &str, hidden: bool, me_hex: &str) -> String {
    if hidden {
        format!("# Not found\n\nThe page `{path}` does not exist here.\n\n[Front page](/)\n")
    } else {
        format!("@title Locked page\n@locked-notice {me_hex}\n# 🔒 This page is locked\n\nOnly some people can open it. Your address is `{me_hex}`; send it to the owner if you should be on the list.\n\n[Front page](/)\n")
    }
}

/// Reads the lock lines of a page.
pub fn lock_of(markup: &str) -> Lock {
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
pub fn parse_date(s: &str) -> Option<u64> {
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

/// The markup sent to a reader: lock lines cut out.
pub fn strip_locks(markup: &str) -> String {
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

/// The folders whose `index.mu` locks also cover `path` (a page or picture
/// path relative to `pages/`): every folder it is in, not the top folder.
pub fn lock_folders(path: &str) -> Vec<String> {
    let segs: Vec<&str> = path.split('/').collect();
    (1..segs.len()).map(|i| segs[..i].join("/")).collect()
}

/// Does the page have a notice board (`@board` alone or with words)?
pub fn has_board(markup: &str) -> bool {
    outside_code(markup).any(|l| l.trim_start() == "@board" || l.trim_start().starts_with("@board "))
}

/// A trimmed line that is a board: `Some(n)` with `n` from `@board [n] [style]`
/// (`default` without n), `None` for any other line.
pub fn board_line(t: &str, default: usize) -> Option<usize> {
    let rest = t.strip_prefix("@board")?;
    if !(rest.is_empty() || rest.starts_with(' ')) {
        return None;
    }
    Some(rest.split_whitespace().find_map(|w| w.parse::<usize>().ok()).unwrap_or(default))
}

/// A kept note line (`time \t address \t name \t text`) as the `@note` line
/// sent to readers (address cut to 8 characters).
pub fn note_line(note: &str) -> String {
    let mut f = note.splitn(4, '\t');
    let (ts, addr, name, text) = (f.next().unwrap_or(""), f.next().unwrap_or(""), f.next().unwrap_or(""), f.next().unwrap_or(""));
    format!("@note {ts}\t{name}\t{}\t{text}\n", &addr[..addr.len().min(8)])
}

/// Is a line from a `.board` file a note?
pub fn is_note(line: &str) -> bool {
    line.split('\t').count() >= 4
}

/// Cleans a note: one line, no tabs, no control characters, trimmed.
pub fn clean_note(text: &[u8]) -> Option<String> {
    let s = core::str::from_utf8(text).ok()?;
    let s: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    (!s.is_empty()).then_some(s)
}

/// May a reader who last wrote at `last_s` write again at `now_s`?
pub fn may_note(last_s: Option<u64>, now_s: u64, every_s: u64) -> bool {
    last_s.is_none_or(|t| now_s.saturating_sub(t) >= every_s)
}

pub fn hex(a: &Address) -> String {
    a.0.iter().map(|b| format!("{b:02x}")).collect()
}

/// A full address in 32 hex characters.
pub fn parse_hex(s: &str) -> Option<Address> {
    let s = s.trim();
    if s.len() != 32 {
        return None;
    }
    let b: Vec<u8> = (0..32).step_by(2).filter_map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect();
    Address::from_slice(&b)
}

// ---------- picture previews ----------

/// base64url without padding.
pub fn b64url(b: &[u8]) -> String {
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

/// The preview token from a picture's size and its 4×3 RGB pixels (12
/// pixels, 36 bytes): `~<w>x<h>:<24 characters>`.
pub fn preview_from_pixels(w: u32, h: u32, rgb: &[u8]) -> String {
    let nibbles: Vec<u8> = rgb.iter().map(|c| c >> 4).collect();
    let bytes: Vec<u8> = nibbles.chunks(2).map(|c| c[0] << 4 | c.get(1).copied().unwrap_or(0)).collect();
    format!("~{w}x{h}:{}", b64url(&bytes))
}

/// Adds the preview token to every picture line outside code blocks.
/// `token` gives the token for a picture path (relative to `pages/`).
pub fn with_previews(markup: &str, token: &mut dyn FnMut(&str) -> Option<String>) -> String {
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
mod tests {
    use super::*;

    #[test]
    fn reads_lock_lines() {
        let l = lock_of("@title x\n@allow 0123456789abcdef0123456789abcdef admins\n@allow group crew\n@locked hidden until 2026-12-24\n# x\n");
        assert_eq!(l.allow, vec!["0123456789abcdef0123456789abcdef", "admins", "group:crew"]);
        assert!(l.hidden);
        assert_eq!(l.until, Some(1_798_070_400));
        assert!(l.applies(1_798_070_399) && !l.applies(1_798_070_400));
        assert!(!lock_of("# open page\n").locked());
    }

    #[test]
    fn examples_in_code_blocks_are_not_settings() {
        let m = "# Help\n```\n@allow 0123456789abcdef0123456789abcdef\n@board 3\n```\n";
        assert!(!lock_of(m).locked());
        assert_eq!(strip_locks(m), m);
        assert!(!outside_code(m).any(|l| l.starts_with("@board")), "a board in a code block");
        assert!(!has_board(m));
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
    fn folders_of_a_path() {
        assert_eq!(lock_folders("crew/logs/a.png"), vec!["crew", "crew/logs"]);
        assert!(lock_folders("index").is_empty(), "never the top folder");
    }

    #[test]
    fn boards_and_notes() {
        assert!(has_board("# x\n@board\n") && has_board("@board 5 wide\n") && !has_board("@boardx\n"));
        assert_eq!(board_line("@board", 50), Some(50));
        assert_eq!(board_line("@board 5 wide", 50), Some(5));
        assert_eq!(board_line("@boardpath x", 50), None);
        assert_eq!(board_line("# heading", 50), None);
        assert_eq!(note_line("1700000000\tabcdef0123456789\tAnna\thi there"), "@note 1700000000\tAnna\tabcdef01\thi there\n");
        assert!(is_note("1\t2\t3\t4") && !is_note("1\t2\t3"));
        assert!(may_note(None, 10, 60) && !may_note(Some(0), 59, 60) && may_note(Some(0), 60, 60));
    }

    #[test]
    fn notes_are_one_clean_line() {
        assert_eq!(clean_note(b"  hello\tthere\nyou  ").as_deref(), Some("hello there you"));
        assert_eq!(clean_note(b"   "), None);
    }

    #[test]
    fn hex_addresses() {
        let a = Address([0xab; 16]);
        assert_eq!(parse_hex(&hex(&a)), Some(a));
        assert_eq!(parse_hex("abc"), None);
    }

    #[test]
    fn picture_lines_get_a_preview() {
        let m = "# Hi\n![Cabin](demo/a.png \"dots wide\")\n![b](b.png)\n```\n![x](a.png)\n```\n";
        let out = with_previews(m, &mut |p| Some(format!("~4x3:{p}")));
        assert!(out.contains("![Cabin](demo/a.png \"dots wide ~4x3:demo/a.png\")"), "{out}");
        assert!(out.contains("![b](b.png \"~4x3:b.png\")"), "{out}");
        assert!(out.contains("```\n![x](a.png)\n```"), "an example in a code block was changed: {out}");
    }

    #[test]
    fn preview_token_shape() {
        let t = preview_from_pixels(120, 80, &[0xff; 36]);
        assert!(t.starts_with("~120x80:") && t.len() == "~120x80:".len() + 24, "{t}");
        assert_eq!(b64url(b"Ma"), "TWE");
    }
}
