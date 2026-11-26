//! Spectrum recordings: the compact file a node (or a client) makes from a long run of spectrum
//! sweeps, and the code that reads it back.
//!
//! A sweep is one row of signal strengths (`Event::Sweep`, one byte per frequency step, `-dBm x 2`).
//! A recording of 10 minutes at 10 sweeps a second would be 384 KB. The [`Recorder`] keeps the
//! strongest and the mean value of every frequency in every time bucket (at most
//! [`MAX_BUCKETS`], so 5 s per bucket for 10 minutes) and writes a file of about 1 KB:
//!
//! * layer 1: noise floor, highest level and the share of time over the threshold for each frequency;
//! * layer 2: a time profile, the level over the floor in 3 dB steps (4 bit), run-length coded;
//! * layer 3: events, connected areas of the profile (6 bytes each).
//!
//! The file layout ("MSR1", all numbers little endian) is in `docs/protocol-spec.md`. The code has no
//! heap, so a node can record on its own; a client can run the same code (or a port that passes
//! `tests/data/sweeprec-golden.*`) to read a file.

use heapless::Vec;

/// First four bytes of a recording.
pub const MAGIC: [u8; 4] = *b"MSR1";
/// Frequency steps in a sweep (the only width a recording takes).
pub const BINS: usize = 64;
/// Most time buckets in a recording.
pub const MAX_BUCKETS: usize = 120;
/// Most events kept in a file.
pub const MAX_EVENTS: usize = 200;
/// Most groups [`Recording::groups`] makes.
pub const MAX_GROUPS: usize = 48;
/// Bytes in the header.
pub const HEADER_LEN: usize = 24;
const CELLS: usize = BINS * MAX_BUCKETS;
/// The largest possible file.
pub const MAX_FILE: usize = HEADER_LEN + BINS * 3 + MAX_BUCKETS.div_ceil(8) + 2 + CELLS + 2 + MAX_EVENTS * 6;
/// Longest recording in seconds (the bucket length has to fit 16 bits of milliseconds).
pub const MAX_DURATION_S: u16 = 7800;
/// dB per step in the time profile.
pub const STEP_DB: i16 = 3;

/// What to record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// First frequency of a sweep, in kHz.
    pub start_khz: u32,
    /// Distance between two steps, in kHz.
    pub step_khz: u16,
    /// How long the recording runs, in seconds (1 to [`MAX_DURATION_S`]).
    pub duration_s: u16,
    /// Time between the start of two sweeps, in ms (100 = back to back; at most 25 500).
    pub interval_ms: u16,
    /// How far over the noise floor a step has to be to count as activity, in dB.
    pub thr_db: u8,
}

/// A sweep where most steps read stronger than -60 dBm: a transmitter very close to the radio
/// overloads the receiver and every frequency looks strong. It says nothing about the band, so
/// it should be pushed as masked.
pub fn is_overload(rssi: &[u8]) -> bool {
    let valid = rssi.iter().filter(|&&v| v < 248).count();
    let strong = rssi.iter().filter(|&&v| v < 120).count();
    valid >= 8 && strong * 10 >= valid * 6
}

/// The file does not fit [`MAX_FILE`] (cannot happen with the limits above).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TooBig;

/// Collects sweeps and makes the file. About 31 KB of RAM.
pub struct Recorder {
    cfg: Config,
    bucket_ms: u32,
    nb: usize,
    used: usize,
    sweeps: u16,
    /// The strongest value (smallest `-dBm x 2`) of each cell, 255 = nothing; index `bin * MAX_BUCKETS + bucket`.
    best: [u8; CELLS],
    /// Sum of the values of each cell (the stack of the event search when the file is made).
    sum: [u16; CELLS],
    /// The profile (level over floor in [`STEP_DB`] steps, at most 15); bit 7 marks a visited cell while events are searched.
    grid: [u8; CELLS],
    /// Sweeps that were measured in each bucket.
    cnt: [u16; MAX_BUCKETS],
}

