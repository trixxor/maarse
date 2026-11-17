// Experiment: linked devices (docs/maybe/multi-device.md). One person has
// two devices: a PC node at home in Aalborg that stays on, and a phone that
// is switched off at 1200 s, travels to Bornholm and is switched on in
// Roenne at 4200 s. Friends in Aalborg, Copenhagen and on Bornholm write to
// the PERSON every 2 minutes. The sender uses the first of the person's
// devices it has a route to (else the PC). With sibling sync, the device that
// receives a message sends a copy to the other device (routed, LoRa or TCP,
// post offices when it is away). The same national preset, trunks and
// 2026-11-16 options as journey_outage.mjs.
//
// Measured for messages written while the phone was off or travelling and
// after, until END: share the phone has (original or copy) before the
// internet returns and by the end, the same for the PC, and airtime.
// Usage: node tools/sim-experiments/linked_devices.mjs [seeds]
// OUTAGE_AFTER_S as in journey_outage.mjs (default 1800).
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

const OFF = 1200, ON = 4200, BACK = ON + Number(process.env.OUTAGE_AFTER_S || 1800), END = BACK + Number(process.env.TAIL_S || 3600);
const MSG_END = BACK + 3600 - 300;
const near = (sc, lat, lon, n, not) => sc.nodes.map((x, i) => [i, x]).filter(([i, x]) => x.kind === "handheld" && !not.includes(i))
  .sort((a, b) => DK.km([a[1].lat, a[1].lon], [lat, lon]) - DK.km([b[1].lat, b[1].lon], [lat, lon])).slice(0, n).map(([i]) => i);

function build(seed, v) {
  const sc = DK.generateDenmark({ seed, trunks: true }, window.DK_LAND, window.DK_TOWNS);
  Object.assign(sc, { trace: false, neighbor_info: "hello", announce_s: 43200, announce_warmup_s: 600, contact_capacity: 0, mail: "post", mail_bundle: true, trunk_summary: true });
  Object.assign(sc.features, { key_request: true, key_cache: true, host_anchor: true, bordercast: true });
  Object.assign(sc.traffic, { start_s: 700, direct_per_min: 2, local_share: 0.9 });
  const rh = sc.nodes.findIndex(n => n.kind === "host" && (n.label || "").startsWith("Roenne"));
  const ch = sc.nodes.findIndex(n => n.kind === "host" && (n.label || "").startsWith("Copenhagen"));
  if (rh >= 0 && !sc.nodes[rh].internet) {
    sc.nodes[rh].internet = true;
    if (ch >= 0) sc.links.push({ a: rh, b: ch, kind: "tcp", up: true });
  }
  const [phone, pc] = near(sc, 57.048, 9.92, 2, []);
  const aal = near(sc, 57.048, 9.92, 3, [phone, pc]);
  const cph = near(sc, 55.68, 12.57, 2, [phone, pc, ...aal]);
  const brn = near(sc, 55.68, 12.57, 5, [phone, pc, ...aal, ...cph]).slice(2);
  brn.forEach((i, k) => Object.assign(sc.nodes[i], { lat: 55.10 + 0.01 * k, lon: 14.70 + 0.02 * k, region: "bornholm" }));
  const friends = [...aal, ...cph, ...brn];
  if (v.groups) sc.device_groups = [[pc, phone]];
  sc.sibling_sync = !!v.sync;
  sc.person_fresh = !!v.fresh;
  sc.redirect_hint = !!v.redirect;
  // Sync button: the owner presses it on the phone 30 s after switching on;
  // both devices also sync by themselves when the internet returns.
  if (v.button) {
    sc.events.push({ t_s: ON + 30, type: "sync", node: phone });
    if (v.down) for (const d of [phone, pc]) sc.events.push({ t_s: BACK + 60, type: "sync", node: d });
  }
  sc.events.push({ t_s: OFF, type: "kill", node: phone });
  if (v.pcOff) sc.events.push({ t_s: OFF, type: "kill", node: pc });
  sc.events.push({ t_s: ON - 1, type: "move", node: phone, area: { circle: [55.103, 14.705, 0], polygon: [] } });
  sc.events.push({ t_s: ON, type: "revive", nodes: [phone] });
  // Today (no groups) the friends write to the phone's own address.
  let k = 0;
  for (let t = OFF + 100; t < MSG_END; t += 120) sc.events.push({ t_s: t, type: "message", from: friends[k++ % friends.length], node: v.groups ? pc : phone });
  if (v.down) { sc.events.push({ t_s: 1000, type: "internet_down" }); sc.events.push({ t_s: BACK, type: "internet_up" }); }
  sc.events.sort((a, b) => a.t_s - b.t_s);
  return { sc, phone, pc, friends };
}

