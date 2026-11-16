// Experiment: automatic forest mode (handhelds become relays when no
// repeater is in range) and the relay hop limit, in the forest and in a town
// with patchy coverage.
// Usage: node tools/sim-experiments/forest_mode.mjs [forest|city|both] [seconds] [seeds]
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
const which = process.argv[2] || "both", secs = +(process.argv[3] || 2700), seeds = (process.argv[4] || "1").split(",").map(Number);
const variants = [
  ["clients", "handheld", false, 4],
  ["relays, 4 hops", "relay", false, 4],
  ["relays, 12 hops", "relay", false, 12],
  ["auto 10 min, 4 hops", "handheld", true, 4, 600],
  ["auto 10 min, 12 hops", "handheld", true, 12, 600],
  ["auto 2 min, 4 hops", "handheld", true, 4, 120],
  ["auto 2 min, 12 hops", "handheld", true, 12, 120],
];
const build = { forest: (role, seed) => DK.forest({ role, seed }), city: (role, seed) => DK.city({ role, seed }) };
for (const place of which === "both" ? ["forest", "city"] : [which]) {
  console.log(`== ${place}`);
  for (const [name, role, auto, hops, after] of variants) {
    const k = ["delivered_pct", "acked_pct", "latency_median_ms", "tx", "airtime_s", "busiest_airtime_s", "coll_next_hop", "coll_flood", "relays_now", "role_switches"];
    const acc = Object.fromEntries(k.map(x => [x, 0]));
    let drops = 0;
    for (const seed of seeds) {
      const sc = build[place](role, seed);
      sc.features.auto_relay = auto; sc.relay_hops = hops; sc.trace = false;
      if (after) sc.auto_relay_after_s = after;
      const sim = new Sim(sc); sim.step(secs * 1000, 1e9); const s = sim.summary(); sim.free();
      for (const x of k) acc[x] += s[x];
      drops += s.duty_drops + s.busy_drops;
    }
    const r = (x, d = 1) => (acc[x] / seeds.length).toFixed(d);
    console.log(`${name.padEnd(21)} delivered ${r("delivered_pct")}% acked ${r("acked_pct")}% latency ${(acc.latency_median_ms / seeds.length / 1000).toFixed(1)}s | tx ${r("tx", 0)} airtime ${r("airtime_s", 0)}s busiest ${r("busiest_airtime_s", 0)}s drops ${(drops / seeds.length).toFixed(0)} next-hop lost ${r("coll_next_hop", 0)} | relays at end ${r("relays_now", 0)} switches ${r("role_switches", 0)}`);
  }
}