impl Recorder {
    /// A recorder, or `None` when the configuration does not fit a recording.
    pub fn new(cfg: Config) -> Option<Self> {
        if cfg.duration_s == 0 || cfg.duration_s > MAX_DURATION_S || cfg.step_khz == 0 {
            return None;
        }
        let interval = (cfg.interval_ms as u32).clamp(100, 25_500);
        let duration_ms = cfg.duration_s as u32 * 1000;
        let bucket_ms = (duration_ms / MAX_BUCKETS as u32).max(interval);
        let nb = (duration_ms.div_ceil(bucket_ms) as usize).min(MAX_BUCKETS);
        let mut cfg = cfg;
        cfg.interval_ms = interval as u16;
        Some(Recorder { cfg, bucket_ms, nb, used: 0, sweeps: 0, best: [255; CELLS], sum: [0; CELLS], grid: [0; CELLS], cnt: [0; MAX_BUCKETS] })
    }

    /// Length of one time bucket in ms.
    pub fn bucket_ms(&self) -> u32 {
        self.bucket_ms
    }

    /// Number of time buckets the recording has room for.
    pub fn buckets(&self) -> usize {
        self.nb
    }

    /// Takes one sweep. `t_ms` is the time since the recording began. `masked` marks a sweep that
    /// says nothing about the band (a known packet was on the air, or the receiver was overloaded):
    /// it counts as time but not as a measurement. Rows that are not [`BINS`] long are ignored.
    pub fn push(&mut self, t_ms: u32, rssi: &[u8], masked: bool) {
        if rssi.len() != BINS {
            return;
        }
        let k = ((t_ms / self.bucket_ms) as usize).min(self.nb - 1);
        self.used = self.used.max(k + 1);
        self.sweeps = self.sweeps.saturating_add(1);
        if masked {
            return;
        }
        let mut any = false;
        for (b, &v) in rssi.iter().enumerate() {
            if v >= 248 {
                continue;
            }
            any = true;
            let i = b * MAX_BUCKETS + k;
            if v < self.best[i] {
                self.best[i] = v;
            }
            self.sum[i] = self.sum[i].saturating_add(v as u16);
        }
        if any {
            self.cnt[k] = self.cnt[k].saturating_add(1);
        }
    }

