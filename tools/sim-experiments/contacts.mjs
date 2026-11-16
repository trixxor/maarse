// Experiment: contact table size and pinning. Every node announces once in the
// first 30 min (the table then looks like after a 12 h announce round), then an
// hour of traffic among circles of friends (8 friends nearby, 90 % of messages).
// Usage: node tools/sim-experiments/contacts.mjs [seeds]
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
const variants = [
  ["unlimited (reference)", 0, false],
  ["64, no pinning (today)", 64, false],
  ["256, no pinning", 256, false],
  ["64 + 32 pinned", 64, true],
  ["256 + 32 pinned", 256, true],
];
const keys = ["messages", "delivered_pct", "shown_pct", "unsent_unknown", "held_waiting", "shown_after_hold", "false_acks", "flash_writes_mean"];
for (const [name, cap, pin] of variants) {
  const acc = Object.fromEntries(keys.map(k => [k, 0]));
  for (const seed of seeds) {
    const sc = DK.PRESETS.normal.build(window.DK_LAND, window.DK_TOWNS, seed);
    sc.announce_s = 43200; sc.announce_warmup_s = 1800;
    sc.contact_capacity = cap; sc.features.pin_contacts = pin; sc.pinned_capacity = 32;
    Object.assign(sc.traffic, { friends: 8, friend_share: 0.9, start_s: 1900, direct_per_min: 6 });
    sc.trace = false;
    const sim = new Sim(sc); sim.step(5500 * 1000, 1e9); const s = sim.summary(); sim.free();
    for (const k of keys) acc[k] += s[k];
  }
  const r = (k, d = 1) => (acc[k] / seeds.length).toFixed(d);
  console.log(`${name.padEnd(24)} msgs ${r("messages", 0)} | delivered ${r("delivered_pct")}% shown ${r("shown_pct")}% | not sent ${r("unsent_unknown")} waiting ${r("held_waiting")} shown-after-wait ${r("shown_after_hold")} acked-not-shown ${r("false_acks")} | flash writes per node ${r("flash_writes_mean")}`);
}
