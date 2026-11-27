//! Spectrum recordings: the compact file a node (or a client) makes from a long run of spectrum
//! sweeps, and the code that reads it back.
//!
//! A sweep is one row of signal strengths (`Event::Sweep`, one byte per frequency step, `-dBm x 2`).
//! A recording of 5 minutes at 10 sweeps a second would be 205 KB. The [`Recorder`] keeps the strongest value of
//! every frequency in every time bucket of about a second (at most [`MAX_BUCKETS`]), turns it into a level over the
//! frequency's usual peak in steps of 3 dB (4 bit), and writes a file of a few hundred bytes (format `MSR2`):
//!
//! * a header with the floor of every frequency (a base and a 4-bit difference each);
//! * the level of every cell, range coded (one adaptive model, the neighbours in time and frequency as context).
//!
//! The events (connected areas of the profile) and the groups made from them are found by the reader, not stored.
//! Files of the first format (`MSR1`, events in the file) are still read.
//!
//! The recorder needs no RAM of its own to speak of: the cells (4 bit each, 8 KB) and a work area of 8 KB are lent
//! by the caller, see [`Tables`].

#![allow(clippy::needless_range_loop)]

use heapless::Vec;

/// First bytes of a file of the first format.
pub const MAGIC: [u8; 4] = *b"MSR1";
/// First bytes of a file of the current format.
pub const MAGIC2: [u8; 4] = *b"MSR2";
/// Frequency steps in a recording.
pub const BINS: usize = 64;
/// Most time buckets in a recording.
pub const MAX_BUCKETS: usize = 256;
/// Most buckets in a file of the first format.
pub const MAX_BUCKETS_V1: usize = 120;
/// Shortest time bucket in ms: the strongest of fewer sweeps than about ten is mostly noise.
pub const MIN_BUCKET_MS: u32 = 1000;
/// Most events the reader keeps.
pub const MAX_EVENTS: usize = 200;
/// Most groups the reader makes.
pub const MAX_GROUPS: usize = 48;
/// Longest recording in seconds.
pub const MAX_DURATION_S: u16 = 7800;
/// dB per step in the profile.
pub const STEP_DB: i16 = 3;
/// Length of the header of the first format.
const HEADER_V1: usize = 24;
/// Length of the header of the current format, with the floors.
const HEADER2: usize = 21 + BINS / 2;
/// Largest file the recorder can make.
pub const MAX_FILE: usize = 6400;

/// What to record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// First frequency of a sweep, in kHz.
    pub start_khz: u32,
    /// Distance between two steps, in kHz.
    pub step_khz: u16,
    /// How long the recording runs, in seconds (1 to [`MAX_DURATION_S`]).
    pub duration_s: u16,
    /// Time between the start of two sweeps, in ms (100 = back to back; at most 25 500). It is only written to the
    /// file; the time buckets are as short as [`MIN_BUCKET_MS`] allows.
    pub interval_ms: u16,
    /// How far over the usual peak a step has to be to count as activity, in dB.
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

/// The file does not fit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TooBig;

/// Cells of a recording: [`BINS`] frequencies by [`MAX_BUCKETS`] time buckets.
pub const CELLS: usize = BINS * MAX_BUCKETS;
/// Bytes of cells (4 bit each).
pub const CELL_BYTES: usize = CELLS / 2;

// Where things sit in the work area. Firmware lends the two 8 KB buffers it keeps for file transfers (a node does not
// transfer files while it records), so a recording costs no RAM of its own: the ESP32's stack is what is left of its RAM
// and has none to spare. None of the offsets is a multiple of 256: the Xtensa compiler (ESP32-S3 firmware) fails with
// "Cannot select" on such a constant added to an address.
//
// the strongest value of each step in the bucket being filled | the usual peak of each step | buckets without a
// measurement (a bit each) | the first buckets, kept until the usual peaks are known | where the file is made.
const W_CUR: usize = 0;
const W_FLOOR: usize = 64;
const W_NONE: usize = 128;
const W_WARM: usize = 160;
/// Buckets kept at the start, until the usual peak of each step can be taken from them.
const WARM: usize = 16;
/// Contexts for "is the cell above the threshold": 9 (left, up) times 2 (the one before the left).
const Z_CTX: usize = 18;
/// Contexts for the level: the level of a neighbour (16) times the 16 nodes of a 4-bit tree.
const PROBS: usize = Z_CTX + 16 * 16;
const W_OUT: usize = W_WARM + WARM * BINS + 4;
/// Smallest work area.
pub const WORK_LEN: usize = 8192;
/// The largest file [`Recorder::finish_in_place`] can make.
pub const MAX_FILE_IN_PLACE: usize = WORK_LEN - W_OUT;

