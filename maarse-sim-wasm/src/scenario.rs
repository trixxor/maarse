//! Scenario format (JSON). The web UI edits this; the engine is built from it.
//! See `docs/denmark/README.md` for the full description.

use serde::{Deserialize, Serialize};

fn yes() -> bool {
    true
}
fn d_sf() -> u8 {
    9
}
fn d_duty() -> u32 {
    100
}
fn d_tx() -> f64 {
    20.0
}
fn d_dur() -> u64 {
    1800
}
fn d_local() -> f64 {
    0.8
}
fn d_relay_hops() -> u8 {
    4
}
fn d_auto_relay_s() -> f64 {
    600.0
}
fn d_pinned() -> usize {
    32
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Scenario {
    pub name: String,
    /// The host's rules (post office, files, Pages, tunnel); default
    /// "Standard (as built)" from maarse-hostrules.
    pub rules: maarse_hostrules::rules::Rules,
    pub seed: u64,
    /// Default spreading factor (a node can override it).
    #[serde(default = "d_sf")]
    pub sf: u8,
    /// Duty cycle budget in permille (100 = 10 %, the EU 869.4-869.65 MHz band).
    #[serde(default = "d_duty")]
    pub duty_permille: u32,
    #[serde(default = "d_tx")]
    pub tx_dbm: f64,
    /// Carrier frequency (MHz) and bandwidth (Hz) of every node. 0 = 868 MHz / 125 kHz.
    pub freq_mhz: f64,
    pub bw_hz: u32,
    /// Radio model calibration from field measurements
    /// (tools/calibrate_radio.py): path loss exponent (0 = 3) and offset.
    pub radio_cal: RadioCal,
    /// Length of a "run to end" (comparisons, headless test), seconds.
    #[serde(default = "d_dur")]
    pub duration_s: u64,
    pub features: Features,
    pub nodes: Vec<NodeSpec>,
    pub links: Vec<LinkSpec>,
    pub traffic: Traffic,
    pub events: Vec<TimedEvent>,
    /// Areas with extra radio loss, e.g. forest: a link loses `db_per_km` for
    /// every km of its straight line that runs inside the area.
    pub clutter: Vec<Clutter>,
    /// "canopy" (default, see [`Clutter`]) or "linear" (the first model:
    /// every km inside the area costs `db_per_km`, heights ignored).
    #[serde(default)]
    pub clutter_model: String,
    /// Every node announces every `announce_s` seconds (the firmware default
    /// is 12 h): the first time at a random moment within the interval, then
    /// every interval ±10 %. 0 = no periodic announces. A rebooted node
    /// announces 5-35 s after the start (at random) and then every 15 min
    /// until it hears a repeater or an anchor, as the firmware does.
    pub announce_s: f64,
    /// How many hops a relay forwards floods to (the router's `relay_hops`,
    /// default 4, at most 12).
    #[serde(default = "d_relay_hops")]
    pub relay_hops: u8,
    /// Automatic forest mode: seconds without a repeater before a handheld
    /// becomes a relay (default 600).
    #[serde(default = "d_auto_relay_s")]
    pub auto_relay_after_s: f64,
    /// Contact table size per node (0 = unlimited, every heard node is known).
    /// With a limit, the contact heard longest ago is evicted when the table
    /// is full (the firmware keeps 64 in RAM and 63 in flash).
    pub contact_capacity: usize,
    /// Pinned contacts per node (with `pin_contacts`), default 32.
    #[serde(default = "d_pinned")]
    pub pinned_capacity: usize,
    /// First announce of every node within this many seconds (0 = within one
    /// announce interval): a quick "everyone has announced once" warm-up.
    pub announce_warmup_s: f64,
    /// Automatic forest mode, edge bridges (with `features.auto_edge`):
    /// "select" = one bridge per stretch (a client defers to a bridge it
    /// hears, lower ID stays); "mpr" = bridge only for a neighbor without
    /// coverage that reports no bridge yet (a bridge stays while one reports
    /// fewer than two), after OLSR multipoint relays.
    pub auto_bridge: String,
    /// Edge bridges forward path requests only across the edge (from a
    /// neighbor without coverage or from a repeater), after Reticulum's
    /// boundary interfaces.
    pub bridge_boundary: bool,
    /// Density check (after Thread's router-eligible end devices): forest
    /// mode only while fewer than this many forwarding neighbors are heard
    /// (0 = off), and a forest-mode relay steps back to client when it hears
    /// at least `auto_drop_fwd` (0 = never).
    pub auto_max_fwd: usize,
    pub auto_drop_fwd: usize,
    /// How long an edge bridge that is no longer needed stays a relay
    /// before it becomes a client again (s, 0 = 120 like forest mode).
    pub bridge_hold_s: f64,
    /// Chance per 15 s check that a client steps in as an edge bridge
    /// (0 = 0.25, negative = adaptive: 1 / (1 + neighbors that hear a
    /// repeater too), at least -`bridge_p` when that is above -1, else at
    /// least 0.05), so the first bridge can announce before others follow.
    pub bridge_p: f64,
    /// Where neighbors' roles come from: "instant" (default: handed over at
    /// once, an idealisation), "announce" (today's firmware: only from 0-hop
    /// announces) or "hello" (announces plus Trickle hellos).
    pub neighbor_info: String,
    /// Contact table eviction: "" = the contact heard longest ago, "used" =
    /// contacts never written to or heard from go first (2Q-like, so the
    /// announces of strangers do not push out the people one talks to).
    pub contact_policy: String,
    /// Route age limit for moving nodes, s (0 = the router default, 30 s).
    pub mobile_route_s: f64,
    /// LoRa trunks: "backup" (default) is used only when the other end
    /// cannot be reached over TCP, carries path and key requests except for
    /// nodes known to be on this side (those on a second request), alerts
    /// always, routed
    /// packets only when the trunk is their next hop, and no other floods;
    /// "backup-all" is the same but carries every request; "always" treats
    /// a trunk like any link.
    pub trunk_policy: String,
    /// Mail for offline nodes: "" = none for handhelds (today), "named" =
    /// the recipient's fixed home host is its post box and delivers when it
    /// hears the recipient's announce (the old design), "post" = post offices
    /// (see `maarse_core::mail`): drop at the sender's nearest office, three
    /// host copies, MailSync, "mail waiting" in hellos, fetch on presence,
    /// sync to the previous home office.
    pub mail: String,
    /// The old rule: host-to-host mail ops only over TCP (before
    /// 2026-11-19). Off: sync/fetch/delivered go routed over radio and
    /// trunks when two hosts have no TCP between them.
    pub mail_tcp_only: bool,
    /// Repeaters are small post offices (class 1, a few messages each).
    pub repeater_mail: bool,
    /// Hosts remember the nodes they heard over their radio in the last 24 h
    /// and fetch their mail 1 and 5 minutes after the backbone comes back.
    pub mail_refetch: bool,
    /// Offices without the backbone also retry after 1, 2 and 4 hours.
    pub mail_long_retry: bool,
    /// LoRa trunks drop floods (path and key requests) first when queued:
    /// a flood waits at most 5 s, routed packets up to 30 s.
    pub trunk_priority: bool,
    /// Offices deliver one message at a time per recipient (the next when the
    /// last is acknowledged), except to a local recipient with the backbone up.
    pub mail_paced: bool,
    /// An office off the backbone also passes a copy to the recipient's home
    /// office (named in its announce), over LoRa, so one sync there gets all.
    pub mail_consolidate: bool,
    /// Hosts repeat a path request after 90 s (not 45 s) and anchors
    /// remember a leg for 300 s (not 120 s): routes over trunks are slow.
    pub host_slow_timers: bool,
    /// All trunks on one frequency: trunks that share an endpoint (two links
    /// into Roenne) share one channel there, so one sends at a time.
    pub trunk_same_channel: bool,
    /// Honest one-frequency model: a trunk sends on the mesh's channel and
    /// SF, so its transmissions occupy the channel around the sending end
    /// (nodes there wait or collide), and a packet arriving at the far end
    /// is lost if a mesh transmission there is within 6 dB (after
    /// `trunk_rejection_db` of rejection by the directional antenna).
    pub trunk_mesh_channel: bool,
    pub trunk_rejection_db: f64,
    /// One trunk per request: of the trunks in the same group, the one with
    /// the highest rendezvous score for the destination carries it (all of
    /// them on a second request), as ECMP spreads flows over equal paths.
    pub trunk_hash: bool,
    /// Trunk ends know who is behind each trunk (a small summary, e.g. a
    /// Bloom filter every 10 min): a request crosses only towards the side
    /// where its destination is (unknown destinations as before).
    pub trunk_summary: bool,
    /// Hosts (anchors) answer path requests from fresh routes (`proxy_reply`).
    pub proxy_reply: bool,
    /// Offices pack up to three waiting messages for a recipient into one
    /// packet (one ack for the bundle).
    pub mail_bundle: bool,
    /// Linked devices: each inner list is one person's devices (node
    /// indices). A message to any of them is addressed to the person: the
    /// sender uses the first device it has a route to (else the first one).
    pub device_groups: Vec<Vec<usize>>,
    /// The device that receives a message for the person sends a copy to
    /// each of the person's other devices (routed, over LoRa or TCP, with the
    /// post offices when the sibling is away).
    pub sibling_sync: bool,
    /// Linked devices: senders pick the device whose announce they got
    /// last (the newest announce for the person wins) instead of the first
    /// one they have a route to.
    pub person_fresh: bool,
    /// Linked devices: a device that forwards a copy to a sibling it heard
    /// from in the last hour tells the sender (in the ack) to use that
    /// sibling next time.
    pub redirect_hint: bool,
    /// Carry: lifetime of a copy a node starts carrying (s; 0 = the
    /// firmware's 30 min) and messages a node carries at most (0 = 8).
    pub carry_ttl_s: f64,
    pub carry_max: usize,
    /// Post offices off the backbone hand the mail they hold to carriers
    /// (`office_carry_tokens` copies each, living `carry_ttl_s`).
    pub office_carry: bool,
    pub office_carry_tokens: u8,
    /// Smart carry in every router (see `Router::carry_smart`).
    pub carry_smart: bool,
    /// Share of the handhelds that sleep to save battery: the radio is off
    /// except `wake_s` seconds of every `sleep_s + wake_s`; on waking they
    /// sync their post office (MailSync).
    pub sleepy_share: f64,
    pub sleep_s: f64,
    pub wake_s: f64,
    /// Messages to a sleepy node go straight to the post office (its
    /// announce and hello say it sleeps); no route is looked for.
    pub sleepy_mail_first: bool,
    /// The old radio queue that takes a second copy of a packet already
    /// waiting (before the fix of 2026-11-16), for before/after comparisons.
    pub no_queue_dedup: bool,
    /// Expanding ring search: first path request at most this many hops.
    pub path_ring: u8,
    /// Announce scope in hops (0 = the router default, `ANNOUNCE_SCOPE`).
    pub announce_scope: u8,
    /// Weak hops count as two when a destination picks a trail.
    pub weak_hop_penalty: bool,
    /// Hosts drop a route when a message they forwarded on it got no ack.
    pub drop_unacked_routes: bool,
    /// Text messages are compressed (maarse-core textz: about 56 % of the
    /// size for short chat messages): 34 bytes instead of 60.
    pub text_compress: bool,
    /// Record the event trace (needed for follow / reach / animation).
    #[serde(default = "yes")]
    pub trace: bool,
}

impl Default for Scenario {
    fn default() -> Self {
        Scenario {
            name: String::new(),
            rules: Default::default(),
            seed: 1,
            sf: 9,
            duty_permille: 100,
            tx_dbm: 20.0,
            freq_mhz: 0.0,
            bw_hz: 0,
            radio_cal: RadioCal::default(),
            duration_s: 1800,
            features: Features::default(),
            nodes: Vec::new(),
            links: Vec::new(),
            traffic: Traffic::default(),
            events: Vec::new(),
            clutter: Vec::new(),
            clutter_model: String::new(),
            announce_s: 0.0,
            relay_hops: 4,
            auto_relay_after_s: 600.0,
            contact_capacity: 0,
            pinned_capacity: 32,
            announce_warmup_s: 0.0,
            auto_bridge: String::new(),
            bridge_boundary: false,
            auto_max_fwd: 0,
            auto_drop_fwd: 0,
            bridge_hold_s: 0.0,
            bridge_p: 0.0,
            neighbor_info: String::new(),
            contact_policy: String::new(),
            mobile_route_s: 0.0,
            trunk_policy: String::new(),
            mail: String::new(),
            mail_tcp_only: false,
            repeater_mail: false,
            mail_refetch: false,
            mail_long_retry: false,
            trunk_priority: false,
            mail_paced: false,
            mail_consolidate: false,
            host_slow_timers: false,
            trunk_same_channel: false,
            trunk_mesh_channel: false,
            trunk_rejection_db: 15.0,
            trunk_hash: false,
            trunk_summary: false,
            proxy_reply: false,
            mail_bundle: false,
            device_groups: Vec::new(),
            sibling_sync: false,
            person_fresh: false,
            redirect_hint: false,
            carry_ttl_s: 0.0,
            carry_max: 0,
            office_carry: false,
            office_carry_tokens: 4,
            carry_smart: false,
            sleepy_share: 0.0,
            sleep_s: 300.0,
            wake_s: 30.0,
            sleepy_mail_first: false,
            no_queue_dedup: false,
            path_ring: 0,
            announce_scope: 0,
            weak_hop_penalty: false,
            drop_unacked_routes: false,
            text_compress: false,
            trace: true,
        }
    }
}

/// Switches for router features and simulator layers.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Features {
    /// Firmware rule (2026-11-19): a companion node whose phone is away does
    /// not acknowledge direct messages (Router::hold_direct), so they go to
    /// the post office instead of being lost; synced when the phone is back.
    pub hold_direct: bool,
    /// Hosts announce at start and every 10 minutes, as maarse-host does
    /// (`ANNOUNCE_EVERY`), with their name and capabilities (pages).
    pub host_announce: bool,
    /// EU sub-band duty limits (maarse_core::duty::eu_band_permille): a
    /// channel outside 869.4-869.65 MHz gets 1 % or 0.1 % airtime.
    pub duty_bands: bool,
    /// The host's guard on internet links (rate limits and bans,
    /// maarse_hostrules::guard). On in every real host.
    pub guard: bool,
    /// Store-carry-forward (`Router::carry_copies` = each node's `carry`, or 2).
    pub carry: bool,
    /// `Router::local_repair`.
    pub local_repair: bool,
    /// Backbone links (TCP / Wi-Fi) between hosts.
    pub backbone: bool,
    /// Regions: nodes serve `dk` + their part of the country; alerts are sent
    /// with the region that contains their area.
    pub regions: bool,
    /// Flood slot sized from the SF (`qos::slot_ms`, `Router::set_slot`).
    pub sf_slot: bool,
    /// Scale ack / path retry / hop timeouts with the slot.
    pub scale_timeouts: bool,
    /// `Router::load_control` (path request quota under high channel load).
    pub load_control: bool,
    /// Simulator layer: forwarders far outside an alert's area do not forward
    /// it (proposal, see AREA-ALERTS.md). Off = plain bulletin flood.
    pub area_scope: bool,
    /// Backbone path requests go only to the host that can reach the
    /// destination (a DHT lookup plus a direct connection), instead of to
    /// every connected backbone link as `maarse-host` does with its peers.
    pub dht_backbone: bool,
    /// Model of routing over the fixed infrastructure: home repeaters, a
    /// location lookup at the gateway and fixed routes (see
    /// `Engine::send_message`). Not a protocol that exists yet.
    pub home_routing: bool,
    /// A node keeps its known contacts across a reboot (stored in flash, as
    /// the firmware does since 2026-11-15). Off: a reboot forgets them.
    pub persist_known: bool,
    /// A node keeps its acknowledged routes (at most 32, the newest)
    /// across a reboot, as if stored in flash next to the contacts.
    pub persist_routes: bool,
    /// A message from a sender the node does not know (yet) is kept until
    /// the sender's announce arrives. Off: it is dropped (after the router
    /// has already acknowledged it).
    pub hold_unknown: bool,
    /// Automatic forest mode for handhelds: a handheld that has not heard a
    /// repeater or anchor for 10 min becomes a relay (forwards floods within
    /// `relay_hops`); after 2 min of hearing one again it is a client again.
    pub auto_relay: bool,
    /// Pin conversation partners in the contact table: never evicted, and
    /// the only contacts written to flash (see `contact_capacity`).
    pub pin_contacts: bool,
    /// With `auto_relay`: a handheld that hears a repeater and has a
    /// neighbor that does not stays (or becomes) a relay, the group's edge
    /// bridge into the repeater network.
    pub auto_edge: bool,
    /// Hosts re-flood a path request (or key request) that came over TCP on
    /// their radio only when the destination was heard announcing in their
    /// own region (or on a second request for it): ZRP-style bordercast.
    pub bordercast: bool,
    /// Hosts are anchors (split routes longer than 12 hops).
    pub host_anchor: bool,
    /// A node that lacks a contact's key asks for it (key request) instead
    /// of giving up or waiting for the next announce.
    pub key_request: bool,
    /// Hosts answer key requests heard over the radio from the announces
    /// they keep (unlimited on a PC).
    pub key_cache: bool,
    /// Moving nodes (cars) say so: routes to them go stale after 30 s, and
    /// their own routes too.
    pub mobile_mode: bool,
}

