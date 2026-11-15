// Runs the web simulator's model (docs/simulator/engine.js) in Node: every scenario,
// all four designs, and prints a table. Usage: node tools/sim_web_test.mjs [seed]
import { createRequire } from "module";
const require = createRequire(import.meta.url);
const S = require("../docs/simulator/engine.js");
const seed = +(process.argv[2] || 1);
let bad = 0;
for (const id of ["motorway", "forest", "city", "country", "mailbox", "festival", "blackout", "oslo"]) {
  const sc = S.SCENARIOS[id];
  const opt = { ...S.DEFAULTS, f: { ...S.DEFAULTS.f }, ...sc.defaults, seed };
  const t0 = Date.now();
  const r = S.runAll(id, opt);
  const cells = S.PIDS.map(p => {
    const v = r[p];
    if (!(v.delivered >= 0 && v.delivered <= 1) || !isFinite(v.air)) bad++;
    return `${p} ${(v.delivered * 100).toFixed(0).padStart(3)}%${v.late ? "(" + v.late + "L)" : ""} ${v.air.toFixed(0).padStart(4)}s`;
  });
  console.log(id.padEnd(9), cells.join(" | "), `${Date.now() - t0}ms`);
}
const rc = S.race("motorway", { ...S.DEFAULTS, f: { ...S.DEFAULTS.f }, ...S.SCENARIOS.motorway.defaults }, null, 0, true);
console.log("race:", rc && Object.entries(rc.lanes).map(([p, l]) => `${p} ${l.ok ? "ok" : "fail"} ${l.steps.length} steps`).join(", "), "ko", rc?.ko);
if (bad) { console.log("FAIL: invalid numbers"); process.exit(1); }
console.log("OK");
