// Experiment: today's setup against the changes from the review of the
// whole chain (handheld in the forest -> bridge -> repeater -> host -> TCP ->
// host -> repeater -> node, forest or car), added one at a time.
//
// "today" is what runs on the boards: neighbors' roles only from 0-hop
// announces (12 h apart once settled), 64 contacts evicting the one heard
// longest ago, hosts as repeaters, a backbone that re-floods every path
// request in every region, automatic forest mode without bridges, no
// mobile flag. The changes:
//   hello      neighbor hello on a Trickle timer (RFC 6206)
//   bridges    named edge bridges + boundary forwarding (rule Q)
//   contacts   32 pinned contacts + 256 in RAM, strangers evicted first
//   keys       key requests, answered by the node or a host's announce cache
//   anchor     hosts as anchors (routes split at hosts)
//   bordercast hosts re-flood a TCP-borne request on the radio only for a
//              destination heard in their region (or on a second request)
//   mobile     cars say they move; routes to and from them go stale in 30 s
//   twins      a second host next to every host (host failure scenario)
//
// Usage: node tools/sim-experiments/all_changes.mjs <national|edge|car|f2f|hosts> [seeds]
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
const which = process.argv[2] || "national";
const seeds = (process.argv[3] || "1,2,3").split(",").map(Number);

// Settings per change; applied on top of each other in this order.
const CHANGES = {
  today: sc => {
    Object.assign(sc, { neighbor_info: "announce", announce_s: 43200, announce_warmup_s: 600, contact_capacity: 64, pinned_capacity: 32, contact_policy: "" });
    Object.assign(sc.features, { pin_contacts: false, key_request: false, key_cache: false, host_anchor: false, bordercast: false, mobile_mode: false, auto_edge: false });
  },
  hello: sc => { sc.neighbor_info = "hello"; },
  bridges: sc => { sc.features.auto_edge = true; Object.assign(sc, { auto_bridge: "named", bridge_boundary: true, bridge_p: -0.25 }); },
  contacts: sc => { sc.contact_capacity = 256; sc.features.pin_contacts = true; sc.contact_policy = "used"; },
  keys: sc => { sc.features.key_request = true; sc.features.key_cache = true; },
  anchor: sc => { sc.features.host_anchor = true; },
  bordercast: sc => { sc.features.bordercast = true; },
  mobile: sc => { sc.features.mobile_mode = true; },
};
const ORDER = ["today", "hello", "bridges", "contacts", "keys", "anchor", "bordercast", "mobile"];
function variants(names) {
  // Cumulative: today, +hello, +hello+bridges, ...
  return names.map((n, k) => [k === 0 ? "today" : "+ " + n, names.slice(0, k + 1)]);
}
function run(build, names, secs, extra) {
  const keys = ["messages", "delivered_pct", "shown_pct", "acked_pct", "cross_msgs", "cross_delivered_pct", "airtime_s", "latency_median_ms", "hellos", "key_requests", "role_switches"];
  const acc = Object.fromEntries(keys.map(k => [k, 0]));
  for (const seed of seeds) {
    const sc = build(seed);
    sc.trace = false;
    for (const n of names) CHANGES[n](sc);
    if (extra) extra(sc);
    // Traffic starts after the announce warm-up (every node announces once
    // in the first 10 minutes), so the burst does not hit the messages.
    sc.traffic.start_s = Math.max(sc.traffic.start_s || 0, 700);
    const sim = new Sim(sc); sim.step(secs * 1000, 1e9); const s = sim.summary(); sim.free();
    for (const k of keys) acc[k] += s[k] || 0;
  }
  for (const k of keys) acc[k] /= seeds.length;
  return acc;
}
const f1 = x => x.toFixed(1), f0 = x => x.toFixed(0);
function line(name, a, cross) {
  return `${name.padEnd(16)} delivered ${f1(a.delivered_pct).padStart(5)}% shown ${f1(a.shown_pct).padStart(5)}% acked ${f1(a.acked_pct).padStart(5)}%` +
    (cross ? ` | in/out ${f1(a.cross_delivered_pct).padStart(5)}%` : "") +
    ` | airtime ${f0(a.airtime_s).padStart(6)}s latency ${(a.latency_median_ms / 1000).toFixed(1)}s | hellos ${f0(a.hellos)} key req ${f0(a.key_requests)} switches ${f0(a.role_switches)}`;
}

if (which === "national") {
  for (const per of [6, 30]) {
    console.log(`== national, ${per} messages/min, friends (3600 s of traffic)`);
    const build = seed => {
      const sc = DK.PRESETS.normal.build(window.DK_LAND, window.DK_TOWNS, seed);
      Object.assign(sc.traffic, { friends: 8, friend_share: 0.9, start_s: 900, direct_per_min: per });
      return sc;
    };
    for (const [name, names] of variants(["today", "contacts", "keys", "hello", "anchor", "bordercast"])) console.log(line(name, run(build, names, 4500)));
  }
}

