//! Talebeskeder med Codec2.
//!
//! Format (MIME `audio/x-codec2`):
//!
//! ```text
//! "C2" | tilstand (u8: 0 = 3200, 1 = 2400, 2 = 1600, 3 = 1400, 4 = 1300, 5 = 1200 bit/s)
//!      | antal rammer (u16) | rammer (bits_per_frame/8 bytes hver)
//! ```
//!
//! Lyden er 8 kHz mono, 16 bit. Ved 1200 bit/s er en ramme 40 ms og 6 bytes,
//! så 10 sekunder fylder 1505 bytes.
//!
//! Til webklienten bygges crate'en som WebAssembly (`wasm32-unknown-unknown`)
//! med en lille C-lignende grænseflade: `voice_alloc`, `voice_free`,
//! `voice_encode` og `voice_decode`. Kun den del bruger `unsafe`.

use codec2::{Codec2, Codec2Mode};

pub const MAGIC: &[u8; 2] = b"C2";
pub const HEADER: usize = 5;
pub const SAMPLE_RATE: u32 = 8000;

fn mode(m: u8) -> Option<Codec2Mode> {
    Some(match m {
        0 => Codec2Mode::MODE_3200,
        1 => Codec2Mode::MODE_2400,
        2 => Codec2Mode::MODE_1600,
        3 => Codec2Mode::MODE_1400,
        4 => Codec2Mode::MODE_1300,
        5 => Codec2Mode::MODE_1200,
        _ => return None,
    })
}

/// Koder 8 kHz PCM til en talebesked. En sidste ufuldstændig ramme fyldes
/// med stilhed.
pub fn encode(pcm: &[i16], m: u8) -> Option<Vec<u8>> {
    let mut c = Codec2::new(mode(m)?);
    let n = c.samples_per_frame();
    let bytes = c.bits_per_frame().div_ceil(8);
    let frames = pcm.len().div_ceil(n);
    if frames > u16::MAX as usize {
        return None;
    }
    let mut out = Vec::with_capacity(HEADER + frames * bytes);
    out.extend_from_slice(MAGIC);
    out.push(m);
    out.extend_from_slice(&(frames as u16).to_le_bytes());
    let mut frame = vec![0i16; n];
    let mut bits = vec![0u8; bytes];
    for chunk in pcm.chunks(n) {
        frame.fill(0);
        frame[..chunk.len()].copy_from_slice(chunk);
        c.encode(&mut bits, &frame);
        out.extend_from_slice(&bits);
    }
    Some(out)
}

/// Afkoder en talebesked til 8 kHz PCM. `None` ved ukendt format eller
/// afkortede data.
pub fn decode(data: &[u8]) -> Option<Vec<i16>> {
    if data.len() < HEADER || &data[..2] != MAGIC {
        return None;
    }
    let mut c = Codec2::new(mode(data[2])?);
    let frames = u16::from_le_bytes([data[3], data[4]]) as usize;
    let n = c.samples_per_frame();
    let bytes = c.bits_per_frame().div_ceil(8);
    let body = &data[HEADER..];
    if body.len() < frames * bytes {
        return None;
    }
    let mut out = vec![0i16; frames * n];
    for (i, bits) in body.chunks(bytes).take(frames).enumerate() {
        c.decode(&mut out[i * n..(i + 1) * n], bits);
    }
    Some(out)
}

// ---------- WebAssembly-grænseflade ----------

#[cfg(target_arch = "wasm32")]
mod wasm {
    use std::alloc::{Layout, alloc, dealloc};

    /// Resultatet af sidste `voice_encode`/`voice_decode`.
    static mut RESULT: Vec<u8> = Vec::new();

    #[unsafe(no_mangle)]
    pub extern "C" fn voice_alloc(n: usize) -> *mut u8 {
        unsafe { alloc(Layout::from_size_align(n.max(1), 8).unwrap()) }
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn voice_free(p: *mut u8, n: usize) {
        unsafe { dealloc(p, Layout::from_size_align(n.max(1), 8).unwrap()) }
    }

    /// Koder `n` samples (i16) ved `pcm`. Returnerer længden af resultatet
    /// (0 ved fejl); hent det med `voice_result_ptr`.
    #[unsafe(no_mangle)]
    pub extern "C" fn voice_encode(pcm: *const i16, n: usize, mode: u8) -> usize {
        let pcm = unsafe { std::slice::from_raw_parts(pcm, n) };
        set(super::encode(pcm, mode).unwrap_or_default())
    }

    /// Afkoder `n` bytes ved `data` til PCM (i16, little endian). Returnerer
    /// længden i bytes (0 ved fejl).
    #[unsafe(no_mangle)]
    pub extern "C" fn voice_decode(data: *const u8, n: usize) -> usize {
        let data = unsafe { std::slice::from_raw_parts(data, n) };
        let pcm = super::decode(data).unwrap_or_default();
        set(pcm.iter().flat_map(|s| s.to_le_bytes()).collect())
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn voice_result_ptr() -> *const u8 {
        unsafe { (*core::ptr::addr_of!(RESULT)).as_ptr() }
    }

    fn set(v: Vec<u8>) -> usize {
        let n = v.len();
        unsafe { *core::ptr::addr_of_mut!(RESULT) = v };
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(secs: f32) -> Vec<i16> {
        (0..(secs * SAMPLE_RATE as f32) as usize)
            .map(|i| ((i as f32 * 2.0 * std::f32::consts::PI * 220.0 / 8000.0).sin() * 8000.0) as i16)
            .collect()
    }

    #[test]
    fn ten_seconds_at_1200_fits_lora() {
        let enc = encode(&tone(10.0), 5).unwrap();
        assert_eq!(enc.len(), HEADER + 250 * 6);
        let dec = decode(&enc).unwrap();
        assert_eq!(dec.len(), 250 * 320);
        // Der kommer lyd ud (ikke kun stilhed).
        let energy: f64 = dec.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / dec.len() as f64;
        assert!(energy > 1e5, "energi {energy}");
    }

    #[test]
    fn all_modes_roundtrip_and_bad_input() {
        for m in 0..=5 {
            let enc = encode(&tone(1.0), m).unwrap();
            assert!(decode(&enc).is_some(), "tilstand {m}");
        }
        assert!(encode(&tone(1.0), 9).is_none());
        assert!(decode(b"C2").is_none());
        assert!(decode(b"XX\x05\x01\x00").is_none());
        let mut enc = encode(&tone(1.0), 5).unwrap();
        enc.truncate(enc.len() - 1);
        assert!(decode(&enc).is_none(), "afkortet");
    }
}
