// Hotspot gateway experiment (docs/maybe/later-features.md, section 2): k phones behind one ESP32 radio.
// The phones are modelled as k nodes at the same spot (they hear each other perfectly, like the node's echo),
// and the airtime of the group is the airtime the one shared radio would have to carry. The duty-cycle budget
// of the real radio is 10 % = 360 s per hour for all phones together.
//   node tools/sim-experiments/hotspot.mjs [seeds] [profile,...] [role,...] [k,...]
// Needs the wasm build: cargo build --release --target wasm32-unknown-unknown -p maarse-sim-wasm
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
const wasm = process.env.WASM || path.join(here, "..", "..", "target", "wasm32-unknown-unknown", "release", "maarse_sim_wasm.wasm");
const { Sim } = await load(readFileSync(wasm));

const NSEEDS = +(process.argv[2] || 3);
const list = (i, d) => (process.argv[i] ? process.argv[i].split(",") : d);
// Messages per phone per minute.
const PROFILES = { light: 0.1, normal: 0.33, heavy: 1, flood: 3 };
const profiles = list(3, ["light", "normal", "heavy"]);
const roles = list(4, ["handheld", "relay"]);
const ks = list(5, ["1", "2", "4", "8"]).map(Number);
const START = 700, END = 3400, WINDOW = END - START;

function build(seed, k, role, perMin) {
  const sc = DK.city({ seed });
  Object.assign(sc, {
    trace: false, neighbor_info: "hello", announce_s: 43200, announce_warmup_s: 600, contact_capacity: 256, contact_policy: "used",
    announce_scope: 3, path_ring: 4, text_compress: true, preamble: 16, dm_overhead: 34,
  });
  Object.assign(sc.traffic, { direct_per_min: 0, channel_per_min: 0 });
  const users = sc.nodes.map((n, i) => [i, n]).filter(([, n]) => n.kind === "handheld").map(([i]) => i);
  // The gateway sits at the middle of the town's users.
  const lat = users.reduce((a, i) => a + sc.nodes[i].lat, 0) / users.length;
  const lon = users.reduce((a, i) => a + sc.nodes[i].lon, 0) / users.length;
  const phones = [];
  for (let j = 0; j < k; j++) {
    sc.nodes.push({ ...JSON.parse(JSON.stringify(sc.nodes[users[0]])), kind: role, lat: lat + j * 1e-5, lon: lon, label: "Phone " + (j + 1), wander_m: 0, route: [] });
    phones.push(sc.nodes.length - 1);
  }
  // Explicit traffic: every phone writes to a random town user; the town users chat among themselves at 0.2 / min each.
  let rnd = seed * 7919 + 17;
  const r = () => (rnd = (rnd * 1103515245 + 12345) & 0x7fffffff) / 0x7fffffff;
  for (const p of phones) {
    for (let t = START + r() * (60 / perMin); t < END; t += (60 / perMin) * (0.7 + 0.6 * r())) {
      sc.events.push({ t_s: Math.round(t), type: "message", from: p, node: users[Math.floor(r() * users.length)] });
    }
  }
  for (const u of users) {
    for (let t = START + r() * 300; t < END; t += 300 * (0.7 + 0.6 * r())) {
      let to = users[Math.floor(r() * users.length)]; if (to === u) to = users[0] === u ? users[1] : users[0];
      sc.events.push({ t_s: Math.round(t), type: "message", from: u, node: to });
    }
  }
  sc.events.sort((a, b) => a.t_s - b.t_s);
  return { sc, phones };
}

const median = a => (a.length ? a.slice().sort((x, y) => x - y)[a.length >> 1] : 0);
for (const profile of profiles) for (const role of roles) for (const k of ks) {
  const acc = { group_s_per_h: 0, per_phone_s_per_h: 0, share_pct: 0, delivered: 0, latency_s: 0, net_s_per_h: 0, collisions: 0 };
  for (let seed = 1; seed <= NSEEDS; seed++) {
    const { sc, phones } = build(seed, k, role, PROFILES[profile]);
    const sim = new Sim(sc);
    sim.step(START * 1000, 1e9);
    const a0 = sim.call({ cmd: "state" }).airtime_ms.slice();
    const t0 = sim.summary().airtime_s;
    sim.step((END - START) * 1000, 1e9);
    const a1 = sim.call({ cmd: "state" }).airtime_ms;
    const s = sim.summary();
    const group = phones.reduce((x, p) => x + (a1[p] - a0[p]) / 1000, 0);
    const mine = sim.call({ cmd: "msgs", from: 0, max: 100000 }).msgs.filter(m => phones.includes(m[1]));
    const got = mine.filter(m => m[4] != null);
    sim.free();
    const f = 3600 / WINDOW / NSEEDS;
    acc.group_s_per_h += group * f;
    acc.per_phone_s_per_h += group * f / k;
    acc.share_pct += group * 3600 / WINDOW / 360 * 100 / NSEEDS;
    acc.delivered += 100 * got.length / Math.max(1, mine.length) / NSEEDS;
    acc.latency_s += median(got.map(m => (m[4] - m[3]) / 1000)) / NSEEDS;
    acc.net_s_per_h += (s.airtime_s - t0) * f;
  }
  const r = (x, d = 1) => Math.round(x * 10 ** d) / 10 ** d;
  console.log("RESULT " + JSON.stringify({ profile, role, k, group_air_s_per_h: r(acc.group_s_per_h), per_phone_s_per_h: r(acc.per_phone_s_per_h),
    share_of_10pct_budget: r(acc.share_pct), delivered_pct: r(acc.delivered), latency_median_s: r(acc.latency_s), network_air_s_per_h: r(acc.net_s_per_h, 0) }));
}
