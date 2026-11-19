// Phones: simulated phones on the map, each a floating window bound to a node
// in the simulated world.
//
// No phones at start. "+ Add phone" opens a setup (where in Denmark: a click on
// the map, a town or an existing node; internet and/or a LoRa radio), then
// Start boots it: a new spot becomes a new node (DKSim.addNode: appended to the
// scenario, engine rebuilt), the node announces itself, and the phone shows
// what its router hears. Up to 10 phones; each window can be dragged, docked to
// the tray, minimised to a chip or closed.
//
// Each phone runs either the Android app replica (phone-app/, default) or the
// real web client (client.html, a copy of maarse-client/index.html). Both talk
// the companion protocol: their frames go to the engine with
// sim_call {"cmd":"phone_cmd"}, and the node's event frames come back from
// {"cmd":"phone_events"} as postMessage {simFrames, simAlive}.
//
// Uses window.DKSim from app.js: { sim, scenario, name(i), map, addNode(spec, links) }.
(function () {
  "use strict";
  const $ = (id) => document.getElementById(id);
  const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
  const MAX_PHONES = 10;
  const USER_KINDS = ["app", "handheld", "mobile", "host", "relay"];
  const LETTERS = "ABCDEFGHIJ";

  // ---------- the shim that runs inside the web client ----------
  // Kept as a function so it is real code; it is serialised into the iframe.
  function shim(phoneId) {
    const mem = new Map();
    const storage = {
      getItem: (k) => (mem.has(k) ? mem.get(k) : null),
      setItem: (k, v) => { mem.set(k, String(v)); },
      removeItem: (k) => { mem.delete(k); },
      clear: () => mem.clear(),
      key: (i) => [...mem.keys()][i] ?? null,
      get length() { return mem.size; },
    };
    try { Object.defineProperty(window, "localStorage", { value: storage, configurable: true }); } catch (e) { /* keep the real one */ }
    class SimSocket {
      constructor(url) {
        this.url = String(url);
        this.readyState = 0;
        this.binaryType = "arraybuffer";
        if (/\/mc\b/.test(this.url)) {
          // MeshCore bridging lives in maarse-host only; say so and stay closed.
          setTimeout(() => { this.readyState = 3; this.onerror && this.onerror({}); this.onclose && this.onclose({}); }, 0);
          return;
        }
        window.__simSocket = this;
        setTimeout(() => { this.readyState = 1; this.onopen && this.onopen({}); }, 0);
      }
      send(b) {
        if (this.readyState !== 1) return;
        if (typeof b === "string") { parent.postMessage({ simPhone: phoneId, text: b }, "*"); return; }
        const u = b instanceof ArrayBuffer ? new Uint8Array(b) : new Uint8Array(b.buffer, b.byteOffset, b.byteLength);
        parent.postMessage({ simPhone: phoneId, frame: Array.from(u) }, "*");
      }
      close() {
        if (this.readyState === 3) return;
        this.readyState = 3;
        if (window.__simSocket === this) window.__simSocket = null;
        this.onclose && this.onclose({});
      }
    }
    SimSocket.CONNECTING = 0; SimSocket.OPEN = 1; SimSocket.CLOSING = 2; SimSocket.CLOSED = 3;
    window.WebSocket = SimSocket;
    window.addEventListener("message", (e) => {
      const d = e.data;
      if (!d || e.source !== parent) return;
      const s = window.__simSocket;
      if (d.simFrames && s && s.onmessage) for (const f of d.simFrames) s.onmessage({ data: new Uint8Array(f).buffer });
      if (d.simDrop && s) s.close();
    });
  }

  // ---------- state ----------
  let clientHtml = null;
  /** { id, node, client: "app"|"web", frame, win, ready, alive, sent, got, state: "setup"|"on", docked, min, x, y, setup } */
  const phones = [];
  let lastSim = null;
  let picking = null; // the phone whose setup waits for a map click
  let pickMarker = null;

  const dk = () => window.DKSim;
  const sim = () => (dk() ? dk().sim : null);
  const nodes = () => (dk() && dk().scenario ? dk().scenario.nodes : []);
  const candidates = () => nodes().map((n, i) => [n, i]).filter(([n]) => USER_KINDS.includes(n.kind));

  // ---------- layout: tray, chips, add button ----------
  function build() {
    const wrap = $("mapwrap");
    if (!wrap || $("ph-layer")) return;
    const layer = document.createElement("div");
    layer.id = "ph-layer";
    layer.innerHTML = `<button id="ph-add" class="ph-addbtn primary" title="Add a simulated phone (up to ${MAX_PHONES})">+ Add phone</button>
      <div id="ph-tray" class="ph-tray hidden"></div><div id="ph-chips" class="ph-chips"></div>`;
    wrap.appendChild(layer);
    $("ph-add").onclick = addPhone;
    const side = $("ph-side");
    if (side) { const t = side.querySelector("#ph-toggle"); if (t) { t.textContent = "+ Add phone"; t.onclick = addPhone; } }
    fit();
  }

  // A real Android screen (412 x 892 dp, like a Pixel 7) in a 10 px bezel,
  // scaled as a whole to fit the map's height.
  const DEV_H = 892 + 2 * 10;
  function fit() {
    const wrap = $("mapwrap"), layer = $("ph-layer");
    if (!wrap || !layer) return;
    const k = Math.max(0.4, Math.min(0.9, (wrap.clientHeight - 90) / DEV_H));
    layer.style.setProperty("--ph-k", k.toFixed(3));
  }
  window.addEventListener("resize", fit);

  function updateAdd() {
    const b = $("ph-add");
    if (b) { b.disabled = phones.length >= MAX_PHONES; b.textContent = phones.length >= MAX_PHONES ? `${MAX_PHONES} phones (the most)` : "+ Add phone"; }
    $("ph-tray").classList.toggle("hidden", !phones.some((p) => p.docked && !p.min));
  }

  function addPhone() {
    if (phones.length >= MAX_PHONES || !dk()) return;
    const id = [...LETTERS].find((l) => !phones.some((p) => p.id === l));
    const n = phones.length;
    const p = { id, node: -1, client: "app", frame: null, win: null, ready: false, alive: true, sent: 0, got: 0, state: "setup", docked: false, min: false,
      x: 70 + (n % 5) * 40, y: 12 + (n % 5) * 30, setup: { where: "map", ll: null, town: 0, existing: -1, inet: true, lora: false } };
    phones.push(p);
    makeWindow(p);
    updateAdd();
  }

  function makeWindow(p) {
    const w = document.createElement("div");
    w.className = "ph-win";
    w.id = "ph-" + p.id;
    w.style.left = p.x + "px"; w.style.top = p.y + "px";
    w.innerHTML = `<div class="ph-bar" title="Drag to move"><span class="ph-dot"></span><b>Phone ${p.id}</b><span class="ph-what"></span>
      <button class="ph-b" data-a="lose" title="Lose this device; a new one is restored from the recovery phrase after 5 min">↺</button><button class="ph-b" data-a="bt" title="Bluetooth to the radio drops for 60 s (firmware nodes): what arrives meanwhile is lost to the phone">ᛒ</button><button class="ph-b" data-a="dock" title="Dock to the tray / float">⇥</button><button class="ph-b" data-a="min" title="Minimise">–</button><button class="ph-b" data-a="close" title="Close (removes the phone)">×</button></div>
      <div class="ph-body"></div><div class="ph-stat"></div>`;
    $("ph-layer").appendChild(w);
    p.win = w;
    w.querySelector('[data-a="dock"]').onclick = () => dock(p, !p.docked);
    w.querySelector('[data-a="lose"]').onclick = () => {
      const s = sim();
      if (!s || p.node < 0) return;
      const t = +(s.summary().t / 1000).toFixed(1);
      const ev = { t_s: t, type: "device_lost", node: p.node, duration_s: 300 };
      window.DKSim.scenario.events.push(ev);
      s.call({ cmd: "inject", event: ev });
    };
    w.querySelector('[data-a="bt"]').onclick = () => {
      const s = sim();
      if (!s || p.node < 0) return;
      // On the timeline, so Reset replays it.
      const sc = window.DKSim.scenario;
      const t = +(s.summary().t / 1000).toFixed(1);
      const ev = { t_s: t, type: "phone_link_drop", node: p.node, duration_s: 60 };
      sc.events.push(ev);
      s.call({ cmd: "inject", event: ev });
    };
    w.querySelector('[data-a="min"]').onclick = () => minimise(p, true);
    w.querySelector('[data-a="close"]').onclick = () => closePhone(p);
    w.addEventListener("pointerdown", () => raise(p));
    drag(p, w.querySelector(".ph-bar"));
    renderSetup(p);
    raise(p);
  }

  let zTop = 1300;
  function raise(p) { if (!p.docked) p.win.style.zIndex = ++zTop; }

  function drag(p, handle) {
    handle.addEventListener("pointerdown", (e) => {
      if (e.target.closest("button") || p.docked) return;
      const r = p.win.getBoundingClientRect(), wr = $("mapwrap").getBoundingClientRect();
      const dx = e.clientX - r.left, dy = e.clientY - r.top;
      handle.setPointerCapture(e.pointerId);
      const move = (ev) => {
        p.x = Math.max(0, Math.min(wr.width - 60, ev.clientX - wr.left - dx));
        p.y = Math.max(0, Math.min(wr.height - 30, ev.clientY - wr.top - dy));
        p.win.style.left = p.x + "px"; p.win.style.top = p.y + "px";
      };
      const up = () => { handle.removeEventListener("pointermove", move); handle.removeEventListener("pointerup", up); };
      handle.addEventListener("pointermove", move);
      handle.addEventListener("pointerup", up);
    });
  }

  function dock(p, on) {
    p.docked = on;
    if (on) { $("ph-tray").appendChild(p.win); p.win.classList.add("docked"); }
    else { $("ph-layer").appendChild(p.win); p.win.classList.remove("docked"); p.win.style.left = p.x + "px"; p.win.style.top = p.y + "px"; raise(p); }
    updateAdd();
  }

  function minimise(p, on) {
    p.min = on;
    p.win.classList.toggle("hidden", on);
    const chips = $("ph-chips");
    chips.innerHTML = phones.filter((x) => x.min).map((x) => `<button class="ph-chip" data-id="${x.id}" title="Show phone ${x.id}"><span class="ph-dot${x.alive ? "" : " down"}"></span>Phone ${x.id}${x.node >= 0 ? " · " + esc(dk().name(x.node)) : ""}</button>`).join("");
    chips.querySelectorAll(".ph-chip").forEach((c) => (c.onclick = () => minimise(phones.find((x) => x.id === c.dataset.id), false)));
    updateAdd();
  }

  function closePhone(p) {
    const s = sim();
    if (s && p.node >= 0) { try { s.call({ cmd: "phone_detach", node: p.node, screen: LETTERS.indexOf(p.id) + 1 }); } catch (e) { /* old engine */ } }
    if (picking === p) stopPicking();
    p.win.remove();
    phones.splice(phones.indexOf(p), 1);
    minimise(p, false);
    updateAdd();
  }

  // ---------- setup: where, internet, LoRa, which client ----------
  function renderSetup(p) {
    const s = p.setup;
    const towns = (window.DK_TOWNS || []).slice(0, 80);
    const cand = candidates();
    p.win.querySelector(".ph-what").textContent = "setting up";
    p.win.querySelector(".ph-body").innerHTML = `<div class="ph-setup">
      <h4>Where is the phone?</h4>
      <label><input type="radio" name="w${p.id}" value="map" ${s.where === "map" ? "checked" : ""}> A spot on the map (a new node)</label>
      <div class="ph-indent"><button class="ph-pick">${picking === p ? "Click on the map…" : s.ll ? "Pick another spot" : "Click on the map"}</button> <span class="ph-ll">${s.ll ? s.ll.map((v) => v.toFixed(4)).join(", ") : "no spot yet"}</span></div>
      <label><input type="radio" name="w${p.id}" value="town" ${s.where === "town" ? "checked" : ""}> In a town (a new node)</label>
      <div class="ph-indent"><select class="ph-town">${towns.map((t, i) => `<option value="${i}" ${i === s.town ? "selected" : ""}>${esc(t[0])}</option>`).join("")}</select></div>
      <label><input type="radio" name="w${p.id}" value="node" ${s.where === "node" ? "checked" : ""}> An existing node</label>
      <div class="ph-indent"><select class="ph-exist">${cand.length ? cand.map(([n, i]) => `<option value="${i}" ${i === s.existing ? "selected" : ""}>${esc(dk().name(i))} (${n.kind})</option>`).join("") : "<option value=-1>no phone or handheld nodes in this world</option>"}</select></div>
      <h4>How it connects</h4>
      <label class="ph-new"><input type="checkbox" class="ph-inet" ${s.inet ? "checked" : ""}> Internet (mobile data or Wi-Fi: a TCP link to the nearest host with internet)</label>
      <label class="ph-new"><input type="checkbox" class="ph-lora" ${s.lora ? "checked" : ""}> LoRa radio attached (a handheld over Bluetooth)</label>
      <p class="ph-help">${s.where === "node" ? "An existing node keeps how it connects." : !s.inet && !s.lora ? "<b>Neither:</b> the phone can reach nobody until you give it one." : ""}</p>
      <h4>App</h4>
      <label><input type="radio" name="c${p.id}" value="app" ${p.client === "app" ? "checked" : ""}> Android app (as on a phone)</label>
      <label><input type="radio" name="c${p.id}" value="web" ${p.client === "web" ? "checked" : ""}> Web client (maarse-client in a browser)</label>
      <p class="ph-help">Adding a new node rebuilds the simulated world: the clock starts again from 0 with the phone in it. Press play afterwards.</p>
      <button class="ph-start primary">Start</button></div>`;
    const b = p.win.querySelector(".ph-body");
    b.querySelectorAll(`input[name=w${p.id}]`).forEach((r) => (r.onchange = () => { s.where = r.value; renderSetup(p); }));
    b.querySelectorAll(`input[name=c${p.id}]`).forEach((r) => (r.onchange = () => { p.client = r.value; }));
    b.querySelector(".ph-pick").onclick = () => { s.where = "map"; startPicking(p); renderSetup(p); };
    b.querySelector(".ph-town").onchange = (e) => { s.town = +e.target.value; s.where = "town"; renderSetup(p); };
    b.querySelector(".ph-exist").onchange = (e) => { s.existing = +e.target.value; s.where = "node"; renderSetup(p); };
    if (s.existing < 0 && cand.length) s.existing = cand[0][1];
    b.querySelector(".ph-inet").onchange = (e) => { s.inet = e.target.checked; renderSetup(p); };
    b.querySelector(".ph-lora").onchange = (e) => { s.lora = e.target.checked; renderSetup(p); };
    b.querySelector(".ph-start").onclick = () => start(p);
    status(p, "Not started yet");
  }

  function startPicking(p) {
    stopPicking();
    picking = p;
    const map = dk().map;
    map.getContainer().style.cursor = "crosshair";
    map.once("click", (e) => {
      if (picking !== p) return;
      p.setup.ll = [+e.latlng.lat.toFixed(5), +e.latlng.lng.toFixed(5)];
      stopPicking();
      pickMarker = L.circleMarker(p.setup.ll, { radius: 8, color: "#e8930c", weight: 3, fillOpacity: 0.3 }).addTo(map).bindTooltip("Phone " + p.id);
      renderSetup(p);
    });
  }
  function stopPicking() {
    picking = null;
    const map = dk() && dk().map;
    if (map) map.getContainer().style.cursor = "";
    if (pickMarker) { pickMarker.remove(); pickMarker = null; }
  }

  // The nearest node with internet (hosts, servers, or any marked internet).
  function nearestInternet(lat, lon) {
    let best = -1, bd = Infinity;
    nodes().forEach((n, i) => {
      const net = n.internet ?? (n.kind === "host" || n.kind === "server");
      if (!net || n.kind === "app") return;
      const d = (n.lat - lat) ** 2 + ((n.lon - lon) * Math.cos(lat * Math.PI / 180)) ** 2;
      if (d < bd) { bd = d; best = i; }
    });
    return best;
  }

  function start(p) {
    const s = p.setup;
    let node = -1;
    if (s.where === "node") node = s.existing;
    else {
      const ll = s.where === "map" ? s.ll : (() => { const t = window.DK_TOWNS[s.town]; return t ? [t[1] + (Math.random() - 0.5) * 0.01, t[2] + (Math.random() - 0.5) * 0.02] : null; })();
      if (!ll) { status(p, "Click on the map first (or pick a town)."); return; }
      const town = window.DkScenarios && window.DK_TOWNS ? window.DkScenarios.nearestTown(window.DK_TOWNS, ll[0], ll[1]) : ["", 0, 0, 0, ""];
      // A radio makes it a handheld (radio over Bluetooth); internet only is the app kind.
      const spec = { kind: s.lora ? "handheld" : "app", lat: ll[0], lon: ll[1], region: town[4] || "", label: `Phone ${p.id}`, internet: s.inet, radio: s.lora, mobile: false };
      const links = [];
      if (s.inet) {
        const h = nearestInternet(ll[0], ll[1]);
        if (h >= 0) links.push({ a: nodes().length, b: h, kind: "tcp", up: true });
      }
      stopPicking();
      node = dk().addNode(spec, links); // rebuilds the engine
      lastSim = sim();
      // Other phones keep their windows and chats; their node's phone state is new.
      phones.forEach((o) => { if (o !== p && o.state === "on") reset(o); });
    }
    if (node < 0) { status(p, "Pick a node first."); return; }
    p.node = node;
    p.state = "on";
    p.win.querySelector(".ph-what").textContent = "· " + dk().name(node);
    p.win.querySelector(".ph-body").innerHTML = `<div class="ph-fit"><div class="ph-device"><div class="ph-notch"></div><iframe title="Phone ${p.id}" class="ph-screen"></iframe></div></div>`;
    p.frame = p.win.querySelector("iframe");
    load(p);
    // The node tells the world it is there; the phone then shows what it hears.
    try { sim().call({ cmd: "announce", node }); } catch (e) { /* engine without the command */ }
  }

  async function load(p) {
    p.ready = false;
    if (p.node < 0) return;
    const n = nodes()[p.node] || {};
    if (p.client === "app") {
      const q = new URLSearchParams({ phone: p.id, kind: n.kind || "app", node: dk().name(p.node), inet: n.internet ?? n.kind === "app" ? "1" : "0", radio: n.radio ?? n.kind !== "app" ? "1" : "0" });
      p.frame.onload = () => { p.ready = true; status(p, ""); };
      p.frame.removeAttribute("srcdoc");
      p.frame.src = "phone-app/index.html?" + q;
      return;
    }
    if (!clientHtml) {
      try { clientHtml = await (await fetch("client.html", { cache: "no-store" })).text(); }
      catch (e) { status(p, "Could not load client.html: " + e.message); return; }
    }
    const tag = `<script>(${shim.toString()})(${JSON.stringify(p.id)});<\/script>`;
    p.frame.onload = () => {
      try { p.frame.contentWindow.connect("ws"); p.ready = true; status(p, ""); }
      catch (e) { status(p, "The client did not start: " + e.message); }
    };
    p.frame.srcdoc = clientHtml.replace(/<head>/i, "<head>" + tag);
  }

  /** A new engine: the app connects again (keeps its chats); the web client reloads. */
  function reset(p) {
    if (p.node >= nodes().length) { status(p, "Its node is gone from this world. Close the phone and add a new one."); p.ready = false; return; }
    if (p.client === "app" && p.ready) p.frame.contentWindow.postMessage({ simReset: true }, "*");
    else load(p);
  }

  function status(p, text) {
    const el = p.win.querySelector(".ph-stat");
    const n = nodes()[p.node];
    const what = n ? `${n.kind}${n.internet || n.kind === "app" ? " · internet" : ""}${n.radio ?? n.kind !== "app" ? " · LoRa" : ""}` : "";
    el.textContent = text || `${what} · ${p.sent} frames out, ${p.got} in${p.alive ? "" : " · node is down"}${p.link === false ? " · Bluetooth to the radio is DOWN" : ""}`;
    p.win.querySelector(".ph-dot").classList.toggle("down", !p.alive);
  }

  // ---------- frames between the phones and the engine ----------
  window.addEventListener("message", (e) => {
    const d = e.data;
    if (!d || !d.simPhone) return;
    const p = phones.find((x) => x.id === d.simPhone);
    if (!p || !p.frame || e.source !== p.frame.contentWindow) return;
    const s = sim();
    if (!s || p.node < 0) return;
    if (d.text) return; // JSON over the socket is maarse-host only (MeshCore); not simulated
    p.sent++;
    try { s.call({ cmd: "phone_cmd", node: p.node, screen: LETTERS.indexOf(p.id) + 1, frame: d.frame }); }
    catch (err) { status(p, "Engine: " + err.message); }
    pump(p);
  });

  function pump(p) {
    const s = sim();
    if (!s || p.node < 0 || !p.ready || p.state !== "on") return;
    let r;
    try { r = s.call({ cmd: "phone_events", node: p.node, screen: LETTERS.indexOf(p.id) + 1 }); } catch (e) { return; }
    p.link = r.link;
    const alive = r.alive !== false;
    const changed = alive !== p.alive;
    p.alive = alive;
    if ((r.frames && r.frames.length) || changed) {
      p.got += (r.frames || []).length;
      p.frame.contentWindow.postMessage({ simFrames: r.frames || [], simAlive: alive }, "*");
    }
  }

  setInterval(() => {
    const s = sim();
    if (s !== lastSim) {
      // A new engine (reset, preset, edit): the old node state is gone.
      lastSim = s;
      phones.forEach((p) => { if (p.state === "on") reset(p); else renderSetup(p); });
      return;
    }
    phones.forEach((p) => { if (p.state !== "on") return; pump(p); if (p.ready) status(p, ""); });
  }, 200);

  // For the map's node menu (remote admin): the nodes the running phones are on.
  window.DKPhones = { nodes: () => phones.filter((p) => p.state === "on" && p.node >= 0).map((p) => p.node) };
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", build); else build();
})();
