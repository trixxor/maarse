//! Sensor nodes: measurements, the recipe that tells a node what to do with
//! them, and the engine that runs the recipe.
//!
//! A sensor node is an ordinary node with a *recipe*: a small, bounded
//! description (at most [`MAX_RECIPE`] bytes) made in the designer in the web
//! client or the Android app. There are no loops and no free-form code, so a
//! recipe cannot hang or brick a node. The same [`Engine`] runs on the nRF52,
//! the ESP32, in the host (for the designer's test view) and in tests.
//!
//! # Measurements
//!
//! Every value has a *channel* (chosen in the designer, stable for the node)
//! and a *kind* ([`kind`]) that gives its unit and scale. Values are integers:
//! `value / kind::scale(kind)` is the number to show. Kinds from
//! [`kind::CUSTOM`] up are free to use; their unit is up to the designer.
//!
//! # Messages (content type `content::SENSOR`)
//!
//! ```text
//! READINGS (1): seq (1) | n (1) | n x sample
//!   sample = age s (u16 LE, how long before sending) | m (1) | m x value
//!   value  = channel (1) | kind (1) | value (zigzag varint)
//! ALARM    (2): rule (1) | flags (1, bit 0 = raised) | age s (u16) |
//!               channel (1) | kind (1) | value (zigzag varint) | text (rest)
//! ```
//!
//! # Recipe (version 1, all numbers little endian)
//!
//! ```text
//! version (1) | flags (1) | interval s (u16) | report_to (1) | alarm_to (1)
//! | n sources (1) | n rules (1) | text length (1) | [channel address (16)]
//! | sources | rules | text
//! source = channel (1) | kind (1) | input (1) | a (1) | b (1)
//! rule   = id (1) | condition (1) | condition data | limit (1, units of 5 min)
//!          | n actions (1) | actions
//! ```
//!
//! `report_to` and `alarm_to` say who gets readings and alarms: bits 0-3 are
//! the node's owners (the first four on its list), bit 4 is the recipe's
//! private channel. The public channel is never allowed.

use crate::Address;
use heapless::Vec;

pub const RECIPE_VERSION: u8 = 1;
pub const MAX_SOURCES: usize = 8;
pub const MAX_RULES: usize = 8;
pub const MAX_ACTIONS: usize = 3;
pub const MAX_TEXT: usize = 64;
/// Longest recipe in bytes.
pub const MAX_RECIPE: usize = 256;
/// Fewest seconds between two cycles: keeps a bad recipe from using up the
/// radio's duty cycle.
pub const MIN_INTERVAL_S: u16 = 30;
/// Owners a node has, and so the number of bits in a recipient mask.
pub const MAX_OWNERS: usize = 4;
/// Bit in a recipient mask: the recipe's private channel.
pub const TO_CHANNEL: u8 = 0x10;
/// Longest message in bytes (it must fit in one encrypted direct message).
pub const MAX_MSG: usize = 140;

/// Recipe flag: sleep between cycles when not powered over USB.
pub const FLAG_SLEEP: u8 = 0x01;
/// Recipe flag: the header holds a channel address.
pub const FLAG_CHANNEL: u8 = 0x02;

/// What a value means.
pub mod kind {
    pub const TEMPERATURE: u8 = 1;
    pub const HUMIDITY: u8 = 2;
    pub const PRESSURE: u8 = 3;
    pub const VOLTAGE: u8 = 4;
    pub const DIGITAL: u8 = 5;
    pub const COUNTER: u8 = 6;
    pub const PERCENT: u8 = 7;
    pub const DISTANCE: u8 = 8;
    /// Kinds from here up are the owner's own (shown as plain numbers).
    pub const CUSTOM: u8 = 128;

    /// `value / scale` is the number in [`unit`].
    pub fn scale(kind: u8) -> i32 {
        match kind {
            TEMPERATURE | HUMIDITY | PRESSURE | PERCENT => 100,
            VOLTAGE => 1000,
            _ => 1,
        }
    }

    pub fn unit(kind: u8) -> &'static str {
        match kind {
            TEMPERATURE => "°C",
            HUMIDITY | PERCENT => "%",
            PRESSURE => "hPa",
            VOLTAGE => "V",
            DISTANCE => "mm",
            _ => "",
        }
    }

    pub fn name(kind: u8) -> &'static str {
        match kind {
            TEMPERATURE => "Temperature",
            HUMIDITY => "Humidity",
            PRESSURE => "Pressure",
            VOLTAGE => "Voltage",
            DIGITAL => "Pin",
            COUNTER => "Counter",
            PERCENT => "Level",
            DISTANCE => "Distance",
            _ => "Value",
        }
    }

    pub fn is_known(kind: u8) -> bool {
        (1..=8).contains(&kind) || kind >= CUSTOM
    }
}

// ---------------------------------------------------------------- recipe

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecipeError {
    /// Too short, or a count points past the end.
    Malformed,
    /// A version this firmware does not know.
    Version,
    /// More sources, rules, actions or text than a recipe may hold.
    TooLarge,
    /// A reference to a source, text or kind that does not exist, or a value
    /// out of range.
    Invalid,
}

/// Pull of an input pin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pull {
    None,
    Up,
    Down,
}

