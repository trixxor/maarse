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
//!
//! Kommandoerne 0x30-0x33 og hændelserne 0xB0-0xB3 er reserveret til
//! beskedhistorikken i maarse-host (se `maarse-host/src/history.rs`). De
//! afkodes kun dér; firmware svarer med `Error` (ugyldig ramme).

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
    /// 0x11: sæt nodens regioner (kommasepareret, fx `dk,nordjylland`, højst
    /// 8) og `strict` (1 = med tom liste videresendes kun floods uden region).
    /// Gemmes i flash. Se [`crate::region`].
    SetRegions { strict: bool, names: Text },
    /// 0x12: vis nodens regioner.
    GetRegions,
    /// 0x13: deltag i en privat kanal med en tilfældig nøgle (fra en
    /// invitation). Format: nøgle (32 bytes) | navn (UTF-8, resten). Svaret
    /// er `ChannelJoined` som for andre kanaler.
    JoinPrivateChannel { key: [u8; 32], name: Text },
    /// 0x20: trace route til `dest`: SNR for hvert spring begge veje. Svaret
    /// er `TraceResult` (eller `Error` med `req`). Se [`crate::diag`].
    TraceRoute { req: u16, dest: Address },
    /// 0x21: vis direkte naboer. Svaret er en eller flere `Neighbors`.
    GetNeighbors,
    /// 0x22: hvad noden deler i sine announces: position (præcision 0 =
    /// slukket, standard) og telemetri. Gemmes. Svaret er `Sharing`.
    SetSharing(Sharing),
    /// 0x23: vis delingsindstillingerne.
    GetSharing,
    /// 0x24: sæt nodens visningsnavn (UTF-8, højst 24 bytes; tomt = intet).
    /// Svar: `Name`. Navnet sendes i announces og er ikke bevist.
    SetName { name: Text },
    /// 0x25: hent visningsnavnet. Svar: `Name`.
    GetName,
    /// 0x26: bed `dest` om siden `path` (`maarse://<dest>/<path>`). Svar:
    /// `Sent`; siden kommer som en fil med MIME
    /// [`crate::content::PAGE_MIME`]. Ugyldig sti giver `Error`.
    RequestPage { req: u16, dest: Address, path: Text },
    /// 0x18: start (eller genoptag) en firmwareopdatering. `manifest` er det
    /// signerede manifest (112 bytes, se `maarse_crypto::ota`). Flag bit 0:
    /// tillad ældre version (kun lokalt, ignoreres via `Remote`). Svaret er
    /// `OtaState` med det offset, klienten skal sende fra. Kan sendes via
    /// `Remote` (kun administratorer). 0x14-0x17 er holdt fri til andet arbejde.
    OtaBegin { req: u16, flags: u8, manifest: Text },
    /// 0x19: en bid af imaget. Offset og længde skal være delelige med 4
    /// (undtagen sidste bid). Svaret er `OtaState` med næste offset.
    OtaChunk { req: u16, offset: u32, data: Text },
    /// 0x1a: afslut. Imaget skal være helt overført og verificeret; med
    /// `apply` installeres det, og noden genstarter.
    OtaEnd { req: u16, apply: bool },
    /// 0x1b: hvor langt er en opdatering? Svaret er `OtaState`.
    OtaStatus,
    /// 0x14: hvor stor en fil må sendes til `dest` ad den aktuelle rute?
    /// Svaret er `FileLimit`. Firmware-noder kender ikke kommandoen (de
    /// svarer med `Error`); klienten bruger så `Info.max_file`.
    GetFileLimit { dest: Address },
    /// 0x34: udskift nodens identitet med `secret` (64 bytes: Ed25519-frø |
    /// X25519-nøgle), typisk afledt af en gendannelsesfrase i klienten (se
    /// `maarse_crypto::recovery`). Kun lokalt (USB/BLE) — afvises altid via
    /// `Remote`. Kræver `confirm` (flag [`IDENTITY_CONFIRM`]), ellers afvises
    /// den. Noden gemmer nøglen, glemmer ruter og forward secrecy-sessioner
    /// og sender et nyt announce. Svaret er `IdentityChanged`.
    /// 0x35-0x37 er holdt fri til tilknyttede enheder.
    SetIdentity { confirm: bool, secret: [u8; 64] },
}

/// Flag i `Command::SetIdentity`: brugeren har bekræftet udskiftningen.
pub const IDENTITY_CONFIRM: u8 = 1;

