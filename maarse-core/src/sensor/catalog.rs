//! Sensors on the I2C bus, described as data.
//!
//! Most small I2C sensors work the same way: write a few set-up bytes, tell
//! the chip to measure, wait, write a register number, read a few bytes, and
//! turn some of the bits into a number. An [`I2cDesc`] says exactly that, so
//! one small reader in the firmware serves every I2C sensor. The recipe carries
//! the description, so a node needs no update to read a new kind of chip: the
//! list of known chips (the *catalogue*) lives in the repository's `sensors/`
//! folder and in the host, not in the firmware.
//!
//! A sensor that needs calibration tables or a long start-up (the BME280, a
//! distance sensor) is not described this way: it has its own driver.

use heapless::Vec;

/// Longest set-up (a list of commands, each one length byte and its bytes).
pub const MAX_INIT: usize = 8;
pub const MAX_TRIGGER: usize = 4;
pub const MAX_PTR: usize = 2;
/// Most bytes one read returns, check bytes included.
pub const MAX_READ: usize = 16;
/// Longest wait between the trigger and the read.
pub const MAX_WAIT_MS: u16 = 10_000;
/// Bytes of [`I2cDesc::field_bytes`].
pub const FIELD_LEN: usize = 15;

/// Which bits of the bytes read are the number, and how it becomes a value:
/// `value = raw * mul / div + add` in the unit of the reading's kind
/// (0.01 °C, 0.01 %, hPa times 100, and so on).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field {
    /// Where the number starts, in bits from the first bit of the first byte.
    /// With `little`, a byte offset times 8.
    pub bit_off: u8,
    /// How many bits, 1 to 32. With `little`, a multiple of 8.
    pub bits: u8,
    pub signed: bool,
    /// Little endian, byte aligned (the default is big endian, any bit).
    pub little: bool,
    pub mul: i32,
    pub div: i32,
    pub add: i32,
}

/// How to read one value from an I2C sensor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct I2cDesc {
    /// 7-bit address.
    pub addr: u8,
    /// Commands sent first, every cycle: `[len, bytes...]` repeated.
    pub init: [u8; MAX_INIT],
    pub init_len: u8,
    /// Bytes that start a measurement (none: the chip measures by itself).
    pub trigger: [u8; MAX_TRIGGER],
    pub trigger_len: u8,
    /// Wait after the trigger, in ms.
    pub wait_ms: u16,
    /// Register or command written before the read (none: read directly).
    pub ptr: [u8; MAX_PTR],
    pub ptr_len: u8,
    /// Bytes to read (with `crc`, a multiple of 3).
    pub read_len: u8,
    /// Every two data bytes are followed by a CRC-8 (Sensirion: polynomial
    /// 0x31, start 0xFF). The check bytes are dropped before `field` looks at
    /// the bytes.
    pub crc: bool,
    pub field: Field,
}

impl I2cDesc {
    pub const fn new(addr: u8) -> I2cDesc {
        I2cDesc {
            addr,
            init: [0; MAX_INIT],
            init_len: 0,
            trigger: [0; MAX_TRIGGER],
            trigger_len: 0,
            wait_ms: 0,
            ptr: [0; MAX_PTR],
            ptr_len: 0,
            read_len: 2,
            crc: false,
            field: Field { bit_off: 0, bits: 16, signed: false, little: false, mul: 1, div: 1, add: 0 },
        }
    }

    pub fn init_bytes(&self) -> &[u8] {
        &self.init[..self.init_len as usize]
    }
    pub fn trigger_bytes(&self) -> &[u8] {
        &self.trigger[..self.trigger_len as usize]
    }
    pub fn ptr_bytes(&self) -> &[u8] {
        &self.ptr[..self.ptr_len as usize]
    }

    /// The commands of the set-up, one write each.
    pub fn init_commands(&self) -> impl Iterator<Item = &[u8]> {
        let mut rest = self.init_bytes();
        core::iter::from_fn(move || {
            let (&n, tail) = rest.split_first()?;
            let (cmd, tail) = tail.split_at(n as usize);
            rest = tail;
            Some(cmd)
        })
    }

    /// Two descriptions that make the chip measure and answer in the same way
    /// (they may take different numbers from the answer). The node reads the
    /// chip once per cycle for all of them.
    pub fn same_read(&self, o: &I2cDesc) -> bool {
        self.addr == o.addr
            && self.init_bytes() == o.init_bytes()
            && self.trigger_bytes() == o.trigger_bytes()
            && self.wait_ms == o.wait_ms
            && self.ptr_bytes() == o.ptr_bytes()
            && self.read_len == o.read_len
            && self.crc == o.crc
    }

    /// Bytes the field looks at once the check bytes are dropped.
    fn data_len(&self) -> usize {
        if self.crc { self.read_len as usize / 3 * 2 } else { self.read_len as usize }
    }