/// Where a recording keeps its cells: two tables the caller lends for as long as the recording runs.
pub struct Tables<'a> {
    /// The level of every cell, 4 bit each, at least [`CELL_BYTES`]; cell `bin * MAX_BUCKETS + bucket`.
    pub cells: &'a mut [u8],
    /// A work area of at least [`WORK_LEN`] bytes. [`Recorder::finish_in_place`] makes the file here.
    pub work: &'a mut [u8],
}

fn cell_get(c: &[u8], idx: usize) -> u8 {
    (c[idx >> 1] >> ((idx & 1) * 4)) & 15
}

fn cell_set(c: &mut [u8], idx: usize, v: u8) {
    let s = (idx & 1) * 4;
    c[idx >> 1] = (c[idx >> 1] & !(15 << s)) | (v << s);
}

/// Somewhere to write the file.
trait Sink {
    fn put(&mut self, bytes: &[u8]) -> Result<(), TooBig>;
}

impl<const N: usize> Sink for Vec<u8, N> {
    fn put(&mut self, bytes: &[u8]) -> Result<(), TooBig> {
        self.extend_from_slice(bytes).map_err(|_| TooBig)
    }
}

struct SliceSink<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl Sink for SliceSink<'_> {
    fn put(&mut self, bytes: &[u8]) -> Result<(), TooBig> {
        let end = self.len + bytes.len();
        if end > self.buf.len() {
            return Err(TooBig);
        }
        self.buf[self.len..end].copy_from_slice(bytes);
        self.len = end;
        Ok(())
    }
}

// ---------- range coder (the one of LZMA: 11-bit probabilities, shift 5) ----------

const PROB_INIT: u16 = 1024;

struct RcEnc<'a, S: Sink> {
    out: &'a mut S,
    low: u64,
    range: u32,
    cache: u8,
    cache_size: u64,
    /// The first byte is always 0 and is not written.
    first: bool,
    /// Zero bytes not written yet: the reader reads zeros past the end, so a tail of zeros is left out.
    zeros: usize,
    failed: bool,
}

impl<'a, S: Sink> RcEnc<'a, S> {
    fn new(out: &'a mut S) -> Self {
        RcEnc { out, low: 0, range: 0xFFFF_FFFF, cache: 0, cache_size: 1, first: true, zeros: 0, failed: false }
    }

    fn byte(&mut self, b: u8) {
        if self.first {
            self.first = false;
            return;
        }
        if b == 0 {
            self.zeros += 1;
            return;
        }
        while self.zeros > 0 {
            self.zeros -= 1;
            self.failed |= self.out.put(&[0]).is_err();
        }
        self.failed |= self.out.put(&[b]).is_err();
    }

    fn shift_low(&mut self) {
        if (self.low as u32) < 0xFF00_0000 || (self.low >> 32) != 0 {
            let carry = (self.low >> 32) as u8;
            let mut temp = self.cache;
            loop {
                self.byte(temp.wrapping_add(carry));
                temp = 0xFF;
                self.cache_size -= 1;
                if self.cache_size == 0 {
                    break;
                }
            }
            self.cache = ((self.low >> 24) & 0xFF) as u8;
        }
        self.cache_size += 1;
        self.low = (self.low & 0x00FF_FFFF) << 8;
    }

    fn bit(&mut self, p: &mut u16, bit: bool) {
        let bound = (self.range >> 11) * (*p as u32);
        if !bit {
            self.range = bound;
            *p += (2048 - *p) >> 5;
        } else {
            self.low += bound as u64;
            self.range -= bound;
            *p -= *p >> 5;
        }
        while self.range < (1 << 24) {
            self.range <<= 8;
            self.shift_low();
        }
    }

    fn finish(mut self) -> Result<(), TooBig> {
        for _ in 0..5 {
            self.shift_low();
        }
        if self.failed {
            Err(TooBig)
        } else {
            Ok(())
        }
    }
}

struct RcDec<'a> {
    data: &'a [u8],
    pos: usize,
    range: u32,
    code: u32,
}

impl<'a> RcDec<'a> {
    fn new(data: &'a [u8]) -> Self {
        let mut d = RcDec { data, pos: 0, range: 0xFFFF_FFFF, code: 0 };
        for _ in 0..4 {
            d.code = (d.code << 8) | d.next() as u32;
        }
        d
    }

    fn next(&mut self) -> u8 {
        let b = self.data.get(self.pos).copied().unwrap_or(0);
        self.pos += 1;
        b
    }

