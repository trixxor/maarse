// Saving runs in the browser, snapshots, branches and share links.
//
// A saved run or snapshot is the scenario (nodes, links, features, traffic
// and every timeline event you added) plus the simulated time. The engine is
// deterministic for a seed, so opening a snapshot rebuilds the scenario and
// runs it again to that time: the same world, exactly (it costs CPU time).
// Everything stays in this browser (IndexedDB); nothing goes to a server.
(function () {
  "use strict";
  const $ = (id) => document.getElementById(id);
  const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));
  const fmt = (ms) => { const s = Math.round(ms / 1000), m = Math.floor(s / 60); return `${m}:${String(s % 60).padStart(2, "0")}`; };

  let dbp = null;
  function db() {
    if (dbp) return dbp;
    dbp = new Promise((res, rej) => {
      let r;
      try { r = indexedDB.open("maarse-denmark", 1); } catch (e) { rej(e); return; }
      r.onupgradeneeded = () => r.result.createObjectStore("runs", { keyPath: "id", autoIncrement: true });
      r.onsuccess = () => res(r.result);
      r.onerror = () => rej(r.error);
    });
    return dbp;
  }
  async function write(fn) {
    const d = await db();
    return new Promise((res, rej) => {
      const t = d.transaction("runs", "readwrite");
      fn(t.objectStore("runs"));
      t.oncomplete = () => res();
      t.onerror = () => rej(t.error);
    });
  }
  async function allRuns() {
    const d = await db();
    return new Promise((res, rej) => {
      const out = [];
      const c = d.transaction("runs").objectStore("runs").openCursor();
      c.onsuccess = () => { const k = c.result; if (k) { out.push(k.value); k.continue(); } else res(out); };
      c.onerror = () => rej(c.error);
    });
  }

  function current(kind) {
    const S = window.DkSim;
    const sc = JSON.parse(JSON.stringify(S.scenario()));
    const t = S.sim() ? S.sim().summary().t : 0;
    return { kind, name: `${sc.name || "Scenario"} at ${fmt(t)}`, saved: Date.now(), t_ms: t, scenario: sc };
  }
  async function save(kind) {
    try {
      const run = current(kind);
      run.name = ($("st-name").value || "").trim() || run.name;
      await write((s) => s.put(run));
      $("st-name").value = "";
      note(`${kind === "snapshot" ? "Snapshot" : "Run"} saved: ${run.name}.`);
      render();
    } catch (e) { note("Could not save in this browser (" + e.message + "). Use Export JSON instead."); }
  }
  function open(run, branch) {
    const S = window.DkSim;
    const sc = JSON.parse(JSON.stringify(run.scenario));
    if (branch) sc.name = (sc.name || "Scenario") + " (branch)";
    S.load(sc);
    if (run.t_ms > 0) {
      note(`Running to ${fmt(run.t_ms)}…`);
      setTimeout(() => { S.advance(run.t_ms); note(`${branch ? "Branch from" : "Opened"} ${run.name}: at ${fmt(run.t_ms)}. Change something and play.`); }, 30);
    } else note(`Opened ${run.name}.`);
  }
  function note(t) { $("st-note").textContent = t; }

  async function render() {
    let runs = [];
    try { runs = await allRuns(); } catch (e) { $("st-list").innerHTML = `<p class="hint">Saving is not available in this browser window.</p>`; return; }
    runs.sort((a, b) => b.saved - a.saved);
    $("st-list").innerHTML = runs.length ? `<table>${runs.map(r => `<tr><td>${r.kind === "snapshot" ? "◷" : "▣"} ${esc(r.name)}<div class="hint" style="margin:0">${r.scenario.nodes.length} nodes · ${fmt(r.t_ms)} · ${new Date(r.saved).toLocaleString()}</div></td>
      <td style="white-space:nowrap"><button data-open="${r.id}">Open</button>${r.t_ms > 0 ? ` <button data-branch="${r.id}" title="Open at that time as a new branch">Branch</button>` : ""} <button data-del="${r.id}" aria-label="Delete ${esc(r.name)}">✕</button></td></tr>`).join("")}</table>` : `<p class="hint">Nothing saved yet.</p>`;
    $("st-list").querySelectorAll("[data-open]").forEach(b => b.onclick = () => open(runs.find(r => r.id === +b.dataset.open), false));
    $("st-list").querySelectorAll("[data-branch]").forEach(b => b.onclick = () => open(runs.find(r => r.id === +b.dataset.branch), true));
    $("st-list").querySelectorAll("[data-del]").forEach(b => b.onclick = async () => { await write((s) => s.delete(+b.dataset.del)); render(); });
  }

  // Share link: the scenario, deflated and base64url-encoded in the URL hash.
  async function deflate(text) {
    const cs = new Blob([text]).stream().pipeThrough(new CompressionStream("deflate-raw"));
    const buf = new Uint8Array(await new Response(cs).arrayBuffer());
    let bin = "";
    for (let i = 0; i < buf.length; i += 0x8000) bin += String.fromCharCode.apply(null, buf.subarray(i, i + 0x8000));
    return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  }
  async function inflate(b64) {
    const bin = atob(b64.replace(/-/g, "+").replace(/_/g, "/"));
    const buf = Uint8Array.from(bin, (c) => c.charCodeAt(0));
    const ds = new Blob([buf]).stream().pipeThrough(new DecompressionStream("deflate-raw"));
    return new Response(ds).text();
  }
  async function shareLink() {
    const run = current("link");
    const code = await deflate(JSON.stringify({ t: run.t_ms, sc: run.scenario }));
    const url = location.origin + location.pathname + "#run=" + code;
    $("st-link").value = url;
    $("st-link").classList.remove("hidden");
    try { await navigator.clipboard.writeText(url); note(`Link copied (${Math.round(url.length / 1024)} KB). It opens this scenario and runs it to ${fmt(run.t_ms)}.`); }
    catch (e) { note(`Copy the link below (${Math.round(url.length / 1024)} KB).`); }
  }
  async function openFromHash(hash) {
    const m = (hash || location.hash.slice(1)).match(/^run=([\w-]+)/);
    if (!m) return false;
    try {
      const v = JSON.parse(await inflate(m[1]));
      open({ name: "shared run", t_ms: v.t || 0, scenario: v.sc }, false);
      return true;
    } catch (e) { note("The link could not be read: " + e.message); return false; }
  }

  function init() {
    $("st-save").onclick = () => save("run");
    $("st-snap").onclick = () => save("snapshot");
    $("st-share").onclick = shareLink;
    render();
  }
  window.DkStore = { init, openFromHash, render };
})();
