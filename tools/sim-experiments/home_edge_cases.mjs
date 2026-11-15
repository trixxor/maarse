// Experiment: the home routing model in edge cases: a moving car (E45), the
// forest (no gateway), and a destination on an island without internet.
// Usage: node tools/sim-experiments/home_edge_cases.mjs
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
const modes = { today: {}, home: { home_routing: true }, "home+dht": { home_routing: true, dht_backbone: true } };
const line = (label, s) => console.log(`${label.padEnd(26)} deliv ${s.delivered_pct.toFixed(1)}% ack ${s.acked_pct.toFixed(1)}% lat ${(s.latency_median_ms / 1000).toFixed(1)}s | tx ${s.tx} PR ${s.by_type.PathRequest || 0} airtime ${s.airtime_s.toFixed(0)}s | lookups ${s.home_lookups} local ${s.home_local} routed ${s.home_routed} unreachable ${s.home_unreachable} fallback ${JSON.stringify(s.home_fallback)}`);
for (const preset of ["e45", "forest"]) for (const seed of [1, 2, 3]) for (const [name, f] of Object.entries(modes)) {
  const sc = DK.PRESETS[preset].build(window.DK_LAND, window.DK_TOWNS, seed);
  Object.assign(sc.features, f); sc.trace = false;
  const sim = new Sim(sc); sim.step((preset === "e45" ? 3600 : 1800) * 1000, 1e9); line(`${preset} seed ${seed} ${name}`, sim.summary()); sim.free();
}
// Skagen -> Roenne when Roenne's host has no internet (seed 1): no flood for nothing?
for (const [name, f] of Object.entries(modes)) {
  const sc = DK.PRESETS.normal.build(window.DK_LAND, window.DK_TOWNS, 1);
  sc.traffic.direct_per_min = 0; sc.traffic.channel_per_min = 0; Object.assign(sc.features, f);
  const near = (la, lo) => { let b = -1, bd = 1e9; sc.nodes.forEach((n, i) => { if (n.kind !== "handheld") return; const d = DK.km([la, lo], [n.lat, n.lon]); if (d < bd) { bd = d; b = i; } }); return b; };
  const sim = new Sim(sc); sim.step(30000);
  const before = sim.summary().tx;
  sim.send(near(57.72, 10.59), near(55.10, 14.70)); sim.step(180000);
  const s = sim.summary();
  console.log(`Skagen->Roenne ${name.padEnd(9)} delivered ${s.delivered_pct.toFixed(0)}% | transmissions caused ${s.tx - before} | unreachable known ${s.home_unreachable}`);
  sim.send(near(57.72, 10.59), near(55.676, 12.568)); sim.step(120000);
  const s2 = sim.summary();
  console.log(`Skagen->Copenhagen ${name.padEnd(5)} delivered ${s2.delivered_pct > 49 ? "yes" : "no"} | transmissions caused ${s2.tx - s.tx} | latency ${(s2.latency_median_ms / 1000).toFixed(1)}s`);
  sim.free();
}
