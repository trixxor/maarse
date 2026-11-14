//! Fuzz-mål for DHT-beskeder (bytes fra vilkårlige TCP-peers).
//! Kør med `cargo test -p maarse-host --release fuzz_`. Antal iterationer
//! styres af `MAARSE_FUZZ_ITERS` (se maarse-fuzz).

use crate::dht::{self, Contact, Record, Req, Resp, Store};
use maarse_core::Address;
use maarse_crypto::Identity;
use maarse_fuzz::run;

/// Største DHT-besked, `read_request`/`rpc` accepterer.
const MAX_MSG: usize = 4096;

fn record() -> Record {
    let id = Identity::from_secret_bytes(&[5; 64]);
    Record {
        dest: id.address(),
        gw: Address([9; 16]),
        ep: "10.0.0.1:4000".into(),
        hops: 2,
        age_s: 10,
        announce: id.announce(1, b"x").unwrap().to_vec(),
    }
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
            // Det, serveren gør med en STORE: valider og gem.
            if let Req::Store(r) = req {
                let _ = store.put(r);
                let _ = store.get(&from);
                store.expire();
            }
        }
    });
    // Også store, tilfældige beskeder op til MAX_MSG.
    run("dht-req-stor", &[], MAX_MSG, |b| {
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
