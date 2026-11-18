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
// Only TCP links need the internet; LoRa trunks stay up.
const tcpIdx = (k) => (sc.links[k] || {}).kind === "tcp";
const tcpUp = before.links_ok.filter((ok, k) => ok && tcpIdx(k)).length;
sim.step(280_000);
const s = sim.summary();
const after = sim.state();
console.log(`ran ${s.t / 1000} s simulated in ${Date.now() - t0} ms: ${s.messages} messages, delivered ${s.delivered_pct.toFixed(1)} %, acked ${s.acked_pct.toFixed(1)} %, tx ${s.tx}, collisions ${s.collisions}`);
check(s.delivered_pct > 0, "direct messages are delivered");
const tcpAfter = after.links_ok.filter((ok, k) => ok && tcpIdx(k)).length;
check(tcpUp > 0 && tcpAfter === 0, `internet down takes the TCP links down (${tcpUp} up before, ${tcpAfter} after)`);
check(sc.links.filter(l => l.kind === "lora").length === 2 && after.links_ok.filter((ok, k) => ok && sc.links[k].kind === "lora").length === 2, "the two LoRa trunks stay up without internet");
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
check(Array.isArray(ins.dead_hops), "node inspector lists dead next hops");

// Transparency: routed transmissions name their next hop, receptions carry
// SNR, collisions their cause, and the collision split adds up.
const all = [];
for (let since = 0; ;) { const r = sim.events(since, 50000); all.push(...r.lines); if (r.next === since || !r.lines.length) break; since = r.next; }
const routedTx = all.filter(e => e.ev === "tx" && e.path);
check(routedTx.length > 0 && routedTx.every(e => typeof e.nx === "number"), `routed transmissions carry their path and next hop (${routedTx.length})`);
check(all.filter(e => e.ev === "tx" && e.p === "PathRequest").every(e => e.nx === -1 && Array.isArray(e.trail)), "path requests are floods with a trail, not a next hop");
const rxs = all.filter(e => e.ev === "rx");
check(rxs.every(e => e.snr.length === e.ok.length && e.cw.length === e.coll.length), "receptions carry one SNR per receiver and one cause per collision");
check(s.coll_flood + s.coll_next_hop <= s.collisions, `collision split adds up (flood ${s.coll_flood}, next hop ${s.coll_next_hop}, all ${s.collisions})`);
sim.free();

// 1b. Granular disasters: kill / revive nodes, destroy an area, radio loss.
{
  const sc2 = DK.generateDenmark({ users: 150, repeaterKm: 22, seed }, window.DK_LAND, window.DK_TOWNS);
  sc2.duration_s = 300;
  const reps = sc2.nodes.map((n, i) => n.kind === "repeater" ? i : -1).filter(i => i >= 0).slice(0, 3);
  const x = new Sim(sc2);
  x.step(10_000);
  x.inject({ type: "kill", nodes: reps });
  check(reps.every(i => x.state().state[i] === "k"), "kill {nodes} kills exactly those nodes");
  x.inject({ type: "revive", nodes: [reps[0]] });
  const st = x.state().state;
  check(st[reps[0]] === "a" && st[reps[1]] === "k", "revive {nodes} revives only the listed node");
  const before = x.inspect(reps[1] + 1).radio_neighbors.length;
  const c = sc2.nodes[reps[1] + 1];
  x.inject({ type: "degrade", area: { circle: [c.lat, c.lon, 30000] }, db: 30 });
  const after = x.inspect(reps[1] + 1).radio_neighbors.length;
  check(after < before || before === 0, `radio loss in an area removes links (${before} -> ${after} neighbours)`);
  x.inject({ type: "degrade_clear" });
  check(x.inspect(reps[1] + 1).radio_neighbors.length === before, "clearing the radio loss restores the links");
  x.inject({ type: "kill_area", area: { circle: [c.lat, c.lon, 15000] }, node_kind: "any" });
  check(x.state().state[reps[1] + 1] === "k", "kill_area destroys nodes inside the area");
  x.free();
}

