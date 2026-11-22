//! The companion protocol between a client (app, web page) and a node.
//!
//! The same frames are used over serial/USB, Bluetooth (Nordic UART Service)
//! and WebSocket. The stream consists of frames:
//!
//! ```text
//! length (u16, little endian) | type (u8) | content
//! ```
//!
//! `length` counts type + content. Numbers are little endian. Addresses are 16 bytes.
//! Text is UTF-8 and fills the rest of the frame.
//!
//! The client sends commands (type < 0x80); the node sends replies and
//! events (type >= 0x80).
//!
//! Commands 0x30-0x33 and events 0xB0-0xB3 are reserved for the
//! message history in maarse-host (see `maarse-host/src/history.rs`). They
//! are only decoded there; firmware answers with `Error` (invalid frame).
//!
//! Commands 0x2a-0x2b and events 0xa8-0xab are the range test (see
//! [`crate::rangetest`]); only firmware nodes with a radio answer them.

use crate::addr::Address;

pub const MAX_FRAME: usize = 300;
pub const MAX_TEXT: usize = 240;
pub const PROTOCOL_VERSION: u8 = 2;

pub type Text = heapless::Vec<u8, MAX_TEXT>;

/// Wi-Fi backbone settings for nodes with Wi-Fi (ESP32-S3): the node joins
/// the network `ssid` and keeps a TCP link to `peer` (`host:port`, a
/// maarse-host listening with `--listen`). Command `0x28`.
///
/// `modem = false`: the node is a mesh node of its own and peers with the
/// host over the backbone. `modem = true`: the node is the host's LoRa modem
/// over Wi-Fi (like `--serial` over USB); the host does all routing.
///
/// On the wire the first byte holds flags: bit 0 enabled, bit 1 modem.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WifiSettings {
    pub enabled: bool,
    pub modem: bool,
    pub ssid: heapless::Vec<u8, 32>,
    pub password: heapless::Vec<u8, 64>,
    pub peer: heapless::Vec<u8, 64>,
    /// The modem key (see `modemauth`), from flash; never sent in a frame.
    pub modem_key: Option<[u8; 32]>,
}

/// Wi-Fi state in [`Event::Wifi`].
pub mod wifi_state {
    /// Wi-Fi is off (disabled, or enabled but the node has not restarted yet).
    pub const OFF: u8 = 0;
    /// Joining the Wi-Fi network.
    pub const JOINING: u8 = 1;
    /// On the network with an IP address, connecting to the peer.
    pub const ONLINE: u8 = 2;
    /// Backbone link to the peer is up.
    pub const LINKED: u8 = 3;
    /// Not started: the board crashed twice in a row with Wi-Fi on.
    pub const SKIPPED_AFTER_CRASH: u8 = 10;
    /// Not started: the SSID or password is not valid for the driver.
    pub const BAD_SETTINGS: u8 = 11;
    /// Not started: the Wi-Fi driver could not be initialised.
    pub const DRIVER_FAILED: u8 = 12;
}
pub type FrameBuf = heapless::Vec<u8, { MAX_FRAME + 2 }>;

/// Commands from client to node.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)] // text fields; frames are small and short-lived
pub enum Command {
    /// 0x01: who are you?
    GetInfo,
    /// 0x02: send an encrypted direct message.
    SendDirect { req: u16, dest: Address, text: Text },
    /// 0x03: send to a channel we are a member of.
    SendChannel {
        req: u16,
        channel: Address,
        text: Text,
    },
    /// 0x04: join a channel. A name like `#aarhus`, or `public`.
    JoinChannel { name: Text },
    /// 0x05: send an announce now.
    AnnounceNow,
    /// 0x06: start a file to `dest`. The data follows in `FileData`.
    FileBegin {
        req: u16,
        dest: Address,
        size: u32,
        mime: Text,
        name: Text,
    },
    /// 0x07: a chunk of the file (up to 240 bytes).
    FileData { req: u16, offset: u32, data: Text },
    /// 0x08: the whole file has been transferred to the node; send it.
    FileEnd { req: u16 },
    /// 0x09: ask for status and statistics.
    GetStats,
    /// 0x0a: set radio settings (stored in flash).
    SetRadio(RadioConfig),
    /// 0x0b: which radio settings are in use?
    GetRadio,
    /// 0x0c: send a command (`GetStats`, `GetRadio`, `SetRadio`, `SetRole`) to a
    /// remote node we are an administrator of. The reply comes as
    /// `RemoteReply`.
    Remote { req: u16, dest: Address, frame: Text },
    /// 0x0d: who may administer this node (at most 4). Local only.
    SetAdmins { admins: heapless::Vec<Address, 4> },
    /// 0x0e: show the list of administrators.
    GetAdmins,
    /// 0x0f: the clock (Unix seconds). The client sends it on connect,
    /// so nodes without a clock can timestamp announces and control commands.
    SetTime { unix: u32 },
    /// 0x10: choose the node's role (0 = client, 1 = repeater, 2 = relay). Stored
    /// in flash and can also be sent via `Remote`.
    SetRole { role: u8 },
    /// 0x11: set the node's regions (comma separated, e.g. `dk,nordjylland`, at most
    /// 8) and `strict` (1 = with an empty list only floods without a region are forwarded).
    /// Stored in flash. See [`crate::region`].
    SetRegions { strict: bool, names: Text },
    /// 0x12: show the node's regions.
    GetRegions,
    /// 0x13: join a private channel with a random key (from an
    /// invitation). Format: key (32 bytes) | name (UTF-8, the rest). The reply
    /// is `ChannelJoined` as for other channels.
    JoinPrivateChannel { key: [u8; 32], name: Text },
    /// 0x20: trace route to `dest`: SNR for every hop both ways. The reply
    /// is `TraceResult` (or `Error` with `req`). See [`crate::diag`].
    TraceRoute { req: u16, dest: Address },
    /// 0x21: show direct neighbours. The reply is one or more `Neighbors`.
    GetNeighbors,
    /// 0x22: what the node shares in its announces: position (precision 0 =
    /// off, the default) and telemetry. Stored. The reply is `Sharing`.
    SetSharing(Sharing),
    /// 0x23: show the sharing settings.
    GetSharing,
    /// 0x24: set the node's display name (UTF-8, at most 24 bytes; empty = none).
    /// Reply: `Name`. The name is sent in announces and is not proven.
    SetName { name: Text },
    /// 0x25: get the display name. Reply: `Name`.
    GetName,
    /// 0x3a: set the site of a fixed node (antenna height, gain, owner; see
    /// [`crate::appdata::Site`]). Stored and sent in announces while the
    /// height is set; height 0 clears it. The node fills in its own transmit
    /// power. Reply: `Site`.
    SetSite(crate::appdata::Site),
    /// 0x3b: get the site. Reply: `Site`.
    GetSite,
    /// 0x28: set the Wi-Fi backbone settings (local only: USB or Bluetooth,
    /// never through `Remote`, as it carries the password). Saved; the node
    /// restarts to apply them. Reply: `Wifi`. Nodes without Wi-Fi answer
    /// `Error`.
    SetWifi(WifiSettings),
    /// 0x2d: the Wi-Fi modem key's passphrase (empty = no key). Local only:
    /// refused over remote control. The node keeps only the derived key.
    SetModemKey { passphrase: Text },
    /// 0x29: Wi-Fi status. Reply: `Wifi` (never contains the password).
    GetWifi,
    /// 0x26: ask `dest` for the page `path` (`maarse://<dest>/<path>`). Reply:
    /// `Sent`; the page arrives as a file with MIME
    /// [`crate::content::PAGE_MIME`]. An invalid path gives `Error`.
    RequestPage { req: u16, dest: Address, path: Text },
    /// Writes a note on a page's notice board (`@board`) at `dest`.
    PostBoard { req: u16, dest: Address, path: Text, text: Text },
    /// 0x27: call data to `dest` (signalling or Opus audio, the format
    /// belongs to the clients). TCP only: when `dest` can only be reached over LoRa,
    /// the reply is `Error` with `NOT_OVER_TCP`. No acknowledgement to the client and
    /// no mailbox; old audio packets are worthless.
    SendCall { dest: Address, data: Text },
    /// 0x18: start (or resume) a firmware update. `manifest` is the
    /// signed manifest (112 bytes, see `maarse_crypto::ota`). Flag bit 0:
    /// allow an older version (local only, ignored via `Remote`). The reply is
    /// `OtaState` with the offset the client should send from. Can be sent via
    /// `Remote` (administrators only). 0x14-0x17 are kept free for other work.
    OtaBegin { req: u16, flags: u8, manifest: Text },
    /// 0x19: a chunk of the image. Offset and length must be multiples of 4
    /// (except the last chunk). The reply is `OtaState` with the next offset.
    OtaChunk { req: u16, offset: u32, data: Text },
    /// 0x1a: finish. The image must be fully transferred and verified; with
    /// `apply` it is installed and the node restarts.
    OtaEnd { req: u16, apply: bool },
    /// 0x1b: how far along is an update? The reply is `OtaState`.
    OtaStatus,
    /// 0x14: how large a file may be sent to `dest` along the current route?
    /// The reply is `FileLimit`. Firmware nodes do not know the command (they
    /// answer with `Error`); the client then uses `Info.max_file`.
    GetFileLimit { dest: Address },
    /// 0x34: replace the node's identity with `secret` (64 bytes: Ed25519 seed |
    /// X25519 key), typically derived from a recovery phrase in the client (see
    /// `maarse_crypto::recovery`). Local only (USB/BLE) — always rejected via
    /// `Remote`. Requires `confirm` (flag [`IDENTITY_CONFIRM`]), otherwise it is
    /// rejected. The node stores the key, forgets routes and forward secrecy sessions
    /// and sends a new announce. The reply is `IdentityChanged`.
    /// 0x35-0x37 are kept free for linked devices.
    SetIdentity { confirm: bool, secret: [u8; 64] },
    /// 0x2a: start or stop the range test (see [`crate::rangetest::Config`]).
    /// With [`crate::rangetest::FLAG_PERSIST`] the setting is kept in flash and
    /// resumed after a restart; otherwise it lasts until the next restart. Local
    /// only. The reply is `RangeState`.
    SetRangeTest(crate::rangetest::Config),
    /// 0x2b: range test status. The reply is `RangeState` followed by `Power`.
    GetRangeTest,
    /// 0x2c: issue a signed alert (hosts with an issuer key only; see
    /// `maarse_crypto::alert`). Alerts are never sent by typing on the
    /// bulletin channel. The reply is `Sent` (msg_id = the alert's serial).
    SendAlert { req: u16, meta: AlertMeta, text: Text },
    /// 0x35: on the master device, link `device` (which must be waiting, see
    /// [`person_op::AWAIT_LINK`]): sign its certificate, add it to the device
    /// list and send both to it. Local only. Replies: `Person`, and
    /// `DeviceLinked` on every device of the person.
    LinkDevice { req: u16, device: Address },
    /// 0x36: the person this device belongs to. The reply is `Person`.
    GetPerson,
    /// 0x37: set up linked devices on this device, see [`person_op`]. Local
    /// only. Replies: `Person` (and `PersonPhrase` after `CREATE`).
    PersonSetup { req: u16, op: u8, text: Text },
    /// 0x38: stop sending a file (the reply is `FileDone` with ok = false).
    CancelFile { file_id: u64 },
    /// 0x39: sensor nodes, see [`crate::sensor`]. `dest` is the node the
    /// command is for: all zeros = the connected node (USB, Bluetooth) or the
    /// host itself. A remote node gets it as a control command (the sender must
    /// be one of its owners). The answers are `Event::Sensor`.
    Sensor { req: u16, dest: Address, op: u8, data: Text },
}

/// Operations of `Command::PersonSetup` (linked devices,
/// `docs/maybe/multi-device.md`).
pub mod person_op {
    /// Make a new person with this device as the master device. The node
    /// makes a fresh 24-word phrase and sends it once as `PersonPhrase`.
    pub const CREATE: u8 = 1;
    /// Make this device the master device of the person behind the phrase in
    /// `text` (lowercase words, one space between).
    pub const RESTORE: u8 = 2;
    /// Wait (10 minutes) to be linked by a master device; announces at once.
    pub const AWAIT_LINK: u8 = 3;
    /// Forget the person on this device (the device keeps its own identity).
    pub const LEAVE: u8 = 4;
    /// Stop waiting to be linked.
    pub const CANCEL: u8 = 5;
    /// On the master device: remove the device whose address (32 hex
    /// characters) is in `text`. It is marked revoked in a new device list,
    /// and the inbox key starts a new chain it does not get.
    pub const REMOVE: u8 = 6;
    /// On the master device: write an encrypted backup of the master
    /// (`account-backup.maarse` in the data folder) and send its 6-word code
    /// once as `PersonBackup`.
    pub const BACKUP: u8 = 7;
    /// Make this device the master device from a backup the client has put
    /// in the data folder as `account-restore.maarse`; `text` is the code.
    /// The old master device is removed and the inbox key starts a new chain.
    pub const RESTORE_BACKUP: u8 = 8;
    /// Sync now: fetch the person's mail from its post offices and ask the
    /// other devices for copies of what this device lacks.
    pub const SYNC: u8 = 9;
    /// Sign a MeshCore service challenge supplied as an operation byte and 32 raw bytes.
    pub const MESHCORE_PROOF: u8 = 10;
}