if (which === "edge") {
  const forest = seed => {
    const sc = DK.forest({ role: "handheld", seed, hillH: 25 });
    Object.assign(sc.nodes.find(n => n.label === "Relay on the hill"), { kind: "handheld", label: "Person on the hill" });
    return sc;
  };
  const auto = sc => { sc.features.auto_relay = true; sc.auto_relay_after_s = 120; };
  for (const [place, build] of [["forest (person on a hill)", forest], ["town (patchy coverage)", seed => DK.city({ seed })]]) {
    console.log(`== ${place}, 45 min of traffic`);
    for (const [name, names] of variants(["today", "contacts", "keys", "hello", "bridges"])) console.log(line(name, run(build, names, 3400, auto), true));
    console.log(line("(Q, instant)", run(build, ["today", "contacts", "keys", "bridges"], 3400, sc => { auto(sc); sc.neighbor_info = "instant"; }), true));
  }
}

if (which === "car") {
  console.log("== E45: a car at 110 km/h and a base writing to each other, 60 min of traffic");
  const build = seed => DK.PRESETS.e45.build(window.DK_LAND, window.DK_TOWNS, seed);
  for (const [name, names] of variants(["today", "contacts", "keys", "hello", "mobile"])) console.log(line(name, run(build, names, 4300)));
}

if (which === "f2f") {
  // Two forest groups far apart, each with a person on a hill, a chain of
  // repeaters to a host, and the hosts linked over TCP: about 15 hops.
  const build = seed => {
    const a = DK.forest({ role: "handheld", seed, hillH: 25 });
    const b = DK.forest({ role: "handheld", seed: seed + 100, hillH: 25 });
    Object.assign(a.nodes.find(n => n.label === "Relay on the hill"), { kind: "handheld" });
    Object.assign(b.nodes.find(n => n.label === "Relay on the hill"), { kind: "handheld" });
    // Move forest B 1.3 degrees south (about 145 km).
    const dy = -1.3;
    for (const n of b.nodes) n.lat += dy;
    for (const c of b.clutter) c.polygon = c.polygon.map(([la, lo]) => [la + dy, lo]);
    const off = a.nodes.length;
    a.nodes.push(...b.nodes);
    a.clutter.push(...b.clutter);
    // Chains: each forest's repeater (12 km east) -> two more repeaters
    // every 13 km -> host.
    const hosts = [];
    for (const [rep, lat0] of [[0, 56.80], [off, 56.80 + dy]]) {
      const r = a.nodes[rep];
      let lon = r.lon;
      for (let k = 0; k < 2; k++) { lon += 0.21; a.nodes.push({ kind: "repeater", lat: lat0, lon, label: "Chain repeater" }); }
      lon += 0.21;
      hosts.push(a.nodes.push({ kind: "host", lat: lat0, lon, internet: true, label: "Host" }) - 1);
    }
    a.links = [{ a: hosts[0], b: hosts[1], kind: "tcp", up: true }];
    a.traffic = { direct_per_min: 2, channel_per_min: 0, local_share: 0, local_km: 1, start_s: 600, stop_s: 0, spammers: [], spam_per_min: 0 };
    return a;
  };
  const auto = sc => { sc.features.auto_relay = true; sc.auto_relay_after_s = 120; };
  console.log("== forest to forest over TCP (about 15 hops), 60 min");
  for (const [name, names] of variants(["today", "contacts", "keys", "hello", "bridges", "anchor"])) console.log(line(name, run(build, names, 4300, auto), true));
}

if (which === "hosts") {
  console.log("== national, 6 messages/min, 30 % of hosts fail at 20 min (all changes on)");
  const all = ["today", "hello", "contacts", "keys", "anchor", "bordercast"];
  for (const twins of [false, true]) {
    const build = seed => {
      const sc = DK.generateDenmark ? DK.generateDenmark({ seed, hostPairs: twins }, window.DK_LAND, window.DK_TOWNS) : DK.PRESETS.normal.build(window.DK_LAND, window.DK_TOWNS, seed);
      Object.assign(sc.traffic, { friends: 8, friend_share: 0.9, start_s: 900, direct_per_min: 6 });
      const hosts = sc.nodes.map((n, i) => n.kind === "host" ? i : -1).filter(i => i >= 0);
      sc.events.push({ t_s: 1200, type: "kill_random", n: Math.round(hosts.length * 0.3 / (twins ? 2 : 1)) * (twins ? 2 : 1), node_kind: "host" });
      return sc;
    };
    console.log(line(twins ? "twin hosts" : "one host each", run(build, all, 4500)));
  }
}