    /// Makes the file. Call it once; the recorder is used up afterwards.
    pub fn finish(&mut self, out: &mut Vec<u8, MAX_FILE>) -> Result<(), TooBig> {
        let nb = self.used.max(1);
        // Noise floor of each frequency: the middle of its bucket means.
        let mut floor_raw = [236u8; BINS];
        for (b, fl) in floor_raw.iter_mut().enumerate() {
            let mut tmp = [0u8; MAX_BUCKETS];
            let mut n = 0;
            for k in 0..nb {
                let i = b * MAX_BUCKETS + k;
                if self.cnt[k] > 0 && self.best[i] != 255 {
                    tmp[n] = (self.sum[i] / self.cnt[k]).min(247) as u8;
                    n += 1;
                }
            }
            if n > 0 {
                tmp[..n].sort_unstable();
                *fl = tmp[n / 2];
            }
        }
        // A constant carrier must not count as floor: no frequency may sit more than 2 dB above the typical one.
        let mut sorted = floor_raw;
        sorted.sort_unstable();
        let typical = sorted[BINS / 2];
        for fl in floor_raw.iter_mut() {
            *fl = (*fl).max(typical.saturating_sub(4));
        }
        // The profile.
        let valid = (0..nb).filter(|&k| self.cnt[k] > 0).count();
        let mut max_raw = [255u8; BINS];
        let mut duty = [0u8; BINS];
        for b in 0..BINS {
            let mut hot = 0usize;
            for k in 0..nb {
                let i = b * MAX_BUCKETS + k;
                if self.cnt[k] == 0 || self.best[i] == 255 {
                    self.grid[i] = 0;
                    continue;
                }
                max_raw[b] = max_raw[b].min(self.best[i]);
                let d2 = floor_raw[b] as i16 - self.best[i] as i16;
                let v = if d2 >= 2 * self.cfg.thr_db as i16 { ((d2 + STEP_DB * 2 / 2) / (STEP_DB * 2)).clamp(1, 15) as u8 } else { 0 };
                self.grid[i] = v;
                if v > 0 {
                    hot += 1;
                }
            }
            duty[b] = if valid > 0 { ((hot * 100 + valid / 2) / valid) as u8 } else { 0 };
            if max_raw[b] == 255 {
                max_raw[b] = floor_raw[b];
            }
            // Buckets without a measurement take the lower of their neighbours, so a gap does not look like activity.
            let mut k = 0;
            while k < nb {
                if self.cnt[k] > 0 {
                    k += 1;
                    continue;
                }
                let mut k1 = k;
                while k1 + 1 < nb && self.cnt[k1 + 1] == 0 {
                    k1 += 1;
                }
                let v = if k > 0 && k1 + 1 < nb { self.grid[b * MAX_BUCKETS + k - 1].min(self.grid[b * MAX_BUCKETS + k1 + 1]) } else { 0 };
                for q in k..=k1 {
                    self.grid[b * MAX_BUCKETS + q] = v;
                }
                k = k1 + 1;
            }
        }
        // Events: connected areas of the profile.
        let mut events: Vec<Event, MAX_EVENTS> = Vec::new();
        for start in 0..CELLS {
            let (b0, k0) = (start / MAX_BUCKETS, start % MAX_BUCKETS);
            if k0 >= nb || self.grid[start] == 0 || self.grid[start] & 0x80 != 0 {
                continue;
            }
            let mut ev = Event { k0: k0 as u8, k1: k0 as u8, b0: b0 as u8, b1: b0 as u8, peak: 0 };
            let mut sp = 0usize;
            self.sum[0] = start as u16;
            sp += 1;
            self.grid[start] |= 0x80;
            while sp > 0 {
                sp -= 1;
                let x = self.sum[sp] as usize;
                let (b, k) = (x / MAX_BUCKETS, x % MAX_BUCKETS);
                let v = self.grid[x] & 0x7f;
                ev.b0 = ev.b0.min(b as u8);
                ev.b1 = ev.b1.max(b as u8);
                ev.k0 = ev.k0.min(k as u8);
                ev.k1 = ev.k1.max(k as u8);
                ev.peak = ev.peak.max(v * STEP_DB as u8);
                let mut visit = |nbin: usize, nk: usize| {
                    let y = nbin * MAX_BUCKETS + nk;
                    if self.grid[y] != 0 && self.grid[y] & 0x80 == 0 {
                        self.grid[y] |= 0x80;
                        self.sum[sp] = y as u16;
                        sp += 1;
                    }
                };
                if b > 0 {
                    visit(b - 1, k);
                }
                if b + 1 < BINS {
                    visit(b + 1, k);
                }
                if k > 0 {
                    visit(b, k - 1);
                }
                if k + 1 < nb {
                    visit(b, k + 1);
                }
            }
            let _ = events.push(ev);
        }
        for g in self.grid.iter_mut() {
            *g &= 0x7f;
        }
        // The file.
        out.clear();
        let duration_s = ((nb as u32 * self.bucket_ms).div_ceil(1000)).min(u16::MAX as u32) as u16;
        let put = |out: &mut Vec<u8, MAX_FILE>, bytes: &[u8]| out.extend_from_slice(bytes).map_err(|_| TooBig);
        put(out, &MAGIC)?;
        put(out, &[BINS as u8, self.cfg.thr_db, (self.cfg.interval_ms / 100) as u8, 0])?;
        put(out, &duration_s.to_le_bytes())?;
        put(out, &(self.bucket_ms as u16).to_le_bytes())?;
        put(out, &(nb as u16).to_le_bytes())?;
        put(out, &self.sweeps.to_le_bytes())?;
        put(out, &self.cfg.start_khz.to_le_bytes())?;
        put(out, &self.cfg.step_khz.to_le_bytes())?;
        put(out, &[0, 0])?;
        for fl in floor_raw {
            put(out, &[(-((fl >> 1) as i16)) as i8 as u8])?;
        }
        for m in max_raw {
            put(out, &[(-((m >> 1) as i16)) as i8 as u8])?;
        }
        put(out, &duty)?;
        let mut bitmap = [0u8; MAX_BUCKETS.div_ceil(8)];
        for k in 0..nb {
            if self.cnt[k] == 0 {
                bitmap[k >> 3] |= 1 << (k & 7);
            }
        }
        put(out, &bitmap[..nb.div_ceil(8)])?;
        let len_at = out.len();
        put(out, &[0, 0])?;
        for b in 0..BINS {
            let row = &self.grid[b * MAX_BUCKETS..b * MAX_BUCKETS + nb];
            let mut k = 0;
            while k < nb {
                let v = row[k];
                let mut run = 1;
                if v == 0 {
                    while k + run < nb && row[k + run] == 0 && run < 128 {
                        run += 1;
                    }
                    put(out, &[(run - 1) as u8])?;
                } else {
                    while k + run < nb && row[k + run] == v && run < 8 {
                        run += 1;
                    }
                    put(out, &[0x80 | (v << 3) | (run - 1) as u8])?;
                }
                k += run;
            }
        }
        let stream_len = (out.len() - len_at - 2) as u16;
        out[len_at..len_at + 2].copy_from_slice(&stream_len.to_le_bytes());
        put(out, &(events.len() as u16).to_le_bytes())?;
        for e in &events {
            put(out, &[e.k0, e.k1, e.b0, e.b1, e.peak, 0])?;
        }
        Ok(())
    }
}