/// States in `Event::Person`.
pub mod person_state {
    pub const NONE: u8 = 0;
    pub const AWAITING_LINK: u8 = 1;
    pub const MEMBER: u8 = 2;
}

/// `Event::Person`: the person this device belongs to.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PersonInfo {
    /// `person_state::*`.
    pub state: u8,
    /// This device holds the master secret.
    pub master: bool,
    /// The person's address (zero when `NONE` or `AWAITING_LINK`).
    pub person: Address,
    /// This device's number.
    pub index: u8,
    pub version: u32,
    /// (device, revoked).
    pub devices: heapless::Vec<(Address, bool), 4>,
    /// While waiting to be linked: the words the master device must show too.
    pub words: heapless::Vec<u8, 48>,
}

/// The fields of an alert besides text, issuer and serial (see
/// `maarse_crypto::alert`). 15 bytes, or 25 with an area.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct AlertMeta {
    /// 0 actual, 1 exercise, 2 test, 3 system.
    pub status: u8,
    /// 0 alert, 1 update, 2 cancel.
    pub kind: u8,
    pub event: u8,
    /// 0 minor .. 3 extreme.
    pub severity: u8,
    pub expires_min: u16,
    /// Serial of the alert an update or cancel refers to.
    pub reference: u32,
    /// Latitude, longitude (1e-5 degrees) and radius (100 m).
    pub area: Option<(i32, i32, u16)>,
}

impl AlertMeta {
    fn put(&self, w: W) -> W {
        let w = w
            .b(&[self.status, self.kind, self.event, self.severity])
            .b(&self.expires_min.to_le_bytes())
            .b(&self.reference.to_le_bytes());
        match self.area {
            Some((lat, lon, r)) => w.b(&[1]).b(&lat.to_le_bytes()).b(&lon.to_le_bytes()).b(&r.to_le_bytes()),
            None => w.b(&[0]),
        }
    }
    fn take(r: &mut R) -> Result<Self, FrameError> {
        let mut m = AlertMeta { status: r.u8()?, kind: r.u8()?, event: r.u8()?, severity: r.u8()?, ..Default::default() };
        m.expires_min = r.u16()?;
        m.reference = r.u32()?;
        if r.u8()? == 1 {
            m.area = Some((r.u32()? as i32, r.u32()? as i32, r.u16()?));
        }
        Ok(m)
    }
}

/// How a shown alert was trusted (`Event::Alert`).
pub mod alert_trust {
    /// An issuer key configured on the host.
    pub const KEY: u8 = 0;
    /// A certificate from a configured root key (`name` is from it).
    pub const CERT: u8 = 1;
    /// An old v1 bulletin (signed, but without expiry or serial).
    pub const LEGACY: u8 = 2;
}

/// Flag in `Command::SetIdentity`: the user has confirmed the replacement.
pub const IDENTITY_CONFIRM: u8 = 1;

/// Sharing settings (10 bytes): precision (u8, see
/// [`crate::appdata::Precision`]) | latitude (i32, 1e-7°) | longitude (i32)
/// | telemetry (u8, 0/1). The position is stored precisely on the node, but only
/// sent rounded to the chosen precision.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Sharing {
    pub precision: u8,
    pub lat_e7: i32,
    pub lon_e7: i32,
    pub telemetry: bool,
}

impl Sharing {
    pub const LEN: usize = 10;
    pub fn to_bytes(&self) -> [u8; Self::LEN] {
        let mut b = [0u8; Self::LEN];
        b[0] = self.precision;
        b[1..5].copy_from_slice(&self.lat_e7.to_le_bytes());
        b[5..9].copy_from_slice(&self.lon_e7.to_le_bytes());
        b[9] = self.telemetry as u8;
        b
    }
    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        if b.len() < Self::LEN {
            return None;
        }
        Some(Sharing {
            precision: b[0].min(3),
            lat_e7: i32::from_le_bytes(b[1..5].try_into().ok()?),
            lon_e7: i32::from_le_bytes(b[5..9].try_into().ok()?),
            telemetry: b[9] != 0,
        })
    }
    /// The position as it is sent (rounded), or `None` when off or awaiting a fix.
    pub fn position(&self) -> Option<crate::appdata::Position> {
        if self.lat_e7 == 0 && self.lon_e7 == 0 {
            return None;
        }
        crate::appdata::Position {
            precision: crate::appdata::Precision::from_u8(self.precision),
            lat_e7: self.lat_e7,
            lon_e7: self.lon_e7,
        }
        .quantised()
    }
}

/// A neighbour in `Event::Neighbors` (15 bytes): path ID (u16) | address
/// prefix (4, zeros = unknown) | SNR (i8) | RSSI (i16, 0 = unknown) |
/// seconds since (u32) | packet count (u16).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NeighborInfo {
    pub id: u16,
    pub prefix: [u8; 4],
    pub snr: i8,
    pub rssi: i16,
    pub age_s: u32,
    pub count: u16,
}

impl NeighborInfo {
    pub fn from_neighbor(n: &crate::diag::Neighbor, now_ms: u64) -> Self {
        let mut prefix = [0u8; 4];
        if let Some(a) = n.addr {
            prefix.copy_from_slice(&a.0[..4]);
        }
        NeighborInfo {
            id: n.id,
            prefix,
            snr: n.snr,
            rssi: n.rssi,
            age_s: (now_ms.saturating_sub(n.last_ms) / 1000) as u32,
            count: n.count.min(u16::MAX as u32) as u16,
        }
    }
}

/// At most this many neighbours per `Neighbors` frame.
pub const NEIGHBORS_PER_FRAME: usize = 16;

/// Builds `Neighbors` frames from a neighbour table (at least one, even if empty).
pub fn neighbor_events<const N: usize>(
    t: &crate::diag::NeighborTable<N>,
    now_ms: u64,
) -> heapless::Vec<Event, 4> {
    let all = t.sorted();
    let mut out = heapless::Vec::new();
    let mut part = 0u8;
    let mut chunks = all.chunks(NEIGHBORS_PER_FRAME).peekable();
    if chunks.peek().is_none() {
        let _ = out.push(Event::Neighbors { part: 0, last: true, items: heapless::Vec::new() });
    }
    while let Some(c) = chunks.next() {
        let items = c.iter().map(|n| NeighborInfo::from_neighbor(n, now_ms)).collect();
        let last = chunks.peek().is_none();
        let _ = out.push(Event::Neighbors { part, last, items });
        part += 1;
    }
    out
}

/// States in `Event::OtaState`.
pub mod ota_state {
    pub const IDLE: u8 = 0;
    pub const RECEIVING: u8 = 1;
    pub const VERIFIED: u8 = 2;
    pub const INSTALLING: u8 = 3;
    pub const ERROR: u8 = 4;
    /// The board does not support over-the-air updates.
    pub const UNSUPPORTED: u8 = 5;
}

/// Flag in `Command::OtaBegin`.
pub const OTA_ALLOW_DOWNGRADE: u8 = 1;

/// Replies and events from node to client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// 0x81
    Info {
        addr: Address,
        repeater: bool,
        version: u8,
        max_text: u8,
        max_file: u32,
        /// The node's public keys (Ed25519 | X25519), for safety numbers.
        /// Added at the end; older nodes do not send them.
        identity: Option<[u8; 64]>,
    },
    /// 0x82: the message has been sent (or queued while the route is found).
    Sent { req: u16, msg_id: u64 },
    /// 0x83
    DirectReceived { from: Address, hops: u8, text: Text },
    /// 0x84
    ChannelReceived {
        channel: Address,
        from: Address,
        hops: u8,
        text: Text,
    },
    /// 0x85: the recipient has acknowledged.
    Acked { msg_id: u64 },
    /// 0x86
    RouteLearned { dest: Address, hops: u8 },
    /// 0x87: we heard and verified an announce. `identity` is
    /// the sender's public keys (64 bytes at the end, if known).
    Heard { addr: Address, hops: u8, identity: Option<[u8; 64]> },
    /// 0x88
    ChannelJoined { channel: Address, name: Text },
    /// 0x89: a file is on its way to us.
    FileIncoming {
        file_id: u64,
        from: Address,
        size: u32,
        mime: Text,
        name: Text,
    },
    /// 0x8a: a chunk of a received file (sent to the client when the file is complete).
    FileChunk {
        file_id: u64,
        offset: u32,
        data: Text,
    },
    /// 0x8b
    FileProgress {
        file_id: u64,
        done: u16,
        total: u16,
        outgoing: bool,
    },
    /// 0x8c
    FileDone {
        file_id: u64,
        ok: bool,
        outgoing: bool,
        text: Text,
    },
    /// 0x8d: reply to `FileEnd`: the file has got an ID and is being sent now.
    FileStarted { req: u16, file_id: u64, chunks: u16 },
    /// 0x8e: free text for debugging.
    Log { text: Text },
    /// 0x90: the recipient could not be reached; the message is now in its mailbox.
    Deposited { msg_id: u64 },
    /// 0x91: status as pairs of (key, value). See [`stat`].
    Stats { items: Stats },
    /// 0x92: the current radio settings.
    Radio(RadioConfig),
    /// 0x93: reply from a remote node: a complete event without a length field.
    RemoteReply { from: Address, frame: Text },
    /// 0x94: administrators of this node.
    Admins { admins: heapless::Vec<Address, 4> },
    /// 0x95: the node's regions (comma separated) and `strict`.
    Regions { strict: bool, names: Text },
    /// 0x9a: the firmware update state (see [`ota_state`]). `next` is
    /// the next expected offset, `size` and `version` from the manifest (0 without),
    /// `current` the running version. Text explains errors.
    OtaState { req: u16, state: u8, next: u32, size: u32, version: u32, current: u32, text: Text },
    /// 0x96: reply to `GetFileLimit`. Format: dest (16) | max_bytes (u32) |
    /// flags (u8: bit 0 = over LoRa or unknown route, bit 1 = the recipient
    /// cannot be reached, the file goes to a mailbox) | est_airtime_s (u32, estimated
    /// time on air for `max_bytes` over LoRa; 0 over the internet).
    FileLimit {
        dest: Address,
        max_bytes: u32,
        via_lora: bool,
        mailbox: bool,
        est_airtime_s: u32,
    },
    /// 0x8f
    Error { req: u16, code: u8, text: Text },
    /// 0xa0: reply to `TraceRoute`. `path` is the repeaters' path IDs; `out` is
    /// SNR for every hop out (last = the target's), `back` for every hop
    /// back (last = ours). Format: req (u16) | target (16) | k | k × ID
    /// (u16) | m | m × SNR (i8) | SNR back (the rest).
    TraceResult {
        req: u16,
        dest: Address,
        path: heapless::Vec<u16, 12>,
        out: heapless::Vec<i8, 13>,
        back: heapless::Vec<i8, 13>,
    },
    /// 0xa1: direct neighbours, most recently heard first. `part` counts from 0;
    /// `last` is set on the last frame. Format: part | last | neighbours (15 B).
    Neighbors {
        part: u8,
        last: bool,
        items: heapless::Vec<NeighborInfo, NEIGHBORS_PER_FRAME>,
    },
    /// 0xa2: sharing settings (see [`Sharing`]).
    Sharing(Sharing),
    /// 0xa4: the node's own display name (empty = none).
    Name { name: Text },
    /// 0xbb: the node's site (height 0 = not set).
    Site(crate::appdata::Site),
    /// 0xa5: call data from `from` (see `SendCall`).
    Call { from: Address, data: Text },
    /// 0xa6: Wi-Fi backbone status (see [`wifi_state`]). `ip` is 0.0.0.0
    /// until the node has an address.
    Wifi {
        enabled: bool,
        /// The node is the host's LoRa modem over Wi-Fi (see [`WifiSettings`]).
        modem: bool,
        state: u8,
        ip: [u8; 4],
        ssid: heapless::Vec<u8, 32>,
        peer: heapless::Vec<u8, 64>,
    },
    /// 0xa7: a message was delivered by carrying (store-carry-forward): the
    /// destination's delivered notice came back. Shown as "delivered
    /// (carried)"; otherwise like `Acked`.
    CarryAcked { msg_id: u64 },
    /// 0xa3: position and/or telemetry from another node's announce.
    /// Format: address (16) | hops | app data fields (0x03/0x04, see
    /// [`crate::appdata`]).
    NodeInfo {
        addr: Address,
        hops: u8,
        position: Option<crate::appdata::Position>,
        telemetry: Option<crate::appdata::Telemetry>,
        name: Option<crate::appdata::Name>,
        /// Capabilities from the announce (`appdata::CAP_*`), if it had them.
        caps: Option<u8>,
    },
    /// 0xb4: the node's identity has been replaced (reply to `SetIdentity`).
    /// Format: new address (16) | public identity (64).
    IdentityChanged { addr: Address, identity: [u8; 64] },
    /// 0xa8: range test status (reply to `SetRangeTest`/`GetRangeTest`, and
    /// after every SF step on a receiver). See [`RangeState`].
    RangeState(RangeState),
    /// 0xa9: the receiver heard a range test beacon. See [`RangeRx`].
    RangeRx(RangeRx),
    /// 0xaa: the beacon queued a beacon for transmission. See [`RangeTx`].
    RangeTx(RangeTx),
    /// 0xab: power use estimate and battery (every minute while a range test
    /// runs, and after `GetRangeTest`). See [`PowerReport`].
    Power(PowerReport),
    /// 0xac: a verified alert, shown once (repeats and replays are dropped
    /// by the host). The client draws it apart from all chat text, so no
    /// message can look like one.
    Alert {
        issuer: Address,
        serial: u32,
        issued: u32,
        meta: AlertMeta,
        hops: u8,
        /// `alert_trust::*`.
        trust: u8,
        /// The issuer's name from its certificate (empty when trusted by key).
        name: heapless::Vec<u8, 24>,
        text: Text,
    },
    /// 0xb5: a device was linked to our person (shown on every device, so an
    /// unknown device is noticed). `words`: the new device's link words, which
    /// it showed while waiting; the user checks they are the same.
    DeviceLinked { device: Address, index: u8, words: heapless::Vec<u8, 48> },
    /// 0xb6: the person this device belongs to (reply to `GetPerson`, and on
    /// every change). Format: state | flags (bit 0 master) | person (16) |
    /// index | version (u32) | count | [device (16) | revoked (1)]… | words.
    Person(PersonInfo),
    /// 0xb7: the 24 words of a new person, sent once after
    /// `PersonSetup CREATE`. Written on paper; the node does not keep them.
    PersonPhrase { phrase: Text },
    /// 0xbd: an answer or a push about a sensor node (`node` = its address, all
    /// zeros for the connected node): see `sensor::op::EV_*`.
    Sensor { node: Address, op: u8, data: Text },
    /// 0xb8: a backup was written (`account-backup.maarse`); its 6-word code,
    /// sent once. The node does not keep the code.
    PersonBackup { code: Text },
    /// 0xbc: one chunk of a linked-device proof for the MeshCore service.
    MeshCoreProof { req: u16, offset: u16, total: u16, data: Text },
}

