// Experiment: automatic forest mode and edge bridges, in the forest and in a
// town, with the canopy clutter model (ITU-R P.833 per-end saturation; an
// antenna above the canopy sees over it). In the forest the person on the
// hill (25 m, above the trees) is a handheld like the others and is the
// group's only way to the repeater 12 km away. The town has 300 handhelds
// and repeaters in its western half only.
//
// Variants borrow from other systems: OLSR multipoint relays (a node without
// coverage reports how many bridges it hears; "mpr"), Reticulum boundary
// interfaces (a bridge forwards path requests only across the edge;
// "boundary"), and Thread router-eligible end devices (forest mode only where
// few forwarders are around; "density").
// Usage: node tools/sim-experiments/edge_auto.mjs [forest|city|both] [seconds] [seeds]
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
// name, kind ("handheld" with auto, or fixed), settings
const V = [
  ["all clients", "handheld", {}],
  ["all relays", "relay", {}],
  ["A auto, no bridges", "handheld", { auto: true }],
  ["B auto + select", "handheld", { auto: true, edge: true, bridge: "select" }],
  ["C auto + mpr", "handheld", { auto: true, edge: true, bridge: "mpr" }],
  ["D select + boundary", "handheld", { auto: true, edge: true, bridge: "select", boundary: true }],
  ["E mpr + boundary", "handheld", { auto: true, edge: true, bridge: "mpr", boundary: true }],
  ["A + load control", "handheld", { auto: true, load: true }],
  ["B + load control", "handheld", { auto: true, edge: true, bridge: "select", load: true }],
  ["B + load + timeouts", "handheld", { auto: true, edge: true, bridge: "select", load: true, timeouts: true }],
  ["I D + hold 10 min", "handheld", { auto: true, edge: true, bridge: "select", boundary: true, hold: 600 }],
  ["J D + hold 30 min", "handheld", { auto: true, edge: true, bridge: "select", boundary: true, hold: 1800 }],
  ["K named", "handheld", { auto: true, edge: true, bridge: "named" }],
  ["L named + boundary", "handheld", { auto: true, edge: true, bridge: "named", boundary: true }],
  ["M L, p 0.1", "handheld", { auto: true, edge: true, bridge: "named", boundary: true, p: 0.1 }],
  ["N L, p 0.05", "handheld", { auto: true, edge: true, bridge: "named", boundary: true, p: 0.05 }],
  ["O L, p 0.1, hold 5 min", "handheld", { auto: true, edge: true, bridge: "named", boundary: true, p: 0.1, hold: 300 }],
  ["P L, adaptive p", "handheld", { auto: true, edge: true, bridge: "named", boundary: true, p: -1 }],
  ["Q L, adaptive p >= 0.25", "handheld", { auto: true, edge: true, bridge: "named", boundary: true, p: -0.25 }],
  ["R L, adaptive p >= 0.5", "handheld", { auto: true, edge: true, bridge: "named", boundary: true, p: -0.5 }],
];
if (process.argv[5]) V.splice(0, V.length, ...V.filter(v => process.argv[5].split(",").some(p => v[0].startsWith(p))));
const keys = ["delivered_pct", "acked_pct", "cross_msgs", "cross_delivered_pct", "airtime_s", "relays_now", "role_switches"];
const builders = {
  forest: (kind, seed) => {
    const sc = DK.forest({ role: kind, seed, hillH: 25 });
    const h = sc.nodes.find(n => n.label === "Relay on the hill");
    Object.assign(h, { kind, label: "Person on the hill" });
    return sc;
  },
  city: (kind, seed) => DK.city({ role: kind, seed }),
};
for (const place of which === "both" ? ["forest", "city"] : [which]) {
  console.log(`== ${place}`);
  for (const [name, kind, o] of V) {
    const acc = Object.fromEntries(keys.map(x => [x, 0]));
    for (const seed of seeds) {
      const sc = builders[place](kind, seed);
      Object.assign(sc, { trace: false, auto_relay_after_s: 120, announce_s: 43200, auto_bridge: o.bridge || "", bridge_boundary: !!o.boundary, auto_max_fwd: o.maxFwd || 0, auto_drop_fwd: o.dropFwd || 0, bridge_hold_s: o.hold || 0, bridge_p: o.p || 0 });
      sc.features.auto_relay = !!o.auto; sc.features.auto_edge = !!o.edge;
      if (o.load) sc.features.load_control = true;
      if (o.timeouts) { sc.features.sf_slot = true; sc.features.scale_timeouts = true; }
      const sim = new Sim(sc); sim.step(secs * 1000, 1e9); const s = sim.summary(); sim.free();
      for (const x of keys) acc[x] += s[x];
    }
    const r = (x, d = 1) => (acc[x] / seeds.length).toFixed(d);
    console.log(`${name.padEnd(20)} delivered ${r("delivered_pct").padStart(5)}% acked ${r("acked_pct").padStart(5)}% | in/out of no coverage ${r("cross_msgs", 0).padStart(3)} msgs ${r("cross_delivered_pct").padStart(5)}% | airtime ${r("airtime_s", 0).padStart(5)}s | relays ${r("relays_now", 0)} switches ${r("role_switches", 0)}`);
  }
}
