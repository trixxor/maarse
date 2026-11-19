// Coverage layer for the Denmark simulator: how far radios reach over the
// real ground. A planning picture, not a simulation result.
//
// The same radio model as the simulation engine (maarse-sim radio.rs and
// maarse-sim-wasm terrain.rs), so the picture and the run agree:
// * path loss 31.2 dB at 1 m + 30 log10(d), the calibrated log-distance law,
// * height gain 10 log10(h / 1.5 m) per end, the transmitter counted from
//   the mean ground near it (a mast on a hill stands taller),
// * hills: knife-edge loss beyond a grazing edge's 6 dB over the highest
//   hill as seen from the transmitter, with the ground taken relative to the
//   straight line between the two ends' ground, as the engine does,
// * nothing beyond the radio horizon (4/3 earth) of the two antennas,
// * receiver: noise floor -117 dBm at 125 kHz, minimum SNR per SF.
// Heights: the engine's 250 m grid of Denmark (dk-dem.bin), so a whole
// network is drawn in a second or two. Several sites show the best of
// them, on land only, from light (weak) to dark (strong).
(function () {
  const dk = window.DKSim;
  if (!dk || !dk.map) return;
  const map = dk.map;
  const $ = (id) => document.getElementById(id);
  const FREQ_MHZ = 869.525, LAMBDA = 299.792458 / FREQ_MHZ;
  const NOISE = -174 + 10 * Math.log10(125000) + 6;
  const SNR_MIN = { 7: -7.5, 8: -10, 9: -12.5, 10: -15, 11: -17.5, 12: -20 };
  const KIND_GAIN = { repeater: 6, anchor: 6, host: 6, wifi: 6 };   // engine: fixed stations 6 dB
  const heightDb = (h) => 10 * Math.log10(Math.max(1, h) / 1.5);
  const esc = (t) => String(t ?? "").replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);

  // ---------- the engine's height grid ----------
  let grid = null;
  async function loadGrid() {
    if (grid) return grid;
    const [meta, bin] = await Promise.all([fetch("dk-dem.json").then((r) => r.json()), fetch("dk-dem.bin").then((r) => r.blob())]);
    const z = new Uint8Array(await new Response(bin.stream().pipeThrough(new DecompressionStream("deflate"))).arrayBuffer());
    grid = Object.assign({ z }, meta);
    return grid;
  }
  function groundAt(lat, lon) {
    const g = grid, fy = (lat - g.lat0) / g.dlat, fx = (lon - g.lon0) / g.dlon;
    if (fx < 0 || fy < 0 || fx >= g.w - 1 || fy >= g.h - 1) return 0;
    const x = fx | 0, y = fy | 0, tx = fx - x, ty = fy - y, w = g.w, z = g.z;
    const a = z[y * w + x] * (1 - tx) + z[y * w + x + 1] * tx, b = z[(y + 1) * w + x] * (1 - tx) + z[(y + 1) * w + x + 1] * tx;
    return a * (1 - ty) + b * ty;
  }

  // ---------- settings and sites ----------
  const S = { sf: 9, hr: 1.5, extra: 0, radius: 30, ht: 15, gain: 6, power: 20, cal: 0 };   // cal: correction from real measurements (dB)
  const sites = [];   // { id, ll, h, gain, name, node, marker, rays }
  let nextId = 1;

  // One site: 360 rays; margin (dB over sensitivity) every 250 m.
  const RAYS = 360, STEP = 250;
  function computeSite(s) {
    const R = S.radius * 1000, NS = Math.ceil(R / STEP);
    const mLat = 111320, mLon = 111320 * Math.cos(s.ll.lat * Math.PI / 180);
    const budget = (s.tx ?? S.power) + s.gain - S.extra + S.cal + heightDb(S.hr) - (NOISE + SNR_MIN[S.sf]);
    const g0 = groundAt(s.ll.lat, s.ll.lng), g = new Float32Array(NS + 1), top = g0 + s.h;
    s.rays = [];
    for (let r = 0; r < RAYS; r++) {
      const az = r * 2 * Math.PI / RAYS, dx = Math.sin(az) * STEP / mLon, dy = Math.cos(az) * STEP / mLat;
      g[0] = g0;
      for (let k = 1; k <= NS; k++) g[k] = groundAt(s.ll.lat + dy * k, s.ll.lng + dx * k);
      const near = Math.min(NS, 60);   // 15 km
      let sum = 0; for (let k = 1; k <= near; k++) sum += g[k];
      const hEff = s.h + Math.max(0, g0 - sum / near);
      const gain = heightDb(hEff), horizon = 4120 * (Math.sqrt(Math.max(0.5, hEff)) + Math.sqrt(Math.max(0.5, S.hr)));
      const out = new Float32Array(NS + 1);
      out[0] = 99;
      // The highest obstacle as seen from the transmitter (largest
      // elevation angle) is the one to bend over; kept while walking out.
      let bestI = 0, bestSlope = -Infinity;
      for (let k = 1; k <= NS; k++) {
        const d = k * STEP;
        if (d > horizon) { out[k] = -99; continue; }
        let hills = 0;
        if (bestI > 0) {
          const d1 = bestI * STEP, d2 = d - d1;
          const rel = g[bestI] - (g0 + (g[k] - g0) * d1 / d);
          const h = rel - (s.h + (S.hr - s.h) * d1 / d);
          const v = h * Math.sqrt(2 * d / (LAMBDA * d1 * d2));
          if (v > 0) hills = 20 * Math.log10(Math.sqrt((v - 0.1) ** 2 + 1) + v - 0.1) + 0.9;
        }
        out[k] = budget + gain - (31.2 + 30 * Math.log10(d)) - hills;
        const slope = (g[k] - top) / d;
        if (slope > bestSlope) { bestSlope = slope; bestI = k; }
      }
      s.rays.push(out);
    }
    s.ns = NS;
  }

  // ---------- drawing: the best margin of all sites, on land ----------
  let overlay = null, visible = true;
  // Margin (dB over sensitivity) -> colour, light = weak, dark = strong.
  const RAMP = [[20, [15, 82, 186]], [10, [37, 125, 230]], [3, [96, 165, 250]], [0, [165, 205, 252]]];
  function landMask(W, H, w0, n0, cellLon, cellLat) {
    const c = document.createElement("canvas"); c.width = W; c.height = H;
    const g = c.getContext("2d");
    g.fillStyle = "#000";
    const ring = (pts) => { pts.forEach(([lon, lat], i) => { const x = (lon - w0) / cellLon, y = (n0 - lat) / cellLat; if (i) g.lineTo(x, y); else g.moveTo(x, y); }); g.closePath(); };
    for (const f of (window.DK_LAND && window.DK_LAND.features) || []) {
      const geo = f.geometry; if (!geo) continue;
      const polys = geo.type === "Polygon" ? [geo.coordinates] : geo.type === "MultiPolygon" ? geo.coordinates : [];
      for (const p of polys) { g.beginPath(); p.forEach(ring); g.fill("evenodd"); }
    }
    return g.getImageData(0, 0, W, H).data;
  }
  function draw() {
    if (overlay) { map.removeLayer(overlay); overlay = null; }
    if (!sites.length) { stats(); return; }
    const R = S.radius * 1000;
    let s0 = 90, n0 = -90, w0 = 180, e0 = -180;
    for (const s of sites) {
      const mLon = 111320 * Math.cos(s.ll.lat * Math.PI / 180);
      s0 = Math.min(s0, s.ll.lat - R / 111320); n0 = Math.max(n0, s.ll.lat + R / 111320);
      w0 = Math.min(w0, s.ll.lng - R / mLon); e0 = Math.max(e0, s.ll.lng + R / mLon);
    }
    const midLat = (s0 + n0) / 2, mLonMid = 111320 * Math.cos(midLat * Math.PI / 180);
    const cell = Math.max(80, Math.max((e0 - w0) * mLonMid, (n0 - s0) * 111320) / 1800);   // metres per pixel
    const cellLat = cell / 111320, cellLon = cell / mLonMid;
    const W = Math.ceil((e0 - w0) / cellLon), H = Math.ceil((n0 - s0) / cellLat);
    const best = new Float32Array(W * H).fill(-1e9);
    for (const s of sites) {
      const mLon = 111320 * Math.cos(s.ll.lat * Math.PI / 180);
      const cx = (s.ll.lng - w0) / cellLon, cy = (n0 - s.ll.lat) / cellLat;
      const rx = R / mLon / cellLon, ry = R / 111320 / cellLat;
      for (let y = Math.max(0, Math.floor(cy - ry)); y < Math.min(H, Math.ceil(cy + ry)); y++) {
        const ny = (cy - y - 0.5) * cellLat * 111320;
        for (let x = Math.max(0, Math.floor(cx - rx)); x < Math.min(W, Math.ceil(cx + rx)); x++) {
          const ex = (x + 0.5 - cx) * cellLon * mLon, d = Math.hypot(ex, ny);
          if (d > R) continue;
          let az = Math.atan2(ex, ny); if (az < 0) az += 2 * Math.PI;
          const m = s.rays[Math.round(az / (2 * Math.PI) * RAYS) % RAYS][Math.min(s.ns, Math.round(d / STEP))];
          const i = y * W + x;
          if (m > best[i]) best[i] = m;
        }
      }
    }
    const land = landMask(W, H, w0, n0, cellLon, cellLat);
    const c = document.createElement("canvas"); c.width = W; c.height = H;
    const g2 = c.getContext("2d"), img = g2.createImageData(W, H), o = img.data;
    let covered = 0;
    for (let i = 0; i < best.length; i++) {
      const m = best[i];
      if (m < 0) continue;
      // Land: the coastline, or ground above the sea (neighbouring countries).
      if (!land[i * 4 + 3]) {
        const lat = n0 - (Math.floor(i / W) + 0.5) * cellLat, lon = w0 + (i % W + 0.5) * cellLon;
        if (groundAt(lat, lon) <= 0) continue;
      }
      let col = RAMP[RAMP.length - 1][1];
      for (const [lim, cc] of RAMP) if (m >= lim) { col = cc; break; }
      o[i * 4] = col[0]; o[i * 4 + 1] = col[1]; o[i * 4 + 2] = col[2]; o[i * 4 + 3] = 255;
      covered++;
    }
    g2.putImageData(img, 0, 0);
    overlay = L.imageOverlay(c.toDataURL(), [[s0, w0], [n0, e0]], { opacity: +$("cv-op").value, interactive: false, pane: "heightPane" });
    if (visible) overlay.addTo(map);
    stats(covered * cell * cell / 1e6);
  }
  function stats(km2) {
    $("cv-sum").textContent = sites.length ? `${sites.length} site${sites.length > 1 ? "s" : ""}: about ${Math.round(km2 || 0).toLocaleString("en")} km² of land reached at SF${S.sf}.` : "No sites yet. Add one on the map, or all masts at once.";
    $("cv-list").innerHTML = sites.length <= 12 ? sites.map((s) => `<li style="display:flex;justify-content:space-between;gap:6px"><span>${esc(s.name)} · ${s.h} m</span><button data-x="${s.id}" title="Remove" style="border:0;background:transparent">✕</button></li>`).join("") : `<li class="hint">${sites.length} sites (too many to list)</li>`;
    $("cv-list").querySelectorAll("button[data-x]").forEach((b) => b.onclick = () => removeSite(sites.find((s) => s.id === +b.dataset.x)));
  }

  // ---------- sites on the map ----------
  const pin = L.divIcon({ className: "", iconSize: [16, 16], iconAnchor: [8, 8],
    html: '<div style="width:12px;height:12px;border-radius:50%;background:#facc15;border:2px solid #111;box-shadow:0 1px 3px rgba(0,0,0,.4);cursor:grab"></div>' });
  const siteLayer = L.layerGroup().addTo(map);
  let busy = false;
  async function withGrid(fn) {
    if (busy) return;
    busy = true; $("cv-sum").textContent = "Working…";
    try { await loadGrid(); await new Promise((r) => setTimeout(r, 20)); fn(); draw(); }
    catch (e) { $("cv-sum").textContent = "Could not load the height grid: " + e.message; }
    finally { busy = false; }
  }
  function makeSite(ll, h, gain, name, node) {
    const s = { id: nextId++, ll: L.latLng(ll), h, gain, name, node };
    if (node === undefined) {   // nodes already have a marker; points get a handle
      s.marker = L.marker(s.ll, { icon: pin, draggable: true, title: `${name}: drag to move, right-click to remove` }).addTo(siteLayer);
      s.marker.on("dragend", () => { s.ll = s.marker.getLatLng(); withGrid(() => computeSite(s)); });
      s.marker.on("contextmenu", (e) => { L.DomEvent.stop(e); removeSite(s); });
    }
    sites.push(s);
    return s;
  }
  function removeSite(s) {
    if (!s) return;
    if (s.marker) siteLayer.removeLayer(s.marker);
    sites.splice(sites.indexOf(s), 1);
    draw();
  }
  function clearAll() { sites.length = 0; siteLayer.clearLayers(); draw(); }
  function addPoint(ll) { show(true); withGrid(() => computeSite(makeSite(ll, S.ht, S.gain, `Point ${nextId}`))); }
  function addNode(i) {
    const n = dk.scenario.nodes[i];
    const h = dk.sim ? dk.sim.inspect(i).h : n.h || S.ht;
    return makeSite([n.lat, n.lon], h, KIND_GAIN[n.kind] ?? 0, dk.name(i), i);
  }
  function addKinds(kinds) {
    show(true);
    const nodes = (dk.scenario && dk.scenario.nodes) || [];
    withGrid(() => {
      for (let i = 0; i < nodes.length; i++) {
        if (!kinds.includes(nodes[i].kind) || nodes[i].radio === false || sites.some((s) => s.node === i)) continue;
        computeSite(addNode(i));
      }
    });
  }
  function recomputeAll() { withGrid(() => sites.forEach(computeSite)); }

  // Predicted received power (dBm) from a site to a point, with the same
  // model (used to compare with measured links). `site`: {lat, lon, h, gain, tx};
  // `rx`: {lat, lon, h, gain}.
  async function predictRx(site, rx) {
    await loadGrid();
    const a = L.latLng(site.lat, site.lon), b = L.latLng(rx.lat, rx.lon), d = Math.max(1, map.distance(a, b));
    const n = Math.max(2, Math.ceil(d / STEP));
    const g = Array.from({ length: n + 1 }, (_, k) => groundAt(a.lat + (b.lat - a.lat) * k / n, a.lng + (b.lng - a.lng) * k / n));
    const near = Math.min(n, 60);
    let sum = 0; for (let k = 1; k <= near; k++) sum += g[k];
    const hEffA = site.h + Math.max(0, g[0] - sum / near);
    sum = 0; for (let k = n - 1; k >= n - near; k--) sum += g[k];
    const hEffB = rx.h + Math.max(0, g[n] - sum / near);
    let vmax = 0;
    for (let i = 1; i < n; i++) {
      const d1 = d * i / n, d2 = d - d1;
      const rel = g[i] - (g[0] + (g[n] - g[0]) * d1 / d) - (site.h + (rx.h - site.h) * d1 / d);
      vmax = Math.max(vmax, rel * Math.sqrt(2 * d / (LAMBDA * d1 * d2)));
    }
    const hills = vmax > 0 ? 20 * Math.log10(Math.sqrt((vmax - 0.1) ** 2 + 1) + vmax - 0.1) + 0.9 : 0;
    return (site.tx ?? S.power) + site.gain + (rx.gain || 0) + heightDb(hEffA) + heightDb(hEffB) - (31.2 + 30 * Math.log10(d)) - hills;
  }
  // Real sites (real-world mode): replaces the sites with these.
  function setSites(list) {
    show(true);
    sites.length = 0; siteLayer.clearLayers();
    withGrid(() => { for (const x of list) { const s = makeSite([x.lat, x.lon], x.h, x.gain, x.name, -1); s.tx = x.tx; computeSite(s); } });
  }
  function setCalibration(db) { S.cal = db; recomputeAll(); }

  // ---------- the card on the map ----------
  const card = document.createElement("div");
  card.id = "cv-card"; card.className = "ov hidden";
  card.style.cssText = "right:10px;bottom:44px;width:300px;max-height:calc(100% - 120px);overflow:auto;padding:8px 10px;font-size:12px";
  card.innerHTML = `
    <h4 style="margin:0 0 6px;display:flex;justify-content:space-between;align-items:center">Coverage <button id="cv-x" style="border:0;background:transparent" title="Hide the card (the layer stays)">✕</button></h4>
    <div style="display:flex;gap:4px;flex-wrap:wrap">
      <button id="cv-add" class="primary" title="Then click the map, or a node">+ Site on the map</button>
      <button id="cv-masts" title="Every repeater, anchor, host and Wi-Fi node in the network">All masts</button>
      <button id="cv-all" title="Every node with a radio">All nodes</button>
      <button id="cv-clear">Clear</button>
    </div>
    <p id="cv-sum" class="hint" style="margin:6px 0 2px">No sites yet. Add one on the map, or all masts at once.</p>
    <ul id="cv-list" style="list-style:none;margin:0;padding:0;max-height:120px;overflow:auto"></ul>
    <div style="margin:6px 0 2px;display:flex;align-items:center;gap:6px">weak
      ${RAMP.slice().reverse().map(([, c]) => `<span style="display:inline-block;width:22px;height:10px;background:rgb(${c})"></span>`).join("")} strong</div>
    <details><summary style="cursor:pointer">Settings</summary>
      <label>Receiver: SF <select id="cv-sf">${[7, 8, 9, 10, 11, 12].map((s) => `<option ${s === S.sf ? "selected" : ""}>${s}</option>`).join("")}</select> at <input type="number" id="cv-hr" value="${S.hr}" min="0.5" max="100" step="0.5" style="width:48px"> m</label>
      <label>New points: antenna <input type="number" id="cv-ht" value="${S.ht}" min="0.5" max="300" step="0.5" style="width:48px"> m, gain <input type="number" id="cv-g" value="${S.gain}" min="-5" max="20" style="width:40px"> dB</label>
      <label>Power <input type="number" id="cv-p" value="${S.power}" min="0" max="30" style="width:44px"> dBm, radius <input type="number" id="cv-r" value="${S.radius}" min="2" max="80" style="width:44px"> km</label>
      <label>Extra loss (forest, town) <input type="number" id="cv-cl" value="${S.extra}" min="0" max="40" style="width:44px"> dB</label>
      <label>See-through <input type="range" id="cv-op" min="0.2" max="1" step="0.05" value="0.7" style="width:100px"></label>
      <p class="hint">Same radio model as the run, with the hills of the 250 m height grid; land only. Nodes keep their own antenna height. A yellow point can be dragged; right-click it to remove it. A planning picture, not a measurement.</p>
    </details>`;
  $("mapwrap").appendChild(card);
  L.DomEvent.disableClickPropagation(card); L.DomEvent.disableScrollPropagation(card);
  function show(on) { card.classList.toggle("hidden", !on); }
  $("cv-x").onclick = () => show(false);
  $("cv-masts").onclick = () => addKinds(["repeater", "anchor", "host", "wifi"]);
  $("cv-all").onclick = () => addKinds(["repeater", "anchor", "host", "wifi", "relay", "handheld", "mobile"]);
  $("cv-clear").onclick = clearAll;
  const read = () => { S.sf = +$("cv-sf").value; S.hr = +$("cv-hr").value || 1.5; S.ht = +$("cv-ht").value || 15; S.gain = +$("cv-g").value || 0; S.power = +$("cv-p").value || 0; S.radius = +$("cv-r").value || 30; S.extra = +$("cv-cl").value || 0; };
  ["cv-sf", "cv-hr", "cv-p", "cv-r", "cv-cl"].forEach((id) => $(id).addEventListener("change", () => { read(); recomputeAll(); }));
  ["cv-ht", "cv-g"].forEach((id) => $(id).addEventListener("change", read));
  $("cv-op").oninput = () => overlay && overlay.setOpacity(+$("cv-op").value);

  // Adding a site: click the map once (a click near a node takes the node).
  let picking = false;
  function pick(on) {
    picking = on;
    map.getContainer().style.cursor = on ? "crosshair" : "";
    $("cv-add").textContent = on ? "Click the map…" : "+ Site on the map";
  }
  $("cv-add").onclick = () => pick(!picking);
  map.getContainer().parentElement.addEventListener("click", (ev) => {
    if (!picking || !map.getContainer().contains(ev.target) || ev.target.closest(".leaflet-control")) return;
    ev.stopPropagation(); ev.preventDefault();
    pick(false);
    const pt = map.mouseEventToContainerPoint(ev);
    const nodes = (dk.scenario && dk.scenario.nodes) || [];
    let bestN = -1, bd = 14;
    nodes.forEach((n, i) => { const d = map.latLngToContainerPoint([n.lat, n.lon]).distanceTo(pt); if (d < bd) { bd = d; bestN = i; } });
    if (bestN >= 0) { show(true); withGrid(() => computeSite(addNode(bestN))); }
    else addPoint(map.containerPointToLatLng(pt));
  }, true);
  document.addEventListener("keydown", (e) => { if (e.key === "Escape" && picking) pick(false); });

  // From the tool bar: open the card (and pick a site); right-click: a site there.
  function start(at) {
    show(true);
    if (at) addPoint(at); else if (!sites.length) pick(true);
  }
  // The Layers panel switches the picture and the site markers on and off.
  function setVisible(on) {
    visible = on;
    for (const l of [overlay, siteLayer]) if (l) { if (on) l.addTo(map); else map.removeLayer(l); }
  }
  window.DKCoverage = { start, clear: clearAll, show, predictRx, setSites, setCalibration, setVisible, noise: NOISE, get count() { return sites.length; } };
})();
