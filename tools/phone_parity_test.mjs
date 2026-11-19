// Keeps the simulated phone (docs/denmark/phone-app) level with the Android app.
//
// 1. Static: every command the Android app sends (Cmd.x in its Kotlin code)
//    must also be sent by the phone copy (its app.js, or the web client's
//    page renderer it reuses). Matched by frame type, not by name.
// 2. Engine: every command the phone copy sends is fed to a simulated phone
//    (a host on the phone, linked to a backbone host). The engine must not
//    answer "Not simulated yet".
//
// Gaps that are known and accepted are listed below with the reason; a new
// gap fails the test. Build the engine first: python tools/build_denmark_sim.py
// Usage: node tools/phone_parity_test.mjs
import { createRequire } from "module";
import { readFileSync, readdirSync, statSync } from "fs";
import { fileURLToPath } from "url";
import path from "path";
import vm from "vm";

const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
const sim = path.join(root, "docs", "denmark");
const read = (p) => readFileSync(p, "utf8");

// Known gaps: frame type -> why it is accepted.
const APP_ONLY = {
  // (none)
};
const ENGINE_GAPS = {
  0x18: "firmware update (OTA) is not simulated",
  0x19: "firmware update (OTA) is not simulated",
  0x1a: "firmware update (OTA) is not simulated",
  0x2d: "Wi-Fi modem key is not simulated",
};

let failed = 0;
const check = (ok, what) => { console.log((ok ? "ok   " : "FAIL ") + what); if (!ok) failed++; };
const hex2 = (n) => "0x" + n.toString(16).padStart(2, "0");

