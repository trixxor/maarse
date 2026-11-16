// Experiment: a journey from North Jutland to Bornholm. The traveller lives
// in Aalborg, is online at home, switches the phone off at 1200 s, travels,
// and switches it on again in Roenne at 4200 s. Friends in Aalborg,
// Copenhagen and on Bornholm write every 2 minutes all the time. National
// preset with both LoRa trunks (Aarhus - Zealand, Zealand - Bornholm) and all
// 2026-11-16 changes; with and without internet (down from 1000 s).
//
// Mail designs: none for handhelds (today), "named" (the old design: the
// traveller's fixed home host is the post box and delivers when it hears the
// traveller's announce) and "post" (post offices: drop at the sender's
// nearest office, three host copies, MailSync, "mail waiting" in hellos,
// fetch on presence, sync to the previous home office), optionally with
// repeaters as small offices.
// Usage: node tools/sim-experiments/journey.mjs [seeds]
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

const OFF = 1200, ON = 4200, END = 7800;
const near = (sc, lat, lon, n, not) => sc.nodes.map((x, i) => [i, x]).filter(([i, x]) => x.kind === "handheld" && !not.includes(i))
  .sort((a, b) => DK.km([a[1].lat, a[1].lon], [lat, lon]) - DK.km([b[1].lat, b[1].lon], [lat, lon])).slice(0, n).map(([i]) => i);

function build(seed, mail, down) {
  const sc = DK.generateDenmark({ seed, trunks: true }, window.DK_LAND, window.DK_TOWNS);
  Object.assign(sc, { trace: false, neighbor_info: "hello", announce_s: 43200, announce_warmup_s: 600, contact_capacity: 0, mail: mail.replace("+rep", ""), repeater_mail: mail.endsWith("+rep") });
  Object.assign(sc.features, { key_request: true, key_cache: true, host_anchor: true, bordercast: true });
  Object.assign(sc.traffic, { start_s: 700, direct_per_min: 2, local_share: 0.9 });
  // Bornholm's host has internet (the generator gives small towns 70 %).
  // (and a TCP link to the Copenhagen host, which the generator only gives
  // hosts that drew internet).
  const rh = sc.nodes.findIndex(n => n.kind === "host" && (n.label || "").startsWith("Roenne"));
  const ch = sc.nodes.findIndex(n => n.kind === "host" && (n.label || "").startsWith("Copenhagen"));
  if (rh >= 0 && !sc.nodes[rh].internet) {
    sc.nodes[rh].internet = true;
    if (ch >= 0) sc.links.push({ a: rh, b: ch, kind: "tcp", up: true });
  }
  const T = near(sc, 57.048, 9.92, 1, [])[0];
  const aal = near(sc, 57.048, 9.92, 3, [T]);
  const cph = near(sc, 55.68, 12.57, 2, [T, ...aal]);
  // Three friends on Bornholm (moved there; the generator puts few users there).
  const brn = near(sc, 55.68, 12.57, 5, [T, ...aal, ...cph]).slice(2);
  brn.forEach((i, k) => Object.assign(sc.nodes[i], { lat: 55.10 + 0.01 * k, lon: 14.70 + 0.02 * k, region: "bornholm" }));
  const friends = [...aal, ...cph, ...brn];
  sc.events.push({ t_s: OFF, type: "kill", node: T });
  sc.events.push({ t_s: ON - 1, type: "move", node: T, area: { circle: [55.103, 14.705, 0], polygon: [] } });
  sc.events.push({ t_s: ON, type: "revive", nodes: [T] });
  let k = 0;
  for (let t = OFF + 100; t < END - 300; t += 120) sc.events.push({ t_s: t, type: "message", from: friends[k++ % friends.length], node: T });
  if (down) sc.events.push({ t_s: 1000, type: "internet_down" });
  sc.events.sort((a, b) => a.t_s - b.t_s);
  return { sc, T, friends };
}

const VARIANTS = [["no mail (today)", ""], ["named post box (old)", "named"], ["post offices (new)", "post"], ["post offices + repeaters", "post+rep"]];
if (process.argv[3]) {
  // Debug: node journey.mjs <seed> <mail> [down]
  const { sc, T, friends } = build(seeds[0], process.argv[3], process.argv[4] === "down");
  const sim = new Sim(sc); sim.step(END * 1000, 1e9); const s = sim.summary();
  console.log(JSON.stringify({ deposits: s.mail_deposits, stored: s.mail_stored, full: s.mail_full, ops: s.mail_ops, tcp: s.mail_tcp, deliveries: s.mail_deliveries, syncs: s.mail_syncs, airtime: s.airtime_s, trunk: s.trunk_packets, by_type: s.by_type }));
  const ml = sim.call({ cmd: "msgs", from: 0, max: 100000 });
  console.log("to T:", ml.msgs.filter(r => r[2] === T && friends.includes(r[1])).map(r => `${(r[3] / 1000).toFixed(0)}:${r[4] ? "ok@" + (r[4] / 1000).toFixed(0) : "x"}/f${friends.indexOf(r[1])}`).join(" "));
  process.exit(0);
}
for (const down of [false, true]) {
  console.log(`== internet ${down ? "DOWN from 1000 s" : "up"}: travel Aalborg -> Roenne, offline ${OFF}-${ON} s`);
  for (const [name, mail] of VARIANTS) {
    const acc = { off: [0, 0], on: [0, 0], delays: [], ops: 0, air: 0, trunk: 0 };
    for (const seed of seeds) {
      const { sc, T, friends } = build(seed, mail, down);
      const sim = new Sim(sc);
      sim.step(END * 1000, 1e9);
      const s = sim.summary();
      const ml = sim.call({ cmd: "msgs", from: 0, max: 100000 });
      for (const [, from, to, sent, deliv] of ml.msgs) {
        if (to !== T || !friends.includes(from)) continue;
        const ok = deliv != null;
        if (sent < ON * 1000) {
          acc.off[1]++;
          if (ok) { acc.off[0]++; acc.delays.push(Math.max(0, deliv - ON * 1000)); }
        } else if (sent > ON * 1000 + 60_000) {
          acc.on[1]++;
          if (ok) acc.on[0]++;
        }
      }
      acc.ops += s.mail_ops || 0; acc.air += s.airtime_s; acc.trunk += s.trunk_packets || 0;
      sim.free();
    }
    const pct = ([a, n]) => n ? `${(100 * a / n).toFixed(0)}% (${a}/${n})` : "-";
    acc.delays.sort((a, b) => a - b);
    const med = acc.delays.length ? `${(acc.delays[acc.delays.length >> 1] / 1000).toFixed(0)} s` : "-";
    const n = seeds.length;
    console.log(`${name.padEnd(26)} written while offline: ${pct(acc.off).padEnd(15)} median after switch-on ${med.padEnd(7)} | written while on Bornholm: ${pct(acc.on).padEnd(15)} | mail packets ${(acc.ops / n).toFixed(0)} trunk packets ${(acc.trunk / n).toFixed(0)} airtime ${(acc.air / n).toFixed(0)} s`);
  }
}
