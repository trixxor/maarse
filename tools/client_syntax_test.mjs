// Tjekker, at webklientens JavaScript kan parses (fanger fx en manglende
// "}" efter en fletning, som ellers stopper hele klienten i browseren).
// Brug: node tools/client_syntax_test.mjs
import { readFileSync } from "node:fs";
import vm from "node:vm";

const html = readFileSync(new URL("../maarse-client/index.html", import.meta.url), "utf8");
const scripts = [...html.matchAll(/<script>([\s\S]*?)<\/script>/g)].map(m => m[1]);
let fejl = 0;
scripts.forEach((src, i) => {
  try {
    new vm.Script(src, { filename: `index.html <script> nr. ${i + 1}` });
  } catch (e) {
    fejl++;
    console.error(`FEJL i script nr. ${i + 1}: ${e.message}`);
  }
});
for (const f of ["sw.js"]) {
  try {
    new vm.Script(readFileSync(new URL(`../maarse-client/${f}`, import.meta.url), "utf8"), { filename: f });
  } catch (e) {
    fejl++;
    console.error(`FEJL i ${f}: ${e.message}`);
  }
}
if (fejl) process.exit(1);
console.log(`OK  webklientens JavaScript kan parses (${scripts.length} scripts + sw.js)`);
