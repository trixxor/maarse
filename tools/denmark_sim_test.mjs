// Headless test of the Denmark simulator: loads the real WebAssembly engine
// (maarse-core Router in every node) and checks a small generated scenario.
// Build first: python tools/build_denmark_sim.py
// Usage: node tools/denmark_sim_test.mjs [seed]
import { createRequire } from "module";
import { readFileSync } from "fs";
import { fileURLToPath } from "url";
import path from "path";

const require = createRequire(import.meta.url);
const dir = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "docs", "denmark");
globalThis.window = globalThis;
require(path.join(dir, "dk-land.js"));
require(path.join(dir, "dk-towns.js"));
const DK = require(path.join(dir, "scenarios.js"));
const { load } = require(path.join(dir, "maarse-sim.js"));

const seed = +(process.argv[2] || 3);
let failed = 0;
const check = (ok, what) => { console.log((ok ? "ok   " : "FAIL ") + what); if (!ok) failed++; };

const { Sim } = await load(readFileSync(path.join(dir, "maarse_sim_wasm.wasm")));

// 1. Generated Denmark: internet down at 4 min, a fire alert around Aarhus at 2 min.
const sc = DK.generateDenmark({ users: 250, repeaterKm: 22, seed }, window.DK_LAND, window.DK_TOWNS);
sc.duration_s = 480;
const aarhusHost = sc.nodes.findIndex(n => n.kind === "host" && (n.label || "").startsWith("Aarhus"));
const area = { circle: [56.157, 10.211, 20000] };
sc.events.push({ t_s: 60, type: "announce", node: sc.nodes.findIndex(n => n.kind === "repeater") });
sc.events.push({ t_s: 120, type: "alert", from: aarhusHost, alert: "fire", area, text: "Fire near Aarhus" });
sc.events.push({ t_s: 240, type: "internet_down" });
console.log(`nodes ${sc.nodes.length}, links ${sc.links.length}`);

const t0 = Date.now();
const sim = new Sim(sc);
sim.step(200_000);
const before = sim.state();
const tcpUp = before.links_ok.filter(Boolean).length;
sim.step(280_000);
const s = sim.summary();
const after = sim.state();
console.log(`ran ${s.t / 1000} s simulated in ${Date.now() - t0} ms: ${s.messages} messages, delivered ${s.delivered_pct.toFixed(1)} %, acked ${s.acked_pct.toFixed(1)} %, tx ${s.tx}, collisions ${s.collisions}`);
check(s.delivered_pct > 0, "direct messages are delivered");
check(tcpUp > 0 && after.links_ok.filter(Boolean).length === 0, `internet down takes the TCP links down (${tcpUp} up before, 0 after)`);
check(s.internet === false, "summary reports internet down");

const al = sim.alert(0);
console.log(`alert: ${al.inside_received}/${al.inside_alive} nodes in the area, coverage ${al.coverage_pct.toFixed(0)} %, median ${al.latency_median_ms} ms, outside ${al.outside_received}`);
check(al.inside_received > 0, "the area alert reaches nodes inside its area");
const insideSet = new Set(al.inside_nodes);
check(al.recv.some(([n, , inside]) => inside && insideSet.has(n)), "receivers flagged inside really are inside");

// Follow one delivered message: it must have tx and rx events tagged with it.
const list = sim.msgs(0, 5000).msgs;
const delivered = list.find(m => m[4] !== null);
const f = sim.follow(delivered[0]);
const kinds = new Set(f.events.map(e => e.ev));
console.log(`follow msg ${delivered[0]}: ${f.events.length} events (${[...kinds].join(", ")})`);
check(kinds.has("tx") && kinds.has("rx") && kinds.has("deliver"), "a traced message produces tx, rx and deliver events");
check(f.events.some(e => e.ev === "tx" && e.hops > 0) || f.events.some(e => e.ev === "bb"), "a traced message produces hop events (forwarded or over the backbone)");

// Announce reach.
const ev = sim.events(0, 100, { ev: "announce" }).lines[0];
const reach = sim.reach(ev.pid);
console.log(`announce from ${ev.n}: heard by ${reach.heard}, forwarded by ${reach.forwarded}`);
check(reach.heard > 1, "the announce reach set is not empty");

// Inspector.
const ins = sim.inspect(ev.n);
check(Array.isArray(ins.neighbors) && ins.stats && typeof ins.stats.local_repairs === "number", "node inspector returns neighbours and stats");
sim.free();

// 2. Every preset builds and runs a minute.
for (const [id, p] of Object.entries(DK.PRESETS)) {
  const t1 = Date.now();
  const x = new Sim(p.build(window.DK_LAND, window.DK_TOWNS, seed));
  x.step(60_000);
  const y = x.summary();
  console.log(`preset ${id.padEnd(14)} ${String(y.nodes).padStart(5)} nodes, ${String(y.tx).padStart(6)} tx in 60 s, ${Date.now() - t1} ms`);
  check(y.nodes > 0 && y.t === 60000, `preset ${id} runs`);
  x.free();
}

// 3. Scale: 2,000 nodes for 2 simulated minutes.
const big = DK.generateDenmark({ users: 1700, repeaterKm: 16, seed }, window.DK_LAND, window.DK_TOWNS);
big.trace = false;
const t2 = Date.now();
const b = new Sim(big);
b.step(120_000);
const bs = b.summary();
console.log(`scale: ${bs.nodes} nodes, 120 s simulated in ${Date.now() - t2} ms (${bs.tx} tx)`);
check(bs.nodes >= 1900, "about 2,000 nodes load");
b.free();

if (failed) { console.log(`${failed} check(s) failed`); process.exit(1); }
console.log("OK");
