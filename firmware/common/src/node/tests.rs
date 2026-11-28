//! Two radio nodes on the PC: their packets are handed to each other and the
//! clock is moved by hand. Covers what the radios decide about messages, not
//! the radio itself.
//!
//! Run in `firmware/common`:
//! `cargo test --lib --config 'patch.crates-io.lora-phy.path="../vendor/lora-phy"'`
//!
//! There is one test: the channels between the tasks and the clock are global.

extern crate std;

use super::*;
use crate::bus::{BLE_OUT, RADIO_CANCEL};
use embassy_time::MockDriver;
use maarse_core::mail::MAX_DEPOSITED;
use rand_core::OsRng;
use std::boxed::Box;
use std::collections::VecDeque;
use std::vec::Vec;

/// Flash in memory: only what this test needs to be kept.
#[derive(Default)]
struct Store {
    replays: Vec<(u64, [u8; 8])>,
}

impl Settings for Store {
    fn save_mail_keys(&mut self, _keys: &[u8; mailkey::STORED_LEN]) -> bool {
        true
    }
    fn replay_add(&mut self, id: u64, proof: &[u8; 8]) {
        self.replays.push((id, *proof));
    }
    fn save_radio(&mut self, _cfg: &RadioConfig) {}
    fn save_admins(&mut self, _admins: &[Address]) {}
    fn load_admins(&mut self) -> heapless::Vec<Address, 4> {
        heapless::Vec::new()
    }
}

type Radio = FwNode<OsRng, Store>;

struct Air {
    nodes: [&'static mut Radio; 2],
    /// Packets under way: (arrival, to, packet, router id).
    queue: VecDeque<(Instant, usize, Pkt, u64)>,
    /// Events each node has told its app.
    events: [Vec<Event>; 2],
    /// Packets reach the second node.
    b_hears: bool,
}

fn node() -> &'static mut Radio {
    let mem = Box::leak(Box::<Radio>::new_uninit());
    FwNode::new_in(mem, Identity::generate(&mut OsRng), OsRng, Store::default(), RadioConfig::EU_DEFAULT, true)
}

impl Air {
    /// What node `i` has just put out: packets for the other, events for its app.
    fn collect(&mut self, i: usize) {
        while let Ok(id) = RADIO_CANCEL.try_receive() {
            self.queue.retain(|q| q.3 != id || q.1 == i);
        }
        while let Ok(t) = RADIO_TX.try_receive() {
            let at = Instant::now() + Duration::from_millis(u64::from(t.delay_ms) + 300);
            self.queue.push_back((at, 1 - i, t.packet, t.id));
        }
        while let Ok(f) = USB_OUT.try_receive() {
            if let Some(e) = Event::decode(&f[2..]).ok() {
                self.events[i].push(e);
            }
        }
        while BLE_OUT.try_receive().is_ok() {}
    }

    fn run(&mut self, secs: u64) {
        for _ in 0..secs * 10 {
            MockDriver::get().advance(Duration::from_millis(100));
            for i in 0..2 {
                if self.nodes[i].stamping.is_some() {
                    self.nodes[i].stamp_step();
                    self.collect(i);
                }
                if Instant::now() >= self.nodes[i].next_deadline() {
                    self.nodes[i].on_timer();
                    self.collect(i);
                }
            }
            let now = Instant::now();
            while let Some(n) = self.queue.iter().position(|q| q.0 <= now) {
                let Some((_, to, packet, _)) = self.queue.remove(n) else { break };
                if to == 1 && !self.b_hears {
                    continue;
                }
                self.nodes[to].on_radio(packet, 8);
                self.collect(to);
            }
        }
    }

    fn command(&mut self, i: usize, c: Command) {
        let encoded = c.encode();
        let frame = heapless::Vec::from_slice(&encoded[2..]).unwrap();
        self.nodes[i].on_frame(frame);
        self.collect(i);
    }

    fn shown(&self, i: usize, words: &[u8]) -> usize {
        self.events[i].iter().filter(|e| matches!(e, Event::DirectReceived { text: t, .. } if t.ends_with(words))).count()
    }

    fn sent(&self, req_wanted: u16) -> Option<u64> {
        self.events[0].iter().find_map(|e| match e {
            Event::Sent { req, msg_id } if *req == req_wanted => Some(*msg_id),
            _ => None,
        })
    }