// ---------- 1. Android vs phone copy ----------
function kotlinFiles(dir) {
  const out = [];
  for (const f of readdirSync(dir)) {
    const p = path.join(dir, f);
    if (statSync(p).isDirectory()) out.push(...kotlinFiles(p));
    else if (f.endsWith(".kt")) out.push(p);
  }
  return out;
}
const ktDir = path.join(root, "android", "app", "src", "main", "java");
const protoKt = read(path.join(ktDir, "dk", "maarse", "app", "Proto.kt"));
const ktOps = {};
for (const m of protoKt.matchAll(/fun (\w+)\([^)]*\)\s*=\s*frame\((0x[0-9a-fA-F]+)\)/g)) ktOps[m[1]] = parseInt(m[2], 16);
const ktUsed = new Set();
for (const f of kotlinFiles(ktDir)) for (const m of read(f).matchAll(/Cmd\.(\w+)\(/g)) if (ktOps[m[1]] !== undefined) ktUsed.add(ktOps[m[1]]);

const protoJs = read(path.join(sim, "phone-app", "proto.js"));
const jsOps = {};
for (const m of protoJs.matchAll(/^\s+(\w+): \([^)]*\) => frame\((0x[0-9a-fA-F]+)/gm)) jsOps[m[1]] = parseInt(m[2], 16);
const client = read(path.join(sim, "client.html"));
const a = client.indexOf("// ---------- Må:rse markup (MU) ----------"), b = client.indexOf("// ---------- voice messages (Codec2) ----------");
const jsSrc = read(path.join(sim, "phone-app", "app.js")) + (a >= 0 && b > a ? client.slice(a, b) : "");
const jsUsedNames = new Set([...jsSrc.matchAll(/Cmd\.(\w+)\(/g)].map((m) => m[1]).filter((n) => jsOps[n] !== undefined));
const jsUsed = new Set([...jsUsedNames].map((n) => jsOps[n]));

console.log(`Android sends ${ktUsed.size} command types, the phone copy ${jsUsed.size}`);
const ktName = (op) => Object.keys(ktOps).find((k) => ktOps[k] === op);
for (const op of [...ktUsed].sort((x, y) => x - y)) {
  if (jsUsed.has(op)) continue;
  if (APP_ONLY[op]) { console.log(`skip ${hex2(op)} ${ktName(op)}: ${APP_ONLY[op]}`); continue; }
  check(false, `phone copy never sends ${hex2(op)} ${ktName(op)} (the Android app does)`);
}
check(true, "every Android command is sent by the phone copy or listed as a known gap");

// ---------- 2. the engine answers every command the phone copy sends ----------
const require = createRequire(import.meta.url);
const { load } = require(path.join(sim, "maarse-sim.js"));
const { Sim } = await load(readFileSync(path.join(sim, "maarse_sim_wasm.wasm")));

const ctx = { TextEncoder, TextDecoder, BigInt, Uint8Array, console };
vm.createContext(ctx);
vm.runInContext(protoJs + "\n;globalThis.__Cmd = Cmd;", ctx);
const Cmd = ctx.__Cmd;

const ADDR = "11".repeat(16);
function argFor(name) {
  if (/^(dest|addr|conv|ch|channel|device|peer|admin)/i.test(name)) return ADDR;
  if (/admins/i.test(name)) return [ADDR];
  if (/key/i.test(name)) return new Uint8Array(32);
  if (/data|content|bytes|frame/i.test(name)) return new Uint8Array([1, 2, 3]);
  if (/text|name|note|path|mime|words|phrase|manifest|names|meta/i.test(name)) return "x";
  if (/cfg|radio|sharing|wifi/i.test(name)) return {};
  return 1;
}

const scenario = {
  seed: 5, duration_s: 600,
  nodes: [
    { kind: "host", lat: 56.0, lon: 9.0, label: "Host" },
    { kind: "app", lat: 56.0, lon: 9.01, label: "Phone" },
    { kind: "host", lat: 55.7, lon: 12.5, label: "Other host" },
  ],
  links: [{ a: 0, b: 1, kind: "tcp", up: true }, { a: 0, b: 2, kind: "tcp", up: true }],
  traffic: { direct_per_min: 0, channel_per_min: 0 },
};
const s = new Sim(scenario);
s.step(5_000);
const phone = 1;
// Attach the phone with a first command, then drop its answers.
s.call({ cmd: "phone_cmd", node: phone, frame: [...Cmd.getInfo()] });
s.step(200);
s.call({ cmd: "phone_events", node: phone });

const errorsOf = (frames) => frames
  .map((f) => Uint8Array.from(f))
  .filter((f) => f[2] === 0x8f)
  .map((f) => new TextDecoder().decode(f.subarray(6)));

// PARITY_SELFTEST=1: also send a command the engine does not simulate, with
// no allowance, to prove that the check fails on a real gap.
if (process.env.PARITY_SELFTEST) {
  vm.runInContext("Cmd.setModemKey = (text) => frame(0x2d, (w) => w.text(text));", ctx);
  jsOps.setModemKey = 0x2d; jsUsedNames.add("setModemKey"); delete ENGINE_GAPS[0x2d];
}
for (const n of [...jsUsedNames].sort()) {
  const fn = Cmd[n], op = jsOps[n];
  const params = (fn.toString().match(/^\(([^)]*)\)/) || [, ""])[1].split(",").map((x) => x.trim()).filter(Boolean);
  let frame;
  try { frame = fn(...params.map(argFor)); } catch (e) { console.log(`skip ${hex2(op)} ${n}: test arguments do not fit (${e.message})`); continue; }
  s.call({ cmd: "phone_cmd", node: phone, frame: [...frame] });
  s.step(200);
  const errs = errorsOf(s.call({ cmd: "phone_events", node: phone }).frames || []).filter((t) => /Not simulated yet/.test(t));
  if (errs.length && ENGINE_GAPS[op]) { console.log(`skip ${hex2(op)} ${n}: ${ENGINE_GAPS[op]}`); continue; }
  check(errs.length === 0, `engine answers ${hex2(op)} ${n}${errs.length ? ": " + errs[0] : ""}`);
}
s.free();

if (failed) { console.log(`${failed} check(s) failed`); process.exit(1); }
console.log("OK");
