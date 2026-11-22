//! What the sensor designer can offer: the boards and the sensors. It is data
//! (the repository's `sensors/` folder, merged into `sensors/catalog.json`),
//! not code: the host embeds a copy and can replace it with a newer, signed one
//! from GitHub (see `catalog_update.rs`). The web client and the Android app
//! both draw their pickers from `GET /api/sensors/catalog`, so a new sensor or
//! board shows up in both at once.
//!
//! An I2C chip is a description (how to make it answer, which bits are the
//! number) that goes into the recipe, so a node needs no update to read it.

use maarse_core::sensor::catalog::{self, Field, I2cDesc};
use maarse_core::sensor::{Input, Recipe, kind};
use serde::Deserialize;
use serde_json::{Value as J, json};
use std::sync::{Arc, LazyLock, RwLock};

/// The copy that ships with this build (works offline).
const EMBEDDED: &str = include_str!("../../../sensors/catalog.json");
/// The catalogue format this host understands.
pub const SCHEMA: u64 = 1;

// ------------------------------------------------------------------ boards

#[derive(Deserialize, Clone, Debug)]
pub struct PinSpec {
    pub id: u8,
    pub label: String,
}

#[derive(Deserialize, Clone, Debug)]
pub struct I2cPins {
    pub sda: u8,
    pub scl: u8,
    #[serde(default)]
    pub text: String,
}

#[derive(Deserialize, Clone, Debug)]
pub struct AnalogPin {
    pub id: u8,
    pub label: String,
    #[serde(default)]
    pub max_mv: u32,
}

/// A board a sensor can run on.
#[derive(Deserialize, Clone, Debug)]
pub struct Board {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub note: String,
    /// The firmware reads sensors on this board. A board that is not ready is
    /// listed (so people see what is coming) but cannot be chosen.
    pub available: bool,
    /// Free pins: the pin id the recipe uses and its label on the board.
    pub pins: Vec<PinSpec>,
    pub i2c: Option<I2cPins>,
    #[serde(default)]
    pub analog: Vec<AnalogPin>,
    /// Pins that can wake the node from sleep (`None`: all free pins).
    pub wake: Option<Vec<u8>>,
    #[serde(default)]
    pub wake_text: String,
    #[serde(default)]
    pub grove: String,
}

// ------------------------------------------------------------------ kinds

/// A kind of measurement: what the clients call it, its unit and how the
/// whole numbers in a reading become that unit (`value / scale`). The recipe
/// and the nodes only know the number `id`; a new kind from the catalogue uses an
/// id from 128 up, which every firmware accepts.
#[derive(Deserialize, Clone, Debug)]
pub struct KindSpec {
    pub key: String,
    pub id: u8,
    pub name: String,
    #[serde(default)]
    pub unit: String,
    pub scale: i32,
}

fn kind_id_in(kinds: &[KindSpec], v: &J) -> Result<u8, String> {
    if let Some(n) = v.as_u64() {
        return u8::try_from(n).ok().filter(|k| kinds.iter().any(|x| x.id == *k) || kind::is_known(*k)).ok_or_else(|| format!("Unknown kind {n}"));
    }
    let s = v.as_str().ok_or("A source needs a kind")?;
    kinds.iter().find(|k| k.key == s).map(|k| k.id).ok_or_else(|| format!("Unknown kind \"{s}\""))
}

// ------------------------------------------------------------------ chips

#[derive(Deserialize, Clone, Debug, Default)]
struct ReadSpec {
    #[serde(default)]
    init: Vec<String>,
    #[serde(default)]
    trigger: String,
    #[serde(default)]
    wait_ms: u32,
    #[serde(default)]
    ptr: String,
    #[serde(default = "two")]
    read_len: u32,
    #[serde(default)]
    crc: bool,
}

fn two() -> u32 {
    2
}

#[derive(Deserialize, Clone, Debug)]
struct FieldSpec {
    bit_off: u32,
    bits: u32,
    #[serde(default)]
    signed: bool,
    #[serde(default)]
    little: bool,
    mul: i64,
    div: i64,
    #[serde(default)]
    add: i64,
}

#[derive(Deserialize, Clone, Debug)]
struct ChanSpec {
    name: String,
    kind: String,
    /// The BME280 driver's measurement number.
    what: Option<u8>,
    /// A chip with one register per value: replaces the chip's read.
    read: Option<ReadSpec>,
    field: Option<FieldSpec>,
}