// 1c. DHT backbone: path requests go to one gateway host, not to every peer.
{
  const sc3 = DK.generateDenmark({ users: 250, repeaterKm: 22, seed }, window.DK_LAND, window.DK_TOWNS);
  sc3.duration_s = 300; sc3.features.dht_backbone = true;
  const x = new Sim(sc3);
  x.step(300_000);
  const y = x.summary();
  check(y.dht_lookups > 0 && y.delivered_pct > 0, `DHT backbone looks up gateways and still delivers (${y.dht_lookups} lookups, ${y.delivered_pct.toFixed(0)} %)`);
  x.free();
}

// 1e. Home routing model: lookups give routes over the fixed infrastructure,
// so far fewer path requests go on the air than today.
{
  const run = (home) => {
    const sc4 = DK.generateDenmark({ users: 250, repeaterKm: 22, seed }, window.DK_LAND, window.DK_TOWNS);
    sc4.duration_s = 300; sc4.trace = false; sc4.features.home_routing = home; sc4.features.dht_backbone = home;
    const x = new Sim(sc4); x.step(300_000); const y = x.summary(); x.free(); return y;
  };
  const today = run(false), home = run(true);
  console.log(`home routing: ${home.home_lookups} lookups (${home.home_local} local), ${home.home_routed} routed; path requests ${home.by_type.PathRequest || 0} vs ${today.by_type.PathRequest || 0} today; delivered ${home.delivered_pct.toFixed(0)} % vs ${today.delivered_pct.toFixed(0)} %`);
  check(home.home_routed > 0 && home.delivered_pct > 0, "home routing answers lookups with routes and delivers");
  check((home.by_type.PathRequest || 0) < (today.by_type.PathRequest || 0) / 2, "home routing floods far fewer path requests than today");
}

// 1f. Reboots: without stored contacts a rebooted node drops messages it can
// no longer read (after the router acked them); with them nothing is lost.
{
  const run = (persist, hold) => {
    const sc5 = DK.generateDenmark({ users: 200, repeaterKm: 22, seed }, window.DK_LAND, window.DK_TOWNS);
    sc5.trace = false; sc5.announce_s = 0;
    sc5.traffic.direct_per_min = 20; sc5.traffic.local_share = 1;
    Object.assign(sc5.features, { persist_known: persist, hold_unknown: hold });
    const hh = sc5.nodes.map((n, i) => n.kind === "handheld" ? i : -1).filter(i => i >= 0);
    sc5.events.push({ t_s: 30, type: "reboot", nodes: hh.slice(0, Math.floor(hh.length / 2)) });
    const x = new Sim(sc5); x.step(240_000); const y = x.summary(); x.free(); return y;
  };
  const before = run(false, false), kept = run(false, true), now = run(true, true);
  console.log(`reboot: before the fix ${before.false_acks} acked-not-shown, ${before.dropped_unknown} dropped, ${before.unsent_unknown} unsent; keep-unknown ${kept.held_waiting} kept; stored contacts ${now.false_acks} acked-not-shown`);
  check(before.reboots > 0 && before.dropped_unknown + before.unsent_unknown > 0, "a reboot without stored contacts loses messages");
  check(kept.dropped_unknown === 0 && kept.held_waiting + kept.shown_after_hold > 0, "keeping messages from unknown senders drops none");
  check(now.false_acks === 0 && now.unsent_unknown === 0, "with stored contacts a reboot shows every delivered message");
}

// 1g. Automatic forest mode: handhelds out of repeater range become relays.
{
  const sc6 = DK.forest({ role: "handheld", seed });
  sc6.features.auto_relay = true; sc6.auto_relay_after_s = 120; sc6.trace = false;
  const x = new Sim(sc6); x.step(600_000); const y = x.summary(); x.free();
  console.log(`auto forest mode: ${y.role_switches} role changes, ${y.relays_now} relays now`);
  check(y.role_switches > 0 && y.relays_now > 0, "automatic forest mode turns handhelds without a repeater into relays");
}