/// Per-SF results on a range test receiver (12 bytes): sf | rx (u16) | lost
/// (u16) | average RSSI (i16, dBm) | lowest RSSI (i16) | average SNR x 4 (i16)
/// | lowest SNR (i8, dB).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct RangeSfStat {
    pub sf: u8,
    pub rx: u16,
    pub lost: u16,
    pub rssi_avg: i16,
    pub rssi_min: i16,
    pub snr_avg_x4: i16,
    pub snr_min: i8,
}

/// Range test status (`Event::RangeState`). Format: config (15) | current SF |
/// flags (bit 0: the receiver follows the beacon's schedule) | count (u32) |
/// missed (u32) | seq (u32) | per-SF results ([`RangeSfStat`], 12 B each).
///
/// On a beacon `count` is beacons queued, `missed` beacons that could not be
/// transmitted (duty cycle or busy channel) and `seq` the next sequence number.
/// On a receiver `count` is beacons received, `missed` beacons lost (from
/// sequence gaps) and `seq` the last sequence number heard.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct RangeState {
    pub config: crate::rangetest::Config,
    pub sf: u8,
    pub synced: bool,
    pub count: u32,
    pub missed: u32,
    pub seq: u32,
    pub per_sf: heapless::Vec<RangeSfStat, { crate::rangetest::MAX_STEPS }>,
}

/// A received beacon (`Event::RangeRx`). Format: uptime (u32, ms) | sender (4) |
/// seq (u32) | sf | rssi (i16, dBm) | snr (i8, dB) | length | lost (u16, since
/// the previous beacon) | skipped (u16, the sender's own count of beacons it
/// could not send) | tx power (i8, dBm) | battery (u16, mV, the beacon's) |
/// [lat_e7 (i32) | lon_e7 (i32)].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct RangeRx {
    pub uptime_ms: u32,
    pub sender: [u8; 4],
    pub seq: u32,
    pub sf: u8,
    pub rssi: i16,
    pub snr: i8,
    pub len: u8,
    pub lost: u16,
    pub skipped: u16,
    pub tx_power_dbm: i8,
    pub battery_mv: u16,
    pub position: Option<(i32, i32)>,
}

/// A beacon was queued (`Event::RangeTx`). Format: uptime (u32, ms) | seq
/// (u32) | sf | slot | interval (u16, s) | airtime (u16, ms) | estimated charge
/// (u32, mA*ms = uC, airtime x the board's estimated TX current) | skipped (u16).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct RangeTx {
    pub uptime_ms: u32,
    pub seq: u32,
    pub sf: u8,
    pub slot: u8,
    pub interval_s: u16,
    pub airtime_ms: u16,
    pub charge_ma_ms: u32,
    pub skipped: u16,
}

/// Time and estimated charge spent in one range test mode since boot (12 B):
/// seconds (u32) | TX on-time (u32, ms) | estimated charge (u32, mA*s).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct PowerPhase {
    pub secs: u32,
    pub tx_ms: u32,
    pub charge_mas: u32,
}

/// Power report (`Event::Power`). All currents and charges are *estimates*
/// from the board's current profile (base + receive + transmit current) and
/// the measured TX on-time, not measurements. Format: uptime (u32, s) |
/// battery (u16, mV, 0 = unknown) | external power (i8: -1 unknown, 0, 1) |
/// mode | tx power (i8, dBm) | base current (u16, 0.1 mA) | receive current
/// (u16, 0.1 mA) | transmit current (u16, 0.1 mA, at the current TX power) |
/// TX packets (u32) | phases off, beacon, receiver ([`PowerPhase`], 12 B each).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct PowerReport {
    pub uptime_s: u32,
    pub battery_mv: u16,
    pub ext_power: i8,
    pub mode: u8,
    pub tx_power_dbm: i8,
    pub base_ma_x10: u16,
    pub rx_ma_x10: u16,
    pub tx_ma_x10: u16,
    pub tx_packets: u32,
    pub phases: [PowerPhase; 3],
}

/// The LoRa radio settings.
///
/// All nodes in a net must use the same frequency, bandwidth, spreading
/// factor and coding rate to hear each other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RadioConfig {
    pub freq_hz: u32,
    /// Bandwidth in Hz: 7800, 10400, 15600, 20800, 31250, 41700, 62500,
    /// 125000, 250000 or 500000.
    pub bw_hz: u32,
    /// Spreading factor 5..=12.
    pub sf: u8,
    /// Coding rate denominator 5..=8 (4/5..4/8).
    pub cr: u8,
    pub tx_power_dbm: i8,
    /// Duty cycle in permille (100 = 10 %).
    pub duty_permille: u16,
    pub preamble: u16,
    /// How often the node announces once it has heard a repeater or an
    /// anchor, in quarter hours (0 = the default, 12 h; up to about 64 h).
    /// Until then it announces every 15 minutes, so it is found quickly after
    /// a start. Byte 15, which older nodes and clients leave at 0.
    pub announce_quarters: u8,
}

impl RadioConfig {
    pub const LEN: usize = 16;
    /// Announce interval when `announce_quarters` is 0: 12 hours.
    pub const DEFAULT_ANNOUNCE_S: u32 = 12 * 3600;
    /// Announce interval after a start, until a repeater or anchor is heard.
    pub const BOOT_ANNOUNCE_S: u32 = 15 * 60;

    /// The announce interval in seconds once a repeater has been heard.
    pub fn announce_interval_s(&self) -> u32 {
        match self.announce_quarters {
            0 => Self::DEFAULT_ANNOUNCE_S,
            q => q as u32 * 900,
        }
    }
    /// Må:rse default in the EU: 869.525 MHz (the 10 % band), 125 kHz, SF9, 4/5, 22 dBm.
    pub const EU_DEFAULT: RadioConfig = RadioConfig {
        freq_hz: 869_525_000,
        bw_hz: 125_000,
        sf: 9,
        cr: 5,
        tx_power_dbm: 22,
        duty_permille: 100,
        preamble: 16,
        announce_quarters: 0,
    };
    pub const BANDWIDTHS: [u32; 10] = [
        7800, 10400, 15600, 20800, 31250, 41700, 62500, 125_000, 250_000, 500_000,
    ];

    pub fn to_bytes(&self) -> [u8; Self::LEN] {
        let mut b = [0u8; Self::LEN];
        b[0..4].copy_from_slice(&self.freq_hz.to_le_bytes());
        b[4..8].copy_from_slice(&self.bw_hz.to_le_bytes());
        b[8] = self.sf;
        b[9] = self.cr;
        b[10] = self.tx_power_dbm as u8;
        b[11..13].copy_from_slice(&self.duty_permille.to_le_bytes());
        b[13..15].copy_from_slice(&self.preamble.to_le_bytes());
        b[15] = self.announce_quarters;
        b
    }

    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        if b.len() < Self::LEN {
            return None;
        }
        Some(RadioConfig {
            freq_hz: u32::from_le_bytes(b[0..4].try_into().ok()?),
            bw_hz: u32::from_le_bytes(b[4..8].try_into().ok()?),
            sf: b[8],
            cr: b[9],
            tx_power_dbm: b[10] as i8,
            duty_permille: u16::from_le_bytes([b[11], b[12]]),
            preamble: u16::from_le_bytes([b[13], b[14]]),
            announce_quarters: b[15],
        })
    }

    /// Checks that the settings can be used by an SX1262.
    pub fn validate(&self) -> Result<(), &'static str> {
        if !(150_000_000..=960_000_000).contains(&self.freq_hz) {
            return Err("The frequency must be between 150 and 960 MHz");
        }
        if !Self::BANDWIDTHS.contains(&self.bw_hz) {
            return Err("Invalid bandwidth");
        }
        if !(5..=12).contains(&self.sf) {
            return Err("Spreading factor must be 5-12");
        }
        if !(5..=8).contains(&self.cr) {
            return Err("Coding rate must be 4/5-4/8");
        }
        if !(-9..=22).contains(&self.tx_power_dbm) {
            return Err("Transmit power must be -9 to 22 dBm");
        }
        if self.duty_permille == 0 || self.duty_permille > 1000 {
            return Err("Duty cycle must be 0.1-100 %");
        }
        if !(6..=1000).contains(&self.preamble) {
            return Err("The preamble must be 6-1000 symbols");
        }
        Ok(())
    }

    /// Parameters for computing time on air.
    pub fn airtime(&self) -> crate::airtime::LoraParams {
        crate::airtime::LoraParams {
            sf: self.sf,
            bw_hz: self.bw_hz,
            cr: self.cr.saturating_sub(4).max(1),
            preamble: self.preamble,
        }
    }
}

/// Status pairs: (key, value). At most 59, so the frame stays below 300 bytes.
pub type Stats = heapless::Vec<(u8, i32), 59>;

