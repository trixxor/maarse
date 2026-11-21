// Layout test for the Denmark simulator's page: opens every panel, window and
// card in both views (Simulation and Real network) in a real browser
// (headless Edge/Chrome) at several window sizes, and fails when two panels
// overlap, when a panel is partly covered by another element, or when a
// panel reaches outside the window.
//
//   node tools/denmark_layout_test.mjs            all window sizes
//   node tools/denmark_layout_test.mjs --quick    one window size
//   node tools/denmark_layout_test.mjs --shots    also save a screenshot per state to a temp folder
//
// The page needs the Leaflet library from the internet. The real network
// view is fed with made-up nodes and history, and its servers are pointed
// at an address nobody answers, so the result does not depend on any server.
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { existsSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { extname, join, normalize, resolve } from "node:path";

const args = process.argv.slice(2);
const quick = args.includes("--quick"), shots = args.includes("--shots");
const docs = resolve(import.meta.dirname, "..", "docs");
const browsers = [process.env.BROWSER, "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe", "C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe",
  "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe", "/usr/bin/chromium", "/usr/bin/chromium-browser", "/usr/bin/google-chrome", "/usr/bin/microsoft-edge"].filter(Boolean);
const browser = browsers.find((b) => existsSync(b));
if (!browser) { console.log("SKIPPED: no Edge/Chrome/Chromium found (set BROWSER=<path>)"); process.exit(0); }

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const MIME = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".json": "application/json", ".wasm": "application/wasm", ".svg": "image/svg+xml", ".png": "image/png", ".webp": "image/webp", ".bin": "application/octet-stream" };
const server = createServer((req, res) => {
  let p = normalize(decodeURIComponent(new URL(req.url, "http://x").pathname)).replace(/^([/\\])+/, "");
  let file = resolve(docs, p);
  if (!file.startsWith(docs)) { res.writeHead(403).end(); return; }
  if (existsSync(file) && statSync(file).isDirectory()) file = join(file, "index.html");
  if (!existsSync(file)) { res.writeHead(404).end(); return; }
  res.writeHead(200, { "Content-Type": MIME[extname(file)] || "application/octet-stream", "Cache-Control": "no-store" }).end(readFileSync(file));
}).listen(8197, "127.0.0.1");

const tmp = mkdtempSync(join(tmpdir(), "maarse-layout-"));
const b = spawn(browser, ["--headless=new", "--remote-debugging-port=9396", `--user-data-dir=${join(tmp, "profile")}`, "--no-first-run", "--hide-scrollbars", "--force-device-scale-factor=1", "about:blank"], { stdio: "ignore" });
const cleanup = () => { try { b.kill(); } catch { /* gone */ } server.close(); try { rmSync(join(tmp, "profile"), { recursive: true, force: true }); } catch { /* in use */ } };
process.on("exit", cleanup);

let targets;
for (let i = 0; i < 40 && !targets; i++) { await sleep(300); try { targets = await (await fetch("http://127.0.0.1:9396/json")).json(); } catch { /* not up yet */ } }
const page = targets?.find((t) => t.type === "page");
if (!page) { console.log("FAIL could not control the browser"); process.exit(1); }
const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((r, j) => { ws.onopen = r; ws.onerror = j; });
let id = 0;
const waiting = new Map(), errors = [];
ws.onmessage = (e) => {
  const m = JSON.parse(e.data);
  if (m.id && waiting.has(m.id)) { waiting.get(m.id)(m); waiting.delete(m.id); }
  if (m.method === "Runtime.exceptionThrown") { const d = m.params.exceptionDetails; errors.push(d.exception?.description || d.text); }
};
const cdp = (method, params = {}) => new Promise((r) => { const i = ++id; waiting.set(i, r); ws.send(JSON.stringify({ id: i, method, params })); });
const ev = async (expr) => (await cdp("Runtime.evaluate", { expression: expr, awaitPromise: true, returnByValue: true })).result?.result?.value;
const until = async (expr, secs) => { for (let i = 0; i < secs * 4; i++) { const v = await ev(expr); if (v) return v; await sleep(250); } return null; };
await cdp("Runtime.enable"); await cdp("Page.enable");
await cdp("Page.addScriptToEvaluateOnNewDocument", { source: `try { localStorage.setItem("maarse.realworld.servers", JSON.stringify(["http://127.0.0.1:9"])); } catch (e) {}` });

