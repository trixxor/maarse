// The Android app as a web page, for the Denmark simulator's phones.
//
// A replica of the Compose screens in android/app/src/main/java/dk/maarse/app/ui
// (Shell.kt: five places Chats, Pages, Nodes, Network, Settings; Chats.kt,
// Nodes.kt, Network.kt, Settings.kt, Account.kt, PagesScreen.kt, CallScreen.kt,
// AnnouncesScreen.kt) with the same wording, and the node logic of Node.kt,
// Pages.kt and Calls.kt: the same companion commands, sent in the same order.
//
// Transport: the parent page (phone.js) hands our command frames to the engine
// (phone_cmd) and posts the node's event frames back ({simFrames, simAlive}).
// URL: ?phone=A&kind=app|handheld|host|...&node=<label>
"use strict";

const Q = new URLSearchParams(location.search);
const PHONE = Q.get("phone") || "A";
const KIND = Q.get("kind") || "app";
const NODE_LABEL = Q.get("node") || "";
// How this phone connects (Shell.kt Mode): an app-only phone is the node
// itself (PHONE); a handheld is "through my radio only" (BLE); a host is
// followed from the phone (REMOTE).
// A phone added with internet and a LoRa radio is the phone node with the
// radio as its modem (inet=1&radio=1).
const INET = Q.get("inet") === "1" || KIND === "app";
const MODE = KIND === "host" || KIND === "server" ? "REMOTE" : INET ? "PHONE" : "BLE";
const HAS_RADIO = MODE === "BLE" || Q.get("radio") === "1";

const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const $ = (s, r = document) => r.querySelector(s);

// ---------- node state (Node.kt) ----------
const N = {
  online: true, alive: true, offlineWhy: "",
  me: null, infoAt: 0,
  stats: {}, neighbors: [], log: [], channels: {}, nodes: {}, nick: {}, announces: [],
  msgs: [], nextRow: 1, unread: {}, pending: {}, pendingFiles: {}, fileOut: {}, fileIn: {},
  person: null, lastError: null, traces: [], traceSent: {}, traceRtt: {}, regions: null, sharing: null, radio: null, backbones: [], backbonesNext: [],
  alerts: [], dismissed: new Set(), openConv: null, nextReq: 0, fastPoll: false, fileLimitWaiter: null,
  favUsers: [], notSim: [],
};
const req = () => { N.nextReq = (N.nextReq % 0x6fff) + 1; return N.nextReq; };

function send(f) {
  if (!N.online) return false;
  parent.postMessage({ simPhone: PHONE, frame: Array.from(f) }, "*");
  return true;
}
function note(s) {
  const t = new Date().toTimeString().slice(0, 8);
  N.log = (N.log.concat(`${t}  ${s}`)).slice(-300);
}
const label = (a) => N.nick[a] || (N.nodes[a]?.name ? "~" + N.nodes[a].name : shortAddr(a));
function convTitle(conv) {
  const [k, a] = conv.split(/:(.*)/s);
  return k === "ch" ? "#" + (N.channels[a] || shortAddr(a)) : label(a);
}
const maxText = (conv) => (conv.startsWith("ch:") ? 190 : (N.me?.maxText || 148));
const bytes = (s) => new TextEncoder().encode(s).length;
function heard(addr, f) {
  if (addr === N.me?.addr) return;
  const n = f(Object.assign({ addr, hops: null, lastHeard: 0, name: null }, N.nodes[addr]));
  n.lastHeard = Date.now();
  N.nodes[addr] = n;
}
const byMsgId = (id) => N.msgs.find((m) => m.msgId !== 0n && m.msgId === id);
function setStatus(row, s, msgId) { const m = N.msgs.find((x) => x.id === row); if (m) { m.status = s; if (msgId !== undefined) m.msgId = msgId; } }
function insert(m) { m.id = N.nextRow++; m.msgId = m.msgId ?? 0n; m.hops = m.hops ?? 0; N.msgs.push(m); return m.id; }

function onOpen() {
  send(Cmd.getInfo()); send(Cmd.getName()); send(Cmd.setTime()); send(Cmd.getStats()); send(Cmd.getRegions()); send(Cmd.getSharing());
  if (MODE === "BLE") send(Cmd.getRadio());
  if (MODE !== "BLE") send(Cmd.getPerson());
  note("Connected (simulated node" + (NODE_LABEL ? " " + NODE_LABEL : "") + ")");
}

