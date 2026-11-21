// Real network, measured part: the measured radio links of the backbone
// servers (coloured by SNR), the coverage of their fixed nodes with the
// simulator's radio model, and the model checked against the measurements.
// The servers share this with Settings: Share the public map
// (GET /api/public/map). The Real network view (overview.js) shows the nodes,
// packets, channels and history, and owns the server list shown here.
// Server addresses are kept in this browser only.
(function () {
  const dk = window.DKSim, C = window.DKCoverage;
  if (!dk || !dk.map) return;
  const map = dk.map;
  const $ = (id) => document.getElementById(id);
  const esc = (t) => String(t ?? "").replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
  const STORE = "maarse.realworld.servers";
  const DEFAULT_COLLECTOR = "https://nj01.dk.eu.maarse.dk";
  const ROLE_H = { Repeater: 15, Anchor: 15, Relay: 2, Client: 1.5 };   // when a node has no site
  let servers = [];
  try { servers = JSON.parse(localStorage.getItem(STORE) || "[]"); } catch { /* storage blocked */ }

  const layer = L.layerGroup();
  let data = { nodes: new Map(), links: [], hosts: new Map() }, timer = 0, calib = null;
  let mapRequest = 0;

  const card = document.createElement("div");
  card.id = "rw-card"; card.className = "ov hidden";
  card.style.cssText = "left:54px;top:84px;width:320px;max-height:calc(100% - 160px);overflow:auto;padding:8px 10px;font-size:12px";
  card.innerHTML = `
    <h4 style="margin:0 0 6px;display:flex;justify-content:space-between;align-items:center">Servers and coverage <button id="rw-x" style="border:0;background:transparent" title="Close">✕</button></h4>
    <p class="hint" style="margin:0 0 6px">The backbone servers the Real network view reads. A server shares its map with <b>Share the public map</b> in its Settings. Fixed nodes show the site their owner set: antenna height, gain, power, owner.</p>
    <label>Servers (one per line, e.g. https://host.example.dk:8080)</label>
    <textarea id="rw-servers" rows="3" style="width:100%;box-sizing:border-box;font-size:12px">${esc((servers.length ? servers : [DEFAULT_COLLECTOR]).join("\n"))}</textarea>
    <div style="display:flex;gap:4px;flex-wrap:wrap;margin-top:4px">
      <button id="rw-load" class="primary">Apply</button>
      <button id="rw-cov" title="Coverage of the real fixed nodes, with the simulator's radio model">Coverage</button>
      <button id="rw-hide" hidden>Hide</button>
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
  $("rw-hide").onclick = () => {
    clearInterval(timer); timer = 0; mapRequest++;
    map.removeLayer(layer); C && C.clear();
    $("rw-note").textContent = "Hidden.";
  };
  $("rw-load").onclick = () => {
    servers = [...new Set($("rw-servers").value.split(/\s+/).map((x) => x.trim().replace(/\/+$/, "")).filter(Boolean))];
    try { localStorage.setItem(STORE, JSON.stringify(servers)); } catch { /* storage blocked */ }
    data.hosts.clear();
    clearInterval(timer);
    if (!servers.length) {
      timer = 0; mapRequest++; map.removeLayer(layer);
      $("rw-note").textContent = "Add at least one host.";
      return;
    }
    timer = setInterval(refresh, 10000);
    refresh(true);
    window.dispatchEvent(new Event("dk-servers"));
  };
  $("rw-cov").onclick = () => coverage();

  async function refresh(fit = false) {
    const request = ++mapRequest;
    $("rw-note").textContent = "Fetching…";
    const nodes = new Map(), links = [], errors = [], hosts = new Map();
    await Promise.all(servers.map(async (url) => {
      try {
        const r = await fetch(url + "/api/public/map", { cache: "no-store", credentials: "omit", signal: AbortSignal.timeout(4500) });
        if (!r.ok) throw new Error(r.status === 404 ? "does not share its map (turn on Share the public map in its Settings)" : "answered " + r.status);
        const j = await r.json();
        const h = j.host || {};
        hosts.set(url, h);
        for (const n of j.nodes || []) nodes.set(n.addr, Object.assign({}, nodes.get(n.addr), n));
        for (const l of j.links || []) links.push(Object.assign({ from: h.addr, server: url }, l));
      } catch (e) { errors.push(`${url}: ${e.message}`); }
    }));
    if (request !== mapRequest) return;
    for (const h of hosts.values()) {
      if (h.addr && locationOf(h)) nodes.set(h.addr, { ...nodes.get(h.addr), ...h, role: "Host" });
    }
    // Links name the neighbour by address, or only by its path id.
    const byPath = new Map([...nodes.values()].filter((n) => n.path_id != null).map((n) => [n.path_id, n.addr]));
    for (const l of links) l.to = l.addr || byPath.get(l.path_id) || null;
    data = { nodes, links, hosts };
    draw();
    const shown = links.filter((l) => l.to && nodes.has(l.to) && nodes.has(l.from)).length;
    $("rw-note").textContent = `${nodes.size} nodes with a position, ${shown} of ${links.length} measured links on the map. Updated ${new Date().toLocaleTimeString()}.` + (errors.length ? " Problems: " + errors.join("; ") : "");
    calibrate();
  }

  function locationOf(host) {
    return Number.isFinite(host.lat) && Math.abs(host.lat) <= 90 && Number.isFinite(host.lon) && Math.abs(host.lon) <= 180
      ? [host.lat, host.lon] : null;
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

  // The Real network view starts and stops the measured part, and opens the card.
  window.DKRealWorld = {
    start: () => { if (!timer) $("rw-load").click(); },
    stop: () => { $("rw-hide").click(); card.classList.add("hidden"); },
    coverage: () => coverage(),
    toggleCard: () => card.classList.toggle("hidden"),
    get card() { return card; },
    get nodes() { return data.nodes; },
  };
})();
