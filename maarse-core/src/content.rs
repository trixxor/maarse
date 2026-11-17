//! The content of a direct message (after decryption).
//!
//! The first byte gives the type:
//! * `0x00` text (UTF-8)
//! * `0x01` file offer: a file follows in `Fragment` packets
//! * `0x02` control command to a node: counter (u32) + companion command
//! * `0x03` reply to a control command: counter (u32) + companion event
//! * `0x04` resync: no data. Sent as init in a new FS session when we
//!   have received a ratchet message we cannot read (e.g. after a restart).
//! * `0x05` request for a `maarse://` page: path (UTF-8, at most
//!   [`MAX_PAGE_PATH`] bytes). The reply is an ordinary file with MIME
//!   [`PAGE_MIME`] (deflate-compressed page markup) and the path as name.
//!   See [`page_path`].
//! * `0x06` call: signalling and audio (Opus) between two clients. The format
//!   inside is decided by the clients; nodes only pass it on, and only
//!   over TCP (maarse-host), never over LoRa.
//! * `0x07` IP tunnel: a piece of an IPv6 packet, TCP only.
//!
//! Control commands are encrypted direct messages, so the recipient knows for
//! sure who the sender is. The node only carries them out if the sender is
//! on its list of administrators and the counter is higher than last time.
//!
//! ```text
//! file offer: 0x01 | file ID (8) | size (u32) | chunk count (u16)
//!            | chunk size (u8) | key (32) | SHA-256/16 (16)
//!            | MIME length (u8) | MIME | name
//! ```

pub const TEXT: u8 = 0x00;
pub const FILE_OFFER: u8 = 0x01;
pub const CONTROL: u8 = 0x02;
pub const CONTROL_REPLY: u8 = 0x03;
pub const RESYNC: u8 = 0x04;
pub const PAGE_REQUEST: u8 = 0x05;
pub const CALL: u8 = 0x06;
/// IP tunnel: a piece of an IPv6 packet (see maarse-host/src/tunnel.rs).
pub const IP: u8 = 0x07;
/// Text compressed with `textz` (sent only to nodes whose announce says they
/// can read it, see `appdata::CAP_TEXTZ`).
pub const TEXT_Z: u8 = 0x08;
/// Linked devices (one person, several devices): `PERSON | op | data`, see
/// the `person` module below and `docs/maybe/multi-device.md`.
pub const PERSON: u8 = 0x09;

/// A sequence number in front of other content: `SEQ | n (u32 LE) | content`.
/// The sender numbers its messages to each contact (1, 2, 3 …), so the
/// receiver can show "messages missing" instead of a silent gap. Sent only to
/// nodes whose announce has `appdata::CAP_SEQ`.
pub const SEQ: u8 = 0x0A;

/// Operations in [`PERSON`] content.
pub mod person {
    /// "I am a device of this person": device certificate (129 bytes).
    pub const CERT: u8 = 1;
    /// Asks a device for its certificate (empty).
    pub const CERT_REQUEST: u8 = 2;
    /// The person's signed device list.
    pub const LIST: u8 = 3;
    /// From the master device to a device waiting to be linked: its new
    /// certificate (129 bytes); the device list follows as [`LIST`].
    pub const LINK_GRANT: u8 = 4;
    /// A copy for a sibling device: kind (1) | peer (16) | id (u32 LE) |
    /// part (high nibble: number from 0, low nibble: count) | text piece.
    pub const SIBLING: u8 = 5;
    /// [`SIBLING`] kinds: a message the device received, or one it sent.
    pub const SIBLING_RECEIVED: u8 = 1;
    pub const SIBLING_SENT: u8 = 2;
    pub const SIBLING_HEADER: usize = 2 + 1 + 16 + 4 + 1;
    /// From a device to a contact: the person's inbox key this week
    /// (`maarse_crypto::person::InboxPublic`, 35 bytes), for mail kept by
    /// post offices under the person.
    pub const INBOX_PUB: u8 = 6;
    /// Between devices of one person (secret): the inbox chain
    /// (`maarse_crypto::person::InboxChain`, 35 bytes).
    pub const INBOX_STATE: u8 = 7;
}

/// MIME for a `maarse://` page: Må:rse markup (UTF-8), compressed with
/// raw deflate (RFC 1951). Not HTML: the client only shows text, headings
/// and links, so a page from a stranger cannot run code.
pub const PAGE_MIME: &[u8] = b"text/x-maarse-page;deflate";
/// Longest path in a page request.
pub const MAX_PAGE_PATH: usize = 60;

