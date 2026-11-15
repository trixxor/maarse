// Runs the web client in a real browser (headless Edge/Chrome) against two
// maarse-hosts and checks that the script loads without errors, that the client
// connects, that a maarse:// page can be fetched and shown, that
// voice messages can be encoded and decoded with Codec2 (WebAssembly), and that a
// call (Opus over TCP) sends audio both ways.
//
//   cargo build --release -p maarse-host
//   node tools/client_browser_test.mjs target/release/maarse-host[.exe]
//
// Catches errors that client_syntax_test.mjs cannot see (e.g. a regular
// expression that only fails when the script runs). Requires Node 22+ (WebSocket).
import { spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const host = process.argv[2];
if (!host) { console.error("Usage: node tools/client_browser_test.mjs <path to maarse-host>"); process.exit(2); }
const browsers = [
  process.env.BROWSER,
  "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
  "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
  "/usr/bin/chromium", "/usr/bin/chromium-browser", "/usr/bin/google-chrome", "/usr/bin/microsoft-edge",
].filter(Boolean);
const browser = browsers.find((b) => existsSync(b));
if (!browser) { console.log("SKIPPED: no Edge/Chrome/Chromium found (set BROWSER=<path>)"); process.exit(0); }

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const tmp = mkdtempSync(join(tmpdir(), "maarse-browser-"));
const procs = [];
let failed = false;
const ok = (m) => console.log("  OK  " + m);
const fail = (m) => { console.log("  FAIL " + m); failed = true; };

function start(name, args) {
  const p = spawn(host, ["--data", join(tmp, name), ...args], { stdio: ["ignore", "pipe", "pipe"] });
  let out = "";
  p.stdout.on("data", (d) => (out += d));
  p.stderr.on("data", (d) => (out += d));
  procs.push(p);
  return { p, out: () => out };
}

try {
  // The server host with two pages.
  mkdirSync(join(tmp, "srv", "pages"), { recursive: true });
  writeFileSync(join(tmp, "srv", "pages", "index.mu"), "# Test page æøå\n\nHello **world**.\n\n- [About](/about)\n");
  writeFileSync(join(tmp, "srv", "pages", "about.mu"), "# About\n\n[Back](/)\n");
  const srv = start("srv", ["--http", "127.0.0.1:8195", "--listen", "127.0.0.1:4295"]);
  await sleep(1500);
  const addr = srv.out().match(/Address:\s+([0-9a-f]{32})/)?.[1];
  if (!addr) throw new Error("the server host showed no address:\n" + srv.out());
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
  if (!page) throw new Error("could not control the browser");
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
  if (errors.length) fail("script errors on load: " + errors.join(" | ")); else ok("the script loads without errors");

  if (await until("transport && transport.name === 'maarse-host'", 10)) ok("the client connects to maarse-host"); else fail("the client did not connect");

  await ev(`openPage('maarse://${addr}/')`);
  const text = await until("document.querySelector('.page')?.innerText", 15);
  if (text?.includes("Test page æøå") && text.includes("world")) ok("the front page is fetched, unpacked and shown (UTF-8)");
  else fail("the front page was not shown (status: " + (await ev("pages.status")) + ")");

  await ev("[...document.querySelectorAll('.page a')].find(a => a.textContent === 'About')?.click()");
  const om = await until("document.querySelector('.page h2')?.innerText === 'About' && pages.url", 15);
  if (om?.endsWith("/about")) ok("a link to another page is followed"); else fail("the link was not followed");

  if (await ev("!!document.querySelector('.page script, .page iframe')")) fail("the page contains HTML elements");

  // Voice messages: Codec2 in WebAssembly encodes and decodes a 2 s tone.
  const voice = await ev(`(async () => {
    const pcm = new Int16Array(16000).map((_, i) => Math.round(Math.sin(i * 2 * Math.PI * 220 / 8000) * 8000));
    const enc = await voiceEncode(pcm);
    const dec = await voiceDecode(enc);
    const wav = wavBlob(dec);
    return [enc.length, dec.length, wav.size, String.fromCharCode(enc[0], enc[1])].join(",");
  })()`);
  if (voice === "305,16000,32044,C2") ok("voice message: Codec2 1200 bit/s encodes and decodes in the browser (2 s = 305 bytes)");
  else fail("voice message: unexpected result " + voice);
  if (errors.length) fail("script errors along the way: " + errors.join(" | "));

  // Voice message all the way: simulated microphone in the client -> file to the server host,
  // which is opened in another tab and must be able to play it.
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
  const sent = await until("conv('dm:" + addr + "').find(m => m.voice && m.status && /delivered/.test(m.status))?.text", 20);
  if (sent) ok("voice message recorded and delivered: " + sent); else fail("the voice message was not delivered (" + (await ev("JSON.stringify(conv('dm:" + addr + "').map(m => m.status || m.text))")) + ")");
  const got = await until2("[...state.convs.values()].flat().find(m => m.voice && m.url)?.text", 20);
  if (got) ok("the recipient decodes the voice message: " + got); else fail("the recipient got no playable voice message");

  // Call over TCP: A calls B, B answers, audio both ways, A hangs up.
  const srvAddr = addr, cliAddr = await ev("state.me");
  await ev(`callStart("${srvAddr}")`);
  if (await until2("call.state === 'ringing'", 10)) ok("call: B receives the call"); else fail("call: B did not ring");
  await ev2("callAnswer()");
  if (await until("call.state === 'active'", 10) && await until2("call.state === 'active'", 10)) ok("call: both in conversation");
  else fail("call: did not start (A " + (await ev("call.state")) + ", B " + (await ev2("call.state")) + ")");
  await sleep(3000);
  const ab = await ev("[call.tx, call.rx].join('/')"), ba = await ev2("[call.tx, call.rx].join('/')");
  const [atx, arx] = ab.split("/").map(Number), [btx, brx] = ba.split("/").map(Number);
  if (atx > 10 && brx > 10 && btx > 10 && arx > 10) ok(`call: audio both ways (A sent/received ${ab}, B ${ba} packets)`);
  else fail(`call: too little audio (A ${ab}, B ${ba})`);
  await ev("callHangup()");
  if (await until2("call.state === 'idle'", 10)) ok("call: hung up on both sides"); else fail("call: B did not hang up");
  void cliAddr;
  ws2.close();
  ws.close();
} catch (e) {
  fail(e.message);
} finally {
  for (const p of procs) { try { p.kill(); } catch {} }
  await sleep(500);
  try { rmSync(tmp, { recursive: true, force: true }); } catch {}
}
console.log(failed ? "FAILED" : "All browser tests passed");
process.exit(failed ? 1 : 0);
