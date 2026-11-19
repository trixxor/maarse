// Bornholm mail chain: who does the Roenne recipient name as its home office?
import { createRequire } from "module";
import { readFileSync } from "fs";
import path from "path";
import { fileURLToPath } from "url";
const require = createRequire(import.meta.url);
const dir = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "..", "docs", "denmark");
globalThis.window = globalThis;
require(path.join(dir, "dk-land.js")); require(path.join(dir, "dk-towns.js"));
const DK = require(path.join(dir, "scenarios.js"));
const { load } = require(path.join(dir, "maarse-sim.js"));
const { Sim } = await load(readFileSync(path.join(dir, "maarse_sim_wasm.wasm")));
const sc = DK.PRESETS.bornholm_alone.build(window.DK_LAND, window.DK_TOWNS, 3);
sc.trace = true;
const msg = sc.events.filter(e => e.type === "message");
const b = msg[0].node;
const label = (i) => i < 0 ? "-" : `${i} ${sc.nodes[i].kind} ${sc.nodes[i].label || ""} (${sc.nodes[i].lat.toFixed(2)},${sc.nodes[i].lon.toFixed(2)})`;
console.log("from", label(msg[0].from), "to", label(b));
const x = new Sim(sc);
const all = []; let since = 0;
for (let t = 0; t < 1800; t += 5) { x.step(5000, 1e9); const r = x.events(since, 1e7, {}); since = r.next; for (const l of r.lines) if (l.ev && l.ev.startsWith("mail")) all.push(l); }
const syncs = all.filter(l => l.ev === "mail_sync" && l.n === b);
const homes = {}; for (const l of syncs) homes[l.home] = (homes[l.home] || 0) + 1;
console.log("syncs by named home:"); for (const [h, c] of Object.entries(homes)) console.log("  ", c, label(+h));
const sends = all.filter(l => l.ev === "mail_send");
const ops = {}; for (const l of sends) if (l.rec === b) { const k = `${l.n}->${l.to} op${l.op}`; ops[k] = (ops[k] || 0) + 1; }
console.log("mail ops about recipient:", ops);
const hosts = sc.nodes.map((n, i) => [n, i]).filter(([n]) => n.kind === "host" && n.lon > 14.5).map(([, i]) => label(i));
console.log("hosts on Bornholm:", hosts);
console.log("delivered:", x.msgs(0, 10).msgs.map(m => m[4]));
const evc = {}; for (const l of all) evc[l.ev] = (evc[l.ev] || 0) + 1; console.log("mail events:", evc);
console.log("summary syncs:", x.summary().mail_syncs); console.log(all.filter(l=>l.ev==="mail_dbg").slice(0,3), all.filter(l=>l.ev==="mail_dbg").slice(-2));
