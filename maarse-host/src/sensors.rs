//! Sensor nodes, the host side: keeps what the sensors send (readings and
//! alarms), turns the designer's JSON "setup" into a recipe and back, and runs
//! a recipe on test data so the designer can show what it would do.
//!
//! The web client and the Android app both use this one implementation: the
//! designer works on the setup JSON, and the host compiles, checks and
//! simulates it with the same `maarse_core::sensor` engine the nodes run. The
//! node's own commands (writing the recipe, owners) go as companion commands
//! (`Command::Sensor`); history and the designer's help go as HTTP/JSON:
//!
//! ```text
//! GET  /api/sensors                         the sensors and their last values
//! GET  /api/sensors/series?node=&chan=&from=&to=&max=
//! GET  /api/sensors/alarms?node=
//! GET  /api/sensors/setup?node=             the designer's document for a sensor
//! PUT  /api/sensors/setup?node=             save it
//! POST /api/sensors/meta                    {"node", "name"?, "favourite"?}
//! POST /api/sensors/forget                  {"node"}
//! POST /api/sensors/compile                 setup -> recipe (hex), size, checks
//! POST /api/sensors/decompile               {"hex"} -> setup
//! POST /api/sensors/simulate                {"setup", "scenario"} -> what it would do
//! GET  /api/sensors/catalog                 the boards and sensors the designer offers
//! GET  /api/sensors/catalog/update          which list is in use and when it was checked
//! POST /api/sensors/catalog/update          {"check": true} look on GitHub now, {"auto": bool}, or
//!                                           {"catalog", "sums", "sig"} a signed list (the app downloads it)
//! ```

mod catalog;
mod catalog_update;

use maarse_core::Address;
use maarse_core::sensor::catalog::{Field, I2cDesc};
use maarse_core::sensor::{self, Action, Cmp, Cond, Edge, Input, Map, Message, Pull, Recipe, Rule, Sample, Source, Value, kind};
use serde_json::{Value as J, json};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

/// Points kept per channel (a week at 15 minutes is 672).
const MAX_POINTS: usize = 2000;
const MAX_ALARMS: usize = 30;
/// Sensors a host keeps; more are ignored (a stranger cannot fill the disk).
pub const MAX_SENSORS: usize = 50;
/// Samples one sensor may send per hour before the host drops them.
const MAX_PER_HOUR: u32 = 400;

#[derive(Default, Clone)]
struct Channel {
    kind: u8,
    points: VecDeque<(i64, i32)>,
}

#[derive(Default, Clone)]
struct AlarmRec {
    t: i64,
    rule: u8,
    raised: bool,
    chan: u8,
    kind: u8,
    value: i32,
    text: String,
}

#[derive(Default, Clone)]
struct SensorNode {
    name: String,
    favourite: bool,
    first_seen: i64,
    last_seen: i64,
    channels: BTreeMap<u8, Channel>,
    alarms: VecDeque<AlarmRec>,
    /// The designer's document (names and all), kept on the host.
    setup: Option<J>,
    window_start: i64,
    window_count: u32,
    /// Alarms up to this time are read ("Mark as read"): they no longer show as active.
    dismissed: i64,
}

#[derive(Default)]
pub struct Store {
    nodes: HashMap<Address, SensorNode>,
    path: Option<PathBuf>,
    dirty: bool,
}

pub static STORE: LazyLock<Mutex<Store>> = LazyLock::new(|| Mutex::new(Store::default()));

fn hex(a: &Address) -> String {
    a.0.iter().map(|b| format!("{b:02x}")).collect()
}

fn parse_hex(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

fn parse_addr(s: &str) -> Option<Address> {
    Address::from_slice(&parse_hex(s)?)
}

fn scaled(kind: u8, value: i32) -> f64 {
    value as f64 / catalog::kind_scale(kind) as f64
}

impl Store {
    /// Loads the saved sensors from `<data>/sensors.json` and keeps the path.
    pub fn open(&mut self, dir: &Path) {
        let path = dir.join("sensors.json");
        self.nodes.clear();
        if let Ok(text) = std::fs::read_to_string(&path)
            && let Ok(J::Object(map)) = serde_json::from_str::<J>(&text)
        {
            for (k, v) in map {
                let Some(addr) = parse_addr(&k) else { continue };
                let mut n = SensorNode {
                    name: v["name"].as_str().unwrap_or("").to_string(),
                    favourite: v["favourite"].as_bool().unwrap_or(false),
                    first_seen: v["first_seen"].as_i64().unwrap_or(0),
                    last_seen: v["last_seen"].as_i64().unwrap_or(0),
                    setup: v.get("setup").filter(|s| !s.is_null()).cloned(),
                    dismissed: v["dismissed"].as_i64().unwrap_or(0),
                    ..SensorNode::default()
                };
                if let Some(chs) = v["channels"].as_object() {
                    for (c, ch) in chs {
                        let Ok(c) = c.parse::<u8>() else { continue };
                        let mut channel = Channel { kind: ch["kind"].as_u64().unwrap_or(0) as u8, ..Channel::default() };
                        for p in ch["points"].as_array().into_iter().flatten() {
                            if let (Some(t), Some(v)) = (p[0].as_i64(), p[1].as_i64()) {
                                channel.points.push_back((t, v as i32));
                            }
                        }
                        n.channels.insert(c, channel);
                    }
                }
                for a in v["alarms"].as_array().into_iter().flatten() {
                    n.alarms.push_back(AlarmRec {
                        t: a["t"].as_i64().unwrap_or(0),
                        rule: a["rule"].as_u64().unwrap_or(0) as u8,
                        raised: a["raised"].as_bool().unwrap_or(false),
                        chan: a["chan"].as_u64().unwrap_or(0) as u8,
                        kind: a["kind"].as_u64().unwrap_or(0) as u8,
                        value: a["value"].as_i64().unwrap_or(0) as i32,
                        text: a["text"].as_str().unwrap_or("").to_string(),
                    });
                }
                self.nodes.insert(addr, n);
            }
        }
        self.path = Some(path);
        self.dirty = false;
    }

    /// Writes the sensors to disk if anything changed.
    pub fn save(&mut self) {
        let Some(path) = self.path.clone() else { return };
        if !self.dirty {
            return;
        }
        let mut map = serde_json::Map::new();
        for (addr, n) in &self.nodes {
            let mut chs = serde_json::Map::new();
            for (c, ch) in &n.channels {
                let pts: Vec<J> = ch.points.iter().map(|(t, v)| json!([t, v])).collect();
                chs.insert(c.to_string(), json!({"kind": ch.kind, "points": pts}));
            }
            let alarms: Vec<J> = n
                .alarms
                .iter()
                .map(|a| json!({"t": a.t, "rule": a.rule, "raised": a.raised, "chan": a.chan, "kind": a.kind, "value": a.value, "text": a.text}))
                .collect();
            map.insert(
                hex(addr),
                json!({"name": n.name, "favourite": n.favourite, "first_seen": n.first_seen, "last_seen": n.last_seen,
                       "channels": chs, "alarms": alarms, "setup": n.setup, "dismissed": n.dismissed}),
            );
        }
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, J::Object(map).to_string()).is_ok() && std::fs::rename(&tmp, &path).is_ok() {
            self.dirty = false;
        }
    }

    /// A message from a sensor. Returns false if it was refused (too many
    /// sensors, or one that talks too much).
    pub fn ingest(&mut self, from: Address, msg: &[u8], now: i64) -> bool {
        let Some(m) = sensor::decode(msg) else { return false };
        if !self.nodes.contains_key(&from) && self.nodes.len() >= MAX_SENSORS {
            return false;
        }
        let n = self.nodes.entry(from).or_insert_with(|| SensorNode { first_seen: now, ..SensorNode::default() });
        if now - n.window_start >= 3600 {
            n.window_start = now;
            n.window_count = 0;
        }
        n.last_seen = now;
        match m {
            Message::Readings { samples, .. } => {
                for s in &samples {
                    n.window_count += 1;
                    if n.window_count > MAX_PER_HOUR {
                        return false;
                    }
                    let t = now - s.age_s as i64;
                    for v in &s.values {
                        let ch = n.channels.entry(v.chan).or_default();
                        ch.kind = v.kind;
                        // The same reading can come twice (it was sent again after a missed ack).
                        if ch.points.iter().rev().take(4).any(|(pt, pv)| (pt - t).abs() < 20 && *pv == v.value) {
                            continue;
                        }
                        let at = ch.points.iter().rposition(|(pt, _)| *pt <= t).map_or(0, |i| i + 1);
                        ch.points.insert(at, (t, v.value));
                        while ch.points.len() > MAX_POINTS {
                            ch.points.pop_front();
                        }
                    }
                }
            }
            Message::Alarm { rule, raised, age_s, value, text } => {
                let t = now - age_s as i64;
                let text = String::from_utf8_lossy(&text).to_string();
                let dup = n.alarms.iter().rev().take(3).any(|a| a.rule == rule && a.raised == raised && (a.t - t).abs() < 30);
                if !dup {
                    n.alarms.push_back(AlarmRec { t, rule, raised, chan: value.chan, kind: value.kind, value: value.value, text });
                    while n.alarms.len() > MAX_ALARMS {
                        n.alarms.pop_front();
                    }
                }
            }
        }
        self.dirty = true;
        true
    }

    /// Marks the alarms of a sensor as read.
    pub fn dismiss(&mut self, node: &Address, now: i64) {
        if let Some(n) = self.nodes.get_mut(node) {
            n.dismissed = now;
            self.dirty = true;
        }
    }

    /// The sensors this host has a setup for.
    pub fn managed(&self) -> Vec<Address> {
        self.nodes.iter().filter(|(_, n)| n.setup.is_some()).map(|(a, _)| *a).collect()
    }

    fn list_json(&self, now: i64) -> J {
        let mut out: Vec<J> = Vec::new();
        for (addr, n) in &self.nodes {
            let chans: Vec<J> = n
                .channels
                .iter()
                .map(|(c, ch)| {
                    let last = ch.points.back();
                    json!({"chan": c, "kind": ch.kind, "name": catalog::kind_name(ch.kind), "unit": catalog::kind_unit(ch.kind),
                           "value": last.map(|(_, v)| scaled(ch.kind, *v)), "t": last.map(|(t, _)| *t)})
                })
                .collect();
            // An alarm is active while its last record is a "raised".
            let mut active: BTreeMap<u8, &AlarmRec> = BTreeMap::new();
            for a in n.alarms.iter().filter(|a| a.t > n.dismissed) {
                if a.raised {
                    active.insert(a.rule, a);
                } else {
                    active.remove(&a.rule);
                }
            }
            let alarm = active.values().next_back().map(|a| json!({"text": a.text, "t": a.t, "rule": a.rule}));
            out.push(json!({"node": hex(addr), "name": n.name, "favourite": n.favourite, "first_seen": n.first_seen,
                            "last_seen": n.last_seen, "age_s": now - n.last_seen, "channels": chans, "alarm": alarm,
                            "has_setup": n.setup.is_some()}));
        }
        out.sort_by(|a, b| {
            let key = |v: &J| (!v["favourite"].as_bool().unwrap_or(false), v["name"].as_str().unwrap_or("").to_lowercase(), v["node"].as_str().unwrap_or("").to_string());
            key(a).cmp(&key(b))
        });
        J::Array(out)
    }

    fn series_json(&self, node: &Address, chan: u8, from: i64, to: i64, max: usize) -> Option<J> {
        let ch = self.nodes.get(node)?.channels.get(&chan)?;
        let pts: Vec<&(i64, i32)> = ch.points.iter().filter(|(t, _)| *t >= from && *t <= to).collect();
        // Thin out long series: every n-th point, always the last.
        let step = pts.len().div_ceil(max.max(1)).max(1);
        let mut out: Vec<J> = pts.iter().step_by(step).map(|(t, v)| json!([t, scaled(ch.kind, *v)])).collect();
        if let Some((t, v)) = pts.last()
            && step > 1
        {
            out.push(json!([t, scaled(ch.kind, *v)]));
        }
        Some(json!({"kind": ch.kind, "name": catalog::kind_name(ch.kind), "unit": catalog::kind_unit(ch.kind), "points": out}))
    }
}

