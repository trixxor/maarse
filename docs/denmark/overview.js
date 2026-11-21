// Network overview: the real network at a glance, in the style of a mesh
// telemetry site. It reads the public feeds of backbone servers
// (GET /api/public/map, /traffic and /network) and shows
// * key figures along the top (nodes, packets per minute, transports, hosts),
// * a searchable node list on the left, with a detail card,
// * a live packet feed, the open channel messages and a type breakdown on the right,
// * optional sound alerts for public messages, direct messages and new nodes.
// The servers are the ones set in the Real world card (kept in this browser
// only). A server shares these feeds with "Share the public map" and
// "Share live traffic" under Settings.
(function () {
  const dk = window.DKSim;
  if (!dk || !dk.map) return;
  const map = dk.map;
  const $ = (id) => document.getElementById(id);
  const esc = (t) => String(t ?? "").replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
  const STORE = "maarse.realworld.servers", BIB_STORE = "maarse.overview.sound";
  const DEFAULT_COLLECTOR = "https://nj01.dk.eu.maarse.dk";
  const ROLE_COLOR = { Repeater: "#60a5fa", Anchor: "#a78bfa", Relay: "#34d399", Client: "#fbbf24", Host: "#f87171" };
  const KIND_COLOR = { Announce: "#eab308", Data: "#60a5fa", Ack: "#22c55e", Channel: "#f472b6", PathRequest: "#fb923c", PathResponse: "#a3e635", Fragment: "#a78bfa", Probe: "#94a3b8", Hello: "#38bdf8" };
  const TRANSPORTS = ["radio", "trunk", "backbone"];
  const hosts = () => {
    try { const v = JSON.parse(localStorage.getItem(STORE) || "[]"); if (Array.isArray(v) && v.length) return v; } catch { /* storage blocked */ }
    return [DEFAULT_COLLECTOR];
  };

  const S = {
    open: false, tab: "feed", filter: "", roles: new Set(), sel: null,
    nodes: new Map(), status: new Map(), packets: [], messages: [], rate: [], types: {}, observers: new Set(),
    seenGroup: new Set(), seenMessage: new Set(), seenSecond: new Set(), known: new Set(), ready: false, timers: [],
  };

  // ---------- sound ----------
  const TONES = {
    public: { label: "Public channel message", def: true, tone: [523, 659, 784] },
    channel: { label: "Other open channel", def: false, tone: [440, 554] },
    message: { label: "Direct message passing", def: true, tone: [660, 880] },
    newnode: { label: "New node", def: true, tone: [988] },
  };
  let bib = { on: false, vol: 0.5, types: {} };
  try { Object.assign(bib, JSON.parse(localStorage.getItem(BIB_STORE) || "{}")); } catch { /* storage blocked */ }
  bib.types ||= {};
  for (const [k, v] of Object.entries(TONES)) if (bib.types[k] === undefined) bib.types[k] = v.def;
  const saveBib = () => { try { localStorage.setItem(BIB_STORE, JSON.stringify(bib)); } catch { /* storage blocked */ } };
  let actx = null, lastBeep = {}, recent = [];
  function beep(type, force = false) {
    if (!force && (!bib.on || !bib.types[type])) return;
    const now = Date.now();
    if (!force) {
      if (now - (lastBeep[type] || 0) < 600) return;
      recent = recent.filter((t) => now - t < 5000);
      if (recent.length >= 12) return;
      recent.push(now); lastBeep[type] = now;
    }
    try {
      actx ||= new (window.AudioContext || window.webkitAudioContext)();
      if (actx.state === "suspended") actx.resume();
      let t0 = actx.currentTime + 0.01;
      for (const f of TONES[type].tone) {
        const o = actx.createOscillator(), g = actx.createGain();
        o.type = "sine"; o.frequency.value = f;
        g.gain.setValueAtTime(0.0001, t0);
        g.gain.exponentialRampToValueAtTime(Math.max(0.0002, bib.vol * 0.4), t0 + 0.015);
        g.gain.exponentialRampToValueAtTime(0.0001, t0 + 0.14);
        o.connect(g).connect(actx.destination); o.start(t0); o.stop(t0 + 0.16); t0 += 0.12;
      }
    } catch { /* no audio */ }
  }

  // ---------- markup ----------
  const style = document.createElement("style");
  style.textContent = `
    #ov-mode { position:absolute; top:8px; right:56px; z-index:1000; display:flex; border:1px solid var(--ovline); border-radius:10px; overflow:hidden; background:var(--ov); box-shadow:0 4px 16px rgba(0,0,0,.18); }
    #ov-mode button { border:0; border-radius:0; background:transparent; padding:5px 12px; cursor:pointer; }
    #ov-mode button[aria-pressed="true"] { background:var(--acc); color:#fff; }
    .ovx { position:absolute; z-index:900; background:var(--ov); border:1px solid var(--ovline); border-radius:12px; color:var(--ink); font-size:12px; box-shadow:0 8px 28px rgba(0,0,0,.22); }
    .ovx.hidden { display:none; }
    #mapwrap.real-only #hud { display:none; }
    #ov-top { left:54px; right:12px; top:44px; padding:8px 10px; display:flex; gap:8px; flex-wrap:wrap; align-items:stretch; }
    #ov-top .t { min-width:84px; padding:2px 10px 2px 2px; border-right:1px solid var(--ovline); }
    #ov-top .t b { display:block; font-size:19px; line-height:1.15; font-variant-numeric:tabular-nums; }
    #ov-top .t span { color:var(--mute); font-size:11px; }
    #ov-top .sp { flex:1; }
    #ov-top .acts { display:flex; gap:6px; align-items:center; position:relative; }
    #ov-nodes { left:54px; top:118px; bottom:70px; width:290px; display:flex; flex-direction:column; }
    #ov-feed { right:12px; top:118px; bottom:70px; width:380px; display:flex; flex-direction:column; }
    .ovx h4 { margin:0; padding:8px 10px 4px; font-size:12px; text-transform:uppercase; letter-spacing:.04em; color:var(--mute); }
    .ovx .bar { padding:4px 10px 6px; display:flex; gap:4px; flex-wrap:wrap; }
    .ovx .bar input { flex:1; min-width:120px; }
    .ovx .chip { padding:2px 8px; border-radius:99px; font-size:11px; cursor:pointer; }
    .ovx .chip[aria-pressed="true"] { background:var(--acc); color:#fff; border-color:var(--acc); }
    .ovx .list { flex:1; overflow:auto; border-top:1px solid var(--ovline); }
    .ovx .item { display:flex; gap:8px; align-items:center; padding:5px 10px; border-bottom:1px solid var(--ovline); cursor:pointer; }
    .ovx .item:hover, .ovx .item.on { background:color-mix(in srgb, var(--acc) 14%, transparent); }
    .ovx .dot { width:9px; height:9px; border-radius:50%; flex:none; }
    .ovx .grow { flex:1; min-width:0; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
    .ovx .mute { color:var(--mute); }
    .ovx time { color:var(--mute); font-variant-numeric:tabular-nums; flex:none; }
    .ovx .k { display:inline-block; padding:0 6px; border-radius:6px; color:#0b0f14; font-weight:600; font-size:11px; flex:none; }
    .ovx .msg { padding:6px 10px; border-bottom:1px solid var(--ovline); }
    .ovx .msg .who { font-weight:600; }
    .ovx .card { padding:8px 10px; border-bottom:1px solid var(--ovline); }
    .ovx .facts { display:grid; grid-template-columns:1fr 1fr; gap:4px 12px; margin:6px 0; }
    .ovx .facts span { display:block; color:var(--mute); font-size:11px; }
    .ovx .barline { display:flex; align-items:center; gap:6px; padding:3px 10px; }
    .ovx .barline i { display:block; height:8px; border-radius:4px; }
    .ovx .spark { display:flex; align-items:flex-end; gap:1px; height:46px; padding:4px 10px; }
    .ovx .spark i { flex:1; background:var(--acc); border-radius:1px 1px 0 0; min-height:1px; }
    #ov-sound { position:absolute; right:0; top:34px; width:250px; padding:8px 10px; z-index:1100; }
    #ov-sound label { display:flex; gap:8px; align-items:center; margin:3px 0; }
    #ov-sound.hidden { display:none; }
    @media (max-width: 1100px) { #ov-feed { width:300px; } #ov-nodes { width:230px; } }
  `;
  document.head.appendChild(style);

  const wrap = $("mapwrap");
  const modeBar = document.createElement("div");
  modeBar.id = "ov-mode"; modeBar.setAttribute("role", "group"); modeBar.setAttribute("aria-label", "View");
  modeBar.innerHTML = [["sim", "Simulation", "The simulated network only"], ["real", "Real network", "The real network from the public servers; the simulation is hidden"], ["both", "Both", "Simulation and real network together"]]
    .map(([k, l, t]) => `<button data-mode="${k}" title="${t}" aria-pressed="${k === "sim"}">${l}</button>`).join("");
  wrap.appendChild(modeBar);
  L.DomEvent.disableClickPropagation(modeBar);

  const top = document.createElement("section");
  top.id = "ov-top"; top.className = "ovx hidden"; top.setAttribute("aria-label", "Network figures");
  const nodesPanel = document.createElement("section");
  nodesPanel.id = "ov-nodes"; nodesPanel.className = "ovx hidden"; nodesPanel.setAttribute("aria-label", "Nodes");
  nodesPanel.innerHTML = `<h4>Nodes</h4>
    <div class="bar"><input id="ov-search" type="search" placeholder="Search name, address, role"></div>
    <div class="bar" id="ov-roles">${Object.keys(ROLE_COLOR).map((r) => `<button class="chip" data-role="${r}" aria-pressed="false">${r}</button>`).join("")}</div>
    <div class="list" id="ov-list"></div>`;
  const feedPanel = document.createElement("section");
  feedPanel.id = "ov-feed"; feedPanel.className = "ovx hidden"; feedPanel.setAttribute("aria-label", "Packets and messages");
  feedPanel.innerHTML = `<div id="ov-card"></div>
    <div class="bar" id="ov-tabs" style="padding-top:8px">
      <button class="chip" data-tab="feed" aria-pressed="true">Live packets</button>
      <button class="chip" data-tab="channels" aria-pressed="false">Channels</button>
      <button class="chip" data-tab="types" aria-pressed="false">Types</button>
      <span class="grow"></span><button class="chip" id="ov-close" title="Close the overview">Close</button></div>
    <div class="list" id="ov-body"></div>`;
  [top, nodesPanel, feedPanel].forEach((el) => { wrap.appendChild(el); L.DomEvent.disableClickPropagation(el); L.DomEvent.disableScrollPropagation(el); });

  // ---------- helpers ----------
  const ago = (unix) => {
    if (!unix) return "";
    const s = Math.max(0, Date.now() / 1000 - unix);
    return s < 90 ? Math.round(s) + " s" : s < 5400 ? Math.round(s / 60) + " min" : s < 172800 ? Math.round(s / 3600) + " h" : Math.round(s / 86400) + " d";
  };
  const clock = (ms) => new Date(ms).toLocaleTimeString([], { hour12: false });
  const nodeName = (addr) => { const n = S.nodes.get(addr); return n && n.name ? n.name : addr ? addr.slice(0, 8) : ""; };
  const byPrefix = (prefix) => { if (!prefix) return null; for (const n of S.nodes.values()) if (n.addr.startsWith(prefix)) return n; return null; };
  const roleOf = (n) => n.host ? "Host" : n.role || "Client";
  async function getJson(url, path, ms = 4500) {
    const r = await fetch(url + path, { cache: "no-store", credentials: "omit", signal: AbortSignal.timeout(ms) });
    if (r.status === 404) throw new Error("not shared (turn on \"Share the public map\" and \"Share live traffic\" in its Settings)");
    if (!r.ok) throw new Error("answered " + r.status);
    return r.json();
  }
  const setStatus = (url, patch) => S.status.set(url, { ...(S.status.get(url) || {}), ...patch });

  // ---------- polling ----------
  async function loadMap() {
    await Promise.all(hosts().map(async (url) => {
      try {
        const j = await getJson(url, "/api/public/map");
        if (j.host?.addr) S.nodes.set(j.host.addr, { ...j.host, role: "Anchor", host: true, last_seen: Math.floor(Date.now() / 1000), name: j.host.name || new URL(url).hostname });
        for (const n of j.nodes || []) S.nodes.set(n.addr, { ...(S.nodes.get(n.addr) || {}), ...n });
        setStatus(url, { map: true, error: null });
      } catch (e) { setStatus(url, { map: false, error: String(e.message || e) }); }
    }));
    for (const n of S.nodes.values()) S.known.add(n.addr);
    drawReal(); applyLayers();
  }
  async function loadNetwork() {
    await Promise.all(hosts().map(async (url) => {
      try {
        const j = await getJson(url, "/api/public/network");
        setStatus(url, { net: true });
        for (const g of j.packets || []) {
          for (const o of g.observations || []) { if (o.observer) S.observers.add(o.observer); }
          if (S.seenGroup.has(g.id)) continue;
          S.seenGroup.add(g.id);
          S.packets.unshift(g);
          S.types[g.kind] = (S.types[g.kind] || 0) + 1;
          const subject = (g.observations || []).find((o) => o.subject)?.subject;
          if (g.kind === "Announce" && subject) {
            const fresh = !S.known.has(subject); S.known.add(subject);
            if (S.ready && fresh) beep("newnode");
          } else if (S.ready && g.kind === "Data") beep("message");
        }
        S.packets.length = Math.min(S.packets.length, 300);
        for (const m of j.messages || []) {
          const key = m.time + "|" + m.channel + "|" + m.sender + "|" + m.text;
          if (S.seenMessage.has(key)) continue;
          S.seenMessage.add(key);
          S.messages.unshift(m);
          if (S.ready) beep(m.channel === "public" ? "public" : "channel");
        }
        S.messages.sort((a, b) => b.time - a.time); S.messages.length = Math.min(S.messages.length, 200);
      } catch (e) { setStatus(url, { net: false, error: String(e.message || e) }); }
    }));
    S.ready = true;
  }
  async function loadTraffic() {
    await Promise.all(hosts().map(async (url) => {
      try {
        const j = await getJson(url, "/api/public/traffic", 3500);
        for (const b of j.buckets || []) {
          const key = url + j.session + ":" + b.second;
          if (S.seenSecond.has(key)) continue;
          S.seenSecond.add(key);
          S.rate.push({ at: Date.now(), rx: b.rx, tx: b.tx });
        }
        if (S.seenSecond.size > 4000) S.seenSecond.clear();
        S.rate = S.rate.filter((r) => Date.now() - r.at < 120000);
        setStatus(url, { traffic: true });
      } catch (e) { setStatus(url, { traffic: false, error: String(e.message || e) }); }
    }));
  }
  function loop(fn, ms) { const tick = async () => { if (!S.open) return; try { await fn(); } catch { /* shown in the status */ } render(); S.timers.push(setTimeout(tick, ms)); }; tick(); }

  // ---------- drawing ----------
  const sum = (r) => (r.radio || 0) + (r.trunk || 0) + (r.backbone || 0);
  function renderTop() {
    const now = Date.now() / 1000, nodes = [...S.nodes.values()];
    const minute = S.rate.filter((r) => Date.now() - r.at < 60000).reduce((a, r) => a + sum(r.rx) + sum(r.tx), 0);
    const last10 = (dir, kind) => S.rate.filter((r) => Date.now() - r.at < 10000).reduce((a, r) => a + (r[dir][kind] || 0), 0);
    const ok = [...S.status.values()].filter((s) => s.map || s.net).length;
    const t = (v, l, title = "") => `<div class="t" title="${esc(title)}"><b>${v}</b><span>${l}</span></div>`;
    const problems = [...S.status.entries()].filter(([, s]) => s.error).map(([u, s]) => `${u}: ${s.error}`);
    top.innerHTML = t(nodes.length, "nodes on the map") + t(nodes.filter((n) => now - (n.last_seen || 0) < 3600).length, "heard in the last hour")
      + t(minute, "packets / min") + TRANSPORTS.map((k) => t(`${last10("rx", k)}↓ ${last10("tx", k)}↑`, k + " · 10 s")).join("")
      + t(`${ok}/${hosts().length}`, "servers reporting", problems.join("\n")) + t(S.observers.size, "observers heard") + t(S.messages.length, "channel messages")
      + `<span class="sp"></span><div class="acts"><button id="ov-snd" aria-pressed="${bib.on}">Sound: ${bib.on ? "on" : "off"}</button>
        <section id="ov-sound" class="ovx hidden"><label><input type="checkbox" id="ov-snd-on" ${bib.on ? "checked" : ""}> <b>Sound alerts</b></label>
        <label>Volume <input type="range" id="ov-vol" min="0" max="1" step="0.05" value="${bib.vol}" style="flex:1"></label>
        ${Object.entries(TONES).map(([k, v]) => `<label><input type="checkbox" data-bib="${k}" ${bib.types[k] ? "checked" : ""}> ${esc(v.label)}</label>`).join("")}
        <button class="chip" id="ov-snd-test">Test sound</button></section></div>`;
    if (problems.length) top.insertAdjacentHTML("beforeend", `<div class="mute" style="width:100%">${esc(problems[0])}</div>`);
  }
  function renderNodes() {
    const q = S.filter.toLowerCase(), now = Date.now() / 1000;
    const rows = [...S.nodes.values()].filter((n) => (!S.roles.size || S.roles.has(roleOf(n))) && (!q || `${n.name} ${n.addr} ${roleOf(n)}`.toLowerCase().includes(q)))
      .sort((a, b) => (b.last_seen || 0) - (a.last_seen || 0));
    $("ov-list").innerHTML = rows.map((n) => `<div class="item ${S.sel === n.addr ? "on" : ""}" data-addr="${n.addr}"><span class="dot" style="background:${ROLE_COLOR[roleOf(n)] || "#94a3b8"};opacity:${now - (n.last_seen || 0) < 3600 ? 1 : .35}"></span>
      <span class="grow">${esc(n.name || n.addr.slice(0, 8))}</span><span class="mute">${esc(roleOf(n))}</span><time>${ago(n.last_seen)}</time></div>`).join("") || '<div class="item mute">No nodes yet.</div>';
  }
  function renderCard() {
    const n = S.sel && S.nodes.get(S.sel);
    if (!n) { $("ov-card").innerHTML = ""; return; }
    const seen = S.packets.filter((g) => (g.observations || []).some((o) => o.subject === n.addr)).slice(0, 5);
    const fact = (k, v) => v == null || v === "" ? "" : `<div><span>${esc(k)}</span>${esc(v)}</div>`;
    $("ov-card").innerHTML = `<div class="card"><div style="display:flex;gap:8px;align-items:center"><span class="dot" style="background:${ROLE_COLOR[roleOf(n)] || "#94a3b8"}"></span><b class="grow">${esc(n.name || n.addr.slice(0, 8))}</b><button class="chip" id="ov-card-x">Close</button></div>
      <div class="mute" style="word-break:break-all">${esc(n.addr)}</div><div class="facts">${fact("Role", roleOf(n))}${fact("Last heard", n.last_seen ? ago(n.last_seen) + " ago" : "")}
      ${fact("Position", n.lat != null ? `${n.lat.toFixed(3)}, ${n.lon.toFixed(3)}` : "")}${fact("Antenna height", n.height_m != null ? n.height_m + " m" : "")}${fact("Gain", n.gain_dbi != null ? n.gain_dbi + " dBi" : "")}
      ${fact("Power", n.tx_dbm != null ? n.tx_dbm + " dBm" : "")}${fact("Owner", n.owner)}${fact("Moving", n.mobile ? "yes" : "")}</div>
      ${n.lat != null ? '<button class="chip" id="ov-fly">Show on the map</button>' : ""}
      ${seen.length ? `<div class="mute" style="margin-top:6px">Recently announced: ${seen.map((g) => clock(g.observations[0].time)).join(", ")}</div>` : ""}</div>`;
  }
  function renderBody() {
    const body = $("ov-body");
    if (S.tab === "channels") {
      body.innerHTML = S.messages.map((m) => { const who = byPrefix(m.sender); return `<div class="msg"><div><time>${clock(m.time)}</time> <span class="k" style="background:#f472b6">${esc(m.channel)}</span> <span class="who">${esc(who ? who.name || who.addr.slice(0, 8) : (m.sender || "?"))}</span></div><div>${esc(m.text)}</div></div>`; }).join("")
        || '<div class="item mute">No messages yet. Only open channels (public and #hashtags) are shown; direct messages are encrypted.</div>';
    } else if (S.tab === "types") {
      const entries = Object.entries(S.types).sort((a, b) => b[1] - a[1]), max = Math.max(1, ...entries.map((e) => e[1]));
      const secs = Array.from({ length: 60 }, (_, i) => { const t1 = Date.now() - (59 - i) * 2000; return S.rate.filter((r) => r.at <= t1 && r.at > t1 - 2000).reduce((a, r) => a + sum(r.rx) + sum(r.tx), 0); });
      const peak = Math.max(1, ...secs);
      body.innerHTML = `<h4>Packets per 2 s · last 2 min</h4><div class="spark">${secs.map((v) => `<i style="height:${Math.max(2, Math.round(v / peak * 100))}%"></i>`).join("")}</div>
        <h4>Packet types since the overview opened</h4>${entries.map(([k, c]) => `<div class="barline"><span class="k" style="background:${KIND_COLOR[k] || "#94a3b8"};min-width:84px">${esc(k)}</span><i style="width:${Math.round(c / max * 170)}px;background:${KIND_COLOR[k] || "#94a3b8"}"></i><span>${c}</span></div>`).join("") || '<div class="item mute">Nothing yet.</div>'}`;
    } else {
      body.innerHTML = S.packets.slice(0, 120).map((g) => {
        const o = g.observations || [], first = o[0] || {}, subject = o.find((x) => x.subject)?.subject;
        const tr = [...new Set(o.map((x) => x.transport))].join(" ");
        return `<div class="item" ${subject ? `data-addr="${subject}"` : ""}><time>${first.time ? clock(first.time) : ""}</time><span class="k" style="background:${KIND_COLOR[g.kind] || "#94a3b8"}">${esc(g.kind)}</span><span class="grow">${esc(subject ? nodeName(subject) : "")}</span><span class="mute">${esc(tr)} · ${first.hops ?? "?"} hop${first.hops === 1 ? "" : "s"}${first.snr != null ? " · " + first.snr + " dB" : ""}${g.observer_count > 1 ? " · " + g.observer_count + " obs" : ""}</span></div>`;
      }).join("") || '<div class="item mute">No packets in the last two minutes. Servers share these when "Share live traffic" is on in their Settings.</div>';
    }
  }
  function render() {
    if (!S.open) return;
    const active = document.activeElement;
    const typing = active && active.id === "ov-search";
    const panelOpen = $("ov-sound") && !$("ov-sound").classList.contains("hidden");
    if (!typing && !(active && active.id === "ov-vol")) { renderTop(); if (panelOpen) $("ov-sound").classList.remove("hidden"); }
    renderNodes(); renderCard(); renderBody();
    for (const b of document.querySelectorAll("#ov-tabs [data-tab]")) b.setAttribute("aria-pressed", String(b.dataset.tab === S.tab));
    for (const b of document.querySelectorAll("#ov-roles [data-role]")) b.setAttribute("aria-pressed", String(S.roles.has(b.dataset.role)));
  }

  // ---------- selection on the map ----------
  let halo = null;
  function select(addr) {
    S.sel = addr || null;
    if (halo) { map.removeLayer(halo); halo = null; }
    const n = addr && S.nodes.get(addr);
    if (n && n.lat != null) halo = L.circleMarker([n.lat, n.lon], { radius: 16, color: "#5b8def", weight: 3, fillOpacity: 0.08, interactive: false }).addTo(map);
    render();
  }

  // ---------- real nodes on the map, and the view switch ----------
  let mode = "sim", hiddenSim = [];
  const realLayer = L.layerGroup();
  const simLayers = () => ["nodes", "links", "clutter", "disaster"].map((k) => dk.layers && dk.layers[k]).filter(Boolean);
  function drawReal() {
    realLayer.clearLayers();
    const now = Date.now() / 1000;
    for (const n of S.nodes.values()) {
      if (n.lat == null || n.lon == null) continue;
      const color = ROLE_COLOR[roleOf(n)] || "#94a3b8", fresh = now - (n.last_seen || 0) < 3600;
      L.circleMarker([n.lat, n.lon], { radius: n.host ? 9 : 7, color, weight: 2, fillColor: color, fillOpacity: fresh ? 0.85 : 0.3 })
        .bindTooltip(`${esc(n.name || n.addr.slice(0, 8))} · ${esc(roleOf(n))}${n.last_seen ? " · " + ago(n.last_seen) : ""}`)
        .on("click", () => select(n.addr)).addTo(realLayer);
    }
  }
  function applyLayers() {
    if (mode !== "real") {
      for (const l of hiddenSim) if (!map.hasLayer(l)) map.addLayer(l);
      hiddenSim = [];
    } else {
      for (const l of simLayers()) if (map.hasLayer(l)) { if (!hiddenSim.includes(l)) hiddenSim.push(l); map.removeLayer(l); }
    }
    if (mode === "sim") { if (map.hasLayer(realLayer)) map.removeLayer(realLayer); }
    else if (!map.hasLayer(realLayer)) realLayer.addTo(map);
  }
  function startPanels() {
    if (S.open) return;
    S.open = true; S.ready = false;
    $("rw-card")?.classList.add("hidden");
    for (const el of [top, nodesPanel, feedPanel]) el.classList.remove("hidden");
    loop(loadMap, 10000); loop(loadNetwork, 2000); loop(loadTraffic, 1000);
    render();
  }
  function stopPanels() {
    S.open = false;
    S.timers.forEach(clearTimeout); S.timers = [];
    for (const el of [top, nodesPanel, feedPanel]) el.classList.add("hidden");
    select(null);
  }
  function setMode(next) {
    mode = next;
    for (const b of modeBar.querySelectorAll("[data-mode]")) b.setAttribute("aria-pressed", String(b.dataset.mode === mode));
    wrap.classList.toggle("real-only", mode === "real");
    if (mode === "sim") stopPanels(); else startPanels();
    drawReal(); applyLayers();
  }
  const open = () => setMode("real"), close = () => setMode("sim");
  modeBar.onclick = (e) => { const b = e.target.closest("[data-mode]"); if (b) setMode(b.dataset.mode); };
  $("ov-close").onclick = close;
  $("ov-search").oninput = (e) => { S.filter = e.target.value; renderNodes(); };
  $("ov-roles").onclick = (e) => { const b = e.target.closest("[data-role]"); if (!b) return; S.roles.has(b.dataset.role) ? S.roles.delete(b.dataset.role) : S.roles.add(b.dataset.role); render(); };
  $("ov-tabs").onclick = (e) => { const b = e.target.closest("[data-tab]"); if (!b) return; S.tab = b.dataset.tab; render(); };
  nodesPanel.addEventListener("click", (e) => { const it = e.target.closest(".item[data-addr]"); if (it) select(it.dataset.addr); });
  feedPanel.addEventListener("click", (e) => {
    const it = e.target.closest(".item[data-addr]"); if (it && S.nodes.has(it.dataset.addr)) { select(it.dataset.addr); return; }
    if (e.target.id === "ov-card-x") select(null);
    if (e.target.id === "ov-fly") { const n = S.nodes.get(S.sel); if (n && n.lat != null) map.flyTo([n.lat, n.lon], Math.max(map.getZoom(), 10)); }
  });
  top.addEventListener("click", (e) => {
    if (e.target.id === "ov-snd") $("ov-sound").classList.toggle("hidden");
    if (e.target.id === "ov-snd-test") beep("public", true);
  });
  top.addEventListener("change", (e) => {
    if (e.target.id === "ov-snd-on") { bib.on = e.target.checked; saveBib(); if (bib.on) beep("public", true); renderTop(); $("ov-sound").classList.remove("hidden"); }
    else if (e.target.dataset.bib) { bib.types[e.target.dataset.bib] = e.target.checked; saveBib(); beep(e.target.dataset.bib, true); }
  });
  top.addEventListener("input", (e) => { if (e.target.id === "ov-vol") { bib.vol = +e.target.value; saveBib(); } });
  window.DKOverview = { open, close, setMode, toggle: () => (mode === "sim" ? open() : close()) };
  if (location.hash === "#network" || new URLSearchParams(location.search).get("view") === "network") setMode("real");
})();
