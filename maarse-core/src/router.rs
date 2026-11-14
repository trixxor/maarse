//! Routing-motoren.
//!
//! `Router` har ingen I/O og ingen ur. Den får bytes og tidspunkter ind og
//! returnerer [`Action`]s: pakker der skal sendes (med en forsinkelse),
//! beskeder der skal leveres til appen, og hændelser som "rute fundet".
//! Det gør den let at teste og at køre ens i firmware og simulator.
//!
//! ## Sådan findes en rute
//! 1. Afsenderen floodes en `PathRequest` med et tilfældigt svar-token.
//! 2. Hver repeater tilføjer sit sti-ID og sender videre efter en
//!    forsinkelse: jo bedre SNR, jo længere ventetid. Så vinder den
//!    repeater, der er længst væk, og ruterne får færre hop. Repeatere med
//!    lavt batteri venter ekstra og bliver derfor sjældnere valgt.
//! 3. Destinationen svarer med en `PathResponse` til tokenet ad den
//!    omvendte sti. Afsenderens adresse står aldrig i klartekst.
//! 4. Afsenderen gemmer stien og sender beskeden som kildesti.
//! 5. Modtageren kvitterer med en `Ack` til et token afledt af beskedens ID.

use crate::addr::{Address, PathId};
use crate::dedup::{DedupCache, packet_id};
use crate::packet::{Header, Packet, PacketType, Path, encode};
use crate::routes::{Route, RouteTable};
use crate::stamp;
use crate::{LOCAL_SCOPE, MAX_HOPS, MAX_PACKET, MAX_PATH};

pub type Buf = heapless::Vec<u8, MAX_PACKET>;
pub type Actions = heapless::Vec<Action, 8>;

/// Forsinkelse før en repeater sender en kildestyret pakke videre (ms).
pub const SOURCE_ROUTE_DELAY_MS: u32 = 50;
/// Største payload i en direkte besked (plads til en fuld sti på 12 hop).
pub const MAX_DIRECT_PAYLOAD: usize = MAX_PACKET - 18 - 1 - 2 * MAX_PATH;

/// Standard-tidsslot for flood-ventetid (ms). Bør være omkring sendetiden
/// for en kort pakke ved den valgte spreading factor.
pub const DEFAULT_SLOT_MS: u32 = 200;
/// Hvor længe vi venter på at høre næste repeater sende videre (ms).
pub const DEFAULT_HOP_TIMEOUT_MS: u32 = 3000;
/// Antal gange et hop gensendes, hvis næste repeater ikke høres.
pub const HOP_RETRIES: u8 = 4;

/// Standard hop-grænse for relays i gruppe-tilstand.
pub const DEFAULT_RELAY_HOPS: u8 = 4;

const MAX_PENDING: usize = 8;
/// Destinationen venter så mange tidsslots, før den svarer på en
/// ruteforespørgsel (floodet skal nå at dø ud omkring den).
pub const RESPONSE_WAIT_SLOTS: u32 = 10;
/// Uden kvittering inden for denne tid antages ruten at være brudt: den
/// glemmes, og beskeden sendes igen med en ny ruteforespørgsel (ms).
pub const DEFAULT_ACK_TIMEOUT_MS: u32 = 20_000;
/// Antal gange en besked sendes igen fra afsenderen.
pub const E2E_RETRIES: u8 = 2;
/// En mobil node stoler ikke på ruter ældre end dette (ms).
pub const MOBILE_ROUTE_MS: u64 = 30_000;
/// Ventetid før en ubesvaret ruteforespørgsel gentages (ms).
///
/// Svaret kan være længe undervejs: over mange hop, hvor hvert hop kan
/// gensendes flere gange med `DEFAULT_HOP_TIMEOUT_MS` imellem. Gentages
/// forespørgslen for tidligt, floodes nettet igen, mens svaret stadig er på
/// vej, og det nye flood overdøver svaret. I simulatoren var det hovedårsagen
/// til tabte kvitteringer i store, tynde net (15 s gav 50 % kvitteret, 45 s
/// gav over 80 %).
pub const PATH_RETRY_MS: u64 = 45_000;
/// Antal gentagelser af en ruteforespørgsel.
pub const PATH_RETRIES: u8 = 2;
const MAX_FORWARDS: usize = 32;
const MAX_FLOOD_PENDING: usize = 16;
const MAX_QUEUE: usize = 8;
const MAX_AWAIT: usize = 32;
const MAX_CHANNELS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Mobil node eller node på batteri uden opladning. Videresender ikke.
    Client,
    /// Står stille og har positivt energibudget (fast strøm eller sol).
    Repeater,
    /// Håndholdt node i gruppe-tilstand ("skov-tilstand"), når der ingen
    /// repeatere er. Videresender som en repeater, men venter længere (så
    /// rigtige repeatere vinder) og kun inden for `relay_hops`.
    Relay,
}

impl Role {
    /// Rollen som tal på wire/i flash (0 = klient, 1 = repeater, 2 = relay).
    pub fn as_u8(self) -> u8 {
        match self {
            Role::Client => 0,
            Role::Repeater => 1,
            Role::Relay => 2,
        }
    }

    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Role::Client),
            1 => Some(Role::Repeater),
            2 => Some(Role::Relay),
            _ => None,
        }
    }

    /// Videresender noden for andre (repeater eller relay)?
    pub fn forwards(self) -> bool {
        self != Role::Client
    }
}

/// Energistatus, som meddeles i announces og påvirker routing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Energy {
    /// Videresender intet; noden sover, til den er ladet op.
    Critical = 0,
    /// Videresender kun direkte beskeder og ruteforespørgsler.
    Low = 1,
    Ok = 2,
    Full = 3,
}

