// JS side of maarse-sim-wasm: JSON in, JSON out through one shared buffer.
// Works in the browser (window.MaarseSim) and in Node (module.exports).
(function (root) {
  "use strict";
  const enc = new TextEncoder();
  const dec = new TextDecoder();

  async function load(source) {
    let bytes;
    if (source instanceof ArrayBuffer || ArrayBuffer.isView(source)) bytes = source;
    else bytes = await (await fetch(source || "maarse_sim_wasm.wasm")).arrayBuffer();
    const { instance } = await WebAssembly.instantiate(bytes, {});
    const x = instance.exports;
    function put(str) {
      const b = enc.encode(str);
      const p = x.buf_alloc(b.length);
      new Uint8Array(x.memory.buffer, p, b.length).set(b);
    }
    function out(len) {
      const p = x.out_ptr();
      return JSON.parse(dec.decode(new Uint8Array(x.memory.buffer, p, len)));
    }
    class Sim {
      constructor(scenario) {
        put(JSON.stringify(scenario));
        this.h = x.sim_create();
        if (!this.h) {
          throw new Error("scenario rejected: " + out(x.out_len()).error);
        }
      }
      call(cmd) {
        put(JSON.stringify(cmd));
        const r = out(x.sim_call(this.h));
        if (r && r.error) throw new Error(r.error);
        return r;
      }
      step(ms, maxEvents) { return this.call({ cmd: "step", ms, max_events: maxEvents }); }
      summary() { return this.call({ cmd: "summary" }); }
      state() { return this.call({ cmd: "state" }); }
      inspect(node) { return this.call({ cmd: "inspect", node }); }
      events(since, max, filter) { return this.call(Object.assign({ cmd: "events", since, max }, filter || {})); }
      reach(pid) { return this.call({ cmd: "reach", pid }); }
      announce(node) { return this.call({ cmd: "announce", node }); }
      send(from, to) { return this.call({ cmd: "send", from, to }); }
      inject(event) { return this.call({ cmd: "inject", event }); }
      msgs(from, max) { return this.call({ cmd: "msgs", from: from || 0, max: max || 500 }); }
      follow(msg) { return this.call({ cmd: "follow", msg }); }
      alert(index) { return this.call({ cmd: "alert", index }); }
      runToEnd() { return this.call({ cmd: "run_to_end" }); }
      free() { if (this.h) x.sim_free(this.h); this.h = 0; }
    }
    return { Sim };
  }

  const api = { load };
  if (typeof module !== "undefined" && module.exports) module.exports = api;
  else root.MaarseSim = api;
})(typeof window !== "undefined" ? window : globalThis);
