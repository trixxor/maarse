#![recursion_limit = "512"]
//! maarse-sim-wasm: the Denmark simulator engine, compiled to WebAssembly.
//!
//! The engine (`engine.rs`) runs `maarse_core::Router` unchanged in every node.
//! The interface to JavaScript is deliberately tiny and needs no generated glue
//! (no wasm-bindgen): JSON in, JSON out through one shared buffer.
//!
//! * `buf_alloc(len) -> ptr`: make room for `len` input bytes; JS writes there.
//! * `sim_create() -> handle`: build an engine from the scenario JSON in the buffer.
//! * `sim_call(handle) -> len`: run the command JSON in the buffer; the reply
//!   JSON is at `out_ptr()` with length `len`.
//! * `sim_free(handle)`.
//!
//! See `docs/denmark/maarse-sim.js` for the JS side and the command list in [`call`].



pub mod engine;
pub mod scenario;

use engine::Engine;
use serde_json::{Value, json};
use std::cell::RefCell;

thread_local! {
    static SIMS: RefCell<Vec<Option<Engine>>> = const { RefCell::new(Vec::new()) };
    static IN: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static OUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

#[cfg(target_arch = "wasm32")]
fn no_entropy(_: &mut [u8]) -> Result<(), getrandom::Error> {
    Err(getrandom::Error::UNSUPPORTED)
}
#[cfg(target_arch = "wasm32")]
getrandom::register_custom_getrandom!(no_entropy);

/// Builds an engine from scenario JSON.
pub fn create(json: &str) -> Result<Engine, String> {
    let sc: scenario::Scenario = serde_json::from_str(json).map_err(|e| e.to_string())?;
    Ok(Engine::new(sc))
}

/// Runs one command on an engine. Commands (`cmd`):
/// step {ms, max_events?} | run_to_end | summary | state | inspect {node} |
/// events {since, max, node?, ptype?, msg?} | reach {pid} | announce {node} |
/// send {from, to} | inject {event} | msgs {from?, max?} | follow {msg} |
/// alert {index} | components
pub fn call(e: &mut Engine, cmd: &Value) -> Value {
    let u = |k: &str| cmd[k].as_u64().unwrap_or(0) as usize;
    match cmd["cmd"].as_str().unwrap_or("") {
        "step" => {
            let ms = cmd["ms"].as_u64().unwrap_or(1000);
            let max = cmd["max_events"].as_u64().unwrap_or(2_000_000) as usize;
            let target = e.now + ms;
            let done = e.run_until(target, max);
            json!({"t": e.now, "events": done, "behind": e.now < target})
        }
        "run_to_end" => {
            let end = e.sc.duration_s * 1000;
            e.run_until(end, usize::MAX);
            e.summary()
        }
        "summary" => e.summary(),
        "state" => e.node_state(),
        "components" => json!(e.components()),
        "inspect" if u("node") < e.nodes.len() => e.inspect(u("node")),
        "events" => {
            let since = cmd["since"].as_u64().unwrap_or(0);
            let max = cmd["max"].as_u64().unwrap_or(5000) as usize;
            let node = cmd["node"].as_u64().map(|n| (format!("\"n\":{n},"), format!("\"n\":{n}}}"), format!("[{n},"), format!(",{n}]"), format!(",{n},"), format!("[{n}]")));
            let ptype = cmd["ptype"].as_str().map(|p| format!("\"p\":\"{p}\""));
            let msg = cmd["msg"].as_u64().map(|m| (format!("\"msg\":{m},"), format!("\"msg\":{m}}}")));
            let evk = cmd["ev"].as_str().map(|p| format!("\"ev\":\"{p}\""));
            let f = |l: &str| {
                node.as_ref().is_none_or(|(a, b, c, d, e2, f2)| l.contains(a) || l.contains(b) || l.contains(c) || l.contains(d) || l.contains(e2) || l.contains(f2))
                    && ptype.as_ref().is_none_or(|p| l.contains(p))
                    && msg.as_ref().is_none_or(|(a, b)| l.contains(a) || l.contains(b))
                    && evk.as_ref().is_none_or(|p| l.contains(p))
            };
            let (next, lines) = e.events(since, max, &f);
            // Lines are JSON already; splice them into the reply without reparsing.
            let body = lines.join(",");
            serde_json::from_str(&format!("{{\"next\":{next},\"lines\":[{body}]}}")).unwrap_or(json!({"next": next, "lines": []}))
        }
        "reach" => e.reach(cmd["pid"].as_str().unwrap_or("")),
        "announce" if u("node") < e.nodes.len() => json!({"pid": format!("{:016x}", e.announce(u("node")))}),
        "send" if u("from") < e.nodes.len() && u("to") < e.nodes.len() => json!({"msg": e.send_message(u("from"), u("to"))}),
        "inject" => match serde_json::from_value::<scenario::TimedEvent>(cmd["event"].clone()) {
            Ok(ev) => {
                e.inject(&ev);
                json!({"ok": true, "alerts": e.alerts.len()})
            }
            Err(err) => json!({"error": err.to_string()}),
        },
        "msgs" => e.msg_list(u("from"), cmd["max"].as_u64().unwrap_or(500) as usize),
        "follow" => e.follow(u("msg")),
        "alert" if u("index") < e.alerts.len() => e.alert_summary(u("index"), true),
        other => json!({"error": format!("unknown or invalid command: {other}")}),
    }
}

fn set_out(v: &Value) -> u32 {
    let s = serde_json::to_vec(v).unwrap_or_default();
    let n = s.len() as u32;
    OUT.with(|o| *o.borrow_mut() = s);
    n
}

fn take_in() -> String {
    IN.with(|i| String::from_utf8_lossy(&i.borrow()).into_owned())
}

#[unsafe(no_mangle)]
pub extern "C" fn buf_alloc(len: u32) -> *mut u8 {
    IN.with(|i| {
        let mut v = i.borrow_mut();
        v.clear();
        v.resize(len as usize, 0);
        v.as_mut_ptr()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn out_len() -> u32 {
    OUT.with(|o| o.borrow().len() as u32)
}

#[unsafe(no_mangle)]
pub extern "C" fn out_ptr() -> *const u8 {
    OUT.with(|o| o.borrow().as_ptr())
}

/// Returns a handle (>= 1), or 0 on error (the error JSON is in the output buffer).
#[unsafe(no_mangle)]
pub extern "C" fn sim_create() -> u32 {
    match create(&take_in()) {
        Ok(e) => SIMS.with(|s| {
            let mut s = s.borrow_mut();
            s.push(Some(e));
            set_out(&json!({"ok": true}));
            s.len() as u32
        }),
        Err(err) => {
            set_out(&json!({"error": err}));
            0
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn sim_call(h: u32) -> u32 {
    let input = take_in();
    let cmd: Value = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(err) => return set_out(&json!({"error": err.to_string()})),
    };
    let out = SIMS.with(|s| {
        let mut s = s.borrow_mut();
        match s.get_mut((h as usize).wrapping_sub(1)).and_then(|x| x.as_mut()) {
            Some(e) => call(e, &cmd),
            None => json!({"error": "no such simulation"}),
        }
    });
    set_out(&out)
}

#[unsafe(no_mangle)]
pub extern "C" fn sim_free(h: u32) {
    SIMS.with(|s| {
        if let Some(x) = s.borrow_mut().get_mut((h as usize).wrapping_sub(1)) {
            *x = None;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small island: repeaters on a line with handhelds, a host pair linked
    /// over TCP to a second cluster 80 km away.
    fn scenario() -> String {
        let mut nodes = Vec::new();
        for k in 0..6 {
            nodes.push(json!({"kind": "repeater", "lat": 56.0, "lon": 9.0 + k as f64 * 0.15, "region": "midtjylland"}));
        }
        for k in 0..12 {
            nodes.push(json!({"kind": "handheld", "lat": 56.0 + ((k % 3) as f64 - 1.0) * 0.03, "lon": 9.0 + (k / 2) as f64 * 0.15, "region": "midtjylland"}));
        }
        // Second cluster far away (Zealand), only reachable over the backbone.
        nodes.push(json!({"kind": "host", "lat": 56.0, "lon": 9.75, "region": "midtjylland"})); // 18
        nodes.push(json!({"kind": "host", "lat": 55.6, "lon": 12.0, "region": "hovedstaden"})); // 19
        nodes.push(json!({"kind": "repeater", "lat": 55.62, "lon": 12.1, "region": "hovedstaden"})); // 20
        nodes.push(json!({"kind": "handheld", "lat": 55.63, "lon": 12.15, "region": "hovedstaden"})); // 21
        json!({
            "seed": 7, "duration_s": 600,
            "nodes": nodes,
            "links": [{"a": 18, "b": 19, "kind": "tcp"}],
            "traffic": {"direct_per_min": 6, "channel_per_min": 1, "local_share": 1.0, "local_km": 40},
            "events": [
                {"t_s": 60, "type": "announce", "node": 2},
                {"t_s": 120, "type": "alert", "from": 18, "alert": "fire", "area": {"circle": [56.0, 9.3, 15000]}, "text": "Forest fire"},
                {"t_s": 300, "type": "internet_down"}
            ]
        })
        .to_string()
    }

    #[test]
    fn delivers_traces_alerts_and_internet_down() {
        let mut e = create(&scenario()).unwrap();
        // Cross-backbone message while the internet is up.
        e.run_until(30_000, usize::MAX);
        let m = e.send_message(21, 8);
        e.run_until(600_000, usize::MAX);
        let s = e.summary();
        assert!(s["delivered_pct"].as_f64().unwrap() > 0.0, "{s}");
        assert!(s["backbone_packets"].as_u64().unwrap() > 0);
        assert_eq!(s["internet"], json!(false));
        let a = e.alert_summary(0, true);
        assert!(a["inside_received"].as_u64().unwrap() > 0, "{a}");
        // Follow: the cross-backbone message has tx and bb events.
        let f = e.follow(m);
        let evs = f["events"].as_array().unwrap();
        assert!(evs.iter().any(|v| v["ev"] == "tx"), "{f}");
        // Announce reach.
        let pid = format!("{:016x}", e.announces[0].1);
        let r = e.reach(&pid);
        assert!(r["heard"].as_u64().unwrap() > 1, "{r}");
        // After internet down the TCP link is gone.
        let st = e.node_state();
        assert_eq!(st["links_ok"][0], json!(false));
    }

    #[test]
    fn same_seed_same_result() {
        let mut a = create(&scenario()).unwrap();
        let mut b = create(&scenario()).unwrap();
        a.run_until(200_000, usize::MAX);
        b.run_until(200_000, usize::MAX);
        assert_eq!(a.summary()["tx"], b.summary()["tx"]);
    }
}