/// A connected area of the time profile: buckets `k0..=k1`, frequency steps `b0..=b1`, strongest
/// level in dB over the floor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    pub k0: u8,
    pub k1: u8,
    pub b0: u8,
    pub b1: u8,
    pub peak: u8,
}

/// Why a file could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// Does not start with `MSR1`, or is too short.
    NotARecording,
    /// The header or a section does not fit the file or the limits.
    Damaged,
}

/// A recording that has been read.
pub struct Recording {
    pub thr_db: u8,
    pub interval_ms: u16,
    pub duration_s: u16,
    pub bucket_ms: u16,
    pub buckets: usize,
    pub sweeps: u16,
    pub start_khz: u32,
    pub step_khz: u16,
    /// Noise floor of each frequency, dBm.
    pub floor: [i8; BINS],
    /// Highest level of each frequency, dBm.
    pub max: [i8; BINS],
    /// Share of the measured time each frequency was over the threshold, percent.
    pub duty: [u8; BINS],
    /// Buckets without a measurement.
    pub masked: [bool; MAX_BUCKETS],
    /// Level over the floor in [`STEP_DB`] steps; index `bin * MAX_BUCKETS + bucket`.
    pub grid: [u8; CELLS],
    pub events: Vec<Event, MAX_EVENTS>,
}