// ---- what is measured, in the page ----
const AUDIT = `(() => {
  const nm = (e) => e.id ? "#" + e.id : (typeof e.className === "string" && e.className.trim() ? "." + e.className.trim().split(/\\s+/)[0] : e.tagName.toLowerCase());
  const POPOVER = ["#rw-card", "#cv-card"], DROPDOWN = ["#ov-sound"], WINDOW = ["#ov-pkt", "#ov-chan"], CONTROL = [".leaflet-control-attribution", ".leaflet-control-zoom"], MENU = [".pop"], BAR = ["#tl", "#ov-tl"];
  // A panel is something floating that has a face: a background, a border or a shadow. Empty,
  // transparent layers (the phone layer, the dock, the toast area) are not panels; what is in them is.
  const hasFace = (cs) => {
    const bg = cs.backgroundColor, alpha = bg.startsWith("rgba") ? parseFloat(bg.split(",")[3]) : bg === "transparent" ? 0 : 1;
    return alpha > 0.05 || parseFloat(cs.borderTopWidth) > 0 || parseFloat(cs.borderLeftWidth) > 0 || cs.boxShadow !== "none" || cs.backgroundImage !== "none";
  };
  const floating = (e) => { for (let n = e; n && n !== document.body; n = n.parentElement) if (["absolute", "fixed"].includes(getComputedStyle(n).position) && n.id !== "mapwrap") return true; return false; };
  const all = [...document.querySelectorAll("body *")].filter((e) => {
    if ((e.closest("#map") && !e.matches(".leaflet-control-attribution, .leaflet-control-zoom")) || e.closest("script,style,svg")) return false;
    const cs = getComputedStyle(e);
    if (cs.display === "none" || cs.visibility === "hidden" || +cs.opacity === 0) return false;
    if (!hasFace(cs) || !(floating(e) || e.matches(".leaflet-control-attribution, .leaflet-control-zoom"))) return false;
    const r = e.getBoundingClientRect();
    return r.width > 24 && r.height > 14 && r.right > 0 && r.bottom > 0 && r.left < innerWidth && r.top < innerHeight;
  });
  const top = all.filter((e) => !all.some((o) => o !== e && o.contains(e)));
  const issues = [];
  const is = (e, list) => list.some((s) => e.matches(s));
  for (let i = 0; i < top.length; i++) for (let j = i + 1; j < top.length; j++) {
    const A = top[i], B = top[j];
    if (is(A, DROPDOWN) || is(B, DROPDOWN)) continue;                              // a drop-down opens over what is below it
    if ((is(A, MENU) && !is(B, BAR)) || (is(B, MENU) && !is(A, BAR))) continue;       // a menu opens over the map, but never over the play bar
    if ((is(A, POPOVER) && is(B, WINDOW)) || (is(B, POPOVER) && is(A, WINDOW))) continue; // a card may lie over a window
    if ((is(A, CONTROL) && is(B, WINDOW)) || (is(B, CONTROL) && is(A, WINDOW))) continue;  // a window takes over the whole map area, controls included
    const a = A.getBoundingClientRect(), b = B.getBoundingClientRect();
    const w = Math.min(a.right, b.right) - Math.max(a.left, b.left), h = Math.min(a.bottom, b.bottom) - Math.max(a.top, b.top);
    if (w > 6 && h > 6) issues.push("overlap " + nm(A) + " and " + nm(B) + " (" + Math.round(w) + "x" + Math.round(h) + " px)");
  }
  for (const e of top) {
    const r = e.getBoundingClientRect();
    if (is(e, CONTROL) && document.querySelector("#ov-pkt:not(.hidden), #ov-chan:not(.hidden)")) continue;
    if (getComputedStyle(e).pointerEvents === "none") continue;                      // a message that cannot be clicked is not covered by anything that matters
    if (r.left < -2 || r.top < -2 || r.right > innerWidth + 2 || r.bottom > innerHeight + 2) issues.push(nm(e) + " reaches outside the window");
    const pts = [[r.left + r.width / 2, r.top + r.height / 2], [r.left + 8, r.top + 8], [r.right - 8, r.top + 8], [r.left + 8, r.bottom - 8], [r.right - 8, r.bottom - 8]];
    const covered = new Set();
    for (const [x, y] of pts) { if (x < 0 || y < 0 || x > innerWidth || y > innerHeight) continue; const t = document.elementFromPoint(x, y); if (t && !e.contains(t) && !t.contains(e) && !t.closest(".pop, #ov-sound") && !(is(e, WINDOW) && t.closest("#rw-card, #cv-card"))) covered.add(nm(t)); }
    if (covered.size) issues.push(nm(e) + " is partly covered by " + [...covered].join(", "));
  }
  return { visible: top.map(nm), issues };
})()`;