    pub fn validate(&self) -> bool {
        let f = &self.field;
        if !(1..=0x77).contains(&self.addr)
            || self.init_len as usize > MAX_INIT
            || self.trigger_len as usize > MAX_TRIGGER
            || self.ptr_len as usize > MAX_PTR
            || self.wait_ms > MAX_WAIT_MS
            || !(1..=MAX_READ as u8).contains(&self.read_len)
            || (self.crc && self.read_len % 3 != 0)
            || !(1..=32).contains(&f.bits)
            || f.div == 0
            || f.bit_off as usize + f.bits as usize > self.data_len() * 8
            || (f.little && (f.bit_off % 8 != 0 || f.bits % 8 != 0))
        {
            return false;
        }
        // The set-up must be whole commands of 1 to 4 bytes.
        let mut left = self.init_bytes();
        while let Some((&n, tail)) = left.split_first() {
            if n == 0 || n > 4 || tail.len() < n as usize {
                return false;
            }
            left = &tail[n as usize..];
        }
        true
    }

    /// The value in `data` (what the chip answered), or `None` if a check
    /// byte is wrong or the answer is too short.
    pub fn extract(&self, data: &[u8]) -> Option<i32> {
        if data.len() < self.read_len as usize {
            return None;
        }
        let mut words = [0u8; MAX_READ];
        let bytes: &[u8] = if self.crc {
            let mut n = 0;
            for g in data[..self.read_len as usize].chunks(3) {
                if g.len() < 3 || crc8(&g[..2]) != g[2] {
                    return None;
                }
                words[n..n + 2].copy_from_slice(&g[..2]);
                n += 2;
            }
            &words[..n]
        } else {
            &data[..self.read_len as usize]
        };
        let f = &self.field;
        let raw: u64 = if f.little {
            let first = f.bit_off as usize / 8;
            let mut v = 0u64;
            for i in (0..f.bits as usize / 8).rev() {
                v = v << 8 | *bytes.get(first + i)? as u64;
            }
            v
        } else {
            let first = f.bit_off as usize / 8;
            let last = (f.bit_off as usize + f.bits as usize - 1) / 8;
            let mut v = 0u64;
            for i in first..=last {
                v = v << 8 | *bytes.get(i)? as u64;
            }
            let after = (last + 1) * 8 - (f.bit_off as usize + f.bits as usize);
            (v >> after) & ((1u64 << f.bits) - 1)
        };
        let n = if f.signed && raw >> (f.bits - 1) & 1 == 1 { raw as i64 - (1i64 << f.bits) } else { raw as i64 };
        let v = n * f.mul as i64 / f.div as i64 + f.add as i64;
        Some(v.clamp(i32::MIN as i64, i32::MAX as i64) as i32)
    }

    /// The part that makes the chip measure and answer: address, set-up,
    /// trigger, wait, register, how much to read. Sources of one chip share it.
    pub fn read_bytes(&self, out: &mut Vec<u8, 24>) -> Result<(), ()> {
        let put = |o: &mut Vec<u8, 24>, b: &[u8]| o.extend_from_slice(b).map_err(|_| ());
        put(out, &[self.addr, self.init_len])?;
        put(out, self.init_bytes())?;
        put(out, &[self.trigger_len])?;
        put(out, self.trigger_bytes())?;
        put(out, &self.wait_ms.to_le_bytes())?;
        put(out, &[self.ptr_len])?;
        put(out, self.ptr_bytes())?;
        put(out, &[self.read_len, self.crc as u8])
    }

    /// The part that takes a number out of the answer.
    pub fn field_bytes(&self) -> [u8; FIELD_LEN] {
        let f = &self.field;
        let mut o = [0u8; FIELD_LEN];
        o[0] = f.signed as u8 | (f.little as u8) << 1;
        o[1] = f.bit_off;
        o[2] = f.bits;
        o[3..7].copy_from_slice(&f.mul.to_le_bytes());
        o[7..11].copy_from_slice(&f.div.to_le_bytes());
        o[11..15].copy_from_slice(&f.add.to_le_bytes());
        o
    }

