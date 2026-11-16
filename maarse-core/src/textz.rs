//! Compression of short text messages (after SMAZ and Meshtastic's use of
//! Unishox2): a static codebook of common Danish and English fragments.
//!
//! Format: each byte is either a codebook index (`0..CODES`), `ESC` followed
//! by one literal byte, or `RUN` followed by a length and that many literal
//! bytes. Text is UTF-8; fragments with æ, ø and å are in the codebook as their
//! UTF-8 bytes. Encoding is greedy longest match. Short chat messages shrink
//! by about a third (see the test).

const ESC: u8 = 254;
const RUN: u8 = 255;

/// The codebook (at most 254 entries; longer fragments first is not needed,
/// the encoder tries every entry).
const BOOK: &[&str] = &[
    " ", "e", "r", "n", "t", "a", "i", "s", "d", "l", "o", "g", "m", "k", "v", "u", "p", "h", "f", "b", "j", "y", "c", "w",
    "æ", "ø", "å", ".", ",", "?", "!", "-", ":", "'", "\n", "0", "1", "2", "3", "4", "5", "6", "7", "8", "9",
    "er", "en", "de", "et", "ig", "ik", "ke", "nd", "re", "st", "te", "ge", "le", "ne", "se", "me", "ve", "an", "in", "or",
    "ar", "el", "il", "is", "ri", "ra", "li", "ed", "es", "on", "at", "al", "om", "ti", "in", "th", "he", "ha", "ou", "it",
    "e ", "r ", "n ", "t ", "s ", "g ", "d ", "k ", "l ", "m ", " d", " s", " h", " f", " v", " k", " m", " e", " a", " t",
    " og ", " er ", " det ", " jeg ", " du ", " ikke ", " på ", " til ", " med ", " har ", " en ", " af ", " for ", " at ",
    " vi ", " kan ", " vil ", " skal ", " der ", " den ", " som ", " men ", " om ", " så ", " nu ", " her ", " hvor ",
    " hvad ", " godt ", " okay", " ok", " ja", " nej", " tak", " hej", "Hej", "hej", "Ja", "Nej", "Ok", "Tak",
    " the ", " and ", " to ", " you ", " is ", " are ", " we ", " in ", " on ", " at ", " of ", " for ", " it ",
    "hjælp", "brug for", "kommer", "hjemme", "vand", "strøm", "mad", "vej", "bil", "skov", "ved ", "lige ",
    "ingen", "alle", "også", "noget", "meget", "være", "været", "bare", "kun ", "efter", "over", "under",
    "ing", "and", "ion", "ent", "the", "ere", "ige", "lig", "ste", "ter", "der", "ende", "erne", "ene", "else",
    "?\n", ". ", ", ", "! ", "? ", " - ", "..", "km", "kl.", "min", "time", "dag", "nat", "morgen", "aften",
];
const CODES: usize = if BOOK.len() < 254 { BOOK.len() } else { 254 };

/// Compresses `text` into `out`; returns the length, or `None` if it does
/// not fit or would not be shorter.
pub fn compress(text: &[u8], out: &mut [u8]) -> Option<usize> {
    let mut o = 0;
    let mut i = 0;
    let mut lit: (usize, usize) = (0, 0); // pending literal run (start, len)
    let flush = |out: &mut [u8], o: &mut usize, start: usize, len: usize| -> Option<()> {
        if len == 0 {
            return Some(());
        }
        if len == 1 {
            *out.get_mut(*o)? = ESC;
            *out.get_mut(*o + 1)? = text[start];
            *o += 2;
        } else {
            *out.get_mut(*o)? = RUN;
            *out.get_mut(*o + 1)? = len as u8;
            out.get_mut(*o + 2..*o + 2 + len)?.copy_from_slice(&text[start..start + len]);
            *o += 2 + len;
        }
        Some(())
    };
    while i < text.len() {
        let best = BOOK[..CODES]
            .iter()
            .enumerate()
            .filter(|(_, w)| text[i..].starts_with(w.as_bytes()))
            .max_by_key(|(_, w)| w.len());
        match best {
            Some((code, w)) => {
                flush(out, &mut o, lit.0, lit.1)?;
                lit = (0, 0);
                *out.get_mut(o)? = code as u8;
                o += 1;
                i += w.len();
            }
            None => {
                if lit.1 == 0 {
                    lit.0 = i;
                }
                lit.1 += 1;
                i += 1;
                if lit.1 == 255 {
                    flush(out, &mut o, lit.0, lit.1)?;
                    lit = (0, 0);
                }
            }
        }
    }
    flush(out, &mut o, lit.0, lit.1)?;
    (o < text.len()).then_some(o)
}

/// Expands compressed text into `out`; returns the length.
pub fn decompress(z: &[u8], out: &mut [u8]) -> Option<usize> {
    let mut o = 0;
    let mut i = 0;
    while i < z.len() {
        match z[i] {
            ESC => {
                *out.get_mut(o)? = *z.get(i + 1)?;
                o += 1;
                i += 2;
            }
            RUN => {
                let n = *z.get(i + 1)? as usize;
                out.get_mut(o..o + n)?.copy_from_slice(z.get(i + 2..i + 2 + n)?);
                o += n;
                i += 2 + n;
            }
            c if (c as usize) < CODES => {
                let w = BOOK[c as usize].as_bytes();
                out.get_mut(o..o + w.len())?.copy_from_slice(w);
                o += w.len();
                i += 1;
            }
            _ => return None,
        }
    }
    Some(o)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLES: &[&str] = &[
        "Hej, er du okay? Vi har ingen strøm her.",
        "Jeg kommer om 20 min med vand og mad",
        "Ja tak, vi har brug for hjælp ved skoven",
        "Vejen er spærret efter stormen, tag den anden vej",
        "Er alle hjemme? Ring når du kan",
        "Ok, jeg er på vej nu",
        "Where are you? We need water and food.",
        "Bilen sidder fast i sneen ved Rold Skov, kan nogen hjælpe?",
    ];

    #[test]
    fn roundtrip_and_ratio() {
        let mut total = (0, 0);
        for s in SAMPLES {
            let mut z = [0u8; 300];
            let mut back = [0u8; 300];
            let n = compress(s.as_bytes(), &mut z).expect("shorter");
            let m = decompress(&z[..n], &mut back).unwrap();
            assert_eq!(&back[..m], s.as_bytes());
            total.0 += s.len();
            total.1 += n;
        }
        let ratio = total.1 as f64 / total.0 as f64;
        assert!(ratio < 0.8, "ratio {ratio:.2}");
        std::println!("textz: {} -> {} bytes ({:.0} %)", total.0, total.1, ratio * 100.0);
    }

    #[test]
    fn odd_input_does_not_panic() {
        let mut z = [0u8; 600];
        let mut back = [0u8; 600];
        let bin: Vec<u8> = (0..=255u8).collect();
        if let Some(n) = compress(&bin, &mut z) {
            let m = decompress(&z[..n], &mut back).unwrap();
            assert_eq!(&back[..m], &bin[..]);
        }
        assert!(decompress(&[RUN, 200, 1], &mut back).is_none());
    }
}
