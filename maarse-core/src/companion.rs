//! Companion-protokollen mellem en klient (app, webside) og en node.
//!
//! Samme rammer bruges over seriel/USB, Bluetooth (Nordic UART Service) og
//! WebSocket. Strømmen består af rammer:
//!
//! ```text
//! længde (u16, little endian) | type (u8) | indhold
//! ```
//!
//! `længde` tæller type + indhold. Tal er little endian. Adresser er 16 bytes.
//! Tekst er UTF-8 og fylder resten af rammen.
//!
//! Klienten sender kommandoer (type < 0x80); noden sender svar og
//! hændelser (type >= 0x80).

use crate::addr::Address;

pub const MAX_FRAME: usize = 300;
pub const MAX_TEXT: usize = 240;
pub const PROTOCOL_VERSION: u8 = 2;

pub type Text = heapless::Vec<u8, MAX_TEXT>;
pub type FrameBuf = heapless::Vec<u8, { MAX_FRAME + 2 }>;

/// Kommandoer fra klient til node.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)] // tekstfelter; rammer er små og kortlivede
pub enum Command {
    /// 0x01: hvem er du?
    GetInfo,
    /// 0x02: send en krypteret direkte besked.
    SendDirect { req: u16, dest: Address, text: Text },
    /// 0x03: send til en kanal, vi er med i.
    SendChannel {
        req: u16,
        channel: Address,
        text: Text,
    },
    /// 0x04: deltag i en kanal. Navn som `#aarhus`, eller `offentlig`.
    JoinChannel { name: Text },
    /// 0x05: send et announce nu.
    AnnounceNow,
    /// 0x06: start en fil til `dest`. Data følger i `FileData`.
    FileBegin {
        req: u16,
        dest: Address,
        size: u32,
        mime: Text,
        name: Text,
    },
    /// 0x07: en bid af filen (op til 240 bytes).
    FileData { req: u16, offset: u32, data: Text },
    /// 0x08: hele filen er overført til noden; send den.
    FileEnd { req: u16 },
    /// 0x09: bed om status og statistik.
    GetStats,
    /// 0x0a: sæt radioindstillinger (gemmes i flash).
    SetRadio(RadioConfig),
    /// 0x0b: hvilke radioindstillinger bruges?
    GetRadio,
    /// 0x0c: send en kommando (`GetStats`, `GetRadio`, `SetRadio`, `SetRole`) til en
    /// fjern node, som vi er administrator for. Svaret kommer som
    /// `RemoteReply`.
    Remote { req: u16, dest: Address, frame: Text },
    /// 0x0d: hvem må administrere denne node (højst 4). Kun lokalt.
    SetAdmins { admins: heapless::Vec<Address, 4> },
    /// 0x0e: vis listen over administratorer.
    GetAdmins,
    /// 0x0f: klokken (Unix-sekunder). Klienten sender den ved forbindelse,
    /// så noder uden ur kan tidsstemple announces og kontrolkommandoer.
    SetTime { unix: u32 },
    /// 0x10: vælg nodens rolle (0 = klient, 1 = repeater, 2 = relay). Gemmes
    /// i flash og kan også sendes via `Remote`.
    SetRole { role: u8 },
}

/// Svar og hændelser fra node til klient.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// 0x81
    Info {
        addr: Address,
        repeater: bool,
        version: u8,
        max_text: u8,
        max_file: u32,
    },
    /// 0x82: beskeden er sendt (eller sat i kø, mens ruten findes).
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
    /// 0x85: modtageren har kvitteret.
    Acked { msg_id: u64 },
    /// 0x86
    RouteLearned { dest: Address, hops: u8 },
    /// 0x87: vi har hørt og verificeret et announce.
    Heard { addr: Address, hops: u8 },
    /// 0x88
    ChannelJoined { channel: Address, name: Text },
    /// 0x89: en fil er på vej til os.
    FileIncoming {
        file_id: u64,
        from: Address,
        size: u32,
        mime: Text,
        name: Text,
    },
    /// 0x8a: en bid af en modtaget fil (sendes til klienten, når filen er hel).
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
    /// 0x8d: svar på `FileEnd`: filen har fået et ID og sendes nu.
    FileStarted { req: u16, file_id: u64, chunks: u16 },
    /// 0x8e: fri tekst til fejlsøgning.
    Log { text: Text },
    /// 0x90: modtageren var ikke at nå; beskeden ligger nu i dens postkasse.
    Deposited { msg_id: u64 },
    /// 0x91: status som par af (nøgle, værdi). Se [`stat`].
    Stats { items: Stats },
    /// 0x92: de aktuelle radioindstillinger.
    Radio(RadioConfig),
    /// 0x93: svar fra en fjern node: en komplet hændelse uden længdefelt.
    RemoteReply { from: Address, frame: Text },
    /// 0x94: administratorer for denne node.
    Admins { admins: heapless::Vec<Address, 4> },
    /// 0x8f
    Error { req: u16, code: u8, text: Text },
}

