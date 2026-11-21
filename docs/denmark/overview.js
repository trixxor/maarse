// Network overview: the real network at a glance, in the style of a mesh
// telemetry site. It reads the public feeds of backbone servers
// (GET /api/public/map, /traffic, /network and /history) and shows
// * key figures in the top bar (nodes, packets per minute, transports, servers),
// * a node list on the left and the selected node on the right,
// * a Packets window (every packet group with hash, hops, type, observer and
//   how many observers saw it; addresses only, no names) and a Channels
//   window (channel list and conversation),
// * in the real network view a history bar instead of the simulation's play
//   bar: the server keeps 24 hours, so the whole view can be stepped back,
// * optional sound for public messages, direct messages and new nodes.
// The servers are the ones set in the Real world card (kept in this browser
// only). A server shares these feeds with "Share the public map" and
// "Share live traffic" under Settings; "Public history" keeps the 24 hours.
(function () {
  const dk = window.DKSim;
  if (!dk || !dk.map) return;
  const map = dk.map;
  const $ = (id) => document.getElementById(id);
  const esc = (t) => String(t ?? "").replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
  const STORE = "maarse.realworld.servers", BIB_STORE = "maarse.overview.sound";
  const DEFAULT_COLLECTOR = "https://nj01.dk.eu.maarse.dk";
  const MIN = 60000, HOUR = 3600000, DAY = 86400000;
  const ROLE_COLOR = { Repeater: "#60a5fa", Anchor: "#a78bfa", Relay: "#34d399", Client: "#fbbf24", Host: "#f87171" };
  const KIND_COLOR = { Announce: "#22c55e", Data: "#f59e0b", Ack: "#38bdf8", Channel: "#3b82f6", PathRequest: "#a855f7", PathResponse: "#14b8a6", Fragment: "#ec4899", Probe: "#94a3b8", Hello: "#64748b", Control: "#94a3b8", Deposit: "#f97316", Datagram: "#84cc16", Carry: "#eab308", KeyRequest: "#a855f7", MailSync: "#f97316", Summary: "#64748b" };
  const TRANSPORTS = ["radio", "trunk", "backbone"];
  const hosts = () => {
    try { const v = JSON.parse(localStorage.getItem(STORE) || "[]"); if (Array.isArray(v) && v.length) return v; } catch { /* storage blocked */ }
    return [DEFAULT_COLLECTOR];
  };

  const S = {
    mode: "sim", open: false, win: null, filter: "", roles: new Set(), sel: null,
    nodes: new Map(), status: new Map(), packets: [], rate: [], observers: new Set(),
    seenGroup: new Set(), seenMessage: new Set(), seenSecond: new Set(), known: new Set(), ready: false, timers: [],
    channels: new Set(["public"]), chan: "public", chanSig: "",
    pkt: { filter: "", type: "", transport: "", grouped: true, open: new Set() },
    view: null, play: null, speed: 300, range: DAY,
  };
  // The history: per minute, the announces and the open channel messages.
  const H = { buckets: new Map(), announces: [], messages: [], server: false, note: "" };
  let annBy = new Map();

  // ---------- time ----------
  const at = () => S.view ?? Date.now();
  const minuteOf = (t) => Math.floor(t / MIN) * MIN;
  const clock = (ms) => new Date(ms).toLocaleTimeString([], { hour12: false });
  const stamp = (ms) => new Date(ms).toLocaleString([], { weekday: "short", day: "numeric", month: "short", hour: "2-digit", minute: "2-digit", hour12: false });
  const span = (ms) => { const s = Math.max(0, Math.round(ms / 1000)); return s < 90 ? s + " s" : s < 5400 ? Math.round(s / 60) + " min" : s < 172800 ? Math.round(s / 3600) + " h" : Math.round(s / 86400) + " d"; };
  const ago = (ms) => (ms == null ? "" : span(at() - ms));
  const sum = (r) => (r?.radio || 0) + (r?.trunk || 0) + (r?.backbone || 0);
  const shortAddr = (a) => String(a || "").slice(0, 8);
  const hue = (s) => { let h = 0; for (const c of String(s)) h = (h * 31 + c.charCodeAt(0)) % 360; return h; };

  function rebuildAnn() {
    annBy = new Map();
    for (const a of H.announces) { let l = annBy.get(a.addr); if (!l) annBy.set(a.addr, l = []); l.push(a.t); }
    for (const l of annBy.values()) l.sort((x, y) => x - y);
  }
  function lastHeard(addr) {
    const limit = at(), times = annBy.get(addr);
    let best = null;
    if (times) { let lo = 0, hi = times.length - 1; while (lo <= hi) { const m = (lo + hi) >> 1; if (times[m] <= limit) { best = times[m]; lo = m + 1; } else hi = m - 1; } }
    const n = S.nodes.get(addr), seen = n && n.last_seen ? n.last_seen * 1000 : null;
    if (seen != null && seen <= limit && (best == null || seen > best)) best = seen;
    return best;
  }

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
    if (!force && (!bib.on || !bib.types[type] || S.view != null)) return;
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
    #mapwrap.ov-docked { --ovl:270px; --ovr:310px; --ovb:0px; }
    #mapwrap.real-only { --ovb:74px; }
    #mapwrap.ov-docked #map { left:var(--ovl); right:var(--ovr); bottom:var(--ovb); }
    body.real-view #mapwrap > :not(#map):not(#ov-nodes):not(#ov-side):not(#ov-pkt):not(#ov-chan):not(#ov-tl):not(#rw-card):not(#cv-card) { display:none !important; }
    body.real-view #cv-card { position:absolute !important; left:auto !important; right:calc(var(--ovr) + 12px) !important; top:10px !important; bottom:auto !important; height:auto !important; width:330px !important; z-index:1200 !important; max-height:calc(100% - var(--ovb, 0px) - 110px) !important; overflow:auto; }
    #ov-row { grid-column:1 / -1; display:flex; gap:10px 14px; align-items:center; flex-wrap:wrap; padding-top:6px; margin-top:2px; border-top:1px solid var(--line); min-height:34px; font-size:12px; }
    #ov-mode { display:flex; border:1px solid var(--line); border-radius:10px; overflow:hidden; }
    #ov-mode button { border:0; border-radius:0; background:transparent; padding:4px 12px; cursor:pointer; }
    #ov-mode button[aria-pressed="true"] { background:var(--acc); color:#fff; }
    #ov-figs { display:flex; gap:4px 14px; flex-wrap:wrap; flex:1; min-width:0; }
    #ov-figs .fg { white-space:nowrap; color:var(--mute); }
    #ov-figs .fg b { color:var(--ink); font-size:14px; font-variant-numeric:tabular-nums; }
    #ov-wins { display:flex; gap:4px; }
    #ov-wins button[aria-pressed="true"], #ov-row .chip[aria-pressed="true"] { background:var(--acc); color:#fff; border-color:var(--acc); }
    #ov-row .acts { position:relative; display:flex; gap:6px; }
    .ovx { position:absolute; z-index:900; background:var(--ov); color:var(--ink); font-size:12px; }
    .ovx.hidden, #ov-row .hidden { display:none !important; }
    #ov-nodes { left:0; top:0; bottom:var(--ovb); width:var(--ovl); border-right:1px solid var(--ovline); display:flex; flex-direction:column; }
    #ov-side { right:0; top:0; bottom:var(--ovb); width:var(--ovr); border-left:1px solid var(--ovline); display:flex; flex-direction:column; overflow:auto; }
    .ov-win { left:var(--ovl); right:var(--ovr); top:0; bottom:var(--ovb); display:flex; flex-direction:column; z-index:1100; background:var(--panel); }
    .ovx h4 { margin:0; padding:8px 10px 4px; font-size:12px; text-transform:uppercase; letter-spacing:.04em; color:var(--mute); }
    .ovx .bar { padding:4px 10px 6px; display:flex; gap:6px; flex-wrap:wrap; align-items:center; }
    .ovx .bar input[type=search], .ovx .bar input[type=text] { flex:1; min-width:120px; }
    .chip { padding:2px 8px; border-radius:99px; font-size:11px; cursor:pointer; }
    .chip[aria-pressed="true"] { background:var(--acc); color:#fff; border-color:var(--acc); }
    .ovx .list { flex:1; overflow:auto; border-top:1px solid var(--ovline); }
    .ovx .item { display:flex; gap:8px; align-items:center; padding:5px 10px; border-bottom:1px solid var(--ovline); cursor:pointer; }
    .ovx .item:hover, .ovx .item.on { background:color-mix(in srgb, var(--acc) 14%, transparent); }
    .dot { width:9px; height:9px; border-radius:50%; flex:none; display:inline-block; }
    .grow { flex:1; min-width:0; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
    .mute { color:var(--mute); }
    .ovx time { color:var(--mute); font-variant-numeric:tabular-nums; flex:none; }
    .k { display:inline-block; padding:0 7px; border-radius:6px; color:#fff; font-weight:600; font-size:10.5px; letter-spacing:.03em; text-transform:uppercase; flex:none; }
    .card { padding:8px 10px; border-bottom:1px solid var(--ovline); }
    .facts { display:grid; grid-template-columns:1fr 1fr; gap:4px 12px; margin:6px 0; }
    .facts span { display:block; color:var(--mute); font-size:11px; }
    .barline { display:flex; align-items:center; gap:6px; padding:3px 10px; }
    .barline i { display:block; height:8px; border-radius:4px; }
    .spark { display:flex; align-items:flex-end; gap:1px; height:46px; padding:4px 10px; }
    .spark i { flex:1; background:var(--acc); border-radius:1px 1px 0 0; min-height:1px; }
    .mono { font-family:ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; }
    .ov-win header { display:flex; gap:8px; align-items:center; padding:8px 12px; border-bottom:1px solid var(--ovline); flex-wrap:wrap; }
    .ov-win header h3 { margin:0; font-size:15px; }
    .ov-win .tbl { flex:1; overflow:auto; }
    .ov-win table { width:100%; border-collapse:collapse; }
    .ov-win th { position:sticky; top:0; background:var(--panel); text-align:left; font-size:10.5px; letter-spacing:.05em; text-transform:uppercase; color:var(--mute); padding:5px 8px; border-bottom:1px solid var(--ovline); z-index:1; }
    .ov-win td { padding:4px 8px; border-bottom:1px solid var(--ovline); white-space:nowrap; font-variant-numeric:tabular-nums; }
    .ov-win tr.g td:first-child { border-left:3px solid var(--edge, transparent); cursor:pointer; }
    .ov-win tr.g:hover td { background:color-mix(in srgb, var(--acc) 10%, transparent); }
    .ov-win tr.sub td { background:color-mix(in srgb, var(--ink) 4%, transparent); color:var(--mute); }
    .rpt { display:inline-block; min-width:26px; text-align:center; padding:0 6px; border-radius:99px; border:1px solid var(--ovline); font-size:11px; }
    #ov-chan { flex-direction:row; }
    #ch-list { width:270px; flex:none; border-right:1px solid var(--ovline); overflow:auto; }
    #ch-list .item { padding:9px 12px; }
    .ava { width:34px; height:34px; border-radius:50%; display:flex; align-items:center; justify-content:center; font-weight:700; color:#fff; font-size:12px; flex:none; }
    #ch-main { flex:1; min-width:0; display:flex; flex-direction:column; }
    #ch-msgs { flex:1; overflow:auto; padding:10px 16px; }
    .bub { display:flex; gap:10px; margin:12px 0; }
    .bub .who { font-weight:600; font-size:12px; }
    .bub .txt { display:inline-block; margin-top:3px; padding:7px 11px; border-radius:12px; background:color-mix(in srgb, var(--ink) 8%, transparent); max-width:70ch; white-space:pre-wrap; overflow-wrap:anywhere; }
    .bub .meta { color:var(--mute); font-size:11px; margin-top:3px; }
    .day { text-align:center; color:var(--mute); font-size:11px; margin:14px 0 4px; }
    #ov-tl { position:absolute; z-index:1150; left:calc(var(--ovl) + 10px); right:calc(var(--ovr) + 10px); bottom:10px; height:56px; display:flex; gap:8px; align-items:center; padding:4px 8px; border:1px solid var(--ovline); border-radius:12px; background:var(--ov); font-size:12px; }
    #ov-tl.hidden { display:none; }
    #ov-tl canvas { flex:1; min-width:80px; height:44px; cursor:pointer; }
    #ov-tl .when { min-width:132px; text-align:right; line-height:1.2; }
    #ov-sound { position:absolute; right:0; top:34px; width:250px; padding:8px 10px; z-index:1100; border:1px solid var(--ovline); border-radius:10px; box-shadow:0 8px 28px rgba(0,0,0,.25); }
    #ov-sound label { display:flex; gap:8px; align-items:center; margin:3px 0; }
    body.real-view #top { z-index:2000; }
    body.real-view :is(#preset, #btn-file, #modes, #btn-reset, #side) { display:none !important; }
    #rw-card { left:auto !important; right:calc(var(--ovr) + 12px) !important; top:10px !important; bottom:auto !important; height:auto !important; width:330px !important; z-index:1200 !important; max-height:calc(100% - var(--ovb, 0px) - 110px) !important; }
    #ov-wins .sep { width:1px; background:var(--line); margin:0 4px; }
    .lp { display:flex; gap:6px; align-items:center; padding:4px 8px; border-left:3px solid var(--edge, transparent); border-bottom:1px solid var(--ovline); cursor:pointer; }
    .lp:hover { background:color-mix(in srgb, var(--acc) 10%, transparent); }
    .lp .hb { font-size:10.5px; padding:0 5px; border-radius:5px; border:1px solid var(--ovline); color:var(--mute); }
    .lp-list { max-height:38vh; overflow:auto; border-top:1px solid var(--ovline); }
    .legend { padding:4px 10px 10px; display:grid; grid-template-columns:1fr; gap:3px; color:var(--mute); }
    .legend i { display:inline-block; width:9px; height:9px; border-radius:2px; margin-right:7px; vertical-align:-1px; }
    #ov-tl .rng { display:flex; gap:2px; }
    #ov-tl .live { background:#b91c1c; color:#fff; border-color:#b91c1c; font-weight:700; letter-spacing:.05em; }
    #ov-tl .live[aria-pressed="false"] { background:transparent; color:var(--ink); border-color:var(--ovline); }
    #ov-tl .dot-live { color:#22c55e; white-space:nowrap; }
    #ov-tl .dot-live.off { color:var(--mute); }
    #ov-tl .clock { font-family:ui-monospace, Consolas, monospace; font-size:20px; letter-spacing:.06em; padding:2px 8px; border-radius:6px; background:#0b1410; color:#4ade80; min-width:104px; text-align:center; }
    @media (max-width: 1100px) { #mapwrap.ov-docked { --ovl:220px; --ovr:240px; } #ch-list { width:200px; } }
  `;
  document.head.appendChild(style);

  const wrap = $("mapwrap"), header = $("top");
  const row = document.createElement("div");
  row.id = "ov-row";
  row.innerHTML = `<div id="ov-mode" role="group" aria-label="View">${[["sim", "Simulation", "The simulated network only"], ["real", "Real network", "The real network from the public servers, with 24 hours of history"]]
    .map(([k, l, t]) => `<button data-mode="${k}" title="${t}" aria-pressed="${k === "sim"}">${l}</button>`).join("")}</div>
    <div id="ov-figs" class="hidden"></div>
    <div id="ov-wins" class="hidden"><button class="chip" data-win="packets" aria-pressed="false">Packets</button><button class="chip" data-win="channels" aria-pressed="false">Channels</button><span class="sep"></span><button class="chip" data-act="servers" aria-pressed="false" title="Servers the view reads, and the model-versus-measured check">Servers</button><button class="chip" data-act="coverage" title="Coverage of the real fixed nodes, with the simulator's radio model">Coverage</button></div>
    <div class="acts hidden" id="ov-acts"><button class="chip" id="ov-snd" aria-pressed="false">Sound: off</button>
      <section id="ov-sound" class="ovx hidden" style="position:absolute"><label><input type="checkbox" id="ov-snd-on"> <b>Sound alerts</b></label>
      <label>Volume <input type="range" id="ov-vol" min="0" max="1" step="0.05" style="flex:1"></label>
      ${Object.entries(TONES).map(([k, v]) => `<label><input type="checkbox" data-bib="${k}"> ${esc(v.label)}</label>`).join("")}
      <button class="chip" id="ov-snd-test">Test sound</button></section></div>`;
  header.appendChild(row);

  const nodesPanel = document.createElement("section");
  nodesPanel.id = "ov-nodes"; nodesPanel.className = "ovx hidden"; nodesPanel.setAttribute("aria-label", "Nodes");
  nodesPanel.innerHTML = `<h4>Nodes</h4>
    <div class="bar"><input id="ov-search" type="search" placeholder="Search name, address, role"></div>
    <div class="bar" id="ov-roles">${Object.keys(ROLE_COLOR).map((r) => `<button class="chip" data-role="${r}" aria-pressed="false">${r}</button>`).join("")}</div>
    <div class="list" id="ov-list"></div>`;
  const side = document.createElement("section");
  side.id = "ov-side"; side.className = "ovx hidden"; side.setAttribute("aria-label", "Selection and activity");
  side.innerHTML = `<div id="ov-card"></div><div id="ov-now"></div>`;

  const pktWin = document.createElement("section");
  pktWin.id = "ov-pkt"; pktWin.className = "ovx ov-win hidden"; pktWin.setAttribute("aria-label", "Packets");
  pktWin.innerHTML = `<header><h3>Packets <span class="mute" id="pk-count"></span></h3>
      <input id="pk-filter" type="search" placeholder="Filter: hash, address, type, observer" style="flex:1;min-width:160px">
      <select id="pk-type" aria-label="Type"><option value="">All types</option></select>
      <select id="pk-transport" aria-label="Transport"><option value="">All transports</option>${TRANSPORTS.map((t) => `<option>${t}</option>`).join("")}</select>
      <label><input type="checkbox" id="pk-group" checked> Group by hash</label>
      <button class="chip" data-close="1">Close</button></header>
    <div class="tbl" id="pk-body"></div>`;
  const chanWin = document.createElement("section");
  chanWin.id = "ov-chan"; chanWin.className = "ovx ov-win hidden"; chanWin.setAttribute("aria-label", "Channels");
  chanWin.innerHTML = `<div id="ch-list"></div><div id="ch-main"><header><h3 id="ch-title">Channels</h3><span class="grow"></span><button class="chip" data-close="1">Close</button></header><div id="ch-msgs"></div></div>`;

  const tl = document.createElement("div");
  tl.id = "ov-tl"; tl.className = "hidden";
  tl.innerHTML = `<button class="chip" data-step="-600000" title="Back ten minutes (left arrow)">⏪</button><button class="chip" id="tl-play" title="Play the history">▶</button><button class="chip" data-step="600000" title="Forward ten minutes (right arrow)">⏩</button>
    <button class="chip live" id="tl-live" aria-pressed="true" title="Back to live">LIVE</button>
    <select id="tl-speed" title="Play speed"><option value="1">1×</option><option value="10">10×</option><option value="60">60×</option><option value="300" selected>300×</option><option value="1800">1800×</option></select>
    <span class="dot-live" id="tl-dot">● LIVE</span>
    <span class="rng" id="tl-range">${[["3600000", "1h"], ["21600000", "6h"], ["43200000", "12h"], ["86400000", "24h"]].map(([v, l]) => `<button class="chip" data-range="${v}" aria-pressed="${v === "86400000"}">${l}</button>`).join("")}</span>
    <canvas id="tl-graph" height="44" aria-label="Packets per minute; click to go to a time"></canvas>
    <div class="clock" id="tl-clock">--:--:--</div>`;
  [nodesPanel, side, pktWin, chanWin, tl].forEach((el) => { wrap.appendChild(el); L.DomEvent.disableClickPropagation(el); L.DomEvent.disableScrollPropagation(el); });

  // ---------- helpers ----------
  const nodeName = (addr) => { const n = S.nodes.get(addr); return n && n.name ? n.name : shortAddr(addr); };
  const byPrefix = (prefix) => { if (!prefix) return null; for (const n of S.nodes.values()) if (n.addr.startsWith(prefix)) return n; return null; };
  const roleOf = (n) => (n.host ? "Host" : n.role || "Client");
  async function getJson(url, path, ms = 4500) {
    const r = await fetch(url + path, { cache: "no-store", credentials: "omit", signal: AbortSignal.timeout(ms) });
    if (r.status === 404) throw new Error("not shared");
    if (!r.ok) throw new Error("answered " + r.status);
    return r.json();
  }
  const setStatus = (url, patch) => S.status.set(url, { ...(S.status.get(url) || {}), ...patch });
  const notShared = 'turn on "Share the public map" and "Share live traffic" in its Settings';

  function addMessage(m) {
    const key = m.time + "|" + m.channel + "|" + m.sender + "|" + m.text;
    if (S.seenMessage.has(key)) return false;
    S.seenMessage.add(key);
    H.messages.push(m); S.channels.add(m.channel);
    return true;
  }
  function bucketFor(t) {
    const k = minuteOf(t);
    let b = H.buckets.get(k);
    if (!b) H.buckets.set(k, b = { t: k, rx: {}, tx: {}, types: {} });
    return b;
  }
  function pruneLocal() {
    const oldest = Date.now() - DAY;
    for (const k of H.buckets.keys()) if (k < oldest) H.buckets.delete(k);
    H.announces = H.announces.filter((a) => a.t >= oldest);
    H.messages = H.messages.filter((m) => m.time >= oldest);
  }

  // ---------- polling ----------
  async function loadMap() {
    await Promise.all(hosts().map(async (url) => {
      try {
        const j = await getJson(url, "/api/public/map");
        if (j.host?.addr) S.nodes.set(j.host.addr, { ...j.host, role: "Anchor", host: true, last_seen: Math.floor(Date.now() / 1000), name: j.host.name || new URL(url).hostname });
        for (const n of j.nodes || []) S.nodes.set(n.addr, { ...(S.nodes.get(n.addr) || {}), ...n });
        setStatus(url, { map: true, error: null });
      } catch (e) { setStatus(url, { map: false, error: e.message === "not shared" ? notShared : String(e.message || e) }); }
    }));
    for (const n of S.nodes.values()) S.known.add(n.addr);
    drawReal(); applyLayers();
  }
  async function loadNetwork() {
    await Promise.all(hosts().map(async (url) => {
      try {
        const j = await getJson(url, "/api/public/network");
        setStatus(url, { net: true });
        for (const c of j.channels || []) S.channels.add(c);
        for (const g of j.packets || []) {
          for (const o of g.observations || []) { if (o.observer) S.observers.add(o.observer); }
          if (S.seenGroup.has(g.id)) continue;
          S.seenGroup.add(g.id);
          S.packets.unshift(g);
          if (S.ready) pulseFor(g);
          const first = (g.observations || [])[0] || {}, subject = (g.observations || []).find((o) => o.subject)?.subject;
          if (!H.server && first.time) {
            bucketFor(first.time).types[g.kind] = (bucketFor(first.time).types[g.kind] || 0) + 1;
            if (g.kind === "Announce" && subject) { H.announces.push({ t: first.time, addr: subject }); annBy = new Map(); rebuildAnn(); }
          }
          if (g.kind === "Announce" && subject) {
            const fresh = !S.known.has(subject); S.known.add(subject);
            if (S.ready && fresh) beep("newnode");
          } else if (S.ready && g.kind === "Data") beep("message");
        }
        S.packets.sort((a, b) => ((b.observations || [])[0]?.time || 0) - ((a.observations || [])[0]?.time || 0));
        S.packets.length = Math.min(S.packets.length, 400);
        for (const m of j.messages || []) if (addMessage(m) && S.ready) beep(m.channel === "public" ? "public" : "channel");
      } catch (e) { setStatus(url, { net: false, error: e.message === "not shared" ? notShared : String(e.message || e) }); }
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
          if (!H.server) {
            const bk = bucketFor(Date.now());
            for (const d of ["rx", "tx"]) for (const k of TRANSPORTS) bk[d][k] = (bk[d][k] || 0) + (b[d]?.[k] || 0);
          }
        }
        if (S.seenSecond.size > 4000) S.seenSecond.clear();
        S.rate = S.rate.filter((r) => Date.now() - r.at < 120000);
        setStatus(url, { traffic: true });
      } catch (e) { setStatus(url, { traffic: false, error: e.message === "not shared" ? notShared : String(e.message || e) }); }
    }));
  }
  function useHistory(j) {
    H.server = true; H.note = "";
    H.buckets = new Map((j.buckets || []).map((b) => [b.t, b]));
    H.announces = (j.announces || []).slice().sort((a, b) => a.t - b.t);
    for (const m of j.messages || []) addMessage(m);
    rebuildAnn();
  }
  async function loadHistory() {
    for (const url of hosts()) {
      try { useHistory(await getJson(url, "/api/public/history?since=" + (Date.now() - DAY), 9000)); return; }
      catch (e) { H.note = e.message === "not shared" || e instanceof TypeError ? "This server keeps no history yet: update it and set Public history (hours) in its Settings. Until then only this session is kept." : String(e.message || e); }
    }
    H.server = false;
  }
  function loop(fn, ms) { const tick = async () => { if (!S.open) return; try { await fn(); } catch { /* shown in the status */ } pruneLocal(); render(); S.timers.push(setTimeout(tick, ms)); }; tick(); }

  // ---------- header figures ----------
  function renderFigs() {
    const t = at(), live = S.view == null, nodes = [...S.nodes.values()];
    const heard = nodes.filter((n) => lastHeard(n.addr) != null);
    const active = heard.filter((n) => t - lastHeard(n.addr) < HOUR);
    const bk = H.buckets.get(minuteOf(t));
    const minute = live ? S.rate.filter((r) => Date.now() - r.at < 60000).reduce((a, r) => a + sum(r.rx) + sum(r.tx), 0) : bk ? sum(bk.rx) + sum(bk.tx) : 0;
    const flow = (dir, kind) => live ? S.rate.filter((r) => Date.now() - r.at < 10000).reduce((a, r) => a + (r[dir][kind] || 0), 0) : (bk?.[dir]?.[kind] || 0);
    const ok = [...S.status.values()].filter((s) => s.map || s.net).length;
    const problems = [...S.status.entries()].filter(([, s]) => s.error).map(([u, s]) => `${u}: ${s.error}`);
    const f = (v, l, title = "") => `<span class="fg" title="${esc(title)}"><b>${v}</b> ${l}</span>`;
    $("ov-figs").innerHTML = f(heard.length, "nodes") + f(active.length, "active 1 h") + f(minute, live ? "pkts/min" : "pkts that minute")
      + TRANSPORTS.map((k) => f(`${flow("rx", k)}↓ ${flow("tx", k)}↑`, k, k + (live ? " · last 10 s" : " · that minute"))).join("")
      + f(`${ok}/${hosts().length}`, "servers", problems.join("\n") || "reporting") + f(S.observers.size, "observers")
      + f(H.messages.filter((m) => m.time <= t).length, "messages") + (problems.length ? `<span class="fg" title="${esc(problems.join("\n"))}" style="color:var(--warn)">⚠ ${esc(problems[0].slice(0, 70))}</span>` : "");
    $("ov-snd").textContent = "Sound: " + (bib.on ? "on" : "off"); $("ov-snd").setAttribute("aria-pressed", String(bib.on));
  }

  // ---------- nodes and selection ----------
  function renderNodes() {
    const q = S.filter.toLowerCase(), t = at();
    const rows = [...S.nodes.values()].map((n) => ({ n, heard: lastHeard(n.addr) }))
      .filter(({ n }) => (!S.roles.size || S.roles.has(roleOf(n))) && (!q || `${n.name} ${n.addr} ${roleOf(n)}`.toLowerCase().includes(q)))
      .sort((a, b) => (b.heard || 0) - (a.heard || 0));
    $("ov-list").innerHTML = rows.map(({ n, heard }) => `<div class="item ${S.sel === n.addr ? "on" : ""}" data-addr="${n.addr}"><span class="dot" style="background:${ROLE_COLOR[roleOf(n)] || "#94a3b8"};opacity:${heard != null && t - heard < HOUR ? 1 : .3}"></span>
      <span class="grow">${esc(n.name || shortAddr(n.addr))}</span><span class="mute">${esc(roleOf(n))}</span><time>${heard != null ? span(t - heard) : "–"}</time></div>`).join("") || '<div class="item mute">No nodes yet.</div>';
  }
  function renderSide() {
    const n = S.sel && S.nodes.get(S.sel), t = at();
    if (n) {
      const heard = lastHeard(n.addr), times = (annBy.get(n.addr) || []).filter((x) => x <= t).slice(-5).reverse();
      const fact = (k, v) => v == null || v === "" ? "" : `<div><span>${esc(k)}</span>${esc(v)}</div>`;
      $("ov-card").innerHTML = `<div class="card"><div style="display:flex;gap:8px;align-items:center"><span class="dot" style="background:${ROLE_COLOR[roleOf(n)] || "#94a3b8"}"></span><b class="grow">${esc(n.name || shortAddr(n.addr))}</b><button class="chip" id="ov-card-x">Close</button></div>
        <div class="mute mono" style="word-break:break-all">${esc(n.addr)}</div><div class="facts">${fact("Role", roleOf(n))}${fact("Last heard", heard != null ? span(t - heard) + " before" : "")}
        ${fact("Position", n.lat != null ? `${n.lat.toFixed(3)}, ${n.lon.toFixed(3)}` : "")}${fact("Antenna height", n.height_m != null ? n.height_m + " m" : "")}${fact("Gain", n.gain_dbi != null ? n.gain_dbi + " dBi" : "")}
        ${fact("Power", n.tx_dbm != null ? n.tx_dbm + " dBm" : "")}${fact("Owner", n.owner)}${fact("Moving", n.mobile ? "yes" : "")}</div>
        ${n.lat != null ? '<button class="chip" id="ov-fly">Show on the map</button>' : ""}
        ${times.length ? `<div class="mute" style="margin-top:6px">Announced: ${times.map((x) => clock(x)).join(", ")}</div>` : ""}</div>`;
    } else $("ov-card").innerHTML = "";
    // The live packet list: addresses only, newest first.
    const now = Date.now();
    const lp = (kind, hops, count, observer, subject, when, id) => {
      const edge = KIND_COLOR[kind] || "#94a3b8";
      return `<div class="lp" data-g="${esc(id || "")}" ${subject ? `data-addr="${esc(subject)}"` : ""} style="--edge:${edge}"><span class="k" style="background:${edge}">${esc(kind)}</span>${hops != null ? `<span class="hb">${hops}→</span>` : ""}${count != null ? `<span class="rpt" title="observers that heard it">${count}</span>` : ""}
        <span class="mono">${esc(shortAddr(observer))}</span><span class="mono grow mute">${subject ? "→ " + esc(shortAddr(subject)) : ""}</span><time>${when}</time></div>`;
    };
    const rows = S.view == null
      ? S.packets.slice(0, 60).map((g) => { const o = g.observations || [], f = o[0] || {}; return lp(g.kind, f.hops, g.observer_count ?? 1, f.observer, o.find((x) => x.subject)?.subject, f.time ? span(now - f.time) + " ago" : "", g.id); }).join("")
      : H.announces.filter((a) => a.t <= t).slice(-60).reverse().map((a) => lp("Announce", null, null, null, a.addr, span(t - a.t) + " before")).join("");
    const types = {};
    for (let k = minuteOf(t) - 9 * MIN; k <= minuteOf(t); k += MIN) { const b = H.buckets.get(k); if (b) for (const [kind, c] of Object.entries(b.types || {})) types[kind] = (types[kind] || 0) + c; }
    const entries = Object.entries(types).sort((a, b) => b[1] - a[1]), max = Math.max(1, ...entries.map((e) => e[1]));
    const secs = Array.from({ length: 30 }, (_, i) => { const b = H.buckets.get(minuteOf(t) - (29 - i) * MIN); return b ? sum(b.rx) + sum(b.tx) : 0; }), peak = Math.max(1, ...secs);
    const LEGEND = { Announce: "Node identity", Data: "Direct message", Channel: "Channel message", Ack: "Acknowledgement", PathRequest: "Route search", PathResponse: "Route answer", Fragment: "Part of a file", Probe: "Trace route", Hello: "Neighbour" };
    $("ov-now").innerHTML = `<h4>Live packets <span class="mute">${S.view == null ? "" : "· announces up to " + clock(t)}</span></h4><div class="lp-list">${rows || '<div class="item mute">No packets in the last two minutes.</div>'}</div>
      <h4>Packets per minute · 30 min</h4><div class="spark">${secs.map((v) => `<i style="height:${Math.max(2, Math.round(v / peak * 100))}%"></i>`).join("")}</div>
      <h4>Packet types · 10 min</h4>${entries.map(([k, c]) => `<div class="barline"><span class="k" style="background:${KIND_COLOR[k] || "#94a3b8"};min-width:78px;text-align:center">${esc(k)}</span><i style="width:${Math.round(c / max * 100)}px;background:${KIND_COLOR[k] || "#94a3b8"}"></i><span>${c}</span></div>`).join("") || '<div class="item mute">No packets counted in this period.</div>'}
      <h4>Packet types</h4><div class="legend">${Object.entries(LEGEND).map(([k, d]) => `<span><i style="background:${KIND_COLOR[k]}"></i>${esc(k)} — ${esc(d)}</span>`).join("")}</div>
      <h4>Node roles</h4><div class="legend">${Object.entries(ROLE_COLOR).map(([k, c]) => `<span><i style="background:${c};border-radius:50%"></i>${esc(k)}</span>`).join("")}</div>
      ${H.note && !H.server ? `<h4>History</h4><div class="card mute">${esc(H.note)}</div>` : ""}`;
  }

  // ---------- packets window ----------
  function renderPackets() {
    if (S.win !== "packets") return;
    const body = $("pk-body");
    const kinds = new Set(S.packets.map((g) => g.kind));
    const sel = $("pk-type");
    if (sel.options.length - 1 !== kinds.size) sel.innerHTML = '<option value="">All types</option>' + [...kinds].sort().map((k) => `<option ${k === S.pkt.type ? "selected" : ""}>${esc(k)}</option>`).join("");
    if (S.view != null) {
      const t = at(), rows = [];
      for (let k = minuteOf(t); k >= minuteOf(t) - 60 * MIN; k -= MIN) {
        const b = H.buckets.get(k); if (!b) continue;
        rows.push(`<tr class="g" style="--edge:var(--acc)"><td>${clock(k)}</td><td>${sum(b.rx) + sum(b.tx)}</td><td>${TRANSPORTS.map((x) => `${x} ${b.rx[x] || 0}↓ ${b.tx[x] || 0}↑`).join(" · ")}</td>
          <td>${Object.entries(b.types || {}).sort((a, c) => c[1] - a[1]).map(([kind, c]) => `<span class="k" style="background:${KIND_COLOR[kind] || "#94a3b8"}">${esc(kind)} ${c}</span>`).join(" ")}</td></tr>`);
      }
      const ann = H.announces.filter((a) => a.t <= t && a.t > t - 60 * MIN).slice(-40).reverse();
      body.innerHTML = `<div class="card mute">History at ${esc(stamp(t))}. The server keeps packet counts per minute and type, not every packet.</div>
        <table><thead><tr><th>Minute</th><th>Packets</th><th>Transports</th><th>Types</th></tr></thead><tbody>${rows.join("") || '<tr><td colspan="4" class="mute">No packets counted in this hour.</td></tr>'}</tbody></table>
        <h4>Announces heard · address only</h4><table><tbody>${ann.map((a) => `<tr><td>${clock(a.t)}</td><td><span class="k" style="background:${KIND_COLOR.Announce}">Announce</span></td><td class="mono">${esc(shortAddr(a.addr))}</td></tr>`).join("") || '<tr><td class="mute">None.</td></tr>'}</tbody></table>`;
      $("pk-count").textContent = "history"; return;
    }
    const q = S.pkt.filter.toLowerCase(), now = Date.now();
    const rows = [];
    let shown = 0;
    for (const g of S.packets) {
      const obs = g.observations || [];
      if (S.pkt.type && g.kind !== S.pkt.type) continue;
      if (S.pkt.transport && !obs.some((o) => o.transport === S.pkt.transport)) continue;
      const first = obs[0] || {}, subject = obs.find((o) => o.subject)?.subject;
      if (q && !`${g.id} ${g.kind} ${subject || ""} ${obs.map((o) => o.observer).join(" ")}`.toLowerCase().includes(q)) continue;
      const edge = KIND_COLOR[g.kind] || "#94a3b8";
      const cells = (o, many) => `<td>${o.time ? span(now - o.time) + " ago" : ""}</td><td class="mono">${esc(String(g.id).slice(0, 10))}</td><td>${o.hops ?? ""}</td><td><span class="k" style="background:${edge}">${esc(g.kind)}</span></td>
        <td>${esc(o.transport || "")} ${o.direction === "tx" ? "↑" : "↓"}</td><td class="mono">${esc(shortAddr(o.observer))}${many ? ` <span class="mute">+${many}</span>` : ""}</td><td><span class="rpt" title="observers">${g.observer_count ?? 1}</span></td>
        <td class="mono">${subject ? "→ " + esc(shortAddr(subject)) : ""}${o.snr != null ? ` <span class="mute">${o.snr} dB</span>` : ""}</td>`;
      if (S.pkt.grouped) {
        const open = S.pkt.open.has(g.id);
        rows.push(`<tr class="g" data-g="${esc(g.id)}" style="--edge:${edge}">${cells(first, Math.max(0, new Set(obs.map((o) => o.observer)).size - 1))}</tr>`);
        if (open) for (const o of obs) rows.push(`<tr class="sub"><td>${o.time ? clock(o.time) : ""}</td><td></td><td>${o.hops ?? ""}</td><td>${esc(o.direction === "tx" ? "sent" : "received")}</td><td>${esc(o.transport || "")}</td><td class="mono">${esc(shortAddr(o.observer))}</td><td></td><td>${o.snr != null ? o.snr + " dB" : ""}</td></tr>`);
        shown++;
      } else {
        for (const o of obs) { rows.push(`<tr class="g" style="--edge:${edge}">${cells(o, 0)}</tr>`); shown++; }
      }
      if (shown >= 300) break;
    }
    body.innerHTML = `<table><thead><tr><th>Time</th><th>Hash</th><th>Hops</th><th>Type</th><th>Transport</th><th>Observer</th><th>Rpt</th><th>Details</th></tr></thead><tbody>${rows.join("") || '<tr><td colspan="8" class="mute" style="padding:14px">No packets in the last two minutes. Servers share these when "Share live traffic" is on in their Settings.</td></tr>'}</tbody></table>`;
    $("pk-count").textContent = "(" + shown + ")";
  }

  // ---------- channels window ----------
  function renderChannels() {
    if (S.win !== "channels") return;
    const t = at(), per = new Map();
    for (const name of S.channels) per.set(name, []);
    for (const m of H.messages) if (m.time <= t) { if (!per.has(m.channel)) per.set(m.channel, []); per.get(m.channel).push(m); }
    for (const l of per.values()) l.sort((a, b) => a.time - b.time);
    if (!per.has(S.chan)) S.chan = per.has("public") ? "public" : [...per.keys()][0] || "public";
    const order = [...per.entries()].sort((a, b) => (b[1].at(-1)?.time || 0) - (a[1].at(-1)?.time || 0));
    const sender = (m) => { const n = byPrefix(m.sender); return n && n.name ? n.name : m.sender || "?"; };
    $("ch-list").innerHTML = `<h4>Channels · ${order.length}</h4>` + order.map(([name, list]) => { const last = list.at(-1); return `<div class="item ${name === S.chan ? "on" : ""}" data-chan="${esc(name)}">
      <span class="ava" style="background:hsl(${hue(name)} 55% 42%)">${esc(name.replace(/^#/, "").slice(0, 2).toUpperCase())}</span>
      <span class="grow"><b>${esc(name)}</b><br><span class="mute">${last ? esc(sender(last) + ": " + last.text) : "no messages yet"}</span></span><time>${last ? span(t - last.time) : ""}</time></div>`; }).join("");
    const list = per.get(S.chan) || [];
    $("ch-title").textContent = `${S.chan} — ${list.length} message${list.length === 1 ? "" : "s"}`;
    const sig = S.chan + "|" + list.length + "|" + (list.at(-1)?.time || 0);
    if (sig === S.chanSig) return;
    S.chanSig = sig;
    const box = $("ch-msgs"), stick = box.scrollHeight - box.scrollTop - box.clientHeight < 60 || !box.dataset.ready;
    let day = "";
    box.innerHTML = list.map((m) => {
      const d = new Date(m.time).toDateString(), sep = d !== day ? `<div class="day">${esc(new Date(m.time).toLocaleDateString([], { weekday: "long", day: "numeric", month: "long" }))}</div>` : ""; day = d;
      const who = sender(m);
      return `${sep}<div class="bub"><span class="ava" style="background:hsl(${hue(m.sender)} 55% 42%)">${esc(String(who).slice(0, 2).toUpperCase())}</span><div><div class="who" style="color:hsl(${hue(m.sender)} 70% 60%)">${esc(who)} <span class="mute mono" style="font-weight:400">${esc(m.sender)}</span></div><div class="txt">${esc(m.text)}</div><div class="meta">${clock(m.time)} · ${esc(m.channel)}</div></div></div>`;
    }).join("") || '<div class="mute" style="padding:20px">No messages yet. Only open channels (public and #hashtags) are shown; direct messages are encrypted.</div>';
    box.dataset.ready = "1";
    if (stick) box.scrollTop = box.scrollHeight;
  }

  // ---------- history bar ----------
  function drawGraph() {
    const cv = $("tl-graph"); if (!cv || tl.classList.contains("hidden")) return;
    const w = cv.clientWidth || 600, h = 44;
    if (cv.width !== w) cv.width = w;
    const c = cv.getContext("2d"), end = Date.now(), start = end - S.range, css = getComputedStyle(document.documentElement);
    c.clearRect(0, 0, w, h);
    let peak = 1; for (const b of H.buckets.values()) if (b.t >= start) peak = Math.max(peak, sum(b.rx) + sum(b.tx));
    c.fillStyle = css.getPropertyValue("--acc").trim() || "#5b8def";
    const bw = Math.max(1, w * MIN / S.range);
    for (const b of H.buckets.values()) { if (b.t < start) continue; const x = (b.t - start) / S.range * w, bh = Math.max(1, (sum(b.rx) + sum(b.tx)) / peak * (h - 12)); c.globalAlpha = 0.8; c.fillRect(x, h - bh, bw, bh); }
    c.globalAlpha = 1; c.fillStyle = css.getPropertyValue("--mute").trim() || "#888"; c.font = "10px system-ui";
    const step = S.range <= HOUR ? 10 * MIN : S.range <= 6 * HOUR ? HOUR : S.range <= 12 * HOUR ? 2 * HOUR : 6 * HOUR;
    for (let k = Math.ceil(start / step) * step; k < end; k += step) { const x = (k - start) / S.range * w, d = new Date(k); c.fillRect(x, 0, 1, 6); c.fillText(String(d.getHours()).padStart(2, "0") + ":" + String(d.getMinutes()).padStart(2, "0"), x + 3, 10); }
    const x = ((S.view ?? end) - start) / S.range * w;
    c.fillStyle = css.getPropertyValue("--warn").trim() || "#f2875a"; c.fillRect(Math.min(w - 2, Math.max(0, x - 1)), 0, 2, h);
  }
  function renderTl() {
    if (S.mode !== "real") return;
    const t = S.view ?? Date.now();
    $("tl-clock").textContent = new Date(t).toLocaleTimeString([], { hour12: false });
    $("tl-dot").textContent = S.view == null ? "● LIVE" : "● " + stamp(S.view) + " · " + span(Date.now() - S.view) + " ago";
    $("tl-dot").classList.toggle("off", S.view != null);
    $("tl-live").setAttribute("aria-pressed", String(S.view == null));
    $("tl-play").textContent = S.play ? "❚❚" : "▶";
    for (const b of document.querySelectorAll("#tl-range [data-range]")) b.setAttribute("aria-pressed", String(+b.dataset.range === S.range));
    drawGraph();
  }
  function setView(ms) {
    const now = Date.now();
    S.view = ms == null || ms >= now - 5000 ? null : Math.max(now - DAY, ms);
    if (S.view == null && S.play) { clearInterval(S.play); S.play = null; }
    S.chanSig = ""; drawReal(); render();
  }
  function play() {
    if (S.play) { clearInterval(S.play); S.play = null; renderTl(); return; }
    if (S.view == null) setView(Date.now() - Math.min(S.range, 2 * HOUR));
    S.play = setInterval(() => { if (S.view == null) { clearInterval(S.play); S.play = null; renderTl(); return; } setView(S.view + S.speed * 250); }, 250);
    renderTl();
  }
  function pickTime(ev) {
    const cv = $("tl-graph"), r = cv.getBoundingClientRect(), f = Math.min(1, Math.max(0, (ev.clientX - r.left) / r.width));
    setView(Date.now() - S.range + f * S.range);
  }
  // A ring on the map where a packet was heard from: the announced node, else the observer.
  const calm = matchMedia("(prefers-reduced-motion: reduce)");
  let pulses = 0;
  function pulseFor(g) {
    if (calm.matches || S.mode === "sim" || S.view != null || pulses > 14) return;
    const o = g.observations || [], subject = o.find((x) => x.subject)?.subject;
    const n = (subject && S.nodes.get(subject)) || S.nodes.get(o[0]?.observer);
    if (!n || n.lat == null) return;
    const color = KIND_COLOR[g.kind] || "#94a3b8", ring = L.circleMarker([n.lat, n.lon], { radius: 4, color, weight: 2, fillOpacity: 0.15, interactive: false }).addTo(map), t0 = performance.now();
    pulses++;
    const step = (t) => { const f = (t - t0) / 1400; if (f >= 1) { map.removeLayer(ring); pulses--; return; } ring.setRadius(4 + 26 * f); ring.setStyle({ opacity: 1 - f, fillOpacity: 0.15 * (1 - f) }); requestAnimationFrame(step); };
    requestAnimationFrame(step);
  }

  // ---------- rendering ----------
  function render() {
    if (!S.open) return;
    const active = document.activeElement, typing = active && ["ov-search", "pk-filter"].includes(active.id);
    renderFigs();
    renderNodes(); renderSide(); renderPackets(); renderChannels(); renderTl();
    for (const b of document.querySelectorAll("#ov-roles [data-role]")) b.setAttribute("aria-pressed", String(S.roles.has(b.dataset.role)));
    for (const b of document.querySelectorAll("#ov-wins [data-win]")) b.setAttribute("aria-pressed", String(S.win === b.dataset.win));
    if (typing) active.focus();
  }

  // ---------- map: real nodes and layers ----------
  let hiddenSim = [], halo = null;
  const realLayer = L.layerGroup();
  const simLayers = () => ["nodes", "links", "clutter", "disaster"].map((k) => dk.layers && dk.layers[k]).filter(Boolean);
  function drawReal() {
    realLayer.clearLayers();
    const t = at();
    for (const n of S.nodes.values()) {
      if (n.lat == null || n.lon == null) continue;
      const heard = lastHeard(n.addr), color = ROLE_COLOR[roleOf(n)] || "#94a3b8", fresh = heard != null && t - heard < HOUR;
      L.circleMarker([n.lat, n.lon], { radius: n.host ? 9 : 7, color, weight: 2, fillColor: color, fillOpacity: fresh ? 0.85 : 0.25, opacity: heard == null ? 0.3 : 1 })
        .bindTooltip(`${esc(n.name || shortAddr(n.addr))} · ${esc(roleOf(n))}${heard != null ? " · " + span(t - heard) : ""}`)
        .on("click", () => select(n.addr)).addTo(realLayer);
    }
  }
  function applyLayers() {
    if (S.mode !== "real") { for (const l of hiddenSim) if (!map.hasLayer(l)) map.addLayer(l); hiddenSim = []; }
    else for (const l of simLayers()) if (map.hasLayer(l)) { if (!hiddenSim.includes(l)) hiddenSim.push(l); map.removeLayer(l); }
    if (S.mode === "sim") { if (map.hasLayer(realLayer)) map.removeLayer(realLayer); } else if (!map.hasLayer(realLayer)) realLayer.addTo(map);
  }
  function select(addr) {
    S.sel = addr || null;
    if (halo) { map.removeLayer(halo); halo = null; }
    const n = addr && S.nodes.get(addr);
    if (n && n.lat != null) halo = L.circleMarker([n.lat, n.lon], { radius: 16, color: "#5b8def", weight: 3, fillOpacity: 0.08, interactive: false }).addTo(map);
    render();
  }

  // ---------- modes and windows ----------
  function showWin(name) {
    S.win = S.win === name ? null : name;
    pktWin.classList.toggle("hidden", S.win !== "packets");
    chanWin.classList.toggle("hidden", S.win !== "channels");
    S.chanSig = ""; render();
  }
  function startPanels() {
    if (S.open) return;
    S.open = true; S.ready = false;
    $("rw-card")?.classList.add("hidden");
    for (const el of [nodesPanel, side]) el.classList.remove("hidden");
    for (const id of ["ov-figs", "ov-wins", "ov-acts"]) $(id).classList.remove("hidden");
    loop(loadMap, 10000); loop(loadNetwork, 2000); loop(loadTraffic, 1000); loop(loadHistory, 30000);
    render();
  }
  function stopPanels() {
    S.open = false;
    S.timers.forEach(clearTimeout); S.timers = [];
    if (S.play) { clearInterval(S.play); S.play = null; }
    S.view = null; S.win = null;
    for (const el of [nodesPanel, side, pktWin, chanWin]) el.classList.add("hidden");
    for (const id of ["ov-figs", "ov-wins", "ov-acts"]) $(id).classList.add("hidden");
    select(null);
  }
  let cvHome = null;
  function coverageCardTo(real) {
    const cv = $("cv-card");
    if (!cv) return;
    if (real) { if (cv.parentElement !== wrap) { cvHome = cv.parentElement; wrap.appendChild(cv); } }
    else if (cvHome && cv.parentElement === wrap) { cvHome.appendChild(cv); cvHome = null; }
    if (!real) cv.classList.add("hidden");
  }
  function setMode(next) {
    S.mode = next === "real" ? "real" : "sim";
    const real = S.mode === "real";
    for (const b of row.querySelectorAll("[data-mode]")) b.setAttribute("aria-pressed", String(b.dataset.mode === S.mode));
    wrap.classList.toggle("ov-docked", real);
    wrap.classList.toggle("real-only", real);
    document.body.classList.toggle("real-view", real);
    coverageCardTo(real);
    tl.classList.toggle("hidden", !real);
    if (real) { startPanels(); window.DKRealWorld && DKRealWorld.start(); }
    else { stopPanels(); window.DKRealWorld && DKRealWorld.stop(); }
    drawReal(); applyLayers();
    setTimeout(() => { map.invalidateSize(); render(); }, 60);
  }
  const open = () => setMode("real"), close = () => setMode("sim");

  // ---------- events ----------
  row.addEventListener("click", (e) => {
    const m = e.target.closest("[data-mode]"); if (m) { setMode(m.dataset.mode); return; }
    const w = e.target.closest("[data-win]"); if (w) { showWin(w.dataset.win); return; }
    const a = e.target.closest("[data-act]");
    if (a && window.DKRealWorld) {
      $("ov-sound").classList.add("hidden");
      if (a.dataset.act === "servers") { $("cv-card")?.classList.add("hidden"); DKRealWorld.toggleCard(); }
      else { DKRealWorld.hideCard(); coverageCardTo(true); DKRealWorld.start(); DKRealWorld.coverage(); }
      return;
    }
    if (e.target.id === "ov-snd") { $("ov-sound").classList.toggle("hidden"); if (!$("ov-sound").classList.contains("hidden") && window.DKRealWorld) { DKRealWorld.hideCard(); $("cv-card")?.classList.add("hidden"); } }
    if (e.target.id === "ov-snd-test") beep("public", true);
  });
  row.addEventListener("change", (e) => {
    if (e.target.id === "ov-snd-on") { bib.on = e.target.checked; saveBib(); if (bib.on) beep("public", true); renderFigs(); }
    else if (e.target.dataset.bib) { bib.types[e.target.dataset.bib] = e.target.checked; saveBib(); beep(e.target.dataset.bib, true); }
  });
  row.addEventListener("input", (e) => { if (e.target.id === "ov-vol") { bib.vol = +e.target.value; saveBib(); } });
  $("ov-snd-on").checked = bib.on; $("ov-vol").value = bib.vol;
  row.querySelectorAll("[data-bib]").forEach((el) => { el.checked = !!bib.types[el.dataset.bib]; });
  $("ov-search").oninput = (e) => { S.filter = e.target.value; renderNodes(); };
  $("ov-roles").onclick = (e) => { const b = e.target.closest("[data-role]"); if (!b) return; S.roles.has(b.dataset.role) ? S.roles.delete(b.dataset.role) : S.roles.add(b.dataset.role); render(); };
  nodesPanel.addEventListener("click", (e) => { const it = e.target.closest(".item[data-addr]"); if (it) select(it.dataset.addr); });
  side.addEventListener("click", (e) => {
    const lpRow = e.target.closest(".lp[data-addr]"); if (lpRow && S.nodes.has(lpRow.dataset.addr)) select(lpRow.dataset.addr);
    if (e.target.id === "ov-card-x") select(null);
    if (e.target.id === "ov-fly") { const n = S.nodes.get(S.sel); if (n && n.lat != null) map.flyTo([n.lat, n.lon], Math.max(map.getZoom(), 10)); }
  });
  for (const win of [pktWin, chanWin]) win.addEventListener("click", (e) => { if (e.target.closest("[data-close]")) showWin(S.win); });
  $("pk-filter").oninput = (e) => { S.pkt.filter = e.target.value; renderPackets(); };
  $("pk-type").onchange = (e) => { S.pkt.type = e.target.value; renderPackets(); };
  $("pk-transport").onchange = (e) => { S.pkt.transport = e.target.value; renderPackets(); };
  $("pk-group").onchange = (e) => { S.pkt.grouped = e.target.checked; renderPackets(); };
  $("pk-body").addEventListener("click", (e) => { const tr = e.target.closest("tr[data-g]"); if (!tr) return; const id = tr.dataset.g; S.pkt.open.has(id) ? S.pkt.open.delete(id) : S.pkt.open.add(id); renderPackets(); });
  $("ch-list").addEventListener("click", (e) => { const it = e.target.closest("[data-chan]"); if (it) { S.chan = it.dataset.chan; S.chanSig = ""; renderChannels(); } });
  tl.addEventListener("click", (e) => {
    const st = e.target.closest("[data-step]"); if (st) { setView((S.view ?? Date.now()) + (+st.dataset.step)); return; }
    const rg = e.target.closest("[data-range]"); if (rg) { S.range = +rg.dataset.range; renderTl(); return; }
    if (e.target.id === "tl-play") play();
    if (e.target.id === "tl-live") setView(null);
  });
  $("tl-speed").onchange = (e) => { S.speed = +e.target.value; };
  let dragging = false;
  $("tl-graph").addEventListener("pointerdown", (e) => { dragging = true; e.target.setPointerCapture(e.pointerId); pickTime(e); });
  $("tl-graph").addEventListener("pointermove", (e) => { if (dragging) pickTime(e); });
  $("tl-graph").addEventListener("pointerup", () => { dragging = false; });
  window.addEventListener("dk-servers", () => { if (S.open) { S.nodes.clear(); S.status.clear(); loadMap(); loadHistory(); } });
  window.addEventListener("resize", () => { if (S.open) { map.invalidateSize(); drawGraph(); } });
  document.addEventListener("keydown", (e) => {
    if (S.mode !== "real" || /input|select|textarea/i.test(e.target.tagName)) return;
    if (e.key === "ArrowLeft") setView((S.view ?? Date.now()) - 10 * MIN);
    else if (e.key === "ArrowRight" && S.view != null) setView(S.view + 10 * MIN);
  });

  // One pop-up at a time: when the Servers card, the Coverage card or the Sound menu opens, the others close.
  // One observer sees every change in order, so when two open at once the one opened last stays.
  const popovers = () => [$("rw-card"), $("cv-card"), $("ov-sound")].filter(Boolean);
  const oneAtATime = new MutationObserver((records) => {
    if (S.mode !== "real") return;
    let winner = null;
    for (const rec of records) if (!rec.target.classList.contains("hidden")) winner = rec.target;
    if (winner) for (const other of popovers()) if (other !== winner) other.classList.add("hidden");
  });
  for (const el of popovers()) oneAtATime.observe(el, { attributes: true, attributeFilter: ["class"] });

  window.DKOverview = { open, close, setMode, toggle: () => (S.mode === "sim" ? open() : close()), useHistory: (j) => { useHistory(j); render(); }, setView, state: S, history: H };
  // Opened with ?view=network: wait until every script has run, so the simulation shell has finished moving its cards.
  if (location.hash === "#network" || new URLSearchParams(location.search).get("view") === "network") {
    if (document.readyState === "complete") setMode("real"); else window.addEventListener("load", () => setMode("real"));
  }
})();
