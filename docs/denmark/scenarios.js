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

  const DEFAULT_FEATURES = { carry: false, local_repair: true, backbone: true, regions: false, sf_slot: false, scale_timeouts: false, load_control: false, area_scope: false };

  function base(name, seed) {
    return { name, seed, sf: 9, duty_permille: 100, tx_dbm: 20, duration_s: 1800, features: Object.assign({}, DEFAULT_FEATURES), nodes: [], links: [], traffic: { direct_per_min: 6, channel_per_min: 1, local_share: 0.8, local_km: 25, start_s: 5, stop_s: 0, spammers: [], spam_per_min: 0 }, events: [], trace: true };
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
    // Users, weighted by population^0.8, scattered around their town on land.
    const w = towns.map(t => Math.pow(t[3], 0.8));
    const sum = w.reduce((a, b) => a + b, 0);
    for (let k = 0; k < o.users; k++) {
      let r = R() * sum, t = towns[0];
      for (let i = 0; i < towns.length; i++) { r -= w[i]; if (r <= 0) { t = towns[i]; break; } }
      const spread = Math.min(12, 1 + Math.sqrt(t[3]) / 60);
      for (let tries = 0; tries < 10; tries++) {
        const ang = R() * Math.PI * 2, d = Math.sqrt(R()) * spread;
        const [la, lo] = offset(t[1], t[2], Math.cos(ang) * d, Math.sin(ang) * d);
        if (L.island(la, lo) < 0 && tries < 9) continue;
        add({ kind: "handheld", lat: +la.toFixed(5), lon: +lo.toFixed(5), region: t[4] });
        break;
      }
    }
    sc.traffic.direct_per_min = Math.max(2, Math.round(o.users / 60));
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
      text: "Like maarse-sim --forest/--hill: ten walkers with relays in Rold Skov, a repeater 12 km away by the base, one relay on a hill (20 m antenna). Clients are relays here.",
      build: (land, towns, seed) => {
        const s = base("Group in the forest", seed);
        const R = rng(seed), c = [56.80, 9.85];
        s.nodes.push({ kind: "repeater", lat: 56.80, lon: 10.05, label: "Repeater 12 km away" });
        s.nodes.push({ kind: "handheld", lat: 56.802, lon: 10.055, label: "Base" });
        s.nodes.push({ kind: "relay", lat: c[0], lon: c[1], h: 20, label: "Relay on the hill" });
        for (let k = 0; k < 10; k++) {
          const route = [];
          for (let j = 0; j < 6; j++) route.push(offset(c[0], c[1], (R() - 0.5) * 8, (R() - 0.5) * 8));
          s.nodes.push({ kind: "relay", lat: route[0][0], lon: route[0][1], route, speed_kmh: 5, label: "Walker " + (k + 1) });
        }
        s.traffic = { direct_per_min: 3, channel_per_min: 0.5, local_share: 1, local_km: 30, start_s: 5, stop_s: 0, spammers: [], spam_per_min: 0 };
        return s;
      },
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

  const api = { rng, landIndex, generateDenmark, PRESETS, km, offset, nearestTown, DEFAULT_FEATURES, base };
  if (typeof module !== "undefined" && module.exports) module.exports = api;
  else root.DkScenarios = api;
})(typeof window !== "undefined" ? window : globalThis);
