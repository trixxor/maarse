// Regression suite: the key scenarios with fixed seeds and the current best
// settings, compared with a stored baseline. Run before a commit:
//
//   node tools/sim-experiments/regress.mjs            compare with the baseline
//   node tools/sim-experiments/regress.mjs --update   store the results as the baseline
//   EXTRA='{"key":value}' node ...                    extra scenario settings (try a change)
//
// Fails (exit 1) when a scenario delivers more than 5 points less than the
// baseline, or uses more than 15 % more airtime.
import { createRequire } from "module";
import { readFileSync, writeFileSync, existsSync } from "fs";
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
const BASELINE = path.join(here, "regress-baseline.json");
const update = process.argv.includes("--update");
const extra = process.env.EXTRA ? JSON.parse(process.env.EXTRA) : {};

/// The settings everything is measured with (the current best).
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
const auto = sc => { sc.features.auto_relay = true; sc.auto_relay_after_s = 120; return sc; };

function run(sc, secs) {
  const sim = new Sim(sc); sim.step(secs * 1000, 1e9); const s = sim.summary();
  const ml = sim.call({ cmd: "msgs", from: 0, max: 100000 });
  sim.free();
  return { s, msgs: ml.msgs };
}

/// Mean of a scenario over several seeds.
function avg(seeds, f) {
  const acc = { delivered: 0, airtime: 0 };
  for (const seed of seeds) { const r = f(seed); acc.delivered += r.delivered / seeds.length; acc.airtime += r.airtime / seeds.length; }
  return acc;
}

const near = (sc, lat, lon, n, not) => sc.nodes.map((x, i) => [i, x]).filter(([i, x]) => x.kind === "handheld" && !not.includes(i))
  .sort((a, b) => DK.km([a[1].lat, a[1].lon], [lat, lon]) - DK.km([b[1].lat, b[1].lon], [lat, lon])).slice(0, n).map(([i]) => i);

const SCENARIOS = {
  national: () => avg([1, 2, 3], seed => {
    const sc = best(DK.PRESETS.normal.build(window.DK_LAND, window.DK_TOWNS, seed));
    Object.assign(sc.traffic, { friends: 8, friend_share: 0.9, direct_per_min: 12 });
    const { s } = run(sc, 2500);
    return { delivered: s.shown_pct, airtime: s.airtime_s };
  }),
  forest: () => avg([1, 2, 3], seed => {
    const sc = DK.forest({ role: "handheld", seed, hillH: 25 });
    Object.assign(sc.nodes.find(n => n.label === "Relay on the hill"), { kind: "handheld" });
    const { s } = run(auto(best(sc)), 3400);
    return { delivered: s.shown_pct, airtime: s.airtime_s };
  }),
  town: () => avg([1, 2, 3], seed => {
    const { s } = run(auto(best(DK.city({ seed }))), 3400);
    return { delivered: s.shown_pct, airtime: s.airtime_s };
  }),
  car: () => avg([1, 2, 3], seed => {
    const { s } = run(best(DK.PRESETS.e45.build(window.DK_LAND, window.DK_TOWNS, seed)), 4300);
    return { delivered: s.shown_pct, airtime: s.airtime_s };
  }),
  outage: () => avg([6, 7, 8], seed => {
    // The journey Aalborg -> Roenne with the internet down from 1000 s until
    // 30 min after switching on; share of mail written while off that
    // arrives before the internet returns.
    const OFF = 1200, ON = 4200, BACK = 6000;
    const sc = best(DK.generateDenmark({ seed, trunks: "multi" }, window.DK_LAND, window.DK_TOWNS));
    Object.assign(sc.traffic, { direct_per_min: 2, local_share: 0.9 });
    const rh = sc.nodes.findIndex(n => n.kind === "host" && (n.label || "").startsWith("Roenne"));
    const ch = sc.nodes.findIndex(n => n.kind === "host" && (n.label || "").startsWith("Copenhagen"));
    if (rh >= 0 && !sc.nodes[rh].internet) { sc.nodes[rh].internet = true; if (ch >= 0) sc.links.push({ a: rh, b: ch, kind: "tcp", up: true }); }
    const T = near(sc, 57.048, 9.92, 1, [])[0];
    const aal = near(sc, 57.048, 9.92, 3, [T]);
    const cph = near(sc, 55.68, 12.57, 2, [T, ...aal]);
    const brn = near(sc, 55.68, 12.57, 5, [T, ...aal, ...cph]).slice(2);
    brn.forEach((i, k) => Object.assign(sc.nodes[i], { lat: 55.10 + 0.01 * k, lon: 14.70 + 0.02 * k, region: "bornholm" }));
    const friends = [...aal, ...cph, ...brn];
    sc.events.push({ t_s: OFF, type: "kill", node: T }, { t_s: ON - 1, type: "move", node: T, area: { circle: [55.103, 14.705, 0], polygon: [] } }, { t_s: ON, type: "revive", nodes: [T] });
    let k = 0;
    for (let t = OFF + 100; t < ON; t += 120) sc.events.push({ t_s: t, type: "message", from: friends[k++ % friends.length], node: T });
    sc.events.push({ t_s: 1000, type: "internet_down" }, { t_s: BACK, type: "internet_up" });
    sc.events.sort((a, b) => a.t_s - b.t_s);
    const { s, msgs } = run(sc, BACK);
    const mine = msgs.filter(r => r[2] === T && friends.includes(r[1]) && r[3] < ON * 1000);
    return { delivered: 100 * mine.filter(r => r[4] != null).length / Math.max(1, mine.length), airtime: s.airtime_s };
  }),
};

const t0 = Date.now();
const results = {};
for (const [name, f] of Object.entries(SCENARIOS)) {
  const t = Date.now();
  results[name] = f();
  console.log(`${name.padEnd(9)} delivered ${results[name].delivered.toFixed(1)}%  airtime ${results[name].airtime.toFixed(0)} s  (${((Date.now() - t) / 1000).toFixed(0)} s)`);
}
if (update || !existsSync(BASELINE)) {
  writeFileSync(BASELINE, JSON.stringify({ when: new Date().toISOString(), results }, null, 1));
  console.log(`baseline written: ${BASELINE} (${((Date.now() - t0) / 1000).toFixed(0)} s)`);
  process.exit(0);
}
const base = JSON.parse(readFileSync(BASELINE, "utf8")).results;
let bad = 0;
for (const [name, r] of Object.entries(results)) {
  const b = base[name];
  if (!b) continue;
  const dd = r.delivered - b.delivered, da = (r.airtime - b.airtime) / b.airtime * 100;
  const fail = dd < -5 || da > 15;
  if (fail) bad++;
  console.log(`${fail ? "FAIL" : "ok  "} ${name.padEnd(9)} delivered ${dd >= 0 ? "+" : ""}${dd.toFixed(1)} pts, airtime ${da >= 0 ? "+" : ""}${da.toFixed(0)} %`);
}
console.log(bad ? `${bad} scenario(s) got worse` : `all scenarios within tolerance (${((Date.now() - t0) / 1000).toFixed(0)} s)`);
process.exit(bad ? 1 : 0);
