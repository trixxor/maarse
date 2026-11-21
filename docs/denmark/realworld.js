// Real-world mode: the real network on the map, from backbone hosts that
// share their map (maarse-host --public-map, GET /api/public/map).
//
// Each host reports itself (position, site), the nodes it knows that share
// a position (with their site: antenna height, gain, power, owner) and the
// radio links its own radio hears (SNR, RSSI, packets, last heard). Here:
// * the nodes as markers, the measured links as lines coloured by SNR,
// * coverage of the real sites with the simulator's radio model,
// * the model checked against the measured links: the mean difference is
//   offered as a correction for the coverage picture.
// Server addresses are kept in this browser only.
(function () {
  const dk = window.DKSim, C = window.DKCoverage;
  if (!dk || !dk.map) return;
  const map = dk.map;
  const $ = (id) => document.getElementById(id);
  const esc = (t) => String(t ?? "").replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
  const STORE = "maarse.realworld.servers";
  const DEFAULT_COLLECTOR = "https://nj01.dk.eu.maarse.dk";
  const ROLE_H = { Repeater: 15, Anchor: 15, Relay: 2, Client: 1.5 };   // when a node has no site
  let servers = [];
  try { servers = JSON.parse(localStorage.getItem(STORE) || "[]"); } catch { /* storage blocked */ }

  const layer = L.layerGroup();
  const liveLayer = L.layerGroup();
  const motion = matchMedia("(prefers-reduced-motion: reduce)");
  const transport = ["radio", "trunk", "backbone"];
  const colors = { radio: "#fb923c", trunk: "#a3e635", backbone: "#60a5fa" };
  const HISTORY_MS = 60000;
  let data = { nodes: new Map(), links: [], hosts: new Map() }, timer = 0, calib = null;
  let mapRequest = 0, live = null;
  const requestedView = new URLSearchParams(location.search).get("view");
  let initialView = ["packets", "channels"].includes(requestedView) ? requestedView : null;

  const card = document.createElement("div");
  card.id = "rw-card"; card.className = "ov hidden";
  card.style.cssText = "left:54px;top:84px;width:320px;max-height:calc(100% - 160px);overflow:auto;padding:8px 10px;font-size:12px";
  card.innerHTML = `
    <h4 style="margin:0 0 6px;display:flex;justify-content:space-between;align-items:center">Real world <button id="rw-x" style="border:0;background:transparent" title="Close (the map stays until you hide it)">✕</button></h4>
    <p class="hint" style="margin:0 0 6px">The real network, from backbone hosts that share their map (<code>--public-map</code>). Fixed nodes show the site their owner set: antenna height, gain, power, owner.</p>
    <label>Hosts (one per line, e.g. https://host.example.dk:8080)</label>
    <textarea id="rw-servers" rows="3" style="width:100%;box-sizing:border-box;font-size:12px">${esc((servers.length ? servers : [DEFAULT_COLLECTOR]).join("\n"))}</textarea>
    <div style="display:flex;gap:4px;flex-wrap:wrap;margin-top:4px">
      <button id="rw-load" class="primary">Show</button>
      <button id="rw-cov" title="Coverage of the real fixed nodes, with the simulator's radio model">Coverage</button>
      <button id="rw-hide">Hide</button>
    </div>
    <label title="Hosts must opt in with --public-traffic"><input type="checkbox" id="rw-live" checked> Live observed traffic</label>
    <p id="rw-note" class="hint" style="margin:6px 0 2px"></p>
    <div id="rw-cal"></div>
    <div style="margin-top:4px;display:flex;gap:8px;flex-wrap:wrap" class="hint">
      <span><span style="display:inline-block;width:16px;height:3px;background:#16a34a;vertical-align:middle"></span> SNR ≥ 5 dB</span>
      <span><span style="display:inline-block;width:16px;height:3px;background:#eab308;vertical-align:middle"></span> −5…5</span>
      <span><span style="display:inline-block;width:16px;height:3px;background:#dc2626;vertical-align:middle"></span> &lt; −5</span>
    </div>`;
  $("mapwrap").appendChild(card);
  L.DomEvent.disableClickPropagation(card); L.DomEvent.disableScrollPropagation(card);
  const trafficPanel = document.createElement("section");
  trafficPanel.id = "rw-traffic"; trafficPanel.className = "ov hidden";
  trafficPanel.setAttribute("aria-label", "Observed traffic");
  trafficPanel.innerHTML = `
    <div class="rw-traffic-head"><div><b>Real-world activity</b><small id="rw-status"></small></div>
      <button id="rw-settings">Hosts</button><button id="rw-exit">Simulation</button>
    </div>
    <nav id="rw-views" aria-label="Real-world view"><button data-rw-view="activity" aria-pressed="true">Activity</button><button data-rw-view="packets" aria-pressed="false">Packets</button><button data-rw-view="channels" aria-pressed="false">Public channels</button></nav>
    <div id="rw-transports" aria-label="Filter transport">${transport.map(kind =>
      `<button data-transport="${kind}" aria-pressed="true" style="--transport:${colors[kind]}">${kind}</button>`).join("")}</div>
    <div id="rw-view-activity" class="rw-view">
      <details id="rw-host-totals"><summary>Host totals · last 10 s</summary><ul id="rw-traffic-hosts"></ul></details>
      <div class="hint">Solid RX · dashed TX</div>
      <div id="rw-observations" aria-label="Recent observations"></div>
      <div id="rw-selection" class="hidden"><span id="rw-selected-label"></span><button id="rw-replay">Replay pulse</button><button id="rw-unpin" aria-label="Clear selection">✕</button></div>
      <div id="rw-activity" aria-hidden="true"></div>
      <div class="rw-time"><button id="rw-now" aria-pressed="true">Live</button>
        <input id="rw-time" type="range" step="1000" aria-label="Observed traffic history">
        <output id="rw-clock" for="rw-time"></output></div>
    </div>
    <div id="rw-view-packets" class="rw-view hidden">
      <div id="rw-radio-today" class="hint"></div>
      <div id="rw-packets" aria-label="Recent packets"></div>
      <div id="rw-packet-details" class="hidden"><button id="rw-packet-clear">Clear selection</button><ul id="rw-packet-sources"></ul></div>
    </div>
    <div id="rw-view-channels" class="rw-view hidden">
      <label>Channel <select id="rw-channel"><option value="">All shared channels</option></select></label>
      <div id="rw-messages" aria-label="New public channel messages"></div>
    </div>`;
  $("mapwrap").appendChild(trafficPanel);
  L.DomEvent.disableClickPropagation(trafficPanel); L.DomEvent.disableScrollPropagation(trafficPanel);
  $("rw-settings").onclick = () => card.classList.toggle("hidden");
  $("rw-exit").onclick = () => { $("rw-hide").click(); card.classList.add("hidden"); };
  $("rw-x").onclick = () => card.classList.add("hidden");
  $("rw-hide").onclick = () => {
    clearInterval(timer); timer = 0; mapRequest++;
    stopLive(); map.removeLayer(layer); C && C.clear();
    $("rw-note").textContent = "Hidden.";
  };
  $("rw-load").onclick = () => {
    servers = [...new Set($("rw-servers").value.split(/\s+/).map((x) => x.trim().replace(/\/+$/, "")).filter(Boolean))];
    try { localStorage.setItem(STORE, JSON.stringify(servers)); } catch { /* storage blocked */ }
    stopLive();
    data.hosts.clear();
    clearInterval(timer);
    if (!servers.length) {
      timer = 0; mapRequest++; map.removeLayer(layer);
      $("rw-note").textContent = "Add at least one host.";
      return;
    }
    timer = setInterval(refresh, 60000);
    refresh(true);
  };
  $("rw-live").onchange = () => {
    if ($("rw-live").checked && timer && data.hosts.size) startLive();
    else stopLive();
  };
  $("rw-cov").onclick = () => coverage();

  async function refresh(fit = false) {
    const request = ++mapRequest;
    $("rw-note").textContent = "Fetching…";
    const nodes = new Map(), links = [], errors = [], hosts = new Map();
    await Promise.all(servers.map(async (url) => {
      try {
        const r = await fetch(url + "/api/public/map", { cache: "no-store", credentials: "omit", signal: AbortSignal.timeout(4500) });
        if (!r.ok) throw new Error(r.status === 404 ? "does not share its map (start it with --public-map)" : "answered " + r.status);
        const j = await r.json();
        const h = j.host || {};
        hosts.set(url, h);
        for (const n of j.nodes || []) nodes.set(n.addr, Object.assign({}, nodes.get(n.addr), n));
        for (const l of j.links || []) links.push(Object.assign({ from: h.addr, server: url }, l));
      } catch (e) { errors.push(`${url}: ${e.message}`); }
    }));
    if (request !== mapRequest) return;
    for (const h of hosts.values()) {
      if (h.addr && locationOf(h)) nodes.set(h.addr, { ...nodes.get(h.addr), ...h, role: "Host" });
    }
    // Links name the neighbour by address, or only by its path id.
    const byPath = new Map([...nodes.values()].filter((n) => n.path_id != null).map((n) => [n.path_id, n.addr]));
    for (const l of links) l.to = l.addr || byPath.get(l.path_id) || null;
    data = { nodes, links, hosts };
    draw();
    if (fit && $("rw-live").checked) {
      const points = [...hosts.values()].map(locationOf).filter(Boolean);
      const bounds = map.getBounds();
      if (points.length === 1 && !bounds.contains(points[0])) map.panTo(points[0], { animate: false });
      else if (points.length > 1 && points.some(point => !bounds.contains(point))) {
        map.fitBounds(points, { paddingTopLeft: [40, 60], paddingBottomRight: [40, 120], maxZoom: 8, animate: false });
      }
    }
    if ($("rw-live").checked && !live && hosts.size) startLive();
    const shown = links.filter((l) => l.to && nodes.has(l.to) && nodes.has(l.from)).length;
    $("rw-note").textContent = `${nodes.size} nodes with a position, ${shown} of ${links.length} measured links on the map. Updated ${new Date().toLocaleTimeString()}.` + (errors.length ? " Problems: " + errors.join("; ") : "");
    calibrate();
  }

  function locationOf(host) {
    return Number.isFinite(host.lat) && Math.abs(host.lat) <= 90 && Number.isFinite(host.lon) && Math.abs(host.lon) <= 180
      ? [host.lat, host.lon] : null;
  }

  function startLive() {
    if (live) return;
    live = { hosts: new Map(), pulses: new Map(), entries: [], kinds: new Set(transport),
      cursor: null, selected: null, sequence: 0, timer: 0, frame: 0,
      view: "activity", network: new Map(), networkTimer: 0, packet: null };
    if (window.DKShell) window.DKShell.closePanel();
    dk.observedView(true);
    liveLayer.addTo(map);
    $("rw-traffic").classList.remove("hidden");
    $("rw-status").textContent = "Connecting to hosts…";
    card.classList.add("hidden");
    for (const button of $("rw-transports").querySelectorAll("button")) button.setAttribute("aria-pressed", "true");
    showView(live, initialView || "activity");
    initialView = null;
    renderObservations(live);
    pollLive(live);
    pollNetwork(live);
  }

  function stopLive() {
    if (live) {
      clearTimeout(live.timer);
      clearTimeout(live.networkTimer);
      cancelAnimationFrame(live.frame);
      live = null;
    }
    dk.observedView(false);
    liveLayer.clearLayers();
    map.removeLayer(liveLayer);
    $("rw-traffic").classList.add("hidden");
    $("rw-traffic-hosts").replaceChildren();
  }

  const count = (bucket, direction, kind) => {
    const n = bucket[direction]?.[kind];
    return Number.isSafeInteger(n) && n > 0 && n <= 0xffffffff ? n : 0;
  };
  async function pollLive(run) {
    const now = Date.now();
    await Promise.all([...data.hosts].map(async ([url, host]) => {
      const state = run.hosts.get(url) || { text: "Connecting…", cursor: -1, retry: 0 };
      run.hosts.set(url, state);
      if (state.retry > now) return;
      try {
        const reply = await fetch(url + "/api/public/traffic", { cache: "no-store", credentials: "omit", signal: AbortSignal.timeout(3500) });
        if (reply.status === 404) {
          state.text = "Live traffic unavailable on this host";
          state.active = false;
          state.retry = now + 60000;
          return;
        }
        if (!reply.ok) throw new Error("Traffic request failed");
        const packet = await reply.json();
        if (live !== run) return;
        if (packet.version !== 1 || !Number.isSafeInteger(packet.session)
            || !Number.isSafeInteger(packet.second) || !Array.isArray(packet.buckets)) {
          throw new Error("Unsupported traffic response");
        }
        if (state.session !== packet.session) {
          state.session = packet.session;
          state.cursor = packet.second - 11;
        }
        const totals = { rx: 0, tx: 0 };
        const byTransport = [];
        const point = locationOf(host);
        const buckets = packet.buckets.slice(-10).filter(b => b && Number.isSafeInteger(b.second)
          && b.second >= packet.second - 10 && b.second < packet.second);
        for (const kind of transport) {
          let rx = 0, tx = 0;
          for (const b of buckets) {
            const received = count(b, "rx", kind), sent = count(b, "tx", kind);
            rx += received;
            tx += sent;
            if (b.second > state.cursor && (received || sent)) {
              const entry = { id: ++run.sequence, url, name: host.name || url, kind, point, rx: received, tx: sent,
                time: Math.floor(now / 1000) * 1000 - (packet.second - b.second) * 1000 };
              run.entries.push(entry);
              if (run.view === "activity" && run.cursor === null && !run.selected && b.second >= packet.second - 3 && run.kinds.has(kind)) showObservation(run, entry);
            }
          }
          if (rx || tx) byTransport.push(`${kind} ${rx} RX / ${tx} TX`);
          totals.rx += rx;
          totals.tx += tx;
        }
        state.cursor = packet.second - 1;
        state.active = true;
        state.retry = 0;
        state.text = `${totals.rx} RX · ${totals.tx} TX${point ? "" : " · no shared position"}`;
        state.detail = byTransport.join(" · ");
      } catch {
        state.active = false;
        state.text = "Could not reach live traffic. Retrying…";
        state.retry = now + 5000;
      }
    }));
    if (live !== run) return;
    const reporting = [...data.hosts.keys()].filter(url => run.hosts.get(url)?.active).length;
    run.reporting = reporting;
    renderNetworkStatus(run);
    const list = $("rw-traffic-hosts");
    const rows = [...data.hosts].map(([url, host]) => {
      const state = run.hosts.get(url);
      const row = document.createElement("li");
      row.textContent = `${host.name || url} · ${state?.text || "Connecting…"}`;
      if (state?.active && state.detail) {
        const detail = document.createElement("small");
        detail.textContent = state.detail;
        row.append(detail);
      }
      return row;
    });
    list.replaceChildren(...rows);
    run.entries = run.entries.filter(entry => entry.time >= now - HISTORY_MS);
    run.entries.sort((a, b) => b.time - a.time || b.id - a.id);
    if (run.cursor !== null) run.cursor = Math.max(run.cursor, historyStart(run));
    if (run.selected && !run.entries.includes(run.selected)) run.selected = null;
    renderObservations(run);
    run.timer = setTimeout(() => pollLive(run), 1000);
  }

  const wallTime = (time) => new Date(time).toLocaleTimeString();
  function renderNetworkStatus(run) {
    const reporting = run.view === "activity" ? run.reporting || 0
      : [...run.network.values()].filter(state => state.feed).length;
    $("rw-status").textContent = reporting ? `${reporting} ${reporting === 1 ? "host" : "hosts"} reporting` : "Waiting for a live traffic feed";
  }

  function showView(run, view) {
    run.view = view;
    run.packet = null;
    clearPulses(run);
    for (const button of $("rw-views").querySelectorAll("button")) {
      const name = button.dataset.rwView;
      button.setAttribute("aria-pressed", String(name === view));
      $("rw-view-" + name).classList.toggle("hidden", name !== view);
    }
    $("rw-transports").classList.toggle("hidden", view === "channels");
    renderNetworkStatus(run);
    renderNetwork(run);
  }

  $("rw-views").onclick = (event) => {
    const button = event.target.closest("[data-rw-view]");
    if (button && live) showView(live, button.dataset.rwView);
  };

  async function pollNetwork(run) {
    const now = Date.now();
    for (const url of run.network.keys()) {
      if (!data.hosts.has(url)) run.network.delete(url);
    }
    await Promise.all([...data.hosts.keys()].map(async (url) => {
      const state = run.network.get(url) || { retry: 0, seen: new Set(), feed: null };
      run.network.set(url, state);
      if (state.retry > now) return;
      try {
        const reply = await fetch(url + "/api/public/network", {
          cache: "no-store", credentials: "omit", signal: AbortSignal.timeout(3500)
        });
        if (reply.status === 404) {
          state.retry = now + 60000;
          state.feed = null;
          return;
        }
        if (!reply.ok) throw new Error("Network feed unavailable");
        const feed = await reply.json();
        if (live !== run) return;
        if (feed.version !== 1 || ![feed.packets, feed.channels, feed.messages].every(Array.isArray)) {
          throw new Error("Unsupported network feed");
        }
        state.feed = feed;
        state.retry = 0;
        const newIds = new Set();
        for (const packet of feed.packets) {
          if (!packet || !Array.isArray(packet.observations)) continue;
          for (const observation of packet.observations) {
            if (!observation || !transport.includes(observation.transport)) continue;
            const id = packet.id + "/" + observation.id;
            newIds.add(id);
            if (run.view === "packets" && !run.packet && !state.seen.has(id)
                && Math.abs(now - observation.time) < 5000 && run.kinds.has(observation.transport)) {
              showPacketObservation(run, observation);
            }
          }
        }
        state.seen = newIds;
      } catch {
        state.retry = now + 5000;
        state.feed = null;
      }
    }));
    if (live !== run) return;
    renderNetworkStatus(run);
    renderNetwork(run);
    run.networkTimer = setTimeout(() => pollNetwork(run), 2000);
  }

  function packetRows(run) {
    const rows = [];
    for (const [url, state] of run.network) {
      for (const packet of state.feed?.packets || []) {
        if (!packet || typeof packet.id !== "string" || !Array.isArray(packet.observations)) continue;
        const observations = packet.observations.filter(o => o && run.kinds.has(o.transport));
        if (!observations.length) continue;
        rows.push({ key: url + "/" + packet.id, packet, observations, time: Math.max(...observations.map(o => Number(o.time) || 0)) });
      }
    }
    return rows.sort((a, b) => b.time - a.time);
  }

  function showPacketObservation(run, observation) {
    const node = data.nodes.get(observation.observer);
    const point = node && locationOf(node);
    if (!point) return;
    showObservation(run, { url: observation.observer, point, kind: observation.transport,
      rx: observation.direction === "rx", tx: observation.direction === "tx" });
  }

  function renderNetwork(run) {
    if (run.view === "packets") {
      const stats = [...run.network.entries()].filter(([, state]) => state.feed?.radio_today);
      updateHtml("rw-radio-today", stats.map(([url, state]) => {
        const day = state.feed.radio_today;
        const host = stats.length > 1 ? `${esc(data.hosts.get(url)?.name || url)} · ` : "";
        return `<div>${host}UTC today · ${esc(day.unique_packets)}${day.capped ? "+" : ""} unique LoRa packets · ${esc(day.observations)} receptions · ${esc(day.sites)} sites</div>`;
      }).join(""));
      const rows = packetRows(run);
      const selected = rows.find(row => row.key === run.packet);
      if (!selected) run.packet = null;
      const html = rows.slice(0, 32).map(({ key, packet, time }) => `
        <button class="rw-packet" data-packet="${esc(key)}" aria-pressed="${selected?.key === key}">
          <time>${esc(wallTime(time))}</time><b>${esc(packet.kind)}</b>
          <span>${esc(packet.observer_count)} observers · ${esc(packet.count)} obs.</span>
        </button>`).join("");
      updateHtml("rw-packets", html || '<p class="hint">No recent packets from a configured collector.</p>');
      $("rw-packet-details").classList.toggle("hidden", !selected);
      if (selected) {
        updateHtml("rw-packet-sources", selected.observations.map(o => {
          const observer = data.nodes.get(o.observer)?.name || String(o.observer).slice(0, 8);
          return `<li>${esc(observer)} · ${esc(o.transport)} ${o.direction === "tx" ? "TX" : "RX"} · hop ${esc(o.hops)}${Number.isInteger(o.snr) ? ` · ${o.snr} dB SNR` : ""}</li>`;
        }).join(""));
      }
    }
    if (run.view === "channels") renderPublicChannels(run);
  }

  function updateHtml(id, html) {
    const el = $(id);
    if (el.dataset.content !== html) {
      el.innerHTML = html;
      el.dataset.content = html;
    }
  }

  function renderPublicChannels(run) {
    const feeds = [...run.network.values()].map(state => state.feed).filter(Boolean);
    const channels = [...new Set(feeds.flatMap(feed => feed.channels).filter(channel => typeof channel === "string"))].sort();
    const filter = $("rw-channel");
    const channel = channels.includes(filter.value) ? filter.value : "";
    updateHtml("rw-channel", '<option value="">All shared channels</option>' + channels.map(name =>
      `<option value="${esc(name)}">${esc(name)}</option>`).join(""));
    filter.value = channel;
    const messages = feeds.flatMap(feed => feed.messages).filter(m => m && channels.includes(m.channel)
      && typeof m.text === "string" && Number.isFinite(m.time) && (!channel || m.channel === channel));
    messages.sort((a, b) => b.time - a.time);
    updateHtml("rw-messages", messages.slice(0, 100).map(m => `<article>
      <header><b>${esc(m.channel)}</b><span title="Unverified sender">${esc(m.sender)}</span><time>${esc(wallTime(m.time))}</time></header>
      <p>${esc(m.text)}</p>
    </article>`).join("") || `<p class="hint">${channels.length ? "No new messages in this view." : "This server does not share channel messages."}</p>`);
  }

  $("rw-channel").onchange = () => { if (live) renderPublicChannels(live); };
  $("rw-packets").onclick = (event) => {
    const button = event.target.closest("[data-packet]");
    if (!live || !button) return;
    const row = packetRows(live).find(packet => packet.key === button.dataset.packet);
    if (!row) return;
    live.packet = row.key;
    clearPulses(live);
    for (const observation of row.observations) showPacketObservation(live, observation);
    renderNetwork(live);
  };
  $("rw-packet-clear").onclick = () => {
    if (!live) return;
    live.packet = null;
    clearPulses(live);
    renderNetwork(live);
  };

  const historyStart = (run) => run.entries.at(-1)?.time ?? Math.floor(Date.now() / 1000) * 1000;
  function renderObservations(run) {
    const end = Math.floor(Date.now() / 1000) * 1000;
    const start = historyStart(run);
    const time = run.cursor ?? end;
    const range = $("rw-time");
    range.min = start;
    range.max = end;
    range.value = time;
    range.disabled = start >= end;
    range.setAttribute("aria-valuetext", wallTime(time));
    $("rw-now").setAttribute("aria-pressed", String(run.cursor === null && !run.selected));
    $("rw-clock").textContent = wallTime(time);
    drawActivity(run, end, time);
    $("rw-selection").classList.toggle("hidden", !run.selected);
    if (run.selected) {
      const it = run.selected;
      $("rw-selected-label").textContent = `${it.name} · ${it.kind} · ${it.rx} RX / ${it.tx} TX`;
      $("rw-replay").disabled = !it.point;
    }
    const list = $("rw-observations");
    const entries = run.entries.filter(it => it.time <= time && run.kinds.has(it.kind)).slice(0, 26);
    const html = entries.map(it =>
      `<button class="rw-observation" data-observation="${it.id}" aria-pressed="${run.selected === it}" style="--transport:${colors[it.kind]}">
        <time>${esc(wallTime(it.time))}</time><b>${it.kind}</b><span title="${esc(it.name)}">${esc(it.name)}</span>
        <span>${it.rx}↓ ${it.tx}↑</span></button>`).join("") || '<p class="hint">No observations in this view.</p>';
    if (list.dataset.content === html) return;
    const focused = list.contains(document.activeElement) ? document.activeElement.dataset.observation : null;
    list.innerHTML = html;
    list.dataset.content = html;
    if (focused) list.querySelector(`[data-observation="${focused}"]`)?.focus({ preventScroll: true });
  }

  function drawActivity(run, end, time) {
    const start = end - HISTORY_MS;
    const bars = new Array(30).fill(0);
    for (const it of run.entries) {
      if (it.time < start || !run.kinds.has(it.kind)) continue;
      const i = Math.min(bars.length - 1, Math.floor((it.time - start) / 2000));
      bars[i] += it.rx + it.tx;
    }
    const max = Math.max(1, ...bars);
    const cursor = Math.min(bars.length - 1, Math.floor((time - start) / 2000));
    $("rw-activity").innerHTML = bars.map((count, i) =>
      `<i class="${i === cursor ? "current" : ""}" style="--activity:${Math.round(count / max * 100)}%"></i>`).join("");
  }

  function showObservation(run, entry) {
    if (!entry.point) return;
    const key = entry.url + "/" + entry.kind;
    if (entry.rx) pulse(run, key + "/rx", entry.point, colors[entry.kind], 8, null);
    if (entry.tx) pulse(run, key + "/tx", entry.point, colors[entry.kind], 14, "4 4");
  }
  function focusObservation(entry) {
    if (!entry.point) return;
    const bottom = Math.min(trafficPanel.offsetHeight + 50, Math.floor(map.getSize().y * 0.65));
    map.panInside(entry.point, { paddingTopLeft: [22, 30], paddingBottomRight: [24, bottom], animate: false });
  }
  function clearPulses(run) {
    cancelAnimationFrame(run.frame);
    run.frame = 0;
    run.pulses.clear();
    liveLayer.clearLayers();
  }
  $("rw-transports").onclick = (e) => {
    const button = e.target.closest("[data-transport]");
    if (!button || !live) return;
    const kind = button.dataset.transport;
    if (!live.kinds.delete(kind)) live.kinds.add(kind);
    button.setAttribute("aria-pressed", String(live.kinds.has(kind)));
    if (!live.kinds.has(live.selected?.kind)) live.selected = null;
    clearPulses(live);
    renderObservations(live);
    renderNetwork(live);
  };
  $("rw-observations").onclick = (e) => {
    const row = e.target.closest("[data-observation]");
    if (!row || !live) return;
    const entry = live.entries.find(it => it.id === +row.dataset.observation);
    if (!entry) return;
    live.selected = entry;
    clearPulses(live);
    showObservation(live, entry);
    renderObservations(live);
    focusObservation(entry);
  };
  $("rw-replay").onclick = () => {
    if (!live?.selected) return;
    clearPulses(live);
    focusObservation(live.selected);
    showObservation(live, live.selected);
  };
  $("rw-unpin").onclick = () => {
    if (!live) return;
    live.selected = null;
    clearPulses(live);
    renderObservations(live);
  };
  $("rw-time").oninput = (e) => {
    if (!live) return;
    live.cursor = +e.target.value;
    live.selected = null;
    clearPulses(live);
    for (const it of live.entries) {
      if (it.time === live.cursor && live.kinds.has(it.kind)) showObservation(live, it);
    }
    renderObservations(live);
  };
  $("rw-now").onclick = () => {
    if (!live) return;
    live.cursor = null;
    $("rw-unpin").click();
  };

  function pulse(run, key, point, color, radius, dashArray) {
    const previous = run.pulses.get(key);
    if (previous) for (const ring of previous.rings) liveLayer.removeLayer(ring);
    const rings = Array.from({ length: motion.matches ? 1 : 3 }, (_, i) =>
      L.circleMarker(point, { color, radius, dashArray, opacity: i ? 0 : 0.9, weight: 2, fill: false, interactive: false }).addTo(liveLayer));
    run.pulses.set(key, { rings, radius, start: performance.now() });
    if (!run.frame) run.frame = requestAnimationFrame(t => animatePulses(run, t));
  }

  function animatePulses(run, t) {
    for (const [key, entry] of run.pulses) {
      const progress = (t - entry.start) / 1900;
      if (progress >= 1) {
        run.pulses.delete(key);
        for (const ring of entry.rings) liveLayer.removeLayer(ring);
      } else if (!motion.matches) {
        entry.rings.forEach((ring, i) => {
          const phase = progress - i * 0.14;
          ring.setRadius(entry.radius + Math.max(0, phase) * 30);
          ring.setStyle({ opacity: phase < 0 ? 0 : 0.9 * (1 - phase) ** 2 });
        });
      }
    }
    run.frame = run.pulses.size ? requestAnimationFrame(time => animatePulses(run, time)) : 0;
  }

  const snrCol = (s) => s >= 5 ? "#16a34a" : s >= -5 ? "#eab308" : "#dc2626";
  function draw() {
    layer.clearLayers();
    layer.addTo(map);
    for (const l of data.links) {
      const a = data.nodes.get(l.from), b = data.nodes.get(l.to);
      if (!a || !b) continue;
      L.polyline([[a.lat, a.lon], [b.lat, b.lon]], { color: snrCol(l.snr), weight: 3, opacity: 0.85 })
        .bindTooltip(`${esc(a.name || a.addr.slice(0, 8))} ↔ ${esc(b.name || b.addr.slice(0, 8))}<br>SNR ${l.snr} dB · RSSI ${l.rssi} dBm · ${l.packets} packets · ${fmtAge(l.age_s)} ago`, { sticky: true })
        .addTo(layer);
    }
    for (const n of data.nodes.values()) {
      const point = locationOf(n);
      if (!point) continue;
      const fixed = n.height_m != null || n.role === "Host" || n.role === "Repeater" || n.role === "Anchor";
      L.circleMarker(point, { radius: n.role === "Host" ? 5.5 : fixed ? 3.6 : 2.6, color: n.role === "Host" ? "#e0f2fe" : "#bcc8d4", weight: 1.2,
        fillColor: n.role === "Host" ? "#2563eb" : fixed ? "#d66b64" : "#8298ad", fillOpacity: 1 })
        .bindPopup(`<b>${esc(n.name || n.addr.slice(0, 12))}</b><br>${esc(n.role || "node")}${n.mobile ? " · mobile" : ""}<br>` +
          (n.height_m != null ? `Antenna ${n.height_m} m · ${n.gain_dbi ?? 0} dBi · ${n.tx_dbm ?? "?"} dBm<br>` : `No site set (antenna height guessed from the role)<br>`) +
          (n.owner ? `Owner: ${esc(n.owner)}<br>` : "") + (n.last_seen ? `Last heard ${fmtAge(Date.now() / 1000 - n.last_seen)} ago` : ""))
        .addTo(layer);
    }
  }
  function fmtAge(s) { s = Math.max(0, Math.round(s)); return s < 90 ? s + " s" : s < 5400 ? Math.round(s / 60) + " min" : s < 172800 ? Math.round(s / 3600) + " h" : Math.round(s / 86400) + " days"; }
  const siteOf = (n) => ({ lat: n.lat, lon: n.lon, h: n.height_m ?? ROLE_H[n.role] ?? 2, gain: n.gain_dbi ?? (n.height_m != null || n.role === "Host" ? 6 : 0), tx: n.tx_dbm ?? undefined, name: n.name || n.addr.slice(0, 8) });

  function coverage() {
    if (!C) return;
    const fixed = [...data.nodes.values()].filter((n) => !n.mobile && (n.height_m != null || ["Host", "Repeater", "Anchor"].includes(n.role)));
    if (!fixed.length) { $("rw-note").textContent = "No fixed nodes with a position yet: press Show first."; return; }
    C.setSites(fixed.map(siteOf));
  }

  // Model versus measurement: predicted SNR for every measured link whose
  // ends both have a position; the mean difference is the correction.
  async function calibrate() {
    if (!C || !C.predictRx) return;
    const rows = [];
    for (const l of data.links) {
      const a = data.nodes.get(l.from), b = data.nodes.get(l.to);
      if (!a || !b || l.snr == null) continue;
      const rx = await C.predictRx(siteOf(b), siteOf(a));
      rows.push({ a, b, measured: l.snr, predicted: rx - C.noise, km: map.distance([a.lat, a.lon], [b.lat, b.lon]) / 1000 });
    }
    if (!rows.length) { $("rw-cal").innerHTML = ""; calib = null; return; }
    // SNR saturates near the top; strong links say little about the model.
    const use = rows.filter((r) => r.measured < 10);
    const diff = (use.length ? use : rows).map((r) => r.measured - r.predicted);
    calib = diff.reduce((x, y) => x + y, 0) / diff.length;
    $("rw-cal").innerHTML = `<h4 style="margin:6px 0 2px;font-size:12px">Model versus measured</h4><table>${rows.slice(0, 8).map((r) =>
      `<tr><td>${esc(r.a.name || r.a.addr.slice(0, 6))} ↔ ${esc(r.b.name || r.b.addr.slice(0, 6))}</td><td class="n">${r.km.toFixed(1)} km</td><td class="n">${r.measured} / ${r.predicted.toFixed(0)} dB</td></tr>`).join("")}</table>
      <p class="hint">Measured / predicted SNR. On average reality is <b>${calib >= 0 ? "+" : ""}${calib.toFixed(1)} dB</b> from the model.</p>
      <button id="rw-apply">Use ${calib >= 0 ? "+" : ""}${calib.toFixed(1)} dB in the coverage picture</button>`;
    $("rw-apply").onclick = () => { C.setCalibration(calib); $("rw-note").textContent = `Coverage corrected by ${calib.toFixed(1)} dB from ${diff.length} measured links.`; };
  }

  window.DKRealWorld = { show: () => { card.classList.remove("hidden"); if (servers.length && !timer) $("rw-load").click(); } };
  if (initialView) {
    card.classList.remove("hidden");
    $("rw-load").click();
  }
})();
