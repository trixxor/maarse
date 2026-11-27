// Airtime round 2026-11-26: the frame and policy ideas from
// docs/maybe/airtime-optimisation.md, one at a time on the "today" profile.
//   node tools/sim-experiments/airtime_round.mjs [scenario,...|all] [seeds] [config,...]
// Needs a wasm build with the experiment knobs:
//   cargo build --release --target wasm32-unknown-unknown -p maarse-sim-wasm
// Writes one JSON line per (scenario, config) to stdout (prefix RESULT).
import { createRequire } from "module";
import { readFileSync, writeFileSync } from "fs";
import { fileURLToPath } from "url";
import path from "path";
const require = createRequire(import.meta.url);
const here = path.dirname(fileURLToPath(import.meta.url));
const dir = path.join(here, "..", "..", "docs", "denmark");
globalThis.window = globalThis;
require(path.join(dir, "dk-land.js")); require(path.join(dir, "dk-towns.js"));
const DK = require(path.join(dir, "scenarios.js"));
const { load } = require(path.join(dir, "maarse-sim.js"));
const wasm = process.env.WASM || path.join(here, "..", "..", "target", "wasm32-unknown-unknown", "release", "maarse_sim_wasm.wasm");
const { Sim } = await load(readFileSync(wasm));
const only = process.argv[2] && process.argv[2] !== "all" ? process.argv[2].split(",") : null;
const NSEEDS = +(process.argv[3] || 6);
const cfgOnly = process.argv[4] ? process.argv[4].split(",") : null;

const STEPS = {
  "boards before": sc => {
    Object.assign(sc, {
      trace: false, neighbor_info: "announce", announce_s: 43200, announce_warmup_s: 600, contact_capacity: 64, pinned_capacity: 32,
      contact_policy: "", mail: "", announce_scope: 8, path_ring: 0, text_compress: false, no_queue_dedup: true,
    });
    Object.assign(sc.features, { pin_contacts: false, key_request: false, key_cache: false, host_anchor: false, bordercast: false, mobile_mode: false, auto_edge: false });
  },
  "chain fixes": sc => {
    Object.assign(sc, {
      neighbor_info: "hello", contact_capacity: 256, contact_policy: "used", auto_bridge: "named", bridge_boundary: true, bridge_p: -0.25,
      mail: "post", trunk_mesh_channel: true, mail_bundle: true, trunk_summary: true,
    });
    Object.assign(sc.features, { pin_contacts: true, key_request: true, key_cache: true, host_anchor: true, bordercast: true, mobile_mode: true, auto_edge: true });
  },
  "+ queue fix": sc => { sc.no_queue_dedup = false; },
  "+ ring search": sc => { sc.path_ring = 4; },
  "+ compression": sc => { sc.text_compress = true; },
  "+ announce 3": sc => { sc.announce_scope = 3; },
};
const NAMES = Object.keys(STEPS);

const near = (sc, lat, lon, n, not) => sc.nodes.map((x, i) => [i, x]).filter(([i, x]) => x.kind === "handheld" && !not.includes(i))
  .sort((a, b) => DK.km([a[1].lat, a[1].lon], [lat, lon]) - DK.km([b[1].lat, b[1].lon], [lat, lon])).slice(0, n).map(([i]) => i);
const auto = sc => { sc.features.auto_relay = true; sc.auto_relay_after_s = 120; return sc; };

