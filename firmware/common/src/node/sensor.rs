//! Sensor node: runs a recipe (`maarse_core::sensor`) in cycles.
//!
//! One cycle: ask the board for the readings of the recipe's sources, run the
//! engine, send what it asks for (readings and alarms) as encrypted direct
//! messages to the owners, wait for the acknowledgements, and then, on
//! battery, sleep until the next cycle. Readings that are not acknowledged
//! stay in a small backlog and go along with the next cycle.
//!
//! Owners are persons: the node keeps each owner's master public identity and
//! the latest signed device list (`maarse_crypto::person`). A device in a
//! list is an owner's device: it may send commands, and readings go to it.
//! A newer list (sent as a `PERSON::LIST` message) replaces the old one,
//! which is also how a lost phone is removed.

use super::SENSOR;
use super::{FwNode, Settings, emit, log};
use crate::bus;
use core::sync::atomic::Ordering::Relaxed;
use embassy_time::{Duration, Instant};
use heapless::Vec;
use maarse_core::Address;
use maarse_core::companion::{Event, Text};
use maarse_core::content;
use maarse_core::sensor::{self, Effect, Engine, Input, MAX_RECIPE, Recipe, RecipeAssembler, Sample, Status, Value, op};
use maarse_crypto::PublicIdentity;
use maarse_crypto::person::DeviceList;
use rand_core::{CryptoRng, RngCore};

/// How long the board may take to read the sources.
const SAMPLE_WAIT: Duration = Duration::from_secs(6);
/// How long a node stays awake for acknowledgements after it has sent.
const ACK_WAIT: Duration = Duration::from_secs(40);
/// Awake at least this long after a cycle, so replies to a mail sync can arrive.
const MIN_AWAKE: Duration = Duration::from_secs(8);
/// Ask the post offices for mail (new recipes wait there) this often.
const MAIL_EVERY: Duration = Duration::from_secs(3600);
/// Only sleep if the next cycle is at least this far away.
const MIN_SLEEP_S: u64 = 20;
/// Tries an alarm gets before it is given up.
const ALARM_TRIES: u8 = 6;
const BACKLOG: usize = 8;
const ALARMS: usize = 4;
const CHUNK: usize = 180;
pub(super) const OWNERS_MAX: usize = 4 * (2 + 64 + 1 + 137);

#[derive(Clone)]
pub(super) struct Owner {
    master: PublicIdentity,
    list: Option<DeviceList>,
}

impl Owner {
    fn person(&self) -> Address {
        self.master.address()
    }
    fn has(&self, device: &Address) -> bool {
        self.list.as_ref().is_some_and(|l| l.active().any(|d| d == *device))
    }
}

enum Phase {
    Idle,
    Sampling(Instant),
    /// Sent; waiting for acknowledgements until `until`.
    Sending { since: Instant, until: Instant },
}

struct Entry {
    uid: u8,
    t_s: u32,
    /// Owners (bit per slot) that have not acknowledged it, and the channel bit.
    to: u8,
    values: Vec<Value, { sensor::MAX_SOURCES }>,
}

struct AlarmOut {
    uid: u8,
    rule: u8,
    raised: bool,
    t_s: u32,
    value: Value,
    off: u8,
    len: u8,
    to: u8,
    tries: u8,
}

enum What {
    Readings(Vec<u8, 8>),
    Alarm(u8),
}

struct Pending {
    id: u64,
    owner: u8,
    what: What,
}

pub(super) struct Sensor {
    owners: [Option<Owner>; 4],
    engine: Option<Engine>,
    recipe: Vec<u8, MAX_RECIPE>,
    asm: RecipeAssembler,
    last_error: u8,
    phase: Phase,
    next_cycle: Instant,
    cycle_started: Instant,
    cycles: u16,
    seq: u8,
    uid: u8,
    backlog: Vec<Entry, BACKLOG>,
    alarms: Vec<AlarmOut, ALARMS>,
    pending: Vec<Pending, 12>,
    /// Output sources that are on (bit per source index).
    outputs: u8,
    force: bool,
    last_mail: Option<Instant>,
    /// The radio and most timers are off until `next_cycle`.
    pub(super) sleeping: bool,
    /// No sleep before this (time to connect over USB after a start).
    grace_until: Instant,
    /// The pin that woke the chip from deep sleep (reported at boot), until it is handled.
    boot_wake: Option<u8>,
    /// The source a wake pin belongs to, until the next cycle has read it: the
    /// event may be over by then (a flap that opened for a second), so the
    /// cycle takes the pin as 1.
    wake_src: Option<u8>,
    /// The significant part of the state last written to flash.
    saved_sig: Vec<u8, 512>,
}

