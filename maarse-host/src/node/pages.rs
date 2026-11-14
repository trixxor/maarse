//! `maarse://`-sider: hosten svarer på sideanmodninger (indholdstype
//! `0x05`) med filer fra `<data>/sider/`, og klienter kan bede andre noder
//! om sider (`RequestPage`).
//!
//! En side er Må:rse-markup i en `.mu`-fil (UTF-8). Stien `vejr/aarhus`
//! slås op som `sider/vejr/aarhus.mu` og derefter
//! `sider/vejr/aarhus/index.mu`. Siden sendes komprimeret (rå deflate) som
//! en almindelig fil med MIME [`content::PAGE_MIME`] og stien som navn, så
//! den bruger den samme kvittering pr. bid og grænse over LoRa som filer.

use super::{Node, err_ev, short};
use crate::ClientId;
use maarse_core::Address;
use maarse_core::companion::{Command, Event, error_code};
use maarse_core::content::{self, Small};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Højst så mange sider til samme node inden for [`WINDOW`], så ingen kan
/// bede os fylde luften med sider, men man stadig kan klikke et par links
/// hurtigt efter hinanden.
const BURST: usize = 5;
const WINDOW: Duration = Duration::from_secs(30);

#[derive(Default)]
pub(super) struct Pages {
    served: HashMap<Address, Vec<Instant>>,
}

/// Finder sidens fil i `root`. Stien er allerede renset med
/// [`content::page_path`], så den kan ikke pege uden for mappen.
pub(crate) fn find_page(root: &Path, path: &str) -> Option<PathBuf> {
    let base = root.join(path);
    let file = base.with_file_name(format!("{}.mu", base.file_name()?.to_string_lossy()));
    if file.is_file() {
        return Some(file);
    }
    let idx = base.join("index.mu");
    idx.is_file().then_some(idx)
}

/// Komprimerer en side (rå deflate, som `DecompressionStream("deflate-raw")`).
pub(crate) fn compress(markup: &[u8]) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec(markup, 9)
}

impl Node {
    fn pages_dir(&self) -> PathBuf {
        self.data_dir.join("sider")
    }

    /// Markup til svaret på `path`.
    fn page_markup(&self, path: &str) -> String {
        let root = self.pages_dir();
        if !root.is_dir() {
            return "# Ingen sider\n\nDenne node har ingen sider.\n".into();
        }
        match find_page(&root, path).and_then(|f| fs::read_to_string(f).ok()) {
            Some(m) => m,
            None => format!("# Ikke fundet\n\nSiden `{path}` findes ikke her.\n\n[Forsiden](/)\n"),
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
            self.log(format!("sideanmodning fra {} afvist (for mange)", short(&from)));
            return;
        }
        times.push(now);

        let path_s = String::from_utf8_lossy(path).into_owned();
        let mut data = compress(self.page_markup(&path_s).as_bytes());
        let max = self.file_limit(&from).max_bytes as usize;
        if data.len() > max {
            let msg = format!("# For stor\n\nSiden `{path_s}` er {} KB komprimeret; grænsen hertil er {} KB.\n",
                data.len().div_ceil(1024), max / 1024);
            data = compress(msg.as_bytes());
        }
        let mime = Small::from_slice(content::PAGE_MIME).unwrap_or_default();
        self.log(format!("side '{path_s}' til {} ({} bytes)", short(&from), data.len()));
        if let Err((_, e)) = self.send_file_bytes(from, data, mime, path.clone()) {
            self.log(format!("side til {}: {e}", short(&from)));
        }
    }

    /// `RequestPage`. Returnerer kommandoen, hvis den ikke hører til her.
    pub(super) fn page_command(&mut self, id: ClientId, cmd: Command) -> Option<Command> {
        let Command::RequestPage { req, dest, path } = cmd else { return Some(cmd) };
        let Some(plain) = content::page_request(&path) else {
            self.send_client(id, &err_ev(req, error_code::TOO_LARGE, "Ugyldig sti"));
            return None;
        };
        self.log(format!("beder {} om siden '{}'", short(&dest), String::from_utf8_lossy(&plain[1..])));
        match self.send_sealed(dest, &plain, super::Pending::Control) {
            Ok(msg_id) => self.send_client(id, &Event::Sent { req, msg_id }),
            Err(e) => self.send_client(id, &err_ev(req, error_code::UNKNOWN_DEST, e)),
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_pages_and_index() {
        let dir = std::env::temp_dir().join(format!("maarse-sider-{}", std::process::id()));
        fs::create_dir_all(dir.join("vejr")).unwrap();
        fs::write(dir.join("index.mu"), "# Hej").unwrap();
        fs::write(dir.join("om.mu"), "# Om").unwrap();
        fs::write(dir.join("vejr/index.mu"), "# Vejr").unwrap();
        assert_eq!(find_page(&dir, "index"), Some(dir.join("index.mu")));
        assert_eq!(find_page(&dir, "om"), Some(dir.join("om.mu")));
        assert_eq!(find_page(&dir, "vejr"), Some(dir.join("vejr").join("index.mu")));
        assert_eq!(find_page(&dir, "mangler"), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn compression_roundtrip() {
        let m = "# Overskrift\n\nTekst ".repeat(100);
        let c = compress(m.as_bytes());
        assert!(c.len() < m.len() / 5);
        assert_eq!(miniz_oxide::inflate::decompress_to_vec(&c).unwrap(), m.as_bytes());
    }
}
