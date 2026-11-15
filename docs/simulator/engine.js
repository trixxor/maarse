/* Må:rse mesh simulator: the model.
 *
 * A simplified, seeded model of four LoRa mesh designs on the same map, the
 * same people and the same messages. It runs in the browser (app.js) and in
 * Node (tools/sim_web_test.mjs). It is meant for intuition, not for radio
 * planning: the project's real numbers come from maarse-sim, which runs the
 * actual Må:rse router code.
 */
(function (root) {
"use strict";

// ---------- helpers ----------
function rng(seed) {
  let a = seed >>> 0;
  return () => {
    a |= 0; a = a + 0x6D2B79F5 | 0;
    let t = Math.imul(a ^ a >>> 15, 1 | a);
    t = t + Math.imul(t ^ t >>> 7, 61 | t) ^ t;
    return ((t ^ t >>> 14) >>> 0) / 4294967296;
  };
}
function hash3(a, b, c) {
  let h = Math.imul(a + 1, 73856093) ^ Math.imul(b + 1, 19349663) ^ Math.imul(c + 1, 83492791);
  h = Math.imul(h ^ (h >>> 13), 0x5bd1e995); h ^= h >>> 15;
  return (h >>> 0) / 4294967296;
}
const clamp = (v, a, b) => Math.max(a, Math.min(b, v));
// Bounce inside [lo, hi] (a person walking into the edge turns around).
function reflect(v, lo, hi) { const L = hi - lo; let x = ((v - lo) % (2 * L) + 2 * L) % (2 * L); return lo + (x > L ? 2 * L - x : x); }

const PIDS = ["mt", "mc", "rn", "ma"];
const NAMES = { mt: "Meshtastic", mc: "MeshCore", rn: "Reticulum", ma: "Må:rse" };
// Seconds on the air for a ~50-byte packet at 125 kHz (rounded LoRa calculator values).
const AIRTIME = { 7: 0.1, 8: 0.18, 9: 0.33, 10: 0.62, 11: 1.24, 12: 2.3 };
const TX_MA = 120; // mA while transmitting at about 22 dBm (radio + PA, rough)

const CAUSES = {
  hops: "ran out of hops",
  range: "nobody could carry it there",
  noise: "lost to noise and collisions",
  broken: "the route broke",
  offline: "recipient offline, not stored",
  duty: "duty-cycle limit reached",
};

const DEFAULTS = {
  seed: 1, msgs: 40, noise: 0.1, speed: 1, duty: 0.1, sf: 0, hops: 3, people: 1, failures: 3,
  gw: true, server: true,
  f: { hopAck: true, bestPath: true, relay: true, carry: true, mailbox: true, backbone: true },
};

// ---------- scenarios (geometry and people; texts and drawing live in app.js) ----------
// Units: km and seconds. `range` is a node's typical range at the scenario's SF.
function towns(n, r, x, y, spread, count, extra) {
  for (let i = 0; i < count; i++) n.push({ x: x + (r() - .5) * spread, y: y + (r() - .5) * spread, role: "cli", range: 3, home: true, ...extra });
}
const SCENARIOS = {
  motorway: {
    w: 120, h: 30, sf: 10, duration: 3600, mobile: true,
    defaults: { msgs: 40, noise: 0.1, gw: false },
    build(r, o) {
      const n = [];
      for (let x = 7.5; x < 120; x += 15) n.push({ x, y: 15 + (r() - .5) * 2, role: "rep", range: 40, mast: true });
      n.push({ x: 2.5, y: 18, role: "cli", range: 3, gw: true, label: "Town A host" });
      n.push({ x: 117.5, y: 12, role: "cli", range: 3, gw: true, label: "Town B host" });
      towns(n, r, 3, 18, 5, 4, { area: 0 });
      towns(n, r, 117, 12, 5, 4, { area: 1 });
      const cars = Math.round(12 * o.people);
      for (let i = 0; i < cars; i++) n.push({ x: r() * 120, y: 15 + (r() < .5 ? -.6 : .6), role: "cli", range: 4, car: true, v: (r() < .5 ? 1 : -1) * 0.0306 });
      return n;
    },
    pick(r, w, cl) {
      const cars = cl.filter(n => n.car), homes = cl.filter(n => n.home), x = r();
      const a = x < .5 ? cars : x < .75 ? cars : homes.filter(n => n.area === 0);
      const b = x < .5 ? homes : x < .75 ? cars : homes.filter(n => n.area === 1);
      return [a[Math.floor(r() * a.length)], b[Math.floor(r() * b.length)]];
    },
  },
  forest: {
    w: 8, h: 8, sf: 10, duration: 2400, mobile: true, relayZone: true,
    defaults: { msgs: 40, noise: 0.1, gw: false },
    build(r, o) {
      const n = [], groups = [[2.1, 2.3], [5.6, 2.8], [3.5, 5.8]], people = Math.round(18 * o.people);
      for (let i = 0; i < people; i++) {
        const [gx, gy] = groups[i % 3];
        n.push({ x: gx + (r() - .5) * 2.4, y: gy + (r() - .5) * 2.4, role: "cli", range: 2, walker: true, dir: r() * 6.283, spd: 0.0012 });
      }
      return n;
    },
  },
  city: {
    w: 5, h: 5, sf: 8, duration: 1800, busy: true,
    defaults: { msgs: 80, noise: 0.15, gw: false },
    build(r, o) {
      const n = [];
      for (let i = 0; i < 4; i++) for (let j = 0; j < 3; j++) n.push({ x: .6 + i * 1.25 + (r() - .5) * .3, y: .85 + j * 1.65 + (r() - .5) * .3, role: "rep", range: 2.6, roof: true });
      const people = Math.round(110 * o.people);
      for (let i = 0; i < people; i++) n.push({ x: .1 + r() * 4.8, y: .1 + r() * 4.8, role: "cli", range: 1.2 });
      return n;
    },
  },
  country: {
    w: 50, h: 36, sf: 11, duration: 3600,
    defaults: { msgs: 40, noise: 0.15, gw: false },
    build(r, o) {
      const n = [];
      [[8, 8], [25, 6], [42, 10], [12, 27], [30, 22], [44, 30]].forEach(([x, y]) => n.push({ x, y, role: "rep", range: 40, mast: true }));
      n.push({ x: 4, y: 32, role: "cli", range: 4, gw: true, label: "farm with internet" });
      n.push({ x: 47, y: 3, role: "cli", range: 4, gw: true, label: "farm with internet" });
      const farms = Math.round(26 * o.people);
      for (let i = 0; i < farms; i++) n.push({ x: 2 + r() * 46, y: 2 + r() * 32, role: "cli", range: 4 });
      return n;
    },
  },
  mailbox: {
    w: 20, h: 14, sf: 9, duration: 5400, offline: true,
    defaults: { msgs: 40, noise: 0.1, gw: false },
    build(r, o) {
      const n = [];
      [[4, 4], [10, 6.5], [16, 4], [10, 11.5]].forEach(([x, y]) => n.push({ x, y, role: "rep", range: 10 }));
      if (o.server) n.push({ x: 10.6, y: 7.4, role: "cli", range: 4, server: true, label: "server" });
      const people = Math.round(22 * o.people);
      for (let i = 0; i < people; i++) {
        const off = r() < .5, from = 300 + r() * 2400;
        n.push({ x: 1 + r() * 18, y: 1 + r() * 12, role: "cli", range: 4, offFrom: off ? from : null, offTo: off ? from + 900 + r() * 1800 : null });
      }
      return n;
    },
    pick(r, w, cl, t) {
      const off = cl.filter(n => w.offlineAt(n, t));
      const d = off.length && r() < .65 ? off[Math.floor(r() * off.length)] : cl[Math.floor(r() * cl.length)];
      return [cl[Math.floor(r() * cl.length)], d];
    },
  },
  festival: {
    w: 1.6, h: 1.6, sf: 8, duration: 1800, busy: true, mobile: true,
    defaults: { msgs: 100, noise: 0.3, gw: false },
    build(r, o) {
      const n = [];
      [[.3, .3], [1.3, .3], [.3, 1.3], [1.3, 1.3]].forEach(([x, y]) => n.push({ x, y, role: "rep", range: 1.5, tower: true }));
      const people = Math.round(160 * o.people);
      for (let i = 0; i < people; i++) {
        const near = r() < .5;
        n.push({ x: near ? .5 + r() * .6 : .05 + r() * 1.5, y: near ? .1 + r() * .45 : .05 + r() * 1.5, role: "cli", range: .55, walker: true, dir: r() * 6.283, spd: 0.0007 });
      }
      return n;
    },
  },
  blackout: {
    w: 14, h: 10, sf: 9, duration: 3600,
    defaults: { msgs: 50, noise: 0.15, gw: false },
    build(r, o) {
      const n = [];
      for (let i = 0; i < 4; i++) for (let j = 0; j < 3; j++) if (!(i === 3 && j === 2)) n.push({ x: 1.8 + i * 3.5 + (r() - .5), y: 1.7 + j * 3.3 + (r() - .5), role: "rep", range: 7 });
      n.push({ x: 1, y: 9, role: "cli", range: 2.5, gw: true, label: "host" });
      n.push({ x: 13, y: 1, role: "cli", range: 2.5, gw: true, label: "host" });
      const people = Math.round(40 * o.people);
      for (let i = 0; i < people; i++) n.push({ x: .5 + r() * 13, y: .5 + r() * 9, role: "cli", range: 2.5 });
      return n;
    },
    // A storm takes out repeaters a quarter of the way into the run.
    failures(w, r) {
      const reps = w.nodes.filter(n => n.role === "rep");
      reps.sort((a, b) => Math.hypot(a.x - 7, a.y - 5) - Math.hypot(b.x - 7, b.y - 5));
      const k = Math.min(w.opt.failures, reps.length);
      for (let i = 0; i < k; i++) reps[i].failAt = w.duration * .25;
    },
  },
  oslo: {
    w: 100, h: 70, sf: 9, duration: 3600,
    defaults: { msgs: 40, noise: 0.1, gw: true },
    build(r, o) {
      const n = [];
      const isl = [[24, 46, "Aarhus"], [48, 60, "Copenhagen"], [58, 14, "Oslo"]];
      isl.forEach(([cx, cy, name], k) => {
        for (let i = 0; i < 4; i++) { const a = i / 4 * 6.283 + r() * .5; n.push({ x: cx + Math.cos(a) * 3.8, y: cy + Math.sin(a) * 3.8, role: "rep", range: 9, area: k }); }
        n.push({ x: cx + .5, y: cy - .5, role: "cli", range: 6, gw: true, area: k, label: name + " host" });
        const people = Math.round(8 * o.people);
        for (let i = 0; i < people; i++) { const a = r() * 6.283, d = 1 + r() * 7; n.push({ x: cx + Math.cos(a) * d, y: cy + Math.sin(a) * d * .8, role: "cli", range: 3, area: k }); }
      });
      return n;
    },
    pick(r, w, cl) {
      const s = cl[Math.floor(r() * cl.length)];
      const other = cl.filter(n => r() < .7 ? n.area !== s.area : n.area === s.area && n !== s);
      return [s, other[Math.floor(r() * other.length)] || cl[0]];
    },
  },
  sandbox: { w: 10, h: 7, sf: 9, duration: 2400, mobile: false, defaults: { msgs: 30, noise: 0.1, gw: true } },

  // Hidden scenarios used by the scaling lab.
  labgrid: {
    w: 7, h: 7, sf: 9, duration: 1800, busy: true,
    build(r, o) {
      const N = o.labN, side = Math.sqrt(N / 4), n = [], reps = Math.max(2, Math.round(N / 10)), g = Math.ceil(Math.sqrt(reps));
      this.w = this.h = side;
      for (let k = 0; k < reps; k++) n.push({ x: ((k % g) + .5) * side / g, y: (Math.floor(k / g) + .5) * side / g, role: "rep", range: 2.8 });
      for (let i = 0; i < N - reps; i++) n.push({ x: r() * side, y: r() * side, role: "cli", range: 1.3 });
      return n;
    },
  },
  labchain: {
    w: 60, h: 6, sf: 9, duration: 1800,
    build(r, o) {
      const n = [{ x: 0, y: 3, role: "cli", range: 3, label: "from" }];
      for (let i = 0; i < o.labHops; i++) n.push({ x: 1 + i * 4, y: 3, role: "rep", range: 8 });
      n.push({ x: 2 + (o.labHops - 1) * 4, y: 3, role: "cli", range: 3, label: "to" });
      this.w = Math.max(8, 4 + o.labHops * 4);
      return n;
    },
    pick(r, w, cl) { return [cl[0], cl[cl.length - 1]]; },
  },
};

// ---------- world ----------
class World {
  constructor(scId, opt, custom) {
    this.scId = scId; this.sc = SCENARIOS[scId]; this.opt = opt;
    const r = rng(opt.seed * 7919 + 13);
    const raw = scId === "sandbox" ? (custom || []).map(n => ({ ...n })) : this.sc.build(r, opt);
    this.w = this.sc.w; this.h = this.sc.h;
    this.sf = opt.sf || this.sc.sf;
    this.rangeK = Math.pow(1.25, this.sf - this.sc.sf);
    this.at = AIRTIME[this.sf];
    this.duration = this.sc.duration;
    this.nodes = raw.map((n, i) => ({ ...n, id: i, x0: n.x, y0: n.y, off: false }));
    this.N = this.nodes.length;
    this.gws = this.nodes.filter(n => n.gw).map(n => n.id);
    this.loss = 1;
    this.Q = new Float32Array(this.N * this.N);
    this.movers = this.nodes.filter(n => n.car || n.walker).map(n => n.id);
    if (this.sc.failures) this.sc.failures(this, r);
    this.t = -1;
    this.msgs = this.makeMessages(r);
    this.setTime(0, true);
  }
  offlineAt(n, t) { return (n.offFrom != null && t >= n.offFrom && t < n.offTo) || (n.failAt != null && t >= n.failAt); }
  makeMessages(r) {
    let cl = this.nodes.filter(n => n.role === "cli" && !n.gw && !n.server);
    if (cl.length < 2) cl = this.nodes.filter(n => n.role === "cli");
    if (cl.length < 2) cl = this.nodes.slice();
    const out = [];
    if (cl.length < 2) return out;
    for (let k = 0; k < this.opt.msgs; k++) {
      for (let tries = 0; tries < 60; tries++) {
        const t = Math.floor(r() * this.duration * .75);
        let [s, d] = this.sc.pick ? this.sc.pick(r, this, cl, t) : [cl[Math.floor(r() * cl.length)], cl[Math.floor(r() * cl.length)]];
        if (!s || !d || s === d || this.offlineAt(s, t)) continue;
        out.push({ t, s: s.id, d: d.id, k });
        break;
      }
    }
    out.sort((a, b) => a.t - b.t);
    // The sandbox can name its own sender and recipient: that pair goes first.
    const pr = this.opt.pair;
    if (pr && this.nodes[pr[0]] && this.nodes[pr[1]] && pr[0] !== pr[1]) out.unshift({ t: 0, s: pr[0], d: pr[1] });
    out.forEach((m, i) => m.k = i);
    return out;
  }
  linkQ(a, b) {
    const d = Math.hypot(a.x - b.x, a.y - b.y), R = (a.range + b.range) / 2 * this.rangeK;
    if (d >= R) return 0;
    const base = d < .55 * R ? .97 : .97 * Math.pow(1 - (d - .55 * R) / (.45 * R), 1.4);
    return base * (.8 + .2 * hash3(Math.min(a.id, b.id), Math.max(a.id, b.id), this.opt.seed));
  }
  setTime(t, full) {
    const o = this.opt, N = this.N;
    for (const n of this.nodes) {
      if (n.car) n.x = ((n.x0 + n.v * o.speed * t) % this.w + this.w) % this.w;
      if (n.walker) { n.x = reflect(n.x0 + Math.cos(n.dir) * n.spd * o.speed * t, .05, this.w - .05); n.y = reflect(n.y0 + Math.sin(n.dir) * n.spd * o.speed * t, .05, this.h - .05); }
      n.off = this.offlineAt(n, t);
    }
    const rows = full || this.t < 0 ? this.nodes.map(n => n.id) : (o.speed > 0 ? this.movers : []);
    for (const i of rows) for (let j = 0; j < N; j++) {
      if (i === j) continue;
      const v = this.linkQ(this.nodes[i], this.nodes[j]);
      this.Q[i * N + j] = this.Q[j * N + i] = v;
    }
    this.nb = this.nodes.map((n, i) => { const l = []; if (n.off) return l; for (let j = 0; j < N; j++) if (j !== i && this.Q[i * N + j] > .02 && !this.nodes[j].off) l.push(j); return l; });
    for (const n of this.nodes) n.hearsRep = this.nb[n.id].some(j => this.nodes[j].role === "rep" && this.q(n.id, j) > .3);
    this.t = t;
    this._gwFor = new Map();
  }
  q(i, j) { return this.nodes[i].off || this.nodes[j].off ? 0 : this.Q[i * this.N + j]; }
  p(i, j) { return this.q(i, j) * (1 - this.opt.noise) * this.loss; }
  tcpLink(a, b) { return this.opt.gw && this.nodes[a].gw && this.nodes[b].gw && !this.nodes[a].off && !this.nodes[b].off; }
  // Gateways that can reach node d over LoRa (where the DHT says d lives).
  gwFor(d) {
    if (this._gwFor.has(d)) return this._gwFor.get(d);
    const seen = new Set([d]), out = [], queue = [d];
    while (queue.length) {
      const u = queue.shift();
      if (this.nodes[u].gw && u !== d) { out.push(u); continue; }
      if (u !== d && this.nodes[u].role !== "rep") continue;
      for (const v of this.nb[u]) if (!seen.has(v) && this.q(u, v) > .2) { seen.add(v); queue.push(v); }
    }
    if (this.nodes[d].gw) out.push(d);
    this._gwFor.set(d, out);
    return out;
  }
  // Fewest radio hops from s to d through nodes allowed by fwd (ignores hop limits).
  reachHops(s, d, fwd) {
    const dist = new Map([[s, 0]]), queue = [s];
    while (queue.length) {
      const u = queue.shift(), h = dist.get(u);
      if (u === d) return h;
      if (u !== s && !fwd(u, h)) continue;
      for (const v of this.nb[u]) if (!dist.has(v) && this.q(u, v) > .25) { dist.set(v, h + 1); queue.push(v); }
      if (this.opt.gw && this.nodes[u].gw) for (const g of this.gws) if (!dist.has(g) && !this.nodes[g].off) { dist.set(g, h); queue.push(g); }
    }
    return Infinity;
  }
  avgDegree() { let s = 0; for (let i = 0; i < this.N; i++) for (const j of this.nb[i]) if (this.q(i, j) > .3) s++; return s / Math.max(1, this.N); }
}

// ---------- accounting ----------
function newStats(w, pid) {
  return {
    pid, air: 0, tx: 0, bystander: 0, nodeAir: new Float64Array(w.N),
    duty: w.opt.duty > 0 ? w.opt.duty * w.duration : Infinity, dutyHit: false,
    paths: new Map(), ann: new Map(), cur: null,
    saved: { hopRetry: 0, repair: 0, carry: 0, mailbox: 0, backbone: 0, relay: 0, late: 0 },
  };
}
// Spend airtime on node i if its duty-cycle budget allows it.
function spend(w, st, i, factor) {
  const a = w.at * factor;
  if (st.nodeAir[i] + a > st.duty) { st.dutyHit = true; return false; }
  st.nodeAir[i] += a; st.air += a; st.tx += 1;
  const n = w.nodes[i], m = st.cur;
  if (n.role === "cli" && !n.gw && !n.server && (!m || (i !== m.s && i !== m.d))) st.bystander += a;
  return true;
}

// ---------- building blocks ----------
// Slot-based flood. Everyone that hears a new packet in step k may repeat it in step k+1.
function flood(w, st, src, o, R) {
  const got = new Map([[src, { hop: 0, parent: -1, first: -1, q: 1 }]]);
  let tx = [src], slots = 0;
  while (tx.length && slots < 80) {
    slots++;
    const sent = tx.filter(i => spend(w, st, i, o.factor));
    if (!sent.length) break;
    const cand = new Map();
    for (const i of sent) for (const j of w.nb[i]) { if (got.has(j)) continue; let a = cand.get(j); if (!a) cand.set(j, a = []); a.push(i); }
    const newly = [];
    for (const [j, arr] of cand) {
      let strong = 0;
      for (const i of arr) if (w.q(i, j) > .5) strong++;
      // Copies in the same step mostly collide only when several are strong
      // (random rebroadcast delays spread the rest out).
      const cap = 1 / (1 + .15 * Math.max(0, strong - 1));
      let best = -1, bq = 0; const succ = [];
      for (const i of arr) if (R() < w.p(i, j) * cap) { succ.push(i); if (w.q(i, j) > bq) { bq = w.q(i, j); best = i; } }
      if (best >= 0) { const first = succ[Math.floor(R() * succ.length)]; got.set(j, { hop: got.get(first).hop + 1, parent: best, first, q: bq }); newly.push(j); }
    }
    const tcp = [];
    if (o.bridge && w.opt.gw) {
      for (const j of newly.slice()) if (w.nodes[j].gw) {
        const targets = o.bridge === "all" ? w.gws : w.gwFor(o.dst);
        for (const k of targets) if (!got.has(k) && !w.nodes[k].off) { got.set(k, { hop: got.get(j).hop, parent: j, first: j, q: 1, tcp: true }); newly.push(k); tcp.push([j, k]); }
      }
    }
    o.trace?.push({ kind: "flood", tx: sent, newly, tcp, label: o.label, factor: o.factor });
    const next = newly.filter(j => j !== o.dst && got.get(j).hop <= o.hopLimit && (got.get(j).tcp || o.fwd(j, got.get(j).hop)));
    // Weak signal = far away = shorter wait, so the farthest node repeats first.
    next.sort((a, b) => got.get(a).q - got.get(b).q);
    const sel = [];
    for (const j of next) {
      if (o.suppress && o.suppress(j) && sel.some(k => R() < w.p(k, j))) continue; // heard a neighbour repeat it: cancel
      sel.push(j);
    }
    tx = sel;
  }
  return { got, slots };
}
function chainVia(got, d, key) { const p = []; for (let x = d; x !== -1; x = got.get(x)[key]) { p.unshift(x); if (p.length > 200) break; } return p; }

// Send along a path. acked: acknowledgement and resend on every hop (Må:rse).
function sendPath(w, st, path, o, R) {
  let time = 0, retried = 0;
  for (let h = 0; h + 1 < path.length; h++) {
    const a = path[h], b = path[h + 1];
    if (w.tcpLink(a, b)) { time += .15; o.trace?.push({ kind: "tcp", a, b, label: o.label }); st.usedTcp = true; continue; }
    let ok = false;
    const tries = o.tries || 1;
    for (let t = 0; t < tries; t++) {
      if (!spend(w, st, a, o.factor)) { o.trace?.push({ kind: "hop", a, b, ok: false, duty: true, label: o.label }); return { ok: false, time, cause: "duty", at: h, retried }; }
      time += w.at * o.factor + .05;
      const okData = R() < w.p(a, b);
      o.trace?.push({ kind: "hop", a, b, ok: okData, retry: t > 0, label: o.label });
      if (okData) {
        ok = true;
        if (t > 0) retried++;
        if (o.acked) { spend(w, st, b, .3); time += w.at * .3; o.trace?.push({ kind: "ack", a: b, b: a, label: o.label }); }
        break;
      }
    }
    if (!ok) return { ok: false, time, cause: w.q(a, b) < .02 ? "broken" : "noise", at: h, retried };
  }
  return { ok: true, time, retried };
}

// Best path by link quality (Dijkstra on -log q) through allowed nodes; TCP links between gateways are free.
function bestPath(w, s, d, allowed, maxHops) {
  const N = w.N, dist = new Float64Array(N).fill(Infinity), prev = new Int32Array(N).fill(-1), hops = new Int32Array(N), done = new Uint8Array(N);
  dist[s] = 0;
  const noise = 1 - w.opt.noise;
  for (;;) {
    let u = -1, m = Infinity;
    for (let i = 0; i < N; i++) if (!done[i] && dist[i] < m) { m = dist[i]; u = i; }
    if (u < 0 || u === d) break;
    done[u] = 1;
    if (u !== s && !allowed(u)) continue;
    if (hops[u] >= (maxHops || 64)) continue;
    const relax = (v, cost) => { const nd = dist[u] + cost; if (nd < dist[v]) { dist[v] = nd; prev[v] = u; hops[v] = hops[u] + 1; } };
    for (const v of w.nb[u]) { const q = w.q(u, v) * noise; if (q > .05) relax(v, -Math.log(q) + .05); }
    if (w.opt.gw && w.nodes[u].gw) for (const g of w.gws) if (g !== u && !w.nodes[g].off) relax(g, .01);
  }
  if (dist[d] === Infinity) return null;
  const path = []; for (let x = d; x !== -1; x = prev[x]) path.unshift(x);
  return path;
}

function causeFor(w, m, fwd, hopLimit, st) {
  if (st.dutyHit) return "duty";
  const h = w.reachHops(m.s, m.d, fwd);
  if (h === Infinity) return "range";
  if (hopLimit != null && h > hopLimit + 1) return "hops";
  return "noise";
}
const isRep = w => j => w.nodes[j].role === "rep";

// ---------- the four designs: one direct message each ----------
const PROTO = {
  // Meshtastic: managed flood. Routers always repeat, clients cancel if they hear a neighbour repeat.
  mt(w, st, m, R, tr) {
    let time = 0, delivered = false;
    const fwd = () => true;
    const sup = j => w.nodes[j].role === "cli";
    for (let a = 0; a < 3; a++) {
      const f = flood(w, st, m.s, { fwd, suppress: sup, hopLimit: w.opt.hops, factor: 1, dst: -1, bridge: "all", trace: tr, label: a ? "No acknowledgement came back, so the whole flood is sent again" : `Flood: every node that hears it repeats it (hop limit ${w.opt.hops})` }, R);
      time += f.slots * w.at * 2.5;
      if (f.got.has(m.d)) {
        if (!delivered) st.reach = f.got;
        delivered = true;
        const back = flood(w, st, m.d, { fwd, suppress: sup, hopLimit: w.opt.hops, factor: .45, dst: m.s, bridge: "all", trace: tr, label: "The recipient's acknowledgement is flooded back" }, R);
        if (back.got.has(m.s)) return { ok: true, time };
      } else time += 6;
    }
    if (delivered) return { ok: true, time };
    return { ok: false, time, cause: causeFor(w, m, fwd, w.opt.hops, st) };
  },

  // MeshCore: only repeaters forward. The first message is flooded, then it follows the path it learned.
  mc(w, st, m, R, tr) {
    let time = 0, delivered = false;
    const key = m.s + ">" + m.d, rep = isRep(w);
    for (let a = 0; a < 3; a++) {
      const path = st.paths.get(key);
      if (path) {
        const r = sendPath(w, st, path, { factor: .9, trace: tr, label: "Direct along the path it learned last time" }, R);
        time += r.time;
        if (r.ok) {
          sendPath(w, st, path.slice().reverse(), { factor: .3, trace: tr, label: "End-to-end acknowledgement comes back" }, R);
          return { ok: true, time };
        }
        st.paths.delete(key); time += 8;
        tr?.push({ kind: "note", label: "No acknowledgement: forget the path and flood again" });
        continue;
      }
      const f = flood(w, st, m.s, { fwd: rep, hopLimit: 64, factor: .9, dst: m.d, trace: tr, label: "Flood through the repeaters (clients do not repeat)" }, R);
      time += f.slots * w.at * 1.6;
      if (!f.got.has(m.d)) { time += 8; continue; }
      st.reach = f.got;
      delivered = true;
      const p = chainVia(f.got, m.d, "first");
      const back = sendPath(w, st, p.slice().reverse(), { factor: .5, trace: tr, label: "The recipient returns the path (with the acknowledgement)" }, R);
      if (back.ok) { st.paths.set(key, p); return { ok: true, time }; }
      // The sender never hears the acknowledgement, so it sends the message again.
      tr?.push({ kind: "note", label: "The acknowledgement was lost, so the sender floods the message again" });
    }
    if (delivered) return { ok: true, time };
    return { ok: false, time, cause: causeFor(w, m, rep, 64, st) };
  },

  // Reticulum: announces teach transport nodes the next hop; a path request when the path is unknown.
  rn(w, st, m, R, tr) {
    let time = 0;
    const tn = isRep(w);
    for (let a = 0; a < 5; a++) { // LXMF makes several delivery attempts
      let path = st.paths.get(m.s + ">" + m.d);
      if (!path) {
        const ann = st.ann.get(m.d);
        if (ann && ann.has(m.s) && !a) {
          path = chainVia(ann, m.s, "first").reverse();
          tr?.push({ kind: "note", label: "Path known from the recipient's announce, heard earlier" });
        }
      }
      if (!path) {
        const f = flood(w, st, m.s, { fwd: tn, hopLimit: 128, factor: 1, dst: m.d, bridge: "all", trace: tr, label: "Path request floods the transport nodes" }, R);
        time += f.slots * w.at * 1.6;
        if (!f.got.has(m.d)) { time += 10; continue; }
        path = chainVia(f.got, m.d, "first");
        const back = sendPath(w, st, path.slice().reverse(), { factor: 2.2, tries: 2, trace: tr, label: "Path response (a signed announce) comes back" }, R);
        if (!back.ok) { time += 10; continue; }
      }
      st.paths.set(m.s + ">" + m.d, path);
      const r = sendPath(w, st, path, { factor: 1.4, trace: tr, label: "Along the path, one next hop at a time" }, R);
      time += r.time;
      if (r.ok) {
        sendPath(w, st, path.slice().reverse(), { factor: .5, trace: tr, label: "Delivery proof comes back" }, R);
        return { ok: true, time };
      }
      st.paths.delete(m.s + ">" + m.d); time += 12;
      tr?.push({ kind: "note", label: "No proof came back: ask for a new path" });
    }
    return { ok: false, time, cause: causeFor(w, m, tn, 128, st) };
  },

  // Må:rse: path request, best-SNR path, acknowledgement on every hop and route repair,
  // relay role for handhelds when no repeater is heard, internet backbone with DHT.
  ma(w, st, m, R, tr) {
    const F = w.opt.f, key = m.s + ">" + m.d;
    const relayOk = j => F.relay && w.sc.relayZone !== false && w.nodes[j].role === "cli" && !w.nodes[j].car && !w.nodes[j].hearsRep && !!(w.sc.relayZone || w.scId === "sandbox");
    const fwd = (j, hop) => w.nodes[j].role === "rep" || (relayOk(j) && hop <= 4);
    let time = 0, usedRelay = false, disc = 0, sends = 0;
    // Up to 3 path requests and 3 sends (each send retries every hop up to 3 times).
    while (disc < 3 && sends < 3) {
      let path = st.paths.get(key);
      if (!path) {
        const f = flood(w, st, m.s, { fwd, suppress: j => w.nodes[j].role === "cli", hopLimit: 64, factor: .6, dst: m.d, bridge: F.backbone ? "dht" : null, trace: tr,
          label: F.backbone && w.opt.gw && w.gws.length ? "Small path request via repeaters; a host looks the recipient up in the DHT" : relayOk(m.s) || w.sc.relayZone ? "Small path request: no repeaters here, so handhelds relay (max 4 hops)" : "Small path request floods the repeaters" }, R);
        time += f.slots * w.at * 1.6;
        disc++;
        st.lastFlood = f.got;
        if (!f.got.has(m.d)) { time += 5; tr?.push({ kind: "note", label: "No answer: try the path request again" }); continue; }
        st.reach = f.got;
        path = F.bestPath ? bestPath(w, m.s, m.d, j => f.got.has(j) && (fwd(j, f.got.get(j).hop) || w.nodes[j].gw), 64) : null;
        if (!path) path = chainVia(f.got, m.d, "first");
        const back = sendPath(w, st, path.slice().reverse(), { factor: .5, acked: F.hopAck, tries: F.hopAck ? 3 : 1, trace: tr,
          label: F.bestPath ? "The recipient answers along the best-signal path" : "The recipient answers along the first copy's path" }, R);
        time += back.time;
        if (!back.ok) { tr?.push({ kind: "note", label: "The answer was lost: ask again" }); continue; }
        st.paths.set(key, path);
      }
      usedRelay = usedRelay || path.some(j => w.nodes[j].role === "cli" && j !== m.s && j !== m.d && !w.nodes[j].gw);
      const r = sendPath(w, st, path, { factor: 1, acked: F.hopAck, tries: F.hopAck ? 3 : 1, trace: tr, label: F.hopAck ? "Along the path, every hop acknowledged (resent on that hop if lost)" : "Along the path (per-hop acknowledgement switched off)" }, R);
      time += r.time;
      sends++;
      st.saved.hopRetry += r.retried || 0;
      if (r.ok) {
        if (!F.hopAck) sendPath(w, st, path.slice().reverse(), { factor: .3, trace: tr, label: "End-to-end acknowledgement comes back" }, R);
        if (path.some((x, i) => i && w.tcpLink(path[i - 1], x))) st.saved.backbone++;
        if (usedRelay) st.saved.relay++;
        if (sends > 1 || disc > 1) st.saved.repair++;
        return { ok: true, time };
      }
      st.paths.delete(key);
      time += F.hopAck ? 1 : 10; // with per-hop acks the break is noticed at once
      tr?.push({ kind: "note", label: F.hopAck ? "A hop failed after 3 tries: route repair starts at once" : "No acknowledgement: wait, then look for a new path" });
    }
    return { ok: false, time, cause: causeFor(w, m, fwd, null, st) };
  },
};

// Reticulum: every node announces at start-up; transport nodes learn the way back to it.
function announceAll(w, st, R) {
  const a0 = st.air;
  for (const n of w.nodes) {
    if (n.off) continue;
    const f = flood(w, st, n.id, { fwd: isRep(w), hopLimit: 128, factor: 2.2, dst: -1, bridge: "all" }, R);
    st.ann.set(n.id, f.got);
  }
  st.annAir = (st.annAir || 0) + st.air - a0;
}

// Where Må:rse keeps a message for later: a host or server, else the nearest repeater (flash mailbox).
function mailboxHolder(w, s) {
  const allowed = j => w.nodes[j].role === "rep" || w.nodes[j].gw;
  const hosts = w.nodes.filter(n => (n.gw || n.server) && !n.off && n.id !== s);
  if (w.nodes[s].gw || w.nodes[s].server) return s;
  let best = null, bl = Infinity;
  for (const h of hosts) { const p = bestPath(w, s, h.id, allowed, 12); if (p && p.length < bl) { bl = p.length; best = h.id; } }
  if (best != null) return best;
  for (const r of w.nodes) if (r.role === "rep" && !r.off && w.q(s, r.id) > .3) return r.id;
  return null;
}
function routeTo(w, st, pid, s, d, R) { return PROTO[pid](w, st, { s, d, t: w.t }, R); }

// ---------- a whole run ----------
function runProtocol(w, pid, loss) {
  w.loss = loss;
  const st = newStats(w, pid), F = w.opt.f;
  const R = rng(w.opt.seed * 31 + pid.charCodeAt(0) * 7 + pid.charCodeAt(1) * 3);
  const res = w.msgs.map(m => ({ m, ok: false, late: false, time: null, cause: null }));
  const pend = [];
  const TICK = 30;
  let mi = 0;
  const server = () => w.nodes.find(n => n.server && !n.off);
  for (let t = 0; t <= w.duration; t += TICK) {
    w.setTime(t);
    if (pid === "rn" && t === 0) announceAll(w, st, R);
    for (const q of pend) {
      if (q.done || t < q.next) continue;
      const m = q.m, r0 = res[m.k];
      if (t > q.expire) { q.done = true; continue; }
      if (w.nodes[m.d].off) continue;
      st.cur = m;
      let ok = false, extra = 0;
      if (q.kind === "carry") {
        const relay = j => w.nodes[j].role === "rep" || (w.nodes[j].role === "cli" && !w.nodes[j].car);
        for (const c of q.carriers) {
          if (w.nodes[c].off) continue;
          const p = bestPath(w, c, m.d, relay, 4);
          if (!p) continue;
          const r = sendPath(w, st, p, { factor: 1, acked: true, tries: 3 }, R);
          extra += r.time;
          if (r.ok) { ok = true; break; }
        }
        if (ok) { for (const c of q.carriers) spend(w, st, c, .3); st.saved.carry++; } // the "delivered" notice
        q.next = t + TICK;
      } else {
        const holder = q.holder;
        if (holder == null || w.nodes[holder].off) { q.next = t + 300; continue; }
        if (q.kind === "sf") { // Meshtastic: the user asks the Store & Forward router for history
          const f = flood(w, st, holder, { fwd: () => true, suppress: j => w.nodes[j].role === "cli", hopLimit: w.opt.hops, factor: 1, dst: -1 }, R);
          ok = f.got.has(m.d);
        } else {
          const r = routeTo(w, st, pid, holder, m.d, R);
          ok = r.ok; extra = r.time;
          if (ok && pid === "ma") st.saved.mailbox++;
        }
        q.next = t + q.every;
      }
      if (ok) { q.done = true; r0.ok = true; r0.late = true; r0.time = t - m.t + extra; st.saved.late++; }
    }
    for (; mi < w.msgs.length && w.msgs[mi].t < t + TICK; mi++) {
      const m = w.msgs[mi], r0 = res[m.k];
      st.cur = m; st.dutyHit = false; st.lastFlood = null;
      if (w.nodes[m.d].off) {
        r0.cause = "offline";
        const srv = w.opt.server ? server() : null;
        if (pid === "ma" && F.mailbox) {
          const h = mailboxHolder(w, m.s);
          if (h != null) {
            const r = h === m.s ? { ok: true } : routeTo(w, st, pid, m.s, h, R);
            if (r.ok) pend.push({ m, kind: "box", holder: h, next: t + TICK, every: 60, expire: m.t + 7 * 86400 });
          }
        } else if (pid === "rn" && srv) {
          const r = routeTo(w, st, pid, m.s, srv.id, R);
          if (r.ok) pend.push({ m, kind: "prop", holder: srv.id, next: t + R() * 1800, every: 1800, expire: m.t + 86400 });
        } else if (pid === "mt") {
          const f = flood(w, st, m.s, { fwd: () => true, suppress: j => w.nodes[j].role === "cli", hopLimit: w.opt.hops, factor: 1, dst: -1, bridge: "all" }, R);
          if (srv && f.got.has(srv.id)) pend.push({ m, kind: "sf", holder: srv.id, next: t + R() * 3600, every: 3600, expire: m.t + 86400 });
        } else {
          // Resent a few times while nobody answers, then given up.
          PROTO[pid](w, st, m, R);
        }
        continue;
      }
      const r = PROTO[pid](w, st, m, R);
      if (r.ok) { r0.ok = true; r0.time = r.time; continue; }
      r0.cause = r.cause;
      if (pid === "ma") {
        const h = F.mailbox ? mailboxHolder(w, m.s) : null;
        if (h != null && h !== m.s && F.mailbox && w.opt.server !== false && (w.nodes[h].gw || w.nodes[h].server)) {
          const rr = routeTo(w, st, pid, m.s, h, R);
          if (rr.ok) { pend.push({ m, kind: "box", holder: h, next: t + 60, every: 60, expire: m.t + 7 * 86400 }); continue; }
        }
        if (F.carry && (w.sc.mobile || w.movers.length)) {
          const got = st.lastFlood ? [...st.lastFlood.keys()] : [];
          const carriers = [m.s, ...got.filter(j => j !== m.s && j !== m.d && w.nodes[j].role === "cli" && !w.nodes[j].gw).slice(0, 3)];
          for (const c of carriers.slice(1)) spend(w, st, m.s, 1); // hand out the copies
          pend.push({ m, kind: "carry", carriers, next: t + TICK, every: TICK, expire: m.t + 1800 });
        }
      }
    }
  }
  return summarise(w, st, res, loss);
}

function summarise(w, st, res, loss) {
  const causes = {}; for (const k in CAUSES) causes[k] = 0;
  let onTime = 0, late = 0;
  const lat = [];
  for (const r of res) {
    if (r.ok) { r.late ? late++ : onTime++; lat.push(r.time); }
    else causes[r.cause || "noise"]++;
  }
  lat.sort((a, b) => a - b);
  const pct = q => lat.length ? lat[Math.min(lat.length - 1, Math.floor(lat.length * q))] : NaN;
  const n = Math.max(1, res.length);
  return {
    pid: st.pid, n: res.length, onTime, late, delivered: (onTime + late) / n, causes,
    air: st.air, tx: st.tx, txPerMsg: st.tx / n, airPerMsg: st.air / n,
    bystanderMah: st.bystander * TX_MA / 3600, latMed: pct(.5), latP90: pct(.9),
    saved: st.saved, loss, res, annAir: st.annAir || 0,
  };
}

// Runs all four designs. A first pass measures the traffic; the second adds the losses
// a busy channel causes (ALOHA-like: the more airtime around a node, the more overlaps).
function runAll(scId, opt, custom, only) {
  const out = {};
  for (const pid of only || PIDS) {
    const w1 = new World(scId, opt, custom);
    const first = runProtocol(w1, pid, 1);
    const sc = SCENARIOS[scId];
    // Reticulum caps announce traffic to a small share of each interface, so announces
    // are queued rather than colliding: they count as airtime but not as channel load here.
    const G = (first.air - first.annAir) / sc.duration * Math.min(1, w1.avgDegree() / Math.max(1, w1.N)) * (sc.busy ? 3 : 1.5);
    const w2 = new World(scId, opt, custom);
    out[pid] = runProtocol(w2, pid, Math.exp(-2 * G));
  }
  return out;
}

// ---------- one message, traced, for the race view ----------
function race(scId, opt, custom, msgIndex, knockOut) {
  const base = new World(scId, opt, custom);
  const pool = base.msgs.filter(m => !base.offlineAt(base.nodes[m.d], m.t));
  if (!pool.length) return null;
  const at = k => pool[((k % pool.length) + pool.length) % pool.length];
  let m = at(msgIndex), ko = null;
  if (knockOut) {
    // Learn a Må:rse route, then knock out a repeater in the middle of it that leaves
    // another way round. Short routes have nothing to knock out, so look further.
    for (let tryK = 0; tryK < Math.min(pool.length, 25) && ko == null; tryK++) {
      const cand = at(msgIndex + tryK);
      const w0 = new World(scId, opt, custom); w0.setTime(cand.t);
      const st0 = newStats(w0, "ma"); st0.cur = cand;
      PROTO.ma(w0, st0, cand, rng(opt.seed * 97 + cand.k));
      const p = st0.paths.get(cand.s + ">" + cand.d) || [];
      const mids = p.slice(2, -2).filter(j => w0.nodes[j].role === "rep");
      for (const c of mids) {
        w0.nodes[c].failAt = 0; w0.setTime(cand.t, true);
        const alt = bestPath(w0, cand.s, cand.d, j => w0.nodes[j].role === "rep" || w0.nodes[j].gw, 64);
        const strong = alt && alt.every((x, i) => !i || w0.tcpLink(alt[i - 1], x) || w0.q(alt[i - 1], x) > .5);
        w0.nodes[c].failAt = null; w0.setTime(cand.t, true);
        if (strong) { ko = c; m = cand; break; }
      }
    }
  }
  const out = { m, ko, lanes: {} };
  for (const pid of PIDS) {
    const w = new World(scId, opt, custom);
    w.setTime(m.t);
    const st = newStats(w, pid); st.cur = m;
    const R = rng(opt.seed * 131 + m.k * 17 + pid.charCodeAt(1));
    if (pid === "rn") announceAll(w, st, R);
    if (knockOut) PROTO[pid](w, st, m, R); // first message: everyone learns the route
    if (ko != null) { w.nodes[ko].failAt = 0; w.setTime(m.t, true); }
    const air0 = st.air, tx0 = st.tx, tr = [];
    const r = PROTO[pid](w, st, m, R, tr);
    out.lanes[pid] = { steps: tr, ok: r.ok, cause: r.cause, air: st.air - air0, tx: st.tx - tx0, time: r.time };
  }
  out.world = base;
  base.setTime(m.t, true);
  if (ko != null) { base.nodes[ko].failAt = 0; base.setTime(m.t, true); }
  return out;
}

// ---------- scaling lab ----------
const LAB = {
  growth: { title: "Airtime per message as the network grows", x: "nodes in the network", xs: [20, 40, 80, 120, 160, 200], y: "airtime per message (s)",
    run(x, base) { return runAll("labgrid", { ...base, labN: x, msgs: 24, noise: .1, gw: false }); }, metric: r => r.airPerMsg },
  chain: { title: "Delivery over a long chain of repeaters", x: "radio hops from sender to recipient", xs: [1, 2, 3, 4, 5, 6, 8, 10, 12, 14], y: "delivered (%)",
    run(x, base) { return runAll("labchain", { ...base, labHops: Math.max(1, x - 1), msgs: 40, gw: false }); }, metric: r => r.delivered * 100 },
  noise: { title: "Delivery as interference rises (countryside)", x: "interference (% of packets damaged)", xs: [0, 10, 20, 30, 40, 50, 60], y: "delivered (%)",
    run(x, base) { return runAll("country", { ...base, noise: x / 100, msgs: 30, gw: false }); }, metric: r => r.delivered * 100 },
  speed: { title: "Delivery as the cars drive faster (motorway)", x: "speed (× 110 km/h)", xs: [0, .5, 1, 1.5, 2, 3], y: "delivered (%)",
    run(x, base) { return runAll("motorway", { ...base, speed: x, msgs: 30, gw: false }); }, metric: r => r.delivered * 100 },
};

const api = { rng, PIDS, NAMES, AIRTIME, CAUSES, DEFAULTS, SCENARIOS, World, runAll, runProtocol, race, LAB, PROTO, flood, sendPath, bestPath, newStats };
if (typeof module !== "undefined" && module.exports) module.exports = api;
root.MeshSim = api;
})(typeof globalThis !== "undefined" ? globalThis : this);