/// LoRa-radioens indstillinger.
///
/// Alle noder i et net skal bruge samme frekvens, båndbredde, spreading
/// factor og coding rate for at kunne høre hinanden.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RadioConfig {
    pub freq_hz: u32,
    /// Båndbredde i Hz: 7800, 10400, 15600, 20800, 31250, 41700, 62500,
    /// 125000, 250000 eller 500000.
    pub bw_hz: u32,
    /// Spreading factor 5..=12.
    pub sf: u8,
    /// Coding rate-nævner 5..=8 (4/5..4/8).
    pub cr: u8,
    pub tx_power_dbm: i8,
    /// Duty cycle i promille (100 = 10 %).
    pub duty_permille: u16,
    pub preamble: u16,
}

impl RadioConfig {
    pub const LEN: usize = 16;
    /// Må:rse-standard i EU: 869,525 MHz (10 %-båndet), 125 kHz, SF9, 4/5, 22 dBm.
    pub const EU_DEFAULT: RadioConfig = RadioConfig {
        freq_hz: 869_525_000,
        bw_hz: 125_000,
        sf: 9,
        cr: 5,
        tx_power_dbm: 22,
        duty_permille: 100,
        preamble: 16,
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
        })
    }

    /// Tjekker, at indstillingerne kan bruges af en SX1262.
    pub fn validate(&self) -> Result<(), &'static str> {
        if !(150_000_000..=960_000_000).contains(&self.freq_hz) {
            return Err("Frekvensen skal være mellem 150 og 960 MHz");
        }
        if !Self::BANDWIDTHS.contains(&self.bw_hz) {
            return Err("Ugyldig båndbredde");
        }
        if !(5..=12).contains(&self.sf) {
            return Err("Spreading factor skal være 5-12");
        }
        if !(5..=8).contains(&self.cr) {
            return Err("Coding rate skal være 4/5-4/8");
        }
        if !(-9..=22).contains(&self.tx_power_dbm) {
            return Err("Sendestyrken skal være -9 til 22 dBm");
        }
        if self.duty_permille == 0 || self.duty_permille > 1000 {
            return Err("Duty cycle skal være 0,1-100 %");
        }
        if !(6..=1000).contains(&self.preamble) {
            return Err("Præamblen skal være 6-1000 symboler");
        }
        Ok(())
    }

    /// Parametre til udregning af sendetid.
    pub fn airtime(&self) -> crate::airtime::LoraParams {
        crate::airtime::LoraParams {
            sf: self.sf,
            bw_hz: self.bw_hz,
            cr: self.cr - 4,
            preamble: self.preamble,
        }
    }
}

/// Statuspar: (nøgle, værdi). Højst 59, så rammen holder sig under 300 bytes.
pub type Stats = heapless::Vec<(u8, i32), 59>;

/// Nøgler i `Event::Stats`. Klienten viser dem med danske navne.
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
    /// Fejlsøgning: hvilket trin radioløkken står i (se firmware/common/src/radio.rs).
    pub const RADIO_STAGE: u8 = 53;
    /// Fejlsøgning: antal gennemløb af radioløkken.
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
    /// Announces og ruteforespørgsler smidt væk pga. manglende/forkert stempel.
    pub const BAD_STAMPS: u8 = 77;
}

