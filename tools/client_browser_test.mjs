// Kører webklienten i en rigtig browser (Edge/Chrome uden vindue) mod to
// maarse-hosts og tjekker, at scriptet indlæses uden fejl, at klienten
// forbinder, at en maarse://-side kan hentes og vises, og at
// talebeskeder kan kodes og afkodes med Codec2 (WebAssembly).
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
  const b = spawn(browser, ["--headless=new", "--remote-debugging-port=9395", `--user-data-dir=${prof}`, "--no-first-run", "--use-fake-device-for-media-stream", "--use-fake-ui-for-media-stream", "about:blank"], { stdio: "ignore" });
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

  // Talebeskeder: Codec2 i WebAssembly koder og afkoder 2 s tone.
  const voice = await ev(`(async () => {
    const pcm = new Int16Array(16000).map((_, i) => Math.round(Math.sin(i * 2 * Math.PI * 220 / 8000) * 8000));
    const enc = await voiceEncode(pcm);
    const dec = await voiceDecode(enc);
    const wav = wavBlob(dec);
    return [enc.length, dec.length, wav.size, String.fromCharCode(enc[0], enc[1])].join(",");
  })()`);
  if (voice === "305,16000,32044,C2") ok("talebesked: Codec2 1200 bit/s koder og afkoder i browseren (2 s = 305 bytes)");
  else fail("talebesked: uventet resultat " + voice);
  if (errors.length) fail("script-fejl undervejs: " + errors.join(" | "));

  // Talebesked hele vejen: simuleret mikrofon i klienten -> fil til server-hosten,
  // som åbnes i en anden fane og skal kunne afspille den.
  const recv = await (await fetch("http://127.0.0.1:9395/json/new?http://127.0.0.1:8195/", { method: "PUT" })).json();
  const ws2 = new WebSocket(recv.webSocketDebuggerUrl);
  await new Promise((r, j) => { ws2.onopen = r; ws2.onerror = j; });
  let id2 = 0; const wait2 = new Map();
  ws2.onmessage = (e) => { const m = JSON.parse(e.data); if (m.id && wait2.has(m.id)) { wait2.get(m.id)(m); wait2.delete(m.id); } };
  const ev2 = async (expr) => (await new Promise((r) => { const i = ++id2; wait2.set(i, r);
    ws2.send(JSON.stringify({ id: i, method: "Runtime.evaluate", params: { expression: expr, awaitPromise: true, returnByValue: true } })); })).result?.result?.value;
  const until2 = async (expr, secs) => { for (let i = 0; i < secs * 4; i++) { const v = await ev2(expr); if (v) return v; await sleep(250); } return null; };
  await until2("transport && transport.name === 'maarse-host'", 10);
  await ev(`state.active = "dm:${addr}"; renderChat(); voiceStart()`);
  await sleep(2500);
  await ev("voiceStop()");
  const sent = await until("conv('dm:" + addr + "').find(m => m.voice && m.status && /leveret/.test(m.status))?.text", 20);
  if (sent) ok("talebesked optaget og leveret: " + sent); else fail("talebeskeden blev ikke leveret (" + (await ev("JSON.stringify(conv('dm:" + addr + "').map(m => m.status || m.text))")) + ")");
  const got = await until2("[...state.convs.values()].flat().find(m => m.voice && m.url)?.text", 20);
  if (got) ok("modtageren afkoder talebeskeden: " + got); else fail("modtageren fik ingen afspillelig talebesked");
  ws2.close();
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
