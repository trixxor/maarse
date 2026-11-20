use maarse_core::Address;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

const MAX_AGE: u32 = 7 * 24 * 3600;
const MAX_FUTURE: u32 = 10 * 60;
const RECORD_BYTES: usize = 20;
const MAX_ENTRIES: usize = 16_384;
const LOG_LIMIT: usize = MAX_ENTRIES * RECORD_BYTES * 2;

pub(super) struct AnnounceFreshness {
    path: PathBuf,
    latest: HashMap<Address, u32>,
    log_bytes: usize,
    next_prune: u32,
}

impl AnnounceFreshness {
    pub(super) fn open(path: &Path, now: u32) -> io::Result<Self> {
        let mut latest = HashMap::new();
        let mut log_bytes = 0;
        match OpenOptions::new().read(true).write(true).open(path) {
            Ok(mut file) => {
                let mut record = [0; RECORD_BYTES];
                loop {
                    match file.read_exact(&mut record) {
                        Ok(()) => {
                            log_bytes += RECORD_BYTES;
                            if log_bytes > LOG_LIMIT {
                                return Err(io::Error::new(io::ErrorKind::InvalidData, "announce freshness log exceeds its limit"));
                            }
                            let address = Address::from_slice(&record[..16]).unwrap();
                            let stamp = u32::from_le_bytes(record[16..].try_into().unwrap());
                            if stamp != 0 && stamp >= now.saturating_sub(MAX_AGE) {
                                let entry = latest.entry(address).or_insert(stamp);
                                *entry = (*entry).max(stamp);
                            }
                        }
                        Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => {
                            if file.metadata()?.len() != log_bytes as u64 {
                                file.set_len(log_bytes as u64)?;
                            }
                            break;
                        }
                        Err(err) => return Err(err),
                    }
                }
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
        Ok(Self { path: path.to_owned(), latest, log_bytes, next_prune: now })
    }

    /// The caller has verified the announce signature and destination.
    pub(super) fn accept(&mut self, address: Address, stamp: u32, now: u32) -> io::Result<bool> {
        let previous = self.latest.get(&address).copied();
        if stamp == 0 {
            // A clockless sender can only be used while we lack a fresh dated statement.
            return Ok(previous.is_none_or(|known| known < now.saturating_sub(MAX_AGE)));
        }
        if stamp < now.saturating_sub(MAX_AGE) || stamp > now.saturating_add(MAX_FUTURE) {
            return Ok(false);
        }
        if let Some(previous) = previous {
            if stamp <= previous {
                return Ok(stamp == previous);
            }
        }
        let new_address = previous.is_none();
        if self.log_bytes >= LOG_LIMIT
            || (new_address && self.latest.len() >= MAX_ENTRIES && now >= self.next_prune)
        {
            self.compact(now)?;
        }
        if new_address && self.latest.len() >= MAX_ENTRIES {
            return Ok(false);
        }
        let mut file = OpenOptions::new().append(true).create(true).open(&self.path)?;
        let record = Self::record(address, stamp);
        if let Err(err) = file.write_all(&record).and_then(|()| file.sync_data()) {
            // Leave the latest accepted record intact if an append was interrupted.
            let _ = file.set_len(self.log_bytes as u64);
            return Err(err);
        }
        self.log_bytes += RECORD_BYTES;
        self.latest.insert(address, stamp);
        Ok(true)
    }

    fn record(address: Address, stamp: u32) -> [u8; RECORD_BYTES] {
        let mut record = [0; RECORD_BYTES];
        record[..16].copy_from_slice(&address.0);
        record[16..].copy_from_slice(&stamp.to_le_bytes());
        record
    }

    fn compact(&mut self, now: u32) -> io::Result<()> {
        self.latest.retain(|_, stamp| *stamp >= now.saturating_sub(MAX_AGE));
        let temp = self.path.with_extension("tmp");
        let mut file = File::create(&temp)?;
        for (address, stamp) in &self.latest {
            file.write_all(&Self::record(*address, *stamp))?;
        }
        file.sync_all()?;
        drop(file);
        std::fs::rename(temp, &self.path)?;
        self.log_bytes = self.latest.len() * RECORD_BYTES;
        self.next_prune = now.saturating_add(3600);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_newer_dated_announce_survives_restart() {
        let dir = std::env::temp_dir().join(format!("maarse-announce-floor-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("freshness.bin");
        let _ = std::fs::remove_file(&path);
        let now = 2_000_000_000;
        let addr = Address([1; 16]);
        let mut state = AnnounceFreshness::open(&path, now).unwrap();
        assert!(state.accept(addr, 0, now).unwrap());
        assert!(state.accept(addr, now, now).unwrap());
        let mut state = AnnounceFreshness::open(&path, now + 5).unwrap();
        assert!(!state.accept(addr, now - 1, now + 5).unwrap());
        assert!(!state.accept(addr, 0, now + 5).unwrap());
        assert!(!state.accept(addr, now + MAX_FUTURE + 6, now + 5).unwrap());
        assert!(state.accept(addr, now + 4, now + 5).unwrap());
        let _ = std::fs::remove_file(path);
    }
}