/// Writes "Temperature 8.4 °C" style text for a channel message.
fn fmt_value(out: &mut heapless::String<160>, v: &Value) {
    use core::fmt::Write;
    let scale = sensor::kind::scale(v.kind) as i64;
    let name = sensor::kind::name(v.kind);
    let unit = sensor::kind::unit(v.kind);
    // One decimal for scale 100, two for 1000, none otherwise.
    let (mul, dec) = match scale {
        100 => (10, 1),
        1000 => (100, 2),
        _ => (1, 0),
    };
    let x = (v.value as i64 * mul + if v.value >= 0 { scale / 2 } else { -scale / 2 }) / scale;
    let sign = if x < 0 { "-" } else { "" };
    let a = x.abs();
    let _ = write!(out, "{name} {sign}");
    if dec == 0 {
        let _ = write!(out, "{a}");
    } else {
        let _ = write!(out, "{}.{:0w$}", a / mul, a % mul, w = dec);
    }
    if !unit.is_empty() {
        let _ = write!(out, " {unit}");
    }
}

/// The part of a saved state that decides whether it must be written to flash
/// again: which rules are active, the outputs, what has not been delivered.
/// Counters and times that change every cycle are left out (an `every` rule
/// or a baseline for a `change` rule is simply taken up again after a wake).
fn significant(blob: &[u8]) -> Vec<u8, 512> {
    let mut v: Vec<u8, 512> = Vec::from_slice(blob).unwrap_or_default();
    let state = sensor::STATE_LEN;
    if v.len() >= 5 + state {
        v[3] = 0; // the sequence number
        for r in 0..sensor::MAX_RULES {
            let o = 5 + 2 + r * 20;
            for b in &mut v[o + 4..o + 20] {
                *b = 0;
            }
        }
    }
    v
}

pub(super) fn mono_s() -> u32 {
    bus::MONO_BASE_S.load(Relaxed).wrapping_add(Instant::now().as_secs() as u32)
}

impl Sensor {
    pub(super) fn new() -> Self {
        Sensor {
            owners: [None, None, None, None],
            engine: None,
            recipe: Vec::new(),
            asm: RecipeAssembler::default(),
            last_error: 0,
            phase: Phase::Idle,
            next_cycle: Instant::now() + Duration::from_secs(10),
            cycle_started: Instant::now(),
            cycles: 0,
            seq: 0,
            uid: 0,
            backlog: Vec::new(),
            alarms: Vec::new(),
            pending: Vec::new(),
            outputs: 0,
            force: false,
            last_mail: None,
            sleeping: false,
            grace_until: Instant::now() + Duration::from_secs(bus::SENSOR_GRACE_S.load(Relaxed) as u64),
            boot_wake: None,
            wake_src: None,
            saved_sig: Vec::new(),
        }
    }

    /// The person that owns `device`, if any.
    pub(super) fn owner_person(&self, device: &Address) -> Option<Address> {
        if !SENSOR {
            return None;
        }
        self.owners.iter().flatten().find(|o| o.has(device)).map(|o| o.person())
    }

    pub(super) fn is_owner_person(&self, person: &Address) -> bool {
        if !SENSOR {
            return false;
        }
        self.owners.iter().flatten().any(|o| o.person() == *person)
    }

    pub(super) fn owner_count(&self) -> u8 {
        if !SENSOR {
            return 0;
        }
        self.owners.iter().flatten().count() as u8
    }

    /// When the sensor next needs the node's attention.
    pub(super) fn deadline(&self) -> Option<Instant> {
        if !SENSOR {
            return None;
        }
        self.engine.as_ref()?;
        if self.force {
            return Some(Instant::now());
        }
        Some(match &self.phase {
            Phase::Idle => self.next_cycle,
            Phase::Sampling(t) => *t + SAMPLE_WAIT,
            Phase::Sending { until, .. } => (*until).min(Instant::now() + Duration::from_secs(5)),
        })
    }

    fn owners_blob(&self) -> Vec<u8, OWNERS_MAX> {
        let mut v = Vec::new();
        for (slot, o) in self.owners.iter().enumerate() {
            let Some(o) = o else { continue };
            let list = o.list.as_ref().map(|l| l.to_bytes()).unwrap_or_default();
            let _ = v.extend_from_slice(&[1, slot as u8]);
            let _ = v.extend_from_slice(&o.master.to_bytes());
            let _ = v.push(list.len() as u8);
            let _ = v.extend_from_slice(&list);
        }
        v
    }

    fn load_owners(&mut self, mut b: &[u8]) {
        self.owners = [None, None, None, None];
        while b.len() >= 2 + 64 + 1 {
            let slot = b[1] as usize;
            let master = PublicIdentity::from_bytes(&b[2..66]);
            let n = b[66] as usize;
            let Some(list_bytes) = b.get(67..67 + n) else { break };
            if let (Some(master), true) = (master, slot < 4) {
                let list = DeviceList::verify(&master, list_bytes).ok();
                self.owners[slot] = Some(Owner { master, list });
            }
            b = &b[67 + n..];
        }
    }

    fn zero_values() -> [Option<i32>; 8] {
        [None; 8]
    }
}

/// The answer to a sensor command on a board without sensor setups.
pub(super) const NO_SENSORS: &str = "This board has no sensor setups";