/// Where a value comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    /// The battery voltage.
    Battery,
    /// The chip's own temperature.
    ChipTemp,
    /// A digital input pin. `pin` is the board's pin number (see the
    /// board's pin table); the value is 0 or 1 (after `invert`). With `wake`
    /// the pin also wakes the node from sleep the moment its value becomes 1
    /// (a reed switch in a mailbox, a button): a node that is woken by the
    /// pin knows that something happened, instead of looking once per
    /// cycle. A wake pin needs a pull against the level that wakes it.
    GpioIn { pin: u8, pull: Pull, invert: bool, wake: bool },
    /// An analogue input: millivolts times `mul16 / 16`.
    Adc { pin: u8, mul16: u8 },
    /// A BME280 or BMP280 on I2C at `addr`; `what` 0 = temperature,
    /// 1 = humidity, 2 = pressure.
    Bme280 { addr: u8, what: u8 },
    /// Counts edges on a pin since the node started.
    Counter { pin: u8, falling: bool, pull: Pull },
    /// A digital output pin (the value is the level the recipe set).
    GpioOut { pin: u8, active_low: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Source {
    pub chan: u8,
    pub kind: u8,
    pub input: Input,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmp {
    Lt,
    Gt,
    Le,
    Ge,
    Eq,
    Ne,
}

impl Cmp {
    fn from_u8(v: u8) -> Option<Cmp> {
        Some(match v {
            0 => Cmp::Lt,
            1 => Cmp::Gt,
            2 => Cmp::Le,
            3 => Cmp::Ge,
            4 => Cmp::Eq,
            5 => Cmp::Ne,
            _ => return None,
        })
    }
    pub fn holds(self, v: i32, limit: i32) -> bool {
        match self {
            Cmp::Lt => v < limit,
            Cmp::Gt => v > limit,
            Cmp::Le => v <= limit,
            Cmp::Ge => v >= limit,
            Cmp::Eq => v == limit,
            Cmp::Ne => v != limit,
        }
    }
}

/// Which way a pin must change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Low,
    High,
    Any,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cond {
    /// Every `secs` seconds.
    Every { secs: u16 },
    /// A source compared with a value for `count` readings in a row. The rule
    /// clears when the value passes `clear` (equal to `value`: when the
    /// comparison stops holding).
    Compare { src: u8, op: Cmp, value: i32, clear: i32, count: u8 },
    /// A source moved by at least `delta` since the rule last fired.
    Change { src: u8, delta: u32 },
    /// A pin changed.
    Edge { src: u8, to: Edge },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Send readings: the sources in `mask` (bit i = source i; 0xFF = all) to
    /// `to` (0 = the recipe's `report_to`).
    Report { mask: u8, to: u8 },
    /// Send an alarm with the text at `off`/`len` in the recipe's text, to
    /// `to` (0 = the recipe's `alarm_to`). Sends "cleared" when the rule
    /// clears.
    Alarm { off: u8, len: u8, to: u8 },
    /// Set an output source on (and back off when the rule clears).
    SetOut { src: u8, on: bool },
    /// Use this interval until the rule clears.
    SetInterval { secs: u16 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub id: u8,
    pub cond: Cond,
    /// Fewest minutes between two alarms from this rule, in steps of 5 (0 = no limit).
    pub limit_5min: u8,
    pub actions: Vec<Action, MAX_ACTIONS>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recipe {
    pub flags: u8,
    pub interval_s: u16,
    pub report_to: u8,
    pub alarm_to: u8,
    pub channel: Option<Address>,
    pub sources: Vec<Source, MAX_SOURCES>,
    pub rules: Vec<Rule, MAX_RULES>,
    pub text: Vec<u8, MAX_TEXT>,
}

impl Default for Recipe {
    fn default() -> Self {
        Recipe {
            flags: FLAG_SLEEP,
            interval_s: 900,
            report_to: 1,
            alarm_to: 1,
            channel: None,
            sources: Vec::new(),
            rules: Vec::new(),
            text: Vec::new(),
        }
    }
}

struct Rd<'a>(&'a [u8]);

impl<'a> Rd<'a> {
    fn u8(&mut self) -> Result<u8, RecipeError> {
        let (&b, rest) = self.0.split_first().ok_or(RecipeError::Malformed)?;
        self.0 = rest;
        Ok(b)
    }
    fn u16(&mut self) -> Result<u16, RecipeError> {
        Ok(u16::from_le_bytes([self.u8()?, self.u8()?]))
    }
    fn i32(&mut self) -> Result<i32, RecipeError> {
        Ok(i32::from_le_bytes([self.u8()?, self.u8()?, self.u8()?, self.u8()?]))
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], RecipeError> {
        if self.0.len() < n {
            return Err(RecipeError::Malformed);
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
}

fn pull_from(v: u8) -> Pull {
    match v & 3 {
        1 => Pull::Up,
        2 => Pull::Down,
        _ => Pull::None,
    }
}

fn pull_to(p: Pull) -> u8 {
    match p {
        Pull::None => 0,
        Pull::Up => 1,
        Pull::Down => 2,
    }
}

impl Input {
    fn code(&self) -> (u8, u8, u8) {
        match *self {
            Input::Battery => (1, 0, 0),
            Input::ChipTemp => (2, 0, 0),
            Input::GpioIn { pin, pull, invert, wake } => (3, pin, pull_to(pull) | (invert as u8) << 2 | (wake as u8) << 3),
            Input::Adc { pin, mul16 } => (4, pin, mul16),
            Input::Bme280 { addr, what } => (5, addr, what),
            Input::Counter { pin, falling, pull } => (6, pin, pull_to(pull) | (falling as u8) << 2),
            Input::GpioOut { pin, active_low } => (7, pin, active_low as u8),
        }
    }

    fn from_code(t: u8, a: u8, b: u8) -> Result<Input, RecipeError> {
        Ok(match t {
            1 => Input::Battery,
            2 => Input::ChipTemp,
            3 => Input::GpioIn { pin: a, pull: pull_from(b), invert: b & 4 != 0, wake: b & 8 != 0 },
            4 if b > 0 => Input::Adc { pin: a, mul16: b },
            5 if b <= 2 && (a == 0x76 || a == 0x77) => Input::Bme280 { addr: a, what: b },
            6 => Input::Counter { pin: a, falling: b & 4 != 0, pull: pull_from(b) },
            7 => Input::GpioOut { pin: a, active_low: b & 1 != 0 },
            _ => return Err(RecipeError::Invalid),
        })
    }
}

impl Recipe {
    /// Which board hardware the recipe needs at start-up: bit 0 = the I2C bus,
    /// bit 1 = the analogue input. A board that sets these up at boot
    /// restarts when a new recipe needs a different set.
    pub fn pin_plan(&self) -> u8 {
        let mut plan = 0;
        let mut wake = 0u8;
        for s in &self.sources {
            match s.input {
                Input::Bme280 { .. } => plan |= 1,
                Input::Adc { .. } => plan |= 2,
                Input::GpioIn { pin, wake: true, .. } => wake ^= pin.wrapping_mul(7).wrapping_add(1),
                _ => {}
            }
        }
        // Bits 2-7: which pins wake the node (a different set restarts the board).
        plan | (wake & 0x3f) << 2
    }

    /// The pins that wake the node: (pin, level that wakes it: true = high).
    /// The pull is always against that level (`validate` checks it).
    pub fn wake_pins(&self) -> Vec<(u8, bool), 2> {
        let mut v = Vec::new();
        for s in &self.sources {
            if let Input::GpioIn { pin, invert, wake: true, .. } = s.input {
                let _ = v.push((pin, !invert));
            }
        }
        v
    }

    /// A source of this kind of input is read, not set.
    pub fn is_output(&self, src: u8) -> bool {
        matches!(self.sources.get(src as usize).map(|s| s.input), Some(Input::GpioOut { .. }))
    }

    /// Checks references and ranges. `to_bytes` and `from_bytes` both call it.
    pub fn validate(&self) -> Result<(), RecipeError> {
        if self.interval_s < MIN_INTERVAL_S {
            return Err(RecipeError::Invalid);
        }
        if (self.flags & FLAG_CHANNEL != 0) != self.channel.is_some() {
            return Err(RecipeError::Invalid);
        }
        let has_ch = self.channel.is_some();
        let mask_ok = |m: u8| m & !(0x0f | TO_CHANNEL) == 0 && (m & TO_CHANNEL == 0 || has_ch);
        if !mask_ok(self.report_to) || !mask_ok(self.alarm_to) {
            return Err(RecipeError::Invalid);
        }
        let mut chans = 0u32;
        for s in &self.sources {
            if !kind::is_known(s.kind) || s.chan >= 32 || chans & (1 << s.chan) != 0 {
                return Err(RecipeError::Invalid);
            }
            chans |= 1 << s.chan;
        }
        let mut wake = 0;
        for s in &self.sources {
            if let Input::GpioIn { pull, invert, wake: true, .. } = s.input {
                wake += 1;
                // The pull must hold the pin at the level that does not wake the node.
                let active_high = !invert;
                if wake > 2 || (active_high && pull != Pull::Down) || (!active_high && pull != Pull::Up) {
                    return Err(RecipeError::Invalid);
                }
            }
        }
        let n = self.sources.len() as u8;
        let src_ok = |i: u8| i < n && !self.is_output(i);
        for r in &self.rules {
            match r.cond {
                Cond::Every { secs } if secs >= MIN_INTERVAL_S => {}
                Cond::Compare { src, count, .. } if src_ok(src) && count >= 1 => {}
                Cond::Change { src, delta } if src_ok(src) && delta > 0 => {}
                Cond::Edge { src, .. } if src_ok(src) => {}
                _ => return Err(RecipeError::Invalid),
            }
            for a in &r.actions {
                match *a {
                    Action::Report { to, .. } if mask_ok(to) => {}
                    Action::Alarm { off, len, to } if mask_ok(to) && off as usize + len as usize <= self.text.len() => {}
                    Action::SetOut { src, .. } if src < n && self.is_output(src) => {}
                    Action::SetInterval { secs } if secs >= MIN_INTERVAL_S => {}
                    _ => return Err(RecipeError::Invalid),
                }
            }
        }
        if core::str::from_utf8(&self.text).is_err() {
            return Err(RecipeError::Invalid);
        }
        Ok(())
    }

    pub fn to_bytes(&self) -> Result<Vec<u8, MAX_RECIPE>, RecipeError> {
        self.validate()?;
        let mut v: Vec<u8, MAX_RECIPE> = Vec::new();
        let put = |v: &mut Vec<u8, MAX_RECIPE>, b: &[u8]| v.extend_from_slice(b).map_err(|_| RecipeError::TooLarge);
        put(&mut v, &[RECIPE_VERSION, self.flags])?;
        put(&mut v, &self.interval_s.to_le_bytes())?;
        put(&mut v, &[self.report_to, self.alarm_to, self.sources.len() as u8, self.rules.len() as u8, self.text.len() as u8])?;
        if let Some(c) = &self.channel {
            put(&mut v, &c.0)?;
        }
        for s in &self.sources {
            let (t, a, b) = s.input.code();
            put(&mut v, &[s.chan, s.kind, t, a, b])?;
        }
        for r in &self.rules {
            put(&mut v, &[r.id])?;
            match r.cond {
                Cond::Every { secs } => {
                    put(&mut v, &[1])?;
                    put(&mut v, &secs.to_le_bytes())?;
                }
                Cond::Compare { src, op, value, clear, count } => {
                    put(&mut v, &[2, src, op as u8, count])?;
                    put(&mut v, &value.to_le_bytes())?;
                    put(&mut v, &clear.to_le_bytes())?;
                }
                Cond::Change { src, delta } => {
                    put(&mut v, &[3, src])?;
                    put(&mut v, &delta.to_le_bytes())?;
                }
                Cond::Edge { src, to } => {
                    put(&mut v, &[4, src, to as u8])?;
                }
            }
            put(&mut v, &[r.limit_5min, r.actions.len() as u8])?;
            for a in &r.actions {
                match *a {
                    Action::Report { mask, to } => put(&mut v, &[1, mask, to])?,
                    Action::Alarm { off, len, to } => put(&mut v, &[2, off, len, to])?,
                    Action::SetOut { src, on } => put(&mut v, &[3, src, on as u8])?,
                    Action::SetInterval { secs } => {
                        put(&mut v, &[4])?;
                        put(&mut v, &secs.to_le_bytes())?
                    }
                }
            }
        }
        put(&mut v, &self.text)?;
        Ok(v)
    }

    pub fn from_bytes(b: &[u8]) -> Result<Recipe, RecipeError> {
        if b.len() > MAX_RECIPE {
            return Err(RecipeError::TooLarge);
        }
        let mut r = Rd(b);
        if r.u8()? != RECIPE_VERSION {
            return Err(RecipeError::Version);
        }
        let flags = r.u8()?;
        let interval_s = r.u16()?;
        let report_to = r.u8()?;
        let alarm_to = r.u8()?;
        let (ns, nr, nt) = (r.u8()? as usize, r.u8()? as usize, r.u8()? as usize);
        if ns > MAX_SOURCES || nr > MAX_RULES || nt > MAX_TEXT {
            return Err(RecipeError::TooLarge);
        }
        let channel = if flags & FLAG_CHANNEL != 0 { Address::from_slice(r.take(16)?) } else { None };
        let mut rec = Recipe { flags, interval_s, report_to, alarm_to, channel, ..Recipe::default() };
        for _ in 0..ns {
            let (chan, k, t, a, bb) = (r.u8()?, r.u8()?, r.u8()?, r.u8()?, r.u8()?);
            let input = Input::from_code(t, a, bb)?;
            rec.sources.push(Source { chan, kind: k, input }).map_err(|_| RecipeError::TooLarge)?;
        }
        for _ in 0..nr {
            let id = r.u8()?;
            let cond = match r.u8()? {
                1 => Cond::Every { secs: r.u16()? },
                2 => {
                    let (src, op, count) = (r.u8()?, r.u8()?, r.u8()?);
                    let op = Cmp::from_u8(op).ok_or(RecipeError::Invalid)?;
                    Cond::Compare { src, op, value: r.i32()?, clear: r.i32()?, count }
                }
                3 => {
                    let src = r.u8()?;
                    let d = r.i32()? as u32;
                    Cond::Change { src, delta: d }
                }
                4 => {
                    let src = r.u8()?;
                    let to = match r.u8()? {
                        0 => Edge::Low,
                        1 => Edge::High,
                        2 => Edge::Any,
                        _ => return Err(RecipeError::Invalid),
                    };
                    Cond::Edge { src, to }
                }
                _ => return Err(RecipeError::Invalid),
            };
            let limit_5min = r.u8()?;
            let na = r.u8()? as usize;
            if na > MAX_ACTIONS {
                return Err(RecipeError::TooLarge);
            }
            let mut actions = Vec::new();
            for _ in 0..na {
                let a = match r.u8()? {
                    1 => Action::Report { mask: r.u8()?, to: r.u8()? },
                    2 => Action::Alarm { off: r.u8()?, len: r.u8()?, to: r.u8()? },
                    3 => Action::SetOut { src: r.u8()?, on: r.u8()? != 0 },
                    4 => Action::SetInterval { secs: r.u16()? },
                    _ => return Err(RecipeError::Invalid),
                };
                let _ = actions.push(a);
            }
            rec.rules.push(Rule { id, cond, limit_5min, actions }).map_err(|_| RecipeError::TooLarge)?;
        }
        rec.text.extend_from_slice(r.take(nt)?).map_err(|_| RecipeError::TooLarge)?;
        if !r.0.is_empty() {
            return Err(RecipeError::Malformed);
        }
        rec.validate()?;
        Ok(rec)
    }

    /// The text of an alarm action.
    pub fn alarm_text(&self, off: u8, len: u8) -> &str {
        self.text
            .get(off as usize..off as usize + len as usize)
            .and_then(|t| core::str::from_utf8(t).ok())
            .unwrap_or("")
    }
}

// ---------------------------------------------------------------- engine

/// What the engine asks the node to do after a cycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Send the readings of the sources in `mask` to the recipients in `to`.
    Report { mask: u8, to: u8 },
    /// Send an alarm (`raised`) or its all-clear to `to`.
    Alarm { rule: u8, raised: bool, src: u8, value: i32, off: u8, len: u8, to: u8 },
    /// Set an output source.
    SetOut { src: u8, on: bool },
}

#[derive(Clone, Copy, Default)]
struct RuleState {
    active: bool,
    run: u8,
    seen: bool,
    last_fire_s: u32,
    last_alarm_s: u32,
    alarmed: bool,
    last_val: i32,
    prev: i32,
}

/// Bytes of [`Engine::save_state`].
pub const STATE_LEN: usize = 2 + MAX_RULES * 20;

/// Runs a recipe, one cycle at a time. Pure: the node gives it readings and
/// the time, it says what to do. Holds a little state (which rules are
/// active, when they last fired) that [`Engine::save_state`] can keep across a
/// deep sleep.
#[derive(Clone)]
pub struct Engine {
    recipe: Recipe,
    st: [RuleState; MAX_RULES],
    interval_override: Option<u16>,
}

impl Engine {
    pub fn new(recipe: Recipe) -> Self {
        Engine { recipe, st: [RuleState::default(); MAX_RULES], interval_override: None }
    }

    pub fn recipe(&self) -> &Recipe {
        &self.recipe
    }

    /// Seconds from one cycle to the next.
    pub fn interval_s(&self) -> u32 {
        self.interval_override.unwrap_or(self.recipe.interval_s) as u32
    }

    /// The node was woken by the pin of source `src`, whose value is now
    /// `value`: a rule that waits for that pin to change fires in the next
    /// `step`, whatever the pin looked like at the last cycle (the node may have
    /// slept for hours, or restarted).
    pub fn pin_woke(&mut self, src: u8, value: i32) {
        for (i, r) in self.recipe.rules.iter().enumerate() {
            if let Cond::Edge { src: s, .. } = r.cond
                && s == src
            {
                self.st[i].seen = true;
                self.st[i].prev = (value == 0) as i32;
            }
        }
    }

    /// Runs one cycle. `samples[i]` is the reading of source i (`None` when it
    /// could not be read). `now_s` is any clock that only goes up.
    pub fn step(&mut self, now_s: u32, samples: &[Option<i32>], fx: &mut Vec<Effect, 16>) {
        let mut report = (0u8, 0u8);
        for i in 0..self.recipe.rules.len() {
            let rule = self.recipe.rules[i].clone();
            let s = &mut self.st[i];
            let mut raise = false;
            let mut clear = false;
            let mut src_hit = 0u8;
            let mut value = 0i32;
            let sample = |src: u8| samples.get(src as usize).copied().flatten();
            match rule.cond {
                Cond::Every { secs } => {
                    if !s.seen || now_s.wrapping_sub(s.last_fire_s) >= secs as u32 {
                        s.seen = true;
                        s.last_fire_s = now_s;
                        raise = true;
                    }
                }
                Cond::Compare { src, op, value: limit, clear: clear_at, count } => {
                    if let Some(v) = sample(src) {
                        src_hit = src;
                        value = v;
                        if !s.active {
                            if op.holds(v, limit) {
                                s.run = s.run.saturating_add(1);
                                if s.run >= count {
                                    s.active = true;
                                    s.run = 0;
                                    raise = true;
                                }
                            } else {
                                s.run = 0;
                            }
                        } else {
                            let cleared = if clear_at == limit {
                                !op.holds(v, limit)
                            } else {
                                match op {
                                    Cmp::Lt | Cmp::Le => v >= clear_at,
                                    Cmp::Gt | Cmp::Ge => v <= clear_at,
                                    _ => !op.holds(v, limit),
                                }
                            };
                            if cleared {
                                s.active = false;
                                clear = true;
                            }
                        }
                    }
                }
                Cond::Change { src, delta } => {
                    if let Some(v) = sample(src) {
                        src_hit = src;
                        value = v;
                        if !s.seen || v.abs_diff(s.last_val) >= delta {
                            s.seen = true;
                            s.last_val = v;
                            raise = true;
                        }
                    }
                }
                Cond::Edge { src, to } => {
                    if let Some(v) = sample(src) {
                        src_hit = src;
                        value = v;
                        let v = (v != 0) as i32;
                        if s.seen && s.prev != v && (to == Edge::Any || (to == Edge::High) == (v == 1)) {
                            raise = true;
                        }
                        s.seen = true;
                        s.prev = v;
                    }
                }
            }
            if !raise && !clear {
                continue;
            }
            for a in &rule.actions {
                match *a {
                    Action::Report { mask, to } if raise => {
                        report.0 |= mask;
                        report.1 |= if to == 0 { self.recipe.report_to } else { to };
                    }
                    Action::Alarm { off, len, to } => {
                        let to = if to == 0 { self.recipe.alarm_to } else { to };
                        if raise {
                            let limited = rule.limit_5min > 0
                                && s.alarmed
                                && now_s.wrapping_sub(s.last_alarm_s) < rule.limit_5min as u32 * 300;
                            if !limited {
                                s.alarmed = true;
                                s.last_alarm_s = now_s;
                                let _ = fx.push(Effect::Alarm { rule: rule.id, raised: true, src: src_hit, value, off, len, to });
                            }
                        } else {
                            let _ = fx.push(Effect::Alarm { rule: rule.id, raised: false, src: src_hit, value, off, len, to });
                        }
                    }
                    Action::SetOut { src, on } => {
                        let _ = fx.push(Effect::SetOut { src, on: if raise { on } else { !on } });
                    }
                    Action::SetInterval { secs } => {
                        self.interval_override = if raise { Some(secs) } else { None };
                    }
                    _ => {}
                }
            }
        }
        if report.0 != 0 {
            let _ = fx.push(Effect::Report { mask: report.0, to: report.1 });
        }
    }

    /// The engine's state, for keeping it across a deep sleep.
    pub fn save_state(&self, out: &mut Vec<u8, STATE_LEN>) {
        out.clear();
        let _ = out.push(1);
        let _ = out.push(self.interval_override.is_some() as u8);
        for s in &self.st {
            let _ = out.extend_from_slice(&[s.active as u8, s.run, s.seen as u8, s.alarmed as u8]);
            let _ = out.extend_from_slice(&s.last_fire_s.to_le_bytes());
            let _ = out.extend_from_slice(&s.last_alarm_s.to_le_bytes());
            let _ = out.extend_from_slice(&s.last_val.to_le_bytes());
            let _ = out.extend_from_slice(&s.prev.to_le_bytes());
        }
    }

    /// Takes a state saved by [`Engine::save_state`] for the same recipe;
    /// anything else is ignored.
    pub fn restore_state(&mut self, b: &[u8]) {
        if b.len() != STATE_LEN || b[0] != 1 {
            return;
        }
        let le = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        for (n, s) in self.st.iter_mut().enumerate() {
            let o = 2 + n * 20;
            *s = RuleState {
                active: b[o] != 0,
                run: b[o + 1],
                seen: b[o + 2] != 0,
                alarmed: b[o + 3] != 0,
                last_fire_s: le(o + 4),
                last_alarm_s: le(o + 8),
                last_val: le(o + 12) as i32,
                prev: le(o + 16) as i32,
            };
        }
        // An active SetInterval rule is simply run again: its condition still holds.
        self.interval_override = None;
        for (n, r) in self.recipe.rules.iter().enumerate() {
            if self.st[n].active {
                for a in &r.actions {
                    if let Action::SetInterval { secs } = a {
                        self.interval_override = Some(*secs);
                    }
                }
            }
        }
    }
}

// -------------------------------------------------------------- messages

/// One value in a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Value {
    pub chan: u8,
    pub kind: u8,
    pub value: i32,
}

/// The readings of one cycle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sample {
    /// Seconds between the reading and the message (saturates at 65535).
    pub age_s: u16,
    pub values: Vec<Value, MAX_SOURCES>,
}

pub const OP_READINGS: u8 = 1;
pub const OP_ALARM: u8 = 2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    Readings { seq: u8, samples: Vec<Sample, 8> },
    Alarm { rule: u8, raised: bool, age_s: u16, value: Value, text: Vec<u8, 48> },
}

fn put_varint(out: &mut Vec<u8, MAX_MSG>, v: i32) -> bool {
    let mut z = ((v << 1) ^ (v >> 31)) as u32;
    loop {
        let b = (z & 0x7f) as u8;
        z >>= 7;
        let ok = if z == 0 { out.push(b) } else { out.push(b | 0x80) };
        if ok.is_err() {
            return false;
        }
        if z == 0 {
            return true;
        }
    }
}

fn get_varint(b: &mut &[u8]) -> Option<i32> {
    let mut z = 0u32;
    for shift in (0..35).step_by(7) {
        let (&x, rest) = b.split_first()?;
        *b = rest;
        z |= ((x & 0x7f) as u32).checked_shl(shift)?;
        if x & 0x80 == 0 {
            return Some(((z >> 1) as i32) ^ -((z & 1) as i32));
        }
    }
    None
}

/// Builds a READINGS message (without the content type byte) from as many
/// of the samples as fit, oldest first. Returns the message and how many
/// samples it holds.
pub fn encode_readings(seq: u8, samples: &[Sample]) -> (Vec<u8, MAX_MSG>, usize) {
    let mut out: Vec<u8, MAX_MSG> = Vec::new();
    let _ = out.extend_from_slice(&[OP_READINGS, seq, 0]);
    let mut n = 0;
    for s in samples.iter().take(8) {
        let before = out.len();
        let mut ok = out.extend_from_slice(&s.age_s.to_le_bytes()).is_ok() && out.push(s.values.len() as u8).is_ok();
        for v in &s.values {
            ok = ok && out.extend_from_slice(&[v.chan, v.kind]).is_ok() && put_varint(&mut out, v.value);
        }
        if !ok {
            out.truncate(before);
            break;
        }
        n += 1;
    }
    out[2] = n as u8;
    (out, n)
}

/// Builds an ALARM message (without the content type byte).
pub fn encode_alarm(rule: u8, raised: bool, age_s: u16, v: Value, text: &str) -> Vec<u8, MAX_MSG> {
    let mut out: Vec<u8, MAX_MSG> = Vec::new();
    let _ = out.extend_from_slice(&[OP_ALARM, rule, raised as u8]);
    let _ = out.extend_from_slice(&age_s.to_le_bytes());
    let _ = out.extend_from_slice(&[v.chan, v.kind]);
    put_varint(&mut out, v.value);
    for &c in text.as_bytes().iter().take(48) {
        if out.push(c).is_err() {
            break;
        }
    }
    out
}

pub fn decode(msg: &[u8]) -> Option<Message> {
    let (&op, mut b) = msg.split_first()?;
    match op {
        OP_READINGS => {
            let (&seq, rest) = b.split_first()?;
            let (&n, rest) = rest.split_first()?;
            b = rest;
            let mut samples = Vec::new();
            for _ in 0..n {
                let age_s = u16::from_le_bytes([*b.first()?, *b.get(1)?]);
                let m = *b.get(2)? as usize;
                b = &b[3..];
                if m > MAX_SOURCES {
                    return None;
                }
                let mut values = Vec::new();
                for _ in 0..m {
                    let (chan, kind) = (*b.first()?, *b.get(1)?);
                    b = &b[2..];
                    let value = get_varint(&mut b)?;
                    values.push(Value { chan, kind, value }).ok()?;
                }
                samples.push(Sample { age_s, values }).ok()?;
            }
            b.is_empty().then_some(Message::Readings { seq, samples })
        }
        OP_ALARM => {
            let rule = *b.first()?;
            let raised = *b.get(1)? & 1 != 0;
            let age_s = u16::from_le_bytes([*b.get(2)?, *b.get(3)?]);
            let (chan, kind) = (*b.get(4)?, *b.get(5)?);
            let mut rest = &b[6..];
            let value = get_varint(&mut rest)?;
            let text = Vec::from_slice(&rest[..rest.len().min(48)]).ok()?;
            Some(Message::Alarm { rule, raised, age_s, value: Value { chan, kind, value }, text })
        }
        _ => None,
    }
}

// ------------------------------------------------- companion commands

/// Operations of `Command::Sensor` and `Event::Sensor`.
///
/// Commands to a node (the connected one, or a remote one by address):
/// `GET_RECIPE`, `WRITE_RECIPE`, `READ_NOW`, `GET_STATUS`. Commands from 0x80
/// are for a host (history, test run, names).
pub mod op {
    pub const GET_RECIPE: u8 = 1;
    /// `offset (u16) | total (u16) | bytes`: the recipe in pieces, in order.
    pub const WRITE_RECIPE: u8 = 2;
    pub const READ_NOW: u8 = 3;
    pub const GET_STATUS: u8 = 4;
    /// `slot (1) | master public identity (64) | device list (optional, the
    /// signed list of `maarse_crypto::person`)`: makes a person an owner of the
    /// node. Over USB or Bluetooth always; from far away only an owner may.
    pub const ADD_OWNER: u8 = 6;
    /// `slot (1)`.
    pub const REMOVE_OWNER: u8 = 7;
    pub const GET_OWNERS: u8 = 8;
    /// Host: the sensors it knows (`EV_LIST`).
    pub const LIST: u8 = 0x81;
    /// Host: `node (16) | channel (1) | from (u32 LE, Unix s) | to (u32)`.
    pub const SERIES: u8 = 0x82;
    /// Host: `scenario (1) | recipe`; runs the recipe on test data.
    pub const SIMULATE: u8 = 0x83;
    /// Host: `node (16) | name`; gives a sensor its name.
    pub const RENAME: u8 = 0x84;
    /// Host: `node (16) | on (1)`; favourite or not.
    pub const FAVOURITE: u8 = 0x85;
    /// Host: `node (16)`; forget a sensor and its history.
    pub const FORGET: u8 = 0x86;
    /// Host: the information a sensor needs to take its owner: the person's
    /// master public identity (64) and signed device list (answer `EV_OWNER_INFO`).
    pub const OWNER_INFO: u8 = 0x87;

    /// Events: `offset (u16) | total (u16) | bytes` of the node's recipe.
    pub const EV_RECIPE: u8 = 1;
    /// Events: a `Status`.
    pub const EV_STATUS: u8 = 2;
    /// Events: a message from a sensor (`sensor::decode`).
    pub const EV_MESSAGE: u8 = 3;
    /// Events: owners, each `slot (1) | person address (16) | list version
    /// (u32 LE) | devices in the list (1)`.
    pub const EV_OWNERS: u8 = 4;
    pub const EV_LIST: u8 = 0x81;
    pub const EV_SERIES: u8 = 0x82;
    pub const EV_SIMULATION: u8 = 0x83;
    /// Events: `master public identity (64) | signed device list`, for `ADD_OWNER`.
    pub const EV_OWNER_INFO: u8 = 0x87;
}

/// The pin plan (`Recipe::pin_plan`) of a stored recipe; 0 if it is not valid.
pub fn recipe_pin_plan(recipe: &[u8]) -> u8 {
    Recipe::from_bytes(recipe).map_or(0, |r| r.pin_plan())
}

/// True if a stored recipe wants the node to sleep between cycles. Boards
/// read this at start-up, before the node runs, to decide what to switch on.
pub fn recipe_sleeps(recipe: &[u8]) -> bool {
    recipe.len() > 2 && recipe[0] == RECIPE_VERSION && recipe[1] & FLAG_SLEEP != 0
}

/// Why a recipe was not accepted, in `Status::error`.
pub mod error {
    pub const NONE: u8 = 0;
    pub const NOT_OWNER: u8 = 1;
    pub const MALFORMED: u8 = 2;
    pub const VERSION: u8 = 3;
    pub const TOO_LARGE: u8 = 4;
    pub const INVALID: u8 = 5;
    pub const SEQUENCE: u8 = 6;
    pub const STORAGE: u8 = 7;
    pub const NO_SENSORS: u8 = 8;
}

impl RecipeError {
    pub fn code(self) -> u8 {
        match self {
            RecipeError::Malformed => error::MALFORMED,
            RecipeError::Version => error::VERSION,
            RecipeError::TooLarge => error::TOO_LARGE,
            RecipeError::Invalid => error::INVALID,
        }
    }
}

/// CRC-16/CCITT-FALSE, for checking that a recipe arrived whole.
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0xFFFFu16;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 };
        }
    }
    crc
}

