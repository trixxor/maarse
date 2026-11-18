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
use maarse_core::companion::{Command, Event, error_code};
use maarse_core::content::{self, Small};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// At most this many pages and pictures to the same node within [`WINDOW`]
/// over LoRa, so nobody can make us fill the air, while one can still click a
/// couple of links in quick succession.
const BURST: usize = 5;
/// The same over the internet, where only a runaway client is stopped: a page
/// with a few pictures is several requests at once.
const BURST_TCP: usize = 60;
const WINDOW: Duration = Duration::from_secs(30);

#[derive(Default)]
pub(super) struct Pages {
    served: HashMap<Address, Vec<Instant>>,
    /// Pages and pictures we asked for (node, path) and when: only those
    /// are taken when offered. Anything else that looks like a page is
    /// refused, so it never turns up as a file in a chat.
    asked: HashMap<(Address, Vec<u8>), Instant>,
}

/// How long an asked-for page may take to come.
const ASKED_KEEP: Duration = Duration::from_secs(600);

/// A page or a picture on a page (`_style/file`), by its offer.
pub(super) fn is_page_file(mime: &[u8], name: &[u8]) -> bool {
    mime == content::PAGE_MIME || (name.first() == Some(&b'_') && name.contains(&b'/'))
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

/// Which copy of a picture is asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PicStyle {
    Grey,
    Dots,
    Colour,
    /// Colour 4 KB and Colour 8 KB: asked for when the reader wants a better
    /// picture than the greyscale one.
    Colour4,
    Colour8,
    /// Full size: the original over the internet (scaled to the limit only
    /// if it is larger); over LoRa never more than Colour 8 KB.
    Full,
    /// Forced by the page ("grey!", "dots!", "colour!"): this copy on any
    /// route, never over 8 KB over LoRa.
    ForceGrey,
    ForceDots,
    ForceColour,
}

/// The most a picture may be over LoRa, whatever the page asks.
pub(crate) const LORA_PICTURE_MAX: usize = 8 * 1024;
/// The greyscale step aims at this size.
const GREY_TARGET: usize = 1400;
/// The middle colour step.
const COLOUR4_MAX: usize = 4 * 1024;

