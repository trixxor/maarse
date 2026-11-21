// Map tools for the Denmark simulator: a tool bar on the map (ruler, radio
// path, heights, forest, walkers, alert area) and a right-click menu on an
// empty spot (height there, radio path or ruler from there). The tools reuse
// the side panel's own buttons, so both places do the same thing.
(function () {
  const dk = window.DKSim, T = window.DKTerrain;
  if (!dk || !dk.map) return;
  const map = dk.map;
  const $ = (id) => document.getElementById(id);

  const css = document.createElement("style");
  css.textContent = `
    #mt-bar { top:84px; left:10px; display:flex; flex-direction:column; gap:2px; padding:4px; }
    #mt-bar button { display:flex; align-items:center; gap:8px; border:0; background:transparent; padding:6px 8px; border-radius:8px; font-size:12px; color:var(--ink); text-align:left; min-height:32px; white-space:nowrap; }
    #mt-bar button .ic { width:18px; text-align:center; font-size:15px; }
    #mt-bar button .tx { display:none; } #mt-bar kbd { font:11px ui-monospace,monospace; color:var(--mute); margin-left:6px; }
    #mt-bar:hover button .tx, #mt-bar:focus-within button .tx { display:inline; }
    #mt-bar button:hover, #mt-bar button:focus-visible { background:rgba(127,127,127,.18); outline:0; }
    #mt-bar button.on { background:var(--acc, #2563eb); color:#fff; }
    #mt-bar hr { border:0; border-top:1px solid var(--ovline); margin:2px 4px; }
    .mt-km { background:var(--ov); color:var(--ink); border:1px solid var(--ovline); border-radius:6px; padding:1px 6px; font-size:12px; font-weight:600; box-shadow:0 1px 4px rgba(0,0,0,.2); }
    #mt-search { top:84px; left:54px; width:300px; max-width:calc(100% - 70px); padding:6px; z-index:1100; }
    #mt-search input { width:100%; box-sizing:border-box; padding:6px 8px; font-size:13px; }
    #mt-search ul { list-style:none; margin:4px 0 0; padding:0; max-height:300px; overflow:auto; }
    #mt-search li button { width:100%; text-align:left; border:0; background:transparent; padding:6px 8px; border-radius:6px; font-size:12px; color:var(--ink); }
    #mt-search li button:hover, #mt-search li button:focus, #mt-search li button.sel { background:rgba(127,127,127,.18); outline:0; }
    #mt-search li small { color:var(--mute); display:block; }
    #mt-search .hint { margin:4px 2px 0; }
    @media (max-width: 600px) { #mt-bar { top:auto; bottom:44px; } }`;
  document.head.appendChild(css);

  // One tool bar; each tool has a key (also listed under "?" and in the
  // command palette). Icons: 24 px stroke drawings (window.DKIcons, shell.js
  // reads the same set).
  const I = {
    search: '<circle cx="11" cy="11" r="7"/><path d="m20 20-3.5-3.5"/>',
    ruler: '<path d="M21.3 15.3 8.7 2.7a1 1 0 0 0-1.4 0L2.7 7.3a1 1 0 0 0 0 1.4l12.6 12.6a1 1 0 0 0 1.4 0l4.6-4.6a1 1 0 0 0 0-1.4z"/><path d="m7.5 10.5 2-2M10.5 13.5l2-2M13.5 16.5l2-2"/>',
    path: '<path d="M3 20h18"/><path d="m3 20 5-8 4 4 3-5 6 9"/><path d="M4 5h16" stroke-dasharray="2 3"/>',
    coverage: '<path d="M4.9 16.1a10 10 0 0 1 0-14.2M19.1 1.9a10 10 0 0 1 0 14.2M7.8 13.2a6 6 0 0 1 0-8.4M16.2 4.8a6 6 0 0 1 0 8.4"/><circle cx="12" cy="9" r="2"/><path d="M12 11v10"/>',
    heights: '<path d="m8 3 4 8 5-5 5 15H2L8 3z"/>',
    globe: '<circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3a14 14 0 0 1 0 18M12 3a14 14 0 0 0 0 18"/>',
    area: '<path d="M12 3 4 9l2 10h12l2-10-8-6z"/><circle cx="12" cy="3" r="1.2"/><circle cx="4" cy="9" r="1.2"/><circle cx="20" cy="9" r="1.2"/>',
    walkers: '<circle cx="9" cy="5" r="2"/><path d="M9 7v6l-3 8M9 13l3 8M6 10l3-3 3 3"/><circle cx="17" cy="7" r="1.6"/><path d="M17 9v4l-2 5M17 13l2 5"/>',
    alert: '<path d="M3 11v2a1 1 0 0 0 1 1h3l5 4V6L7 10H4a1 1 0 0 0-1 1z"/><path d="M16 9a3 3 0 0 1 0 6M19 6a7 7 0 0 1 0 12"/>',
    phone: '<rect x="6" y="2" width="12" height="20" rx="2.5"/><path d="M11 18h2"/>',
    layers: '<path d="m12 3 9 5-9 5-9-5 9-5z"/><path d="m3 13 9 5 9-5"/>',
  };
  window.DKIcons = Object.assign(window.DKIcons || {}, I);
  const svg = (k) => `<svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${I[k]}</svg>`;
  const TOOLS = [
    { id: "search", ic: "search", key: "/", tx: "Find a place", run: () => toggleSearch() },
    "-",
    { id: "ruler", ic: "ruler", key: "M", tx: "Measure distance", run: () => toggleRuler() },
    { id: "profile", ic: "path", key: "R", tx: "Radio path", run: () => (mode === "profile" ? T.cancelProfile() || setMode(null) : T.startProfile()) },
    { id: "coverage", ic: "coverage", key: "C", tx: "Coverage", run: () => window.DKCoverage && DKCoverage.start() },
    "-",
    { id: "forest", ic: "area", key: "F", tx: "Forest or town area", run: () => side("terrain", "btn-clutter") },
    { id: "walkers", ic: "walkers", key: "W", tx: "Group of walkers", run: () => side("terrain", "btn-walkers") },
    { id: "alert", ic: "alert", key: "A", tx: "Alert area", run: () => side("alerts", "btn-al-poly") },
    { id: "phone", ic: "phone", key: "P", tx: "Add a phone", run: () => { const b = $("ph-add") || $("ph-toggle"); if (b) b.click(); } },
    "-",
    { id: "realworld", ic: "globe", key: "G", tx: "Real network", run: () => window.DKOverview && DKOverview.toggle() },
    { id: "layers", ic: "layers", key: "L", tx: "Layers", run: () => window.DKShell && DKShell.toggleLayers() },
    // Key only (also in Layers): the height map.
    { id: "heights", ic: "heights", key: "H", tx: "Height map", hidden: true, run: () => { T.showHeights(!T.heightsShown); paint(); } },
  ];
  const bar = document.createElement("div");
  bar.id = "mt-bar"; bar.className = "ov"; bar.setAttribute("role", "toolbar"); bar.setAttribute("aria-label", "Map tools");
  bar.innerHTML = TOOLS.filter((t) => !t.hidden).map((t) => t === "-" ? "<hr>" : `<button data-t="${t.id}" data-tip="${t.tx}" data-key="${t.key}" aria-label="${t.tx}" aria-keyshortcuts="${t.key}">${svg(t.ic)}</button>`).join("");
  $("mapwrap").appendChild(bar);
  L.DomEvent.disableClickPropagation(bar);
  bar.querySelectorAll("button").forEach((b) => b.onclick = () => TOOLS.find((t) => t.id === b.dataset.t).run());

  let mode = null;
  function setMode(m) { mode = m; paint(); }
  function paint() {
    bar.querySelectorAll("button").forEach((b) => b.classList.toggle("on", b.dataset.t === mode || (b.dataset.t === "heights" && T.heightsShown)));
  }
  T.onMode = (m) => setMode(m);
  // Keys for the tools, while no text field has the focus.
  document.addEventListener("keydown", (e) => {
    if (e.ctrlKey || e.metaKey || e.altKey || /INPUT|SELECT|TEXTAREA/.test(document.activeElement.tagName)) return;
    const t = TOOLS.find((x) => x !== "-" && x.key !== "/" && x.key.toLowerCase() === e.key.toLowerCase());
    if (t) { e.preventDefault(); t.run(); }
  });
  window.DKMapTools = { paint, tools: TOOLS.filter((t) => t !== "-") };
  // The side panel holds the form for a tool (sizes, loss); open it and press its button.
  function side(tab, btn) {
    if (mode === "ruler") stopRuler();
    dk.openTab(tab);
    $(btn).click();
  }

  // ---------- ruler ----------
  const rulerLayer = L.layerGroup().addTo(map);
  let pts = [], line = null;
  const fmt = (m) => m >= 1000 ? (m / 1000).toFixed(m >= 10000 ? 1 : 2) + " km" : m.toFixed(0) + " m";
  function total(list) { let d = 0; for (let i = 1; i < list.length; i++) d += map.distance(list[i - 1], list[i]); return d; }
  function drawRuler(extra) {
    const all = extra ? [...pts, extra] : pts;
    if (!line) line = L.polyline([], { color: "#f59e0b", weight: 3, dashArray: "8 6", interactive: false }).addTo(rulerLayer);
    line.setLatLngs(all);
    if (all.length >= 2) line.bindTooltip(fmt(total(all)), { permanent: true, direction: "right", className: "mt-km", offset: [10, 0] }).openTooltip(all[all.length - 1]);
  }
  function toggleRuler(from) {
    if (mode === "ruler") return stopRuler();
    T.cancelProfile();
    rulerLayer.clearLayers(); line = null; pts = [];
    setMode("ruler");
    map.getContainer().style.cursor = "crosshair";
    map.doubleClickZoom.disable();
    if (from) addPoint(from);
  }
  function addPoint(ll) {
    pts.push(ll);
    L.circleMarker(ll, { radius: 4, color: "#f59e0b", fillColor: "#fff", fillOpacity: 1, weight: 2, interactive: false }).addTo(rulerLayer);
    drawRuler();
  }
  function stopRuler() {
    if (mode !== "ruler") return;
    setMode(null);
    map.getContainer().style.cursor = "";
    map.doubleClickZoom.enable();
    if (pts.length < 2) { rulerLayer.clearLayers(); line = null; } else drawRuler();
  }
  map.getContainer().parentElement.addEventListener("click", (ev) => {
    if (mode !== "ruler" || !map.getContainer().contains(ev.target) || ev.target.closest(".leaflet-control")) return;
    ev.stopPropagation(); ev.preventDefault();
    addPoint(map.mouseEventToLatLng(ev));
  }, true);
  map.getContainer().addEventListener("dblclick", () => { if (mode === "ruler") stopRuler(); });
  map.on("mousemove", (e) => { if (mode === "ruler" && pts.length) drawRuler(e.latlng); });
  document.addEventListener("keydown", (e) => {
    if (e.key !== "Escape") return;
    if (mode === "ruler") stopRuler();
    else if (mode === "profile") { T.cancelProfile(); setMode(null); }
    else if (mode === "search") toggleSearch();
  });

  // ---------- address search ----------
  // Photon (komoot) geocoder over OpenStreetMap: addresses, towns, places.
  // Only what is typed in the box is sent, limited to Denmark's box.
  // "56.12, 9.8" (a position) is handled here without a lookup.
  const box = document.createElement("div");
  box.id = "mt-search"; box.className = "ov hidden";
  box.innerHTML = `<input id="mt-q" type="search" placeholder="Address, town or place, or 56.12, 9.80" aria-label="Find an address or place" autocomplete="off"><ul id="mt-res" role="listbox"></ul><p class="hint">Search by Photon / OpenStreetMap; what you type is sent there.</p>`;
  $("mapwrap").appendChild(box);
  L.DomEvent.disableClickPropagation(box); L.DomEvent.disableScrollPropagation(box);
  const foundLayer = L.layerGroup().addTo(map);
  let results = [], qTimer = 0, qSeq = 0;
  function toggleSearch() {
    const show = box.classList.contains("hidden");
    box.classList.toggle("hidden", !show);
    setMode(show ? "search" : null);
    if (show) { $("mt-q").focus(); $("mt-q").select(); }
  }
  const esc = (t) => String(t ?? "").replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
  function label(p) {
    const street = [p.street, p.housenumber].filter(Boolean).join(" ");
    const main = p.name || street || p.city || "?";
    const sub = [p.name && street, [p.postcode, p.city].filter(Boolean).join(" "), p.country !== "Danmark" && p.country !== "Denmark" ? p.country : ""].filter(Boolean).join(", ");
    return { main, sub };
  }
  function showResults(list, note) {
    results = list;
    $("mt-res").innerHTML = list.map((r, k) => `<li><button data-k="${k}" role="option">${esc(r.main)}<small>${esc(r.sub)}</small></button></li>`).join("") || (note ? `<li class="hint">${esc(note)}</li>` : "");
    $("mt-res").querySelectorAll("button").forEach((b) => b.onclick = () => go(results[+b.dataset.k]));
  }
  async function lookup(q) {
    const m = q.match(/^\s*(-?\d+(?:\.\d+)?)\s*[,; ]\s*(-?\d+(?:\.\d+)?)\s*$/);
    if (m) return showResults([{ main: `${(+m[1]).toFixed(5)}, ${(+m[2]).toFixed(5)}`, sub: "position", ll: L.latLng(+m[1], +m[2]) }]);
    if (q.trim().length < 3) return showResults([]);
    const seq = ++qSeq;
    try {
      const url = "https://photon.komoot.io/api/?" + new URLSearchParams({ q, limit: "8", lat: "56.0", lon: "10.5", bbox: "7.5,54.4,15.4,57.9" });
      const r = await fetch(url);
      const j = await r.json();
      if (seq !== qSeq) return;
      showResults((j.features || []).map((f) => Object.assign(label(f.properties), { ll: L.latLng(f.geometry.coordinates[1], f.geometry.coordinates[0]), ext: f.properties.extent })), "Nothing found.");
    } catch (e) { if (seq === qSeq) showResults([], "Search failed (needs internet)."); }
  }
  async function go(r) {
    if (!r) return;
    foundLayer.clearLayers();
    if (r.ext) map.flyToBounds([[r.ext[1], r.ext[0]], [r.ext[3], r.ext[2]]], { maxZoom: 16, duration: 0.8 });
    else map.flyTo(r.ll, 15, { duration: 0.8 });
    // A drawn pin: Leaflet's default marker is an image file this page does not ship.
    const pin = L.divIcon({ className: "", iconSize: [26, 34], iconAnchor: [13, 33], popupAnchor: [0, -30],
      html: '<svg width="26" height="34" viewBox="0 0 26 34" aria-hidden="true"><path d="M13 1C6.4 1 1 6.3 1 12.8 1 22 13 33 13 33s12-11 12-20.2C25 6.3 19.6 1 13 1z" fill="#e11d48" stroke="#fff" stroke-width="2"/><circle cx="13" cy="13" r="4.5" fill="#fff"/></svg>' });
    const mk = L.marker(r.ll, { title: r.main, icon: pin }).addTo(foundLayer);
    let h = "";
    try { h = ` · height ${Math.max(0, await T.heightAt(r.ll.lat, r.ll.lng)).toFixed(0)} m`; } catch { /* offline */ }
    mk.bindPopup(`<b>${esc(r.main)}</b><br><span class="hint">${esc(r.sub)}${h}</span><br><button class="mt-from">Radio path from here</button> <button class="mt-clear">Remove</button>`).openPopup();
    mk.on("popupopen", wire); wire();
    function wire() {
      const el = mk.getPopup().getElement(); if (!el) return;
      el.querySelector(".mt-from").onclick = () => { map.closePopup(); T.startProfile(r.ll); };
      el.querySelector(".mt-clear").onclick = () => foundLayer.clearLayers();
    }
    box.classList.add("hidden"); setMode(null);
  }
  $("mt-q").addEventListener("input", () => { clearTimeout(qTimer); qTimer = setTimeout(() => lookup($("mt-q").value), 300); });
  $("mt-q").addEventListener("keydown", (e) => {
    if (e.key === "Enter") { e.preventDefault(); clearTimeout(qTimer); if (results.length) go(results[0]); else lookup($("mt-q").value).then(() => go(results[0])); }
    else if (e.key === "ArrowDown") { e.preventDefault(); const b = $("mt-res").querySelector("button"); if (b) b.focus(); }
    else if (e.key === "Escape") toggleSearch();
  });
  $("mt-res").addEventListener("keydown", (e) => {
    const bs = [...$("mt-res").querySelectorAll("button")], k = bs.indexOf(document.activeElement);
    if (e.key === "ArrowDown" && k < bs.length - 1) { e.preventDefault(); bs[k + 1].focus(); }
    if (e.key === "ArrowUp") { e.preventDefault(); (k > 0 ? bs[k - 1] : $("mt-q")).focus(); }
  });
  // Ctrl+F / "/" opens the search while the map page has focus.
  document.addEventListener("keydown", (e) => {
    if (/INPUT|SELECT|TEXTAREA/.test(document.activeElement.tagName)) return;
    if (e.key === "/" || ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "f")) { e.preventDefault(); if (box.classList.contains("hidden")) toggleSearch(); else $("mt-q").focus(); }
  });

  // ---------- right-click on an empty spot ----------
  map.on("contextmenu", async (e) => {
    const ll = e.latlng;
    let h = "…";
    try { h = Math.max(0, await T.heightAt(ll.lat, ll.lng)).toFixed(0) + " m"; } catch { h = "unknown (offline)"; }
    const items = [
      { label: "Radio path from here…", run: () => T.startProfile(ll) },
      { label: "Coverage from here", run: () => window.DKCoverage && DKCoverage.start(ll) },
      { label: "Measure from here…", run: () => { if (mode === "ruler") stopRuler(); toggleRuler(ll); } },
      { label: T.heightsShown ? "Hide the height map" : "Show the height map", run: () => { T.showHeights(!T.heightsShown); paint(); } },
      { label: "Copy the position", run: () => navigator.clipboard && navigator.clipboard.writeText(`${ll.lat.toFixed(5)}, ${ll.lng.toFixed(5)}`) },
    ];
    if (rulerLayer.getLayers().length && mode !== "ruler") items.push({ label: "Clear the ruler", run: () => { rulerLayer.clearLayers(); line = null; pts = []; } });
    dk.showMenu(e, `Height ${h} · ${ll.lat.toFixed(4)}, ${ll.lng.toFixed(4)}`, items);
  });
})();