    fn acked(&self, id: u64) -> bool {
        self.events[0].iter().any(|e| matches!(e, Event::Acked { msg_id } if *msg_id == id))
    }
}

#[test]
fn two_radios_post_office_mail_receipts_replays_and_forged_packets() {
    MockDriver::get().advance(Duration::from_secs(3600));
    let mut air = Air { nodes: [node(), node()], queue: VecDeque::new(), events: Default::default(), b_hears: true };
    let (a, b) = (air.nodes[0].addr, air.nodes[1].addr);
    air.command(0, Command::SetTime { unix: 1_795_000_000 });
    air.command(1, Command::SetTime { unix: 1_795_000_000 });
    air.run(90);
    assert!(air.nodes[0].known.contains_key(&b) && air.nodes[1].known.contains_key(&a), "no announces");

    // A conversation, so that there is a session and the keys are told.
    air.command(0, Command::SendDirect { req: 1, dest: b, text: text(b"hello") });
    air.run(60);
    air.command(1, Command::SendDirect { req: 2, dest: a, text: text(b"hi") });
    air.run(120);
    assert_eq!((air.shown(1, b"hello"), air.shown(0, b"hi")), (1, 1));
    assert!(air.acked(air.sent(1).unwrap()), "not acknowledged");

    // B is away: the copy A would leave at a post office is delivered later.
    // Short: sealed to B's weekly key. Long: too long for that, static.
    let long = [b'x'; 125];
    for (req, words, weekly) in [(3u16, &b"weekly mail"[..], true), (4, &long[..], false)] {
        air.b_hears = false;
        air.command(0, Command::SendDirect { req, dest: b, text: text(words) });
        let id = air.sent(req).expect("not sent");
        // The router gives up; the copy is still remembered.
        air.run(200);
        let sealed = air.nodes[0].outbox.iter().find(|e| e.0 == id).expect("no copy kept").2.clone();
        if weekly {
            assert_eq!(sealed[0], maarse_crypto::person::INBOX, "not sealed to the weekly key");
        } else {
            assert_eq!(sealed.len(), MAX_DEPOSITED, "the longest form a radio can deposit");
        }
        assert!(!air.acked(id), "acknowledged while B was away");
        air.b_hears = true;
        air.nodes[1].on_data(&sealed, 1);
        air.collect(1);
        air.run(60);
        assert_eq!(air.shown(1, words), 1, "weekly {weekly}: not shown");
        assert!(air.acked(id), "weekly {weekly}: the sender got no receipt");
        assert!(!air.nodes[0].outbox.iter().any(|e| e.0 == id));
        // The same mail again (a second post office, or a recording).
        air.nodes[1].on_data(&sealed, 1);
        air.collect(1);
        air.run(10);
        assert_eq!(air.shown(1, words), 1, "weekly {weekly}: shown twice");
        assert!(!air.nodes[1].settings.replays.is_empty(), "not kept across restarts");
    }

    // The first compact message after an absence may be unreadable (B missed
    // the full messages of the new chain). B then announces, A goes back to
    // the long form, and the conversation goes on.
    air.command(0, Command::SendDirect { req: 30, dest: b, text: text(b"back again") });
    air.run(90);
    air.command(0, Command::SendDirect { req: 31, dest: b, text: text(b"and again") });
    air.run(90);
    assert_eq!(air.shown(1, b"and again"), 1, "no recovery after the absence");

    // A forged packet that claims to be from A does not throw the session away.
    let stranger = Identity::generate(&mut OsRng);
    let b_public = air.nodes[1].identity.public();
    let mut out = [0u8; 255];
    let (n, _) = ratchet::seal(&stranger, &b_public, &mut FixedStore::<2>::new(), true, &mut OsRng, &[content::TEXT, b'z'], &mut out).unwrap();
    ratchet::forge_sender_for_test(&air.nodes[1].identity, &mut out[..n], &a);
    air.events[1].clear();
    air.nodes[1].on_data(&out[..n], 1);
    air.collect(1);
    air.run(60);
    assert_eq!(air.shown(1, UNREADABLE_NOTICE.as_bytes()), 0, "the session was replaced");
    air.command(0, Command::SendDirect { req: 5, dest: b, text: text(b"still here") });
    air.run(90);
    assert_eq!(air.shown(1, b"still here"), 1);
    assert!(air.acked(air.sent(5).unwrap()));
}
