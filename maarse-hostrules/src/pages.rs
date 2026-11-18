//! `maarse://` pages: the pure rules. Page path lookup, which picture copy a
//! path asks for, the copy size targets, the 4×4 dithering threshold, the
//! burst window and the size limit for a page answer. Reading files and
//! making pictures stay in `maarse-host`.

use maarse_core::content;

/// The page request burst window.
pub const WINDOW_MS: u64 = 30_000;
/// How long an asked-for page may take to come.
pub const ASKED_KEEP_MS: u64 = 600_000;
/// Page requests served per reader within [`WINDOW_MS`] when the reader
/// may be reached over LoRa (`--pages-burst-lora`) ...
pub const BURST_LORA: usize = 5;
/// ... and over the internet (`--pages-burst-net`).
pub const BURST_NET: usize = 60;
/// The most a picture (or page) may be over LoRa, whatever the page asks.
pub const LORA_PICTURE_MAX: usize = 8 * 1024;
/// The greyscale step aims at this size.
pub const GREY_TARGET: usize = 1400;
/// The middle colour step.
pub const COLOUR4_MAX: usize = 4 * 1024;
/// 4×4 ordered dithering matrix (Bayer) for the dots copy.
pub const BAYER4: [u8; 16] = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];

/// The files tried for a page path, in order: `path.mu`, then `path/index.mu`
/// (relative to `pages/`; the path has already been cleaned with
/// [`content::page_path`]).
pub fn page_candidates(path: &str) -> [String; 2] {
    [format!("{path}.mu"), format!("{path}/index.mu")]
}

/// A page or a picture on a page (`_style/file`), by its offer.
pub fn is_page_file(mime: &[u8], name: &[u8]) -> bool {
    mime == content::PAGE_MIME || (name.first() == Some(&b'_') && name.contains(&b'/'))
}

/// Which copy of a picture is asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicStyle {
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

/// Splits `_dots/hytte.jpg` into (style, `hytte.jpg`) when the rest is a
/// picture; `None` for pages.
pub fn picture_path(path: &str) -> Option<(PicStyle, &str)> {
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

/// Dots: is the pixel at (`x`, `y`) with contrast-stretched grey `v`
/// (0-255) a white dot?
pub fn dot_on(v: f32, x: u32, y: u32) -> bool {
    let t = (BAYER4[(y % 4 * 4 + x % 4) as usize] as f32 + 0.5) * 16.0;
    v > t
}

/// The largest page answer along a route: never over 8 KB when it may go
/// over LoRa (also an unknown route, where it would otherwise wait in the
/// mailbox).
pub fn page_max(via_lora: bool, max_bytes: usize) -> usize {
    if via_lora { max_bytes.min(LORA_PICTURE_MAX) } else { max_bytes }
}

/// What to do with a page request, given the times (ms) a reader was
/// served within the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Burst {
    /// Serve it.
    Serve,
    /// Too many: send a "Slow down" page once.
    SlowDown,
    /// Too many, and the reader has been told: stay quiet.
    Quiet,
}

/// The burst rule for one reader: drops times outside the window, then
/// decides, recording `now_ms` when the request is served or told off.
pub fn burst(times: &mut Vec<u64>, now_ms: u64, burst: usize) -> Burst {
    times.retain(|t| now_ms.saturating_sub(*t) < WINDOW_MS);
    if times.len() >= burst {
        // Say so once, so the reader is not left waiting; then stay quiet.
        if times.len() == burst {
            times.push(now_ms);
            return Burst::SlowDown;
        }
        return Burst::Quiet;
    }
    times.push(now_ms);
    Burst::Serve
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_lookup_order() {
        assert_eq!(page_candidates("weather/aarhus"), ["weather/aarhus.mu".to_string(), "weather/aarhus/index.mu".to_string()]);
    }

    #[test]
    fn page_files_by_offer() {
        assert!(is_page_file(content::PAGE_MIME, b"index"));
        assert!(is_page_file(b"image/webp", b"_grey/a.jpg"));
        assert!(!is_page_file(b"image/webp", b"a.jpg"));
    }

    #[test]
    fn picture_paths_and_styles() {
        assert_eq!(picture_path("_dots/hytte.jpg"), Some((PicStyle::Dots, "hytte.jpg")));
        assert_eq!(picture_path("vejr/kort.png"), Some((PicStyle::Grey, "vejr/kort.png")));
        assert_eq!(picture_path("_colour/a.webp"), Some((PicStyle::Colour, "a.webp")));
        assert_eq!(picture_path("_c4/a.jpg"), Some((PicStyle::Colour4, "a.jpg")));
        assert_eq!(picture_path("_c8/a.jpg"), Some((PicStyle::Colour8, "a.jpg")));
        assert_eq!(picture_path("_best/a.jpg"), Some((PicStyle::Colour8, "a.jpg")));
        assert_eq!(picture_path("_full/a.jpg"), Some((PicStyle::Full, "a.jpg")));
        assert_eq!(picture_path("_fdots/a.jpg"), Some((PicStyle::ForceDots, "a.jpg")));
        assert_eq!(picture_path("_fcolour/a.jpg"), Some((PicStyle::ForceColour, "a.jpg")));
        assert_eq!(picture_path("weather/index"), None);
        assert_eq!(picture_path("_dots/readme"), None);
    }

    #[test]
    fn dithering_covers_the_grey_scale() {
        // Black has no dots, white all, mid grey half.
        let count = |v: f32| (0..4).flat_map(|y| (0..4).map(move |x| (x, y))).filter(|&(x, y)| dot_on(v, x, y)).count();
        assert_eq!(count(0.0), 0);
        assert_eq!(count(255.0), 16);
        assert_eq!(count(128.0), 8);
        assert_eq!(dot_on(100.0, 1, 1), dot_on(100.0, 5, 9), "the pattern repeats every 4");
    }

    #[test]
    fn size_limits() {
        assert_eq!(page_max(true, 1 << 20), LORA_PICTURE_MAX);
        assert_eq!(page_max(true, 4096), 4096);
        assert_eq!(page_max(false, 1 << 20), 1 << 20);
        assert!(GREY_TARGET < COLOUR4_MAX && COLOUR4_MAX < LORA_PICTURE_MAX);
    }

    #[test]
    fn burst_window() {
        let mut t = Vec::new();
        assert_eq!(burst(&mut t, 0, 2), Burst::Serve);
        assert_eq!(burst(&mut t, 1, 2), Burst::Serve);
        assert_eq!(burst(&mut t, 2, 2), Burst::SlowDown);
        assert_eq!(burst(&mut t, 3, 2), Burst::Quiet);
        assert_eq!(burst(&mut t, 3 + WINDOW_MS, 2), Burst::Serve, "the window has passed");
    }
}