impl<R: RngCore + CryptoRng, S: Settings> FwNode<R, S> {
    /// Loads the recipe and the owners from flash, and what a deep sleep kept.
    pub(super) fn sensor_load(&mut self) {
        if !SENSOR {
            return;
        }
        let blob = self.settings.load_owners();
        self.sensor.load_owners(&blob);
        let bytes = self.settings.load_sensor_recipe();
        if bus::SENSOR_PERSIST.load(Relaxed) {
            let state = self.settings.load_sensor_state();
            self.sensor.saved_sig = significant(&state);
            bus::SENSOR_STATE.lock(|s| *s.borrow_mut() = state);
        }
        if let Ok(r) = Recipe::from_bytes(&bytes) {
            self.sensor.recipe = bytes;
            self.sensor.engine = Some(Engine::new(r));
            self.sensor_restore();
            let w = bus::BOOT_WAKE_PIN.load(Relaxed);
            if w >= 0 {
                self.sensor.boot_wake = Some(w as u8);
            }
        }
    }

    /// Takes the state a deep sleep kept in `bus::SENSOR_STATE`.
    fn sensor_restore(&mut self) {
        let blob: Vec<u8, 512> = bus::SENSOR_STATE.lock(|s| s.borrow().clone());
        let state_len = sensor::STATE_LEN;
        if blob.len() < 5 + state_len || blob[0] != 1 {
            return;
        }
        // The recipe must be the one the state belongs to.
        if blob[1..3] != sensor::crc16(&self.sensor.recipe).to_le_bytes() {
            return;
        }
        let s = &mut self.sensor;
        s.seq = blob[3];
        s.outputs = blob[4];
        if let Some(e) = s.engine.as_mut() {
            e.restore_state(&blob[5..5 + state_len]);
        }
        let mut b = &blob[5 + state_len..];
        let Some((&n, rest)) = b.split_first() else { return };
        b = rest;
        for _ in 0..n {
            if b.len() < 6 {
                return;
            }
            let (t_s, to, m) = (u32::from_le_bytes([b[0], b[1], b[2], b[3]]), b[4], b[5] as usize);
            if b.len() < 6 + m * 6 || m > sensor::MAX_SOURCES {
                return;
            }
            let mut values = Vec::new();
            for i in 0..m {
                let o = 6 + i * 6;
                let _ = values.push(Value { chan: b[o], kind: b[o + 1], value: i32::from_le_bytes([b[o + 2], b[o + 3], b[o + 4], b[o + 5]]) });
            }
            s.uid = s.uid.wrapping_add(1);
            let _ = s.backlog.push(Entry { uid: s.uid, t_s, to, values });
            b = &b[6 + m * 6..];
        }
        // Alarms that were not acknowledged.
        let Some((&n, rest)) = b.split_first() else { return };
        b = rest;
        for _ in 0..n {
            if b.len() < 18 {
                return;
            }
            s.uid = s.uid.wrapping_add(1);
            let _ = s.alarms.push(AlarmOut {
                uid: s.uid,
                rule: b[0],
                raised: b[1] != 0,
                t_s: u32::from_le_bytes([b[2], b[3], b[4], b[5]]),
                value: Value { chan: b[6], kind: b[7], value: i32::from_le_bytes([b[8], b[9], b[10], b[11]]) },
                off: b[12],
                len: b[13],
                to: b[14],
                tries: b[15],
            });
            b = &b[18..];
        }
        // The first cycle after a deep sleep starts soon.
        s.next_cycle = Instant::now() + Duration::from_secs(4);
    }

    /// Keeps the state for a deep sleep (the board copies it to memory that
    /// survives).
    fn sensor_save(&mut self) {
        let blob = {
        let s = &self.sensor;
        let Some(e) = s.engine.as_ref() else { return };
        let mut blob: Vec<u8, 512> = Vec::new();
        let _ = blob.push(1);
        let _ = blob.extend_from_slice(&sensor::crc16(&s.recipe).to_le_bytes());
        let _ = blob.extend_from_slice(&[s.seq, s.outputs]);
        let mut st: Vec<u8, { sensor::STATE_LEN }> = Vec::new();
        e.save_state(&mut st);
        let _ = blob.extend_from_slice(&st);
        let entries: Vec<&Entry, 3> = s.backlog.iter().rev().take(3).collect();
        let _ = blob.push(entries.len() as u8);
        for en in entries.iter().rev() {
            let m = en.values.len().min(6);
            let _ = blob.extend_from_slice(&en.t_s.to_le_bytes());
            let _ = blob.extend_from_slice(&[en.to, m as u8]);
            for v in &en.values[..m] {
                let _ = blob.extend_from_slice(&[v.chan, v.kind]);
                let _ = blob.extend_from_slice(&v.value.to_le_bytes());
            }
        }
        let alarms: Vec<&AlarmOut, 2> = s.alarms.iter().take(2).collect();
        let _ = blob.push(alarms.len() as u8);
        for a in alarms {
            let _ = blob.extend_from_slice(&[a.rule, a.raised as u8]);
            let _ = blob.extend_from_slice(&a.t_s.to_le_bytes());
            let _ = blob.extend_from_slice(&[a.value.chan, a.value.kind]);
            let _ = blob.extend_from_slice(&a.value.value.to_le_bytes());
            let _ = blob.extend_from_slice(&[a.off, a.len, a.to, a.tries, 0, 0]);
        }
        blob
        };
        if bus::SENSOR_PERSIST.load(Relaxed) {
            let sig = significant(&blob);
            if sig != self.sensor.saved_sig && self.settings.save_sensor_state(&blob) {
                self.sensor.saved_sig = sig;
            }
        }
        bus::SENSOR_STATE.lock(|st| *st.borrow_mut() = blob);
    }

