//! `maarse://` pages: the host answers page requests (content type
//! `0x05`) with files from `<data>/pages/`, and clients can ask other nodes
//! for pages (`RequestPage`).
//!
//! A page is Må:rse markup in a `.mu` file (UTF-8). The path `weather/aarhus`
//! is looked up as `pages/weather/aarhus.mu` and then
//! `pages/weather/aarhus/index.mu`. The page is sent compressed (raw deflate) as
//! an ordinary file with MIME [`content::PAGE_MIME`] and the path as its name, so
//! it uses the same per-chunk acknowledgement and limit over LoRa as files.
//!
//! Pictures: a page may show `![caption](hytte.jpg "grey")`. The client asks
//! for the picture like a page, with the step as a first path segment. The
//! host makes every copy itself, as lossy WebP (the format with the most
//! picture per byte in our format test), in three steps:
//!
//! - `_grey/hytte.jpg`: **Greyscale**, about 1.3 KB. Over LoRa this always
//!   comes first, whatever the page asks (`_colour/…` too), unless dots
//!   (`_dots/…`, 1-bit PNG, best for drawings) are smaller.
//! - `_c4/hytte.jpg`: **Colour 4 KB**, asked for when the reader taps the
//!   picture for a better one.
//! - `_c8/hytte.jpg`: **Colour 8 KB**, the most LoRa carries.
//! - `_full/hytte.jpg`: **Full size**, the original, over the internet only
//!   (over LoRa it is Colour 8 KB).
//!
//! The steps are the same over the internet (where the first loads without
//! a tap), so a page looks the same on every route and no picture is ever
//! over 8 KB; the original is never sent. `_best/…` (older clients) is
//! Colour 8 KB.
//!
//! A page can force a copy with "!": `![map](kort.png "dots!")` is asked for
//! as `_fdots/kort.png` and always comes as that copy, over LoRa and over the
//! internet: `_fgrey` Greyscale, `_fdots` the dots, `_fcolour` Colour 8 KB.

use super::{Node, err_ev, short};
use crate::ClientId;
use maarse_core::Address;
use maarse_core::companion::{Command, Event, MAX_TEXT, error_code, text};
use rand_core::RngCore;
use maarse_core::content::{self, Small};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

// Page paths, picture copy sizes, dithering, the burst window and the call
// rule live in maarse-hostrules (shared with the simulator).
pub(crate) use maarse_hostrules::pages::{LORA_PICTURE_MAX, PicStyle, is_page_file, picture_path};
use maarse_hostrules::pages::{self as rules, COLOUR4_MAX, GREY_TARGET};

#[derive(Default)]
pub(super) struct Pages {
    /// The page server's settings (see `Config`).
    pub(super) off: bool,
    burst_lora: usize,
    burst_net: usize,
    pub(super) board_keep: usize,
    pub(super) board_every: u64,
    previews_off: bool,
    /// When each reader was served (node clock, ms).
    served: HashMap<Address, Vec<u64>>,
    /// Pages and pictures we asked for (node, path) and when: only those
    /// are taken when offered. Anything else that looks like a page is
    /// refused, so it never turns up as a file in a chat.
    asked: HashMap<(Address, Vec<u8>), Instant>,
    /// When each reader last wrote a note (unix seconds).
    pub(super) noted: HashMap<Address, u64>,
    /// Picture previews by file, with the file's time (made once).
    previews: std::cell::RefCell<HashMap<PathBuf, (std::time::SystemTime, Option<String>)>>,
}

/// How long an asked-for page may take to come.
const ASKED_KEEP: Duration = Duration::from_millis(rules::ASKED_KEEP_MS);

impl Pages {
    pub(super) fn new(cfg: &crate::Config) -> Self {
        Pages {
            off: cfg.no_pages,
            burst_lora: cfg.pages_burst_lora.max(1),
            burst_net: cfg.pages_burst_net.max(1),
            board_keep: cfg.board_keep.max(1),
            board_every: cfg.board_every,
            previews_off: cfg.no_previews,
            ..Default::default()
        }
    }
}

