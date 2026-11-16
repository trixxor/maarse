// Experiment: what a reboot does to direct messages, before and after the
// firmware fix of 2026-11-15 (contacts kept in flash; messages from unknown
// senders kept until their announce). Preset "reboot": national day with
// announces every 15 min, 40 % of the handhelds reboot at 3 min.
// Usage: node tools/sim-experiments/reboot.mjs [seconds] [seeds]
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
const secs = +(process.argv[2] || 1200), seeds = (process.argv[3] || "1").split(",").map(Number);
const modes = {
  "before the fix": { persist_known: false, hold_unknown: false },
  "keep unknown only": { persist_known: false, hold_unknown: true },
  "flash contacts only": { persist_known: true, hold_unknown: false },
  "both (firmware now)": { persist_known: true, hold_unknown: true },
};
const keys = ["messages", "delivered_pct", "shown_pct", "false_acks", "dropped_unknown", "held_waiting", "shown_after_hold", "unsent_unknown", "reboots", "tx"];
for (const [name, f] of Object.entries(modes)) {
  const acc = Object.fromEntries(keys.map(k => [k, 0]));
  for (const seed of seeds) {
    const sc = DK.PRESETS.reboot.build(window.DK_LAND, window.DK_TOWNS, seed);
    Object.assign(sc.features, f); sc.trace = false;
    const sim = new Sim(sc); sim.step(secs * 1000, 1e9); const s = sim.summary(); sim.free();
    for (const k of keys) acc[k] += s[k];
  }
  const r = (k, d = 1) => (acc[k] / seeds.length).toFixed(d);
  console.log(`${name.padEnd(21)} msgs ${r("messages", 0)} | delivered ${r("delivered_pct")}% shown ${r("shown_pct")}% | acked-not-shown ${r("false_acks", 1)} dropped ${r("dropped_unknown", 1)} kept ${r("held_waiting", 1)} shown-after-wait ${r("shown_after_hold", 1)} unsent ${r("unsent_unknown", 1)} | reboots ${r("reboots", 0)} tx ${r("tx", 0)}`);
}
