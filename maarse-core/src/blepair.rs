//! The Bluetooth code of a radio node: pairing with a 6-digit code. Every
//! node starts with the same factory code ([`DEFAULT_PIN`]); until the person
//! setting it up has chosen their own, nothing but that works over Bluetooth.
//!
//! The node uses ordinary Bluetooth LE pairing (Secure Connections, passkey
//! entry): the code is the passkey, the connection is encrypted, and the
//! node answers nothing over Bluetooth until the other side has paired with
//! it. This module is the part without hardware: the rules for guessing and
//! the form in which paired phones are kept in flash.
//!
//! A fixed passkey leaks about one bit for each pairing that is tried with a
//! wrong code, so a guesser needs about twenty tries. [`Guard`] makes each
//! further try wait longer.

/// The largest code (codes are 6 digits, leading zeros allowed).
pub const MAX_PIN: u32 = 999_999;
/// The factory code, the same for every node.
pub const DEFAULT_PIN: u32 = 123_456;

/// May `pin` be a node's own code?
pub fn is_own_pin(pin: u32) -> bool {
    pin <= MAX_PIN && pin != DEFAULT_PIN
}

/// With the factory code, may a Bluetooth client send the companion command of
/// this kind? Only `GetInfo`, `SetBlePin` and `GetBlePin`.
pub fn command_before_own_code(kind: u8) -> bool {
    matches!(kind, 0x01 | 0x46 | 0x47)
}

/// With the factory code, may the node send a Bluetooth client the event of
/// this kind? Only `Info`, `Error` and `BlePin`: no messages, no settings.
pub fn event_before_own_code(kind: u8) -> bool {
    matches!(kind, 0x81 | 0x8f | 0xc1)
}

/// Failed pairings before pairing is refused for a while.
const FREE_TRIES: u8 = 3;
const FIRST_WAIT_MS: u64 = 10_000;
const LONGEST_WAIT_MS: u64 = 60 * 60 * 1000;

/// Counts failed pairings. While blocked the node must refuse new pairings
/// (phones that are already paired still connect).
#[derive(Debug, Default)]
pub struct Guard {
    failures: u8,
    until_ms: u64,
}

impl Guard {
    pub const fn new() -> Self {
        Guard { failures: 0, until_ms: 0 }
    }

    /// A pairing was tried and did not succeed.
    pub fn failed(&mut self, now_ms: u64) {
        self.failures = self.failures.saturating_add(1);
        if self.failures >= FREE_TRIES {
            let doubling = (self.failures - FREE_TRIES).min(20);
            self.until_ms = now_ms + (FIRST_WAIT_MS << doubling).min(LONGEST_WAIT_MS);
        }
    }

    /// How long pairing is still refused, if it is.
    pub fn blocked_ms(&self, now_ms: u64) -> Option<u64> {
        self.until_ms.checked_sub(now_ms).filter(|&ms| ms > 0)
    }

    /// A new pairing succeeded, or the code was changed.
    pub fn reset(&mut self) {
        *self = Guard::new();
    }
}

/// A paired phone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bond {
    pub addr_kind: u8,
    pub addr: [u8; 6],
    /// Identity resolving key: recognises a phone that changes its address.
    pub irk: Option<[u8; 16]>,
    /// The long-term key of the encrypted connection.
    pub ltk: [u8; 16],
    /// 1 = encrypted, 2 = encrypted and authenticated with the code.
    pub level: u8,
}

/// Phones remembered; a new one replaces the oldest.
pub const MAX_BONDS: usize = 4;
const BOND_LEN: usize = 44;
/// The stored form: count (4) and [`MAX_BONDS`] records.
pub const BONDS_LEN: usize = 4 + MAX_BONDS * BOND_LEN;

/// The paired phones, oldest first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bonds {
    list: heapless::Vec<Bond, MAX_BONDS>,
}

impl Bonds {
    pub const fn new() -> Self {
        Bonds { list: heapless::Vec::new() }
    }