/// Finds the page's file in `root`. The path has already been cleaned with
/// [`content::page_path`], so it cannot point outside the folder.
pub(crate) fn find_page(root: &Path, path: &str) -> Option<PathBuf> {
    let base = root.join(path);
    let file = base.with_file_name(format!("{}.mu", base.file_name()?.to_string_lossy()));
    if crate::pagefiles::checked_path(root, &file).is_ok() && file.is_file() {
        return Some(file);
    }
    let idx = base.join("index.mu");
    (crate::pagefiles::checked_path(root, &idx).is_ok() && idx.is_file()).then_some(idx)
}

/// Compresses a page (raw deflate, like `DecompressionStream("deflate-raw")`).
pub(crate) fn compress(markup: &[u8]) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec(markup, 9)
}

/// Lossy WebP of `img` (grey pictures as RGB with equal channels: the
/// colour planes then cost almost nothing).
fn webp(img: &image::RgbImage, q: f32) -> Vec<u8> {
    webp::Encoder::from_rgb(img.as_raw(), img.width(), img.height()).encode(q).to_vec()
}

/// The largest, sharpest WebP that fits `limit`: each side from big to small,
/// quality from high down to `floor`, the first that fits. A smaller sharp
/// picture reads better than a big blocky one, hence the floor. If nothing
/// fits, the smallest tried.
fn webp_fit(img: &image::DynamicImage, grey: bool, limit: usize, sides: &[u32], floor: f32) -> Vec<u8> {
    let mut smallest: Option<Vec<u8>> = None;
    for &side in sides {
        let t = img.thumbnail(side, side);
        let rgb = if grey { image::DynamicImage::ImageLuma8(t.to_luma8()).to_rgb8() } else { t.to_rgb8() };
        let mut q = 80.0;
        while q >= floor {
            let b = webp(&rgb, q);
            if b.len() <= limit {
                return b;
            }
            if smallest.as_ref().is_none_or(|x| b.len() < x.len()) {
                smallest = Some(b);
            }
            q -= 10.0;
        }
    }
    smallest.unwrap_or_default()
}

/// Greyscale: about 1.3 KB, the first copy over LoRa.
pub(crate) fn grey_picture(img: &image::DynamicImage) -> Vec<u8> {
    webp_fit(img, true, GREY_TARGET, &[256, 224, 192, 160, 128, 96], 10.0)
}

/// Colour that fits `limit` (Colour 4 KB, Colour 8 KB, or an internet limit).
pub(crate) fn colour_picture(img: &image::DynamicImage, limit: usize) -> Option<Vec<u8>> {
    // Within the LoRa sizes a picture over 640 px never fits: skip those tries.
    const SIDES: [u32; 11] = [1280, 960, 800, 640, 560, 480, 400, 320, 256, 192, 160];
    let first = if limit <= LORA_PICTURE_MAX { 3 } else { 0 };
    let b = webp_fit(img, false, limit, &SIDES[first..], 30.0);
    (b.len() <= limit && !b.is_empty()).then_some(b)
}

/// Dots: longest side 256, contrast stretched (2 % cut at each end), 4×4
/// ordered dithering, written as a 1-bit grey PNG.
pub(crate) fn dots_picture(img: &image::DynamicImage) -> Vec<u8> {
    let g = img.thumbnail(256, 256).to_luma8();
    let (w, h) = g.dimensions();
    let mut hist = [0usize; 256];
    for p in g.pixels() {
        hist[p.0[0] as usize] += 1;
    }
    let cut = (w * h) as usize / 50;
    let (mut lo, mut s) = (0usize, 0usize);
    while lo < 255 {
        s += hist[lo];
        if s >= cut {
            break;
        }
        lo += 1;
    }
    let (mut hi, mut s) = (255usize, 0usize);
    while hi > 0 {
        s += hist[hi];
        if s >= cut {
            break;
        }
        hi -= 1;
    }
    let span = hi.saturating_sub(lo).max(1) as i32;
    let stride = w.div_ceil(8) as usize + 1;
    let mut raw = vec![0u8; stride * h as usize];
    for (x, y, p) in g.enumerate_pixels() {
        let v = ((p.0[0] as i32 - lo as i32) * 255 / span).clamp(0, 255) as f32;
        if rules::dot_on(v, x, y) {
            raw[y as usize * stride + 1 + (x as usize >> 3)] |= 0x80 >> (x & 7);
        }
    }
    png_grey1(w, h, &raw)
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
        }
    }
    !c
}