/// Splits `_dots/hytte.jpg` into (style, `hytte.jpg`) when the rest is a
/// picture; `None` for pages.
pub(crate) fn picture_path(path: &str) -> Option<(PicStyle, &str)> {
    let (style, rest) = match path.split_once('/') {
        Some(("_grey", r)) => (PicStyle::Grey, r),
        Some(("_dots", r)) => (PicStyle::Dots, r),
        Some(("_colour", r)) => (PicStyle::Colour, r),
        Some(("_c4", r)) => (PicStyle::Colour4, r),
        Some(("_c8" | "_best", r)) => (PicStyle::Colour8, r),
        Some(("_full", r)) => (PicStyle::Full, r),
        Some(("_fgrey", r)) => (PicStyle::ForceGrey, r),
        Some(("_fdots", r)) => (PicStyle::ForceDots, r),
        Some(("_fcolour", r)) => (PicStyle::ForceColour, r),
        _ => (PicStyle::Grey, path),
    };
    let ext = rest.rsplit_once('.')?.1;
    ["jpg", "jpeg", "png", "webp"].contains(&ext).then_some((style, rest))
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

const BAYER4: [u8; 16] = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];

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
        let t = (BAYER4[(y % 4 * 4 + x % 4) as usize] as f32 + 0.5) * 16.0;
        if v > t {
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
        return colour_picture(&image::load_from_memory(data).ok()?, max).map(|b| (b, WEBP));
    }
    let max = max.min(LORA_PICTURE_MAX);
    let img = image::load_from_memory(data).ok()?;
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
        let lora = self.file_limit(&from).via_lora;
        let burst = if lora { BURST } else { BURST_TCP };
        let times = self.pages.served.entry(from).or_default();
        if times.len() >= burst {
            // Say so once, so the reader is not left waiting; then stay quiet.
            let first = times.len() == burst;
            if first {
                times.push(now);
            }
            self.log(format!("page request from {} rejected (too many)", short(&from)));
            if first {
                let msg = format!("# Slow down

Too many pages at once: at most {burst} in {} seconds. Wait a moment and try again.

[Front page](/)
", WINDOW.as_secs());
                let _ = self.send_page_reply(from, compress(msg.as_bytes()), Small::from_slice(content::PAGE_MIME).unwrap_or_default(), path.clone());
            }
            return;
        }
        times.push(now);

        let path_s = String::from_utf8_lossy(path).into_owned();
        if let Some((style, rel)) = picture_path(&path_s) {
            let rel = rel.to_string();
            self.serve_picture(from, path, style, rel);
            return;
        }
        let mut data = compress(self.page_markup(&path_s).as_bytes());
        let lim = self.file_limit(&from);
        // Never over 8 KB when the page may go over LoRa (also an unknown
        // route, where it would otherwise wait in the mailbox).
        let max = if lim.via_lora { (lim.max_bytes as usize).min(LORA_PICTURE_MAX) } else { lim.max_bytes as usize };
        if data.len() > max {
            let msg = format!("# Too large\n\nThe page `{path_s}` is {} KB compressed; the limit here is {} KB.\n",
                data.len().div_ceil(1024), max / 1024);
            data = compress(msg.as_bytes());
        }
        let mime = Small::from_slice(content::PAGE_MIME).unwrap_or_default();
        self.log(format!("page '{path_s}' to {} ({} bytes)", short(&from), data.len()));
        if let Err((_, e)) = self.send_page_reply(from, data, mime, path.clone()) {
            self.log(format!("page to {}: {e}", short(&from)));
        }
    }

    /// Sends a picture from `pages/` in the form the route allows; a short
    /// page saying so when the picture is missing or cannot be made to fit.
    fn serve_picture(&mut self, from: Address, path: &Small, style: PicStyle, rel: String) {
        let lim = self.file_limit(&from);
        let max = lim.max_bytes as usize;
        let made = fs::read(self.pages_dir().join(&rel)).ok().and_then(|d| picture_for(&d, &rel, style, lim.via_lora, max));
        let (data, mime) = match made {
            Some((d, m)) if d.len() <= max => (d, Small::from_slice(m.as_bytes()).unwrap_or_default()),
            _ => {
                let msg = format!("# No picture\n\nThe picture `{rel}` is missing here or could not be made small enough.\n");
                (compress(msg.as_bytes()), Small::from_slice(content::PAGE_MIME).unwrap_or_default())
            }
        };
        let route = if lim.via_lora { "LoRa" } else { "internet" };
        self.log(format!("picture '{rel}' ({style:?}, {route}) to {} ({} bytes)", short(&from), data.len()));
        if let Err((_, e)) = self.send_page_reply(from, data, mime, path.clone()) {
            self.log(format!("picture to {}: {e}", short(&from)));
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
    fn picture_paths_and_styles() {
        assert_eq!(picture_path("_dots/hytte.jpg"), Some((PicStyle::Dots, "hytte.jpg")));
        assert_eq!(picture_path("vejr/kort.png"), Some((PicStyle::Grey, "vejr/kort.png")));
        assert_eq!(picture_path("_colour/a.webp"), Some((PicStyle::Colour, "a.webp")));
        assert_eq!(picture_path("_c4/a.jpg"), Some((PicStyle::Colour4, "a.jpg")));
        assert_eq!(picture_path("_c8/a.jpg"), Some((PicStyle::Colour8, "a.jpg")));
        assert_eq!(picture_path("_best/a.jpg"), Some((PicStyle::Colour8, "a.jpg")));
        assert_eq!(picture_path("_fdots/a.jpg"), Some((PicStyle::ForceDots, "a.jpg")));
        assert_eq!(picture_path("_fcolour/a.jpg"), Some((PicStyle::ForceColour, "a.jpg")));
        assert_eq!(picture_path("weather/index"), None);
        assert_eq!(picture_path("_dots/readme"), None);
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