function sendText(conv, text) {
  const t = text.trim();
  if (!t) return null;
  if (bytes(t) > maxText(conv)) return "The message is too long";
  if (!N.alive) { insert({ conv, mine: true, from: N.me?.addr || "", text: t, ts: Date.now(), status: "queued" }); return null; }
  const id = insert({ conv, mine: true, from: N.me?.addr || "", text: t, ts: Date.now(), status: "sending…" });
  transmit(id, conv, t);
  return null;
}
function transmit(row, conv, t) {
  const [k, a] = conv.split(/:(.*)/s);
  const r = req();
  N.pending[r] = row;
  send(k === "ch" ? Cmd.sendChannel(r, a, t) : Cmd.sendDirect(r, a, t));
}
function flushQueued() {
  if (!N.alive) return;
  const q = N.msgs.filter((m) => m.status === "queued");
  if (!q.length) return;
  q.forEach((m) => { m.status = "sending…"; transmit(m.id, m.conv, m.text); });
  note(`Sent ${q.length} message(s) written while offline`);
}
const joinChannel = (name) => send(Cmd.join(name.trim().replace(/^#/, "")));
function trace(dest) { N.traceSent[dest] = Date.now(); send(Cmd.traceRoute(req(), dest)); }
function personSetup(op, text = "") { N.lastError = null; send(Cmd.personSetup(req(), op, text)); }

// Files (Node.kt sendFile / queryFileLimit / onFileEvent)
const VOICE_MIME = "audio/x-codec2", LORA_FILE = 8192;
const fileMax = () => { const m = N.me?.maxFile > 0 ? N.me.maxFile : 8192; return MODE === "BLE" ? Math.min(m, 8192) : m; };
function fileLabel(mime, name, data) {
  if (mime === VOICE_MIME) return "Voice message" + (data ? ` · ${voiceSecs(data)} s` : "");
  if (mime.startsWith("image/")) return "Photo";
  return `${name} (${Math.ceil((data?.length || 0) / 1024)} KB)`;
}
function voiceSecs(d) { if (d.length < 5) return 0; const frames = d[3] | (d[4] << 8); const ms = d[2] <= 1 ? 20 : 40; return Math.ceil(frames * ms / 1000); }
function queryFileLimit(dest, cb) {
  if (MODE === "BLE") return cb(null);
  let done = false;
  const finish = (ev) => { if (!done) { done = true; N.fileLimitWaiter = null; cb(ev); } };
  N.fileLimitWaiter = (ev) => finish(ev && ev.dest === dest ? ev : null);
  setTimeout(() => finish(null), 1500);
  if (!send(Cmd.getFileLimit(dest))) finish(null);
}
function sendFile(conv, data, mime, name, maxBytes) {
  if (!conv.startsWith("dm:")) return "Files and voice messages go to one person, not to a channel";
  if (!N.alive) return "Not connected — files are not queued; send it when you are online";
  const max = maxBytes ? Math.min(maxBytes, fileMax()) : fileMax();
  if (data.length > max) return `Too large: ${Math.ceil(data.length / 1024)} KB, at most ${Math.floor(max / 1024)} KB here` + (MODE === "BLE" ? " (over LoRa)" : "");
  const url = URL.createObjectURL(new Blob([data], { type: mime }));
  const row = insert({ conv, mine: true, from: N.me?.addr || "", text: fileLabel(mime, name, data), ts: Date.now(), status: "sending…", file: url, mime, data });
  const r = req();
  N.pendingFiles[r] = row;
  const dest = conv.slice(3);
  send(Cmd.fileBegin(r, dest, data.length, mime, name));
  for (let o = 0; o < data.length; o += 240) send(Cmd.fileData(r, o, data.subarray(o, Math.min(data.length, o + 240))));
  send(Cmd.fileEnd(r));
  return null;
}
function onFileEvent(ev) {
  const k = String(ev.fileId);
  switch (ev.t) {
    case "FileStarted": { const row = N.pendingFiles[ev.req]; if (row) { delete N.pendingFiles[ev.req]; N.fileOut[k] = row; setStatus(row, `sending 0/${ev.chunks}`); } break; }
    case "FileIncoming":
      if (Pages.onIncoming(ev)) break;
      if (ev.size <= 0 || ev.size > 16 * 1024 * 1024 || Pages.isPageFile(ev.mime, ev.name)) break;
      heard(ev.from, (n) => n);
      N.fileIn[k] = { row: insert({ conv: "dm:" + ev.from, mine: false, from: ev.from, text: fileLabel(ev.mime, ev.name, null) + " — receiving…", ts: Date.now(), status: "", mime: ev.mime }), buf: new Uint8Array(ev.size), mime: ev.mime, name: ev.name, from: ev.from };
      break;
    case "FileChunk": { if (Pages.onChunk(ev)) break; const f = N.fileIn[k]; if (f) f.buf.set(ev.data.subarray(0, Math.max(0, f.buf.length - ev.offset)), ev.offset); break; }
    case "FileProgress":
      if (!ev.outgoing && Pages.onProgress(ev)) break;
      if (ev.outgoing) { const row = N.fileOut[k]; if (row) setStatus(row, `sending ${ev.done}/${ev.total}`); }
      else { const f = N.fileIn[k]; if (f) { const m = N.msgs.find((x) => x.id === f.row); m.text = fileLabel(f.mime, f.name, null) + ` — receiving ${Math.floor(100 * ev.done / Math.max(1, ev.total))} %`; } }
      break;
    case "FileDone":
      if (!ev.outgoing && Pages.onDone(ev)) break;
      if (ev.outgoing) {
        const row = N.fileOut[k] ?? byMsgId(ev.fileId)?.id;
        delete N.fileOut[k];
        if (row) setStatus(row, !ev.ok ? (ev.text.startsWith("cancelled") ? ev.text : "error: " + ev.text) : ev.text.includes("put in mailbox") ? "filebox" : "delivered", ev.ok && ev.text.includes("put in mailbox") ? ev.fileId : undefined);
      } else {
        const f = N.fileIn[k]; if (!f) break;
        delete N.fileIn[k];
        const m = N.msgs.find((x) => x.id === f.row);
        if (!ev.ok) { m.text = fileLabel(f.mime, f.name, null) + " — " + (ev.text.startsWith("cancelled") ? ev.text : `not received (${ev.text})`); break; }
        m.file = URL.createObjectURL(new Blob([f.buf], { type: f.mime })); m.data = f.buf; m.text = fileLabel(f.mime, f.name, f.buf);
        if (N.openConv !== m.conv) N.unread[m.conv] = (N.unread[m.conv] || 0) + 1;
      }
      break;
  }
}

function incoming(conv, from, text, hops) {
  insert({ conv, mine: false, from, text, ts: Date.now(), status: "", hops });
  if (N.openConv !== conv) N.unread[conv] = (N.unread[conv] || 0) + 1;
}

// Not part of the app: the engine answers what it cannot do yet with an
// error that says so. Shown as a short note so nothing fails silently.
function simNote(text) {
  N.notSim.push(text);
  UI.toast = { text, at: Date.now() };
  clearTimeout(UI.toastTimer);
  UI.toastTimer = setTimeout(() => { UI.toast = null; render(); }, 6000);
}

function onEvent(ev) {
  switch (ev.t) {
    case "Info": N.infoAt = Date.now(); N.me = { addr: ev.addr, maxText: ev.maxText, maxFile: ev.maxFile, repeater: ev.repeater, version: ev.version, name: N.me?.name || "" }; note(`Node ${shortAddr(ev.addr)} (protocol v${ev.version}, ${ev.maxText} bytes per message)`); break;
    case "Name": if (N.me) N.me.name = ev.name; break;
    case "Joined": N.channels[ev.channel] = ev.name; break;
    case "Heard":
      if (ev.addr !== N.me?.addr && Date.now() - N.infoAt > 3000) {
        const now = Date.now();
        N.announces = [[ev.addr, ev.hops, now]].concat(N.announces.filter((a) => !(a[0] === ev.addr && now - a[2] < 60000))).slice(0, 200);
      }
      heard(ev.addr, (n) => Object.assign(n, { hops: ev.hops }));
      break;
    case "Route": heard(ev.dest, (n) => Object.assign(n, { hops: ev.hops })); break;
    case "NodeInfo": heard(ev.addr, (n) => Object.assign(n, { hops: ev.hops, name: ev.name ?? n.name, mv: ev.mv ?? n.mv, lat: ev.lat ?? n.lat, lon: ev.lon ?? n.lon })); if (ev.caps & 0x20) Pages.noteSharing(ev.addr); break;
    case "Sent": { const row = N.pending[ev.req]; if (row) { delete N.pending[ev.req]; setStatus(row, "sent", ev.msgId); } break; }
    case "Acked": { const m = byMsgId(ev.msgId); if (m) m.status = "delivered"; break; }
    case "Deposited": { const m = byMsgId(ev.msgId); if (m) m.status = "mailbox"; break; }
    case "CarryAcked": { const m = byMsgId(ev.msgId); if (m) m.status = "carried"; break; }
    case "FileStarted": case "FileIncoming": case "FileChunk": case "FileProgress": case "FileDone": onFileEvent(ev); break;
    case "Call": Calls.onData(ev.from, ev.data); break;
    case "Backbone": if (ev.link === 0) { N.backbones = N.backbonesNext; N.backbonesNext = []; } else N.backbonesNext.push(ev); break;
    case "Error": {
      const row = N.pending[ev.req]; if (row) { delete N.pending[ev.req]; setStatus(row, "error: " + ev.text); }
      const frow = N.pendingFiles[ev.req]; if (frow) { delete N.pendingFiles[ev.req]; setStatus(frow, "error: " + ev.text); }
      if (ev.code === 9) Calls.failed(ev.text);
      Pages.onError(ev);
      N.lastError = ev.text;
      note("Error: " + ev.text);
      // Right after connecting the app asks for settings the engine may not
      // have yet; those go to the log only.
      if (Date.now() - UI.started > 2500) simNote(ev.text);
      break;
    }
    case "FileLimit": N.fileLimitWaiter?.(ev); break;
    case "Person": N.person = ev; break;
    case "Direct": heard(ev.from, (n) => Object.assign(n, { hops: ev.hops })); incoming("dm:" + ev.from, ev.from, ev.text, ev.hops); break;
    case "Channel": incoming("ch:" + ev.channel, ev.from, ev.text, ev.hops); break;
    case "Stats": {
      const fresh = ev.items.some(([k]) => k === 1);
      const m = fresh ? Object.fromEntries(Object.entries(N.stats).filter(([k]) => (k >= 60 && k <= 70) || k >= 93)) : Object.assign({}, N.stats);
      for (const [k, v] of ev.items) m[k] = v;
      N.stats = m;
      break;
    }
    case "Radio": N.radio = ev.cfg; break;
    case "Regions": N.regions = ev; break;
    case "Sharing": N.sharing = ev; break;
    case "Neighbors": N.neighbors = ev.part === 0 ? ev.items : N.neighbors.concat(ev.items); break;
    case "Trace": { const t = N.traceSent[ev.dest]; if (t) { delete N.traceSent[ev.dest]; N.traceRtt[ev.dest] = Date.now() - t; } N.traces = [ev].concat(N.traces).slice(0, 10); break; }
    case "Alert": if (!N.alerts.some((a) => a.key === ev.key)) N.alerts.push(ev); break;
    case "Log": note(ev.text); break;
    case "IdentityChanged": note("The node's identity changed: " + shortAddr(ev.addr)); break;
  }
}

// ---------- calls (Calls.kt): signalling over SendCall 0x27 ----------
const Calls = {
  RING: 1, ANSWER: 2, END: 3, AUDIO: 4,
  s: { phase: "IDLE", peer: "", started: 0, muted: false, speaker: false, note: "" }, id: 0,
  possible: () => MODE === "PHONE" || MODE === "REMOTE",
  frame(kind, extra = []) { const b = new Uint8Array(5 + extra.length); const v = new DataView(b.buffer); b[0] = kind; v.setInt32(1, this.id, true); b.set(extra, 5); return b; },
  send(kind, extra) { send(Cmd.sendCall(this.s.peer, this.frame(kind, extra))); },
  start(peer, mode = 0) {
    if (this.s.phase !== "IDLE") return;
    this.id = crypto.getRandomValues(new Int32Array(1))[0];
    this.s = { phase: "CALLING", peer, started: 0, muted: false, speaker: false, note: "" };
    this.send(this.RING, mode > 0 ? [mode] : []);
    setTimeout(() => { if (this.s.phase === "CALLING" && this.s.peer === peer) { this.send(this.END); this.stop("No answer"); } }, 45000);
    render();
  },
  answer() { if (this.s.phase !== "RINGING") return; this.send(this.ANSWER); this.begin(); },
  hangup() { if (this.s.phase === "IDLE") return; this.send(this.END); this.stop(""); },
  failed(why) { if (this.s.phase !== "IDLE") this.stop(why); },
  stop(why) { this.s = { phase: "IDLE", peer: "", note: why }; if (why) note("Call: " + why); render(); },
  begin() { this.s = Object.assign({}, this.s, { phase: "ACTIVE", started: Date.now() }); render(); },
  onData(from, d) {
    if (d.length < 5) return;
    const kind = d[0], cid = new DataView(d.buffer, d.byteOffset).getInt32(1, true);
    if (kind === this.RING) {
      if (this.s.phase !== "IDLE") { const b = new Uint8Array(5); b[0] = this.END; new DataView(b.buffer).setInt32(1, cid, true); send(Cmd.sendCall(from, b)); return; }
      this.id = cid; this.s = { phase: "RINGING", peer: from }; return;
    }
    if (from !== this.s.peer || cid !== this.id) return;
    if (kind === this.ANSWER && this.s.phase === "CALLING") this.begin();
    else if (kind === this.END) this.stop(this.s.phase === "CALLING" ? "Rejected or busy" : "The other side hung up");
  },
};

// ---------- maarse:// pages (Pages.kt), drawn by the web client's own MU code ----------
const PAGE_MIME = "text/x-maarse-page;deflate";
const Pages = {
  url: "", status: "", loading: false, history: [], forward: [], recent: [], cache: new Map(), waits: new Set(), files: {}, pics: {}, lora: {}, sharing: new Set(), favPages: [], partial: null, startQ: "", pageBytes: 0, pageAt: 0, reqs: {},
  parse(s, base) {
    const t = String(s || "").trim();
    if (t.startsWith("/") && base) return { addr: base.addr, path: t.replace(/^\/+|\/+$/g, "") || "index" };
    const m = t.replace(/^maarse:\/\//i, "").match(/^([0-9a-fA-F]{32})(\/.*)?$/);
    if (!m && base && /^[a-z0-9_][a-z0-9_./-]*$/.test(t)) return { addr: base.addr, path: t.replace(/\/+$/, "") || "index" };
    if (!m) return null;
    return { addr: m[1].toLowerCase(), path: (m[2] || "").replace(/^\/+|\/+$/g, "") || "index" };
  },
  str: (u) => `maarse://${u.addr}/${u.path === "index" ? "" : u.path}`,
  key: (addr, path) => addr + "/" + path,
  cur() { return this.parse(this.url); },
  isPageFile: (mime, name) => mime === PAGE_MIME || (name.startsWith("_") && name.includes("/")),
  noteSharing(a) { this.sharing.add(a); },
  overLora(a) { return this.lora[a] !== false; },
  askRoute(addr) { queryFileLimit(addr, (l) => { this.lora[addr] = l == null || l.viaLora || l.mailbox; render(); }); },
  title(markup) {
    const m = markup.match(/^@title\s+(.+)$/m) || markup.match(/^#{1,3}\s+(.+)$/m);
    return m ? m[1].replace(/^->\s*(.*?)\s*<-\s*$/, "$1").replace(/\{!?[a-z]+:([^{}]+)\}/g, "$1").replace(/[*`~]/g, "").trim().slice(0, 80) : "";
  },
  siteTitle(a) { const c = this.cache.get(a + "/index"); return c ? this.title(c.markup) : ""; },
  open(input, push = true) {
    const u = this.parse(input, this.cur());
    if (!u) { this.status = "Invalid address. Write maarse://<32 hex characters>/path"; render(); return; }
    if (this.url && push && this.url !== this.str(u)) { this.history.push(this.url); this.forward = []; }
    this.url = this.str(u);
    this.loading = true; this.partial = null;
    const key = this.key(u.addr, u.path);
    setTimeout(() => { if (this.loading && this.url === this.str(u) && this.waits.has(key)) { this.loading = false; this.status = "No answer from the node (is it online, and does it have pages?)"; render(); } }, 60000);
    if (!(u.addr in this.lora)) this.askRoute(u.addr);
    this.status = this.cache.has(key) ? "From this session's copy – fetching a new one…" : "Fetching…";
    this.waits.add(key);
    const r = req(); this.reqs[r] = key;
    if (!send(Cmd.requestPage(r, u.addr, u.path))) this.status = "Not connected";
    render();
  },
  back() { const u = this.history.pop(); if (u) { if (this.url) this.forward.push(this.url); this.open(u, false); } },
  fwd() { const u = this.forward.pop(); if (u) { if (this.url) this.history.push(this.url); this.open(u, false); } },
  reload() { if (this.url) this.open(this.url, false); },
  home() { if (this.url) { this.history.push(this.url); this.forward = []; } this.url = ""; this.loading = false; this.status = ""; render(); },
  requestPicture(addr, path) {
    const key = this.key(addr, path);
    if (this.waits.has(key)) return;
    this.waits.add(key); this.pics[key] = { s: "asking" };
    const r = req(); this.reqs[r] = key;
    if (!send(Cmd.requestPage(r, addr, path))) { this.waits.delete(key); this.pics[key] = { s: "failed", why: "not connected" }; }
    setTimeout(() => { if (this.waits.has(key)) { this.waits.delete(key); this.pics[key] = { s: "failed", why: "no answer" }; render(); } }, 120000);
    render();
  },
  // The engine refused the request (Pages not simulated yet): say so on the page.
  onError(ev) {
    const key = this.reqs[ev.req]; if (!key) return;
    delete this.reqs[ev.req]; this.waits.delete(key);
    if (this.pics[key]) this.pics[key] = { s: "failed", why: ev.text };
    else if (this.url && this.key(this.cur().addr, this.cur().path) === key) { this.loading = false; this.status = "Error: " + ev.text; }
  },
  onIncoming(ev) {
    const key = this.key(ev.from, ev.name);
    if (!this.waits.has(key)) return false;
    const pic = ev.name.startsWith("_");
    if (!pic && ev.mime !== PAGE_MIME) return false;
    if (ev.size <= 0 || ev.size > 2 * 1024 * 1024) return false;
    this.waits.delete(key);
    this.files[String(ev.fileId)] = { key, pic, mime: ev.mime, buf: new Uint8Array(ev.size) };
    if (pic) this.pics[key] = { s: "busy", pct: 0 };
    return true;
  },
  onChunk(ev) { const f = this.files[String(ev.fileId)]; if (!f) return false; f.buf.set(ev.data.subarray(0, Math.max(0, f.buf.length - ev.offset)), ev.offset); return true; },
  onProgress(ev) {
    const f = this.files[String(ev.fileId)]; if (!f) return false;
    const pct = Math.floor(100 * ev.done / Math.max(1, ev.total));
    if (f.pic) this.pics[f.key] = { s: "busy", pct }; else this.status = `Receiving… ${pct} %`;
    return true;
  },
  onDone(ev) {
    const id = String(ev.fileId), f = this.files[id]; if (!f) return false;
    delete this.files[id];
    if (!ev.ok) { if (f.pic) this.pics[f.key] = { s: "failed", why: ev.text }; else { this.status = "Error: " + ev.text; this.loading = false; } return true; }
    if (f.pic) { this.pics[f.key] = f.mime === PAGE_MIME ? { s: "failed", why: "missing on the node" } : { s: "got", url: URL.createObjectURL(new Blob([f.buf], { type: f.mime })) }; return true; }
    new Response(new Blob([f.buf]).stream().pipeThrough(new DecompressionStream("deflate-raw"))).text().then((m) => {
      this.cache.set(f.key, { markup: m });
      const from = f.key.split("/")[0];
      this.noteSharing(from); this.askRoute(from);
      const cur = this.cur();
      if (cur && this.key(cur.addr, cur.path) === f.key) {
        this.status = ""; this.loading = false; this.pageBytes = f.buf.length; this.pageAt = Date.now();
        this.recent = [[this.url, this.title(m)]].concat(this.recent.filter((r) => r[0] !== this.url)).slice(0, 20);
      }
      render();
    }).catch(() => { this.status = "Could not unpack the page"; render(); });
    return true;
  },
};

let MU = null; // { renderMarkup, pageTitle } from the web client
Reuse.markup({
  pages: Pages, state: { view: "app", me: null, nodeInfo: null, files: new Map(), favs: { p: [] }, get nextReq() { return req(); }, set nextReq(v) {} },
  openPage: (u, push) => Pages.open(u, push !== false), label, parsePageUrl: (s, b) => Pages.parse(s, b), pageUrl: (p) => Pages.str(p), pageKey: (a, p) => Pages.key(a, p),
  pagePicture: (caption, src, style, base) => pagePicture(caption, src, style, base), transport: { send }, Cmd, log: note, siteTitle: (a) => Pages.siteTitle(a),
  requestPicture: (a, p) => Pages.requestPicture(a, p),
}).then((m) => { MU = m; render(); }).catch((e) => note("Pages renderer: " + e.message));

function pagePicture(caption, src, style, base) {
  const path = `_${style || "grey"}/${src.replace(/^\/+/, "")}`;
  const key = Pages.key(base.addr, path);
  const p = Pages.pics[key];
  const box = document.createElement("div");
  box.className = "picbox";
  if (p?.s === "got") { const img = document.createElement("img"); img.src = p.url; img.alt = caption; box.append(img); }
  else box.textContent = `🖼 ${caption || src} · ` + (!p ? (Pages.overLora(base.addr) ? "tap to load over LoRa" : "tap to load") : p.s === "busy" ? `Receiving · ${p.pct} %` : p.s === "asking" ? "asking…" : "not loaded: " + p.why);
  box.onclick = () => { if (!p || p.s === "failed") Pages.requestPicture(base.addr, path); };
  return box;
}

// ---------- navigation (Shell.kt Nav) ----------
const TABS = [["CHATS", "Chats", "chat"], ["PAGES", "Pages", "public"], ["CONTACTS", "Nodes", "people"], ["NETWORK", "Network", "hub"], ["SETTINGS", "Settings", "settings"]];
const UI = {
  tab: "CHATS", stacks: { CHATS: [], PAGES: [], CONTACTS: [], NETWORK: [], SETTINGS: [] }, sheet: false, dialog: null, toast: null, started: Date.now(),
  draft: {}, filter: "", annQuery: "", shown: null, err: null, rec: null, dark: false, scroll: {}, editingUrl: false,
};
const topScreen = () => UI.stacks[UI.tab].at(-1) || { s: "home" };
function go(s) { UI.stacks[UI.tab].push(s); render(); }
function back() { const st = UI.stacks[UI.tab]; if (st.length) st.pop(); else if (UI.tab !== "CHATS") UI.tab = "CHATS"; render(); }
function select(t) { if (t === UI.tab) { UI.stacks[t] = []; if (t === "PAGES") Pages.home(); } UI.tab = t; render(); }

// ---------- small pieces (Design.kt, Common.kt) ----------
const AV_L = ["#3B6EA8", "#6B5B95", "#B5651D", "#2E7D8C", "#556B2F", "#9C4A6E", "#2F7D5B", "#7A5C2E"];
const AV_D = ["#5B8CC6", "#8A7AB5", "#C98447", "#4B9DAC", "#7A8F4F", "#BA6A8E", "#4F9D7B", "#9C7C4C"];
function hashCode(s) { let h = 0; for (let i = 0; i < s.length; i++) h = (Math.imul(31, h) + s.charCodeAt(i)) | 0; return h; }
const avatarColor = (key) => (UI.dark ? AV_D : AV_L)[(hashCode(key) & 0x7fffffff) % 8];
function initials(title) {
  const t = title.trim().replace(/^~/, "");
  if (t.startsWith("#")) return "#";
  const w = t.split(/[ ·-]/).filter((x) => x.trim());
  return !w.length ? "?" : w.length === 1 ? [...w[0]][0].toUpperCase() : ([...w[0]][0] + [...w[1]][0]).toUpperCase();
}
const avatar = (title, key, size = 48) => `<div class="av" style="width:${size}px;height:${size}px;background:${avatarColor(key)};font-size:${(size * 0.38).toFixed(1)}px">${esc(initials(title))}</div>`;
function time(ms) {
  const d = new Date(ms), today = d.toDateString() === new Date().toDateString();
  return today ? d.toTimeString().slice(0, 5) : d.toLocaleDateString("en-GB", { day: "numeric", month: "short" }) + " " + d.toTimeString().slice(0, 5);
}
function deliveryIcon(status, channel) {
  let [ic, col, lbl] = ["schedule", "var(--on-surface-v)", "Sending"];
  if (status === "delivered" || status === "carried") [ic, col, lbl] = ["doneAll", "var(--primary)", "Delivered"];
  else if (status === "mailbox") [ic, col, lbl] = ["mail", "var(--amber)", "Waiting in a post office"];
  else if (status === "filebox") [ic, col, lbl] = ["mail", "var(--amber)", "Kept by your node until they are reachable"];
  else if (status === "sent") [ic, col, lbl] = ["done", "var(--on-surface-v)", channel ? "Sent" : "Sent, waiting for receipt"];
  else if (status === "queued") [ic, col, lbl] = ["schedule", "var(--amber)", "Waiting for a connection"];
  else if (status.startsWith("error") || status.startsWith("cancelled")) [ic, col, lbl] = ["error", "var(--error)", status];
  return `<span title="${esc(lbl)}" style="color:${col}">${icon(ic, 15)}</span>`;
}
function statusText(s, channel) {
  if (channel && s === "sent") return ["Sent to the channel", null];
  if (s === "delivered") return ["✓ Delivered", "var(--green)"];
  if (s === "carried") return ["✓ Delivered (carried by others)", "var(--green)"];
  if (s === "mailbox") return ["✉ Waiting in a post office", "var(--amber)"];
  if (s === "filebox") return ["✉ Kept by your node until they are reachable", "var(--amber)"];
  if (s === "sent") return ["Sent, waiting for receipt", null];
  if (s === "queued") return ["Waiting — no connection. Sent when you are back online", "var(--amber)"];
  if (s.startsWith("error")) return [s[0].toUpperCase() + s.slice(1), "var(--error)"];
  if (s.startsWith("cancelled")) return ["✕ " + s[0].toUpperCase() + s.slice(1), "var(--error)"];
  return [s, null];
}
const gtitle = (t) => `<div class="gtitle">${esc(t)}</div>`;
const hint = (t) => `<div class="hint">${esc(t)}</div>`;
const group = (rows) => `<div class="group">${rows.join("")}</div>`;
const grow = (labelT, sub, act, last = false, lead = "") =>
  `<div class="gr" data-act="${act}">${lead}<div class="grow col" style="gap:2px"><div class="bl">${esc(labelT)}</div>${sub ? `<div class="bs mute">${esc(sub)}</div>` : ""}</div>${icon("chevron", 24, "chev")}</div>${last ? "" : '<div class="div"></div>'}`;
const card2 = (inner, style = "") => `<div class="card2" style="${style}">${inner}</div>`;
const modeName = (m) => ({ PHONE: "Phone node", BLE: "Radio over Bluetooth", REMOTE: "Home computer", SERVER: "Server on this phone" })[m];

function connectionBanner() {
  if (N.alive) return "";
  return `<div class="banner" data-act="sheet"><span class="dot" style="background:var(--amber)"></span><div class="col"><div class="sb">No connection</div><div class="bs">${esc(N.offlineWhy || "The node is down (power or failure in the simulation).")}</div></div></div>`;
}
function alertCard(a, hide) {
  const ended = Date.now() >= a.endsMs, drill = a.status === 1 || a.status === 2, loud = a.kind !== 2 && a.status === 0 && a.severity >= 2;
  const bg = ended ? "var(--surface-v)" : drill ? "#3B5B8C" : loud ? "#C0392B" : "#B7791F";
  const st = Names.ALERT_STATUS[a.status] || "";
  const from = a.name || "trusted key " + shortAddr(a.issuer);
  const how = a.trust === 1 ? "verified by certificate" : a.trust === 2 ? "verified (old format, no expiry)" : "verified by key";
  return `<div class="alert" style="background:${bg};color:${ended ? "var(--on-surface-v)" : "#fff"}"><div class="row"><b>${st ? esc(st) + " · " : ""}</b><b class="grow">${esc(a.kind === 2 ? "Cancelled: alert " + a.reference : (Names.ALERT_EVENTS[a.event] || "Alert") + (a.kind === 1 ? " (update)" : ""))}</b><span>${esc(Names.ALERT_SEVERITY[a.severity] || "")}</span></div>${a.text ? `<div class="bl">${esc(a.text)}</div>` : ""}<div class="lsm">From ${esc(from)} · ${how} · issued ${time(a.issued * 1000)} · valid until ${time(a.endsMs)} · ${a.hops} hops · no. ${a.serial}</div>${hide ? `<button class="btn text" style="color:inherit;align-self:flex-start" data-act="hideAlert" data-k="${esc(a.key)}">Hide</button>` : ""}</div>`;
}
const alertBar = () => N.alerts.filter((a) => a.kind !== 2 && a.status === 0 && a.severity >= 2 && !N.dismissed.has(a.key)).slice(-3).reverse().map((a) => alertCard(a, true)).join("");

// ---------- screens ----------
function home() {
  if (UI.tab === "PAGES") return pagesPage();
  const name = N.me?.name || "?";
  let body = "";
  if (UI.tab === "CHATS") body = chatsScreen();
  else if (UI.tab === "CONTACTS") body = contactsScreen();
  else if (UI.tab === "NETWORK") body = networkScreen();
  else if (UI.tab === "SETTINGS") body = meScreen();
  return `<div class="homehead"><div data-act="tab" data-t="SETTINGS" style="cursor:pointer">${avatar(name, N.me?.addr || "me", 40)}</div><div class="hs">${esc(TABS.find((t) => t[0] === UI.tab)[1])}</div></div>${connectionBanner()}${alertBar()}<div class="main">${body}</div>`;
}

function convList() {
  const last = new Map();
  for (const m of N.msgs) last.set(m.conv, m);
  const convs = [...last.values()].sort((a, b) => b.ts - a.ts).map((m) => m.conv);
  for (const ch of Object.keys(N.channels)) if (!last.has("ch:" + ch)) convs.push("ch:" + ch);
  return { convs, last };
}
function chatsScreen() {
  const { convs, last } = convList();
  const rows = convs.map((c) => {
    const m = last.get(c), n = N.unread[c] || 0, title = convTitle(c);
    const prev = m ? (m.mine ? "" : c.startsWith("ch:") ? label(m.from) + ": " : "") + m.text : "No messages yet";
    return `<div class="li" data-act="open" data-c="${esc(c)}">${avatar(title, c.split(/:(.*)/s)[1])}<div class="grow col" style="gap:3px"><div class="row"><div class="grow ell t">${esc(title)}</div>${m ? `<span class="lsm" style="color:${n ? "var(--primary)" : "var(--on-surface-v)"}">${time(m.ts)}</span>` : ""}</div><div class="row" style="gap:6px">${m?.mine ? deliveryIcon(m.status, c.startsWith("ch:")) : ""}<div class="grow ell bm mute">${esc(prev)}</div>${n ? `<span class="count">${n}</span>` : ""}</div></div></div>`;
  });
  const al = N.alerts.at(-1);
  const alerts = `<div class="li" data-act="net" data-p="ALERTS">${avatar("!", "alerts")}<div class="grow col" style="gap:3px"><div class="sb">Alerts</div><div class="ell bm mute">${esc(al?.text || (N.alerts.length ? N.alerts.length + " received" : "Verified public warnings"))}</div></div></div>`;
  const empty = convs.length ? "" : `<div class="col" style="padding:24px;gap:6px;position:absolute;top:60px"><div class="tm" style="font-weight:400">No conversations yet</div><div class="bm mute">Join a channel or write to someone with the button below. People who write to you show up here.</div></div>`;
  return `<div class="scroll pb88" data-scroll="chats">${alerts}${rows.join("")}</div>${empty}<button class="fab" data-act="newchat" title="New chat">${icon("edit")}</button>`;
}

function reachText(conv) {
  if (conv.startsWith("ch:")) return "Channel";
  const n = N.nodes[conv.slice(3)];
  if (!n) return "Not heard yet";
  return n.hops == null ? "Heard " + time(n.lastHeard) : n.hops === 0 ? "Nearby · direct" : `Reachable · ${n.hops} hops`;
}
function fileProgress(m) {
  if (m.mine) {
    if (!m.status.startsWith("sending")) return null;
    const p = m.status.replace("sending", "").trim().split("/");
    const done = parseFloat(p[0]), total = parseFloat(p[1]);
    return !(total > 0) || !(done > 0) ? -1 : Math.min(1, done / total);
  }
  if (m.file) return null;
  const i = m.text.lastIndexOf("receiving");
  if (i < 0) return null;
  const pct = parseFloat(m.text.slice(i + 9));
  return Number.isFinite(pct) ? Math.min(1, pct / 100) : -1;
}
function bubble(m, isDm) {
  const mine = m.mine;
  let att = "";
  if (m.file && m.mime?.startsWith("image/")) att = `<img src="${m.file}" class="${m.mime === "image/png" ? "px" : ""}" alt="Picture">`;
  else if (m.file && m.mime === VOICE_MIME) att = `<div class="row"><button class="iconbtn on" data-act="play" data-id="${m.id}" title="Play">${icon(UI.playing === m.id ? "stop" : "play")}</button><span>${esc(m.text)}</span></div>`;
  else if (m.file) att = `<div class="row" style="padding:4px 0">${icon("file")}<a class="pri sb" href="${m.file}" download style="text-decoration:none">&nbsp; Open</a></div>`;
  const p = fileProgress(m);
  const prog = p == null ? "" : `<div class="prog${p < 0 ? " ind" : ""}"><i style="width:${p < 0 ? 30 : p * 100}%"></i></div><div class="lsm mute">${p < 0 ? (mine ? "Waiting for the recipient…" : "Receiving…") : `${mine ? "Sending" : "Receiving"} ${Math.floor(p * 100)} %`}</div>`;
  const txt = !m.file || !(m.mime === VOICE_MIME || m.mime?.startsWith("image/")) ? `<div>${esc(m.text)}</div>` : "";
  let open = "";
  if (UI.shown === m.id) {
    const [w, c] = statusText(m.status, !isDm);
    open = `<div class="lsm" style="color:${c || "var(--on-surface-v)"}">${esc(mine ? w : m.hops > 0 ? `Came over ${m.hops} hops` : "Came directly")}</div>`;
  }
  const sender = !mine && !isDm ? `<div class="sender" style="color:${avatarColor(m.from)}" data-act="contact" data-a="${m.from}">${esc(label(m.from))}</div>` : "";
  return `<div class="brow${mine ? " mine" : ""}"><div class="bubble" data-act="bubble" data-id="${m.id}">${sender}${att}${prog}${txt}<div class="meta">${new Date(m.ts).toTimeString().slice(0, 5)}${mine ? deliveryIcon(m.status, !isDm) : ""}</div>${open}</div></div>`;
}
function chatScreen(conv) {
  N.openConv = conv;
  if (N.unread[conv]) delete N.unread[conv];
  const isDm = conv.startsWith("dm:"), title = convTitle(conv), peer = conv.split(/:(.*)/s)[1];
  const msgs = N.msgs.filter((m) => m.conv === conv);
  let list = "", lastDay = "";
  for (const m of msgs) {
    const day = new Date(m.ts).toLocaleDateString("en-GB", { weekday: "long", day: "numeric", month: "short" }).replace(",", "");
    if (day !== lastDay) { lastDay = day; list += `<div class="day"><span>${esc(day)}</span></div>`; }
    list += bubble(m, isDm);
  }
  const text = UI.draft[conv] || "", max = maxText(conv), used = bytes(text);
  const canSend = N.me && text.trim() && used <= max;
  let comp;
  if (UI.rec) {
    const secs = Math.floor((Date.now() - UI.rec.started) / 1000);
    comp = `<button class="iconbtn on" data-act="recCancel" title="Cancel recording">${icon("close")}</button><div class="rec"><span class="dot" style="background:var(--error)"></span><span>&nbsp; Recording  0:${String(secs).padStart(2, "0")} / 0:20</span></div><button class="round" data-act="recSend" title="Send voice message">${icon("send")}</button>`;
  } else {
    comp = (isDm ? `<button class="iconbtn on" data-act="attach" title="Send a picture or file" ${N.me ? "" : "disabled"}>${icon("attach")}</button>` : "") +
      `<div class="tf"><textarea id="composer" rows="1" placeholder="${N.alive ? "Message" : "Not connected — the message waits"}" ${N.me ? "" : "disabled"}>${esc(text)}</textarea>${used > max * 3 / 4 ? `<span class="bm" style="color:${used > max ? "var(--error)" : "var(--on-surface-v)"}">${used}/${max}</span>` : ""}</div>` +
      (isDm && !text.trim() ? `<button class="round" data-act="mic" title="Record a voice message" ${N.me ? "" : "disabled"}>${icon("mic")}</button>` : `<button class="round" data-act="send" title="Send" ${canSend ? "" : "disabled"}>${icon("send")}</button>`);
  }
  return `<div class="chat"><div class="chathead"><button class="iconbtn on" data-act="back" title="Back">${icon("back")}</button><div class="who" ${isDm ? `data-act="contact" data-a="${peer}"` : ""}>${avatar(title, peer, 40)}<div class="col" style="min-width:0"><div class="sb ell">${esc(title)}</div><div class="lsm pri">${esc(reachText(conv))}</div></div></div>${isDm && Calls.possible() ? `<button class="iconbtn on" data-act="call" data-a="${peer}" title="Call (over the internet)">${icon("call")}</button>` : ""}</div><div class="msgs" id="msgs">${list}</div>${UI.err ? `<div class="errline">${esc(UI.err)}</div>` : ""}<div class="composer">${comp}</div></div>`;
}

function reach(n) {
  const out = [];
  if (n.hops != null) out.push(n.hops === 0 ? "nearby" : `${n.hops} hops`);
  if (n.lastHeard > 0) out.push("heard " + time(n.lastHeard));
  if (n.mv != null) out.push((n.mv / 1000).toFixed(1) + " V");
  return out.join(" · ");
}
function contactRow(n) { const t = label(n.addr); return `<div class="li" data-act="contact" data-a="${n.addr}">${avatar(t, n.addr, 44)}<div class="grow col"><div class="sb">${esc(t)}</div><div class="bs mute">${esc(reach(n))}</div></div></div>`; }
function contactsScreen() {
  const f = UI.filter.toLowerCase();
  const list = Object.values(N.nodes).filter((n) => !f || label(n.addr).toLowerCase().includes(f) || n.addr.startsWith(f))
    .sort((a, b) => (!!N.nick[b.addr] - !!N.nick[a.addr]) || b.lastHeard - a.lastHeard);
  const saved = list.filter((n) => N.nick[n.addr]), rest = list.filter((n) => !N.nick[n.addr]);
  return `<div class="field">${icon("search")}<input id="filter" placeholder="Search by name or address" value="${esc(UI.filter)}"></div>${list.length ? "" : `<div class="bl mute" style="padding:24px">Nobody heard yet. People show up when they announce themselves or write to you. Give someone a name to save them.</div>`}<div class="scroll" data-scroll="contacts">${group([grow("Announces", "Nodes and repeaters announcing themselves, live", "net\" data-p=\"ANNOUNCES", true)])}${saved.length ? gtitle(`Saved (${saved.length})`) + saved.map(contactRow).join("") : ""}${rest.length ? gtitle("Heard recently") + rest.map(contactRow).join("") : ""}</div>`;
}
function contactScreen(addr) {
  const n = N.nodes[addr], title = label(addr), fav = N.favUsers.includes(addr);
  const t = N.traces.find((x) => x.dest === addr);
  return `<div class="scroll"><div class="row"><button class="iconbtn on" data-act="back" style="margin:4px" title="Back">${icon("back")}</button><div class="grow"></div><button class="iconbtn" data-act="fav" data-a="${addr}" title="Favourite" style="color:${fav ? "#E7A94B" : "var(--on-surface-v)"}">${icon(fav ? "starFilled" : "star")}</button></div>
    <div class="big">${avatar(title, addr, 88)}<div class="hs b">${esc(title)}</div>${n?.name ? `<div class="bs mute">Announces as ~${esc(n.name)}</div>` : ""}<div class="bs pri">${esc((n && reach(n)) || "Not heard yet")}</div></div>
    <div class="actions"><button class="action" data-act="open" data-c="dm:${addr}">${icon("chatFilled")}<span class="ll">Message</span></button>${Calls.possible() ? `<button class="action" data-act="call" data-a="${addr}">${icon("call")}<span class="ll">Call</span></button>` : `<button class="action" data-act="trace" data-a="${addr}">${icon("route")}<span class="ll">Route</span></button>`}<button class="action" data-act="nick" data-a="${addr}">${icon("edit")}<span class="ll">Name</span></button></div>
    ${t ? gtitle("Route") + card2(`<div class="sb">${t.path.length} hops</div><div class="bs">Signal out: ${t.out.map((x) => x + " dB").join(" · ")}</div><div class="bs">Signal back: ${t.back.map((x) => x + " dB").join(" · ")}</div>`) : ""}
    ${n?.lat != null ? gtitle("Position (shared by them)") + card2(`${n.lat.toFixed(4)}, ${n.lon.toFixed(4)}`) : ""}
    ${gtitle("Address")}${card2(`<div class="bs mono">${addr}</div><div class="bs mute">Messages to this address are end-to-end encrypted. Short form: ${shortAddr(addr)}</div>`)}<div style="height:24px"></div></div>`;
}

function networkScreen() {
  const s = N.stats;
  const hasRadio = HAS_RADIO;
  const near = hasRadio ? s[83] || 0 : 0, links = s[5] || 0;
  const [headline, color] = !N.alive ? ["No connection", "var(--amber)"] : near === 0 && links === 0 ? ["Connected, but nobody in reach yet", "var(--amber)"] : ["All good", "var(--primary)"];
  let story;
  if (!N.alive) story = "The phone has no internet" + (hasRadio ? "" : " and no radio") + ". Messages you write wait and go out when you are back online.";
  else {
    const b = [];
    if (links > 0) b.push(`You reach others over the internet (${links} link${links === 1 ? "" : "s"})`);
    if (near > 0) b.push(`${near} radio${near === 1 ? "" : "s"} heard nearby over LoRa`);
    if (!hasRadio) b.push("No radio: LoRa is not used (add one under Connection)");
    if (links === 0 && near === 0) b.push("No internet link and no radio heard yet. Messages wait and go when someone is in reach");
    if (s[7] != null) b.push(`this node works as ${(Names.ROLES[s[7]] || "?").toLowerCase()}`);
    story = b.join(". ") + ".";
  }
  const tile = (t, v, sub) => `<div class="tile"><div class="lm mute">${esc(t)}</div><div class="tl sb">${esc(v)}</div><div class="lsm mute">${esc(sub)}</div></div>`;
  const tiles = N.alive && Object.keys(s).length ? `<div class="tiles">${tile("Radio airtime", s[41] != null ? formatStat(41, s[41]) : "–", "of the 10 % allowed")}${tile("Post office", `${s[61] || 0} kept`, s[60] > 0 ? `for ${s[60]} people` : "mail for people away")}${tile("Carrying", `${s[96] || 0}`, "messages for others")}${tile("Backbone", `${links} links`, s[72] != null ? `${s[72]} DHT contacts` : "internet links")}</div>` : "";
  const bbs = N.backbones.length ? gtitle("Backbones") + group(N.backbones.map((b, i) => grow(b.name.replace(/^tcp (out|in) /, "").replace(/:[^:]*$/, ""), (b.flags & 1 ? "server" : "host") + (b.flags & 4 ? " · post office" : "") + (b.flags & 8 ? " · relay" : ""), "noop", i === N.backbones.length - 1))) : "";
  return `<div class="scroll pb24" data-scroll="net"><div class="btnrow"><button class="btn tonal" data-act="announce">Announce now</button><button class="btn tonal" data-act="sync" ${N.person?.state === PersonState.MEMBER ? "" : "disabled"}>Sync now</button></div>
    ${card2(`<div class="row"><span class="dot" style="background:${color}"></span><span class="tm sb" style="margin-left:8px">${esc(headline)}</span></div><div class="bm mute">${esc(story)}</div>`)}${tiles}${bbs}
    ${gtitle("Details")}${group([grow("Neighbours and routes", "Who you hear, and how well", "net\" data-p=\"NEIGHBOURS"), grow("All numbers", "Every counter the node keeps", "net\" data-p=\"NUMBERS"), grow("Log", "What happened, live", "net\" data-p=\"LOG", true)])}</div>`;
}
const NET_TITLES = { BACKBONE: "Backbone", ALERTS: "Alerts", NEIGHBOURS: "Neighbours and routes", NUMBERS: "All numbers", LOG: "Log", ANNOUNCES: "Announces" };
function netPage(p) {
  if (p === "NEIGHBOURS") {
    if (!N.neighbors.length) return hint("No radios heard directly yet.") + hint("Trace a route from a contact's page.");
    const rows = [...N.neighbors].sort((a, b) => b.snr - a.snr).map((n, i, all) => {
      const addr = Object.keys(N.nodes).find((a) => a.startsWith(n.prefix));
      const t = addr ? label(addr) : n.prefix;
      return `<div class="row" style="padding:10px 14px;gap:12px">${avatar(t, addr || n.prefix, 36)}<div class="grow col"><div class="md">${esc(t)}</div><div class="bs mute">Signal ${n.snr} dB · ${n.rssi} dBm · heard ${n.count}×</div></div></div>${i < all.length - 1 ? '<div class="hdiv"></div>' : ""}`;
    });
    return group(rows) + hint("Trace a route from a contact's page.");
  }
  if (p === "NUMBERS") {
    const keys = Object.keys(N.stats).map(Number);
    if (!keys.length) return hint("No numbers yet.");
    let out = "";
    for (const g of STAT_GROUPS.concat("Other")) {
      const ks = keys.filter((k) => (STATS[k]?.group || "Other") === g).sort((a, b) => a - b);
      if (!ks.length) continue;
      out += gtitle(g) + group(ks.map((k) => `<div class="kv"><span>${esc(STATS[k]?.label || "Key " + k)}</span><span>${esc(formatStat(k, N.stats[k]))}</span></div>`));
    }
    return out + '<div style="height:16px"></div>';
  }
  if (p === "LOG") return `<div style="padding:16px">${N.log.slice().reverse().map((l) => `<div class="bs" style="padding:1px 0">${esc(l)}</div>`).join("")}</div>`;
  if (p === "ALERTS") return hint("Alerts are signed by trusted issuers and checked by maarse-host (the phone node or your computer). A radio alone does not check them yet.") + (N.alerts.length ? N.alerts.slice().reverse().map((a) => alertCard(a)).join("") : hint("No alerts received."));
  if (p === "ANNOUNCES") return announcesPage();
  return "";
}
function ago(ms) { const s = Math.floor(ms / 1000); return s < 60 ? `${s} s ago` : s < 3600 ? `${Math.floor(s / 60)} min ago` : `${Math.floor(s / 3600)} h ago`; }
function announcesPage() {
  const q = UI.annQuery.trim().toLowerCase(), now = Date.now();
  const match = (a) => !q || label(a).toLowerCase().includes(q) || a.startsWith(q);
  const live = N.announces.filter((a) => match(a[0])).slice(0, 100);
  const favs = N.favUsers.filter(match);
  const line = (addr, sub) => `<div class="row" style="padding:6px 16px 6px 4px;gap:8px;cursor:pointer" data-act="contact" data-a="${addr}"><button class="iconbtn" data-act="fav" data-a="${addr}" style="color:${N.favUsers.includes(addr) ? "#E7A94B" : "var(--on-surface-v)"}">${icon(N.favUsers.includes(addr) ? "starFilled" : "star")}</button>${avatar(label(addr), addr)}<div class="grow col"><div class="sb">${esc(label(addr))}</div><div class="bs mute">${esc(sub)}</div></div></div>`;
  const rows = live.map(([addr, hops, at]) => line(addr, [hops === 0 ? "direct" : `${hops} hop${hops === 1 ? "" : "s"}`, N.nodes[addr]?.lat != null ? "shares position" : "", Pages.sharing.has(addr) ? "has pages" : "", ago(now - at)].filter(Boolean).join(" · ")));
  return `<div style="border-bottom:1px solid var(--outline-v);display:flex"><div class="ts pri" style="flex:1;text-align:center;padding:14px 0 12px;border-bottom:3px solid var(--primary);margin:0 60px">Nodes and repeaters</div></div><div class="field" style="margin:8px 16px">${icon("search")}<input id="annq" placeholder="Search nodes and repeaters by name or address" value="${esc(UI.annQuery)}"></div>
    ${!favs.length && !live.length ? hint(q ? "Nobody matches." : "No announces yet. They come in while you are connected; nodes announce themselves every few minutes.") : ""}${favs.length ? gtitle("Favourites") + favs.map((a) => line(a, "favourite")).join("") + gtitle("Latest announces") : ""}${rows.join("")}`;
}

function meScreen() {
  const me = N.me, p = N.person, host = MODE === "PHONE" || MODE === "REMOTE";
  const sub = p?.state === PersonState.MEMBER ? `Account ${p.person.slice(0, 8)} · ${p.devices.filter((d) => !d[1]).length} devices` : me ? `Address ${me.addr.slice(0, 8)}` : "Not connected";
  const meRows = [];
  if (host) meRows.push(grow("Account and devices", p?.state === PersonState.MEMBER ? (p.master ? "This is the master device · backup, add devices" : "Linked to your account") : p?.state === PersonState.AWAITING ? "Waiting to be linked" : "One address for your phone, PC and more", "page\" data-p=\"ACCOUNT"));
  if (p?.state === PersonState.MEMBER) meRows.push(grow("Sync now", "Fetch your mail and copies from your other devices (also done at start-up)", "sync"));
  meRows.push(grow("Name and position", "What your announces share", "page\" data-p=\"NODE", true));
  return `<div class="scroll pb24" data-scroll="me">${card2(`<div class="row" style="gap:14px"><div data-act="page" data-p="NODE" style="cursor:pointer">${avatar(me?.name || "?", me?.addr || "me", 56)}</div><div class="grow col"><div class="tm sb">${esc(me?.name || "No name yet")}</div><div class="bs mute">${esc(sub)}</div></div></div>`)}
    ${gtitle("Me")}${group(meRows)}
    ${gtitle("Radio")}${group([grow("How this phone connects", `${modeName(MODE)} · ${N.alive ? "online" : "no connection"}${MODE === "PHONE" && N.stats[5] != null ? ` · ${N.stats[5]} internet link${N.stats[5] === 1 ? "" : "s"}` : ""}`, "sheet"), grow("Role and regions", "How this node helps others", "page\" data-p=\"RADIO"), grow("All connection options", "Server mode, the phone node's radio", "page\" data-p=\"CONNECTION", true)])}
    ${MODE === "PHONE" ? gtitle("Connections") + group([grow("Internet link and post office", "Backbone, peers, mail for others, keys", "page\" data-p=\"HOST", true)]) : ""}
    ${gtitle("This phone")}${group([grow("Notifications and background", "Messages, channels, alerts", "page\" data-p=\"APP", true)])}${hint("Må:rse (simulated) · node " + (NODE_LABEL || "?"))}</div>`;
}
const ME_TITLES = { ACCOUNT: "Account and devices", NODE: "Name and position", RADIO: "Role and regions", APP: "Notifications and background", CONNECTION: "All connection options", HOST: "Internet link and post office" };
const notSimFor = (word) => N.notSim.find((t) => t.toLowerCase().includes(word));
function mePage(p) {
  const me = N.me;
  if ((p === "ACCOUNT" || p === "NODE" || p === "RADIO") && !me) return hint("Connect first.");
  if (p === "ACCOUNT") {
    let out = `<div class="sect">Account (linked devices)</div>`;
    const ns = notSimFor("linked devices");
    if (!N.person && !ns) out += hint("Fetching…");
    else if (!N.person || N.person.state === PersonState.NONE) {
      out += hint("With an account, friends write to one address and the message reaches all your devices (phone, PC …). Your sent messages show on all of them too.") +
        `<div class="col" style="gap:8px;padding:8px 16px;align-items:flex-start"><button class="btn" data-act="person" data-op="${PersonOp.CREATE}">Create an account (this is the master device)</button><button class="btn outlined" data-act="person" data-op="${PersonOp.AWAIT_LINK}">Link this device to my account</button><button class="btn text" data-act="person" data-op="${PersonOp.RESTORE}">Restore an account (backup file or 24 words)</button></div>`;
    } else if (N.person.state === PersonState.AWAITING) {
      out += hint("This device is waiting to be linked (10 minutes). On your master device, open Account → Add device and enter this address:") + card2(`<div class="mono">${me.addr}</div><div class="sb">Check words: ${esc(N.person.words)}</div>`);
    } else {
      out += hint("Your account address — give this to friends:") + card2(`<div class="mono">${N.person.person}</div>`) + `<div class="ll" style="padding:8px 16px 0">Devices</div>` + N.person.devices.map(([d, rev]) => `<div class="li">${avatar(shortAddr(d), d, 36)}<div class="grow">${shortAddr(d)}${d === me.addr ? " (this device)" : ""}${rev ? " · removed" : ""}</div></div>`).join("");
    }
    if (N.lastError) out += `<div class="err" style="padding:0 16px">${esc(N.lastError)}</div>`;
    return out;
  }
  if (p === "NODE") {
    const sh = N.sharing, ns = notSimFor("node settings");
    return gtitle("Your name") + card2(`<div class="bs mute">Shown to others as ~name next to your address. Anyone can pick any name, so it is a hint, not a proof.</div><div class="row" style="gap:4px"><div class="ofield grow"><label>Name</label><input id="myname" maxlength="24" value="${esc(UI.myname ?? me.name ?? "")}"></div><button class="btn text" data-act="saveName">Save</button></div>`) +
      gtitle("Share my position") + card2(`<div class="bs mute">Put your position in your announces, so others see you on their map. Rounded unless you choose Precise. Uses the phone's location.</div>${sh ? PRECISION.map((t, i) => `<div class="row" data-act="prec" data-i="${i}" style="cursor:pointer"><span class="radio${sh.prec === i ? " on" : ""}"></span>${esc(t)}</div>`).join("") + `<div class="row"><div class="grow col"><div>Share battery and uptime</div><div class="bs mute">Helps others see if a node is healthy.</div></div><span class="sw${sh.telemetry ? " on" : ""}" data-act="tel"></span></div>` : ns ? `<div class="bs amb">${esc(ns)}</div>` : "<div>Fetching…</div>"}`) +
      `<button class="btn text" style="margin:8px 16px;align-self:flex-start" data-act="announce">Announce now (tell others who you are)</button>` + hint("Address: " + me.addr);
  }
  if (p === "RADIO") {
    const role = N.stats[7], ns = notSimFor("node settings");
    const roles = MODE === "BLE"
      ? `<div class="bs mute">Your radio's role. It decides whether the radio passes on messages for others.</div>` + Names.ROLES.map((r, i) => `<div class="optrow" data-act="role" data-i="${i}"><span class="radio${role === i ? " on" : ""}"></span><div class="col" style="padding-top:8px"><div class="sb">${r}</div><div class="bs mute">${esc(ROLE_HELP[i])}</div></div></div>`).join("") + `<div class="bs">Tip: a radio you carry around → Auto. A radio on a roof or mast → Repeater. In a forest or at sea with no repeaters → Relay.</div>`
      : MODE === "PHONE" ? `<div class="bs mute">The phone node routes like a fixed node. Choose how:</div><div class="optrow"><span class="radio on"></span><div class="col" style="padding-top:8px"><div class="sb">Anchor (default)</div><div class="bs mute">Passes on messages, splits long paths and answers path requests on behalf of others. Best for a node on the internet.</div></div></div><div class="optrow"><span class="radio"></span><div class="col" style="padding-top:8px"><div class="sb">Repeater</div><div class="bs mute">Passes on messages, but does not split paths. Use it if your node is often offline.</div></div></div>`
      : `<div class="bs">You follow a host on another computer. Its role is set on that computer (Host settings there); a radio's role is set when you connect to the radio over Bluetooth.</div>`;
    return gtitle("Role — how this node helps others") + card2(roles) + gtitle("Regions") + card2(`<div class="bs mute">Channel messages marked with a region (e.g. #help@nordjylland) only travel where nodes have that region. Leave empty to get everything.</div><div class="row"><div class="ofield grow"><label>e.g. dk, nordjylland</label><input id="regions" value="${esc(UI.regions ?? N.regions?.names ?? "")}"></div><button class="btn text" data-act="saveRegions">Save</button></div>${!N.regions && ns ? `<div class="bs amb">${esc(ns)}</div>` : ""}`) +
      (N.radio ? gtitle("Radio") + card2(`<div class="md">${(N.radio.freq / 1e6).toFixed(3)} MHz · SF${N.radio.sf} · ${(N.radio.bw / 1e3).toFixed(1)} kHz · ${N.radio.power} dBm</div><div class="bs mute">All Må:rse radios use the same settings, so everyone hears everyone.</div>`) : "");
  }
  if (p === "APP") return `<div class="row" style="padding:8px 16px"><span class="grow">Notify about direct messages</span><span class="sw on"></span></div><div class="row" style="padding:8px 16px"><span class="grow">Notify about channel messages</span><span class="sw"></span></div>` + hint("Alerts always notify. Severe, actual alerts use their own loud channel; change it in Android's notification settings.");
  if (p === "CONNECTION") return modeChooser() + hint("In the simulator the connection is set when the phone is added (internet and/or a LoRa radio).");
  if (p === "HOST") return hint("Settings for the host running on this phone. Saved in its settings.txt; the node restarts.") + hint("Not simulated yet: host settings (backbone, peers, post office) are the simulator's scenario settings.");
  return "";
}
const ROLE_HELP = ["Does not forward. For a phone's radio in a town with repeaters.", "Forwards for others. For fixed nodes on a mast or roof.", "Forest mode: forwards and carries messages (store-carry-forward) where there are no repeaters.", "Repeater that splits long paths and answers on behalf of others.", "Handheld: becomes a relay by itself when no repeater has been heard for 2 minutes."];
function modeChooser() {
  const c = [["PHONE", "smartphone", "This phone is the node", "Recommended. Works over the internet; add your radio for LoRa. Your own address and account."],
    ["BLE", "router", "Through my radio only", "The radio over Bluetooth is the node. No internet needed; fewer features."],
    ["REMOTE", "computer", "My home computer", "Follow the maarse-host on your PC (on Wi-Fi or over VPN) with its access key."]];
  return `<div class="col" style="gap:8px;padding:0 16px">${c.map(([m, ic, t, s]) => `<div class="mode${MODE === m ? " on" : ""}"><div class="box">${icon(ic)}</div><div class="grow col" style="gap:3px"><div class="sb">${t}</div><div class="bs mute">${s}</div></div>${MODE === m ? `<span class="pri">${icon("check")}</span>` : ""}</div>`).join("")}</div>`;
}

function pagesPage() {
  const cur = Pages.cur();
  const c = cur ? Pages.cache.get(Pages.key(cur.addr, cur.path)) : null;
  const title = c ? Pages.title(c.markup) : "";
  const editing = UI.editingUrl || !cur;
  const bar = `<div class="bbar">${editing && cur ? `<button class="iconbtn on" data-act="pgCancel" title="Cancel">${icon("close")}</button>` : `<button class="iconbtn on" data-act="pgBack" title="Back">${icon("back")}</button>${!editing ? `<button class="iconbtn on" data-act="pgFwd" title="Forward" ${Pages.forward.length ? "" : "disabled"}>${icon("forward")}</button>` : ""}`}
    <div class="addr" data-act="pgEdit">${cur && !editing ? `<span class="rb ${Pages.overLora(cur.addr) ? "lora" : "net"}">${Pages.overLora(cur.addr) ? "LoRa" : "Net"}</span><div class="grow col"><div class="bm sb ell">${esc(title || Pages.url)}</div><div class="lsm mute ell">${esc(label(cur.addr) + (cur.path === "index" ? "" : " / " + cur.path))}</div></div>` : `<input id="pgurl" placeholder="maarse://address/path" value="${esc(UI.pgInput ?? Pages.url)}">`}</div>
    ${editing ? `<button class="iconbtn on" data-act="pgGo" title="Go">${icon("forward")}</button>` : Pages.loading ? `<button class="iconbtn on" data-act="pgHome" title="Stop">${icon("close")}</button>` : `<button class="iconbtn on" data-act="pgReload" title="Reload">${icon("refresh")}</button>`}
    ${!editing ? `${cur ? `<button class="iconbtn" data-act="pgFav" style="color:${Pages.favPages.includes(cur.addr) ? "#E7A94B" : "var(--on-surface-v)"}">${icon(Pages.favPages.includes(cur.addr) ? "starFilled" : "star")}</button>` : ""}<button class="iconbtn on" data-act="pgHome" title="Start page">${icon("home")}</button>` : ""}</div>`;
  let body;
  if (!cur) body = startPage();
  else if (!c) body = `<div class="col" style="padding:24px;gap:8px"><div class="bl">${esc(Pages.loading ? "Loading the page…" : Pages.status || "This page is not here yet.")}</div>${Pages.loading ? "" : `<button class="btn outlined" style="align-self:flex-start" data-act="pgReload">Try again</button>`}</div>`;
  else body = `<div class="pagebody">${Pages.status && !Pages.loading ? `<div class="bs err">${esc(Pages.status)}</div>` : ""}<div id="mu-slot"></div><div style="height:24px"></div></div>`;
  const st = !cur ? "" : Pages.loading ? `Loading from ${label(cur.addr)}…` : Pages.pageBytes > 0 ? `${Pages.overLora(cur.addr) ? "Over LoRa" : "Over the internet"} · ${Pages.pageBytes} bytes · fetched ${new Date(Pages.pageAt).toTimeString().slice(0, 5)}` : "";
  return `${bar}<div class="loadbar${Pages.loading ? " on" : ""}"><i></i></div><div class="hdiv"></div><div class="scroll" data-scroll="page">${body}</div>${st ? `<div class="pstatus">${esc(st)}</div>` : ""}`;
}
function startPage() {
  const q = (UI.pgQ || "").toLowerCase();
  const name = (a) => Pages.siteTitle(a) || (N.nodes[a]?.name ? "~" + N.nodes[a].name : a.slice(0, 8));
  const hit = (...t) => !q || t.some((x) => String(x).toLowerCase().includes(q));
  const server = (a) => `<div class="row" style="gap:12px;padding:6px 0;cursor:pointer" data-act="pgOpen" data-u="maarse://${a}/">${avatar(name(a), a)}<div class="grow col"><div class="sb">${esc(name(a))}</div><div class="lsm mute">maarse://${a.slice(0, 8)}…/</div></div></div>`;
  const favs = Pages.favPages.filter((a) => hit(name(a), a));
  const servers = [...Pages.sharing].filter((a) => !Pages.favPages.includes(a) && hit(name(a), a));
  const recent = Pages.recent.filter(([u, t]) => hit(t, u));
  return `<div class="col" style="padding:14px;gap:10px"><div class="ofield"><input id="pgq" placeholder="Search pages and page servers" value="${esc(UI.pgQ || "")}"></div>
    <div class="tm sb">Favourites</div>${favs.length ? favs.map(server).join("") : `<div class="bs mute">${q ? "No favourite matches." : "Star a page to keep it here."}</div>`}
    ${recent.length ? `<div class="tm sb">Recently visited</div>` + recent.map(([u, t]) => `<div class="col" style="padding:6px 0;cursor:pointer" data-act="pgOpen" data-u="${esc(u)}"><div class="md">${esc(t || u)}</div><div class="lsm mute ell">${esc(u)}</div></div>`).join("") : ""}
    <div class="tm sb">Page servers</div>${servers.length ? servers.map(server).join("") : `<div class="bs mute">${q ? "No page server matches." : "No page servers heard yet."}</div>`}
    <div class="bs mute">Or type an address in the bar above (maarse://&lt;32 hex characters&gt;/).</div></div>`;
}

function callOverlay() {
  const s = Calls.s;
  if (s.phase === "IDLE") return "";
  const name = label(s.peer);
  const rb = (ic, lbl, bg, act, small) => `<div class="rbtn"><button data-act="${act}" style="width:${small ? 56 : 72}px;height:${small ? 56 : 72}px;background:${bg};${bg.includes("sch") ? "color:var(--on-surface)" : ""}">${icon(ic)}</button><span class="lm">${lbl}</span></div>`;
  const secs = Math.floor((Date.now() - s.started) / 1000);
  const line = s.phase === "CALLING" ? "Calling…" : s.phase === "RINGING" ? "is calling you" : `${Math.floor(secs / 60)}:${String(secs % 60).padStart(2, "0")} · over the internet`;
  const btns = s.phase === "RINGING" ? `<div class="row" style="gap:64px">${rb("callEnd", "Reject", "#C0392B", "hangup")}${rb("call", "Answer", "#2F7D5B", "answer")}</div>`
    : `<div class="row" style="gap:32px">${s.phase === "ACTIVE" ? rb(s.muted ? "micOff" : "mic", s.muted ? "Unmute" : "Mute", s.muted ? "var(--primary)" : "var(--sch)", "mute", true) : ""}${rb("callEnd", "Hang up", "#C0392B", "hangup")}${s.phase === "ACTIVE" ? rb("volume", "Speaker", s.speaker ? "var(--primary)" : "var(--sch)", "speaker", true) : ""}</div>`;
  return `<div class="call"><div style="height:48px"></div>${avatar(name, s.peer, 120)}<div style="height:20px"></div><div class="hm b">${esc(name)}</div><div class="mute">${line}</div>${s.phase === "ACTIVE" ? `<div class="bs mute" style="padding-top:8px;text-align:center">Sound is not simulated: the call's signalling goes through the simulated network, audio does not.</div>` : ""}<div class="grow"></div>${btns}<div style="height:32px"></div></div>`;
}

function dialogHtml() {
  const d = UI.dialog;
  if (!d) return "";
  let inner = "";
  if (d.kind === "newchat") inner = `<div class="hs">New chat</div><div class="ofield"><label>Their address or account (32 characters)</label><input id="dlgAddr"></div><div class="lm">or join a channel</div><div class="ofield"><label>Channel name, e.g. public</label><input id="dlgCh"></div><div class="acts"><button class="btn text" data-act="dlgClose">Cancel</button><button class="btn text" data-act="newchatOk">Open</button></div>`;
  if (d.kind === "nick") inner = `<div class="hs">Your name for ${shortAddr(d.addr)}</div><div class="ofield"><label>Only on this phone</label><input id="dlgNick" value="${esc(N.nick[d.addr] || "")}"></div><div class="acts"><button class="btn text" data-act="dlgClose">Cancel</button><button class="btn text" data-act="nickOk">Save</button></div>`;
  if (d.kind === "msg") {
    const m = N.msgs.find((x) => x.id === d.id);
    inner = `<div class="hs">${m.mine ? "Your message" : "Message"}</div><div class="col" style="gap:4px;align-items:flex-start">${m.file ? "" : `<button class="btn text" data-act="copyMsg">Copy text</button>`}<button class="btn text" style="color:var(--error)" data-act="delMsg">Delete from this phone</button>${m.mine ? `<div class="bs">Already sent messages cannot be called back; this only removes your copy.</div>` : ""}</div><div class="acts"><button class="btn text" data-act="dlgClose">Close</button></div>`;
  }
  if (d.kind === "delchat") inner = `<div class="hs">Delete the chat with ${esc(convTitle(d.conv))}?</div><div class="bm mute">Removes it from this phone and your node's history. The other person keeps their copy.${d.conv.startsWith("ch:") ? " You stay in the channel." : " It comes back if they write again."}</div><div class="acts"><button class="btn text" data-act="dlgClose">Cancel</button><button class="btn text" style="color:var(--error)" data-act="delChatOk">Delete</button></div>`;
  if (d.kind === "pic") inner = `<div class="hs">This picture goes over LoRa</div><div class="bm">Pick how it is sent. Smaller arrives sooner and uses less air time.</div>${d.opts ? d.opts.map((o, i) => `<div class="row" style="gap:12px;padding:8px;border-radius:10px;background:var(--sc);cursor:pointer" data-act="picPick" data-i="${i}"><img src="${o.url}" style="width:72px;height:72px;object-fit:contain;border-radius:6px;background:#fff;${o.mime === "image/png" ? "image-rendering:pixelated" : ""}"><div class="grow col"><div class="sb">${o.title}</div><div class="bs">${(o.data.length / 1024).toFixed(1)} KB · about ${Math.round(o.data.length * 8 / 1000 * 1.6)} s</div><div class="bs mute">${o.sub}</div></div></div>`).join("") : `<div class="mute">Making the small versions…</div>`}<div class="acts"><button class="btn text" data-act="dlgClose">Cancel</button></div>`;
  if (d.kind === "nolora") inner = `<div class="hs">No internet route</div><div class="bm">The recipient is not reachable over the internet right now. Files over 8 KB are not sent over LoRa, so this one (${Math.ceil(d.data.length / 1024)} KB) waits until there is an internet route.${d.mime.startsWith("image/") ? "<br><br>Compress the picture to 8 KB and send it over LoRa now?" : "<br><br>Only pictures can be compressed."}</div><div class="acts"><button class="btn text" data-act="dlgClose">Cancel</button><button class="btn text" data-act="sendFull">Send full size</button>${d.mime.startsWith("image/") ? `<button class="btn text" data-act="toPic">Send over LoRa</button>` : ""}</div>`;
  return `<div class="scrim" data-act="scrim"><div class="dialog">${inner}</div></div>`;
}
function sheetHtml() {
  if (!UI.sheet) return "";
  return `<div class="scrim" data-act="sheetClose"></div><div class="sheet"><div class="handle"></div><div class="tl b">How this phone connects</div><div class="bm mute">You can change this any time. Your chats and your account stay.</div><div style="height:6px"></div>${modeChooser().replace('style="gap:8px;padding:0 16px"', 'style="gap:8px"')}<div class="bs mute" style="padding:8px 4px">In the simulator this phone is bound to node ${esc(NODE_LABEL || "?")}${MODE === "BLE" ? " (a handheld radio)" : MODE === "PHONE" ? " (the app on the internet)" : " (a host)"}. To change it, close the phone and add a new one.</div><button class="btn" data-act="sheetClose">Close</button></div>`;
}

// ---------- render ----------
let renderQueued = false;
function render() { if (!renderQueued) { renderQueued = true; requestAnimationFrame(doRender); } }
function doRender() {
  renderQueued = false;
  const root = $("#app");
  const ae = document.activeElement, focusId = ae && ae.id, sel = focusId && ae.selectionStart != null ? [ae.selectionStart, ae.selectionEnd] : null;
  root.querySelectorAll("[data-scroll]").forEach((e) => (UI.scroll[e.dataset.scroll + UI.tab] = e.scrollTop));
  const msgsEl = $("#msgs"), atBottom = msgsEl ? msgsEl.scrollHeight - msgsEl.scrollTop - msgsEl.clientHeight < 40 : true;
  const t = topScreen();
  if (t.s !== "chat") N.openConv = null;
  let screen;
  if (t.s === "home") screen = home();
  else if (t.s === "chat") screen = chatScreen(t.conv);
  else if (t.s === "contact") screen = `<div class="main">${contactScreen(t.addr)}</div>`;
  else screen = `<div class="subhead"><button class="iconbtn on" data-act="back" title="Back">${icon("back")}</button><div class="tl">${esc(t.s === "net" ? NET_TITLES[t.p] : ME_TITLES[t.p])}</div></div><div class="scroll" data-scroll="sub${t.p}">${t.s === "net" ? netPage(t.p) : mePage(t.p)}</div>`;
  const unread = Object.values(N.unread).reduce((a, b) => a + b, 0);
  const nav = `<div class="navbar">${TABS.map(([k, l, ic]) => `<button class="navitem${UI.tab === k ? " sel" : ""}" data-act="tab" data-t="${k}"><span class="pill">${icon(ic)}${k === "CHATS" && unread ? `<span class="badge">${unread}</span>` : ""}</span><span class="lm">${l}</span></button>`).join("")}</div>`;
  const battery = N.stats[85];
  const status = `<div class="statusbar"><span>${new Date().toTimeString().slice(0, 5)}</span><span class="sp"></span>${MODE === "BLE" ? `<span class="lsm mute" style="margin-right:6px">LoRa</span>` : ""}${N.alive ? icon("wifi", 16) + icon("signal", 16) : `<span class="lsm" style="margin-right:4px">No service</span>`}${icon("battery", 16)}${battery != null ? `<span class="lsm">${battery}%</span>` : ""}</div>`;
  const toast = UI.toast ? `<div class="toast"><div class="grow"><div class="simtag">Simulator</div>${esc(UI.toast.text)}</div><button class="iconbtn" style="width:24px;height:24px;color:inherit" data-act="toastClose">${icon("close", 18)}</button></div>` : "";
  root.innerHTML = status + `<div class="main">${screen}</div>` + nav + dialogHtml() + sheetHtml() + callOverlay() + toast;
  root.querySelectorAll("[data-scroll]").forEach((e) => { const v = UI.scroll[e.dataset.scroll + UI.tab]; if (v) e.scrollTop = v; });
  const m2 = $("#msgs");
  if (m2) { const n = m2.children.length; if (atBottom || UI.lastMsgCount !== n) m2.scrollTop = m2.scrollHeight; UI.lastMsgCount = n; }
  const slot = $("#mu-slot");
  if (slot && MU) {
    const cur = Pages.cur(), c = Pages.cache.get(Pages.key(cur.addr, cur.path));
    try { slot.append(MU.renderMarkup(c.markup, cur)); } catch (e) { slot.textContent = "The page could not be drawn: " + e.message; }
  } else if (slot) slot.textContent = "Loading the page renderer…";
  if (focusId) { const e = document.getElementById(focusId); if (e) { e.focus(); if (sel && e.setSelectionRange) try { e.setSelectionRange(sel[0], sel[1]); } catch (x) { /* not a text field */ } } }
  const comp = $("#composer"); if (comp) { comp.style.height = "24px"; comp.style.height = Math.min(120, comp.scrollHeight) + "px"; }
}

// ---------- input ----------
document.addEventListener("input", (e) => {
  const id = e.target.id, v = e.target.value;
  const t = topScreen();
  if (id === "composer") { UI.draft[t.conv] = v; UI.err = null; const was = !!$(".round[data-act=mic]"); if (was === !!v.trim() || v.length < 2) render(); else { const s = $(".round[data-act=send]"); if (s) s.disabled = !(v.trim() && bytes(v) <= maxText(t.conv)); } }
  else if (id === "filter") { UI.filter = v; render(); }
  else if (id === "annq") { UI.annQuery = v; render(); }
  else if (id === "pgurl") UI.pgInput = v;
  else if (id === "pgq") { UI.pgQ = v; render(); }
  else if (id === "myname") UI.myname = v;
  else if (id === "regions") UI.regions = v;
});
document.addEventListener("keydown", (e) => {
  if (e.key === "Enter" && e.target.id === "composer" && !e.shiftKey) { e.preventDefault(); act("send"); }
  if (e.key === "Enter" && e.target.id === "pgurl") act("pgGo");
});
let pressTimer = null;
document.addEventListener("pointerdown", (e) => {
  const b = e.target.closest("[data-act=bubble], [data-act=open]");
  if (!b) return;
  pressTimer = setTimeout(() => { pressTimer = "long"; UI.dialog = b.dataset.act === "bubble" ? { kind: "msg", id: +b.dataset.id } : { kind: "delchat", conv: b.dataset.c }; render(); }, 550);
});
document.addEventListener("pointerup", () => { if (pressTimer !== "long") clearTimeout(pressTimer); });
document.addEventListener("click", (e) => {
  if (pressTimer === "long") { pressTimer = null; return; }
  const el = e.target.closest("[data-act]");
  if (!el) return;
  if (el.dataset.act === "scrim" && e.target !== el) return;
  e.stopPropagation();
  act(el.dataset.act, el);
});

function act(a, el) {
  const d = el?.dataset || {};
  const t = topScreen();
  switch (a) {
    case "tab": select(d.t); return;
    case "back": back(); return;
    case "open": UI.err = null; if (UI.tab !== "CHATS" && t.s === "contact") { UI.stacks.CHATS = []; UI.tab = "CHATS"; } go({ s: "chat", conv: d.c }); return;
    case "contact": go({ s: "contact", addr: d.a }); return;
    case "net": go({ s: "net", p: d.p }); if (d.p === "NEIGHBOURS") send(Cmd.getNeighbors()); return;
    case "page": go({ s: "page", p: d.p }); if (d.p === "ACCOUNT" && MODE !== "BLE") send(Cmd.getPerson()); return;
    case "sheet": UI.sheet = true; break;
    case "sheetClose": UI.sheet = false; break;
    case "newchat": UI.dialog = { kind: "newchat" }; break;
    case "scrim": case "dlgClose": UI.dialog = null; break;
    case "newchatOk": {
      const raw = $("#dlgAddr").value.toLowerCase(), addr = (raw.match(/[0-9a-f]{32}/) || [raw.trim()])[0], ch = $("#dlgCh").value;
      if (/^[0-9a-f]{32}$/.test(addr)) { UI.dialog = null; go({ s: "chat", conv: "dm:" + addr }); return; }
      if (ch.trim()) { joinChannel(ch); UI.dialog = null; }
      break;
    }
    case "nick": UI.dialog = { kind: "nick", addr: d.a }; break;
    case "nickOk": { const v = $("#dlgNick").value.trim(); if (v) N.nick[UI.dialog.addr] = v; else delete N.nick[UI.dialog.addr]; UI.dialog = null; break; }
    case "copyMsg": navigator.clipboard?.writeText(N.msgs.find((x) => x.id === UI.dialog.id)?.text || ""); UI.dialog = null; break;
    case "delMsg": N.msgs = N.msgs.filter((x) => x.id !== UI.dialog.id); UI.dialog = null; break;
    case "delChatOk": N.msgs = N.msgs.filter((x) => x.conv !== UI.dialog.conv); UI.dialog = null; break;
    case "bubble": UI.shown = UI.shown === +d.id ? null : +d.id; break;
    case "send": { const txt = UI.draft[t.conv] || ""; if (!N.me || !txt.trim()) return; UI.err = sendText(t.conv, txt); if (!UI.err) UI.draft[t.conv] = ""; break; }
    case "call": Calls.start(d.a); return;
    case "hangup": Calls.hangup(); return;
    case "answer": Calls.answer(); return;
    case "mute": Calls.s.muted = !Calls.s.muted; break;
    case "speaker": Calls.s.speaker = !Calls.s.speaker; break;
    case "trace": trace(d.a); simNoteIfLater(); break;
    case "fav": { const i = N.favUsers.indexOf(d.a); if (i >= 0) N.favUsers.splice(i, 1); else N.favUsers.push(d.a); break; }
    case "announce": send(Cmd.announce()); break;
    case "sync": personSetup(PersonOp.SYNC); break;
    case "person": personSetup(+d.op); break;
    case "saveName": { const v = ($("#myname")?.value || "").trim(); send(Cmd.setName(v)); send(Cmd.getName()); send(Cmd.announce()); UI.myname = undefined; break; }
    case "saveRegions": send(Cmd.setRegions(N.regions?.strict || false, ($("#regions")?.value || "").trim())); send(Cmd.getRegions()); break;
    case "prec": { const sh = N.sharing; send(Cmd.setSharing(+d.i, Math.round(sh.lat * 1e7), Math.round(sh.lon * 1e7), sh.telemetry)); send(Cmd.getSharing()); break; }
    case "tel": { const sh = N.sharing; send(Cmd.setSharing(sh.prec, Math.round(sh.lat * 1e7), Math.round(sh.lon * 1e7), !sh.telemetry)); send(Cmd.getSharing()); break; }
    case "role": send(Cmd.setRole(+d.i)); send(Cmd.getStats()); break;
    case "hideAlert": N.dismissed.add(d.k); break;
    case "toastClose": UI.toast = null; break;
    case "attach": pickFile(t.conv); return;
    case "mic": startRec(); return;
    case "recCancel": stopRec(false); return;
    case "recSend": stopRec(true); return;
    case "play": playVoice(+d.id); return;
    case "picPick": { const o = UI.dialog.opts[+d.i], conv = UI.dialog.conv; UI.dialog = null; UI.err = sendFile(conv, o.data, o.mime, o.name, LORA_FILE); break; }
    case "sendFull": { const x = UI.dialog; UI.dialog = null; UI.err = sendFile(x.conv, x.data, x.mime, x.name); break; }
    case "toPic": { const x = UI.dialog; askPicture(x.conv, x.data, x.mime, x.name); return; }
    // Pages
    case "pgBack": if (Pages.history.length) Pages.back(); else select("CHATS"); return;
    case "pgFwd": Pages.fwd(); return;
    case "pgReload": Pages.reload(); return;
    case "pgHome": UI.editingUrl = false; Pages.home(); return;
    case "pgEdit": if (!UI.editingUrl && Pages.cur()) { UI.editingUrl = true; UI.pgInput = Pages.url; render(); setTimeout(() => { const i = $("#pgurl"); if (i) { i.focus(); i.select(); } }, 30); } return;
    case "pgCancel": UI.editingUrl = false; break;
    case "pgGo": { const v = ($("#pgurl")?.value || "").trim(); if (v) { UI.editingUrl = false; UI.pgInput = undefined; Pages.open(v); } return; }
    case "pgOpen": Pages.open(d.u); return;
    case "pgFav": { const a2 = Pages.cur().addr, i = Pages.favPages.indexOf(a2); if (i >= 0) Pages.favPages.splice(i, 1); else Pages.favPages.push(a2); break; }
    case "noop": return;
  }
  render();
}
function simNoteIfLater() { /* the engine's own Error reply (if any) becomes the note */ }

// ---------- files, pictures and voice from the composer ----------
function pickFile(conv) {
  const inp = document.createElement("input");
  inp.type = "file";
  inp.onchange = async () => {
    const f = inp.files[0]; if (!f) return;
    const data = new Uint8Array(await f.arrayBuffer()), mime = f.type || "application/octet-stream", name = f.name || "file";
    queryFileLimit(conv.slice(3), (l) => {
      note(l == null ? "File route: no answer from the node" : `File route: up to ${Math.floor(l.max / 1024)} KB, via LoRa ${l.viaLora}, post office ${l.mailbox}`);
      const lora = l ? l.viaLora && !l.mailbox : fileMax() <= LORA_FILE;
      if (l && l.mailbox && data.length > LORA_FILE) { UI.dialog = { kind: "nolora", conv, data, mime, name }; render(); }
      else if (mime.startsWith("image/") && lora) askPicture(conv, data, mime, name);
      else { UI.err = sendFile(conv, data, mime, name); render(); }
    });
  };
  inp.click();
}
async function canvasOf(data, mime, maxSide) {
  const bmp = await createImageBitmap(new Blob([data], { type: mime }));
  const k = Math.min(1, maxSide / Math.max(bmp.width, bmp.height));
  const c = document.createElement("canvas");
  c.width = Math.max(1, Math.round(bmp.width * k)); c.height = Math.max(1, Math.round(bmp.height * k));
  c.getContext("2d").drawImage(bmp, 0, 0, c.width, c.height);
  return c;
}
const blobBytes = async (c, type, q) => new Uint8Array(await (await new Promise((r) => c.toBlob(r, type, q))).arrayBuffer());
// Colour, grey or dots (Images.kt shrink / grey / dots), each at most 8 KB.
async function askPicture(conv, data, mime, name) {
  UI.dialog = { kind: "pic", conv, opts: null }; render();
  const base = name.replace(/\.[^.]*$/, "");
  const opts = [];
  try {
    let colour = data, cm = mime;
    for (let side = 640; colour.length > LORA_FILE && side >= 64; side = Math.round(side * 0.75)) { colour = await blobBytes(await canvasOf(data, mime, side), "image/webp", 0.5); cm = "image/webp"; }
    opts.push({ title: "Colour", sub: "Best picture", data: colour, mime: cm, name: cm === mime ? name : base + ".webp" });
    const g = await canvasOf(data, mime, 320), gx = g.getContext("2d"), px = gx.getImageData(0, 0, g.width, g.height);
    for (let i = 0; i < px.data.length; i += 4) { const y = 0.299 * px.data[i] + 0.587 * px.data[i + 1] + 0.114 * px.data[i + 2]; px.data[i] = px.data[i + 1] = px.data[i + 2] = y; }
    gx.putImageData(px, 0, 0);
    let grey = await blobBytes(g, "image/webp", 0.4);
    for (let q = 0.3; grey.length > LORA_FILE && q > 0.05; q -= 0.1) grey = await blobBytes(g, "image/webp", q);
    opts.push({ title: "Grey, small", sub: "Fastest; text stays readable", data: grey, mime: "image/webp", name: base + ".webp" });
    const dc = await canvasOf(data, mime, 200), dx = dc.getContext("2d"), dp = dx.getImageData(0, 0, dc.width, dc.height);
    for (let i = 0; i < dp.data.length; i += 4) { const y = 0.299 * dp.data[i] + 0.587 * dp.data[i + 1] + 0.114 * dp.data[i + 2]; const v = y > 128 ? 255 : 0; dp.data[i] = dp.data[i + 1] = dp.data[i + 2] = v; }
    dx.putImageData(dp, 0, 0);
    opts.push({ title: "Dots", sub: "Retro look, best for drawings", data: await blobBytes(dc, "image/png"), mime: "image/png", name: base + ".png" });
  } catch (e) { note("Picture: " + e.message); }
  for (const o of opts) o.url = URL.createObjectURL(new Blob([o.data], { type: o.mime }));
  if (UI.dialog?.kind === "pic") { UI.dialog.opts = opts.filter((o) => o.data.length <= LORA_FILE); render(); }
}
async function startRec() {
  const conv = topScreen().conv;
  try {
    const stream = await navigator.mediaDevices.getUserMedia({ audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true } });
    const ctx = new AudioContext(), src = ctx.createMediaStreamSource(stream), node = ctx.createScriptProcessor(4096, 1, 1), chunks = [];
    node.onaudioprocess = (e) => chunks.push(new Float32Array(e.inputBuffer.getChannelData(0)));
    src.connect(node); node.connect(ctx.destination);
    UI.rec = { stream, ctx, node, chunks, started: Date.now(), conv };
    UI.recTimer = setInterval(() => { if ((Date.now() - UI.rec.started) / 1000 >= 20) stopRec(true); else render(); }, 250);
  } catch (e) { UI.err = "The microphone could not be opened"; }
  render();
}
async function stopRec(sendIt) {
  const r = UI.rec; if (!r) return;
  UI.rec = null; clearInterval(UI.recTimer);
  r.node.disconnect(); r.stream.getTracks().forEach((t) => t.stop());
  const rate = r.ctx.sampleRate; await r.ctx.close();
  render();
  if (!sendIt) return;
  const len = r.chunks.reduce((n, c) => n + c.length, 0);
  if (len < rate * 0.3) { UI.err = "Too short"; render(); return; }
  const all = new Float32Array(len); let o = 0; for (const c of r.chunks) { all.set(c, o); o += c.length; }
  const off = new OfflineAudioContext(1, Math.ceil(len * 8000 / rate), 8000);
  const buf = off.createBuffer(1, len, rate); buf.copyToChannel(all, 0);
  const s = off.createBufferSource(); s.buffer = buf; s.connect(off.destination); s.start();
  const out = (await off.startRendering()).getChannelData(0);
  let peak = 0; for (const v of out) peak = Math.max(peak, Math.abs(v));
  const gain = peak > 0 ? Math.min(0.9 / peak, 8) : 1;
  const pcm = new Int16Array(out.length);
  for (let i = 0; i < out.length; i++) pcm[i] = Math.max(-32768, Math.min(32767, Math.round(out[i] * gain * 32767)));
  try { const data = await Reuse.voiceEncode(pcm); UI.err = sendFile(r.conv, data, VOICE_MIME, "voice.c2"); }
  catch (e) { UI.err = "Could not encode the voice message: " + e.message; }
  render();
}
let player = null;
async function playVoice(id) {
  if (player) { player.pause(); player = null; if (UI.playing === id) { UI.playing = null; render(); return; } }
  const m = N.msgs.find((x) => x.id === id); if (!m?.data) return;
  try {
    player = new Audio(URL.createObjectURL(Reuse.wavBlob(await Reuse.voiceDecode(m.data))));
    UI.playing = id; player.onended = () => { UI.playing = null; player = null; render(); };
    player.play();
  } catch (e) { note("Voice: " + e.message); }
  render();
}

// ---------- the link to the simulator ----------
const decoder = new FrameDecoder();
window.addEventListener("message", (e) => {
  const d = e.data;
  if (!d || e.source !== parent) return;
  if (d.simAlive !== undefined && d.simAlive !== N.alive) {
    N.alive = d.simAlive;
    N.offlineWhy = N.alive ? "" : "The simulated node is down (power cut, destroyed or switched off).";
    if (N.alive) flushQueued();
  }
  if (d.simFrames) for (const f of d.simFrames) for (const fr of decoder.push(Uint8Array.from(f))) { try { onEvent(decodeEvent(fr)); } catch (err) { note("Event skipped: " + err.message); } }
  // The engine was rebuilt (a phone was added, the scenario changed): the
  // node's phone state is new, so connect again; chats stay on this phone.
  if (d.simReset) onOpen();
  if (d.simFrames || d.simAlive !== undefined || d.simReset) render();
});

// Dark or light, following the simulator page (same origin).
function theme() {
  let dark = false;
  try { const t = parent.document.documentElement.dataset.theme; dark = t === "dark" || (t !== "light" && matchMedia("(prefers-color-scheme: dark)").matches); }
  catch (e) { dark = matchMedia("(prefers-color-scheme: dark)").matches; }
  if (Q.get("theme")) dark = Q.get("theme") === "dark";
  if (dark !== UI.dark) { UI.dark = dark; document.documentElement.classList.toggle("dark", dark); render(); }
}
setInterval(theme, 500);
theme();

// Status every 15 s (5 s with neighbours while the Network tab is open), as Node.kt's poll.
let lastPoll = 0;
setInterval(() => {
  const fast = UI.tab === "NETWORK";
  if (Date.now() - lastPoll < (fast ? 5000 : 15000)) return;
  lastPoll = Date.now();
  send(Cmd.getStats());
  if (fast) send(Cmd.getNeighbors());
}, 1000);
setInterval(() => { if (Calls.s.phase === "ACTIVE" || UI.tab === "CONTACTS" || topScreen().p === "ANNOUNCES") render(); }, 1000);

onOpen();
render();
// For tests driving the page (headless browser): the state and the actions.
window.MaarseApp = { N, UI, act, Pages, Calls };
