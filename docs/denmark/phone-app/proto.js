// The companion protocol as the Android app speaks it: a line-by-line port of
// android/app/src/main/java/dk/maarse/app/Proto.kt (Cmd, decodeEvent,
// FrameDecoder, Names, STATS, formatStat). Frames:
//
//   length (u16, little endian) | type (u8) | content
//
// Commands have type < 0x80; replies and events >= 0x80.
"use strict";

const MAX_FRAME = 300;
const utf8 = new TextEncoder();
const fromUtf8 = new TextDecoder();
const hex = (b) => Array.from(b, (x) => x.toString(16).padStart(2, "0")).join("");
const unhex = (h) => Uint8Array.from((h.match(/../g) || []).map((x) => parseInt(x, 16)));
const shortAddr = (h) => String(h).slice(0, 8);

class W {
  constructor() { this.b = []; }
  u8(v) { this.b.push(v & 0xff); return this; }
  u16(v) { this.u8(v); this.u8(v >> 8); return this; }
  u32(v) { v = Number(v); this.u16(v & 0xffff); this.u16(Math.floor(v / 65536) & 0xffff); return this; }
  u64(v) { let x = BigInt.asUintN(64, BigInt(v)); for (let i = 0; i < 8; i++) { this.u8(Number(x & 0xffn)); x >>= 8n; } return this; }
  bytes(v) { for (const x of v) this.b.push(x & 0xff); return this; }
  text(s) { return this.bytes(utf8.encode(s)); }
  addr(h) { return this.bytes(unhex(h)); }
}

function frame(kind, body) {
  const w = new W().u8(kind);
  if (body) body(w);
  const inner = w.b;
  return Uint8Array.from([inner.length & 0xff, inner.length >> 8, ...inner]);
}

/** Commands from the app to the node (Proto.kt object Cmd). */
const Cmd = {
  getInfo: () => frame(0x01),
  sendDirect: (req, dest, text) => frame(0x02, (w) => w.u16(req).addr(dest).text(text)),
  sendChannel: (req, ch, text) => frame(0x03, (w) => w.u16(req).addr(ch).text(text)),
  join: (name) => frame(0x04, (w) => w.text(name)),
  announce: () => frame(0x05),
  getStats: () => frame(0x09),
  getRadio: () => frame(0x0b),
  getAdmins: () => frame(0x0e),
  setTime: () => frame(0x0f, (w) => w.u32(Math.floor(Date.now() / 1000))),
  setRole: (r) => frame(0x10, (w) => w.u8(r)),
  setRegions: (strict, names) => frame(0x11, (w) => w.u8(strict ? 1 : 0).text(names)),
  getRegions: () => frame(0x12),
  traceRoute: (req, dest) => frame(0x20, (w) => w.u16(req).addr(dest)),
  getNeighbors: () => frame(0x21),
  setSharing: (prec, latE7, lonE7, tel) => frame(0x22, (w) => w.u8(prec).u32(latE7 >>> 0).u32(lonE7 >>> 0).u8(tel ? 1 : 0)),
  getSharing: () => frame(0x23),
  setName: (name) => frame(0x24, (w) => w.text(name)),
  getName: () => frame(0x25),
  // Site of a fixed node: antenna height (0.1 m), gain (0.5 dB), power (filled in by the node), owner.
  setSite: (heightDm, gainHdb, owner) => frame(0x3a, (w) => w.u16(heightDm).u8(gainHdb & 0xff).u8(0).text(owner)),
  getSite: () => frame(0x3b),
  getBackbones: () => frame(0x39),
  // History on maarse-host (maarse-host/src/history.rs), as Proto.kt.
  histGet: (req, conv, before, limit) => frame(0x30, (w) => { w.u16(req); if (conv) w.addr(conv); else w.bytes(new Uint8Array(16)); w.u64(before).u8(limit); }),
  histMarkRead: (conv, upTo) => frame(0x31, (w) => w.addr(conv).u64(upTo)),
  /** flags: 1 outgoing, 2 the whole conversation. */
  histDelete: (conv, flags, unix, content) => frame(0x33, (w) => w.addr(conv).u8(flags).u32(unix).bytes(content)),
  // Files (the same frames as the web client's sendFile).
  fileBegin: (req, dest, size, mime, name) => frame(0x06, (w) => {
    const m = utf8.encode(mime).slice(0, 60);
    w.u16(req).addr(dest).u32(size).u8(m.length).bytes(m).bytes(utf8.encode(name).slice(0, 60));
  }),
  fileData: (req, offset, data) => frame(0x07, (w) => w.u16(req).u32(offset).bytes(data)),
  fileEnd: (req) => frame(0x08, (w) => w.u16(req)),
  cancelFile: (fileId) => frame(0x38, (w) => w.u64(fileId)),
  getFileLimit: (dest) => frame(0x14, (w) => w.addr(dest)),
  /** Asks a node for a maarse:// page or a picture on one (0x26). */
  requestPage: (req, dest, path) => frame(0x26, (w) => w.u16(req).addr(dest).text(path)),
  /** A note for a page's notice board (`@board`): at most 100 bytes. */
  postBoard: (req, dest, path, note) => frame(0x2e, (w) => { const p = utf8.encode(path); w.u16(req).addr(dest).u8(p.length).bytes(p).text(note); }),
  /** Call signalling and audio (0x27): content belongs to the clients, see Calls.kt. */
  sendCall: (dest, data) => frame(0x27, (w) => w.addr(dest).bytes(data)),
  // Linked devices (maarse-host only).
  linkDevice: (req, device) => frame(0x35, (w) => w.u16(req).addr(device)),
  getPerson: () => frame(0x36),
  personSetup: (req, op, text = "") => frame(0x37, (w) => w.u16(req).u8(op).text(text)),
};