#[derive(Deserialize, Clone, Debug)]
struct TestSpec {
    channel: usize,
    answer: String,
    value: f64,
}

#[derive(Deserialize, Clone, Debug)]
struct ChipSpec {
    id: String,
    name: String,
    #[serde(default)]
    addrs: Vec<u8>,
    #[serde(default)]
    measure_ms: u32,
    /// A sensor with its own firmware driver instead of a description (`bme280`).
    driver: Option<String>,
    read: Option<ReadSpec>,
    channels: Vec<ChanSpec>,
    #[serde(default)]
    tests: Vec<TestSpec>,
}

fn unhex(t: &str) -> Result<Vec<u8>, String> {
    let t: String = t.chars().filter(|c| !c.is_whitespace() && *c != '_').collect();
    if t.len() % 2 != 0 {
        return Err(format!("\"{t}\" is not whole hex bytes"));
    }
    (0..t.len() / 2).map(|i| u8::from_str_radix(&t[2 * i..2 * i + 2], 16).map_err(|_| format!("\"{t}\" is not hex"))).collect()
}

impl ChipSpec {
    /// How to read channel number `what` of this chip at `addr`.
    fn desc(&self, addr: u8, what: usize) -> Result<I2cDesc, String> {
        let ch = self.channels.get(what).ok_or_else(|| format!("{} has no measurement number {what}", self.name))?;
        let read = ch.read.as_ref().or(self.read.as_ref()).ok_or_else(|| format!("{}: no read description", self.id))?;
        let f = ch.field.as_ref().ok_or_else(|| format!("{}: {} has no field", self.id, ch.name))?;
        let mut d = I2cDesc::new(addr);
        let mut left = 0usize;
        for cmd in &read.init {
            let b = unhex(cmd)?;
            if !(1..=4).contains(&b.len()) || left + 1 + b.len() > catalog::MAX_INIT {
                return Err(format!("{}: the set-up commands do not fit", self.id));
            }
            d.init[left] = b.len() as u8;
            d.init[left + 1..left + 1 + b.len()].copy_from_slice(&b);
            left += 1 + b.len();
        }
        d.init_len = left as u8;
        let t = unhex(&read.trigger)?;
        let p = unhex(&read.ptr)?;
        if t.len() > catalog::MAX_TRIGGER || p.len() > catalog::MAX_PTR {
            return Err(format!("{}: the trigger or the register is too long", self.id));
        }
        d.trigger[..t.len()].copy_from_slice(&t);
        d.trigger_len = t.len() as u8;
        d.ptr[..p.len()].copy_from_slice(&p);
        d.ptr_len = p.len() as u8;
        d.wait_ms = u16::try_from(read.wait_ms).map_err(|_| format!("{}: the wait is too long", self.id))?;
        d.read_len = u8::try_from(read.read_len).map_err(|_| format!("{}: too many bytes", self.id))?;
        d.crc = read.crc;
        let i32_of = |v: i64| i32::try_from(v).map_err(|_| format!("{}: a number is out of range", self.id));
        d.field = Field {
            bit_off: u8::try_from(f.bit_off).map_err(|_| format!("{}: bit_off is out of range", self.id))?,
            bits: u8::try_from(f.bits).map_err(|_| format!("{}: bits is out of range", self.id))?,
            signed: f.signed,
            little: f.little,
            mul: i32_of(f.mul)?,
            div: i32_of(f.div)?,
            add: i32_of(f.add)?,
        };
        if d.validate() { Ok(d) } else { Err(format!("{}: {} is not a valid description", self.id, ch.name)) }
    }

    /// What a client needs to show the sensor.
    fn ui(&self, item: &J) -> J {
        let chans: Vec<J> = self.channels.iter().enumerate().map(|(i, c)| json!({"what": c.what.unwrap_or(i as u8), "name": c.name, "kind": c.kind})).collect();
        json!({"id": self.id, "name": self.name, "title": item["title"], "advanced": item["advanced"], "group": item["group"], "bus": "i2c", "device": self.id, "grove": item["grove"], "note": item["note"],
               "addrs": self.addrs, "measure_ms": self.measure_ms, "driver": self.driver, "channels": chans})
    }

