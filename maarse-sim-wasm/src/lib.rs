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
pub mod terrain;

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

/// `send_file`: a file, voice message (Codec2, `secs` long) or picture
/// (a Pages picture copy: `quality` grey | c4 | c8, or `bytes`).
fn content_file(e: &mut Engine, cmd: &Value) -> Value {
    use engine::transfer::{ItemKind, VOICE_BYTES_PER_S, VOICE_MIME};
    use maarse_hostrules::pages;
    let (from, to) = (cmd["from"].as_u64().unwrap_or(0) as usize, cmd["to"].as_u64().unwrap_or(0) as usize);
    let given = cmd["bytes"].as_u64().map(|b| b as u32);
    let (kind, bytes, mime, name) = match cmd["kind"].as_str().unwrap_or("file") {
        "voice" => {
            let secs = cmd["secs"].as_u64().unwrap_or(10) as u32;
            (ItemKind::Voice, given.unwrap_or(secs * VOICE_BYTES_PER_S), VOICE_MIME, "voice.c2")
        }
        "picture" => {
            let q = match cmd["quality"].as_str().unwrap_or("c4") {
                "grey" => pages::GREY_TARGET,
                "c8" => pages::LORA_PICTURE_MAX,
                _ => pages::COLOUR4_MAX,
            };
            (ItemKind::Picture, given.unwrap_or(q as u32), "image/webp", "picture.webp")
        }
        _ => (ItemKind::File, given.unwrap_or(4096), "application/octet-stream", "file.bin"),
    };
    match e.xf_send_file(from, to, kind, bytes, mime, name, None) {
        Ok(k) => e.item_json(k),
        Err((_, why)) => json!({"state": "refused", "why": why, "bytes": bytes}),
    }
}

/// `terrain {lat0, lon0, dlat, dlon, w, h, data}`: loads the ground height
/// grid (base64 of zlib-deflated bytes, one metre per step, row 0 = lat0)
/// for every engine built after it with `"terrain": true`; no `data` clears it.
fn set_terrain(cmd: &Value) -> Value {
    let Some(b64) = cmd["data"].as_str() else {
        terrain::set(None);
        return json!({"ok": true, "cells": 0});
    };
    let f = |k: &str| cmd[k].as_f64().unwrap_or(0.0);
    let (w, h) = (cmd["w"].as_u64().unwrap_or(0) as usize, cmd["h"].as_u64().unwrap_or(0) as usize);
    let z = match base64(b64).and_then(|raw| miniz_oxide::inflate::decompress_to_vec_zlib(&raw).ok()) {
        Some(z) if z.len() == w * h && w > 1 && h > 1 => z,
        _ => return json!({"error": "bad terrain grid"}),
    };
    terrain::set(Some(terrain::Grid { lat0: f("lat0"), lon0: f("lon0"), dlat: f("dlat"), dlon: f("dlon"), w, h, z }));
    json!({"ok": true, "cells": w * h})
}