/// Keys in `Event::Stats`. The client shows them with readable names.
pub mod stat {
    pub const UPTIME_S: u8 = 1;
    pub const KNOWN_NODES: u8 = 2;
    pub const ROUTES: u8 = 3;
    pub const CHANNELS: u8 = 4;
    pub const LINKS: u8 = 5;
    pub const CLIENTS: u8 = 6;
    pub const ROLE: u8 = 7;
    // Routing
    pub const RX_PACKETS: u8 = 10;
    pub const RX_INVALID: u8 = 11;
    pub const RX_DUPLICATES: u8 = 12;
    pub const FORWARDED: u8 = 13;
    pub const ORIGINATED: u8 = 14;
    pub const DELIVERED: u8 = 15;
    pub const ACKED: u8 = 16;
    pub const ROUTES_LEARNED: u8 = 17;
    pub const PATH_REQUESTS: u8 = 18;
    pub const HOP_RETRIES: u8 = 19;
    pub const FLOODS_CANCELLED: u8 = 20;
    // Radio
    pub const RADIO_RX_OK: u8 = 30;
    pub const RADIO_RX_ERR: u8 = 31;
    pub const RADIO_TX_OK: u8 = 32;
    pub const RADIO_TX_ERR: u8 = 33;
    pub const LAST_RSSI_DBM: u8 = 34;
    pub const LAST_SNR_DB: u8 = 35;
    pub const AVG_RSSI_DBM: u8 = 36;
    pub const AVG_SNR_DB: u8 = 37;
    pub const CAD_BUSY: u8 = 38;
    pub const DUTY_DEFERRED: u8 = 39;
    pub const AIRTIME_MS: u8 = 40;
    pub const DUTY_USED_PERMILLE: u8 = 41;
    pub const RADIO_BYTES_RX: u8 = 42;
    pub const RADIO_BYTES_TX: u8 = 43;
    pub const TX_QUEUE: u8 = 44;
    pub const FREQ_KHZ: u8 = 45;
    pub const SF: u8 = 46;
    pub const BW_KHZ: u8 = 47;
    pub const TX_POWER_DBM: u8 = 48;
    pub const BATTERY_MV: u8 = 49;
    pub const MIN_RSSI_DBM: u8 = 50;
    pub const MAX_RSSI_DBM: u8 = 51;
    pub const RADIO_DROPPED: u8 = 52;
    /// Debugging: which step the radio loop is in (see firmware/common/src/radio.rs).
    pub const RADIO_STAGE: u8 = 53;
    /// Debugging: number of passes through the radio loop.
    pub const RADIO_LOOPS: u8 = 54;
    // Host
    pub const MAILBOX_RECIPIENTS: u8 = 60;
    pub const MAILBOX_MESSAGES: u8 = 61;
    pub const FILES_OUT: u8 = 62;
    pub const FILES_IN: u8 = 63;
    pub const OUTBOX_WAITING: u8 = 64;
    pub const LINK_BYTES_IN: u8 = 65;
    pub const LINK_BYTES_OUT: u8 = 66;
    pub const LINK_PACKETS_IN: u8 = 67;
    pub const LINK_PACKETS_OUT: u8 = 68;
    pub const DEPOSITS_MADE: u8 = 69;
    pub const MAIL_DELIVERED: u8 = 70;
    pub const ERRORS: u8 = 71;
    // Backbone-DHT
    pub const DHT_CONTACTS: u8 = 72;
    pub const DHT_RECORDS: u8 = 73;
    pub const DHT_LOOKUPS: u8 = 74;
    pub const DHT_FOUND: u8 = 75;
    pub const DHT_PEERS: u8 = 76;
    /// Announces and path requests dropped because of a missing/wrong stamp.
    pub const BAD_STAMPS: u8 = 77;
    // Diagnostics and power
    /// Estimated noise floor (dBm) from the RSSI and SNR of received packets.
    pub const NOISE_FLOOR_DBM: u8 = 80;
    /// Energy status (0 = critical, 1 = low, 2 = ok, 3 = full).
    pub const ENERGY: u8 = 81;
    /// External power (USB): 0 = no, 1 = yes, -1 = unknown.
    pub const EXT_POWER: u8 = 82;
    /// Number of direct neighbours.
    pub const NEIGHBORS: u8 = 83;
    /// Shared position precision (0 = off ... 3 = precise).
    pub const POSITION_PRECISION: u8 = 84;
    /// Battery in percent (approximate, from the voltage).
    pub const BATTERY_PERCENT: u8 = 85;
    /// Chip-temperatur i 0,1 °C (lidt over omgivelserne).
    pub const TEMPERATURE_DC: u8 = 86;
    /// Main stack never used since boot (bytes); firmware diagnostics.
    pub const STACK_FREE: u8 = 87;
    /// Free heap right now (bytes).
    pub const HEAP_FREE: u8 = 88;
    /// Smallest free heap since boot (bytes).
    pub const HEAP_MIN_FREE: u8 = 89;
    /// Crashes in a row (firmware restarts from a panic), see Wi-Fi guard.
    pub const CRASHES: u8 = 90;
    /// Line of the last panic (firmware diagnostics, 0 = none).
    pub const PANIC_LINE: u8 = 91;
    /// Hash of the last panic's file name (FNV-1a, 32 bit).
    pub const PANIC_FILE: u8 = 92;
    // Sent by the firmware in a second `Stats` frame (the first is full).
    /// Hello packets sent.
    pub const HELLOS: u8 = 93;
    /// Key requests sent ("who is X?").
    pub const KEY_REQUESTS: u8 = 94;
    /// Packets routed around a repeater that stopped answering.
    pub const LOCAL_REPAIRS: u8 = 95;
    /// Messages carried for others (store-carry-forward).
    pub const CARRIED: u8 = 96;
    /// Pinned contacts (the ones kept in flash).
    pub const PINNED: u8 = 97;
    /// 1 when the node says it is moving (mobile mode).
    pub const MOBILE: u8 = 98;
    /// Packets dropped by the load quota.
    pub const QUOTA_DROPPED: u8 = 99;
    /// Messages waiting for a recipient's key.
    pub const AWAIT_KEY: u8 = 100;
    /// Messages kept for a later post office deposit.
    pub const OUTBOX: u8 = 101;
    /// 1 when a Wi-Fi modem works as an ordinary node because its link is down.
    pub const MODEM_FALLBACK: u8 = 102;
}

/// Error codes in `Event::Error`.
pub mod error_code {
    pub const UNKNOWN_DEST: u8 = 1;
    pub const TOO_LARGE: u8 = 2;
    pub const QUEUE_FULL: u8 = 3;
    pub const NOT_IN_CHANNEL: u8 = 4;
    pub const BAD_FRAME: u8 = 5;
    pub const UNKNOWN_SENDER: u8 = 6;
    pub const FILE_TOO_LARGE: u8 = 7;
    pub const NO_FILE: u8 = 8;
    /// Calls need TCP all the way (see `SendCall`).
    pub const NOT_OVER_TCP: u8 = 9;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameError {
    TooShort,
    UnknownType(u8),
    TooLong,
}

struct W(FrameBuf);

impl W {
    fn new(kind: u8) -> Self {
        let mut v = FrameBuf::new();
        let _ = v.extend_from_slice(&[0, 0, kind]);
        W(v)
    }
    fn b(mut self, b: &[u8]) -> Self {
        let room = self.0.capacity() - self.0.len();
        let _ = self.0.extend_from_slice(&b[..b.len().min(room)]);
        self
    }
    fn done(mut self) -> FrameBuf {
        let len = (self.0.len() - 2) as u16;
        self.0[..2].copy_from_slice(&len.to_le_bytes());
        self.0
    }
}

struct R<'a>(&'a [u8]);

impl<'a> R<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], FrameError> {
        if self.0.len() < n {
            return Err(FrameError::TooShort);
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn u8(&mut self) -> Result<u8, FrameError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, FrameError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Result<u32, FrameError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn u64(&mut self) -> Result<u64, FrameError> {
        let b = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        Ok(u64::from_le_bytes(a))
    }
    fn addr(&mut self) -> Result<Address, FrameError> {
        Address::from_slice(self.take(16)?).ok_or(FrameError::TooShort)
    }
    /// A string with a one-byte length in front, at most `N` bytes.
    fn short<const N: usize>(&mut self) -> Result<heapless::Vec<u8, N>, FrameError> {
        let n = self.u8()? as usize;
        heapless::Vec::from_slice(self.take(n)?).map_err(|_| FrameError::TooLong)
    }

    fn text(&mut self) -> Result<Text, FrameError> {
        let t = Text::from_slice(self.0).map_err(|_| FrameError::TooLong)?;
        self.0 = &[];
        Ok(t)
    }
    /// Optional public identity (64 bytes) at the end of the frame.
    fn identity(&mut self) -> Option<[u8; 64]> {
        let mut k = [0u8; 64];
        k.copy_from_slice(self.take(64).ok()?);
        Some(k)
    }
    /// Up to 4 addresses, the rest of the frame.
    fn addrs(&mut self) -> Result<heapless::Vec<Address, 4>, FrameError> {
        let mut v = heapless::Vec::new();
        while self.0.len() >= 16 {
            v.push(self.addr()?).map_err(|_| FrameError::TooLong)?;
        }
        Ok(v)
    }
    /// MIME length (u8) + MIME + name (the rest).
    fn mime_name(&mut self) -> Result<(Text, Text), FrameError> {
        let n = self.u8()? as usize;
        let mime = Text::from_slice(self.take(n)?).map_err(|_| FrameError::TooLong)?;
        Ok((mime, self.text()?))
    }
}

/// Text truncated to `MAX_TEXT` bytes.
pub fn text(s: &[u8]) -> Text {
    Text::from_slice(&s[..s.len().min(MAX_TEXT)]).unwrap_or_default()
}

impl Command {
    pub fn encode(&self) -> FrameBuf {
        match self {
            Command::GetInfo => W::new(0x01).done(),
            Command::SendDirect { req, dest, text } => {
                W::new(0x02).b(&req.to_le_bytes()).b(&dest.0).b(text).done()
            }
            Command::SendChannel { req, channel, text } => W::new(0x03)
                .b(&req.to_le_bytes())
                .b(&channel.0)
                .b(text)
                .done(),
            Command::JoinChannel { name } => W::new(0x04).b(name).done(),
            Command::AnnounceNow => W::new(0x05).done(),
            Command::FileBegin {
                req,
                dest,
                size,
                mime,
                name,
            } => W::new(0x06)
                .b(&req.to_le_bytes())
                .b(&dest.0)
                .b(&size.to_le_bytes())
                .b(&[mime.len() as u8])
                .b(mime)
                .b(name)
                .done(),
            Command::FileData { req, offset, data } => W::new(0x07)
                .b(&req.to_le_bytes())
                .b(&offset.to_le_bytes())
                .b(data)
                .done(),
            Command::FileEnd { req } => W::new(0x08).b(&req.to_le_bytes()).done(),
            Command::GetStats => W::new(0x09).done(),
            Command::SetRadio(c) => W::new(0x0a).b(&c.to_bytes()).done(),
            Command::GetRadio => W::new(0x0b).done(),
            Command::Remote { req, dest, frame } => W::new(0x0c).b(&req.to_le_bytes()).b(&dest.0).b(frame).done(),
            Command::SetAdmins { admins } => {
                let mut w = W::new(0x0d);
                for a in admins {
                    w = w.b(&a.0);
                }
                w.done()
            }
            Command::GetAdmins => W::new(0x0e).done(),
            Command::SetTime { unix } => W::new(0x0f).b(&unix.to_le_bytes()).done(),
            Command::SetRole { role } => W::new(0x10).b(&[*role]).done(),
            Command::SetRegions { strict, names } => {
                W::new(0x11).b(&[*strict as u8]).b(names).done()
            }
            Command::GetRegions => W::new(0x12).done(),
            Command::JoinPrivateChannel { key, name } => W::new(0x13).b(key).b(name).done(),
            Command::TraceRoute { req, dest } => W::new(0x20).b(&req.to_le_bytes()).b(&dest.0).done(),
            Command::GetNeighbors => W::new(0x21).done(),
            Command::SetSharing(s) => W::new(0x22).b(&s.to_bytes()).done(),
            Command::GetSharing => W::new(0x23).done(),
            Command::SetName { name } => W::new(0x24).b(name).done(),
            Command::SetWifi(w) => W::new(0x28)
                .b(&[w.enabled as u8 | (w.modem as u8) << 1, w.ssid.len() as u8])
                .b(&w.ssid)
                .b(&[w.password.len() as u8])
                .b(&w.password)
                .b(&w.peer)
                .done(),
            Command::GetWifi => W::new(0x29).done(),
            Command::SetModemKey { passphrase } => W::new(0x2d).b(passphrase).done(),
            Command::GetName => W::new(0x25).done(),
            Command::SetSite(site) => W::new(0x3a).b(&site.to_bytes()).done(),
            Command::GetSite => W::new(0x3b).done(),
            Command::RequestPage { req, dest, path } => W::new(0x26).b(&req.to_le_bytes()).b(&dest.0).b(path).done(),
            Command::PostBoard { req, dest, path, text } => W::new(0x2e).b(&req.to_le_bytes()).b(&dest.0).b(&[path.len() as u8]).b(path).b(text).done(),
            Command::SendCall { dest, data } => W::new(0x27).b(&dest.0).b(data).done(),
            Command::OtaBegin { req, flags, manifest } => {
                W::new(0x18).b(&req.to_le_bytes()).b(&[*flags]).b(manifest).done()
            }
            Command::OtaChunk { req, offset, data } => {
                W::new(0x19).b(&req.to_le_bytes()).b(&offset.to_le_bytes()).b(data).done()
            }
            Command::OtaEnd { req, apply } => W::new(0x1a).b(&req.to_le_bytes()).b(&[*apply as u8]).done(),
            Command::OtaStatus => W::new(0x1b).done(),
            Command::GetFileLimit { dest } => W::new(0x14).b(&dest.0).done(),
            Command::SetIdentity { confirm, secret } => {
                let flags = if *confirm { IDENTITY_CONFIRM } else { 0 };
                W::new(0x34).b(&[flags]).b(secret).done()
            }
            Command::SetRangeTest(c) => W::new(0x2a).b(&c.to_bytes()).done(),
            Command::GetRangeTest => W::new(0x2b).done(),
            Command::SendAlert { req, meta, text } => meta.put(W::new(0x2c).b(&req.to_le_bytes())).b(text).done(),
            Command::LinkDevice { req, device } => W::new(0x35).b(&req.to_le_bytes()).b(&device.0).done(),
            Command::GetPerson => W::new(0x36).done(),
            Command::PersonSetup { req, op, text } => W::new(0x37).b(&req.to_le_bytes()).b(&[*op]).b(text).done(),
            Command::CancelFile { file_id } => W::new(0x38).b(&file_id.to_le_bytes()).done(),
            Command::Sensor { req, dest, op, data } => W::new(0x39).b(&req.to_le_bytes()).b(&dest.0).b(&[*op]).b(data).done(),
        }
    }