const PersonOp = { CREATE: 1, RESTORE: 2, AWAIT_LINK: 3, LEAVE: 4, CANCEL: 5, REMOVE: 6, BACKUP: 7, RESTORE_BACKUP: 8, SYNC: 9 };
const PersonState = { NONE: 0, AWAITING: 1, MEMBER: 2 };

class Rd {
  constructor(f) { this.f = f; }
  u8(o) { return o < this.f.length ? this.f[o] : 0; }
  s8(o) { const v = this.u8(o); return v > 127 ? v - 256 : v; }
  u16(o) { return this.u8(o) | (this.u8(o + 1) << 8); }
  s16(o) { const v = this.u16(o); return v > 32767 ? v - 65536 : v; }
  u32(o) { return this.u16(o) + this.u16(o + 2) * 65536; }
  i32(o) { return this.u32(o) | 0; }
  u64(o) { let v = 0n; for (let i = 7; i >= 0; i--) v = (v << 8n) | BigInt(this.u8(o + i)); return v; }
  addr(o) { return o + 16 <= this.f.length ? hex(this.f.subarray(o, o + 16)) : ""; }
  txt(o, end = this.f.length) { return o < end ? fromUtf8.decode(this.f.subarray(o, end)) : ""; }
}

/** Events from the node (Proto.kt decodeEvent). Message ids are BigInt. */
function decodeEvent(f) {
  if (!f.length) return { t: "Unknown", kind: 0 };
  const r = new Rd(f);
  const t = r.u8(0);
  const sub = (o) => f.subarray(Math.min(o, f.length));
  switch (t) {
    case 0x81: return { t: "Info", addr: r.addr(1), repeater: r.u8(17) !== 0, version: r.u8(18), maxText: r.u8(19), maxFile: f.length >= 24 ? r.u32(20) : 0, identity: f.length >= 88 ? f.slice(24, 88) : null };
    case 0x82: return { t: "Sent", req: r.u16(1), msgId: r.u64(3) };
    case 0x83: return { t: "Direct", from: r.addr(1), hops: r.u8(17), text: r.txt(18) };
    case 0x84: return { t: "Channel", channel: r.addr(1), from: r.addr(17), hops: r.u8(33), text: r.txt(34) };
    case 0x85: return { t: "Acked", msgId: r.u64(1) };
    case 0x86: return { t: "Route", dest: r.addr(1), hops: r.u8(17) };
    case 0x87: return { t: "Heard", addr: r.addr(1), hops: r.u8(17), identity: f.length >= 82 ? f.slice(18, 82) : null };
    case 0x88: return { t: "Joined", channel: r.addr(1), name: r.txt(17) };
    case 0x8e: return { t: "Log", text: r.txt(1) };
    case 0x8f: return { t: "Error", req: r.u16(1), code: r.u8(3), text: r.txt(4) };
    case 0x90: return { t: "Deposited", msgId: r.u64(1) };
    case 0x91: { const items = []; for (let i = 1; i < f.length - 4; i += 5) items.push([r.u8(i), r.i32(i + 1)]); return { t: "Stats", items }; }
    case 0x92: return { t: "Radio", cfg: { freq: r.u32(1), bw: r.u32(5), sf: r.u8(9), cr: r.u8(10), power: r.s8(11), duty: r.u16(12), preamble: r.u16(14), announce: r.u8(16) } };
    case 0x95: return { t: "Regions", strict: r.u8(1) !== 0, names: r.txt(2) };
    case 0xa0: {
      const k = r.u8(19), ids = [];
      for (let i = 0; i < k; i++) ids.push(r.u16(20 + 2 * i));
      const m = r.u8(20 + 2 * k), o = 21 + 2 * k, out = [], back = [];
      for (let i = o; i < Math.min(o + m, f.length); i++) out.push(r.s8(i));
      for (let i = o + m; i < f.length; i++) back.push(r.s8(i));
      return { t: "Trace", req: r.u16(1), dest: r.addr(3), path: ids, out, back };
    }
    case 0xa1: {
      const items = [];
      for (let o = 3; o <= f.length - 15; o += 15) items.push({ id: r.u16(o), prefix: hex(f.subarray(o + 2, o + 6)), snr: r.s8(o + 6), rssi: r.s16(o + 7), ageS: r.u32(o + 9), count: r.u16(o + 13) });
      return { t: "Neighbors", part: r.u8(1), last: r.u8(2) !== 0, items };
    }
    case 0xa2: return { t: "Sharing", prec: r.u8(1), lat: r.i32(2) / 1e7, lon: r.i32(6) / 1e7, telemetry: r.u8(10) !== 0 };
    case 0xa3: return nodeInfo(r.addr(1), r.u8(17), sub(18));
    case 0xa4: return { t: "Name", name: r.txt(1) };
    case 0xbb: return { t: "Site", heightDm: r.u8(1) | r.u8(2) << 8, gainHdb: r.u8(3) << 24 >> 24, txDbm: r.u8(4) << 24 >> 24, owner: r.txt(5) };
    case 0xa7: return { t: "CarryAcked", msgId: r.u64(1) };
    case 0xb4: return { t: "IdentityChanged", addr: r.addr(1) };
    case 0x89: { const ml = r.u8(29); return { t: "FileIncoming", fileId: r.u64(1), from: r.addr(9), size: r.u32(25), mime: r.txt(30, Math.min(30 + ml, f.length)), name: r.txt(Math.min(30 + ml, f.length)) }; }
    case 0x8a: return { t: "FileChunk", fileId: r.u64(1), offset: r.u32(9), data: sub(13) };
    case 0x8b: return { t: "FileProgress", fileId: r.u64(1), done: r.u16(9), total: r.u16(11), outgoing: r.u8(13) !== 0 };
    case 0x8c: return { t: "FileDone", fileId: r.u64(1), ok: r.u8(9) !== 0, outgoing: r.u8(10) !== 0, text: r.txt(11) };
    case 0x8d: return { t: "FileStarted", req: r.u16(1), fileId: r.u64(3), chunks: r.u16(11) };
    case 0x96: return { t: "FileLimit", dest: r.addr(1), max: r.u32(17), viaLora: (r.u8(21) & 1) !== 0, mailbox: (r.u8(21) & 2) !== 0 };
    case 0xba: {
      const vl = r.u8(24), nl = r.u8(25 + vl);
      const s = (o, n) => (o + n <= f.length ? fromUtf8.decode(f.subarray(o, o + n)) : "");
      return { t: "Backbone", link: r.u16(1), flags: r.u8(3), upSecs: r.u32(4), addr: r.addr(8), version: s(25, vl), name: s(26 + vl, nl), regions: r.u8(26 + vl + nl) };
    }
    case 0xa5: return { t: "Call", from: r.addr(1), data: sub(17) };
    case 0xb5: return { t: "DeviceLinked", device: r.addr(1), index: r.u8(17), words: r.txt(18) };
    case 0xb6: {
      const n = r.u8(24), devices = [];
      for (let i = 0; i < n; i++) devices.push([r.addr(25 + 17 * i), r.u8(41 + 17 * i) !== 0]);
      return { t: "Person", state: r.u8(1), master: (r.u8(2) & 1) !== 0, person: r.addr(3), index: r.u8(19), version: r.u32(20), devices, words: r.txt(25 + 17 * n) };
    }
    case 0xb7: return { t: "PersonPhrase", phrase: r.txt(1) };
    case 0xb8: return { t: "PersonBackup", code: r.txt(1) };
    case 0xac: return alert(r);
    case 0xb0: return { t: "HistoryItem", id: r.u64(1), ts: r.u32(9), outgoing: (r.u8(13) & 1) !== 0, live: (r.u8(13) & 2) !== 0, kind: r.u8(14), st: r.u8(15), hops: r.u8(16), msgId: r.u64(17), conv: r.addr(25), from: r.addr(41), content: sub(57) };
    case 0xb1: return { t: "HistoryEnd", req: r.u16(1), conv: r.addr(3), count: r.u8(19), more: r.u8(20) !== 0, oldest: r.u64(21) };
    case 0xb2: return { t: "ReadMark", conv: r.addr(1), upTo: r.u64(17), unread: r.u16(25) };
    case 0xb3: return { t: "HistoryInfo", days: r.u16(2), count: r.u32(4) };
    case 0xb9: return { t: "Deleted", conv: r.addr(1), flags: r.u8(17), unix: r.u32(18), content: sub(22) };
    default: return { t: "Unknown", kind: t };
  }
}