/// Status flags in `Status::flags`.
pub mod status {
    pub const HAS_RECIPE: u8 = 0x01;
    pub const SLEEPS: u8 = 0x02;
    pub const ON_USB: u8 = 0x04;
    pub const CLOCK_SET: u8 = 0x08;
}

/// What a sensor node says about itself (`op::EV_STATUS`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// `error::*` of the last recipe write (0 = none).
    pub error: u8,
    pub flags: u8,
    pub recipe_len: u16,
    pub recipe_crc: u16,
    pub interval_s: u16,
    pub battery_mv: u16,
    pub owners: u8,
    /// Cycles run since the node started.
    pub cycles: u16,
}

impl Status {
    pub const LEN: usize = 13;

    pub fn to_bytes(&self) -> [u8; Self::LEN] {
        let mut b = [0u8; Self::LEN];
        b[0] = self.error;
        b[1] = self.flags;
        b[2..4].copy_from_slice(&self.recipe_len.to_le_bytes());
        b[4..6].copy_from_slice(&self.recipe_crc.to_le_bytes());
        b[6..8].copy_from_slice(&self.interval_s.to_le_bytes());
        b[8..10].copy_from_slice(&self.battery_mv.to_le_bytes());
        b[10] = self.owners;
        b[11..13].copy_from_slice(&self.cycles.to_le_bytes());
        b
    }