    fn bit(&mut self, p: &mut u16) -> bool {
        let bound = (self.range >> 11) * (*p as u32);
        let bit = if self.code < bound {
            self.range = bound;
            *p += (2048 - *p) >> 5;
            false
        } else {
            self.code -= bound;
            self.range -= bound;
            *p -= *p >> 5;
            true
        };
        while self.range < (1 << 24) {
            self.range <<= 8;
            self.code = (self.code << 8) | self.next() as u32;
        }
        bit
    }
}

/// The context of a cell: the cell before it in time (`a`), the one before that (`a2`) and the one for the frequency
/// below (`u`). Returns the context for "above the threshold" and the one for the level.
fn contexts(a: u8, a2: u8, u: u8) -> (usize, usize) {
    let z = (a.min(2) * 3 + u.min(2)) as usize + if a2 > 0 { 9 } else { 0 };
    let v = if a > 0 { a } else { u } as usize;
    (z, v)
}

// ---------- the recorder ----------

/// Collects sweeps and makes the file. Only a few bytes of its own: the cells and the rest live in [`Tables`].
pub struct Recorder {
    cfg: Config,
    bucket_ms: u32,
    nb: usize,
    used: usize,
    sweeps: u16,
    /// The bucket being filled.
    cur_k: usize,
    /// Sweeps measured in it.
    cur_cnt: u16,
    /// The usual peak of each step is known.
    warm_done: bool,
    /// The strongest value (smallest) seen.
    peak_raw: u8,
}

fn median64(v: &[u8]) -> u8 {
    let mut s = [0u8; BINS];
    s.copy_from_slice(&v[..BINS]);
    crate::sort::sort_by(&mut s[..], |a, b| a < b);
    s[BINS / 2]
}

impl Recorder {
    /// A recorder that is not recording. It is `const`, so firmware can keep one in static RAM;
    /// [`Recorder::start`] begins a recording.
    pub const fn empty() -> Self {
        Recorder { cfg: Config { start_khz: 0, step_khz: 0, duration_s: 0, interval_ms: 0, thr_db: 0 }, bucket_ms: 1000, nb: 1, used: 0, sweeps: 0, cur_k: 0, cur_cnt: 0, warm_done: false, peak_raw: 255 }
    }

    /// Clears the tables and begins a recording. `false` when the configuration does not fit or the tables are too short.
    pub fn start(&mut self, cfg: Config, t: &mut Tables) -> bool {
        if cfg.duration_s == 0 || cfg.duration_s > MAX_DURATION_S || cfg.step_khz == 0 || t.cells.len() < CELL_BYTES || t.work.len() < WORK_LEN {
            return false;
        }
        let duration_ms = cfg.duration_s as u32 * 1000;
        let bucket_ms = duration_ms.div_ceil(MAX_BUCKETS as u32).max(MIN_BUCKET_MS);
        let mut cfg = cfg;
        cfg.interval_ms = (cfg.interval_ms as u32).clamp(100, 25_500) as u16;
        self.cfg = cfg;
        self.bucket_ms = bucket_ms;
        self.nb = (duration_ms.div_ceil(bucket_ms) as usize).min(MAX_BUCKETS);
        self.used = 0;
        self.sweeps = 0;
        self.cur_k = 0;
        self.cur_cnt = 0;
        self.warm_done = false;
        self.peak_raw = 255;
        t.cells[..CELL_BYTES].fill(0);
        t.work[W_CUR..W_CUR + BINS].fill(255);
        t.work[W_FLOOR..W_FLOOR + BINS].fill(236);
        t.work[W_NONE..W_NONE + MAX_BUCKETS / 8].fill(0);
        true
    }

    /// Length of one time bucket in ms.
    pub fn bucket_ms(&self) -> u32 {
        self.bucket_ms
    }

    /// Number of time buckets the recording has room for.
    pub fn buckets(&self) -> usize {
        self.nb
    }

    /// Sweeps taken so far.
    pub fn sweeps(&self) -> u16 {
        self.sweeps
    }

