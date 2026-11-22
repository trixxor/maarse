//! The named pipe between the tray app (a normal user) and the service (SYSTEM).
//!
//! One request per connection: the client writes one line (`status`, `check`, `update` or `start`),
//! the service answers with one line of JSON ([`Status`]) and closes. Only the local interactive user
//! and administrators can open the pipe (see `pipe.rs`).

use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};
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
                let text = read_answer(&mut pipe).map_err(|e| e.to_string())?;
                return serde_json::from_str(text.trim()).map_err(|e| format!("unreadable answer: {e}"));
            }
            // 231 = all pipe instances busy: try again in a moment.
            Err(e) if e.raw_os_error() == Some(231) => {
                last = e.to_string();
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(e) => return Err(format!("the Må:rse service is not running ({e})")),
        }
    }
    Err(last)
}

/// Reads the answer: one line of JSON. The service closes the pipe with `DisconnectNamedPipe`, which makes
/// the next read fail with 233 (no process at the other end) instead of reporting the end of the data, so
/// that error, and 109 (broken pipe), count as the end once something has been read.
fn read_answer(pipe: &mut impl Read) -> io::Result<String> {
    let mut data = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match pipe.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                data.extend_from_slice(&buf[..n]);
                if data.contains(&b'\n') {
                    break;
                }
            }
            Err(e) if !data.is_empty() && matches!(e.raw_os_error(), Some(233 | 109)) => break,
            Err(e) => return Err(e),
        }
    }
    String::from_utf8(data).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Gives its data, then fails like a pipe the server has disconnected.
    struct Disconnected(Vec<u8>, bool);
    impl Read for Disconnected {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            if self.0.is_empty() {
                return if self.1 { Err(io::Error::from_raw_os_error(233)) } else { Ok(0) };
            }
            let n = self.0.len().min(out.len());
            out[..n].copy_from_slice(&self.0[..n]);
            self.0.drain(..n);
            Ok(n)
        }
    }

    #[test]
    fn answer_survives_a_disconnected_pipe() {
        // Without a newline: the data, then error 233, is still the whole answer.
        assert_eq!(read_answer(&mut Disconnected(b"{\"a\":1}".to_vec(), true)).unwrap(), "{\"a\":1}");
        assert_eq!(read_answer(&mut Disconnected(b"{\"a\":1}\n".to_vec(), false)).unwrap(), "{\"a\":1}\n");
        // Nothing at all before the error is a real error.
        assert!(read_answer(&mut Disconnected(Vec::new(), true)).is_err());
    }

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