// 1d. Forest: clutter areas cut links; walkers move.
{
  const plain = DK.forest({ dbPerKm: 0, seed });
  const dense = DK.forest({ dbPerKm: 20, seed });
  const a = new Sim(plain), d = new Sim(dense);
  const walker = plain.nodes.findIndex(n => n.wander_m > 0);
  const na = a.inspect(walker).radio_neighbors.length, nd = d.inspect(walker).radio_neighbors.length;
  check(nd < na, `forest loss cuts radio links (${na} neighbours without, ${nd} with 20 dB/km)`);
  const p0 = [a.state().lat[walker], a.state().lon[walker]];
  a.step(120_000);
  const p1 = [a.state().lat[walker], a.state().lon[walker]];
  check(p0[0] !== p1[0] || p0[1] !== p1[1], "walkers wander");
  a.free(); d.free();
}

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

// 2b. Presets with expected results run to their end and must meet them.
for (const [id, p] of Object.entries(DK.PRESETS)) {
  if (!p.expect) continue;
  const t1 = Date.now();
  const sc = p.build(window.DK_LAND, window.DK_TOWNS, seed);
  sc.trace = true;
  const x = new Sim(sc);
  x.step((p.run_s || sc.duration_s) * 1000, 1e9);
  for (const [ok, text, known] of p.expect(x)) {
    if (known && !ok) console.log(`KNOWN ${id}: ${text} (known gap, not counted)`);
    else check(!!ok, `${id}: ${text}`);
  }
  console.log(`  (${id}: ${((Date.now() - t1) / 1000).toFixed(1)} s)`);
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

// Content types with the host's real rules (send.js uses the same commands).
{
  const cs = new Sim({ seed: 5, duration_s: 900, nodes: [
    { kind: "host", lat: 56.0, lon: 9.0 }, { kind: "repeater", lat: 56.0, lon: 9.05 }, { kind: "host", lat: 56.0, lon: 9.1 },
    { kind: "handheld", lat: 56.0, lon: 9.02 }, { kind: "handheld", lat: 56.0, lon: 9.08 },
  ], links: [{ a: 0, b: 2, kind: "tcp" }], traffic: { direct_per_min: 0, channel_per_min: 0 } });
  cs.step(30_000);
  const rules = cs.call({ cmd: "rules" });
  check(rules.name === "Standard (as built)" && rules.current.max_file === 1048576, "rules: the standard profile from maarse-hostrules");
  const voice = cs.call({ cmd: "send_file", from: 3, to: 4, kind: "voice", secs: 8 });
  const tooBig = cs.call({ cmd: "send_file", from: 3, to: 4, kind: "file", bytes: 20000 });
  const page = cs.call({ cmd: "page", from: 3, host: 0, path: "weather/aarhus" });
  const callLora = cs.call({ cmd: "call", from: 3, to: 4, secs: 5 });
  const callTcp = cs.call({ cmd: "call", from: 0, to: 2, secs: 5 });
  const ssh = cs.call({ cmd: "tunnel", from: 0, to: 2, workload: "ssh" });
  cs.step(600_000);
  const st = (v) => cs.call({ cmd: "item", item: v.item });
  check(st(voice).state === "delivered", `voice message over LoRa delivered (${st(voice).bytes} bytes)`);
  check(tooBig.state === "refused" && /8 KB/.test(tooBig.why), "20 KB from a handheld refused (8 KB over LoRa)");
  check(st(page).state === "delivered", "page fetched from a host");
  check(callLora.state === "refused" && /internet all the way/.test(callLora.why), "call over LoRa refused with the host's text");
  check(st(callTcp).state === "done" && st(callTcp).call.recv[0] > 0, "call over TCP carries audio");
  check(st(ssh).state === "done" && st(ssh).tunnel.answered === 40, `ssh over TCP: ${st(ssh).why}`);
  check(st(voice).events.length > 0, "item events in the trace for the follow view");
  cs.free();
}

if (failed) { console.log(`${failed} check(s) failed`); process.exit(1); }
console.log("OK");