const VARIANTS = [
  ["today: phone's own address", {}],
  ["person, no sibling sync", { groups: true }],
  ["person + sibling sync", { groups: true, sync: true }],
  ["+ sync, newest announce", { groups: true, sync: true, fresh: true }],
  ["+ sync button", { groups: true, sync: true, button: true }],
  ["+ newest + redirect hint", { groups: true, sync: true, fresh: true, redirect: true }],
  ["+ newest + redirect + button", { groups: true, sync: true, fresh: true, redirect: true, button: true }],
  ["+ sync, PC off too", { groups: true, sync: true, pcOff: true }],
  ["+ PC off, sync button", { groups: true, sync: true, pcOff: true, button: true }],
];
const ONLY = process.env.VARIANTS ? process.env.VARIANTS.split(",") : null;
for (const down of [false, true]) {
  console.log(down ? `\n== internet down 1000-${BACK} s` : "\n== internet up");
  for (const [name, v] of VARIANTS.filter(([n]) => !ONLY || ONLY.some(o => n.startsWith(o)))) {
    const acc = { g: { A: [0, 0], C: [0, 0], B: [0, 0] }, total: 0, phoneDur: 0, phoneEnd: 0, pcEnd: 0, copies: 0, air: 0, lat: [] };
    for (const seed of seeds) {
      const { sc, phone, pc, friends } = build(seed, { ...v, down });
      const sim = new Sim(sc);
      sim.step(END * 1000, 1e9);
      const s = sim.summary();
      const ml = sim.call({ cmd: "msgs", from: 0, max: 100000 }).msgs;
      const first = new Map(); // original index -> {phone, pc} earliest delivery
      for (const [i, from, to, sent, deliv, , , syncOf] of ml) {
        const orig = syncOf ?? i;
        if (syncOf != null) acc.copies++;
        if (deliv == null) continue;
        const e = first.get(orig) || {};
        const who = to === phone ? "phone" : to === pc ? "pc" : null;
        if (who && (e[who] == null || deliv < e[who])) e[who] = deliv;
        first.set(orig, e);
      }
      for (const [i, from, to, sent] of ml) {
        if (!friends.includes(from) || sent < OFF * 1000 || (to !== phone && to !== pc)) continue;
        acc.total++;
        const e = first.get(i) || {};
        const gi = friends.indexOf(from), g = gi < 3 ? "A" : gi < 5 ? "C" : "B";
        acc.g[g][1]++;
        if (e.phone != null && e.phone < BACK * 1000) { acc.phoneDur++; acc.g[g][0]++; }
        if (e.phone != null) { acc.phoneEnd++; acc.lat.push(e.phone - Math.max(sent, ON * 1000)); }
        if (e.pc != null) acc.pcEnd++;
      }
      acc.air += s.airtime_s; acc.hints = (acc.hints || 0) + (s.redirect_hints || 0); acc.back = (acc.back || 0) + (s.backfilled || 0);
      sim.free();
    }
    acc.lat.sort((a, b) => a - b);
    const pct = x => `${(100 * x / acc.total).toFixed(0)}%`;
    const med = acc.lat.length ? `${(acc.lat[acc.lat.length >> 1] / 1000).toFixed(0)} s` : "-";
    const gp = k => acc.g[k][1] ? `${(100 * acc.g[k][0] / acc.g[k][1]).toFixed(0)}%` : "-";
    console.log(`${name.padEnd(28)} (${acc.total} msgs) phone: ${down ? `before internet returns ${pct(acc.phoneDur)} (from Aalborg ${gp("A")}, Copenhagen ${gp("C")}, Bornholm ${gp("B")}), ` : ""}by end ${pct(acc.phoneEnd)} (median ${med} after switch-on/sending) | PC by end ${pct(acc.pcEnd)} | copies ${(acc.copies / seeds.length).toFixed(0)} | airtime ${(acc.air / seeds.length).toFixed(0)} s${acc.hints || acc.back ? ` | hints ${(acc.hints / seeds.length).toFixed(0)} backfilled ${(acc.back / seeds.length).toFixed(0)}` : ""}`);
  }
}