/** App data fields as in maarse-core::appdata (0x03 position, 0x04 telemetry, 0x05 name, 0x07 caps). */
function nodeInfo(addr, hops, b) {
  const ev = { t: "NodeInfo", addr, hops, name: null, mv: null, usb: null, lat: null, lon: null, caps: null };
  let i = 0;
  while (i + 1 < b.length) {
    const t = b[i], n = b[i + 1];
    const d = new Rd(b.subarray(i + 2, Math.min(i + 2 + n, b.length)));
    if (t === 0x03 && n >= 9) { ev.lat = d.i32(1) / 1e7; ev.lon = d.i32(5) / 1e7; }
    if (t === 0x04 && n >= 10) { ev.mv = d.u16(0); ev.usb = (d.u8(2) & 1) !== 0; }
    if (t === 0x05 && n > 0) ev.name = d.txt(0).replace(/[\x00-\x1f\x7f]/g, "").trim();
    if (t === 0x07 && n >= 1) ev.caps = d.u8(0);
    i += 2 + n;
  }
  return ev;
}

function alert(r) {
  let o = 36, area = null;
  if (r.u8(35) === 1) { area = [r.i32(36) / 1e5, r.i32(40) / 1e5, r.u16(44) / 10]; o = 46; }
  const n = r.u8(o + 2);
  const a = { t: "Alert", issuer: r.addr(1), serial: r.u32(17), issued: r.u32(21), status: r.u8(25), kind: r.u8(26), event: r.u8(27), severity: r.u8(28),
    expires: r.u16(29), reference: r.u32(31), area, hops: r.u8(o), trust: r.u8(o + 1), name: r.txt(o + 3, Math.min(o + 3 + n, r.f.length)), text: r.txt(Math.min(o + 3 + n, r.f.length)) };
  a.key = a.issuer + ":" + a.serial;
  a.endsMs = (a.issued + Math.max(1, a.expires) * 60) * 1000;
  return a;
}