/// A 1-bit grey PNG from filtered rows (filter byte 0, then packed bits).
fn png_grey1(w: u32, h: u32, raw: &[u8]) -> Vec<u8> {
    let mut out = vec![137, 80, 78, 71, 13, 10, 26, 10];
    let mut chunk = |kind: &[u8; 4], body: &[u8]| {
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        let mut t = kind.to_vec();
        t.extend_from_slice(body);
        out.extend_from_slice(&t);
        out.extend_from_slice(&crc32(&t).to_be_bytes());
    };
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[1, 0, 0, 0, 0]);
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &miniz_oxide::deflate::compress_to_vec_zlib(raw, 9));
    chunk(b"IEND", &[]);
    out
}

pub(super) fn decode_picture(data: &[u8]) -> Option<image::DynamicImage> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(data)).with_guessed_format().ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    reader.decode().ok()
}

/// The bytes and MIME of a picture: the same steps on every route, so a
/// page looks the same over LoRa and over the internet and no picture is ever
/// over 8 KB. The first copy is Greyscale (or dots when smaller); Colour 4 KB
/// and Colour 8 KB on request; a forced copy ("!") as the page says.
pub(crate) fn picture_for(data: &[u8], name: &str, style: PicStyle, via_lora: bool, max: usize) -> Option<(Vec<u8>, &'static str)> {
    const WEBP: &str = "image/webp";
    // Full size over the internet: the original as it is when it fits.
    if style == PicStyle::Full && !via_lora {
        if data.len() <= max {
            let mime = match name.rsplit_once('.').map(|x| x.1) {
                Some("png") => "image/png",
                Some("webp") => "image/webp",
                _ => "image/jpeg",
            };
            return Some((data.to_vec(), mime));
        }
        return colour_picture(&decode_picture(data)?, max).map(|b| (b, WEBP));
    }
    let max = max.min(LORA_PICTURE_MAX);
    let img = decode_picture(data)?;
    match style {
        PicStyle::ForceGrey => Some((grey_picture(&img), WEBP)).filter(|(b, _)| b.len() <= max),
        PicStyle::ForceDots => {
            // A busy picture can make big dots: smaller until it fits.
            for side in [256, 192, 128, 96] {
                let d = dots_picture(&img.thumbnail(side, side));
                if d.len() <= max {
                    return Some((d, "image/png"));
                }
            }
            None
        }
        PicStyle::Colour4 => colour_picture(&img, max.min(COLOUR4_MAX)).map(|b| (b, WEBP)),
        // Over LoRa full size is never more than Colour 8 KB.
        PicStyle::Colour8 | PicStyle::ForceColour | PicStyle::Full => colour_picture(&img, max).map(|b| (b, WEBP)),
        PicStyle::Grey | PicStyle::Dots | PicStyle::Colour => {
            // The smallest first copy.
            let grey = grey_picture(&img);
            let dots = dots_picture(&img);
            if dots.len() < grey.len() && dots.len() <= max {
                Some((dots, "image/png"))
            } else {
                Some((grey, WEBP)).filter(|(b, _)| b.len() <= max)
            }
        }
    }
}

