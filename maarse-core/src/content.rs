//! Indholdet i en direkte besked (efter dekryptering).
//!
//! Første byte angiver typen:
//! * `0x00` tekst (UTF-8)
//! * `0x01` filtilbud: der kommer en fil i `Fragment`-pakker
//! * `0x02` kontrolkommando til en node: tæller (u32) + companion-kommando
//! * `0x03` svar på en kontrolkommando: tæller (u32) + companion-hændelse
//! * `0x04` resync: ingen data. Sendes som init i en ny FS-session, når vi
//!   har fået en ratchet-besked, vi ikke kan læse (fx efter genstart).
//!
//! Kontrolkommandoer er krypterede direkte beskeder, så modtageren ved med
//! sikkerhed, hvem afsenderen er. Noden udfører dem kun, hvis afsenderen
//! står på dens liste over administratorer, og tælleren er højere end sidst.
//!
//! ```text
//! filtilbud: 0x01 | fil-ID (8) | størrelse (u32) | antal bidder (u16)
//!            | bidstørrelse (u8) | nøgle (32) | SHA-256/16 (16)
//!            | MIME-længde (u8) | MIME | navn
//! ```

pub const TEXT: u8 = 0x00;
pub const FILE_OFFER: u8 = 0x01;
pub const CONTROL: u8 = 0x02;
pub const CONTROL_REPLY: u8 = 0x03;
pub const RESYNC: u8 = 0x04;

/// Fast del af et filtilbud.
pub const OFFER_FIXED: usize = 1 + 8 + 4 + 2 + 1 + 32 + 16 + 1;

pub type Small = heapless::Vec<u8, 64>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileOffer {
    pub file_id: u64,
    pub size: u32,
    pub chunks: u16,
    pub chunk_size: u8,
    pub key: [u8; 32],
    pub hash: [u8; 16],
    pub mime: Small,
    pub name: Small,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Content<'a> {
    Text(&'a [u8]),
    FileOffer(FileOffer),
    /// (tæller, companion-kommando uden længdefelt)
    Control(u32, &'a [u8]),
    /// (tæller, companion-hændelse uden længdefelt)
    ControlReply(u32, &'a [u8]),
    /// Afsenderen har startet en ny FS-session med os.
    Resync,
    Unknown(u8),
}

impl FileOffer {
    pub fn encode(&self) -> heapless::Vec<u8, 160> {
        let mut v = heapless::Vec::new();
        let _ = v.push(FILE_OFFER);
        let _ = v.extend_from_slice(&self.file_id.to_le_bytes());
        let _ = v.extend_from_slice(&self.size.to_le_bytes());
        let _ = v.extend_from_slice(&self.chunks.to_le_bytes());
        let _ = v.push(self.chunk_size);
        let _ = v.extend_from_slice(&self.key);
        let _ = v.extend_from_slice(&self.hash);
        let _ = v.push(self.mime.len() as u8);
        let _ = v.extend_from_slice(&self.mime);
        let _ = v.extend_from_slice(&self.name);
        v
    }

    fn decode(b: &[u8]) -> Option<Self> {
        if b.len() < OFFER_FIXED || b[0] != FILE_OFFER {
            return None;
        }
        let ml = b[64] as usize;
        let mime = b.get(OFFER_FIXED..OFFER_FIXED + ml)?;
        let name = b.get(OFFER_FIXED + ml..)?;
        Some(FileOffer {
            file_id: u64::from_le_bytes(b[1..9].try_into().ok()?),
            size: u32::from_le_bytes(b[9..13].try_into().ok()?),
            chunks: u16::from_le_bytes(b[13..15].try_into().ok()?),
            chunk_size: b[15],
            key: b[16..48].try_into().ok()?,
            hash: b[48..64].try_into().ok()?,
            mime: Small::from_slice(mime).ok()?,
            name: Small::from_slice(&name[..name.len().min(64)]).ok()?,
        })
    }
}

/// Fortolker en dekrypteret direkte besked.
pub fn parse(plain: &[u8]) -> Option<Content<'_>> {
    match *plain.first()? {
        TEXT => Some(Content::Text(&plain[1..])),
        FILE_OFFER => FileOffer::decode(plain).map(Content::FileOffer),
        CONTROL | CONTROL_REPLY if plain.len() >= 6 => {
            let n = u32::from_le_bytes(plain[1..5].try_into().ok()?);
            Some(if plain[0] == CONTROL {
                Content::Control(n, &plain[5..])
            } else {
                Content::ControlReply(n, &plain[5..])
            })
        }
        RESYNC => Some(Content::Resync),
        t => Some(Content::Unknown(t)),
    }
}

/// Bygger en kontrolkommando eller et svar (`kind` = `CONTROL` /
/// `CONTROL_REPLY`). `frame` er type + indhold uden længdefelt.
pub fn control(kind: u8, counter: u32, frame: &[u8]) -> heapless::Vec<u8, 256> {
    let mut v = heapless::Vec::new();
    let _ = v.push(kind);
    let _ = v.extend_from_slice(&counter.to_le_bytes());
    let _ = v.extend_from_slice(&frame[..frame.len().min(250)]);
    v
}

/// Bygger en tekstbesked.
pub fn text(t: &[u8]) -> heapless::Vec<u8, 256> {
    let mut v = heapless::Vec::new();
    let _ = v.push(TEXT);
    let _ = v.extend_from_slice(&t[..t.len().min(255)]);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offer_roundtrip() {
        let o = FileOffer {
            file_id: 77,
            size: 7000,
            chunks: 38,
            chunk_size: 186,
            key: [1; 32],
            hash: [2; 16],
            mime: Small::from_slice(b"image/webp").unwrap(),
            name: Small::from_slice("kort æ.webp".as_bytes()).unwrap(),
        };
        let b = o.encode();
        assert_eq!(parse(&b), Some(Content::FileOffer(o)));
    }

    #[test]
    fn control_roundtrip() {
        let c = control(CONTROL, 7, &[0x09]);
        assert_eq!(parse(&c), Some(Content::Control(7, &[0x09][..])));
        let r = control(CONTROL_REPLY, 7, &[0x92, 1, 2]);
        assert_eq!(parse(&r), Some(Content::ControlReply(7, &[0x92, 1, 2][..])));
        assert_eq!(parse(&[CONTROL, 1, 0, 0, 0]), Some(Content::Unknown(CONTROL)), "mangler kommando");
    }

    #[test]
    fn text_and_unknown() {
        assert_eq!(parse(&text(b"hej")), Some(Content::Text(b"hej")));
        assert_eq!(parse(&[9, 1]), Some(Content::Unknown(9)));
        assert_eq!(parse(&[RESYNC]), Some(Content::Resync));
        assert_eq!(parse(&[]), None);
        assert_eq!(parse(&[FILE_OFFER, 1, 2]), None);
    }
}