impl Recording {
    /// Reads a file.
    pub fn parse(f: &[u8]) -> Result<Self, ParseError> {
        if f.len() < HEADER_LEN || f[..4] != MAGIC {
            return Err(ParseError::NotARecording);
        }
        let u16at = |o: usize| u16::from_le_bytes([f[o], f[o + 1]]);
        if f[4] as usize != BINS {
            return Err(ParseError::Damaged);
        }
        let buckets = u16at(12) as usize;
        if buckets == 0 || buckets > MAX_BUCKETS {
            return Err(ParseError::Damaged);
        }
        let mut r = Recording {
            thr_db: f[5],
            interval_ms: f[6] as u16 * 100,
            duration_s: u16at(8),
            bucket_ms: u16at(10),
            buckets,
            sweeps: u16at(14),
            start_khz: u32::from_le_bytes([f[16], f[17], f[18], f[19]]),
            step_khz: u16at(20),
            floor: [0; BINS],
            max: [0; BINS],
            duty: [0; BINS],
            masked: [false; MAX_BUCKETS],
            grid: [0; CELLS],
            events: Vec::new(),
        };
        let mut p = HEADER_LEN;
        let nbm = buckets.div_ceil(8);
        if f.len() < p + BINS * 3 + nbm + 2 {
            return Err(ParseError::Damaged);
        }
        for b in 0..BINS {
            r.floor[b] = f[p + b] as i8;
            r.max[b] = f[p + BINS + b] as i8;
            r.duty[b] = f[p + 2 * BINS + b];
        }
        p += BINS * 3;
        for k in 0..buckets {
            r.masked[k] = (f[p + (k >> 3)] >> (k & 7)) & 1 == 1;
        }
        p += nbm;
        let slen = u16at(p) as usize;
        p += 2;
        if f.len() < p + slen + 2 {
            return Err(ParseError::Damaged);
        }
        let (mut b, mut k) = (0usize, 0usize);
        for &t in &f[p..p + slen] {
            let (v, run) = if t & 0x80 != 0 { ((t >> 3) & 15, (t & 7) as usize + 1) } else { (0, (t & 127) as usize + 1) };
            for _ in 0..run {
                if b >= BINS {
                    return Err(ParseError::Damaged);
                }
                r.grid[b * MAX_BUCKETS + k] = v;
                k += 1;
                if k == buckets {
                    k = 0;
                    b += 1;
                }
            }
        }
        if b != BINS || k != 0 {
            return Err(ParseError::Damaged);
        }
        p += slen;
        let n = u16at(p) as usize;
        p += 2;
        if n > MAX_EVENTS || f.len() < p + n * 6 {
            return Err(ParseError::Damaged);
        }
        for i in 0..n {
            let o = p + i * 6;
            let e = Event { k0: f[o], k1: f[o + 1], b0: f[o + 2], b1: f[o + 3], peak: f[o + 4] };
            if e.k0 > e.k1 || e.k1 as usize >= buckets || e.b0 > e.b1 || e.b1 as usize >= BINS {
                return Err(ParseError::Damaged);
            }
            let _ = r.events.push(e);
        }
        Ok(r)
    }

    /// Frequency of a step in kHz.
    pub fn khz(&self, bin: usize) -> u32 {
        self.start_khz + self.step_khz as u32 * bin as u32
    }

    /// Length of the recording in ms (all buckets).
    pub fn length_ms(&self) -> u32 {
        self.buckets as u32 * self.bucket_ms as u32
    }

    /// Profile value of a cell (0 = under the threshold).
    pub fn cell(&self, bin: usize, bucket: usize) -> u8 {
        self.grid[bin * MAX_BUCKETS + bucket]
    }