    fn sensor_status(&self) -> Status {
        let s = &self.sensor;
        let mut flags = 0;
        if s.engine.is_some() {
            flags |= sensor::status::HAS_RECIPE;
        }
        if s.engine.as_ref().is_some_and(|e| e.recipe().flags & sensor::FLAG_SLEEP != 0) {
            flags |= sensor::status::SLEEPS;
        }
        if crate::power::EXT_POWER.load(Relaxed) == 1 {
            flags |= sensor::status::ON_USB;
        }
        if self.unix_now().is_some() {
            flags |= sensor::status::CLOCK_SET;
        }
        Status {
            error: s.last_error,
            flags,
            recipe_len: s.recipe.len() as u16,
            recipe_crc: sensor::crc16(&s.recipe),
            interval_s: s.engine.as_ref().map_or(0, |e| e.interval_s().min(u16::MAX as u32) as u16),
            battery_mv: crate::power::BATTERY_MV.load(Relaxed).min(u16::MAX as u32) as u16,
            owners: s.owner_count(),
            cycles: s.cycles,
        }
    }

    /// A sensor command from the connected client (`local`) or from an owner
    /// far away. Returns the answers as (`op::EV_*`, data).
    pub(super) fn sensor_exec(&mut self, local: bool, op: u8, data: &[u8]) -> Vec<(u8, Text), 6> {
        if !SENSOR {
            return Default::default();
        }
        let mut out: Vec<(u8, Text), 6> = Vec::new();
        let status = |me: &Self| (op::EV_STATUS, Text::from_slice(&me.sensor_status().to_bytes()).unwrap_or_default());
        match op {
            op::GET_RECIPE => {
                let total = self.sensor.recipe.len();
                if total == 0 {
                    let _ = out.push(status(self));
                }
                for (i, piece) in self.sensor.recipe.chunks(CHUNK).enumerate() {
                    let mut d = Text::new();
                    let _ = d.extend_from_slice(&((i * CHUNK) as u16).to_le_bytes());
                    let _ = d.extend_from_slice(&(total as u16).to_le_bytes());
                    let _ = d.extend_from_slice(piece);
                    let _ = out.push((op::EV_RECIPE, d));
                }
            }
            op::WRITE_RECIPE if data.len() >= 4 => {
                let (offset, total) = (u16::from_le_bytes([data[0], data[1]]), u16::from_le_bytes([data[2], data[3]]));
                match self.sensor.asm.push(offset, total, &data[4..]) {
                    Ok(None) => {}
                    Ok(Some((recipe, bytes))) => {
                        self.sensor.last_error = if self.settings.save_sensor_recipe(&bytes) {
                            // A board sets its I2C bus and analogue input up at boot.
                            if sensor::recipe_pin_plan(&self.sensor.recipe) != recipe.pin_plan() {
                                bus::RESTART.signal(());
                            }
                            self.sensor.recipe = bytes;
                            self.sensor.engine = Some(Engine::new(recipe));
                            self.sensor.backlog.clear();
                            self.sensor.alarms.clear();
                            self.sensor.pending.clear();
                            self.sensor.outputs = 0;
                            // Run the new recipe soon, to show it works.
                            self.sensor.next_cycle = Instant::now() + Duration::from_secs(3);
                            log("Sensor recipe saved");
                            sensor::error::NONE
                        } else {
                            sensor::error::STORAGE
                        };
                        let _ = out.push(status(self));
                    }
                    Err(code) => {
                        self.sensor.last_error = code;
                        let _ = out.push(status(self));
                    }
                }
            }
            op::READ_NOW => {
                self.sensor.force = true;
                let _ = out.push(status(self));
            }
            op::GET_STATUS => {
                let _ = out.push(status(self));
            }
            op::ADD_OWNER if data.len() >= 65 => {
                let slot = data[0] as usize;
                let master = PublicIdentity::from_bytes(&data[1..65]);
                let list = master.as_ref().and_then(|m| DeviceList::verify(m, &data[65..]).ok());
                match master {
                    Some(master) if slot < 4 && (data.len() == 65 || list.is_some()) => {
                        self.sensor.owners[slot] = Some(Owner { master, list });
                        self.sensor.last_error = self.sensor_save_owners();
                    }
                    _ => self.sensor.last_error = sensor::error::INVALID,
                }
                let _ = out.push(self.owners_event());
            }
            op::REMOVE_OWNER if !data.is_empty() => {
                if let Some(o) = self.sensor.owners.get_mut(data[0] as usize) {
                    *o = None;
                }
                self.sensor.last_error = self.sensor_save_owners();
                let _ = out.push(self.owners_event());
            }
            op::GET_OWNERS => {
                let _ = out.push(self.owners_event());
            }
            _ => {
                self.sensor.last_error = sensor::error::INVALID;
                let _ = out.push(status(self));
            }
        }
        let _ = local;
        out
    }

