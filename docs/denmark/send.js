// "What to send" in the Send mode: text, voice message, picture, file, page,
// call, SSH, ping or bulk copy between the From and To of the journey form.
// The engine uses the host's real rules (maarse-hostrules): route kind, file
// limits, file mailbox, call refusal, page burst rule, tunnel framing.
// Text goes the existing way (followed on the map); everything else shows
// its state in a small result box that follows the item while it runs.
(function () {
  "use strict";
  const $ = (id) => document.getElementById(id);
  const box = $("send-what");
  if (!box) return;
  const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));

  const KINDS = [
    ["text", "Text message"],
    ["voice", "Voice message"],
    ["picture", "Picture"],
    ["file", "File"],
    ["page", "Page (To = the host)"],
    ["call", "Call"],
    ["ssh", "SSH (IP tunnel)"],
    ["ping", "Ping (IP tunnel)"],
    ["bulk", "Bulk copy (IP tunnel)"],
  ];
  box.innerHTML = `
    <h3>What to send</h3>
    <label>Send <select id="sw-kind">${KINDS.map(([k, t]) => `<option value="${k}">${t}</option>`).join("")}</select></label>
    <span id="sw-opts"></span>
    <button id="sw-go">Send</button>
    <div id="sw-result" class="hint"></div>`;

  const opts = {
    text: "",
    voice: `<label>Seconds <input id="sw-secs" type="number" min="1" max="60" value="8" size="4"></label>`,
    picture: `<label>Copy <select id="sw-quality"><option value="grey">Greyscale (~1.4 KB)</option><option value="c4" selected>Colour 4 KB</option><option value="c8">Colour 8 KB</option></select></label>`,
    file: `<label>KB <input id="sw-kb" type="number" min="1" max="1024" value="4" size="5"></label>`,
    page: `<label>Path <input id="sw-path" value="index" size="14" title="e.g. weather/aarhus, or _grey/map.jpg for a picture"></label>`,
    call: `<label>Seconds <input id="sw-secs" type="number" min="1" max="600" value="10" size="4"></label> <label title="Only when the callee runs the call test service (the Copenhagen host in generated maps)">Test service <select id="sw-mode"><option value="music">music</option><option value="echo">echo</option><option value="speed">speed (5 s)</option></select></label>`,
    ssh: `<label><input type="checkbox" id="sw-tunlora"> Allow over LoRa (--tun-lora)</label>`,
    ping: `<label><input type="checkbox" id="sw-tunlora"> Allow over LoRa (--tun-lora)</label>`,
    bulk: `<label>KB <input id="sw-kb" type="number" min="1" max="10240" value="64" size="5"></label> <label><input type="checkbox" id="sw-tunlora"> Allow over LoRa</label>`,
  };
  const kind = $("sw-kind");
  const showOpts = () => { $("sw-opts").innerHTML = opts[kind.value] || ""; };
  kind.onchange = showOpts;
  showOpts();

  const num = (id, d) => { const e = $(id); const v = e ? +e.value : NaN; return Number.isFinite(v) && v > 0 ? v : d; };
  let timer = null;
  let current = -1;

  function describe(v) {
    const S = window.DkSim;
    const nm = (i) => (S && S.name ? S.name(i) : "node " + i);
    if (v.state === "refused" && v.item === undefined) return `<b>Refused</b>: ${esc(v.why)}`;
    const lines = [];
    const kindTxt = { file: "File", voice: "Voice message", picture: "Picture", page: "Page", page_reply: "Page answer", call: "Call", tunnel: "IP tunnel" }[v.kind] || v.kind;
    lines.push(`<b>${kindTxt}</b> ${esc(nm(v.from))} → ${esc(nm(v.to))} · route <b>${esc(v.route)}</b>`);
    const st = {
      queued: "Queued (offer on its way)", asked: "Page asked for", answering: "Host is sending the page",
      sending: `Sending: ${v.done}/${v.chunks} chunks`, mailbox: `Waiting in the file mailbox${v.mailbox_host >= 0 ? " at " + esc(nm(v.mailbox_host)) : ""}`,
      delivered: "Delivered", refused: "Refused", failed: "Failed", cancelled: "Cancelled",
      ringing: "Ringing", active: "Active", done: "Finished",
    }[v.state] || v.state;
    lines.push(`State: <b>${esc(st)}</b>${v.why ? " – " + esc(v.why) : ""}`);
    if (v.bytes) lines.push(`${v.bytes} bytes${v.chunks ? `, ${v.chunks} chunks` : ""}${v.from_box ? ", from the file mailbox" : ""}`);
    if (v.delivered_ms !== null && v.delivered_ms !== undefined) lines.push(`Took ${((v.delivered_ms - v.created_ms) / 1000).toFixed(1)} s`);
    if (v.call) lines.push(`Audio packets ${v.call.recv[0] + v.call.recv[1]}/${v.call.sent[0] + v.call.sent[1]}, latency median ${v.call.latency_median_ms ?? "–"} ms, p95 ${v.call.latency_p95_ms ?? "–"} ms`);
    if (v.tunnel) {
      const t = v.tunnel;
      lines.push(`${esc(t.workload)}: ${t.answered}/${t.total} answered, round trip median ${t.rtt_median_ms ?? "–"} ms, p95 ${t.rtt_p95_ms ?? "–"} ms` + (t.kbps !== null ? `, ${t.kbps.toFixed(2)} KB/s` : "") + `, ${t.resent} resent, ${t.nacks} NACKs`);
    }
    const busy = !["delivered", "refused", "failed", "cancelled", "done"].includes(v.state);
    if (busy && ["file", "voice", "picture"].includes(v.kind)) lines.push(`<button id="sw-cancel">Cancel (sender)</button> <button id="sw-cancel-r">Cancel (recipient)</button>`);
    return lines.join("<br>");
  }

  function poll() {
    const S = window.DkSim;
    const sim = S && S.sim();
    if (!sim || current < 0) return;
    let v;
    try { v = sim.call({ cmd: "item", item: current }); } catch (e) { return; }
    $("sw-result").innerHTML = describe(v);
    const c1 = $("sw-cancel"), c2 = $("sw-cancel-r");
    if (c1) c1.onclick = () => { sim.call({ cmd: "cancel", item: current, by: v.from }); poll(); };
    if (c2) c2.onclick = () => { sim.call({ cmd: "cancel", item: current, by: v.to }); poll(); };
    if (["delivered", "refused", "failed", "cancelled", "done"].includes(v.state)) { clearInterval(timer); timer = null; }
  }

  $("sw-go").onclick = () => {
    const S = window.DkSim;
    const sim = S && S.sim();
    if (!sim) return;
    const a = S.resolveEnd($("jr-from").value), b = S.resolveEnd($("jr-to").value);
    if (a < 0 || b < 0 || a === b) { $("sw-result").innerHTML = "Pick two different nodes in From and To above."; return; }
    const k = kind.value;
    if (k === "text") { $("btn-journey").click(); return; }
    const lora = !!($("sw-tunlora") && $("sw-tunlora").checked);
    let cmd;
    if (k === "voice") cmd = { cmd: "send_file", from: a, to: b, kind: "voice", secs: num("sw-secs", 8) };
    else if (k === "picture") cmd = { cmd: "send_file", from: a, to: b, kind: "picture", quality: $("sw-quality").value };
    else if (k === "file") cmd = { cmd: "send_file", from: a, to: b, kind: "file", bytes: Math.round(num("sw-kb", 4) * 1024) };
    else if (k === "page") cmd = { cmd: "page", from: a, host: b, path: ($("sw-path").value || "index").trim() };
    else if (k === "call") cmd = { cmd: "call", from: a, to: b, secs: num("sw-secs", 10), mode: ($("sw-mode") || {}).value || "music" };
    else cmd = { cmd: "tunnel", from: a, to: b, workload: k, kb: num("sw-kb", 64), tun_lora: lora };
    let v;
    try { v = sim.call(cmd); } catch (e) { $("sw-result").textContent = e.message; return; }
    current = v.item === undefined ? -1 : v.item;
    $("sw-result").innerHTML = describe(v);
    if (current >= 0) {
      S.play(true);
      if (timer) clearInterval(timer);
      timer = setInterval(poll, 500);
    }
  };
})();
