// Hills in the Denmark simulator: loads the height grid (docs/denmark/dk-dem.*)
// into the real engine and compares radio links with and without terrain.
// Build first: python tools/build_denmark_sim.py (and tools/make_dk_dem.py once)
// Usage: node tools/terrain_test.mjs
import { createRequire } from "module";
import { readFileSync } from "fs";
import { fileURLToPath } from "url";
import path from "path";

const require = createRequire(import.meta.url);
const dir = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "docs", "denmark");
globalThis.window = globalThis;
require(path.join(dir, "dk-land.js"));
require(path.join(dir, "dk-towns.js"));
require(path.join(dir, "demo-sites.js"));
const DK = require(path.join(dir, "scenarios.js"));
const { load } = require(path.join(dir, "maarse-sim.js"));
const { Sim } = await load(readFileSync(path.join(dir, "maarse_sim_wasm.wasm")));

let failed = 0;
const check = (ok, what) => { console.log((ok ? "ok   " : "FAIL ") + what); if (!ok) failed++; };

const meta = JSON.parse(readFileSync(path.join(dir, "dk-dem.json"), "utf8"));
const data = readFileSync(path.join(dir, "dk-dem.bin")).toString("base64");

function pair(a, b, h, terrain) {
  return {
    seed: 1, duration_s: 60, terrain,
    nodes: [{ kind: "relay", lat: a[0], lon: a[1], h, label: "A" }, { kind: "relay", lat: b[0], lon: b[1], h, label: "B" }],
    links: [], events: [], traffic: { direct_per_min: 0, channel_per_min: 0 },
  };
}

const probe = new Sim(pair([56, 9], [56, 9.01], 2, false));
check(probe.call(Object.assign({ cmd: "terrain", data }, meta)).ok, `height grid loads (${meta.w} x ${meta.h} cells)`);

// 1. Two masts 30 km apart with Ejer Bavnehøj-Himmelbjerg ridge between them,
//    and the same distance over flat sea (Kattegat).
const cases = [
  ["across the East Jutland hills (Horsens - Silkeborg area)", [55.86, 9.60], [56.12, 9.86]],
  ["over the Kattegat (flat sea)", [56.40, 11.20], [56.40, 11.68]],
];
for (const [what, a, b] of cases) {
  for (const h of [3, 30]) {
    const s = new Sim(pair(a, b, h, true));
    const db = s.call({ cmd: "terrain_loss", a: 0, b: 1 }).db;
    console.log(`     ${what}, masts ${h} m: extra loss ${db.toFixed(1)} dB`);
    s.free();
  }
}
{
  const hill = new Sim(pair(cases[0][1], cases[0][2], 3, true)).call({ cmd: "terrain_loss", a: 0, b: 1 }).db;
  const sea = new Sim(pair(cases[1][1], cases[1][2], 3, true)).call({ cmd: "terrain_loss", a: 0, b: 1 }).db;
  const tall = new Sim(pair(cases[0][1], cases[0][2], 30, true)).call({ cmd: "terrain_loss", a: 0, b: 1 }).db;
  const off = new Sim(pair(cases[0][1], cases[0][2], 3, false)).call({ cmd: "terrain_loss", a: 0, b: 1 }).db;
  check(hill > sea + 10, "hills cost more than the same distance over sea");
  check(Math.abs(sea) < 1, "flat sea adds nothing to the calibrated model");
  check(tall < hill, "taller masts lose less over the hills");
  check(off === 0, "terrain off: no hill loss");
}

// Antenna heights per kind, and a node's own height before that.
{
  const sc2 = pair([56, 9], [56, 9.05], 0, false);
  sc2.nodes[0].kind = sc2.nodes[1].kind = "repeater"; sc2.nodes[1].h = 8;
  sc2.heights = { repeater: 45 };
  const s = new Sim(sc2);
  check(s.inspect(0).h === 45 && s.inspect(1).h === 8, "per-kind height applies; a node's own height wins");
  s.free();
}

// 2. Whole generated Denmark, with and without hills.
const sc = DK.generateDenmark({ users: 250, repeaterKm: 22, seed: 3 }, window.DK_LAND, window.DK_TOWNS);
sc.duration_s = 300;
const linkSet = (terrain) => {
  const s = new Sim(Object.assign({}, sc, { terrain }));
  const set = new Set();
  for (let i = 0; i < sc.nodes.length; i++) for (const [j] of s.inspect(i).radio_neighbors) if (j > i) set.add(i + "-" + j);
  const sum = s.runToEnd();
  s.free();
  return { set, sum };
};
const flat = linkSet(false), hilly = linkSet(true);
const lost = [...flat.set].filter((k) => !hilly.set.has(k)).length, gained = [...hilly.set].filter((k) => !flat.set.has(k)).length;
console.log(`     Denmark, ${sc.nodes.length} nodes: ${flat.set.size} radio links on flat ground, ${hilly.set.size} with terrain (${lost} lost behind hills, ${gained} new from high ground)`);
console.log(`     delivered: flat ${flat.sum.delivered_pct.toFixed(1)} %, terrain ${hilly.sum.delivered_pct.toFixed(1)} %`);
check(lost > 0, "hills cut some radio links");
check(gained > 0, "high ground opens some new ones");
check(Math.abs(hilly.set.size - flat.set.size) < flat.set.size * 0.5, "the network keeps its overall shape (Denmark is flat)");

if (failed) { console.log(`${failed} check(s) failed`); process.exit(1); }
console.log("OK");
