// Tests the web client's network crawl (topology crawl over remote neighbour lists) with a fake transport.
//   node tools/crawl_test.mjs
import { readFileSync } from "fs";
import { fileURLToPath } from "url";
import path from "path";
import assert from "assert";

const here = path.dirname(fileURLToPath(import.meta.url));
const html = readFileSync(path.join(here, "..", "maarse-client", "index.html"), "utf8");
const a = html.indexOf("// ---------- topology crawl:");
const b = html.indexOf("// ---------- ping test:");
assert(a > 0 && b > a, "the crawl block was not found in index.html");
const block = html.slice(a, b).replace("const CRAWL_TIMEOUT_MS = 40000;", "const CRAWL_TIMEOUT_MS = 80;");

const A = "a1".repeat(16), B = "b2".repeat(16), C = "c3".repeat(16), D = "d4".repeat(16);
const sent = [];
const h = new Function("sent", "A", "B", "C", "D", `
  const state = { nextReq: 1, contacts: new Map([[A, {}], [B, {}], [C, {}], [D, {}]]), me: "", view: "other" };
  const transport = { send: f => sent.push(f) };
  const Cmd = { remote: (req, to, inner) => ({ req, to, inner }), getNeighbors: () => "nb" };
  const label = x => ({ [A]: "A", [B]: "B", [C]: "C", [D]: "D" })[x] || x.slice(0, 4);
  const nodeName = p => p;
  const log = () => {};
  const renderChat = () => {};
  const rtCsv = (head, rows) => [head, ...rows].map(r => r.join(",")).join("\\n");
  const snrText = v => v + " dB";
  ${block}
  return { crawl, crawlStart, crawlStop, crawlOnReply, crawlLinks, crawlSummary, crawlCsv };
`)(sent, A, B, C, D);

const sleep = ms => new Promise(r => setTimeout(r, ms));
const nb = (addr, snr) => ({ prefix: addr.slice(0, 8), snr, rssi: -100, age: 5, count: 3 });

// A hears B (+6) and an unknown node; B hears A (+5, even), C (-12); C hears B (+2: uneven with B's -12), nothing of D.
h.crawlStart(A, 10, 3, 0.05);
await sleep(5);
assert.equal(sent.length, 1);
assert.equal(sent[0].to, A);
assert(h.crawlOnReply(A, { type: "neighbors", part: 0, last: false, items: [nb(B, 6)] }));
assert.equal(h.crawl.run.current.addr, A, "still waiting for the last part");
assert(h.crawlOnReply(A, { type: "neighbors", part: 1, last: true, items: [nb("ee".repeat(16), -3)] }));
assert.equal(h.crawlOnReply(C, { type: "neighbors", part: 0, last: true, items: [] }), false, "a reply from a node not asked is ignored");
await sleep(70);
assert.equal(sent.length, 2, "B asked after the pause");
assert.equal(sent[1].to, B);
h.crawlOnReply(B, { type: "neighbors", part: 0, last: true, items: [nb(A, 5), nb(C, -12)] });
await sleep(70);
assert.equal(sent[2].to, C);
h.crawlOnReply(C, { type: "neighbors", part: 0, last: true, items: [nb(B, 2), nb(D, 1)] });
await sleep(70);
assert.equal(sent[3].to, D, "D is a neighbour of C and is asked");
await sleep(150); // D never answers
const run = h.crawl.run;
assert(run.finished, "the crawl ends when the queue is empty");
assert.equal(sent.length, 4, "nobody is asked twice");
assert.equal(run.nodes.get(D).state, "silent");

const links = h.crawlLinks(run);
const find = (x, y) => links.find(l => [l.a, l.b].sort().join() === [x, y].sort().join());
assert.equal(find(A, B).note, "", "A and B hear each other about equally");
assert.equal(find(B, C).note, "uneven: one side hears it much better");
assert.equal(find(C, D).ba, undefined, "D never answered, so only one side is known");
assert(links.some(l => l.note === "node not known here"), "the unknown neighbour is listed");

const s = h.crawlSummary(run);
assert.equal(s.asked, 4); assert.equal(s.answered, 3); assert.equal(s.silent, 1); assert.equal(s.odd, 1); assert.equal(s.unknown, 1);
assert(h.crawlCsv().text.split("\n")[0].startsWith("node_a,node_b"));

// Limits: at most 2 nodes
sent.length = 0;
h.crawlStart(A, 2, 3, 0.05);
await sleep(5);
h.crawlOnReply(A, { type: "neighbors", part: 0, last: true, items: [nb(B, 6), nb(C, 1)] });
await sleep(70);
h.crawlOnReply(B, { type: "neighbors", part: 0, last: true, items: [nb(A, 5)] });
await sleep(70);
assert.equal(sent.length, 2, "the node limit stops the crawl");
assert(h.crawl.run.finished);

// An error reply (not an administrator) counts as refused, and stop ends the run.
h.crawlStart(A, 5, 1, 0.05);
await sleep(5);
h.crawlOnReply(A, { type: "error", text: "not allowed" });
assert.equal(h.crawl.run.nodes.get(A).state, "refused");
h.crawlStop();
assert(h.crawl.run.finished);
console.log("OK  network crawl: asking, limits, silent nodes, uneven and one-way links, CSV");
