// Phones: the real Må:rse web client (client.html, a copy of maarse-client/index.html)
// runs in an iframe and talks to a node in the simulated world.
//
// The client thinks it is connected to maarse-host over a WebSocket. A small shim,
// injected at the top of the client's <head>, replaces `WebSocket` with a fake that
// carries the companion-protocol frames (length-prefixed, maarse_core::companion)
// to this page with postMessage. This page hands them to the engine
// (sim_call {"cmd":"phone_cmd"}) and polls the node's event frames
// ({"cmd":"phone_events"}) back into the client. Each phone keeps its own
// in-memory localStorage, so two phones do not share contacts.
//
// Uses window.DKSim from app.js: { sim, scenario, name(i), onRebuild }.
(function () {
  "use strict";
  const $ = (id) => document.getElementById(id);
  const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
  const PHONES = ["A", "B"];
  const USER_KINDS = ["app", "handheld", "mobile", "host", "relay"];

  // ---------- the shim that runs inside the client ----------
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
  const phones = PHONES.map((id) => ({ id, node: -1, frame: null, ready: false, alive: true, sent: 0, got: 0 }));
  let lastSim = null;
  let shown = false;

  const dk = () => window.DKSim;
  const sim = () => (dk() ? dk().sim : null);
  const nodes = () => (dk() && dk().scenario ? dk().scenario.nodes : []);

  function candidates() {
    return nodes().map((n, i) => [n, i]).filter(([n]) => USER_KINDS.includes(n.kind));
  }
  function defaultNodes() {
    const c = candidates();
    const apps = c.filter(([n]) => n.kind === "app");
    const hand = c.filter(([n]) => n.kind === "handheld");
    const pool = apps.length >= 2 ? apps : hand.length >= 2 ? hand : c;
    if (pool.length < 2) return pool.map(([, i]) => i);
    // The two far apart, so the message has a journey.
    const [a] = pool[0];
    let best = pool[1], bd = -1;
    for (const p of pool) { const d = (p[0].lat - a.lat) ** 2 + (p[0].lon - a.lon) ** 2; if (d > bd) { bd = d; best = p; } }
    return [pool[0][1], best[1]];
  }

  // ---------- UI ----------
  function build() {
    const wrap = $("mapwrap");
    if (!wrap || $("phones")) return;
    const box = document.createElement("div");
    box.id = "phones";
    box.className = "ph-panel hidden";
    box.innerHTML = `<div class="ph-head"><b>Phones</b><span class="ph-note">The real web client, connected to a simulated node. Keeps running while you break the network.</span><button class="ph-x" title="Hide the phones">×</button></div>
      <div class="ph-row">${phones.map((p) => `<div class="ph-card" id="ph-${p.id}">
        <div class="ph-bar"><span class="ph-dot" title="Node state"></span><span>Phone ${p.id} is node</span><select class="ph-node"></select></div>
        <div class="ph-device"><iframe title="Phone ${p.id}" class="ph-screen"></iframe></div>
        <div class="ph-stat"></div></div>`).join("")}</div>`;
    wrap.appendChild(box);
    box.querySelector(".ph-x").onclick = () => show(false);
    phones.forEach((p) => {
      const card = $("ph-" + p.id);
      p.frame = card.querySelector("iframe");
      card.querySelector(".ph-node").onchange = (e) => bind(p, +e.target.value);
    });
    const side = $("ph-side");
    if (side) side.querySelector("#ph-toggle").onclick = () => show(!shown);
  }

  function show(on) {
    shown = on;
    $("phones").classList.toggle("hidden", !on);
    const t = $("ph-toggle");
    if (t) t.textContent = on ? "Hide the phones" : "Show the phones";
    if (on) { refreshPickers(); phones.forEach((p) => { if (p.node < 0) return; if (!p.frame.srcdoc) load(p); }); }
  }

  function refreshPickers() {
    const opts = candidates();
    const def = defaultNodes();
    phones.forEach((p, k) => {
      if (p.node < 0 || p.node >= nodes().length || !USER_KINDS.includes(nodes()[p.node].kind)) p.node = def[k] ?? -1;
      const sel = $("ph-" + p.id).querySelector(".ph-node");
      sel.innerHTML = opts.map(([, i]) => `<option value="${i}"${i === p.node ? " selected" : ""}>${esc(dk().name(i))}</option>`).join("");
    });
  }

  function bind(p, node) {
    const s = sim();
    if (s && p.node >= 0) { try { s.call({ cmd: "phone_detach", node: p.node }); } catch (e) { /* old engine */ } }
    p.node = node;
    load(p);
  }

  async function load(p) {
    p.ready = false;
    if (p.node < 0) return;
    if (!clientHtml) {
      try { clientHtml = await (await fetch("client.html", { cache: "no-store" })).text(); }
      catch (e) { status(p, "Could not load client.html: " + e.message); return; }
    }
    const tag = `<script>(${shim.toString()})(${JSON.stringify(p.id)});<\/script>`;
    p.frame.onload = () => {
      const w = p.frame.contentWindow;
      try {
        // Same origin (srcdoc): the client's top-level connect() is a global.
        w.connect("ws");
        p.ready = true;
        status(p, "");
      } catch (e) { status(p, "The client did not start: " + e.message); }
    };
    p.frame.srcdoc = clientHtml.replace(/<head>/i, "<head>" + tag);
  }

  function status(p, text) {
    const n = nodes()[p.node];
    const el = $("ph-" + p.id).querySelector(".ph-stat");
    const what = n ? `${n.kind}${n.kind === "app" ? " (no radio, over its host)" : ""}` : "";
    el.textContent = text || `${what} · ${p.sent} frames out, ${p.got} in${p.alive ? "" : " · node is down"}`;
    $("ph-" + p.id).querySelector(".ph-dot").classList.toggle("down", !p.alive);
  }

  // ---------- frames between client and engine ----------
  window.addEventListener("message", (e) => {
    const d = e.data;
    if (!d || !d.simPhone) return;
    const p = phones.find((x) => x.id === d.simPhone);
    if (!p || e.source !== p.frame.contentWindow) return;
    const s = sim();
    if (!s || p.node < 0) return;
    if (d.text) return; // JSON over the socket is maarse-host only (MeshCore); not simulated
    p.sent++;
    try { s.call({ cmd: "phone_cmd", node: p.node, frame: d.frame }); }
    catch (err) { status(p, "Engine: " + err.message); }
    pump(p);
  });

  function pump(p) {
    const s = sim();
    if (!s || p.node < 0 || !p.ready) return;
    let r;
    try { r = s.call({ cmd: "phone_events", node: p.node }); } catch (e) { return; }
    p.alive = r.alive !== false;
    if (r.frames && r.frames.length) {
      p.got += r.frames.length;
      p.frame.contentWindow.postMessage({ simFrames: r.frames }, "*");
    }
  }

  setInterval(() => {
    const s = sim();
    if (s !== lastSim) {
      // A new engine (reset, preset, edit): the old node state is gone.
      lastSim = s;
      if ($("phones")) { refreshPickers(); if (shown) phones.forEach(load); else phones.forEach((p) => { p.frame.srcdoc = ""; p.ready = false; }); }
      return;
    }
    phones.forEach((p) => { pump(p); if (shown && p.ready) status(p, ""); });
  }, 200);

  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", build); else build();
})();