/** Splits a byte stream into frames. */
class FrameDecoder {
  constructor() { this.buf = new Uint8Array(0); }
  push(bytes) {
    const b = new Uint8Array(this.buf.length + bytes.length);
    b.set(this.buf); b.set(bytes, this.buf.length);
    this.buf = b;
    const out = [];
    while (this.buf.length >= 2) {
      const len = this.buf[0] | (this.buf[1] << 8);
      if (len === 0 || len > MAX_FRAME) { this.buf = this.buf.slice(1); continue; }
      if (this.buf.length < 2 + len) break;
      out.push(this.buf.slice(2, 2 + len));
      this.buf = this.buf.slice(2 + len);
    }
    return out;
  }
}

// ---------- names for what the node reports ----------

const Names = {
  ROLES: ["Client", "Repeater", "Relay", "Anchor", "Auto"],
  ALERT_EVENTS: ["Information", "Evacuate", "Take shelter", "Fire", "Flood", "Drinking water", "Chemical release", "Power outage", "Severe weather", "Missing person"],
  ALERT_SEVERITY: ["Minor", "Moderate", "Severe", "Extreme"],
  ALERT_STATUS: ["", "Exercise", "Test", "System"],
  ENERGY: ["Critical", "Low", "Ok", "Full"],
};