    /// Every address and channel must give a valid description, and every
    /// test from the data sheet must give its number.
    fn check(&self, kinds: &[KindSpec]) -> Result<(), String> {
        if self.name.is_empty() || self.channels.is_empty() {
            return Err(format!("{}: needs a name and at least one channel", self.id));
        }
        if self.driver.is_some() {
            return Ok(());
        }
        if self.addrs.is_empty() {
            return Err(format!("{}: needs at least one address", self.id));
        }
        if self.tests.is_empty() {
            return Err(format!("{}: needs tests with numbers from the data sheet", self.id));
        }
        for &a in &self.addrs {
            for i in 0..self.channels.len() {
                self.desc(a, i)?;
            }
        }
        for (n, t) in self.tests.iter().enumerate() {
            let d = self.desc(self.addrs[0], t.channel)?;
            let ch = &self.channels[t.channel];
            let answer = unhex(&t.answer)?;
            let k = kind_id_in(kinds, &json!(ch.kind))?;
            let scale = kinds.iter().find(|x| x.id == k).map_or(1, |x| x.scale);
            let got = d.extract(&answer).ok_or_else(|| format!("{}: test {n} could not be read (check bytes?)", self.id))?;
            let want = (t.value * scale as f64).round() as i32;
            if got != want {
                return Err(format!("{}: test {n} ({}) gives {got}, the data sheet says {want}", self.id, ch.name));
            }
        }
        Ok(())
    }
}

// --------------------------------------------------------------- the catalogue

#[derive(Debug)]
pub struct Catalog {
    pub version: u64,
    boards: Vec<Board>,
    /// The sensors as the clients see them, in file order.
    ui: Vec<J>,
    kinds: Vec<KindSpec>,
    /// Starting points: a title, a sentence and the first version of a setup.
    templates: Vec<J>,
    chips: Vec<ChipSpec>,
}

impl Catalog {
    /// Reads and checks a catalogue: every description must fit together and
    /// every test from a data sheet must give its number.
    pub fn parse(text: &str) -> Result<Catalog, String> {
        let root: J = serde_json::from_str(text).map_err(|e| format!("The sensor catalogue is not JSON: {e}"))?;
        let schema = root["schema"].as_u64().ok_or("The catalogue has no schema number")?;
        if schema != SCHEMA {
            return Err(format!("The catalogue has format {schema}; this host reads format {SCHEMA}. Update the host."));
        }
        let version = root["version"].as_u64().ok_or("The catalogue has no version")?;
        let boards: Vec<Board> = serde_json::from_value(root["boards"].clone()).map_err(|e| format!("The boards are not valid: {e}"))?;
        let kinds: Vec<KindSpec> = serde_json::from_value(root["kinds"].clone()).map_err(|e| format!("The kinds are not valid: {e}"))?;
        let mut keys = std::collections::HashSet::new();
        let mut ids = std::collections::HashSet::new();
        for k in &kinds {
            // The nodes check a recipe's kinds: 1 to 11 are theirs, 128 and up are free.
            if k.scale < 1 || !(kind::is_known(k.id)) || !keys.insert(k.key.clone()) || !ids.insert(k.id) {
                return Err(format!("The kind \"{}\" is not valid (ids are 1 to 11 or 128 and up, keys and ids are unique, scale is at least 1)", k.key));
            }
        }
        let templates: Vec<J> = root["templates"].as_array().cloned().unwrap_or_default();
        let (mut ui, mut chips) = (Vec::new(), Vec::new());
        let mut seen = std::collections::HashSet::new();
        for item in root["sensors"].as_array().ok_or("The catalogue has no sensors")? {
            let id = item["id"].as_str().ok_or("A sensor has no id")?;
            if !seen.insert(id.to_string()) {
                return Err(format!("The sensor id \"{id}\" is used twice"));
            }
            match item["bus"].as_str() {
                Some("i2c") => {
                    let chip: ChipSpec = serde_json::from_value(item.clone()).map_err(|e| format!("{id}: {e}"))?;
                    chip.check(&kinds)?;
                    ui.push(chip.ui(item));
                    chips.push(chip);
                }
                Some("analog" | "digital_in" | "digital_out") => {
                    for k in ["name", "group", "kind"] {
                        if item[k].as_str().is_none_or(str::is_empty) {
                            return Err(format!("{id}: missing {k}"));
                        }
                    }
                    kind_id_in(&kinds, &item["kind"]).map_err(|e| format!("{id}: {e}"))?;
                    ui.push(item.clone());
                }
                other => return Err(format!("{id}: unknown bus {other:?}")),
            }
        }
        // The same order in every client: how a person looks for a sensor, not the order of the files.
        const GROUPS: [&str; 10] = [
            "Temperature, humidity and pressure",
            "Air quality",
            "Light",
            "Soil, water and weather",
            "Motion",
            "Sound",
            "Buttons and switches",
            "Analogue inputs",
            "Switch something on",
            "Other",
        ];
        ui.sort_by_key(|s: &J| (GROUPS.iter().position(|g| Some(*g) == s["group"].as_str()).unwrap_or(GROUPS.len()), s["title"].as_str().or(s["name"].as_str()).unwrap_or("").to_lowercase()));
        Ok(Catalog { version, boards, ui, chips, kinds, templates })
    }