    /// Decodes type + content (without the length field).
    pub fn decode(frame: &[u8]) -> Result<Self, FrameError> {
        let mut r = R(frame);
        Ok(match r.u8()? {
            0x01 => Command::GetInfo,
            0x02 => Command::SendDirect {
                req: r.u16()?,
                dest: r.addr()?,
                text: r.text()?,
            },
            0x03 => Command::SendChannel {
                req: r.u16()?,
                channel: r.addr()?,
                text: r.text()?,
            },
            0x04 => Command::JoinChannel { name: r.text()? },
            0x05 => Command::AnnounceNow,
            0x06 => {
                let (req, dest, size) = (r.u16()?, r.addr()?, r.u32()?);
                let (mime, name) = r.mime_name()?;
                Command::FileBegin {
                    req,
                    dest,
                    size,
                    mime,
                    name,
                }
            }
            0x07 => Command::FileData {
                req: r.u16()?,
                offset: r.u32()?,
                data: r.text()?,
            },
            0x08 => Command::FileEnd { req: r.u16()? },
            0x09 => Command::GetStats,
            0x0a => Command::SetRadio(
                RadioConfig::from_bytes(r.take(RadioConfig::LEN)?).ok_or(FrameError::TooShort)?,
            ),
            0x0b => Command::GetRadio,
            0x0c => Command::Remote { req: r.u16()?, dest: r.addr()?, frame: r.text()? },
            0x0d => Command::SetAdmins { admins: r.addrs()? },
            0x0e => Command::GetAdmins,
            0x0f => Command::SetTime { unix: r.u32()? },
            0x10 => Command::SetRole { role: r.u8()? },
            0x11 => Command::SetRegions { strict: r.u8()? != 0, names: r.text()? },
            0x12 => Command::GetRegions,
            0x13 => {
                let mut key = [0u8; 32];
                key.copy_from_slice(r.take(32)?);
                Command::JoinPrivateChannel { key, name: r.text()? }
            }
            0x20 => Command::TraceRoute { req: r.u16()?, dest: r.addr()? },
            0x21 => Command::GetNeighbors,
            0x22 => Command::SetSharing(Sharing::from_bytes(r.take(Sharing::LEN)?).ok_or(FrameError::TooShort)?),
            0x23 => Command::GetSharing,
            0x24 => Command::SetName { name: r.text()? },
            0x28 => {
                let flags = r.u8()?;
                let ssid = r.short()?;
                let password = r.short()?;
                let peer = heapless::Vec::from_slice(r.take(r.0.len())?).map_err(|_| FrameError::TooLong)?;
                Command::SetWifi(WifiSettings { enabled: flags & 1 != 0, modem: flags & 2 != 0, ssid, password, peer, modem_key: None })
            }
            0x29 => Command::GetWifi,
            0x2d => Command::SetModemKey { passphrase: r.text()? },
            0x25 => Command::GetName,
            0x3a => Command::SetSite(crate::appdata::Site::from_bytes(r.text()?.as_slice()).ok_or(FrameError::TooShort)?),
            0x3b => Command::GetSite,
            0x26 => Command::RequestPage { req: r.u16()?, dest: r.addr()?, path: r.text()? },
            0x2e => {
                let (req, dest) = (r.u16()?, r.addr()?);
                let n = r.u8()? as usize;
                let path = Text::from_slice(r.take(n)?).map_err(|_| FrameError::TooShort)?;
                Command::PostBoard { req, dest, path, text: r.text()? }
            }
            0x27 => Command::SendCall { dest: r.addr()?, data: r.text()? },
            0x18 => Command::OtaBegin { req: r.u16()?, flags: r.u8()?, manifest: r.text()? },
            0x19 => Command::OtaChunk { req: r.u16()?, offset: r.u32()?, data: r.text()? },
            0x1a => Command::OtaEnd { req: r.u16()?, apply: r.u8()? != 0 },
            0x1b => Command::OtaStatus,
            0x14 => Command::GetFileLimit { dest: r.addr()? },
            0x34 => {
                let confirm = r.u8()? & IDENTITY_CONFIRM != 0;
                let mut secret = [0u8; 64];
                secret.copy_from_slice(r.take(64)?);
                Command::SetIdentity { confirm, secret }
            }
            0x2a => Command::SetRangeTest(
                crate::rangetest::Config::from_bytes(r.take(crate::rangetest::Config::LEN)?).ok_or(FrameError::TooShort)?,
            ),
            0x2b => Command::GetRangeTest,
            0x2c => Command::SendAlert { req: r.u16()?, meta: AlertMeta::take(&mut r)?, text: r.text()? },
            0x35 => Command::LinkDevice { req: r.u16()?, device: r.addr()? },
            0x36 => Command::GetPerson,
            0x37 => Command::PersonSetup { req: r.u16()?, op: r.u8()?, text: r.text()? },
            0x38 => Command::CancelFile { file_id: r.u64()? },
            0x39 => Command::Sensor { req: r.u16()?, dest: r.addr()?, op: r.u8()?, data: r.text()? },
            t => return Err(FrameError::UnknownType(t)),
        })
    }
}

impl Event {
    pub fn encode(&self) -> FrameBuf {
        match self {
            Event::Info {
                addr,
                repeater,
                version,
                max_text,
                max_file,
                identity,
            } => W::new(0x81)
                .b(&addr.0)
                .b(&[*repeater as u8, *version, *max_text])
                .b(&max_file.to_le_bytes())
                .b(identity.as_ref().map_or(&[][..], |k| &k[..]))
                .done(),
            Event::Sent { req, msg_id } => W::new(0x82)
                .b(&req.to_le_bytes())
                .b(&msg_id.to_le_bytes())
                .done(),
            Event::DirectReceived { from, hops, text } => {
                W::new(0x83).b(&from.0).b(&[*hops]).b(text).done()
            }
            Event::ChannelReceived {
                channel,
                from,
                hops,
                text,
            } => W::new(0x84)
                .b(&channel.0)
                .b(&from.0)
                .b(&[*hops])
                .b(text)
                .done(),
            Event::Acked { msg_id } => W::new(0x85).b(&msg_id.to_le_bytes()).done(),
            Event::RouteLearned { dest, hops } => W::new(0x86).b(&dest.0).b(&[*hops]).done(),
            Event::Heard { addr, hops, identity } => W::new(0x87)
                .b(&addr.0)
                .b(&[*hops])
                .b(identity.as_ref().map_or(&[][..], |k| &k[..]))
                .done(),
            Event::ChannelJoined { channel, name } => W::new(0x88).b(&channel.0).b(name).done(),
            Event::FileIncoming {
                file_id,
                from,
                size,
                mime,
                name,
            } => W::new(0x89)
                .b(&file_id.to_le_bytes())
                .b(&from.0)
                .b(&size.to_le_bytes())
                .b(&[mime.len() as u8])
                .b(mime)
                .b(name)
                .done(),
            Event::FileChunk {
                file_id,
                offset,
                data,
            } => W::new(0x8a)
                .b(&file_id.to_le_bytes())
                .b(&offset.to_le_bytes())
                .b(data)
                .done(),
            Event::FileProgress {
                file_id,
                done,
                total,
                outgoing,
            } => W::new(0x8b)
                .b(&file_id.to_le_bytes())
                .b(&done.to_le_bytes())
                .b(&total.to_le_bytes())
                .b(&[*outgoing as u8])
                .done(),
            Event::FileDone {
                file_id,
                ok,
                outgoing,
                text,
            } => W::new(0x8c)
                .b(&file_id.to_le_bytes())
                .b(&[*ok as u8, *outgoing as u8])
                .b(text)
                .done(),
            Event::FileStarted {
                req,
                file_id,
                chunks,
            } => W::new(0x8d)
                .b(&req.to_le_bytes())
                .b(&file_id.to_le_bytes())
                .b(&chunks.to_le_bytes())
                .done(),
            Event::Log { text } => W::new(0x8e).b(text).done(),
            Event::Deposited { msg_id } => W::new(0x90).b(&msg_id.to_le_bytes()).done(),
            Event::Radio(c) => W::new(0x92).b(&c.to_bytes()).done(),
            Event::RemoteReply { from, frame } => W::new(0x93).b(&from.0).b(frame).done(),
            Event::Admins { admins } => {
                let mut w = W::new(0x94);
                for a in admins {
                    w = w.b(&a.0);
                }
                w.done()
            }
            Event::Regions { strict, names } => {
                W::new(0x95).b(&[*strict as u8]).b(names).done()
            }
            Event::OtaState { req, state, next, size, version, current, text } => W::new(0x9a)
                .b(&req.to_le_bytes())
                .b(&[*state])
                .b(&next.to_le_bytes())
                .b(&size.to_le_bytes())
                .b(&version.to_le_bytes())
                .b(&current.to_le_bytes())
                .b(text)
                .done(),
            Event::Stats { items } => {
                let mut w = W::new(0x91);
                for (k, v) in items {
                    w = w.b(&[*k]).b(&v.to_le_bytes());
                }
                w.done()
            }
            Event::FileLimit { dest, max_bytes, via_lora, mailbox, est_airtime_s } => W::new(0x96)
                .b(&dest.0)
                .b(&max_bytes.to_le_bytes())
                .b(&[*via_lora as u8 | (*mailbox as u8) << 1])
                .b(&est_airtime_s.to_le_bytes())
                .done(),
            Event::Error { req, code, text } => W::new(0x8f)
                .b(&req.to_le_bytes())
                .b(&[*code])
                .b(text)
                .done(),
            Event::TraceResult { req, dest, path, out, back } => {
                let mut w = W::new(0xa0).b(&req.to_le_bytes()).b(&dest.0).b(&[path.len() as u8]);
                for id in path {
                    w = w.b(&id.to_le_bytes());
                }
                w = w.b(&[out.len() as u8]);
                for v in out.iter().chain(back.iter()) {
                    w = w.b(&[*v as u8]);
                }
                w.done()
            }
            Event::Neighbors { part, last, items } => {
                let mut w = W::new(0xa1).b(&[*part, *last as u8]);
                for n in items {
                    w = w
                        .b(&n.id.to_le_bytes())
                        .b(&n.prefix)
                        .b(&[n.snr as u8])
                        .b(&n.rssi.to_le_bytes())
                        .b(&n.age_s.to_le_bytes())
                        .b(&n.count.to_le_bytes());
                }
                w.done()
            }
            Event::Sharing(s) => W::new(0xa2).b(&s.to_bytes()).done(),
            Event::Site(site) => W::new(0xbb).b(&site.to_bytes()).done(),
            Event::Name { name } => W::new(0xa4).b(name).done(),
            Event::Call { from, data } => W::new(0xa5).b(&from.0).b(data).done(),
            Event::CarryAcked { msg_id } => W::new(0xa7).b(&msg_id.to_le_bytes()).done(),
            Event::Wifi { enabled, modem, state, ip, ssid, peer } => W::new(0xa6)
                .b(&[*enabled as u8 | (*modem as u8) << 1, *state])
                .b(ip)
                .b(&[ssid.len() as u8])
                .b(ssid)
                .b(peer)
                .done(),
            Event::NodeInfo { addr, hops, position, telemetry, name, caps } => {
                let mut data = crate::appdata::build(None, false, *position, telemetry.as_ref(), name.as_deref());
                if let Some(c) = caps {
                    crate::appdata::push_caps(&mut data, *c);
                }
                W::new(0xa3).b(&addr.0).b(&[*hops]).b(&data).done()
            }
            Event::IdentityChanged { addr, identity } => W::new(0xb4).b(&addr.0).b(identity).done(),
            Event::RangeState(st) => {
                let mut w = W::new(0xa8)
                    .b(&st.config.to_bytes())
                    .b(&[st.sf, st.synced as u8])
                    .b(&st.count.to_le_bytes())
                    .b(&st.missed.to_le_bytes())
                    .b(&st.seq.to_le_bytes());
                for p in &st.per_sf {
                    w = w
                        .b(&[p.sf])
                        .b(&p.rx.to_le_bytes())
                        .b(&p.lost.to_le_bytes())
                        .b(&p.rssi_avg.to_le_bytes())
                        .b(&p.rssi_min.to_le_bytes())
                        .b(&p.snr_avg_x4.to_le_bytes())
                        .b(&[p.snr_min as u8]);
                }
                w.done()
            }
            Event::RangeRx(x) => {
                let w = W::new(0xa9)
                    .b(&x.uptime_ms.to_le_bytes())
                    .b(&x.sender)
                    .b(&x.seq.to_le_bytes())
                    .b(&[x.sf])
                    .b(&x.rssi.to_le_bytes())
                    .b(&[x.snr as u8, x.len])
                    .b(&x.lost.to_le_bytes())
                    .b(&x.skipped.to_le_bytes())
                    .b(&[x.tx_power_dbm as u8])
                    .b(&x.battery_mv.to_le_bytes());
                match x.position {
                    Some((lat, lon)) => w.b(&lat.to_le_bytes()).b(&lon.to_le_bytes()).done(),
                    None => w.done(),
                }
            }
            Event::Alert { issuer, serial, issued, meta, hops, trust, name, text } => meta
                .put(W::new(0xac).b(&issuer.0).b(&serial.to_le_bytes()).b(&issued.to_le_bytes()))
                .b(&[*hops, *trust, name.len() as u8])
                .b(name)
                .b(text)
                .done(),
            Event::RangeTx(x) => W::new(0xaa)
                .b(&x.uptime_ms.to_le_bytes())
                .b(&x.seq.to_le_bytes())
                .b(&[x.sf, x.slot])
                .b(&x.interval_s.to_le_bytes())
                .b(&x.airtime_ms.to_le_bytes())
                .b(&x.charge_ma_ms.to_le_bytes())
                .b(&x.skipped.to_le_bytes())
                .done(),
            Event::DeviceLinked { device, index, words } => W::new(0xb5).b(&device.0).b(&[*index]).b(words).done(),
            Event::Person(p) => {
                let mut w = W::new(0xb6)
                    .b(&[p.state, p.master as u8])
                    .b(&p.person.0)
                    .b(&[p.index])
                    .b(&p.version.to_le_bytes())
                    .b(&[p.devices.len() as u8]);
                for (d, revoked) in &p.devices {
                    w = w.b(&d.0).b(&[*revoked as u8]);
                }
                w.b(&p.words).done()
            }
            Event::PersonPhrase { phrase } => W::new(0xb7).b(phrase).done(),
            Event::Sensor { node, op, data } => W::new(0xbd).b(&node.0).b(&[*op]).b(data).done(),
            Event::PersonBackup { code } => W::new(0xb8).b(code).done(),
            Event::MeshCoreProof { req, offset, total, data } => W::new(0xbc)
                .b(&req.to_le_bytes()).b(&offset.to_le_bytes()).b(&total.to_le_bytes()).b(data).done(),
            Event::Power(p) => {
                let mut w = W::new(0xab)
                    .b(&p.uptime_s.to_le_bytes())
                    .b(&p.battery_mv.to_le_bytes())
                    .b(&[p.ext_power as u8, p.mode, p.tx_power_dbm as u8])
                    .b(&p.base_ma_x10.to_le_bytes())
                    .b(&p.rx_ma_x10.to_le_bytes())
                    .b(&p.tx_ma_x10.to_le_bytes())
                    .b(&p.tx_packets.to_le_bytes());
                for ph in &p.phases {
                    w = w.b(&ph.secs.to_le_bytes()).b(&ph.tx_ms.to_le_bytes()).b(&ph.charge_mas.to_le_bytes());
                }
                w.done()
            }
        }
    }

