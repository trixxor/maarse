// Real-world mode: the real network on the map, from backbone hosts that
// share their map (maarse-host --public-map, GET /api/public/map).
//
// Each host reports itself (position, site), the nodes it knows that share
// a position (with their site: antenna height, gain, power, owner) and the
// radio links its own radio hears (SNR, RSSI, packets, last heard). Here:
// * the nodes as markers, the measured links as lines coloured by SNR,
// * coverage of the real sites with the simulator's radio model,
// * the model checked against the measured links: the mean difference is
//   offered as a correction for the coverage picture.
// Server addresses are kept in this browser only.
(function () {
  const dk = window.DKSim, C = window.DKCoverage;
  if (!dk || !dk.map) return;
  const map = dk.map;
  const $ = (id) => document.getElementById(id);
  const esc = (t) => String(t ?? "").replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
  const STORE = "maarse.realworld.servers";
  const ROLE_H = { Repeater: 15, Anchor: 15, Relay: 2, Client: 1.5 };   // when a node has no site
  let servers = [];
  try { servers = JSON.parse(localStorage.getItem(STORE) || "[]"); } catch { /* storage blocked */ }

  const layer = L.layerGroup();
  let data = { nodes: new Map(), links: [] }, timer = 0, calib = null;

  const card = document.createElement("div");
  card.id = "rw-card"; card.className = "ov hidden";
  card.style.cssText = "left:54px;top:84px;width:320px;max-height:calc(100% - 160px);overflow:auto;padding:8px 10px;font-size:12px";
  card.innerHTML = `
    <h4 style="margin:0 0 6px;display:flex;justify-content:space-between;align-items:center">Real world <button id="rw-x" style="border:0;background:transparent" title="Close (the map stays until you hide it)">✕</button></h4>
    <p class="hint" style="margin:0 0 6px">The real network, from backbone hosts that share their map (<code>--public-map</code>). Fixed nodes show the site their owner set: antenna height, gain, power, owner.</p>
    <label>Hosts (one per line, e.g. https://host.example.dk:8080)</label>
    <textarea id="rw-servers" rows="3" style="width:100%;box-sizing:border-box;font-size:12px">${esc(servers.join("\n"))}</textarea>
    <div style="display:flex;gap:4px;flex-wrap:wrap;margin-top:4px">
      <button id="rw-load" class="primary">Show</button>
      <button id="rw-cov" title="Coverage of the real fixed nodes, with the simulator's radio model">Coverage</button>
      <button id="rw-hide">Hide</button>
    </div>
    <p id="rw-note" class="hint" style="margin:6px 0 2px"></p>
    <div id="rw-cal"></div>
    <div style="margin-top:4px;display:flex;gap:8px;flex-wrap:wrap" class="hint">
      <span><span style="display:inline-block;width:16px;height:3px;background:#16a34a;vertical-align:middle"></span> SNR ≥ 5 dB</span>
      <span><span style="display:inline-block;width:16px;height:3px;background:#eab308;vertical-align:middle"></span> −5…5</span>
      <span><span style="display:inline-block;width:16px;height:3px;background:#dc2626;vertical-align:middle"></span> &lt; −5</span>
    </div>`;
  $("mapwrap").appendChild(card);
  L.DomEvent.disableClickPropagation(card); L.DomEvent.disableScrollPropagation(card);
  $("rw-x").onclick = () => card.classList.add("hidden");
  $("rw-hide").onclick = () => { clearInterval(timer); timer = 0; map.removeLayer(layer); C && C.clear(); $("rw-note").textContent = "Hidden."; };
  $("rw-load").onclick = () => {
    servers = $("rw-servers").value.split(/\s+/).map((x) => x.trim().replace(/\/+$/, "")).filter(Boolean);
    try { localStorage.setItem(STORE, JSON.stringify(servers)); } catch { /* storage blocked */ }
    refresh();
    clearInterval(timer); timer = setInterval(refresh, 60000);
  };
  $("rw-cov").onclick = () => coverage();

  async function refresh() {
    if (!servers.length) { $("rw-note").textContent = "Add at least one host."; return; }
    $("rw-note").textContent = "Fetching…";
    const nodes = new Map(), links = [], errors = [];
    await Promise.all(servers.map(async (url) => {
      try {
        const r = await fetch(url + "/api/public/map", { cache: "no-store" });
        if (!r.ok) throw new Error(r.status === 404 ? "does not share its map (start it with --public-map)" : "answered " + r.status);
        const j = await r.json();
        const h = j.host || {};
        if (h.addr && h.lat != null) nodes.set(h.addr, Object.assign({ role: "Host", host: url }, h, nodes.get(h.addr)));
        for (const n of j.nodes || []) nodes.set(n.addr, Object.assign({}, nodes.get(n.addr), n));
        for (const l of j.links || []) links.push(Object.assign({ from: h.addr, server: url }, l));
      } catch (e) { errors.push(`${url}: ${e.message}`); }
    }));
    // Links name the neighbour by address, or only by its path id.
    const byPath = new Map([...nodes.values()].filter((n) => n.path_id != null).map((n) => [n.path_id, n.addr]));
    for (const l of links) l.to = l.addr || byPath.get(l.path_id) || null;
    data = { nodes, links };
    draw();
    const shown = links.filter((l) => l.to && nodes.has(l.to) && nodes.has(l.from)).length;
    $("rw-note").textContent = `${nodes.size} nodes with a position, ${shown} of ${links.length} measured links on the map. Updated ${new Date().toLocaleTimeString()}.` + (errors.length ? " Problems: " + errors.join("; ") : "");
    calibrate();
  }

  const snrCol = (s) => s >= 5 ? "#16a34a" : s >= -5 ? "#eab308" : "#dc2626";
  function draw() {
    layer.clearLayers();
    layer.addTo(map);
    for (const l of data.links) {
      const a = data.nodes.get(l.from), b = data.nodes.get(l.to);
      if (!a || !b) continue;
      L.polyline([[a.lat, a.lon], [b.lat, b.lon]], { color: snrCol(l.snr), weight: 3, opacity: 0.85 })
        .bindTooltip(`${esc(a.name || a.addr.slice(0, 8))} ↔ ${esc(b.name || b.addr.slice(0, 8))}<br>SNR ${l.snr} dB · RSSI ${l.rssi} dBm · ${l.packets} packets · ${fmtAge(l.age_s)} ago`, { sticky: true })
        .addTo(layer);
    }
    for (const n of data.nodes.values()) {
      const fixed = n.height_m != null || n.role === "Host" || n.role === "Repeater" || n.role === "Anchor";
      L.circleMarker([n.lat, n.lon], { radius: fixed ? 7 : 5, color: "#111", weight: 2, fillColor: n.role === "Host" ? "#7c3aed" : fixed ? "#2563eb" : "#f97316", fillOpacity: 1 })
        .bindPopup(`<b>${esc(n.name || n.addr.slice(0, 12))}</b><br>${esc(n.role || "node")}${n.mobile ? " · mobile" : ""}<br>` +
          (n.height_m != null ? `Antenna ${n.height_m} m · ${n.gain_dbi ?? 0} dBi · ${n.tx_dbm ?? "?"} dBm<br>` : `No site set (antenna height guessed from the role)<br>`) +
          (n.owner ? `Owner: ${esc(n.owner)}<br>` : "") + (n.last_seen ? `Last heard ${fmtAge(Date.now() / 1000 - n.last_seen)} ago` : ""))
        .addTo(layer);
    }
  }
  function fmtAge(s) { s = Math.max(0, Math.round(s)); return s < 90 ? s + " s" : s < 5400 ? Math.round(s / 60) + " min" : s < 172800 ? Math.round(s / 3600) + " h" : Math.round(s / 86400) + " days"; }
  const siteOf = (n) => ({ lat: n.lat, lon: n.lon, h: n.height_m ?? ROLE_H[n.role] ?? 2, gain: n.gain_dbi ?? (n.height_m != null || n.role === "Host" ? 6 : 0), tx: n.tx_dbm ?? undefined, name: n.name || n.addr.slice(0, 8) });

  function coverage() {
    if (!C) return;
    const fixed = [...data.nodes.values()].filter((n) => !n.mobile && (n.height_m != null || ["Host", "Repeater", "Anchor"].includes(n.role)));
    if (!fixed.length) { $("rw-note").textContent = "No fixed nodes with a position yet: press Show first."; return; }
    C.setSites(fixed.map(siteOf));
  }

  // Model versus measurement: predicted SNR for every measured link whose
  // ends both have a position; the mean difference is the correction.
  async function calibrate() {
    if (!C || !C.predictRx) return;
    const rows = [];
    for (const l of data.links) {
      const a = data.nodes.get(l.from), b = data.nodes.get(l.to);
      if (!a || !b || l.snr == null) continue;
      const rx = await C.predictRx(siteOf(b), siteOf(a));
      rows.push({ a, b, measured: l.snr, predicted: rx - C.noise, km: map.distance([a.lat, a.lon], [b.lat, b.lon]) / 1000 });
    }
    if (!rows.length) { $("rw-cal").innerHTML = ""; calib = null; return; }
    // SNR saturates near the top; strong links say little about the model.
    const use = rows.filter((r) => r.measured < 10);
    const diff = (use.length ? use : rows).map((r) => r.measured - r.predicted);
    calib = diff.reduce((x, y) => x + y, 0) / diff.length;
    $("rw-cal").innerHTML = `<h4 style="margin:6px 0 2px;font-size:12px">Model versus measured</h4><table>${rows.slice(0, 8).map((r) =>
      `<tr><td>${esc(r.a.name || r.a.addr.slice(0, 6))} ↔ ${esc(r.b.name || r.b.addr.slice(0, 6))}</td><td class="n">${r.km.toFixed(1)} km</td><td class="n">${r.measured} / ${r.predicted.toFixed(0)} dB</td></tr>`).join("")}</table>
      <p class="hint">Measured / predicted SNR. On average reality is <b>${calib >= 0 ? "+" : ""}${calib.toFixed(1)} dB</b> from the model.</p>
      <button id="rw-apply">Use ${calib >= 0 ? "+" : ""}${calib.toFixed(1)} dB in the coverage picture</button>`;
    $("rw-apply").onclick = () => { C.setCalibration(calib); $("rw-note").textContent = `Coverage corrected by ${calib.toFixed(1)} dB from ${diff.length} measured links.`; };
  }

  window.DKRealWorld = { show: () => { card.classList.remove("hidden"); if (servers.length && !timer) $("rw-load").click(); } };
})();