    pub fn from_bytes(b: &[u8]) -> Option<Status> {
        let b = b.get(..Self::LEN)?;
        let u = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
        Some(Status {
            error: b[0],
            flags: b[1],
            recipe_len: u(2),
            recipe_crc: u(4),
            interval_s: u(6),
            battery_mv: u(8),
            owners: b[10],
            cycles: u(11),
        })
    }
}

/// Puts a recipe together from the pieces of `op::WRITE_RECIPE`.
#[derive(Default)]
pub struct RecipeAssembler {
    buf: Vec<u8, MAX_RECIPE>,
    total: u16,
}

impl RecipeAssembler {
    /// Takes one piece. Pieces must come in order from offset 0; a piece at
    /// offset 0 starts over. When the last piece has arrived, returns the
    /// recipe (checked) and its bytes.
    pub fn push(&mut self, offset: u16, total: u16, bytes: &[u8]) -> Result<Option<(Recipe, Vec<u8, MAX_RECIPE>)>, u8> {
        if total as usize > MAX_RECIPE || total == 0 {
            self.buf.clear();
            return Err(error::TOO_LARGE);
        }
        if offset == 0 {
            self.buf.clear();
            self.total = total;
        }
        if total != self.total || offset as usize != self.buf.len() {
            self.buf.clear();
            return Err(error::SEQUENCE);
        }
        if self.buf.extend_from_slice(bytes).is_err() || self.buf.len() > total as usize {
            self.buf.clear();
            return Err(error::TOO_LARGE);
        }
        if self.buf.len() < total as usize {
            return Ok(None);
        }
        let bytes = core::mem::take(&mut self.buf);
        match Recipe::from_bytes(&bytes) {
            Ok(r) => Ok(Some((r, bytes))),
            Err(e) => Err(e.code()),
        }
    }
}

