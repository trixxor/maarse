// Prints what is in a spectrum recording (.msr), using the same reader as the web client.
// Run: node tools/msr_info.mjs file.msr
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const html = fs.readFileSync(path.join(root, "maarse-client/index.html"), "utf8");
const m = html.match(/\/\/ <sweeprec>([\s\S]*?)\/\/ <\/sweeprec>/);
const SR = new Function(m[1] + "; return SR;")();
const bytes = new Uint8Array(fs.readFileSync(process.argv[2]));
const r = SR.parse(bytes);
console.log(`${bytes.length} bytes: ${r.durationS} s, ${r.sweeps} sweeps, ${r.buckets} buckets of ${r.bucketMs} ms, interval ${r.intervalMs} ms, threshold ${r.thrDb} dB`);
console.log(`band ${r.startKhz / 1000} MHz + ${r.stepKhz} kHz x ${r.floor.length}`);
const top = [...r.duty.keys()].sort((a, b) => r.duty[b] - r.duty[a]).slice(0, 5);
console.log("floor dBm (min/max):", Math.min(...r.floor), Math.max(...r.floor), " strongest dBm:", Math.max(...r.max));
console.log("busiest steps:", top.map(b => `${((r.startKhz + b * r.stepKhz) / 1000).toFixed(2)} MHz ${r.duty[b]}%`).join(", "));
console.log("masked buckets:", r.masked.filter(Boolean).length);
for (const g of r.groups()) console.log(`  ${g.kind} ${g.severity}: steps ${g.b0}-${g.b1}, ${g.count}x, air ${g.airMs} ms, peak ${g.peakDb} dB over floor`);
