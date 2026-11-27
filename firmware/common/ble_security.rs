// The Bluetooth code (see `maarse_core::blepair`): pairing, and what a client
// may do. The nRF and ESP32 firmware use different controllers behind the same
// host library, so both include this file with `#[path]`.

use embassy_time::{Duration, Instant};
use maarse_core::blepair::{Bond, Bonds, Guard, command_before_own_code, event_before_own_code};
use maarse_core::companion::{Event, error_code, text};
use maarse_fw_common::bus;
use trouble_host::prelude::*;
use trouble_host::{BondInformation, IdentityResolvingKey, LongTermKey};

type Stk<'a, C> = Stack<'a, C, DefaultPacketPool>;

/// The client that sends a new code keeps its pairing if the node changes the
/// code this soon after.
const CHANGER_WINDOW: Duration = Duration::from_secs(10);

fn to_info(b: &Bond) -> Option<BondInformation> {
    let level = match b.level {
        1 => SecurityLevel::Encrypted,
        2 => SecurityLevel::EncryptedAuthenticated,
        _ => return None,
    };
    let address = Address { kind: AddrKind::new(b.addr_kind), addr: BdAddr::new(b.addr) };
    let identity = Identity { addr: address, irk: b.irk.and_then(|k| IdentityResolvingKey::new(u128::from_le_bytes(k))) };
    Some(BondInformation::new(identity, LongTermKey::new(u128::from_le_bytes(b.ltk)), level, true))
}

fn from_info(i: &BondInformation) -> Bond {
    let mut addr = [0u8; 6];
    addr.copy_from_slice(i.identity.addr.addr.raw());
    Bond {
        addr_kind: i.identity.addr.kind.into_inner(),
        addr,
        irk: i.identity.irk.map(|k| k.0.get().to_le_bytes()),
        ltk: i.ltk.0.to_le_bytes(),
        level: if i.security_level == SecurityLevel::EncryptedAuthenticated { 2 } else { 1 },
    }
}

/// The paired phones saved in flash, handed to the Bluetooth host.
pub fn restore<C: Controller>(stack: &Stk<'_, C>, bonds: &Bonds) {
    for info in bonds.iter().filter_map(to_info) {
        let _ = stack.add_bond_information(info);
    }
}

/// The code was changed: nobody who paired with the old one stays paired,
/// except `keep` (the client that changed it). Returns the kept pairing.
pub fn forget_others<C: Controller>(stack: &Stk<'_, C>, keep: Option<Identity>) -> Option<Bond> {
    loop {
        let other = stack.with_bond_information(|b| b.iter().find(|b| keep.is_none_or(|k| b.identity != k)).map(|b| b.identity));
        let Some(identity) = other else { break };
        if stack.remove_bond_information(identity).is_err() {
            break;
        }
    }
    stack.with_bond_information(|b| b.first().map(from_info))
}

/// Failed pairings, the pairing being tried now, and who is changing the code.
#[derive(Default)]
pub struct Security {
    guard: Guard,
    trying: bool,
    changing: Option<Instant>,
    refused: bool,
}

impl Security {
    /// Sets the code new pairings must use. While too many guesses have failed
    /// the passkey is random: nobody can pair.
    pub fn apply<C: Controller>(&self, stack: &Stk<'_, C>) {
        let blocked = self.guard.blocked_ms(Instant::now().as_millis()).is_some();
        stack.set_passkey((!blocked).then(bus::ble_pin));
    }

    /// Call when a client connects: it is asked to pair (or, if already
    /// paired, to encrypt).
    pub fn connected<C: Controller>(&mut self, stack: &Stk<'_, C>, conn: &Connection<'_, DefaultPacketPool>) {
        self.trying = false;
        self.changing = None;
        self.refused = false;
        self.apply(stack);
        let _ = conn.set_bondable(true);
        let _ = conn.request_security();
    }

    fn failed(&mut self) {
        if core::mem::take(&mut self.trying) {
            self.guard.failed(Instant::now().as_millis());
        }
    }

    /// May this frame from a paired client reach the node? With the factory
    /// code only what is needed to choose a new one; the client is told once.
    pub fn accept(&mut self, frame: &[u8]) -> bool {
        let Some(&kind) = frame.first() else { return false };
        if kind == 0x46 {
            self.changing = Some(Instant::now());
        }
        if bus::ble_pin_is_own() || command_before_own_code(kind) {
            return true;
        }
        if !core::mem::replace(&mut self.refused, true) {
            let e = Event::Error { req: 0, code: error_code::BAD_FRAME, text: text(b"Choose your own Bluetooth code first (update the app if it cannot)") };
            let _ = bus::BLE_OUT.try_send(e.encode());
        }
        false
    }

    /// The code has changed. `true`: this client changed it and stays paired
    /// and connected; otherwise it must be disconnected.
    pub fn code_changed<C: Controller>(&mut self, stack: &Stk<'_, C>, conn: &Connection<'_, DefaultPacketPool>) -> bool {
        self.guard.reset();
        self.trying = false;
        let own = self.changing.take().is_some_and(|t| t.elapsed() < CHANGER_WINDOW) && allowed(conn);
        let kept = forget_others(stack, own.then(|| conn.peer_identity()));
        self.apply(stack);
        if own && let Some(bond) = kept {
            // The node has just cleared its list: save this one again.
            bus::BLE_BOND.signal(bond);
        }
        own
    }

    /// The code changed while nobody was connected.
    pub fn code_changed_idle<C: Controller>(&mut self, stack: &Stk<'_, C>) {
        self.guard.reset();
        self.trying = false;
        forget_others(stack, None);
    }

    /// Follows the pairing; call it for every connection event.
    pub fn event<C: Controller>(&mut self, stack: &Stk<'_, C>, event: &GattConnectionEvent<'_, '_, DefaultPacketPool>) {
        match event {
            GattConnectionEvent::PassKeyDisplay(_) => self.trying = true,
            GattConnectionEvent::PairingComplete { security_level, bond } => {
                if *security_level == SecurityLevel::EncryptedAuthenticated {
                    self.trying = false;
                    self.guard.reset();
                    if let Some(info) = bond {
                        bus::BLE_BOND.signal(from_info(info));
                    }
                } else {
                    // Paired without the code ("just works"): worth nothing.
                    self.failed();
                    if let Some(info) = bond {
                        let _ = stack.remove_bond_information(info.identity);
                    }
                }
            }
            // A guess that ends in an error or a lost connection counts too.
            GattConnectionEvent::PairingFailed(_) | GattConnectionEvent::Disconnected { .. } => self.failed(),
            _ => {}
        }
    }
}

/// Has the client on `conn` paired with the code?
pub fn allowed(conn: &Connection<'_, DefaultPacketPool>) -> bool {
    matches!(conn.security_level(), Ok(SecurityLevel::EncryptedAuthenticated))
}

/// May this event frame (with its length prefix) go to a paired client?
pub fn may_send(frame: &[u8]) -> bool {
    bus::ble_pin_is_own() || frame.get(2).is_some_and(|&kind| event_before_own_code(kind))
}
