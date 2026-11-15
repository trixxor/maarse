// Må:rse Denmark Simulator: map editor, controls and views around the
// WebAssembly engine (maarse-sim-wasm, real maarse-core Router per node).
(function () {
  "use strict";
  const $ = (id) => document.getElementById(id);
  const esc = (s) => String(s).replace(/[&<>"]/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));
  const DK = window.DkScenarios;

  const KIND_COLOR = { handheld: "#2563eb", relay: "#7c3aed", repeater: "#ea580c", anchor: "#b91c1c", host: "#0f766e", wifi: "#ca8a04", mobile: "#db2777" };
  const KIND_R = { handheld: 3, relay: 3.5, repeater: 5, anchor: 7, host: 6, wifi: 5, mobile: 5 };
  const PTYPES = ["Announce", "PathRequest", "PathResponse", "Data", "Ack", "Channel", "Bulletin", "Carry", "Fragment"];
  const PCOLOR = { Announce: "#a3a3a3", PathRequest: "#f59e0b", PathResponse: "#10b981", Data: "#2563eb", Ack: "#22c55e", Channel: "#8b5cf6", Bulletin: "#ef4444", Carry: "#ec4899", Fragment: "#0ea5e9" };
  const FEATURES = [["carry", "Carry (store-carry-forward)"], ["local_repair", "Local repair"], ["backbone", "Backbone links"], ["regions", "Regions"], ["sf_slot", "SF-sized flood slot"], ["scale_timeouts", "Scaled timeouts"], ["load_control", "Load control"], ["area_scope", "Area scope for alerts (sim layer)"]];

  let Engine = null;        // { Sim }
  let sim = null;           // running engine
  let scenario = null;      // editable scenario JSON
  let playing = false;
  let tool = "select";
  let selected = -1;
  let lastSeq = 0;
  let pendingArea = null;   // { kind: "circle"|"polygon", purpose, pts: [] }
  let routeDraft = null;
  let linkDraft = -1;
  let state = null;
  let txType = new Map();
  let overlay = null;       // reach / alert / follow highlight: Map(node -> color)
  const animOn = {};
  PTYPES.forEach(p => animOn[p] = true);

  // ---------- map ----------
  const map = L.map("map", { preferCanvas: true, zoomControl: true }).setView([56.1, 11.2], 7);
  const canvas = L.canvas({ padding: 0.2 });
  L.tileLayer("https://tile.openstreetmap.org/{z}/{x}/{y}.png", { maxZoom: 17, attribution: "© OpenStreetMap contributors (tiles need internet); coastline: Natural Earth (public domain)" }).addTo(map);
  L.geoJSON(window.DK_LAND, { style: { color: "#64748b", weight: 1, fillColor: "#a3b18a", fillOpacity: 0.25 }, interactive: false }).addTo(map);
  const nodeLayer = L.layerGroup().addTo(map);
  const linkLayer = L.layerGroup().addTo(map);
  const animLayer = L.layerGroup().addTo(map);
  const drawLayer = L.layerGroup().addTo(map);
  const hiLayer = L.layerGroup().addTo(map);
  let markers = [];

  function compColor(c) {
    if (c < 0) return "#9ca3af";
    const h = (c * 137.508) % 360;
    return `hsl(${h},70%,45%)`;
  }

  function drawNodes() {
    nodeLayer.clearLayers();
    markers = scenario.nodes.map((n, i) => {
      const m = L.circleMarker([n.lat, n.lon], { renderer: canvas, radius: KIND_R[n.kind] || 3, weight: 1, color: "#111", fillColor: KIND_COLOR[n.kind], fillOpacity: 0.9 });
      m.on("click", (e) => { L.DomEvent.stopPropagation(e); onNodeClick(i); });
      m.on("mousedown", (e) => startDrag(i, e));
      m.bindTooltip(() => `${i}: ${n.label || n.kind}`, { sticky: true });
      m.addTo(nodeLayer);
      return m;
    });
    drawLinks();
    restyle();
  }

  function drawLinks() {
    linkLayer.clearLayers();
    if ($("opt-backbone").checked) {
      scenario.links.forEach((l, k) => {
        const a = scenario.nodes[l.a], b = scenario.nodes[l.b];
        if (!a || !b) return;
        const ok = state && state.links_ok ? state.links_ok[k] : l.up;
        const pl = L.polyline([[a.lat, a.lon], [b.lat, b.lon]], { color: l.kind === "wifi" ? "#ca8a04" : "#0f766e", weight: 2.5, opacity: ok ? 0.75 : 0.25, dashArray: ok ? null : "4 6" });
        pl.bindTooltip(`${l.kind.toUpperCase()} link ${k}: ${l.a} ↔ ${l.b} ${ok ? "(up)" : "(down)"} — click to toggle`);
        pl.on("click", (e) => { L.DomEvent.stopPropagation(e); toggleLink(k); });
        pl.addTo(linkLayer);
      });
    }
    if ($("opt-links").checked && selected >= 0 && sim) {
      const ins = sim.inspect(selected);
      const me = scenario.nodes[selected];
      for (const [j, snr] of ins.radio_neighbors) {
        const o = scenario.nodes[j];
        L.polyline([[me.lat, me.lon], [o.lat, o.lon]], { color: snr > 0 ? "#16a34a" : snr > -8 ? "#ca8a04" : "#dc2626", weight: 1, opacity: 0.6, interactive: false }).bindTooltip(`SNR ${snr} dB`).addTo(linkLayer);
      }
    }
  }

  function restyle() {
    const byIsland = $("opt-islands").checked;
    markers.forEach((m, i) => {
      const n = scenario.nodes[i];
      let fill = KIND_COLOR[n.kind], stroke = "#111", w = 1, op = 0.9;
      if (state && state.state) {
        const s = state.state[i];
        if (byIsland && state.comp) fill = compColor(state.comp[i]);
        if (s === "b") { stroke = "#f97316"; w = 2.5; }
        if (s !== "a" && s !== "b") { fill = "#9ca3af"; op = 0.5; }
        if (state.lat && n.kind === "mobile") m.setLatLng([state.lat[i], state.lon[i]]);
        else if (state.lat && n.route && n.route.length > 1) m.setLatLng([state.lat[i], state.lon[i]]);
      }
      if (overlay && overlay.has(i)) { fill = overlay.get(i); op = 1; }
      else if (overlay) { op = 0.25; }
      if (i === selected) { stroke = "#000"; w = 3; }
      m.setStyle({ fillColor: fill, color: stroke, weight: w, fillOpacity: op });
    });
  }

  // ---------- engine lifecycle ----------
  function rebuild() {
    if (sim) sim.free();
    sim = null;
    try { sim = new Engine.Sim(scenario); }
    catch (e) { alert("Scenario error: " + e.message); return; }
    lastSeq = 0; txType = new Map(); state = sim.state(); overlay = null; hiLayer.clearLayers(); animLayer.clearLayers();
    drawNodes(); updateClock(); renderStats(); renderTimeline(); fillSenders(); renderAlerts();
    $("sc-info").textContent = `${scenario.name || "scenario"}: ${scenario.nodes.length} nodes, ${scenario.links.length} backbone links, ${scenario.events.length} timeline events.`;
  }
  function edited() { rebuild(); }

  function updateClock() {
    const t = sim ? sim.summary().t / 1000 : 0;
    $("clock").textContent = `t = ${fmtTime(t)}`;
  }
  function fmtTime(s) { const m = Math.floor(s / 60); return m ? `${m} min ${Math.floor(s % 60)} s` : `${s.toFixed(1)} s`; }
  function now() { return sim ? sim.summary().t : 0; }

  // ---------- main loop ----------
  let lastFrame = 0, lastState = 0, lastStats = 0;
  function frame(ts) {
    requestAnimationFrame(frame);
    if (!sim) return;
    const dt = lastFrame ? Math.min(100, ts - lastFrame) : 16;
    lastFrame = ts;
    if (playing) {
      const speed = +$("speed").value;
      const t0 = performance.now();
      const r = sim.step(Math.max(1, Math.round(dt * speed)), scenario.nodes.length > 1500 ? 150000 : 400000);
      $("behind").classList.toggle("hidden", !(r.behind || performance.now() - t0 > 40));
      pullEvents();
    }
    if (ts - lastState > 400) { lastState = ts; state = sim.state(); restyle(); updateClock(); }
    if (ts - lastStats > 1000) { lastStats = ts; renderStats(); if (selected >= 0) renderInspector(); if (playing) drawLinks(); }
    fadeAnim();
  }

  // ---------- animation ----------
  const anims = [];
  function pullEvents() {
    const r = sim.events(lastSeq, 4000);
    lastSeq = r.next;
    if (!$("anim-on").checked) return;
    let drawn = 0;
    const bounds = map.getBounds().pad(0.2);
    for (const e of r.lines) {
      if (e.ev === "tx") { txType.set(e.id, e.p); if (txType.size > 20000) txType = new Map(); continue; }
      if (e.ev !== "rx" || drawn > 300) continue;
      const p = txType.get(e.id) || "Data";
      if (!animOn[p]) continue;
      const s = scenario.nodes[e.n];
      if (!s) continue;
      const sp = state && state.lat ? [state.lat[e.n], state.lon[e.n]] : [s.lat, s.lon];
      if (!bounds.contains(sp)) continue;
      drawn++;
      const col = PCOLOR[p] || "#666";
      if (e.ok.length > 25) {
        anims.push({ l: L.circleMarker(sp, { renderer: canvas, radius: 14, color: col, weight: 2, fill: false, interactive: false }).addTo(animLayer), t: performance.now() });
      } else {
        for (const r2 of e.ok) {
          const o = scenario.nodes[r2];
          const op = state && state.lat ? [state.lat[r2], state.lon[r2]] : [o.lat, o.lon];
          anims.push({ l: L.polyline([sp, op], { renderer: canvas, color: col, weight: 2, opacity: 0.85, interactive: false }).addTo(animLayer), t: performance.now() });
        }
      }
      for (const r2 of e.coll.slice(0, 10)) {
        const o = scenario.nodes[r2];
        anims.push({ l: L.circleMarker([o.lat, o.lon], { renderer: canvas, radius: 6, color: "#dc2626", weight: 2, fill: false, interactive: false }).addTo(animLayer), t: performance.now() });
      }
    }
  }
  function fadeAnim() {
    const t = performance.now();
    while (anims.length && t - anims[0].t > 900) animLayer.removeLayer(anims.shift().l);
    if (anims.length > 3000) anims.splice(0, anims.length - 3000).forEach(a => animLayer.removeLayer(a.l));
  }

  // ---------- tools ----------
  $("tools").addEventListener("click", (e) => {
    const b = e.target.closest("button"); if (!b) return;
    setTool(b.dataset.tool);
  });
  function setTool(t) {
    tool = t; routeDraft = null; linkDraft = -1;
    document.querySelectorAll("#tools button").forEach(x => x.classList.toggle("on", x.dataset.tool === t));
    const hints = { select: "Click a node to inspect it; drag to move.", delete: "Click a node to delete it (its links go too).", mobile: "Click route points; double-click to finish. The car starts at the first point.", "link-tcp": "Click two hosts to link them over TCP (needs internet on both).", "link-wifi": "Click two nodes for a Wi-Fi point-to-point link (no internet needed)." };
    $("tool-hint").textContent = hints[t] || "Click on the map to place it. Edit its settings in the inspector.";
  }

  map.on("click", (e) => {
    const ll = [+e.latlng.lat.toFixed(5), +e.latlng.lng.toFixed(5)];
    if (pendingArea) return areaClick(ll);
    if (tool === "mobile") {
      routeDraft = routeDraft || [];
      routeDraft.push(ll);
      drawLayer.clearLayers();
      L.polyline(routeDraft, { color: KIND_COLOR.mobile, dashArray: "4 4" }).addTo(drawLayer);
      return;
    }
    if (KIND_COLOR[tool]) {
      const town = DK.nearestTown(window.DK_TOWNS, ll[0], ll[1]);
      scenario.nodes.push({ kind: tool, lat: ll[0], lon: ll[1], region: town[4] });
      selected = scenario.nodes.length - 1;
      edited(); renderInspector();
    }
  });
  map.on("dblclick", (e) => {
    if (pendingArea && pendingArea.kind === "polygon") { L.DomEvent.stop(e); return finishArea(); }
    if (tool === "mobile" && routeDraft && routeDraft.length >= 2) {
      L.DomEvent.stop(e);
      scenario.nodes.push({ kind: "mobile", lat: routeDraft[0][0], lon: routeDraft[0][1], route: routeDraft, speed_kmh: 90, label: "Car" });
      routeDraft = null; drawLayer.clearLayers();
      selected = scenario.nodes.length - 1; edited(); renderInspector();
    }
  });
  map.doubleClickZoom.disable();

  function onNodeClick(i) {
    if (pendingArea) return;
    if (tool === "delete") return deleteNode(i);
    if (tool === "link-tcp" || tool === "link-wifi") {
      if (linkDraft < 0) { linkDraft = i; $("tool-hint").textContent = `Link from ${i}: click the other end.`; return; }
      if (linkDraft !== i) { scenario.links.push({ a: linkDraft, b: i, kind: tool === "link-tcp" ? "tcp" : "wifi", up: true }); if (tool === "link-tcp") { scenario.nodes[linkDraft].internet = true; scenario.nodes[i].internet = true; } }
      linkDraft = -1; edited(); return;
    }
    selected = i; overlay = null; hiLayer.clearLayers(); restyle(); drawLinks(); renderInspector();
  }
  function deleteNode(i) {
    scenario.nodes.splice(i, 1);
    scenario.links = scenario.links.filter(l => l.a !== i && l.b !== i).map(l => ({ ...l, a: l.a > i ? l.a - 1 : l.a, b: l.b > i ? l.b - 1 : l.b }));
    selected = -1; $("inspector").classList.add("hidden"); edited();
  }
  let drag = null;
  function startDrag(i, e) {
    if (tool !== "select" || pendingArea) return;
    drag = { i, moved: false }; map.dragging.disable();
  }
  map.on("mousemove", (e) => { if (drag) { drag.moved = true; markers[drag.i].setLatLng(e.latlng); } });
  map.on("mouseup", (e) => {
    if (!drag) return;
    map.dragging.enable();
    if (drag.moved) { const n = scenario.nodes[drag.i]; n.lat = +e.latlng.lat.toFixed(5); n.lon = +e.latlng.lng.toFixed(5); selected = drag.i; edited(); renderInspector(); }
    drag = null;
  });

  function toggleLink(k) {
    const up = !(state && state.links_ok ? state.links_ok[k] : scenario.links[k].up);
    if (sim && now() > 0) addEvent({ type: "link", index: k, up });
    else { scenario.links[k].up = up; edited(); }
  }

  // ---------- areas (alerts, power cut) ----------
  function beginArea(kind, purpose) {
    pendingArea = { kind, purpose, pts: [] };
    drawLayer.clearLayers();
    $("tool-hint").textContent = kind === "circle" ? "Click the centre, then click the edge." : "Click the corners; double-click to finish.";
    map.getContainer().style.cursor = "crosshair";
  }
  function areaClick(ll) {
    const a = pendingArea;
    a.pts.push(ll);
    drawLayer.clearLayers();
    if (a.kind === "circle") {
      if (a.pts.length === 2) return finishArea();
      L.circleMarker(ll, { radius: 4 }).addTo(drawLayer);
    } else {
      L.polygon(a.pts, { color: "#dc2626" }).addTo(drawLayer);
    }
  }
  function finishArea() {
    const a = pendingArea; pendingArea = null;
    map.getContainer().style.cursor = "";
    drawLayer.clearLayers();
    let area;
    if (a.kind === "circle") {
      if (a.pts.length < 2) return;
      const r = Math.round(DK.km(a.pts[0], a.pts[1]) * 1000);
      area = { circle: [a.pts[0][0], a.pts[0][1], r] };
    } else {
      if (a.pts.length < 3) return;
      area = { polygon: a.pts };
    }
    if (a.purpose === "alert") addEvent({ type: "alert", from: +$("al-from").value, alert: $("al-kind").value, text: $("al-text").value, area });
    else addEvent({ type: "power_cut", area });
    setTool(tool);
  }

  // ---------- timeline / disaster ----------
  function addEvent(ev) {
    const at = $("ev-at").value;
    const t = sim ? now() / 1000 : 0;
    ev.t_s = at !== "" ? +at : +t.toFixed(1);
    scenario.events.push(ev);
    if (sim && ev.t_s <= t + 0.01) {
      sim.inject(ev);
      state = sim.state(); restyle(); drawLinks(); renderAlerts();
    } else if (sim && ev.t_s > t) {
      // Future event: it must be in the engine's schedule, so rebuild keeps it consistent.
      if (t === 0) rebuild(); else alert("Scheduled for t = " + ev.t_s + " s. It takes effect after Reset (replay).");
    }
    renderTimeline();
  }
  document.querySelectorAll("#tab-disaster button[data-ev]").forEach(b => b.addEventListener("click", () => {
    const k = b.dataset.ev;
    if (k === "internet_down_region") return addEvent({ type: "internet_down", region: $("ev-region").value });
    if (k === "power_cut") return beginArea("polygon", "power");
    if (k === "kill_random") return addEvent({ type: "kill_random", n: +$("ev-n").value, node_kind: "repeater" });
    addEvent({ type: k });
  }));
  function describe(ev) {
    switch (ev.type) {
      case "internet_down": return ev.region ? `Internet down in ${ev.region}` : "Internet down everywhere";
      case "internet_up": return "Internet back";
      case "power_cut": return "Power cut in area";
      case "power_restore": return "Power back";
      case "kill_random": return `Kill ${ev.n} random ${ev.node_kind || "repeater"}s`;
      case "link": return `Backbone link ${ev.index} ${ev.up ? "up" : "down"}`;
      case "alert": return `${ev.alert} alert from node ${ev.from}: "${ev.text || ""}"`;
      case "announce": return `Announce from node ${ev.node}`;
      case "set_traffic": return "Change traffic" + (ev.traffic && ev.traffic.spam_per_min ? ` (spam ${ev.traffic.spam_per_min}/min from ${ev.traffic.spammers.length} nodes)` : "");
      default: return ev.type;
    }
  }
  function renderTimeline() {
    const evs = scenario.events.map((e, k) => [e, k]).sort((a, b) => a[0].t_s - b[0].t_s);
    $("timeline").innerHTML = evs.length ? `<table>${evs.map(([e, k]) => `<tr><td><input type="number" value="${e.t_s}" data-k="${k}" style="width:70px"> s</td><td>${esc(describe(e))}</td><td><button data-del="${k}">✕</button></td></tr>`).join("")}</table><p class="hint">Edit times, then Reset to replay.</p>` : `<p class="hint">No events.</p>`;
    $("timeline").querySelectorAll("input").forEach(i => i.addEventListener("change", () => { scenario.events[+i.dataset.k].t_s = +i.value; renderTimeline(); }));
    $("timeline").querySelectorAll("button[data-del]").forEach(b => b.addEventListener("click", () => { scenario.events.splice(+b.dataset.del, 1); renderTimeline(); }));
  }

  // ---------- alerts ----------
  function fillSenders() {
    const sel = $("al-from"); const cur = sel.value;
    sel.innerHTML = scenario.nodes.map((n, i) => [n, i]).filter(([n]) => n.kind === "host" || n.kind === "anchor").map(([n, i]) => `<option value="${i}">${i}: ${esc(n.label || n.kind)}</option>`).join("") || scenario.nodes.map((n, i) => `<option value="${i}">${i}: ${esc(n.label || n.kind)}</option>`).join("");
    if (cur) sel.value = cur;
  }
  $("btn-al-circle").onclick = () => beginArea("circle", "alert");
  $("btn-al-poly").onclick = () => beginArea("polygon", "alert");
  function renderAlerts() {
    if (!sim) return;
    const s = sim.summary();
    $("alert-list").innerHTML = s.alerts.length ? `<h3>Sent alerts</h3><table><tr><th>#</th><th>type</th><th>in area</th><th>coverage</th><th>median</th><th>outside</th></tr>${s.alerts.map(a => `<tr class="click" data-a="${a.index}"><td>${a.index}</td><td>${esc(a.kind)}</td><td class="n">${a.inside_received}/${a.inside_alive}</td><td class="n">${a.coverage_pct.toFixed(0)} %</td><td class="n">${(a.latency_median_ms / 1000).toFixed(1)} s</td><td class="n">${a.outside_received}</td></tr>`).join("")}</table><p class="hint">Click an alert for the latency heat map (green fast → red slow, black = missed). "Outside" = nodes outside the area that also got it.</p>` : "";
    $("alert-list").querySelectorAll("tr[data-a]").forEach(r => r.onclick = () => showAlert(+r.dataset.a));
  }
  function heat(f) { const h = 120 - 120 * Math.min(1, Math.max(0, f)); return `hsl(${h},85%,45%)`; }
  function showAlert(i) {
    const a = sim.alert(i);
    overlay = new Map();
    const max = Math.max(1, ...a.recv.filter(r => r[2]).map(r => r[1]));
    for (const n of a.inside_nodes) overlay.set(n, "#111");
    for (const [n, ms, inside] of a.recv) overlay.set(n, inside ? heat(ms / max) : "#93c5fd");
    restyle();
    drawAreaOutline(scenario.events.filter(e => e.type === "alert")[i]);
  }
  function drawAreaOutline(ev) {
    hiLayer.clearLayers();
    if (!ev) return;
    if (ev.area.circle) L.circle([ev.area.circle[0], ev.area.circle[1]], { radius: ev.area.circle[2], color: "#dc2626", fill: false }).addTo(hiLayer);
    else L.polygon(ev.area.polygon, { color: "#dc2626", fill: false }).addTo(hiLayer);
  }

  // ---------- stats ----------
  function renderStats() {
    if (!sim) return;
    const s = sim.summary();
    const k = (v, l) => `<div><b>${v}</b><span>${l}</span></div>`;
    const prio = ["control", "direct/bulletin", "channel", "bulk", "flood"];
    $("stats").innerHTML = `<div class="kpi">
      ${k(s.delivered_pct.toFixed(1) + " %", "delivered")}${k(s.acked_pct.toFixed(1) + " %", "acked")}${k(s.messages, "direct messages")}
      ${k((s.latency_median_ms / 1000).toFixed(1) + " s", "latency median")}${k((s.latency_p90_ms / 1000).toFixed(1) + " s", "latency p90")}${k(s.channel_reach_pct.toFixed(0) + " %", "channel reach")}
      ${k(s.alive + "/" + s.nodes, "nodes alive")}${k(s.tx, "transmissions")}${k(s.airtime_s.toFixed(0) + " s", "total airtime")}
      ${k(s.collisions, "lost to collisions")}${k(s.duty_hits, "duty-cycle hits")}${k(s.duty_drops + s.busy_drops, "dropped (duty/busy)")}
      ${k(s.local_repairs, "local repairs")}${k(s.carry_handoffs + "/" + s.carry_delivered, "carry handoffs/deliv.")}${k(s.backbone_packets, "backbone packets")}
    </div>
    <h3>Queue delay per priority</h3>
    <table><tr><th>priority</th><th>sent</th><th>mean</th><th>max</th></tr>${s.queue_delay.map((q, i) => `<tr><td>${i} ${prio[i]}</td><td class="n">${q.count}</td><td class="n">${q.mean_ms.toFixed(0)} ms</td><td class="n">${q.max_ms} ms</td></tr>`).join("")}</table>
    <h3>Transmissions per type</h3><p class="hint">${Object.entries(s.by_type).map(([t, n]) => `<span class="chip" style="background:${PCOLOR[t] || "#666"}">${t} ${n}</span>`).join(" ")}</p>
    <p class="hint">Internet: ${s.internet ? "up" : "<b>down everywhere</b>"}${s.internet_down.length ? " · down in " + s.internet_down.join(", ") : ""}. Busiest node airtime ${s.busiest_airtime_s.toFixed(1)} s. Path requests ${s.path_requests}, quota drops ${s.quota_dropped}.</p>
    <p class="legend">${Object.entries(KIND_COLOR).map(([k, c]) => `<i style="background:${c}"></i>${k}`).join("")}<br>Orange ring = on battery, grey = dead.</p>`;
    renderAlerts();
  }

  // ---------- compare ----------
  $("btn-compare").onclick = () => {
    const f = $("cmp-feature").value, dur = +$("cmp-dur").value;
    const a = JSON.parse(JSON.stringify(scenario)); a.duration_s = dur; a.trace = false;
    const b = JSON.parse(JSON.stringify(a)); b.features[f] = !a.features[f];
    $("compare").innerHTML = `<p class="hint">Running ${dur} s twice…</p>`;
    const sa = new Engine.Sim(a), sb = new Engine.Sim(b);
    let t = 0;
    const chunk = () => {
      t = Math.min(dur * 1000, t + 30000);
      sa.step(30000); sb.step(30000);
      $("compare").innerHTML = `<p class="hint">${(t / 1000).toFixed(0)} / ${dur} s…</p>`;
      if (t < dur * 1000) return setTimeout(chunk, 0);
      const x = sa.summary(), y = sb.summary(); sa.free(); sb.free();
      const rows = [["delivered %", "delivered_pct", 1], ["acked %", "acked_pct", 1], ["latency median s", "latency_median_ms", 0.001], ["latency p90 s", "latency_p90_ms", 0.001], ["transmissions", "tx", 1], ["airtime s", "airtime_s", 1], ["collisions", "collisions", 1], ["duty hits", "duty_hits", 1], ["local repairs", "local_repairs", 1], ["carry handoffs", "carry_handoffs", 1], ["channel reach %", "channel_reach_pct", 1]];
      const al = (s) => s.alerts.map(q => q.coverage_pct.toFixed(0) + " %").join(", ") || "–";
      $("compare").innerHTML = `<table><tr><th></th><th>${f} ${a.features[f] ? "on" : "off"}</th><th>${f} ${b.features[f] ? "on" : "off"}</th></tr>${rows.map(([l, key, m]) => `<tr><td>${l}</td><td class="n">${(+x[key] * m).toFixed(m === 1 && Number.isInteger(x[key]) ? 0 : 1)}</td><td class="n">${(+y[key] * m).toFixed(m === 1 && Number.isInteger(y[key]) ? 0 : 1)}</td></tr>`).join("")}<tr><td>alert coverage</td><td>${al(x)}</td><td>${al(y)}</td></tr></table><p class="hint">Same seed, same scenario, same traffic draws until the routers behave differently.</p>`;
    };
    setTimeout(chunk, 0);
  };

  // ---------- inspector ----------
  function renderInspector() {
    const box = $("inspector");
    if (selected < 0 || !scenario.nodes[selected]) { box.classList.add("hidden"); return; }
    box.classList.remove("hidden");
    const n = scenario.nodes[selected];
    const ins = sim ? sim.inspect(selected) : null;
    const opt = (v, list) => list.map(x => `<option ${x == v ? "selected" : ""}>${x}</option>`).join("");
    let h = `<h3>Node ${selected} ${esc(n.label || "")}</h3>
      <label>Kind <select id="in-kind">${opt(n.kind, Object.keys(KIND_COLOR))}</select></label>
      <label>Label <input id="in-label" value="${esc(n.label || "")}"></label>
      <label>SF <select id="in-sf">${opt(n.sf || "default", ["default", 7, 8, 9, 10, 11, 12])}</select> Antenna m <input id="in-h" type="number" step="0.5" style="width:60px" value="${n.h || ""}" placeholder="auto"></label>
      <label>Battery Wh <input id="in-batt" type="number" style="width:70px" value="${n.battery_wh ?? ""}" placeholder="auto"> Carry copies <input id="in-carry" type="number" style="width:50px" min="0" max="8" value="${n.carry || 0}"></label>
      <label><input type="checkbox" id="in-net" ${n.internet ? "checked" : ""}> Internet</label>
      <label>Region <input id="in-region" value="${esc(n.region || "")}" size="12"></label>
      <button id="in-apply">Apply (restarts)</button> <button id="in-announce">Send announce, show reach</button> <button id="in-msg">Send message to…</button>`;
    if (ins) {
      h += `<p class="hint">Role ${ins.role} · ${ins.state}${ins.on_battery ? " · on battery " + ins.battery_wh.toFixed(1) + "/" + ins.battery_max_wh + " Wh" : ""} · energy ${ins.energy} · internet ${ins.internet ? "yes" : "no"} · slot ${ins.slot_ms} ms<br>
        Airtime ${ins.airtime_s.toFixed(2)} s · duty budget used ${ins.duty_budget_used_pct.toFixed(1)} % · duty hits ${ins.duty_hits} · carried ${ins.carried}/${ins.carry_copies} copies · ${ins.radio_neighbors.length} radio neighbours</p>
        <h3>Router stats</h3><p class="hint">${Object.entries(ins.stats).map(([k, v]) => `${k} <b>${v}</b>`).join(" · ")}</p>
        <h3>Neighbours (router table)</h3><div class="scroll"><table><tr><th>node</th><th>SNR</th><th>last</th><th>role</th></tr>${ins.neighbors.slice(0, 60).map(x => `<tr class="click" data-n="${x.node}"><td>${x.node}</td><td class="n">${x.snr}</td><td class="n">${x.last_s.toFixed(0)} s</td><td>${x.role || ""}</td></tr>`).join("")}</table></div>
        <h3>Routes</h3><div class="scroll"><table><tr><th>dest</th><th>path (node ids)</th><th>learned</th></tr>${ins.routes.map(r => `<tr><td>${r.dest}</td><td>${r.path.join(" → ") || "direct"}</td><td class="n">${r.learned_s.toFixed(0)} s</td></tr>`).join("") || "<tr><td colspan=3>none</td></tr>"}</table></div>
        <h3>Send queue</h3>${ins.queue.length ? `<table><tr><th>prio</th><th>waited</th><th>bytes</th><th>tries</th></tr>${ins.queue.map(q => `<tr><td>${q.prio}</td><td class="n">${q.waited_ms} ms</td><td class="n">${q.len}</td><td class="n">${q.tries}</td></tr>`).join("")}</table>` : `<p class="hint">empty</p>`}
        ${ins.links.length ? `<h3>Backbone</h3><p class="hint">${ins.links.map(l => `${l.kind} → ${l.peer} ${l.ok ? "up" : "down"}`).join(" · ")}</p>` : ""}`;
    }
    box.innerHTML = h;
    box.querySelectorAll("tr[data-n]").forEach(r => r.onclick = () => onNodeClick(+r.dataset.n));
    $("in-apply").onclick = () => {
      n.kind = $("in-kind").value; n.label = $("in-label").value;
      const sf = $("in-sf").value; n.sf = sf === "default" ? 0 : +sf;
      n.h = +$("in-h").value || 0;
      n.battery_wh = $("in-batt").value === "" ? -1 : +$("in-batt").value;
      n.carry = +$("in-carry").value || 0; n.internet = $("in-net").checked; n.region = $("in-region").value;
      edited(); renderInspector();
    };
    $("in-announce").onclick = () => {
      const r = sim.announce(selected);
      $("tabs").querySelector('[data-tab="trace"]').click();
      $("reach").innerHTML = `<p class="hint">Announce sent; reach updates as the simulation runs. <button id="re-refresh">Refresh reach</button></p>`;
      const go = () => showReach(r.pid, `Announce from node ${selected}`);
      $("re-refresh").onclick = go;
      if (!playing) { sim.step(20000); }
      setTimeout(go, 50);
    };
    $("in-msg").onclick = () => {
      const to = prompt("Destination node index:"); if (to === null) return;
      const r = sim.send(selected, +to);
      $("tabs").querySelector('[data-tab="trace"]').click();
      if (!playing) sim.step(30000);
      setTimeout(() => follow(r.msg), 50);
    };
  }

  // ---------- trace views ----------
  function showReach(pid, title) {
    const r = sim.reach(pid);
    overlay = new Map();
    const maxH = Math.max(1, ...r.nodes.map(x => x[1]));
    const reasons = {};
    for (const [n, hops, , why] of r.nodes) { overlay.set(n, heat(hops / maxH)); reasons[why] = (reasons[why] || 0) + 1; }
    restyle();
    $("reach").innerHTML = `<h3>${esc(title)}</h3><p class="hint">Heard by <b>${r.heard}</b> nodes, forwarded by ${r.forwarded}, ${r.collisions} receptions lost to collisions${r.region ? `, region id ${r.region}` : ""}. Colour = hop count (green near → red far).</p>
      <table><tr><th>why it stopped</th><th>nodes</th></tr>${Object.entries(reasons).sort((a, b) => b[1] - a[1]).map(([w, c]) => `<tr><td>${esc(w)}</td><td class="n">${c}</td></tr>`).join("")}</table>
      <div class="scroll"><table><tr><th>node</th><th>hops</th><th>first heard</th><th>status</th></tr>${r.nodes.sort((a, b) => a[1] - b[1]).slice(0, 400).map(x => `<tr class="click" data-n="${x[0]}"><td>${x[0]}</td><td class="n">${x[1]}</td><td class="n">${(x[2] / 1000).toFixed(2)} s</td><td>${esc(x[3])}</td></tr>`).join("")}</table></div>`;
    $("reach").querySelectorAll("tr[data-n]").forEach(t => t.onclick = () => onNodeClick(+t.dataset.n));
  }

  function renderMsgList() {
    if (!sim) return;
    const r = sim.msgs(Math.max(0, sim.summary().messages - 300), 300);
    $("msg-list").innerHTML = `<table><tr><th>#</th><th>from → to</th><th>sent</th><th>delivered</th><th>ack</th></tr>${r.msgs.slice().reverse().map(m => `<tr class="click" data-m="${m[0]}"><td>${m[0]}</td><td>${m[1]} → ${m[2]}</td><td class="n">${(m[3] / 1000).toFixed(0)} s</td><td class="n">${m[4] !== null ? ((m[4] - m[3]) / 1000).toFixed(1) + " s" : "–"}</td><td>${m[5] !== null ? "✓" : ""}</td></tr>`).join("")}</table>`;
    $("msg-list").querySelectorAll("tr[data-m]").forEach(t => t.onclick = () => follow(+t.dataset.m));
  }
  setInterval(() => { if ($("tab-trace").classList.contains("on")) renderMsgList(); }, 2000);

  function follow(m) {
    const f = sim.follow(m);
    const info = f.info;
    hiLayer.clearLayers();
    overlay = new Map([[info.from, "#2563eb"], [info.to, "#16a34a"]]);
    const pos = (i) => [scenario.nodes[i].lat, scenario.nodes[i].lon];
    const txs = new Map();
    let preqPid = null;
    const rows = [];
    for (const e of f.events) {
      if (e.ev === "tx") { txs.set(e.id, e); if (e.p === "PathRequest" && !preqPid) preqPid = e.pid; }
      if (e.ev === "rx") {
        const tx = txs.get(e.id); const p = tx ? tx.p : "?";
        const style = { Data: { w: 4, d: null }, Carry: { w: 4, d: null }, PathResponse: { w: 3, d: null }, Ack: { w: 2, d: "2 4" }, PathRequest: { w: 1, d: null } }[p] || { w: 1, d: null };
        for (const r of e.ok) {
          if (p === "PathRequest" && e.ok.length > 15) break;
          L.polyline([pos(e.n), pos(r)], { color: PCOLOR[p] || "#666", weight: style.w, opacity: p === "PathRequest" ? 0.35 : 0.85, dashArray: style.d }).addTo(hiLayer);
          if (p !== "PathRequest") overlay.set(r, overlay.get(r) || PCOLOR[p]);
        }
      }
      if (e.ev === "bb") L.polyline([pos(e.from), pos(e.to)], { color: "#0f766e", weight: 4, dashArray: "8 6" }).addTo(hiLayer);
      rows.push(e);
    }
    restyle();
    const detail = (e) => {
      switch (e.ev) {
        case "send": return `${e.from} → ${e.to}`;
        case "tx": return `${e.p} hop ${e.hops}, prio ${e.prio}, queued ${e.qd} ms, airtime ${(e.air / 1000).toFixed(0)} ms`;
        case "rx": return `decoded by [${e.ok.slice(0, 12).join(",")}${e.ok.length > 12 ? "…" : ""}]${e.coll.length ? `, collided at [${e.coll.slice(0, 8).join(",")}]` : ""}`;
        case "cancel": return "forward cancelled: neighbours covered the area";
        case "drop": return `dropped: ${e.why}`;
        case "route": return `route learned to ${e.dest}, ${e.hops} hops`;
        case "bb": return `backbone ${e.from} → ${e.to}`;
        case "deliver": return `delivered after ${e.hops} hops`;
        case "ack": return "end-to-end ack back at the sender";
        case "repair": return "local repair (skipped a dead next hop)";
        case "carry": return "carry handoff";
        case "retry": return "hop retransmitted (next hop not heard forwarding)";
        default: return "";
      }
    };
    $("follow").innerHTML = `<h3>Message ${m}: ${info.from} → ${info.to}</h3>
      <p class="hint">Sent ${(info.sent_ms / 1000).toFixed(1)} s · ${info.delivered_ms !== null ? `delivered after ${((info.delivered_ms - info.sent_ms) / 1000).toFixed(2)} s` : "<b>not delivered</b>"} · ${info.acked_ms !== null ? `acked after ${((info.acked_ms - info.sent_ms) / 1000).toFixed(2)} s` : "no ack"}.
      Map: thin orange = path request flood, green = path response, thick blue = data hops, dashed green = acks, dashed teal = backbone.
      ${preqPid ? `<button id="fl-reach">Path request reach</button>` : ""}</p>
      <div class="scroll"><table><tr><th>t (s)</th><th>event</th><th>node</th><th>detail</th></tr>${rows.slice(0, 600).map(e => `<tr class="click" data-n="${e.n ?? e.from ?? ""}"><td class="n">${(e.t / 1000).toFixed(2)}</td><td>${e.ev}</td><td>${e.n ?? ""}</td><td>${esc(detail(e))}</td></tr>`).join("")}</table></div>`;
    if (preqPid) $("fl-reach").onclick = () => showReach(preqPid, `Path request of message ${m}`);
    $("follow").querySelectorAll("tr[data-n]").forEach(t => t.onclick = () => { if (t.dataset.n !== "") { selected = +t.dataset.n; renderInspector(); restyle(); } });
  }

  // ---------- log ----------
  ["tx", "rx", "send", "deliver", "ack", "cancel", "drop", "defer", "route", "bb", "repair", "carry", "retry", "alert", "alert_rx", "announce", "event", "dead"].forEach(e => $("lg-ev").insertAdjacentHTML("beforeend", `<option>${e}</option>`));
  PTYPES.forEach(p => $("lg-type").insertAdjacentHTML("beforeend", `<option>${p}</option>`));
  function logFilter() {
    const f = {};
    if ($("lg-node").value !== "") f.node = +$("lg-node").value;
    if ($("lg-type").value) f.ptype = $("lg-type").value;
    if ($("lg-msg").value !== "") f.msg = +$("lg-msg").value;
    if ($("lg-ev").value) f.ev = $("lg-ev").value;
    return f;
  }
  function fetchLog(max) {
    const f = logFilter();
    const out = []; let since = 0;
    for (let guard = 0; guard < 400; guard++) {
      const r = sim.events(since, 20000, f);
      out.push(...r.lines);
      if (r.next === since || r.lines.length === 0 && r.next >= sim.summary().trace_end) break;
      since = r.next;
      if (out.length > max) break;
    }
    return out;
  }
  $("btn-log").onclick = () => { const l = fetchLog(100000); $("log").textContent = l.slice(-400).map(e => JSON.stringify(e)).join("\n") || "(no events)"; };
  function download(name, text, type) { const a = document.createElement("a"); a.href = URL.createObjectURL(new Blob([text], { type })); a.download = name; a.click(); }
  $("btn-log-json").onclick = () => download("maarse-denmark-events.json", JSON.stringify(fetchLog(500000)), "application/json");
  $("btn-log-csv").onclick = () => {
    const l = fetchLog(500000);
    const cols = ["t", "ev", "n", "p", "pid", "msg", "alert", "hops", "prio", "why", "from", "to", "ok", "coll"];
    download("maarse-denmark-events.csv", cols.join(",") + "\n" + l.map(e => cols.map(c => e[c] === undefined ? "" : Array.isArray(e[c]) ? `"${e[c].join(" ")}"` : JSON.stringify(e[c])).join(",")).join("\n"), "text/csv");
  };

  // ---------- build tab ----------
  for (const id of ["g-users", "g-rep", "g-net"]) { const i = $(id); const show = () => i.nextElementSibling.textContent = i.value; i.oninput = show; show(); }
  $("btn-generate").onclick = () => {
    scenario = DK.generateDenmark({ users: +$("g-users").value, repeaterKm: +$("g-rep").value, internetPct: +$("g-net").value, seed: +$("g-seed").value }, window.DK_LAND, window.DK_TOWNS);
    syncForm(); edited();
  };
  $("btn-clear").onclick = () => { scenario = DK.base("Empty map", 1); syncForm(); edited(); };
  $("features").innerHTML = FEATURES.map(([k, l]) => `<label><input type="checkbox" data-f="${k}"> ${l}</label>`).join("");
  $("features").addEventListener("change", (e) => { const k = e.target.dataset.f; if (!k) return; scenario.features[k] = e.target.checked; edited(); });
  $("sc-sf").onchange = () => { scenario.sf = +$("sc-sf").value; edited(); };
  $("sc-duty").onchange = () => { scenario.duty_permille = +$("sc-duty").value; edited(); };
  const trMap = { "tr-direct": "direct_per_min", "tr-channel": "channel_per_min", "tr-local": "local_share" };
  for (const [id, key] of Object.entries(trMap)) $(id).onchange = () => { scenario.traffic[key] = +$(id).value; edited(); };
  ["opt-islands", "opt-links", "opt-backbone"].forEach(id => $(id).onchange = () => { drawLinks(); restyle(); });
  function syncForm() {
    document.querySelectorAll("#features input").forEach(i => i.checked = !!scenario.features[i.dataset.f]);
    $("sc-sf").value = scenario.sf; $("sc-duty").value = scenario.duty_permille;
    for (const [id, key] of Object.entries(trMap)) $(id).value = scenario.traffic[key];
  }
  $("btn-export").onclick = () => download((scenario.name || "scenario").replace(/\W+/g, "-").toLowerCase() + ".json", JSON.stringify(scenario, null, 1), "application/json");
  $("file-import").onchange = async (e) => {
    const f = e.target.files[0]; if (!f) return;
    try { scenario = Object.assign(DK.base("Imported", 1), JSON.parse(await f.text())); syncForm(); edited(); }
    catch (err) { alert("Could not read the scenario: " + err.message); }
  };

  // ---------- tabs, controls ----------
  $("tabs").addEventListener("click", (e) => {
    const b = e.target.closest("button"); if (!b) return;
    document.querySelectorAll("#tabs button").forEach(x => x.classList.toggle("on", x === b));
    document.querySelectorAll(".tab").forEach(t => t.classList.toggle("on", t.id === "tab-" + b.dataset.tab));
    if (b.dataset.tab === "trace") renderMsgList();
  });
  $("anim-types").innerHTML = PTYPES.map(p => `<label style="display:inline-flex"><input type="checkbox" data-p="${p}" checked><span class="chip" style="background:${PCOLOR[p]}">${p}</span></label>`).join("");
  $("anim-types").addEventListener("change", (e) => { animOn[e.target.dataset.p] = e.target.checked; });
  $("btn-play").onclick = () => { playing = !playing; $("btn-play").textContent = playing ? "❚❚ Pause" : "▶ Play"; };
  $("btn-step").onclick = () => { if (!sim) return; sim.step(1000); pullEvents(); state = sim.state(); restyle(); updateClock(); renderStats(); };
  $("btn-reset").onclick = () => rebuild();
  const pre = $("preset");
  pre.innerHTML = `<option value="">— preset —</option>` + Object.entries(DK.PRESETS).map(([k, p]) => `<option value="${k}">${p.title}</option>`).join("");
  pre.onchange = () => loadPreset(pre.value);
  function loadPreset(k) {
    const p = DK.PRESETS[k]; if (!p) return;
    scenario = p.build(window.DK_LAND, window.DK_TOWNS, +$("g-seed").value || 1);
    syncForm(); edited();
    $("sc-info").textContent = p.text;
    location.hash = k;
    const b = L.latLngBounds(scenario.nodes.map(n => [n.lat, n.lon])); if (b.isValid()) map.fitBounds(b.pad(0.1));
  }

  // ---------- start ----------
  MaarseSim.load("maarse_sim_wasm.wasm").then((E) => {
    Engine = E;
    const k = location.hash.slice(1);
    if (DK.PRESETS[k]) { pre.value = k; loadPreset(k); }
    else { pre.value = "normal"; loadPreset("normal"); }
    requestAnimationFrame(frame);
  }).catch((e) => {
    document.body.insertAdjacentHTML("afterbegin", `<p style="padding:8px;background:#fee2e2;color:#7f1d1d">Could not load the WebAssembly engine (${esc(e.message)}). Build it with <code>python tools/build_denmark_sim.py</code> and serve <code>docs/</code> over HTTP.</p>`);
  });
})();