// ------------------------------------------------------------- setup <-> recipe

fn kind_from(v: &J) -> Result<u8, String> {
    catalog::current().kind_id(v)
}

fn kind_name(k: u8) -> J {
    catalog::kind_key(k)
}

fn pull_from(v: &J) -> Pull {
    match v.as_str() {
        Some("up") => Pull::Up,
        Some("down") => Pull::Down,
        _ => Pull::None,
    }
}

fn pull_name(p: Pull) -> &'static str {
    match p {
        Pull::None => "none",
        Pull::Up => "up",
        Pull::Down => "down",
    }
}

fn u8_of(v: &J, what: &str) -> Result<u8, String> {
    v.as_u64().and_then(|n| u8::try_from(n).ok()).ok_or_else(|| format!("{what} must be a number from 0 to 255"))
}

fn input_from(v: &J) -> Result<Input, String> {
    let t = v["type"].as_str().ok_or("A source needs an input")?;
    Ok(match t {
        "battery" => Input::Battery,
        "chip_temp" => Input::ChipTemp,
        "gpio_in" => {
            let invert = v["invert"].as_bool().unwrap_or(false);
            let wake = v["wake"].as_bool().unwrap_or(false);
            // A wake pin is held at the level that does not wake the node: the pull follows.
            let pull = if wake { if invert { Pull::Up } else { Pull::Down } } else { pull_from(&v["pull"]) };
            Input::GpioIn { pin: u8_of(&v["pin"], "pin")?, pull, invert, wake }
        }
        "adc" => {
            let mul = v["mul"].as_f64().unwrap_or(1.0);
            if !(1.0 / 16.0..=15.0).contains(&mul) {
                return Err("The ADC multiplier must be between 0.07 and 15".into());
            }
            Input::Adc { pin: u8_of(&v["pin"], "pin")?, mul16: (mul * 16.0).round() as u8 }
        }
        "bme280" => {
            let what = match v["what"].as_str() {
                Some("temperature") | None => 0,
                Some("humidity") => 1,
                Some("pressure") => 2,
                Some(w) => return Err(format!("A BME280 has no \"{w}\"")),
            };
            Input::Bme280 { addr: v["addr"].as_u64().unwrap_or(0x76) as u8, what }
        }
        "i2c" => {
            let device = v["device"].as_str().ok_or("An I2C sensor needs a device")?;
            let addr = v["addr"].as_u64().map(|a| u8::try_from(a).map_err(|_| "The I2C address is a number from 1 to 119")).transpose()?;
            catalog::current().i2c_input(device, addr, &v["what"])?
        }
        "i2c_custom" => Input::I2c(custom_from(v)?),
        "counter" => Input::Counter { pin: u8_of(&v["pin"], "pin")?, falling: v["falling"].as_bool().unwrap_or(false), pull: pull_from(&v["pull"]) },
        "gpio_out" => Input::GpioOut { pin: u8_of(&v["pin"], "pin")?, active_low: v["active_low"].as_bool().unwrap_or(false) },
        other => return Err(format!("Unknown input type \"{other}\"")),
    })
}