impl Default for Features {
    fn default() -> Self {
        Features {
            hold_direct: true,
            host_announce: true,
            duty_bands: true,
            guard: true,
            carry: false,
            local_repair: true,
            backbone: true,
            regions: false,
            sf_slot: false,
            scale_timeouts: false,
            load_control: false,
            area_scope: false,
            dht_backbone: false,
            home_routing: false,
            persist_known: true,
            persist_routes: false,
            hold_unknown: true,
            auto_relay: false,
            pin_contacts: false,
            auto_edge: true,
            bordercast: false,
            host_anchor: false,
            key_request: false,
            key_cache: false,
            mobile_mode: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct NodeSpec {
    /// handheld | relay | repeater | anchor | host | wifi | mobile
    pub kind: String,
    pub lat: f64,
    pub lon: f64,
    /// Antenna height in metres (0 = default for the kind).
    pub h: f64,
    /// Spreading factor (0 = the scenario's).
    pub sf: u8,
    /// Battery capacity in Wh (negative = default for the kind, 0 = none).
    pub battery_wh: f64,
    /// Runs on mains power (falls back to the battery in a power cut).
    pub mains: Option<bool>,
    /// Carry copies (0 = feature default).
    pub carry: u8,
    /// Has internet (hosts: the TCP backbone needs it).
    pub internet: Option<bool>,
    /// Region name (e.g. `nordjylland`), used when regions are on.
    pub region: String,
    /// Mobile nodes: route as [[lat, lon], ...] and speed.
    pub route: Vec<[f64; 2]>,
    pub speed_kmh: f64,
    /// Wandering (people walking around): radius in metres around the start
    /// point; the node walks to random points inside it at `speed_kmh`.
    pub wander_m: f64,
    pub label: String,
    /// Route timing: the node stands still until `start_s`, waits
    /// `pauses_s[k]` seconds at waypoint k, and at the end of the route
    /// "back" (walk back, the default), "stay" or "repeat" (start over).
    pub start_s: f64,
    pub pauses_s: Vec<f64>,
    pub route_end: String,
    /// Mobile (moves) or stationary, as the node says in its announce.
    /// None: mobile when it has a route or is a car.
    pub mobile: Option<bool>,
    /// Has a LoRa radio. None: yes, except kinds "server" (a VPS host)
    /// and "app" (a phone with the app only, reached over the internet).
    pub radio: Option<bool>,
    /// Backbone upkeep (maarse-host `--min-peers`): keep this many TCP
    /// backbone links, connecting to other hosts when links fail. None: 6
    /// for a server (as maarse-server), 0 otherwise.
    pub min_peers: Option<usize>,
    /// maarse:// pages this host shares (maarse-host `pages/`): a site
    /// name for its announce and the page files by path ("index" = front page).
    pub site: Option<Site>,
    /// Runs the call test service (maarse-host `<data>/calltest/`): it
    /// answers every call itself with music, echo or a speed test.
    pub calltest: bool,
    /// Nodes (by number) that may control this node remotely (its admins).
    pub admins: Vec<usize>,
    /// A seed host: one the seed DNS name points at (maarse-host `--seed`).
    pub seed: bool,
    /// Behind NAT: can connect out but cannot be connected to; others reach
    /// it through the hosts it connected to (relays).
    pub nat: bool,
}

/// A host's pages: the name it announces and MU markup per path.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Site {
    pub name: String,
    pub pages: std::collections::BTreeMap<String, String>,
    /// Groups for `@allow group <name>` lines: the node indices in each
    /// (maarse-host keeps them as `groups/<name>` files of addresses).
    pub groups: std::collections::BTreeMap<String, Vec<usize>>,
}

impl Default for NodeSpec {
    fn default() -> Self {
        NodeSpec {
            kind: "handheld".into(),
            lat: 56.0,
            lon: 10.5,
            h: 0.0,
            sf: 0,
            battery_wh: -1.0,
            mains: None,
            carry: 0,
            internet: None,
            region: String::new(),
            route: Vec::new(),
            speed_kmh: 0.0,
            wander_m: 0.0,
            label: String::new(),
            start_s: 0.0,
            pauses_s: Vec::new(),
            route_end: String::new(),
            mobile: None,
            radio: None,
            min_peers: None,
            site: None,
            calltest: false,
            admins: Vec::new(),
            seed: false,
            nat: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct LinkSpec {
    pub a: usize,
    pub b: usize,
    /// `tcp` (internet), `wifi` (point-to-point radio link, no internet
    /// needed) or `lora` (long-distance LoRa trunk between two masts with
    /// directional antennas: no internet needed, but every packet takes its
    /// airtime at `sf`, one at a time, with a pause for the duty cycle).
    pub kind: String,
    #[serde(default = "yes")]
    pub up: bool,
    /// LoRa trunk: spreading factor (default 9), link SNR in dB (default 5),
    /// packet loss (0..1) and duty cycle in permille (default 100 = 10 %).
    #[serde(default)]
    pub sf: u8,
    #[serde(default)]
    pub snr_db: f64,
    #[serde(default)]
    pub loss: f64,
    #[serde(default)]
    pub duty_permille: u32,
    #[serde(default)]
    pub label: String,
    /// Trunks that cross the same water (e.g. "west" for Jutland/Funen -
    /// Zealand): `trunk_hash` picks one of them per request.
    #[serde(default)]
    pub group: String,
}

impl Default for LinkSpec {
    fn default() -> Self {
        LinkSpec { a: 0, b: 0, kind: "tcp".into(), up: true, sf: 0, snr_db: 0.0, loss: 0.0, duty_permille: 0, label: String::new(), group: String::new() }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Traffic {
    /// Direct messages per minute (whole network).
    pub direct_per_min: f64,
    /// Channel messages per minute on the public channel.
    pub channel_per_min: f64,
    /// Share of direct messages to a user within `local_km`.
    #[serde(default = "d_local")]
    pub local_share: f64,
    pub local_km: f64,
    /// Traffic starts at this time (s) and stops at `stop_s` (0 = never).
    pub start_s: f64,
    pub stop_s: f64,
    /// Spam / flood attack: these node indices send channel messages at `spam_per_min` each.
    pub spammers: Vec<usize>,
    pub spam_per_min: f64,
    /// Circles of friends: every user gets this many friends among the users
    /// within `local_km` (0 = no circles) and writes to one of them with
    /// probability `friend_share`.
    pub friends: usize,
    pub friend_share: f64,
    /// A normal day: the clock time (hour, 0-24) when the run starts. The
    /// message rates then follow a day curve (quiet at night, peaks in the
    /// morning and evening; the rates above are the day's average).
    /// Negative = flat rates (the default).
    pub day_start_h: f64,
}

impl Default for Traffic {
    fn default() -> Self {
        Traffic {
            direct_per_min: 6.0,
            channel_per_min: 1.0,
            local_share: 0.8,
            local_km: 25.0,
            start_s: 5.0,
            stop_s: 0.0,
            spammers: Vec::new(),
            spam_per_min: 0.0,
            friends: 0,
            friend_share: 0.9,
            day_start_h: -1.0,
        }
    }
}

/// An area on the map: a circle or a polygon (lat/lon).
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Area {
    /// [lat, lon, radius_m]
    pub circle: Option<[f64; 3]>,
    pub polygon: Vec<[f64; 2]>,
}

/// A clutter area (forest, town). With the default `canopy` model only the
/// part of a link whose straight line runs inside the polygon *below*
/// `canopy_m` counts (an antenna above the trees or roofs sees over them),
/// and the loss saturates at `max_db` as in ITU-R P.833 vegetation loss:
/// A = Am (1 - exp(-γ d / Am)) with γ = `db_per_km` and Am = `max_db`
/// (about 29 dB at 868 MHz: 0.18 f^0.752 with f in MHz), for each end of the
/// link on its own (half the line each). Beyond that depth the signal goes
/// over the canopy instead of through it. With the old
/// `linear` model every km inside the polygon costs `db_per_km`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Clutter {
    pub label: String,
    pub polygon: Vec<[f64; 2]>,
    pub db_per_km: f64,
    pub canopy_m: f64,
    pub max_db: f64,
}

impl Default for Clutter {
    fn default() -> Self {
        Clutter { label: String::new(), polygon: Vec::new(), db_per_km: 0.0, canopy_m: 15.0, max_db: 29.0 }
    }
}

/// One timeline event. `type` picks the action:
/// internet_down {region?}, internet_up {region?}, power_cut {area?} (no area =
/// everywhere), power_restore {area?}, kill_random {n, node_kind},
/// kill {node | nodes}, revive {nodes?} (none = all killed), kill_area {area,
/// node_kind?}, host_internet {node, up}, degrade {area, db} (extra radio loss
/// for links with an end inside), degrade_clear, link {index, up},
/// alert {from, alert, area, text}, announce {node}, set_traffic {traffic},
/// reboot {nodes | area | n, node_kind?} (off and on again at once: RAM is
/// lost, and contacts too unless `persist_known`).
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct TimedEvent {
    pub t_s: f64,
    #[serde(rename = "type")]
    pub kind: String,
    pub region: String,
    pub area: Area,
    pub n: usize,
    pub node_kind: String,
    pub index: usize,
    pub up: bool,
    pub from: usize,
    /// evacuation | fire | flood | water | info. `alert_replay`: `index` is the
    /// alert sent again; `alert_forged`: an alert signed with the wrong key.
    pub alert: String,
    pub text: String,
    pub node: usize,
    pub nodes: Vec<usize>,
    /// Extra loss in dB (degrade).
    pub db: f64,
    pub traffic: Option<Traffic>,
    /// `radio` event: new settings for every node (0 = keep).
    pub sf: u8,
    pub freq_mhz: f64,
    pub bw_hz: u32,
    pub tx_dbm: f64,
    /// `alert`: minutes until the alert expires (0 = 60) and severity
    /// 0-3 (minor .. extreme; 0 here = severe).
    pub expires_min: u16,
    pub severity: u8,
    /// `flood`: packets a second (0 = 1000) for `duration_s` seconds (0 = 60).
    pub per_s: u32,
    pub duration_s: f64,
}

/// Radio model calibration (see tools/calibrate_radio.py).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct RadioCal {
    /// Path loss exponent n; 0 = the model's default (3).
    pub path_n: f64,
    pub offset_db: f64,
}
