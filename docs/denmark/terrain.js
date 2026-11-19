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
  const esc2 = (t) => String(t).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
  const URL = (z, x, y) => `https://s3.amazonaws.com/elevation-tiles-prod/terrarium/${z}/${x}/${y}.png`;
  const DEM_Z = 12;           // zoom used for height lookups (~25 m per pixel in Denmark)
  const FREQ_MHZ = 869.525;   // EU 869 MHz band used by the nodes

  // ---------- tile cache (decoded heights) ----------
  const cache = new Map();
  function demTile(z, x, y) {
    const key = `${z}/${x}/${y}`;
    if (!cache.has(key)) {
      cache.set(key, new Promise((ok, fail) => {
        // The heights are coded in the colours, so the PNG is decoded here:
        // the browser's own image decoding may change a colour by one step,
        // and one step of red is 256 m.
        fetch(URL(z, x, y)).then((r) => { if (!r.ok) throw new Error(r.status); return r.arrayBuffer(); })
          .then(decodePng)
          .then((px) => {
            const h = new Float32Array(256 * 256);
            for (let i = 0; i < h.length; i++) h[i] = px[i * 3] * 256 + px[i * 3 + 1] + px[i * 3 + 2] / 256 - 32768;
            ok(despike(h));
          })
          .catch(() => { cache.delete(key); fail(new Error("height tile failed: " + key)); });
      }));
    }
    return cache.get(key);
  }
  // Minimal PNG reader for the elevation tiles: 8-bit RGB or RGBA, not
  // interlaced. Returns RGB bytes, row by row.
  async function decodePng(buf) {
    const v = new DataView(buf), u8 = new Uint8Array(buf);
    let o = 8, w = 0, h = 0, ct = 0;
    const parts = [];
    while (o < u8.length) {
      const len = v.getUint32(o), type = String.fromCharCode(...u8.subarray(o + 4, o + 8));
      if (type === "IHDR") { w = v.getUint32(o + 8); h = v.getUint32(o + 12); ct = u8[o + 17]; if (u8[o + 16] !== 8 || u8[o + 20] !== 0 || (ct !== 2 && ct !== 6)) throw new Error("unsupported PNG"); }
      else if (type === "IDAT") parts.push(u8.subarray(o + 8, o + 8 + len));
      else if (type === "IEND") break;
      o += 12 + len;
    }
    const raw = new Uint8Array(await new Response(new Blob(parts).stream().pipeThrough(new DecompressionStream("deflate"))).arrayBuffer());
    const bpp = ct === 6 ? 4 : 3, stride = w * bpp, out = new Uint8Array(w * h * 3);
    let prev = new Uint8Array(stride), cur = new Uint8Array(stride);
    for (let y = 0; y < h; y++) {
      const f = raw[y * (stride + 1)], line = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1));
      for (let i = 0; i < stride; i++) {
        const a = i >= bpp ? cur[i - bpp] : 0, b = prev[i], c = i >= bpp ? prev[i - bpp] : 0;
        let p = line[i];
        if (f === 1) p += a; else if (f === 2) p += b; else if (f === 3) p += (a + b) >> 1;
        else if (f === 4) { const pa = Math.abs(b - c), pb = Math.abs(a - c), pc = Math.abs(a + b - 2 * c); p += pa <= pb && pa <= pc ? a : pb <= pc ? b : c; }
        cur[i] = p;
      }
      for (let x = 0; x < w; x++) { const k = (y * w + x) * 3; out[k] = cur[x * bpp]; out[k + 1] = cur[x * bpp + 1]; out[k + 2] = cur[x * bpp + 2]; }
      [prev, cur] = [cur, prev];
    }
    return out;
  }
  // A lone pixel 100 m above or below all four neighbours is a data error,
  // not a hill (a cliff differs from some neighbours, not all): take their median.
  function despike(h) {
    const out = Float32Array.from(h);
    for (let y = 1; y < 255; y++) for (let x = 1; x < 255; x++) {
      const i = y * 256 + x, v = h[i], n = [h[i - 1], h[i + 1], h[i - 256], h[i + 256]];
      if (n.every((u) => Math.abs(u - v) > 100)) { n.sort((p, q) => p - q); out[i] = (n[1] + n[2]) / 2; }
    }
    return out;
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
    <p class="hint">Ground height from public elevation tiles (about 30 m detail, internet needed). Green is low, red is the highest ground (~170 m). Tick "Hills affect the radio links" below to use the heights in the run.</p>
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
    <p class="hint">Click two points (or two nodes: their antenna heights are used): the ground between them, the line of sight, and the first Fresnel zone at 869 MHz with earth curvature. Also in the map's tool bar and right-click menu.</p>
    <label>Antenna heights <input type="number" id="hm-ha" value="2" min="0" max="200" style="width:55px"> m and <input type="number" id="hm-hb" value="2" min="0" max="200" style="width:55px"> m above ground</label>
    <button id="hm-pick">Pick two points</button>`;
  tab.insertBefore(sec, tab.firstChild);
  // Results float on the map: one card with a tab per radio path.
  const float = document.createElement("div");
  float.id = "hm-float"; float.className = "ov hidden";
  float.style.cssText = "left:54px;bottom:44px;width:340px;max-height:calc(100% - 120px);overflow:auto;padding:8px 10px";
  float.innerHTML = `<h4 style="margin:0 0 4px;display:flex;justify-content:space-between;align-items:center">Radio paths <span><button id="hm-new" title="Add another radio path" style="font-size:12px;padding:1px 6px">+ path</button> <button class="x" id="hm-close" style="border:0;background:transparent" title="Remove all paths">✕</button></span></h4>
    <div id="hm-tabs" style="display:flex;gap:4px;flex-wrap:wrap;margin-bottom:4px"></div><div id="hm-prof"></div>`;
  $("mapwrap").appendChild(float);
  L.DomEvent.disableClickPropagation(float); L.DomEvent.disableScrollPropagation(float);

  $("hm-on").onchange = () => { if ($("hm-on").checked) layer.addTo(map); else map.removeLayer(layer); };
  $("hm-op").oninput = () => layer.setOpacity(+$("hm-op").value);
  let hoverT = 0;
  map.on("mousemove", (e) => {
    if (!map.hasLayer(layer)) return;
    const t = ++hoverT;
    heightAt(e.latlng.lat, e.latlng.lng).then((h) => { if (t === hoverT) $("hm-at").textContent = `Height here: ${Math.max(0, h).toFixed(0)} m (${e.latlng.lat.toFixed(4)}, ${e.latlng.lng.toFixed(4)})`; }, () => {});
  });

  // ---------- precise heights (optional key) ----------
  const D2 = window.DKDhm;
  const keyBox = document.createElement("div");
  keyBox.innerHTML = `
    <h3>Precise heights (optional)</h3>
    <p class="hint">With your own free key from <a href="https://dataforsyningen.dk" target="_blank" rel="noopener">dataforsyningen.dk</a> the radio paths use Danmarks Højdemodel (ground to ~2 m, trees and buildings too) and name what stands in the way (GeoDanmark). The key stays in this browser. Without a key: the built-in heights (~30 m).</p>
    <label>Key <input id="hm-key" type="password" autocomplete="off" size="20" placeholder="no key: built-in heights"> <button id="hm-key-save">Save and test</button></label>
    <label><input type="checkbox" id="hm-surface" checked> Count trees and buildings (surface model)</label>
    <p class="hint" id="hm-key-note"></p>`;
  sec.appendChild(keyBox);
  function keyNote() { $("hm-key-note").textContent = D2 && D2.key ? "A key is saved: radio paths use DHM." : "No key: radio paths use the built-in heights."; }
  if (D2) {
    $("hm-key").value = D2.key;
    keyNote();
    $("hm-key-save").onclick = async () => {
      D2.setKey($("hm-key").value);
      if (!D2.key) { keyNote(); recomputeAll(); return; }
      $("hm-key-note").textContent = "Testing the key…";
      try { const h = await D2.test(); $("hm-key-note").textContent = `The key works: DHM answered (${h.toFixed(1)} m at a test spot). Radio paths now use DHM.`; recomputeAll(); }
      catch (e) { $("hm-key-note").textContent = "The key did not work (" + e.message + "). Radio paths use the built-in heights."; }
    };
    $("hm-surface").onchange = () => recomputeAll();
  }

  // ---------- radio paths ----------
  const COLORS = ["#2563eb", "#9333ea", "#0891b2", "#db2777", "#65a30d", "#ea580c"];
  const paths = [];
  let nextId = 1, shown = null;
  const pickLayer = L.layerGroup().addTo(map);
  let picks = null;
  // A click near a node takes the node's spot and antenna height.
  function snap(ev) { return snapAt(map.mouseEventToContainerPoint(ev)); }
  function snapAt(pt) {
    const nodes = (dk.scenario && dk.scenario.nodes) || [];
    let best = -1, bd = 14;
    nodes.forEach((n, i) => { const d = map.latLngToContainerPoint([n.lat, n.lon]).distanceTo(pt); if (d < bd) { bd = d; best = i; } });
    if (best < 0) return { ll: map.containerPointToLatLng(pt) };
    const n = nodes[best];
    return { ll: L.latLng(n.lat, n.lon), h: dk.sim ? dk.sim.inspect(best).h : n.h, name: dk.name(best) };
  }
  function addPick(p) {
    picks.push(p);
    L.circleMarker(p.ll, { radius: 6, color: "#111", fillColor: "#fff", fillOpacity: 1, weight: 2 }).addTo(pickLayer);
    if (picks.length === 2) {
      const [a, b] = picks; picks = null; pickLayer.clearLayers();
      map.getContainer().style.cursor = ""; $("hm-pick").textContent = "Add a radio path"; api.onMode && api.onMode(null);
      addPath(a, b);
    }
  }
  // Capture clicks before Leaflet (and the simulator's own map clicks) see them.
  map.getContainer().parentElement.addEventListener("click", (ev) => {
    if (!picks || !map.getContainer().contains(ev.target) || ev.target.closest(".leaflet-control") || ev.target.closest("#hm-float")) return;
    ev.stopPropagation(); ev.preventDefault();
    addPick(snap(ev));
  }, true);
  function startProfile(from) {
    pickLayer.clearLayers();
    picks = []; map.getContainer().style.cursor = "crosshair";
    $("hm-pick").textContent = from ? "Click the second point…" : "Click the first point…";
    api.onMode && api.onMode("profile");
    if (from) addPick({ ll: from });
  }
  function cancelProfile() { if (picks) { picks = null; pickLayer.clearLayers(); map.getContainer().style.cursor = ""; $("hm-pick").textContent = "Add a radio path"; } }
  $("hm-pick").textContent = "Add a radio path";
  $("hm-pick").onclick = () => startProfile();
  $("hm-new").onclick = () => startProfile();
  $("hm-close").onclick = () => { while (paths.length) removePath(paths[0]); };

  function handleIcon(col) {
    return L.divIcon({ className: "", iconSize: [18, 18], iconAnchor: [9, 9],
      html: `<div style="width:14px;height:14px;border-radius:50%;background:#fff;border:3px solid ${col};box-shadow:0 1px 4px rgba(0,0,0,.4);cursor:grab"></div>` });
  }
  // Each path keeps its two ends as handles: drag one and that path is
  // checked again where it is dropped (on a node: that node's spot and height).
  function addPath(pa, pb) {
    const p = { id: nextId++, pa, pb, run: 0, html: "", verdictCol: "#888" };
    p.col = COLORS[(p.id - 1) % COLORS.length];
    p.layer = L.layerGroup().addTo(map);
    p.line = L.polyline([pa.ll, pb.ll], { color: p.col, weight: 4, dashArray: "8 6" }).addTo(p.layer);
    p.line.on("click", () => show(p));
    p.line.bindTooltip(`Path ${p.id}`, { sticky: true });
    [pa, pb].forEach((e, k) => {
      const m = L.marker(e.ll, { draggable: true, icon: handleIcon(p.col), title: `Path ${p.id}: drag to move this end`, zIndexOffset: 1000 }).addTo(p.layer);
      m.on("drag", () => p.line.setLatLngs(k ? [p.pa.ll, m.getLatLng()] : [m.getLatLng(), p.pb.ll]));
      m.on("dragend", () => {
        const moved = snapAt(map.latLngToContainerPoint(m.getLatLng()));
        m.setLatLng(moved.ll);
        if (k) p.pb = moved; else p.pa = moved;
        show(p);
        compute(p);
      });
    });
    paths.push(p);
    show(p);
    compute(p);
  }
  function removePath(p) {
    map.removeLayer(p.layer);
    paths.splice(paths.indexOf(p), 1);
    if (shown === p) shown = paths[paths.length - 1] || null;
    render();
  }
  function show(p) { shown = p; render(); }
  function render() {
    float.classList.toggle("hidden", !paths.length);
    $("hm-tabs").innerHTML = paths.map((p) => `<span style="display:inline-flex;align-items:center;border:2px solid ${p.col};border-radius:12px;${p === shown ? `background:${p.col};color:#fff;` : ""}font-size:12px"><button data-p="${p.id}" style="border:0;background:transparent;color:inherit;padding:1px 4px 1px 8px">Path ${p.id} <span style="color:${p === shown ? "#fff" : p.verdictCol}">●</span></button><button data-x="${p.id}" title="Remove path ${p.id}" style="border:0;background:transparent;color:inherit;padding:1px 6px 1px 2px">✕</button></span>`).join("");
    $("hm-tabs").querySelectorAll("button[data-p]").forEach((b) => b.onclick = () => show(paths.find((p) => p.id === +b.dataset.p)));
    $("hm-tabs").querySelectorAll("button[data-x]").forEach((b) => b.onclick = () => removePath(paths.find((p) => p.id === +b.dataset.x)));
    $("hm-prof").innerHTML = shown ? shown.html : "";
    paths.forEach((p) => p.line.setStyle({ weight: p === shown ? 5 : 3, opacity: p === shown ? 1 : 0.7 }));
  }
  function recomputeAll() { paths.forEach(compute); }

  async function compute(p) {
    const pa = p.pa, pb = p.pb, a = pa.ll, b = pb.ll, run = ++p.run;
    p.line.setLatLngs([a, b]);
    p.html = `<p class="hint">Reading heights…</p>`; if (shown === p) render();
    const D = map.distance(a, b);
    const useDhm = !!(D2 && D2.key);
    const surf = useDhm && $("hm-surface").checked;
    let N = useDhm ? Math.max(40, Math.min(1000, Math.round(D / 2))) : Math.max(40, Math.min(400, Math.round(D / 25)));
    let g = null, terrain, source = "", note = "";
    if (useDhm) {
      try {
        const r = await D2.profile(a, b, N);
        terrain = r.terrain;
        g = surf ? r.surface.map((s, i) => Math.max(s, r.terrain[i])) : r.terrain;
        source = `DHM ${surf ? "surface (trees and buildings)" : "terrain"}, ${r.res.toFixed(1)} m steps`;
      } catch (e) { note = `DHM failed (${e.message}); built-in heights used. `; N = Math.max(40, Math.min(400, Math.round(D / 25))); }
    }
    if (!g) {
      try { g = await Promise.all(Array.from({ length: N + 1 }, (_, i) => heightAt(a.lat + (b.lat - a.lat) * i / N, a.lng + (b.lng - a.lng) * i / N).then((h) => Math.max(0, h)))); }
      catch (e) { if (run === p.run) { p.html = `<p class="hint">Could not load heights (needs internet).</p>`; render(); } return; }
      terrain = g; source = "built-in (Terrarium / SRTM, ~30 m)";
    }
    if (run !== p.run) return;   // a newer drag already started another check
    const antA = pa.h ?? (+$("hm-ha").value || 0), antB = pb.h ?? (+$("hm-hb").value || 0);
    const ha = terrain[0] + antA, hb = terrain[N] + antB;
    const lambda = 299.792458 / FREQ_MHZ, Re = 6371000 * 4 / 3;
    let worst = Infinity, worstI = 0, blocked = false;
    const rows = g.map((h, i) => {
      const d1 = D * i / N, d2 = D - d1;
      const bulge = d1 * d2 / (2 * Re);
      const los = ha + (hb - ha) * i / N;
      const fz = Math.sqrt(lambda * d1 * d2 / Math.max(1, D));
      const top = (i === 0 || i === N ? terrain[i] : h) + bulge;     // the ends stand on the ground
      if (i > 0 && i < N) {
        const clear = fz > 0 ? (los - top) / fz : 99;   // 1 = whole first zone free, 0 = grazing
        if (clear < worst) { worst = clear; worstI = i; }
        if (top > los) blocked = true;
      }
      return { d1, ground: terrain[i] + bulge, top, los, fz };
    });
    const verdict = blocked && worst < -0.3 ? ["Blocked: something cuts the line of sight (the signal must bend over it).", "#c62828"]
      : blocked ? ["Grazing: the line of sight is just touched.", "#ef6c00"]
      : worst < 0.6 ? ["Line of sight, but the Fresnel zone is less than 60 % clear: expect extra loss.", "#ef6c00"]
      : ["Clear: line of sight and at least 60 % of the Fresnel zone free.", "#2e7d32"];
    // Free-space loss plus a single knife-edge at the worst point (ITU-R P.526).
    const fspl = 32.44 + 20 * Math.log10(D / 1000) + 20 * Math.log10(FREQ_MHZ);
    const v = -Math.SQRT2 * worst;   // Fresnel-Kirchhoff parameter from the clearance ratio
    const knife = v > -0.78 ? 6.9 + 20 * Math.log10(Math.sqrt((v - 0.1) ** 2 + 1) + v - 0.1) : 0;

    // What stands in the way: with DHM, the tallest objects above the
    // ground that reach into the Fresnel zone, named by GeoDanmark.
    let obstacles = "";
    if (surf && source) {
      const hits = [];
      rows.forEach((r, i) => { if (i > 0 && i < N && r.top - r.ground > 2 && r.top > r.los - 0.6 * r.fz) hits.push(i); });
      const groups = [];
      for (const i of hits) { const last = groups[groups.length - 1]; if (last && i - last[last.length - 1] <= 3) last.push(i); else groups.push([i]); }
      const reach = (i) => rows[i].top - rows[i].los;
      const tops = groups.map((gr) => gr.reduce((m, i) => (reach(i) > reach(m) ? i : m), gr[0])).sort((x, y) => reach(y) - reach(x)).slice(0, 4);
      const named = await Promise.all(tops.map(async (i) => ({ i, what: await D2.whatIsAt(a.lat + (b.lat - a.lat) * i / N, a.lng + (b.lng - a.lng) * i / N) })));
      if (run !== p.run) return;
      obstacles = named.length
        ? `<h4 style="margin:6px 0 2px;font-size:12px">In the way (trees, buildings)</h4><table>${named.sort((x, y) => x.i - y.i).map(({ i, what }) =>
          `<tr><td>${(rows[i].d1 / 1000).toFixed(2)} km</td><td>${esc2(what.join(", ") || "unnamed object")}</td><td class="n">${(rows[i].top - rows[i].ground).toFixed(1)} m tall</td></tr>`).join("")}</table>`
        : `<p class="hint">No trees or buildings reach into the Fresnel zone.</p>`;
    }
    p.verdictCol = verdict[1];
    p.line.setStyle({ color: verdict[1] });
    if (p.worst) p.layer.removeLayer(p.worst);
    const wp = [a.lat + (b.lat - a.lat) * worstI / N, a.lng + (b.lng - a.lng) * worstI / N];
    p.worst = L.circleMarker(wp, { radius: 5, color: verdict[1], fillOpacity: 0.8 }).bindTooltip(`Path ${p.id}: worst point`).addTo(p.layer);

    const W = 300, H = 140, P = 4;
    const top = Math.max(...rows.map((r) => Math.max(r.top, r.los + r.fz))) + 5, bot = Math.min(...rows.map((r) => Math.min(r.ground, r.los - r.fz))) - 2;
    const X = (i) => P + (W - 2 * P) * i / N, Y = (h) => H - 18 - (H - 26) * (h - bot) / (top - bot);
    const pts = (f) => rows.map((r, i) => `${X(i).toFixed(1)},${Y(f(r)).toFixed(1)}`).join(" ");
    const svg = `<svg viewBox="0 0 ${W} ${H}" style="width:100%;height:auto" role="img" aria-label="Ground profile of path ${p.id}">
      <polygon points="${X(0)},${H - 18} ${pts((r) => r.top)} ${X(N)},${H - 18}" fill="#2e7d32" opacity=".55"/>
      <polygon points="${X(0)},${H - 18} ${pts((r) => r.ground)} ${X(N)},${H - 18}" fill="#8d6e63" opacity=".85"/>
      <polyline points="${pts((r) => r.los + r.fz)}" fill="none" stroke="#64b5f6" stroke-dasharray="3 3"/>
      <polyline points="${pts((r) => r.los - r.fz)}" fill="none" stroke="#64b5f6" stroke-dasharray="3 3"/>
      <polyline points="${pts((r) => r.los)}" fill="none" stroke="${verdict[1]}" stroke-width="2"/>
      <line x1="${X(worstI)}" x2="${X(worstI)}" y1="8" y2="${H - 18}" stroke="${verdict[1]}" stroke-dasharray="2 3"/>
      <text x="${P}" y="${H - 4}" font-size="10" fill="var(--mute)">0</text>
      <text x="${W - P}" y="${H - 4}" font-size="10" fill="var(--mute)" text-anchor="end">${(D / 1000).toFixed(2)} km</text>
      <text x="${P}" y="10" font-size="10" fill="var(--mute)">${top.toFixed(0)} m</text></svg>`;
    p.html = `${svg}
      <p style="color:${verdict[1]};font-weight:600;margin:4px 0">${verdict[0]}</p>
      <table>
        <tr><td>Distance</td><td class="n">${(D / 1000).toFixed(2)} km</td></tr>
        <tr><td>Ends</td><td class="n">${esc2(pa.name || "point")} · ${esc2(pb.name || "point")}</td></tr>
        <tr><td>Antennas</td><td class="n">${antA} m / ${antB} m above ground</td></tr>
        <tr><td>Ground at the ends</td><td class="n">${terrain[0].toFixed(1)} m / ${terrain[N].toFixed(1)} m</td></tr>
        <tr><td>Worst point</td><td class="n">${(rows[worstI].d1 / 1000).toFixed(2)} km, ${worst > 50 ? "far above" : (worst * 100).toFixed(0) + " % of the zone free"}</td></tr>
        <tr><td>Free-space loss</td><td class="n">${fspl.toFixed(0)} dB</td></tr>
        <tr><td>Extra loss from the obstacle</td><td class="n">≈ ${knife.toFixed(0)} dB</td></tr>
        <tr><td>Heights</td><td class="n">${esc2(source)}</td></tr>
      </table>${obstacles}
      <p class="hint">${esc2(note)}Brown: ground; green: trees and buildings (with a key); both lifted by the earth's curve. Blue dashes: first Fresnel zone. Only the single worst obstacle is counted for the loss.</p>`;
    render();
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

  function showHeights(on) { $("hm-on").checked = on; $("hm-on").onchange(); }
  // The Layers panel switches all radio paths on and off.
  function setPathsVisible(on) { paths.forEach((p) => { if (on) p.layer.addTo(map); else map.removeLayer(p.layer); }); float.classList.toggle("hidden", !on || !paths.length); }
  const api = { heightAt, on: false, syncHeights, startProfile, cancelProfile, showHeights, setPathsVisible, get heightsShown() { return map.hasLayer(layer); }, onMode: null };
  window.DKTerrain = api;
})();
