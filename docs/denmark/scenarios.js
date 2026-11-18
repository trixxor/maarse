// Scenario generator and presets for the Denmark simulator.
// Pure functions: (options, land, towns) -> scenario JSON (see README.md).
// Used by app.js in the browser and by tools/denmark_sim_test.mjs in Node.
(function (root) {
  "use strict";

  function rng(seed) {
    let a = (seed >>> 0) || 1;
    return function () {
      a = (a + 0x6d2b79f5) | 0;
      let t = Math.imul(a ^ (a >>> 15), 1 | a);
      t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
      return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
    };
  }

  // ---------- geometry ----------
  function inRing(lat, lon, ring) {
    let inside = false;
    for (let i = 0, j = ring.length - 1; i < ring.length; j = i++) {
      const [xi, yi] = ring[i], [xj, yj] = ring[j];
      if ((yi > lat) !== (yj > lat) && lon < ((xj - xi) * (lat - yi)) / (yj - yi) + xi) inside = !inside;
    }
    return inside;
  }
  function landIndex(land) {
    const polys = land.features[0].geometry.coordinates.map(p => {
      const ring = p[0];
      let a = 90, b = -90, c = 180, d = -180;
      for (const [lo, la] of ring) { a = Math.min(a, la); b = Math.max(b, la); c = Math.min(c, lo); d = Math.max(d, lo); }
      return { ring, box: [a, b, c, d] };
    });
    return {
      polys,
      // Index of the land polygon (island) a point is on, or -1 at sea.
      island(lat, lon) {
        for (let k = 0; k < polys.length; k++) {
          const { ring, box } = polys[k];
          if (lat < box[0] || lat > box[1] || lon < box[2] || lon > box[3]) continue;
          if (inRing(lat, lon, ring)) return k;
        }
        return -1;
      },
    };
  }
  function km(a, b) {
    const dy = (a[0] - b[0]) * 110.574, dx = (a[1] - b[1]) * 111.32 * Math.cos(56 * Math.PI / 180);
    return Math.hypot(dx, dy);
  }
  function nearestTown(towns, lat, lon) {
    let best = towns[0], bd = 1e9;
    for (const t of towns) { const d = km([lat, lon], [t[1], t[2]]); if (d < bd) { bd = d; best = t; } }
    return best;
  }
  function offset(lat, lon, dxKm, dyKm) {
    return [lat + dyKm / 110.574, lon + dxKm / (111.32 * Math.cos(lat * Math.PI / 180))];
  }

  const DEFAULT_FEATURES = { carry: false, local_repair: true, backbone: true, regions: false, sf_slot: false, scale_timeouts: false, load_control: false, area_scope: false, dht_backbone: false, home_routing: false, persist_known: true, hold_unknown: true, auto_relay: false };

  function base(name, seed) {
    return { name, seed, sf: 9, duty_permille: 100, tx_dbm: 20, duration_s: 1800, features: Object.assign({}, DEFAULT_FEATURES), nodes: [], links: [], traffic: { direct_per_min: 6, channel_per_min: 1, local_share: 0.8, local_km: 25, start_s: 5, stop_s: 0, spammers: [], spam_per_min: 0 }, events: [], clutter: [], announce_s: 0, relay_hops: 4, auto_relay_after_s: 600, trace: true };
  }

  // ---------- Denmark generator ----------
  // opts: users (count), repeaterKm (grid spacing), internetPct, seed, masts (bool)
  function generateDenmark(opts, land, towns) {
    const o = Object.assign({ users: 600, repeaterKm: 18, internetPct: 70, seed: 1, hostMinPop: 12000 }, opts || {});
    const R = rng(o.seed);
    const L = landIndex(land);
    const sc = base("Denmark (generated)", o.seed);
    const add = (n) => { sc.nodes.push(n); return sc.nodes.length - 1; };
    const regionAt = (lat, lon) => nearestTown(towns, lat, lon)[4];
    // Repeaters: a jittered grid on land.
    const step = o.repeaterKm;
    for (let lat = 54.55; lat < 57.8; lat += step / 110.574) {
      const dLon = step / (111.32 * Math.cos(lat * Math.PI / 180));
      for (let lon = 8.0; lon < 15.25; lon += dLon) {
        const [la, lo] = offset(lat, lon, (R() - 0.5) * step * 0.5, (R() - 0.5) * step * 0.5);
        if (L.island(la, lo) < 0) continue;
        add({ kind: "repeater", lat: +la.toFixed(5), lon: +lo.toFixed(5), region: regionAt(la, lo) });
      }
    }
    // Small islands without a grid point still get one repeater at their town.
    for (const t of towns) {
      if (t[3] > 3000) continue;
      const near = sc.nodes.some(n => n.kind === "repeater" && km([n.lat, n.lon], [t[1], t[2]]) < step * 0.6);
      if (!near && L.island(t[1], t[2]) >= 0) add({ kind: "repeater", lat: t[1], lon: t[2], region: t[4], label: t[0] + " repeater" });
    }
    // Masts (anchors) in the big towns.
    for (const t of towns) if (t[3] >= 50000) add({ kind: "anchor", lat: t[1] + 0.01, lon: t[2] + 0.01, region: t[4], label: t[0] + " mast" });
    // Hosts in towns, a share with internet; TCP star to the nearest hub, hubs meshed.
    const hubs = [];
    const hosts = [];
    for (const t of towns) {
      if (t[3] < o.hostMinPop) continue;
      const internet = t[3] >= 100000 || R() * 100 < o.internetPct;
      const i = add({ kind: "host", lat: t[1] - 0.005, lon: t[2] - 0.005, region: t[4], internet, label: t[0] + " host" });
      hosts.push(i);
      if (t[3] >= 100000) hubs.push(i);
    }
    for (let a = 0; a < hubs.length; a++) for (let b = a + 1; b < hubs.length; b++) sc.links.push({ a: hubs[a], b: hubs[b], kind: "tcp", up: true });
    for (const h of hosts) {
      if (hubs.includes(h) || !sc.nodes[h].internet) continue;
      let best = hubs[0], bd = 1e9;
      for (const u of hubs) { const d = km([sc.nodes[h].lat, sc.nodes[h].lon], [sc.nodes[u].lat, sc.nodes[u].lon]); if (d < bd) { bd = d; best = u; } }
      if (best !== undefined) sc.links.push({ a: h, b: best, kind: "tcp", up: true });
    }
    // Long-distance LoRa trunks (masts with directional antennas, no
    // internet needed): Aarhus - Zealand across the Kattegat (about 65 km to
    // Sjaellands Odde) and Zealand - Bornholm across the Baltic (about 145 km
    // from Stevns). Each joins the hosts nearest its two ends.
    if (o.trunks !== false && hosts.length) {
      const nearestHost = (lat, lon, not) => {
        let best = -1, bd = 1e9;
        for (const h of hosts) { if (h === not) continue; const d = km([sc.nodes[h].lat, sc.nodes[h].lon], [lat, lon]); if (d < bd) { bd = d; best = h; } }
        return best;
      };
      const trunk = (a, b, extra) => { if (a >= 0 && b >= 0 && a !== b) sc.links.push(Object.assign({ a, b, kind: "lora", up: true }, extra)); };
      // Every trunk on the mesh's own frequency and SF (SF9): every node must
      // be able to talk to every other.
      const aarhus = nearestHost(56.15, 10.20), odde = nearestHost(55.97, 11.37, aarhus);
      trunk(aarhus, odde, { sf: 9, snr_db: 6, loss: 0.02, group: "west", label: "LoRa trunk Aarhus - Zealand" });
      const stevns = nearestHost(55.30, 12.40), roenne = nearestHost(55.10, 14.70, stevns);
      trunk(stevns, roenne, { sf: 9, snr_db: 3, loss: 0.05, group: "east", label: "LoRa trunk Zealand - Bornholm" });
      if (o.trunks === "multi") {
        // Three crossings to Zealand from the west: Skagen (Frederikshavn) -
        // North Zealand (about 120-150 km over the Kattegat), Aarhus - Holbaek
        // (above) and the Great Belt (Nyborg - Korsoer, about 18 km).
        const nh = (lat, lon, not) => {
          let best = -1, bd = 1e9;
          for (const h of hosts) { if (not.includes(h)) continue; const d = km([sc.nodes[h].lat, sc.nodes[h].lon], [lat, lon]); if (d < bd) { bd = d; best = h; } }
          return best;
        };
        const skagen = nh(57.44, 10.53, [aarhus, odde, stevns, roenne]);
        const nzeal = nh(56.03, 12.6, [aarhus, odde, stevns, roenne, skagen]);
        trunk(skagen, nzeal, { sf: 9, snr_db: 3, loss: 0.05, group: "west", label: "LoRa trunk Skagen - North Zealand" });
        const nyborg = nh(55.31, 10.79, [aarhus, odde, stevns, roenne, skagen, nzeal]);
        const korsoer = nh(55.33, 11.14, [aarhus, odde, stevns, roenne, skagen, nzeal, nyborg]);
        trunk(nyborg, korsoer, { sf: 9, snr_db: 10, loss: 0.01, group: "west", label: "LoRa trunk Great Belt" });
      }
      if (o.trunks === 2) {
        // A second trunk on each crossing, between other hosts and on its
        // own channel: Djursland (Grenaa) - Zealand, and Copenhagen - Roenne.
        const nearestNot = (lat, lon, not) => {
          let best = -1, bd = 1e9;
          for (const h of hosts) { if (not.includes(h)) continue; const d = km([sc.nodes[h].lat, sc.nodes[h].lon], [lat, lon]); if (d < bd) { bd = d; best = h; } }
          return best;
        };
        const grenaa = nearestNot(56.41, 10.88, [aarhus, odde]);
        const odde2 = nearestNot(55.97, 11.37, [aarhus, odde, grenaa]);
        trunk(grenaa, odde2, { sf: 8, snr_db: 6, loss: 0.02, label: "LoRa trunk Djursland - Zealand" });
        const cph = nearestNot(55.68, 12.57, [stevns, roenne]);
        trunk(cph, roenne, { sf: 9, snr_db: 3, loss: 0.05, label: "LoRa trunk Copenhagen - Bornholm" });
      }
    }
    // Optional twin hosts: a second host next to every host with the same
    // TCP links (redundancy when one PC or its internet fails).
    if (o.hostPairs) {
      const links = sc.links.slice();
      for (const h of hosts) {
        const n = sc.nodes[h];
        const t = add({ kind: "host", lat: n.lat + 0.008, lon: n.lon + 0.008, region: n.region, internet: n.internet, label: n.label + " (twin)" });
        sc.links.push({ a: h, b: t, kind: "tcp", up: true });
        for (const l of links) if (l.a === h || l.b === h) sc.links.push({ a: t, b: l.a === h ? l.b : l.a, kind: "tcp", up: true });
      }
    }
    // Users, weighted by population^0.8, scattered around their town on land.
    // With `o.regions` ({region: {users, lora, both, app, cars, relays}} in
    // per cent), each region gets its own count and connection mix:
    //   lora = handheld radio only, both = handheld + internet to its nearest
    //   host, app = phone with the app only (no radio, TCP to its host).
    const netHosts = hosts.filter(h => sc.nodes[h].internet);
    const nearestNetHost = (lat, lon) => {
      let best = -1, bd = 1e9;
      for (const h of netHosts) { const d = km([sc.nodes[h].lat, sc.nodes[h].lon], [lat, lon]); if (d < bd) { bd = d; best = h; } }
      return best;
    };
    const placeAround = (t) => {
      const spread = Math.min(12, 1 + Math.sqrt(t[3]) / 60);
      for (let tries = 0; tries < 10; tries++) {
        const ang = R() * Math.PI * 2, d = Math.sqrt(R()) * spread;
        const [la, lo] = offset(t[1], t[2], Math.cos(ang) * d, Math.sin(ang) * d);
        if (L.island(la, lo) < 0 && tries < 9) continue;
        return [+la.toFixed(5), +lo.toFixed(5)];
      }
      return [t[1], t[2]];
    };
    const pickTown = (list) => {
      const w = list.map(t => Math.pow(t[3], 0.8));
      let r = R() * w.reduce((a, b) => a + b, 0);
      for (let i = 0; i < list.length; i++) { r -= w[i]; if (r <= 0) return list[i]; }
      return list[0];
    };
    const plan = o.regions
      ? Object.entries(o.regions).map(([region, m]) => ({ region, towns: towns.filter(t => t[4] === region), ...m }))
      : [{ region: null, towns, users: o.users, lora: 100, both: 0, app: 0, cars: 0, relays: 0 }];
    let total = 0;
    for (const g of plan) {
      if (!g.towns.length) continue;
      for (let k = 0; k < (g.users || 0); k++) {
        const t = pickTown(g.towns);
        const [la, lo] = placeAround(t);
        const r = R() * 100;
        const cut1 = g.relays || 0, cut2 = cut1 + (g.cars || 0);
        if (r < cut1) { add({ kind: "relay", lat: la, lon: lo, region: t[4] }); total++; continue; }
        if (r < cut2) {
          // A car driving between this town and another one in the region.
          const t2 = pickTown(g.towns);
          const end = placeAround(t2);
          add({ kind: "mobile", lat: la, lon: lo, region: t[4], route: [[la, lo], end], speed_kmh: 70, route_end: "back", label: "Car" });
          total++; continue;
        }
        const m = R() * ((g.lora || 0) + (g.both || 0) + (g.app || 0) || 1);
        const kind = m < (g.lora || 0) ? "lora" : m < (g.lora || 0) + (g.both || 0) ? "both" : "app";
        const h = kind === "lora" ? -1 : nearestNetHost(la, lo);
        if (kind !== "lora" && h < 0) { add({ kind: "handheld", lat: la, lon: lo, region: t[4] }); total++; continue; }
        const i = add(kind === "app" ? { kind: "app", lat: la, lon: lo, region: t[4], internet: true } : { kind: "handheld", lat: la, lon: lo, region: t[4], internet: kind === "both" });
        if (h >= 0) sc.links.push({ a: i, b: h, kind: "tcp", up: true });
        total++;
      }
    }
    sc.traffic.direct_per_min = Math.max(2, Math.round(total / 60));
    return sc;
  }

  // ---------- presets ----------
  const E45 = [[57.441, 10.537], [57.048, 9.919], [56.643, 9.791], [56.461, 10.036], [56.157, 10.211], [55.861, 9.850], [55.709, 9.536], [55.490, 9.472], [55.249, 9.488], [54.830, 9.360]];

  function polyline(points, everyKm) {
    const out = [];
    for (let i = 0; i < points.length - 1; i++) {
      const a = points[i], b = points[i + 1], d = km(a, b), n = Math.max(1, Math.round(d / everyKm));
      for (let k = 0; k < n; k++) out.push([a[0] + (b[0] - a[0]) * k / n, a[1] + (b[1] - a[1]) * k / n]);
    }
    out.push(points[points.length - 1]);
    return out;
  }

  function hostsIn(sc, test) { return sc.nodes.map((n, i) => [n, i]).filter(([n]) => n.kind === "host" && test(n)).map(([, i]) => i); }
  function firstHost(sc, label) { const i = sc.nodes.findIndex(n => n.kind === "host" && (n.label || "").startsWith(label)); return i < 0 ? sc.nodes.findIndex(n => n.kind === "host") : i; }

  const WEST_COAST = [[57.10, 8.20], [57.10, 8.75], [56.40, 8.50], [55.95, 8.40], [55.30, 8.75], [55.30, 8.30], [55.95, 7.95], [56.60, 8.00]];
  const BORNHOLM = [[55.32, 14.60], [55.32, 15.25], [54.95, 15.25], [54.95, 14.60]];

  const PRESETS = {
    normal: {
      title: "National normal day",
      text: "Generated Denmark: repeaters every ~18 km, masts in big towns, hosts with internet linked over TCP. Ordinary chat traffic.",
      build: (land, towns, seed) => { const s = generateDenmark({ seed }, land, towns); s.name = "National normal day"; return s; },
    },
    internet_loss: {
      title: "Total internet loss",
      text: "Normal day, then at 2 min all TCP backbone links go down. Islands that only talked over the internet are cut off.",
      build: (land, towns, seed) => {
        const s = generateDenmark({ seed }, land, towns); s.name = "Total internet loss";
        s.events.push({ t_s: 120, type: "internet_down" });
        return s;
      },
    },
    bornholm: {
      title: "Bornholm cut off",
      text: "Bornholm's backbone links fail at 1 min. At 3 min an authority on Copenhagen sends a water alert to Bornholm: does it get there?",
      build: (land, towns, seed) => {
        const s = generateDenmark({ seed }, land, towns); s.name = "Bornholm cut off";
        const bh = new Set(hostsIn(s, n => n.lon > 14.5));
        s.links.forEach((l, k) => { if (bh.has(l.a) || bh.has(l.b)) s.events.push({ t_s: 60, type: "link", index: k, up: false }); });
        s.events.push({ t_s: 180, type: "alert", from: firstHost(s, "Copenhagen"), alert: "water", area: { polygon: BORNHOLM }, text: "Boil drinking water" });
        return s;
      },
    },
    storm: {
      title: "Storm surge, west coast",
      text: "At 2 min a power cut hits the west coast (repeaters fall back to battery, hosts lose internet); at 2.5 min a flood alert goes to the same area.",
      build: (land, towns, seed) => {
        const s = generateDenmark({ seed }, land, towns); s.name = "Storm surge on the west coast";
        s.events.push({ t_s: 120, type: "power_cut", area: { polygon: WEST_COAST } });
        s.events.push({ t_s: 150, type: "alert", from: firstHost(s, "Aarhus"), alert: "flood", area: { polygon: WEST_COAST }, text: "Storm surge: move inland" });
        return s;
      },
    },
    e45: {
      title: "E45 drive, repeaters every 15 km",
      text: "Like maarse-sim --highway: repeaters every 15 km along the E45, a car drives south at 110 km/h and a base in Frederikshavn writes to it. Try carry on/off.",
      build: (land, towns, seed) => {
        const s = base("E45 drive", seed);
        for (const [la, lo] of polyline(E45, 15)) s.nodes.push({ kind: "repeater", lat: la + 0.003, lon: lo + 0.003, region: "dk" });
        const baseI = s.nodes.push({ kind: "handheld", lat: 57.445, lon: 10.53, label: "Base (Frederikshavn)" }) - 1;
        s.nodes.push({ kind: "mobile", lat: E45[0][0], lon: E45[0][1], route: E45, speed_kmh: 110, label: "Car" });
        s.traffic = { direct_per_min: 2, channel_per_min: 0, local_share: 0, local_km: 1, start_s: 5, stop_s: 0, spammers: [], spam_per_min: 0 };
        s.duration_s = 3600;
        s.features.carry = true;
        s.baseIndex = baseI;
        return s;
      },
    },
    forest: {
      title: "Group in the forest (Rold Skov)",
      text: "Like maarse-sim --forest/--hill: walkers with relays wander in Rold Skov (forest loss per km inside the trees), a repeater 12 km away by the base, one relay on a hill. Change it on the Terrain tab.",
      build: (land, towns, seed) => forest({ seed }),
    },
    festival: {
      title: "Dense city festival (Roskilde)",
      text: "400 handhelds on a festival site, four repeaters, a host. Lots of channel traffic: watch collisions and duty-cycle hits.",
      build: (land, towns, seed) => {
        const s = base("Roskilde Festival", seed);
        const R = rng(seed), c = [55.617, 12.078];
        for (let k = 0; k < 4; k++) s.nodes.push({ kind: "repeater", lat: c[0] + (k % 2 - 0.5) * 0.01, lon: c[1] + ((k >> 1) - 0.5) * 0.02 });
        s.nodes.push({ kind: "host", lat: 55.642, lon: 12.08, internet: true, label: "Roskilde host" });
        for (let k = 0; k < 400; k++) s.nodes.push({ kind: "handheld", lat: c[0] + (R() - 0.5) * 0.012, lon: c[1] + (R() - 0.5) * 0.02 });
        s.traffic = { direct_per_min: 40, channel_per_min: 10, local_share: 1, local_km: 3, start_s: 5, stop_s: 0, spammers: [], spam_per_min: 0 };
        s.duration_s = 900;
        return s;
      },
    },
    reboot: {
      title: "Restart: 40 % of handhelds reboot",
      text: "Normal day; every node announces every 12 h (at random moments). Rebooted nodes announce after 5 s. At 3 min 40 % of the handhelds restart (battery swap, power back). Turn off \"contacts survive reboot\" and \"keep messages from unknown senders\" to see the firmware before 2026-11-15: messages acknowledged but never shown.",
      build: (land, towns, seed) => {
        const s = generateDenmark({ seed }, land, towns); s.name = "Restart after a reboot";
        s.announce_s = 12 * 3600;
        const hh = s.nodes.filter(n => n.kind === "handheld").length;
        s.events.push({ t_s: 180, type: "reboot", n: Math.round(hh * 0.4), node_kind: "handheld" });
        s.duration_s = 1200;
        return s;
      },
    },
    spam: {
      title: "Spam / flood attack",
      text: "Normal day; from 2 min three handhelds in Odense flood the public channel at 30 messages/min each. See queue delays and what happens to direct messages.",
      build: (land, towns, seed) => {
        const s = generateDenmark({ seed }, land, towns); s.name = "Spam attack";
        const od = s.nodes.map((n, i) => [n, i]).filter(([n]) => n.kind === "handheld" && km([n.lat, n.lon], [55.403, 10.402]) < 8).slice(0, 3).map(([, i]) => i);
        const t = Object.assign({}, s.traffic, { spammers: od, spam_per_min: 30 });
        s.events.push({ t_s: 120, type: "set_traffic", traffic: t });
        return s;
      },
    },
  };

  // The forest scenario with options (Terrain tab):
  // walkers, role (relay|handheld), radiusKm, dbPerKm, hill (bool), hillH (m),
  // walkKmh, repeaterKm (distance from the forest centre to the repeater).
  const FOREST_DEFAULTS = { walkers: 10, role: "relay", radiusKm: 4, dbPerKm: 10, hill: true, hillH: 20, walkKmh: 5, repeaterKm: 12, seed: 1 };
  function forest(opts) {
    const o = Object.assign({}, FOREST_DEFAULTS, opts || {});
    const s = base("Group in the forest", o.seed);
    const c = [56.80, 9.85];
    // The forest: an irregular ring of points around the centre.
    const R = rng(o.seed + 99), ring = [];
    for (let k = 0; k < 14; k++) {
      const a = k / 14 * Math.PI * 2, r = (o.radiusKm + 1.5) * (0.85 + R() * 0.3);
      ring.push(offset(c[0], c[1], Math.cos(a) * r, Math.sin(a) * r));
    }
    s.clutter = [{ label: "Rold Skov", polygon: ring, db_per_km: o.dbPerKm }];
    const [rl, ro] = offset(c[0], c[1], o.repeaterKm, 0);
    s.nodes.push({ kind: "repeater", lat: rl, lon: ro, label: "Repeater " + o.repeaterKm + " km away" });
    s.nodes.push({ kind: "handheld", lat: rl + 0.002, lon: ro + 0.005, label: "Base" });
    if (o.hill) s.nodes.push({ kind: "relay", lat: c[0] + 0.01, lon: c[1] + 0.02, h: o.hillH, label: "Relay on the hill" });
    const W = rng(o.seed);
    for (let k = 0; k < o.walkers; k++) {
      const [la, lo] = offset(c[0], c[1], (W() - 0.5) * o.radiusKm, (W() - 0.5) * o.radiusKm);
      s.nodes.push({ kind: o.role, lat: +la.toFixed(5), lon: +lo.toFixed(5), wander_m: o.radiusKm * 1000 / 2, speed_kmh: o.walkKmh, label: "Walker " + (k + 1) });
    }
    s.traffic = { direct_per_min: 3, channel_per_min: 0.5, local_share: 1, local_km: 30, start_s: 5, stop_s: 0, spammers: [], spam_per_min: 0 };
    s.forest = o;
    return s;
  }

  // A town with patchy coverage (Aarhus): handhelds spread over the town,
  // town clutter at street level, and repeaters on one side only, so part
  // of the town has no repeater in range.
  // opts: users, radiusKm, repeaters, dbPerKm, role (handheld|relay), seed.
  const CITY_DEFAULTS = { users: 300, radiusKm: 6, repeaters: 3, dbPerKm: 6, role: "handheld", seed: 1 };
  function city(opts) {
    const o = Object.assign({}, CITY_DEFAULTS, opts || {});
    const s = base("Town with patchy coverage", o.seed);
    const c = [56.157, 10.211];
    const R = rng(o.seed), ring = [];
    for (let k = 0; k < 16; k++) {
      const ang = k / 16 * Math.PI * 2, r = (o.radiusKm + 1) * (0.9 + R() * 0.2);
      ring.push(offset(c[0], c[1], Math.cos(ang) * r, Math.sin(ang) * r));
    }
    s.clutter = [{ label: "Aarhus (buildings)", polygon: ring, db_per_km: o.dbPerKm, canopy_m: 12 }];
    // Repeaters only in the western half.
    for (let k = 0; k < o.repeaters; k++) {
      const ang = Math.PI * (0.6 + 0.8 * k / Math.max(1, o.repeaters - 1)), r = o.radiusKm * 0.5;
      const [la, lo] = offset(c[0], c[1], Math.cos(ang) * r, Math.sin(ang) * r);
      s.nodes.push({ kind: "repeater", lat: +la.toFixed(5), lon: +lo.toFixed(5), label: "Repeater " + (k + 1) });
    }
    s.nodes.push({ kind: "host", lat: c[0] + 0.01, lon: c[1] - 0.03, internet: false, label: "Aarhus host" });
    for (let k = 0; k < o.users; k++) {
      const ang = R() * Math.PI * 2, d = Math.sqrt(R()) * o.radiusKm;
      const [la, lo] = offset(c[0], c[1], Math.cos(ang) * d, Math.sin(ang) * d);
      s.nodes.push({ kind: o.role, lat: +la.toFixed(5), lon: +lo.toFixed(5) });
    }
    s.traffic = { direct_per_min: 20, channel_per_min: 2, local_share: 1, local_km: 4, start_s: 5, stop_s: 0, spammers: [], spam_per_min: 0 };
    s.duration_s = 2700;
    return s;
  }
  PRESETS.city = {
    title: "Town with patchy coverage (Aarhus)",
    text: "300 handhelds in Aarhus, town clutter at street level, three repeaters in the western half only. Try automatic forest mode (Build, Features) and the relay hop limit.",
    build: (land, towns, seed) => city({ seed }),
  };

  const api = { rng, landIndex, generateDenmark, PRESETS, km, offset, nearestTown, DEFAULT_FEATURES, FOREST_DEFAULTS, forest, CITY_DEFAULTS, city, base };
  if (typeof module !== "undefined" && module.exports) module.exports = api;
  else root.DkScenarios = api;
})(typeof window !== "undefined" ? window : globalThis);
