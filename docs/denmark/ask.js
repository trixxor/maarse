// "Ask Claude" for the Denmark simulator. The viewer asks a question; Claude
// plans a test and runs it here through page tools: experiments in the
// WebAssembly engine (the real maarse-core Router), live scenarios on the map,
// a followed message, timeline events.
//
// Two ways to reach Claude:
// * locally, through tools/sim-bridge/bridge.mjs, which serves this page and
//   runs Claude Code (`claude -p`) with these tools as an MCP server (preferred);
// * as a claude.ai artifact with the `sample` capability.
// Elsewhere (e.g. python http.server) the tab stays hidden.
(function () {
  "use strict";
  const $ = (id) => document.getElementById(id);
  const esc = (s) => String(s).replace(/[&<>"]/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));
  const S = window.DkSim;
  if (!S) return;
  const DK = S.DK;

  let sample = null, ctl = null;
  let turns = [];
  const MAX_TURNS = 10;
  const BUDGET_MS = 110000; // a tool call is aborted after 150 s

  const INSTRUCTIONS = `You are the analyst inside the Må:rse Denmark Simulator. Må:rse is a LoRa mesh messaging protocol; the simulator runs the real maarse-core Router (Rust, compiled to WebAssembly) in every node on a simplified radio model of Denmark. The viewer asks questions about the network; answer them by running tests with the tools, then explain the result with numbers. Reply in the language the viewer writes in.

How Må:rse works (what the numbers mean):
- A node without a route floods a PATH REQUEST that carries the first message; every forwarding repeater adds itself to the trail (a source path holds at most 12 repeaters; anchors split longer routes). The destination answers with a PATH RESPONSE along the reverse route, carrying the acknowledgement. Later messages follow the learned route (DATA, hop-by-hop acks, local repair of a dead hop, end-to-end ack, fallback to a new path request).
- Hosts (gateways) with internet are linked over TCP; maarse-host sends every packet, path requests included, to all its TCP peers, so one message to a new destination can flood the whole country.
- Channel messages flood up to 8 hops; signed bulletins (alerts) flood with high priority. Duty cycle 10 % per radio, listen-before-talk.
- "Collisions" count every lost reception at every node (one transmission is heard by ~25 nodes). Most are redundant copies of floods (flood_copies_collided); only lost_at_next_hop costs a retransmission.
- The simulator's traffic generator picks random destinations (every message needs a new route), so flood numbers are a worst case. The radio model is crude (log-distance, antenna height, horizon, forest loss); it is not a field measurement.

Features (switch them per variant): carry, local_repair (default on), backbone (default on), dht_backbone (path requests only to the host that can reach the destination; experimental), home_routing (model: fixed infrastructure routes, home repeaters, a location lookup; not a real protocol yet), regions, sf_slot, scale_timeouts, load_control, area_scope.
Presets: normal (national day, ~800 nodes), internet_loss (all TCP down at 2 min), bornholm (Bornholm cut off at 1 min, water alert at 3 min), storm (west coast power cut at 2 min, flood alert), e45 (car on the E45, repeaters every 15 km), forest (walkers in Rold Skov), festival (Roskilde, 400 handhelds), spam (3 spammers in Odense from 2 min).
Timeline events (objects with t_s and type): internet_down {region?}, internet_up {region?}, power_cut {area?}, power_restore {area?}, kill_random {n, node_kind}, kill {nodes:[index]}, revive {nodes?}, kill_area {area, node_kind: any|repeater|anchor|host|handheld}, host_internet {node, up}, degrade {area, db} (extra radio loss), degrade_clear, link {index, up}, alert {from, alert: evacuation|fire|flood|water|info, area, text}, set_traffic {traffic}. An area is {"circle":[lat, lon, radius_m]} or {"polygon":[[lat,lon],...]}. Regions: nordjylland, midtjylland, syddanmark, sjaelland, hovedstaden.

How to work:
1. If you need the current state (nodes, features, towns, live numbers), call describe_setup.
2. For a question about cause and effect, call run_experiment with 2-4 variants that differ only in what the question is about, seeds [1,2,3] when the difference may be small, and 300-600 s. Keep the total work small: variants x seeds x duration.
3. Let the viewer watch: call show_live with the most telling variant, and follow_message for a question about a single message or route (towns by name, e.g. "Skagen", "Roenne").
4. Answer: what you tested (one line), the key numbers in a small table, what they mean, and the main limitation. Short and concrete. Do not invent numbers you did not measure.`;

  // ---------- tools ----------
  function towns(n) { return window.DK_TOWNS.slice(0, n).map(t => [t[0], t[1], t[2], t[4]]); }
  function kindCounts(sc) { const c = {}; sc.nodes.forEach(n => c[n.kind] = (c[n.kind] || 0) + 1); return c; }
  function buildScenario(preset, seed) {
    if (preset && DK.PRESETS[preset]) return DK.PRESETS[preset].build(window.DK_LAND, window.DK_TOWNS, seed || 1);
    const sc = JSON.parse(JSON.stringify(S.scenario()));
    if (seed) sc.seed = seed;
    return sc;
  }
  function applyChanges(sc, ch) {
    if (ch.features && typeof ch.features === "object") Object.assign(sc.features, ch.features);
    if (ch.traffic && typeof ch.traffic === "object") Object.assign(sc.traffic, ch.traffic);
    if (Array.isArray(ch.events)) sc.events = sc.events.concat(ch.events.filter(e => e && typeof e === "object" && e.type));
    return sc;
  }
  function metrics(s) {
    const r1 = (v) => Math.round(v * 10) / 10;
    return {
      messages: s.messages, delivered_pct: r1(s.delivered_pct), acked_pct: r1(s.acked_pct),
      latency_median_s: r1(s.latency_median_ms / 1000), latency_p90_s: r1(s.latency_p90_ms / 1000),
      transmissions: s.tx, path_requests: s.by_type.PathRequest || 0, airtime_s: Math.round(s.airtime_s),
      busiest_node_airtime_s: r1(s.busiest_airtime_s), dropped: s.duty_drops + s.busy_drops,
      lost_at_next_hop: s.coll_next_hop, flood_copies_collided: s.coll_flood, nodes_alive: s.alive,
      channel_reach_pct: r1(s.channel_reach_pct),
      alerts: s.alerts.map(a => ({ kind: a.kind, coverage_pct: Math.round(a.coverage_pct), median_s: r1(a.latency_median_ms / 1000) })),
      home_lookups: s.home_lookups, home_routed: s.home_routed, home_fallback: s.home_fallback,
    };
  }
  const tick = () => new Promise(r => setTimeout(r, 0));

  const TOOLS = [
    {
      name: "describe_setup",
      description: "Returns the scenario on the map now: node counts by kind, features, traffic, timeline events, simulated time, live results, the presets, and the 40 largest towns with coordinates and region.",
      execute() {
        const sc = S.scenario(), s = S.summary();
        progress("Reading the current setup");
        return {
          preset: S.presetKey() || "custom", name: sc.name, nodes: kindCounts(sc), backbone_links: sc.links.length,
          features: sc.features, traffic: sc.traffic, events: sc.events.map(e => `${e.t_s} s: ${S.describe(e)}`),
          sim_time_s: s ? Math.round(s.t / 1000) : 0, live: s ? metrics(s) : null,
          presets: Object.fromEntries(Object.entries(DK.PRESETS).map(([k, p]) => [k, p.title])),
          towns: towns(40),
        };
      },
    },
    {
      name: "run_experiment",
      description: "Runs variants of a scenario headless in the engine (no animation) and returns the mean results per variant: delivered/acked %, latency, transmissions, path requests, airtime, drops, losses at the next hop, flood copies collided, alert coverage. Use it for every cause-and-effect question.",
      inputSchema: {
        type: "object",
        properties: {
          preset: { type: "string", description: "Preset key; omit to use the scenario on the map now." },
          seeds: { type: "array", items: { type: "integer" }, description: "1-3 seeds, e.g. [1,2,3]. Default [1]." },
          duration_s: { type: "number", description: "Simulated seconds, 60-900. Default 600." },
          traffic: { type: "object", description: "Overrides for all variants, e.g. {direct_per_min: 30}." },
          variants: {
            type: "array", description: "1-4 variants.",
            items: { type: "object", properties: { name: { type: "string" }, features: { type: "object" }, traffic: { type: "object" }, events: { type: "array", items: { type: "object" } } }, required: ["name"] },
          },
        },
        required: ["variants"],
      },
      async execute(input, ctx) {
        const variants = (Array.isArray(input.variants) ? input.variants : []).slice(0, 4);
        if (!variants.length) throw new Error("Give at least one variant.");
        const seeds = (Array.isArray(input.seeds) && input.seeds.length ? input.seeds : [1]).slice(0, 3).map(Number);
        const dur = Math.min(900, Math.max(60, Number(input.duration_s) || 600));
        const Engine = S.engine();
        const t0 = performance.now();
        const rows = [];
        let stopped = false;
        for (const v of variants) {
          const acc = [];
          for (const seed of seeds) {
            if (ctx.signal.aborted) throw new Error("Stopped by the viewer.");
            if (performance.now() - t0 > BUDGET_MS) { stopped = true; break; }
            const sc = applyChanges(applyChanges(buildScenario(input.preset, seed), { traffic: input.traffic }), v);
            sc.trace = false;
            const sim = new Engine.Sim(sc);
            try {
              for (let t = 0; t < dur * 1000; t += 15000) {
                sim.step(Math.min(15000, dur * 1000 - t), 5e7);
                progress(`Experiment: ${v.name}, seed ${seed}, ${Math.round(Math.min(dur, (t + 15000) / 1000))} / ${dur} s`);
                await tick();
                if (ctx.signal.aborted) throw new Error("Stopped by the viewer.");
              }
              acc.push(metrics(sim.summary()));
            } finally { sim.free(); }
          }
          if (!acc.length) break;
          const mean = {};
          for (const k of Object.keys(acc[0])) {
            mean[k] = typeof acc[0][k] === "number" ? Math.round(acc.reduce((a, r) => a + r[k], 0) / acc.length * 10) / 10 : acc[acc.length - 1][k];
          }
          rows.push(Object.assign({ variant: v.name, runs: acc.length }, mean));
          if (stopped) break;
        }
        return { preset: input.preset || S.presetKey() || "current", seeds, duration_s: dur, traffic: input.traffic || null, rows, stopped_early: stopped ? "time budget reached; fewer runs than asked" : false };
      },
    },
    {
      name: "show_live",
      description: "Loads a scenario into the live map and starts it, so the viewer can watch packets, the feed and the timeline. Returns node counts. Use it after an experiment with the most telling variant.",
      inputSchema: {
        type: "object",
        properties: {
          preset: { type: "string", description: "Preset key; omit to keep the current scenario." },
          seed: { type: "integer" },
          features: { type: "object" }, traffic: { type: "object" },
          events: { type: "array", items: { type: "object" } },
          speed: { type: "integer", description: "1, 10, 60, 200 or 1000 (simulated seconds per real second). Default 60." },
        },
      },
      execute(input) {
        const sc = applyChanges(buildScenario(input.preset, Number(input.seed) || 1), input);
        sc.trace = true;
        progress(`Loading ${input.preset || "the current scenario"} on the map`);
        S.load(sc, input.preset && DK.PRESETS[input.preset] ? input.preset : "");
        S.play(true, [1, 10, 60, 200, 1000].includes(Number(input.speed)) ? Number(input.speed) : 60);
        return { loaded: sc.name, nodes: kindCounts(sc), features: sc.features, events: sc.events.map(e => `${e.t_s} s: ${S.describe(e)}`) };
      },
    },
    {
      name: "follow_message",
      description: "Sends one message in the live simulation (towns by name, the nearest handheld there, or node indices as \"#123\"), runs until it is acknowledged or max_s pass, shows it on the map, and returns its story: lookup, path request spread, route, delivery and ack times, trouble on the way, and a diagnosis if it did not arrive.",
      inputSchema: {
        type: "object",
        properties: {
          from: { type: "string" }, to: { type: "string" },
          quiet: { type: "boolean", description: "Stop other traffic first, so only this message is on the air." },
          max_s: { type: "number", description: "Simulated seconds to wait, default 120." },
        },
        required: ["from", "to"],
      },
      async execute(input, ctx) {
        const sim = S.sim();
        const a = S.resolveEnd(String(input.from)), b = S.resolveEnd(String(input.to));
        if (a < 0 || b < 0 || a === b) throw new Error("Could not find two different nodes for those names.");
        if (input.quiet) sim.inject({ type: "set_traffic", traffic: Object.assign({}, S.scenario().traffic, { direct_per_min: 0, channel_per_min: 0, spam_per_min: 0 }) });
        const m = sim.send(a, b).msg;
        S.showMessageOnMap(m, a, b);
        const max = Math.min(600, Math.max(10, Number(input.max_s) || 120)) * 1000;
        for (let t = 0; t < max; t += 2000) {
          S.advance(2000);
          progress(`Following ${S.name(a)} → ${S.name(b)}: ${Math.round((t + 2000) / 1000)} s`);
          await new Promise(r => setTimeout(r, 40));
          if (ctx.signal.aborted) break;
          const info = sim.follow(m).info;
          if (info.acked_ms !== null) break;
        }
        S.follow(m);
        const st = S.story(m);
        const i = st.info;
        return {
          message: m, from: S.name(a), to: S.name(b),
          delivered_after_s: i.delivered_ms !== null ? Math.round((i.delivered_ms - i.sent_ms) / 100) / 10 : null,
          acked_after_s: i.acked_ms !== null ? Math.round((i.acked_ms - i.sent_ms) / 100) / 10 : null,
          route: st.route, steps: st.steps.slice(0, 12), diagnosis: st.diagnosis,
        };
      },
    },
    {
      name: "add_event",
      description: "Applies a timeline event to the live simulation now (internet down, power cut, kill nodes, radio loss, alert ...), optionally undone after duration_min. Returns what was done.",
      inputSchema: {
        type: "object",
        properties: { event: { type: "object", description: "A timeline event without t_s, e.g. {type: \"internet_down\", region: \"hovedstaden\"}." }, duration_min: { type: "number" } },
        required: ["event"],
      },
      execute(input) {
        const ev = Object.assign({}, input.event || {});
        if (!ev.type) throw new Error("The event needs a type.");
        delete ev.t_s;
        const undo = {
          internet_down: { type: "internet_up", region: ev.region || "" }, power_cut: { type: "power_restore", area: ev.area },
          degrade: { type: "degrade_clear" }, kill: { type: "revive", nodes: ev.nodes }, kill_area: { type: "revive" },
          host_internet: ev.up === false ? { type: "host_internet", node: ev.node, up: true } : null, link: ev.up === false ? { type: "link", index: ev.index, up: true } : null,
        }[ev.type] || null;
        const durEl = $("ev-dur"), atEl = $("ev-at"), oldDur = durEl.value, oldAt = atEl.value;
        durEl.value = input.duration_min ? String(Number(input.duration_min)) : ""; atEl.value = "";
        try { S.addEvent(ev, undo); } finally { durEl.value = oldDur; atEl.value = oldAt; }
        progress(`Event: ${S.describe(ev)}`);
        return { done: S.describe(ev), undone_after_min: input.duration_min || null };
      },
    },
  ];

  // ---------- UI ----------
  const log = () => $("ask-log");
  let progEl = null;
  function progress(text) { if (progEl) progEl.textContent = text; }
  function addBubble(cls, html) {
    const d = document.createElement("div");
    d.className = "ask-msg " + cls;
    d.innerHTML = html;
    log().appendChild(d);
    log().scrollTop = log().scrollHeight;
    return d;
  }
  // A small Markdown subset: headings, bold, code, lists, tables, paragraphs.
  function md(text) {
    const lines = esc(text).split("\n");
    const out = [];
    let list = null, table = null;
    const inline = (s) => s.replace(/\*\*(.+?)\*\*/g, "<b>$1</b>").replace(/`([^`]+)`/g, "<code>$1</code>");
    const flush = () => {
      if (list) { out.push(`<ul>${list.map(x => `<li>${inline(x)}</li>`).join("")}</ul>`); list = null; }
      if (table) {
        const rows = table.filter(r => !/^\s*\|?\s*:?-{2,}/.test(r)).map(r => r.replace(/^\s*\|/, "").replace(/\|\s*$/, "").split("|").map(c => inline(c.trim())));
        out.push(`<div class="ask-table"><table>${rows.map((r, i) => `<tr>${r.map(c => i ? `<td>${c}</td>` : `<th>${c}</th>`).join("")}</tr>`).join("")}</table></div>`);
        table = null;
      }
    };
    for (const l of lines) {
      if (/^\s*\|/.test(l)) { if (list) flush(); (table ||= []).push(l); continue; }
      if (/^\s*[-*] /.test(l)) { if (table) flush(); (list ||= []).push(l.replace(/^\s*[-*] /, "")); continue; }
      flush();
      if (/^#{1,4} /.test(l)) out.push(`<h4>${inline(l.replace(/^#+ /, ""))}</h4>`);
      else if (l.trim()) out.push(`<p>${inline(l)}</p>`);
    }
    flush();
    return out.join("");
  }
  const ERR = {
    not_granted: "Claude is not allowed on this page. Allow it in the artifact's permissions to ask questions.",
    sampling_disabled: "Claude is not available for this account.",
    rate_limited: "Too many questions right now. Try again in a little while.",
    session_expired: "Sign in to claude.ai again, then ask again.",
    refused: "Claude declined this question. Try asking it differently.",
    prompt_too_large: "The conversation got too long. Start a new conversation.",
    tools_unavailable: "This view cannot run the simulator tools, so Claude cannot run tests here.",
  };

  // ---------- local bridge ----------
  let local = null; // { token, es }
  let fresh = true;
  let current = null; // { ans, text, resolve }
  const api = (path, body) => fetch(path, { method: body ? "POST" : "GET", headers: { "content-type": "application/json", "x-sim-token": local.token }, body: body ? JSON.stringify(body) : undefined }).then(r => r.json());
  const toolDefs = () => TOOLS.map(t => ({ name: t.name, description: t.description, inputSchema: t.inputSchema || { type: "object", properties: {} } }));
  async function runPageCall(m) {
    const tool = TOOLS.find(t => t.name === m.tool);
    const ac = new AbortController();
    if (current) current.toolAbort = ac;
    try {
      if (!tool) throw new Error("unknown tool " + m.tool);
      if (progEl) progEl.textContent = `Running ${m.tool}…`;
      const result = await tool.execute(m.input || {}, { signal: ac.signal });
      await api("/api/page-result", { id: m.id, ok: true, result });
    } catch (e) {
      await api("/api/page-result", { id: m.id, ok: false, error: String(e && e.message || e) });
    }
  }
  function onBridge(ev) {
    let m; try { m = JSON.parse(ev.data); } catch { return; }
    if (m.type === "page_call") return runPageCall(m);
    if (!current) return;
    if (m.type === "text") { current.text += m.delta; current.ans.innerHTML = md(current.text); log().scrollTop = log().scrollHeight; }
    if (m.type === "tool" && progEl) progEl.textContent = `Claude calls ${m.name}…`;
    if (m.type === "usage" && m.info && m.info.utilization > 0.8) $("ask-note").textContent = `Your Claude usage (${m.info.type}) is at ${Math.round(m.info.utilization * 100)} %.`;
    if (m.type === "done") current.resolve(m);
  }
  async function askLocal(q) {
    const ans = addBubble("claude", `<p class="mute">Thinking…</p>`);
    progEl = addBubble("progress", "Starting Claude…");
    $("ask-send").disabled = true; $("ask-stop").hidden = false;
    const done = new Promise((resolve) => { current = { ans, text: "", resolve }; });
    try {
      const r = await api("/api/ask", { question: q, instructions: INSTRUCTIONS, fresh });
      if (r.error) throw new Error(r.error);
      fresh = false;
      const end = await done;
      if (!current.text) ans.innerHTML = "";
      if (end.error) ans.insertAdjacentHTML("beforeend", `<p class="bad">${esc(end.error)}</p>`);
    } catch (e) {
      ans.innerHTML = `<p class="bad">${esc(e.message || e)}</p>`;
    } finally {
      current = null;
      if (progEl) { progEl.remove(); progEl = null; }
      $("ask-stop").hidden = true; $("ask-send").disabled = false;
    }
  }
  async function connectLocal() {
    const ping = await fetch("/api/ping").then(r => r.ok ? r.json() : null).catch(() => null);
    if (!ping || !ping.ok) return false;
    const { token } = await fetch("/api/token").then(r => r.json());
    local = { token, es: new EventSource(`/api/events?token=${token}`) };
    local.es.onmessage = onBridge;
    local.es.onopen = () => api("/api/tools", { tools: toolDefs() }).catch(() => {});
    $("ask-hint").textContent = "Ask a question about the network. Claude (Claude Code on this machine, with only the simulator's tools) builds a test, runs it here in the real router code, shows it on the map and explains the result. It uses your own Claude usage.";
    if (!ping.claude) { $("ask-note").textContent = "The claude command was not found on this machine, so questions cannot be answered."; $("ask-send").disabled = true; }
    $("tab-ask-btn").hidden = false;
    S.openTab("ask");
    return true;
  }

  async function ask(q) {
    if (!q.trim()) return;
    if (local) {
      $("ask-q").value = "";
      addBubble("you", md(q));
      return askLocal(q);
    }
    if (!sample) return;
    $("ask-q").value = "";
    addBubble("you", md(q));
    turns.push({ role: "user", content: q });
    if (turns.length > MAX_TURNS) turns = turns.slice(-MAX_TURNS);
    while (turns.length && turns[0].role !== "user") turns.shift();
    const ans = addBubble("claude", `<p class="mute">Thinking…</p>`);
    progEl = addBubble("progress", "");
    ctl = new AbortController();
    $("ask-send").disabled = true; $("ask-stop").hidden = false;
    try {
      const { text, truncated } = await sample([{ role: "user", content: INSTRUCTIONS }, ...turns], {
        tools: TOOLS, signal: ctl.signal, modelTier: "default",
        onText: ({ text }) => { ans.innerHTML = md(text); log().scrollTop = log().scrollHeight; },
      });
      ans.innerHTML = md(text) + (truncated ? `<p class="mute">The answer was cut short. Ask for less at a time.</p>` : "");
      turns.push({ role: "assistant", content: text });
    } catch (e) {
      if (e && e.text) ans.innerHTML = md(e.text);
      else ans.innerHTML = "";
      if (!e || e.code !== "cancelled") ans.insertAdjacentHTML("beforeend", `<p class="bad">${esc(ERR[e && e.code] || "Something went wrong on the way. Ask again.")}</p>`);
      if (e && (e.code === "not_granted" || e.code === "sampling_disabled")) $("ask-send").disabled = true;
      turns.pop();
    } finally {
      if (progEl) { progEl.remove(); progEl = null; }
      $("ask-stop").hidden = true;
      if (sample) $("ask-send").disabled = false;
    }
  }

  $("ask-send").onclick = () => ask($("ask-q").value);
  $("ask-q").addEventListener("keydown", (e) => { if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) ask($("ask-q").value); });
  $("ask-stop").onclick = () => {
    if (ctl) ctl.abort();
    if (local) { api("/api/stop", {}).catch(() => {}); if (current && current.toolAbort) current.toolAbort.abort(); }
  };
  $("ask-new").onclick = () => { $("ask-stop").onclick(); turns = []; fresh = true; log().innerHTML = ""; };
  document.querySelectorAll("#ask-examples button").forEach(b => b.onclick = () => ask(b.textContent));

  function enable(s) {
    sample = s;
    $("tab-ask-btn").hidden = false;
    s.limits().then(l => {
      if (l && !l.tools) { $("ask-note").textContent = ERR.tools_unavailable; $("ask-send").disabled = true; }
    }).catch(() => {});
    S.openTab("ask");
  }
  // Prefer the local bridge; fall back to an artifact's `sample`.
  connectLocal().then((ok) => {
    if (!ok && window.claude && window.claude.use) window.claude.use("sample").then(s => { if (s) enable(s); }).catch(() => {});
  });
})();