impl Node {
    pub(super) fn pages_dir(&self) -> PathBuf {
        self.data_dir.join("pages")
    }

    /// Markup for the answer to `path`.
    fn page_markup(&self, path: &str) -> String {
        let root = self.pages_dir();
        if !root.is_dir() || self.pages.off {
            return "# No pages\n\nThis node does not share pages.\n".into();
        }
        match find_page(&root, path).and_then(|f| crate::pagefiles::read_markup(&root, &f).ok()) {
            Some(m) => m,
            None => format!("# Not found\n\nThe page `{path}` does not exist here.\n\n[Front page](/)\n"),
        }
    }

    pub(super) fn on_page_request(&mut self, from: Address, path: &Small) {
        let now = self.now_ms();
        self.pages.served.retain(|_, v| {
            v.retain(|t| now.saturating_sub(*t) < rules::WINDOW_MS);
            !v.is_empty()
        });
        let lora = self.file_limit(&from).via_lora;
        let burst = if lora { self.pages.burst_lora } else { self.pages.burst_net };
        let times = self.pages.served.entry(from).or_default();
        let verdict = rules::burst(times, now, burst);
        if verdict != rules::Burst::Serve {
            self.log(format!("page request from {} rejected (too many)", short(&from)));
            if verdict == rules::Burst::SlowDown {
                let msg = format!("# Slow down

Too many pages at once: at most {burst} in {} seconds. Wait a moment and try again.

[Front page](/)
", rules::WINDOW_MS / 1000);
                let _ = self.send_page_reply(from, compress(msg.as_bytes()), Small::from_slice(content::PAGE_MIME).unwrap_or_default(), path.clone());
            }
            return;
        }

        let path_s = String::from_utf8_lossy(path).into_owned();
        let lim = self.file_limit(&from);
        let (data, mime) = self.page_answer(Some(from), &path_s, lim.via_lora, lim.max_bytes as usize);
        // Statistics for the server's dashboard: which page, how, and whether a lock turned the reader away.
        if self.tele.is_some() {
            let picture = picture_path(&path_s).is_some_and(|(_, r)| r != path_s.as_str());
            let (rel, file) = match picture_path(&path_s) { Some((_, r)) if picture => (r.to_string(), None), _ => (path_s.clone(), find_page(&self.pages_dir(), &path_s)) };
            let refused = matches!(self.access(Some(from), &rel, file.as_deref()), super::pagelock::Access::Locked | super::pagelock::Access::Hidden);
            if let Some(t) = self.tele.as_mut() {
                t.page(&rel, &from, lim.via_lora, data.len(), picture, refused);
            }
        }
        let route = if lim.via_lora { "LoRa" } else { "internet" };
        self.log(format!("page '{path_s}' ({route}) to {} ({} bytes)", short(&from), data.len()));
        if let Err((_, e)) = self.send_page_reply(from, data, mime, path.clone()) {
            self.log(format!("page to {}: {e}", short(&from)));
        }
    }

    /// The answer to a page request: the page (compressed markup) or a
    /// picture in the form the route allows; a short page saying so when it
    /// is missing or cannot be made to fit.
    fn page_answer(&self, reader: Option<Address>, path_s: &str, via_lora: bool, max_bytes: usize) -> (Vec<u8>, Small) {
        let page_mime = || Small::from_slice(content::PAGE_MIME).unwrap_or_default();
        let locked = |hidden: bool| {
            let me = reader.map(|a| a.0.iter().map(|b| format!("{b:02x}")).collect::<String>()).unwrap_or_default();
            let msg = maarse_hostrules::pagelock::refused_page(path_s, hidden, &me);
            (compress(msg.as_bytes()), page_mime())
        };
        if let Some((style, rel)) = picture_path(path_s) {
            match self.access(reader, rel, None) {
                super::pagelock::Access::Locked => return locked(false),
                super::pagelock::Access::Hidden => return locked(true),
                _ => {}
            }
            let root = self.pages_dir();
            let made = crate::pagefiles::read_bounded(&root, &root.join(rel)).ok()
                .and_then(|d| picture_for(&d, rel, style, via_lora, max_bytes));
            return match made {
                Some((d, m)) if d.len() <= max_bytes => (d, Small::from_slice(m.as_bytes()).unwrap_or_default()),
                _ => {
                    let msg = format!("# No picture

The picture `{rel}` is missing here or could not be made small enough.
");
                    (compress(msg.as_bytes()), page_mime())
                }
            };
        }
        let root = self.pages_dir();
        let file = find_page(&root, path_s);
        let mut info = String::new();
        if let Some(f) = &file {
            match self.access(reader, path_s, Some(f)) {
                super::pagelock::Access::Locked => return locked(false),
                super::pagelock::Access::Hidden => return locked(true),
                super::pagelock::Access::Member(n) => info = format!("@lockinfo {n}
"),
                super::pagelock::Access::Open => {}
            }
        }
        let markup = self.page_markup(path_s);
        let markup = match &file {
            Some(f) => {
                let m = self.with_notes(&super::pagelock::strip_locks(&markup), f, path_s);
                if self.pages.previews_off {
                    info + &m
                } else {
                    let mut token = |src: &str| self.preview(src);
                    info + &super::pagelock::with_previews(&m, &mut token)
                }
            }
            None => markup,
        };
        let data = compress(markup.as_bytes());
        // Never over 8 KB when the page may go over LoRa (also an unknown
        // route, where it would otherwise wait in the mailbox).
        let max = rules::page_max(via_lora, max_bytes);
        if data.len() > max {
            let msg = format!("# Too large

The page `{path_s}` is {} KB compressed; the limit here is {} KB.
",
                data.len().div_ceil(1024), max / 1024);
            return (compress(msg.as_bytes()), page_mime());
        }
        (data, page_mime())
    }

    /// The preview token of a picture in `pages/` (cached until the file changes).
    fn preview(&self, src: &str) -> Option<String> {
        let path = self.pages_dir().join(src.to_lowercase());
        if !path.starts_with(self.pages_dir()) || src.contains("..") {
            return None;
        }
        crate::pagefiles::checked_path(&self.pages_dir(), &path).ok()?;
        let modified = fs::metadata(&path).and_then(|m| m.modified()).ok()?;
        let mut cache = self.pages.previews.borrow_mut();
        if let Some((t, tok)) = cache.get(&path)
            && *t == modified
        {
            return tok.clone();
        }
        let tok = crate::pagefiles::read_bounded(&self.pages_dir(), &path).ok().and_then(|d| super::pagelock::preview_token(&d));
        cache.insert(path, (modified, tok.clone()));
        tok
    }

    /// Our own pages, asked for by one of our clients (the owner checking
    /// a page before others read it): answered at once, as the internet
    /// would see them, without going over any link.
    fn own_page(&mut self, id: ClientId, req: u16, path: &Small) {
        let path_s = String::from_utf8_lossy(path).into_owned();
        let (data, mime) = self.page_answer(None, &path_s, false, self.max_file as usize);
        self.send_client(id, &Event::Sent { req, msg_id: 0 });
        let mut b = [0u8; 8];
        rand_core::OsRng.fill_bytes(&mut b);
        let file_id = u64::from_le_bytes(b);
        self.send_client(id, &Event::FileIncoming {
            file_id,
            from: self.addr,
            size: data.len() as u32,
            mime: text(&mime),
            name: text(path),
        });
        for (i, part) in data.chunks(MAX_TEXT).enumerate() {
            self.send_client(id, &Event::FileChunk { file_id, offset: (i * MAX_TEXT) as u32, data: text(part) });
        }
        self.send_client(id, &Event::FileDone { file_id, ok: true, outgoing: false, text: text(path) });
    }

    /// `RequestPage` and `SendCall`. Returns the command if it does not
    /// belong here.
    pub(super) fn page_command(&mut self, id: ClientId, cmd: Command) -> Option<Command> {
        if let Command::SendCall { dest, data } = cmd {
            self.send_call(id, dest, &data);
            return None;
        }
        if let Command::PostBoard { req, dest, path, text: note } = cmd {
            if dest == self.addr {
                match self.on_board_post(self.addr, &String::from_utf8_lossy(&path), &note) {
                    Ok(()) => self.send_client(id, &Event::Sent { req, msg_id: 0 }),
                    Err(e) => self.send_client(id, &err_ev(req, error_code::TOO_LARGE, e)),
                }
                return None;
            }
            let Some(plain) = content::board_post(&path, &note) else {
                self.send_client(id, &err_ev(req, error_code::TOO_LARGE, "Invalid path or note too long (100 bytes)"));
                return None;
            };
            match self.send_sealed(dest, &plain, super::Pending::Control) {
                Ok(msg_id) => self.send_client(id, &Event::Sent { req, msg_id }),
                Err(e) => self.send_client(id, &err_ev(req, error_code::UNKNOWN_DEST, e)),
            }
            return None;
        }
        let Command::RequestPage { req, dest, path } = cmd else { return Some(cmd) };
        let Some(plain) = content::page_request(&path) else {
            self.send_client(id, &err_ev(req, error_code::TOO_LARGE, "Invalid path"));
            return None;
        };
        if dest == self.addr {
            if let Ok(p) = Small::from_slice(&plain[1..]) {
                self.own_page(id, req, &p);
            }
            return None;
        }
        self.log(format!("asking {} for the page '{}'", short(&dest), String::from_utf8_lossy(&plain[1..])));
        let now = Instant::now();
        self.pages.asked.retain(|_, t| now.duration_since(*t) < ASKED_KEEP);
        self.pages.asked.insert((dest, plain[1..].to_vec()), now);
        match self.send_sealed(dest, &plain, super::Pending::Control) {
            Ok(msg_id) => self.send_client(id, &Event::Sent { req, msg_id }),
            Err(e) => self.send_client(id, &err_ev(req, error_code::UNKNOWN_DEST, e)),
        }
        None
    }
}

impl Node {
    /// Sends an answer to a page request: marked so it never goes to the
    /// file mailbox.
    fn send_page_reply(&mut self, dest: Address, data: Vec<u8>, mime: Small, name: Small) -> Result<(u64, u16), (u8, &'static str)> {
        let r = self.send_file_bytes(dest, data, mime, name);
        if let Ok((id, _)) = r
            && let Some(f) = self.out_files.get_mut(&id)
        {
            f.page = true;
        }
        r
    }

    /// An offered page or page picture: did we ask that node for it?
    pub(super) fn page_was_asked(&self, from: &Address, name: &[u8]) -> bool {
        self.pages.asked.get(&(*from, name.to_vec())).is_some_and(|t| t.elapsed() < ASKED_KEEP)
    }

    /// Call data: only when the recipient is reached over TCP all the way.
    fn send_call(&mut self, id: ClientId, dest: Address, data: &[u8]) {
        if let Some((code, why)) = maarse_hostrules::calls::call_refusal(self.path_kind(&dest)) {
            self.send_client(id, &err_ev(0, code, why));
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

    fn is_webp(b: &[u8]) -> bool {
        b.len() > 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP"
    }

    #[test]
    fn picture_steps_for_lora() {
        // A 640×480 photo-like test picture: gradients plus noise.
        let mut rng = 1u32;
        let img = image::RgbImage::from_fn(640, 480, |x, y| {
            rng = rng.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let n = (rng >> 24) as u8 / 8;
            image::Rgb([(x / 3) as u8 / 2 + n, (y / 2) as u8 / 2 + n, 120 + n])
        });
        let mut src = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_with_encoder(image::codecs::jpeg::JpegEncoder::new_with_quality(&mut src, 90))
            .unwrap();
        let get = |st, lora| picture_for(&src, "a.jpg", st, lora, 1 << 20).unwrap();
        // Over LoRa the first copy is Greyscale (WebP, about 1.3 KB) whatever the page asks.
        let (grey, m) = get(PicStyle::Grey, true);
        assert_eq!(m, "image/webp");
        assert!(is_webp(&grey) && grey.len() <= GREY_TARGET, "greyscale {} bytes", grey.len());
        for st in [PicStyle::Dots, PicStyle::Colour] {
            assert_eq!(get(st, true).0.len(), grey.len(), "{st:?}");
        }
        // The better steps: Colour 4 KB, then Colour 8 KB, both WebP, each bigger.
        let (c4, m4) = get(PicStyle::Colour4, true);
        let (c8, m8) = get(PicStyle::Colour8, true);
        assert_eq!((m4, m8), ("image/webp", "image/webp"));
        assert!(grey.len() < c4.len() && c4.len() <= COLOUR4_MAX, "colour 4 KB: {} bytes", c4.len());
        assert!(c4.len() < c8.len() && c8.len() <= LORA_PICTURE_MAX, "colour 8 KB: {} bytes", c8.len());
        // They decode, and the colour steps are larger pictures.
        let w = |b: &[u8]| image::load_from_memory(b).unwrap().width();
        assert!(w(&c8) >= w(&c4) && w(&c4) >= w(&grey));
        // Over the internet the same steps (never the original).
        assert_eq!(get(PicStyle::Grey, false).0, grey);
        assert_eq!(get(PicStyle::Colour4, false).0, c4);
        assert_eq!(get(PicStyle::Colour8, false).0, c8);
        // Forced by the page: the same copy on both routes.
        for lora in [true, false] {
            assert_eq!(get(PicStyle::ForceGrey, lora).0, grey);
            assert_eq!(get(PicStyle::ForceDots, lora).1, "image/png");
        }
        for lora in [true, false] {
            assert_eq!(get(PicStyle::ForceColour, lora).0, c8);
        }
        // Full size: the original over the internet, Colour 8 KB over LoRa.
        assert_eq!(get(PicStyle::Full, false), (src.clone(), "image/jpeg"));
        assert_eq!(get(PicStyle::Full, true).0, c8);
        assert_eq!(picture_path("_full/a.jpg"), Some((PicStyle::Full, "a.jpg")));
        // Noise is the worst case: still within 8 KB over LoRa in every step.
        let mut noise = image::RgbImage::new(600, 600);
        let mut x = 1u32;
        for p in noise.pixels_mut() {
            x ^= x << 13; x ^= x >> 17; x ^= x << 5;
            *p = image::Rgb([x as u8, (x >> 8) as u8, (x >> 16) as u8]);
        }
        let mut nb = Vec::new();
        image::DynamicImage::ImageRgb8(noise).write_to(&mut std::io::Cursor::new(&mut nb), image::ImageFormat::Png).unwrap();
        for st in [PicStyle::Grey, PicStyle::ForceGrey, PicStyle::ForceDots, PicStyle::Colour4, PicStyle::Colour8, PicStyle::ForceColour] {
            let (b, _) = picture_for(&nb, "n.png", st, true, 1 << 20).unwrap_or_else(|| panic!("{st:?}: no copy"));
            assert!(b.len() <= LORA_PICTURE_MAX, "{st:?} noise over LoRa {} bytes", b.len());
        }
        println!("greyscale {} B, colour 4 KB {} B, colour 8 KB {} B", grey.len(), c4.len(), c8.len());
    }

    #[test]
    fn compression_roundtrip() {
        let m = "# Heading\n\nText ".repeat(100);
        let c = compress(m.as_bytes());
        assert!(c.len() < m.len() / 5);
        assert_eq!(miniz_oxide::inflate::decompress_to_vec(&c).unwrap(), m.as_bytes());
    }
}