    pub fn decode(frame: &[u8]) -> Result<Self, FrameError> {
        let mut r = R(frame);
        Ok(match r.u8()? {
            0x81 => Event::Info {
                addr: r.addr()?,
                repeater: r.u8()? != 0,
                version: r.u8()?,
                max_text: r.u8()?,
                max_file: r.u32()?,
                identity: r.identity(),
            },
            0x82 => Event::Sent {
                req: r.u16()?,
                msg_id: r.u64()?,
            },
            0x83 => Event::DirectReceived {
                from: r.addr()?,
                hops: r.u8()?,
                text: r.text()?,
            },
            0x84 => Event::ChannelReceived {
                channel: r.addr()?,
                from: r.addr()?,
                hops: r.u8()?,
                text: r.text()?,
            },
            0x85 => Event::Acked { msg_id: r.u64()? },
            0x86 => Event::RouteLearned {
                dest: r.addr()?,
                hops: r.u8()?,
            },
            0x87 => Event::Heard {
                addr: r.addr()?,
                hops: r.u8()?,
                identity: r.identity(),
            },
            0x88 => Event::ChannelJoined {
                channel: r.addr()?,
                name: r.text()?,
            },
            0x89 => {
                let (file_id, from, size) = (r.u64()?, r.addr()?, r.u32()?);
                let (mime, name) = r.mime_name()?;
                Event::FileIncoming {
                    file_id,
                    from,
                    size,
                    mime,
                    name,
                }
            }
            0x8a => Event::FileChunk {
                file_id: r.u64()?,
                offset: r.u32()?,
                data: r.text()?,
            },
            0x8b => Event::FileProgress {
                file_id: r.u64()?,
                done: r.u16()?,
                total: r.u16()?,
                outgoing: r.u8()? != 0,
            },
            0x8c => Event::FileDone {
                file_id: r.u64()?,
                ok: r.u8()? != 0,
                outgoing: r.u8()? != 0,
                text: r.text()?,
            },
            0x8d => Event::FileStarted {
                req: r.u16()?,
                file_id: r.u64()?,
                chunks: r.u16()?,
            },
            0x8e => Event::Log { text: r.text()? },
            0x90 => Event::Deposited { msg_id: r.u64()? },
            0x93 => Event::RemoteReply { from: r.addr()?, frame: r.text()? },
            0x94 => Event::Admins { admins: r.addrs()? },
            0x95 => Event::Regions { strict: r.u8()? != 0, names: r.text()? },
            0x9a => Event::OtaState {
                req: r.u16()?,
                state: r.u8()?,
                next: r.u32()?,
                size: r.u32()?,
                version: r.u32()?,
                current: r.u32()?,
                text: r.text()?,
            },
            0x96 => {
                let dest = r.addr()?;
                let max_bytes = r.u32()?;
                let flags = r.u8()?;
                Event::FileLimit {
                    dest,
                    max_bytes,
                    via_lora: flags & 1 != 0,
                    mailbox: flags & 2 != 0,
                    est_airtime_s: r.u32()?,
                }
            }
            0x92 => Event::Radio(
                RadioConfig::from_bytes(r.take(RadioConfig::LEN)?).ok_or(FrameError::TooShort)?,
            ),
            0x91 => {
                let mut items = Stats::new();
                while !r.0.is_empty() {
                    let k = r.u8()?;
                    let v = r.u32()? as i32;
                    items.push((k, v)).map_err(|_| FrameError::TooLong)?;
                }
                Event::Stats { items }
            }
            0x8f => Event::Error {
                req: r.u16()?,
                code: r.u8()?,
                text: r.text()?,
            },
            0xa0 => {
                let (req, dest) = (r.u16()?, r.addr()?);
                let k = r.u8()? as usize;
                let mut path = heapless::Vec::new();
                for _ in 0..k {
                    path.push(r.u16()?).map_err(|_| FrameError::TooLong)?;
                }
                let m = r.u8()? as usize;
                let snr = |b: &[u8]| -> Result<heapless::Vec<i8, 13>, FrameError> {
                    if b.len() > 13 {
                        return Err(FrameError::TooLong);
                    }
                    Ok(b.iter().map(|&x| x as i8).collect())
                };
                let out = snr(r.take(m)?)?;
                let back = snr(r.0)?;
                Event::TraceResult { req, dest, path, out, back }
            }
            0xa1 => {
                let (part, last) = (r.u8()?, r.u8()? != 0);
                let mut items = heapless::Vec::new();
                while !r.0.is_empty() {
                    let id = r.u16()?;
                    let mut prefix = [0u8; 4];
                    prefix.copy_from_slice(r.take(4)?);
                    let snr = r.u8()? as i8;
                    let rssi = r.u16()? as i16;
                    let (age_s, count) = (r.u32()?, r.u16()?);
                    items
                        .push(NeighborInfo { id, prefix, snr, rssi, age_s, count })
                        .map_err(|_| FrameError::TooLong)?;
                }
                Event::Neighbors { part, last, items }
            }
            0xbb => Event::Site(crate::appdata::Site::from_bytes(r.text()?.as_slice()).ok_or(FrameError::TooShort)?),
            0xa2 => Event::Sharing(Sharing::from_bytes(r.take(Sharing::LEN)?).ok_or(FrameError::TooShort)?),
            0xa3 => {
                let (addr, hops) = (r.addr()?, r.u8()?);
                let a = crate::appdata::parse(r.0);
                Event::NodeInfo { addr, hops, position: a.position, telemetry: a.telemetry, name: a.name, caps: a.caps }
            }
            0xa4 => Event::Name { name: r.text()? },
            0xa5 => Event::Call { from: r.addr()?, data: r.text()? },
            0xa7 => Event::CarryAcked { msg_id: r.u64()? },
            0xa6 => {
                let flags = r.u8()?;
                let (enabled, modem) = (flags & 1 != 0, flags & 2 != 0);
                let state = r.u8()?;
                let mut ip = [0u8; 4];
                ip.copy_from_slice(r.take(4)?);
                let ssid = r.short()?;
                let peer = heapless::Vec::from_slice(r.take(r.0.len())?).map_err(|_| FrameError::TooLong)?;
                Event::Wifi { enabled, modem, state, ip, ssid, peer }
            }
            0xb5 => Event::DeviceLinked {
                device: r.addr()?,
                index: r.u8()?,
                words: heapless::Vec::from_slice(r.take(r.0.len())?).map_err(|_| FrameError::TooLong)?,
            },
            0xb6 => {
                let (state, master) = (r.u8()?, r.u8()? & 1 != 0);
                let (person, index, version) = (r.addr()?, r.u8()?, r.u32()?);
                let n = r.u8()? as usize;
                let mut devices = heapless::Vec::new();
                for _ in 0..n {
                    let d = (r.addr()?, r.u8()? != 0);
                    devices.push(d).map_err(|_| FrameError::TooLong)?;
                }
                let words = heapless::Vec::from_slice(r.take(r.0.len())?).map_err(|_| FrameError::TooLong)?;
                Event::Person(PersonInfo { state, master, person, index, version, devices, words })
            }
            0xb7 => Event::PersonPhrase { phrase: r.text()? },
            0xbd => Event::Sensor { node: r.addr()?, op: r.u8()?, data: r.text()? },
            0xb8 => Event::PersonBackup { code: r.text()? },
            0xbc => Event::MeshCoreProof { req: r.u16()?, offset: r.u16()?, total: r.u16()?, data: r.text()? },
            0xb4 => {
                let addr = r.addr()?;
                let identity = r.identity().ok_or(FrameError::TooShort)?;
                Event::IdentityChanged { addr, identity }
            }
            0xa8 => {
                let config = crate::rangetest::Config::from_bytes(r.take(crate::rangetest::Config::LEN)?)
                    .ok_or(FrameError::TooShort)?;
                let (sf, synced) = (r.u8()?, r.u8()? & 1 != 0);
                let (count, missed, seq) = (r.u32()?, r.u32()?, r.u32()?);
                let mut per_sf = heapless::Vec::new();
                while !r.0.is_empty() {
                    let p = RangeSfStat {
                        sf: r.u8()?,
                        rx: r.u16()?,
                        lost: r.u16()?,
                        rssi_avg: r.u16()? as i16,
                        rssi_min: r.u16()? as i16,
                        snr_avg_x4: r.u16()? as i16,
                        snr_min: r.u8()? as i8,
                    };
                    per_sf.push(p).map_err(|_| FrameError::TooLong)?;
                }
                Event::RangeState(RangeState { config, sf, synced, count, missed, seq, per_sf })
            }
            0xa9 => {
                let uptime_ms = r.u32()?;
                let mut sender = [0u8; 4];
                sender.copy_from_slice(r.take(4)?);
                let (seq, sf, rssi) = (r.u32()?, r.u8()?, r.u16()? as i16);
                let (snr, len) = (r.u8()? as i8, r.u8()?);
                let (lost, skipped, tx_power_dbm, battery_mv) = (r.u16()?, r.u16()?, r.u8()? as i8, r.u16()?);
                let position = if r.0.len() >= 8 { Some((r.u32()? as i32, r.u32()? as i32)) } else { None };
                Event::RangeRx(RangeRx { uptime_ms, sender, seq, sf, rssi, snr, len, lost, skipped, tx_power_dbm, battery_mv, position })
            }
            0xac => {
                let (issuer, serial, issued) = (r.addr()?, r.u32()?, r.u32()?);
                let meta = AlertMeta::take(&mut r)?;
                let (hops, trust) = (r.u8()?, r.u8()?);
                Event::Alert { issuer, serial, issued, meta, hops, trust, name: r.short()?, text: r.text()? }
            }
            0xaa => Event::RangeTx(RangeTx {
                uptime_ms: r.u32()?,
                seq: r.u32()?,
                sf: r.u8()?,
                slot: r.u8()?,
                interval_s: r.u16()?,
                airtime_ms: r.u16()?,
                charge_ma_ms: r.u32()?,
                skipped: r.u16()?,
            }),
            0xab => {
                let (uptime_s, battery_mv) = (r.u32()?, r.u16()?);
                let (ext_power, mode, tx_power_dbm) = (r.u8()? as i8, r.u8()?, r.u8()? as i8);
                let (base_ma_x10, rx_ma_x10, tx_ma_x10) = (r.u16()?, r.u16()?, r.u16()?);
                let tx_packets = r.u32()?;
                let mut phases = [PowerPhase::default(); 3];
                for ph in phases.iter_mut() {
                    *ph = PowerPhase { secs: r.u32()?, tx_ms: r.u32()?, charge_mas: r.u32()? };
                }
                Event::Power(PowerReport {
                    uptime_s,
                    battery_mv,
                    ext_power,
                    mode,
                    tx_power_dbm,
                    base_ma_x10,
                    rx_ma_x10,
                    tx_ma_x10,
                    tx_packets,
                    phases,
                })
            }
            t => return Err(FrameError::UnknownType(t)),
        })
    }
}