// ---------------------------------------------------------------- BME280

/// Compensation for the Bosch BME280 and BMP280 (integer formulas from the
/// data sheet). Shared by the firmware for both boards.
pub mod bme280 {
    /// Calibration of one chip.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct Calib {
        pub t1: u16,
        pub t2: i16,
        pub t3: i16,
        pub p1: u16,
        pub p2: i16,
        pub p3: i16,
        pub p4: i16,
        pub p5: i16,
        pub p6: i16,
        pub p7: i16,
        pub p8: i16,
        pub p9: i16,
        pub h1: u8,
        pub h2: i16,
        pub h3: u8,
        pub h4: i16,
        pub h5: i16,
        pub h6: i8,
    }

    /// Register 0xD0 of a BME280 (a BMP280 reads 0x58 and has no humidity).
    pub const ID_BME280: u8 = 0x60;
    pub const ID_BMP280: u8 = 0x58;

    impl Calib {
        /// `a` = registers 0x88..=0x9F (24 bytes), `h1` = register 0xA1,
        /// `h` = registers 0xE1..=0xE7 (7 bytes; all zero for a BMP280).
        pub fn from_regs(a: &[u8; 24], h1: u8, h: &[u8; 7]) -> Calib {
            let u = |i: usize| u16::from_le_bytes([a[i], a[i + 1]]);
            let s = |i: usize| u16::from_le_bytes([a[i], a[i + 1]]) as i16;
            Calib {
                t1: u(0),
                t2: s(2),
                t3: s(4),
                p1: u(6),
                p2: s(8),
                p3: s(10),
                p4: s(12),
                p5: s(14),
                p6: s(16),
                p7: s(18),
                p8: s(20),
                p9: s(22),
                h1,
                h2: i16::from_le_bytes([h[0], h[1]]),
                h3: h[2],
                h4: ((h[3] as i8 as i16) << 4) | (h[4] & 0x0f) as i16,
                h5: ((h[5] as i8 as i16) << 4) | (h[4] >> 4) as i16,
                h6: h[6] as i8,
            }
        }

        /// Raw 20-bit temperature and pressure and 16-bit humidity (registers
        /// 0xF7..=0xFE) to 0.01 °C, Pa and 0.01 %RH (`None` without humidity data).
        pub fn compensate(&self, data: &[u8; 8], has_humidity: bool) -> (i32, u32, Option<u32>) {
            let adc_p = ((data[0] as i32) << 12) | ((data[1] as i32) << 4) | (data[2] as i32 >> 4);
            let adc_t = ((data[3] as i32) << 12) | ((data[4] as i32) << 4) | (data[5] as i32 >> 4);
            let adc_h = ((data[6] as i32) << 8) | data[7] as i32;

            let t1 = self.t1 as i32;
            let var1 = (((adc_t >> 3) - (t1 << 1)) * self.t2 as i32) >> 11;
            let d = (adc_t >> 4) - t1;
            let var2 = (((d * d) >> 12) * self.t3 as i32) >> 14;
            let t_fine = var1 + var2;
            let temp = (t_fine * 5 + 128) >> 8;

            let mut v1 = t_fine as i64 - 128_000;
            let mut v2 = v1 * v1 * self.p6 as i64;
            v2 += (v1 * self.p5 as i64) << 17;
            v2 += (self.p4 as i64) << 35;
            v1 = ((v1 * v1 * self.p3 as i64) >> 8) + ((v1 * self.p2 as i64) << 12);
            v1 = (((1i64 << 47) + v1) * self.p1 as i64) >> 33;
            let press = if v1 == 0 {
                0
            } else {
                let mut p = 1_048_576 - adc_p as i64;
                p = (((p << 31) - v2) * 3125) / v1;
                let v1 = (self.p9 as i64 * (p >> 13) * (p >> 13)) >> 25;
                let v2 = (self.p8 as i64 * p) >> 19;
                p = ((p + v1 + v2) >> 8) + ((self.p7 as i64) << 4);
                // Q24.8 -> Pa
                (p >> 8) as u32
            };

            let hum = has_humidity.then(|| {
                let mut x = t_fine - 76_800;
                x = ((((adc_h << 14) - ((self.h4 as i32) << 20) - (self.h5 as i32 * x)) + 16_384) >> 15)
                    * (((((((x * self.h6 as i32) >> 10) * (((x * self.h3 as i32) >> 11) + 32_768)) >> 10) + 2_097_152)
                        * self.h2 as i32
                        + 8192)
                        >> 14);
                x -= (((x >> 15) * (x >> 15)) >> 7) * self.h1 as i32 >> 4;
                let x = x.clamp(0, 419_430_400);
                // Q22.10 %RH -> 0.01 %RH
                ((x >> 12) as u64 * 100 / 1024) as u32
            });
            (temp, press, hum)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frost_recipe() -> Recipe {
        let mut r = Recipe::default();
        r.sources.push(Source { chan: 1, kind: kind::TEMPERATURE, input: Input::Bme280 { addr: 0x76, what: 0 } }).unwrap();
        r.sources.push(Source { chan: 2, kind: kind::DIGITAL, input: Input::GpioIn { pin: 3, pull: Pull::Up, invert: false, wake: false } }).unwrap();
        r.sources.push(Source { chan: 3, kind: kind::DIGITAL, input: Input::GpioOut { pin: 2, active_low: false } }).unwrap();
        r.text.extend_from_slice(b"Cellar is below 10 C").unwrap();
        // Report every 15 minutes.
        r.rules.push(Rule { id: 0, cond: Cond::Every { secs: 900 }, limit_5min: 0, actions: Vec::from_slice(&[Action::Report { mask: 0xff, to: 0 }]).unwrap() }).unwrap();
        // Frost alarm: below 10.00 C twice in a row, clear above 12.00 C, heater on.
        r.rules.push(Rule {
            id: 1,
            cond: Cond::Compare { src: 0, op: Cmp::Lt, value: 1000, clear: 1200, count: 2 },
            limit_5min: 6,
            actions: Vec::from_slice(&[Action::Alarm { off: 0, len: 20, to: 0 }, Action::SetOut { src: 2, on: true }]).unwrap(),
        })
        .unwrap();
        // Door opened.
        r.rules.push(Rule {
            id: 2,
            cond: Cond::Edge { src: 1, to: Edge::High },
            limit_5min: 2,
            actions: Vec::from_slice(&[Action::Alarm { off: 0, len: 6, to: 0 }]).unwrap(),
        })
        .unwrap();
        r
    }

    #[test]
    fn recipe_round_trips_and_is_small() {
        let r = frost_recipe();
        let b = r.to_bytes().unwrap();
        assert!(b.len() < 110, "frost recipe is {} bytes", b.len());
        assert_eq!(Recipe::from_bytes(&b).unwrap(), r);
    }

    #[test]
    fn recipe_with_channel_round_trips() {
        let mut r = frost_recipe();
        r.flags |= FLAG_CHANNEL;
        r.channel = Some(Address([9; 16]));
        r.report_to = TO_CHANNEL | 1;
        assert_eq!(Recipe::from_bytes(&r.to_bytes().unwrap()).unwrap(), r);
        // A channel recipient without a channel is invalid.
        let mut bad = frost_recipe();
        bad.report_to = TO_CHANNEL;
        assert_eq!(bad.to_bytes(), Err(RecipeError::Invalid));
    }

    #[test]
    fn bad_recipes_are_refused() {
        let good = frost_recipe().to_bytes().unwrap();
        assert_eq!(Recipe::from_bytes(&[]), Err(RecipeError::Malformed));
        let mut v = good.clone();
        v[0] = 9;
        assert_eq!(Recipe::from_bytes(&v), Err(RecipeError::Version));
        for cut in 0..good.len() {
            assert!(Recipe::from_bytes(&good[..cut]).is_err(), "cut at {cut}");
        }
        let mut long = good.clone();
        long.push(0).unwrap();
        assert_eq!(Recipe::from_bytes(&long), Err(RecipeError::Malformed));
        // A rule that reads an output, or points at a missing source.
        let mut r = frost_recipe();
        r.rules[1].cond = Cond::Compare { src: 2, op: Cmp::Lt, value: 0, clear: 0, count: 1 };
        assert_eq!(r.validate(), Err(RecipeError::Invalid));
        let mut r = frost_recipe();
        r.rules[1].cond = Cond::Compare { src: 7, op: Cmp::Lt, value: 0, clear: 0, count: 1 };
        assert_eq!(r.validate(), Err(RecipeError::Invalid));
        // Text out of range, interval too short, duplicate channel.
        let mut r = frost_recipe();
        r.rules[1].actions[0] = Action::Alarm { off: 10, len: 40, to: 0 };
        assert_eq!(r.validate(), Err(RecipeError::Invalid));
        let mut r = frost_recipe();
        r.interval_s = 5;
        assert_eq!(r.validate(), Err(RecipeError::Invalid));
        let mut r = frost_recipe();
        r.sources[1].chan = 1;
        assert_eq!(r.validate(), Err(RecipeError::Invalid));
    }

    #[test]
    fn garbage_never_panics() {
        let mut x = 0x1234_5678u32;
        for _ in 0..20_000 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let n = (x % 120) as usize;
            let mut b = [0u8; 120];
            for (i, c) in b.iter_mut().enumerate() {
                *c = (x.rotate_left(i as u32) >> 3) as u8;
            }
            b[0] = RECIPE_VERSION;
            let _ = Recipe::from_bytes(&b[..n]);
            let _ = decode(&b[..n]);
        }
    }

    fn run(e: &mut Engine, now: u32, t: Option<i32>, door: Option<i32>) -> Vec<Effect, 16> {
        let mut fx = Vec::new();
        e.step(now, &[t, door, Some(0)], &mut fx);
        fx
    }

    #[test]
    fn frost_alarm_needs_two_readings_clears_with_hysteresis_and_runs_the_heater() {
        let mut e = Engine::new(frost_recipe());
        // First cycle: the "every" rule fires (report all), nothing else.
        let fx = run(&mut e, 0, Some(1500), Some(0));
        assert_eq!(fx.as_slice(), &[Effect::Report { mask: 0xff, to: 1 }]);
        // One cold reading is not enough.
        assert!(run(&mut e, 60, Some(900), Some(0)).is_empty());
        // The second raises the alarm and the heater.
        let fx = run(&mut e, 120, Some(850), Some(0));
        assert_eq!(
            fx.as_slice(),
            &[
                Effect::Alarm { rule: 1, raised: true, src: 0, value: 850, off: 0, len: 20, to: 1 },
                Effect::SetOut { src: 2, on: true }
            ]
        );
        // Warmer than 10 but not above 12: still active, nothing sent.
        assert!(run(&mut e, 180, Some(1100), Some(0)).is_empty());
        // Above 12: all clear and the heater goes off.
        let fx = run(&mut e, 240, Some(1250), Some(0));
        assert_eq!(
            fx.as_slice(),
            &[
                Effect::Alarm { rule: 1, raised: false, src: 0, value: 1250, off: 0, len: 20, to: 1 },
                Effect::SetOut { src: 2, on: false }
            ]
        );
        // A missing reading changes nothing.
        assert!(run(&mut e, 300, None, Some(0)).is_empty());
    }

    #[test]
    fn alarm_rate_limit_holds_back_a_second_alarm() {
        let mut e = Engine::new(frost_recipe());
        run(&mut e, 0, Some(1500), Some(0));
        run(&mut e, 10, Some(900), Some(0));
        let fx = run(&mut e, 20, Some(900), Some(0));
        assert!(fx.iter().any(|f| matches!(f, Effect::Alarm { raised: true, .. })));
        run(&mut e, 30, Some(1300), Some(0)); // clears
        run(&mut e, 40, Some(900), Some(0));
        // Raised again 10 s after the first alarm: held back (limit 30 minutes).
        let fx = run(&mut e, 50, Some(900), Some(0));
        assert!(!fx.iter().any(|f| matches!(f, Effect::Alarm { raised: true, .. })), "{fx:?}");
        // Cleared and raised again after 31 minutes: sent.
        run(&mut e, 60, Some(1300), Some(0));
        run(&mut e, 1900, Some(900), Some(0));
        let fx = run(&mut e, 1910, Some(900), Some(0));
        assert!(fx.iter().any(|f| matches!(f, Effect::Alarm { raised: true, .. })));
    }

    #[test]
    fn edge_rule_fires_on_the_change_not_on_the_level() {
        let mut e = Engine::new(frost_recipe());
        run(&mut e, 0, Some(1500), Some(0)); // first sight: no edge
        let fx = run(&mut e, 10, Some(1500), Some(1));
        assert!(fx.iter().any(|f| matches!(f, Effect::Alarm { rule: 2, raised: true, off: 0, len: 6, .. })), "{fx:?}");
        // Still high: no new alarm.
        assert!(run(&mut e, 20, Some(1500), Some(1)).is_empty());
    }

    #[test]
    fn a_pin_wake_fires_the_edge_rule_even_after_a_long_sleep() {
        let mut r = Recipe::default();
        r.sources.push(Source { chan: 1, kind: kind::DIGITAL, input: Input::GpioIn { pin: 0, pull: Pull::Up, invert: true, wake: true } }).unwrap();
        r.text.extend_from_slice(b"Post").unwrap();
        r.rules.push(Rule { id: 1, cond: Cond::Edge { src: 0, to: Edge::High }, limit_5min: 0, actions: Vec::from_slice(&[Action::Alarm { off: 0, len: 4, to: 0 }]).unwrap() }).unwrap();
        assert_eq!(Recipe::from_bytes(&r.to_bytes().unwrap()).unwrap(), r, "the wake flag survives");
        assert_eq!(r.wake_pins().as_slice(), &[(0, false)], "an inverted pin wakes on a low level");
        let mut e = Engine::new(r.clone());
        let mut fx = Vec::new();
        // Without the wake hint the first sight of a pin never fires (no earlier state).
        e.step(0, &[Some(1)], &mut fx);
        assert!(fx.is_empty());
        // A fresh engine (a restart after a deep sleep) woken by the pin: post.
        let mut e = Engine::new(r.clone());
        e.pin_woke(0, 1);
        e.step(5000, &[Some(1)], &mut fx);
        assert!(fx.iter().any(|f| matches!(f, Effect::Alarm { raised: true, .. })), "{fx:?}");
        // A pin left on 1 does not fire again at the next timer cycle.
        fx.clear();
        e.step(5900, &[Some(1)], &mut fx);
        assert!(fx.is_empty());
    }

    #[test]
    fn a_wake_pin_needs_a_pull_against_its_wake_level() {
        let mut r = Recipe::default();
        r.sources.push(Source { chan: 1, kind: kind::DIGITAL, input: Input::GpioIn { pin: 3, pull: Pull::Down, invert: false, wake: true } }).unwrap();
        assert_eq!(r.validate(), Ok(()), "high wakes, a pull-down holds it low");
        r.sources[0].input = Input::GpioIn { pin: 3, pull: Pull::Up, invert: false, wake: true };
        assert_eq!(r.validate(), Err(RecipeError::Invalid), "a pull-up would wake it at once");
        r.sources[0].input = Input::GpioIn { pin: 3, pull: Pull::None, invert: true, wake: true };
        assert_eq!(r.validate(), Err(RecipeError::Invalid), "a floating wake pin wakes at random");
        // The pin plan changes with the set of wake pins (the board restarts).
        let a = {
            r.sources[0].input = Input::GpioIn { pin: 3, pull: Pull::Up, invert: true, wake: true };
            r.pin_plan()
        };
        r.sources[0].input = Input::GpioIn { pin: 4, pull: Pull::Up, invert: true, wake: true };
        assert_ne!(a, r.pin_plan());
    }

    #[test]
    fn change_rule_reports_a_move_and_set_interval_lasts_while_active() {
        let mut r = Recipe::default();
        r.sources.push(Source { chan: 1, kind: kind::TEMPERATURE, input: Input::ChipTemp }).unwrap();
        r.rules.push(Rule { id: 0, cond: Cond::Change { src: 0, delta: 50 }, limit_5min: 0, actions: Vec::from_slice(&[Action::Report { mask: 1, to: 0 }]).unwrap() }).unwrap();
        r.rules.push(Rule {
            id: 1,
            cond: Cond::Compare { src: 0, op: Cmp::Gt, value: 3000, clear: 2800, count: 1 },
            limit_5min: 0,
            actions: Vec::from_slice(&[Action::SetInterval { secs: 60 }]).unwrap(),
        })
        .unwrap();
        let mut e = Engine::new(r);
        let mut fx = Vec::new();
        e.step(0, &[Some(2000)], &mut fx);
        assert_eq!(fx.as_slice(), &[Effect::Report { mask: 1, to: 1 }], "baseline is reported");
        fx.clear();
        e.step(60, &[Some(2030)], &mut fx);
        assert!(fx.is_empty(), "a move of 30 is below the delta of 50");
        e.step(120, &[Some(3100)], &mut fx);
        assert_eq!(fx.as_slice(), &[Effect::Report { mask: 1, to: 1 }]);
        assert_eq!(e.interval_s(), 60);
        fx.clear();
        e.step(180, &[Some(2700)], &mut fx);
        assert_eq!(e.interval_s(), 900, "back to the normal interval");
    }

    #[test]
    fn state_survives_a_deep_sleep() {
        let mut e = Engine::new(frost_recipe());
        run(&mut e, 0, Some(1500), Some(0));
        run(&mut e, 10, Some(900), Some(0));
        run(&mut e, 20, Some(900), Some(0)); // active
        let mut s = Vec::new();
        e.save_state(&mut s);
        let mut e2 = Engine::new(frost_recipe());
        e2.restore_state(&s);
        // The rule is still active, so it clears (and does not raise again).
        let fx = run(&mut e2, 30, Some(1300), Some(0));
        assert!(fx.iter().any(|f| matches!(f, Effect::Alarm { raised: false, .. })), "{fx:?}");
        e2.restore_state(&[1, 2, 3]); // wrong length: ignored
    }

    #[test]
    fn readings_round_trip_and_fit_the_message_limit() {
        let mk = |age| Sample {
            age_s: age,
            values: Vec::from_slice(&[
                Value { chan: 1, kind: kind::TEMPERATURE, value: -1234 },
                Value { chan: 2, kind: kind::HUMIDITY, value: 6200 },
                Value { chan: 3, kind: kind::PRESSURE, value: 101_325 },
                Value { chan: 4, kind: kind::VOLTAGE, value: 3840 },
            ])
            .unwrap(),
        };
        let samples = [mk(0), mk(900), mk(1800), mk(2700), mk(3600), mk(4500), mk(5400), mk(6300)];
        let (msg, n) = encode_readings(7, &samples);
        assert!(msg.len() <= MAX_MSG);
        assert!(n >= 4 && n < 8, "{n} samples fit");
        match decode(&msg).unwrap() {
            Message::Readings { seq, samples: got } => {
                assert_eq!(seq, 7);
                assert_eq!(got.as_slice(), &samples[..n]);
            }
            m => panic!("{m:?}"),
        }
    }

    #[test]
    fn alarm_message_round_trips() {
        let v = Value { chan: 1, kind: kind::TEMPERATURE, value: 850 };
        let m = encode_alarm(1, true, 12, v, "Cellar is below 10 C");
        assert_eq!(
            decode(&m),
            Some(Message::Alarm { rule: 1, raised: true, age_s: 12, value: v, text: Vec::from_slice(b"Cellar is below 10 C").unwrap() })
        );
        assert!(decode(&m[..4]).is_none());
        assert!(decode(&[9, 9]).is_none());
    }

    #[test]
    fn varints_cover_the_range() {
        for v in [0, 1, -1, 63, -64, 64, 8191, -8192, i32::MAX, i32::MIN, 100_653] {
            let mut out: Vec<u8, MAX_MSG> = Vec::new();
            assert!(put_varint(&mut out, v));
            let mut s: &[u8] = &out;
            assert_eq!(get_varint(&mut s), Some(v));
            assert!(s.is_empty());
        }
    }

    #[test]
    fn recipe_pieces_are_put_together_and_checked() {
        let bytes = frost_recipe().to_bytes().unwrap();
        let mut a = RecipeAssembler::default();
        let (first, rest) = bytes.split_at(50);
        assert_eq!(a.push(0, bytes.len() as u16, first), Ok(None));
        let got = a.push(50, bytes.len() as u16, rest).unwrap().unwrap();
        assert_eq!(got.0, frost_recipe());
        assert_eq!(got.1.as_slice(), bytes.as_slice());
        // Out of order, too large, and a broken recipe are refused.
        assert_eq!(a.push(0, bytes.len() as u16, first), Ok(None));
        assert_eq!(a.push(10, bytes.len() as u16, rest), Err(error::SEQUENCE));
        assert_eq!(a.push(0, 300, first), Err(error::TOO_LARGE));
        assert_eq!(a.push(0, 3, &[1, 2, 3]), Err(error::MALFORMED));
        assert_eq!(a.push(0, 3, &[9, 0, 0]), Err(error::VERSION));
    }

    #[test]
    fn status_round_trips_and_crc_is_the_standard_one() {
        let s = Status {
            error: 5,
            flags: status::HAS_RECIPE | status::SLEEPS,
            recipe_len: 97,
            recipe_crc: 0xBEEF,
            interval_s: 900,
            battery_mv: 3840,
            owners: 2,
            cycles: 12,
        };
        assert_eq!(Status::from_bytes(&s.to_bytes()), Some(s));
        assert_eq!(Status::from_bytes(&[0; 5]), None);
        // CRC-16/CCITT-FALSE check value.
        assert_eq!(crc16(b"123456789"), 0x29B1);
    }

    /// The example in the Bosch BME280 data sheet (section 4.2.3).
    #[test]
    fn bme280_matches_the_data_sheet_example() {
        use bme280::Calib;
        let c = Calib {
            t1: 27504,
            t2: 26435,
            t3: -1000,
            p1: 36477,
            p2: -10685,
            p3: 3024,
            p4: 2855,
            p5: 140,
            p6: -7,
            p7: 15500,
            p8: -14600,
            p9: 6000,
            ..Calib::default()
        };
        let (adc_p, adc_t) = (415_148u32, 519_888u32);
        let data = [
            (adc_p >> 12) as u8,
            (adc_p >> 4) as u8,
            (adc_p << 4) as u8,
            (adc_t >> 12) as u8,
            (adc_t >> 4) as u8,
            (adc_t << 4) as u8,
            0,
            0,
        ];
        let (t, p, h) = c.compensate(&data, false);
        assert_eq!(t, 2508, "25.08 C");
        assert_eq!(p, 100_653, "100653 Pa");
        assert!(h.is_none());
    }

    #[test]
    fn bme280_humidity_stays_in_range() {
        let c = bme280::Calib { t1: 27504, t2: 26435, t3: -1000, h1: 75, h2: 362, h3: 0, h4: 316, h5: 50, h6: 30, ..Default::default() };
        for raw_h in [0u32, 20_000, 40_000, 65_535] {
            let data = [0x65, 0x5a, 0xc0, 0x7e, 0xed, 0x00, (raw_h >> 8) as u8, raw_h as u8];
            let (_, _, h) = c.compensate(&data, true);
            assert!(h.unwrap() <= 10_000, "{raw_h} -> {h:?}");
        }
    }
}