    fn sensor_save_owners(&mut self) -> u8 {
        let blob = self.sensor.owners_blob();
        if self.settings.save_owners(&blob) { sensor::error::NONE } else { sensor::error::STORAGE }
    }

    fn owners_event(&self) -> (u8, Text) {
        let mut d = Text::new();
        for (slot, o) in self.sensor.owners.iter().enumerate() {
            let Some(o) = o else { continue };
            let _ = d.push(slot as u8);
            let _ = d.extend_from_slice(&o.person().0);
            let (version, n) = o.list.as_ref().map_or((0, 0), |l| (l.version, l.devices.len() as u8));
            let _ = d.extend_from_slice(&version.to_le_bytes());
            let _ = d.push(n);
        }
        (op::EV_OWNERS, d)
    }

    /// A sensor command from the connected client. Returns the command if it
    /// is not one.
    pub(super) fn sensor_command(&mut self, cmd: maarse_core::companion::Command) -> Option<maarse_core::companion::Command> {
        use maarse_core::companion::Command;
        match cmd {
            // Without sensors only our own setup is refused: a setup for
            // another node is still sent on (below).
            Command::Sensor { req, dest, .. } if !SENSOR && (dest.0 == [0u8; 16] || dest == self.addr) => {
                super::err(req, maarse_core::companion::error_code::BAD_FRAME, NO_SENSORS);
                None
            }
            Command::Sensor { dest, op, data, .. } if dest.0 == [0u8; 16] || dest == self.addr => {
                for (ev, d) in self.sensor_exec(true, op, &data) {
                    emit(&Event::Sensor { node: Address([0; 16]), op: ev, data: d });
                }
                None
            }
            // For another node: send it as a control command (like `Remote`).
            Command::Sensor { req, dest, op, data } => {
                let inner = Command::Sensor { req, dest: Address([0; 16]), op, data }.encode();
                let Ok(frame) = Text::from_slice(&inner[2..]) else {
                    super::err(req, maarse_core::companion::error_code::TOO_LARGE, "The command is too long");
                    return None;
                };
                Some(Command::Remote { req, dest, frame })
            }
            other => Some(other),
        }
    }

    /// A signed device list that arrived as a message: if it belongs to an
    /// owner and is newer, it replaces the old one (a lost phone is removed by
    /// sending a list that revokes it).
    pub(super) fn sensor_person_list(&mut self, bytes: &[u8]) {
        if !SENSOR {
            return;
        }
        let mut changed = false;
        for o in self.sensor.owners.iter_mut().flatten() {
            if let Ok(list) = DeviceList::verify(&o.master, bytes)
                && o.list.as_ref().is_none_or(|old| list.version > old.version)
            {
                o.list = Some(list);
                changed = true;
            }
        }
        if changed {
            let code = self.sensor_save_owners();
            log(if code == 0 { "Owner device list updated" } else { "Owner device list could not be saved" });
        }
    }

    /// A wake pin fired (a reed switch, a button): something happened that the
    /// recipe waits for, so a cycle runs now instead of at the next timer.
    pub(super) fn sensor_pin_wake(&mut self, pin: u8) {
        if !SENSOR {
            return;
        }
        let Some(engine) = self.sensor.engine.as_mut() else { return };
        let Some(i) = engine
            .recipe()
            .sources
            .iter()
            .position(|s| matches!(s.input, Input::GpioIn { pin: p, wake: true, .. } if p == pin))
        else {
            return;
        };
        engine.pin_woke(i as u8, 1);
        self.sensor.wake_src = Some(i as u8);
        self.sensor.force = true;
        if self.sensor.sleeping {
            self.sensor.sleeping = false;
            bus::RADIO_SLEEP.signal(false);
        }
        log("Sensor: woken by a pin");
    }

    // ------------------------------------------------------------ cycles

