// The simulator's frame. The map fills the window; everything else waits
// until it is asked for:
// * one floating panel on the left, opened by a mode (Build, Run, Results)
//   and closed again with the same button, ×, or Esc; inside, every topic is
//   a folded row and only one is open at a time;
// * the tools on the map's left edge; Layers (with the legend) behind the
//   last tool;
// * what you select opens in one column on the right (node, message, radio
//   path, coverage, real world);
// * a compact play bar; live packets on demand;
// * Ctrl+K finds any action or setting, so nothing is lost by being folded.
// Every control keeps its id; this file only moves and wraps elements
// (tools/ui_features_test.mjs checks that no control of the old layout is lost).
(function () {
  const dk = window.DKSim, T = window.DKTerrain, C = window.DKCoverage;
  if (!dk || !dk.map) return;
  const map = dk.map;
  const $ = (id) => document.getElementById(id);
  const esc = (t) => String(t ?? "").replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
  const wrap = $("mapwrap");

  // ---------- icons (24 px strokes) ----------
  const ICONS = Object.assign({
    x: '<path d="M6 6l12 12M18 6 6 18"/>',
    send: '<path d="m22 2-7 20-4-9-9-4z"/><path d="M22 2 11 13"/>',
    list: '<path d="M8 6h13M8 12h13M8 18h13"/><circle cx="3.5" cy="6" r="1"/><circle cx="3.5" cy="12" r="1"/><circle cx="3.5" cy="18" r="1"/>',
    bolt: '<path d="M13 2 4 14h7l-1 8 9-12h-7z"/>',
    radio: '<path d="M4.9 19.1a10 10 0 0 1 0-14.2M19.1 4.9a10 10 0 0 1 0 14.2M7.8 16.2a6 6 0 0 1 0-8.4M16.2 7.8a6 6 0 0 1 0 8.4"/><circle cx="12" cy="12" r="2"/>',
    shield: '<path d="M12 3 5 6v6c0 4 3 7.5 7 9 4-1.5 7-5 7-9V6z"/><path d="m9.5 12 2 2 3.5-4"/>',
    key: '<circle cx="8" cy="15" r="4"/><path d="m11 12 9-9M17 6l3 3M15 8l2 2"/>',
    wave: '<path d="M2 12c2-4 4-4 6 0s4 4 6 0 4-4 6 0"/><path d="M2 18c2-4 4-4 6 0s4 4 6 0 4-4 6 0" opacity=".5"/>',
    dice: '<rect x="3" y="3" width="18" height="18" rx="4"/><circle cx="8.5" cy="8.5" r="1.2"/><circle cx="15.5" cy="15.5" r="1.2"/><circle cx="15.5" cy="8.5" r="1.2"/><circle cx="8.5" cy="15.5" r="1.2"/>',
    refresh: '<path d="M21 12a9 9 0 0 1-15.5 6.2L3 16"/><path d="M3 12a9 9 0 0 1 15.5-6.2L21 8"/><path d="M21 3v5h-5M3 21v-5h5"/>',
    clock: '<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>',
    trees: '<path d="M8 3 3 13h10L8 3z"/><path d="M8 13v8"/><path d="m16 7-4 8h8l-4-8z"/><path d="M16 15v6"/>',
    antenna: '<path d="M12 9v12M8 21h8"/><path d="m12 9-5 12M12 9l5 12"/><circle cx="12" cy="7" r="2"/><path d="M7 3.5a7 7 0 0 0 0 7M17 3.5a7 7 0 0 1 0 7"/>',
    sparkle: '<path d="M12 3v4M12 17v4M3 12h4M17 12h4M6 6l2.5 2.5M15.5 15.5 18 18M6 18l2.5-2.5M15.5 8.5 18 6"/>',
    chart: '<path d="M3 3v18h18"/><path d="M7 15l4-4 3 3 5-6"/>',
    compare: '<path d="M8 3v18M16 3v18"/><path d="m4 8 4-4 4 4M12 16l4 4 4-4"/>',
    grid: '<rect x="3" y="3" width="7" height="7" rx="1.5"/><rect x="14" y="3" width="7" height="7" rx="1.5"/><rect x="3" y="14" width="7" height="7" rx="1.5"/><rect x="14" y="14" width="7" height="7" rx="1.5"/>',
    plus: '<circle cx="12" cy="12" r="9"/><path d="M12 8v8M8 12h8"/>',
    gear: '<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z"/>',
    arrows: '<path d="M7 7h13l-4-4M17 17H4l4 4"/>',
    sliders: '<path d="M4 21v-7M4 10V3M12 21v-9M12 8V3M20 21v-5M20 12V3M1 14h6M9 8h6M17 16h6"/>',
    save: '<path d="M5 3h11l5 5v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2z"/><path d="M7 3v6h8V3M7 21v-7h10v7"/>',
    gauge: '<path d="M12 14l4-4"/><path d="M3.3 19a10 10 0 1 1 17.4 0"/>',
    file: '<path d="M14 3H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9z"/><path d="M14 3v6h6M12 18v-6M9 15l3 3 3-3"/>',
    play: '<path d="M7 4l13 8-13 8z"/>',
    moon: '<path d="M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z"/>',
    sun: '<circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"/>',
    help: '<circle cx="12" cy="12" r="9"/><path d="M9.5 9a2.5 2.5 0 1 1 3.5 2.3c-.6.3-1 .9-1 1.6v.6"/>',
    eye: '<path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12z"/><circle cx="12" cy="12" r="3"/>',
    dot: '<circle cx="12" cy="12" r="3"/>',
  }, window.DKIcons || {});
  const svg = (k, s = 18) => `<svg viewBox="0 0 24 24" width="${s}" height="${s}" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${ICONS[k] || ICONS.dot}</svg>`;

  // Short titles, a line about each topic, and its icon (by the old heading).
  const TOPICS = {
    "generate a network": ["Generate a network", "users per region and how they connect", "grid"],
    "devices and people": ["Devices and people", "place nodes, links, cars and walking people", "plus"],
    "protocol features and radio": ["Protocol and radio", "router options, SF, duty cycle, announces", "gear"],
    "traffic": ["Traffic", "message rates and a normal day", "arrows"],
    "rules and radio": ["Rules profiles", "“Standard (as built)” or your own", "sliders"],
    "save, snapshots and links": ["Save, snapshots and links", "in this browser; branch; share", "save"],
    "performance": ["Performance", "limits and the live benchmark", "gauge"],
    "scenario file": ["Scenario file", "export or import JSON", "file"],
    "send one message and watch it": ["Send and follow", "pick two nodes or towns and watch every hop", "send"],
    "any message": ["Any message", "messages from the traffic generator", "list"],
    "single nodes and links": ["Single nodes and links", "kill a node, cut a host's internet", "bolt"],
    "areas (draw a polygon, double-click to finish)": ["Areas", "destroy, radio loss, power cuts", "trees"],
    "radio settings for every node": ["Radio for every node", "switch SF, frequency, bandwidth, power", "radio"],
    "attacks on alerts": ["Attacks on alerts", "forged and replayed alerts", "shield"],
    "lost device, restored from the recovery phrase": ["Lost device", "restored from the recovery phrase", "key"],
    "flood a host over the internet": ["Flood a host", "the guard and the block list", "wave"],
    "internet": ["Internet", "down everywhere, per region, seed DNS", "globe"],
    "random failures": ["Random failures", "kill N repeaters, revive, clear loss", "dice"],
    "reboots": ["Reboots", "random nodes or an area", "refresh"],
    "timeline": ["Timeline", "every event of this run", "clock"],
    "forest and clutter": ["Forest and town areas", "draw areas with a loss per km", "trees"],
    "walkers": ["Walkers", "a group that wanders around a point", "walkers"],
    "forest scenario": ["Forest scenario", "a whole forest test in one click", "trees"],
    "height map": ["Height map", "ground heights on the map", "heights"],
    "antenna heights": ["Antenna heights", "per kind of node", "antenna"],
    "hills in the simulation": ["Hills in the run", "hills block and bend radio links", "heights"],
    "radio path profile": ["Radio paths", "ground, line of sight and Fresnel zone", "path"],
    "precise heights (optional)": ["Precise heights", "your own Dataforsyningen key", "sparkle"],
    "compare runs": ["Compare runs", "the same run with a feature off and on", "compare"],
    "alerts": ["Area alert", "a signed alert to everyone in an area", "alert"],
    "packet log": ["Packet log", "filter and export every packet", "list"],
    "phones": ["Phones", "the real app on simulated nodes", "phone"],
  };
  // Also by the short title, for cards that were already renamed.
  for (const v of Object.values(TOPICS)) TOPICS[v[0].toLowerCase()] = TOPICS[v[0].toLowerCase()] || v;
  const topic = (title) => TOPICS[title.trim().toLowerCase()] || [title.trim(), "", "dot"];
  const MODE_INFO = {
    build: ["Build", "Network, terrain and radio"],
    run: ["Run", "Messages, alerts, events and phones"],
    results: ["Results", "Numbers, comparisons and the log"],
  };

  // ---------- the panel on the left ----------
  const side = $("side");
  const insp = $("inspector");
  wrap.appendChild(side);
  side.classList.add("panel");
  side.setAttribute("aria-label", "Panel");
  const head = document.createElement("div");
  head.className = "panel-head";
  head.innerHTML = `<div><b id="panel-title"></b><small id="panel-sub"></small></div><button class="icon" id="panel-close" aria-label="Close the panel" title="Close (Esc)">${svg("x")}</button>`;
  side.prepend(head);
  L.DomEvent.disableClickPropagation(side); L.DomEvent.disableScrollPropagation(side);

  // Every topic becomes a folded card.
  function card(title, nodes) {
    const [t, sub, ic] = topic(title);
    const d = document.createElement("details");
    d.className = "card";
    d.innerHTML = `<summary><span class="ci">${svg(ic)}</span><span><b>${esc(t)}</b>${sub ? `<small>${esc(sub)}</small>` : ""}</span></summary><div class="card-body"></div>`;
    const body = d.querySelector(".card-body");
    for (const n of nodes) body.appendChild(n);
    return d;
  }
  function sectionize(tab) {
    // Plain wrappers that hold headings (terrain.js) are taken apart first.
    let found = true;
    while (found) {
      found = false;
      for (const ch of [...tab.querySelectorAll(":scope > div")]) {
        if (!ch.id && !ch.className && ch.querySelector(":scope > h3")) { ch.replaceWith(...ch.childNodes); found = true; }
      }
    }
    const kids = [...tab.children];
    if (kids.some((k) => k.matches("details.card"))) {
      for (const d of kids.filter((k) => k.matches("details.card"))) {
        d.open = false;
        const b = d.querySelector("summary b"), ci = d.querySelector("summary .ci");
        if (b) { const [t, sub, ic] = topic(b.textContent); if (ci) ci.innerHTML = svg(ic); b.textContent = t; const sm = d.querySelector("summary small"); if (sm && sub) sm.textContent = sub; }
      }
      return;
    }
    if (!kids.some((k) => k.tagName === "H3")) {
      tab.appendChild(card(tab.dataset.title || "", kids));
      return;
    }
    let cur = null, items = [];
    const flush = () => { if (cur) tab.appendChild(card(cur, items)); items = []; };
    for (const k of kids) {
      if (k.tagName === "H3") { flush(); cur = k.textContent; k.remove(); continue; }
      if (cur) items.push(k);   // before the first heading: stays as it is (e.g. "At t = … for … min")
    }
    flush();
    for (const k of kids) if (k.isConnected && k.parentNode === tab && !k.matches("details.card")) k.classList.add("lead");
  }
  document.querySelectorAll("#side .tab").forEach(sectionize);
  // The phones topic: phone.js keeps its own wrapper; fold it too.
  // One open card at a time in the panel.
  side.addEventListener("toggle", (e) => {
    const d = e.target;
    if (!(d instanceof HTMLDetailsElement) || !d.open || !d.classList.contains("card")) return;
    side.querySelectorAll("details.card[open]").forEach((o) => { if (o !== d) o.open = false; });
    setTimeout(() => d.scrollIntoView({ block: "nearest", behavior: "smooth" }), 30);
  }, true);

  let curMode = null;
  function openPanel(m) {
    curMode = m;
    const [t, sub] = MODE_INFO[m] || [m, ""];
    $("panel-title").textContent = t; $("panel-sub").textContent = sub;
    document.body.classList.add("panel-open");
    syncModeButtons();
    setTimeout(() => map.invalidateSize(), 0);
  }
  function closePanel() {
    document.body.classList.remove("panel-open");
    syncModeButtons();
  }
  function syncModeButtons() {
    const open = document.body.classList.contains("panel-open");
    document.querySelectorAll("#modes button").forEach((b) => b.setAttribute("aria-pressed", String(open && b.dataset.mode === curMode)));
  }
  $("panel-close").onclick = closePanel;
  // A mode's button opens its panel, and closes it again.
  $("modes").addEventListener("click", (e) => {
    const b = e.target.closest("button");
    if (b && b.dataset.mode === curMode && document.body.classList.contains("panel-open")) { e.stopPropagation(); closePanel(); }
  }, true);
  document.addEventListener("dk-mode", (e) => openPanel(e.detail));
  let revealing = false;
  const modeOf = (t) => Object.keys(dk.modes || {}).find((m) => dk.modes[m].includes(t));
  document.addEventListener("dk-tab", (e) => {
    const m = modeOf(e.detail);
    if (m && !(document.body.classList.contains("panel-open") && curMode === m)) openPanel(m);
    if (revealing) return;
    const tab = $("tab-" + e.detail);
    const first = tab && tab.querySelector("details.card");
    if (first && !tab.querySelector("details.card[open]")) first.open = true;
  });
  // Starts closed: the map first.
  curMode = (document.querySelector("#modes button.on") || {}).dataset?.mode || "run";
  closePanel();

  // ---------- the column on the right ----------
  const dock = document.createElement("div");
  dock.id = "dock";
  dock.setAttribute("aria-label", "Selected");
  wrap.appendChild(dock);
  // The node inspector gets a frame with a title and ×; app.js rewrites its
  // inside on every update, so the frame lives outside it.
  const iw = document.createElement("section");
  iw.id = "insp-wrap"; iw.className = "ov";
  iw.innerHTML = `<div class="dock-head"><span>Selected node</span><button class="icon" id="insp-close" aria-label="Close" title="Close">${svg("x")}</button></div>`;
  for (const id of ["card", "inspector", "follow", "watch", "hm-float", "cv-card", "rw-card"]) {
    const el = $(id);
    if (!el) continue;
    if (id === "inspector") { iw.appendChild(el); dock.appendChild(iw); el.classList.remove("ov"); } else dock.appendChild(el);
  }
  $("insp-close").onclick = () => insp.classList.add("hidden");
  new MutationObserver(() => iw.classList.toggle("hidden", insp.classList.contains("hidden"))).observe(insp, { attributes: true, attributeFilter: ["class"] });
  iw.classList.toggle("hidden", insp.classList.contains("hidden"));
  L.DomEvent.disableClickPropagation(dock); L.DomEvent.disableScrollPropagation(dock);
  const busy = () => [...dock.children].some((c) => !c.classList.contains("hidden"));
  const syncDock = () => document.body.classList.toggle("dock-on", busy());
  new MutationObserver(syncDock).observe(dock, { subtree: false, attributes: true, attributeFilter: ["class"], childList: true });
  for (const c of dock.children) new MutationObserver(syncDock).observe(c, { attributes: true, attributeFilter: ["class"] });
  syncDock();

  // ---------- pop-ups (File, help, layers) ----------
  const pops = [];
  function popup(btn, pop, place) {
    const close = () => { pop.classList.add("hidden"); btn.setAttribute("aria-expanded", "false"); btn.classList.remove("on"); };
    const open = () => {
      pops.forEach((p) => p.close());
      pop.classList.remove("hidden");
      place(btn.getBoundingClientRect(), pop);
      btn.setAttribute("aria-expanded", "true"); btn.classList.add("on");
    };
    const toggle = () => (pop.classList.contains("hidden") ? open() : close());
    pops.push({ close, pop });
    document.addEventListener("pointerdown", (e) => { if (!pop.contains(e.target) && !btn.contains(e.target)) close(); });
    return { open, close, toggle };
  }
  const below = (r, p) => { p.style.top = r.bottom + 8 + "px"; p.style.left = Math.max(8, Math.min(r.left, innerWidth - p.offsetWidth - 8)) + "px"; };
  const fileP = popup($("btn-file"), $("file-pop"), below);
  const helpP = popup($("btn-help"), $("help-pop"), (r, p) => { p.style.top = r.bottom + 8 + "px"; p.style.left = Math.max(8, r.right - p.offsetWidth) + "px"; });
  $("btn-file").onclick = (e) => { e.stopPropagation(); fileP.toggle(); };
  $("btn-help").onclick = (e) => { e.stopPropagation(); helpP.toggle(); };
  document.querySelectorAll("#file-pop details").forEach((d) => {
    d.open = false;
    const b = d.querySelector("summary b"), ci = d.querySelector("summary .ci");
    if (b) { const [t, sub, ic] = topic(b.textContent); if (ci) ci.innerHTML = svg(ic); b.textContent = t; const sm = d.querySelector("summary small"); if (sm && sub) sm.textContent = sub; }
  });
  $("file-pop").addEventListener("toggle", (e) => {
    const d = e.target;
    if (d instanceof HTMLDetailsElement && d.open) $("file-pop").querySelectorAll("details[open]").forEach((o) => { if (o !== d) o.open = false; });
  }, true);

  // Layers: the panel and the legend in one pop-up beside the Layers tool.
  const lp = document.createElement("div");
  lp.id = "layers-pop"; lp.className = "pop hidden"; lp.setAttribute("role", "dialog"); lp.setAttribute("aria-label", "Layers");
  document.body.appendChild(lp);
  const layers = $("layers");
  lp.appendChild(layers);
  const legend = $("legend");
  legend.classList.remove("ov");
  lp.appendChild(legend);
  // The legend stays folded in the pop-up until asked for (its own ▸ button).
  const layerBtn = document.querySelector('#mt-bar [data-t="layers"]') || document.createElement("button");
  const layersP = popup(layerBtn, lp, (r, p) => { p.style.left = r.right + 10 + "px"; p.style.top = Math.max(60, Math.min(r.top - 120, innerHeight - p.offsetHeight - 12)) + "px"; });
  window.DKShell = { toggleLayers: () => layersP.toggle(), openPanel, closePanel, palette: () => openPalette() };

  const L2 = dk.layers || {};
  function toggleLayer(l, on) { if (!l) return; if (on) l.addTo(map); else map.removeLayer(l); }
  $("ly-nodes").onchange = (e) => toggleLayer(L2.nodes, e.target.checked);
  $("ly-clutter").onchange = (e) => { toggleLayer(L2.clutter, e.target.checked); toggleLayer(L2.disaster, e.target.checked); };
  $("ly-heights").onchange = (e) => { T && T.showHeights(e.target.checked); window.DKMapTools && DKMapTools.paint(); };
  $("ly-coverage").onchange = (e) => C && C.setVisible(e.target.checked);
  $("ly-paths").onchange = (e) => T && T.setPathsVisible(e.target.checked);
  $("ly-feed").onchange = (e) => { $("feed").classList.toggle("hidden", !e.target.checked); $("feed").classList.remove("min"); feedBtn.setAttribute("aria-pressed", String(e.target.checked)); };
  $("ly-legend").onchange = (e) => $("legend-body").classList.toggle("hidden", !e.target.checked);
  map.on("layeradd layerremove", () => { if (T) $("ly-heights").checked = T.heightsShown; });

  // ---------- status line (what the old panel's note said) ----------
  const toast = document.createElement("div");
  toast.id = "toast"; toast.setAttribute("role", "status");
  wrap.appendChild(toast);
  const info = $("sc-info");
  toast.appendChild(info);
  let toastT = 0;
  new MutationObserver(() => {
    if (!info.textContent.trim()) return;
    toast.classList.add("show");
    clearTimeout(toastT); toastT = setTimeout(() => toast.classList.remove("show"), 7000);
  }).observe(info, { childList: true, characterData: true, subtree: true });

  // ---------- play bar ----------
  const tl = $("tl");
  const feedBtn = document.createElement("button");
  feedBtn.id = "tl-feed"; feedBtn.className = "icon"; feedBtn.title = "Live packets"; feedBtn.setAttribute("aria-label", "Live packets"); feedBtn.setAttribute("aria-pressed", "false");
  feedBtn.innerHTML = svg("list");
  feedBtn.onclick = () => { $("ly-feed").checked = !$("ly-feed").checked; $("ly-feed").onchange({ target: $("ly-feed") }); };
  tl.appendChild(feedBtn);
  $("btn-play").innerHTML = svg("play", 16);
  new MutationObserver(() => {
    const t = $("btn-play").textContent.trim();
    if (t === "▶") $("btn-play").innerHTML = svg("play", 16);
    else if (t === "❚❚") $("btn-play").innerHTML = '<svg viewBox="0 0 24 24" width="16" height="16" fill="currentColor" aria-hidden="true"><rect x="6" y="4" width="4" height="16" rx="1"/><rect x="14" y="4" width="4" height="16" rx="1"/></svg>';
  }).observe($("btn-play"), { childList: true, characterData: true, subtree: true });
  $("ly-feed").onchange({ target: $("ly-feed") });

  // Map controls: zoom at the bottom right, out of the tools' way.
  if (map.zoomControl) map.zoomControl.setPosition("bottomright");

  // Theme button: sun in dark, moon in light.
  const themeIcon = () => {
    const dark = $("theme-name").textContent.trim() === "Dark";
    const s = $("btn-theme").querySelector("svg");
    if (s) s.innerHTML = dark ? ICONS.sun : ICONS.moon;
  };
  new MutationObserver(themeIcon).observe($("theme-name"), { childList: true, characterData: true, subtree: true });
  themeIcon();

  // ---------- command palette (Ctrl+K) ----------
  const pal = document.createElement("div");
  pal.id = "cmdk"; pal.className = "hidden";
  pal.innerHTML = `<div class="cmdk-box" role="dialog" aria-modal="true" aria-label="Search actions">
    <div class="cmdk-in">${svg("search")}<input id="cmdk-q" placeholder="Search actions and settings…" autocomplete="off" aria-label="Search actions and settings" aria-controls="cmdk-list"><kbd>Esc</kbd></div>
    <ul id="cmdk-list" role="listbox"></ul>
    <div class="cmdk-foot"><span><kbd>↑</kbd><kbd>↓</kbd> move</span><span><kbd>Enter</kbd> open</span><span>Everything in the panels, tools and layers</span></div></div>`;
  document.body.appendChild(pal);
  let entries = [], shown = [], sel = 0;
  const clean = (t) => String(t || "").replace(/\s+/g, " ").replace(/[▾▸▴]/g, "").trim();
  function flash(el) { el.classList.remove("flash"); void el.offsetWidth; el.classList.add("flash"); }
  function reveal(d, el) {
    const tab = d.closest(".tab");
    revealing = true;
    if (tab) dk.openTab(tab.id.slice(4));
    else if (d.closest("#file-pop")) fileP.open();
    revealing = false;
    d.open = true;
    setTimeout(() => {
      const target = el || d;
      target.scrollIntoView({ block: "center", behavior: "smooth" });
      flash(target);
      if (el && el.focus) el.focus({ preventScroll: true });
    }, 80);
  }
  function buildIndex() {
    const out = [];
    for (const [m, [t, sub]] of Object.entries(MODE_INFO)) out.push({ t: `${t} panel`, s: sub, ic: "grid", run: () => { const b = document.querySelector(`#modes [data-mode="${m}"]`); if (curMode === m && document.body.classList.contains("panel-open")) return; b.click(); } });
    for (const d of document.querySelectorAll("#side details.card, #file-pop details.card")) {
      const title = clean(d.querySelector("summary b")?.textContent), sub = clean(d.querySelector("summary small")?.textContent);
      const where = d.closest(".tab")?.dataset.title || "File";
      const ic = topic(title)[2];
      out.push({ t: title, s: `${where} · ${sub}`, ic, run: () => reveal(d) });
      for (const el of d.querySelectorAll(".card-body button, .card-body label")) {
        const txt = clean(el.tagName === "LABEL" ? [...el.childNodes].filter((n) => n.nodeType === 3).map((n) => n.textContent).join(" ") : el.textContent);
        if (txt.length < 3 || txt.length > 70 || el.closest("table")) continue;
        out.push({ t: txt, s: `${where} › ${title}`, ic, run: () => reveal(d, el.tagName === "LABEL" ? el.querySelector("input,select,button") || el : el) });
      }
    }
    for (const t of (window.DKMapTools && DKMapTools.tools) || []) out.push({ t: t.tx, s: `Tool · key ${t.key}`, ic: t.ic, run: () => t.run() });
    for (const l of layers.querySelectorAll("label")) {
      const i = l.querySelector("input");
      out.push({ t: `Layer: ${clean(l.textContent)}`, s: i.checked ? "shown · click to hide" : "hidden · click to show", ic: "layers", run: () => { i.click(); } });
    }
    for (const o of $("preset").options) out.push({ t: `Scenario: ${clean(o.textContent)}`, s: "load this scenario", ic: "map", run: () => { $("preset").value = o.value; $("preset").dispatchEvent(new Event("change", { bubbles: true })); } });
    out.push({ t: "Play or pause", s: "Space", ic: "play", run: () => $("btn-play").click() });
    out.push({ t: "Restart the run", s: "from t = 0", ic: "refresh", run: () => $("btn-reset").click() });
    out.push({ t: "Light or dark", s: "theme", ic: "sun", run: () => $("btn-theme").click() });
    out.push({ t: "About and keys", s: "help", ic: "help", run: () => helpP.open() });
    out.push({ t: "Live packets", s: "show or hide", ic: "list", run: () => feedBtn.click() });
    return out;
  }
  function render() {
    const q = $("cmdk-q").value.trim().toLowerCase();
    const words = q.split(/\s+/).filter(Boolean);
    const score = (e) => {
      const t = e.t.toLowerCase(), all = (e.t + " " + e.s).toLowerCase();
      if (!words.every((w) => all.includes(w))) return -1;
      if (!q) return 0;
      return (t.startsWith(q) ? 30 : 0) + (t.includes(q) ? 20 : 0) + words.filter((w) => t.includes(w)).length * 5 - t.length / 40;
    };
    shown = entries.map((e) => [score(e), e]).filter(([s]) => s >= 0).sort((a, b) => b[0] - a[0]).slice(0, 60).map(([, e]) => e);
    sel = Math.min(sel, Math.max(0, shown.length - 1));
    $("cmdk-list").innerHTML = shown.length ? shown.map((e, i) => `<li role="option" id="cmdk-${i}" aria-selected="${i === sel}" data-i="${i}">${svg(e.ic)}<span><b>${esc(e.t)}</b><small>${esc(e.s)}</small></span></li>`).join("")
      : `<li class="empty">Nothing found for “${esc(q)}”.</li>`;
    $("cmdk-q").setAttribute("aria-activedescendant", shown.length ? `cmdk-${sel}` : "");
    const cur = $("cmdk-" + sel); if (cur) cur.scrollIntoView({ block: "nearest" });
  }
  function openPalette() {
    pops.forEach((p) => p.close());
    entries = buildIndex(); sel = 0;
    pal.classList.remove("hidden");
    $("cmdk-q").value = ""; render(); $("cmdk-q").focus();
  }
  function closePalette() { pal.classList.add("hidden"); }
  function choose(i) { const e = shown[i]; if (!e) return; closePalette(); e.run(); }
  $("cmdk-q").addEventListener("input", () => { sel = 0; render(); });
  $("cmdk-q").addEventListener("keydown", (e) => {
    if (e.key === "ArrowDown") { e.preventDefault(); sel = Math.min(shown.length - 1, sel + 1); render(); }
    else if (e.key === "ArrowUp") { e.preventDefault(); sel = Math.max(0, sel - 1); render(); }
    else if (e.key === "Enter") { e.preventDefault(); choose(sel); }
    else if (e.key === "Escape") { e.preventDefault(); closePalette(); }
  });
  $("cmdk-list").addEventListener("click", (e) => { const li = e.target.closest("li[data-i]"); if (li) choose(+li.dataset.i); });
  pal.addEventListener("pointerdown", (e) => { if (e.target === pal) closePalette(); });
  $("btn-cmd").onclick = openPalette;
  document.addEventListener("keydown", (e) => {
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") { e.preventDefault(); if (pal.classList.contains("hidden")) openPalette(); else closePalette(); return; }
    if (e.key === "Escape" && !pal.classList.contains("hidden")) return;
    if (e.key === "Escape") {
      if (pops.some((p) => !p.pop.classList.contains("hidden"))) { pops.forEach((p) => p.close()); return; }
      if (document.body.classList.contains("panel-open") && !/INPUT|SELECT|TEXTAREA/.test(document.activeElement.tagName)) closePanel();
    }
  });

  setTimeout(() => map.invalidateSize(), 0);
})();
