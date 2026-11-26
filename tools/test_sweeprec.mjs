// Checks the JavaScript port of the spectrum recording (maarse-client/index.html, between the sweeprec markers)
// against the files the Rust code writes (maarse-core/tests/data/sweeprec-golden.*).
// Run: node tools/test_sweeprec.mjs
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const html = fs.readFileSync(path.join(root, "maarse-client/index.html"), "utf8");
const m = html.match(/\/\/ <sweeprec>([\s\S]*?)\/\/ <\/sweeprec>/);
if (!m) { console.error("sweeprec markers not found in index.html"); process.exit(1); }
const SR = new Function(m[1] + "; return SR;")();
const dir = path.join(root, "maarse-core/tests/data");
const input = fs.readFileSync(path.join(dir, "sweeprec-golden-sweeps.bin"));
const want = fs.readFileSync(path.join(dir, "sweeprec-golden.msr"));
const wantGroups = fs.readFileSync(path.join(dir, "sweeprec-golden-groups.txt"), "utf8");
const rec = new SR.Recorder({ startKhz: 863000, stepKhz: 110, durationS: 300, intervalMs: 100, thrDb: 6 });
for (let o = 0; o + 68 <= input.length; o += 68) rec.push(input.readUInt32LE(o), input.subarray(o + 4, o + 68), false);
const got = Buffer.from(rec.finish());
let bad = 0;
if (!got.equals(want)) { console.error(`FILE DIFFERS: got ${got.length} bytes, want ${want.length}`); for (let i = 0; i < Math.min(got.length, want.length); i++) if (got[i] !== want[i]) { console.error("first difference at byte", i, got[i], want[i]); break; } bad++; }
else console.log("file: identical,", got.length, "bytes");
const r = SR.parse(new Uint8Array(want));
const text = r.groups().map(g => `${g.kind} ${g.severity} ${g.b0} ${g.b1} ${g.count} ${g.airMs} ${g.firstMs} ${g.peakDb} ${g.intervalMs}`).join("\n") + "\n";
if (text !== wantGroups) { console.error("GROUPS DIFFER\n" + text + "--- want ---\n" + wantGroups); bad++; } else console.log("groups: identical");
process.exit(bad ? 1 : 0);