impl Energy {
    /// Ekstra ventetid i flood, så ruter undgår repeatere med lavt batteri.
    fn penalty_ms(self) -> u32 {
        match self {
            Energy::Full => 0,
            Energy::Ok => 150,
            Energy::Low => 800,
            Energy::Critical => 3000,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Send pakken efter `delay_ms`. Laget under står for duty cycle og CAD.
    /// `id` er pakkens ID, så en senere `Cancel` kan finde den.
    Transmit { packet: Buf, delay_ms: u32, id: u64 },
    /// Annullér en planlagt videresendelse af et flood, fordi naboerne
    /// allerede har dækket området. Er den sendt, ignoreres dette.
    Cancel { id: u64 },
    /// En pakke til os eller til en kanal, vi lytter på.
    Deliver {
        ptype: PacketType,
        dest: Address,
        payload: Buf,
        hops: u8,
    },
    /// Vi har fundet en rute.
    RouteLearned { dest: Address, hops: u8 },
    /// En direkte besked er kvitteret af modtageren.
    Acked { msg_id: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendError {
    TooLarge,
    /// Ingen kendt rute (send en direkte besked først).
    NoRoute,
    QueueFull,
    TooManyChannels,
}

struct PendingRequest {
    token: Address,
    /// Tokens fra tidligere forsøg: et sent svar på dem gælder stadig.
    old_tokens: heapless::Vec<Address, 2>,
    target: Address,
    /// Besked, der blev sendt med i forespørgslen (som MeshCore: første
    /// besked floodes, og svaret er både rute og kvittering).
    carried: Option<u64>,
    /// Hvornår forespørgslen gentages, hvis intet svar.
    retry_at_ms: u64,
    tries: u8,
}

struct Queued {
    ptype: PacketType,
    dest: Address,
    payload: Buf,
}

/// Et svar på en ruteforespørgsel, som venter på, at flere kopier kan nå
/// frem, så det bedste spor kan vælges.
struct PendingReply {
    token: Address,
    best: Path,
    best_snr: i8,
    reply_at_ms: u64,
}

struct AwaitAck {
    token: Address,
    msg_id: u64,
    ptype: PacketType,
    dest: Address,
    payload: Buf,
    sent_ms: u64,
    tries: u8,
}

/// En kildestyret pakke, vi har sendt, og hvor vi lytter efter, at næste
/// repeater sender den videre (implicit kvittering pr. hop).
struct Forward {
    id: u64,
    hops: u8,
    packet: Buf,
    deadline_ms: u64,
    tries: u8,
}

/// Grænse for et svagt link ved en given spreading factor: modtagerens
/// følsomhedsgrænse plus 5 dB margen.
pub fn weak_snr_for_sf(sf: u8) -> i8 {
    let floor = match sf {
        5 | 6 => -5,
        7 => -8,
        8 => -10,
        9 => -13,
        10 => -15,
        11 => -18,
        _ => -20,
    };
    floor + 5
}

/// Token, som modtageren sender kvitteringen til.
pub fn ack_token(msg_id: u64) -> Address {
    let mut b = [0u8; 16];
    b[..8].copy_from_slice(&msg_id.to_le_bytes());
    b[8..].copy_from_slice(&(!msg_id).to_le_bytes());
    Address(b)
}

fn reversed(path: &Path) -> Path {
    let mut p = Path::new();
    for id in path.iter().rev() {
        let _ = p.push(*id);
    }
    p
}

/// Tællere til status-siden.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RouterStats {
    /// Gyldige pakker modtaget.
    pub rx_packets: u32,
    /// Pakker, der ikke kunne afkodes.
    pub rx_invalid: u32,
    /// Kopier af pakker, vi allerede havde set.
    pub rx_duplicates: u32,
    /// Pakker, vi har sendt videre for andre.
    pub forwarded: u32,
    /// Pakker, vi selv har startet.
    pub originated: u32,
    /// Pakker leveret til os (eller en kanal, vi lytter på).
    pub delivered: u32,
    /// Kvitteringer modtaget for vores beskeder.
    pub acked: u32,
    pub routes_learned: u32,
    pub path_requests: u32,
    /// Hop gensendt, fordi næste repeater ikke blev hørt.
    pub hop_retries: u32,
    /// Flood-udsendelser annulleret, fordi naboer havde dækket området.
    pub floods_cancelled: u32,
    /// Announces og ruteforespørgsler smidt væk, fordi arbejdsbeviset
    /// (stemplet) manglede eller var forkert (se [`crate::stamp`]).
    pub bad_stamps: u32,
}

/// Routing-motor med `R` ruter og et dublet-filter på `D` pakker.
pub struct Router<const R: usize, const D: usize> {
    pub addr: Address,
    pub id: PathId,
    pub role: Role,
    pub energy: Energy,
    routes: RouteTable<R>,
    dedup: DedupCache<D>,
    pending: heapless::Vec<PendingRequest, MAX_PENDING>,
    queue: heapless::Vec<Queued, MAX_QUEUE>,
    awaiting: heapless::Vec<AwaitAck, MAX_AWAIT>,
    channels: heapless::Vec<Address, MAX_CHANNELS>,
    forwards: heapless::Vec<Forward, MAX_FORWARDS>,
    replies: heapless::Vec<PendingReply, 4>,
    /// Floods vi har planlagt at sende videre, og hvor mange kopier vi har hørt.
    flood_pending: heapless::Vec<(u64, u8), MAX_FLOOD_PENDING>,
    /// Tidsslot for flood-ventetid (ms).
    pub slot_ms: u32,
    /// Ventetid på implicit kvittering pr. hop (ms).
    pub hop_timeout_ms: u32,
    /// Annullér egen videresendelse af et flood efter så mange hørte kopier
    /// med god SNR (se `on_duplicate_flood`).
    pub suppress_after: u8,
    /// Største antal hop, en relay videresender (standard 4, højst 7).
    pub relay_hops: u8,
    /// Ventetid på kvittering fra ende til ende, før ruten repareres (ms).
    pub ack_timeout_ms: u32,
    /// Noden bevæger sig (fx i en bil): ruter forældes hurtigt.
    pub mobile: bool,
    /// Et sidste spring med SNR under dette regnes for svagt. Brug
    /// [`weak_snr_for_sf`] til den valgte spreading factor.
    pub weak_snr_db: i8,
    /// Regioner, noden videresender floods for (se [`crate::region`]).
    pub regions: crate::region::Regions,
    /// Med tom regionsliste: videresend kun floods uden region.
    pub strict_region: bool,
    pub stats: RouterStats,
}

impl<const R: usize, const D: usize> Router<R, D> {
    /// Skifter rolle under kørsel (fx efter `SetRole` fra appen).
    pub fn set_role(&mut self, role: Role) {
        self.role = role;
    }

    pub fn new(addr: Address, role: Role) -> Self {
        Router {
            addr,
            id: addr.path_id(),
            role,
            energy: Energy::Full,
            routes: RouteTable::new(),
            dedup: DedupCache::new(),
            pending: heapless::Vec::new(),
            queue: heapless::Vec::new(),
            awaiting: heapless::Vec::new(),
            channels: heapless::Vec::new(),
            forwards: heapless::Vec::new(),
            replies: heapless::Vec::new(),
            flood_pending: heapless::Vec::new(),
            slot_ms: DEFAULT_SLOT_MS,
            hop_timeout_ms: DEFAULT_HOP_TIMEOUT_MS,
            suppress_after: 1,
            relay_hops: DEFAULT_RELAY_HOPS,
            ack_timeout_ms: DEFAULT_ACK_TIMEOUT_MS,
            mobile: false,
            weak_snr_db: weak_snr_for_sf(9),
            regions: crate::region::Regions::new(),
            strict_region: false,
            stats: RouterStats::default(),
        }
    }

    pub fn routes(&self) -> &RouteTable<R> {
        &self.routes
    }

    pub fn subscribe(&mut self, channel: Address) -> Result<(), SendError> {
        if self.channels.contains(&channel) {
            return Ok(());
        }
        self.channels
            .push(channel)
            .map_err(|_| SendError::TooManyChannels)
    }

    /// Glemmer en rute, fx efter flere mistede kvitteringer.
    pub fn forget_route(&mut self, dest: &Address) {
        self.routes.remove(dest);
    }

    /// Ventetid før en repeater sender et flood videre.
    ///
    /// Høj SNR (tæt på) giver lang ventetid, så fjerne repeatere sender først.
    ///
    /// SNR deles i 8 trin à 5 dB; hvert trin er ét tidsslot. Laget under bør
    /// lægge en tilfældig ventetid på 0..slot oveni, så repeatere i samme
    /// trin ikke sender samtidig.
    pub fn flood_delay_ms(&self, snr_db: i8) -> u32 {
        let bucket = ((snr_db as i32).clamp(-20, 19) + 20) / 5; // 0..=7
        let relay = if self.role == Role::Relay {
            2 * self.slot_ms
        } else {
            0
        };
        bucket as u32 * self.slot_ms + self.energy.penalty_ms() + relay
    }

    /// Må vi videresende et flood i `region` (0 = ingen)?
    pub fn forwards_region(&self, region: u16) -> bool {
        crate::region::forwards(&self.regions, self.strict_region, region)
    }

    /// Må vi videresende en pakke, der har taget `hops` hop?
    fn may_forward(&self, hops: u8) -> bool {
        match self.role {
            Role::Client => false,
            Role::Repeater => self.energy != Energy::Critical,
            Role::Relay => self.energy != Energy::Critical && hops < self.relay_hops.min(7),
        }
    }

    fn emit(&mut self, header: &Header, payload: &[u8], delay_ms: u32, out: &mut Actions) {
        let Ok(packet) = encode(header, payload) else {
            return;
        };
        let id = packet_id(header.ptype, &header.dest, payload);
        // Vores egne pakker skal ikke behandles igen, når naboer gentager dem.
        self.dedup.insert(id);
        let _ = out.push(Action::Transmit {
            packet,
            delay_ms,
            id,
        });
    }

    /// Sender en kildestyret pakke og husker den, så den kan gensendes, hvis
    /// vi ikke hører næste repeater sende den videre.
    fn emit_routed(
        &mut self,
        header: &Header,
        payload: &[u8],
        delay_ms: u32,
        now_ms: u64,
        out: &mut Actions,
    ) {
        self.emit(header, payload, delay_ms, out);
        let next_exists = header
            .path
            .as_ref()
            .is_some_and(|p| (header.hops as usize) < p.len());
        if !next_exists {
            return; // Sidste hop: dækkes af kvitteringen fra ende til ende.
        }
        let Ok(packet) = encode(header, payload) else {
            return;
        };
        if self.forwards.is_full() {
            self.forwards.remove(0);
        }
        let _ = self.forwards.push(Forward {
            id: packet_id(header.ptype, &header.dest, payload),
            hops: header.hops,
            packet,
            deadline_ms: now_ms + delay_ms as u64 + self.hop_timeout_ms as u64,
            tries: 0,
        });
    }

    /// Videresender et flood senere og husker det, så vi kan annullere.
    fn emit_flood(&mut self, header: &Header, payload: &[u8], delay_ms: u32, out: &mut Actions) {
        self.emit(header, payload, delay_ms, out);
        let id = packet_id(header.ptype, &header.dest, payload);
        if self.flood_pending.is_full() {
            self.flood_pending.remove(0);
        }
        let _ = self.flood_pending.push((id, 0));
    }

    /// En kopi af et flood, vi allerede har set. Har vi selv planlagt at
    /// sende det videre, og har naboer allerede gjort det, annulleres vores.
    ///
    /// Kun kopier hørt med god SNR tæller: de kommer fra en nær nabo, hvis
    /// udsendelse dækker stort set det samme område som vores. En svag kopi
    /// kommer langvejsfra og dækker et andet område, så den er ikke grund
    /// til at tie. Derfor er én stærk kopi nok (`suppress_after` = 1). Talt
    /// med alle kopier ville floodet dø ud på lange kæder (motorvej), og
    /// de fundne ruter ville kun bestå af svage led.
    fn on_duplicate_flood(&mut self, id: u64, snr_db: i8, out: &mut Actions) {
        if snr_db < self.weak_snr_db {
            return;
        }
        if let Some(i) = self.flood_pending.iter().position(|(x, _)| *x == id) {
            self.flood_pending[i].1 += 1;
            if self.flood_pending[i].1 >= self.suppress_after {
                self.flood_pending.remove(i);
                let _ = out.push(Action::Cancel { id });
            }
        }
    }

    /// Tidspunkt for næste gensendelse, hvis nogen.
    pub fn next_deadline(&self) -> Option<u64> {
        let fwd = self.forwards.iter().map(|f| f.deadline_ms);
        let req = self.pending.iter().map(|p| p.retry_at_ms);
        let t = self.ack_timeout_ms as u64;
        let ack = self.awaiting.iter().map(|a| a.sent_ms + t);
        let rep = self.replies.iter().map(|r| r.reply_at_ms);
        fwd.chain(req).chain(ack).chain(rep).min()
    }

    /// Kaldes, når `next_deadline` er nået: gensender hop, der ikke er
    /// blevet sendt videre, og opgiver dem efter `HOP_RETRIES` forsøg.
    pub fn tick(&mut self, now_ms: u64) -> Actions {
        let mut out = Actions::new();
        // Svar på ruteforespørgsler ad det bedste spor, vi har hørt.
        while let Some(i) = self.replies.iter().position(|r| r.reply_at_ms <= now_ms) {
            let r = self.replies.swap_remove(i);
            let mut payload = Buf::new();
            let _ = payload.push(r.best.len() as u8);
            for id in &r.best {
                let _ = payload.extend_from_slice(&id.to_be_bytes());
            }
            let header = Header::new(PacketType::PathResponse, r.token).with_path(reversed(&r.best));
            self.emit_routed(&header, &payload, 0, now_ms, &mut out);
        }
        // Beskeder uden kvittering: ruten er nok brudt (fx fordi modtageren
        // har flyttet sig). Glem den og send igen med ny ruteforespørgsel.
        let timeout = self.ack_timeout_ms as u64;
        let mut i = 0;
        while i < self.awaiting.len() {
            if self.awaiting[i].sent_ms + timeout > now_ms {
                i += 1;
                continue;
            }
            if self.awaiting[i].tries >= E2E_RETRIES {
                self.awaiting.remove(i);
                continue;
            }
            let a = &mut self.awaiting[i];
            a.tries += 1;
            a.sent_ms = now_ms;
            let (ptype, dest, payload, msg_id, tries) =
                (a.ptype, a.dest, a.payload.clone(), a.msg_id, a.tries);
            self.routes.remove(&dest);
            self.pending.retain(|p| p.target != dest);
            let mut token = [0u8; 16];
            token[..8].copy_from_slice(&(msg_id ^ now_ms.rotate_left(17)).to_le_bytes());
            token[8] = tries;
            if let Ok((_, more)) = self.send_typed(ptype, dest, &payload, token, now_ms) {
                for m in more {
                    let _ = out.push(m);
                }
            }
            i += 1;
        }
        // Ubesvarede ruteforespørgsler gentages med et nyt token, så
        // repeaternes dublet-filter ikke stopper dem.
        let mut i = 0;
        while i < self.pending.len() {
            if self.pending[i].retry_at_ms > now_ms {
                i += 1;
                continue;
            }
            let p = self.pending.swap_remove(i);
            if p.tries >= PATH_RETRIES {
                // Opgiv: modtageren kan ikke findes nu. Beskeder i køen til den
                // droppes, så køen ikke fyldes op (appen kan bruge postkassen).
                self.queue.retain(|q| q.dest != p.target);
                continue;
            }
            let mut token = p.token;
            token.0[15] = token.0[15].wrapping_add(1);
            let mut old = p.old_tokens.clone();
            if old.is_full() {
                old.remove(0);
            }
            let _ = old.push(p.token);
            self.emit_request(p.target, token, old, now_ms, p.tries + 1, &mut out);
        }
        let mut i = 0;
        while i < self.forwards.len() {
            if self.forwards[i].deadline_ms > now_ms {
                i += 1;
                continue;
            }
            if self.forwards[i].tries >= HOP_RETRIES {
                self.forwards.swap_remove(i);
                continue;
            }
            let f = &mut self.forwards[i];
            f.tries += 1;
            self.stats.hop_retries += 1;
            // Lidt forskellig ventetid for hver pakke og hvert forsøg, så to
            // repeatere ikke bliver ved med at kollidere i samme takt.
            let jitter = (f.id.rotate_left(f.tries as u32 * 7) % 1000) * self.hop_timeout_ms as u64 / 1000;
            f.deadline_ms = now_ms + self.hop_timeout_ms as u64 + jitter;
            let _ = out.push(Action::Transmit {
                packet: f.packet.clone(),
                delay_ms: 0,
                id: f.id,
            });
            i += 1;
        }
        out
    }

    // ---------- afsendelse ----------

    /// Sender en direkte besked. Kendes ingen rute, sættes beskeden i kø, og
    /// der sendes en ruteforespørgsel med `req_token` (16 tilfældige bytes).
    ///
    /// Returnerer beskedens ID, som senere kommer tilbage i `Action::Acked`.
    pub fn send_direct(
        &mut self,
        dest: Address,
        payload: &[u8],
        req_token: [u8; 16],
        now_ms: u64,
    ) -> Result<(u64, Actions), SendError> {
        self.send_typed(PacketType::Data, dest, payload, req_token, now_ms)
    }

    /// Som `send_direct`, men med valgfri pakketype (`Data`, `Fragment`,
    /// `Deposit`). Kvitteres fra ende til ende. Kun `Data` kan rejse med i
    /// ruteforespørgslen.
    pub fn send_typed(
        &mut self,
        ptype: PacketType,
        dest: Address,
        payload: &[u8],
        req_token: [u8; 16],
        now_ms: u64,
    ) -> Result<(u64, Actions), SendError> {
        if payload.len() > MAX_DIRECT_PAYLOAD {
            return Err(SendError::TooLarge);
        }
        let msg_id = packet_id(ptype, &dest, payload);
        let mut out = Actions::new();
        if self.mobile
            && self
                .routes
                .get(&dest)
                .is_some_and(|r| r.learned_ms + MOBILE_ROUTE_MS < now_ms)
        {
            // En mobil node har flyttet sig: find en frisk rute.
            self.routes.remove(&dest);
        }
        if let Some(route) = self.routes.get(&dest) {
            let path = route.path.clone();
            self.routes.touch(&dest, now_ms);
            self.send_routed_on(ptype, dest, payload, path, now_ms, &mut out);
        } else {
            let payload = Buf::from_slice(payload).map_err(|_| SendError::TooLarge)?;
            let dup = self
                .queue
                .iter()
                .any(|q| q.dest == dest && q.ptype == ptype && q.payload == payload);
            if !dup {
                self.queue
                    .push(Queued {
                        ptype,
                        dest,
                        payload,
                    })
                    .map_err(|_| SendError::QueueFull)?;
            }
            if !self.pending.iter().any(|p| p.target == dest) {
                self.request_path(dest, req_token, now_ms, &mut out);
            }
        }
        Ok((msg_id, out))
    }

    /// Sender et fragment (fx en bid af en fil) ad en kendt rute. Det
    /// kvitteres fra ende til ende ligesom en direkte besked.
    pub fn send_fragment(
        &mut self,
        dest: Address,
        payload: &[u8],
        now_ms: u64,
    ) -> Result<(u64, Actions), SendError> {
        if payload.len() > MAX_DIRECT_PAYLOAD {
            return Err(SendError::TooLarge);
        }
        let route = self.routes.get(&dest).ok_or(SendError::NoRoute)?;
        let path = route.path.clone();
        self.routes.touch(&dest, now_ms);
        let mut out = Actions::new();
        self.send_routed_on(PacketType::Fragment, dest, payload, path, now_ms, &mut out);
        Ok((packet_id(PacketType::Fragment, &dest, payload), out))
    }

    /// Kender vi en rute til `dest`?
    pub fn has_route(&self, dest: &Address) -> bool {
        self.routes.get(dest).is_some()
    }

    /// Sender en ny ruteforespørgsel, fx hvis den første ikke blev besvaret.
    pub fn retry_path(&mut self, dest: Address, req_token: [u8; 16], now_ms: u64) -> Actions {
        let mut out = Actions::new();
        self.pending.retain(|p| p.target != dest);
        self.request_path(dest, req_token, now_ms, &mut out);
        out
    }

    fn request_path(&mut self, dest: Address, token: [u8; 16], now_ms: u64, out: &mut Actions) {
        if self.pending.is_full() {
            self.pending.remove(0);
        }
        self.emit_request(dest, Address(token), heapless::Vec::new(), now_ms, 0, out);
    }

    fn emit_request(
        &mut self,
        dest: Address,
        token: Address,
        old_tokens: heapless::Vec<Address, 2>,
        now_ms: u64,
        tries: u8,
        out: &mut Actions,
    ) {
        self.stats.path_requests += 1;
        self.stats.originated += 1;
        // Den første besked i køen sendes med, hvis den kan være der, når
        // stien er fuld (12 hop).
        let mut payload = Buf::new();
        let _ = payload.extend_from_slice(&token.0);
        let mut carried = None;
        if let Some(q) = self
            .queue
            .iter()
            .find(|q| q.dest == dest && q.ptype == PacketType::Data)
            && 16 + q.payload.len() + stamp::STAMP_LEN <= MAX_DIRECT_PAYLOAD
            && payload.extend_from_slice(&q.payload).is_ok()
        {
            carried = Some(packet_id(PacketType::Data, &dest, &q.payload));
        }
        let _ = self.pending.push(PendingRequest {
            token,
            old_tokens,
            target: dest,
            carried,
            retry_at_ms: now_ms + PATH_RETRY_MS,
            tries,
        });
        // Arbejdsbevis bundet til målet og tokenet (ca. 256 forsøg i snit).
        stamp::append(PacketType::PathRequest, &dest, &mut payload);
        let header = Header::new(PacketType::PathRequest, dest).with_path(Path::new());
        self.emit(&header, &payload, 0, out);
    }

    fn send_routed_on(
        &mut self,
        ptype: PacketType,
        dest: Address,
        payload: &[u8],
        path: Path,
        now_ms: u64,
        out: &mut Actions,
    ) {
        self.stats.originated += 1;
        let msg_id = packet_id(ptype, &dest, payload);
        if let Some(a) = self.awaiting.iter_mut().find(|a| a.msg_id == msg_id) {
            a.sent_ms = now_ms;
        } else {
            if self.awaiting.is_full() {
                self.awaiting.remove(0);
            }
            let _ = self.awaiting.push(AwaitAck {
                token: ack_token(msg_id),
                msg_id,
                ptype,
                dest,
                payload: Buf::from_slice(payload).unwrap_or_default(),
                sent_ms: now_ms,
                tries: 0,
            });
        }
        let header = Header::new(ptype, dest).with_path(path);
        self.emit_routed(&header, payload, 0, now_ms, out);
    }

    /// Sender en besked til en kanal (floodes lokalt).
    pub fn send_channel(&mut self, channel: Address, payload: &[u8]) -> Result<Actions, SendError> {
        self.send_channel_in(channel, 0, payload)
    }

    /// Sender en kanalbesked mærket med en region (0 = ingen).
    pub fn send_channel_in(
        &mut self,
        channel: Address,
        region: u16,
        payload: &[u8],
    ) -> Result<Actions, SendError> {
        self.stats.originated += 1;
        let header = Header::new(PacketType::Channel, channel).with_region(region);
        if payload.len() > header.max_payload() {
            return Err(SendError::TooLarge);
        }
        let mut out = Actions::new();
        self.emit(&header, payload, 0, &mut out);
        Ok(out)
    }

    // ---------- modtagelse ----------

    /// Behandler en modtaget pakke.
    pub fn on_receive(&mut self, buf: &[u8], snr_db: i8, now_ms: u64) -> Actions {
        let mut out = Actions::new();
        let Ok(pkt) = Packet::decode(buf) else {
            self.stats.rx_invalid += 1;
            return out;
        };
        // Announces og ruteforespørgsler uden gyldigt arbejdsbevis behandles
        // ikke og sendes ikke videre. Det koster én hash at tjekke.
        if stamp::required(pkt.header.ptype)
            && stamp::verify(pkt.header.ptype, &pkt.header.dest, pkt.payload).is_err()
        {
            self.stats.bad_stamps += 1;
            return out;
        }
        self.stats.rx_packets += 1;
        let id = packet_id(pkt.header.ptype, &pkt.header.dest, pkt.payload);
        match (&pkt.header.ptype, &pkt.header.path) {
            (PacketType::PathRequest, _) => {
                if self.dedup.insert(id) {
                    self.on_path_request(&pkt, snr_db, now_ms, &mut out);
                } else {
                    self.stats.rx_duplicates += 1;
                    if pkt.header.dest == self.addr {
                        self.consider_reply(&pkt, snr_db);
                    }
                    self.on_duplicate_flood(id, snr_db, &mut out);
                }
            }
            (_, Some(path)) => {
                // Hører vi næste repeater sende vores pakke videre, er hoppet lykkedes.
                let hops = pkt.header.hops;
                self.forwards.retain(|f| !(f.id == id && hops > f.hops));
                let path = path.clone();
                self.on_source_routed(&pkt, &path, id, now_ms, &mut out);
            }
            (_, None) => {
                if self.dedup.insert(id) {
                    self.on_flood(&pkt, snr_db, &mut out);
                } else {
                    self.stats.rx_duplicates += 1;
                    self.on_duplicate_flood(id, snr_db, &mut out);
                }
            }
        }
        for a in &out {
            match a {
                Action::Deliver { .. } => self.stats.delivered += 1,
                Action::Acked { .. } => self.stats.acked += 1,
                Action::RouteLearned { .. } => self.stats.routes_learned += 1,
                Action::Cancel { .. } => self.stats.floods_cancelled += 1,
                Action::Transmit { .. } => {}
            }
        }
        out
    }

    /// En kopi mere af en ruteforespørgsel til os. Er dens spor bedre end det
    /// hidtil bedste, huskes det i stedet.
    ///
    /// Først undgås svage sidste spring (de vandt, fordi de kom længst, men
    /// svaret tilbage går ofte tabt). Derefter færrest hop og bedst SNR.
    fn consider_reply(&mut self, pkt: &Packet, snr_db: i8) {
        let Some(token) = Address::from_slice(pkt.payload) else { return };
        let weak = self.weak_snr_db;
        let Some(r) = self.replies.iter_mut().find(|r| r.token == token) else { return };
        let path = pkt.header.path.clone().unwrap_or_default();
        let score = |snr: i8, hops: usize| ((snr < weak) as u8, hops, -(snr as i16));
        if score(snr_db, path.len()) < score(r.best_snr, r.best.len()) {
            r.best = path;
            r.best_snr = snr_db;
        }
    }

    fn on_path_request(&mut self, pkt: &Packet, snr_db: i8, now_ms: u64, out: &mut Actions) {
        let h = &pkt.header;
        let record = h.path.clone().unwrap_or_default();
        if h.dest == self.addr {
            // Vi er målet: svar til tokenet ad den omvendte sti, og send
            // den fundne sti med i payloaden.
            let Some(token) = Address::from_slice(pkt.payload) else {
                return;
            };
            // Indeholder forespørgslen en besked, leveres den nu. Svaret
            // fungerer så også som kvittering.
            let data = &stamp::strip(PacketType::PathRequest, pkt.payload)[16..];
            if !data.is_empty()
                && self
                    .dedup
                    .insert(packet_id(PacketType::Data, &self.addr, data))
            {
                let _ = out.push(Action::Deliver {
                    ptype: PacketType::Data,
                    dest: self.addr,
                    payload: Buf::from_slice(data).unwrap_or_default(),
                    hops: h.hops,
                });
            }
            // Svar ikke med det samme: vent på flere kopier ad andre veje og
            // vælg den bedste (se `consider_reply`).
            if self.replies.iter().any(|r| r.token == token) {
                return;
            }
            if self.replies.is_full() {
                self.replies.remove(0);
            }
            let _ = self.replies.push(PendingReply {
                token,
                best: record,
                best_snr: snr_db,
                reply_at_ms: now_ms + (RESPONSE_WAIT_SLOTS * self.slot_ms) as u64,
            });
            return;
        }
        if !self.may_forward(h.hops) || h.hops >= MAX_HOPS {
            return;
        }
        let mut record = record;
        if record.push(self.id).is_err() {
            return; // Længere end en kildesti kan bære.
        }
        let mut header = Header::new(PacketType::PathRequest, h.dest).with_path(record);
        header.hops = h.hops + 1;
        let delay = self.flood_delay_ms(snr_db);
        self.stats.forwarded += 1;
        self.emit_flood(&header, pkt.payload, delay, out);
    }

    fn on_source_routed(
        &mut self,
        pkt: &Packet,
        path: &Path,
        id: u64,
        now_ms: u64,
        out: &mut Actions,
    ) {
        let h = &pkt.header;
        let idx = h.hops as usize;
        if idx < path.len() {
            // Er vi næste repeater på stien? En kopi, vi har set før, sendes
            // også videre: forrige hop gensender kun, når det ikke hørte os.
            if path[idx] != self.id || !self.may_forward(h.hops) {
                return;
            }
            self.dedup.insert(id);
            let mut header = h.clone();
            header.hops += 1;
            self.stats.forwarded += 1;
            self.emit_routed(&header, pkt.payload, SOURCE_ROUTE_DELAY_MS, now_ms, out);
            return;
        }
        if idx != path.len() {
            return;
        }
        // Sidste stræk: er pakken til os?
        match h.ptype {
            PacketType::PathResponse => {
                let Some(i) = self
                    .pending
                    .iter()
                    .position(|p| p.token == h.dest || p.old_tokens.contains(&h.dest))
                else {
                    return;
                };
                if !self.dedup.insert(id) {
                    return;
                }
                let Some(forward) = parse_path(pkt.payload) else {
                    return;
                };
                let req = self.pending.swap_remove(i);
                if let Some(msg_id) = req.carried {
                    self.awaiting.retain(|a| a.msg_id != msg_id);
                    // Beskeden kom frem med forespørgslen: fjern den fra køen.
                    let target = req.target;
                    self.queue
                        .retain(|q| packet_id(q.ptype, &target, &q.payload) != msg_id);
                    let _ = out.push(Action::Acked { msg_id });
                }
                self.learn_route(req.target, forward, now_ms, out);
            }
            PacketType::Ack => {
                let Some(i) = self.awaiting.iter().position(|a| a.token == h.dest) else {
                    return;
                };
                if !self.dedup.insert(id) {
                    return;
                }
                let msg_id = self.awaiting.remove(i).msg_id;
                let _ = out.push(Action::Acked { msg_id });
            }
            _ if h.dest == self.addr => {
                // En gentaget besked leveres ikke igen, men kvitteres igen,
                // for den første kvittering kan være gået tabt.
                if self.dedup.insert(id) {
                    deliver(pkt, out);
                }
                if matches!(
                    h.ptype,
                    PacketType::Data | PacketType::Fragment | PacketType::Deposit
                ) {
                    let header =
                        Header::new(PacketType::Ack, ack_token(id)).with_path(reversed(path));
                    self.emit_routed(&header, &[], SOURCE_ROUTE_DELAY_MS, now_ms, out);
                }
            }
            _ => {}
        }
    }

    fn learn_route(&mut self, target: Address, path: Path, now_ms: u64, out: &mut Actions) {
        let hops = path.len() as u8;
        self.routes.upsert(Route {
            dest: target,
            cost: hops as u16,
            path: path.clone(),
            last_seen_ms: now_ms,
            learned_ms: now_ms,
        });
        let _ = out.push(Action::RouteLearned { dest: target, hops });
        // Send beskeder, der ventede på ruten.
        let mut i = 0;
        while i < self.queue.len() {
            if self.queue[i].dest == target {
                let q = self.queue.swap_remove(i);
                self.send_routed_on(q.ptype, q.dest, &q.payload, path.clone(), now_ms, out);
            } else {
                i += 1;
            }
        }
    }

    fn on_flood(&mut self, pkt: &Packet, snr_db: i8, out: &mut Actions) {
        let h = &pkt.header;
        // Announces går altid til appen, så den kan verificere og huske nøglen.
        if h.dest == self.addr || self.channels.contains(&h.dest) || h.ptype == PacketType::Announce
        {
            deliver(pkt, out);
        }
        if !self.may_forward(h.hops) || h.dest == self.addr {
            return;
        }
        let local = matches!(
            h.ptype,
            PacketType::Channel | PacketType::Announce | PacketType::Probe
        );
        // Ved lavt batteri videresendes kun det vigtigste.
        let allowed = match self.energy {
            Energy::Full | Energy::Ok => true,
            Energy::Low => !local,
            Energy::Critical => false,
        };
        let scope = if local { LOCAL_SCOPE } else { MAX_HOPS };
        if !allowed || h.hops >= scope || !self.forwards_region(h.region) {
            return;
        }
        let mut header = h.clone();
        header.hops += 1;
        let delay = self.flood_delay_ms(snr_db);
        self.stats.forwarded += 1;
        self.emit_flood(&header, pkt.payload, delay, out);
    }
}

/// Leverer pakken til appen. Et stempel fjernes først, så appen ser samme
/// payload, som afsenderen lavede (fx et signeret announce).
fn deliver(pkt: &Packet, out: &mut Actions) {
    let payload = stamp::strip(pkt.header.ptype, pkt.payload);
    let _ = out.push(Action::Deliver {
        ptype: pkt.header.ptype,
        dest: pkt.header.dest,
        payload: Buf::from_slice(payload).unwrap_or_default(),
        hops: pkt.header.hops,
    });
}

fn parse_path(payload: &[u8]) -> Option<Path> {
    let n = *payload.first()? as usize;
    if n > MAX_PATH || payload.len() < 1 + 2 * n {
        return None;
    }
    let mut p = Path::new();
    for c in payload[1..1 + 2 * n].chunks_exact(2) {
        let _ = p.push(u16::from_be_bytes([c[0], c[1]]));
    }
    Some(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    type R = Router<32, 64>;

    fn node(n: u8, role: Role) -> R {
        let mut a = [n; 16];
        a[0] = 0;
        a[1] = n; // sti-ID = n
        R::new(Address(a), role)
    }

    /// Lader `n` vente det tidsslot, hvor den samler kopier af en
    /// ruteforespørgsel, og returnerer det, den så sender.
    fn reply(n: &mut R, now: u64) -> Vec<Buf> {
        let d = n.next_deadline().expect("venter på at svare");
        assert!(d >= now);
        transmits(&n.tick(d))
    }

    fn transmits(actions: &Actions) -> Vec<Buf> {
        actions
            .iter()
            .filter_map(|a| match a {
                Action::Transmit { packet, .. } => Some(packet.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn drops_packets_without_valid_stamp() {
        let mut r = node(2, Role::Repeater);
        let dest = Address([5; 16]);
        // Ruteforespørgsel uden stempel.
        let h = Header::new(PacketType::PathRequest, dest).with_path(Path::new());
        let out = r.on_receive(&encode(&h, &[9u8; 16]).unwrap(), 0, 0);
        assert!(out.is_empty());
        // Announce med forkert stempel (rigtigt stempel, men andet tidsstempel).
        let mut body: Buf = Buf::from_slice(&[1u8; 69]).unwrap();
        assert!(stamp::append(PacketType::Announce, &dest, &mut body));
        body[67] ^= 0xff;
        let h = Header::new(PacketType::Announce, dest);
        let out = r.on_receive(&encode(&h, &body).unwrap(), 0, 0);
        assert!(out.is_empty());
        assert_eq!(r.stats.bad_stamps, 2);
        assert_eq!(r.stats.rx_packets, 0);
        assert_eq!(r.stats.forwarded, 0);
    }

    #[test]
    fn stamped_announce_is_delivered_without_stamp_and_forwarded_with_it() {
        let mut r = node(2, Role::Repeater);
        let dest = Address([5; 16]);
        let body: Buf = Buf::from_slice(&[1u8; 140]).unwrap();
        let mut stamped = body.clone();
        assert!(stamp::append(PacketType::Announce, &dest, &mut stamped));
        let pkt = encode(&Header::new(PacketType::Announce, dest), &stamped).unwrap();
        let out = r.on_receive(&pkt, 0, 0);
        assert!(out.iter().any(
            |x| matches!(x, Action::Deliver { payload, .. } if payload.as_slice() == body.as_slice())
        ));
        let fwd = transmits(&out);
        assert_eq!(fwd.len(), 1);
        assert_eq!(Packet::decode(&fwd[0]).unwrap().payload, stamped.as_slice());
        assert_eq!(r.stats.bad_stamps, 0);
    }

    /// Kæde: A (klient) - R1 - R2 - B (klient). Hver hører kun sine naboer.
    #[test]
    fn finds_route_delivers_and_acks_over_two_repeaters() {
        let mut a = node(1, Role::Client);
        let mut r1 = node(2, Role::Repeater);
        let mut r2 = node(3, Role::Repeater);
        let mut b = node(4, Role::Client);

        let (first_id, out) = a.send_direct(b.addr, b"hej B", [7; 16], 0).unwrap();
        let req = transmits(&out);
        assert_eq!(req.len(), 1);

        // Forespørgslen (med første besked) floodes A -> R1 -> R2 -> B.
        let f1 = transmits(&r1.on_receive(&req[0], 5, 0));
        let f2 = transmits(&r2.on_receive(&f1[0], 5, 0));
        let at_b = b.on_receive(&f2[0], 5, 0);
        assert!(at_b.iter().any(|x| matches!(x,
            Action::Deliver { ptype: PacketType::Data, payload, hops: 2, .. } if payload.as_slice() == b"hej B")));
        let resp = reply(&mut b, 0);

        // Svaret går B -> R2 -> R1 -> A som kildesti og er også kvittering.
        let s2 = transmits(&r2.on_receive(&resp[0], 5, 0));
        let s1 = transmits(&r1.on_receive(&s2[0], 5, 0));
        let got = a.on_receive(&s1[0], 5, 0);
        assert!(got.contains(&Action::RouteLearned {
            dest: b.addr,
            hops: 2
        }));
        assert!(got.contains(&Action::Acked { msg_id: first_id }));
        assert_eq!(a.routes().get(&b.addr).unwrap().path.as_slice(), &[2, 3]);

        // Næste besked bruger ruten: A -> R1 -> R2 -> B.
        let (msg_id, out) = a.send_direct(b.addr, b"igen", [8; 16], 10).unwrap();
        let data = transmits(&out);
        assert_eq!(
            Packet::decode(&data[0]).unwrap().header.ptype,
            PacketType::Data
        );
        let d1 = transmits(&r1.on_receive(&data[0], 5, 0));
        let d2 = transmits(&r2.on_receive(&d1[0], 5, 0));
        let at_b = b.on_receive(&d2[0], 5, 0);
        assert!(at_b.iter().any(|x| matches!(x,
            Action::Deliver { ptype: PacketType::Data, payload, .. } if payload.as_slice() == b"igen")));

        // Kvittering B -> R2 -> R1 -> A.
        let ack = transmits(&at_b);
        let k2 = transmits(&r2.on_receive(&ack[0], 5, 0));
        let k1 = transmits(&r1.on_receive(&k2[0], 5, 0));
        assert!(
            a.on_receive(&k1[0], 5, 0)
                .contains(&Action::Acked { msg_id })
        );
    }

    #[test]
    fn first_message_rides_along_with_the_path_request() {
        let mut a = node(1, Role::Client);
        let mut r = node(2, Role::Repeater);
        let mut b = node(4, Role::Client);
        let (msg_id, out) = a.send_direct(b.addr, b"med det samme", [7; 16], 0).unwrap();
        let req = transmits(&r.on_receive(&transmits(&out)[0], 0, 0));
        let at_b = b.on_receive(&req[0], 0, 0);
        assert!(at_b.iter().any(|x| matches!(x,
            Action::Deliver { ptype: PacketType::Data, payload, .. } if payload.as_slice() == b"med det samme")));
        let resp = transmits(&r.on_receive(&reply(&mut b, 0)[0], 0, 0));
        let got = a.on_receive(&resp[0], 0, 0);
        assert!(got.contains(&Action::Acked { msg_id }));
        assert!(transmits(&got).is_empty(), "beskeden sendes ikke igen");
    }

    #[test]
    fn clients_do_not_forward() {
        let mut a = node(1, Role::Client);
        let mut c = node(2, Role::Client);
        let out = a.send_channel(Address([9; 16]), b"x").unwrap();
        assert!(transmits(&c.on_receive(&transmits(&out)[0], 0, 0)).is_empty());
        let (_, out) = a.send_direct(Address([5; 16]), b"x", [1; 16], 0).unwrap();
        assert!(transmits(&c.on_receive(&transmits(&out)[0], 0, 0)).is_empty());
    }

    /// Routerens størrelse i firmwaren (nRF52840 har 256 KB RAM). Kør med
    /// `--nocapture` for at se tallet.
    #[test]
    fn router_size_in_firmware() {
        let size = core::mem::size_of::<Router<256, 1024>>();
        std::println!("size_of::<Router<256, 1024>>() = {size} bytes");
        assert!(size < 96 * 1024);
    }

    #[test]
    fn duplicates_are_dropped() {
        let mut a = node(1, Role::Client);
        let mut r = node(2, Role::Repeater);
        let out = a.send_channel(Address([9; 16]), b"x").unwrap();
        let pkt = &transmits(&out)[0];
        assert_eq!(transmits(&r.on_receive(pkt, 0, 0)).len(), 1);
        // En svag kopi behandles ikke igen (og annullerer intet).
        assert!(r.on_receive(pkt, -15, 0).is_empty());
        // En stærk kopi behandles heller ikke igen; den annullerer kun.
        assert!(transmits(&r.on_receive(pkt, 0, 0)).is_empty());
    }

    #[test]
    fn channel_delivered_to_subscribers_and_limited_in_scope() {
        let ch = Address([9; 16]);
        let mut a = node(1, Role::Client);
        let mut r = node(2, Role::Repeater);
        r.subscribe(ch).unwrap();
        let out = a.send_channel(ch, b"hej kanal").unwrap();
        let mut pkt = transmits(&out)[0].clone();
        let got = r.on_receive(&pkt, 0, 0);
        assert!(got.iter().any(|x| matches!(x, Action::Deliver { .. })));
        assert_eq!(transmits(&got).len(), 1);

        // Ved grænsen videresendes ikke.
        let mut r2 = node(3, Role::Repeater);
        pkt[1] = LOCAL_SCOPE;
        assert!(transmits(&r2.on_receive(&pkt, 0, 0)).is_empty());
    }

    #[test]
    fn scoped_channel_forwarded_only_inside_region() {
        use crate::region::region_id;
        let (dk, nj) = (region_id("dk"), region_id("nordjylland"));
        let ch = Address([9; 16]);
        let mut a = node(1, Role::Client);
        let pkt = transmits(&a.send_channel_in(ch, nj, b"hej").unwrap())[0].clone();
        assert_eq!(Packet::decode(&pkt).unwrap().header.region, nj);

        // Aalborg: dk + nordjylland -> videresender, regionen bevares.
        let mut aalborg = node(2, Role::Repeater);
        aalborg.regions = crate::region::Regions::from_slice(&[dk, nj]).unwrap();
        let fwd = transmits(&aalborg.on_receive(&pkt, 0, 0));
        assert_eq!(fwd.len(), 1);
        assert_eq!(Packet::decode(&fwd[0]).unwrap().header.region, nj);

        // Aarhus: kun dk -> videresender ikke, men leverer til egen bruger.
        let mut aarhus = node(3, Role::Repeater);
        aarhus.regions = crate::region::Regions::from_slice(&[dk]).unwrap();
        aarhus.subscribe(ch).unwrap();
        let got = aarhus.on_receive(&pkt, 0, 0);
        assert!(transmits(&got).is_empty());
        assert!(got.iter().any(|x| matches!(x, Action::Deliver { .. })));
        // ... men en dk-besked og en global besked videresendes.
        let dkpkt = transmits(&a.send_channel_in(ch, dk, b"x").unwrap())[0].clone();
        assert_eq!(transmits(&aarhus.on_receive(&dkpkt, 0, 0)).len(), 1);
        let global = transmits(&a.send_channel(ch, b"y").unwrap())[0].clone();
        assert_eq!(transmits(&aarhus.on_receive(&global, 0, 0)).len(), 1);

        // Tom liste: alt videresendes; strict: kun globalt.
        let mut open = node(4, Role::Repeater);
        assert_eq!(transmits(&open.on_receive(&pkt, 0, 0)).len(), 1);
        let mut strict = node(5, Role::Repeater);
        strict.strict_region = true;
        assert!(transmits(&strict.on_receive(&pkt, 0, 0)).is_empty());
        let global2 = transmits(&a.send_channel(ch, b"z").unwrap())[0].clone();
        assert_eq!(transmits(&strict.on_receive(&global2, 0, 0)).len(), 1);
    }

    #[test]
    fn far_repeaters_forward_first_and_low_battery_waits() {
        let r = node(2, Role::Repeater);
        assert!(r.flood_delay_ms(-15) < r.flood_delay_ms(10));
        let mut low = node(3, Role::Repeater);
        low.energy = Energy::Low;
        assert!(low.flood_delay_ms(-15) > r.flood_delay_ms(-15));
    }

    #[test]
    fn low_energy_skips_channels_critical_skips_all() {
        let mut a = node(1, Role::Client);
        let mut r = node(2, Role::Repeater);
        r.energy = Energy::Low;
        let ch = transmits(&a.send_channel(Address([9; 16]), b"x").unwrap())[0].clone();
        assert!(transmits(&r.on_receive(&ch, 0, 0)).is_empty());
        let (_, out) = a.send_direct(Address([5; 16]), b"x", [1; 16], 0).unwrap();
        let req = transmits(&out)[0].clone();
        assert_eq!(transmits(&r.on_receive(&req, 0, 0)).len(), 1);

        let mut dead = node(3, Role::Repeater);
        dead.energy = Energy::Critical;
        assert!(transmits(&dead.on_receive(&req, 0, 0)).is_empty());
    }

    #[test]
    fn only_the_named_repeater_forwards_source_routed() {
        let mut r = node(2, Role::Repeater);
        let path = Path::from_slice(&[9, 2]).unwrap();
        let header = Header::new(PacketType::Data, Address([5; 16])).with_path(path);
        let pkt = encode(&header, b"x").unwrap();
        assert!(r.on_receive(&pkt, 0, 0).is_empty(), "hop 0 er repeater 9");
        let mut h2 = header.clone();
        h2.hops = 1;
        let pkt = encode(&h2, b"x").unwrap();
        let fwd = transmits(&r.on_receive(&pkt, 0, 0));
        assert_eq!(Packet::decode(&fwd[0]).unwrap().header.hops, 2);
    }

    #[test]
    fn destination_overhearing_early_still_gets_final_copy() {
        let mut b = node(4, Role::Client);
        let path = Path::from_slice(&[2, 3]).unwrap();
        let mut h = Header::new(PacketType::Data, b.addr).with_path(path);
        h.hops = 1; // B hører den, mens den er på vej til R2
        assert!(b.on_receive(&encode(&h, b"x").unwrap(), 0, 0).is_empty());
        h.hops = 2;
        let got = b.on_receive(&encode(&h, b"x").unwrap(), 0, 0);
        assert!(got.iter().any(|x| matches!(x, Action::Deliver { .. })));
    }

    #[test]
    fn flood_is_cancelled_after_hearing_enough_copies() {
        let mut a = node(1, Role::Client);
        let mut r = node(2, Role::Repeater);
        let pkt = transmits(&a.send_channel(Address([9; 16]), b"x").unwrap())[0].clone();
        let out = r.on_receive(&pkt, 0, 0);
        let (fwd, id) = match &out[0] {
            Action::Transmit { packet, id, .. } => (packet.clone(), *id),
            _ => panic!("forventede videresendelse"),
        };
        // En svag kopi kommer langvejsfra: den dækker ikke vores område.
        assert!(r.on_receive(&fwd, -15, 0).is_empty());
        // En stærk kopi fra en nær nabo: området er dækket, annullér.
        assert_eq!(
            r.on_receive(&fwd, 0, 0).as_slice(),
            &[Action::Cancel { id }]
        );
    }

    #[test]
    fn hop_is_retransmitted_until_next_repeater_is_heard() {
        let mut r = node(2, Role::Repeater);
        let header = Header::new(PacketType::Data, Address([5; 16]))
            .with_path(Path::from_slice(&[2, 3]).unwrap());
        let pkt = encode(&header, b"x").unwrap();
        assert_eq!(transmits(&r.on_receive(&pkt, 0, 0)).len(), 1);
        let d = r.next_deadline().expect("venter på næste hop");
        assert!(r.tick(d - 1).is_empty());
        assert_eq!(transmits(&r.tick(d)).len(), 1, "gensendes");
        // Nu hører vi repeater 3 sende videre.
        let mut h2 = header.clone();
        h2.hops = 2;
        r.on_receive(&encode(&h2, b"x").unwrap(), 0, d + 10);
        assert!(r.next_deadline().is_none());
    }

    #[test]
    fn hop_retries_give_up() {
        let mut r = node(2, Role::Repeater);
        let header = Header::new(PacketType::Data, Address([5; 16]))
            .with_path(Path::from_slice(&[2, 3]).unwrap());
        r.on_receive(&encode(&header, b"x").unwrap(), 0, 0);
        let mut sent = 0;
        while let Some(d) = r.next_deadline() {
            sent += transmits(&r.tick(d)).len();
        }
        assert_eq!(sent, HOP_RETRIES as usize);
    }

    #[test]
    fn last_hop_is_not_tracked() {
        let mut r = node(3, Role::Repeater);
        let mut header = Header::new(PacketType::Data, Address([5; 16]))
            .with_path(Path::from_slice(&[2, 3]).unwrap());
        header.hops = 1;
        r.on_receive(&encode(&header, b"x").unwrap(), 0, 0);
        assert!(r.next_deadline().is_none());
    }

    #[test]
    fn repeated_message_is_acked_again_but_delivered_once() {
        let mut b = node(4, Role::Client);
        let header = Header::new(PacketType::Data, b.addr).with_path(Path::new());
        let pkt = encode(&header, b"x").unwrap();
        let first = b.on_receive(&pkt, 0, 0);
        let second = b.on_receive(&pkt, 0, 0);
        assert!(first.iter().any(|x| matches!(x, Action::Deliver { .. })));
        assert!(!second.iter().any(|x| matches!(x, Action::Deliver { .. })));
        assert_eq!(transmits(&second).len(), 1, "ny kvittering");
    }

    #[test]
    fn relay_forwards_within_its_limit_and_waits_longer() {
        let mut a = node(1, Role::Client);
        let mut relay = node(2, Role::Relay);
        let rep = node(3, Role::Repeater);
        assert!(
            relay.flood_delay_ms(0) > rep.flood_delay_ms(0),
            "rigtige repeatere vinder"
        );
        let mut pkt = transmits(&a.send_channel(Address([9; 16]), b"x").unwrap())[0].clone();
        assert_eq!(transmits(&relay.on_receive(&pkt, 0, 0)).len(), 1);
        let mut relay2 = node(4, Role::Relay);
        pkt[1] = DEFAULT_RELAY_HOPS; // allerede 4 hop
        assert!(transmits(&relay2.on_receive(&pkt, 0, 0)).is_empty());
    }

    #[test]
    fn fragments_need_a_route_and_are_acked() {
        let mut a = node(1, Role::Client);
        let mut b = node(4, Role::Client);
        assert_eq!(
            a.send_fragment(b.addr, b"bid", 0).err(),
            Some(SendError::NoRoute)
        );
        // Lær en direkte rute (naboer, tom sti).
        let (_, out) = a.send_direct(b.addr, b"tilbud", [1; 16], 0).unwrap();
        b.on_receive(&transmits(&out)[0], 0, 0);
        let resp = reply(&mut b, 0);
        a.on_receive(&resp[0], 0, 0);
        assert!(a.has_route(&b.addr));
        let (id, out) = a.send_fragment(b.addr, b"bid 0", 0).unwrap();
        let at_b = b.on_receive(&transmits(&out)[0], 0, 0);
        assert!(at_b.iter().any(|x| matches!(
            x,
            Action::Deliver {
                ptype: PacketType::Fragment,
                ..
            }
        )));
        let ack = transmits(&at_b);
        assert!(
            a.on_receive(&ack[0], 0, 0)
                .contains(&Action::Acked { msg_id: id })
        );
    }

    #[test]
    fn unanswered_path_request_is_retried_with_new_token() {
        let mut a = node(1, Role::Client);
        let (_, out) = a.send_direct(Address([5; 16]), b"x", [1; 16], 0).unwrap();
        let first = transmits(&out)[0].clone();
        let d = a.next_deadline().unwrap();
        assert_eq!(d, PATH_RETRY_MS);
        let again = transmits(&a.tick(d));
        assert_eq!(again.len(), 1);
        assert_ne!(again[0], first, "nyt token");
        let d2 = a.next_deadline().unwrap();
        assert_eq!(transmits(&a.tick(d2)).len(), 1);
        let d3 = a.next_deadline().unwrap();
        assert!(transmits(&a.tick(d3)).is_empty());
        assert!(
            a.next_deadline().is_none(),
            "opgiver efter {PATH_RETRIES} gentagelser"
        );
        // Samme besked igen: ingen dublet i køen, men en ny forespørgsel.
        let (_, out) = a
            .send_direct(Address([5; 16]), b"x", [2; 16], 100_000)
            .unwrap();
        assert_eq!(transmits(&out).len(), 1);
    }

    #[test]
    fn statistics_are_counted() {
        let mut a = node(1, Role::Client);
        let mut r = node(2, Role::Repeater);
        let out = a.send_channel(Address([9; 16]), b"x").unwrap();
        let pkt = transmits(&out)[0].clone();
        r.on_receive(&pkt, 0, 0);
        r.on_receive(&pkt, 0, 0);
        r.on_receive(&[1, 2, 3], 0, 0);
        assert_eq!(a.stats.originated, 1);
        assert_eq!(r.stats.rx_packets, 2);
        assert_eq!(r.stats.rx_duplicates, 1);
        assert_eq!(r.stats.rx_invalid, 1);
        assert_eq!(r.stats.forwarded, 1);
    }

    #[test]
    fn missing_ack_repairs_route_and_resends() {
        let mut a = node(1, Role::Client);
        let mut b = node(4, Role::Client);
        // Lær en rute (direkte naboer) med første besked.
        let (_, out) = a.send_direct(b.addr, b"1", [1; 16], 0).unwrap();
        b.on_receive(&transmits(&out)[0], 0, 0);
        let resp = reply(&mut b, 0);
        a.on_receive(&resp[0], 0, 0);
        assert!(a.has_route(&b.addr));
        // Næste besked sendes ad ruten, men B er flyttet og svarer ikke.
        let (_, out) = a.send_direct(b.addr, b"2", [2; 16], 1000).unwrap();
        assert_eq!(
            Packet::decode(&transmits(&out)[0]).unwrap().header.ptype,
            PacketType::Data
        );
        let t = 1000 + DEFAULT_ACK_TIMEOUT_MS as u64;
        let again = transmits(&a.tick(t));
        assert!(!a.has_route(&b.addr), "ruten er glemt");
        let req = again
            .iter()
            .find(|p| Packet::decode(p).unwrap().header.ptype == PacketType::PathRequest);
        let req = req.expect("ny ruteforespørgsel med beskeden");
        // B (nu et andet sted) modtager forespørgslen, får beskeden og svarer.
        let at_b = b.on_receive(req, 0, t);
        assert!(
            at_b.iter().any(
                |x| matches!(x, Action::Deliver { payload, .. } if payload.as_slice() == b"2")
            )
        );
        let got = a.on_receive(&reply(&mut b, t)[0], 0, t);
        assert!(got.iter().any(|x| matches!(x, Action::Acked { .. })));
        assert!(a.next_deadline().is_none(), "intet mere at vente på");
    }

    #[test]
    fn mobile_node_refreshes_old_routes() {
        let mut a = node(1, Role::Client);
        a.mobile = true;
        let mut b = node(4, Role::Client);
        let (_, out) = a.send_direct(b.addr, b"1", [1; 16], 0).unwrap();
        b.on_receive(&transmits(&out)[0], 0, 0);
        let resp = reply(&mut b, 0);
        a.on_receive(&resp[0], 0, 0);
        let (_, out) = a.send_direct(b.addr, b"2", [2; 16], 10_000).unwrap();
        assert_eq!(
            Packet::decode(&transmits(&out)[0]).unwrap().header.ptype,
            PacketType::Data
        );
        let (_, out) = a
            .send_direct(b.addr, b"3", [3; 16], MOBILE_ROUTE_MS + 1)
            .unwrap();
        assert_eq!(
            Packet::decode(&transmits(&out)[0]).unwrap().header.ptype,
            PacketType::PathRequest
        );
    }

    #[test]
    fn destination_prefers_a_strong_last_hop() {
        let mut b = node(4, Role::Client);
        let token = [9u8; 16];
        let dest = b.addr;
        let mk = |path: &[u16]| {
            let mut h = Header::new(PacketType::PathRequest, dest).with_path(Path::from_slice(path).unwrap());
            h.hops = path.len() as u8;
            let mut p: Buf = Buf::from_slice(&token).unwrap();
            assert!(stamp::append(PacketType::PathRequest, &dest, &mut p));
            encode(&h, &p).unwrap()
        };
        // Første kopi: 1 hop, men næsten ved støjgrænsen.
        b.on_receive(&mk(&[7]), -11, 0);
        // Anden kopi: 2 hop med et stærkt sidste spring.
        b.on_receive(&mk(&[5, 6]), 5, 10);
        let resp = reply(&mut b, 10);
        let p = Packet::decode(&resp[0]).unwrap();
        assert_eq!(p.header.path.unwrap().as_slice(), &[6, 5]);
    }

    #[test]
    fn rejects_oversized_direct_message() {
        let mut a = node(1, Role::Client);
        let big = [0u8; MAX_DIRECT_PAYLOAD + 1];
        assert_eq!(
            a.send_direct(Address([5; 16]), &big, [0; 16], 0).err(),
            Some(SendError::TooLarge)
        );
    }
}