// Each scenario: build(seed) -> [scenario, seconds, measure(summary, msgs)].
const plain = s => ({ delivered: s.shown_pct, airtime: s.airtime_s, latency: (s.latency_median_ms || 0) / 1000 });
const SCEN = {
  national: { seeds: [1, 2, 3, 4, 5].slice(0, Math.max(2, NSEEDS - 1)), build: seed => {
    const sc = DK.PRESETS.normal.build(window.DK_LAND, window.DK_TOWNS, seed);
    Object.assign(sc.traffic, { friends: 8, friend_share: 0.9, direct_per_min: 12, start_s: 700 });
    return [sc, 2500, plain];
  } },
  forest: { seeds: [1, 2, 3, 4, 5, 6].slice(0, NSEEDS), build: seed => {
    const sc = DK.forest({ role: "handheld", seed, hillH: 25 });
    Object.assign(sc.nodes.find(n => n.label === "Relay on the hill"), { kind: "handheld" });
    sc.traffic.start_s = Math.max(sc.traffic.start_s || 0, 700);
    return [auto(sc), 3400, plain];
  } },
  town: { seeds: [1, 2, 3, 4, 5, 6].slice(0, NSEEDS), build: seed => {
    const sc = DK.city({ seed }); sc.traffic.start_s = Math.max(sc.traffic.start_s || 0, 700);
    return [auto(sc), 3400, plain];
  } },
  car: { seeds: [1, 2, 3, 4, 5, 6, 7, 8].slice(0, NSEEDS + 2), build: seed => {
    const sc = DK.PRESETS.e45.build(window.DK_LAND, window.DK_TOWNS, seed); sc.traffic.start_s = 700;
    return [sc, 4300, plain];
  } },
  outage: { seeds: [6, 7, 8, 9, 10, 11].slice(0, NSEEDS), build: seed => {
    // Aalborg -> Roenne with the internet down; share of the mail written
    // while the traveller was off that arrives before the internet returns.
    const OFF = 1200, ON = 4200, BACK = 6000;
    const sc = DK.generateDenmark({ seed, trunks: "multi" }, window.DK_LAND, window.DK_TOWNS);
    Object.assign(sc.traffic, { direct_per_min: 2, local_share: 0.9, start_s: 700 });
    const rh = sc.nodes.findIndex(n => n.kind === "host" && (n.label || "").startsWith("Roenne"));
    const ch = sc.nodes.findIndex(n => n.kind === "host" && (n.label || "").startsWith("Copenhagen"));
    if (rh >= 0 && !sc.nodes[rh].internet) { sc.nodes[rh].internet = true; if (ch >= 0) sc.links.push({ a: rh, b: ch, kind: "tcp", up: true }); }
    const T = near(sc, 57.048, 9.92, 1, [])[0];
    const aal = near(sc, 57.048, 9.92, 3, [T]);
    const cph = near(sc, 55.68, 12.57, 2, [T, ...aal]);
    const brn = near(sc, 55.68, 12.57, 5, [T, ...aal, ...cph]).slice(2);
    brn.forEach((i, k) => Object.assign(sc.nodes[i], { lat: 55.10 + 0.01 * k, lon: 14.70 + 0.02 * k, region: "bornholm" }));
    const friends = [...aal, ...cph, ...brn];
    sc.events.push({ t_s: OFF, type: "kill", node: T }, { t_s: ON - 1, type: "move", node: T, area: { circle: [55.103, 14.705, 0], polygon: [] } }, { t_s: ON, type: "revive", nodes: [T] });
    let k = 0;
    for (let t = OFF + 100; t < ON; t += 120) sc.events.push({ t_s: t, type: "message", from: friends[k++ % friends.length], node: T });
    sc.events.push({ t_s: 1000, type: "internet_down" }, { t_s: BACK, type: "internet_up" });
    sc.events.sort((a, b) => a.t_s - b.t_s);
    return [sc, BACK, (s, msgs) => {
      const mine = msgs.filter(r => r[2] === T && friends.includes(r[1]) && r[3] < ON * 1000);
      const got = mine.filter(r => r[4] != null);
      const lat = got.map(r => (r[4] - ON * 1000) / 1000).sort((a, b) => a - b);
      return { delivered: 100 * got.length / Math.max(1, mine.length), airtime: s.airtime_s, latency: lat.length ? lat[lat.length >> 1] : 0 };
    }];
  } },
};