    /// Takes one sweep. `t_ms` is the time since the recording began. `masked` marks a sweep that
    /// says nothing about the band (a known packet was on the air, or the receiver was overloaded):
    /// it counts as time but not as a measurement. Rows that are not [`BINS`] long are ignored.
    pub fn push(&mut self, t: &mut Tables, t_ms: u32, rssi: &[u8], masked: bool) {
        if rssi.len() != BINS {
            return;
        }
        let k = ((t_ms / self.bucket_ms) as usize).min(self.nb - 1).max(self.cur_k);
        while self.cur_k < k {
            self.close_bucket(t);
            self.cur_k += 1;
        }
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
            if v < t.work[W_CUR + b] {
                t.work[W_CUR + b] = v;
            }
            self.peak_raw = self.peak_raw.min(v);
        }
        if any {
            self.cur_cnt = self.cur_cnt.saturating_add(1);
        }
    }

    /// The usual peak of each step, with no step stronger than 2 dB over the middle of all steps (a constant carrier must
    /// not count as the floor).
    fn floors(t: &Tables) -> [u8; BINS] {
        let typical = median64(&t.work[W_FLOOR..W_FLOOR + BINS]);
        let mut f = [0u8; BINS];
        for b in 0..BINS {
            f[b] = t.work[W_FLOOR + b].max(typical.saturating_sub(4));
        }
        f
    }

    /// Writes the levels of bucket `k`, given the strongest value of each step in it (255 = none).
    fn levels(&self, t: &mut Tables, k: usize, row: &[u8; BINS], f: &[u8; BINS]) {
        for b in 0..BINS {
            let best = row[b];
            if best == 255 {
                continue;
            }
            let d2 = f[b] as i16 - best as i16;
            let v = if d2 >= 2 * self.cfg.thr_db as i16 { ((d2 + STEP_DB) / (STEP_DB * 2)).clamp(1, 15) as u8 } else { 0 };
            cell_set(t.cells, b * MAX_BUCKETS + k, v);
        }
    }

    /// The first buckets are complete: the usual peak of each step is the middle of its values in them, and their
    /// levels are made.
    fn finish_warm(&mut self, t: &mut Tables, n: usize) {
        for b in 0..BINS {
            let mut tmp = [0u8; WARM];
            let mut m = 0;
            for k in 0..n {
                let v = t.work[W_WARM + k * BINS + b];
                if v != 255 {
                    tmp[m] = v;
                    m += 1;
                }
            }
            if m > 0 {
                crate::sort::sort_by(&mut tmp[..m], |a, b| a < b);
                t.work[W_FLOOR + b] = tmp[m / 2];
            }
        }
        let f = Self::floors(t);
        for k in 0..n {
            let mut row = [0u8; BINS];
            row.copy_from_slice(&t.work[W_WARM + k * BINS..W_WARM + (k + 1) * BINS]);
            self.levels(t, k, &row, &f);
        }
        self.warm_done = true;
    }

    /// The bucket being filled is complete.
    fn close_bucket(&mut self, t: &mut Tables) {
        let k = self.cur_k;
        let mut row = [0u8; BINS];
        row.copy_from_slice(&t.work[W_CUR..W_CUR + BINS]);
        if self.cur_cnt == 0 {
            t.work[W_NONE + (k >> 3)] |= 1 << (k & 7);
        }
        if !self.warm_done {
            t.work[W_WARM + k * BINS..W_WARM + (k + 1) * BINS].copy_from_slice(&row);
            if k == WARM - 1 {
                self.finish_warm(t, WARM);
            }
        } else if self.cur_cnt > 0 {
            let f = Self::floors(t);
            self.levels(t, k, &row, &f);
            // The usual peak follows the band slowly: half a dB a bucket towards each value.
            for b in 0..BINS {
                let best = row[b];
                let fl = t.work[W_FLOOR + b];
                if best != 255 {
                    if best > fl {
                        t.work[W_FLOOR + b] = fl + 1;
                    } else if best < fl {
                        t.work[W_FLOOR + b] = fl - 1;
                    }
                }
            }
        }
        t.work[W_CUR..W_CUR + BINS].fill(255);
        self.cur_cnt = 0;
    }

    /// Makes the file into `out`. Call it once; the recorder is used up afterwards.
    pub fn finish<const N: usize>(&mut self, t: &mut Tables, out: &mut Vec<u8, N>) -> Result<(), TooBig> {
        self.end(t);
        out.clear();
        self.write(t, out)
    }

    /// Makes the file in the work area, and returns its length: the file is `t.work[..length]`. Call it once. The
    /// file may be at most [`MAX_FILE_IN_PLACE`] bytes.
    pub fn finish_in_place(&mut self, t: &mut Tables) -> Result<usize, TooBig> {
        self.end(t);
        let mut file = [0u8; HEADER2 + MAX_BUCKETS / 8];
        let head = self.header(t, &mut file);
        // The file is made after the model in the work area, then moved to its start.
        let (_, out) = t.work.split_at_mut(W_OUT);
        let mut sink = SliceSink { buf: out, len: 0 };
        sink.put(&file[..head])?;
        Self::code_profile(t.cells, self.used.max(1), &mut sink)?;
        let n = sink.len;
        t.work.copy_within(W_OUT..W_OUT + n, 0);
        Ok(n)
    }

    /// The last bucket is complete, and the usual peaks are known.
    fn end(&mut self, t: &mut Tables) {
        if self.used > 0 {
            self.close_bucket(t);
            if !self.warm_done {
                self.finish_warm(t, self.used.min(WARM));
            }
        } else {
            self.used = 1;
            t.work[W_NONE] |= 1;
        }
    }

    /// The header, with the floors and the buckets without a measurement. Returns its length.
    fn header(&self, t: &Tables, h: &mut [u8]) -> usize {
        let nb = self.used.max(1);
        let f = if self.warm_done { Self::floors(t) } else { [236; BINS] };
        let mut dbm = [0i8; BINS];
        for b in 0..BINS {
            dbm[b] = -((f[b] >> 1) as i16) as i8;
        }
        let mut sorted = dbm;
        crate::sort::sort_by(&mut sorted[..], |a, b| a < b);
        let base = sorted[BINS / 2];
        let any_none = (0..nb).any(|k| (t.work[W_NONE + (k >> 3)] >> (k & 7)) & 1 == 1);
        h[..4].copy_from_slice(&MAGIC2);
        h[4] = any_none as u8;
        h[5] = self.cfg.thr_db;
        h[6..8].copy_from_slice(&(self.bucket_ms as u16).to_le_bytes());
        h[8..10].copy_from_slice(&(nb as u16).to_le_bytes());
        h[10..12].copy_from_slice(&self.sweeps.to_le_bytes());
        h[12..16].copy_from_slice(&self.cfg.start_khz.to_le_bytes());
        h[16..18].copy_from_slice(&self.cfg.step_khz.to_le_bytes());
        let peak = if self.peak_raw == 255 { base } else { -((self.peak_raw >> 1) as i16) as i8 };
        h[18] = peak as u8;
        h[19] = (self.cfg.interval_ms / 100) as u8;
        h[20] = base as u8;
        for b in 0..BINS / 2 {
            let d = |i: usize| ((dbm[i] as i16 - base as i16).clamp(-8, 7) & 15) as u8;
            h[21 + b] = d(2 * b) | (d(2 * b + 1) << 4);
        }
        let mut n = HEADER2;
        if any_none {
            let m = nb.div_ceil(8);
            h[n..n + m].copy_from_slice(&t.work[W_NONE..W_NONE + m]);
            n += m;
        }
        n
    }

    /// The level of every cell, range coded.
    fn code_profile<S: Sink>(cells: &[u8], nb: usize, out: &mut S) -> Result<(), TooBig> {
        let mut p = [PROB_INIT; PROBS];
        let mut enc = RcEnc::new(out);
        for b in 0..BINS {
            for k in 0..nb {
                let v = cell_get(cells, b * MAX_BUCKETS + k);
                let a = if k > 0 { cell_get(cells, b * MAX_BUCKETS + k - 1) } else { 0 };
                let a2 = if k > 1 { cell_get(cells, b * MAX_BUCKETS + k - 2) } else { 0 };
                let u = if b > 0 { cell_get(cells, (b - 1) * MAX_BUCKETS + k) } else { 0 };
                let (zc, vc) = contexts(a, a2, u);
                enc.bit(&mut p[zc], v != 0);
                if v != 0 {
                    let x = v - 1;
                    let mut node = 1usize;
                    for i in (0..4).rev() {
                        let bit = (x >> i) & 1 == 1;
                        enc.bit(&mut p[Z_CTX + vc * 16 + node], bit);
                        node = node * 2 + bit as usize;
                    }
                }
            }
        }
        enc.finish()
    }

    fn write<S: Sink>(&self, t: &Tables, out: &mut S) -> Result<(), TooBig> {
        let mut file = [0u8; HEADER2 + MAX_BUCKETS / 8];
        let head = self.header(t, &mut file);
        out.put(&file[..head])?;
        Self::code_profile(t.cells, self.used.max(1), out)
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
    /// Level over the floor in [`STEP_DB`] steps (the usual peak of the step in a file of the current format); index
    /// `bin * MAX_BUCKETS + bucket`.
    pub grid: [u8; CELLS],
    pub events: Vec<Event, MAX_EVENTS>,
}