fn hex_of(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// A sensor described by hand: addresses and register numbers are numbers,
/// the bytes to send are hex strings.
fn custom_from(v: &J) -> Result<I2cDesc, String> {
    let bytes = |key: &str, max: usize| -> Result<Vec<u8>, String> {
        let t = v[key].as_str().unwrap_or("").replace([' ', '_'], "");
        let b = parse_hex(&t).ok_or_else(|| format!("\"{key}\" must be hex bytes, like 24 00"))?;
        if b.len() > max { Err(format!("\"{key}\" is at most {max} bytes")) } else { Ok(b) }
    };
    let num = |x: &J, what: &str, d: i64| -> Result<i64, String> {
        match x {
            J::Null => Ok(d),
            J::Number(n) => n.as_f64().map(|f| f.round() as i64).ok_or_else(|| format!("{what} is not a number")),
            _ => Err(format!("{what} must be a number")),
        }
    };
    let addr = num(&v["addr"], "The address", -1)?;
    if !(1..=0x77).contains(&addr) {
        return Err("The I2C address must be between 1 and 119 (0x01 to 0x77)".into());
    }
    let mut d = I2cDesc::new(addr as u8);
    let mut left = 0;
    for cmd in v["init"].as_array().into_iter().flatten() {
        let b = parse_hex(&cmd.as_str().unwrap_or("").replace(' ', "")).filter(|b| (1..=4).contains(&b.len())).ok_or("Each set-up command is 1 to 4 hex bytes")?;
        if left + 1 + b.len() > sensor::catalog::MAX_INIT {
            return Err(format!("The set-up commands are too long (at most {} bytes with their lengths)", sensor::catalog::MAX_INIT));
        }
        d.init[left] = b.len() as u8;
        d.init[left + 1..left + 1 + b.len()].copy_from_slice(&b);
        left += 1 + b.len();
    }
    d.init_len = left as u8;
    let t = bytes("trigger", sensor::catalog::MAX_TRIGGER)?;
    d.trigger[..t.len()].copy_from_slice(&t);
    d.trigger_len = t.len() as u8;
    let p = bytes("ptr", sensor::catalog::MAX_PTR)?;
    d.ptr[..p.len()].copy_from_slice(&p);
    d.ptr_len = p.len() as u8;
    d.wait_ms = num(&v["wait_ms"], "The wait", 0)?.clamp(0, u16::MAX as i64) as u16;
    d.read_len = num(&v["read_len"], "The number of bytes to read", 2)?.clamp(0, 255) as u8;
    d.crc = v["crc"].as_bool().unwrap_or(false);
    let f = &v["field"];
    d.field = Field {
        bit_off: num(&f["bit_off"], "bit_off", 0)?.clamp(0, 255) as u8,
        bits: num(&f["bits"], "bits", 16)?.clamp(0, 255) as u8,
        signed: f["signed"].as_bool().unwrap_or(false),
        little: f["little"].as_bool().unwrap_or(false),
        mul: num(&f["mul"], "mul", 1)?.clamp(i32::MIN as i64, i32::MAX as i64) as i32,
        div: num(&f["div"], "div", 1)?.clamp(i32::MIN as i64, i32::MAX as i64) as i32,
        add: num(&f["add"], "add", 0)?.clamp(i32::MIN as i64, i32::MAX as i64) as i32,
    };
    if !d.validate() {
        return Err("The sensor description does not fit together: check that the bits are inside the bytes read (with check bytes, 2 of every 3 bytes count), that the divisor is not 0, and that little endian fields start on a byte".into());
    }
    Ok(d)
}

fn custom_json(d: &I2cDesc) -> J {
    let init: Vec<String> = d.init_commands().map(hex_of).collect();
    json!({"type": "i2c_custom", "addr": d.addr, "init": init, "trigger": hex_of(d.trigger_bytes()), "wait_ms": d.wait_ms, "ptr": hex_of(d.ptr_bytes()),
           "read_len": d.read_len, "crc": d.crc,
           "field": {"bit_off": d.field.bit_off, "bits": d.field.bits, "signed": d.field.signed, "little": d.field.little, "mul": d.field.mul, "div": d.field.div, "add": d.field.add}})
}

fn input_json(i: &Input) -> J {
    match *i {
        Input::Battery => json!({"type": "battery"}),
        Input::ChipTemp => json!({"type": "chip_temp"}),
        Input::GpioIn { pin, pull, invert, wake } => json!({"type": "gpio_in", "pin": pin, "pull": pull_name(pull), "invert": invert, "wake": wake}),
        Input::Adc { pin, mul16 } => json!({"type": "adc", "pin": pin, "mul": mul16 as f64 / 16.0}),
        Input::Bme280 { addr, what } => {
            let what = ["temperature", "humidity", "pressure"][what.min(2) as usize];
            json!({"type": "bme280", "addr": addr, "what": what})
        }
        // A catalogue chip is shown by its name; anything else as the description it is.
        Input::I2c(d) => match catalog::current().recognise(&d) {
            Some((device, addr, what)) => json!({"type": "i2c", "device": device, "addr": addr, "what": what}),
            None => custom_json(&d),
        },
        Input::Counter { pin, falling, pull } => json!({"type": "counter", "pin": pin, "falling": falling, "pull": pull_name(pull)}),
        Input::GpioOut { pin, active_low } => json!({"type": "gpio_out", "pin": pin, "active_low": active_low}),
    }
}

/// Owner slots and the channel flag as a recipient mask.
fn mask_from(v: &J) -> Result<u8, String> {
    if v.is_null() {
        return Ok(0);
    }
    let mut m = 0u8;
    for o in v["owners"].as_array().into_iter().flatten() {
        let slot = o.as_u64().filter(|s| *s < 4).ok_or("An owner is a number from 0 to 3")?;
        m |= 1 << slot;
    }
    if v["channel"].as_bool().unwrap_or(false) {
        m |= sensor::TO_CHANNEL;
    }
    Ok(m)
}

fn mask_json(m: u8) -> J {
    let owners: Vec<u8> = (0..4).filter(|s| m >> s & 1 != 0).collect();
    json!({"owners": owners, "channel": m & sensor::TO_CHANNEL != 0})
}

/// The index of the source with this channel.
fn src_index(sources: &[Source], chan: &J) -> Result<u8, String> {
    let c = u8_of(chan, "chan")?;
    sources.iter().position(|s| s.chan == c).map(|i| i as u8).ok_or_else(|| format!("No source on channel {c}"))
}

fn to_units(sources: &[Source], src: u8, x: f64) -> i32 {
    (x * catalog::kind_scale(sources[src as usize].kind) as f64).round() as i32
}

/// Builds a recipe from the designer's setup.
pub fn compile(setup: &J) -> Result<Recipe, String> {
    let mut r = Recipe {
        flags: 0,
        interval_s: setup["interval_s"].as_u64().unwrap_or(900).min(u16::MAX as u64) as u16,
        ..Recipe::default()
    };
    if setup["sleep"].as_bool().unwrap_or(true) {
        r.flags |= sensor::FLAG_SLEEP;
    }
    if let Some(c) = setup["channel"].as_str().filter(|c| !c.is_empty()) {
        let a = parse_addr(c).ok_or("The channel address must be 32 hex digits")?;
        r.flags |= sensor::FLAG_CHANNEL;
        r.channel = Some(a);
    }
    r.report_to = mask_from(&setup["report_to"])?;
    r.alarm_to = mask_from(&setup["alarm_to"])?;
    for s in setup["sources"].as_array().ok_or("The setup has no sources")? {
        let src = Source { chan: u8_of(&s["chan"], "chan")?, kind: kind_from(&s["kind"])?, input: input_from(&s["input"])? };
        r.sources.push(src).map_err(|_| format!("At most {} sources", sensor::MAX_SOURCES))?;
    }
    // A map turns a reading into another range (millivolts into a percentage).
    for (i, s) in setup["sources"].as_array().into_iter().flatten().enumerate() {
        let m = &s["map"];
        if m.is_null() {
            continue;
        }
        let pair = |key: &str| -> Result<(f64, f64), String> {
            match (m[key][0].as_f64(), m[key][1].as_f64()) {
                (Some(a), Some(b)) => Ok((a, b)),
                _ => Err(format!("A map needs \"{key}\": [from, to]")),
            }
        };
        let ((i0, i1), (o0, o1)) = (pair("in")?, pair("out")?);
        let scale = catalog::kind_scale(r.sources[i].kind) as f64;
        let fit = |x: f64, what: &str| -> Result<i16, String> {
            let v = x.round();
            if (i16::MIN as f64..=i16::MAX as f64).contains(&v) { Ok(v as i16) } else { Err(format!("The {what} of a map is out of range")) }
        };
        r.maps
            .push(Map { src: i as u8, in_lo: fit(i0, "input")?, in_hi: fit(i1, "input")?, out_lo: fit(o0 * scale, "output")?, out_hi: fit(o1 * scale, "output")? })
            .map_err(|_| format!("At most {} maps", sensor::MAX_MAPS))?;
    }
    if let Some(b) = setup["board"].as_str() {
        catalog::check_board(b, &r)?;
    }
    let sources: Vec<Source> = r.sources.to_vec();
    for (i, rule) in setup["rules"].as_array().ok_or("The setup has no rules")?.iter().enumerate() {
        let id = rule["id"].as_u64().map_or(i as u64 + 1, |v| v) as u8;
        let w = &rule["when"];
        let cond = match w["type"].as_str().ok_or("A rule needs a condition")? {
            "every" => Cond::Every { secs: w["secs"].as_u64().ok_or("\"every\" needs secs")?.min(u16::MAX as u64) as u16 },
            "compare" => {
                let src = src_index(&sources, &w["chan"])?;
                let op = match w["op"].as_str().unwrap_or("<") {
                    "<" => Cmp::Lt,
                    ">" => Cmp::Gt,
                    "<=" => Cmp::Le,
                    ">=" => Cmp::Ge,
                    "==" => Cmp::Eq,
                    "!=" => Cmp::Ne,
                    o => return Err(format!("Unknown comparison \"{o}\"")),
                };
                let value = to_units(&sources, src, w["value"].as_f64().ok_or("A comparison needs a value")?);
                let clear = w["clear"].as_f64().map_or(value, |c| to_units(&sources, src, c));
                Cond::Compare { src, op, value, clear, count: w["count"].as_u64().unwrap_or(1).clamp(1, 255) as u8 }
            }
            "change" => {
                let src = src_index(&sources, &w["chan"])?;
                let delta = to_units(&sources, src, w["delta"].as_f64().ok_or("\"change\" needs a delta")?);
                Cond::Change { src, delta: delta.unsigned_abs() }
            }
            "edge" => Cond::Edge {
                src: src_index(&sources, &w["chan"])?,
                to: match w["to"].as_str().unwrap_or("any") {
                    "high" | "open" => Edge::High,
                    "low" | "closed" => Edge::Low,
                    _ => Edge::Any,
                },
            },
            t => return Err(format!("Unknown condition \"{t}\"")),
        };
        let mut actions = heapless::Vec::new();
        for a in rule["then"].as_array().into_iter().flatten() {
            let act = match a["type"].as_str().ok_or("An action needs a type")? {
                "report" => {
                    let mask = match a["chans"].as_array() {
                        // Every source (the same mask a decompiled recipe gives).
                        None => ((1u16 << sources.len()) - 1) as u8,
                        Some(cs) => {
                            let mut m = 0u8;
                            for c in cs {
                                m |= 1 << src_index(&sources, c)?;
                            }
                            m
                        }
                    };
                    Action::Report { mask, to: mask_from(&a["to"])? }
                }
                "alarm" => {
                    let text = a["text"].as_str().unwrap_or("").trim();
                    let off = match find_text(&r.text, text.as_bytes()) {
                        Some(o) => o,
                        None => {
                            let o = r.text.len();
                            r.text.extend_from_slice(text.as_bytes()).map_err(|_| "The alarm texts are too long together (at most 64 bytes)")?;
                            o
                        }
                    };
                    Action::Alarm { off: off as u8, len: text.len() as u8, to: mask_from(&a["to"])? }
                }
                "set_out" => Action::SetOut { src: src_index(&sources, &a["chan"])?, on: a["on"].as_bool().unwrap_or(true) },
                "interval" => Action::SetInterval { secs: a["secs"].as_u64().ok_or("\"interval\" needs secs")?.min(u16::MAX as u64) as u16 },
                t => return Err(format!("Unknown action \"{t}\"")),
            };
            actions.push(act).map_err(|_| format!("At most {} actions in a rule", sensor::MAX_ACTIONS))?;
        }
        let limit = rule["limit_min"].as_u64().unwrap_or(0).div_ceil(5).min(255) as u8;
        r.rules.push(Rule { id, cond, limit_5min: limit, actions }).map_err(|_| format!("At most {} rules", sensor::MAX_RULES))?;
    }
    r.validate().map_err(|e| match e {
        sensor::RecipeError::Invalid => "The setup is not valid: check intervals (at least 30 s), channels, recipients and texts".to_string(),
        e => format!("{e:?}"),
    })?;
    Ok(r)
}

fn find_text(pool: &[u8], t: &[u8]) -> Option<usize> {
    if t.is_empty() {
        return Some(0);
    }
    pool.windows(t.len()).position(|w| w == t)
}

/// The designer's setup for a recipe (names are made up: the recipe has none).
pub fn decompile(r: &Recipe) -> J {
    let sources: Vec<J> = r
        .sources
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let mut j = json!({"chan": s.chan, "kind": kind_name(s.kind), "name": format!("{} {}", catalog::kind_name(s.kind), s.chan), "input": input_json(&s.input)});
            if let Some(m) = r.maps.iter().find(|m| m.src as usize == i) {
                let sc = catalog::kind_scale(s.kind) as f64;
                j["map"] = json!({"in": [m.in_lo, m.in_hi], "out": [m.out_lo as f64 / sc, m.out_hi as f64 / sc]});
            }
            j
        })
        .collect();
    let chan_of = |i: u8| r.sources.get(i as usize).map_or(0, |s| s.chan);
    let units = |i: u8, v: i32| r.sources.get(i as usize).map_or(v as f64, |s| scaled(s.kind, v));
    let rules: Vec<J> = r
        .rules
        .iter()
        .map(|rule| {
            let when = match rule.cond {
                Cond::Every { secs } => json!({"type": "every", "secs": secs}),
                Cond::Compare { src, op, value, clear, count } => json!({
                    "type": "compare", "chan": chan_of(src),
                    "op": match op { Cmp::Lt => "<", Cmp::Gt => ">", Cmp::Le => "<=", Cmp::Ge => ">=", Cmp::Eq => "==", Cmp::Ne => "!=" },
                    "value": units(src, value), "clear": units(src, clear), "count": count}),
                Cond::Change { src, delta } => json!({"type": "change", "chan": chan_of(src), "delta": units(src, delta as i32)}),
                Cond::Edge { src, to } => json!({"type": "edge", "chan": chan_of(src), "to": match to { Edge::Low => "low", Edge::High => "high", Edge::Any => "any" }}),
            };
            let then: Vec<J> = rule
                .actions
                .iter()
                .map(|a| match *a {
                    Action::Report { mask, to } => {
                        let chans: Vec<u8> = (0..r.sources.len() as u8).filter(|i| mask >> i & 1 != 0).map(chan_of).collect();
                        json!({"type": "report", "chans": chans, "to": if to == 0 { J::Null } else { mask_json(to) }})
                    }
                    Action::Alarm { off, len, to } => json!({"type": "alarm", "text": r.alarm_text(off, len), "to": if to == 0 { J::Null } else { mask_json(to) }}),
                    Action::SetOut { src, on } => json!({"type": "set_out", "chan": chan_of(src), "on": on}),
                    Action::SetInterval { secs } => json!({"type": "interval", "secs": secs}),
                })
                .collect();
            json!({"id": rule.id, "name": format!("Rule {}", rule.id), "when": when, "then": then, "limit_min": rule.limit_5min as u32 * 5})
        })
        .collect();
    json!({"v": 1, "interval_s": r.interval_s, "sleep": r.flags & sensor::FLAG_SLEEP != 0,
           "report_to": mask_json(r.report_to), "alarm_to": mask_json(r.alarm_to),
           "channel": r.channel.map(|c| hex(&c)), "sources": sources, "rules": rules})
}