const ALL = Object.keys(STEPS);
const TODAY = sc => { for (const n of ALL) STEPS[n](sc); if (process.env.TCPONLY) sc.mail_tcp_only = true; };
// "honest" = what the boards really send: preamble 16, a ratchet message is 69 B + type + 40 B text.
const HONEST = sc => Object.assign(sc, { preamble: 16, dm_overhead: 69 });
const CONFIGS = {
  "legacy":      sc => {},
  "honest":      sc => HONEST(sc),
  "legacy_tcponly": sc => { sc.mail_tcp_only = true; },
  "preamble8":   sc => { HONEST(sc); sc.preamble = 8; },
  "lean_header": sc => { HONEST(sc); sc.lean_header = true; },
  "session16":   sc => { HONEST(sc); sc.dm_overhead = 19; },
  "scope2":      sc => { HONEST(sc); sc.announce_scope = 2; },
  "warmup1800":  sc => { HONEST(sc); sc.announce_warmup_s = 1800; },
  "req_jit1s":   sc => { HONEST(sc); sc.request_jitter_ms = 1000; },
  "req_jit3s":   sc => { HONEST(sc); sc.request_jitter_ms = 3000; },
  "req_fwd80":   sc => { HONEST(sc); sc.request_fwd_permille = 800; },
  "req_fwd60":   sc => { HONEST(sc); sc.request_fwd_permille = 600; },
  "retry90":     sc => { HONEST(sc); sc.path_retry_s = 90; },
  "retry20":     sc => { HONEST(sc); sc.path_retry_s = 20; },
  "tcpmail":     sc => { HONEST(sc); sc.mail_tcp_only = true; },
  "proxy":       sc => { HONEST(sc); sc.proxy_reply = true; },
  // Built: the compact session frame measured in crypto tests (average overhead
  // 29.7 B mixed chat, 33.6 B strictly alternating; the old frame is 69 B).
  "compact":     sc => { HONEST(sc); sc.dm_overhead = 34; },
  // Router defaults of 2026-11-26 (80 % request forwarding where a node has 3+ neighbours, 20 s retry) against the old ones.
  "c_old":       sc => { HONEST(sc); sc.dm_overhead = 34; sc.request_fwd_permille = 1000; sc.path_retry_s = 45; },
  "c_new":       sc => { HONEST(sc); sc.dm_overhead = 34; },
  "c_req":       sc => { HONEST(sc); sc.dm_overhead = 34; sc.request_fwd_permille = 800; sc.path_retry_s = 20; },
  "c_req80":     sc => { HONEST(sc); sc.dm_overhead = 34; sc.request_fwd_permille = 800; },
  "c_retry20":   sc => { HONEST(sc); sc.dm_overhead = 34; sc.path_retry_s = 20; },
  "c_pre8":      sc => { HONEST(sc); sc.dm_overhead = 34; sc.preamble = 8; },
  "c_pre8_req":  sc => { HONEST(sc); sc.dm_overhead = 34; sc.preamble = 8; sc.request_fwd_permille = 800; sc.path_retry_s = 20; },
  "c_pre8_req_lean": sc => { HONEST(sc); sc.dm_overhead = 34; sc.preamble = 8; sc.request_fwd_permille = 800; sc.path_retry_s = 20; sc.lean_header = true; },
  "secure_all":  sc => { HONEST(sc); sc.preamble = 8; sc.lean_header = true; sc.dm_overhead = 19; },
};
for (const [name, def] of Object.entries(SCEN)) {
  if (only && !only.includes(name)) continue;
  for (const [cname, cfg] of Object.entries(CONFIGS)) {
    if (cfgOnly && !cfgOnly.includes(cname)) continue;
    const acc = { delivered: 0, airtime: 0, latency: 0 };
    const t0 = Date.now();
    for (const seed of def.seeds) {
      const [sc, secs, measure] = def.build(seed);
      TODAY(sc); cfg(sc);
      const sim = new Sim(sc); sim.step(secs * 1000, 1e9); const s = sim.summary();
      const msgs = sim.call({ cmd: "msgs", from: 0, max: 100000 }).msgs; sim.free();
      const r = measure(s, msgs);
      acc.by = acc.by || {};
      for (const [k, v] of Object.entries(s.air_by_type || {})) acc.by[k] = (acc.by[k] || 0) + v / 1000 / def.seeds.length;
      for (const key of ["delivered", "airtime", "latency"]) acc[key] += r[key] / def.seeds.length;
    }
    console.log("RESULT " + JSON.stringify({ scenario: name, config: cname, seeds: def.seeds.length, ...acc, secs: (Date.now() - t0) / 1000 }));
  }
}
