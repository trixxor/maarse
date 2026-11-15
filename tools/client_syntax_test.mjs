// Checks that the web client's JavaScript parses (catches e.g. a missing
// "}" after a merge, which would otherwise stop the whole client in the browser).
// Usage: node tools/client_syntax_test.mjs
import { readFileSync } from "node:fs";
import vm from "node:vm";

const html = readFileSync(new URL("../maarse-client/index.html", import.meta.url), "utf8");
const scripts = [...html.matchAll(/<script>([\s\S]*?)<\/script>/g)].map(m => m[1]);
let errors = 0;
scripts.forEach((src, i) => {
  try {
    new vm.Script(src, { filename: `index.html <script> no. ${i + 1}` });
  } catch (e) {
    errors++;
    console.error(`ERROR in script no. ${i + 1}: ${e.message}`);
  }
});
for (const f of ["sw.js"]) {
  try {
    new vm.Script(readFileSync(new URL(`../maarse-client/${f}`, import.meta.url), "utf8"), { filename: f });
  } catch (e) {
    errors++;
    console.error(`ERROR in ${f}: ${e.message}`);
  }
}
if (errors) process.exit(1);
console.log(`OK  the web client's JavaScript parses (${scripts.length} scripts + sw.js)`);