const SEED = `(() => {
  const O = window.DKOverview, now = Date.now(), MIN = 60000;
  const nodes = [["43916f06181f7d313e99f66580564166", "nj01.test", "Host", 57.0, 10.0], ["256665f180b8e3ce156fd52764c98012", "node a", "Anchor", 57.275, 9.885], ["b0029bf6ef69d577de7fa1443f69b50a", "node b", "Client", 56.2, 10.2], ["c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1", "node c", "Repeater", 55.5, 9.0], ["d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2", "node d", "Relay", 55.7, 12.4]];
  for (const [addr, name, role, lat, lon] of nodes) O.state.nodes.set(addr, { addr, name, role, lat, lon, last_seen: Math.floor(now / 1000) - 60, height_m: role === "Client" ? null : 20, host: role === "Host" });
  const buckets = [], announces = [], messages = [];
  for (let i = 1440; i >= 1; i--) buckets.push({ t: Math.floor(now / MIN) * MIN - i * MIN, rx: { radio: 0, trunk: 0, backbone: 4 + (i % 7) }, tx: { radio: 0, trunk: 0, backbone: 6 }, types: { Announce: 2, Data: 3, Channel: 1, Ack: 2 } });
  for (let i = 0; i < 100; i++) for (const n of nodes) if (i % 3 === 0) announces.push({ t: now - i * 10 * MIN, addr: n[0] });
  for (let i = 0; i < 60; i++) messages.push({ time: now - i * 17 * MIN, channel: ["public", "#ping", "#test", "#dk"][i % 4], sender: nodes[i % 5][0].slice(0, 8), text: "message number " + i + " with some words in it" });
  O.useHistory({ buckets, announces, messages });
  for (let i = 0; i < 30; i++) O.state.packets.push({ id: "00ab" + String(i).padStart(12, "0"), kind: ["Announce", "Data", "Channel", "Ack"][i % 4], observer_count: 1 + (i % 4), observations: [{ observer: nodes[i % 5][0], subject: nodes[(i + 1) % 5][0], time: now - i * 4000, transport: "backbone", direction: "rx", hops: i % 4, snr: null }] });
  return true;
})()`;

const click = (sel) => `(() => { const e = document.querySelector(${JSON.stringify(sel)}); if (!e) return "missing ${sel.replace(/"/g, "")}"; e.click(); return "ok"; })()`;
const REAL = `DKOverview.setMode("real")`;
const SIM_STATES = [
  ["Simulation", "true"],
  ["Simulation: File menu", click("#btn-file")],
  ["Simulation: help", click("#btn-help")],
  ["Simulation: Build", click('#modes [data-mode="build"]')],
  ["Simulation: Results", click('#modes [data-mode="results"]')],
  ["Simulation: add phone", click("#ph-add")],
  ["Simulation: coverage card", `DKCoverage.setSites([{ lat: 56.2, lon: 10.2, h: 15, gain: 6, name: "test" }])`],
  ["Simulation: layers", `DKShell.toggleLayers()`],
  ["Simulation: packet feed (forced)", `(() => { document.querySelector("#feed")?.classList.remove("hidden"); return true; })()`],
  ["Simulation: follow card (forced)", `(() => { document.querySelector("#follow")?.classList.remove("hidden"); return true; })()`],
  ["Simulation: watch list (forced)", `(() => { document.querySelector("#watch")?.classList.remove("hidden"); return true; })()`],
  ["Simulation: node card", `(() => { const e = DKSim.scenario.nodes[0]; return DKSim.openNode ? DKSim.openNode(0) : true; })()`],
];
const REAL_STATES = [
  ["Real network", "true"],
  ["Real network: node selected", `(() => { document.querySelector("#ov-list .item").click(); return true; })()`],
  ["Real network: Packets", click('#ov-wins [data-win="packets"]'), "#ov-pkt"],
  ["Real network: Channels", click('#ov-wins [data-win="channels"]'), "#ov-chan"],
  ["Real network: Servers card", click('#ov-wins [data-act="servers"]'), "#rw-card"],
  ["Real network: Coverage without data (the Servers card explains)", click('#ov-wins [data-act="coverage"]'), "#rw-card"],
  ["Real network: Coverage card", `(() => { document.querySelector('#ov-wins [data-act="coverage"]').click(); DKCoverage.setSites([{ lat: 56.2, lon: 10.2, h: 15, gain: 6, name: "test" }]); return true; })()`, "#cv-card"],
  ["Real network: sound panel", click("#ov-snd"), "#ov-sound"],
  ["Real network: 3 h back", `DKOverview.setView(Date.now() - 3 * 3600000)`],
  ["Real network: 3 h back, Packets", click('#ov-wins [data-win="packets"]')],
  ["Real network: back to live", `DKOverview.setView(null)`],
];

