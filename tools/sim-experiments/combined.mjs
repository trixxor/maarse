// Experiment: today's setup against the proposed fixes that the simulator
// can model, national preset. Announces every 12 h (all nodes once in the
// first 30 min), then an hour of traffic among circles of friends (8 nearby,
// 90 % of messages). Home routing + DHT stands in for "bordercast in the
// host + hosts as anchors" (an upper bound: the full model).
// Usage: node tools/sim-experiments/combined.mjs [seeds] [msgs/min,...]
import { createRequire } from "module";
import { readFileSync } from "fs";
import { fileURLToPath } from "url";
import path from "path";
const require = createRequire(import.meta.url);
const dir = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "..", "docs", "denmark");
globalThis.window = globalThis;
require(path.join(dir, "dk-land.js")); require(path.join(dir, "dk-towns.js"));
const DK = require(path.join(dir, "scenarios.js"));
const { load } = require(path.join(dir, "maarse-sim.js"));
const { Sim } = await load(readFileSync(path.join(dir, "maarse_sim_wasm.wasm")));
const seeds = (process.argv[2] || "1").split(",").map(Number);
const loads = (process.argv[3] || "6,30").split(",").map(Number);
const V = [
  ["today (64 contacts, flooding backbone)", { cap: 64 }],
  ["+ pinned contacts", { cap: 64, pin: true }],
  ["+ pinned + home routing", { cap: 64, pin: true, home: true }],
  ["+ pinned + home routing + DHT", { cap: 64, pin: true, home: true, dht: true }],
  ["reference: unlimited contacts, today's routing", { cap: 0 }],
];
const keys = ["messages", "delivered_pct", "shown_pct", "acked_pct", "airtime_s", "latency_median_ms"];
for (const per of loads) {
  console.log(`== ${per} messages/min`);
  for (const [name, o] of V) {
    const acc = Object.fromEntries(keys.map(k => [k, 0]));
    for (const seed of seeds) {
      const sc = DK.PRESETS.normal.build(window.DK_LAND, window.DK_TOWNS, seed);
      Object.assign(sc, { announce_s: 43200, announce_warmup_s: 1800, contact_capacity: o.cap, pinned_capacity: 32, trace: false });
      sc.features.pin_contacts = !!o.pin; sc.features.home_routing = !!o.home; sc.features.dht_backbone = !!o.dht;
      Object.assign(sc.traffic, { friends: 8, friend_share: 0.9, start_s: 1900, direct_per_min: per });
      const sim = new Sim(sc); sim.step(5500 * 1000, 1e9); const s = sim.summary(); sim.free();
      for (const k of keys) acc[k] += s[k];
    }
    const r = (k, d = 1) => (acc[k] / seeds.length).toFixed(d);
    console.log(`${name.padEnd(48)} delivered ${r("delivered_pct").padStart(5)}% shown ${r("shown_pct").padStart(5)}% acked ${r("acked_pct").padStart(5)}% | airtime ${r("airtime_s", 0).padStart(6)}s | latency ${(acc.latency_median_ms / seeds.length / 1000).toFixed(1)}s`);
  }
}