// ------------------------------------------------------------------ simulation

/// A reading for source `s` at second `t` of the scenario.
fn scenario_value(scenario: &str, s: &Source, t: u32) -> Option<i32> {
    let hours = t as f64 / 3600.0;
    let day = |h: f64| (h % 24.0 + 24.0) % 24.0;
    match s.input {
        Input::Bme280 { what: 0, .. } | Input::ChipTemp => {
            // A cold night: about 15 degrees in the afternoon, 8 at 04:00.
            let base = match scenario {
                "cold_night" => 11.5 + 3.5 * ((day(hours) - 4.0 - 6.0) * std::f64::consts::PI / 12.0).sin(),
                _ => 11.0,
            };
            Some((base * 100.0).round() as i32)
        }
        Input::Bme280 { what: 1, .. } => Some(6200 + ((hours * 7.0).sin() * 300.0) as i32),
        Input::Bme280 { .. } => Some(101_325 - 20 * hours as i32),
        Input::Battery => Some(3840 - hours as i32 * 2),
        Input::Adc { .. } => Some(1200),
        Input::GpioIn { invert, .. } => {
            // The door is open for 20 minutes at 08:10 and 14:30 in "door".
            let open = scenario == "door" && {
                let m = (day(hours) * 60.0) as u32;
                (490..510).contains(&m) || (870..890).contains(&m)
            };
            Some((open ^ invert) as i32)
        }
        Input::Counter { .. } => Some((hours * 12.0) as i32),
        Input::GpioOut { .. } => None,
        // A chip the host cannot read: a believable value for what it measures.
        Input::I2c(_) => Some(match s.kind {
            kind::TEMPERATURE => (match scenario {
                "cold_night" => 11.5 + 3.5 * ((day(hours) - 4.0 - 6.0) * std::f64::consts::PI / 12.0).sin(),
                _ => 11.0,
            } * 100.0)
                .round() as i32,
            kind::HUMIDITY => 6200 + ((hours * 7.0).sin() * 300.0) as i32,
            kind::PRESSURE => 101_325 - 20 * hours as i32,
            kind::CO2 => 600 + ((hours * 3.0).sin() * 150.0) as i32,
            kind::LIGHT => (400.0 * ((day(hours) - 6.0) * std::f64::consts::PI / 12.0).sin().max(0.0)) as i32,
            kind::VOLTAGE => 1200,
            kind::ACCEL => 1000,
            _ => 0,
        }),
    }
}

