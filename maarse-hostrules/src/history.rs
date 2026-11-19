//! Message history: the companion protocol part (commands 0x30-0x33,
//! events 0xB0-0xB9) shared by maarse-host (which stores the log on disk,
//! `history.rs` there) and the Denmark simulator (in memory). See
//! maarse-host's `history.rs` for the record format and the protocol.

use maarse_core::Address;

pub const CMD_GET_HISTORY: u8 = 0x30;
pub const CMD_MARK_READ: u8 = 0x31;
pub const CMD_GET_READ_STATE: u8 = 0x32;
pub const CMD_DELETE: u8 = 0x33;
pub const EV_DELETED: u8 = 0xB9;
pub const DEL_OUTGOING: u8 = 1;
pub const DEL_CONVERSATION: u8 = 2;
pub const EV_ITEM: u8 = 0xB0;
pub const EV_END: u8 = 0xB1;
pub const EV_READ_MARK: u8 = 0xB2;
pub const EV_INFO: u8 = 0xB3;

pub const KIND_DIRECT: u8 = 1;
pub const KIND_CHANNEL: u8 = 2;
pub const KIND_FILE: u8 = 3;

pub const ST_RECEIVED: u8 = 0;
pub const ST_SENT: u8 = 1;
pub const ST_ACKED: u8 = 2;
pub const ST_DEPOSITED: u8 = 3;
/// Delivered by carrying (store-carry-forward).
pub const ST_CARRIED: u8 = 4;

pub const FLAG_OUTGOING: u8 = 1;
pub const FLAG_LIVE: u8 = 2;

pub const MAX_PAGE: usize = 200;
pub const MAX_CONTENT: usize = 240;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub id: u64,
    pub ts: u32,
    pub kind: u8,
    pub outgoing: bool,
    pub state: u8,
    pub hops: u8,
    pub msg_id: u64,
    pub conv: Address,
    pub from: Address,
    pub content: Vec<u8>,
}

/// Commands handled by the history (types 0x30-0x32).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistCmd {
    Get { req: u16, conv: Option<Address>, before: u64, limit: u8 },
    MarkRead { conv: Address, up_to: u64 },
    GetReadState,
    Delete { conv: Address, flags: u8, ts: u32, content: Vec<u8> },
}

pub fn is_history_command(frame: &[u8]) -> bool {
    matches!(frame.first(), Some(0x30..=0x33))
}

pub fn parse_command(frame: &[u8]) -> Option<HistCmd> {
    let (&t, b) = frame.split_first()?;
    match t {
        CMD_GET_HISTORY if b.len() >= 27 => {
            let conv = Address::from_slice(&b[2..18])?;
            Some(HistCmd::Get {
                req: u16::from_le_bytes([b[0], b[1]]),
                conv: (conv != Address::default()).then_some(conv),
                before: u64::from_le_bytes(b[18..26].try_into().ok()?),
                limit: b[26],
            })
        }
        CMD_MARK_READ if b.len() >= 24 => Some(HistCmd::MarkRead {
            conv: Address::from_slice(&b[..16])?,
            up_to: u64::from_le_bytes(b[16..24].try_into().ok()?),
        }),
        CMD_GET_READ_STATE => Some(HistCmd::GetReadState),
        CMD_DELETE if b.len() >= 21 => Some(HistCmd::Delete {
            conv: Address::from_slice(&b[..16])?,
            flags: b[16],
            ts: u32::from_le_bytes(b[17..21].try_into().ok()?),
            content: b[21..].to_vec(),
        }),
        _ => None,
    }
}

/// Content for a file: size | MIME length | MIME | name.
/// A file record that is a page or a page picture (see [`file_content`]).
pub fn is_page_file(content: &[u8]) -> bool {
    let Some(&ml) = content.get(4) else { return false };
    let mime = content.get(5..5 + ml as usize).unwrap_or(&[]);
    let name = content.get(5 + ml as usize..).unwrap_or(&[]);
    mime == maarse_core::content::PAGE_MIME || (name.first() == Some(&b'_') && name.contains(&b'/'))
}

pub fn file_content(size: u32, mime: &[u8], name: &[u8]) -> Vec<u8> {
    let mime = &mime[..mime.len().min(64)];
    let mut v = size.to_le_bytes().to_vec();
    v.push(mime.len() as u8);
    v.extend_from_slice(mime);
    v.extend_from_slice(name);
    v.truncate(MAX_CONTENT);
    v
}

pub fn frame(kind: u8, body: &[u8]) -> Vec<u8> {
    let len = (body.len() + 1) as u16;
    let mut v = Vec::with_capacity(body.len() + 3);
    v.extend_from_slice(&len.to_le_bytes());
    v.push(kind);
    v.extend_from_slice(body);
    v
}

pub fn item_frame(it: &Item, live: bool) -> Vec<u8> {
    let mut b = Vec::with_capacity(56 + it.content.len());
    b.extend_from_slice(&it.id.to_le_bytes());
    b.extend_from_slice(&it.ts.to_le_bytes());
    let flags = if it.outgoing { FLAG_OUTGOING } else { 0 } | if live { FLAG_LIVE } else { 0 };
    b.extend_from_slice(&[flags, it.kind, it.state, it.hops]);
    b.extend_from_slice(&it.msg_id.to_le_bytes());
    b.extend_from_slice(&it.conv.0);
    b.extend_from_slice(&it.from.0);
    b.extend_from_slice(&it.content[..it.content.len().min(MAX_CONTENT)]);
    frame(EV_ITEM, &b)
}

pub fn end_frame(req: u16, conv: Option<Address>, count: u8, more: bool, oldest: u64) -> Vec<u8> {
    let mut b = req.to_le_bytes().to_vec();
    b.extend_from_slice(&conv.unwrap_or_default().0);
    b.extend_from_slice(&[count, more as u8]);
    b.extend_from_slice(&oldest.to_le_bytes());
    frame(EV_END, &b)
}

pub fn read_mark_frame(conv: Address, up_to: u64, unread: usize) -> Vec<u8> {
    let mut b = conv.0.to_vec();
    b.extend_from_slice(&up_to.to_le_bytes());
    b.extend_from_slice(&(unread.min(u16::MAX as usize) as u16).to_le_bytes());
    frame(EV_READ_MARK, &b)
}

