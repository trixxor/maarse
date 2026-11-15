// MCP server (stdio) that gives Claude the Denmark simulator's tools.
// Started by `claude -p` through the bridge's --mcp-config. It holds no
// simulator itself: the tool list comes from the open simulator page (via
// the bridge), and every call is relayed to that page, which runs it in the
// WebAssembly engine and shows it live. No dependencies.
import { createInterface } from "node:readline";

const BRIDGE = process.env.SIM_BRIDGE || "http://127.0.0.1:8765";
const TOKEN = process.env.SIM_TOKEN || "";
const headers = { "content-type": "application/json", "x-sim-token": TOKEN };

function send(msg) { process.stdout.write(JSON.stringify(msg) + "\n"); }
function reply(id, result) { send({ jsonrpc: "2.0", id, result }); }
function fail(id, code, message) { send({ jsonrpc: "2.0", id, error: { code, message } }); }

async function tools() {
  const r = await fetch(`${BRIDGE}/api/tools`, { headers });
  if (!r.ok) throw new Error(`bridge: ${r.status}`);
  const j = await r.json();
  return (j.tools || []).map(t => ({ name: t.name, description: t.description, inputSchema: t.inputSchema || { type: "object", properties: {} } }));
}

async function call(name, input) {
  const r = await fetch(`${BRIDGE}/api/page-call`, { method: "POST", headers, body: JSON.stringify({ tool: name, input: input || {} }) });
  const j = await r.json().catch(() => ({ ok: false, error: `bridge: ${r.status}` }));
  return j;
}

const rl = createInterface({ input: process.stdin });
rl.on("line", async (line) => {
  let msg;
  try { msg = JSON.parse(line); } catch { return; }
  const { id, method, params } = msg;
  try {
    switch (method) {
      case "initialize":
        return reply(id, { protocolVersion: (params && params.protocolVersion) || "2025-06-18", capabilities: { tools: {} }, serverInfo: { name: "maarse-sim", version: "0.1.0" } });
      case "notifications/initialized":
      case "notifications/cancelled":
        return;
      case "ping":
        return reply(id, {});
      case "tools/list":
        return reply(id, { tools: await tools() });
      case "tools/call": {
        const r = await call(params.name, params.arguments);
        const text = r.ok ? (typeof r.result === "string" ? r.result : JSON.stringify(r.result)) : `Error: ${r.error || "the tool failed"}`;
        return reply(id, { content: [{ type: "text", text }], isError: !r.ok });
      }
      default:
        if (id !== undefined) fail(id, -32601, `unknown method ${method}`);
    }
  } catch (e) {
    if (id !== undefined) fail(id, -32603, String(e && e.message || e));
  }
});
