//! Sensor nodes, the node side of the host: messages from sensors are kept
//! (see `crate::sensors`), commands for a sensor go to it as control commands,
//! and the owner's device list is pushed to the sensors this host manages.

use super::{Node, Pending, err_ev, short};
use crate::ClientId;
use maarse_core::Address;
use maarse_core::companion::{Command, Event, Text, error_code};
use maarse_core::content;
use maarse_core::sensor::op;
use std::sync::atomic::{AtomicI64, Ordering};

/// Seconds between two writes of the sensor history to disk.
const SAVE_EVERY: i64 = 60;
static LAST_SAVE: AtomicI64 = AtomicI64::new(0);

impl Node {
    pub(super) fn sensors_open(&mut self) {
        crate::sensors::STORE.lock().unwrap_or_else(|e| e.into_inner()).open(&self.data_dir);
    }

    /// Writes the history now and then (and when asked to).
    pub(super) fn sensors_tick(&mut self) {
        let now = crate::sensors::unix_now();
        if now - LAST_SAVE.load(Ordering::Relaxed) >= SAVE_EVERY {
            LAST_SAVE.store(now, Ordering::Relaxed);
            crate::sensors::STORE.lock().unwrap_or_else(|e| e.into_inner()).save();
        }
    }

    /// Readings or an alarm from a sensor node.
    pub(super) fn on_sensor(&mut self, from: Address, msg: &[u8]) {
        let kept = crate::sensors::STORE.lock().unwrap_or_else(|e| e.into_inner()).ingest(from, msg, crate::sensors::unix_now());
        if !kept {
            self.log(format!("sensor message from {} not kept (too many sensors, too many readings or not understood)", short(&from)));
            return;
        }
        if let Ok(data) = Text::from_slice(msg) {
            self.broadcast(&Event::Sensor { node: from, op: op::EV_MESSAGE, data });
        }
    }

    /// A reply from a sensor to a control command: shown to the clients as a
    /// sensor event from that node. Returns false if it is not one.
    pub(super) fn on_sensor_reply(&mut self, from: Address, frame: &[u8]) -> bool {
        match Event::decode(frame) {
            Ok(Event::Sensor { op, data, .. }) => {
                self.broadcast(&Event::Sensor { node: from, op, data });
                true
            }
            _ => false,
        }
    }

    /// `Command::Sensor`: for the host itself (owner information for claiming
    /// a sensor over USB) or for a sensor far away (sent as a control command).
    pub(super) fn sensor_command(&mut self, id: ClientId, cmd: Command) -> Option<Command> {
        let Command::Sensor { req, dest, op: o, data } = cmd else { return Some(cmd) };
        if dest.0 != [0u8; 16] && dest != self.addr {
            // The sensor gets the command with an all-zero destination: it is for itself.
            let inner = Command::Sensor { req, dest: Address([0; 16]), op: o, data }.encode();
            let Ok(frame) = Text::from_slice(&inner[2..]) else {
                self.send_client(id, &err_ev(req, error_code::TOO_LARGE, "The command is too long"));
                return None;
            };
            return Some(Command::Remote { req, dest, frame });
        }
        match o {
            op::OWNER_INFO => match self.sensor_owner_info() {
                Some(info) => match Text::from_slice(&info) {
                    Ok(data) => self.send_client(id, &Event::Sensor { node: Address([0; 16]), op: op::EV_OWNER_INFO, data }),
                    Err(_) => self.send_client(id, &err_ev(req, error_code::TOO_LARGE, "The owner information is too long")),
                },
                None => self.send_client(
                    id,
                    &err_ev(req, error_code::BAD_FRAME, "Make an account first (Account and devices): sensors belong to a person"),
                ),
            },
            _ => self.send_client(id, &err_ev(req, error_code::BAD_FRAME, "That command is for a sensor node: choose which one")),
        }
        None
    }

    /// Sends this person's device list to every sensor the host has a setup
    /// for, so a device that was added or removed takes effect on the sensors.
    pub(super) fn sensors_push_list(&mut self, list_bytes: &[u8]) {
        let nodes = crate::sensors::STORE.lock().unwrap_or_else(|e| e.into_inner()).managed();
        let mut plain = vec![content::PERSON, content::person::LIST];
        plain.extend_from_slice(list_bytes);
        for n in nodes {
            let _ = self.send_sealed(n, &plain, Pending::Control);
        }
    }
}