const SK = (group, label, unit = "") => ({ group, label, unit });
const STATS = {
  1: SK("General", "Uptime", "time"), 7: SK("General", "Role", "role"), 2: SK("General", "Known nodes"), 3: SK("General", "Known routes"),
  4: SK("General", "Channels"), 5: SK("General", "Links (TCP/serial)"), 6: SK("General", "Connected clients"),
  45: SK("Radio", "Frequency", "MHz"), 47: SK("Radio", "Bandwidth", "kHz"), 46: SK("Radio", "Spreading factor", "SF"),
  48: SK("Radio", "Transmit power", "dBm"), 34: SK("Radio", "Latest RSSI", "dBm"), 35: SK("Radio", "Latest SNR", "dB"),
  36: SK("Radio", "Average RSSI", "dBm"), 37: SK("Radio", "Average SNR", "dB"), 50: SK("Radio", "Weakest RSSI", "dBm"),
  51: SK("Radio", "Strongest RSSI", "dBm"), 30: SK("Radio", "Packets received"), 32: SK("Radio", "Packets sent"),
  42: SK("Radio", "Data received", "bytes"), 43: SK("Radio", "Data sent", "bytes"), 40: SK("Radio", "Total airtime", "ms"),
  41: SK("Radio", "Duty cycle used (last hour)", "permille"), 44: SK("Radio", "Transmit queue"),
  38: SK("Radio", "Channel busy (CAD)"), 39: SK("Radio", "Deferred due to duty cycle"),
  31: SK("Radio", "Receive errors (CRC/header)"), 33: SK("Radio", "Transmit errors"), 52: SK("Radio", "Packets dropped"),
  53: SK("Radio", "Radio driver stage"), 54: SK("Radio", "Radio loop restarts"),
  49: SK("Power and diagnostics", "Battery", "mV"), 85: SK("Power and diagnostics", "Battery (approx.)", "%"),
  86: SK("Power and diagnostics", "Chip temperature", "temp"), 80: SK("Power and diagnostics", "Noise floor (estimated)", "dBm"),
  81: SK("Power and diagnostics", "Energy status", "energy"), 82: SK("Power and diagnostics", "USB power", "usb"),
  83: SK("Power and diagnostics", "Direct neighbours"), 84: SK("Power and diagnostics", "Sharing position", "precision"),
  87: SK("Power and diagnostics", "Free stack", "bytes"), 88: SK("Power and diagnostics", "Free heap", "bytes"),
  89: SK("Power and diagnostics", "Lowest free heap", "bytes"), 90: SK("Power and diagnostics", "Crashes"),
  91: SK("Power and diagnostics", "Last panic line"), 92: SK("Power and diagnostics", "Last panic file (hash)"),
  10: SK("Routing", "Packets received"), 11: SK("Routing", "Invalid packets"), 12: SK("Routing", "Duplicates"),
  13: SK("Routing", "Forwarded for others"), 14: SK("Routing", "Originated"), 15: SK("Routing", "Delivered to me"),
  16: SK("Routing", "Acknowledgements received"), 17: SK("Routing", "Routes found"), 18: SK("Routing", "Route requests"),
  19: SK("Routing", "Hops retransmitted"), 20: SK("Routing", "Floods cancelled"), 77: SK("Routing", "Without proof of work"),
  93: SK("Routing", "Hellos sent"), 94: SK("Routing", "Key requests"), 95: SK("Routing", "Local route repairs"), 99: SK("Routing", "Dropped by quota"),
  96: SK("Carry and contacts", "Messages carried for others"), 98: SK("Carry and contacts", "Mobile mode", "bool"),
  97: SK("Carry and contacts", "Pinned contacts"), 100: SK("Carry and contacts", "Waiting for a key"), 101: SK("Carry and contacts", "Outbox"),
  60: SK("Mailbox and files", "Recipients with mail"), 61: SK("Mailbox and files", "Messages in the mailbox"),
  64: SK("Mailbox and files", "Messages without acknowledgement"), 69: SK("Mailbox and files", "Put in others' mailbox"),
  70: SK("Mailbox and files", "Delivered from the mailbox"), 62: SK("Mailbox and files", "Files being sent"), 63: SK("Mailbox and files", "Files being received"),
  67: SK("Network", "Packets in"), 68: SK("Network", "Packets out"), 65: SK("Network", "Data in", "bytes"),
  66: SK("Network", "Data out", "bytes"), 71: SK("Network", "Errors"), 102: SK("Network", "Wi-Fi modem fell back to a node", "bool"),
  72: SK("DHT", "Contacts"), 73: SK("DHT", "Records stored"), 74: SK("DHT", "Lookups"), 75: SK("DHT", "Found"), 76: SK("DHT", "Direct gateways"),
};
const STAT_GROUPS = ["General", "Carry and contacts", "Mailbox and files", "Routing", "Radio", "Power and diagnostics", "Network", "DHT"];
const PRECISION = ["Off", "About 10 km", "About 1 km", "Precise"];

