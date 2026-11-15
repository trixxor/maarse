// Experiment: today's routing vs. the DHT backbone vs. the home routing model
// on the national preset at three message rates.
// Usage: node tools/sim-experiments/home_routing.mjs [seconds] [seeds]
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
const secs = +(process.argv[2] || 600), seeds = (process.argv[3] || "1").split(",").map(Number);
const modes = { today: {}, dht: { dht_backbone: true }, home: { home_routing: true }, "home+dht": { home_routing: true, dht_backbone: true } };
for (const rate of [6, 30, 90]) {
  for (const [name, f] of Object.entries(modes)) {
    const acc = { d: 0, a: 0, lat: 0, p90: 0, tx: 0, pr: 0, air: 0, busiest: 0, drops: 0, nh: 0, routed: 0, local: 0, look: 0, unr: 0, fb: {} };
    for (const seed of seeds) {
      const sc = DK.PRESETS.normal.build(window.DK_LAND, window.DK_TOWNS, seed);
      sc.traffic.direct_per_min = rate; Object.assign(sc.features, f); sc.trace = false;
      const sim = new Sim(sc); sim.step(secs * 1000, 1e9); const s = sim.summary(); sim.free();
      acc.d += s.delivered_pct; acc.a += s.acked_pct; acc.lat += s.latency_median_ms; acc.p90 += s.latency_p90_ms; acc.tx += s.tx;
      acc.pr += s.by_type.PathRequest || 0; acc.air += s.airtime_s; acc.busiest += s.busiest_airtime_s; acc.drops += s.duty_drops + s.busy_drops; acc.nh += s.coll_next_hop;
      acc.routed += s.home_routed; acc.local += s.home_local; acc.look += s.home_lookups; acc.unr += s.home_unreachable;
      for (const [k, v] of Object.entries(s.home_fallback || {})) acc.fb[k] = (acc.fb[k] || 0) + v;
    }
    const n = seeds.length, r = (v, d = 1) => (v / n).toFixed(d);
    console.log(`${String(rate).padStart(2)}/min ${name.padEnd(9)} deliv ${r(acc.d)}% ack ${r(acc.a)}% lat ${r(acc.lat / 1000)}s p90 ${r(acc.p90 / 1000)}s | tx ${r(acc.tx, 0)} PR ${r(acc.pr, 0)} airtime ${r(acc.air, 0)}s busiest ${r(acc.busiest, 0)}s drops ${r(acc.drops, 0)} nexthop ${r(acc.nh, 0)}${acc.look ? ` | lookups ${r(acc.look, 0)} local ${r(acc.local, 0)} routed ${r(acc.routed, 0)} unreachable ${r(acc.unr, 0)} fallback ${JSON.stringify(Object.fromEntries(Object.entries(acc.fb).map(([k, v]) => [k, +(v / n).toFixed(0)])))}` : ""}`);
  }
}