impl Recording {
    /// Reads a file.
    pub fn parse(f: &[u8]) -> Result<Self, ParseError> {
        if f.len() >= 4 && f[..4] == MAGIC2 {
            return Self::parse2(f);
        }
        if f.len() < HEADER_V1 || f[..4] != MAGIC {
            return Err(ParseError::NotARecording);
        }
        let u16at = |o: usize| u16::from_le_bytes([f[o], f[o + 1]]);
        if f[4] as usize != BINS {
            return Err(ParseError::Damaged);
        }
        let buckets = u16at(12) as usize;
        if buckets == 0 || buckets > MAX_BUCKETS_V1 {
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
        let mut p = HEADER_V1;
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

    /// Reads a file of the current format.
    fn parse2(f: &[u8]) -> Result<Self, ParseError> {
        if f.len() < HEADER2 {
            return Err(ParseError::Damaged);
        }
        let u16at = |o: usize| u16::from_le_bytes([f[o], f[o + 1]]);
        let buckets = u16at(8) as usize;
        let bucket_ms = u16at(6);
        if buckets == 0 || buckets > MAX_BUCKETS || bucket_ms == 0 {
            return Err(ParseError::Damaged);
        }
        let peak = f[18] as i8;
        let base = f[20] as i8;
        let mut r = Recording {
            thr_db: f[5],
            interval_ms: f[19] as u16 * 100,
            duration_s: (buckets as u32 * bucket_ms as u32).div_ceil(1000).min(u16::MAX as u32) as u16,
            bucket_ms,
            buckets,
            sweeps: u16at(10),
            start_khz: u32::from_le_bytes([f[12], f[13], f[14], f[15]]),
            step_khz: u16at(16),
            floor: [0; BINS],
            max: [0; BINS],
            duty: [0; BINS],
            masked: [false; MAX_BUCKETS],
            grid: [0; CELLS],
            events: Vec::new(),
        };
        for b in 0..BINS {
            let nib = (f[21 + b / 2] >> ((b & 1) * 4)) & 15;
            r.floor[b] = base.saturating_add(((nib as i8) << 4) >> 4);
        }
        let mut p = HEADER2;
        if f[4] & 1 == 1 {
            let m = buckets.div_ceil(8);
            if f.len() < p + m {
                return Err(ParseError::Damaged);
            }
            for k in 0..buckets {
                r.masked[k] = (f[p + (k >> 3)] >> (k & 7)) & 1 == 1;
            }
            p += m;
        }
        let mut dec = RcDec::new(&f[p..]);
        let mut pr = [PROB_INIT; PROBS];
        for b in 0..BINS {
            for k in 0..buckets {
                let a = if k > 0 { r.grid[b * MAX_BUCKETS + k - 1] } else { 0 };
                let a2 = if k > 1 { r.grid[b * MAX_BUCKETS + k - 2] } else { 0 };
                let u = if b > 0 { r.grid[(b - 1) * MAX_BUCKETS + k] } else { 0 };
                let (zc, vc) = contexts(a, a2, u);
                if dec.bit(&mut pr[zc]) {
                    let mut node = 1usize;
                    for _ in 0..4 {
                        node = node * 2 + dec.bit(&mut pr[Z_CTX + vc * 16 + node]) as usize;
                    }
                    r.grid[b * MAX_BUCKETS + k] = (node - 16) as u8 + 1;
                }
            }
        }
        let measured = (0..buckets).filter(|&k| !r.masked[k]).count().max(1);
        for b in 0..BINS {
            let (mut hot, mut top) = (0usize, 0u8);
            for k in 0..buckets {
                let v = r.grid[b * MAX_BUCKETS + k];
                if v > 0 && !r.masked[k] {
                    hot += 1;
                }
                top = top.max(v);
            }
            r.duty[b] = ((hot * 100 + measured / 2) / measured) as u8;
            r.max[b] = (r.floor[b] as i16 + top as i16 * STEP_DB).min(peak as i16).max(r.floor[b] as i16) as i8;
        }
        r.find_events();
        Ok(r)
    }

    /// The events: connected areas (four neighbours) of cells above the threshold, found in the order of their first
    /// cell, step by step. An area grows pass by pass over its own bounding box.
    fn find_events(&mut self) {
        let nb = self.buckets;
        for start in 0..BINS * MAX_BUCKETS {
            let (b0, k0) = (start / MAX_BUCKETS, start % MAX_BUCKETS);
            if k0 >= nb || self.grid[start] == 0 || self.grid[start] & 0x80 != 0 {
                continue;
            }
            let mut ev = Event { k0: k0 as u8, k1: k0 as u8, b0: b0 as u8, b1: b0 as u8, peak: self.grid[start] * STEP_DB as u8 };
            self.grid[start] |= 0x80;
            let mut reverse = false;
            loop {
                let mut changed = false;
                let (lb, hb) = ((ev.b0 as usize).saturating_sub(1), (ev.b1 as usize + 1).min(BINS - 1));
                let (lk, hk) = ((ev.k0 as usize).saturating_sub(1), (ev.k1 as usize + 1).min(nb - 1));
                for bi in 0..=(hb - lb) {
                    let b = if reverse { hb - bi } else { lb + bi };
                    for ki in 0..=(hk - lk) {
                        let k = if reverse { hk - ki } else { lk + ki };
                        let i = b * MAX_BUCKETS + k;
                        let g = self.grid[i];
                        if g == 0 || g & 0x80 != 0 {
                            continue;
                        }
                        let touches = (b > 0 && self.grid[i - MAX_BUCKETS] & 0x80 != 0)
                            || (b + 1 < BINS && self.grid[i + MAX_BUCKETS] & 0x80 != 0)
                            || (k > 0 && self.grid[i - 1] & 0x80 != 0)
                            || (k + 1 < nb && self.grid[i + 1] & 0x80 != 0);
                        if touches {
                            self.grid[i] = g | 0x80;
                            ev.b0 = ev.b0.min(b as u8);
                            ev.b1 = ev.b1.max(b as u8);
                            ev.k0 = ev.k0.min(k as u8);
                            ev.k1 = ev.k1.max(k as u8);
                            ev.peak = ev.peak.max(g * STEP_DB as u8);
                            changed = true;
                        }
                    }
                }
                if !changed {
                    break;
                }
                reverse = !reverse;
            }
            let _ = self.events.push(ev);
        }
        for g in self.grid.iter_mut() {
            *g &= 0x7f;
        }
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
        crate::sort::sort_by(&mut order[..], |a, b| (a.b0 as u16 + a.b1 as u16, a.k0, a.b0) < (b.b0 as u16 + b.b1 as u16, b.k0, b.b0));
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
            crate::sort::sort_by(&mut members[..], |a, b| (a.k0, a.b0) < (b.k0, b.b0));
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
        crate::sort::sort_by(&mut out[..], |a, b| (b.air_ms, a.first_ms, a.b0) < (a.air_ms, b.first_ms, b.b0));
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

    /// A recorder with tables of its own, for the tests.
    struct Rig {
        rec: Recorder,
        cells: StdVec<u8>,
        work: StdVec<u8>,
    }

    impl Rig {
        fn new(cfg: Config) -> Option<Rig> {
            let mut r = Rig { rec: Recorder::empty(), cells: std::vec![0; CELL_BYTES], work: std::vec![0; WORK_LEN] };
            let mut t = Tables { cells: &mut r.cells, work: &mut r.work };
            r.rec.start(cfg, &mut t).then_some(r)
        }
        fn push(&mut self, t_ms: u32, rssi: &[u8], masked: bool) {
            let mut t = Tables { cells: &mut self.cells, work: &mut self.work };
            self.rec.push(&mut t, t_ms, rssi, masked);
        }
        fn finish(&mut self, out: &mut Vec<u8, MAX_FILE>) -> Result<(), TooBig> {
            let mut t = Tables { cells: &mut self.cells, work: &mut self.work };
            self.rec.finish(&mut t, out)
        }
        fn finish_in_place(&mut self) -> Result<StdVec<u8>, TooBig> {
            let mut t = Tables { cells: &mut self.cells, work: &mut self.work };
            let n = self.rec.finish_in_place(&mut t)?;
            Ok(self.work[..n].to_vec())
        }
    }

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

    fn record(duration_s: u16, interval_ms: u16) -> (Rig, Vec<u8, MAX_FILE>) {
        let cfg = Config { start_khz: 863_000, step_khz: 110, duration_s, interval_ms, thr_db: 6 };
        let mut rec = Rig::new(cfg).unwrap();
        for (t, row) in scene(duration_s as u32, interval_ms as u32) {
            rec.push(t, &row, false);
        }
        let mut out: Vec<u8, MAX_FILE> = Vec::new();
        rec.finish(&mut out).unwrap();
        (rec, out)
    }

    #[test]
    fn in_place_gives_the_same_file() {
        let (_, want) = record(600, 100);
        let cfg = Config { start_khz: 863_000, step_khz: 110, duration_s: 600, interval_ms: 100, thr_db: 6 };
        let mut rig = Rig::new(cfg).unwrap();
        for (t, row) in scene(600, 100) {
            rig.push(t, &row, false);
        }
        assert_eq!(rig.finish_in_place().unwrap().as_slice(), want.as_slice());
    }

    #[test]
    fn small_file_and_roundtrip() {
        let (_, f) = record(300, 100);
        assert!(f.len() < 700, "file is {} bytes", f.len());
        let r = Recording::parse(&f).unwrap();
        assert_eq!(r.buckets, 256);
        assert_eq!(r.bucket_ms, 1172);
        assert_eq!(r.start_khz, 863_000);
        assert_eq!(r.step_khz, 110);
        assert_eq!(r.sweeps, 3000);
        assert_eq!(r.khz(41), 863_000 + 110 * 41);
        assert!(r.duty[41] > 90, "carrier duty {}", r.duty[41]);
        assert!(r.duty[0] < 5);
    }

    #[test]
    fn the_file_decodes_to_the_cells_the_recorder_made() {
        let (rig, f) = record(120, 100);
        let r = Recording::parse(&f).unwrap();
        for b in 0..BINS {
            for k in 0..r.buckets {
                assert_eq!(r.cell(b, k), cell_get(&rig.cells, b * MAX_BUCKETS + k), "cell {b},{k}");
            }
        }
    }

    #[test]
    fn finds_the_three_things() {
        let (_, f) = record(600, 100);
        let g = Recording::parse(&f).unwrap().groups();
        let has = |k: Kind, lo: u8, hi: u8| g.iter().any(|x| x.kind == k && x.b0 >= lo && x.b1 <= hi);
        assert!(has(Kind::Constant, 39, 43), "{g:?}");
        assert!(has(Kind::Periodic, 16, 23), "{g:?}");
        assert!(g.iter().any(|x| matches!(x.kind, Kind::Rising | Kind::Long) && x.b0 >= 52 && x.b1 <= 62), "{g:?}");
        assert_eq!(g[0].severity, Severity::High);
    }

    #[test]
    fn a_short_recording_works_before_the_usual_peaks_are_known() {
        let (_, f) = record(10, 100);
        let r = Recording::parse(&f).unwrap();
        assert_eq!(r.buckets, 10);
        assert!(r.groups().iter().any(|x| x.kind == Kind::Constant), "{:?}", r.groups());
    }

    #[test]
    fn masked_sweeps_do_not_make_events() {
        let cfg = Config { start_khz: 863_000, step_khz: 110, duration_s: 120, interval_ms: 1000, thr_db: 6 };
        let mut rec = Rig::new(cfg).unwrap();
        let quiet = [236u8; BINS];
        let loud = [150u8; BINS];
        for i in 0..120u32 {
            let loudish = (40..60).contains(&i);
            rec.push(i * 1000, if loudish { &loud } else { &quiet }, loudish);
        }
        let mut out: Vec<u8, MAX_FILE> = Vec::new();
        rec.finish(&mut out).unwrap();
        let r = Recording::parse(&out).unwrap();
        assert!(r.events.is_empty());
        assert!(r.masked[45]);
        assert!(!r.masked[10]);
    }

    #[test]
    fn nothing_recorded_is_a_valid_file() {
        let cfg = Config { start_khz: 863_000, step_khz: 110, duration_s: 60, interval_ms: 100, thr_db: 6 };
        let mut rec = Rig::new(cfg).unwrap();
        let mut out: Vec<u8, MAX_FILE> = Vec::new();
        rec.finish(&mut out).unwrap();
        let r = Recording::parse(&out).unwrap();
        assert_eq!(r.sweeps, 0);
        assert!(r.events.is_empty());
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
        assert_eq!(Recording::parse(&f[..30]).err(), Some(ParseError::Damaged));
        assert!(Rig::new(Config { start_khz: 0, step_khz: 0, duration_s: 10, interval_ms: 100, thr_db: 6 }).is_none());
        assert!(Rig::new(Config { start_khz: 0, step_khz: 1, duration_s: 0, interval_ms: 100, thr_db: 6 }).is_none());
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
        let rows = scene(300, 100);
        let mut input = StdVec::new();
        for (t, row) in &rows {
            input.extend_from_slice(&t.to_le_bytes());
            input.extend_from_slice(row);
        }
        let cfg = Config { start_khz: 863_000, step_khz: 110, duration_s: 300, interval_ms: 100, thr_db: 6 };
        let mut rec = Rig::new(cfg).unwrap();
        for (t, row) in &rows {
            rec.push(*t, row, false);
        }
        let mut out: Vec<u8, MAX_FILE> = Vec::new();
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