/// Collects frames from a byte stream (serial, BLE) that may arrive in pieces.
pub struct FrameDecoder {
    buf: heapless::Vec<u8, { 2 * (MAX_FRAME + 2) }>,
}

impl Default for FrameDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameDecoder {
    pub const fn new() -> Self {
        FrameDecoder {
            buf: heapless::Vec::new(),
        }
    }

    /// Adds bytes. Then call `next_frame` until it returns `None`.
    pub fn push(&mut self, bytes: &[u8]) {
        for &b in bytes {
            if self.buf.push(b).is_err() {
                // The buffer is full of garbage: start over.
                self.buf.clear();
                let _ = self.buf.push(b);
            }
        }
    }

    /// The next complete frame (type + content), if there is one.
    pub fn next_frame(&mut self) -> Option<heapless::Vec<u8, MAX_FRAME>> {
        loop {
            if self.buf.len() < 2 {
                return None;
            }
            let len = u16::from_le_bytes([self.buf[0], self.buf[1]]) as usize;
            if len == 0 || len > MAX_FRAME {
                // Out of sync: drop one byte and try again.
                self.buf.remove(0);
                continue;
            }
            if self.buf.len() < 2 + len {
                return None;
            }
            let frame = heapless::Vec::from_slice(&self.buf[2..2 + len]).ok();
            let rest = self.buf.len() - (2 + len);
            self.buf.copy_within(2 + len.., 0);
            self.buf.truncate(rest);
            return frame;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip_cmd(c: Command) {
        let f = c.encode();
        assert_eq!(u16::from_le_bytes([f[0], f[1]]) as usize, f.len() - 2);
        assert_eq!(Command::decode(&f[2..]).unwrap(), c);
    }

    fn roundtrip_ev(e: Event) {
        let f = e.encode();
        assert_eq!(Event::decode(&f[2..]).unwrap(), e);
    }

    #[test]
    fn commands_roundtrip() {
        roundtrip_cmd(Command::GetInfo);
        roundtrip_cmd(Command::SendDirect {
            req: 7,
            dest: Address([1; 16]),
            text: text("hello æøå".as_bytes()),
        });
        roundtrip_cmd(Command::SendChannel {
            req: 8,
            channel: Address([2; 16]),
            text: text(b"x"),
        });
        roundtrip_cmd(Command::JoinChannel {
            name: text(b"#aarhus"),
        });
        roundtrip_cmd(Command::AnnounceNow);
        roundtrip_cmd(Command::FileBegin {
            req: 3,
            dest: Address([5; 16]),
            size: 8000,
            mime: text(b"image/webp"),
            name: text("picture æ.webp".as_bytes()),
        });
        roundtrip_cmd(Command::FileData {
            req: 3,
            offset: 240,
            data: text(&[1u8; 240]),
        });
        roundtrip_cmd(Command::FileEnd { req: 3 });
        roundtrip_cmd(Command::GetStats);
        roundtrip_cmd(Command::SetRadio(RadioConfig::EU_DEFAULT));
        roundtrip_cmd(Command::GetRadio);
        roundtrip_cmd(Command::SetRole { role: 2 });
        roundtrip_cmd(Command::JoinPrivateChannel { key: [9; 32], name: text("cabin æ".as_bytes()) });
        assert_eq!(&Command::JoinPrivateChannel { key: [7; 32], name: text(b"x") }.encode()[..3], &[34, 0, 0x13]);
        assert!(Command::decode(&[0x13, 1, 2]).is_err());
        assert_eq!(&Command::SetRole { role: 0 }.encode()[..], &[2, 0, 0x10, 0]);
        roundtrip_cmd(Command::Remote { req: 4, dest: Address([6; 16]), frame: text(&[0x09]) });
        let admins: heapless::Vec<Address, 4> = [Address([1; 16]), Address([2; 16])].into_iter().collect();
        roundtrip_cmd(Command::SetAdmins { admins: admins.clone() });
        roundtrip_cmd(Command::SetAdmins { admins: heapless::Vec::new() });
        roundtrip_cmd(Command::GetAdmins);
        roundtrip_cmd(Command::OtaBegin { req: 1, flags: OTA_ALLOW_DOWNGRADE, manifest: text(&[7; 112]) });
        roundtrip_cmd(Command::OtaChunk { req: 2, offset: 4096, data: text(&[1; 240]) });
        roundtrip_cmd(Command::OtaEnd { req: 3, apply: true });
        roundtrip_cmd(Command::OtaStatus);
        roundtrip_cmd(Command::SetIdentity { confirm: true, secret: [7; 64] });
        roundtrip_cmd(Command::LinkDevice { req: 4, device: Address([3; 16]) });
        roundtrip_cmd(Command::GetPerson);
        roundtrip_cmd(Command::CancelFile { file_id: 0x1122334455667788 });
        roundtrip_cmd(Command::PersonSetup { req: 5, op: person_op::RESTORE, text: text(b"abandon ability") });
        roundtrip_cmd(Command::SetIdentity { confirm: false, secret: [0; 64] });
        roundtrip_ev(Event::IdentityChanged { addr: Address([5; 16]), identity: [9; 64] });
        roundtrip_ev(Event::DeviceLinked { device: Address([6; 16]), index: 2, words: heapless::Vec::from_slice(b"a b c d").unwrap() });
        let mut devices = heapless::Vec::new();
        devices.push((Address([1; 16]), false)).unwrap();
        devices.push((Address([2; 16]), true)).unwrap();
        roundtrip_ev(Event::Person(PersonInfo {
            state: person_state::MEMBER, master: true, person: Address([8; 16]), index: 1, version: 7, devices,
            words: heapless::Vec::from_slice(b"apple brick cloud dune").unwrap(),
        }));
        roundtrip_ev(Event::Person(PersonInfo::default()));
        roundtrip_ev(Event::PersonPhrase { phrase: text(b"abandon ability able") });
        roundtrip_ev(Event::Sensor { node: Address([4; 16]), op: 2, data: text(&[1, 2, 3]) });
        roundtrip_cmd(Command::Sensor { req: 3, dest: Address([0; 16]), op: 2, data: text(&[9; 100]) });
        roundtrip_ev(Event::PersonBackup { code: text(b"one two three four five six") });
        roundtrip_ev(Event::MeshCoreProof { req: 2, offset: 240, total: 359, data: text(&[0xaa; 119]) });
        assert_eq!(Command::SetIdentity { confirm: true, secret: [1; 64] }.encode()[2], 0x34);
        assert!(Command::decode(&[0x34, 1, 2, 3]).is_err());
        roundtrip_ev(Event::OtaState {
            req: 9,
            state: ota_state::RECEIVING,
            next: 128,
            size: 300_000,
            version: 0x000200,
            current: 0x000100,
            text: text(b"ok"),
        });
        // An OTA chunk over radio: control header (5) + frame without length must
        // fit in a forward secrecy message (MAX_FS_TEXT = 143).
        let f = Command::OtaChunk { req: 2, offset: 0, data: text(&[0; 128]) }.encode();
        assert!(5 + f.len() - 2 <= 143);
        let f = Command::OtaBegin { req: 2, flags: 0, manifest: text(&[0; 112]) }.encode();
        assert!(5 + f.len() - 2 <= 143);
        roundtrip_cmd(Command::SetRegions { strict: true, names: text(b"dk,nordjylland") });
        roundtrip_cmd(Command::SetRegions { strict: false, names: text(b"") });
        roundtrip_cmd(Command::GetRegions);
        roundtrip_cmd(Command::GetFileLimit { dest: Address([9; 16]) });
        assert_eq!(&Command::GetFileLimit { dest: Address([9; 16]) }.encode()[..3], &[17, 0, 0x14]);
        roundtrip_ev(Event::FileLimit {
            dest: Address([9; 16]),
            max_bytes: 8192,
            via_lora: true,
            mailbox: false,
            est_airtime_s: 130,
        });
        roundtrip_ev(Event::FileLimit {
            dest: Address([8; 16]),
            max_bytes: 1 << 20,
            via_lora: false,
            mailbox: true,
            est_airtime_s: 0,
        });
        assert_eq!(
            Event::FileLimit { dest: Address([0; 16]), max_bytes: 1, via_lora: true, mailbox: true, est_airtime_s: 2 }
                .encode()[23],
            3
        );
        roundtrip_ev(Event::Regions { strict: false, names: text(b"dk") });

        roundtrip_cmd(Command::SetTime { unix: 1_800_000_000 });
        roundtrip_ev(Event::Admins { admins });
        roundtrip_ev(Event::RemoteReply { from: Address([3; 16]), frame: text(&[0x92, 1, 2, 3]) });
        roundtrip_ev(Event::Radio(RadioConfig {
            tx_power_dbm: -3,
            announce_quarters: 8,
            ..RadioConfig::EU_DEFAULT
        }));
        assert_eq!(RadioConfig::EU_DEFAULT.announce_interval_s(), 12 * 3600);
        assert_eq!(RadioConfig { announce_quarters: 8, ..RadioConfig::EU_DEFAULT }.announce_interval_s(), 2 * 3600);
        assert!(RadioConfig::EU_DEFAULT.validate().is_ok());
        assert!(
            RadioConfig {
                sf: 13,
                ..RadioConfig::EU_DEFAULT
            }
            .validate()
            .is_err()
        );
        assert!(
            RadioConfig {
                bw_hz: 100_000,
                ..RadioConfig::EU_DEFAULT
            }
            .validate()
            .is_err()
        );
        assert_eq!(
            RadioConfig::EU_DEFAULT.airtime(),
            crate::airtime::LoraParams {
                sf: 9,
                bw_hz: 125_000,
                cr: 1,
                preamble: 16
            }
        );
    }

    #[test]
    fn events_roundtrip() {
        let a = Address([3; 16]);
        roundtrip_ev(Event::Info {
            addr: a,
            repeater: true,
            version: 2,
            max_text: 147,
            max_file: 1 << 20,
            identity: None,
        });
        roundtrip_ev(Event::Info {
            addr: a,
            repeater: false,
            version: 2,
            max_text: 147,
            max_file: 0,
            identity: Some([8; 64]),
        });
        roundtrip_ev(Event::Sent {
            req: 1,
            msg_id: u64::MAX - 3,
        });
        roundtrip_ev(Event::DirectReceived {
            from: a,
            hops: 2,
            text: text(b"hej"),
        });
        roundtrip_ev(Event::ChannelReceived {
            channel: a,
            from: Address([4; 16]),
            hops: 1,
            text: text(b"k"),
        });
        roundtrip_ev(Event::Acked { msg_id: 42 });
        roundtrip_ev(Event::CarryAcked { msg_id: 43 });
        roundtrip_ev(Event::RouteLearned { dest: a, hops: 3 });
        roundtrip_ev(Event::Heard { addr: a, hops: 0, identity: None });
        roundtrip_ev(Event::Heard { addr: a, hops: 2, identity: Some([5; 64]) });
        roundtrip_ev(Event::ChannelJoined {
            channel: a,
            name: text(b"#test"),
        });
        roundtrip_ev(Event::FileIncoming {
            file_id: 9,
            from: a,
            size: 10,
            mime: text(b"text/plain"),
            name: text(b"a.txt"),
        });
        roundtrip_ev(Event::FileChunk {
            file_id: 9,
            offset: 0,
            data: text(b"data"),
        });
        roundtrip_ev(Event::FileProgress {
            file_id: 9,
            done: 3,
            total: 4,
            outgoing: true,
        });
        roundtrip_ev(Event::FileDone {
            file_id: 9,
            ok: false,
            outgoing: false,
            text: text(b"tidsudloeb"),
        });
        roundtrip_ev(Event::FileStarted {
            req: 2,
            file_id: 9,
            chunks: 4,
        });
        roundtrip_ev(Event::Log { text: text(b"log") });
        roundtrip_ev(Event::Deposited { msg_id: 12 });
        let mut items = Stats::new();
        items.push((stat::LAST_RSSI_DBM, -117)).unwrap();
        items.push((stat::UPTIME_S, 3600)).unwrap();
        roundtrip_ev(Event::Stats {
            items: items.clone(),
        });
        let full: Stats = (0..59).map(|i| (i as u8, i * -1000)).collect();
        assert!(
            Event::Stats {
                items: full.clone()
            }
            .encode()
            .len()
                <= MAX_FRAME + 2
        );
        roundtrip_ev(Event::Stats { items: full });
        roundtrip_ev(Event::Error {
            req: 9,
            code: error_code::TOO_LARGE,
            text: text(b"for lang"),
        });
    }

    #[test]
    fn diagnostics_roundtrip() {
        roundtrip_cmd(Command::TraceRoute { req: 5, dest: Address([4; 16]) });
        roundtrip_cmd(Command::GetNeighbors);
        roundtrip_cmd(Command::GetSharing);
        let sh = Sharing { precision: 2, lat_e7: 561_567_000, lon_e7: -102_107_000, telemetry: true };
        roundtrip_cmd(Command::SetSharing(sh));
        roundtrip_ev(Event::Sharing(sh));
        let site = crate::appdata::Site { height_dm: 150, gain_hdb: -2, tx_dbm: 22, owner: crate::appdata::clean_owner("OZ1ABC".as_bytes()) };
        roundtrip_cmd(Command::SetSite(site.clone()));
        roundtrip_cmd(Command::GetSite);
        roundtrip_ev(Event::Site(site));
        roundtrip_ev(Event::Site(Default::default()));
        assert_eq!(Command::SetSharing(Sharing::default()).encode()[2], 0x22);
        roundtrip_ev(Event::TraceResult {
            req: 3,
            dest: Address([2; 16]),
            path: [0x0102, 0xfffe].into_iter().collect(),
            out: [5, -7, 1].into_iter().collect(),
            back: [-20, 3, 9].into_iter().collect(),
        });
        roundtrip_ev(Event::TraceResult {
            req: 0,
            dest: Address([2; 16]),
            path: heapless::Vec::new(),
            out: [4].into_iter().collect(),
            back: heapless::Vec::new(),
        });
        let mut t: crate::diag::NeighborTable<40> = crate::diag::NeighborTable::new();
        for i in 0..20u16 {
            t.heard(i, (i == 3).then_some(Address([0xab; 16])), -(i as i8), -100 - i as i16, i as u64 * 1000);
        }
        let evs = neighbor_events(&t, 30_000);
        assert_eq!(evs.len(), 2);
        for e in &evs {
            assert!(e.encode().len() <= MAX_FRAME + 2);
            roundtrip_ev(e.clone());
        }
        let Event::Neighbors { part: 0, last: false, items } = &evs[0] else { panic!() };
        assert_eq!(items[0].id, 19);
        assert_eq!(items[0].age_s, 11);
        assert_eq!(items[0].rssi, -119);
        assert!(matches!(&evs[1], Event::Neighbors { part: 1, last: true, .. }));
        let empty = neighbor_events(&crate::diag::NeighborTable::<4>::new(), 0);
        assert!(matches!(&empty[0], Event::Neighbors { part: 0, last: true, items } if items.is_empty()));
        roundtrip_ev(Event::NodeInfo {
            addr: Address([1; 16]),
            hops: 2,
            position: sh.position(),
            telemetry: Some(crate::appdata::Telemetry { battery_mv: 3700, energy: 2, uptime_s: 5, ..Default::default() }),
            name: crate::appdata::clean_name("Ida on the hill".as_bytes()),
            caps: None,
        });
        roundtrip_ev(Event::NodeInfo { addr: Address([1; 16]), hops: 0, position: None, telemetry: None, name: None, caps: None });
        roundtrip_ev(Event::NodeInfo { addr: Address([1; 16]), hops: 1, position: None, telemetry: None, name: None, caps: Some(0x20) });
        roundtrip_cmd(Command::GetName);
        roundtrip_cmd(Command::RequestPage { req: 9, dest: Address([3; 16]), path: text(b"vejr") });
        roundtrip_cmd(Command::PostBoard { req: 9, dest: Address([3; 16]), path: text(b"crew"), text: text(b"hello board") });
        roundtrip_cmd(Command::SetName { name: text("Ida".as_bytes()) });
        roundtrip_cmd(Command::SetWifi(WifiSettings {
            enabled: true,
            modem: true,
            ssid: heapless::Vec::from_slice(b"Telefon").unwrap(),
            password: heapless::Vec::from_slice(b"hemmelig kode").unwrap(),
            peer: heapless::Vec::from_slice(b"example.org:4242").unwrap(),
            modem_key: None,
        }));
        roundtrip_cmd(Command::SetModemKey { passphrase: Text::from_slice(b"mast modem key").unwrap() });
        roundtrip_cmd(Command::SetWifi(WifiSettings::default()));
        roundtrip_cmd(Command::GetWifi);
        roundtrip_ev(Event::Wifi {
            enabled: true,
            modem: false,
            state: wifi_state::LINKED,
            ip: [192, 168, 43, 17],
            ssid: heapless::Vec::from_slice(b"Telefon").unwrap(),
            peer: heapless::Vec::from_slice(b"10.8.0.2:4242").unwrap(),
        });
        // A 33-byte SSID does not fit.
        let mut long = vec![0x28, 1, 33];
        long.extend_from_slice(&[b'x'; 33]);
        long.push(0);
        assert!(Command::decode(&long).is_err());
        roundtrip_ev(Event::Name { name: text(b"Ida") });
        roundtrip_cmd(Command::SendCall { dest: Address([4; 16]), data: text(&[1, 2, 3]) });
        roundtrip_ev(Event::Call { from: Address([4; 16]), data: text(&[9]) });
    }

    #[test]
    fn range_test_roundtrip() {
        use crate::rangetest::{Config, FLAG_PERSIST, mode};
        let cfg = Config { mode: mode::BEACON, flags: FLAG_PERSIST, interval_s: 10, sf_mask: 0x3f, per_step: 5, pad: 0, lat_e7: 561_000_000, lon_e7: 102_000_000 };
        roundtrip_cmd(Command::SetRangeTest(cfg));
        roundtrip_cmd(Command::SetRangeTest(Config::default()));
        roundtrip_cmd(Command::GetRangeTest);
        assert_eq!(&Command::GetRangeTest.encode()[..], &[1, 0, 0x2b]);
        let meta = AlertMeta { status: 1, kind: 2, event: 4, severity: 3, expires_min: 90, reference: 77, area: Some((5_615_000, -1_020_000, 50)) };
        roundtrip_cmd(Command::SendAlert { req: 9, meta, text: Text::from_slice(b"Flood").unwrap() });
        roundtrip_cmd(Command::SendAlert { req: 9, meta: AlertMeta { area: None, ..meta }, text: Text::new() });
        roundtrip_ev(Event::Alert {
            issuer: Address([3; 16]),
            serial: 1_800_000_001,
            issued: 1_800_000_000,
            meta,
            hops: 4,
            trust: alert_trust::CERT,
            name: heapless::Vec::from_slice(b"Beredskab Nord").unwrap(),
            text: Text::from_slice(b"Move to higher ground").unwrap(),
        });
        assert_eq!(Command::SetRangeTest(cfg).encode().len(), 2 + 1 + Config::LEN);
        assert!(Command::decode(&[0x2a, 1, 2, 3]).is_err());

        let stat = |sf| RangeSfStat { sf, rx: 40, lost: 3, rssi_avg: -118, rssi_min: -131, snr_avg_x4: -37, snr_min: -14 };
        let full = RangeState {
            config: Config { mode: mode::RECEIVER, ..cfg },
            sf: 12,
            synced: true,
            count: 1234,
            missed: 56,
            seq: 1290,
            per_sf: (7..=12).map(stat).collect(),
        };
        assert!(Event::RangeState(full.clone()).encode().len() <= MAX_FRAME + 2);
        roundtrip_ev(Event::RangeState(full));
        roundtrip_ev(Event::RangeState(RangeState::default()));

        let rx = RangeRx {
            uptime_ms: 3_600_123,
            sender: [0xde, 0xad, 0xbe, 0xef],
            seq: 77,
            sf: 11,
            rssi: -127,
            snr: -15,
            len: 30,
            lost: 2,
            skipped: 1,
            tx_power_dbm: 22,
            battery_mv: 4012,
            position: Some((561_567_000, -102_107_000)),
        };
        roundtrip_ev(Event::RangeRx(rx));
        roundtrip_ev(Event::RangeRx(RangeRx { position: None, ..rx }));
        assert_eq!(Event::RangeRx(RangeRx { position: None, ..rx }).encode().len(), 2 + 25);

        roundtrip_ev(Event::RangeTx(RangeTx {
            uptime_ms: 99,
            seq: 5,
            sf: 12,
            slot: 4,
            interval_s: 21,
            airtime_ms: 1581,
            charge_ma_ms: 186_558,
            skipped: 0,
        }));
        let p = PowerReport {
            uptime_s: 7200,
            battery_mv: 3950,
            ext_power: -1,
            mode: mode::BEACON,
            tx_power_dbm: 22,
            base_ma_x10: 30,
            rx_ma_x10: 53,
            tx_ma_x10: 1180,
            tx_packets: 300,
            phases: [
                PowerPhase { secs: 3600, tx_ms: 1000, charge_mas: 30_000 },
                PowerPhase { secs: 3000, tx_ms: 300_000, charge_mas: 60_000 },
                PowerPhase { secs: 600, tx_ms: 0, charge_mas: 5000 },
            ],
        };
        roundtrip_ev(Event::Power(p));
        roundtrip_ev(Event::Power(PowerReport::default()));
        assert_eq!(Event::Power(p).encode().len(), 2 + 56);
        for (e, code) in [
            (Event::RangeState(RangeState::default()), 0xa8),
            (Event::RangeRx(rx), 0xa9),
            (Event::Power(p), 0xab),
        ] {
            assert_eq!(e.encode()[2], code);
        }
    }

    #[test]
    fn decoder_handles_split_and_joined_frames() {
        let f1 = Event::Acked { msg_id: 1 }.encode();
        let f2 = Event::Log { text: text(b"to") }.encode();
        let mut stream = std::vec::Vec::new();
        stream.extend_from_slice(&f1);
        stream.extend_from_slice(&f2);
        let mut d = FrameDecoder::new();
        // Delivered in pieces of 3 bytes, as over BLE.
        let mut got = std::vec::Vec::new();
        for chunk in stream.chunks(3) {
            d.push(chunk);
            while let Some(f) = d.next_frame() {
                got.push(Event::decode(&f).unwrap());
            }
        }
        assert_eq!(
            got,
            [Event::Acked { msg_id: 1 }, Event::Log { text: text(b"to") }]
        );
    }

    #[test]
    fn decoder_resyncs_after_garbage() {
        let mut d = FrameDecoder::new();
        d.push(&[0, 0, 0xff, 0xff]);
        d.push(&Event::Acked { msg_id: 5 }.encode());
        let mut found = false;
        while let Some(f) = d.next_frame() {
            if Event::decode(&f) == Ok(Event::Acked { msg_id: 5 }) {
                found = true;
            }
        }
        assert!(found);
    }
}
