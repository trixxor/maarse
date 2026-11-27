// Tests the web client's ping test (repeated trace routes) with a fake transport:
// the statistics, loss, error and time-out handling, the CSV and the distance maths.
//   node tools/ping_test.mjs
import { readFileSync } from "fs";
import { fileURLToPath } from "url";
import path from "path";
import assert from "assert";

const here = path.dirname(fileURLToPath(import.meta.url));
const html = readFileSync(path.join(here, "..", "maarse-client", "index.html"), "utf8");
const a = html.indexOf("// ---------- ping test:");
const b = html.indexOf("function renderDiag(box)");
assert(a > 0 && b > a, "the ping test block was not found in index.html");
const block = html.slice(a, b).replace("const PING_TIMEOUT_MS = 30000;", "const PING_TIMEOUT_MS = 80;");

const sent = [];
const logs = [];
const harness = new Function("sent", "logs", `
  const state = { nextReq: 1, traceReq: new Map(), nodeInfo: new Map(), contacts: new Map(), view: "other" };
  const transport = { send: f => sent.push(f) };
  const Cmd = { traceRoute: (req, d, via = 0) => ({ req, d, via }) };
  const label = x => x.slice(0, 4);
  const idName = id => "r" + id;
  const log = m => logs.push(m);
  const renderChat = () => {};
  const rtCsv = (head, rows) => [head, ...rows].map(r => r.join(",")).join("\\n");
  const rtExport = () => {};
  const snrText = v => v === undefined ? "-" : v + " dB";
  const navigator = {};
  ${block}
  return { ping, pingStart, pingStop, pingOnTrace, pingOnError, pingSummary, pingCsv, pingDistance, state };
`)(sent, logs);

const { ping, pingStart, pingOnTrace, pingOnError, pingSummary, pingCsv, pingDistance, state } = harness;
const sleep = ms => new Promise(r => setTimeout(r, ms));
const dest = "ab".repeat(16);

// distance: one degree of latitude is about 111 km
const d = pingDistance({ lat: 56, lon: 10 }, { lat: 57, lon: 10 });
assert(d > 110000 && d < 112500, "distance " + d);

// A run of 4 pings, 20 ms apart: #1 answered, #2 error, #3 answered, #4 never answered.
pingStart(dest, 4, 0.05, false);
await sleep(5);
assert.equal(sent.length, 1, "first ping sent at once");
assert.equal(sent[0].d, dest);
pingOnTrace({ req: sent[0].req, path: [7, 9], out: [-3, 5, 8], back: [4, -2, 6] });
await sleep(70);
assert.equal(sent.length, 2, "second ping after the interval");
assert(pingOnError({ req: sent[1].req, text: "no route" }));
await sleep(70);
assert.equal(sent.length, 3);
pingOnTrace({ req: sent[2].req, path: [], out: [10], back: [9] });
assert.equal(pingOnTrace({ req: 12345, path: [], out: [], back: [] }), false, "a foreign trace is not taken");
await sleep(70);
assert.equal(sent.length, 4);
await sleep(300); // the last one times out; the run finishes

const run = ping.run;
assert(run.finished, "the run finishes after the last time-out");
assert.deepEqual(run.results.map(r => r.state), ["ok", "error", "ok", "lost"]);
assert.equal(run.results[0].hops, 2);
assert.equal(run.results[0].outMin, -3);
assert.equal(run.results[0].backMin, -2);
assert.equal(run.results[0].path, "r7 > r9");
assert.equal(run.results[1].text, "no route");
const s = pingSummary(run);
assert.deepEqual([s.sent, s.answered, s.lost, s.loss], [4, 2, 2, 50]);
assert(s.min <= s.avg && s.avg <= s.max);
const csv = pingCsv().text.split("\n");
assert.equal(csv.length, 5);
assert(csv[0].startsWith("time_iso,target,seq,result,via,rtt_ms"));
assert(csv[2].includes(",error,"));
assert(csv[4].includes(",lost,"));

// Stopping early marks what is waiting as lost and stops sending.
sent.length = 0;
pingStart(dest, 50, 0.05, false);
await sleep(5);
harness.pingStop();
const n = sent.length;
await sleep(120);
assert.equal(sent.length, n, "nothing is sent after Stop");
assert(ping.run.finished && ping.run.results.every(r => r.state !== "waiting"));

// Positions: the farthest answered ping is measured from the first position.
sent.length = 0;
state.nodeInfo.clear();
pingStart(dest, 2, 0.05, false);
await sleep(5);
ping.run.results[0].lat = 56.0; ping.run.results[0].lon = 10.0;
pingOnTrace({ req: sent[0].req, path: [], out: [0], back: [0] });
await sleep(70);
ping.run.results[1].lat = 56.0; ping.run.results[1].lon = 10.0 + 0.0161; // about 1 km east at 56 degrees north
pingOnTrace({ req: sent[1].req, path: [], out: [0], back: [0] });
const s2 = pingSummary(ping.run);
assert(s2.farthest > 950 && s2.farthest < 1050, "farthest " + s2.farthest);
assert.equal(s2.baseIsTarget, false);

// Compare mode: LoRa (1) and TCP (2) take turns, and the statistics are kept per kind of link.
sent.length = 0;
pingStart(dest, 4, 0.05, false, 3);
for (let i = 0; i < 4; i++) {
  await sleep(i === 0 ? 5 : 70);
  assert.equal(sent[i].via, i % 2 === 0 ? 1 : 2, "ping " + (i + 1) + " goes over " + (i % 2 === 0 ? "LoRa" : "TCP"));
  if (sent[i].via === 2) pingOnTrace({ req: sent[i].req, path: [], out: [5], back: [5] });
  else pingOnTrace({ req: sent[i].req, path: [3, 4], out: [-8, -9, -7], back: [-6, -8, -7] });
}
await sleep(40);
const s3 = pingSummary(ping.run);
assert.equal(s3.byVia.length, 2);
const lora = s3.byVia.find(v => v.via === 1), tcp = s3.byVia.find(v => v.via === 2);
assert.deepEqual([lora.sent, lora.answered, lora.hops], [2, 2, 2]);
assert.deepEqual([tcp.sent, tcp.answered, tcp.hops], [2, 2, 0]);
const csvRows = pingCsv().text.split(String.fromCharCode(10));
assert(csvRows[0].includes(",via,"), "the CSV has a via column");
assert(csvRows[1].includes(",lora,") && csvRows[2].includes(",tcp,"), "the CSV says which link each ping used");

console.log("OK  ping test: statistics, loss, errors, time-out, stop, CSV and distance");
process.exit(0);