/// Fejlkoder i `Event::Error`.
pub mod error_code {
    pub const UNKNOWN_DEST: u8 = 1;
    pub const TOO_LARGE: u8 = 2;
    pub const QUEUE_FULL: u8 = 3;
    pub const NOT_IN_CHANNEL: u8 = 4;
    pub const BAD_FRAME: u8 = 5;
    pub const UNKNOWN_SENDER: u8 = 6;
    pub const FILE_TOO_LARGE: u8 = 7;
    pub const NO_FILE: u8 = 8;
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
    fn text(&mut self) -> Result<Text, FrameError> {
        let t = Text::from_slice(self.0).map_err(|_| FrameError::TooLong)?;
        self.0 = &[];
        Ok(t)
    }
    /// Op til 4 adresser, resten af rammen.
    fn addrs(&mut self) -> Result<heapless::Vec<Address, 4>, FrameError> {
        let mut v = heapless::Vec::new();
        while self.0.len() >= 16 {
            v.push(self.addr()?).map_err(|_| FrameError::TooLong)?;
        }
        Ok(v)
    }
    /// MIME-længde (u8) + MIME + navn (resten).
    fn mime_name(&mut self) -> Result<(Text, Text), FrameError> {
        let n = self.u8()? as usize;
        let mime = Text::from_slice(self.take(n)?).map_err(|_| FrameError::TooLong)?;
        Ok((mime, self.text()?))
    }
}

/// Tekst, der afkortes til `MAX_TEXT` bytes.
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
        }
    }

    /// Afkoder type + indhold (uden længdefeltet).
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
            } => W::new(0x81)
                .b(&addr.0)
                .b(&[*repeater as u8, *version, *max_text])
                .b(&max_file.to_le_bytes())
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
            Event::Heard { addr, hops } => W::new(0x87).b(&addr.0).b(&[*hops]).done(),
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
            Event::Stats { items } => {
                let mut w = W::new(0x91);
                for (k, v) in items {
                    w = w.b(&[*k]).b(&v.to_le_bytes());
                }
                w.done()
            }
            Event::Error { req, code, text } => W::new(0x8f)
                .b(&req.to_le_bytes())
                .b(&[*code])
                .b(text)
                .done(),
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
            t => return Err(FrameError::UnknownType(t)),
        })
    }
}

/// Samler rammer fra en bytestrøm (seriel, BLE), der kan komme i stykker.
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

    /// Tilføjer bytes. Kald derefter `next_frame` til den returnerer `None`.
    pub fn push(&mut self, bytes: &[u8]) {
        for &b in bytes {
            if self.buf.push(b).is_err() {
                // Bufferen er fuld af skrald: start forfra.
                self.buf.clear();
                let _ = self.buf.push(b);
            }
        }
    }

    /// Næste hele ramme (type + indhold), hvis der er en.
    pub fn next_frame(&mut self) -> Option<heapless::Vec<u8, MAX_FRAME>> {
        loop {
            if self.buf.len() < 2 {
                return None;
            }
            let len = u16::from_le_bytes([self.buf[0], self.buf[1]]) as usize;
            if len == 0 || len > MAX_FRAME {
                // Ude af synk: smid én byte og prøv igen.
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
            text: text("hej æøå".as_bytes()),
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
            name: text("billede æ.webp".as_bytes()),
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
        assert_eq!(&Command::SetRole { role: 0 }.encode()[..], &[2, 0, 0x10, 0]);
        roundtrip_cmd(Command::Remote { req: 4, dest: Address([6; 16]), frame: text(&[0x09]) });
        let admins: heapless::Vec<Address, 4> = [Address([1; 16]), Address([2; 16])].into_iter().collect();
        roundtrip_cmd(Command::SetAdmins { admins: admins.clone() });
        roundtrip_cmd(Command::SetAdmins { admins: heapless::Vec::new() });
        roundtrip_cmd(Command::GetAdmins);
        roundtrip_cmd(Command::SetTime { unix: 1_800_000_000 });
        roundtrip_ev(Event::Admins { admins });
        roundtrip_ev(Event::RemoteReply { from: Address([3; 16]), frame: text(&[0x92, 1, 2, 3]) });
        roundtrip_ev(Event::Radio(RadioConfig {
            tx_power_dbm: -3,
            ..RadioConfig::EU_DEFAULT
        }));
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
        roundtrip_ev(Event::RouteLearned { dest: a, hops: 3 });
        roundtrip_ev(Event::Heard { addr: a, hops: 0 });
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
    fn decoder_handles_split_and_joined_frames() {
        let f1 = Event::Acked { msg_id: 1 }.encode();
        let f2 = Event::Log { text: text(b"to") }.encode();
        let mut stream = std::vec::Vec::new();
        stream.extend_from_slice(&f1);
        stream.extend_from_slice(&f2);
        let mut d = FrameDecoder::new();
        // Leveret i stykker á 3 bytes, som over BLE.
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