    pub fn iter(&self) -> impl Iterator<Item = &Bond> {
        self.list.iter()
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// Adds a phone, replacing what was known about the same one.
    pub fn add(&mut self, bond: Bond) {
        self.list.retain(|b| {
            let same_key = bond.irk.is_some() && b.irk == bond.irk;
            !(same_key || (b.addr_kind, b.addr) == (bond.addr_kind, bond.addr))
        });
        if self.list.is_full() {
            self.list.remove(0);
        }
        let _ = self.list.push(bond);
    }

    pub fn to_bytes(&self) -> [u8; BONDS_LEN] {
        let mut out = [0u8; BONDS_LEN];
        out[0] = self.list.len() as u8;
        for (b, r) in self.list.iter().zip(out[4..].chunks_exact_mut(BOND_LEN)) {
            r[0] = b.addr_kind;
            r[1..7].copy_from_slice(&b.addr);
            r[7] = b.irk.is_some() as u8;
            r[8..24].copy_from_slice(&b.irk.unwrap_or_default());
            r[24..40].copy_from_slice(&b.ltk);
            r[40] = b.level;
        }
        out
    }

    /// Reads the stored form; anything else (erased flash) gives no phones.
    pub fn from_bytes(data: &[u8]) -> Self {
        let mut bonds = Bonds::new();
        if data.len() < BONDS_LEN || data[0] as usize > MAX_BONDS {
            return bonds;
        }
        for r in data[4..].chunks_exact(BOND_LEN).take(data[0] as usize) {
            let mut b = Bond { addr_kind: r[0], addr: [0; 6], irk: None, ltk: [0; 16], level: r[40] };
            b.addr.copy_from_slice(&r[1..7]);
            if r[7] == 1 {
                let mut irk = [0u8; 16];
                irk.copy_from_slice(&r[8..24]);
                b.irk = Some(irk);
            }
            b.ltk.copy_from_slice(&r[24..40]);
            let _ = bonds.list.push(b);
        }
        bonds
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guessing_waits_longer_and_longer() {
        let mut g = Guard::new();
        g.failed(0);
        g.failed(0);
        assert_eq!(g.blocked_ms(0), None, "a mistyped code costs nothing at first");
        g.failed(0);
        assert_eq!(g.blocked_ms(0), Some(10_000));
        assert_eq!(g.blocked_ms(10_000), None);
        g.failed(10_000);
        assert_eq!(g.blocked_ms(10_000), Some(20_000));
        for _ in 0..200 {
            g.failed(100_000);
        }
        assert_eq!(g.blocked_ms(100_000), Some(60 * 60 * 1000), "at most an hour");
        g.reset();
        assert_eq!(g.blocked_ms(100_000), None);
    }

    #[test]
    fn twenty_guesses_take_hours() {
        let (mut g, mut now) = (Guard::new(), 0u64);
        for _ in 0..20 {
            now += g.blocked_ms(now).unwrap_or(0);
            g.failed(now);
        }
        assert!(now > 8 * 60 * 60 * 1000, "{now} ms");
    }

    #[test]
    fn with_the_factory_code_only_a_new_code_can_be_chosen() {
        use crate::companion::{Command, Event};
        let kind = |c: Command| c.encode()[2];
        for c in [Command::GetInfo, Command::GetBlePin, Command::SetBlePin { pin: 1 }] {
            assert!(command_before_own_code(kind(c.clone())), "{c:?}");
        }
        for c in [Command::SetAdmins { admins: Default::default() }, Command::GetStats, Command::SetTime { unix: 1 }, Command::GetAdmins] {
            assert!(!command_before_own_code(kind(c.clone())), "{c:?}");
        }
        assert!(event_before_own_code(Event::BlePin { own: false }.encode()[2]));
        assert!(event_before_own_code(Event::Error { req: 0, code: 1, text: Default::default() }.encode()[2]));
        let message = Event::DirectReceived { from: crate::Address([1; 16]), hops: 0, text: Default::default() };
        assert!(!event_before_own_code(message.encode()[2]), "no messages");
    }

    #[test]
    fn every_node_starts_with_the_same_code_which_cannot_be_chosen_again() {
        assert_eq!(DEFAULT_PIN, 123_456);
        assert!(!is_own_pin(DEFAULT_PIN));
        assert!(!is_own_pin(MAX_PIN + 1));
        assert!(is_own_pin(0) && is_own_pin(654_321) && is_own_pin(MAX_PIN));
    }

    fn bond(n: u8) -> Bond {
        Bond { addr_kind: 1, addr: [n; 6], irk: Some([n; 16]), ltk: [n ^ 0xFF; 16], level: 2 }
    }

    #[test]
    fn paired_phones_survive_the_stored_form() {
        let mut b = Bonds::new();
        b.add(bond(1));
        b.add(Bond { irk: None, ..bond(2) });
        assert_eq!(Bonds::from_bytes(&b.to_bytes()), b);
        assert!(Bonds::from_bytes(&[0xFF; BONDS_LEN]).is_empty(), "erased flash");
        assert!(Bonds::from_bytes(&[]).is_empty());
    }

    #[test]
    fn the_same_phone_is_kept_once_and_the_oldest_goes_first() {
        let mut b = Bonds::new();
        b.add(bond(1));
        // The same phone under a new address (recognised by its key).
        b.add(Bond { addr: [9; 6], ..bond(1) });
        assert_eq!(b.len(), 1);
        for n in 2..=5 {
            b.add(bond(n));
        }
        assert_eq!(b.len(), MAX_BONDS);
        assert!(b.iter().all(|x| x.addr != [9; 6]), "the oldest was replaced");
    }
}