/// Runs the recipe over 24 hours of a test scenario ("cold_night", "door",
/// "silent": the node stops answering at 10:00).
pub fn simulate(r: &Recipe, scenario: &str) -> J {
    let mut engine = sensor::Engine::new(r.clone());
    let mut outputs = 0u8;
    let (mut next_reg, end) = (0u32, 24 * 3600);
    // A node with a wake pin is woken the moment the pin changes, between two cycles.
    let wake_idx = r.sources.iter().position(|s| matches!(s.input, Input::GpioIn { wake: true, .. }));
    let wake_events: Vec<u32> = if wake_idx.is_some() && scenario == "door" { vec![490 * 60, 870 * 60] } else { Vec::new() };
    let mut ei = 0usize;
    let mut times: Vec<u32> = Vec::new();
    let mut series: BTreeMap<u8, Vec<J>> = BTreeMap::new();
    let mut events: Vec<J> = Vec::new();
    let (mut reports, mut alarms, mut out_on_s) = (0u32, 0u32, 0u32);
    let mut last_out_t = 0u32;
    let mut bytes_sent = 0usize;
    let mut cycles = 0u32;
    while next_reg < end || ei < wake_events.len() {
        let (t, is_event) = match wake_events.get(ei) {
            Some(&e) if e <= next_reg || next_reg >= end => (e, true),
            _ => (next_reg, false),
        };
        let silent = scenario == "silent" && t >= 10 * 3600;
        let mut values: Vec<Option<i32>> = r
            .sources
            .iter()
            .enumerate()
            .map(|(i, s)| if matches!(s.input, Input::GpioOut { .. }) { Some((outputs >> i & 1) as i32) } else if silent { None } else { scenario_value(scenario, s, t) })
            .collect();
        r.apply_maps(&mut values);
        if values.is_empty() {
            values.push(None);
        }
        if is_event && let Some(i) = wake_idx {
            values[i] = Some(1);
            engine.pin_woke(i as u8, 1);
        }
        if !silent {
            cycles += 1;
            times.push(t);
            for (i, s) in r.sources.iter().enumerate() {
                series.entry(s.chan).or_default().push(values[i].map_or(J::Null, |v| json!(scaled(s.kind, v))));
            }
            let mut fx: heapless::Vec<sensor::Effect, 16> = heapless::Vec::new();
            engine.step(t, &values, &mut fx);
            for e in fx {
                match e {
                    sensor::Effect::Report { mask, to } => {
                        reports += 1;
                        let vs: heapless::Vec<Value, { sensor::MAX_SOURCES }> = r
                            .sources
                            .iter()
                            .enumerate()
                            .filter(|(i, _)| mask >> i & 1 != 0)
                            .filter_map(|(i, s)| values[i].map(|v| Value { chan: s.chan, kind: s.kind, value: v }))
                            .collect();
                        let (msg, _) = sensor::encode_readings(0, &[Sample { age_s: 0, values: vs }]);
                        // One message per recipient (owner), plus the channel.
                        bytes_sent += (msg.len() + 1 + 64 + 18) * (to & 0x1f).count_ones().max(1) as usize;
                        events.push(json!({"t": t, "type": "report"}));
                    }
                    sensor::Effect::Alarm { rule, raised, src, value, off, len, .. } => {
                        if raised {
                            alarms += 1;
                        }
                        let chan = r.sources.get(src as usize).map_or(0, |s| s.chan);
                        events.push(json!({"t": t, "type": "alarm", "raised": raised, "rule": rule, "chan": chan,
                                           "value": r.sources.get(src as usize).map_or(value as f64, |s| scaled(s.kind, value)),
                                           "text": r.alarm_text(off, len)}));
                        bytes_sent += 18 + 1 + 64 + 12 + len as usize;
                    }
                    sensor::Effect::SetOut { src, on } => {
                        let now_on = outputs >> src & 1 != 0;
                        if on != now_on {
                            if on {
                                last_out_t = t;
                            } else {
                                out_on_s += t - last_out_t;
                            }
                            if on {
                                outputs |= 1 << src;
                            } else {
                                outputs &= !(1 << src);
                            }
                            events.push(json!({"t": t, "type": "output", "chan": r.sources.get(src as usize).map_or(0, |s| s.chan), "on": on}));
                        }
                    }
                }
            }
        }
        if is_event {
            ei += 1;
        } else {
            next_reg += engine.interval_s().max(sensor::MIN_INTERVAL_S as u32);
        }
    }
    if outputs != 0 {
        out_on_s += end - last_out_t;
    }
    // Airtime of the radio at the default settings (SF9, 125 kHz).
    let air = maarse_core::companion::RadioConfig::EU_DEFAULT.airtime();
    let per_msg_s = if reports + alarms > 0 { air.airtime_us((bytes_sent / (reports + alarms) as usize).clamp(30, 255)) as f64 / 1e6 } else { 0.0 };
    json!({"times": times, "series": series, "events": events,
           "summary": {"cycles": cycles, "reports": reports, "alarms": alarms, "output_on_s": out_on_s,
                       "airtime_s_per_day": ((reports + alarms) as f64 * per_msg_s * 10.0).round() / 10.0,
                       "airtime_percent": ((reports + alarms) as f64 * per_msg_s / 864.0 * 1000.0).round() / 1000.0}})
}

