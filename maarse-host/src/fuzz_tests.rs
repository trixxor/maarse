//! Fuzz targets for DHT messages (bytes from arbitrary TCP peers).
//! Run with `cargo test -p maarse-host --release fuzz_`. The number of iterations
//! is controlled by `MAARSE_FUZZ_ITERS` (see maarse-fuzz).

use crate::dht::{self, Contact, Record, Req, Resp, Store};
use maarse_core::Address;
use maarse_crypto::Identity;
use maarse_fuzz::run;

/// Largest DHT message that `read_request`/`rpc` accept.
const MAX_MSG: usize = 4096;

fn record() -> Record {
    let id = Identity::from_secret_bytes(&[5; 64]);
    Record::signed(
        &id,
        id.address(),
        "10.0.0.1:4000".into(),
        2,
        id.announce(1, b"x").unwrap().to_vec(),
    )
}

#[test]
fn fuzz_dht_decode_req() {
    let me = Address([1; 16]);
    let corpus: Vec<Vec<u8>> = [
        Req::Ping,
        Req::FindNode(Address([3; 16])),
        Req::FindValue(Address([4; 16])),
        Req::Store(record()),
    ]
    .iter()
    .map(|r| dht::encode_req(&me, Some("h:1"), r))
    .collect();
    let mut store = Store::new();
    run("dht-req", &corpus, 600, |b| {
        if let Some((from, ep, req)) = dht::decode_req(b) {
            assert_eq!(dht::decode_req(&dht::encode_req(&from, ep.as_deref(), &req)), Some((from, ep, req.clone())));
            // What the server does with a STORE: validate and store.
            if let Req::Store(r) = req {
                let _ = store.put(r);
                let _ = store.get(&from);
                store.expire();
            }
        }
    });
    // Also large, random messages up to MAX_MSG.
    run("dht-req-large", &[], MAX_MSG, |b| {
        let _ = dht::decode_req(b);
    });
}

#[test]
fn fuzz_dht_decode_resp() {
    let me = Address([2; 16]);
    let corpus: Vec<Vec<u8>> = [
        Resp::Pong,
        Resp::Nodes(vec![
            Contact { id: Address([5; 16]), ep: "x:2".into() },
            Contact { id: Address([6; 16]), ep: "y:3".into() },
        ]),
        Resp::Value(record()),
        Resp::Stored(true),
    ]
    .iter()
    .map(|r| dht::encode_resp(&me, r))
    .collect();
    run("dht-resp", &corpus, MAX_MSG, |b| {
        if let Some((from, resp)) = dht::decode_resp(b) {
            if let Resp::Value(r) = &resp {
                let _ = r.valid();
            }
            let _ = from;
        }
    });
}

/// Serial-link and KISS byte streams: the unstuffer and frame decoders must
/// never panic, every decoded packet must re-encode to a frame that decodes
/// to the same packet, and a valid frame after any garbage is still found.
#[test]
fn fuzz_link_framing() {
    use crate::framing::{Callsign, Framing, Unstuffer, udp_decode, udp_encode};
    let framings = [
        Framing::SerialLink,
        Framing::Kiss { callsign: None },
        Framing::Kiss { callsign: Some(Callsign::parse("N0CALL-3").unwrap()) },
    ];
    let corpus: Vec<Vec<u8>> = framings.iter().map(|f| f.encode(&[0x30, 0xC0, 0xDB, 1, 2])).collect();
    run("link-framing", &corpus, 600, |b| {
        for f in &framings {
            let mut u = Unstuffer::new();
            u.push(b);
            while let Some(body) = u.next_body() {
                if let Some(pkt) = f.decode(&body) {
                    let mut again = Unstuffer::new();
                    again.push(&f.encode(&pkt));
                    assert_eq!(f.decode(&again.next_body().unwrap()), Some(pkt));
                }
            }
            // Resynchronisation: a good frame after the garbage is decoded.
            u.push(&f.encode(b"sync"));
            let mut last = None;
            while let Some(body) = u.next_body() {
                last = f.decode(&body).or(last);
            }
            assert_eq!(last.as_deref(), Some(&b"sync"[..]));
        }
        if let Some((inst, pkt)) = udp_decode(b) {
            assert_eq!(udp_decode(&udp_encode(inst, pkt)), Some((inst, pkt)));
        }
    });
}
