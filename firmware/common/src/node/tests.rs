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
    contacts: Vec<[u8; 64]>,
}

impl Settings for Store {
    fn save_mail_keys(&mut self, _keys: &[u8; mailkey::STORED_LEN]) -> bool {
        true
    }
    fn replay_add(&mut self, id: u64, proof: &[u8; 8]) {
        self.replays.push((id, *proof));
    }
    fn known_load(&mut self, f: &mut dyn FnMut(&[u8; 64])) {
        for c in &self.contacts {
            f(c);
        }
    }
    fn known_add(&mut self, id: &[u8; 64]) -> bool {
        self.contacts.push(*id);
        true
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
    node_with(Identity::generate(&mut OsRng), Store::default())
}

fn node_with(identity: Identity, flash: Store) -> &'static mut Radio {
    let mem = Box::leak(Box::<Radio>::new_uninit());
    FwNode::new_in(mem, identity, OsRng, flash, RadioConfig::EU_DEFAULT, true)
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

    // Left at a post office: the receipt counts long after the sealed copy is
    // gone, and only with the recipient's proof.
    air.nodes[0].home_po = Some((0, Some(Address([9; 16]))));
    air.b_hears = false;
    air.command(0, Command::SendDirect { req: 20, dest: b, text: text(b"for the post office") });
    let id = air.sent(20).expect("not sent");
    let sealed = air.nodes[0].outbox.iter().find(|e| e.0 == id).expect("no copy kept").2.clone();
    air.run(70);
    assert!(!air.nodes[0].outbox.iter().any(|e| e.0 == id), "the copy was not handed over");
    // B is back: only the post office delivers now.
    air.b_hears = true;
    air.events[1].clear();
    air.run(300);
    assert_eq!(air.shown(1, b"for the post office"), 0, "sent directly although a post office has it");
    air.b_hears = false;
    air.run(1500);
    let receipt = maarse_core::mail::receipt_id(&b, &sealed);
    assert_eq!(air.nodes[0].receipt_for(receipt, None), None, "a receipt anyone can make");
    assert_eq!(air.nodes[0].receipt_for(receipt, Some(7)), None);
    air.b_hears = true;
    air.nodes[1].on_data(&sealed, 1);
    air.collect(1);
    air.run(60);
    assert!(air.acked(id), "no delivered mark after half an hour");
    assert!(air.nodes[0].posted.is_empty());
    air.nodes[0].home_po = None;

    // Mail a post office sends as ordinary data (too long for a bundle) is
    // acknowledged too, also when it comes again.
    {
        let mut out = [0u8; 255];
        let key = air.nodes[0].mailkeys.key_for_test(&b).expect("A has B's key");
        let mut plain = std::vec![content::TEXT];
        plain.extend_from_slice(&[b'y'; 124]);
        let n = maarse_crypto::person::seal_inbox(&air.nodes[0].identity, &b, &key, &mut OsRng, &plain, &mut out).unwrap();
        assert!(n > maarse_core::mail::MAX_BUNDLED);
        let header = maarse_core::packet::Header::new(PacketType::Data, b).with_path(maarse_core::packet::Path::new());
        let packet = encode(&header, &out[..n]).unwrap();
        let token = maarse_core::router::ack_token(maarse_core::dedup::packet_id(PacketType::Data, &b, &out[..n]));
        for round in 0..3 {
            air.queue.clear();
            air.events[1].clear();
            air.nodes[1].on_radio(packet.clone(), 8);
            air.collect(1);
            let acks: Vec<usize> = air
                .queue
                .iter()
                .filter_map(|q| maarse_core::packet::Packet::decode(&q.2).ok())
                .filter(|p| p.header.ptype == PacketType::Ack && p.header.dest == token)
                .map(|p| p.payload.len())
                .collect();
            assert_eq!(acks, [8], "round {round}");
            air.run(30);
        }
    }

    // A bundle from a post office is answered with B's signature.
    let mut body: heapless::Vec<u8, 300> = heapless::Vec::new();
    assert!(maarse_core::mail::build(&mut body, &b, maarse_core::mail::OP_BUNDLE, &[3, 1, 2, 3]));
    assert!(maarse_core::stamp::append(PacketType::Deposit, &b, &mut body));
    let header = maarse_core::packet::Header::new(PacketType::Deposit, b).with_path(maarse_core::packet::Path::new());
    let packet = encode(&header, &body).unwrap();
    let id = maarse_core::dedup::packet_id(PacketType::Deposit, &b, &body);
    air.queue.clear();
    air.nodes[1].on_radio(packet, 8);
    air.collect(1);
    let b_key = air.nodes[1].identity.public();
    let signed = air.queue.iter().any(|q| {
        maarse_core::packet::Packet::decode(&q.2)
            .is_ok_and(|p| p.header.ptype == PacketType::Ack && p.payload.try_into().is_ok_and(|s| b_key.verify_delivery(id, s)))
    });
    assert!(signed, "the bundle was not signed for");

    // The first message after an absence arrives: nothing was acknowledged
    // meanwhile, so it still has the long form.
    air.command(0, Command::SendDirect { req: 30, dest: b, text: text(b"back again") });
    air.run(90);
    assert_eq!(air.shown(1, b"back again"), 1, "unreadable after the absence");
    air.command(0, Command::SendDirect { req: 31, dest: b, text: text(b"and again") });
    air.run(90);
    assert_eq!(air.shown(1, b"and again"), 1);

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

    restart_scenario(&mut air);
}

/// The second node restarts (it keeps its identity and contacts, not its
/// sessions); the first one goes on writing.
fn restart_scenario(air: &mut Air) {
    for (req, w) in [(50u16, &b"warm one"[..]), (51, b"warm two")] {
        air.command(0, Command::SendDirect { req, dest: air.nodes[1].addr, text: text(w) });
        air.run(30);
        air.command(1, Command::SendDirect { req: req + 5, dest: air.nodes[0].addr, text: text(w) });
        air.run(30);
    }
    let secret = air.nodes[1].identity.to_secret_bytes();
    let flash = core::mem::take(&mut air.nodes[1].settings);
    air.nodes[1] = node_with(Identity::from_secret_bytes(&secret), flash);
    air.queue.clear();
    air.command(1, Command::SetTime { unix: 1_795_010_000 });
    air.run(40);
    let b = air.nodes[1].addr;
    let mut arrived = 0;
    for i in 0..4u16 {
        let words = std::format!("after restart {i}");
        air.events[1].clear();
        air.command(0, Command::SendDirect { req: 60 + i, dest: b, text: text(words.as_bytes()) });
        air.run(75);
        arrived += usize::from(air.shown(1, words.as_bytes()) > 0);
    }
    assert!(arrived >= 3, "only {arrived} of 4 arrived after the restart");
}