    /// Called from the node's timer.
    pub(super) fn sensor_tick(&mut self) {
        if !SENSOR {
            return;
        }
        if self.sensor.engine.is_none() {
            return;
        }
        let now = Instant::now();
        if let Some(pin) = self.sensor.boot_wake.take() {
            self.sensor_pin_wake(pin);
        }
        match self.sensor.phase {
            Phase::Idle => {
                let on_usb = crate::power::EXT_POWER.load(Relaxed) == 1;
                // Plugged in while asleep: wake up and stay awake.
                if self.sensor.sleeping && on_usb {
                    self.sensor.sleeping = false;
                    bus::RADIO_SLEEP.signal(false);
                }
                if now >= self.sensor.next_cycle || self.sensor.force {
                    if self.sensor.sleeping {
                        self.sensor.sleeping = false;
                        bus::RADIO_SLEEP.signal(false);
                    }
                    self.sensor.force = false;
                    let sources = self.sensor.engine.as_ref().map(|e| e.recipe().sources.clone()).unwrap_or_default();
                    while bus::SENSE_DONE.try_receive().is_ok() {}
                    bus::SENSE_REQ.signal(sources);
                    self.sensor.phase = Phase::Sampling(now);
                    self.sensor.cycle_started = now;
                }
            }
            Phase::Sampling(since) => {
                if now >= since + SAMPLE_WAIT {
                    log("Sensor: the board did not answer");
                    self.sensor_samples(Sensor::zero_values());
                }
            }
            Phase::Sending { since, until } => {
                self.sensor_send_all();
                let idle = self.sensor.pending.is_empty() && !self.sensor_has_unsent();
                if (idle && now >= since + MIN_AWAKE) || now >= until {
                    self.sensor_finish();
                }
            }
        }
    }

    fn sensor_has_unsent(&self) -> bool {
        let s = &self.sensor;
        s.backlog.iter().any(|e| e.to & 0x1f != 0) || s.alarms.iter().any(|a| a.to & 0x1f != 0)
    }

    /// The board has read the sources.
    pub(super) fn sensor_samples(&mut self, mut values: [Option<i32>; 8]) {
        if !SENSOR {
            return;
        }
        let Phase::Sampling(_) = self.sensor.phase else { return };
        let now_s = mono_s();
        let Some(engine) = self.sensor.engine.as_mut() else { return };
        let recipe = engine.recipe().clone();
        // Scale the readings (a map turns millivolts into a percentage).
        recipe.apply_maps(&mut values[..recipe.sources.len()]);
        // The level of an output is what the recipe last set.
        for (i, src) in recipe.sources.iter().enumerate() {
            if matches!(src.input, sensor::Input::GpioOut { .. }) {
                values[i] = Some((self.sensor.outputs >> i & 1) as i32);
            }
        }
        // The pin that woke the node counts as 1 in this cycle, also if it is back to 0.
        if let Some(i) = self.sensor.wake_src.take()
            && (i as usize) < values.len()
        {
            values[i as usize] = Some(1);
        }
        let mut fx: Vec<Effect, 16> = Vec::new();
        engine.step(now_s, &values[..recipe.sources.len()], &mut fx);
        self.sensor.cycles = self.sensor.cycles.wrapping_add(1);
        for e in fx {
            match e {
                Effect::Report { mask, to } => {
                    let mut vs: Vec<Value, { sensor::MAX_SOURCES }> = Vec::new();
                    for (i, src) in recipe.sources.iter().enumerate() {
                        if mask >> i & 1 != 0
                            && let Some(v) = values[i]
                        {
                            let _ = vs.push(Value { chan: src.chan, kind: src.kind, value: v });
                        }
                    }
                    if vs.is_empty() {
                        continue;
                    }
                    // Connected clients see every reading at once.
                    let sample = Sample { age_s: 0, values: vs.clone() };
                    let (msg, _) = sensor::encode_readings(self.sensor.seq, core::slice::from_ref(&sample));
                    emit(&Event::Sensor { node: Address([0; 16]), op: op::EV_MESSAGE, data: Text::from_slice(&msg).unwrap_or_default() });
                    let to = self.sensor_recipients(to);
                    if to != 0 {
                        self.sensor.uid = self.sensor.uid.wrapping_add(1);
                        if self.sensor.backlog.is_full() {
                            self.sensor.backlog.remove(0);
                        }
                        let uid = self.sensor.uid;
                        let _ = self.sensor.backlog.push(Entry { uid, t_s: now_s, to, values: vs });
                    }
                }
                Effect::Alarm { rule, raised, src, value, off, len, to } => {
                    let s = recipe.sources.get(src as usize);
                    let v = Value { chan: s.map_or(0, |s| s.chan), kind: s.map_or(0, |s| s.kind), value };
                    // Connected clients see every alarm at once.
                    let text = recipe.alarm_text(off, len);
                    let msg = sensor::encode_alarm(rule, raised, 0, v, text);
                    emit(&Event::Sensor { node: Address([0; 16]), op: op::EV_MESSAGE, data: Text::from_slice(&msg).unwrap_or_default() });
                    let to = self.sensor_recipients(to);
                    log(if raised { "Sensor alarm raised" } else { "Sensor alarm cleared" });
                    if to != 0 {
                        self.sensor.uid = self.sensor.uid.wrapping_add(1);
                        if self.sensor.alarms.is_full() {
                            self.sensor.alarms.remove(0);
                        }
                        let uid = self.sensor.uid;
                        let _ = self.sensor.alarms.push(AlarmOut { uid, rule, raised, t_s: now_s, value: v, off, len, to, tries: 0 });
                    }
                }
                Effect::SetOut { src, on } => {
                    if let Some(s) = recipe.sources.get(src as usize) {
                        let _ = bus::SET_OUT.try_send((*s, on));
                        if on {
                            self.sensor.outputs |= 1 << src;
                        } else {
                            self.sensor.outputs &= !(1 << src);
                        }
                    }
                }
            }
        }
        let now = Instant::now();
        self.sensor.phase = Phase::Sending { since: now, until: now + ACK_WAIT };
        // Ask for mail (a new recipe may wait in a post office) about once an hour.
        if self.sensor.last_mail.is_none_or(|t| t.elapsed() >= MAIL_EVERY) {
            self.sensor.last_mail = Some(now);
            self.mail_sync();
        }
        self.sensor_send_all();
    }