/// Delingsindstillinger (10 bytes): præcision (u8, se
/// [`crate::appdata::Precision`]) | bredde (i32, 1e-7°) | længde (i32)
/// | telemetri (u8, 0/1). Positionen gemmes præcist på noden, men sendes
/// kun afrundet til den valgte præcision.
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
    /// Positionen, som den sendes (afrundet), eller `None` (slukket).
    pub fn position(&self) -> Option<crate::appdata::Position> {
        crate::appdata::Position {
            precision: crate::appdata::Precision::from_u8(self.precision),
            lat_e7: self.lat_e7,
            lon_e7: self.lon_e7,
        }
        .quantised()
    }
}

/// En nabo i `Event::Neighbors` (15 bytes): sti-ID (u16) | adresse-
/// præfiks (4, nuller = ukendt) | SNR (i8) | RSSI (i16, 0 = ukendt) |
/// sekunder siden (u32) | antal pakker (u16).
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

/// Højst så mange naboer pr. `Neighbors`-ramme.
pub const NEIGHBORS_PER_FRAME: usize = 16;

/// Bygger `Neighbors`-rammer ud fra en nabotabel (mindst én, også tom).
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

/// Tilstande i `Event::OtaState`.
pub mod ota_state {
    pub const IDLE: u8 = 0;
    pub const RECEIVING: u8 = 1;
    pub const VERIFIED: u8 = 2;
    pub const INSTALLING: u8 = 3;
    pub const ERROR: u8 = 4;
    /// Boardet understøtter ikke opdatering over luften.
    pub const UNSUPPORTED: u8 = 5;
}