/// The compiled recipe and what the designer wants to know about it.
pub(crate) fn compile_json(setup: &J) -> J {
    match compile(setup).and_then(|r| r.to_bytes().map(|b| (r, b)).map_err(|e| format!("{e:?}"))) {
        Ok((r, bytes)) => {
            let sim = simulate(&r, "cold_night");
            let mut warnings: Vec<&str> = Vec::new();
            if r.flags & sensor::FLAG_SLEEP != 0 && r.sources.iter().any(|s| matches!(s.input, Input::GpioOut { .. })) {
                warnings.push("On the ESP32 an output switches off while the node sleeps");
            }
            // Pins are read once per cycle: a short event between two cycles is missed.
            let pin_rule = r.rules.iter().any(|ru| matches!(ru.cond, Cond::Edge { .. }));
            if pin_rule && r.interval_s > 120 && r.wake_pins().is_empty() {
                warnings.push("A pin is read once per cycle: an opening shorter than the interval is missed. Switch on \"Wake the sensor\" for the pin, or use a shorter interval");
            }
            let slow = r.sources.iter().any(|s| matches!(s.input, Input::I2c(d) if d.wait_ms >= 1000));
            if slow && r.flags & sensor::FLAG_SLEEP != 0 && r.interval_s < 300 {
                warnings.push("A CO2 sensor is awake for 5 seconds at every reading: on a battery use an interval of 5 minutes or more");
            }
            if r.report_to == 0 && r.alarm_to == 0 {
                warnings.push("Nobody receives anything: choose who gets the readings and alarms");
            }
            json!({"ok": true, "hex": bytes.iter().map(|b| format!("{b:02x}")).collect::<String>(), "size": bytes.len(),
                   "limit": sensor::MAX_RECIPE, "rules": r.rules.len(), "rule_limit": sensor::MAX_RULES,
                   "sources": r.sources.len(), "source_limit": sensor::MAX_SOURCES,
                   "crc": sensor::crc16(&bytes), "airtime_s_per_day": sim["summary"]["airtime_s_per_day"],
                   "airtime_percent": sim["summary"]["airtime_percent"], "warnings": warnings})
        }
        Err(e) => json!({"ok": false, "error": e}),
    }
}

// ----------------------------------------------------------------------- API

fn param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))
}

/// Reads the saved sensor list (checked again) and starts the daily look at GitHub.
pub fn open_catalog(data_dir: &Path) {
    catalog_update::open(data_dir);
}

