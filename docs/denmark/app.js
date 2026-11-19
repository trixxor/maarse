// Må:rse Denmark Simulator: map editor, live view and controls around the
// WebAssembly engine (maarse-sim-wasm, the real maarse-core Router per node).
//
// Live view (inspired by mesh "live" maps): a dark map, a packet feed, a
// detail card per transmission, a timeline with an activity histogram, and a
// "journey" that tells one message's story and replays it on the map.
(function () {
  "use strict";
  const $ = (id) => document.getElementById(id);
  const esc = (s) => String(s).replace(/[&<>"]/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));
  const DK = window.DkScenarios;

  const KIND_COLOR = { handheld: "#60a5fa", relay: "#a78bfa", repeater: "#fb923c", anchor: "#f87171", host: "#2dd4bf", wifi: "#facc15", mobile: "#f472b6", server: "#94a3b8", app: "#e8930c" };
  const KIND_R = { handheld: 3, relay: 3.5, repeater: 5, anchor: 7, host: 6, wifi: 5, mobile: 5, server: 6, app: 3 };
  const PTYPES = ["PathRequest", "PathResponse", "Data", "Ack", "Datagram", "Channel", "Bulletin", "Announce", "Carry", "Fragment", "Backbone"];
  const PCOLOR = { Announce: "#a3a3a3", PathRequest: "#f59e0b", PathResponse: "#10b981", Data: "#3b82f6", Ack: "#22c55e", Channel: "#8b5cf6", Bulletin: "#ef4444", Carry: "#ec4899", Fragment: "#0ea5e9", Datagram: "#eab308", Lookup: "#eab308", Backbone: "#14b8a6", Drop: "#dc2626", Event: "#64748b", DHT: "#14b8a6" };
  const PEXPLAIN = {
    PathRequest: "flood to find a route (carries the first message)", PathResponse: "the route back to the sender (carries the ack)", Data: "a message along a known route",
    Ack: "end-to-end acknowledgement", Channel: "channel message (flood)", Bulletin: "signed alert (flood, high priority)", Announce: "a node says it exists (flood)",
    Carry: "store-carry-forward handoff", Datagram: "home routing lookup (question or answer)", Fragment: "piece of a file", Backbone: "copy over TCP / Wi-Fi between hosts",
  };
  const FLOODS = new Set(["PathRequest", "Announce", "Channel", "Bulletin"]);
  const FEATURES = [["carry", "Carry (store-carry-forward)"], ["local_repair", "Local repair"], ["backbone", "Backbone links"], ["dht_backbone", "DHT backbone: path requests only to the host that can reach the destination (experimental)"], ["home_routing", "Home routing: fixed infrastructure, home repeaters and a location lookup (model)"], ["persist_known", "Contacts survive a reboot (stored in flash)"], ["hold_unknown", "Keep messages from unknown senders until their announce"], ["auto_relay", "Automatic forest mode: handhelds without a repeater in range become relays"], ["regions", "Regions"], ["sf_slot", "SF-sized flood slot"], ["scale_timeouts", "Scaled timeouts"], ["load_control", "Load control"], ["area_scope", "Area scope for alerts (sim layer)"]];

  let Engine = null, sim = null, scenario = null;
  let playing = false, speed = 10;
  let tool = "select", dtool = null, pick = null;
  let selected = -1, lastSeq = 0;
  let pendingArea = null, routeDraft = null, linkDraft = -1;
  let state = null, overlay = null;
  let jsPending = [];            // future events added while running (injected when due)
  let followMsg = -1, journeyKey = "";
  let townOf = [];
  const animOn = {};
  PTYPES.forEach(p => animOn[p] = true);

  // ---------- map ----------
  const map = L.map("map", { preferCanvas: true, zoomControl: true }).setView([56.1, 11.2], 7);
  const canvas = L.canvas({ padding: 0.2 });
  // One OSM layer; the dark look is a CSS filter on the tile pane (style.css).
  const tiles = L.tileLayer("https://tile.openstreetmap.org/{z}/{x}/{y}.png", { maxZoom: 17, attribution: "© OpenStreetMap contributors (tiles need internet); coastline: Natural Earth" }).addTo(map);
  // Where tiles cannot load (offline, or a page that may not fetch images),
  // the embedded coastline carries the map.
  let tilesFailed = false;
  tiles.on("tileerror", () => {
    if (tilesFailed) return;
    tilesFailed = true;
    map.getContainer().classList.add("no-tiles");
    land.setStyle({ fillOpacity: 0.35, weight: 1.2 });
  });
  const land = L.geoJSON(window.DK_LAND, { style: { color: "#64748b", weight: 1, fillColor: "#a3b18a", fillOpacity: 0.12 }, interactive: false }).addTo(map);
  const clutterLayer = L.layerGroup().addTo(map);
  const disasterLayer = L.layerGroup().addTo(map);
  const linkLayer = L.layerGroup().addTo(map);
  const nodeLayer = L.layerGroup().addTo(map);
  const hiLayer = L.layerGroup().addTo(map);
  const animLayer = L.layerGroup().addTo(map);
  const drawLayer = L.layerGroup().addTo(map);
  let markers = [];

  function isDark() {
    const t = document.documentElement.dataset.theme;
    return t ? t === "dark" : matchMedia("(prefers-color-scheme: dark)").matches;
  }
  function applyTheme() {
    const dark = isDark();
    map.getContainer().classList.toggle("dark-tiles", dark);
    $("theme-name").textContent = dark ? "Dark" : "Light";
    if (!tilesFailed) land.setStyle({ fillOpacity: dark ? 0.06 : 0.18 });
  }
  $("btn-theme").onclick = () => {
    const t = isDark() ? "light" : "dark";
    document.documentElement.dataset.theme = t;
    try { localStorage.setItem("dk-theme", t); } catch (e) { /* private window */ }
    applyTheme();
  };
  try { const t = localStorage.getItem("dk-theme"); if (t) document.documentElement.dataset.theme = t; } catch (e) { /* private window */ }
  applyTheme();

  function compColor(c) {
    if (c < 0) return "#6b7280";
    return `hsl(${(c * 137.508) % 360},75%,58%)`;
  }
  const pos = (i) => state && state.lat ? [state.lat[i], state.lon[i]] : [scenario.nodes[i].lat, scenario.nodes[i].lon];

  // Readable node names: "586 handheld · Skagen".
  function computeTowns() {
    townOf = scenario.nodes.map(n => DK.nearestTown(window.DK_TOWNS, n.lat, n.lon)[0]);
  }
  function name(i) {
    const n = scenario.nodes[i];
    if (!n) return `#${i}`;
    return n.label ? `${i} ${n.label}` : `${i} ${n.kind} · ${townOf[i] || ""}`;
  }
  const nameH = (i) => `<a href="#" class="nd" data-n="${i}">${esc(name(i))}</a>`;
  document.addEventListener("click", (e) => {
    const a = e.target.closest("a.nd");
    if (!a) return;
    e.preventDefault();
    selectNode(+a.dataset.n, true);
  });

  function drawNodes() {
    nodeLayer.clearLayers();
    markers = scenario.nodes.map((n, i) => {
      const m = L.circleMarker([n.lat, n.lon], { renderer: canvas, radius: KIND_R[n.kind] || 3, weight: 1, color: "#0b0f14", fillColor: KIND_COLOR[n.kind], fillOpacity: 0.9 });
      m.on("click", (e) => { L.DomEvent.stopPropagation(e); onNodeClick(i); });
      m.on("mousedown", (e) => startDrag(i, e));
      m.on("contextmenu", (e) => { L.DomEvent.stop(e); nodeMenu(i, e); });
      m.bindTooltip(() => `${esc(name(i))}<br><span class="tip-hint">click: inspect · right-click: actions</span>`, { sticky: true });
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
        const lora = l.kind === "lora";
        const pl = L.polyline([[a.lat, a.lon], [b.lat, b.lon]], {
          color: lora ? "#c084fc" : l.kind === "wifi" ? "#facc15" : "#14b8a6",
          weight: lora ? 3 : 2, opacity: ok ? (lora ? 0.85 : 0.55) : 0.25,
          dashArray: ok ? (lora ? "10 6" : null) : "4 6",
        });
        const tr = lora && state && state.trunks ? state.trunks.find(t => t.label === l.label) : null;
        const kindText = lora ? `${esc(l.label || "LoRa trunk")} (SF${l.sf || 9}, ${l.snr_db || 5} dB)` : `${l.kind.toUpperCase()} link ${k}`;
        pl.bindTooltip(`${kindText}: ${esc(name(l.a))} ↔ ${esc(name(l.b))} ${ok ? "(up)" : "(down)"}${tr ? ` · ${tr.packets} packets, ${tr.airtime_s.toFixed(0)} s airtime` : ""}.<br><span class="tip-hint">click: ${ok ? "cut" : "restore"} · right-click: more</span>`);
        pl.on("click", (e) => { L.DomEvent.stopPropagation(e); toggleLink(k); });
        pl.on("contextmenu", (e) => { L.DomEvent.stop(e); linkMenu(k, e); });
        pl.addTo(linkLayer);
      });
    }
    if ($("opt-links").checked && selected >= 0 && sim) {
      const ins = sim.inspect(selected);
      const me = pos(selected);
      for (const [j, snr] of ins.radio_neighbors) {
        L.polyline([me, pos(j)], { color: snr > 0 ? "#22c55e" : snr > -8 ? "#eab308" : "#ef4444", weight: 1, opacity: 0.6, interactive: false }).addTo(linkLayer);
      }
    }
  }

  function restyle() {
    const byIsland = $("opt-islands").checked;
    markers.forEach((m, i) => {
      const n = scenario.nodes[i];
      let fill = KIND_COLOR[n.kind], stroke = "#0b0f14", w = 1, op = 0.9;
      if (state && state.state) {
        const s = state.state[i];
        if (byIsland && state.comp) fill = compColor(state.comp[i]);
        if (s === "b") { stroke = "#f97316"; w = 2.5; }
        if (s !== "a" && s !== "b") { fill = "#4b5563"; op = 0.55; stroke = s === "k" ? "#ef4444" : "#0b0f14"; w = s === "k" ? 1.5 : 1; }
        if (state.lat && (n.kind === "mobile" || (n.route && n.route.length > 1) || n.wander_m > 0)) m.setLatLng([state.lat[i], state.lon[i]]);
      }
      if (overlay && overlay.has(i)) { fill = overlay.get(i); op = 1; }
      else if (overlay) { op = 0.2; }
      if (i === selected) { stroke = "#fff"; w = 3; }
      m.setStyle({ fillColor: fill, color: stroke, weight: w, fillOpacity: op });
    });
  }

  function drawClutter() {
    clutterLayer.clearLayers();
    (scenario.clutter || []).forEach((c, k) => {
      L.polygon(c.polygon, { color: "#16a34a", weight: 1, fillColor: "#16a34a", fillOpacity: 0.18, interactive: false })
        .bindTooltip(`${esc(c.label || "Forest " + (k + 1))}: ${c.db_per_km} dB/km`).addTo(clutterLayer);
    });
    renderClutterList();
  }

  // Disaster areas on the map: dashed until they happen, filled while active.
  function drawDisasters() {
    disasterLayer.clearLayers();
    const t = now() / 1000;
    const evs = scenario.events.slice().sort((a, b) => a.t_s - b.t_s);
    const endsAfter = (ev, kinds) => evs.some(o => kinds.includes(o.type) && o.t_s >= ev.t_s && o.t_s <= t && o !== ev);
    for (const ev of evs) {
      if (!ev.area || (!ev.area.circle && !(ev.area.polygon && ev.area.polygon.length >= 3))) continue;
      let col, label, over = false;
      if (ev.type === "power_cut") { col = "#f97316"; label = "Power cut"; over = endsAfter(ev, ["power_restore"]); }
      else if (ev.type === "degrade") { col = "#a855f7"; label = `Radio loss ${ev.db} dB`; over = endsAfter(ev, ["degrade_clear"]); }
      else if (ev.type === "kill_area") { col = "#ef4444"; label = "Destroyed"; over = endsAfter(ev, ["revive"]); }
      else continue;
      const active = ev.t_s <= t && !over;
      const style = { color: col, weight: 1.5, dashArray: active ? null : "5 5", fillColor: col, fillOpacity: active ? 0.15 : 0.03, interactive: false };
      const shape = ev.area.circle ? L.circle([ev.area.circle[0], ev.area.circle[1]], Object.assign({ radius: ev.area.circle[2] }, style)) : L.polygon(ev.area.polygon, style);
      shape.bindTooltip(`${label} at ${fmtTime(ev.t_s)}${active ? " (active)" : over ? " (over)" : " (planned)"}`).addTo(disasterLayer);
    }
  }

  // ---------- engine lifecycle ----------
  function rebuild() {
    if (sim) sim.free();
    sim = null;
    try { sim = new Engine.Sim(scenario); }
    catch (e) { showNote("Scenario error: " + e.message); return; }
    lastSeq = 0; jsPending = []; overlay = null; followMsg = -1; journeyKey = "";
    feed.items = []; feed.byId.clear(); feed.sel = null; hist.clear(); txTimes.length = 0;
    hiLayer.clearLayers(); animLayer.clearLayers(); $("card").classList.add("hidden");
    state = sim.state();
    computeTowns(); drawNodes(); drawClutter(); drawDisasters();
    updateClock(); renderStats(); renderTimeline(); fillSenders(); renderAlerts(); renderHud(); renderFeed(true); drawHist();
    $("sc-info").textContent = `${scenario.name || "scenario"}: ${scenario.nodes.length} nodes, ${scenario.links.length} backbone links, ${scenario.events.length} timeline events.${scenario.nodes.length > perf.maxNodes ? ` Over this browser's limit of ${perf.maxNodes} nodes (Build > Performance): it may run slowly.` : ""}`;
  }
  function edited() { rebuild(); }
  // Phones (phone.js) read the engine and scenario through this.
  window.DKSim = { get sim() { return sim; }, get scenario() { return scenario; }, name: (i) => name(i), map,
    // A phone added in the Phones panel becomes a node: appended to the scenario, engine rebuilt.
    addNode(spec, links) { scenario.nodes.push(spec); for (const l of links || []) scenario.links.push(l); edited(); return scenario.nodes.length - 1; } };
  function showNote(text) { $("sc-info").textContent = text; }

  function fmtTime(s) { const m = Math.floor(s / 60); const r = s - m * 60; return `${m}:${r < 10 ? "0" : ""}${r.toFixed(r < 60 && m === 0 ? 1 : 0)}`; }
  function fmtT(ms) { const s = ms / 1000, m = Math.floor(s / 60); return `${m}:${(s - m * 60).toFixed(1).padStart(4, "0")}`; }
  function now() { return sim ? lastT : 0; }
  let lastT = 0;
  function updateClock() { lastT = sim ? sim.summary().t : 0; $("clock").textContent = fmtTime(lastT / 1000); }

  // ---------- main loop ----------
  let lastFrame = 0, lastState = 0, lastStats = 0, lastFeed = 0;
  // CPU limits for this browser (Build > Performance), measured by the benchmark.
  const perf = Object.assign({ maxEvents: 400000, maxSpeed: 1000, maxNodes: 2000 }, (() => { try { return JSON.parse(localStorage.getItem("dk-perf") || "{}"); } catch (e) { return {}; } })());
  function savePerf() { try { localStorage.setItem("dk-perf", JSON.stringify(perf)); } catch (e) { /* private window */ } }
  function syncPerf() { $("pf-events").value = perf.maxEvents; $("pf-speed").value = perf.maxSpeed; $("pf-nodes").value = perf.maxNodes; }
  function benchmark() {
    // Runs a copy of the current scenario as fast as possible (up to 15
    // simulated minutes or 2 s of real time) and works out what this browser
    // can keep up with.
    const b = new Engine.Sim(JSON.parse(JSON.stringify(scenario)));
    const t0 = performance.now();
    let events = 0, simS = 0;
    while (simS < 900 && performance.now() - t0 < 2000) { events += b.step(10000, 1e9).events; simS += 10; }
    const ms = performance.now() - t0;
    const evPerSec = events / (ms / 1000);
    b.free();
    // Real time per simulated second; keep a frame under ~25 ms.
    const msPerSimS = ms / simS;
    const speedOk = Math.max(1, Math.floor(1000 / msPerSimS * 0.6));
    perf.maxEvents = Math.max(100000, Math.round(evPerSec * 0.025));
    perf.maxSpeed = Math.min(1000, speedOk);
    savePerf(); syncPerf();
    $("pf-out").textContent = `${scenario.nodes.length} nodes: ${simS} s simulated in ${(ms / 1000).toFixed(2)} s (${Math.round(evPerSec).toLocaleString()} events/s). This browser keeps up to about ${speedOk}× real time; limits set to ${perf.maxEvents.toLocaleString()} events per frame and ${perf.maxSpeed}× speed.`;
  }
  function frame(ts) {
    requestAnimationFrame(frame);
    if (!sim) return;
    const dt = lastFrame ? Math.min(100, ts - lastFrame) : 16;
    lastFrame = ts;
    if (playing) {
      const t0 = performance.now();
      const r = sim.step(Math.max(1, Math.round(dt * Math.min(speed, perf.maxSpeed))), perf.maxEvents);
      lastT = r.t;
      $("behind").classList.toggle("hidden", !(r.behind || performance.now() - t0 > 40));
      injectDue();
      pullEvents();
    }
    if (ts - lastState > 400) { lastState = ts; state = sim.state(); restyle(); $("clock").textContent = fmtTime(lastT / 1000); }
    if (ts - lastFeed > 300) { lastFeed = ts; renderFeed(); drawHist(); }
    if (ts - lastStats > 1000) {
      lastStats = ts;
      renderHud(); drawDisasters();
      if ($("tab-stats").classList.contains("on")) renderStats();
      if (selected >= 0) renderInspector();
      if (playing && $("opt-links").checked) drawLinks();
      if (followMsg >= 0) renderJourney();
    }
    fadeAnim();
  }
  function injectDue() {
    const t = lastT / 1000;
    const due = jsPending.filter(e => e.t_s <= t + 1e-6);
    if (!due.length) return;
    jsPending = jsPending.filter(e => e.t_s > t + 1e-6);
    for (const ev of due) sim.inject(ev);
    state = sim.state(); restyle(); drawLinks();
  }

  // ---------- events: animation, feed, histogram ----------
  const anims = [];
  const feed = { items: [], byId: new Map(), paused: false, sel: null, dirty: false, seq: 0 };
  const hist = new Map();        // 10 s bucket -> { type: count }
  const txTimes = [];            // tx times of the last minute (HUD)
  const txType = new Map();
  function feedPush(it) {
    it.k = ++feed.seq;
    feed.items.push(it);
    if (it.id !== undefined) feed.byId.set(it.id, it);
    if (feed.items.length > 600) {
      const old = feed.items.splice(0, feed.items.length - 600);
      for (const o of old) if (o.id !== undefined) feed.byId.delete(o.id);
    }
    feed.dirty = true;
  }
  function pullEvents() {
    const r = sim.events(lastSeq, 20000);
    lastSeq = r.next;
    const anim = playing && speed <= 200;
    let drawn = 0;
    const bounds = map.getBounds().pad(0.2);
    for (const e of r.lines) {
      switch (e.ev) {
        case "tx": {
          txType.set(e.id, e.p);
          if (txType.size > 30000) txType.clear();
          feedPush({ kind: "tx", t: e.t, id: e.id, p: e.p, n: e.n, hops: e.hops, nx: e.nx, dst: e.dst, path: e.path, trail: e.trail, prio: e.prio, qd: e.qd, air: e.air, end: e.end, pid: e.pid, msg: e.msg, alert: e.alert });
          txTimes.push(e.t);
          const b = Math.floor(e.t / 10000), h = hist.get(b) || {};
          h[e.p] = (h[e.p] || 0) + 1; hist.set(b, h);
          break;
        }
        case "rx": {
          const it = feed.byId.get(e.id);
          if (it) { it.ok = e.ok; it.snr = e.snr; it.coll = e.coll; it.cw = e.cw; feed.dirty = true; }
          const p = txType.get(e.id) || "Data";
          if (!anim || !animOn[p] || drawn > 350) break;
          const sp = pos(e.n);
          if (!bounds.contains(sp)) break;
          drawn++;
          const col = PCOLOR[p] || "#888";
          const routed = !FLOODS.has(p);
          if (!routed && e.ok.length > 25) {
            anims.push({ l: L.circleMarker(sp, { renderer: canvas, radius: 16, color: col, weight: 2, fill: false, interactive: false }).addTo(animLayer), t: performance.now() });
          } else {
            for (const r2 of e.ok) {
              const isNext = r2 === e.nx;
              if (routed && e.nx >= 0 && !isNext) continue; // bystanders of routed packets: skip
              anims.push({ l: L.polyline([sp, pos(r2)], { renderer: canvas, color: col, weight: routed ? 3 : 1.5, opacity: routed ? 0.95 : 0.6, interactive: false }).addTo(animLayer), t: performance.now() });
            }
          }
          for (let k = 0; k < Math.min(e.coll.length, 10); k++) {
            const r2 = e.coll[k];
            if (routed && r2 !== e.nx) continue;
            anims.push({ l: L.circleMarker(pos(r2), { renderer: canvas, radius: routed ? 9 : 5, color: "#ef4444", weight: 2, fill: false, interactive: false }).addTo(animLayer), t: performance.now() });
          }
          break;
        }
        case "bb":
          feedPush({ kind: "bb", t: e.t, p: "Backbone", n: e.from, to: e.to, msg: e.msg, alert: e.alert, pid: e.pid });
          if (anim && animOn.Backbone && drawn < 350) { drawn++; anims.push({ l: L.polyline([pos(e.from), pos(e.to)], { renderer: canvas, color: PCOLOR.Backbone, weight: 2, dashArray: "6 6", interactive: false }).addTo(animLayer), t: performance.now() }); }
          break;
        case "dht": feedPush({ kind: "dht", t: e.t, p: "DHT", n: e.n, to: e.to, dst: e.dst, msg: e.msg }); break;
        case "lookup": feedPush({ kind: "ev", t: e.t, p: "Lookup", n: e.n, text: `asks ${e.local ? "its home" : "the gateway"} ${e.to} where #${e.msg}'s destination is`, msg: e.msg }); break;
        case "lookup_answer": feedPush({ kind: "ev", t: e.t, p: "Lookup", n: e.n, text: e.ok ? `answers: route ${e.path.join(" → ")}` : `answers: ${e.why || "no route"}`, msg: e.msg }); break;
        case "lookup_fallback": feedPush({ kind: "drop", t: e.t, p: "Drop", n: e.n, why: "lookup " + e.why + ", path request instead", msg: e.msg }); break;
        case "drop": feedPush({ kind: "drop", t: e.t, p: "Drop", n: e.n, why: e.why, msg: e.msg, alert: e.alert, pid: e.pid }); break;
        case "dead": feedPush({ kind: "ev", t: e.t, p: "Event", n: e.n, text: `node down (${e.why})` }); break;
        case "revive": feedPush({ kind: "ev", t: e.t, p: "Event", n: e.n, text: "node revived" }); break;
        case "event": feedPush({ kind: "ev", t: e.t, p: "Event", n: -1, text: e.what.replace(/_/g, " ") + (e.region ? " " + e.region : "") }); break;
        case "deliver": feedPush({ kind: "ev", t: e.t, p: "Event", n: e.n, text: `message ${e.msg} delivered (${e.hops} hops)`, msg: e.msg }); break;
        case "alert_rx": break;
        default: break;
      }
    }
    const cut = lastT - 60000;
    while (txTimes.length && txTimes[0] < cut) txTimes.shift();
  }
  function fadeAnim() {
    const t = performance.now();
    while (anims.length && t - anims[0].t > 900) animLayer.removeLayer(anims.shift().l);
    if (anims.length > 3000) anims.splice(0, anims.length - 3000).forEach(a => animLayer.removeLayer(a.l));
  }

  function feedVisible(it) {
    if (!animOn[it.p] && PTYPES.includes(it.p)) return false;
    if ($("fd-routed").checked && (FLOODS.has(it.p) || (it.p === "Backbone" && it.msg === undefined && it.alert === undefined))) return false;
    if ($("fd-msg").checked && it.msg !== followMsg) return false;
    return true;
  }
  function rowResult(it) {
    if (it.kind === "tx") {
      if (!it.ok) return `<span class="mute">on air</span>`;
      if (!FLOODS.has(it.p) && it.nx >= 0) {
        const got = it.ok.includes(it.nx), lost = it.coll.includes(it.nx);
        if (got) return `<span class="ok">→ ${it.nx} ✓</span>`;
        if (viaBackbone(it)) return `<span class="ok">⇢ ${it.nx} backbone</span>`;
        return lost ? `<span class="bad">→ ${it.nx} ✗ collision</span>` : `<span class="bad">→ ${it.nx} not heard</span>`;
      }
      return `✓${it.ok.length}${it.coll.length ? ` <span class="bad">✗${it.coll.length}</span>` : ""}`;
    }
    if (it.kind === "bb") return `⇢ ${it.to}`;
    if (it.kind === "dht") return it.to >= 0 ? `gateway ${it.to}` : `<span class="bad">no gateway</span>`;
    if (it.kind === "drop") return `<span class="bad">${esc(it.why)}</span>`;
    return "";
  }
  // A routed packet whose next hop is a backbone peer travels over TCP/Wi-Fi,
  // not the radio: look for the matching backbone copy.
  function viaBackbone(it) {
    for (let k = feed.items.length - 1; k >= 0; k--) {
      const b = feed.items[k];
      if (b.t < it.t - 2000) break;
      if (b.kind === "bb" && b.n === it.n && b.to === it.nx && b.t <= it.t + 2000) return true;
    }
    return false;
  }
  function renderFeed(force) {
    if (feed.paused && !force) return;
    if (!feed.dirty && !force) return;
    feed.dirty = false;
    const rows = [];
    for (let k = feed.items.length - 1; k >= 0 && rows.length < 120; k--) {
      const it = feed.items[k];
      if (!feedVisible(it)) continue;
      const who = it.kind === "ev" ? esc(it.text) : `${esc(name(it.n))}${it.hops ? ` <span class="mute">h${it.hops}</span>` : ""}${it.msg !== undefined ? ` <span class="mute">#${it.msg}</span>` : ""}${it.alert !== undefined ? ` <span class="mute">alert ${it.alert}</span>` : ""}`;
      rows.push(`<div class="fr${feed.sel === it ? " sel" : ""}" data-k="${it.k}"><span class="mute">${fmtT(it.t)}</span><span class="chip" style="background:${PCOLOR[it.p] || "#666"}">${esc(it.p)}</span><span class="who">${who}</span><span class="res">${rowResult(it)}</span></div>`);
    }
    $("feed-rows").innerHTML = rows.join("") || `<div class="fr"><span></span><span></span><span class="mute">Press ▶ to start. Packets appear here.</span><span></span></div>`;
  }
  $("feed-rows").addEventListener("click", (e) => {
    const r = e.target.closest(".fr[data-k]"); if (!r) return;
    const it = feed.items.find(x => x.k === +r.dataset.k);
    if (it) { feed.sel = it; showCard(it); renderFeed(true); }
  });
  $("fd-title").onclick = () => { const m = $("feed").classList.toggle("min"); $("fd-title").textContent = m ? "Live packets ▸" : "Live packets ▾"; };
  $("fd-pause").onclick = () => { feed.paused = !feed.paused; $("fd-pause").textContent = feed.paused ? "▶" : "❚❚"; renderFeed(true); };
  ["fd-routed", "fd-msg"].forEach(id => $(id).onchange = () => renderFeed(true));

  // Detail card for one feed item.
  function showCard(it) {
    const c = $("card");
    c.classList.remove("hidden");
    hiLayer.clearLayers();
    let h = `<h4><span><span class="chip" style="background:${PCOLOR[it.p] || "#666"}">${esc(it.p)}</span> at ${fmtT(it.t)}</span><button class="x" id="card-x">✕</button></h4>`;
    if (it.kind === "tx") {
      const flood = FLOODS.has(it.p);
      h += `<p class="hint">${esc(PEXPLAIN[it.p] || "")}</p>
        <table><tr><td>sender</td><td>${nameH(it.n)}</td></tr>
        <tr><td>hop</td><td>${it.hops}${it.dst >= 0 ? ` · destination ${nameH(it.dst)}` : ""}</td></tr>
        ${it.path ? `<tr><td>source path</td><td>${it.path.map(nameH).join(" → ") || "–"}</td></tr>` : ""}
        ${it.trail ? `<tr><td>trail so far</td><td>${it.trail.length} repeaters${it.trail.length >= 12 ? ' <b class="bad">(full: 12 is the most a source path holds)</b>' : ""}</td></tr>` : ""}
        <tr><td>priority</td><td>${it.prio} · waited ${it.qd} ms in the queue · airtime ${(it.air / 1000).toFixed(0)} ms</td></tr>
        ${it.msg !== undefined ? `<tr><td>message</td><td>#${it.msg}</td></tr>` : ""}</table>`;
      if (it.ok) {
        if (!flood && it.nx >= 0) {
          const got = it.ok.includes(it.nx), k = it.coll.indexOf(it.nx), bbOk = !got && viaBackbone(it);
          h += `<div class="diag ${got || bbOk ? "good" : ""}">Next hop ${nameH(it.nx)}: ${got ? `received it (SNR ${it.snr[it.ok.indexOf(it.nx)]} dB).` : bbOk ? "got it over the backbone (TCP / Wi-Fi), not by radio." : k >= 0 ? `<b>lost it to a collision</b>: ${causeText(it.cw[k], it)}. The sender retransmits when it does not hear the next hop forward.` : "<b>did not hear it</b> (out of range, asleep or dead)."}</div>`;
        }
        h += `<h3>Decoded by ${it.ok.length}</h3><div class="scroll" style="max-height:130px"><table>${it.ok.slice(0, 60).map((r, k) => `<tr><td>${nameH(r)}</td><td class="n">${it.snr[k]} dB</td></tr>`).join("") || "<tr><td>nobody</td></tr>"}</table></div>`;
        if (it.coll.length) {
          h += `<h3>Lost to collisions at ${it.coll.length}</h3><p class="hint">${flood ? "Copies of a flood: harmless while other copies reach the next nodes." : "Bystanders do not matter for a routed packet; only the next hop does."}</p><div class="scroll" style="max-height:130px"><table>${it.coll.slice(0, 40).map((r, k) => `<tr><td>${nameH(r)}</td><td>${causeText(it.cw[k], it)}</td></tr>`).join("")}</table></div>`;
        }
        // Draw it: sender, receivers, collisions.
        const sp = pos(it.n);
        L.circleMarker(sp, { radius: 10, color: "#fff", weight: 2, fill: false }).addTo(hiLayer);
        it.ok.slice(0, 200).forEach(r => L.polyline([sp, pos(r)], { color: PCOLOR[it.p], weight: r === it.nx ? 4 : 1.5, opacity: 0.8 }).addTo(hiLayer));
        it.coll.slice(0, 100).forEach((r, k) => {
          L.circleMarker(pos(r), { radius: 6, color: "#ef4444", weight: 2, fill: false }).addTo(hiLayer);
          if (it.cw[k] >= 0) L.polyline([pos(it.cw[k]), pos(r)], { color: "#ef4444", weight: 1, dashArray: "2 4", opacity: 0.7 }).addTo(hiLayer);
        });
      } else h += `<p class="hint">Still on the air.</p>`;
      h += `<div class="btns">${it.msg !== undefined ? `<button data-a="follow">Follow message #${it.msg}</button>` : ""}${flood ? `<button data-a="reach">How far did it reach?</button>` : ""}<button data-a="sender">Inspect sender</button></div>`;
    } else if (it.kind === "bb") {
      h += `<p>${nameH(it.n)} ⇢ ${nameH(it.to)} over the backbone (TCP or Wi-Fi). The receiving host hands it to its router as if heard with SNR 10 dB.</p>`;
      L.polyline([pos(it.n), pos(it.to)], { color: PCOLOR.Backbone, weight: 4, dashArray: "8 6" }).addTo(hiLayer);
      if (it.msg !== undefined) h += `<div class="btns"><button data-a="follow">Follow message #${it.msg}</button></div>`;
    } else if (it.kind === "dht") {
      h += `<p>${nameH(it.n)} looked up ${it.dst >= 0 ? nameH(it.dst) : "the destination"} in the DHT: ${it.to >= 0 ? `gateway ${nameH(it.to)}; the path request goes there directly.` : "<b>no host with internet can reach it</b>."}</p>`;
    } else if (it.kind === "drop") {
      const why = { busy: "the channel stayed busy for 8 tries (listen-before-talk)", duty: "the duty-cycle budget is used up (10 % airtime)", quota: "path request quota under high load", area: "outside the alert area (area scope)" }[it.why] || it.why;
      h += `<p>${nameH(it.n)} dropped a packet: ${esc(why)}.</p>`;
      if (it.msg !== undefined) h += `<div class="btns"><button data-a="follow">Follow message #${it.msg}</button></div>`;
    } else {
      h += `<p>${it.n >= 0 ? nameH(it.n) + ": " : ""}${esc(it.text)}</p>`;
    }
    c.innerHTML = h;
    $("card-x").onclick = () => { c.classList.add("hidden"); hiLayer.clearLayers(); feed.sel = null; renderFeed(true); };
    c.querySelectorAll("button[data-a]").forEach(b => b.onclick = () => {
      const a = b.dataset.a;
      if (a === "follow") { openTab("journey"); followMessage(it.msg); }
      if (a === "reach") { openTab("journey"); showReach(it.pid, `${it.p} from ${name(it.n)}`); }
      if (a === "sender") selectNode(it.n, true);
    });
  }
  function causeText(by, it) {
    if (by === undefined) return "";
    if (by < 0) return "half-duplex: it was transmitting itself";
    // Which packet did the interferer send? Look for its transmission at the same time.
    const o = feed.items.find(x => x.kind === "tx" && x.n === by && x.t < it.end && x.end > it.t);
    return `${nameH(by)} sent ${o ? `<span class="chip" style="background:${PCOLOR[o.p]}">${o.p}</span>` : "a packet"} at the same time`;
  }

  // Timeline: activity histogram (10 s buckets, colour by packet type), event marks.
  function drawHist() {
    const cv = $("tl-canvas");
    const w = cv.clientWidth, hgt = 34;
    if (cv.width !== w) cv.width = w;
    const ctx = cv.getContext("2d");
    ctx.clearRect(0, 0, w, hgt);
    const bw = 3, n = Math.floor(w / bw);
    const curB = Math.floor(lastT / 10000);
    const first = Math.max(0, curB - n + 1);
    let max = 1;
    for (let b = first; b <= curB; b++) { const h = hist.get(b); if (h) max = Math.max(max, Object.values(h).reduce((a, c) => a + c, 0)); }
    for (let b = first; b <= curB; b++) {
      const h = hist.get(b); if (!h) continue;
      let y = hgt;
      for (const p of PTYPES) {
        const c = h[p]; if (!c) continue;
        const bh = c / max * (hgt - 4);
        ctx.fillStyle = PCOLOR[p];
        ctx.fillRect((b - first) * bw, y - bh, bw - 1, bh);
        y -= bh;
      }
    }
    for (const ev of scenario ? scenario.events : []) {
      const b = Math.floor(ev.t_s / 10);
      if (b < first || b > curB + 60) continue;
      const fix = ev.up === true || ["revive", "power_restore", "internet_up", "degrade_clear"].includes(ev.type);
      const col = fix ? "#22c55e" : "#ef4444";
      ctx.fillStyle = ev.t_s * 1000 <= lastT ? col : col + "66";
      ctx.fillRect((b - first) * bw, 0, 2, hgt);
    }
    // The followed message: sent (blue) and delivered (green), as dots on top.
    if (followMsg >= 0 && replay.f && replay.f.info) {
      const info = replay.f.info;
      for (const [t, col] of [[info.sent_ms, "#60a5fa"], [info.delivered_ms, "#22c55e"]]) {
        if (t === null || t === undefined) continue;
        const b = Math.floor(t / 10000);
        if (b < first || b > curB) continue;
        ctx.fillStyle = col; ctx.beginPath(); ctx.arc((b - first) * bw + 1, 5, 4, 0, 7); ctx.fill();
      }
    }
    ctx.fillStyle = isDark() ? "#e5e7eb" : "#111827";
    ctx.fillRect((curB - first) * bw + bw, 0, 1, hgt);
    cv.title = `Transmissions per 10 s (max ${max}); red = damage, green = repair; blue/green dots = the followed message sent/delivered. ${PTYPES.map(p => p).join(", ")}`;
  }

  function renderHud() {
    if (!sim) return;
    const s = sim.summary();
    lastT = s.t;
    const perMin = txTimes.length;
    $("hud").innerHTML = `<span title="Simulated time">t <b>${fmtTime(s.t / 1000)}</b></span>
      <span title="Transmissions in the last simulated minute"><b>${perMin}</b> tx/min</span>
      <span title="Nodes alive / all"><b>${s.alive}</b>/${s.nodes} up</span>
      <span title="Direct messages delivered / acknowledged"><b>${s.delivered_pct.toFixed(0)} %</b> delivered · ${s.acked_pct.toFixed(0)} % acked (${s.messages})</span>
      <span title="Routed packets lost at the node that should forward them: these cost a retransmission"><b class="${s.coll_next_hop ? "bad" : ""}">${s.coll_next_hop}</b> lost at next hop</span>
      <span title="Copies of floods (path requests, announces, channel messages) that collided at some node. Mostly harmless: other copies get through."><b>${s.coll_flood}</b> flood copies collided</span>
      <span title="Internet backbone">${s.internet ? (s.internet_down.length ? "internet down in " + s.internet_down.join(", ") : "internet up") : '<b class="bad">internet down</b>'}</span>
      ${s.degraded_areas ? `<span class="bad">${s.degraded_areas} radio-loss area(s)</span>` : ""}`;
  }

  // Legend: packet types (click toggles them in the feed and animation), node kinds.
  function renderLegend() {
    $("legend-body").innerHTML = `<div class="sub">Packets (click to hide)</div>${PTYPES.map(p => `<div class="lp" data-p="${p}" style="cursor:pointer;opacity:${animOn[p] ? 1 : 0.35}" title="${esc(PEXPLAIN[p] || "")}"><i style="background:${PCOLOR[p]}"></i>${p}</div>`).join("")}
      <div class="sub">Nodes</div>${Object.entries(KIND_COLOR).map(([k, c]) => `<div><i style="background:${c}"></i>${k}</div>`).join("")}
      <div class="sub">Marks</div><div><i style="background:transparent;border:2px solid #ef4444"></i>collision</div><div><i style="background:transparent;border:2px solid #f97316"></i>on battery</div><div><i style="background:#4b5563"></i>dead</div>
      <div class="hint" style="margin:4px 0 0">With "Colour by island" on, nodes take the colour of their radio island.</div>`;
    $("legend-body").querySelectorAll(".lp").forEach(d => d.onclick = () => { animOn[d.dataset.p] = !animOn[d.dataset.p]; renderLegend(); renderFeed(true); });
  }
  $("lg-toggle").onclick = () => { $("legend-body").classList.toggle("closed"); $("lg-toggle").textContent = $("legend-body").classList.contains("closed") ? "Legend ▸" : "Legend ▾"; };

  // ---------- tools ----------
  $("tools").addEventListener("click", (e) => { const b = e.target.closest("button"); if (b) setTool(b.dataset.tool); });
  function setTool(t) {
    tool = t; dtool = null; pick = null; routeDraft = null; linkDraft = -1;
    document.querySelectorAll("#tools button").forEach(x => x.classList.toggle("on", x.dataset.tool === t));
    $("person-opts").classList.toggle("hidden", t !== "person");
    document.querySelectorAll("[data-dtool]").forEach(x => x.classList.remove("on"));
    const hints = { select: "Click a node to inspect it; drag to move.", delete: "Click a node to delete it (its links go too).", mobile: "Click route points; double-click to finish. The car starts at the first point.", person: "Click A, then waypoints, double-click at B. Set what they carry, speed and timing first.", server: "Click to place a server: a host on the internet without radio (backbone, DHT, post office).", app: "Click to place a phone with the app only: it needs a TCP link to a host (Link: TCP).", "link-tcp": "Click two hosts to link them over TCP (needs internet on both).", "link-wifi": "Click two nodes for a Wi-Fi point-to-point link (no internet needed)." };
    $("tool-hint").textContent = hints[t] || "Click on the map to place it. Edit its settings in the inspector.";
  }
  document.querySelectorAll("[data-dtool]").forEach(b => b.onclick = () => {
    const on = dtool !== b.dataset.dtool;
    setTool("select");
    dtool = on ? b.dataset.dtool : null;
    b.classList.toggle("on", on);
  });

  map.on("click", (e) => {
    const ll = [+e.latlng.lat.toFixed(5), +e.latlng.lng.toFixed(5)];
    if (pendingArea) return areaClick(ll);
    if (walkerPlace) return placeWalkers(ll);
    if (tool === "mobile" || tool === "person") {
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
    if (tool === "person" && routeDraft && routeDraft.length >= 2) {
      L.DomEvent.stop(e);
      const pause = (+$("ps-pause").value || 0) * 60;
      const kind = $("ps-kind").value;
      const town = DK.nearestTown(window.DK_TOWNS, routeDraft[0][0], routeDraft[0][1]);
      scenario.nodes.push({ kind, lat: routeDraft[0][0], lon: routeDraft[0][1], region: town[4], route: routeDraft, speed_kmh: +$("ps-speed").value,
        start_s: (+$("ps-start").value || 0) * 60, pauses_s: routeDraft.map((_, k) => (k > 0 && k < routeDraft.length - 1 ? pause : 0)), route_end: $("ps-end").value, mobile: true, label: "Person " + (scenario.nodes.filter(n => (n.label || "").startsWith("Person")).length + 1) });
      routeDraft = null; drawLayer.clearLayers();
      selected = scenario.nodes.length - 1; edited(); renderInspector();
      return;
    }
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
    if (pick) { $(pick === "from" ? "jr-from" : "jr-to").value = "#" + i; pick = null; document.querySelectorAll("#jr-pick-from,#jr-pick-to").forEach(b => b.classList.remove("on")); return; }
    if (dtool === "kill") {
      const dead = state && state.state[i] === "k";
      return addEvent(dead ? { type: "revive", nodes: [i] } : { type: "kill", nodes: [i] }, dead ? null : { type: "revive", nodes: [i] });
    }
    if (dtool === "net") {
      if (scenario.nodes[i].kind !== "host") return showNote("Pick a host: only hosts have internet links.");
      const up = !(sim && sim.inspect(i).internet);
      return addEvent({ type: "host_internet", node: i, up }, up ? null : { type: "host_internet", node: i, up: true });
    }
    if (tool === "delete") return deleteNode(i);
    if (tool === "link-tcp" || tool === "link-wifi") {
      if (linkDraft < 0) { linkDraft = i; $("tool-hint").textContent = `Link from ${name(i)}: click the other end.`; return; }
      if (linkDraft !== i) { scenario.links.push({ a: linkDraft, b: i, kind: tool === "link-tcp" ? "tcp" : "wifi", up: true }); if (tool === "link-tcp") { scenario.nodes[linkDraft].internet = true; scenario.nodes[i].internet = true; } }
      linkDraft = -1; edited(); return;
    }
    selectNode(i, false);
  }
  function selectNode(i, pan) {
    selected = i; restyle(); drawLinks(); renderInspector();
    if (pan) map.panTo(pos(i));
  }
  function deleteNode(i) {
    scenario.nodes.splice(i, 1);
    scenario.links = scenario.links.filter(l => l.a !== i && l.b !== i).map(l => ({ ...l, a: l.a > i ? l.a - 1 : l.a, b: l.b > i ? l.b - 1 : l.b }));
    selected = -1; $("inspector").classList.add("hidden"); edited();
  }
  let drag = null;
  function startDrag(i) {
    if (tool !== "select" || pendingArea || dtool || pick) return;
    drag = { i, moved: false }; map.dragging.disable();
  }
  map.on("mousemove", (e) => { if (drag) { drag.moved = true; markers[drag.i].setLatLng(e.latlng); } });
  map.on("mouseup", (e) => {
    if (!drag) return;
    map.dragging.enable();
    if (drag.moved) { const n = scenario.nodes[drag.i]; n.lat = +e.latlng.lat.toFixed(5); n.lon = +e.latlng.lng.toFixed(5); selected = drag.i; edited(); renderInspector(); }
    drag = null;
  });

  // ---------- tracking (watch list) ----------
  // A tracked node leaves a line on the map coloured by its best contact
  // (green: hears a repeater, anchor or host; yellow: only handhelds or
  // relays; red: nobody), and the watch list says what it got and when.
  const tracked = new Map();
  const trackLayer = L.layerGroup().addTo(map);
  const FIXED = ["repeater", "anchor", "host", "wifi"];
  function toggleTrack(i) {
    if (tracked.has(i)) tracked.delete(i);
    else tracked.set(i, { pts: [], lost: null, lastCls: null, changes: [] });
    renderWatch(); drawTracks();
  }
  function contactOf(i) {
    const ins = sim.inspect(i);
    const nb = ins.radio_neighbors.filter(([j]) => state && state.state[j] !== "k");
    const fixed = nb.filter(([j]) => FIXED.includes(scenario.nodes[j].kind));
    if (fixed.length) { const best = fixed.sort((a, b) => b[1] - a[1])[0]; return { cls: "good", via: best[0], snr: best[1], ins }; }
    if (nb.length) { const best = nb.sort((a, b) => b[1] - a[1])[0]; return { cls: "weak", via: best[0], snr: best[1], ins }; }
    return { cls: "none", via: -1, snr: null, ins };
  }
  function sampleTracks() {
    if (!sim || !tracked.size) return;
    const t = now();
    for (const [i, tr] of tracked) {
      const c = contactOf(i);
      const p = pos(i);
      const last = tr.pts[tr.pts.length - 1];
      if (!last || last.t + 2000 <= t) tr.pts.push({ ll: p, cls: c.cls, t });
      if (tr.lastCls !== c.cls) {
        tr.changes.push({ t, cls: c.cls, via: c.via });
        if (tr.changes.length > 40) tr.changes.shift();
        tr.lost = c.cls === "none" ? t : null;
        tr.lastCls = c.cls;
      }
      tr.now = c;
    }
    drawTracks();
    if (!$("watch").classList.contains("hidden")) renderWatch();
  }
  function drawTracks() {
    trackLayer.clearLayers();
    const col = { good: "#22c55e", weak: "#eab308", none: "#ef4444" };
    for (const [, tr] of tracked) {
      for (let k = 1; k < tr.pts.length; k++) {
        const a = tr.pts[k - 1], b = tr.pts[k];
        L.polyline([a.ll, b.ll], { color: col[b.cls], weight: 4, opacity: 0.85, interactive: false }).addTo(trackLayer);
      }
    }
  }
  function renderWatch() {
    const box = $("watch");
    if (!tracked.size) { box.classList.add("hidden"); return; }
    box.classList.remove("hidden");
    const t = now();
    const all = sim ? sim.msgs(0, 100000).msgs : [];
    box.innerHTML = `<div class="w-head"><b>Watch list</b><span class="mute">${tracked.size} tracked</span></div>` + [...tracked.entries()].map(([i, tr]) => {
      const c = tr.now;
      const ins = c ? c.ins : null;
      const got = all.filter(m => m[2] === i), sent = all.filter(m => m[1] === i);
      const lastGot = got.filter(m => m[4] !== null).pop();
      const st = !c ? "starting…" : c.cls === "good" ? `in contact via ${nameH(c.via)} (${c.snr} dB)` : c.cls === "weak" ? `only near ${nameH(c.via)} (no repeater)` : `<span class="bad">no contact for ${fmtTime((t - tr.lost) / 1000)}</span>`;
      const role = ins ? ins.role : "";
      return `<div class="w-node"><div class="w-name">${nameH(i)} <button class="x" data-untrack="${i}" aria-label="Stop tracking">✕</button></div>
        <div>${st}</div>
        <div class="mute">${role ? "role " + esc(role) : ""}${role === "Relay" && scenario.nodes[i].kind === "handheld" ? " (forest mode)" : ""}${ins && ins.on_battery !== undefined ? ` · battery ${ins.battery_max_wh ? Math.round(100 * ins.battery_wh / ins.battery_max_wh) + " %" : "–"}` : ""}</div>
        <div class="mute">got ${got.filter(m => m[4] !== null).length} of ${got.length} messages${lastGot ? `, last at ${fmtTime(lastGot[4] / 1000)}` : ""} · sent ${sent.length} (${sent.filter(m => m[4] !== null).length} delivered)</div>
        <div class="w-changes">${tr.changes.slice(-4).map(ch => `<span class="w-${ch.cls}">${fmtTime(ch.t / 1000)} ${ch.cls === "good" ? "contact" : ch.cls === "weak" ? "weak" : "lost"}</span>`).join(" ")}</div></div>`;
    }).join("");
    box.querySelectorAll("[data-untrack]").forEach(b => b.onclick = () => toggleTrack(+b.dataset.untrack));
  }
  setInterval(sampleTracks, 1000);

  // ---------- right-click menus on the map ----------
  function showMenu(e, title, items) {
    const box = $("ctx");
    const pt = map.mouseEventToContainerPoint(e.originalEvent);
    box.innerHTML = `<div class="ctx-title">${esc(title)}</div>` + items.map((it, k) => `<button role="menuitem" data-k="${k}" class="${it.danger ? "danger" : ""}">${esc(it.label)}</button>`).join("");
    box.classList.remove("hidden");
    const W = map.getContainer().clientWidth, H = map.getContainer().clientHeight;
    box.style.left = Math.min(pt.x, W - box.offsetWidth - 8) + "px";
    box.style.top = Math.min(pt.y, H - box.offsetHeight - 8) + "px";
    box.querySelectorAll("button").forEach(b => b.onclick = () => { hideMenu(); items[+b.dataset.k].run(); });
    const first = box.querySelector("button"); if (first) first.focus();
  }
  function hideMenu() { $("ctx").classList.add("hidden"); }
  document.addEventListener("keydown", (e) => { if (e.key === "Escape") hideMenu(); });
  map.on("click movestart zoomstart", hideMenu);
  function nodeMenu(i, e) {
    const n = scenario.nodes[i];
    const dead = state && state.state[i] === "k";
    const items = [
      { label: "Inspect", run: () => selectNode(i, false) },
      { label: "Send a message from here…", run: () => { $("jr-from").value = "#" + i; openTab("journey"); } },
      { label: "Send a message to here…", run: () => { $("jr-to").value = "#" + i; openTab("journey"); } },
      { label: "Announce now: show reach", run: () => { selected = i; renderInspector(); $("in-announce").click(); } },
      dead ? { label: "Start this node again", run: () => addEvent({ type: "revive", nodes: [i] }) }
           : { label: "Stop this node", run: () => addEvent({ type: "kill", nodes: [i] }, { type: "revive", nodes: [i] }) },
      { label: "Reboot", run: () => addEvent({ type: "reboot", nodes: [i] }) },
      { label: tracked.has(i) ? "Stop tracking" : "Track this node (contact, messages, battery)", run: () => toggleTrack(i) },
    ];
    if (n.kind === "host") {
      const up = !(sim && sim.inspect(i).internet);
      items.push({ label: up ? "Give it internet again" : "Cut its internet", run: () => addEvent({ type: "host_internet", node: i, up }, up ? null : { type: "host_internet", node: i, up: true }) });
    }
    items.push({ label: "Delete", danger: true, run: () => deleteNode(i) });
    showMenu(e, name(i), items);
  }
  function linkMenu(k, e) {
    const l = scenario.links[k];
    const ok = state && state.links_ok ? state.links_ok[k] : l.up;
    showMenu(e, `${l.label || l.kind.toUpperCase() + " link"}: ${name(l.a)} ↔ ${name(l.b)}`, [
      { label: ok ? "Cut this link" : "Restore this link", run: () => toggleLink(k) },
      { label: "Inspect " + name(l.a), run: () => selectNode(l.a, true) },
      { label: "Inspect " + name(l.b), run: () => selectNode(l.b, true) },
      { label: "Delete link", danger: true, run: () => { scenario.links.splice(k, 1); edited(); } },
    ]);
  }

  function toggleLink(k) {
    const up = !(state && state.links_ok ? state.links_ok[k] : scenario.links[k].up);
    if (sim && now() > 0) addEvent({ type: "link", index: k, up }, up ? null : { type: "link", index: k, up: true });
    else { scenario.links[k].up = up; edited(); }
  }

  // ---------- areas ----------
  function beginArea(kind, purpose) {
    pendingArea = { kind, purpose, pts: [] };
    drawLayer.clearLayers();
    showNote(kind === "circle" ? "Click the centre, then click the edge." : "Click the corners on the map; double-click to finish.");
    map.getContainer().style.cursor = "crosshair";
  }
  function areaClick(ll) {
    const a = pendingArea;
    a.pts.push(ll);
    drawLayer.clearLayers();
    if (a.kind === "circle") {
      if (a.pts.length === 2) return finishArea();
      L.circleMarker(ll, { radius: 4 }).addTo(drawLayer);
    } else L.polygon(a.pts, { color: "#ef4444" }).addTo(drawLayer);
  }
  function inPoly(lat, lon, poly) {
    let inside = false;
    for (let i = 0, j = poly.length - 1; i < poly.length; j = i++) {
      const [yi, xi] = poly[i], [yj, xj] = poly[j];
      if ((yi > lat) !== (yj > lat) && lon < (xj - xi) * (lat - yi) / (yj - yi) + xi) inside = !inside;
    }
    return inside;
  }
  function finishArea() {
    const a = pendingArea; pendingArea = null;
    map.getContainer().style.cursor = "";
    drawLayer.clearLayers();
    let area;
    if (a.kind === "circle") {
      if (a.pts.length < 2) return;
      area = { circle: [a.pts[0][0], a.pts[0][1], Math.round(DK.km(a.pts[0], a.pts[1]) * 1000)] };
    } else {
      if (a.pts.length < 3) return;
      area = { polygon: a.pts };
    }
    switch (a.purpose) {
      case "alert": addEvent({ type: "alert", from: +$("al-from").value, alert: $("al-kind").value, text: $("al-text").value, area }); break;
      case "power": addEvent({ type: "power_cut", area }, { type: "power_restore", area }); break;
      case "reboot": addEvent({ type: "reboot", area, node_kind: $("ev-kind").value }); break;
      case "degrade": addEvent({ type: "degrade", area, db: +$("ev-db").value }, { type: "degrade_clear" }); break;
      case "kill_area": {
        const want = $("ev-kind").value;
        const nodes = scenario.nodes.map((n, i) => [n, i]).filter(([n]) => (want === "any" || n.kind === want) && inPoly(n.lat, n.lon, area.polygon)).map(([, i]) => i);
        if (!nodes.length) return showNote("No such nodes in that area.");
        addEvent({ type: "kill_area", area, node_kind: want, nodes }, { type: "revive", nodes });
        showNote(`Destroyed ${nodes.length} nodes.`);
        break;
      }
      case "clutter":
        scenario.clutter = scenario.clutter || [];
        scenario.clutter.push({ label: "Forest " + (scenario.clutter.length + 1), polygon: area.polygon, db_per_km: +$("cl-db").value });
        edited();
        break;
    }
  }

  // ---------- timeline / disaster ----------
  // `undo` is applied after the duration in "for N min" (if given).
  function addEvent(ev, undo) {
    const at = $("ev-at").value, dur = $("ev-dur").value;
    const t = now() / 1000;
    ev.t_s = at !== "" ? +at : +t.toFixed(1);
    const list = [ev];
    if (undo && dur !== "" && +dur > 0) list.push(Object.assign({}, undo, { t_s: +(ev.t_s + +dur * 60).toFixed(1) }));
    for (const e of list) {
      scenario.events.push(e);
      if (!sim) continue;
      if (e.t_s <= t + 0.01) sim.inject(e);
      else if (t === 0) { /* rebuild below schedules it */ }
      else jsPending.push(e);
    }
    if (sim && t === 0 && list.some(e => e.t_s > 0)) rebuild();
    else if (sim) { state = sim.state(); restyle(); drawLinks(); renderAlerts(); }
    renderTimeline(); drawDisasters();
  }
  document.querySelectorAll("#tab-disaster button[data-ev]").forEach(b => b.addEventListener("click", () => {
    const k = b.dataset.ev;
    switch (k) {
      case "internet_down_region": { const r = $("ev-region").value; return addEvent({ type: "internet_down", region: r }, { type: "internet_up", region: r }); }
      case "internet_down": return addEvent({ type: "internet_down" }, { type: "internet_up" });
      case "power_cut": return beginArea("polygon", "power");
      case "power_cut_all": return addEvent({ type: "power_cut" }, { type: "power_restore" });
      case "kill_area": return beginArea("polygon", "kill_area");
      case "degrade": return beginArea("polygon", "degrade");
      case "kill_random": return addEvent({ type: "kill_random", n: +$("ev-n").value, node_kind: "repeater" }, { type: "revive" });
      case "reboot_random": return addEvent({ type: "reboot", n: +$("ev-n").value, node_kind: $("ev-kind").value });
      case "reboot_area": return beginArea("polygon", "reboot");
      default: return addEvent({ type: k });
    }
  }));
  function describe(ev) {
    switch (ev.type) {
      case "radio": return "Every node switches radio: " + [ev.sf && "SF" + ev.sf, ev.freq_mhz && ev.freq_mhz + " MHz", ev.bw_hz && ev.bw_hz / 1000 + " kHz", ev.tx_dbm && ev.tx_dbm + " dBm"].filter(Boolean).join(", ");
      case "internet_down": return ev.region ? `Internet down in ${ev.region}` : "Internet down everywhere";
      case "internet_up": return ev.region ? `Internet back in ${ev.region}` : "Internet back";
      case "power_cut": return ev.area && (ev.area.circle || (ev.area.polygon || []).length) ? "Power cut in area" : "Power cut everywhere";
      case "power_restore": return ev.area && (ev.area.circle || (ev.area.polygon || []).length) ? "Power back in area" : "Power back";
      case "kill_random": return `Kill ${ev.n} random ${ev.node_kind || "repeater"}s`;
      case "kill": return `Kill ${(ev.nodes || [ev.node]).map(name).join(", ")}`;
      case "kill_area": return `Destroy ${ev.node_kind === "any" ? "all nodes" : ev.node_kind + "s"} in area (${(ev.nodes || []).length})`;
      case "revive": return ev.nodes && ev.nodes.length ? `Revive ${ev.nodes.length <= 3 ? ev.nodes.map(name).join(", ") : ev.nodes.length + " nodes"}` : "Revive all killed nodes";
      case "host_internet": return `${name(ev.node)}: internet ${ev.up ? "back" : "cut"}`;
      case "reboot": return ev.nodes && ev.nodes.length ? `Reboot ${ev.nodes.length <= 3 ? ev.nodes.map(name).join(", ") : ev.nodes.length + " nodes"}` : `Reboot ${ev.n ? ev.n + " random " : "all "}${ev.node_kind && ev.node_kind !== "any" ? ev.node_kind + "s" : "nodes"}${ev.area && (ev.area.circle || (ev.area.polygon || []).length) ? " in area" : ""}`;
      case "degrade": return `Radio loss ${ev.db} dB in area`;
      case "degrade_clear": return "Radio loss cleared";
      case "link": return `Backbone link ${ev.index} ${ev.up ? "up" : "down"}`;
      case "alert": return `${ev.alert} alert from ${name(ev.from)}: "${ev.text || ""}"`;
      case "announce": return `Announce from ${name(ev.node)}`;
      case "set_traffic": return "Change traffic" + (ev.traffic && ev.traffic.spam_per_min ? ` (spam ${ev.traffic.spam_per_min}/min from ${ev.traffic.spammers.length} nodes)` : "");
      default: return ev.type;
    }
  }
  function renderTimeline() {
    const evs = scenario.events.map((e, k) => [e, k]).sort((a, b) => a[0].t_s - b[0].t_s);
    $("timeline").innerHTML = evs.length ? `<table>${evs.map(([e, k]) => `<tr><td><input type="number" value="${e.t_s}" data-k="${k}" style="width:70px"> s</td><td>${esc(describe(e))}</td><td><button data-del="${k}">✕</button></td></tr>`).join("")}</table><p class="hint">Edit times, then Reset to replay.</p>` : `<p class="hint">No events.</p>`;
    $("timeline").querySelectorAll("input").forEach(i => i.addEventListener("change", () => { scenario.events[+i.dataset.k].t_s = +i.value; renderTimeline(); }));
    $("timeline").querySelectorAll("button[data-del]").forEach(b => b.addEventListener("click", () => { scenario.events.splice(+b.dataset.del, 1); renderTimeline(); drawDisasters(); }));
  }

  // ---------- terrain ----------
  $("btn-clutter").onclick = () => beginArea("polygon", "clutter");
  function renderClutterList() {
    const cl = scenario.clutter || [];
    $("clutter-list").innerHTML = cl.length ? `<table>${cl.map((c, k) => `<tr><td>${esc(c.label || "Forest " + (k + 1))}</td><td><input type="number" value="${c.db_per_km}" data-k="${k}" style="width:55px"> dB/km</td><td><button data-del="${k}">✕</button></td></tr>`).join("")}</table><p class="hint">Changes rebuild the run.</p>` : `<p class="hint">No forest areas.</p>`;
    $("clutter-list").querySelectorAll("input").forEach(i => i.onchange = () => { scenario.clutter[+i.dataset.k].db_per_km = +i.value; edited(); });
    $("clutter-list").querySelectorAll("button[data-del]").forEach(b => b.onclick = () => { scenario.clutter.splice(+b.dataset.del, 1); edited(); });
  }
  let walkerPlace = false;
  $("btn-walkers").onclick = () => { walkerPlace = true; showNote("Click the map where the group should be."); map.getContainer().style.cursor = "crosshair"; };
  function placeWalkers(ll) {
    walkerPlace = false; map.getContainer().style.cursor = "";
    const n = +$("wk-n").value, r = +$("wk-r").value, v = +$("wk-v").value, role = $("wk-role").value;
    const R = DK.rng(scenario.nodes.length + 7);
    const town = DK.nearestTown(window.DK_TOWNS, ll[0], ll[1]);
    for (let k = 0; k < n; k++) {
      const [la, lo] = DK.offset(ll[0], ll[1], (R() - 0.5) * r, (R() - 0.5) * r);
      scenario.nodes.push({ kind: role, lat: +la.toFixed(5), lon: +lo.toFixed(5), wander_m: r * 1000, speed_kmh: v, region: town[4], label: `Walker ${k + 1} (${town[0]})` });
    }
    edited();
  }
  function fillForestForm(o) {
    o = Object.assign({}, DK.FOREST_DEFAULTS, o || {});
    $("fo-walkers").value = o.walkers; $("fo-role").value = o.role; $("fo-r").value = o.radiusKm; $("fo-db").value = o.dbPerKm;
    $("fo-rep").value = o.repeaterKm; $("fo-hill").checked = o.hill; $("fo-hillh").value = o.hillH; $("fo-v").value = o.walkKmh;
  }
  $("btn-forest").onclick = () => {
    scenario = DK.forest({ walkers: +$("fo-walkers").value, role: $("fo-role").value, radiusKm: +$("fo-r").value, dbPerKm: +$("fo-db").value, repeaterKm: +$("fo-rep").value, hill: $("fo-hill").checked, hillH: +$("fo-hillh").value, walkKmh: +$("fo-v").value, seed: +$("g-seed").value || 1 });
    syncForm(); edited(); fitScenario();
    $("preset").value = "forest";
  };

  // ---------- alerts ----------
  function fillSenders() {
    const sel = $("al-from"); const cur = sel.value;
    const opts = scenario.nodes.map((n, i) => [n, i]).filter(([n]) => n.kind === "host" || n.kind === "anchor");
    sel.innerHTML = (opts.length ? opts : scenario.nodes.map((n, i) => [n, i])).map(([, i]) => `<option value="${i}">${esc(name(i))}</option>`).join("");
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
  function heat(f) { return `hsl(${120 - 120 * Math.min(1, Math.max(0, f))},85%,50%)`; }
  function showAlert(i) {
    const a = sim.alert(i);
    overlay = new Map();
    const max = Math.max(1, ...a.recv.filter(r => r[2]).map(r => r[1]));
    for (const n of a.inside_nodes) overlay.set(n, "#000");
    for (const [n, ms, inside] of a.recv) overlay.set(n, inside ? heat(ms / max) : "#93c5fd");
    restyle();
    hiLayer.clearLayers();
    const ev = scenario.events.filter(e => e.type === "alert")[i];
    if (ev && ev.area.circle) L.circle([ev.area.circle[0], ev.area.circle[1]], { radius: ev.area.circle[2], color: "#ef4444", fill: false }).addTo(hiLayer);
    else if (ev) L.polygon(ev.area.polygon, { color: "#ef4444", fill: false }).addTo(hiLayer);
  }

  // ---------- journey: one message, told and replayed ----------
  const norm = (s) => s.toLowerCase().replace(/æ/g, "ae").replace(/ø/g, "oe").replace(/å/g, "aa").trim();
  $("town-list").innerHTML = window.DK_TOWNS.map(t => `<option value="${esc(t[0])}">`).join("");
  function resolveEnd(text) {
    text = (text || "").trim();
    const m = text.match(/^#?(\d+)$/);
    if (m && +m[1] < scenario.nodes.length) return +m[1];
    const t = window.DK_TOWNS.find(x => norm(x[0]) === norm(text)) || window.DK_TOWNS.find(x => norm(x[0]).startsWith(norm(text)));
    if (!t || !text) return -1;
    let best = -1, bd = 1e9;
    scenario.nodes.forEach((n, i) => {
      if (!["handheld", "mobile", "relay"].includes(n.kind)) return;
      if (state && state.state[i] !== "a" && state.state[i] !== "b") return;
      const d = DK.km([t[1], t[2]], [n.lat, n.lon]);
      if (d < bd) { bd = d; best = i; }
    });
    return best;
  }
  ["from", "to"].forEach(w => $("jr-pick-" + w).onclick = () => {
    pick = pick === w ? null : w;
    document.querySelectorAll("#jr-pick-from,#jr-pick-to").forEach(b => b.classList.toggle("on", b.id === "jr-pick-" + pick));
    if (pick) showNote(`Click the node the message should go ${w}.`);
  });
  $("btn-journey").onclick = () => {
    if (!sim) return;
    const a = resolveEnd($("jr-from").value), b = resolveEnd($("jr-to").value);
    if (a < 0 || b < 0 || a === b) { $("journey").innerHTML = `<div class="diag">Pick two different nodes (a town name finds the nearest handheld there).</div>`; return; }
    if ($("jr-quiet").checked) sim.inject({ type: "set_traffic", traffic: Object.assign({}, scenario.traffic, { direct_per_min: 0, channel_per_min: 0, spam_per_min: 0 }) });
    const r = sim.send(a, b);
    if (!playing) togglePlay();
    $("fd-msg").checked = true; renderFeed(true);
    followMessage(r.msg);
    fitAround([pos(a), pos(b)]);
  };
  function clearJourney() {
    followMsg = -1; journeyKey = ""; replay.playing = false; replay.t = null;
    overlay = null; hiLayer.clearLayers(); restyle();
    $("journey").innerHTML = ""; $("follow").classList.add("hidden"); $("mapwrap").classList.remove("following"); if (whereMarker) { whereMarker.remove(); whereMarker = null; } $("fd-msg").checked = false; renderFeed(true);
  }
  // Where the followed message is now, in plain words, from its last event.
  function followStatus(f) {
    const info = f.info;
    if (info.acked_ms !== null) return { cls: "ok", title: "Delivered and acknowledged", text: `Delivered after ${((info.delivered_ms - info.sent_ms) / 1000).toFixed(1)} s; the sender has the acknowledgement.` };
    if (info.delivered_ms !== null) return { cls: "ok", title: "Delivered", text: `Arrived after ${((info.delivered_ms - info.sent_ms) / 1000).toFixed(1)} s; waiting for the acknowledgement.` };
    // Relays cancelling their flood copy (neighbours covered it) is no news.
    const evs = f.events.filter(e => !["rx", "dht", "lookup_answer"].includes(e.ev) && !(e.ev === "cancel" && e.n !== info.from));
    const e = evs[evs.length - 1];
    const at = e && e.n !== undefined ? ` at ${nameH(e.n)}` : "";
    const S = {
      mail_send: ["post", "Waiting in a post office", `Kept${at} for the receiver; sent on when the receiver is heard again.`],
      mail_op: ["post", "Waiting in a post office", `Kept${at}; the post office is syncing it.`],
      carry: ["post", "Carried", `A node keeps a copy${at} and passes it on when it meets a way forward.`],
      held: ["warn", "Held", `The receiver has it but does not know the sender yet; it waits for an announce.`],
      bb: ["air", "On the backbone", `Between hosts over the internet or a trunk${at}.`],
      tx: ["air", "On air", `A radio hop is being sent${at}.`],
      send: ["wait", "Searching for a route", `Sent${at}; the route search is running.`],
      lookup: ["wait", "Looking up where the receiver is", `A host asks the backbone${at}.`],
      defer: ["wait", "Queued", `Waiting for a free channel or duty cycle${at}.`],
      retry: ["wait", "Trying again", `No acknowledgement from the next hop yet${at}.`],
      drop: ["bad", "A copy was lost", `Dropped${at}${e && e.why ? ": " + esc(e.why) : ""}. Other copies or a post office may still carry it.`],
      cancel: ["bad", "Given up", `The sender stopped trying${e && e.why ? ": " + esc(e.why) : ""}.`],
    };
    const v = e && S[e.ev];
    if (!v) return { cls: "wait", title: "On its way", text: "No news from it yet." };
    const idle = (now() - (e.end || e.t)) / 1000;
    if (idle > 20 && v[0] !== "post" && v[0] !== "warn") return { cls: "bad", title: "No progress", text: `Nothing has happened to it for ${fmtTime(idle)}. Last: ${v[1].toLowerCase()}${at}. No route, no post office and no carrier has it.` };
    return { cls: v[0], title: v[1], text: v[2] };
  }
  // A pulsing dot on the map where the followed message is now.
  let whereMarker = null;
  function placeWhere(f, st) {
    const info = f.info;
    let n = info.delivered_ms !== null ? info.to : -1;
    if (n < 0) { const evs = f.events.filter(e => e.n !== undefined && !(e.ev === "cancel" && e.n !== info.from)); n = evs.length ? evs[evs.length - 1].n : info.from; }
    const col = { ok: "#22c55e", post: "#a78bfa", bad: "#ef4444", warn: "#f59e0b", air: "#60a5fa" }[st.cls] || "#94a3b8";
    if (!whereMarker) whereMarker = L.marker(pos(n), { interactive: false, icon: L.divIcon({ className: "where-dot", html: '<span></span>', iconSize: [22, 22] }) }).addTo(map);
    whereMarker.setLatLng(pos(n));
    whereMarker.getElement() && whereMarker.getElement().style.setProperty("--c", col);
  }
  function renderFollowHead(f) {
    const st = followStatus(f), info = f.info;
    placeWhere(f, st);
    $("follow-head").innerHTML = `<div class="fh-top"><div><div class="fh-kicker">Following</div><b>Message #${followMsg}</b></div><button class="x" id="fh-close" aria-label="Stop following">✕</button></div>
      <div class="fh-ends">${nameH(info.from)} <span class="mute">→</span> ${nameH(info.to)}</div>
      <div class="fh-status ${st.cls}"><b>${st.title}</b><div>${st.text}</div></div>`;
    $("fh-close").onclick = clearJourney;
    $("follow").classList.remove("hidden");
    $("mapwrap").classList.add("following");
  }
  setInterval(() => { if (followMsg >= 0 && replay.f && !$("follow").classList.contains("hidden")) renderFollowHead(replay.f); }, 1000);
  function followMessage(m) {
    followMsg = m; journeyKey = "";
    replay.t = null;
    renderJourney();
  }

  function renderMsgList() {
    if (!sim) return;
    const total = sim.summary().messages;
    const r = sim.msgs(Math.max(0, total - 200), 200);
    $("msg-list").innerHTML = `<table><tr><th>#</th><th>from → to</th><th>sent</th><th>delivered</th><th>ack</th></tr>${r.msgs.slice().reverse().map(m => `<tr class="click" data-m="${m[0]}"><td>${m[0]}</td><td>${esc(townOf[m[1]] || m[1])} → ${esc(townOf[m[2]] || m[2])}</td><td class="n">${fmtTime(m[3] / 1000)}</td><td class="n ${m[4] === null ? "bad" : ""}">${m[4] !== null ? ((m[4] - m[3]) / 1000).toFixed(1) + " s" : "–"}</td><td>${m[5] !== null ? "✓" : ""}</td></tr>`).join("")}</table>`;
    $("msg-list").querySelectorAll("tr[data-m]").forEach(t => t.onclick = () => followMessage(+t.dataset.m));
  }
  setInterval(() => { if ($("tab-journey").classList.contains("on") && sim) renderMsgList(); }, 2500);

  const replay = { t: null, playing: false, f: null, t0: 0, t1: 0 };

  function renderJourney() {
    if (followMsg < 0 || !sim) return;
    const f = sim.follow(followMsg);
    const key = followMsg + ":" + f.events.length;
    if (key === journeyKey) return;
    journeyKey = key;
    replay.f = f;
    const info = f.info;
    replay.t0 = info.sent_ms;
    replay.t1 = Math.max(info.sent_ms + 1, ...f.events.map(e => e.end || e.t));
    const story = narrate(f);
    const done = info.acked_ms !== null;
    renderFollowHead(f);
    $("journey").innerHTML = `<h3>Message #${followMsg}: ${nameH(info.from)} → ${nameH(info.to)}</h3>
      <ol class="story">${story.steps.map(s => `<li class="${s.cls || ""}">${s.html}</li>`).join("")}</ol>
      ${story.diag.map(d => `<div class="diag ${d.good ? "good" : ""}">${d.html}</div>`).join("")}
      <div class="replay"><button id="rp-play">${replay.playing ? "❚❚" : "▶ Replay"}</button><input type="range" id="rp-t" min="0" max="1000" value="${replay.t === null ? 1000 : Math.round((replay.t - replay.t0) / (replay.t1 - replay.t0) * 1000)}"><span id="rp-label" class="mono"></span></div>
      <p class="hint">Map: faint orange = path request copies, dashed teal = backbone, green = path response, thick blue = message hops, dashed green = ack, white = the route that won (numbered). ${done ? "" : "Updates while it travels."}</p>
      <div class="row">${story.prPid ? `<button id="jr-reach">How far did the path request reach?</button>` : ""}<button id="jr-clear">Clear</button></div>
      <p class="hint">Node colours: blue = sender, green = destination, yellow = the route, brown = heard the path request, dim = not involved.</p>`;
    if (story.prPid) $("jr-reach").onclick = () => showReach(story.prPid, `Path request of message #${followMsg}`);
    $("jr-clear").onclick = clearJourney;
    // Colour only what matters for this message; dim the rest.
    overlay = new Map();
    f.events.forEach(e => { if (e.ev === "rx" && e.ok.length && (f.events.find(t => t.ev === "tx" && t.id === e.id) || {}).p === "PathRequest") e.ok.forEach(r => overlay.set(r, "#92400e")); });
    journeyRoute(f).forEach(n => overlay.set(n, "#fde047"));
    overlay.set(info.from, "#60a5fa"); overlay.set(info.to, "#22c55e");
    restyle();
    $("rp-play").onclick = () => { replay.playing = !replay.playing; if (replay.playing && (replay.t === null || replay.t >= replay.t1)) replay.t = replay.t0; tickReplay(); };
    $("rp-t").oninput = () => { replay.playing = false; replay.t = replay.t0 + (+$("rp-t").value / 1000) * (replay.t1 - replay.t0); drawJourney(replay.f, replay.t); };
    drawJourney(f, replay.t === null ? Infinity : replay.t);
  }
  let replayLast = 0;
  function tickReplay(ts) {
    if (!replay.playing) { renderJourneyButtons(); return; }
    const dt = replayLast && ts ? ts - replayLast : 16;
    replayLast = ts || 0;
    // The whole journey in about 10 s of real time.
    replay.t = Math.min(replay.t1, replay.t + (replay.t1 - replay.t0) * dt / 10000);
    drawJourney(replay.f, replay.t);
    const r = $("rp-t"); if (r) r.value = Math.round((replay.t - replay.t0) / (replay.t1 - replay.t0) * 1000);
    if (replay.t >= replay.t1) { replay.playing = false; replayLast = 0; renderJourneyButtons(); return; }
    requestAnimationFrame(tickReplay);
  }
  function renderJourneyButtons() { const b = $("rp-play"); if (b) b.textContent = replay.playing ? "❚❚" : "▶ Replay"; }

  // The message's story from its trace lines.
  function narrate(f) {
    const info = f.info, ev = f.events;
    const steps = [], diag = [];
    const txs = new Map();
    ev.forEach(e => { if (e.ev === "tx") txs.set(e.id, e); });
    const rel = (t) => `+${((t - info.sent_ms) / 1000).toFixed(2)} s`;
    steps.push({ html: `${nameH(info.from)} sends at ${fmtT(info.sent_ms)}.` });
    const first = ev.find(e => e.ev === "tx" && e.n === info.from && e.hops === 0);
    const pr = ev.filter(e => e.ev === "tx" && e.p === "PathRequest");
    const prRx = ev.filter(e => e.ev === "rx" && txs.get(e.id) && txs.get(e.id).p === "PathRequest");
    const bb = ev.filter(e => e.ev === "bb");
    const dht = ev.filter(e => e.ev === "dht");
    let prPid = pr.length ? pr[0].pid : null;
    // Home routing model: the lookup comes first.
    const look = ev.find(e => e.ev === "lookup"), ans = ev.find(e => e.ev === "lookup_answer"), done = ev.find(e => e.ev === "lookup_done"), fb = ev.find(e => e.ev === "lookup_fallback"), split = ev.find(e => e.ev === "route_split");
    if (look) steps.push({ html: `Home routing: ${nameH(info.from)} asks ${look.local ? "its home repeater" : "the gateway"} ${nameH(look.to)} where ${nameH(info.to)} is${look.path && look.path.length ? ` (via ${look.path.map(nameH).join(" → ")})` : ""}.` });
    if (ans) steps.push({ cls: ans.ok ? "" : "bad", html: ans.ok ? `${nameH(ans.n)} answers at ${rel(ans.t)} with a route over the fixed infrastructure: ${ans.path.map(nameH).join(" → ") || "direct"}${split ? `, and gives the anchor ${nameH(split.n)} the rest of the way (${split.path.map(nameH).join(" → ")})` : ""}.` : `${nameH(ans.n)} has no route (${esc(ans.why)}).` });
    if (done && done.unreachable) steps.push({ cls: "bad", html: "The destination is known to be unreachable (another island without a working gateway), so nothing is flooded. In the real system the message would wait in the mailbox." });
    if (fb) steps.push({ cls: "bad", html: `The lookup failed (${esc(fb.why)}), so it falls back to a path request.` });
    if (first && first.p === "PathRequest") steps.push({ html: `It has no route to ${nameH(info.to)}, so it floods a <b>path request</b> that carries the message. Every repeater that hears it adds itself to the trail and passes it on.` });
    else if (first && first.p === "Data") steps.push({ html: `It knows a route (${first.path ? first.path.length : 0} repeaters), so it sends the message along it: ${first.path ? first.path.map(nameH).join(" → ") : ""}.` });
    else if (!first && info.delivered_ms === null) steps.push({ cls: "wait", html: "Not on the air yet (queued, or the sender is down)." });
    if (pr.length) {
      const senders = new Set(pr.map(e => e.n));
      const regions = new Set(pr.map(e => scenario.nodes[e.n].region).filter(Boolean));
      const heard = new Set(); let copiesLost = 0;
      prRx.forEach(e => { e.ok.forEach(r => heard.add(r)); copiesLost += e.coll.length; });
      const maxTrail = Math.max(0, ...pr.map(e => (e.trail || []).length));
      const hostsTx = pr.filter(e => scenario.nodes[e.n].kind === "host").length;
      steps.push({ html: `The path request went on the air <b>${pr.length}</b> times from ${senders.size} nodes in ${regions.size} region${regions.size === 1 ? "" : "s"} (${[...regions].join(", ") || "–"}), crossed the backbone <b>${bb.length}</b> times${hostsTx ? `, and ${hostsTx} hosts flooded it into their own area` : ""}. ${heard.size} nodes heard it. ${copiesLost} copies collided somewhere: harmless as long as other copies get through. Longest trail: ${maxTrail} repeaters${maxTrail >= 12 ? ' <b class="bad">(the 12-repeater limit of a source path)</b>' : ""}.` });
      if (dht.length) steps.push({ html: `DHT: ${dht.map(d => `${nameH(d.n)} looked up the destination: ${d.to >= 0 ? "gateway " + nameH(d.to) : '<b class="bad">no gateway</b>'}`).slice(0, 4).join("; ")}.` });
    }
    const deliver = ev.find(e => e.ev === "deliver");
    const route = journeyRoute(f);
    if (deliver) {
      steps.push({ cls: "ok", html: `<b>Delivered</b> to ${nameH(info.to)} at ${rel(deliver.t)} after ${deliver.hops} hops.${first && first.p === "PathRequest" ? " The message rode inside the path request." : ""}` });
      if (route.length) steps.push({ html: `<div class="route">The route that won: ${routeHtml(route, f)}</div>` });
    }
    if (deliver && done && done.path && !route.length) steps.push({ html: `<div class="route">The route: ${routeHtml([info.from, ...done.path, info.to], f)}</div>` });
    const resp = ev.filter(e => e.ev === "tx" && e.p === "PathResponse");
    if (resp.length) steps.push({ html: `${nameH(info.to)} answers with a <b>path response</b> along the reverse route (${resp.length} transmissions); it carries the acknowledgement.` });
    if (info.acked_ms !== null) steps.push({ cls: "ok", html: `<b>Acknowledged</b>: back at ${nameH(info.from)} at ${rel(info.acked_ms)}.` });
    // Trouble on the way.
    const trouble = [];
    const retries = ev.filter(e => e.ev === "retry"), repairs = ev.filter(e => e.ev === "repair"), drops = ev.filter(e => e.ev === "drop"), carry = ev.filter(e => e.ev === "carry");
    const nhLost = ev.filter(e => e.ev === "rx" && e.nx >= 0 && e.coll.includes(e.nx));
    if (nhLost.length) trouble.push(`${nhLost.length}× the next hop lost the packet to a collision (${nhLost.slice(0, 3).map(e => `${nameH(e.nx)} ← interfered by ${e.cw[e.coll.indexOf(e.nx)] >= 0 ? nameH(e.cw[e.coll.indexOf(e.nx)]) : "half-duplex"}`).join("; ")})`);
    if (retries.length) trouble.push(`${retries.length} hop retransmission${retries.length > 1 ? "s" : ""} (at ${[...new Set(retries.map(e => e.n))].slice(0, 4).map(nameH).join(", ")})`);
    if (repairs.length) trouble.push(`${repairs.length} local repair${repairs.length > 1 ? "s" : ""}: a dead next hop was skipped`);
    if (drops.length) trouble.push(`${drops.length} drop${drops.length > 1 ? "s" : ""}: ${Object.entries(drops.reduce((m, d) => (m[d.why] = (m[d.why] || 0) + 1, m), {})).map(([w, c]) => `${c}× ${w}`).join(", ")}`);
    if (carry.length) trouble.push(`${carry.length} carry handoff${carry.length > 1 ? "s" : ""}`);
    if (trouble.length) steps.push({ cls: "bad", html: `Trouble on the way: ${trouble.join("; ")}.` });
    // Diagnosis if it is not there.
    const age = lastT - info.sent_ms;
    if (info.delivered_ms === null && age > 15000) diag.push(...diagnose(f, prRx));
    else if (info.delivered_ms === null) steps.push({ cls: "wait", html: `Travelling… (${(age / 1000).toFixed(0)} s)` });
    if (deliver && info.acked_ms === null && age > 30000) diag.push({ html: "Delivered, but the acknowledgement has not come back: the path response was lost on the way back (see trouble above), so the sender will retry." });
    return { steps, diag, prPid };
  }

  // The winning route (sender ... destination) from the first path response
  // (its source path is the request's trail, reversed) or a data packet.
  function journeyRoute(f) {
    const info = f.info;
    const resp = f.events.find(e => e.ev === "tx" && e.p === "PathResponse" && e.n === info.to && e.path);
    if (resp) return [info.from, ...resp.path.slice().reverse(), info.to];
    const data = f.events.find(e => e.ev === "tx" && e.p === "Data" && e.n === info.from && e.path);
    if (data) return [info.from, ...data.path, info.to];
    return [];
  }
  function routeHtml(route, f) {
    const bbPairs = new Set(f.events.filter(e => e.ev === "bb").map(e => e.from + ">" + e.to));
    return route.map((n, k) => {
      const hop = `<span class="hop" data-n="${n}" title="${esc(name(n))}">${k}. ${esc(name(n))}</span>`;
      if (k === 0) return hop;
      const prev = route[k - 1];
      const tcp = bbPairs.has(prev + ">" + n) || bbPairs.has(n + ">" + prev);
      return (tcp ? ` <span class="tcp" title="over the backbone">⇢ TCP ⇢</span> ` : " → ") + hop;
    }).join("");
  }

  function diagnose(f, prRx) {
    const info = f.info, out = [];
    const st = state ? state.state : "";
    const alive = (i) => st[i] === "a" || st[i] === "b";
    if (!alive(info.to)) { out.push({ html: `<b>The destination is down</b> (${({ p: "power cut", e: "battery empty", k: "killed" })[st[info.to]] || "dead"}).` }); return out; }
    if (!alive(info.from)) { out.push({ html: "<b>The sender is down.</b>" }); return out; }
    const comp = state.comp;
    if (comp && comp[info.from] !== comp[info.to]) {
      const island = scenario.nodes.map((n, i) => i).filter(i => comp[i] === comp[info.to]);
      const hosts = island.filter(i => scenario.nodes[i].kind === "host");
      let why = `<b>No way there:</b> ${nameH(info.to)} is in another island (${island.length} nodes) with no radio path and no working backbone link to the sender's island.`;
      if (!hosts.length) why += " That island has no host (gateway) at all.";
      else {
        const ins = hosts.map(h => sim.inspect(h));
        const off = ins.filter(x => !x.internet);
        why += off.length === hosts.length ? ` Its host${hosts.length > 1 ? "s" : ""} (${hosts.slice(0, 3).map(nameH).join(", ")}) ha${hosts.length > 1 ? "ve" : "s"} no internet.` : " Its hosts have internet, but no backbone link reaches the sender's side.";
      }
      why += " Fix: give a host there internet and a TCP link, add a Wi-Fi link, or place repeaters across the gap.";
      out.push({ html: why });
      return out;
    }
    const heard = new Set(); prRx.forEach(e => e.ok.forEach(r => heard.add(r)));
    if (prRx.length && !heard.has(info.to)) {
      let best = -1, bd = 1e9;
      heard.forEach(n => { const d = DK.km(pos(n), pos(info.to)); if (d < bd) { bd = d; best = n; } });
      let html = `<b>The path request never reached the destination.</b> Closest: ${best >= 0 ? `${nameH(best)}, ${bd.toFixed(1)} km away` : "nobody"}.`;
      const maxTrail = Math.max(0, ...f.events.filter(e => e.ev === "tx" && e.p === "PathRequest").map(e => (e.trail || []).length));
      if (maxTrail >= 12) html += " Its trail hit the 12-repeater limit of a source path: repeaters drop a request they cannot add themselves to (anchors can carry it further).";
      html += " Click \"How far did the path request reach?\" to see why each node stopped.";
      out.push({ html });
      return out;
    }
    out.push({ html: "<b>Not delivered yet.</b> The destination is reachable in principle; see the trouble list (collisions at the next hop, drops, retries) and the replay." });
    return out;
  }

  // Draw a journey on the map up to time `upto` (ms).
  function drawJourney(f, upto) {
    hiLayer.clearLayers();
    const info = f.info;
    const txs = new Map();
    let prLines = 0;
    for (const e of f.events) {
      if (e.t > upto) break;
      if (e.ev === "tx") { txs.set(e.id, e); continue; }
      if (e.ev === "rx") {
        const tx = txs.get(e.id); const p = tx ? tx.p : "?";
        const sp = pos(e.n);
        if (p === "PathRequest") {
          for (const r of e.ok) { if (prLines++ > 600) break; L.polyline([sp, pos(r)], { color: PCOLOR.PathRequest, weight: 1, opacity: 0.3, interactive: false }).addTo(hiLayer); }
        } else {
          const style = { Data: { w: 5 }, Carry: { w: 5 }, PathResponse: { w: 3.5 }, Ack: { w: 2.5, d: "3 5" } }[p] || { w: 2 };
          const targets = e.nx >= 0 ? e.ok.filter(r => r === e.nx) : e.ok;
          for (const r of targets) L.polyline([sp, pos(r)], { color: PCOLOR[p] || "#888", weight: style.w, opacity: 0.9, dashArray: style.d, interactive: false }).addTo(hiLayer);
          if (e.nx >= 0 && e.coll.includes(e.nx)) L.circleMarker(pos(e.nx), { radius: 10, color: "#ef4444", weight: 3, fill: false }).bindTooltip("next hop lost it (collision)").addTo(hiLayer);
        }
      }
      if (e.ev === "bb") L.polyline([pos(e.from), pos(e.to)], { color: PCOLOR.Backbone, weight: 3, dashArray: "8 6", interactive: false }).addTo(hiLayer);
      if (e.ev === "drop") L.circleMarker(pos(e.n), { radius: 8, color: "#dc2626", weight: 2, fillColor: "#dc2626", fillOpacity: 0.4 }).bindTooltip("dropped: " + esc(e.why)).addTo(hiLayer);
    }
    const route = journeyRoute(f);
    const delivered = f.events.find(e => e.ev === "deliver");
    if (route.length && delivered && delivered.t <= upto) {
      L.polyline(route.map(pos), { color: "#ffffff", weight: 2, opacity: 0.9, dashArray: "1 6", interactive: false }).addTo(hiLayer);
      route.forEach((n, k) => {
        if (k === 0 || k === route.length - 1) return;
        L.marker(pos(n), { icon: L.divIcon({ className: "", html: `<div style="background:#111827;color:#fff;border:1px solid #fff;border-radius:9px;font:10px system-ui;padding:0 4px">${k}</div>`, iconAnchor: [6, 18] }), interactive: false }).addTo(hiLayer);
      });
    }
    L.circleMarker(pos(info.from), { radius: 11, color: "#60a5fa", weight: 3, fill: false }).bindTooltip("from: " + esc(name(info.from))).addTo(hiLayer);
    L.circleMarker(pos(info.to), { radius: 11, color: "#22c55e", weight: 3, fill: false }).bindTooltip("to: " + esc(name(info.to))).addTo(hiLayer);
    const lab = $("rp-label");
    if (lab) lab.textContent = upto === Infinity ? "" : `+${((upto - info.sent_ms) / 1000).toFixed(2)} s`;
  }

  function showReach(pid, title) {
    const r = sim.reach(pid);
    overlay = new Map();
    const maxH = Math.max(1, ...r.nodes.map(x => x[1]));
    const reasons = {};
    for (const [n, hops, , why] of r.nodes) { overlay.set(n, heat(hops / maxH)); reasons[why] = (reasons[why] || 0) + 1; }
    restyle();
    $("reach").innerHTML = `<h3>${esc(title)}</h3><p class="hint">Heard by <b>${r.heard}</b> nodes, forwarded by ${r.forwarded}, ${r.collisions} receptions lost to collisions${r.region ? `, region id ${r.region}` : ""}. Colour = hops from the sender (green near → red far). <button id="re-clear">Clear colours</button></p>
      <table><tr><th>why it stopped there</th><th>nodes</th></tr>${Object.entries(reasons).sort((a, b) => b[1] - a[1]).map(([w, c]) => `<tr><td>${esc(w)}</td><td class="n">${c}</td></tr>`).join("")}</table>
      <div class="scroll"><table><tr><th>node</th><th>hops</th><th>first heard</th><th>status</th></tr>${r.nodes.sort((a, b) => a[1] - b[1]).slice(0, 400).map(x => `<tr class="click" data-n="${x[0]}"><td>${esc(name(x[0]))}</td><td class="n">${x[1]}</td><td class="n">${fmtT(x[2])}</td><td>${esc(x[3])}</td></tr>`).join("")}</table></div>`;
    $("reach").querySelectorAll("tr[data-n]").forEach(t => t.onclick = () => selectNode(+t.dataset.n, true));
    $("re-clear").onclick = () => { overlay = null; restyle(); $("reach").innerHTML = ""; };
  }
  $("journey").addEventListener("click", (e) => { const h = e.target.closest(".hop[data-n]"); if (h) selectNode(+h.dataset.n, true); });

  // ---------- stats ----------
  // Results overview: delivery time over the run, busiest nodes, and (on
  // demand) why messages were not delivered.
  let lossReport = "";
  function statsOverview(s) {
    const all = sim.msgs(0, 100000).msgs; // [id, from, to, sent, delivered, acked]
    const tEnd = Math.max(60000, lastT);
    const bucket = Math.max(60000, Math.ceil(tEnd / 40 / 60000) * 60000);
    const nb = Math.ceil(tEnd / bucket);
    const med = [], p95 = [];
    for (let b = 0; b < nb; b++) {
      const d = all.filter(m => m[4] !== null && Math.floor(m[3] / bucket) === b).map(m => m[4] - m[3]).sort((x, y) => x - y);
      med.push(d.length ? d[Math.floor(d.length / 2)] / 1000 : null);
      p95.push(d.length ? d[Math.min(d.length - 1, Math.floor(d.length * 0.95))] / 1000 : null);
    }
    const ymax = Math.max(5, ...p95.filter(v => v !== null)) * 1.1;
    const W = 360, H = 130, x = (b) => 34 + (b + 0.5) * (W - 40) / nb, y = (v) => H - 18 - v / ymax * (H - 28);
    const line = (arr, col) => { let d = "", pen = false; arr.forEach((v, b) => { if (v === null) { pen = false; return; } d += (pen ? "L" : "M") + x(b).toFixed(1) + "," + y(v).toFixed(1); pen = true; }); return `<path d="${d}" fill="none" stroke="${col}" stroke-width="2" stroke-linejoin="round"></path>`; };
    const marks = scenario.events.filter(e => e.t_s * 1000 <= tEnd).map(e => { const fix = e.up === true || ["revive", "power_restore", "internet_up", "degrade_clear"].includes(e.type); const xx = 34 + e.t_s * 1000 / tEnd * (W - 40); return `<line x1="${xx}" x2="${xx}" y1="8" y2="${H - 18}" stroke="${fix ? "#22c55e" : "#ef4444"}" stroke-dasharray="3 3"><title>${esc(describe(e))}</title></line>`; }).join("");
    const grid = [0, 0.5, 1].map(f => `<line x1="34" x2="${W - 6}" y1="${y(ymax * f / 1.1)}" y2="${y(ymax * f / 1.1)}" stroke="var(--line)"></line><text x="30" y="${y(ymax * f / 1.1) + 4}" text-anchor="end" font-size="10" fill="var(--mute)">${(ymax * f / 1.1).toFixed(0)} s</text>`).join("");
    const chart = `<svg viewBox="0 0 ${W} ${H}" role="img" aria-label="Delivery time over the run: median and 95th percentile" style="width:100%;height:auto">${grid}${marks}${line(med, "var(--acc)")}${line(p95, "#eb6834")}<text x="34" y="${H - 4}" font-size="10" fill="var(--mute)">0</text><text x="${W - 6}" y="${H - 4}" font-size="10" fill="var(--mute)" text-anchor="end">${fmtTime(tEnd / 1000)}</text></svg>`;
    // Busiest nodes by airtime.
    let busy = "";
    if (state && state.airtime_ms) {
      const top = state.airtime_ms.map((a, i) => [a, i]).sort((p, q) => q[0] - p[0]).slice(0, 6);
      busy = `<table><tr><th>node</th><th class="n">airtime</th><th class="n">queue</th></tr>${top.map(([a, i]) => `<tr class="click" data-n="${i}"><td>${esc(name(i))}</td><td class="n">${(a / 1000).toFixed(1)} s · ${(a / Math.max(1, lastT) * 100).toFixed(1)} %</td><td class="n">${state.queue ? state.queue[i] : ""}</td></tr>`).join("")}</table>`;
    }
    return `<h3>Delivery time over the run</h3>
      <div class="legend-row"><span><i style="background:var(--acc)"></i>median</span><span><i style="background:#eb6834"></i>95th percentile</span><span class="mute">dashed: your events (red damage, green repair)</span></div>
      ${chart}
      <h3>Busiest nodes</h3>${busy}
      <h3>Why messages were not delivered</h3>
      <div class="row"><button id="st-loss">Analyse undelivered messages</button></div><div id="st-loss-out">${lossReport}</div>`;
  }
  function analyseLosses() {
    const all = sim.msgs(0, 100000).msgs.filter(m => m[4] === null && lastT - m[3] > 60000).slice(-200);
    const count = new Map();
    for (const m of all) { const st = followStatus(sim.follow(m[0])); count.set(st.title, (count.get(st.title) || 0) + 1); }
    const rows = [...count.entries()].sort((a, b) => b[1] - a[1]);
    const max = rows.length ? rows[0][1] : 1;
    lossReport = all.length ? `<p class="hint">${all.length} messages older than a minute and not delivered${all.length === 200 ? " (the newest 200)" : ""}, by where they are now:</p><table>${rows.map(([t, n]) => `<tr><td>${esc(t)}</td><td style="width:45%"><div style="height:10px;border-radius:0 4px 4px 0;background:#eb6834;width:${(n / max * 100).toFixed(0)}%"></div></td><td class="n">${n}</td></tr>`).join("")}</table>` : `<p class="hint">Every message older than a minute was delivered.</p>`;
    $("st-loss-out").innerHTML = lossReport;
  }
  document.addEventListener("click", (e) => { if (e.target.id === "st-loss") analyseLosses(); });
  function renderStats() {
    if (!sim) return;
    const s = sim.summary();
    const k = (v, l, t) => `<div title="${esc(t || "")}"><b>${v}</b><span>${l}</span></div>`;
    const prio = ["control", "direct/bulletin", "channel", "bulk", "flood"];
    const routedBy = s.collisions - s.coll_flood - s.coll_next_hop;
    $("stats").innerHTML = `<div class="kpi">
      ${k(s.delivered_pct.toFixed(1) + " %", "delivered")}${k(s.acked_pct.toFixed(1) + " %", "acked")}${k(s.messages, "direct messages")}
      ${k((s.latency_median_ms / 1000).toFixed(1) + " s", "latency median")}${k((s.latency_p90_ms / 1000).toFixed(1) + " s", "latency p90")}${k(s.channel_reach_pct.toFixed(0) + " %", "channel reach")}
      ${k(s.alive + "/" + s.nodes, "nodes alive")}${k(s.tx, "transmissions")}${k(s.airtime_s.toFixed(0) + " s", "total airtime")}
      ${k(s.duty_hits, "duty-cycle hits")}${k(s.duty_drops + s.busy_drops, "dropped (duty/busy)")}${k(s.local_repairs, "local repairs")}
      ${k(s.carry_handoffs + "/" + s.carry_delivered, "carry handoffs/deliv.")}${k(s.backbone_packets, "backbone packets")}${k(s.dht_lookups ? s.dht_lookups + "/" + s.dht_misses : "–", "DHT lookups/misses")}
    </div>
    ${statsOverview(s)}
    <h3>Collisions: what was lost</h3>
    <table>
      <tr><td>copies of floods</td><td class="n">${s.coll_flood}</td><td class="hint">path requests, announces, channel messages: other copies usually get through</td></tr>
      <tr><td>routed, at a bystander</td><td class="n">${routedBy}</td><td class="hint">a node that was not meant to forward it: no harm</td></tr>
      <tr><td><b>routed, at the next hop</b></td><td class="n"><b>${s.coll_next_hop}</b></td><td class="hint">these cost a retransmission</td></tr>
      <tr><td>of which half-duplex</td><td class="n">${s.coll_half_duplex}</td><td class="hint">receiver was transmitting (rare: listen-before-talk)</td></tr>
      <tr><td>all receptions lost</td><td class="n">${s.collisions}</td><td class="hint">one transmission heard by 40 nodes can count 40 times</td></tr>
    </table>
    <h3>Reboots and contacts</h3>
    <table>
      <tr><td>reboots</td><td class="n">${s.reboots}</td><td class="hint">off and on again (also revive and power back)</td></tr>
      <tr><td>shown to the recipient</td><td class="n">${s.shown_pct.toFixed(1)} %</td><td class="hint">delivered and readable: the recipient knows the sender</td></tr>
      <tr><td><b>acked but not shown</b></td><td class="n"><b>${s.false_acks}</b></td><td class="hint">the sender sees "delivered", the recipient saw nothing (yet)</td></tr>
      <tr><td>dropped: unknown sender</td><td class="n">${s.dropped_unknown}</td><td class="hint">without "keep messages from unknown senders"</td></tr>
      <tr><td>kept, waiting for an announce</td><td class="n">${s.held_waiting}</td><td class="hint">shown when the sender announces (${s.shown_after_hold} so far)</td></tr>
      <tr><td>not sent: recipient unknown</td><td class="n">${s.unsent_unknown}</td><td class="hint">the sender lost the recipient's keys in a reboot</td></tr>
    </table>
    <h3>Queue delay per priority</h3>
    <table><tr><th>priority</th><th>sent</th><th>mean</th><th>max</th></tr>${s.queue_delay.map((q, i) => `<tr><td>${i} ${prio[i]}</td><td class="n">${q.count}</td><td class="n">${q.mean_ms.toFixed(0)} ms</td><td class="n">${q.max_ms} ms</td></tr>`).join("")}</table>
    <h3>Transmissions per type</h3><p class="hint">${Object.entries(s.by_type).map(([t, n]) => `<span class="chip" style="background:${PCOLOR[t] || "#666"}">${t} ${n}</span>`).join(" ")}</p>
    <p class="hint">Internet: ${s.internet ? "up" : "<b>down everywhere</b>"}${s.internet_down.length ? " · down in " + s.internet_down.join(", ") : ""}. Busiest node airtime ${s.busiest_airtime_s.toFixed(1)} s. Path requests started ${s.path_requests}, quota drops ${s.quota_dropped}.</p>`;
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
      const rows = [["delivered %", "delivered_pct", 1], ["acked %", "acked_pct", 1], ["latency median s", "latency_median_ms", 0.001], ["latency p90 s", "latency_p90_ms", 0.001], ["transmissions", "tx", 1], ["airtime s", "airtime_s", 1], ["lost at next hop", "coll_next_hop", 1], ["flood copies collided", "coll_flood", 1], ["duty hits", "duty_hits", 1], ["local repairs", "local_repairs", 1], ["carry handoffs", "carry_handoffs", 1], ["channel reach %", "channel_reach_pct", 1]];
      const al = (s) => s.alerts.map(q => q.coverage_pct.toFixed(0) + " %").join(", ") || "–";
      const fmt = (v, m) => (+v * m).toFixed(m === 1 && Number.isInteger(v) ? 0 : 1);
      $("compare").innerHTML = `<table><tr><th></th><th>${f} ${a.features[f] ? "on" : "off"}</th><th>${f} ${b.features[f] ? "on" : "off"}</th></tr>${rows.map(([l, key, m]) => `<tr><td>${l}</td><td class="n">${fmt(x[key], m)}</td><td class="n">${fmt(y[key], m)}</td></tr>`).join("")}<tr><td>alert coverage</td><td>${al(x)}</td><td>${al(y)}</td></tr></table><p class="hint">Same seed, same scenario, same traffic draws until the routers behave differently.</p>`;
    };
    setTimeout(chunk, 0);
  };

  // ---------- inspector ----------
  function renderInspector() {
    const box = $("inspector");
    if (selected < 0 || !scenario.nodes[selected]) { box.classList.add("hidden"); return; }
    if (box.contains(document.activeElement) && document.activeElement.tagName !== "BUTTON") return; // do not redraw while typing
    box.classList.remove("hidden");
    const n = scenario.nodes[selected];
    const ins = sim ? sim.inspect(selected) : null;
    const opt = (v, list) => list.map(x => `<option ${x == v ? "selected" : ""}>${x}</option>`).join("");
    let h = `<h3>${esc(name(selected))}</h3>
      <div class="row"><button id="in-from">Send from here</button><button id="in-to">Send to here</button><button id="in-announce">Announce: show reach</button></div>
      <label>Kind <select id="in-kind">${opt(n.kind, Object.keys(KIND_COLOR))}</select> Label <input id="in-label" value="${esc(n.label || "")}" size="12"></label>
      <label>SF <select id="in-sf">${opt(n.sf || "default", ["default", 7, 8, 9, 10, 11, 12])}</select> Antenna m <input id="in-h" type="number" step="0.5" style="width:60px" value="${n.h || ""}" placeholder="auto"></label>
      <label>Battery Wh <input id="in-batt" type="number" style="width:70px" value="${n.battery_wh ?? ""}" placeholder="auto"> Carry copies <input id="in-carry" type="number" style="width:50px" min="0" max="8" value="${n.carry || 0}"></label>
      <label>Wander radius m <input id="in-wander" type="number" style="width:70px" value="${n.wander_m || ""}" placeholder="0"> speed km/h <input id="in-speed" type="number" style="width:55px" value="${n.speed_kmh || ""}"></label>
      <label><input type="checkbox" id="in-net" ${n.internet ? "checked" : ""}> Internet</label>
      <label><input type="checkbox" id="in-radio" ${n.radio ?? !["server", "app"].includes(n.kind) ? "checked" : ""}> LoRa radio</label>
      <label>Moves <select id="in-mobile"><option value="">auto (route = mobile)</option><option value="1" ${n.mobile === true ? "selected" : ""}>mobile</option><option value="0" ${n.mobile === false ? "selected" : ""}>stationary</option></select></label>
      ${n.route && n.route.length >= 2 ? `<label>Route ${n.route.length} points · starts at <input id="in-start" type="number" min="0" style="width:60px" value="${((n.start_s || 0) / 60).toFixed(1)}"> min · end <select id="in-end">${["back", "stay", "repeat"].map(x => `<option ${x === (n.route_end || "back") ? "selected" : ""}>${x}</option>`).join("")}</select></label>
      <label>Pause per waypoint (min, comma) <input id="in-pauses" size="16" value="${(n.pauses_s || []).map(x => +(x / 60).toFixed(1)).join(",")}"></label>` : ""}
      <label>Region <input id="in-region" value="${esc(n.region || "")}" size="12"></label>
      <button id="in-apply">Apply (restarts)</button>`;
    if (ins) {
      h += `<p class="hint">Role ${ins.role} · ${ins.state}${ins.on_battery ? " · on battery " + ins.battery_wh.toFixed(1) + "/" + ins.battery_max_wh + " Wh" : ""} · energy ${ins.energy} · internet ${ins.internet ? "yes" : "no"} · slot ${ins.slot_ms} ms${ins.gateway >= 0 ? " · DHT gateway " + ins.gateway : ""}<br>
        Airtime ${ins.airtime_s.toFixed(2)} s · duty budget used ${ins.duty_budget_used_pct.toFixed(1)} % · duty hits ${ins.duty_hits} · carried ${ins.carried}/${ins.carry_copies} copies · ${ins.radio_neighbors.length} radio neighbours</p>
        <h3>Router stats</h3><p class="hint">${Object.entries(ins.stats).filter(([, v]) => v).map(([k2, v]) => `${k2} <b>${v}</b>`).join(" · ") || "nothing yet"}</p>
        <h3>Neighbours (router table)</h3><div class="scroll"><table><tr><th>node</th><th>SNR</th><th>last</th><th>role</th></tr>${ins.neighbors.slice(0, 60).map(x => `<tr class="click" data-n="${x.node}"><td>${esc(name(x.node))}</td><td class="n">${x.snr}</td><td class="n">${x.last_s.toFixed(0)} s</td><td>${x.role || ""}</td></tr>`).join("")}</table></div>
        <h3>Routes</h3><div class="scroll"><table><tr><th>dest</th><th>path</th><th>learned</th></tr>${ins.routes.map(r => `<tr><td>${esc(name(r.dest))}</td><td>${r.path.join(" → ") || "direct"}</td><td class="n">${r.learned_s.toFixed(0)} s</td></tr>`).join("") || "<tr><td colspan=3>none</td></tr>"}</table></div>
        ${ins.dead_hops && ins.dead_hops.length ? `<h3>Dead next hops (local repair)</h3><p class="hint">${ins.dead_hops.map(d => `${esc(name(d.node))} for ${d.for_s.toFixed(0)} s`).join(" · ")}</p>` : ""}
        <h3>Send queue</h3>${ins.queue.length ? `<table><tr><th>prio</th><th>waited</th><th>bytes</th><th>tries</th></tr>${ins.queue.map(q => `<tr><td>${q.prio}</td><td class="n">${q.waited_ms} ms</td><td class="n">${q.len}</td><td class="n">${q.tries}</td></tr>`).join("")}</table>` : `<p class="hint">empty</p>`}
        ${ins.links.length ? `<h3>Backbone</h3><p class="hint">${ins.links.map(l => `${l.kind} → ${esc(name(l.peer))} ${l.ok ? "up" : "down"}`).join(" · ")}</p>` : ""}`;
    }
    box.innerHTML = h;
    box.querySelectorAll("tr[data-n]").forEach(r => r.onclick = () => selectNode(+r.dataset.n, true));
    $("in-apply").onclick = () => {
      n.kind = $("in-kind").value; n.label = $("in-label").value;
      const sf = $("in-sf").value; n.sf = sf === "default" ? 0 : +sf;
      n.h = +$("in-h").value || 0;
      n.battery_wh = $("in-batt").value === "" ? -1 : +$("in-batt").value;
      n.carry = +$("in-carry").value || 0; n.internet = $("in-net").checked; n.region = $("in-region").value;
      n.wander_m = +$("in-wander").value || 0; n.speed_kmh = +$("in-speed").value || 0;
      n.radio = $("in-radio").checked;
      n.mobile = $("in-mobile").value === "" ? undefined : $("in-mobile").value === "1";
      if ($("in-start")) { n.start_s = (+$("in-start").value || 0) * 60; n.route_end = $("in-end").value; n.pauses_s = $("in-pauses").value.split(",").filter(x => x.trim() !== "").map(x => (+x || 0) * 60); }
      edited(); renderInspector();
    };
    $("in-from").onclick = () => { $("jr-from").value = "#" + selected; openTab("journey"); };
    $("in-to").onclick = () => { $("jr-to").value = "#" + selected; openTab("journey"); };
    $("in-announce").onclick = () => {
      const r = sim.announce(selected);
      openTab("journey");
      if (!playing) sim.step(20000);
      setTimeout(() => showReach(r.pid, `Announce from ${name(selected)}`), 50);
    };
  }

  // ---------- log ----------
  ["tx", "rx", "send", "deliver", "ack", "cancel", "drop", "defer", "route", "bb", "dht", "repair", "carry", "retry", "alert", "alert_rx", "announce", "event", "dead", "revive"].forEach(e => $("lg-ev").insertAdjacentHTML("beforeend", `<option>${e}</option>`));
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
      if (r.next === since || (r.lines.length === 0 && r.next >= sim.summary().trace_end)) break;
      since = r.next;
      if (out.length > max) break;
    }
    return out;
  }
  $("btn-log").onclick = () => { const l = fetchLog(100000); $("log").textContent = l.slice(-400).map(e => JSON.stringify(e)).join("\n") || "(no events)"; };
  let downloads = null;
  if (window.claude && window.claude.use) window.claude.use("downloads").then(d => { downloads = d; }).catch(() => {});
  function download(fname, text, type) {
    if (downloads) {
      downloads.save({ filename: fname, data: new Blob([text], { type }) }).catch(() => showNote("The file was not saved."));
      return;
    }
    const a = document.createElement("a"); a.href = URL.createObjectURL(new Blob([text], { type })); a.download = fname; a.click();
  }
  $("btn-log-json").onclick = () => download("maarse-denmark-events.json", JSON.stringify(fetchLog(500000)), "application/json");
  $("btn-log-csv").onclick = () => {
    const l = fetchLog(500000);
    const cols = ["t", "ev", "n", "p", "pid", "msg", "alert", "hops", "nx", "dst", "prio", "why", "from", "to", "path", "trail", "ok", "snr", "coll", "cw"];
    download("maarse-denmark-events.csv", cols.join(",") + "\n" + l.map(e => cols.map(c => e[c] === undefined ? "" : Array.isArray(e[c]) ? `"${e[c].join(" ")}"` : JSON.stringify(e[c])).join(",")).join("\n"), "text/csv");
  };

  // ---------- build tab ----------
  for (const id of ["g-rep", "g-net"]) { const i = $(id); const show = () => i.nextElementSibling.textContent = i.value; i.oninput = show; show(); }
  // Generator: users and connection mix per region (defaults from the mockup).
  const GEN_REGIONS = [["hovedstaden", "Hovedstaden", 160, 20, 45, 35, 6, 4], ["sjaelland", "Sjælland", 80, 35, 45, 20, 8, 6], ["syddanmark", "Syddanmark", 110, 35, 45, 20, 8, 6], ["midtjylland", "Midtjylland", 110, 30, 45, 25, 8, 5], ["nordjylland", "Nordjylland", 70, 45, 40, 15, 10, 8]];
  const genCols = ["users", "lora", "both", "app", "cars", "relays"];
  $("g-regions").insertAdjacentHTML("beforeend", GEN_REGIONS.map(([id, label, ...v]) => `<tr data-r="${id}"><td>${label}</td>${v.map((x, k) => `<td><input type="number" min="0" ${k ? 'max="100"' : ""} data-c="${genCols[k]}" value="${x}" style="width:${k ? 46 : 58}px" aria-label="${label} ${genCols[k]}"></td>`).join("")}</tr>`).join(""));
  function genRegions() {
    const out = {};
    $("g-regions").querySelectorAll("tr[data-r]").forEach(tr => { const m = {}; tr.querySelectorAll("input").forEach(i => m[i.dataset.c] = +i.value || 0); out[tr.dataset.r] = m; });
    return out;
  }
  function genTotal() { const t = Object.values(genRegions()).reduce((a, m) => a + m.users, 0); $("g-total").textContent = `${t} users in all${t > 1800 ? " — near the 2,000 node limit with infrastructure" : ""}.`; }
  $("g-regions").addEventListener("input", genTotal); genTotal();
  $("btn-generate").onclick = () => {
    scenario = DK.generateDenmark({ regions: genRegions(), repeaterKm: +$("g-rep").value, internetPct: +$("g-net").value, seed: +$("g-seed").value }, window.DK_LAND, window.DK_TOWNS);
    syncForm(); edited(); fitScenario();
    // Before you run: what was made, and users cut off from everyone else.
    setTimeout(() => {
      if (!sim) return;
      const c = {}; scenario.nodes.forEach(n => c[n.kind] = (c[n.kind] || 0) + 1);
      const comp = sim.call({ cmd: "components" });
      const size = new Map(); comp.forEach(x => size.set(x, (size.get(x) || 0) + 1));
      const main = [...size.entries()].sort((a, b) => b[1] - a[1])[0];
      const cut = scenario.nodes.filter((n, i) => ["handheld", "app", "relay", "mobile"].includes(n.kind) && comp[i] !== (main ? main[0] : -2)).length;
      $("g-total").textContent = `${scenario.nodes.length} nodes (limit 2,000): ${Object.entries(c).map(([k, v]) => `${v} ${k}`).join(", ")}. ${cut ? `${cut} users have no path to the main network right now (islands without radio or internet).` : "Every user can reach the main network."}`;
    }, 300);
  };
  $("btn-clear").onclick = () => { scenario = DK.base("Empty map", 1); syncForm(); edited(); };
  $("features").innerHTML = FEATURES.map(([k2, l]) => `<label><input type="checkbox" data-f="${k2}"> ${l}</label>`).join("");
  $("features").addEventListener("change", (e) => { const k2 = e.target.dataset.f; if (!k2) return; scenario.features[k2] = e.target.checked; edited(); });
  $("sc-sf").onchange = () => { scenario.sf = +$("sc-sf").value; edited(); };
  $("sc-duty").onchange = () => { scenario.duty_permille = +$("sc-duty").value; edited(); };
  $("sc-ann").onchange = () => { scenario.announce_s = Math.max(0, +$("sc-ann").value) * 3600; edited(); };
  $("sc-rh").onchange = () => { scenario.relay_hops = Math.max(1, Math.min(12, +$("sc-rh").value || 4)); edited(); };
  $("sc-ar").onchange = () => { scenario.auto_relay_after_s = Math.max(0, +$("sc-ar").value) * 60; edited(); };
  const trMap = { "tr-direct": "direct_per_min", "tr-channel": "channel_per_min", "tr-local": "local_share", "tr-day": "day_start_h" };
  for (const [id, key] of Object.entries(trMap)) $(id).onchange = () => { scenario.traffic[key] = +$(id).value; edited(); };
  ["opt-islands", "opt-links", "opt-backbone"].forEach(id => $(id).onchange = () => { drawLinks(); restyle(); });
  syncPerf();
  $("rd-go").onclick = () => {
    const ev = { type: "radio", sf: +$("rd-sf").value || 0, freq_mhz: +$("rd-f").value || 0, bw_hz: +$("rd-bw").value || 0, tx_dbm: $("rd-p").value === "" ? 0 : +$("rd-p").value };
    if (!ev.sf && !ev.freq_mhz && !ev.bw_hz && !ev.tx_dbm) return showNote("Pick at least one radio setting to change.");
    addEvent(ev);
    drawLinks(); restyle();
    showNote(`Every node switched: ${[ev.sf && "SF" + ev.sf, ev.freq_mhz && ev.freq_mhz + " MHz", ev.bw_hz && ev.bw_hz / 1000 + " kHz", ev.tx_dbm && ev.tx_dbm + " dBm"].filter(Boolean).join(", ")}.`);
  };
  $("pf-events").onchange = () => { perf.maxEvents = Math.max(1000, +$("pf-events").value || 400000); savePerf(); };
  $("pf-speed").onchange = () => { perf.maxSpeed = Math.max(1, +$("pf-speed").value || 1000); savePerf(); };
  $("pf-nodes").onchange = () => { perf.maxNodes = Math.max(10, +$("pf-nodes").value || 2000); savePerf(); };
  $("pf-bench").onclick = () => { $("pf-out").textContent = "Measuring…"; setTimeout(benchmark, 30); };
  function syncForm() {
    document.querySelectorAll("#features input").forEach(i => i.checked = !!scenario.features[i.dataset.f]);
    $("sc-sf").value = scenario.sf; $("sc-duty").value = scenario.duty_permille;
    $("sc-ann").value = +((scenario.announce_s || 0) / 3600).toFixed(2);
    $("sc-rh").value = scenario.relay_hops || 4;
    $("sc-ar").value = +((scenario.auto_relay_after_s ?? 600) / 60).toFixed(1);
    for (const [id, key] of Object.entries(trMap)) $(id).value = scenario.traffic[key] ?? (key === "day_start_h" ? -1 : "");
    fillForestForm(scenario.forest);
  }
  $("btn-export").onclick = () => download((scenario.name || "scenario").replace(/\W+/g, "-").toLowerCase() + ".json", JSON.stringify(scenario, null, 1), "application/json");
  $("file-import").onchange = async (e) => {
    const f = e.target.files[0]; if (!f) return;
    try { scenario = Object.assign(DK.base("Imported", 1), JSON.parse(await f.text())); syncForm(); edited(); fitScenario(); }
    catch (err) { showNote("Could not read the scenario: " + err.message); }
  };

  // ---------- tabs, controls ----------
  // Four modes; each shows one or more of the old tab sections, stacked.
  const MODES = { build: ["build", "terrain"], send: ["journey", "alerts"], break: ["disaster"], results: ["stats", "log"], phones: ["phones"] };
  let mode = "send";
  function openMode(m) {
    mode = m;
    document.querySelectorAll("#modes button").forEach(x => x.classList.toggle("on", x.dataset.mode === m));
    document.querySelectorAll(".tab").forEach(x => x.classList.toggle("on", MODES[m].includes(x.id.slice(4))));
    if (m === "send") renderMsgList();
    if (m === "results") renderStats();
  }
  // Opens the mode that holds an old tab section and scrolls to it.
  function openTab(t) {
    const m = Object.keys(MODES).find(k => MODES[k].includes(t)) || "send";
    if (m !== mode) openMode(m);
    const sec = $("tab-" + t);
    if (sec && MODES[m][0] !== t) sec.scrollIntoView({ block: "start" });
  }
  $("modes").addEventListener("click", (e) => { const b = e.target.closest("button"); if (b) openMode(b.dataset.mode); });
  openMode("send");
  function togglePlay() { playing = !playing; $("btn-play").textContent = playing ? "❚❚" : "▶"; }
  $("btn-play").onclick = togglePlay;
  document.addEventListener("keydown", (e) => { if (e.code === "Space" && !/INPUT|SELECT|TEXTAREA|BUTTON/.test(document.activeElement.tagName)) { e.preventDefault(); togglePlay(); } });
  $("speeds").addEventListener("click", (e) => { const b = e.target.closest("button"); if (!b) return; speed = +b.dataset.s; document.querySelectorAll("#speeds button").forEach(x => x.classList.toggle("on", x === b)); });
  $("btn-step").onclick = () => { if (!sim) return; const r = sim.step(1000); lastT = r.t; injectDue(); pullEvents(); state = sim.state(); restyle(); $("clock").textContent = fmtTime(lastT / 1000); renderFeed(true); drawHist(); renderHud(); };
  $("btn-reset").onclick = () => rebuild();
  const pre = $("preset");
  pre.innerHTML = `<option value="">— preset —</option>` + Object.entries(DK.PRESETS).map(([k2, p]) => `<option value="${k2}">${p.title}</option>`).join("");
  pre.onchange = () => loadPreset(pre.value);
  // Fit points into the part of the map the overlays leave free.
  function fitAround(points) {
    const feedH = $("feed").offsetHeight || 0;
    map.fitBounds(L.latLngBounds(points), { paddingTopLeft: [30, 70], paddingBottomRight: [30, feedH + 70] });
  }
  function fitScenario() { const b = L.latLngBounds(scenario.nodes.map(n => [n.lat, n.lon])); if (b.isValid()) map.fitBounds(b.pad(0.1)); }
  function loadPreset(k2) {
    const p = DK.PRESETS[k2]; if (!p) return;
    scenario = p.build(window.DK_LAND, window.DK_TOWNS, +$("g-seed").value || 1);
    syncForm(); edited();
    $("sc-info").textContent = p.text;
    location.hash = k2;
    fitScenario();
  }

  // ---------- page API (console and headless tests) ----------
  const plain = (html) => { const d = document.createElement("div"); d.innerHTML = html; return d.textContent.replace(/\s+/g, " ").trim(); };
  window.DkSim = {
    DK,
    engine: () => Engine,
    sim: () => sim,
    scenario: () => scenario,
    presetKey: () => $("preset").value,
    name,
    describe,
    resolveEnd,
    openTab,
    summary: () => sim ? sim.summary() : null,
    // Load a scenario into the live view (and fit the map to it).
    load(sc, presetKey) {
      scenario = sc;
      syncForm(); edited(); fitScenario();
      if (presetKey !== undefined) $("preset").value = presetKey;
    },
    play(on, s) {
      if (s) { speed = s; document.querySelectorAll("#speeds button").forEach(x => x.classList.toggle("on", +x.dataset.s === s)); }
      if (on !== playing) togglePlay();
    },
    // Advance the live simulation by `ms` at once (for "follow a message").
    advance(ms) {
      if (!sim) return;
      const r = sim.step(ms, 5e7); lastT = r.t; injectDue(); pullEvents(); state = sim.state(); restyle(); renderHud(); renderFeed(true); drawHist();
    },
    addEvent,
    follow(m) { followMessage(m); },
    track(i) { toggleTrack(i); },
    showMessageOnMap(m, from, to) { followMessage(m); fitAround([pos(from), pos(to)]); },
    // A message's story as plain text (the Journey tab's steps and diagnosis).
    story(m) {
      const f = sim.follow(m);
      const st = narrate(f);
      return { info: f.info, steps: st.steps.map(x => plain(x.html)), diagnosis: st.diag.map(x => plain(x.html)), route: journeyRoute(f).map(name) };
    },
  };

  // ---------- start ----------
  renderLegend();
  MaarseSim.load("maarse_sim_wasm.wasm?v=" + (document.querySelector("script[src^=\"app.js\"]").src.split("v=")[1] || "0")).then((E) => {
    Engine = E;
    const k2 = location.hash.slice(1);
    if (DK.PRESETS[k2]) { pre.value = k2; loadPreset(k2); }
    else { pre.value = "normal"; loadPreset("normal"); }
    if (window.DkStore) { DkStore.init(); if (k2.startsWith("run=")) setTimeout(() => DkStore.openFromHash(k2), 50); }
    requestAnimationFrame(frame);
  }).catch((e) => {
    document.body.insertAdjacentHTML("afterbegin", `<p style="padding:8px;background:#fee2e2;color:#7f1d1d">Could not load the WebAssembly engine (${esc(e.message)}). Build it with <code>python tools/build_denmark_sim.py</code> and serve <code>docs/</code> over HTTP.</p>`);
  });
})();
