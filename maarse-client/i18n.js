// Language support for the Må:rse web pages (client, server admin, login).
//
// The English text in the page is the lookup key: nothing in the pages is renamed.
// A language file (i18n/<code>.json, served as /i18n/<code>.json) maps English text to
// the translation. This script translates text nodes and the attributes title,
// placeholder, aria-label and alt, both for static HTML and for everything the page
// builds later. A text that is missing in the language stays English.
//
// Entries may contain {0}, {1} ... for the variable parts ("Sent {0} messages");
// the variable parts are translated too, so "Status: {0}" works with a translated state.
// Elements marked translate="no" (and scripts, inputs, text areas) are never touched.
// English as language: nothing is done at all.
"use strict";
(function () {
  const CHOICE_KEY = "maarse:lang";          // "auto" or a language code
  const DICT_KEY = "maarse:i18n:";           // cached dictionary per language code
  const LIST_KEY = "maarse:i18n:list";       // cached list of available languages
  const ATTRS = ["title", "placeholder", "aria-label", "alt"];
  const SKIP = "script,style,textarea,input,[contenteditable=true],[translate=no]";
  const SKIP_ATTR = "[translate=no]";       // placeholders and titles of inputs are translated

  // Language files are fetched next to this script (the host serves /i18n.js and /i18n/..., the
  // flasher site keeps them in its own folder).
  const BASE = (() => { try { return new URL(".", document.currentScript.src).href; } catch { return "/"; } })();
  const st = { lang: "en", dict: null, pats: [], cache: new Map(), list: [], listeners: [] };
  const nodeRec = new WeakMap();   // text node -> { o: original text, s: shown text }
  const attrRec = new WeakMap();   // element -> { attr: { o, s } }
  const titleRec = { o: null, s: null };   // the page title: original and shown
  let observer = null, busy = false;

  const lsGet = k => { try { return localStorage.getItem(k); } catch { return null; } };
  const lsSet = (k, v) => { try { localStorage.setItem(k, v); } catch { /* private mode */ } };
  const norm = s => s.replace(/\s+/g, " ").trim();
  const esc = s => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

  function compile(texts) {
    const dict = new Map(), pats = [];
    for (const [k, v] of Object.entries(texts || {})) {
      if (typeof v !== "string" || !v) continue;
      const key = norm(k);
      if (/\{\d+\}/.test(key)) {
        const parts = key.split(/\{\d+\}/);
        const re = new RegExp("^" + parts.map(esc).join("(.+?)") + "$");
        pats.push({ re, pre: parts[0], suf: parts[parts.length - 1], min: parts.join("").length, out: v });
      } else dict.set(key, v);
    }
    // Longest fixed text first, so the most specific pattern wins.
    pats.sort((a, b) => b.min - a.min);
    return { dict, pats };
  }

  function core(c) {
    if (st.cache.has(c)) return st.cache.get(c);
    let r = st.dict.get(c);
    if (r === undefined) {
      for (const p of st.pats) {
        if (c.length < p.min || (p.pre && !c.startsWith(p.pre)) || (p.suf && !c.endsWith(p.suf))) continue;
        const m = p.re.exec(c);
        if (m) { r = p.out.replace(/\{(\d+)\}/g, (_, i) => (m[+i + 1] === undefined ? "" : core(m[+i + 1]))); break; }
      }
    }
    if (r === undefined && /[.!?] \S/.test(c)) {
      // A paragraph made of several known sentences: translate each one.
      const parts = c.split(/(?<=[.!?]) (?=\S)/);
      if (parts.length > 1) {
        let any = false;
        const out = parts.map(x => { const y = core(x); if (y !== x) any = true; return y; });
        if (any) r = out.join(" ");
      }
    }
    if (r === undefined) r = c;
    if (st.cache.size > 6000) st.cache.clear();
    st.cache.set(c, r);
    return r;
  }

  // Translates a text, keeping the white space around it.
  function t(text, ...args) {
    let s = String(text);
    if (st.dict && /[A-Za-z]/.test(s)) {
      const m = /^(\s*)([\s\S]*?)(\s*)$/.exec(s);
      if (m[2]) { const r = core(norm(m[2])); if (r !== norm(m[2])) s = m[1] + r + m[3]; }
    }
    if (args.length) s = s.replace(/\{(\d+)\}/g, (_, i) => (args[+i] === undefined ? "" : args[+i]));
    return s;
  }

  // ---------- DOM ----------
  function skipped(node, sel = SKIP) {
    const p = node.nodeType === 3 ? node.parentElement : node;
    return !p || !!p.closest(sel);
  }

  function doText(node) {
    const rec = nodeRec.get(node);
    let orig = node.nodeValue;
    if (rec && rec.s === orig) orig = rec.o;           // our own earlier change: start from the original
    if (!/[A-Za-z]/.test(orig) || skipped(node)) { if (rec) nodeRec.delete(node); return; }
    const out = st.dict ? t(orig) : orig;
    if (out !== node.nodeValue) node.nodeValue = out;
    if (out !== orig) nodeRec.set(node, { o: orig, s: out }); else if (rec) nodeRec.delete(node);
  }

  function doAttrs(elm) {
    if (elm.nodeType !== 1) return;
    let recs = attrRec.get(elm);
    for (const a of ATTRS) {
      if (!elm.hasAttribute(a)) continue;
      const cur = elm.getAttribute(a);
      let orig = cur;
      if (recs && recs[a] && recs[a].s === cur) orig = recs[a].o;
      if (!/[A-Za-z]/.test(orig) || skipped(elm, SKIP_ATTR)) continue;
      const out = st.dict ? t(orig) : orig;
      if (out !== cur) elm.setAttribute(a, out);
      if (out !== orig) { if (!recs) { recs = {}; attrRec.set(elm, recs); } recs[a] = { o: orig, s: out }; }
      else if (recs) delete recs[a];
    }
  }

  function doTree(root) {
    if (!root) return;
    if (root.nodeType === 3) return doText(root);
    if (root.nodeType !== 1 && root.nodeType !== 9 && root.nodeType !== 11) return;
    const w = document.createTreeWalker(root, NodeFilter.SHOW_ELEMENT | NodeFilter.SHOW_TEXT);
    if (root.nodeType === 1) doAttrs(root);
    for (let n = w.nextNode(); n; n = w.nextNode()) n.nodeType === 3 ? doText(n) : doAttrs(n);
  }

  function apply() {
    busy = true;
    try {
      document.documentElement.lang = st.lang;
      if (titleRec.o === null || document.title !== titleRec.s) titleRec.o = document.title;
      titleRec.s = t(titleRec.o);
      if (document.title !== titleRec.s) document.title = titleRec.s;
      if (document.body) doTree(document.body);
    } finally { busy = false; if (observer) observer.takeRecords(); }
  }

  function start() {
    if (observer || !document.body) return;
    observer = new MutationObserver(list => {
      if (busy || !st.dict) return;
      busy = true;
      try {
        for (const m of list) {
          if (m.type === "characterData") doText(m.target);
          else if (m.type === "attributes") doAttrs(m.target);
          else for (const n of m.addedNodes) doTree(n);
        }
      } finally { busy = false; observer.takeRecords(); }
    });
    observer.observe(document.body, { subtree: true, childList: true, characterData: true, attributes: true, attributeFilter: ATTRS });
  }

  // ---------- languages ----------
  function useDict(code, texts) {
    st.lang = code;
    st.cache.clear();
    if (code === "en" || !texts) { st.dict = null; st.pats = []; }
    else { const c = compile(texts); st.dict = c.dict; st.pats = c.pats; }
  }

  function resolve(choice, list) {
    if (choice && choice !== "auto") return choice;
    const have = new Set(list.map(l => l.code));
    for (const l of (navigator.languages && navigator.languages.length ? navigator.languages : [navigator.language || "en"])) {
      const base = String(l).toLowerCase().split("-")[0];
      if (base === "en") return "en";
      if (have.has(String(l).toLowerCase())) return String(l).toLowerCase();
      if (have.has(base)) return base;
    }
    return "en";
  }

  async function fetchJson(url) {
    const r = await fetch(url, { cache: "no-cache" });
    if (!r.ok) throw new Error(String(r.status));
    return r.json();
  }

  function notify() { for (const f of st.listeners) { try { f(st.lang); } catch { /* listener failed */ } } }

  async function load(code) {
    if (code === "en") return null;
    try {
      const j = await fetchJson(BASE + "i18n/" + encodeURIComponent(code) + ".json");
      if (j && j.texts) { lsSet(DICT_KEY + code, JSON.stringify(j.texts)); return j.texts; }
    } catch { /* offline or not served here: use the cached copy */ }
    try { return JSON.parse(lsGet(DICT_KEY + code)); } catch { return null; }
  }

  async function init() {
    // Quick start from the cache, so the page is not shown in English first.
    let list = [];
    try { list = JSON.parse(lsGet(LIST_KEY)) || []; } catch { list = []; }
    st.list = list;
    const choice = lsGet(CHOICE_KEY) || "auto";
    let code = resolve(choice, list);
    if (code !== "en") { try { useDict(code, JSON.parse(lsGet(DICT_KEY + code))); } catch { /* none cached */ } }
    const go = () => { start(); apply(); };
    if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", go); else go();
    // Then the fresh list and dictionary from the host.
    try {
      const l = await fetchJson(BASE + "i18n/index.json");
      if (Array.isArray(l)) { st.list = l; lsSet(LIST_KEY, JSON.stringify(l)); }
    } catch { /* not served here */ }
    code = resolve(choice, st.list);
    const texts = await load(code);
    useDict(texts ? code : "en", texts);
    go();
    notify();
  }

  async function setLanguage(choice) {
    lsSet(CHOICE_KEY, choice);
    const code = resolve(choice, st.list);
    const texts = await load(code);
    useDict(texts ? code : "en", texts);
    start();
    apply();
    notify();
  }

  const ready = init();
  window.t = t;
  window.I18N = {
    t, ready, setLanguage,
    choice: () => lsGet(CHOICE_KEY) || "auto",
    current: () => st.lang,
    languages: () => st.list.slice(),
    onChange: f => st.listeners.push(f),
    // For checking: visible texts that were not translated (same in both languages, or missing).
    untranslated() {
      const out = new Set();
      const w = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
      for (let n = w.nextNode(); n; n = w.nextNode()) {
        const v = norm(n.nodeValue);
        const rec = nodeRec.get(n);
        if (/[A-Za-z]{3}/.test(v) && !skipped(n) && !(rec && rec.s === n.nodeValue) && n.parentElement && n.parentElement.offsetParent) out.add(v);
      }
      return [...out];
    },
  };

  // Browser dialogs: translate the message.
  for (const f of ["alert", "confirm", "prompt"]) {
    const orig = window[f].bind(window);
    window[f] = (msg, ...rest) => orig(t(msg), ...rest);
  }
})();
