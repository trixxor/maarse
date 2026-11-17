// Experiment: crisis (docs/maybe/crisis-carry.md). The national preset is
// hit at t = 0: no internet anywhere, everything (repeaters, masts, hosts)
// within 25 km of Aarhus, Odense, Copenhagen and Aalborg destroyed, and a
// third of the remaining repeaters gone at random. Handhelds are in role
// "auto" (relay after 2 minutes without a repeater). TRAVELLERS handhelds
// drive back and forth between their town and another town 40-120 km away at
// 70 km/h; the rest stay put. People write direct messages to each other
// (half to someone within 25 km) from 10 minutes to 6 hours; measured until
// 8 hours.
//
// Variants: no carry; carry as the firmware has it (4 copies for relays,
// 30 min, 8 in RAM); + long life (6 h) and room (64, flash); + 16 copies;
// + post offices hand their mail to carriers; both; smart carry (copies only
// to moving neighbours, a moving carrier routes on arrival, the sender
// re-routes only when it regains a repeater) alone and with the others.
// Mobile mode is on (travellers say they move).
// Usage: node tools/sim-experiments/crisis.mjs [seeds]   (env TRAVELLERS, HOURS)
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
const TRAVELLERS = Number(process.env.TRAVELLERS || 30);
const HOURS = Number(process.env.HOURS || 8);
const END = HOURS * 3600, SEND_END = Math.min(6 * 3600, END - 1800), START = 600;
const HIT = [["Aarhus", 56.157, 10.211], ["Odense", 55.396, 10.388], ["Copenhagen", 55.676, 12.568], ["Aalborg", 57.048, 9.92]];

function build(seed, v) {
  const sc = DK.generateDenmark({ seed, trunks: true }, window.DK_LAND, window.DK_TOWNS);
  Object.assign(sc, { trace: false, neighbor_info: "hello", announce_s: 43200, announce_warmup_s: 600, contact_capacity: 0, mail: "post", mail_bundle: true, trunk_summary: true, auto_relay_after_s: 120, duration_s: END });
  Object.assign(sc.features, { key_request: true, key_cache: true, host_anchor: true, bordercast: true, auto_relay: true, auto_edge: true, mobile_mode: true, carry: !!v.carry });
  sc.carry_smart = !!v.smart;
  // Traffic from the generator off; our own messages below.
  Object.assign(sc.traffic, { direct_per_min: 0, channel_per_min: 0 });
  if (v.ttl) sc.carry_ttl_s = v.ttl;
  if (v.max) sc.carry_max = v.max;
  if (v.office) { sc.office_carry = true; sc.office_carry_tokens = v.tokens || 4; }
  const users = sc.nodes.map((n, i) => [i, n]).filter(([, n]) => n.kind === "handheld").map(([i]) => i);
  for (const i of users) sc.nodes[i].carry = v.tokens || 4;
  // Travellers: deterministic per seed.
  let r = seed * 9301 + 49297;
  const rnd = () => ((r = (r * 9301 + 49297) % 233280) / 233280);
  const towns = window.DK_TOWNS.filter(t => t[3] >= 5000);
  const trav = [];
  for (let k = 0; k < TRAVELLERS && k < users.length; k++) {
    const i = users[Math.floor(rnd() * users.length)];
    if (trav.includes(i)) continue;
    const n = sc.nodes[i];
    const far = towns.filter(t => { const d = DK.km([n.lat, n.lon], [t[1], t[2]]); return d > 40 && d < 120; });
    if (!far.length) continue;
    const t = far[Math.floor(rnd() * far.length)];
    Object.assign(n, { route: [[n.lat, n.lon], [t[1], t[2]]], speed_kmh: 70 });
    trav.push(i);
  }
  // The attack.
  sc.events.push({ t_s: 1, type: "internet_down" });
  for (const [, la, lo] of HIT) sc.events.push({ t_s: 2, type: "kill_area", node_kind: "any", area: { circle: [la, lo, 25000], polygon: [] } });
  const fixed = sc.nodes.filter(n => n.kind === "repeater").length;
  sc.events.push({ t_s: 3, type: "kill_random", node_kind: "repeater", n: Math.round(fixed / 3) });
  // Messages: one every 20 s, half local (< 25 km), from users outside the hit areas.
  const hit = (n) => HIT.some(([, la, lo]) => DK.km([n.lat, n.lon], [la, lo]) < 25);
  const alive = users.filter(i => !hit(sc.nodes[i]));
  const msgs = [];
  for (let t = START; t < SEND_END; t += 20) {
    const a = alive[Math.floor(rnd() * alive.length)];
    let b;
    if (rnd() < 0.5) {
      const near = alive.filter(j => j !== a && DK.km([sc.nodes[a].lat, sc.nodes[a].lon], [sc.nodes[j].lat, sc.nodes[j].lon]) < 25);
      b = near.length ? near[Math.floor(rnd() * near.length)] : null;
    }
    if (b == null) do { b = alive[Math.floor(rnd() * alive.length)]; } while (b === a);
    const d = DK.km([sc.nodes[a].lat, sc.nodes[a].lon], [sc.nodes[b].lat, sc.nodes[b].lon]);
    sc.events.push({ t_s: t, type: "message", from: a, node: b });
    msgs.push({ a, b, d, trav: trav.includes(a) || trav.includes(b) });
  }
  sc.events.sort((x, y) => x.t_s - y.t_s);
  return { sc, msgs };
}