fn base64(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes().filter(|&c| c != b'=' && !c.is_ascii_whitespace()) {
        acc = (acc << 6) | val(c)? as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// Runs one command on an engine. Commands (`cmd`):
/// step {ms, max_events?} | run_to_end | summary | state | inspect {node} |
/// events {since, max, node?, ptype?, msg?} | reach {pid} | announce {node} |
/// send {from, to} | inject {event} | msgs {from?, max?} | follow {msg} |
/// alert {index} | components | phone_cmd {node, frame} |
/// phone_events {node} | phone_detach {node} (see phone.rs) |
/// content (see engine/transfer.rs): send_file {from, to, bytes?, kind:
/// file|voice|picture, secs?, quality?} | page {from, host, path, bytes?} |
/// call {from, to, secs} | tunnel {from, to, workload: ssh|ping|bulk, kb?,
/// tun_lora?} | cancel {item, by} | file_limit {from, to} | item {item} |
/// items {from?} | rules
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
        "phone_cmd" if u("node") < e.nodes.len() => {
            let bytes: Vec<u8> = cmd["frame"].as_array().map(|a| a.iter().map(|b| b.as_u64().unwrap_or(0) as u8).collect()).unwrap_or_default();
            e.phone_cmd_on(u("node"), cmd["screen"].as_u64().unwrap_or(0) as u32, &bytes)
        }
        "phone_events" if u("node") < e.nodes.len() => e.phone_events_on(u("node"), cmd["screen"].as_u64().unwrap_or(0) as u32),
        "phone_detach" if u("node") < e.nodes.len() => {
            e.phone_detach_screen(u("node"), cmd["screen"].as_u64().unwrap_or(0) as u32);
            json!({"ok": true})
        }
        "alert" if u("index") < e.alerts.len() => e.alert_summary(u("index"), true),
        "terrain" => set_terrain(cmd),
        "terrain_loss" if u("a") < e.nodes.len() && u("b") < e.nodes.len() => json!({"db": e.terrain_loss(u("a"), u("b"))}),
        "rules" => {
            // The firmware's radio default (maarse_core::companion::RadioConfig::EU_DEFAULT).
            let r = maarse_core::companion::RadioConfig::EU_DEFAULT;
            json!({"name": maarse_hostrules::rules::Rules::STANDARD_NAME, "standard": maarse_hostrules::rules::Rules::STANDARD, "current": e.sc.rules,
                "radio_standard": {"sf": r.sf, "freq_mhz": r.freq_hz as f64 / 1e6, "bw_hz": r.bw_hz, "tx_dbm": r.tx_power_dbm, "duty_permille": r.duty_permille}})
        }
        "send_file" if u("from") < e.nodes.len() && u("to") < e.nodes.len() => content_file(e, cmd),
        "page" if u("from") < e.nodes.len() && u("host") < e.nodes.len() => {
            let bytes = cmd["bytes"].as_u64().map(|b| b as u32);
            match e.xf_page(u("from"), u("host"), cmd["path"].as_str().unwrap_or("index"), bytes) {
                Ok(k) => e.item_json(k),
                Err((_, why)) => json!({"state": "refused", "why": why}),
            }
        }
        "board" if u("from") < e.nodes.len() && u("host") < e.nodes.len() => {
            match e.xf_board(u("from"), u("host"), cmd["path"].as_str().unwrap_or("index"), cmd["text"].as_str().unwrap_or("")) {
                Ok(k) => json!({"item": k}),
                Err((code, why)) => json!({"error": why, "code": code}),
            }
        }
        "call" if u("from") < e.nodes.len() && u("to") < e.nodes.len() => {
            let mode = match cmd["mode"].as_str().unwrap_or("music") { "echo" => 2, "speed" => 3, _ => 1 };
            let k = e.xf_call_mode(u("from"), u("to"), cmd["secs"].as_u64().unwrap_or(10) as u32, mode);
            e.item_json(k)
        }
        "tunnel" if u("from") < e.nodes.len() && u("to") < e.nodes.len() => {
            let k = e.xf_tunnel(u("from"), u("to"), cmd["workload"].as_str().unwrap_or("ping"), cmd["kb"].as_u64().unwrap_or(64) as u32, cmd["tun_lora"].as_bool().unwrap_or(false));
            e.item_json(k)
        }
        "cancel" => json!({"ok": e.xf_cancel(u("item"), u("by"))}),
        "file_limit" if u("from") < e.nodes.len() && u("to") < e.nodes.len() => {
            let l = e.xf_limit(u("from"), u("to"));
            let route = match e.xf_path_kind(u("from"), u("to")) {
                maarse_hostrules::filebox::PathKind::Tcp => "tcp",
                maarse_hostrules::filebox::PathKind::Lora => "lora",
                maarse_hostrules::filebox::PathKind::Unknown => "unknown",
            };
            json!({"route": route, "max_bytes": l.max_bytes, "via_lora": l.via_lora, "mailbox": l.mailbox, "est_airtime_s": l.est_airtime_s})
        }
        "item" => e.item_follow(u("item")),
        "items" => e.items_json(u("from")),
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

    /// An app-only phone (no radio) reaches a radio handheld through its home
    /// host over TCP; it is never heard on the radio.
    #[test]
    fn app_user_without_radio_goes_over_its_host() {
        let mut v: Value = serde_json::from_str(&scenario()).unwrap();
        v["events"] = json!([]);
        v["traffic"] = json!({"direct_per_min": 0, "channel_per_min": 0});
        v["nodes"].as_array_mut().unwrap().push(json!({"kind": "app", "lat": 55.6, "lon": 12.0, "region": "hovedstaden"})); // 22, next to host 19
        v["links"].as_array_mut().unwrap().push(json!({"a": 22, "b": 19, "kind": "tcp"}));
        let mut e = create(&v.to_string()).unwrap();
        assert!(!e.nodes[22].radio && e.nodes[19].radio);
        e.run_until(30_000, usize::MAX);
        let m = e.send_message(22, 8);
        e.run_until(300_000, usize::MAX);
        assert!(e.msgs[m].delivered_ms.is_some(), "{}", e.follow(m));
        let f = e.follow(m);
        assert!(!f["events"].as_array().unwrap().iter().any(|x| x["ev"] == "tx" && x["n"] == 22), "the app sent on the radio");
    }

    /// A route with a pause at waypoint 1 and "stay" at the end.
    #[test]
    fn route_waits_at_waypoints_and_stays() {
        let sc = json!({"seed": 1, "duration_s": 3600, "nodes": [
            {"kind": "handheld", "lat": 56.0, "lon": 9.0, "speed_kmh": 36.0, "start_s": 10,
             "route": [[56.0, 9.0], [56.0, 9.01], [56.0, 9.02]], "pauses_s": [0, 60, 0], "route_end": "stay"}
        ], "traffic": {"direct_per_min": 0, "channel_per_min": 0}});
        let mut e = create(&sc.to_string()).unwrap();
        let x0 = e.nodes[0].x;
        e.run_until(9_000, usize::MAX);
        assert_eq!(e.nodes[0].x, x0, "moved before start_s");
        e.run_until(120_000, usize::MAX); // 620 m at 10 m/s = 62 s, then waits 60 s
        let mid = e.nodes[0].x;
        assert!((mid - x0 - 622.0).abs() < 25.0, "not at waypoint 1: {}", mid - x0);
        e.run_until(1_000_000, usize::MAX);
        let end = e.nodes[0].x;
        e.run_until(1_200_000, usize::MAX);
        assert_eq!(e.nodes[0].x, end, "did not stay at the end");
    }

    /// A `radio` event switches every node: SF12 hears further than SF9, and a
    /// higher frequency hears less far.
    #[test]
    fn radio_event_changes_range() {
        // Two masts 14 km apart at -10 dBm: out of reach at SF9, in reach at SF12.
        let sc = json!({"seed": 1, "tx_dbm": -10.0, "nodes": [
            {"kind": "handheld", "lat": 56.0, "lon": 9.0, "h": 30.0},
            {"kind": "handheld", "lat": 56.0, "lon": 9.2257, "h": 30.0}
        ], "traffic": {"direct_per_min": 0, "channel_per_min": 0}});
        let mut e = create(&sc.to_string()).unwrap();
        let links = |e: &Engine| e.nodes.iter().map(|n| n.nbrs.len()).sum::<usize>();
        let sf9 = links(&e);
        e.inject(&serde_json::from_value(json!({"t_s": 0, "type": "radio", "sf": 12})).unwrap());
        let sf12 = links(&e);
        assert!(sf12 > sf9, "SF12 {sf12} <= SF9 {sf9}");
        e.inject(&serde_json::from_value(json!({"t_s": 0, "type": "radio", "freq_mhz": 2400.0})).unwrap());
        assert!(links(&e) < sf12, "2.4 GHz should hear less far");
    }

    /// The day curve averages 1 over 24 h and is quiet at night.
    #[test]
    fn day_curve_averages_one() {
        use crate::engine::day_factor;
        let avg: f64 = (0..24 * 60).map(|m| day_factor(0.0, m as f64 * 60.0)).sum::<f64>() / (24.0 * 60.0);
        assert!((avg - 1.0).abs() < 0.01, "{avg}");
        assert!(day_factor(3.0, 0.0) < 0.2 && day_factor(19.0, 0.0) > 1.5);
        assert_eq!(day_factor(-1.0, 5000.0), 1.0);
    }

    /// A server keeps its backbone links: when its only link is cut, it
    /// connects to other hosts within a minute (maarse-host --min-peers).
    #[test]
    fn server_reconnects_when_a_link_fails() {
        // Hosts 2 and 3 are seeds (the seed DNS name); 3 is behind NAT, so
        // only 2 can be connected to; host 4 is known to nobody.
        let sc = |dns_down: bool| {
            let mut ev = vec![json!({"t_s": 30, "type": "link", "index": 0, "up": false})];
            if dns_down {
                ev.push(json!({"t_s": 0, "type": "seed_dns", "up": false}));
            }
            json!({"seed": 2, "nodes": [
                {"kind": "server", "lat": 55.7, "lon": 12.5, "min_peers": 2},
                {"kind": "host", "lat": 56.1, "lon": 10.2},
                {"kind": "host", "lat": 57.0, "lon": 9.9, "seed": true},
                {"kind": "host", "lat": 55.4, "lon": 10.4, "seed": true, "nat": true},
                {"kind": "host", "lat": 55.0, "lon": 9.5}
            ], "links": [{"a": 0, "b": 1, "kind": "tcp"}],
               "traffic": {"direct_per_min": 0, "channel_per_min": 0},
               "events": ev})
        };
        let mut e = create(&sc(false).to_string()).unwrap();
        e.run_until(200_000, usize::MAX);
        let s = e.summary();
        assert_eq!(s["backbone_reconnects"], json!(1), "only the public seed: {s}");
        let peers: Vec<usize> = e.nodes[0].links.iter().filter(|&&k| e.link_ok(k)).map(|&k| e.links_peer(k, 0)).collect();
        assert_eq!(peers, vec![2], "not the NAT host, not the unknown one");
        // Seed DNS down and the only past peer gone: nobody to connect to.
        let mut e = create(&sc(true).to_string()).unwrap();
        e.run_until(200_000, usize::MAX);
        let s = e.summary();
        assert_eq!(s["backbone_reconnects"], json!(0), "{s}");
        assert!(s["backbone_no_candidates"].as_u64().unwrap() > 0);
    }

    #[test]
    fn alerts_v2_drop_forged_and_expired_replays() {
        let mut sc: Value = serde_json::from_str(&scenario()).unwrap();
        sc["traffic"] = json!({"direct_per_min": 0, "channel_per_min": 0});
        sc["events"] = json!([
            // A real alert valid 5 min, a forgery, and two replays of the
            // real one: inside its time (only repeats) and after it expired.
            {"t_s": 20, "type": "alert", "from": 18, "alert": "fire", "area": {"circle": [56.0, 9.3, 30000]}, "text": "Fire", "expires_min": 5},
            {"t_s": 60, "type": "alert_forged", "from": 3, "area": {"circle": [56.0, 9.3, 30000]}, "text": "Evacuate"},
            {"t_s": 120, "type": "alert_replay", "from": 4, "index": 0},
            // The Zealand cluster is off while the original goes out, so its
            // routers have not seen it (elsewhere the duplicate filter drops
            // a replay before the alert check does).
            {"t_s": 10, "type": "kill", "nodes": [19, 20, 21]},
            {"t_s": 500, "type": "revive", "nodes": [19, 20, 21]},
            {"t_s": 600, "type": "alert_replay", "from": 21, "index": 0}
        ]);
        let mut e = create(&sc.to_string()).unwrap();
        e.run_until(900_000, usize::MAX);
        let real = e.alert_summary(0, false);
        assert!(real["inside_received"].as_u64().unwrap() > 0, "{real}");
        let forged = e.alert_summary(1, false);
        assert_eq!(forged["inside_received"], json!(0), "{forged}");
        assert!(forged["dropped_forged"].as_u64().unwrap() > 0, "{forged}");
        let early = e.alert_summary(2, false);
        assert_eq!(early["origin"], json!("replay"));
        let late = e.alert_summary(3, false);
        assert_eq!(late["inside_received"], json!(0), "an expired replay is never shown: {late}");
        assert!(late["dropped_expired"].as_u64().unwrap() > 0, "{late}");
        let s = e.summary();
        assert!(s["alerts_forged_dropped"].as_u64().unwrap() > 0 && s["alerts_replay_dropped"].as_u64().unwrap() > 0, "{s}");
    }

    #[test]
    fn guard_bans_a_flooding_host() {
        let mut sc: Value = serde_json::from_str(&scenario()).unwrap();
        sc["traffic"] = json!({"direct_per_min": 0, "channel_per_min": 0});
        sc["events"] = json!([{"t_s": 10, "type": "flood", "node": 18, "per_s": 1000, "duration_s": 30}]);
        let mut e = create(&sc.to_string()).unwrap();
        e.run_until(60_000, usize::MAX);
        let s = e.summary();
        assert!(s["flood_packets"].as_u64().unwrap() > 20_000, "{s}");
        assert_eq!(s["guard_bans"], json!(1), "{s}");
        // After the ban every packet is dropped, and normal limits let ~100/s through before it.
        assert!(s["guard_dropped"].as_u64().unwrap() > 20_000, "{s}");
        // Guard off: nothing dropped (a short flood; every packet is routed).
        sc["features"] = json!({"guard": false});
        sc["events"][0]["duration_s"] = json!(1);
        let mut e = create(&sc.to_string()).unwrap();
        e.run_until(60_000, usize::MAX);
        assert_eq!(e.summary()["guard_dropped"], json!(0));
    }

    #[test]
    fn duty_follows_the_eu_sub_band() {
        let mut sc: scenario::Scenario = serde_json::from_str(&scenario()).unwrap();
        sc.duty_permille = 100;
        assert_eq!(engine::effective_duty(&sc), 100, "869.525 MHz default: 10 %");
        sc.freq_mhz = 868.1;
        assert_eq!(engine::effective_duty(&sc), 10, "868.1 MHz: 1 %");
        sc.freq_mhz = 869.0;
        assert_eq!(engine::effective_duty(&sc), 1, "869.0 MHz: 0.1 %");
        sc.features.duty_bands = false;
        assert_eq!(engine::effective_duty(&sc), 100);
    }
}
