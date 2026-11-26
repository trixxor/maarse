// Tries recording settings on captured sweeps (tools/capture_sweeps.py) and prints the size of each file (MSR2) and what it still shows.
// Run: node tools/msr_sizes.mjs sweeps.bin [duration_s]
import fs from "node:fs";
import zlib from "node:zlib";
import path from "node:path";
import { fileURLToPath } from "node:url";
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const html = fs.readFileSync(path.join(root, "maarse-client/index.html"), "utf8");
const SR = new Function(html.match(/\/\/ <sweeprec>([\s\S]*?)\/\/ <\/sweeprec>/)[1] + "; return SR;")();
const input = fs.readFileSync(process.argv[2]);
const durationS = +(process.argv[3] || 300);
const rows = [];
for (let o = 0; o + 68 <= input.length; o += 68) rows.push([input.readUInt32LE(o), input.subarray(o + 4, o + 68)]);
function make(thr, every = 1) {
  const rec = new SR.Recorder({ startKhz: 863000, stepKhz: 110, durationS, intervalMs: 100, thrDb: thr });
  rows.forEach(([t, r], i) => { if (i % every === 0) rec.push(t, r, SR.isOverload(r)); });
  return Buffer.from(rec.finish());
}
const pad = (v, n) => String(v).padStart(n);
console.log(`${rows.length} sweeps over ${durationS} s, ${(rows.length * 68 / 1024).toFixed(0)} KB raw\n`);
console.log("sweeps used (every Nth)   threshold    file  deflate  events  groups  stand-out");
for (const every of [1, 2, 5, 10, 20]) for (const thr of [6, 9, 12]) {
  const b = make(thr, every), r = SR.parse(new Uint8Array(b)), g = r.groups();
  console.log(`${pad("1/" + every + " (" + (10.3 / every).toFixed(1) + "/s)", 18)} ${pad(thr + " dB", 12)} ${pad(b.length, 7)} ${pad(zlib.deflateRawSync(b, { level: 9 }).length, 8)} ${pad(r.events.length, 7)} ${pad(g.length, 7)} ${pad(g.filter(x => x.severity !== "Low").length, 10)}`);
}
