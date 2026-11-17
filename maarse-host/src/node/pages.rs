//! `maarse://` pages: the host answers page requests (content type
//! `0x05`) with files from `<data>/pages/`, and clients can ask other nodes
//! for pages (`RequestPage`).
//!
//! A page is Må:rse markup in a `.mu` file (UTF-8). The path `weather/aarhus`
//! is looked up as `pages/weather/aarhus.mu` and then
//! `pages/weather/aarhus/index.mu`. The page is sent compressed (raw deflate) as
//! an ordinary file with MIME [`content::PAGE_MIME`] and the path as its name, so
//! it uses the same per-chunk acknowledgement and limit over LoRa as files.

use super::{Node, err_ev, short};
use crate::ClientId;
use maarse_core::Address;
use maarse_core::companion::{Command, Event, error_code};
use maarse_core::content::{self, Small};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// At most this many pages to the same node within [`WINDOW`], so nobody can
/// make us fill the air with pages, while one can still click a couple of links
/// in quick succession.
const BURST: usize = 5;
const WINDOW: Duration = Duration::from_secs(30);

#[derive(Default)]
pub(super) struct Pages {
    served: HashMap<Address, Vec<Instant>>,
}

/// Finds the page's file in `root`. The path has already been cleaned with
/// [`content::page_path`], so it cannot point outside the folder.
pub(crate) fn find_page(root: &Path, path: &str) -> Option<PathBuf> {
    let base = root.join(path);
    let file = base.with_file_name(format!("{}.mu", base.file_name()?.to_string_lossy()));
    if file.is_file() {
        return Some(file);
    }
    let idx = base.join("index.mu");
    idx.is_file().then_some(idx)
}

/// Compresses a page (raw deflate, like `DecompressionStream("deflate-raw")`).
pub(crate) fn compress(markup: &[u8]) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec(markup, 9)
}

impl Node {
    fn pages_dir(&self) -> PathBuf {
        self.data_dir.join("pages")
    }

    /// Markup for the answer to `path`.
    fn page_markup(&self, path: &str) -> String {
        let root = self.pages_dir();
        if !root.is_dir() {
            return "# No pages\n\nThis node has no pages.\n".into();
        }
        match find_page(&root, path).and_then(|f| fs::read_to_string(f).ok()) {
            Some(m) => m,
            None => format!("# Not found\n\nThe page `{path}` does not exist here.\n\n[Front page](/)\n"),
        }
    }

    pub(super) fn on_page_request(&mut self, from: Address, path: &Small) {
        let now = Instant::now();
        let served = &mut self.pages.served;
        served.retain(|_, v| {
            v.retain(|t| now.duration_since(*t) < WINDOW);
            !v.is_empty()
        });
        let times = served.entry(from).or_default();
        if times.len() >= BURST {
            self.log(format!("page request from {} rejected (too many)", short(&from)));
            return;
        }
        times.push(now);

        let path_s = String::from_utf8_lossy(path).into_owned();
        let mut data = compress(self.page_markup(&path_s).as_bytes());
        let max = self.file_limit(&from).max_bytes as usize;
        if data.len() > max {
            let msg = format!("# Too large\n\nThe page `{path_s}` is {} KB compressed; the limit here is {} KB.\n",
                data.len().div_ceil(1024), max / 1024);
            data = compress(msg.as_bytes());
        }
        let mime = Small::from_slice(content::PAGE_MIME).unwrap_or_default();
        self.log(format!("page '{path_s}' to {} ({} bytes)", short(&from), data.len()));
        if let Err((_, e)) = self.send_file_bytes(from, data, mime, path.clone()) {
            self.log(format!("page to {}: {e}", short(&from)));
        }
    }

    /// `RequestPage` and `SendCall`. Returns the command if it does not
    /// belong here.
    pub(super) fn page_command(&mut self, id: ClientId, cmd: Command) -> Option<Command> {
        if let Command::SendCall { dest, data } = cmd {
            self.send_call(id, dest, &data);
            return None;
        }
        let Command::RequestPage { req, dest, path } = cmd else { return Some(cmd) };
        let Some(plain) = content::page_request(&path) else {
            self.send_client(id, &err_ev(req, error_code::TOO_LARGE, "Invalid path"));
            return None;
        };
        self.log(format!("asking {} for the page '{}'", short(&dest), String::from_utf8_lossy(&plain[1..])));
        match self.send_sealed(dest, &plain, super::Pending::Control) {
            Ok(msg_id) => self.send_client(id, &Event::Sent { req, msg_id }),
            Err(e) => self.send_client(id, &err_ev(req, error_code::UNKNOWN_DEST, e)),
        }
        None
    }
}

impl Node {
    /// Call data: only when the recipient is reached over TCP all the way.
    fn send_call(&mut self, id: ClientId, dest: Address, data: &[u8]) {
        if self.file_limit(&dest).via_lora {
            self.send_client(id, &err_ev(0, error_code::NOT_OVER_TCP, "Calls require a connection over the internet all the way"));
            return;
        }
        let mut plain = Vec::with_capacity(1 + data.len());
        plain.push(content::CALL);
        plain.extend_from_slice(data);
        // Audio (kind 4) without acknowledgements; ring, answer, end as messages.
        let sent = if data.first() == Some(&4) {
            self.send_call_audio(dest, &plain)
        } else {
            self.send_sealed(dest, &plain, super::Pending::Control).map(|_| ())
        };
        if let Err(e) = sent {
            self.send_client(id, &err_ev(0, error_code::UNKNOWN_DEST, e));
        }
    }

    pub(super) fn on_call(&mut self, from: Address, data: &[u8]) {
        if self.call_test_on(from, data) {
            return;
        }
        let ev = Event::Call { from, data: maarse_core::companion::text(data) };
        self.broadcast(&ev);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_pages_and_index() {
        let dir = std::env::temp_dir().join(format!("maarse-pages-{}", std::process::id()));
        fs::create_dir_all(dir.join("weather")).unwrap();
        fs::write(dir.join("index.mu"), "# Hello").unwrap();
        fs::write(dir.join("about.mu"), "# About").unwrap();
        fs::write(dir.join("weather/index.mu"), "# Weather").unwrap();
        assert_eq!(find_page(&dir, "index"), Some(dir.join("index.mu")));
        assert_eq!(find_page(&dir, "about"), Some(dir.join("about.mu")));
        assert_eq!(find_page(&dir, "weather"), Some(dir.join("weather").join("index.mu")));
        assert_eq!(find_page(&dir, "missing"), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn compression_roundtrip() {
        let m = "# Heading\n\nText ".repeat(100);
        let c = compress(m.as_bytes());
        assert!(c.len() < m.len() / 5);
        assert_eq!(miniz_oxide::inflate::decompress_to_vec(&c).unwrap(), m.as_bytes());
    }
}
