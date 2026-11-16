// Experiment: sleepy handhelds (radio off except a short wake window, mail
// synced on waking). What do they cost in delivery and delay?
//   node tools/sim-experiments/sleepy.mjs [seeds]
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
const seeds = (process.argv[2] || "1,2").split(",").map(Number);
const END = 4200, LAST_MSG = 3000;
const variants = {
  "always on": { sleepy_share: 0 },
  "half sleep 5 min / wake 30 s": { sleepy_share: 0.5, sleep_s: 300, wake_s: 30 },
  "all sleep 5 min / wake 30 s": { sleepy_share: 1, sleep_s: 300, wake_s: 30 },
  "all sleep 60 s / wake 15 s": { sleepy_share: 1, sleep_s: 60, wake_s: 15 },
  "half sleep 5 min, mail first": { sleepy_share: 0.5, sleep_s: 300, wake_s: 30, sleepy_mail_first: true },
  "all sleep 5 min, mail first": { sleepy_share: 1, sleep_s: 300, wake_s: 30, sleepy_mail_first: true },
  "all sleep 60 s, mail first": { sleepy_share: 1, sleep_s: 60, wake_s: 15, sleepy_mail_first: true },
};
for (const [name, v] of Object.entries(variants)) {
  const acc = { got: 0, n: 0, lat: [], air: 0 };
  for (const seed of seeds) {
    const sc = DK.PRESETS.normal.build(window.DK_LAND, window.DK_TOWNS, seed);
    Object.assign(sc, {
      trace: false, neighbor_info: "hello", announce_s: 43200, announce_warmup_s: 600,
      contact_capacity: 256, pinned_capacity: 32, contact_policy: "used",
      auto_bridge: "named", bridge_boundary: true, bridge_p: -0.25,
      mail: "post", mail_bundle: true, path_ring: 4, text_compress: true,
    }, v);
    Object.assign(sc.features, { pin_contacts: true, key_request: true, key_cache: true, host_anchor: true, bordercast: true, mobile_mode: true, auto_edge: true });
    Object.assign(sc.traffic, { friends: 8, friend_share: 0.9, direct_per_min: 4, start_s: 700, stop_s: LAST_MSG });
    const sim = new Sim(sc); sim.step(END * 1000, 1e9); const s = sim.summary();
    const ml = sim.call({ cmd: "msgs", from: 0, max: 100000 }).msgs; sim.free();
    const mine = ml.filter(r => r[3] < LAST_MSG * 1000);
    acc.n += mine.length;
    for (const r of mine) if (r[4] != null) { acc.got++; acc.lat.push((r[4] - r[3]) / 1000); }
    acc.air += s.airtime_s / seeds.length;
  }
  acc.lat.sort((a, b) => a - b);
  const q = p => acc.lat.length ? acc.lat[Math.floor(p * (acc.lat.length - 1))].toFixed(0) : "-";
  console.log(`${name.padEnd(30)} shown ${(100 * acc.got / Math.max(1, acc.n)).toFixed(1)} %  delay median ${q(0.5)} s, 90 % ${q(0.9)} s  airtime ${acc.air.toFixed(0)} s`);
}
