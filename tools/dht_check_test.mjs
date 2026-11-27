// Tests the web client's DHT check (the host's report lines) with a fake transport.
//   node tools/dht_check_test.mjs
import { readFileSync } from "fs";
import { fileURLToPath } from "url";
import path from "path";
import assert from "assert";

const here = path.dirname(fileURLToPath(import.meta.url));
const html = readFileSync(path.join(here, "..", "maarse-client", "index.html"), "utf8");
const a = html.indexOf("// ---------- DHT check:");
const b = html.indexOf("// ---------- topology crawl:");
assert(a > 0 && b > a, "the DHT check block was not found in index.html");
const block = html.slice(a, b).replace("const DHT_TIMEOUT_MS = 60000;", "const DHT_TIMEOUT_MS = 60;");

const sent = [];
const h = new Function("sent", `
  const state = { nextReq: 5, view: "other" };
  const transport = { send: f => sent.push(f) };
  const Cmd = { dhtCheck: req => ({ req }) };
  const renderChat = () => {};
  ${block}
  return { dhtCheck, dhtStart, dhtOnLine, dhtWorst };
`)(sent);

const sleep = ms => new Promise(r => setTimeout(r, ms));

// A check with three lines; the last one ends it. A line for another request is not taken.
h.dhtStart();
assert.equal(sent.length, 1);
const req = sent[0].req;
assert.equal(h.dhtOnLine({ req: req + 1, level: 1, last: true, text: "x" }), false);
assert(h.dhtOnLine({ req, level: 1, last: false, text: "All 2 contacts answer" }));
assert(h.dhtOnLine({ req, level: 2, last: false, text: "1 of 2 hold the record" }));
assert(!h.dhtCheck.run.done);
assert(h.dhtOnLine({ req, level: 0, last: true, text: "info" }));
assert(h.dhtCheck.run.done && h.dhtCheck.run.lines.length === 3);
assert.equal(h.dhtWorst(h.dhtCheck.run), 2);
assert.equal(h.dhtOnLine({ req, level: 3, last: true, text: "late" }), false, "nothing after the end");

// No answer at all (a radio node ignores the command): a problem line after the time-out.
h.dhtStart();
await sleep(120);
assert(h.dhtCheck.run.done);
assert.equal(h.dhtCheck.run.lines.length, 1);
assert.equal(h.dhtWorst(h.dhtCheck.run), 3);
assert(h.dhtCheck.run.lines[0].text.includes("Only a host"));
console.log("OK  DHT check: lines, last marker, foreign request, time-out");