    /// Joins events that look alike (about the same frequency and width) into groups and tells what each is.
    /// Sorted by time on the air, the longest first.
    pub fn groups(&self) -> Vec<Group, MAX_GROUPS> {
        let mut order: Vec<Event, MAX_EVENTS> = self.events.clone();
        order.sort_unstable_by_key(|e| (e.b0 as u16 + e.b1 as u16, e.k0, e.b0));
        // Members of each group (indexes into `order`) are found by a second pass over the same order.
        let mut gid = [0u8; MAX_EVENTS];
        let mut gs: Vec<(i32, i32, i32), MAX_GROUPS> = Vec::new(); // sum of 2 x centre, count, width of the first event
        for (i, e) in order.iter().enumerate() {
            let c2 = e.b0 as i32 + e.b1 as i32;
            let w = e.b1 as i32 - e.b0 as i32 + 1;
            let found = gs.iter().position(|&(sum, n, gw)| (sum - c2 * n).abs() <= 4 * n && (gw - w).abs() <= 3);
            let g = match found {
                Some(g) => g,
                None => {
                    if gs.push((0, 0, w)).is_err() {
                        gid[i] = 255;
                        continue;
                    }
                    gs.len() - 1
                }
            };
            gs[g].0 += c2;
            gs[g].1 += 1;
            gid[i] = g as u8;
        }
        let total_ms = self.length_ms() as i64;
        let bl = self.bucket_ms as i64;
        let mut out: Vec<Group, MAX_GROUPS> = Vec::new();
        for g in 0..gs.len() {
            let mut members: Vec<Event, MAX_EVENTS> = Vec::new();
            for (i, e) in order.iter().enumerate() {
                if gid[i] as usize == g {
                    let _ = members.push(*e);
                }
            }
            members.sort_unstable_by_key(|e| (e.k0, e.b0));
            let count = members.len() as u16;
            let mut air = 0i64;
            let (mut b0, mut b1, mut peak) = (u8::MAX, 0u8, 0u8);
            for e in &members {
                air += (e.k1 as i64 - e.k0 as i64 + 1) * bl;
                b0 = b0.min(e.b0);
                b1 = b1.max(e.b1);
                peak = peak.max(e.peak);
            }
            let first_ms = members[0].k0 as i64 * bl;
            // Mean time between two starts and whether it is regular.
            let mut sum_d = 0i64;
            for w in members.windows(2) {
                sum_d += (w[1].k0 as i64 - w[0].k0 as i64) * bl;
            }
            let nd = members.len() as i64 - 1;
            let mean = if nd > 0 { sum_d / nd } else { 0 };
            let mut var = 0i64;
            for w in members.windows(2) {
                let d = (w[1].k0 as i64 - w[0].k0 as i64) * bl;
                var += (d - mean) * (d - mean);
            }
            let regular = nd >= 2 && mean > 0 && var * 16 < mean * mean * nd;
            // Does a single long event get stronger?
            let mut grow_up = false;
            if count == 1 {
                let e = members[0];
                let len = e.k1 as usize - e.k0 as usize + 1;
                if len >= 6 {
                    let level = |k: usize| (e.b0 as usize..=e.b1 as usize).map(|b| self.cell(b, k) as i64).max().unwrap_or(0) * STEP_DB as i64;
                    let t = (len / 3).max(1);
                    let (mut a, mut c) = (0i64, 0i64);
                    for q in 0..t {
                        a += level(e.k0 as usize + q);
                        c += level(e.k1 as usize - q);
                    }
                    grow_up = c - a > 3 * t as i64;
                }
            }
            let cov = air * 100;
            let kind = if cov > 80 * total_ms {
                Kind::Constant
            } else if count >= 3 && regular {
                Kind::Periodic
            } else if count == 1 && cov > 15 * total_ms {
                if grow_up { Kind::Rising } else { Kind::Long }
            } else if count >= 2 {
                Kind::Pulses
            } else {
                Kind::Single
            };
            let severity = match kind {
                Kind::Single | Kind::Pulses => {
                    if count >= 3 || peak >= 27 { Severity::Medium } else { Severity::Low }
                }
                _ => {
                    if peak >= 18 || cov > 50 * total_ms || kind == Kind::Rising { Severity::High } else { Severity::Medium }
                }
            };
            let _ = out.push(Group {
                kind,
                severity,
                b0,
                b1,
                count,
                air_ms: air as u32,
                first_ms: first_ms as u32,
                peak_db: peak,
                interval_ms: mean as u32,
            });
        }
        out.sort_unstable_by_key(|g| (core::cmp::Reverse(g.air_ms), g.first_ms, g.b0));
        out
    }
}

/// What a group of events is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// On for most of the recording.
    Constant,
    /// Three or more at regular intervals.
    Periodic,
    /// One long event that gets stronger.
    Rising,
    /// One long event.
    Long,
    /// Two or more short ones without a pattern.
    Pulses,
    /// One short one.
    Single,
}

/// How much a group is worth a look.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Low,
    Medium,
    High,
}

