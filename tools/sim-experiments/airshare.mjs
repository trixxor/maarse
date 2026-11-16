// Airtime share per packet type in the regression scenarios (best profile),
// to see where the airtime goes (e.g. how much hellos cost).
//   node tools/sim-experiments/airshare.mjs
//   EXTRA='{"key":value}' node ...   with extra settings
import { createRequire } from "module";
import { readFileSync } from "fs";
import { fileURLToPath } from "url";
import path from "path";
const require = createRequire(import.meta.url);
const here = path.dirname(fileURLToPath(import.meta.url));
const dir = path.join(here, "..", "..", "docs", "denmark");
globalThis.window = globalThis;
require(path.join(dir, "dk-land.js")); require(path.join(dir, "dk-towns.js"));
const DK = require(path.join(dir, "scenarios.js"));
const { load } = require(path.join(dir, "maarse-sim.js"));
const { Sim } = await load(readFileSync(path.join(dir, "maarse_sim_wasm.wasm")));
const extra = process.env.EXTRA ? JSON.parse(process.env.EXTRA) : {};

function best(sc) {
  Object.assign(sc, {
    trace: false, neighbor_info: "hello", announce_s: 43200, announce_warmup_s: 600,
    contact_capacity: 256, pinned_capacity: 32, contact_policy: "used",
    auto_bridge: "named", bridge_boundary: true, bridge_p: -0.25,
    mail: "post", trunk_mesh_channel: true, mail_bundle: true, trunk_summary: true, path_ring: 4, text_compress: true,
  });
  Object.assign(sc.features, { pin_contacts: true, key_request: true, key_cache: true, host_anchor: true, bordercast: true, mobile_mode: true, auto_edge: true });
  sc.traffic.start_s = Math.max(sc.traffic.start_s || 0, 700);
  Object.assign(sc, extra);
  return sc;
}

const cases = {
  national: () => { const sc = best(DK.PRESETS.normal.build(window.DK_LAND, window.DK_TOWNS, 1)); Object.assign(sc.traffic, { friends: 8, friend_share: 0.9, direct_per_min: 12 }); return [sc, 2500]; },
  quiet: () => { const sc = best(DK.PRESETS.normal.build(window.DK_LAND, window.DK_TOWNS, 1)); Object.assign(sc.traffic, { friends: 8, friend_share: 0.9, direct_per_min: 1 }); return [sc, 7200]; },
  town: () => { const sc = best(DK.city({ seed: 1 })); sc.features.auto_relay = true; sc.auto_relay_after_s = 120; return [sc, 3400]; },
};
for (const [name, f] of Object.entries(cases)) {
  const [sc, secs] = f();
  const sim = new Sim(sc); sim.step(secs * 1000, 1e9); const s = sim.summary(); sim.free();
  const tot = Object.values(s.air_by_type).reduce((a, b) => a + b, 0);
  const parts = Object.entries(s.air_by_type).sort((a, b) => b[1] - a[1]).map(([k, v]) => `${k} ${(100 * v / tot).toFixed(1)}%`);
  console.log(`${name.padEnd(9)} shown ${s.shown_pct.toFixed(1)}%  airtime ${(tot / 1000).toFixed(0)} s: ${parts.join(", ")}`);
}
