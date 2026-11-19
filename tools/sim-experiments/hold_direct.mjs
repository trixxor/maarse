// Proposed firmware rule (Router::hold_direct): a companion node whose phone
// is away does not acknowledge direct messages, so senders keep them (post
// office, carry) instead of the phone losing them. Generated Denmark, 40
// phones on handhelds, each losing its Bluetooth link for 10 minutes at a
// random time; with and without the rule, three seeds.
//   node tools/sim-experiments/hold_direct.mjs
import { createRequire } from "module";
import { readFileSync } from "fs";
import path from "path";
import { fileURLToPath } from "url";
const require = createRequire(import.meta.url);
const dir = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "..", "docs", "denmark");
globalThis.window = globalThis;
for (const f of ["dk-land.js", "dk-towns.js", "demo-sites.js"]) require(path.join(dir, f));
const DK = require(path.join(dir, "scenarios.js"));
const { load } = require(path.join(dir, "maarse-sim.js"));
const { Sim } = await load(readFileSync(path.join(dir, "maarse_sim_wasm.wasm")));

function rand(seed) { let x = seed * 2654435761 >>> 0; return () => ((x = (x * 1664525 + 1013904223) >>> 0) / 2 ** 32); }

console.log("seed  rule  msgs-to-phones  lost  held  seen-by-phone%  median-s  p90-s  deposits  airtime-s");
for (const seed of [3, 4, 5]) {
  for (const hold of [false, true]) {
    const sc = DK.generateDenmark({ seed }, window.DK_LAND, window.DK_TOWNS);
    sc.features.hold_direct = hold;
    sc.duration_s = 3600;
    const R = rand(seed);
    const hh = sc.nodes.map((n, i) => i).filter((i) => sc.nodes[i].kind === "handheld");
    const phones = [];
    while (phones.length < 40) { const i = hh[Math.floor(R() * hh.length)]; if (!phones.includes(i)) phones.push(i); }
    const x = new Sim(sc);
    for (const p of phones) x.call({ cmd: "phone_cmd", node: p, frame: [] });
    const drops = phones.map((p) => [300 + Math.floor(R() * 2400), p]).sort((a, b) => a[0] - b[0]);
    let t = 0;
    for (const [at, p] of drops) {
      x.step((at - t) * 1000, 1e9); t = at;
      x.call({ cmd: "inject", event: { type: "phone_link_drop", node: p, duration_s: 600 } });
      x.call({ cmd: "phone_events", node: p });
    }
    x.step((3600 - t) * 1000, 1e9);
    // Messages to the phones' nodes: how many arrived, and when.
    const all = x.msgs(0, 1e6).msgs;
    const set = new Set(phones);
    const to = all.filter((m) => set.has(m[2]) && m[7] === null);
    const lat = to.filter((m) => m[4] !== null).map((m) => (m[4] - m[3]) / 1000).sort((a, b) => a - b);
    const s = x.summary();
    const st = x.call({ cmd: "state" });
    void st;
    const pct = to.length ? (100 * (lat.length - s.phone_lost_msgs) / to.length) : 0;
    console.log(`${String(seed).padEnd(5)} ${hold ? "on " : "off"}   ${String(to.length).padStart(13)}  ${String(s.phone_lost_msgs).padStart(4)}  ${String(hold ? "yes" : "-").padStart(4)}  ${pct.toFixed(1).padStart(10)}  ${(lat[lat.length >> 1] || 0).toFixed(1).padStart(8)}  ${(lat[Math.floor(lat.length * 0.9)] || 0).toFixed(1).padStart(5)}  ${String(s.mail_deposits).padStart(8)}  ${s.airtime_s.toFixed(0).padStart(9)}`);
    x.free();
  }
}