    /// Recipient mask: owner slots that exist, and the channel bit.
    fn sensor_recipients(&self, to: u8) -> u8 {
        let mut m = 0;
        for slot in 0..4 {
            if to >> slot & 1 != 0 && self.sensor.owners[slot].is_some() {
                m |= 1 << slot;
            }
        }
        m | (to & sensor::TO_CHANNEL)
    }

    /// The owner's device to send to: the one heard most recently. If none is
    /// known yet, asks the network for the first one's key.
    fn sensor_owner_device(&mut self, slot: usize) -> Option<Address> {
        let list = self.sensor.owners[slot].as_ref()?.list.clone()?;
        let best = list.active().filter_map(|a| self.known.get(&a).map(|(_, t)| (*t, a))).max_by_key(|(t, _)| *t).map(|(_, a)| a);
        if best.is_none()
            && let Some(first) = list.active().next()
        {
            self.ask_key(first);
        }
        best
    }

    fn sensor_in_flight(&self, owner: usize, uid: u8, alarm: bool) -> bool {
        self.sensor.pending.iter().any(|p| {
            p.owner as usize == owner
                && match &p.what {
                    What::Readings(u) => !alarm && u.contains(&uid),
                    What::Alarm(a) => alarm && *a == uid,
                }
        })
    }

    /// Sends what is waiting to the owners, once per message in flight.
    fn sensor_send_all(&mut self) {
        let now_s = mono_s();
        for slot in 0..4usize {
            if self.sensor.owners[slot].is_none() {
                continue;
            }
            // Alarms first: one message each.
            let alarm_ids: Vec<u8, ALARMS> = self
                .sensor
                .alarms
                .iter()
                .filter(|a| a.to >> slot & 1 != 0 && !self.sensor_in_flight(slot, a.uid, true))
                .map(|a| a.uid)
                .collect();
            for uid in alarm_ids {
                let Some(dest) = self.sensor_owner_device(slot) else { break };
                let Some(a) = self.sensor.alarms.iter().find(|a| a.uid == uid) else { continue };
                let text = self.sensor.engine.as_ref().map_or("", |e| e.recipe().alarm_text(a.off, a.len));
                let msg = sensor::encode_alarm(a.rule, a.raised, now_s.saturating_sub(a.t_s).min(u16::MAX as u32) as u16, a.value, text);
                if let Some(id) = self.sensor_send(dest, &msg) {
                    let _ = self.sensor.pending.push(Pending { id, owner: slot as u8, what: What::Alarm(uid) });
                    if let Some(a) = self.sensor.alarms.iter_mut().find(|a| a.uid == uid) {
                        a.tries += 1;
                    }
                }
            }
            // Readings: as many samples per message as fit, oldest first.
            let waiting: Vec<(u8, Sample), BACKLOG> = self
                .sensor
                .backlog
                .iter()
                .filter(|e| e.to >> slot & 1 != 0 && !self.sensor_in_flight(slot, e.uid, false))
                .map(|e| (e.uid, Sample { age_s: now_s.saturating_sub(e.t_s).min(u16::MAX as u32) as u16, values: e.values.clone() }))
                .collect();
            let mut start = 0;
            while start < waiting.len() {
                let Some(dest) = self.sensor_owner_device(slot) else { break };
                let samples: Vec<Sample, BACKLOG> = waiting[start..].iter().map(|(_, s)| s.clone()).collect();
                let (msg, n) = sensor::encode_readings(self.sensor.seq, &samples);
                if n == 0 {
                    break;
                }
                let Some(id) = self.sensor_send(dest, &msg) else { break };
                self.sensor.seq = self.sensor.seq.wrapping_add(1);
                let uids: Vec<u8, 8> = waiting[start..start + n].iter().map(|(u, _)| *u).collect();
                let _ = self.sensor.pending.push(Pending { id, owner: slot as u8, what: What::Readings(uids) });
                start += n;
            }
        }
        self.sensor_send_to_channel();
        // Alarms that have used their tries are given up.
        self.sensor.alarms.retain(|a| a.tries < ALARM_TRIES);
    }

