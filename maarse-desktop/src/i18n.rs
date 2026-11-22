//! Language of the tray app: the Windows display language, English as fallback.
//!
//! The English text in the code is the key; the language files in `i18n/` (shared with the web client
//! and the Android app) map it to the translation. They are built in. A file in
//! `%ProgramData%\Maarse\i18n\<code>.json` adds a language or replaces a built-in one.

use std::collections::HashMap;
use std::sync::OnceLock;

use windows_sys::Win32::Globalization::{GetUserDefaultUILanguage, LCIDToLocaleName};

include!(concat!(env!("OUT_DIR"), "/i18n_builtin.rs"));

static DICT: OnceLock<HashMap<String, String>> = OnceLock::new();

fn user_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var_os("ProgramData").unwrap_or_else(|| r"C:\ProgramData".into())).join("Maarse").join("i18n")
}

/// The Windows display language as a tag like "da-dk".
fn ui_tag() -> String {
    let mut buf = [0u16; 85];
    let n = unsafe { LCIDToLocaleName(GetUserDefaultUILanguage() as u32, buf.as_mut_ptr(), buf.len() as i32, 0) };
    if n <= 1 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..(n - 1) as usize]).to_lowercase()
}

fn file(code: &str) -> Option<String> {
    if code.is_empty() || !code.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') {
        return None;
    }
    if let Ok(text) = std::fs::read_to_string(user_dir().join(format!("{code}.json"))) {
        if serde_json::from_str::<serde_json::Value>(&text).is_ok() {
            return Some(text);
        }
    }
    BUILTIN.iter().find(|(c, _)| *c == code).map(|(_, t)| (*t).to_string())
}

fn load() -> HashMap<String, String> {
    let tag = ui_tag();
    let base = tag.split('-').next().unwrap_or("").to_string();
    for code in [tag.as_str(), base.as_str()] {
        if code == "en" || code.starts_with("en-") {
            return HashMap::new();
        }
        if let Some(text) = file(code) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Some(texts) = v["texts"].as_object() {
                    return texts.iter().filter_map(|(k, v)| Some((norm(k), v.as_str()?.to_string()))).collect();
                }
            }
        }
    }
    HashMap::new()
}

fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Translates a text, keeping the white space around it; a text that is missing in the language
/// stays English. Runs of white space in the text count as one space, as in the other programs.
pub fn tr(text: &str) -> String {
    let body = text.trim();
    if body.is_empty() {
        return text.to_string();
    }
    let lead = &text[..text.len() - text.trim_start().len()];
    let tail = &text[text.trim_end().len()..];
    match DICT.get_or_init(load).get(&norm(body)) {
        Some(v) => format!("{lead}{v}{tail}"),
        None => text.to_string(),
    }
}

/// Translates a text with `{0}`, `{1}` ... and fills in the arguments.
pub fn tr_args(text: &str, args: &[&str]) -> String {
    let mut s = tr(text);
    for (i, a) in args.iter().enumerate() {
        s = s.replace(&format!("{{{i}}}"), a);
    }
    s
}