    /// The number of a kind, from its key ("co2") or its number.
    pub fn kind_id(&self, v: &J) -> Result<u8, String> {
        kind_id_in(&self.kinds, v)
    }

    fn kind(&self, id: u8) -> Option<&KindSpec> {
        self.kinds.iter().find(|k| k.id == id)
    }

    pub fn board(&self, id: &str) -> Option<&Board> {
        self.boards.iter().find(|b| b.id == id)
    }

    fn chip(&self, id: &str) -> Option<&ChipSpec> {
        self.chips.iter().find(|c| c.id == id)
    }

    /// The recipe input for channel `what` (number or name) of `device` at `addr`
    /// (default: the chip's usual one).
    pub fn i2c_input(&self, device: &str, addr: Option<u8>, what: &J) -> Result<Input, String> {
        let chip = self.chip(device).ok_or_else(|| format!("Unknown sensor \"{device}\""))?;
        let addr = addr.or(chip.addrs.first().copied()).ok_or("This sensor has no address")?;
        if !chip.addrs.contains(&addr) {
            return Err(format!("A {} cannot have the address 0x{addr:02x}", chip.name));
        }
        let idx = match what {
            J::Number(n) => n.as_u64().map(|n| n as usize),
            J::String(w) => chip.channels.iter().position(|c| c.name.eq_ignore_ascii_case(w)),
            _ => Some(0),
        }
        .filter(|i| *i < chip.channels.len())
        .ok_or_else(|| format!("A {} has no measurement {what}", chip.name))?;
        match &chip.driver {
            Some(d) if d == "bme280" => Ok(Input::Bme280 { addr, what: chip.channels[idx].what.unwrap_or(idx as u8) }),
            Some(d) => Err(format!("The {} needs the \"{d}\" driver, which this host does not know. Update the host.", chip.name)),
            None => chip.desc(addr, idx).map(Input::I2c),
        }
    }

    /// Which catalogue chip a description came from (to show a recipe read
    /// back from a node by its name): (device, address, measurement number).
    pub fn recognise(&self, d: &I2cDesc) -> Option<(String, u8, usize)> {
        for chip in self.chips.iter().filter(|c| c.driver.is_none() && c.addrs.contains(&d.addr)) {
            for i in 0..chip.channels.len() {
                if chip.desc(d.addr, i).is_ok_and(|x| x == *d) {
                    return Some((chip.id.clone(), d.addr, i));
                }
            }
        }
        None
    }

    pub fn to_json(&self) -> J {
        json!({"v": 1, "version": self.version, "boards": self.boards_json(), "sensors": self.ui,
               "kinds": self.kinds.iter().map(|k| json!({"key": k.key, "id": k.id, "name": k.name, "unit": k.unit, "scale": k.scale})).collect::<Vec<_>>(),
               "templates": self.templates,
               "custom_i2c": {"max": maarse_core::sensor::MAX_READS,
                              "help": "Describe a chip the catalogue does not have: the address, what to send to start a measurement, how long to wait, which register to read and which bits are the number."},
               "update": super::catalog_update::status_json()})
    }

    fn boards_json(&self) -> Vec<J> {
        self.boards
            .iter()
            .map(|b| {
                json!({"id": b.id, "name": b.name, "note": b.note, "available": b.available,
                       "pins": b.pins.iter().map(|p| json!({"id": p.id, "label": p.label})).collect::<Vec<_>>(),
                       "i2c": b.i2c.as_ref().map(|i| json!({"sda": i.sda, "scl": i.scl, "text": i.text})),
                       "analog": b.analog.iter().map(|a| json!({"id": a.id, "label": a.label, "max_mv": a.max_mv})).collect::<Vec<_>>(),
                       "wake": b.wake, "grove": b.grove})
            })
            .collect()
    }
}

static CURRENT: LazyLock<RwLock<Arc<Catalog>>> = LazyLock::new(|| RwLock::new(Arc::new(Catalog::parse(EMBEDDED).expect("the embedded sensor catalogue is valid"))));

