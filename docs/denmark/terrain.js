// Height map and radio path profile for the Denmark simulator.
//
// Heights come from the public Terrarium elevation tiles (AWS open data,
// mostly SRTM, ~30 m cells): height = R*256 + G + B/256 - 32768 metres.
// The overlay colours the land by height with hill shading. The profile tool
// takes two points, samples the ground between them and checks the line of
// sight and the first Fresnel zone, with earth curvature (k = 4/3).
// This is a planning aid only: the simulated radio links do not use it yet.
(function () {
  const dk = window.DKSim;
  if (!dk || !dk.map) return;
  const map = dk.map;
  const $ = (id) => document.getElementById(id);
  const URL = (z, x, y) => `https://s3.amazonaws.com/elevation-tiles-prod/terrarium/${z}/${x}/${y}.png`;
  const DEM_Z = 12;           // zoom used for height lookups (~25 m per pixel in Denmark)
  const FREQ_MHZ = 869.525;   // EU 869 MHz band used by the nodes

  // ---------- tile cache (decoded heights) ----------
  const cache = new Map();
  function demTile(z, x, y) {
    const key = `${z}/${x}/${y}`;
    if (!cache.has(key)) {
      cache.set(key, new Promise((ok, fail) => {
        const img = new Image();
        img.crossOrigin = "anonymous";
        img.onload = () => {
          const c = document.createElement("canvas"); c.width = c.height = 256;
          const g = c.getContext("2d"); g.drawImage(img, 0, 0);
          const d = g.getImageData(0, 0, 256, 256).data, h = new Float32Array(256 * 256);
          for (let i = 0; i < h.length; i++) h[i] = d[i * 4] * 256 + d[i * 4 + 1] + d[i * 4 + 2] / 256 - 32768;
          ok(h);
        };
        img.onerror = () => { cache.delete(key); fail(new Error("height tile failed: " + key)); };
        img.src = URL(z, x, y);
      }));
    }
    return cache.get(key);
  }
  function tileXY(lat, lon, z) {
    const n = 2 ** z, r = lat * Math.PI / 180;
    return [(lon + 180) / 360 * n * 256, (1 - Math.log(Math.tan(r) + 1 / Math.cos(r)) / Math.PI) / 2 * n * 256];
  }
  async function heightAt(lat, lon, z = DEM_Z) {
    const [px, py] = tileXY(lat, lon, z);
    const tx = Math.floor(px / 256), ty = Math.floor(py / 256);
    const h = await demTile(z, tx, ty);
    const ix = Math.min(255, Math.floor(px - tx * 256)), iy = Math.min(255, Math.floor(py - ty * 256));
    return h[iy * 256 + ix];
  }

  // ---------- coloured overlay ----------
  const STOPS = [[0, [46, 125, 50]], [15, [124, 179, 66]], [35, [205, 220, 57]], [60, [251, 192, 45]], [90, [239, 108, 0]], [130, [191, 54, 12]], [175, [109, 30, 20]]];
  function colour(h) {
    if (h <= 0.5) return null;
    for (let i = 1; i < STOPS.length; i++) {
      if (h <= STOPS[i][0]) {
        const [h0, c0] = STOPS[i - 1], [h1, c1] = STOPS[i], f = (h - h0) / (h1 - h0);
        return c0.map((v, k) => v + (c1[k] - v) * f);
      }
    }
    return STOPS[STOPS.length - 1][1];
  }
  map.createPane("heightPane");
  map.getPane("heightPane").style.zIndex = 250;   // above tiles (and their dark filter), below markers
  map.getPane("heightPane").style.pointerEvents = "none";
  const HeightLayer = L.GridLayer.extend({
    createTile(coords, done) {
      const c = document.createElement("canvas"); c.width = c.height = 256;
      const z = Math.min(coords.z, 14), s = 2 ** (coords.z - z);
      const tx = Math.floor(coords.x / s), ty = Math.floor(coords.y / s);
      const ox = (coords.x - tx * s) * 256 / s, oy = (coords.y - ty * s) * 256 / s;
      demTile(z, tx, ty).then((h) => {
        const g = c.getContext("2d"), out = g.createImageData(256, 256), o = out.data;
        const mpp = 40075016 * Math.cos(56 * Math.PI / 180) / (256 * 2 ** z);   // metres per source pixel
        const at = (x, y) => h[Math.min(255, Math.max(0, Math.floor(y))) * 256 + Math.min(255, Math.max(0, Math.floor(x)))];
        for (let y = 0; y < 256; y++) for (let x = 0; x < 256; x++) {
          const sx = ox + x / s, sy = oy + y / s, v = at(sx, sy), col = colour(v), i = (y * 256 + x) * 4;
          if (!col) continue;
          // Hill shade from the north-west, exaggerated: Denmark is flat.
          const dx = (at(sx + 1, sy) - at(sx - 1, sy)) / (2 * mpp), dy = (at(sx, sy + 1) - at(sx, sy - 1)) / (2 * mpp);
          const shade = Math.max(0.55, Math.min(1.25, 1 + 6 * (dy - dx) * 0.7));
          o[i] = col[0] * shade; o[i + 1] = col[1] * shade; o[i + 2] = col[2] * shade; o[i + 3] = 255;
        }
        g.putImageData(out, 0, 0);
        done(null, c);
      }, (e) => done(e, c));
      return c;
    },
  });
  const layer = new HeightLayer({ pane: "heightPane", opacity: 0.6, maxZoom: 17, attribution: "Heights: Terrarium tiles (AWS open data, SRTM)" });

  // ---------- panel ----------
  const tab = $("tab-terrain");
  const sec = document.createElement("div");
  sec.innerHTML = `
    <h3>Height map</h3>
    <p class="hint">Ground height from public elevation tiles (about 30 m detail, internet needed). Green is low, red is the highest ground (~170 m). Planning aid only: the simulated links do not use it yet.</p>
    <label><input type="checkbox" id="hm-on"> Show heights</label>
    <label>See-through <input type="range" id="hm-op" min="0.2" max="1" step="0.05" value="0.6" style="width:110px"></label>
    <p class="hint" id="hm-at">Turn on the map to read the height under the mouse.</p>
    <h3>Antenna heights</h3>
    <p class="hint">Metres above the ground, for every node of a kind that has no height of its own (set one node's height in its card). Empty = the built-in default.</p>
    <div id="hm-heights" style="display:grid;grid-template-columns:auto auto;gap:4px 8px;align-items:center"></div>
    <button id="hm-heights-apply">Apply heights (restarts)</button>
    <h3>Hills in the simulation</h3>
    <label><input type="checkbox" id="hm-sim"> Hills affect the radio links</label>
    <p class="hint" id="hm-sim-note">Off: links see flat ground. On: a 250 m height grid of Denmark (1 MB, loaded once) blocks and bends links over hills (Deygout, ITU-R P.526), and a node on a hilltop sees further. Changes rebuild the run.</p>
    <h3>Radio path profile</h3>
    <p class="hint">Click two points: the ground between them, the line of sight, and the first Fresnel zone at 869 MHz with earth curvature.</p>
    <label>Antenna heights <input type="number" id="hm-ha" value="2" min="0" max="200" style="width:55px"> m and <input type="number" id="hm-hb" value="2" min="0" max="200" style="width:55px"> m above ground</label>
    <button id="hm-pick">Pick two points</button>
    <div id="hm-prof"></div>`;
  tab.insertBefore(sec, tab.firstChild);

  $("hm-on").onchange = () => { if ($("hm-on").checked) layer.addTo(map); else map.removeLayer(layer); };
  $("hm-op").oninput = () => layer.setOpacity(+$("hm-op").value);
  let hoverT = 0;
  map.on("mousemove", (e) => {
    if (!map.hasLayer(layer)) return;
    const t = ++hoverT;
    heightAt(e.latlng.lat, e.latlng.lng).then((h) => { if (t === hoverT) $("hm-at").textContent = `Height here: ${Math.max(0, h).toFixed(0)} m (${e.latlng.lat.toFixed(4)}, ${e.latlng.lng.toFixed(4)})`; }, () => {});
  });

  // ---------- profile ----------
  const profLayer = L.layerGroup().addTo(map);
  let picks = null;
  // Capture clicks before Leaflet (and the simulator's own map clicks) see them.
  map.getContainer().parentElement.addEventListener("click", (ev) => {
    if (!picks || !map.getContainer().contains(ev.target) || ev.target.closest(".leaflet-control")) return;
    ev.stopPropagation(); ev.preventDefault();
    picks.push(map.mouseEventToLatLng(ev));
    L.circleMarker(picks[picks.length - 1], { radius: 6, color: "#111", fillColor: "#fff", fillOpacity: 1, weight: 2 }).addTo(profLayer);
    if (picks.length === 2) { const [a, b] = picks; picks = null; map.getContainer().style.cursor = ""; $("hm-pick").textContent = "Pick two points"; profile(a, b); }
  }, true);
  $("hm-pick").onclick = () => {
    profLayer.clearLayers(); $("hm-prof").innerHTML = "";
    picks = []; map.getContainer().style.cursor = "crosshair"; $("hm-pick").textContent = "Click the first point…";
  };

  async function profile(a, b) {
    const D = map.distance(a, b), N = Math.max(40, Math.min(400, Math.round(D / 25)));
    $("hm-prof").innerHTML = `<p class="hint">Reading heights…</p>`;
    let g;
    try { g = await Promise.all(Array.from({ length: N + 1 }, (_, i) => heightAt(a.lat + (b.lat - a.lat) * i / N, a.lng + (b.lng - a.lng) * i / N).then((h) => Math.max(0, h)))); }
    catch (e) { $("hm-prof").innerHTML = `<p class="hint">Could not load heights (needs internet).</p>`; return; }
    const ha = g[0] + (+$("hm-ha").value || 0), hb = g[N] + (+$("hm-hb").value || 0);
    const lambda = 299.792458 / FREQ_MHZ, Re = 6371000 * 4 / 3;
    let worst = Infinity, worstI = 0, blocked = false;
    const rows = g.map((h, i) => {
      const d1 = D * i / N, d2 = D - d1;
      const bulge = d1 * d2 / (2 * Re);
      const ground = h + bulge;                    // ground raised by the earth's curve
      const los = ha + (hb - ha) * i / N;
      const fz = Math.sqrt(lambda * d1 * d2 / Math.max(1, D));
      if (i > 0 && i < N) {
        const clear = fz > 0 ? (los - ground) / fz : 99;   // 1 = whole first zone free, 0 = grazing
        if (clear < worst) { worst = clear; worstI = i; }
        if (ground > los) blocked = true;
      }
      return { d1, ground, los, fz };
    });
    const verdict = blocked ? ["Blocked: the ground cuts the line of sight.", "#c62828"]
      : worst < 0.6 ? ["Line of sight, but the Fresnel zone is less than 60 % clear: expect extra loss.", "#ef6c00"]
      : ["Clear: line of sight and at least 60 % of the Fresnel zone free.", "#2e7d32"];
    // Free-space loss plus a single knife-edge at the worst point (ITU-R P.526).
    const fspl = 32.44 + 20 * Math.log10(D / 1000) + 20 * Math.log10(FREQ_MHZ);
    const v = -Math.SQRT2 * worst;   // Fresnel-Kirchhoff parameter from the clearance ratio
    const knife = v > -0.78 ? 6.9 + 20 * Math.log10(Math.sqrt((v - 0.1) ** 2 + 1) + v - 0.1) : 0;
    L.polyline([a, b], { color: verdict[1], weight: 3, dashArray: "6 6" }).addTo(profLayer);
    const wp = [a.lat + (b.lat - a.lat) * worstI / N, a.lng + (b.lng - a.lng) * worstI / N];
    L.circleMarker(wp, { radius: 5, color: verdict[1], fillOpacity: 0.8 }).bindTooltip("Worst point").addTo(profLayer);

    const W = 300, H = 140, P = 4;
    const top = Math.max(...rows.map((r) => Math.max(r.ground, r.los + r.fz))) + 5, bot = Math.min(...rows.map((r) => Math.min(r.ground, r.los - r.fz))) - 2;
    const X = (i) => P + (W - 2 * P) * i / N, Y = (h) => H - 18 - (H - 26) * (h - bot) / (top - bot);
    const pts = (f) => rows.map((r, i) => `${X(i).toFixed(1)},${Y(f(r)).toFixed(1)}`).join(" ");
    const svg = `<svg viewBox="0 0 ${W} ${H}" style="width:100%;height:auto" role="img" aria-label="Ground profile between the two points">
      <polygon points="${X(0)},${H - 18} ${pts((r) => r.ground)} ${X(N)},${H - 18}" fill="#8d6e63" opacity=".75"/>
      <polyline points="${pts((r) => r.los + r.fz)}" fill="none" stroke="#64b5f6" stroke-dasharray="3 3"/>
      <polyline points="${pts((r) => r.los - r.fz)}" fill="none" stroke="#64b5f6" stroke-dasharray="3 3"/>
      <polyline points="${pts((r) => r.los)}" fill="none" stroke="${verdict[1]}" stroke-width="2"/>
      <line x1="${X(worstI)}" x2="${X(worstI)}" y1="8" y2="${H - 18}" stroke="${verdict[1]}" stroke-dasharray="2 3"/>
      <text x="${P}" y="${H - 4}" font-size="10" fill="var(--mute)">0</text>
      <text x="${W - P}" y="${H - 4}" font-size="10" fill="var(--mute)" text-anchor="end">${(D / 1000).toFixed(1)} km</text>
      <text x="${P}" y="10" font-size="10" fill="var(--mute)">${top.toFixed(0)} m</text></svg>`;
    $("hm-prof").innerHTML = `${svg}
      <p style="color:${verdict[1]};font-weight:600;margin:4px 0">${verdict[0]}</p>
      <table>
        <tr><td>Distance</td><td class="n">${(D / 1000).toFixed(2)} km</td></tr>
        <tr><td>Ground at the ends</td><td class="n">${g[0].toFixed(0)} m / ${g[N].toFixed(0)} m</td></tr>
        <tr><td>Worst point</td><td class="n">${(rows[worstI].d1 / 1000).toFixed(1)} km, ${worst > 50 ? "far above" : (worst * 100).toFixed(0) + " % of the zone free"}</td></tr>
        <tr><td>Free-space loss</td><td class="n">${fspl.toFixed(0)} dB</td></tr>
        <tr><td>Extra loss from the obstacle</td><td class="n">≈ ${knife.toFixed(0)} dB</td></tr>
      </table>
      <p class="hint">Brown: ground, lifted by the earth's curve. Blue dashes: first Fresnel zone. Only the single worst obstacle is counted; trees and houses are not in the height data.</p>`;
  }

  // ---------- hills in the engine ----------
  let gridLoaded = false;
  async function loadGrid() {
    if (gridLoaded) return;
    const [meta, bin] = await Promise.all([fetch("dk-dem.json").then((r) => r.json()), fetch("dk-dem.bin").then((r) => r.arrayBuffer())]);
    const u8 = new Uint8Array(bin);
    let s = "";
    for (let i = 0; i < u8.length; i += 0x8000) s += String.fromCharCode.apply(null, u8.subarray(i, i + 0x8000));
    const r = dk.sim.call(Object.assign({ cmd: "terrain", data: btoa(s) }, meta));
    if (!r.ok) throw new Error(r.error || "terrain grid refused");
    gridLoaded = true;
  }
  $("hm-sim").onchange = async () => {
    const want = $("hm-sim").checked;
    try {
      if (want) { $("hm-sim-note").textContent = "Loading the height grid…"; await loadGrid(); }
      api.on = want;
      dk.edited();
      $("hm-sim-note").textContent = want ? "On: links are checked against the hills (250 m grid). Trees and houses are still only the clutter areas you draw." : "Off: links see flat ground.";
    } catch (e) { $("hm-sim").checked = false; api.on = false; $("hm-sim-note").textContent = "Could not load the height grid: " + e.message; }
  };

  // ---------- antenna height per kind ----------
  const KIND_H = { repeater: 15, anchor: 40, host: 10, wifi: 6, relay: 2, handheld: 1.5, mobile: 1.5 };
  function syncHeights() {
    const hs = (dk.scenario && dk.scenario.heights) || {};
    $("hm-heights").innerHTML = Object.entries(KIND_H).map(([k, def]) =>
      `<span>${k}</span><span><input type="number" min="0.5" max="300" step="0.5" data-kind="${k}" style="width:65px" value="${hs[k] || ""}" placeholder="${def}"> m</span>`).join("");
  }
  $("hm-heights-apply").onclick = () => {
    const hs = {};
    $("hm-heights").querySelectorAll("input").forEach((i) => { if (+i.value > 0) hs[i.dataset.kind] = +i.value; });
    dk.scenario.heights = hs;
    dk.edited();
  };
  syncHeights();

  const api = { heightAt, on: false, syncHeights };
  window.DKTerrain = api;
})();
