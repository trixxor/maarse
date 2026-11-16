// Experiment: the two long-distance LoRa trunks (Aarhus - Zealand, Zealand -
// Bornholm) with and without internet. Does the network use TCP when it is
// there and the trunks when it is not, and what do the trunks cost?
// National preset with all of the 2026-11-16 changes on; 30 % of messages to
// random users anywhere in the country, so many cross the water.
// Usage: node tools/sim-experiments/trunks.mjs [seeds]
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
const seeds = (process.argv[2] || "1,2,3").split(",").map(Number);

// Which side of the water a node is on.
const side = n => n.lon > 14.5 ? "B" : (n.lon > 10.95 && n.lat < 56.2) ? "Z" : "W";
const VARIANTS = [
  ["internet up, no trunks", { trunks: false }],
  ["internet up, trunks (backup)", { trunks: true }],
  ["internet up, trunks (always)", { trunks: true, policy: "always" }],
  ["internet down, no trunks", { trunks: false, down: true }],
  ["internet down, trunks (all floods)", { trunks: true, down: true, policy: "always" }],
  ["internet down, trunks (all requests)", { trunks: true, down: true, policy: "backup-all" }],
  ["internet down, trunks (backup)", { trunks: true, down: true }],
];
for (const [name, o] of VARIANTS) {
  const acc = { all: [0, 0], cross: [0, 0], zb: [0, 0], wz: [0, 0], lat: [], trunk: {} , air: 0 };
  for (const seed of seeds) {
    const sc = DK.generateDenmark({ seed, trunks: o.trunks }, window.DK_LAND, window.DK_TOWNS);
    Object.assign(sc, { trace: false, neighbor_info: "hello", announce_s: 43200, announce_warmup_s: 600, contact_capacity: 256, pinned_capacity: 32, contact_policy: "used", trunk_policy: o.policy || "" });
    Object.assign(sc.features, { pin_contacts: true, key_request: true, key_cache: true, host_anchor: true, bordercast: true });
    Object.assign(sc.traffic, { start_s: 700, direct_per_min: 6, local_share: 0.7 });
    if (o.down) sc.events.push({ t_s: 650, type: "internet_down" });
    // More users on Bornholm (the generator gives it only a handful): move
    // 20 handhelds to spots around Roenne and Nexoe.
    const R = (x => () => (x = (x * 1103515245 + 12345) % 2147483648) / 2147483648)(seed * 7919);
    const hh = sc.nodes.map((n, i) => n.kind === "handheld" ? i : -1).filter(i => i >= 0);
    for (let k = 0; k < 20 && k < hh.length; k++) {
      const n = sc.nodes[hh[(k * 37 + seed) % hh.length]];
      const [la, lo] = k % 2 ? [55.10, 14.72] : [55.07, 15.10];
      n.lat = +(la + (R() - 0.5) * 0.06).toFixed(5); n.lon = +(lo + (R() - 0.5) * 0.08).toFixed(5); n.region = "bornholm";
    }
    const sim = new Sim(sc);
    sim.step(3300 * 1000, 1e9);
    const s = sim.summary();
    const ml = sim.call({ cmd: "msgs", from: 0, max: 100000 });
    for (const [, from, to, sent, deliv] of ml.msgs) {
      const a = side(sc.nodes[from]), b = side(sc.nodes[to]);
      const ok = deliv != null ? 1 : 0;
      acc.all[0] += ok; acc.all[1]++;
      if (a !== b) {
        acc.cross[0] += ok; acc.cross[1]++;
        if (ok) acc.lat.push(deliv - sent);
        const pair = [a, b].sort().join("");
        if (pair === "BZ") { acc.zb[0] += ok; acc.zb[1]++; }
        if (pair === "WZ") { acc.wz[0] += ok; acc.wz[1]++; }
      }
    }
    for (const t of s.trunks || []) {
      const e = acc.trunk[t.label] || (acc.trunk[t.label] = { packets: 0, air: 0, dropped: 0 });
      e.packets += t.packets; e.air += t.airtime_s; e.dropped += t.dropped;
    }
    acc.air += s.airtime_s;
    sim.free();
  }
  const pct = ([a, n]) => n ? (100 * a / n).toFixed(1) + "%" : "-";
  acc.lat.sort((x, y) => x - y);
  const med = acc.lat.length ? (acc.lat[acc.lat.length >> 1] / 1000).toFixed(1) + "s" : "-";
  const n = seeds.length;
  const tr = Object.entries(acc.trunk).map(([l, e]) => `${l.replace("LoRa trunk ", "")}: ${(e.packets / n).toFixed(0)} pkts ${(e.air / n).toFixed(0)}s air ${(e.dropped / n).toFixed(0)} lost`).join("; ");
  console.log(`${name.padEnd(32)} all ${pct(acc.all)} | across the water ${pct(acc.cross)} (W-Z ${pct(acc.wz)}, Z-Bornholm ${pct(acc.zb)}) median ${med} | mesh airtime ${(acc.air / n).toFixed(0)}s | ${tr || "no trunks"}`);
}