const VARIANTS = [
  ["no carry", {}],
  ["carry as today", { carry: true }],
  ["+ 6 h, 64 in flash", { carry: true, ttl: 21600, max: 64 }],
  ["+ 16 copies", { carry: true, ttl: 21600, max: 64, tokens: 16 }],
  ["+ offices hand to carriers", { carry: true, ttl: 21600, max: 64, office: true }],
  ["16 copies + offices", { carry: true, ttl: 21600, max: 64, tokens: 16, office: true }],
  ["smart carry", { carry: true, smart: true }],
  ["smart + 6 h, 64", { carry: true, smart: true, ttl: 21600, max: 64 }],
  ["smart + 6 h + offices", { carry: true, smart: true, ttl: 21600, max: 64, office: true }],
];
const ONLY = process.env.VARIANTS ? process.env.VARIANTS.split(",") : null;
console.log(`== crisis: ${HOURS} h, ${TRAVELLERS} travellers, seeds ${seeds.join(",")}`);
for (const [name, v] of VARIANTS.filter(([n]) => !ONLY || ONLY.some(o => n.startsWith(o)))) {
  const acc = { n: [0, 0, 0], ok: [0, 0, 0], lat: [[], [], []], air: 0, carried: 0, cdel: 0, office: 0, unsent: 0 };
  for (const seed of seeds) {
    const { sc, msgs } = build(seed, v);
    const sim = new Sim(sc);
    sim.step(END * 1000, 1e9);
    const s = sim.summary();
    const ml = sim.call({ cmd: "msgs", from: 0, max: 1e6 }).msgs;
    // Our messages are the ones from the events, in order (the generator's traffic is off).
    ml.slice(0, msgs.length).forEach(([, from, to, sent, deliv], k) => {
      const m = msgs[k];
      if (!m || m.a !== from) return;
      const g = m.d < 25 ? 0 : m.d < 100 ? 1 : 2;
      acc.n[g]++;
      if (deliv != null) { acc.ok[g]++; acc.lat[g].push((deliv - sent) / 1000); }
    });
    acc.air += s.airtime_s; acc.carried += s.carry_handoffs || 0; acc.cdel += s.carry_delivered || 0; acc.rr = (acc.rr || 0) + (s.carry_rerouted || 0); acc.office += s.office_carried || 0;
    sim.free();
  }
  const N = seeds.length;
  const g = k => {
    const l = acc.lat[k].sort((a, b) => a - b);
    const med = l.length ? l[l.length >> 1] : 0;
    const fmt = x => x < 120 ? `${x.toFixed(0)} s` : `${(x / 60).toFixed(0)} min`;
    return `${acc.ok[k]}/${acc.n[k]} (${acc.n[k] ? (100 * acc.ok[k] / acc.n[k]).toFixed(0) : 0}%, median ${fmt(med)})`;
  };
  console.log(`${name.padEnd(28)} <25 km ${g(0)} | 25-100 km ${g(1)} | >100 km ${g(2)} | airtime ${(acc.air / N).toFixed(0)} s | handoffs ${(acc.carried / N).toFixed(0)} carried-delivered ${(acc.cdel / N).toFixed(0)} office->carry ${(acc.office / N).toFixed(0)} rerouted ${((acc.rr || 0) / N).toFixed(0)}`);
}