/// The catalogue in use: the newest signed one, or the one that came with this build.
pub fn current() -> Arc<Catalog> {
    CURRENT.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The version of the catalogue that came with this build.
pub fn embedded_version() -> u64 {
    serde_json::from_str::<J>(EMBEDDED).ok().and_then(|v| v["version"].as_u64()).unwrap_or(0)
}

pub fn install(c: Catalog) {
    *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(c);
}

/// How many units a whole number of this kind has (100 for hundredths of a degree).
pub fn kind_scale(k: u8) -> i32 {
    current().kind(k).map_or_else(|| kind::scale(k), |x| x.scale)
}

pub fn kind_unit(k: u8) -> String {
    current().kind(k).map_or_else(|| kind::unit(k).to_string(), |x| x.unit.clone())
}

pub fn kind_name(k: u8) -> String {
    current().kind(k).map_or_else(|| kind::name(k).to_string(), |x| x.name.clone())
}

/// The key of a kind ("co2"), or its number when the catalogue does not know it.
pub fn kind_key(k: u8) -> J {
    current().kind(k).map_or(json!(k), |x| json!(x.key))
}

pub fn catalog_json() -> J {
    current().to_json()
}

/// Checks that a recipe fits the board it is for.
pub fn check_board(id: &str, r: &Recipe) -> Result<(), String> {
    let cat = current();
    let b = cat.board(id).ok_or_else(|| format!("Unknown board \"{id}\""))?;
    if !b.available {
        return Err(format!("The firmware for the {} cannot read sensors yet", b.name));
    }
    let uses_i2c = r.sources.iter().any(|s| matches!(s.input, Input::Bme280 { .. } | Input::I2c(_)));
    if uses_i2c && b.i2c.is_none() {
        return Err(format!("The {} has no pins for an I2C sensor", b.name));
    }
    let label = |p: u8| b.pins.iter().find(|x| x.id == p).map_or(format!("pin {p}"), |x| x.label.clone());
    for s in &r.sources {
        if let Input::GpioIn { pin, .. } | Input::GpioOut { pin, .. } | Input::Counter { pin, .. } = s.input {
            if !b.pins.iter().any(|x| x.id == pin) {
                return Err(format!("Pin {pin} is not free on the {}", b.name));
            }
            // The I2C bus takes its two pins.
            if uses_i2c && b.i2c.as_ref().is_some_and(|i| pin == i.sda || pin == i.scl) {
                return Err(format!("{} is used by the I2C bus: choose another pin or drop the I2C sensor", label(pin)));
            }
        }
        if let Input::Adc { pin, .. } = s.input
            && !b.analog.iter().any(|a| a.id == pin)
        {
            return Err(format!("Pin {pin} has no analogue input on the {}: use an ADS1115 on I2C", b.name));
        }
    }
    if let Some(ok) = &b.wake
        && r.wake_pins().iter().any(|(p, _)| !ok.contains(p))
    {
        return Err(format!("On the {} only {} can wake the sensor", b.name.split(" + ").next().unwrap_or(&b.name), b.wake_text));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_catalogue_is_valid_and_every_data_sheet_test_passes() {
        // `current()` parsing the embedded copy runs every description and every test.
        let c = current();
        assert!(c.version > 0);
        assert!(c.ui.len() >= 30, "{} sensors", c.ui.len());
        assert!(c.chips.iter().filter(|x| x.driver.is_none()).all(|x| !x.tests.is_empty()));
        // Grouped in a fixed order: the groups of the list never interleave.
        let groups: Vec<&str> = c.ui.iter().map(|s| s["group"].as_str().unwrap()).collect();
        let mut seen: Vec<&str> = Vec::new();
        for g in groups {
            if seen.last() != Some(&g) {
                assert!(!seen.contains(&g), "group {g} appears in two places");
                seen.push(g);
            }
        }
        assert_eq!(seen.first(), Some(&"Temperature, humidity and pressure"));
        for b in &c.boards {
            for w in b.wake.iter().flatten() {
                assert!(b.pins.iter().any(|p| p.id == *w), "{}: wake pin {w}", b.id);
            }
            for a in &b.analog {
                assert!(b.pins.iter().any(|p| p.id == a.id), "{}: analogue pin {}", b.id, a.id);
            }
        }
    }

    #[test]
    fn a_wrong_number_in_a_test_is_caught() {
        let mut v: J = serde_json::from_str(EMBEDDED).unwrap();
        let sht = v["sensors"].as_array_mut().unwrap().iter_mut().find(|s| s["id"] == "sht3x").unwrap();
        sht["tests"][0]["value"] = json!(26.0);
        let err = Catalog::parse(&v.to_string()).err().unwrap();
        assert!(err.contains("sht3x") && err.contains("data sheet"), "{err}");
        // No tests, no entry.
        let mut v: J = serde_json::from_str(EMBEDDED).unwrap();
        v["sensors"].as_array_mut().unwrap().iter_mut().find(|s| s["id"] == "tmp117").unwrap()["tests"] = json!([]);
        assert!(Catalog::parse(&v.to_string()).err().unwrap().contains("needs tests"));
        // A catalogue in a newer format is refused with a clear message.
        let mut v: J = serde_json::from_str(EMBEDDED).unwrap();
        v["schema"] = json!(2);
        assert!(Catalog::parse(&v.to_string()).err().unwrap().contains("Update the host"));
    }

    #[test]
    fn a_chip_becomes_a_description_and_is_recognised_again() {
        let c = current();
        let h = c.i2c_input("sht3x", None, &json!("Humidity")).unwrap();
        let Input::I2c(d) = h else { panic!("{h:?}") };
        assert_eq!(d.addr, 0x44);
        assert_eq!(d.field.bit_off, 16);
        assert_eq!(c.recognise(&d), Some(("sht3x".to_string(), 0x44, 1)));
        // Another address, a chip with one register per value, and a chip with a driver.
        assert!(matches!(c.i2c_input("sht3x", Some(0x45), &json!(0)), Ok(Input::I2c(x)) if x.addr == 0x45));
        let Input::I2c(cold) = c.i2c_input("mcp9600", None, &json!(1)).unwrap() else { panic!() };
        assert_eq!(cold.ptr_bytes(), &[0x02]);
        assert!(matches!(c.i2c_input("bme280", Some(0x77), &json!(2)), Ok(Input::Bme280 { addr: 0x77, what: 2 })));
        assert!(c.i2c_input("sht3x", Some(0x50), &json!(0)).unwrap_err().contains("cannot have the address"));
        assert!(c.i2c_input("nope", None, &json!(0)).unwrap_err().contains("Unknown sensor"));
        assert!(c.i2c_input("sht3x", None, &json!(9)).is_err());
    }

    #[test]
    fn every_starting_point_makes_a_valid_setup_and_the_kinds_are_consistent() {
        let c = current();
        assert!(c.templates.len() >= 8);
        for k in &c.kinds {
            // Kinds the nodes know keep the numbers and units they have always had.
            if k.id <= 11 {
                assert_eq!(k.scale, kind::scale(k.id), "{}", k.key);
                assert_eq!(k.unit, kind::unit(k.id), "{}", k.key);
            }
        }
        for tpl in &c.templates {
            // What a client does: the defaults, then the starting point on top.
            let mut setup = json!({"v": 1, "board": "xiao_nrf52840", "interval_s": 900, "sleep": true,
                                   "report_to": {"owners": [0], "channel": false}, "alarm_to": {"owners": [0], "channel": false}, "channel": ""});
            for (k, v) in tpl["setup"].as_object().unwrap() {
                setup[k] = v.clone();
            }
            let out = super::super::compile_json(&setup);
            assert_eq!(out["ok"], json!(true), "{}: {out}", tpl["id"]);
        }
    }

    #[test]
    fn a_new_kind_from_the_catalogue_works_without_a_new_program() {
        let mut v: J = serde_json::from_str(EMBEDDED).unwrap();
        v["kinds"].as_array_mut().unwrap().push(json!({"key": "dust", "id": 130, "name": "Dust", "unit": "ug/m3", "scale": 10}));
        v["sensors"].as_array_mut().unwrap().push(json!({"id": "dust_pin", "name": "Dust", "group": "Air quality", "bus": "analog", "kind": "dust"}));
        let c = Catalog::parse(&v.to_string()).unwrap();
        assert_eq!(c.kind_id(&json!("dust")), Ok(130));
        assert_eq!(c.kind(130).map(|k| (k.scale, k.unit.as_str())), Some((10, "ug/m3")));
        // The nodes only accept kinds 1 to 11 and 128 up: anything else would be refused on the node.
        v["kinds"].as_array_mut().unwrap().push(json!({"key": "bad", "id": 50, "name": "Bad", "unit": "", "scale": 1}));
        assert!(Catalog::parse(&v.to_string()).err().unwrap().contains("not valid"));
    }
}
