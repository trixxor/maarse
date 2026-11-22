//! The named pipe between the tray app (a normal user) and the service (SYSTEM).
//!
//! One request per connection: the client writes one line (`status`, `check`, `update` or `start`),
//! the service answers with one line of JSON ([`Status`]) and closes. Only the local interactive user
//! and administrators can open the pipe (see `pipe.rs`).

use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::time::Duration;

pub const PIPE: &str = r"\\.\pipe\maarse-service";
pub const SERVICE_NAME: &str = "Maarse";

/// A newer release the service has found.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateInfo {
    pub version: String,
    /// The release notes written on GitHub.
    pub notes: String,
    pub page: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Status {
    /// Version of the running service (and of the files next to it).
    pub version: String,
    /// `starting`, `running`, `stopped` or `updating`.
    pub state: String,
    /// Where the web client is, with the access key as `#token=...`.
    pub url: String,
    /// The last problem worth showing (a failed update), else empty.
    pub error: String,
    pub update: Option<UpdateInfo>,
}

/// Sends one command to the service and returns its answer.
pub fn request(command: &str) -> Result<Status, String> {
    let mut last = String::new();
    for _ in 0..5 {
        match std::fs::OpenOptions::new().read(true).write(true).open(PIPE) {
            Ok(mut pipe) => {
                pipe.write_all(format!("{command}\n").as_bytes()).map_err(|e| e.to_string())?;
                let mut text = String::new();
                pipe.read_to_string(&mut text).map_err(|e| e.to_string())?;
                return serde_json::from_str(text.trim()).map_err(|e| format!("unreadable answer: {e}"));
            }
            // 231 = all pipe instances busy: try again in a moment.
            Err(e) if e.raw_os_error() == Some(231) => {
                last = e.to_string();
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(e) => return Err(format!("the Maarse service is not running ({e})")),
        }
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_round_trips() {
        let s = Status {
            version: "0.8.0".into(),
            state: "running".into(),
            url: "http://127.0.0.1:8080/#token=abc".into(),
            error: String::new(),
            update: Some(UpdateInfo { version: "0.8.1".into(), notes: "Fixes".into(), page: "https://x".into() }),
        };
        let back: Status = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(s, back);
        assert!(serde_json::from_str::<Status>("{}").is_ok(), "missing fields default");
    }
}