/// Events that look alike.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Group {
    pub kind: Kind,
    pub severity: Severity,
    /// First and last frequency step.
    pub b0: u8,
    pub b1: u8,
    /// Number of events.
    pub count: u16,
    /// Total time on the air, ms.
    pub air_ms: u32,
    /// When it was first seen, ms from the start.
    pub first_ms: u32,
    /// Strongest level over the floor, dB.
    pub peak_db: u8,
    /// Mean time between starts, ms (0 for a single event).
    pub interval_ms: u32,
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec::Vec as StdVec;

    /// A deterministic scene, no randomness: a constant carrier on step 41, a burst every 47 s on
    /// steps 17-22, noise that rises on steps 54-62 from 35 % of the time, and a ripple in the floor.
    pub fn scene(duration_s: u32, interval_ms: u32) -> StdVec<(u32, [u8; BINS])> {
        let mut rows = StdVec::new();
        let n = duration_s * 1000 / interval_ms;
        let mut s: u32 = 12345;
        for i in 0..n {
            let t = i * interval_ms;
            let mut row = [0u8; BINS];
            for (b, cell) in row.iter_mut().enumerate() {
                // -dBm x 2: floor -118 dBm = 236, with a ripple of up to 2 half-dB and a little noise.
                s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                let mut v: i32 = 236 - ((b as i32 * 7) % 5) + ((s >> 24) as i32 % 5) - 2;
                if b == 41 {
                    v = v.min(194);
                }
                if (17..=22).contains(&b) && (t % 47_000) < 3_000 && t >= 5_000 {
                    v = v.min(176);
                }
                if (54..=62).contains(&b) && t * 100 / (duration_s * 1000) >= 35 {
                    let shape = 5 - (b as i32 - 58).abs();
                    let prog = ((t * 100 / (duration_s * 1000)) as i32 - 35).clamp(0, 45);
                    v -= shape.max(0) * prog * 28 / 45 / 5 * 2 / 2;
                }
                *cell = v.clamp(0, 247) as u8;
            }
            rows.push((t, row));
        }
        rows
    }

    fn record(duration_s: u16, interval_ms: u16) -> (Recorder, Vec<u8, MAX_FILE>) {
        let cfg = Config { start_khz: 863_000, step_khz: 110, duration_s, interval_ms, thr_db: 6 };
        let mut rec = Recorder::new(cfg).unwrap();
        for (t, row) in scene(duration_s as u32, interval_ms as u32) {
            rec.push(t, &row, false);
        }
        let mut out = Vec::new();
        rec.finish(&mut out).unwrap();
        (rec, out)
    }

    #[test]
    fn small_file_and_roundtrip() {
        let (_, f) = record(600, 2000);
        assert!(f.len() < 2000, "file is {} bytes", f.len());
        let r = Recording::parse(&f).unwrap();
        assert_eq!(r.buckets, 120);
        assert_eq!(r.bucket_ms, 5000);
        assert_eq!(r.start_khz, 863_000);
        assert_eq!(r.step_khz, 110);
        assert_eq!(r.khz(41), 863_000 + 110 * 41);
        assert!(r.duty[41] > 90, "carrier duty {}", r.duty[41]);
        assert!(r.duty[0] < 5);
    }

    #[test]
    fn finds_the_three_things() {
        let (_, f) = record(600, 2000);
        let g = Recording::parse(&f).unwrap().groups();
        let has = |k: Kind, lo: u8, hi: u8| g.iter().any(|x| x.kind == k && x.b0 >= lo && x.b1 <= hi);
        assert!(has(Kind::Constant, 39, 43), "{g:?}");
        assert!(has(Kind::Periodic, 16, 23), "{g:?}");
        assert!(g.iter().any(|x| matches!(x.kind, Kind::Rising | Kind::Long) && x.b0 >= 52 && x.b1 <= 62), "{g:?}");
        assert_eq!(g[0].severity, Severity::High);
    }

    #[test]
    fn dense_scan_gives_the_same_picture() {
        let (_, f) = record(120, 100);
        let r = Recording::parse(&f).unwrap();
        assert_eq!(r.buckets, 120);
        assert!(r.groups().iter().any(|x| x.kind == Kind::Constant));
    }

    #[test]
    fn masked_sweeps_do_not_make_events() {
        let cfg = Config { start_khz: 863_000, step_khz: 110, duration_s: 120, interval_ms: 1000, thr_db: 6 };
        let mut rec = Recorder::new(cfg).unwrap();
        let quiet = [236u8; BINS];
        let loud = [150u8; BINS];
        for i in 0..120u32 {
            let loudish = (40..60).contains(&i);
            rec.push(i * 1000, if loudish { &loud } else { &quiet }, loudish);
        }
        let mut out = Vec::new();
        rec.finish(&mut out).unwrap();
        let r = Recording::parse(&out).unwrap();
        assert!(r.events.is_empty());
        assert!(r.masked[45]);
        assert!(!r.masked[10]);
    }

    #[test]
    fn overload_is_recognised() {
        assert!(is_overload(&[100u8; BINS]));
        assert!(!is_overload(&[236u8; BINS]));
        assert!(!is_overload(&[100u8; 4]));
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!(Recording::parse(b"nope").err(), Some(ParseError::NotARecording));
        let (_, f) = record(60, 500);
        assert_eq!(Recording::parse(&f[..f.len() - 3]).err(), Some(ParseError::Damaged));
        assert!(Recorder::new(Config { start_khz: 0, step_khz: 0, duration_s: 10, interval_ms: 100, thr_db: 6 }).is_none());
        assert!(Recorder::new(Config { start_khz: 0, step_khz: 1, duration_s: 0, interval_ms: 100, thr_db: 6 }).is_none());
    }

    /// One line per group: what ports of the reader have to print for the golden file.
    fn groups_text(file: &[u8]) -> std::string::String {
        use std::fmt::Write;
        let mut t = std::string::String::new();
        for g in Recording::parse(file).unwrap().groups() {
            writeln!(t, "{:?} {:?} {} {} {} {} {} {} {}", g.kind, g.severity, g.b0, g.b1, g.count, g.air_ms, g.first_ms, g.peak_db, g.interval_ms).unwrap();
        }
        t
    }

    /// Writes the files that ports of the reader and the recorder have to match byte for byte:
    /// `tests/data/sweeprec-golden-sweeps.bin` (the input: u32 time in ms, 64 bytes, repeated) and
    /// `sweeprec-golden.msr` (the file). Run with `GOLDEN_WRITE=1 cargo test -p maarse-core golden`.
    #[test]
    fn golden() {
        let rows = scene(300, 1000);
        let mut input = StdVec::new();
        for (t, row) in &rows {
            input.extend_from_slice(&t.to_le_bytes());
            input.extend_from_slice(row);
        }
        let cfg = Config { start_khz: 863_000, step_khz: 110, duration_s: 300, interval_ms: 1000, thr_db: 6 };
        let mut rec = Recorder::new(cfg).unwrap();
        for (t, row) in &rows {
            rec.push(*t, row, false);
        }
        let mut out = Vec::new();
        rec.finish(&mut out).unwrap();
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data");
        if std::env::var("GOLDEN_WRITE").is_ok() {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("sweeprec-golden-sweeps.bin"), &input).unwrap();
            std::fs::write(dir.join("sweeprec-golden.msr"), &out[..]).unwrap();
            std::fs::write(dir.join("sweeprec-golden-groups.txt"), groups_text(&out)).unwrap();
        }
        assert_eq!(std::fs::read(dir.join("sweeprec-golden-sweeps.bin")).unwrap(), input);
        assert_eq!(std::fs::read(dir.join("sweeprec-golden.msr")).unwrap(), &out[..]);
        assert_eq!(std::fs::read_to_string(dir.join("sweeprec-golden-groups.txt")).unwrap(), groups_text(&out));
    }
}