    /// Puts a read part and a field part together; `None` if they do not fit.
    pub fn from_parts(read: &[u8], field: &[u8]) -> Option<I2cDesc> {
        let mut r = read.iter().copied();
        let mut next = || r.next();
        let mut d = I2cDesc::new(next()?);
        d.init_len = next()?;
        if d.init_len as usize > MAX_INIT {
            return None;
        }
        for i in 0..d.init_len as usize {
            d.init[i] = next()?;
        }
        d.trigger_len = next()?;
        if d.trigger_len as usize > MAX_TRIGGER {
            return None;
        }
        for i in 0..d.trigger_len as usize {
            d.trigger[i] = next()?;
        }
        d.wait_ms = u16::from_le_bytes([next()?, next()?]);
        d.ptr_len = next()?;
        if d.ptr_len as usize > MAX_PTR {
            return None;
        }
        for i in 0..d.ptr_len as usize {
            d.ptr[i] = next()?;
        }
        d.read_len = next()?;
        d.crc = next()? & 1 != 0;
        if next().is_some() || field.len() != FIELD_LEN {
            return None;
        }
        let le = |o: usize| i32::from_le_bytes([field[o], field[o + 1], field[o + 2], field[o + 3]]);
        d.field = Field { signed: field[0] & 1 != 0, little: field[0] & 2 != 0, bit_off: field[1], bits: field[2], mul: le(3), div: le(7), add: le(11) };
        d.validate().then_some(d)
    }
}

/// Sensirion's CRC-8: polynomial 0x31, start value 0xFF.
pub fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0xFFu8;
    for &b in data {
        crc ^= b;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 { crc << 1 ^ 0x31 } else { crc << 1 };
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_crc(words: &[u16]) -> std::vec::Vec<u8> {
        let mut v = std::vec::Vec::new();
        for w in words {
            let b = w.to_be_bytes();
            v.extend_from_slice(&b);
            v.push(crc8(&b));
        }
        v
    }

    /// An SHT3x-like description: temperature from the first word, with check bytes.
    fn sht() -> I2cDesc {
        let mut d = I2cDesc::new(0x44);
        d.trigger[..2].copy_from_slice(&[0x24, 0x00]);
        (d.trigger_len, d.wait_ms, d.read_len, d.crc) = (2, 20, 6, true);
        d.field = Field { bit_off: 0, bits: 16, signed: false, little: false, mul: 17500, div: 65535, add: -4500 };
        d
    }

    #[test]
    fn the_crc_matches_the_data_sheet() {
        // Sensirion data sheets: CRC of 0xBEEF is 0x92.
        assert_eq!(crc8(&[0xBE, 0xEF]), 0x92);
    }

    #[test]
    fn bits_are_taken_out_of_the_answer() {
        let t = sht();
        let data = with_crc(&[0x6666, 0x8000]);
        assert_eq!(t.extract(&data), Some(2500));
        let mut h = t;
        h.field = Field { bit_off: 16, bits: 16, signed: false, little: false, mul: 10000, div: 65535, add: 0 };
        assert!(t.same_read(&h));
        assert_eq!(h.extract(&data), Some(5000));
        let mut bad = data.clone();
        bad[2] ^= 1;
        assert_eq!(t.extract(&bad), None, "a wrong check byte is refused");
        // 20 bits that do not start on a byte (AHT20), 13 signed bits (MCP9808), little endian (ADXL345).
        let mut a = I2cDesc::new(0x38);
        a.read_len = 6;
        a.field = Field { bit_off: 28, bits: 20, signed: false, little: false, mul: 20000, div: 1 << 20, add: -5000 };
        assert_eq!(a.extract(&[0x1C, 0x80, 0x00, 0x06, 0x00, 0x00]), Some(2500));
        let mut m = I2cDesc::new(0x18);
        m.field = Field { bit_off: 3, bits: 13, signed: true, little: false, mul: 100, div: 16, add: 0 };
        assert_eq!(m.extract(&[0x01, 0x94]), Some(2525));
        assert_eq!(m.extract(&[0xBF, 0xFC]), Some(-25), "flag bits are ignored, the sign is kept");
        let mut x = I2cDesc::new(0x53);
        x.read_len = 6;
        x.field = Field { bit_off: 16, bits: 16, signed: true, little: true, mul: 39, div: 10, add: 0 };
        assert_eq!(x.extract(&[0, 0, 0x00, 0xFF, 0, 0]), Some(-998));
    }

    #[test]
    fn the_two_parts_put_together_again_and_bad_ones_are_refused() {
        let d = sht();
        let (mut r, f) = (Vec::new(), d.field_bytes());
        d.read_bytes(&mut r).unwrap();
        assert_eq!(I2cDesc::from_parts(&r, &f), Some(d));
        let mut bad = d;
        bad.field.bit_off = 16; // 16 + 16 bits do not fit in 2 data bytes
        bad.field.bits = 24;
        assert!(!bad.validate());
        let mut bad = d;
        bad.field.div = 0;
        assert!(!bad.validate());
        let mut bad = d;
        bad.addr = 0x7F;
        assert!(!bad.validate());
        let mut bad = d;
        bad.init = [9, 1, 2, 3, 4, 5, 6, 7];
        bad.init_len = 8;
        assert!(!bad.validate(), "a command longer than 4 bytes");
        assert_eq!(I2cDesc::from_parts(&[0x40], &f), None);
        assert_eq!(I2cDesc::from_parts(&r, &f[..14]), None);
    }
}