/// Fixed part of a file offer.
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
    /// (counter, companion command without length field)
    Control(u32, &'a [u8]),
    /// (counter, companion event without length field)
    ControlReply(u32, &'a [u8]),
    /// The sender has started a new FS session with us.
    Resync,
    /// Request for a page; the path is cleaned with [`page_path`].
    PageRequest(Small),
    /// Call (signalling or audio); the content belongs to the clients.
    Call(&'a [u8]),
    /// IP tunnel: packet ID (u16) | piece no. | count | data.
    Ip(&'a [u8]),
    /// Linked devices: (operation, data), see [`person`].
    Person(u8, &'a [u8]),
    /// A sequence number and the content it numbers (see [`SEQ`]).
    Seq(u32, &'a [u8]),
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

/// Parses a decrypted direct message.
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
        CALL => Some(Content::Call(&plain[1..])),
        IP => Some(Content::Ip(&plain[1..])),
        PERSON if plain.len() >= 2 => Some(Content::Person(plain[1], &plain[2..])),
        SEQ if plain.len() >= 6 => Some(Content::Seq(u32::from_le_bytes(plain[1..5].try_into().ok()?), &plain[5..])),
        PAGE_REQUEST => Some(match page_path(&plain[1..]) {
            Some(p) => Content::PageRequest(p),
            None => Content::Unknown(PAGE_REQUEST),
        }),
        t => Some(Content::Unknown(t)),
    }
}

/// Builds a control command or a reply (`kind` = `CONTROL` /
/// `CONTROL_REPLY`). `frame` is type + content without length field.
pub fn control(kind: u8, counter: u32, frame: &[u8]) -> heapless::Vec<u8, 256> {
    let mut v = heapless::Vec::new();
    let _ = v.push(kind);
    let _ = v.extend_from_slice(&counter.to_le_bytes());
    let _ = v.extend_from_slice(&frame[..frame.len().min(250)]);
    v
}

/// Cleans a page path: lowercase letters, digits, `-`, `_`, `.` and `/`; no
/// `..`, no empty segments. An empty path (or `/`) becomes `index`. An invalid
/// path gives `None`, so it can never point outside the pages folder.
pub fn page_path(raw: &[u8]) -> Option<Small> {
    let raw = core::str::from_utf8(raw).ok()?;
    let raw = raw.trim().trim_matches('/');
    let raw = if raw.is_empty() { "index" } else { raw };
    if raw.len() > MAX_PAGE_PATH {
        return None;
    }
    let mut out = Small::new();
    for seg in raw.split('/') {
        let ok = !seg.is_empty()
            && !seg.starts_with('.')
            && seg.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"-_.".contains(&c));
        if !ok {
            return None;
        }
        if !out.is_empty() {
            out.push(b'/').ok()?;
        }
        out.extend_from_slice(seg.as_bytes()).ok()?;
    }
    Some(out)
}

/// Builds a page request. `None` if the path is invalid.
pub fn page_request(path: &[u8]) -> Option<heapless::Vec<u8, 64>> {
    let p = page_path(path)?;
    let mut v = heapless::Vec::new();
    v.push(PAGE_REQUEST).ok()?;
    v.extend_from_slice(&p).ok()?;
    Some(v)
}

/// Builds a text message.
/// A text content compressed if that makes it shorter (`[TEXT_Z] + z`);
/// other content is returned unchanged.
pub fn compress(plain: &[u8]) -> heapless::Vec<u8, 256> {
    let mut v: heapless::Vec<u8, 256> = heapless::Vec::from_slice(plain).unwrap_or_default();
    if plain.first() == Some(&TEXT) {
        let mut z = [0u8; 255];
        if let Some(n) = crate::textz::compress(&plain[1..], &mut z) {
            v.clear();
            let _ = v.push(TEXT_Z);
            let _ = v.extend_from_slice(&z[..n]);
        }
    }
    v
}

/// Expands compressed text back to `[TEXT] + text`; other content is
/// returned unchanged. `None` if compressed text is broken.
pub fn expand(plain: &[u8]) -> Option<heapless::Vec<u8, 512>> {
    let mut v: heapless::Vec<u8, 512> = heapless::Vec::new();
    if plain.first() == Some(&TEXT_Z) {
        let mut t = [0u8; 511];
        let n = crate::textz::decompress(&plain[1..], &mut t)?;
        let _ = v.push(TEXT);
        v.extend_from_slice(&t[..n]).ok()?;
    } else {
        v.extend_from_slice(plain).ok()?;
    }
    Some(v)
}

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
            name: Small::from_slice("short æ.webp".as_bytes()).unwrap(),
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
        assert_eq!(parse(&[CONTROL, 1, 0, 0, 0]), Some(Content::Unknown(CONTROL)), "missing command");
    }

    #[test]
    fn text_and_unknown() {
        assert_eq!(parse(&text(b"hej")), Some(Content::Text(b"hej")));
        assert_eq!(parse(&[0x7f, 1]), Some(Content::Unknown(0x7f)));
        assert_eq!(parse(&[PERSON, person::CERT_REQUEST]), Some(Content::Person(person::CERT_REQUEST, &[])));
        assert_eq!(parse(&[SEQ, 7, 0, 0, 0, TEXT, b'h']), Some(Content::Seq(7, &[TEXT, b'h'])));
        assert_eq!(parse(&[RESYNC]), Some(Content::Resync));
        assert_eq!(parse(&[]), None);
        assert_eq!(parse(&[FILE_OFFER, 1, 2]), None);
    }

    #[test]
    fn page_paths() {
        let p = |s: &str| page_path(s.as_bytes()).map(|v| std::string::String::from_utf8(v.to_vec()).unwrap());
        assert_eq!(p(""), Some("index".into()));
        assert_eq!(p("/"), Some("index".into()));
        assert_eq!(p("/vejr/aarhus/"), Some("vejr/aarhus".into()));
        assert_eq!(p("about.mu"), Some("about.mu".into()));
        assert_eq!(p("../hemmelig"), None);
        assert_eq!(p("a/../b"), None);
        assert_eq!(p("a//b"), None);
        assert_eq!(p(".skjult"), None);
        assert_eq!(p("Store"), None);
        assert_eq!(p(r"c:\windows"), None);
        assert_eq!(p(&"a".repeat(61)), None);
        let req = page_request(b"/vejr").unwrap();
        assert_eq!(parse(&req), Some(Content::PageRequest(Small::from_slice(b"vejr").unwrap())));
        assert_eq!(parse(&[PAGE_REQUEST, b'.', b'.']), Some(Content::Unknown(PAGE_REQUEST)));
    }
}