const viewports = quick ? [[1366, 768]] : [[1920, 1080], [1366, 768], [1100, 700], [900, 640]];
let failed = false, checked = 0;
async function load(w, h) {
  await cdp("Emulation.setDeviceMetricsOverride", { width: w, height: h, deviceScaleFactor: 1, mobile: false });
  await cdp("Page.navigate", { url: "http://127.0.0.1:8197/denmark/" });
  await sleep(500);
  if (!(await until("!!(window.DKSim && window.DKOverview && window.DKSim.sim)", 60))) { console.log("FAIL the page did not start (Leaflet from the internet is needed): " + errors.join(" | ")); process.exit(1); }
  await sleep(800);
}
async function state(label, vp, setup, expect) {
  const res = await ev(setup);
  await sleep(700);
  const out = await ev(`JSON.stringify(${AUDIT})`);
  const r = JSON.parse(out);
  checked++;
  const name = `${label} @ ${vp[0]}x${vp[1]}`;
  if (typeof res === "string" && res.startsWith("missing")) { failed = true; console.log("  FAIL " + name + ": " + res); return; }
  if (expect) { const shown = await ev(`(() => { const e = document.querySelector(${JSON.stringify(expect)}); if (!e) return false; const r = e.getBoundingClientRect(), cs = getComputedStyle(e); return cs.display !== "none" && cs.visibility !== "hidden" && r.width > 20 && r.height > 20 && r.right > 0 && r.bottom > 0 && r.left < innerWidth && r.top < innerHeight; })()`); if (!shown) { const why = await ev(`(() => { const e = document.querySelector(${JSON.stringify(expect)}); if (!e) return "not in the page"; const r = e.getBoundingClientRect(), cs = getComputedStyle(e); return "class=" + e.className + ", display=" + cs.display + ", parent=#" + (e.parentElement && e.parentElement.id) + ", rect=" + [r.left, r.top, r.right, r.bottom].map(Math.round) + ", window=" + innerWidth + "x" + innerHeight; })()`); r.issues.push(expect + " should be on screen, but is not (" + why + ")"); } }
  if (r.issues.length) { failed = true; console.log("  FAIL " + name + "\n         " + r.issues.join("\n         ")); } else console.log("  OK   " + name);
  if (shots) { const s = await cdp("Page.captureScreenshot", { format: "png" }); writeFileSync(join(tmp, label.replace(/\W+/g, "_") + `_${vp[0]}.png`), Buffer.from(s.result.data, "base64")); }
}

for (const vp of viewports) {
  for (const [label, setup] of SIM_STATES) { await load(...vp); await state(label, vp, setup); }
  await load(...vp);
  await ev(SEED); await ev(REAL); await sleep(900);
  for (const [label, setup, expect] of REAL_STATES) await state(label, vp, setup, expect);
  // opened directly in the real view (?view=network): the cards must still be where the view expects them
  await cdp("Emulation.setDeviceMetricsOverride", { width: vp[0], height: vp[1], deviceScaleFactor: 1, mobile: false });
  await cdp("Page.navigate", { url: "http://127.0.0.1:8197/denmark/?view=network" });
  await sleep(500);
  if (!(await until("!!(window.DKOverview && window.DKOverview.state.mode === 'real')", 60))) { failed = true; console.log("  FAIL ?view=network did not open the real view @ " + vp.join("x")); }
  else {
    await sleep(800); await ev(SEED); await sleep(800);
    await state("Real network opened by link", vp, "true", "#ov-tl");
    await state("Real network opened by link: Coverage card", vp, `(() => { document.querySelector('#ov-wins [data-act="coverage"]').click(); DKCoverage.setSites([{ lat: 56.2, lon: 10.2, h: 15, gain: 6, name: "test" }]); return true; })()`, "#cv-card");
  }
  // and back: the simulation must look as it did
  await ev(`DKOverview.setMode("sim")`); await sleep(500);
  await state("Back in Simulation", vp, "true");
}
if (errors.length) { failed = true; console.log("  FAIL script errors: " + [...new Set(errors)].slice(0, 5).join(" | ")); }
else console.log("  OK   no script errors");
if (shots) console.log("screenshots: " + tmp);
console.log(failed ? `FAILED (${checked} states checked)` : `OK (${checked} states checked)`);
process.exit(failed ? 1 : 0);
