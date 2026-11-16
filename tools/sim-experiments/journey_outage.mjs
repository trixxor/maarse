// Experiment: the journey from North Jutland to Bornholm (see journey.mjs)
// with an internet outage: down from 1000 s, back 30 minutes after the
// traveller switches on in Roenne, measured for an hour after that. Compares
// post offices with the improvements for outages: refetch when the backbone
// returns, longer retries, and floods dropped first on the trunks.
//
// (The original description of the journey follows.)
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
// Usage: node tools/sim-experiments/journey_outage.mjs [seeds]
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

// OUTAGE_AFTER_S: how long after switching on the internet returns (default 1800).
const OFF = 1200, ON = 4200, BACK = ON + Number(process.env.OUTAGE_AFTER_S || 1800), END = BACK + 3600;
const near = (sc, lat, lon, n, not) => sc.nodes.map((x, i) => [i, x]).filter(([i, x]) => x.kind === "handheld" && !not.includes(i))
  .sort((a, b) => DK.km([a[1].lat, a[1].lon], [lat, lon]) - DK.km([b[1].lat, b[1].lon], [lat, lon])).slice(0, n).map(([i]) => i);

function build(seed, mail, down) {
  // TRUNKS=2: two trunks on each crossing (each on its own channel).
  // TRUNKS=2: two trunks per crossing; TRUNKS=multi: Skagen, Aarhus and the
  // Great Belt to Zealand plus Koege - Roenne.
  const ts = process.env.TRUNKS;
  const sc = DK.generateDenmark({ seed, trunks: ts === "2" ? 2 : ts === "multi" ? "multi" : true }, window.DK_LAND, window.DK_TOWNS);
  // HONEST=1: trunks on the mesh's channel and SF (they occupy the channel
  // around the sender and are jammed by mesh traffic at the far end).
  if (process.env.HONEST) sc.trunk_mesh_channel = true;
  // TRUNK_KIND=wifi: the two trunks as 5 GHz point-to-point links (far more
  // capacity, no duty cycle) instead of LoRa.
  if (process.env.TRUNK_KIND === "wifi") for (const l of sc.links) if (l.kind === "lora") l.kind = "wifi";
  // TRUNK_SF=7: every trunk at that SF (directional antennas give the margin;
  // slightly more loss assumed on the long Bornholm links).
  if (process.env.TRUNK_SF) for (const l of sc.links) if (l.kind === "lora") { l.sf = Number(process.env.TRUNK_SF); if (l.loss > 0.03) l.loss = 0.03; }
  // SAME_CHANNEL=1: all trunks on one frequency (shared channel at common endpoints).
  if (process.env.SAME_CHANNEL) sc.trunk_same_channel = true;
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
  if (down) { sc.events.push({ t_s: 1000, type: "internet_down" }); sc.events.push({ t_s: BACK, type: "internet_up" }); }
  // KILL_TRUNK=<label part>: that trunk fails at 1100 s (e.g. "Aarhus").
  if (process.env.KILL_TRUNK) sc.links.forEach((l, k) => { if (l.kind === "lora" && (l.label || "").includes(process.env.KILL_TRUNK)) sc.events.push({ t_s: 1100, type: "link", index: k, up: false }); });
  sc.events.sort((a, b) => a.t_s - b.t_s);
  return { sc, T, friends };
}

const ONLY = process.env.VARIANTS ? process.env.VARIANTS.split(",") : null;
const VARIANTS_ALL = [
  ["no mail (today)", "", {}],
  ["post offices", "post", {}],
  ["+ paced delivery", "post", { mail_paced: true }],
  ["+ consolidate at home", "post", { mail_consolidate: true }],
  ["+ host timers", "post", { host_slow_timers: true }],
  ["+ longer retries", "post", { mail_long_retry: true }],
  ["+ refetch on return", "post", { mail_refetch: true }],
  ["+ trunk priority", "post", { trunk_priority: true }],
  ["all four", "post", { mail_paced: true, mail_consolidate: true, host_slow_timers: true, mail_long_retry: true }],
  ["+ proxy replies", "post", { proxy_reply: true }],
  ["+ one trunk per request", "post", { trunk_hash: true }],
  ["+ who is behind", "post", { trunk_summary: true }],
  ["+ bundles", "post", { mail_bundle: true }],
  ["all optimisations", "post", { proxy_reply: true, trunk_hash: true, trunk_summary: true, mail_bundle: true }],
  ["bundles + who is behind", "post", { mail_bundle: true, trunk_summary: true }],
];
const VARIANTS = ONLY ? VARIANTS_ALL.filter(v => ONLY.some(o => v[0].startsWith(o))) : VARIANTS_ALL;
console.log(`== internet down 1000-${BACK} s; offline ${OFF}-${ON} s, then on Bornholm`);
for (const [name, mail, extra] of VARIANTS) {
  const acc = { during: [0, 0], after: [], late: 0, total: 0, ops: 0, air: 0, g: { A: [0, 0], C: [0, 0], B: [0, 0] }, tr: {}, jam: 0, proxy: 0 };
  for (const seed of seeds) {
    const { sc, T, friends } = build(seed, mail, true);
    Object.assign(sc, extra);
    const sim = new Sim(sc);
    sim.step(END * 1000, 1e9);
    const s = sim.summary();
    const ml = sim.call({ cmd: "msgs", from: 0, max: 100000 });
    for (const [, from, to, sent, deliv] of ml.msgs) {
      if (to !== T || !friends.includes(from) || sent >= ON * 1000) continue;
      acc.total++;
      const gi = friends.indexOf(from), g = gi < 3 ? "A" : gi < 5 ? "C" : "B";
      acc.g[g][1]++;
      if (deliv != null && deliv < BACK * 1000) acc.g[g][0]++;
      if (deliv != null && deliv < BACK * 1000) acc.during[0]++;
      else if (deliv != null) { acc.late++; acc.after.push(deliv - BACK * 1000); }
    }
    acc.ops += s.mail_ops || 0; acc.air += s.airtime_s;
    acc.jam += s.trunk_jammed || 0; acc.proxy += s.proxy_replies || 0;
    for (const t of s.trunks || []) { const e = acc.tr[t.label] || (acc.tr[t.label] = [0, 0]); e[0] += t.packets; e[1] += t.dropped; }
    sim.free();
  }
  acc.after.sort((a, b) => a - b);
  const med = acc.after.length ? `${(acc.after[acc.after.length >> 1] / 1000).toFixed(0)} s` : "-";
  const all = acc.during[0] + acc.late;
  const n = seeds.length;
  const gp = k => acc.g[k][1] ? `${(100 * acc.g[k][0] / acc.g[k][1]).toFixed(0)}%` : "-";
  const trs = Object.entries(acc.tr).map(([l, [p, d]]) => `${l.replace("LoRa trunk ", "")} ${(p / seeds.length).toFixed(0)}/${(d / seeds.length).toFixed(0)}`).join("; ");
  console.log(`${name.padEnd(22)} written while offline (${acc.total}): before the internet returns ${(100 * acc.during[0] / acc.total).toFixed(0)}% (Aalborg ${gp("A")}, Copenhagen ${gp("C")}, Bornholm ${gp("B")}) | within an hour after ${(100 * acc.late / acc.total).toFixed(0)}% (median ${med} after) | in all ${(100 * all / acc.total).toFixed(0)}% | airtime ${(acc.air / n).toFixed(0)} s${process.env.SHOW_TRUNKS ? ` | trunks sent/lost: ${trs} | jammed ${(acc.jam / n).toFixed(0)} proxy ${(acc.proxy / n).toFixed(0)}` : ""}`);
}
