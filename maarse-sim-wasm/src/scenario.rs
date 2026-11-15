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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Scenario {
    pub name: String,
    pub seed: u64,
    /// Default spreading factor (a node can override it).
    #[serde(default = "d_sf")]
    pub sf: u8,
    /// Duty cycle budget in permille (100 = 10 %, the EU 869.4-869.65 MHz band).
    #[serde(default = "d_duty")]
    pub duty_permille: u32,
    #[serde(default = "d_tx")]
    pub tx_dbm: f64,
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
    /// Record the event trace (needed for follow / reach / animation).
    #[serde(default = "yes")]
    pub trace: bool,
}

impl Default for Scenario {
    fn default() -> Self {
        Scenario {
            name: String::new(),
            seed: 1,
            sf: 9,
            duty_permille: 100,
            tx_dbm: 20.0,
            duration_s: 1800,
            features: Features::default(),
            nodes: Vec::new(),
            links: Vec::new(),
            traffic: Traffic::default(),
            events: Vec::new(),
            clutter: Vec::new(),
            trace: true,
        }
    }
}

/// Switches for router features and simulator layers.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Features {
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
}

impl Default for Features {
    fn default() -> Self {
        Features {
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
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct LinkSpec {
    pub a: usize,
    pub b: usize,
    /// `tcp` (internet) or `wifi` (point-to-point radio link, no internet needed).
    pub kind: String,
    #[serde(default = "yes")]
    pub up: bool,
}

impl Default for LinkSpec {
    fn default() -> Self {
        LinkSpec { a: 0, b: 0, kind: "tcp".into(), up: true }
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

/// A clutter area (forest, town): extra loss per km inside the polygon.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Clutter {
    pub label: String,
    pub polygon: Vec<[f64; 2]>,
    pub db_per_km: f64,
}

/// One timeline event. `type` picks the action:
/// internet_down {region?}, internet_up {region?}, power_cut {area?} (no area =
/// everywhere), power_restore {area?}, kill_random {n, node_kind},
/// kill {node | nodes}, revive {nodes?} (none = all killed), kill_area {area,
/// node_kind?}, host_internet {node, up}, degrade {area, db} (extra radio loss
/// for links with an end inside), degrade_clear, link {index, up},
/// alert {from, alert, area, text}, announce {node}, set_traffic {traffic}.
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
    /// evacuation | fire | flood | water | info
    pub alert: String,
    pub text: String,
    pub node: usize,
    pub nodes: Vec<usize>,
    /// Extra loss in dB (degrade).
    pub db: f64,
    pub traffic: Option<Traffic>,
}