function formatStat(k, v) {
  const unit = STATS[k]?.unit ?? "";
  switch (unit) {
    case "time": { const h = Math.floor(v / 3600), m = Math.floor(v / 60) % 60; return h >= 24 ? `${Math.floor(h / 24)} d ${h % 24} h` : h > 0 ? `${h} h ${m} min` : `${m} min ${v % 60} s`; }
    case "role": return Names.ROLES[v] ?? `${v}`;
    case "energy": return Names.ENERGY[v] ?? `${v}`;
    case "bool": return v !== 0 ? "yes" : "no";
    case "usb": return v < 0 ? "unknown" : v !== 0 ? "yes" : "no";
    case "MHz": return (v / 1000).toFixed(3) + " MHz";
    case "SF": return "SF" + v;
    case "ms": return v < 10000 ? `${v} ms` : (v / 1000).toFixed(1) + " s";
    case "permille": return (v / 10).toFixed(1) + " %";
    case "temp": return (v / 10).toFixed(1) + " °C";
    case "%": return v + " %";
    case "precision": return PRECISION[v] ?? `${v}`;
    case "bytes": return v < 10240 ? `${v} B` : v < 10485760 ? (v / 1024).toFixed(1) + " KB" : (v / 1048576).toFixed(1) + " MB";
    case "": return `${v}`;
    default: return `${v} ${unit}`;
  }
}
