// Local bridge for the Denmark simulator's "Ask Claude" tab.
//
//   node tools/sim-bridge/bridge.mjs        then open http://127.0.0.1:8765/denmark/
//
// Serves docs/ (the simulator) and runs Claude Code headless (`claude -p`)
// when the page asks a question. Claude gets only the simulator's tools (an
// MCP server, sim-mcp.mjs), no file or shell tools; each tool call is relayed
// to the open simulator page, which runs it in the WebAssembly engine and
// shows it live. It uses the Claude Code login on this machine. Listens on
// 127.0.0.1 only, and every API call needs a per-run token that only a page
// served by this bridge can read.
import http from "node:http";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import crypto from "node:crypto";
import { spawn, execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const DOCS = path.resolve(HERE, "..", "..", "docs");
const PORT = +(process.env.PORT || 8765);
const HOST = "127.0.0.1";
const TOKEN = crypto.randomBytes(16).toString("hex");
const CLAUDE = process.env.CLAUDE_BIN || "claude";
const ORIGINS = new Set([`http://${HOST}:${PORT}`, `http://localhost:${PORT}`]);
// A fixed, empty working directory: no project instructions leak into the
// session, and --resume finds the conversation again.
const WORK = path.join(os.tmpdir(), "maarse-sim-ask");
fs.mkdirSync(WORK, { recursive: true });
const MCP_CFG = path.join(WORK, "mcp.json");
fs.writeFileSync(MCP_CFG, JSON.stringify({
  mcpServers: { "maarse-sim": { command: process.execPath, args: [path.join(HERE, "sim-mcp.mjs")], env: { SIM_BRIDGE: `http://${HOST}:${PORT}`, SIM_TOKEN: TOKEN } } },
}));

let version = "";
try { version = execFileSync(CLAUDE, ["--version"], { encoding: "utf8" }).trim(); } catch { version = ""; }

const clients = new Set();       // SSE responses
let page = null;                 // the page that registered tools (runs tool calls)
let toolDefs = [];
const pending = new Map();       // page call id -> {resolve, timer}
let child = null;
let sessionId = null;

function sse(res, obj) { res.write(`data: ${JSON.stringify(obj)}\n\n`); }
function broadcast(obj) { for (const c of clients) sse(c, obj); }
function json(res, code, obj) { res.writeHead(code, { "content-type": "application/json" }); res.end(JSON.stringify(obj)); }
function body(req) {
  return new Promise((resolve) => {
    let b = "";
    req.on("data", (d) => { b += d; if (b.length > 2e6) req.destroy(); });
    req.on("end", () => { try { resolve(JSON.parse(b || "{}")); } catch { resolve({}); } });
  });
}

const MIME = { ".html": "text/html; charset=utf-8", ".js": "text/javascript; charset=utf-8", ".mjs": "text/javascript; charset=utf-8", ".css": "text/css; charset=utf-8", ".wasm": "application/wasm", ".json": "application/json", ".md": "text/plain; charset=utf-8", ".svg": "image/svg+xml", ".png": "image/png", ".jpg": "image/jpeg", ".ico": "image/x-icon" };
function serveStatic(req, res, url) {
  let p = decodeURIComponent(url.pathname);
  if (p === "/") { res.writeHead(302, { location: "/denmark/" }); return res.end(); }
  if (p.endsWith("/")) p += "index.html";
  const file = path.resolve(DOCS, "." + p);
  if (!file.startsWith(DOCS + path.sep)) { res.writeHead(403); return res.end(); }
  fs.readFile(file, (err, data) => {
    if (err) { res.writeHead(404); return res.end("not found"); }
    res.writeHead(200, { "content-type": MIME[path.extname(file)] || "application/octet-stream", "cache-control": "no-cache" });
    res.end(data);
  });
}

// A question: run `claude -p` with only the simulator tools, stream to the page.
function ask(question, instructions, fresh) {
  if (fresh) sessionId = null;
  const allowed = toolDefs.map(t => `mcp__maarse-sim__${t.name}`);
  const args = ["-p", question, "--output-format", "stream-json", "--verbose", "--include-partial-messages",
    "--tools", "", "--strict-mcp-config", "--mcp-config", MCP_CFG, "--allowedTools", ...allowed];
  if (instructions) args.push("--system-prompt", instructions);
  if (sessionId) args.push("--resume", sessionId);
  child = spawn(CLAUDE, args, { cwd: WORK, env: { ...process.env, MCP_TOOL_TIMEOUT: "180000", MCP_TIMEOUT: "30000" }, windowsHide: true });
  let buf = "", errTail = "", done = false, firstMsg = true;
  broadcast({ type: "start" });
  child.stdout.on("data", (d) => {
    buf += d;
    let k;
    while ((k = buf.indexOf("\n")) >= 0) {
      const line = buf.slice(0, k); buf = buf.slice(k + 1);
      let m; try { m = JSON.parse(line); } catch { continue; }
      if (m.session_id) sessionId = m.session_id;
      if (m.type === "stream_event" && m.event) {
        const e = m.event;
        if (e.type === "message_start") { if (!firstMsg) broadcast({ type: "text", delta: "\n\n" }); firstMsg = false; }
        if (e.type === "content_block_delta" && e.delta && e.delta.type === "text_delta") broadcast({ type: "text", delta: e.delta.text });
      } else if (m.type === "assistant" && m.message && Array.isArray(m.message.content)) {
        for (const c of m.message.content) if (c.type === "tool_use") broadcast({ type: "tool", name: String(c.name).replace(/^mcp__maarse-sim__/, ""), input: c.input });
      } else if (m.type === "rate_limit_event" && m.rate_limit_info) {
        broadcast({ type: "usage", info: { type: m.rate_limit_info.rateLimitType, utilization: m.rate_limit_info.utilization, status: m.rate_limit_info.status } });
      } else if (m.type === "result" || (m.stop_reason !== undefined && m.total_cost_usd !== undefined)) {
        done = true;
        broadcast({ type: "done", error: m.is_error ? (m.result || "error") : null, cost_usd: m.total_cost_usd, turns: m.num_turns });
      }
    }
  });
  child.stderr.on("data", (d) => { errTail = (errTail + d).slice(-2000); });
  child.on("close", (code) => {
    child = null;
    if (!done) broadcast({ type: "done", error: code ? `claude stopped (${code}): ${errTail.trim().split("\n").slice(-3).join(" ")}` : null });
  });
  child.on("error", (e) => { child = null; broadcast({ type: "done", error: `could not start claude: ${e.message}` }); });
}

const server = http.createServer(async (req, res) => {
  const url = new URL(req.url, `http://${HOST}:${PORT}`);
  if (!url.pathname.startsWith("/api/")) return serveStatic(req, res, url);
  const origin = req.headers.origin;
  if (origin && !ORIGINS.has(origin)) return json(res, 403, { error: "wrong origin" });
  if (url.pathname === "/api/ping") return json(res, 200, { ok: true, claude: version });
  if (url.pathname === "/api/token") return json(res, 200, { token: TOKEN });
  const tok = req.headers["x-sim-token"] || url.searchParams.get("token");
  if (tok !== TOKEN) return json(res, 401, { error: "missing token" });

  switch (url.pathname) {
    case "/api/events": {
      res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache", connection: "keep-alive" });
      res.write(": connected\n\n");
      clients.add(res);
      const ka = setInterval(() => res.write(": ka\n\n"), 15000);
      req.on("close", () => { clearInterval(ka); clients.delete(res); if (page === res) page = null; });
      return;
    }
    case "/api/tools":
      if (req.method === "POST") {
        const b = await body(req);
        toolDefs = Array.isArray(b.tools) ? b.tools : [];
        // The newest page with tools runs the calls.
        page = [...clients].pop() || null;
        return json(res, 200, { ok: true, tools: toolDefs.length });
      }
      return json(res, 200, { tools: toolDefs });
    case "/api/ask": {
      const b = await body(req);
      if (child) return json(res, 409, { error: "Claude is still answering the last question." });
      if (!version) return json(res, 500, { error: "The claude command was not found on this machine." });
      if (!toolDefs.length) return json(res, 409, { error: "No simulator page has registered its tools yet." });
      ask(String(b.question || ""), String(b.instructions || ""), !!b.fresh);
      return json(res, 200, { ok: true });
    }
    case "/api/stop":
      if (child) child.kill();
      for (const [, p] of pending) p.resolve({ ok: false, error: "Stopped by the viewer." });
      pending.clear();
      return json(res, 200, { ok: true });
    case "/api/page-call": {
      const b = await body(req);
      const target = page && clients.has(page) ? page : [...clients].pop();
      if (!target) return json(res, 200, { ok: false, error: "The simulator page is not open." });
      const id = crypto.randomBytes(6).toString("hex");
      const result = await new Promise((resolve) => {
        const timer = setTimeout(() => { pending.delete(id); resolve({ ok: false, error: "The simulator page did not answer in time." }); }, 170000);
        pending.set(id, { resolve: (r) => { clearTimeout(timer); resolve(r); }, timer });
        sse(target, { type: "page_call", id, tool: b.tool, input: b.input || {} });
      });
      return json(res, 200, result);
    }
    case "/api/page-result": {
      const b = await body(req);
      const p = pending.get(b.id);
      if (p) { pending.delete(b.id); p.resolve({ ok: !!b.ok, result: b.result, error: b.error }); }
      return json(res, 200, { ok: true });
    }
    default:
      return json(res, 404, { error: "unknown" });
  }
});

server.listen(PORT, HOST, () => {
  console.log(`Må:rse simulator with Ask Claude: http://${HOST}:${PORT}/denmark/`);
  console.log(version ? `Claude Code: ${version} (questions use this machine's Claude login)` : "Warning: the claude command was not found; Ask Claude will not work.");
});
