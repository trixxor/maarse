// Kører webklienten i en rigtig browser (Edge/Chrome uden vindue) mod to
// maarse-hosts og tjekker, at scriptet indlæses uden fejl, at klienten
// forbinder, og at en maarse://-side kan hentes og vises.
//
//   cargo build --release -p maarse-host
//   node tools/client_browser_test.mjs target/release/maarse-host[.exe]
//
// Fanger fejl, som client_syntax_test.mjs ikke kan se (fx et regulært
// udtryk, der kun fejler, når scriptet køres). Kræver Node 22+ (WebSocket).
import { spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const host = process.argv[2];
if (!host) { console.error("Brug: node tools/client_browser_test.mjs <sti til maarse-host>"); process.exit(2); }
const browsers = [
  process.env.BROWSER,
  "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
  "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
  "/usr/bin/chromium", "/usr/bin/chromium-browser", "/usr/bin/google-chrome", "/usr/bin/microsoft-edge",
].filter(Boolean);
const browser = browsers.find((b) => existsSync(b));
if (!browser) { console.log("SPRUNGET OVER: ingen Edge/Chrome/Chromium fundet (sæt BROWSER=<sti>)"); process.exit(0); }

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const tmp = mkdtempSync(join(tmpdir(), "maarse-browser-"));
const procs = [];
let failed = false;
const ok = (m) => console.log("  OK  " + m);
const fail = (m) => { console.log("  FEJL " + m); failed = true; };

function start(name, args) {
  const p = spawn(host, ["--data", join(tmp, name), ...args], { stdio: ["ignore", "pipe", "pipe"] });
  let out = "";
  p.stdout.on("data", (d) => (out += d));
  p.stderr.on("data", (d) => (out += d));
  procs.push(p);
  return { p, out: () => out };
}

try {
  // Server-hosten med to sider.
  mkdirSync(join(tmp, "srv", "sider"), { recursive: true });
  writeFileSync(join(tmp, "srv", "sider", "index.mu"), "# Testside æøå\n\nHej **verden**.\n\n- [Om](/om)\n");
  writeFileSync(join(tmp, "srv", "sider", "om.mu"), "# Om\n\n[Tilbage](/)\n");
  const srv = start("srv", ["--http", "127.0.0.1:8195", "--listen", "127.0.0.1:4295"]);
  await sleep(1500);
  const addr = srv.out().match(/Adresse:\s+([0-9a-f]{32})/)?.[1];
  if (!addr) throw new Error("server-hosten viste ingen adresse:\n" + srv.out());
  start("cli", ["--http", "127.0.0.1:8196", "--listen", "off", "--peer", "127.0.0.1:4295"]);
  await sleep(2500);

  const prof = join(tmp, "browser");
  const b = spawn(browser, ["--headless=new", "--remote-debugging-port=9395", `--user-data-dir=${prof}`, "--no-first-run", "about:blank"], { stdio: "ignore" });
  procs.push(b);
  let targets;
  for (let i = 0; i < 30 && !targets; i++) {
    await sleep(300);
    try { targets = await (await fetch("http://127.0.0.1:9395/json")).json(); } catch {}
  }
  const page = targets?.find((t) => t.type === "page");
  if (!page) throw new Error("kunne ikke styre browseren");
  const ws = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((r, j) => { ws.onopen = r; ws.onerror = j; });
  let id = 0;
  const waiting = new Map(), errors = [];
  ws.onmessage = (e) => {
    const m = JSON.parse(e.data);
    if (m.id && waiting.has(m.id)) { waiting.get(m.id)(m); waiting.delete(m.id); }
    if (m.method === "Runtime.exceptionThrown") {
      const d = m.params.exceptionDetails;
      errors.push(d.exception?.description || d.text);
    }
  };
  const cdp = (method, params = {}) => new Promise((r) => { const i = ++id; waiting.set(i, r); ws.send(JSON.stringify({ id: i, method, params })); });
  const ev = async (expr) => (await cdp("Runtime.evaluate", { expression: expr, awaitPromise: true, returnByValue: true })).result?.result?.value;
  const until = async (expr, secs) => { for (let i = 0; i < secs * 4; i++) { const v = await ev(expr); if (v) return v; await sleep(250); } return null; };

  await cdp("Runtime.enable");
  await cdp("Page.navigate", { url: "http://127.0.0.1:8196/" });
  await sleep(1500);
  if (errors.length) fail("script-fejl ved indlæsning: " + errors.join(" | ")); else ok("scriptet indlæses uden fejl");

  if (await until("transport && transport.name === 'maarse-host'", 10)) ok("klienten forbinder til maarse-host"); else fail("klienten forbandt ikke");

  await ev(`openPage('maarse://${addr}/')`);
  const text = await until("document.querySelector('.page')?.innerText", 15);
  if (text?.includes("Testside æøå") && text.includes("verden")) ok("forsiden hentes, pakkes ud og vises (UTF-8)");
  else fail("forsiden blev ikke vist (status: " + (await ev("pages.status")) + ")");

  await ev("[...document.querySelectorAll('.page a')].find(a => a.textContent === 'Om')?.click()");
  const om = await until("document.querySelector('.page h2')?.innerText === 'Om' && pages.url", 15);
  if (om?.endsWith("/om")) ok("link til en anden side følges"); else fail("linket blev ikke fulgt");

  if (await ev("!!document.querySelector('.page script, .page iframe')")) fail("siden indeholder HTML-elementer");
  if (errors.length) fail("script-fejl undervejs: " + errors.join(" | "));
  ws.close();
} catch (e) {
  fail(e.message);
} finally {
  for (const p of procs) { try { p.kill(); } catch {} }
  await sleep(500);
  try { rmSync(tmp, { recursive: true, force: true }); } catch {}
}
console.log(failed ? "FEJLET" : "Alle browsertests bestået");
process.exit(failed ? 1 : 0);
