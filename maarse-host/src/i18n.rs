//! Language files for the web pages (client, server admin, login).
//!
//! The English text in the pages is the key; a language file (`i18n/<code>.json`) maps it to
//! the translation. The files in the repository's `i18n/` folder are built into the program.
//! A language can be added without rebuilding: put `<code>.json` in `<data>/i18n/`
//! (a file there replaces a built-in one with the same code).

use std::path::Path;

include!(concat!(env!("OUT_DIR"), "/i18n_builtin.rs"));

/// The script that translates the pages.
pub const SCRIPT: &str = include_str!("../../maarse-client/i18n.js");

/// A language code is short and plain, so it can never be a path.
fn valid_code(code: &str) -> bool {
    (2..=12).contains(&code.len()) && code.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The language file with this code: the data folder first, then the built-in one.
fn file(code: &str, data_dir: &Path) -> Option<String> {
    if !valid_code(code) {
        return None;
    }
    if let Ok(text) = std::fs::read_to_string(data_dir.join("i18n").join(format!("{code}.json"))) {
        if serde_json::from_str::<serde_json::Value>(&text).is_ok() {
            return Some(text);
        }
    }
    BUILTIN.iter().find(|(c, _)| *c == code).map(|(_, t)| (*t).to_string())
}

/// `[{"code":"da","name":"Dansk"}, ...]` for every language that is available.
fn index(data_dir: &Path) -> String {
    let mut codes: Vec<String> = BUILTIN.iter().map(|(c, _)| (*c).to_string()).collect();
    if let Ok(dir) = std::fs::read_dir(data_dir.join("i18n")) {
        for entry in dir.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|x| x == "json") {
                if let Some(code) = path.file_stem().and_then(|s| s.to_str()) {
                    if valid_code(code) && !codes.iter().any(|c| c == code) {
                        codes.push(code.to_string());
                    }
                }
            }
        }
    }
    codes.sort();
    let list: Vec<serde_json::Value> = codes
        .iter()
        .filter_map(|code| {
            let value: serde_json::Value = serde_json::from_str(&file(code, data_dir)?).ok()?;
            let name = value["meta"]["name"].as_str().unwrap_or(code).to_string();
            Some(serde_json::json!({"code": code, "name": name}))
        })
        .collect();
    serde_json::Value::Array(list).to_string()
}

/// The pages ask for `/i18n.js`, `/i18n/index.json` and `/i18n/<code>.json`.
/// Returns the content type and the body, or None for any other path.
pub fn route(path: &str, data_dir: &Path) -> Option<(&'static str, Vec<u8>)> {
    if path == "/i18n.js" {
        return Some(("text/javascript; charset=utf-8", SCRIPT.as_bytes().to_vec()));
    }
    let rest = path.strip_prefix("/i18n/")?;
    let json = "application/json; charset=utf-8";
    if rest == "index.json" {
        return Some((json, index(data_dir).into_bytes()));
    }
    let code = rest.strip_suffix(".json")?;
    file(code, data_dir).map(|text| (json, text.into_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_plain() {
        assert!(valid_code("da"));
        assert!(valid_code("pt-br"));
        assert!(!valid_code("../x"));
        assert!(!valid_code("D"));
        assert!(!valid_code("a/b"));
    }

    #[test]
    fn builtin_danish_is_served_and_listed() {
        let dir = Path::new("does-not-exist");
        let (_, body) = route("/i18n/da.json", dir).expect("da.json");
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["meta"]["code"], "da");
        assert_eq!(v["texts"]["Settings"], "Indstillinger");
        let (_, list) = route("/i18n/index.json", dir).unwrap();
        assert!(String::from_utf8(list).unwrap().contains("\"Dansk\""));
        assert!(route("/i18n/..%2Fx.json", dir).is_none());
        assert!(route("/i18n/xx.json", dir).is_none());
    }
}
