// Code reused from the real web client (../client.html, a copy of
// maarse-client/index.html) instead of copied: the Må:rse markup (MU)
// renderer with its styles, and the Codec2 voice codec (maarse-voice as
// WebAssembly). The Android app renders MU with Mu.kt, which follows "the same
// rules as the web client's renderer"; running the web client's own code here
// keeps the simulated phone's pages identical to the real ones.
"use strict";

const Reuse = (() => {
  let src = null;
  const load = async () => (src ??= await (await fetch("../client.html", { cache: "no-store" })).text());
  const between = (s, a, b) => { const i = s.indexOf(a), j = s.indexOf(b, i + 1); return i >= 0 && j > i ? s.slice(i, j) : null; };

  /** The client's page styles: only rules about `.page` and `.mu-…`, so nothing else leaks in. */
  function pageCss(html) {
    const css = (html.match(/<style[^>]*>([\s\S]*?)<\/style>/i) || [])[1] || "";
    const sheet = new CSSStyleSheet();
    try { sheet.replaceSync(css); } catch (e) { return ""; }
    const keep = (sel) => /\.page\b|\.mu-/.test(sel);
    const out = [];
    for (const r of sheet.cssRules) {
      if (r.selectorText !== undefined) { if (keep(r.selectorText)) out.push(r.cssText); }
      else if (r.cssRules) {
        const inner = [...r.cssRules].filter((x) => x.selectorText && keep(x.selectorText)).map((x) => x.cssText);
        if (inner.length) out.push(`@media ${r.conditionText || r.media?.mediaText} { ${inner.join("\n")} }`);
      } else if (r.cssText.startsWith("@keyframes") && /mu/.test(r.name || "")) out.push(r.cssText);
    }
    return out.join("\n");
  }

  /**
   * The MU section of the client, run with the app's own page state in place
   * of the client's globals. `env` gives: pages, state, openPage, label,
   * parsePageUrl, pageUrl, pageKey, pagePicture, transport, Cmd.
   */
  async function markup(env) {
    const html = await load();
    const code = between(html, "// ---------- Må:rse markup (MU) ----------", "// ---------- voice messages (Codec2) ----------");
    if (!code) throw new Error("the MU renderer was not found in client.html");
    const style = document.createElement("style");
    style.textContent = pageCss(html);
    document.head.append(style);
    const el = (tag, props = {}, ...kids) => { const e = document.createElement(tag); Object.assign(e, props); for (const k of kids) e.append(k); return e; };
    const $ = () => document.createElement("div"); // the client's own header, not shown here
    const names = ["el", "$", "pages", "state", "openPage", "label", "parsePageUrl", "pageUrl", "pageKey", "pagePicture", "transport", "Cmd", "store", "log", "siteTitle", "requestPicture"];
    const f = new Function(...names, code + "\n;return { renderMarkup, pageTitle, pageSettings };");
    return f(el, $, env.pages, env.state, env.openPage, env.label, env.parsePageUrl, env.pageUrl, env.pageKey, env.pagePicture, env.transport, env.Cmd,
      { get: (k, d) => d, set: () => {} }, env.log, env.siteTitle, env.requestPicture);
  }

  // ---------- voice (Codec2, 1200 bit/s, 8 kHz) ----------
  let voice = null;
  async function voiceModule() {
    if (voice) return voice;
    const html = await load();
    const m = html.match(/<script type="application\/octet-stream" id="voice-wasm">\s*([\s\S]*?)\s*<\/script>/);
    if (!m) throw new Error("the voice codec was not found in client.html");
    const raw = Uint8Array.from(atob(m[1].trim()), (c) => c.charCodeAt(0));
    const bytes = await new Response(new Blob([raw]).stream().pipeThrough(new DecompressionStream("deflate-raw"))).arrayBuffer();
    voice = (await WebAssembly.instantiate(bytes, {})).instance.exports;
    return voice;
  }
  async function voiceCall(fn, input, ...args) {
    const w = await voiceModule();
    const p = w.voice_alloc(input.byteLength);
    new Uint8Array(w.memory.buffer, p, input.byteLength).set(new Uint8Array(input.buffer, input.byteOffset, input.byteLength));
    const n = w[fn](p, fn === "voice_encode" ? input.length : input.byteLength, ...args);
    w.voice_free(p, input.byteLength);
    return new Uint8Array(w.memory.buffer, w.voice_result_ptr(), n).slice();
  }
  const voiceEncode = (pcm) => voiceCall("voice_encode", pcm, 5);
  async function voiceDecode(data) { const b = await voiceCall("voice_decode", data); return new Int16Array(b.buffer, 0, b.length >> 1); }
  function wavBlob(pcm, rate = 8000) {
    const h = new DataView(new ArrayBuffer(44));
    const str = (o, s) => [...s].forEach((c, i) => h.setUint8(o + i, c.charCodeAt(0)));
    str(0, "RIFF"); h.setUint32(4, 36 + pcm.byteLength, true); str(8, "WAVE"); str(12, "fmt ");
    h.setUint32(16, 16, true); h.setUint16(20, 1, true); h.setUint16(22, 1, true); h.setUint32(24, rate, true);
    h.setUint32(28, rate * 2, true); h.setUint16(32, 2, true); h.setUint16(34, 16, true); str(36, "data");
    h.setUint32(40, pcm.byteLength, true);
    return new Blob([h, pcm], { type: "audio/wav" });
  }

  return { markup, voiceEncode, voiceDecode, wavBlob };
})();
