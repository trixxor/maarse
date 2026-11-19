// Keeps every control of the Denmark simulator when its menus are moved
// around: each element id of the layout saved in ui_features_baseline.json
// (taken from docs/denmark/index.html before the 2026-11 redesign) must
// still exist, in index.html or created by one of the page's scripts.
// A control that is removed on purpose is taken out of the baseline in the
// same change, with the reason in the commit.
// Usage: node tools/ui_features_test.mjs
import { readFileSync, readdirSync } from "fs";
import { fileURLToPath } from "url";
import path from "path";

const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
const dir = path.join(root, "docs", "denmark");
const baseline = JSON.parse(readFileSync(path.join(root, "tools", "ui_features_baseline.json"), "utf8"));
const html = readFileSync(path.join(dir, "index.html"), "utf8");
const scripts = readdirSync(dir).filter((f) => f.endsWith(".js")).map((f) => readFileSync(path.join(dir, f), "utf8")).join("\n");

const inHtml = new Set([...html.matchAll(/\bid="([^"]+)"/g)].map((m) => m[1]));
let failed = 0;
for (const id of baseline.ids) {
  const made = scripts.includes(`id="${id}"`) || scripts.includes(`.id = "${id}"`);
  if (inHtml.has(id) || made) continue;
  console.log(`FAIL control #${id} is gone`);
  failed++;
}
// Controls that act through data attributes (tools, events, modes) are
// counted by their values.
for (const attr of ["data-tool", "data-dtool", "data-ev"]) {
  const before = (baseline.attrs || {})[attr] || [];
  for (const v of before) if (!html.includes(`${attr}="${v}"`)) { console.log(`FAIL ${attr}="${v}" is gone`); failed++; }
}
const dup = [...html.matchAll(/\bid="([^"]+)"/g)].map((m) => m[1]).filter((v, i, a) => a.indexOf(v) !== i);
for (const id of new Set(dup)) { console.log(`FAIL id "${id}" is used twice`); failed++; }
console.log(`${baseline.ids.length} controls checked`);
if (failed) { console.log(`${failed} problem(s)`); process.exit(1); }
console.log("OK");