pub fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// Handles `/api/sensors...`. Returns (status line, JSON body), or None if the
/// path is not ours.
pub fn api(method: &str, path: &str, query: &str, body: &[u8]) -> Option<(&'static str, String)> {
    let path = path.strip_prefix("/api/sensors")?;
    let bad = |m: &str| Some(("400 Bad Request", json!({"error": m}).to_string()));
    let ok = |v: J| Some(("200 OK", v.to_string()));
    let parsed = || serde_json::from_slice::<J>(body);
    // The sensor list is not the store: looking on GitHub takes a while and must not hold it.
    match (method, path) {
        ("GET", "/catalog") => return ok(catalog::catalog_json()),
        ("GET", "/catalog/update") => return ok(catalog_update::status_json()),
        ("POST", "/catalog/update") => {
            let Ok(v) = parsed() else { return bad("not JSON") };
            if let Some(on) = v["auto"].as_bool() {
                catalog_update::set_auto(on);
            }
            let result = if let (Some(c), Some(s), Some(g)) = (v["catalog"].as_str(), v["sums"].as_str(), v["sig"].as_str()) {
                Some(catalog_update::install_signed(c.as_bytes(), s, g))
            } else if v["check"].as_bool() == Some(true) {
                Some(catalog_update::check_now())
            } else {
                None
            };
            let mut out = catalog_update::status_json();
            match result {
                Some(Ok(m)) => out["message"] = json!(m),
                Some(Err(e)) => out["error"] = json!(e),
                None => {}
            }
            return ok(out);
        }
        _ => {}
    }
    let mut store = STORE.lock().unwrap_or_else(|e| e.into_inner());
    match (method, path) {
        ("GET", "") => ok(store.list_json(unix_now())),
        ("GET", "/series") => {
            let Some(node) = param(query, "node").and_then(parse_addr) else { return bad("node?") };
            let chan: u8 = param(query, "chan").and_then(|c| c.parse().ok()).unwrap_or(1);
            let now = unix_now();
            let from = param(query, "from").and_then(|v| v.parse().ok()).unwrap_or(now - 24 * 3600);
            let to = param(query, "to").and_then(|v| v.parse().ok()).unwrap_or(now);
            let max = param(query, "max").and_then(|v| v.parse().ok()).unwrap_or(300);
            match store.series_json(&node, chan, from, to, max) {
                Some(s) => ok(s),
                None => Some(("404 Not Found", json!({"error": "No such channel"}).to_string())),
            }
        }
        ("GET", "/alarms") => {
            let Some(node) = param(query, "node").and_then(parse_addr) else { return bad("node?") };
            let list: Vec<J> = store
                .nodes
                .get(&node)
                .map(|n| n.alarms.iter().rev().map(|a| json!({"t": a.t, "rule": a.rule, "raised": a.raised, "text": a.text, "chan": a.chan, "value": scaled(a.kind, a.value), "unit": catalog::kind_unit(a.kind)})).collect())
                .unwrap_or_default();
            ok(J::Array(list))
        }
        ("GET", "/setup") => {
            let Some(node) = param(query, "node").and_then(parse_addr) else { return bad("node?") };
            ok(store.nodes.get(&node).and_then(|n| n.setup.clone()).unwrap_or(J::Null))
        }
        ("PUT", "/setup") => {
            let Some(node) = param(query, "node").and_then(parse_addr) else { return bad("node?") };
            let Ok(setup) = parsed() else { return bad("The setup is not JSON") };
            let n = store.nodes.entry(node).or_insert_with(|| SensorNode { first_seen: unix_now(), ..SensorNode::default() });
            n.setup = Some(setup);
            store.dirty = true;
            ok(json!({"ok": true}))
        }
        ("POST", "/meta") => {
            let Ok(v) = parsed() else { return bad("not JSON") };
            let Some(node) = v["node"].as_str().and_then(parse_addr) else { return bad("node?") };
            let n = store.nodes.entry(node).or_insert_with(|| SensorNode { first_seen: unix_now(), ..SensorNode::default() });
            if let Some(name) = v["name"].as_str() {
                n.name = name.trim().chars().filter(|c| !c.is_control()).take(40).collect();
            }
            if let Some(f) = v["favourite"].as_bool() {
                n.favourite = f;
            }
            store.dirty = true;
            ok(json!({"ok": true}))
        }
        ("POST", "/dismiss") => {
            let Ok(v) = parsed() else { return bad("not JSON") };
            let Some(node) = v["node"].as_str().and_then(parse_addr) else { return bad("node?") };
            store.dismiss(&node, unix_now());
            ok(json!({"ok": true}))
        }
        ("POST", "/forget") => {
            let Ok(v) = parsed() else { return bad("not JSON") };
            let Some(node) = v["node"].as_str().and_then(parse_addr) else { return bad("node?") };
            store.nodes.remove(&node);
            store.dirty = true;
            ok(json!({"ok": true}))
        }
        ("POST", "/compile") => {
            let Ok(setup) = parsed() else { return bad("The setup is not JSON") };
            ok(compile_json(&setup))
        }
        ("POST", "/decompile") => {
            let Ok(v) = parsed() else { return bad("not JSON") };
            let Some(bytes) = v["hex"].as_str().and_then(parse_hex) else { return bad("hex?") };
            match Recipe::from_bytes(&bytes) {
                Ok(r) => ok(decompile(&r)),
                Err(e) => bad(&format!("The recipe is not valid: {e:?}")),
            }
        }
        ("POST", "/simulate") => {
            let Ok(v) = parsed() else { return bad("not JSON") };
            match compile(&v["setup"]) {
                Ok(r) => ok(simulate(&r, v["scenario"].as_str().unwrap_or("cold_night"))),
                Err(e) => bad(&e),
            }
        }
        _ => Some(("404 Not Found", json!({"error": "Not found"}).to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frost_setup() -> J {
        json!({"v": 1, "interval_s": 900, "sleep": true,
               "report_to": {"owners": [0], "channel": false}, "alarm_to": {"owners": [0, 1], "channel": false},
               "sources": [
                   {"chan": 1, "kind": "temperature", "input": {"type": "bme280", "addr": 118, "what": "temperature"}},
                   {"chan": 2, "kind": "digital", "input": {"type": "gpio_in", "pin": 0, "pull": "up"}},
                   {"chan": 3, "kind": "digital", "input": {"type": "gpio_out", "pin": 6}}],
               "rules": [
                   {"id": 1, "when": {"type": "every", "secs": 900}, "then": [{"type": "report"}]},
                   {"id": 2, "when": {"type": "compare", "chan": 1, "op": "<", "value": 10.0, "clear": 12.0, "count": 2}, "limit_min": 30,
                    "then": [{"type": "alarm", "text": "Cellar is below 10 C"}, {"type": "set_out", "chan": 3, "on": true}]},
                   {"id": 3, "when": {"type": "edge", "chan": 2, "to": "high"}, "limit_min": 10,
                    "then": [{"type": "alarm", "text": "Cellar door opened"}]}]})
    }

    #[test]
    fn setup_compiles_round_trips_and_is_small() {
        let r = compile(&frost_setup()).unwrap();
        let bytes = r.to_bytes().unwrap();
        assert!(bytes.len() < 130, "{} bytes", bytes.len());
        // Decompiling and compiling again gives the same recipe.
        let setup2 = decompile(&Recipe::from_bytes(&bytes).unwrap());
        let r2 = compile(&setup2).unwrap();
        assert_eq!(r2, r);
        // Values come back in display units.
        assert_eq!(setup2["rules"][1]["when"]["value"], json!(10.0));
        assert_eq!(setup2["rules"][1]["when"]["clear"], json!(12.0));
        assert_eq!(setup2["rules"][1]["then"][0]["text"], json!("Cellar is below 10 C"));
        // Alarm recipients: owners 0 and 1.
        assert_eq!(r.alarm_to, 0b11);
    }

    #[test]
    fn bad_setups_say_what_is_wrong() {
        let mut s = frost_setup();
        s["rules"][1]["when"]["chan"] = json!(9);
        assert!(compile(&s).unwrap_err().contains("No source on channel 9"));
        let mut s = frost_setup();
        s["interval_s"] = json!(10);
        assert!(compile(&s).unwrap_err().contains("not valid"));
        let mut s = frost_setup();
        s["rules"][1]["then"][0]["text"] = json!("x".repeat(70));
        assert!(compile(&s).unwrap_err().contains("too long"));
        let mut s = frost_setup();
        s["report_to"] = json!({"owners": [5]});
        assert!(compile(&s).is_err());
        let mut s = frost_setup();
        s["report_to"] = json!({"owners": [0], "channel": true});
        assert!(compile(&s).is_err(), "a channel recipient needs a channel");
    }

    #[test]
    fn a_cold_night_raises_the_frost_alarm_once_and_runs_the_heater() {
        let r = compile(&frost_setup()).unwrap();
        let sim = simulate(&r, "cold_night");
        let events = sim["events"].as_array().unwrap();
        let raised: Vec<&J> = events.iter().filter(|e| e["type"] == "alarm" && e["raised"] == true).collect();
        assert_eq!(raised.len(), 1, "{events:?}");
        assert_eq!(raised[0]["text"], json!("Cellar is below 10 C"));
        assert!(events.iter().any(|e| e["type"] == "alarm" && e["raised"] == false), "cleared again");
        assert!(sim["summary"]["output_on_s"].as_u64().unwrap() > 3600);
        assert!(sim["summary"]["reports"].as_u64().unwrap() >= 90);
        assert!(sim["summary"]["airtime_percent"].as_f64().unwrap() < 1.0);
        // The door scenario fires the door alarm twice and no frost alarm.
        let sim = simulate(&r, "door");
        let doors = sim["events"].as_array().unwrap().iter().filter(|e| e["text"] == json!("Cellar door opened")).count();
        assert_eq!(doors, 2);
        // "silent": the node stops answering, so there are fewer cycles.
        let sim = simulate(&r, "silent");
        assert!(sim["summary"]["cycles"].as_u64().unwrap() < 50);
    }

    #[test]
    fn a_wake_pin_catches_the_post_that_a_slow_timer_misses() {
        let setup = |wake: bool| json!({"v": 1, "board": "xiao_nrf52840", "interval_s": 3600, "sleep": true,
            "report_to": {"owners": [0]}, "alarm_to": {"owners": [0]},
            "sources": [{"chan": 1, "kind": "digital", "input": {"type": "gpio_in", "pin": 0, "pull": "up", "invert": true, "wake": wake}}],
            "rules": [{"id": 1, "when": {"type": "edge", "chan": 1, "to": "high"}, "then": [{"type": "alarm", "text": "Post in the mailbox"}]}]});
        let count = |wake: bool| {
            let r = compile(&setup(wake)).unwrap();
            let sim = simulate(&r, "door");
            sim["events"].as_array().unwrap().iter().filter(|e| e["type"] == "alarm" && e["raised"] == true).count()
        };
        assert_eq!(count(true), 2, "woken by the pin at 08:10 and 14:30");
        assert_eq!(count(false), 0, "an hourly look misses a 20-minute opening at 08:10 only if it falls between looks");
        // A wake pin gets its pull from the wake level, and survives the round trip.
        let r = compile(&setup(true)).unwrap();
        assert!(matches!(r.sources[0].input, Input::GpioIn { pull: Pull::Up, invert: true, wake: true, .. }));
        assert_eq!(r.wake_pins().as_slice(), &[(0, false)]);
        let again = compile(&decompile(&Recipe::from_bytes(&r.to_bytes().unwrap()).unwrap())).unwrap();
        assert_eq!(again, r);
        // On the Heltec V4 only RTC pins can wake the node.
        let mut s = setup(true);
        s["board"] = json!("heltec_v4");
        s["sources"][0]["input"]["pin"] = json!(38);
        assert!(compile(&s).unwrap_err().contains("only GPIO 3, 4 and 6"));
        s["sources"][0]["input"]["pin"] = json!(4);
        assert!(compile(&s).is_ok());
    }

    #[test]
    fn readings_are_stored_deduplicated_and_listed() {
        let mut store = Store::default();
        let node = Address([7; 16]);
        let m = sensor::encode_readings(
            1,
            &[Sample { age_s: 120, values: heapless::Vec::from_slice(&[Value { chan: 1, kind: kind::TEMPERATURE, value: 850 }]).unwrap() }],
        )
        .0;
        assert!(store.ingest(node, &m, 10_000));
        assert!(store.ingest(node, &m, 10_000), "the same message again");
        let list = store.list_json(10_060);
        assert_eq!(list[0]["channels"][0]["value"], json!(8.5));
        let s = store.series_json(&node, 1, 0, 20_000, 100).unwrap();
        assert_eq!(s["points"].as_array().unwrap().len(), 1, "the repeat was dropped");
        assert_eq!(s["points"][0][0], json!(9880));
        // An alarm is active until its all-clear.
        let a = sensor::encode_alarm(1, true, 0, Value { chan: 1, kind: kind::TEMPERATURE, value: 850 }, "Frost");
        store.ingest(node, &a, 10_100);
        assert_eq!(store.list_json(10_100)[0]["alarm"]["text"], json!("Frost"));
        let c = sensor::encode_alarm(1, false, 0, Value { chan: 1, kind: kind::TEMPERATURE, value: 1300 }, "Frost");
        store.ingest(node, &c, 10_200);
        assert!(store.list_json(10_200)[0]["alarm"].is_null());
        // "Mark as read" clears an alarm that never clears itself (a mailbox).
        store.ingest(node, &a, 10_300);
        assert!(!store.list_json(10_300)[0]["alarm"].is_null());
        store.dismiss(&node, 10_400);
        assert!(store.list_json(10_400)[0]["alarm"].is_null());
    }

    #[test]
    fn a_host_keeps_only_so_many_sensors_and_so_many_samples() {
        let mut store = Store::default();
        let msg = sensor::encode_alarm(1, true, 0, Value { chan: 1, kind: 1, value: 0 }, "x");
        for i in 0..MAX_SENSORS + 5 {
            let mut a = [0u8; 16];
            a[..8].copy_from_slice(&(i as u64 + 1).to_le_bytes());
            store.ingest(Address(a), &msg, 1000);
        }
        assert_eq!(store.nodes.len(), MAX_SENSORS);
    }

    #[test]
    fn sensors_survive_a_restart() {
        let dir = std::env::temp_dir().join(format!("maarse-sensors-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut store = Store::default();
        store.open(&dir);
        let node = Address([3; 16]);
        let m = sensor::encode_readings(0, &[Sample { age_s: 0, values: heapless::Vec::from_slice(&[Value { chan: 2, kind: kind::HUMIDITY, value: 6100 }]).unwrap() }]).0;
        store.ingest(node, &m, 5000);
        store.nodes.get_mut(&node).unwrap().name = "Cellar".into();
        store.dirty = true;
        store.save();
        let mut again = Store::default();
        again.open(&dir);
        assert_eq!(again.list_json(5100)[0]["name"], json!("Cellar"));
        assert_eq!(again.list_json(5100)[0]["channels"][0]["value"], json!(61.0));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_api_compiles_and_lists() {
        let body = frost_setup().to_string();
        let (status, out) = api("POST", "/api/sensors/compile", "", body.as_bytes()).unwrap();
        assert_eq!(status, "200 OK");
        let v: J = serde_json::from_str(&out).unwrap();
        assert_eq!(v["ok"], json!(true), "{v}");
        assert!(v["size"].as_u64().unwrap() < 130);
        let (_, out) = api("POST", "/api/sensors/decompile", "", json!({"hex": v["hex"]}).to_string().as_bytes()).unwrap();
        let s: J = serde_json::from_str(&out).unwrap();
        assert_eq!(s["rules"].as_array().unwrap().len(), 3);
        let (status, _) = api("GET", "/api/sensors/series", "node=zz", b"").unwrap();
        assert_eq!(status, "400 Bad Request");
        assert!(api("GET", "/api/other", "", b"").is_none());
    }

    fn setup_of(board: &str, sources: J) -> J {
        json!({"v": 1, "board": board, "interval_s": 900, "sleep": true, "report_to": {"owners": [0]}, "alarm_to": {"owners": [0]},
               "sources": sources, "rules": [{"id": 1, "when": {"type": "every", "secs": 900}, "then": [{"type": "report"}]}]})
    }

    #[test]
    fn catalogue_custom_and_mapped_sensors_compile_round_trip_and_simulate() {
        let sources = json!([
            {"chan": 1, "kind": "temperature", "input": {"type": "i2c", "device": "sht3x", "what": 0}},
            {"chan": 2, "kind": "humidity", "input": {"type": "i2c", "device": "sht3x", "what": "Humidity"}},
            {"chan": 3, "kind": "percent", "input": {"type": "i2c", "device": "ads1115", "addr": 72, "what": 0}, "map": {"in": [2600, 1200], "out": [0, 100]}},
            {"chan": 4, "kind": "custom", "input": {"type": "i2c_custom", "addr": 64, "ptr": "05", "read_len": 2,
                "field": {"bits": 16, "signed": true, "mul": 1, "div": 10}}}]);
        let setup = setup_of("heltec_v4", sources);
        let r = compile(&setup).unwrap();
        assert_eq!(r.maps.len(), 1);
        let b = r.to_bytes().unwrap();
        let back = Recipe::from_bytes(&b).unwrap();
        assert_eq!(back, r);
        // Round trip through the designer's JSON keeps the map in display units and the custom sensor.
        let again = decompile(&back);
        assert_eq!(again["sources"][2]["map"], json!({"in": [2600, 1200], "out": [0.0, 100.0]}));
        assert_eq!(again["sources"][1]["input"]["device"], json!("sht3x"));
        assert_eq!(again["sources"][3]["input"]["ptr"], json!("05"));
        let mut again_setup = again.clone();
        again_setup["board"] = json!("heltec_v4");
        assert_eq!(compile(&again_setup).unwrap(), r);
        // The simulator turns 1200 mV into a percentage, as the node would.
        let sim = simulate(&r, "cold_night");
        assert_eq!(sim["series"]["3"][0], json!(100.0));
    }

    #[test]
    fn a_custom_sensor_says_what_is_wrong() {
        let src = |input: J| setup_of("xiao_nrf52840", json!([{"chan": 1, "kind": "custom", "input": input}]));
        let ok = json!({"type": "i2c_custom", "addr": 64, "ptr": "05", "read_len": 2, "field": {"bits": 16}});
        assert!(compile(&src(ok.clone())).is_ok());
        let mut bad = ok.clone();
        bad["addr"] = json!(200);
        assert!(compile(&src(bad)).unwrap_err().contains("address"));
        let mut bad = ok.clone();
        bad["field"]["bits"] = json!(24);
        assert!(compile(&src(bad)).unwrap_err().contains("bits are inside"));
        let mut bad = ok.clone();
        bad["trigger"] = json!("zz");
        assert!(compile(&src(bad)).unwrap_err().contains("hex"));
        let mut bad = ok.clone();
        bad["ptr"] = json!("010203");
        assert!(compile(&src(bad)).unwrap_err().contains("at most 2"));
        let mut bad = ok.clone();
        bad["init"] = json!(["0102", "0304", "0506"]);
        assert!(compile(&src(bad)).unwrap_err().contains("too long"));
        let unknown = src(json!({"type": "i2c", "device": "nope"}));
        assert!(compile(&unknown).unwrap_err().contains("Unknown sensor"));
        let wrong = src(json!({"type": "i2c", "device": "sht3x", "addr": 80}));
        assert!(compile(&wrong).unwrap_err().contains("cannot have the address"));
    }

    #[test]
    fn the_board_decides_which_sensors_fit() {
        let pin = |p: u32| json!([{"chan": 1, "kind": "digital", "input": {"type": "gpio_in", "pin": p, "pull": "up"}}]);
        let i2c = json!([{"chan": 1, "kind": "temperature", "input": {"type": "i2c", "device": "sht3x"}}]);
        assert!(compile(&setup_of("xiao_nrf52840", pin(0))).is_ok());
        assert!(compile(&setup_of("xiao_nrf52840", pin(3))).unwrap_err().contains("not free"));
        assert!(compile(&setup_of("xiao_nrf52840", i2c.clone())).is_ok());
        // The I2C bus takes D6 and D7.
        let both = json!([{"chan": 1, "kind": "temperature", "input": {"type": "i2c", "device": "sht3x"}},
                          {"chan": 2, "kind": "digital", "input": {"type": "gpio_in", "pin": 6, "pull": "up"}}]);
        assert!(compile(&setup_of("xiao_nrf52840", both)).unwrap_err().contains("I2C bus"));
        // An analogue pin only where the board has one.
        let adc = json!([{"chan": 1, "kind": "voltage", "input": {"type": "adc", "pin": 4}}]);
        assert!(compile(&setup_of("heltec_v4", adc.clone())).is_ok());
        assert!(compile(&setup_of("xiao_nrf52840", adc)).unwrap_err().contains("ADS1115"));
        // A board without firmware for sensors is listed but cannot be chosen.
        assert!(compile(&setup_of("xiao_esp32s3", i2c)).unwrap_err().contains("cannot read sensors yet"));
        assert!(compile(&setup_of("nope", pin(0))).unwrap_err().contains("Unknown board"));
    }

    #[test]
    fn the_catalogue_is_served_and_a_co2_sensor_on_a_short_interval_is_warned_about() {
        let (status, body) = api("GET", "/api/sensors/catalog", "", b"").unwrap();
        assert_eq!(status, "200 OK");
        let c: J = serde_json::from_str(&body).unwrap();
        assert!(c["sensors"].as_array().unwrap().iter().any(|s| s["id"] == "scd41" && s["channels"].as_array().unwrap().len() == 3));
        assert!(c["boards"].as_array().unwrap().iter().any(|b| b["id"] == "xiao_esp32s3" && b["available"] == false));
        let mut setup = setup_of("heltec_v4", json!([{"chan": 1, "kind": "co2", "input": {"type": "i2c", "device": "scd41"}}]));
        setup["interval_s"] = json!(60);
        let out = compile_json(&setup);
        assert_eq!(out["ok"], json!(true), "{out}");
        assert!(out["warnings"].as_array().unwrap().iter().any(|w| w.as_str().unwrap().contains("5 seconds")));
    }
}