/// Flag i `Command::OtaBegin`.
pub const OTA_ALLOW_DOWNGRADE: u8 = 1;

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
        /// Nodens offentlige nøgler (Ed25519 | X25519), til sikkerhedsnumre.
        /// Tilføjet bagerst; ældre noder sender dem ikke.
        identity: Option<[u8; 64]>,
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
    /// 0x87: vi har hørt og verificeret et announce. `identity` er
    /// afsenderens offentlige nøgler (64 bytes bagerst, hvis kendt).
    Heard { addr: Address, hops: u8, identity: Option<[u8; 64]> },
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
    /// 0x95: nodens regioner (kommasepareret) og `strict`.
    Regions { strict: bool, names: Text },
    /// 0x9a: firmwareopdateringens tilstand (se [`ota_state`]). `next` er
    /// næste forventede offset, `size` og `version` fra manifestet (0 uden),
    /// `current` den kørende version. Tekst forklarer fejl.
    OtaState { req: u16, state: u8, next: u32, size: u32, version: u32, current: u32, text: Text },
    /// 0x96: svar på `GetFileLimit`. Format: dest (16) | max_bytes (u32) |
    /// flag (u8: bit 0 = over LoRa eller ukendt rute, bit 1 = modtageren er
    /// ikke at nå, filen lægges i postkasse) | est_airtime_s (u32, anslået
    /// sendetid for `max_bytes` over LoRa; 0 over internettet).
    FileLimit {
        dest: Address,
        max_bytes: u32,
        via_lora: bool,
        mailbox: bool,
        est_airtime_s: u32,
    },
    /// 0x8f
    Error { req: u16, code: u8, text: Text },
    /// 0xa0: svar på `TraceRoute`. `path` er repeaternes sti-ID'er; `out` er
    /// SNR for hvert spring ud (sidste = målets), `back` for hvert spring
    /// tilbage (sidste = vores). Format: req (u16) | mål (16) | k | k × ID
    /// (u16) | m | m × SNR (i8) | SNR tilbage (resten).
    TraceResult {
        req: u16,
        dest: Address,
        path: heapless::Vec<u16, 12>,
        out: heapless::Vec<i8, 13>,
        back: heapless::Vec<i8, 13>,
    },
    /// 0xa1: direkte naboer, senest hørt først. `part` tæller fra 0;
    /// `last` er sat på sidste ramme. Format: part | last | naboer (15 B).
    Neighbors {
        part: u8,
        last: bool,
        items: heapless::Vec<NeighborInfo, NEIGHBORS_PER_FRAME>,
    },
    /// 0xa2: delingsindstillinger (se [`Sharing`]).
    Sharing(Sharing),
    /// 0xa4: nodens eget visningsnavn (tomt = intet).
    Name { name: Text },
    /// 0xa3: position og/eller telemetri fra en anden nodes announce.
    /// Format: adresse (16) | hop | app-data-felter (0x03/0x04, se
    /// [`crate::appdata`]).
    NodeInfo {
        addr: Address,
        hops: u8,
        position: Option<crate::appdata::Position>,
        telemetry: Option<crate::appdata::Telemetry>,
        name: Option<crate::appdata::Name>,
    },
    /// 0xb4: nodens identitet er udskiftet (svar på `SetIdentity`).
    /// Format: ny adresse (16) | offentlig identitet (64).
    IdentityChanged { addr: Address, identity: [u8; 64] },
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
            cr: self.cr.saturating_sub(4).max(1),
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
    // Diagnose og strøm
    /// Anslået støjgulv (dBm) ud fra modtagne pakkers RSSI og SNR.
    pub const NOISE_FLOOR_DBM: u8 = 80;
    /// Energistatus (0 = kritisk, 1 = lav, 2 = ok, 3 = fuld).
    pub const ENERGY: u8 = 81;
    /// Ekstern strøm (USB): 0 = nej, 1 = ja, -1 = ukendt.
    pub const EXT_POWER: u8 = 82;
    /// Antal direkte naboer.
    pub const NEIGHBORS: u8 = 83;
    /// Delt positionspræcision (0 = slukket ... 3 = præcis).
    pub const POSITION_PRECISION: u8 = 84;
    /// Batteri i procent (omtrentligt, ud fra spændingen).
    pub const BATTERY_PERCENT: u8 = 85;
    /// Chip-temperatur i 0,1 °C (lidt over omgivelserne).
    pub const TEMPERATURE_DC: u8 = 86;
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
    /// Valgfri offentlig identitet (64 bytes) bagerst i rammen.
    fn identity(&mut self) -> Option<[u8; 64]> {
        let mut k = [0u8; 64];
        k.copy_from_slice(self.take(64).ok()?);
        Some(k)
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
            Command::GetName => W::new(0x25).done(),
            Command::RequestPage { req, dest, path } => W::new(0x26).b(&req.to_le_bytes()).b(&dest.0).b(path).done(),
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
            0x25 => Command::GetName,
            0x26 => Command::RequestPage { req: r.u16()?, dest: r.addr()?, path: r.text()? },
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
            Event::Name { name } => W::new(0xa4).b(name).done(),
            Event::NodeInfo { addr, hops, position, telemetry, name } => {
                let data = crate::appdata::build(None, false, *position, telemetry.as_ref(), name.as_deref());
                W::new(0xa3).b(&addr.0).b(&[*hops]).b(&data).done()
            }
            Event::IdentityChanged { addr, identity } => W::new(0xb4).b(&addr.0).b(identity).done(),
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
            0xa2 => Event::Sharing(Sharing::from_bytes(r.take(Sharing::LEN)?).ok_or(FrameError::TooShort)?),
            0xa3 => {
                let (addr, hops) = (r.addr()?, r.u8()?);
                let a = crate::appdata::parse(r.0);
                Event::NodeInfo { addr, hops, position: a.position, telemetry: a.telemetry, name: a.name }
            }
            0xa4 => Event::Name { name: r.text()? },
            0xb4 => {
                let addr = r.addr()?;
                let identity = r.identity().ok_or(FrameError::TooShort)?;
                Event::IdentityChanged { addr, identity }
            }
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
        roundtrip_cmd(Command::JoinPrivateChannel { key: [9; 32], name: text("hytten æ".as_bytes()) });
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
        roundtrip_cmd(Command::SetIdentity { confirm: false, secret: [0; 64] });
        roundtrip_ev(Event::IdentityChanged { addr: Address([5; 16]), identity: [9; 64] });
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
        // En OTA-bid over radio: kontrolhoved (5) + ramme uden længde skal
        // passe i en forward secrecy-besked (MAX_FS_TEXT = 143).
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
            name: crate::appdata::clean_name("Ida på bakken".as_bytes()),
        });
        roundtrip_ev(Event::NodeInfo { addr: Address([1; 16]), hops: 0, position: None, telemetry: None, name: None });
        roundtrip_cmd(Command::GetName);
        roundtrip_cmd(Command::RequestPage { req: 9, dest: Address([3; 16]), path: text(b"vejr") });
        roundtrip_cmd(Command::SetName { name: text("Ida".as_bytes()) });
        roundtrip_ev(Event::Name { name: text(b"Ida") });
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
