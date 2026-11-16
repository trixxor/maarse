// Experiment: do routes kept across a reboot help? Preset "reboot" (40 % of
// the handhelds reboot at 3 min) with the best profile, several seeds.
//   node tools/sim-experiments/persist_routes.mjs [seconds] [seeds]
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
const secs = +(process.argv[2] || 1800), seeds = (process.argv[3] || "1,2,3").split(",").map(Number);
for (const persist of [false, true]) {
  const acc = { shown: 0, air: 0, preq: 0 };
  for (const seed of seeds) {
    const sc = DK.PRESETS.reboot.build(window.DK_LAND, window.DK_TOWNS, seed);
    Object.assign(sc, {
      trace: false, neighbor_info: "hello", contact_capacity: 256, pinned_capacity: 32, contact_policy: "used",
      auto_bridge: "named", bridge_boundary: true, bridge_p: -0.25, mail: "post", mail_bundle: true, path_ring: 4, text_compress: true,
    });
    Object.assign(sc.features, { pin_contacts: true, key_request: true, key_cache: true, host_anchor: true, bordercast: true, mobile_mode: true, auto_edge: true, persist_known: true, hold_unknown: true, persist_routes: persist });
    Object.assign(sc.traffic, { friends: 8, friend_share: 0.9 });
    const sim = new Sim(sc); sim.step(secs * 1000, 1e9); const s = sim.summary(); sim.free();
    acc.shown += s.shown_pct / seeds.length; acc.air += s.airtime_s / seeds.length; acc.preq += (s.by_type.PathRequest || 0) / seeds.length;
  }
  console.log(`persist_routes ${persist}: shown ${acc.shown.toFixed(1)} %  airtime ${acc.air.toFixed(0)} s  path requests ${acc.preq.toFixed(0)}`);
}