    /// The recipe's private channel: what is waiting for it goes out once, as
    /// a line of text the channel's members can read. There is no
    /// acknowledgement. The public channel is never used.
    fn sensor_send_to_channel(&mut self) {
        let bit = sensor::TO_CHANNEL;
        let Some(channel) = self.sensor.engine.as_ref().and_then(|e| e.recipe().channel) else { return };
        let now_s = mono_s();
        let short = [self.addr.0[0], self.addr.0[1]];
        let mut lines: Vec<heapless::String<160>, 12> = Vec::new();
        for e in self.sensor.backlog.iter_mut().filter(|e| e.to & bit != 0) {
            e.to &= !bit;
            let mut l: heapless::String<160> = heapless::String::new();
            let _ = core::fmt::write(&mut l, format_args!("{:02x}{:02x}:", short[0], short[1]));
            for v in &e.values {
                let _ = l.push(' ');
                fmt_value(&mut l, v);
            }
            let _ = lines.push(l);
        }
        for a in self.sensor.alarms.iter_mut().filter(|a| a.to & bit != 0) {
            a.to &= !bit;
            let text = self.sensor.engine.as_ref().map_or("", |e| e.recipe().alarm_text(a.off, a.len));
            let mut l: heapless::String<160> = heapless::String::new();
            let _ = core::fmt::write(&mut l, format_args!("{:02x}{:02x}: {} {}", short[0], short[1], if a.raised { "ALARM" } else { "cleared:" }, text));
            let _ = lines.push(l);
        }
        let _ = now_s;
        self.sensor.backlog.retain(|e| e.to & 0x1f != 0);
        self.sensor.alarms.retain(|a| a.to & 0x1f != 0);
        for l in lines {
            if !self.sensor_send_channel(channel, content::text(l.as_bytes()).as_slice()) {
                log("Sensor: the private channel is not available (join it first); nothing sent there");
                break;
            }
        }
    }

    fn sensor_send_channel(&mut self, channel: Address, plain: &[u8]) -> bool {
        // Never the public channel: everybody could read the sensor.
        if channel == maarse_crypto::channel_address(&maarse_crypto::public_channel_key()) {
            return false;
        }
        let Some((_, key, nm)) = self.channels.iter().find(|(a, _, _)| *a == channel) else { return false };
        let key = *key;
        let region = maarse_core::region::region_id(maarse_core::region::split_channel(nm).1);
        let mut nonce = [0u8; 8];
        self.rng.fill_bytes(&mut nonce);
        let mut ct = [0u8; 255];
        let Ok(n) = maarse_crypto::seal::seal_channel(&key, &self.addr, nonce, plain, &mut ct) else { return false };
        let Ok(actions) = self.router.send_channel_in(channel, region, &ct[..n]) else { return false };
        self.apply(actions);
        true
    }

    fn sensor_send(&mut self, dest: Address, msg: &[u8]) -> Option<u64> {
        if self.sensor.pending.is_full() {
            return None;
        }
        let mut plain: Vec<u8, 160> = Vec::new();
        plain.push(content::SENSOR).ok()?;
        plain.extend_from_slice(msg).ok()?;
        self.send_content_ret(0, dest, &plain)
    }

    /// An end-to-end acknowledgement (or a delivery receipt) for `msg_id`.
    pub(super) fn sensor_on_ack(&mut self, msg_id: u64) {
        if !SENSOR {
            return;
        }
        let Some(i) = self.sensor.pending.iter().position(|p| p.id == msg_id) else { return };
        let p = self.sensor.pending.remove(i);
        let bit = 1u8 << p.owner;
        match p.what {
            What::Readings(uids) => {
                for e in self.sensor.backlog.iter_mut() {
                    if uids.contains(&e.uid) {
                        e.to &= !bit;
                    }
                }
                self.sensor.backlog.retain(|e| e.to & 0x1f != 0);
            }
            What::Alarm(uid) => {
                for a in self.sensor.alarms.iter_mut() {
                    if a.uid == uid {
                        a.to &= !bit;
                    }
                }
                self.sensor.alarms.retain(|a| a.to & 0x1f != 0);
            }
        }
    }

    /// The cycle is over: plan the next one and, on battery, go to sleep.
    fn sensor_finish(&mut self) {
        let now = Instant::now();
        // Messages still in flight are tried again next cycle.
        self.sensor.pending.clear();
        let interval = self.sensor.engine.as_ref().map_or(900, |e| e.interval_s()) as u64;
        self.sensor.next_cycle = (self.sensor.cycle_started + Duration::from_secs(interval)).max(now + Duration::from_secs(1));
        self.sensor.phase = Phase::Idle;
        let wants_sleep = self.sensor.engine.as_ref().is_some_and(|e| e.recipe().flags & sensor::FLAG_SLEEP != 0);
        let on_usb = crate::power::EXT_POWER.load(Relaxed) == 1;
        let secs = self.sensor.next_cycle.saturating_duration_since(now).as_secs();
        if wants_sleep && !on_usb && !self.client_present() && secs >= MIN_SLEEP_S && now >= self.sensor.grace_until {
            self.sensor_save();
            let pins = self.sensor.engine.as_ref().map(|e| e.recipe().wake_pins()).unwrap_or_default();
            bus::WAKE_PINS.lock(|w| *w.borrow_mut() = pins);
            self.sensor.sleeping = true;
            bus::RADIO_SLEEP.signal(true);
            bus::SLEEP_REQ.signal(secs as u32);
            log("Sensor: sleeping");
        }
    }
}
