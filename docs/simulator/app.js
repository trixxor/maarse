/* Må:rse Mesh Lab: the user interface. The model is in engine.js. */
(function () {
"use strict";
const S = window.MeshSim;
const $ = id => document.getElementById(id);
const PIDS = S.PIDS, NAMES = S.NAMES;
let COL = {};
function readColors() {
  const cs = getComputedStyle(document.documentElement);
  for (const v of ["bg", "panel", "panel2", "ink", "muted", "faint", "line", "map", "map2", "road", "water", "tree", "block", "field1", "field2", "mt", "mc", "rn", "ma", "ok", "late", "bad", "accent",
    "c-hops", "c-range", "c-noise", "c-broken", "c-offline", "c-duty"]) COL[v] = cs.getPropertyValue("--" + v).trim();
}
readColors();
matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => { readColors(); redrawAll(); });
const store = {
  get(k, d) { try { const v = localStorage.getItem("meshlab:" + k); return v == null ? d : JSON.parse(v); } catch { return d; } },
  set(k, v) { try { localStorage.setItem("meshlab:" + k, JSON.stringify(v)); } catch {} },
};
const esc = s => String(s).replace(/[&<>"]/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));

// ---------- scenario texts ----------
const UI = {
  motorway: {
    icon: "🛣️", title: "Motorway", tag: "120 km of road, cars at 110 km/h",
    story: `<h2>🛣️ On the motorway</h2>
<p>Repeaters stand on masts every 15 km. There is a town at each end, and a dozen cars drive between them. People text between the cars and the towns.</p>
<p><b>What to watch:</b> the road is longer than a flood can reach. With Meshtastic's default hop limit a message dies halfway. MeshCore and Må:rse send along the repeaters and reach the other end. The cars keep moving, so a route learned a minute ago can break. Må:rse acknowledges every hop and repairs the route at once. With <i>carry</i>, a car takes the message along until it can be delivered.</p>`,
    lessons: [
      { label: "Raise Meshtastic's hop limit to 7", set: { hops: 7 }, note: "Meshtastic now reaches further, but every message is repeated by many more nodes. Watch the airtime in <i>Run the whole scenario</i>. Trips longer than 8 radio hops still fail." },
      { label: "Make the cars drive twice as fast", set: { speed: 2 }, note: "Routes go stale sooner. MeshCore notices only when the end-to-end acknowledgement is missing, then floods again. Må:rse notices the broken hop at once and asks for a new route." },
      { label: "Switch off Må:rse's per-hop acknowledgements", set: { f: { hopAck: false } }, note: "Without them, one lost packet on an 8-hop road means starting over from the sender. Compare the delivery rate with and without." },
      { label: "Give both towns internet", set: { gw: true }, note: "Town-to-town messages skip the road completely. Må:rse looks the recipient up in the DHT and sends straight to the other town's host over TCP, with no airtime cost." },
    ],
  },
  forest: {
    icon: "🌲", title: "Forest hike", tag: "18 hikers, no repeaters",
    story: `<h2>🌲 In the forest, with no repeaters</h2>
<p>Three groups of hikers are spread over 8 × 8 km of forest. The trees cut the range to about 1.5 km, and nobody has put up a repeater. Everyone keeps walking.</p>
<p><b>What to watch:</b> Meshtastic is good here, because every node repeats. MeshCore and Reticulum only forward through repeaters or transport nodes, so you reach only the people you can hear yourself. Må:rse switches handhelds into <b>relay role</b> when they hear no repeater: they forward for up to 4 hops and wait a little first, so a real repeater would win. Messages that still cannot get through are <b>carried</b> by people walking and handed over when they meet the recipient. A short "delivered" notice then stops the other copies.</p>`,
    lessons: [
      { label: "Switch off Må:rse's relay role", set: { f: { relay: false } }, note: "Without forest mode, Må:rse behaves like MeshCore here: no repeaters means nobody forwards." },
      { label: "Switch off carry", set: { f: { carry: false } }, note: "Fewer messages arrive, and none arrive late. Carry trades extra airtime for messages that otherwise would never arrive." },
      { label: "Twice as many hikers", set: { people: 2 }, note: "More people means more stepping stones. Meshtastic and Må:rse's relays get further; MeshCore still reaches only people in direct range." },
      { label: "Raise the spreading factor to SF12", set: { sf: 12 }, note: "Longer range, so the groups hear each other. But every packet now takes 2.3 s on the air: look at the airtime." },
    ],
  },
  city: {
    icon: "🏙️", title: "Dense city", tag: "110 people, 12 rooftop repeaters",
    story: `<h2>🏙️ A dense city</h2>
<p>Down between the buildings a node reaches about a kilometre. Rooftop repeaters see much further. Many people send at the same time.</p>
<p><b>What to watch:</b> when everyone repeats everything, the channel fills up. Meshtastic sends dozens of copies of each message, and they collide with each other and with the next messages. MeshCore and Må:rse flood only the first time (Må:rse only a small path request), then send along a few rooftops. Look at the packet counts in the race and at the <i>battery used by bystanders</i>: in a flood, everyone's handheld pays for everyone else's messages.</p>`,
    lessons: [
      { label: "A noisy city: 40 % interference", set: { noise: .4 }, note: "Single tries start to fail. A flood survives by sheer redundancy and jams the channel doing it. Må:rse resends a lost packet on the hop where it was lost; MeshCore and Reticulum start over from the sender." },
      { label: "Use a 1 % duty-cycle band", set: { duty: .01 }, note: "Each node may transmit only 1 % of the time. Floods burn through every node's budget, and then messages are dropped." },
      { label: "Switch off Må:rse's best-signal path", set: { f: { bestPath: false } }, note: "Må:rse then takes the path of the first copy, like the others. Weak links mean more resends and more airtime." },
      { label: "Twice as many people", set: { people: 2 }, note: "Flood traffic grows with the number of people; path traffic hardly does." },
    ],
  },
  country: {
    icon: "🌾", title: "Countryside", tag: "farms far apart, SF11, high masts",
    story: `<h2>🌾 Out in the countryside</h2>
<p>Farms lie 5–10 km apart and six high repeaters cover the area. SF11 gives the range, but every packet takes more than a second on the air.</p>
<p><b>What to watch:</b> corner to corner is 4–6 hops, more than Meshtastic's default limit. Raising the limit multiplies the copies on an already slow channel. MeshCore and Må:rse go through the repeaters. Må:rse picks the strongest links, so fewer packets are lost on the way. With internet at two farms, Må:rse uses the backbone for long trips and keeps the slow radio for the last few kilometres.</p>`,
    lessons: [
      { label: "Meshtastic hop limit 7", set: { hops: 7 }, note: "More messages get through, at a large cost in airtime on a channel where every packet takes over a second." },
      { label: "Drop to SF9", set: { sf: 9 }, note: "Packets are four times faster, but the range shrinks and more hops are needed." },
      { label: "Internet at two farms", set: { gw: true }, note: "Long trips go over the backbone. Reticulum can use TCP too, but it learns paths through announces and path requests, which also go out over every radio." },
      { label: "Switch off Må:rse's per-hop acknowledgements", set: { f: { hopAck: false } }, note: "Long radio hops lose packets now and then. Without per-hop resends, a lost packet means a new attempt from the start." },
    ],
  },
  mailbox: {
    icon: "📬", title: "Friend offline", tag: "room server vs propagation node vs mailbox",
    story: `<h2>📬 The recipient's node is switched off</h2>
<p>About half the people turn off their node for 15–45 minutes at a random time. One node in the middle is a server: a Store &amp; Forward router for Meshtastic, a room server for MeshCore and an LXMF propagation node for Reticulum.</p>
<ul>
<li><b>Meshtastic</b> keeps messages on the Store &amp; Forward router. The recipient has to ask for the history (assumed here: about once an hour).</li>
<li><b>MeshCore</b>'s room server is a bulletin board: posts wait, but a <i>direct</i> message to someone offline is resent for a while and then given up.</li>
<li><b>Reticulum</b> leaves the message on the propagation node until the recipient's app syncs (assumed here: every 30 minutes).</li>
<li><b>Må:rse</b> keeps it in a <b>mailbox</b> on a host or in a repeater's flash. It is delivered by itself the moment the recipient announces again. There is nothing to log in to and no sync interval. The sender sees "delivered from mailbox".</li>
</ul>`,
    lessons: [
      { label: "Remove the server node", set: { server: false }, note: "Meshtastic and Reticulum lose their store. Må:rse still has the flash mailbox on the repeaters." },
      { label: "Switch off Må:rse's mailbox", set: { f: { mailbox: false } }, note: "Now Må:rse loses the messages to offline people too." },
      { label: "More messages", set: { msgs: 100 }, note: "Look at <i>delivery time</i>: the mailbox delivers within a minute of the recipient's return, the others at the next history check or sync." },
    ],
  },
  festival: {
    icon: "🎪", title: "Festival", tag: "160 people in 1.6 km², crowded channel",
    story: `<h2>🎪 A festival</h2>
<p>160 people with handheld nodes are squeezed onto a festival site, half of them in front of the stage. Four repeaters sit on the light towers. Bodies absorb the signal, and many people text at once.</p>
<p><b>What to watch:</b> in a crowd everyone is within a few hops, so every design can deliver something. The difference is the cost. A flood makes almost every handheld transmit for every message. Their batteries drain and the channel is constantly busy. Only the four towers carry path traffic. Watch <i>battery used by bystanders</i> and what happens with a 1 % duty-cycle band.</p>`,
    lessons: [
      { label: "Twice the crowd", set: { people: 2 }, note: "Flood airtime doubles with the crowd; path routing hardly notices." },
      { label: "Use a 1 % duty-cycle band", set: { duty: .01 }, note: "The flooding nodes run out of transmit time and start dropping messages." },
      { label: "Quiet radio conditions", set: { noise: .05 }, note: "With little interference even single tries work, and the difference becomes mostly about airtime." },
    ],
  },
  blackout: {
    icon: "⛈️", title: "Storm blackout", tag: "repeaters fail mid-way",
    story: `<h2>⛈️ A storm knocks out the power</h2>
<p>A town with a dozen repeaters. A quarter of the way into the hour, a storm takes out the repeaters in the middle of town. The mobile network is down too, so the mesh is all people have.</p>
<p><b>What to watch:</b> a flood does not care which repeaters are left, so it degrades gracefully. Path-based designs have to notice that a route is gone. MeshCore notices when the end-to-end acknowledgement is missing. Må:rse notices at the broken hop and repairs at once. In the race, tick <i>Knock out a repeater on the route</i> to see a repair up close.</p>`,
    lessons: [
      { label: "Knock out 6 repeaters", set: { failures: 6 }, note: "Half the town's repeaters are gone. Who still finds a way round?" },
      { label: "Switch off Må:rse's per-hop acknowledgements", set: { f: { hopAck: false } }, note: "Må:rse then notices broken routes only by a missing end-to-end acknowledgement, like MeshCore." },
      { label: "The two hosts still have internet", set: { gw: true }, note: "A host with a fibre line and a battery backup joins both ends of town over TCP." },
    ],
  },
  oslo: {
    icon: "🌍", title: "Aarhus ↔ Oslo", tag: "three cities joined by the internet",
    story: `<h2>🌍 Aarhus, Copenhagen and Oslo</h2>
<p>Three cities, each with a few repeaters and one host with internet. No radio reaches across the sea, so messages between cities must use the internet.</p>
<ul>
<li><b>Meshtastic</b> can bridge through an MQTT server: every gateway republishes every packet into its own city.</li>
<li><b>MeshCore</b> has no built-in internet link, only third-party bridges, so it stays inside each city.</li>
<li><b>Reticulum</b> connects over TCP. Path requests and announces go out on every interface, so they are flooded into every city.</li>
<li><b>Må:rse</b> asks the DHT which host can reach the recipient and sends only there. The other cities' radios stay quiet.</li>
</ul>`,
    lessons: [
      { label: "Cut the internet", set: { gw: false }, note: "Without internet nobody crosses the sea. LoRa does not reach 500 km." },
      { label: "Switch off Må:rse's backbone", set: { f: { backbone: false } }, note: "Then Må:rse is limited to each city too." },
      { label: "More people in each city", set: { people: 2 }, note: "Watch the airtime: designs that flood every city pay for it in all three." },
    ],
  },
  sandbox: {
    icon: "🧩", title: "Build your own", tag: "place repeaters, people and hosts",
    story: `<h2>🧩 Build your own</h2>
<p>Place repeaters, people and hosts on the map below. Then race a message, or run many. Use <b>Set sender</b> and <b>Set recipient</b> to choose who talks.</p>
<p><b>Ideas:</b> a valley with a single repeater on a hill. Two villages joined only by a chain of handhelds. A host in each village with internet turned on. Then knock out the repeater everything depends on.</p>`,
    lessons: [
      { label: "Turn on internet", set: { gw: true }, note: "Hosts (squares) are joined over TCP." },
      { label: "Make it noisy (30 %)", set: { noise: .3 }, note: "Per-hop resends start to matter." },
      { label: "Use SF12", set: { sf: 12 }, note: "Everything reaches further, and every packet takes longer." },
    ],
  },
};
const ORDER = ["motorway", "forest", "city", "country", "mailbox", "festival", "blackout", "oslo", "sandbox"];

// ---------- state ----------
const state = {
  sc: store.get("sc", "motorway"), view: "play", sub: "race", opt: null, msgIdx: 0, lesson: null,
  race: null, anim: null, custom: store.get("custom", null), pair: store.get("pair", null), tool: "rep",
  lastRun: null,
};
if (!UI[state.sc]) state.sc = "motorway";
function defaultsFor(sc) {
  const d = S.SCENARIOS[sc].defaults || {};
  return { ...S.DEFAULTS, f: { ...S.DEFAULTS.f }, ...d, sf: S.SCENARIOS[sc].sf };
}
state.opt = defaultsFor(state.sc);

// ---------- controls ----------
const CTL = {
  msgs: { el: "c-msgs", out: "o-msgs", fmt: v => v },
  noise: { el: "c-noise", out: "o-noise", fmt: v => Math.round(v * 100) + " %" },
  speed: { el: "c-speed", out: "o-speed", fmt: v => v + "×" },
  people: { el: "c-people", out: "o-people", fmt: v => v + "×" },
  sf: { el: "c-sf", out: "o-sf", fmt: v => "SF" + v + " · " + S.AIRTIME[v] + " s/packet" },
  hops: { el: "c-hops", out: "o-hops", fmt: v => v },
  seed: { el: "c-seed", out: "o-seed", fmt: v => "#" + v },
};
function syncControls() {
  const o = state.opt;
  for (const [k, c] of Object.entries(CTL)) { $(c.el).value = o[k]; $(c.out).textContent = c.fmt(o[k]); }
  $("c-duty").value = String(o.duty);
  $("c-gw").checked = !!o.gw; $("c-server").checked = !!o.server;
  for (const k of Object.keys(o.f)) $("f-" + k).checked = !!o.f[k];
}
let changeTimer = 0;
function changed(immediate) {
  clearTimeout(changeTimer);
  changeTimer = setTimeout(() => { if (state.sc === "sandbox") drawEditor(); newRace(); if (state.sub === "run" && state.lastRun) runScenario(); }, immediate ? 0 : 220);
}
for (const [k, c] of Object.entries(CTL)) $(c.el).addEventListener("input", e => {
  state.opt[k] = +e.target.value; $(c.out).textContent = c.fmt(state.opt[k]); clearLesson(); changed();
});
$("c-duty").onchange = e => { state.opt.duty = +e.target.value; clearLesson(); changed(true); };
$("c-gw").onchange = e => { state.opt.gw = e.target.checked; clearLesson(); changed(true); };
$("c-server").onchange = e => { state.opt.server = e.target.checked; clearLesson(); changed(true); };
for (const k of Object.keys(S.DEFAULTS.f)) $("f-" + k).onchange = e => { state.opt.f[k] = e.target.checked; clearLesson(); changed(true); };
$("reset").onclick = () => { state.opt = defaultsFor(state.sc); clearLesson(); syncControls(); changed(true); };

// ---------- views ----------
function setView(v) {
  state.view = v;
  for (const b of document.querySelectorAll("#views button")) b.classList.toggle("on", b.dataset.view === v);
  for (const id of ["play", "lab", "model"]) $("view-" + id).classList.toggle("hidden", id !== v);
  if (v === "lab") buildLab();
  if (v === "model") buildModel();
  if (v !== "play") stopAnim();
}
document.querySelectorAll("[data-view]").forEach(b => b.addEventListener("click", e => { e.preventDefault(); setView(b.dataset.view); }));
function setSub(s) {
  state.sub = s;
  for (const b of document.querySelectorAll("#subtabs button")) b.classList.toggle("on", b.dataset.sub === s);
  $("sub-race").classList.toggle("hidden", s !== "race");
  $("sub-run").classList.toggle("hidden", s !== "run");
  if (s === "run" && !state.lastRun) runScenario();
}
document.querySelectorAll("#subtabs button").forEach(b => b.onclick = () => setSub(b.dataset.sub));

// ---------- scenario picker and story ----------
function buildPicker() {
  $("scenarios").innerHTML = ORDER.map(k => `<button class="sc${k === state.sc ? " on" : ""}" data-sc="${k}"><span class="ic">${UI[k].icon}</span><b>${esc(UI[k].title)}</b><small>${esc(UI[k].tag)}</small></button>`).join("");
  $("scenarios").querySelectorAll(".sc").forEach(b => b.onclick = () => selectScenario(b.dataset.sc));
}
function selectScenario(k) {
  state.sc = k; store.set("sc", k);
  state.opt = defaultsFor(k); state.msgIdx = 0; state.lastRun = null;
  history.replaceState(null, "", "#" + k);
  buildPicker(); buildStory(); syncControls();
  $("sandboxPanel").classList.toggle("hidden", k !== "sandbox");
  $("knock").checked = k === "blackout";
  $("runOut").innerHTML = `<p class="busy">Press "Run it" to send every message in the scenario with all four designs.</p>`;
  if (k === "sandbox") drawEditor();
  newRace();
  if (state.sub === "run") runScenario();
}
function buildStory() {
  const u = UI[state.sc];
  $("storyText").innerHTML = u.story;
  $("chips").innerHTML = u.lessons.map((l, i) => `<button class="chip" data-i="${i}">🔬 ${esc(l.label)}</button>`).join("");
  $("chips").querySelectorAll(".chip").forEach(b => b.onclick = () => applyLesson(+b.dataset.i));
  clearLesson();
}
function applyLesson(i) {
  const l = UI[state.sc].lessons[i];
  state.opt = defaultsFor(state.sc);
  for (const [k, v] of Object.entries(l.set)) { if (k === "f") Object.assign(state.opt.f, v); else state.opt[k] = v; }
  syncControls();
  $("chips").querySelectorAll(".chip").forEach((b, j) => b.classList.toggle("on", j === i));
  $("lesson").innerHTML = "<b>What to look for:</b> " + l.note + ` <a href="#" id="undoLesson">Back to the defaults</a>`;
  $("lesson").classList.add("show");
  $("undoLesson").onclick = e => { e.preventDefault(); state.opt = defaultsFor(state.sc); syncControls(); clearLesson(); changed(true); };
  state.lesson = i;
  changed(true);
}
function clearLesson() {
  state.lesson = null;
  $("lesson").classList.remove("show");
  $("chips").querySelectorAll(".chip").forEach(b => b.classList.remove("on"));
}

// ---------- drawing ----------
function fit(canvas, w, h, pad = 14) {
  const k = Math.min((canvas.width - 2 * pad) / w, (canvas.height - 2 * pad) / h);
  const ox = (canvas.width - w * k) / 2, oy = (canvas.height - h * k) / 2;
  return { k, sx: x => ox + x * k, sy: y => oy + y * k, ix: px => (px - ox) / k, iy: py => (py - oy) / k };
}
function seeded(n) { return S.rng(n); }
function paintBackground(g, scId, v, world) {
  const W = g.canvas.width, H = g.canvas.height;
  g.fillStyle = COL.map; g.fillRect(0, 0, W, H);
  const r = seeded(scId.length * 977);
  const rect = (x, y, w, h, c) => { g.fillStyle = c; g.fillRect(v.sx(x), v.sy(y), w * v.k, h * v.k); };
  if (scId === "motorway") {
    for (let i = 0; i < 24; i++) rect(i * 5, r() < .5 ? 0 : 20, 5, 10, i % 2 ? COL.field1 : COL.field2);
    g.strokeStyle = COL.road; g.lineWidth = Math.max(4, v.k * 1.1); g.beginPath(); g.moveTo(v.sx(0), v.sy(15)); g.lineTo(v.sx(120), v.sy(15)); g.stroke();
    g.strokeStyle = COL.map; g.lineWidth = 1; g.setLineDash([6, 8]); g.beginPath(); g.moveTo(v.sx(0), v.sy(15)); g.lineTo(v.sx(120), v.sy(15)); g.stroke(); g.setLineDash([]);
    for (const [x, y] of [[3, 18], [117, 12]]) for (let i = 0; i < 16; i++) rect(x - 4 + r() * 8, y - 3 + r() * 6, .9, .9, COL.block);
    label(g, "Town A", v.sx(1), v.sy(25)); label(g, "Town B", v.sx(111), v.sy(5));
  } else if (scId === "forest") {
    g.fillStyle = COL.map2; g.fillRect(0, 0, W, H);
    g.fillStyle = COL.tree;
    for (let i = 0; i < 420; i++) { g.beginPath(); g.arc(v.sx(r() * 8), v.sy(r() * 8), (.12 + r() * .18) * v.k, 0, 7); g.fill(); }
    g.strokeStyle = COL.road; g.lineWidth = 2; g.setLineDash([4, 5]); g.beginPath(); g.moveTo(v.sx(0), v.sy(1)); g.bezierCurveTo(v.sx(3), v.sy(4), v.sx(5), v.sy(1), v.sx(8), v.sy(6)); g.stroke(); g.setLineDash([]);
  } else if (scId === "city" || scId === "blackout" || scId === "mailbox") {
    const w = world.w, h = world.h, step = scId === "city" ? .42 : scId === "mailbox" ? 1.6 : 1.1;
    for (let x = .1; x < w; x += step) for (let y = .1; y < h; y += step) {
      if (scId === "mailbox" && Math.hypot(x - 10, y - 7) > 6.5 + r() * 2) continue;
      if (r() < .1) continue;
      rect(x, y, step * .72, step * .72, COL.block);
    }
    if (scId === "blackout") { g.font = `${Math.round(18 * W / 900)}px system-ui`; g.fillStyle = COL.muted; g.fillText("⛈ storm at 15 min", v.sx(9.6), v.sy(.7)); }
  } else if (scId === "country") {
    for (let x = 0; x < 50; x += 5) for (let y = 0; y < 36; y += 4) rect(x, y, 5, 4, r() < .5 ? COL.field1 : COL.field2);
    g.strokeStyle = COL.water; g.lineWidth = Math.max(3, v.k * .6); g.beginPath(); g.moveTo(v.sx(0), v.sy(20)); g.bezierCurveTo(v.sx(15), v.sy(14), v.sx(28), v.sy(30), v.sx(50), v.sy(24)); g.stroke();
    g.strokeStyle = COL.road; g.lineWidth = 2; g.beginPath(); g.moveTo(v.sx(5), v.sy(0)); g.bezierCurveTo(v.sx(15), v.sy(15), v.sx(35), v.sy(10), v.sx(45), v.sy(36)); g.stroke();
  } else if (scId === "festival") {
    g.fillStyle = COL.map2; g.fillRect(0, 0, W, H);
    rect(.55, .02, .5, .07, COL.ink); label(g, "STAGE", v.sx(.72), v.sy(.13));
    g.fillStyle = COL.block;
    for (let i = 0; i < 40; i++) { const x = v.sx(.05 + r() * 1.5), y = v.sy(.8 + r() * .75); g.beginPath(); g.moveTo(x, y - 6); g.lineTo(x + 6, y + 4); g.lineTo(x - 6, y + 4); g.fill(); }
  } else if (scId === "oslo") {
    g.fillStyle = COL.water; g.fillRect(0, 0, W, H);
    g.fillStyle = COL.map;
    const poly = pts => { g.beginPath(); pts.forEach(([x, y], i) => i ? g.lineTo(v.sx(x), v.sy(y)) : g.moveTo(v.sx(x), v.sy(y))); g.closePath(); g.fill(); };
    poly([[10, 70], [12, 52], [16, 40], [22, 33], [30, 36], [31, 45], [28, 55], [30, 70]]); // Jutland
    poly([[40, 54], [47, 50], [55, 53], [56, 62], [50, 67], [42, 64]]); // Zealand
    poly([[30, 0], [92, 0], [90, 22], [74, 28], [62, 24], [50, 22], [40, 14], [32, 10]]); // Norway / Sweden
    poly([[70, 30], [92, 26], [100, 40], [100, 70], [64, 70], [62, 56], [70, 44]]); // Sweden south
  } else if (scId === "sandbox") {
    g.strokeStyle = COL.line; g.lineWidth = 1;
    for (let x = 0; x <= world.w; x++) { g.beginPath(); g.moveTo(v.sx(x), v.sy(0)); g.lineTo(v.sx(x), v.sy(world.h)); g.stroke(); }
    for (let y = 0; y <= world.h; y++) { g.beginPath(); g.moveTo(v.sx(0), v.sy(y)); g.lineTo(v.sx(world.w), v.sy(y)); g.stroke(); }
  }
}
function label(g, t, x, y) { g.font = `600 ${Math.max(11, Math.round(g.canvas.width / 70))}px system-ui`; g.fillStyle = COL.muted; g.fillText(t, x, y); }
function drawLinks(g, w, v) {
  g.lineWidth = 1;
  for (let i = 0; i < w.N; i++) for (const j of w.nb[i]) {
    if (j < i) continue;
    const q = w.q(i, j); if (q < .25) continue;
    g.strokeStyle = COL.muted; g.globalAlpha = q * .22;
    g.beginPath(); g.moveTo(v.sx(w.nodes[i].x), v.sy(w.nodes[i].y)); g.lineTo(v.sx(w.nodes[j].x), v.sy(w.nodes[j].y)); g.stroke();
  }
  g.globalAlpha = 1;
  if (w.opt.gw && w.gws.length > 1) {
    g.strokeStyle = COL.accent; g.globalAlpha = .45; g.setLineDash([5, 6]); g.lineWidth = 1.5;
    const a = w.nodes[w.gws[0]];
    for (const id of w.gws.slice(1)) { const b = w.nodes[id]; arc(g, v.sx(a.x), v.sy(a.y), v.sx(b.x), v.sy(b.y)); }
    g.setLineDash([]); g.globalAlpha = 1;
  }
}
function arc(g, x1, y1, x2, y2, t) {
  const mx = (x1 + x2) / 2, my = Math.min(y1, y2) - Math.abs(x2 - x1) * .25 - 20;
  g.beginPath(); g.moveTo(x1, y1); g.quadraticCurveTo(mx, my, x2, y2); g.stroke();
  if (t != null) { const u = 1 - t; const x = u * u * x1 + 2 * u * t * mx + t * t * x2, y = u * u * y1 + 2 * u * t * my + t * t * y2; return [x, y]; }
}
function nodeSize(g) { return Math.max(4, Math.min(8, g.canvas.width / 120)); }
function drawNodes(g, w, v, hl) {
  const z = nodeSize(g);
  for (const n of w.nodes) {
    const x = v.sx(n.x), y = v.sy(n.y);
    const failed = n.failAt != null && w.t >= n.failAt;
    g.globalAlpha = n.off && !failed ? .3 : 1;
    let fill = n.role === "rep" ? COL.ink : COL.muted;
    if (hl && hl.has && hl.has.has(n.id)) fill = hl.color;
    g.fillStyle = fill; g.strokeStyle = fill;
    if (hl && hl.has && hl.has.has(n.id)) { g.globalAlpha = .22; g.beginPath(); g.arc(x, y, z * 2.2, 0, 7); g.fill(); g.globalAlpha = n.off ? .3 : 1; }
    g.beginPath();
    if (n.role === "rep") {
      const s = n.mast || n.tower ? z * 1.35 : z * 1.1;
      g.moveTo(x, y - s); g.lineTo(x + s * .85, y + s * .7); g.lineTo(x - s * .85, y + s * .7); g.closePath(); g.fill();
      g.lineWidth = 1.5; g.beginPath(); g.moveTo(x, y - s); g.lineTo(x, y - s * 1.7); g.stroke();
    } else if (n.gw || n.server) {
      g.fillRect(x - z, y - z, 2 * z, 2 * z);
      g.strokeStyle = COL.panel; g.lineWidth = 1;
      if (n.gw) { g.beginPath(); g.arc(x, y, z * .6, 0, 7); g.moveTo(x - z * .6, y); g.lineTo(x + z * .6, y); g.stroke(); }
      else { g.strokeRect(x - z * .6, y - z * .4, z * 1.2, z * .8); g.beginPath(); g.moveTo(x - z * .6, y - z * .4); g.lineTo(x, y + z * .1); g.lineTo(x + z * .6, y - z * .4); g.stroke(); }
    } else if (n.car) {
      g.fillRect(x - z * .9, y - z * .5, z * 1.8, z);
    } else { g.arc(x, y, z * .7, 0, 7); g.fill(); }
    if (failed) { g.globalAlpha = 1; g.strokeStyle = COL.bad; g.lineWidth = 3; g.beginPath(); g.moveTo(x - z * 1.3, y - z * 1.3); g.lineTo(x + z * 1.3, y + z * 1.3); g.moveTo(x + z * 1.3, y - z * 1.3); g.lineTo(x - z * 1.3, y + z * 1.3); g.stroke(); }
  }
  g.globalAlpha = 1;
  if (hl && hl.m) {
    for (const [id, t] of [[hl.m.s, "from"], [hl.m.d, "to"]]) {
      const n = w.nodes[id], x = v.sx(n.x), y = v.sy(n.y);
      g.strokeStyle = hl.color || COL.accent; g.lineWidth = 2.5; g.beginPath(); g.arc(x, y, z * 2, 0, 7); g.stroke();
      g.font = `700 ${Math.max(11, Math.round(g.canvas.width / 60))}px system-ui`; g.fillStyle = hl.color || COL.accent;
      g.fillText(t, x + z * 2.2, y - z * 1.6);
    }
  }
}

// ---------- race ----------
function laneHtml(pid) {
  return `<div class="lane" id="lane-${pid}"><header><b><span class="dot" style="background:var(--${pid})"></span>${NAMES[pid]}</b><span class="cnt" id="cnt-${pid}"></span></header>
<canvas id="cv-${pid}" width="640" height="360"></canvas><span class="badge" id="badge-${pid}"></span><div class="say" id="say-${pid}"></div></div>`;
}
$("lanes").innerHTML = PIDS.map(laneHtml).join("");
function laneAspect() {
  const sc = S.SCENARIOS[state.sc];
  return Math.min(.72, Math.max(.42, sc.h / sc.w));
}
function sizeCanvases() {
  const asp = laneAspect();
  for (const pid of PIDS) { const c = $("cv-" + pid), w = Math.round(Math.min(900, Math.max(320, c.clientWidth || 600)) * (devicePixelRatio > 1 ? 1.5 : 1)); c.width = w; c.height = Math.round(w * asp); }
}
function raceOpt() {
  const o = { ...state.opt, f: { ...state.opt.f } };
  if (state.sc === "sandbox" && state.pair) o.pair = state.pair;
  return o;
}
function newRace() {
  stopAnim();
  if (state.sc === "sandbox" && (!state.custom || state.custom.filter(n => n.role === "cli").length < 2)) {
    state.race = null; $("raceSum").textContent = "Place at least two people (and some repeaters) on the map to race a message.";
    for (const pid of PIDS) { const c = $("cv-" + pid).getContext("2d"); c.fillStyle = COL.map; c.fillRect(0, 0, c.canvas.width, c.canvas.height); $("say-" + pid).textContent = ""; $("cnt-" + pid).textContent = ""; $("badge-" + pid).className = "badge"; }
    return;
  }
  sizeCanvases();
  state.race = S.race(state.sc, raceOpt(), state.custom, state.msgIdx, $("knock").checked);
  if (!state.race) { $("raceSum").textContent = "No message to race in this setup."; return; }
  state.race.step = 0; state.race.t0 = 0;
  state.race.maxSteps = Math.max(...PIDS.map(p => state.race.lanes[p].steps.length));
  for (const pid of PIDS) { $("badge-" + pid).className = "badge"; $("badge-" + pid).textContent = ""; }
  const m = state.race.m, w = state.race.world;
  let intro = `Message from <b>${who(w, m.s)}</b> to <b>${who(w, m.d)}</b>`;
  if ($("knock").checked) intro += state.race.ko != null
    ? `. Everyone sent one message to this person earlier and learned a route. Then the repeater marked ✕ broke. How does each design cope?`
    : `. No repeater on these routes can be knocked out without cutting the recipient off completely (try the city or storm scenario).`;
  $("raceSum").innerHTML = intro + ". Press ▶ Play.";
  drawRaceFrame(0, 0);
}
function who(w, id) { const n = w.nodes[id]; return n.label || (n.car ? "a car" : n.home ? "a home" : n.role === "rep" ? "a repeater" : "person " + id); }
function stepCounts(steps, upto) {
  let pk = 0;
  for (let i = 0; i < Math.min(upto, steps.length); i++) { const s = steps[i]; pk += s.kind === "flood" ? s.tx.length : s.kind === "hop" || s.kind === "ack" ? 1 : 0; }
  return pk;
}
function holders(steps, upto, m) {
  const has = new Set([m.s]);
  for (let i = 0; i < Math.min(upto, steps.length); i++) {
    const s = steps[i];
    if (s.kind === "flood") s.newly.forEach(j => has.add(j));
    if ((s.kind === "hop" && s.ok) || s.kind === "tcp") has.add(s.b);
  }
  return has;
}
function drawRaceFrame(stepIdx, prog) {
  const R = state.race; if (!R) return;
  const w = R.world;
  for (const pid of PIDS) {
    const lane = R.lanes[pid], c = $("cv-" + pid), g = c.getContext("2d"), v = fit(c, w.w, w.h);
    paintBackground(g, state.sc, v, w);
    drawLinks(g, w, v);
    const color = COL[pid];
    const k = Math.min(stepIdx, lane.steps.length);
    const has = holders(lane.steps, stepIdx >= lane.steps.length ? k : k, R.m);
    const s = stepIdx < lane.steps.length ? lane.steps[stepIdx] : null;
    // current step overlay (under the nodes)
    if (s) drawStep(g, w, v, s, prog, color);
    drawNodes(g, w, v, { has, color, m: R.m });
    if (s) drawStepTop(g, w, v, s, prog, color);
    // texts
    const pk = stepCounts(lane.steps, s ? stepIdx + (prog > .5 ? 1 : 0) : lane.steps.length);
    $("cnt-" + pid).textContent = `step ${Math.min(stepIdx + 1, lane.steps.length)}/${lane.steps.length} · ${pk} packets`;
    const done = stepIdx >= lane.steps.length;
    $("say-" + pid).innerHTML = done ? finalSay(pid, lane) : esc(stepText(s, w));
    const b = $("badge-" + pid);
    if (done) { b.className = "badge " + (lane.ok ? "ok" : "fail"); b.textContent = lane.ok ? "✓ delivered" : "✗ " + (S.CAUSES[lane.cause] || "lost"); }
    else b.className = "badge";
  }
}
function stepText(s, w) {
  if (!s) return "";
  let t = s.label || "";
  if (s.kind === "hop") t += s.ok ? " ✓" : s.duty ? " (out of transmit time)" : s.retry ? " ✗ lost again" : " ✗ lost";
  if (s.kind === "flood") t += ` (${s.tx.length} node${s.tx.length === 1 ? "" : "s"} transmit${s.tx.length === 1 ? "s" : ""})`;
  if (s.kind === "tcp") t = "Over the internet (no airtime)";
  if (s.kind === "ack") t = "Acknowledged on this hop";
  return t;
}
function finalSay(pid, lane) {
  const air = lane.air.toFixed(1);
  if (lane.ok) return `<b>Delivered</b> with ${lane.tx} transmissions, ${air} s of airtime.`;
  return `<b>Not delivered</b>: ${esc(S.CAUSES[lane.cause] || "lost")}. Spent ${lane.tx} transmissions, ${air} s of airtime.`;
}
const ease = t => 1 - Math.pow(1 - t, 3);
function drawStep(g, w, v, s, p, color) {
  if (s.kind === "flood") {
    for (const i of s.tx) {
      const n = w.nodes[i], R = n.range * w.rangeK * .8 * v.k;
      g.strokeStyle = color; g.lineWidth = 2; g.globalAlpha = .7 * (1 - p * .7);
      g.beginPath(); g.arc(v.sx(n.x), v.sy(n.y), Math.max(4, R * ease(p)), 0, 7); g.stroke();
      g.globalAlpha = .08 * (1 - p); g.fillStyle = color; g.fill();
    }
    g.globalAlpha = 1;
    if (s.tcp && s.tcp.length) {
      g.strokeStyle = color; g.lineWidth = 2; g.setLineDash([6, 5]);
      for (const [a, b] of s.tcp) { const A = w.nodes[a], B = w.nodes[b]; const pt = arc(g, v.sx(A.x), v.sy(A.y), v.sx(B.x), v.sy(B.y), p); g.setLineDash([]); g.fillStyle = color; g.beginPath(); g.arc(pt[0], pt[1], 5, 0, 7); g.fill(); g.setLineDash([6, 5]); }
      g.setLineDash([]);
    }
  }
  if (s.kind === "hop" || s.kind === "ack" || s.kind === "tcp") {
    const A = w.nodes[s.a], B = w.nodes[s.b];
    const x1 = v.sx(A.x), y1 = v.sy(A.y), x2 = v.sx(B.x), y2 = v.sy(B.y);
    g.strokeStyle = color; g.lineWidth = s.kind === "hop" ? 4 : 2; g.globalAlpha = s.kind === "hop" ? .55 : .8;
    if (s.kind !== "hop") g.setLineDash([5, 5]);
    if (s.kind === "tcp") arc(g, x1, y1, x2, y2); else { g.beginPath(); g.moveTo(x1, y1); g.lineTo(x2, y2); g.stroke(); }
    g.setLineDash([]); g.globalAlpha = 1;
  }
}
function drawStepTop(g, w, v, s, p, color) {
  if (s.kind === "hop" || s.kind === "ack" || s.kind === "tcp") {
    const A = w.nodes[s.a], B = w.nodes[s.b];
    const x1 = v.sx(A.x), y1 = v.sy(A.y), x2 = v.sx(B.x), y2 = v.sy(B.y);
    const lost = s.kind === "hop" && !s.ok, t = lost ? Math.min(p, .6) / .6 * .6 : ease(p);
    let x = x1 + (x2 - x1) * t, y = y1 + (y2 - y1) * t;
    if (s.kind === "tcp") [x, y] = arc(g, x1, y1, x2, y2, t);
    g.fillStyle = s.kind === "ack" ? COL.panel : color; g.strokeStyle = color; g.lineWidth = 2;
    g.beginPath(); g.arc(x, y, s.kind === "ack" ? 4 : 6, 0, 7); g.fill(); g.stroke();
    if (lost && p > .6) {
      g.strokeStyle = COL.bad; g.lineWidth = 3; const z = 7;
      g.beginPath(); g.moveTo(x - z, y - z); g.lineTo(x + z, y + z); g.moveTo(x + z, y - z); g.lineTo(x - z, y + z); g.stroke();
    }
  }
}
function stopAnim() { if (state.anim) cancelAnimationFrame(state.anim); state.anim = null; $("play").textContent = "▶ Play"; }
function play() {
  const R = state.race; if (!R) return;
  if (state.anim) { stopAnim(); return; }
  if (R.step >= R.maxSteps) R.step = 0;
  $("play").textContent = "⏸ Pause";
  let last = performance.now(), prog = 0;
  const dur = () => +$("speed").value;
  const tick = now => {
    prog += (now - last) / dur(); last = now;
    if (prog >= 1) { prog = 0; R.step++; }
    if (R.step >= R.maxSteps) { drawRaceFrame(R.maxSteps, 0); stopAnim(); summarizeRace(); return; }
    drawRaceFrame(R.step, prog);
    state.anim = requestAnimationFrame(tick);
  };
  state.anim = requestAnimationFrame(tick);
}
function summarizeRace() {
  const R = state.race, L = R.lanes;
  const ok = PIDS.filter(p => L[p].ok);
  const parts = [];
  if (ok.length) {
    const cheapest = ok.reduce((a, b) => L[a].tx <= L[b].tx ? a : b);
    parts.push(`Delivered by ${ok.map(p => NAMES[p]).join(", ")}. Fewest transmissions: <b>${NAMES[cheapest]}</b> (${L[cheapest].tx}).`);
    const mt = L.mt, ma = L.ma;
    if (mt.ok && ma.ok && mt.tx > ma.tx * 1.5) parts.push(`The flood needed ${mt.tx} transmissions where Må:rse's path needed ${ma.tx}. Every one of them is airtime nobody else can use and battery somebody pays for.`);
  } else parts.push("Nobody got this one through.");
  const lost = PIDS.filter(p => !L[p].ok);
  if (lost.length) parts.push("Not delivered: " + lost.map(p => `${NAMES[p]} (${S.CAUSES[L[p].cause] || "lost"})`).join(", ") + ".");
  if (R.ko != null && L.ma.ok) parts.push("Må:rse noticed the broken repeater on the hop before it and found a way round.");
  parts.push(`<a href="#" id="raceRun">Run the whole scenario →</a>`);
  $("raceSum").innerHTML = parts.join(" ");
  $("raceRun").onclick = e => { e.preventDefault(); setSub("run"); };
}
$("play").onclick = play;
$("step").onclick = () => { const R = state.race; if (!R) return; stopAnim(); R.step = Math.min(R.maxSteps, R.step + 1); drawRaceFrame(R.step, R.step >= R.maxSteps ? 0 : .99); if (R.step >= R.maxSteps) summarizeRace(); };
$("restart").onclick = () => { if (!state.race) return; stopAnim(); state.race.step = 0; drawRaceFrame(0, 0); };
$("nextMsg").onclick = () => { state.msgIdx++; newRace(); };
$("knock").onchange = () => newRace();
window.addEventListener("resize", () => { clearTimeout(window._rz); window._rz = setTimeout(() => { if (state.race) { sizeCanvases(); drawRaceFrame(state.race.step, 0); } if (state.sc === "sandbox") drawEditor(); }, 150); });

// ---------- whole scenario ----------
function runScenario() {
  if (state.sc === "sandbox" && (!state.custom || state.custom.filter(n => n.role === "cli").length < 2)) { $("runOut").innerHTML = `<p class="busy">Place at least two people on the map first.</p>`; return; }
  $("runOut").innerHTML = `<p class="busy">Running ${state.opt.msgs} messages through all four designs…</p>`;
  const opt = raceOpt(); delete opt.pair;
  setTimeout(() => {
    const t0 = performance.now();
    const res = S.runAll(state.sc, opt, state.custom);
    state.lastRun = res;
    $("runInfo").textContent = `${res.ma.n} messages · computed in ${Math.round(performance.now() - t0)} ms`;
    renderRun(res);
  }, 30);
}
$("runBtn").onclick = runScenario;
function bars(res, f, fmt, hint, title, opts = {}) {
  const vals = PIDS.map(p => f(res[p]));
  const max = opts.max || Math.max(1e-9, ...vals.filter(isFinite));
  return `<div class="card"><h3>${title}</h3><div class="hint">${hint}</div>` + PIDS.map((p, i) => {
    const v = vals[i], wv = isFinite(v) ? Math.max(1.5, v / max * 100) : 0;
    return `<div class="bar"><span class="name" style="color:var(--${p})">${NAMES[p]}</span><div class="track"><span style="width:${wv}%;background:var(--${p})"></span></div><span class="val">${isFinite(v) ? fmt(v) : "–"}</span></div>`;
  }).join("") + "</div>";
}
const fmtS = v => v < 10 ? v.toFixed(1) + " s" : v < 120 ? Math.round(v) + " s" : (v / 60).toFixed(v < 600 ? 1 : 0) + " min";
function renderRun(res) {
  const causeKeys = Object.keys(S.CAUSES);
  const used = new Set();
  const delivered = `<div class="card" style="grid-column:1/-1"><h3>Messages delivered</h3><div class="hint">Solid: delivered straight away. Light: delivered later (mailbox, carry, Store &amp; Forward, propagation node). Coloured tail: why the rest were lost.</div>` +
    PIDS.map(p => {
      const r = res[p], n = Math.max(1, r.n);
      let segs = `<span style="width:${r.onTime / n * 100}%;background:var(--${p})"></span><span style="width:${r.late / n * 100}%;background:var(--${p});opacity:.45"></span>`;
      for (const c of causeKeys) if (r.causes[c]) { used.add(c); segs += `<span title="${S.CAUSES[c]}: ${r.causes[c]}" style="width:${r.causes[c] / n * 100}%;background:var(--c-${c})"></span>`; }
      return `<div class="bar"><span class="name" style="color:var(--${p})">${NAMES[p]}</span><div class="track">${segs}</div><span class="val">${Math.round(r.delivered * 100)} %</span></div>`;
    }).join("") +
    `<div class="causes">${[...used].map(c => `<span><i style="background:var(--c-${c})"></i>${S.CAUSES[c]}</span>`).join("")}</div></div>`;
  const cards = `<div class="cards">${delivered}
${bars(res, r => r.air, fmtS, "Total time the radios spent transmitting. Lower leaves room for more traffic.", "Airtime used")}
${bars(res, r => r.bystanderMah, v => v.toFixed(v < 1 ? 2 : 1) + " mAh", "Battery that handhelds spent repeating other people's messages.", "Battery used by bystanders")}
${bars(res, r => r.txPerMsg, v => v.toFixed(1), "How many times something was transmitted, per message.", "Transmissions per message")}
${bars(res, r => r.latMed, fmtS, "Median time from sending to delivery (including late deliveries).", "Typical delivery time")}
</div>`;
  $("runOut").innerHTML = verdict(res) + cards + `<div class="explain">${PIDS.map(p => `<div style="border-color:var(--${p})">${explain(p, res[p])}</div>`).join("")}</div>` + featureBox(res.ma);
}
function verdict(res) {
  const best = PIDS.reduce((a, b) => res[a].delivered >= res[b].delivered ? a : b);
  const cheap = PIDS.filter(p => res[p].delivered >= res[best].delivered - .05).reduce((a, b) => res[a].air <= res[b].air ? a : b);
  let t = `<p style="margin-top:0"><b>In this run:</b> most messages delivered by <b style="color:var(--${best})">${NAMES[best]}</b> (${Math.round(res[best].delivered * 100)} %).`;
  if (cheap !== best) t += ` <b style="color:var(--${cheap})">${NAMES[cheap]}</b> came within 5 points using less airtime (${fmtS(res[cheap].air)} vs ${fmtS(res[best].air)}).`;
  else t += ` It also used the least airtime of those that came close.`;
  return t + "</p>";
}
function explain(p, r) {
  const n = r.n, got = r.onTime + r.late;
  let s = `<b>${NAMES[p]}</b> delivered ${got} of ${n}`;
  if (r.late) s += ` (${r.late} later, through ${p === "ma" ? "mailbox or carry" : p === "rn" ? "the propagation node" : p === "mt" ? "Store & Forward" : "a store"})`;
  s += ".";
  const lost = Object.entries(r.causes).filter(([, v]) => v).sort((a, b) => b[1] - a[1]);
  if (lost.length) s += " Lost: " + lost.map(([c, v]) => `${v} ${S.CAUSES[c]}`).join(", ") + ".";
  if (r.bystanderMah > .05) s += ` Handhelds spent ${r.bystanderMah.toFixed(1)} mAh repeating for others.`;
  if (p === "mt" && r.causes.hops) s += ` Raise the hop limit to reach further, at the cost of more copies.`;
  if (p === "mc" && r.causes.range) s += ` Clients do not repeat, so people out of a repeater's reach cannot be reached.`;
  if (p === "rn" && r.annAir > 1) s += ` ${fmtS(r.annAir)} of its airtime went on announces.`;
  return s;
}
function featureBox(r) {
  const f = state.opt.f, sv = r.saved, items = [];
  if (sv.hopRetry) items.push(`<b>${sv.hopRetry}</b> times a packet lost on one hop was resent on that hop instead of from the start (per-hop acknowledgement).`);
  if (sv.repair) items.push(`<b>${sv.repair}</b> messages got through after a route repair or a second path request.`);
  if (sv.relay) items.push(`<b>${sv.relay}</b> messages went through handhelds in relay role.`);
  if (sv.carry) items.push(`<b>${sv.carry}</b> messages were carried by people moving and handed over later.`);
  if (sv.mailbox) items.push(`<b>${sv.mailbox}</b> messages waited in a mailbox and were delivered when the recipient came back.`);
  if (sv.backbone) items.push(`<b>${sv.backbone}</b> messages crossed the internet backbone instead of the radio.`);
  const off = Object.entries(f).filter(([, v]) => !v).map(([k]) => ({ hopAck: "per-hop acknowledgement", bestPath: "best-signal path", relay: "relay role", carry: "carry", mailbox: "mailbox", backbone: "backbone" }[k]));
  return `<div class="features"><b>What Må:rse's features did in this run</b>${items.length ? `<ul>${items.map(i => `<li>${i}</li>`).join("")}</ul>` : `<p class="small muted" style="margin:6px 0 0">Nothing special was needed: plain path routing did the job.</p>`}${off.length ? `<p class="small" style="margin:8px 0 0">Switched off: ${off.join(", ")}.</p>` : ""}</div>`;
}

// ---------- sandbox editor ----------
const PRESETS = {
  valley: [
    { x: 5, y: 1.2, role: "rep", range: 5 }, { x: 1.5, y: 3.5, role: "rep", range: 3.5 }, { x: 8.5, y: 3.5, role: "rep", range: 3.5 },
    { x: .8, y: 6, role: "cli", range: 1.6 }, { x: 2.4, y: 5.6, role: "cli", range: 1.6 }, { x: 4.2, y: 6.3, role: "cli", range: 1.6 },
    { x: 5.8, y: 5.4, role: "cli", range: 1.6 }, { x: 7.5, y: 6.2, role: "cli", range: 1.6 }, { x: 9.3, y: 5.8, role: "cli", range: 1.6 },
    { x: 3.2, y: 2.6, role: "cli", range: 1.6 }, { x: 6.8, y: 2.4, role: "cli", range: 1.6 },
  ],
  islands: [
    { x: 1.6, y: 3.5, role: "rep", range: 3 }, { x: 8.4, y: 3.5, role: "rep", range: 3 },
    { x: .6, y: 2.5, role: "cli", range: 1.6, gw: true }, { x: 9.4, y: 4.5, role: "cli", range: 1.6, gw: true },
    { x: .8, y: 4.8, role: "cli", range: 1.6 }, { x: 2.5, y: 2.2, role: "cli", range: 1.6 }, { x: 2.7, y: 4.6, role: "cli", range: 1.6 },
    { x: 7.4, y: 2.4, role: "cli", range: 1.6 }, { x: 7.3, y: 4.9, role: "cli", range: 1.6 }, { x: 9.2, y: 2.3, role: "cli", range: 1.6 },
    { x: 4.1, y: 3.6, role: "cli", range: 1.6 }, { x: 5.6, y: 3.3, role: "cli", range: 1.6 },
  ],
};
if (!state.custom) state.custom = PRESETS.valley.map(n => ({ ...n }));
function saveCustom() { store.set("custom", state.custom); store.set("pair", state.pair); }
function editorWorld() { return new S.World("sandbox", { ...state.opt, f: { ...state.opt.f }, msgs: 0 }, state.custom); }
function drawEditor() {
  const c = $("editCanvas"); if (!c) return;
  const cw = c.clientWidth || 900; c.width = Math.round(cw * (devicePixelRatio > 1 ? 1.5 : 1)); c.height = Math.round(c.width * .62);
  const g = c.getContext("2d"), w = editorWorld(), v = fit(c, w.w, w.h);
  paintBackground(g, "sandbox", v, w);
  // coverage circles
  for (const n of w.nodes) { g.fillStyle = n.role === "rep" ? COL.accent : COL.muted; g.globalAlpha = .06; g.beginPath(); g.arc(v.sx(n.x), v.sy(n.y), n.range * w.rangeK * .8 * v.k, 0, 7); g.fill(); }
  g.globalAlpha = 1;
  drawLinks(g, w, v);
  const pair = state.pair && state.pair[0] < w.N && state.pair[1] < w.N ? { s: state.pair[0], d: state.pair[1] } : null;
  drawNodes(g, w, v, pair ? { m: pair, color: COL.accent } : null);
}
let drag = null;
function editPos(e) { const c = $("editCanvas"), r = c.getBoundingClientRect(), v = fit(c, S.SCENARIOS.sandbox.w, S.SCENARIOS.sandbox.h); const px = (e.clientX - r.left) * c.width / r.width, py = (e.clientY - r.top) * c.height / r.height; return { x: Math.max(.1, Math.min(9.9, v.ix(px))), y: Math.max(.1, Math.min(6.9, v.iy(py))) }; }
function nearest(p) { let b = -1, bd = .45; state.custom.forEach((n, i) => { const d = Math.hypot(n.x - p.x, n.y - p.y); if (d < bd) { bd = d; b = i; } }); return b; }
$("editCanvas").addEventListener("pointerdown", e => {
  const p = editPos(e), hit = nearest(p), t = state.tool;
  if (t === "move" || (hit >= 0 && (t === "rep" || t === "cli" || t === "gw"))) { if (hit >= 0) { drag = hit; $("editCanvas").setPointerCapture(e.pointerId); } return; }
  if (t === "erase") { if (hit >= 0) { state.custom.splice(hit, 1); state.pair = null; } }
  else if (t === "from" || t === "to") {
    // A half-set pair is ignored by the engine until both ends are chosen.
    if (hit >= 0 && state.custom[hit].role === "cli") { const pr = state.pair || [null, null]; pr[t === "from" ? 0 : 1] = hit; state.pair = pr; }
  }
  else state.custom.push(t === "rep" ? { ...p, role: "rep", range: 3.5 } : t === "gw" ? { ...p, role: "cli", range: 1.6, gw: true } : { ...p, role: "cli", range: 1.6 });
  saveCustom(); drawEditor(); editChanged();
});
$("editCanvas").addEventListener("pointermove", e => { if (drag == null) return; const p = editPos(e); state.custom[drag].x = p.x; state.custom[drag].y = p.y; drawEditor(); });
$("editCanvas").addEventListener("pointerup", () => { if (drag != null) { drag = null; saveCustom(); editChanged(); } });
function editChanged() { state.lastRun = null; state.msgIdx = 0; clearTimeout(changeTimer); changeTimer = setTimeout(newRace, 250); }
document.querySelectorAll("#tools [data-tool]").forEach(b => b.onclick = () => { state.tool = b.dataset.tool; document.querySelectorAll("#tools [data-tool]").forEach(x => x.classList.toggle("on", x === b)); });
$("preset-valley").onclick = () => { state.custom = PRESETS.valley.map(n => ({ ...n })); state.pair = null; saveCustom(); drawEditor(); editChanged(); };
$("preset-islands").onclick = () => { state.custom = PRESETS.islands.map(n => ({ ...n })); state.pair = [4, 9]; saveCustom(); drawEditor(); editChanged(); };
$("clearMap").onclick = () => { state.custom = []; state.pair = null; saveCustom(); drawEditor(); editChanged(); };

// ---------- scaling lab ----------
const LABTEXT = {
  growth: "A flood costs one transmission per node per message, so its airtime grows with the size of the network. A path costs one transmission per hop, which grows slowly. Reticulum's curve includes its announces: every node announces itself to the whole network. This is why large flooding meshes struggle, and why MeshCore and Må:rse send most traffic along paths.",
  chain: "Meshtastic stops at its hop limit (set it under Conditions). A single path with no per-hop resends loses a little at every hop, so long chains fail more often. Må:rse resends a lost packet on the hop where it was lost, so long chains stay reliable.",
  noise: "As interference rises, single tries fail more often. Flooding hides it with redundancy but pays in airtime. Paths without per-hop resends have to start over from the sender. Per-hop acknowledgements keep paths working.",
  speed: "Moving nodes break learned routes. A flood does not care, but it cannot reach far. MeshCore notices a broken route only when the end-to-end acknowledgement is missing. Må:rse notices at the broken hop, and carry moves messages along with the cars.",
};
let labBuilt = false;
const labSeries = {};
function buildLab() {
  if (labBuilt) return; labBuilt = true;
  $("labgrid").innerHTML = Object.entries(S.LAB).map(([k, L]) => `<div class="panel chart" id="lab-${k}"><h2>${esc(L.title)}</h2><p class="muted">${LABTEXT[k]}</p>
<div class="racebar"><button class="btn primary" data-lab="${k}">▶ Run</button><span class="small muted" id="labinfo-${k}"></span></div><div class="progress"><span id="labprog-${k}"></span></div>
<canvas id="labcv-${k}" width="800" height="440"></canvas></div>`).join("");
  document.querySelectorAll("[data-lab]").forEach(b => b.onclick = () => runLab(b.dataset.lab, b));
  for (const k of Object.keys(S.LAB)) drawChart(k, null);
}
function runLab(k, btn) {
  const L = S.LAB[k], xs = L.xs, series = Object.fromEntries(PIDS.map(p => [p, []]));
  const base = { ...S.DEFAULTS, f: { ...state.opt.f }, seed: state.opt.seed, hops: state.opt.hops, duty: state.opt.duty };
  btn.disabled = true; let i = 0;
  const t0 = performance.now();
  const next = () => {
    if (i >= xs.length) { btn.disabled = false; $("labinfo-" + k).textContent = `done in ${((performance.now() - t0) / 1000).toFixed(1)} s · average of 3 maps · Meshtastic hop limit ${base.hops}`; return; }
    // Average three random maps per point so one unlucky map does not dominate.
    const sum = Object.fromEntries(PIDS.map(p => [p, 0]));
    for (let r = 0; r < 3; r++) { const res = L.run(xs[i], { ...base, seed: base.seed + r * 101 }); for (const p of PIDS) sum[p] += L.metric(res[p]) / 3; }
    for (const p of PIDS) series[p].push(sum[p]);
    labSeries[k] = series;
    i++; $("labprog-" + k).style.width = (i / xs.length * 100) + "%";
    drawChart(k, series);
    setTimeout(next, 10);
  };
  $("labinfo-" + k).textContent = "running…";
  setTimeout(next, 20);
}
function drawChart(k, series) {
  const L = S.LAB[k], c = $("labcv-" + k); if (!c) return;
  const cw = c.clientWidth || 600; c.width = Math.round(cw * (devicePixelRatio > 1 ? 1.5 : 1)); c.height = Math.round(c.width * .55);
  const g = c.getContext("2d"), W = c.width, H = c.height, sc = W / 600;
  const pl = 52 * sc, pr = 16 * sc, pt = 18 * sc, pb = 44 * sc;
  g.clearRect(0, 0, W, H);
  const xs = L.xs, isPct = /%/.test(L.y);
  let ymax = isPct ? 100 : 1;
  if (series && !isPct) for (const p of PIDS) for (const v of series[p]) if (isFinite(v)) ymax = Math.max(ymax, v * 1.1);
  const X = i => pl + (W - pl - pr) * (xs.length === 1 ? .5 : i / (xs.length - 1));
  const Y = v => H - pb - (H - pt - pb) * (v / ymax);
  g.strokeStyle = COL.line; g.fillStyle = COL.muted; g.lineWidth = 1; g.font = `${Math.round(11 * sc)}px system-ui`;
  for (let t = 0; t <= 4; t++) { const v = ymax * t / 4, y = Y(v); g.beginPath(); g.moveTo(pl, y); g.lineTo(W - pr, y); g.stroke(); g.fillText(isPct ? Math.round(v) + "%" : v.toFixed(v < 10 ? 1 : 0), 6 * sc, y + 4 * sc); }
  xs.forEach((x, i) => g.fillText(String(x), X(i) - 6 * sc, H - pb + 16 * sc));
  g.fillText(L.x, pl, H - 8 * sc);
  g.save(); g.translate(12 * sc, pt + 40 * sc); g.restore();
  if (!series) { g.fillStyle = COL.faint; g.font = `${Math.round(14 * sc)}px system-ui`; g.fillText("Press Run", W / 2 - 30 * sc, H / 2); return; }
  for (const p of PIDS) {
    const pts = series[p]; g.strokeStyle = COL[p]; g.fillStyle = COL[p]; g.lineWidth = 2.5 * sc;
    g.beginPath(); pts.forEach((v, i) => { const x = X(i), y = Y(v); i ? g.lineTo(x, y) : g.moveTo(x, y); }); g.stroke();
    pts.forEach((v, i) => { g.beginPath(); g.arc(X(i), Y(v), 3.5 * sc, 0, 7); g.fill(); });
  }
  let lx = pl + 6 * sc;
  g.font = `600 ${Math.round(12 * sc)}px system-ui`;
  for (const p of PIDS) { g.fillStyle = COL[p]; g.fillRect(lx, pt, 10 * sc, 10 * sc); g.fillText(NAMES[p], lx + 14 * sc, pt + 9 * sc); lx += g.measureText(NAMES[p]).width + 30 * sc; }
  g.fillStyle = COL.muted; g.font = `${Math.round(11 * sc)}px system-ui`; g.fillText(L.y, W - pr - g.measureText(L.y).width, pt + 9 * sc + 16 * sc);
}

// ---------- the model page ----------
function buildModel() {
  $("modelText").innerHTML = `<h2>How the model works</h2>
<p>This lab is a <b>simplified, seeded model</b>. It builds intuition for why the designs behave differently. It is not a radio planning tool, and its numbers are indicative only. The project's own simulator, <code>maarse-sim</code>, runs the real Må:rse router code and is where the README's numbers come from.</p>
<h3>The radio</h3>
<ul>
<li>Every node has a range. A link works almost every time at short distance and less often further out, and each pair of nodes gets a fixed random "shadowing" factor (hills, walls).</li>
<li><b>Interference</b> damages that share of packets on top of the distance losses.</li>
<li>When several nodes repeat the same packet at the same moment, strong copies can collide. A busy channel loses more: the model runs every scenario twice. The second run adds losses in proportion to the airtime the first run used around each node.</li>
<li><b>Duty cycle:</b> each node may transmit only its share of the time. When its budget is used up, it stops transmitting.</li>
<li><b>Spreading factor</b> scales range (about 25 % per step) and airtime (about ×1.9 per step).</li>
</ul>
<h3>The four designs, as modelled</h3>
<div class="tablewrap"><table>
<tr><th></th><th>Finding the recipient</th><th>Sending</th><th>When it fails</th><th>Offline recipient</th></tr>
<tr><td style="color:var(--mt)"><b>Meshtastic</b></td><td>Managed flood: every node repeats once, up to the hop limit. Clients cancel if they hear a neighbour repeat first; routers always repeat.</td><td>The flood is the message. The acknowledgement is flooded back.</td><td>Sent again up to 3 times.</td><td>Store &amp; Forward router keeps it; the recipient asks for history about once an hour.</td></tr>
<tr><td style="color:var(--mc)"><b>MeshCore</b></td><td>The first message is flooded through repeaters only (clients do not repeat).</td><td>Then along the learned path; end-to-end acknowledgement.</td><td>Up to 3 tries; a failed direct send falls back to a flood.</td><td>A direct message is given up (room servers hold posts, not direct messages).</td></tr>
<tr><td style="color:var(--rn)"><b>Reticulum</b></td><td>Every node announces at start; transport nodes (repeaters) learn the next hop. Unknown or broken path: a path request.</td><td>Next hop by next hop, no resend on a single hop; delivery proof back.</td><td>Up to 5 delivery attempts (LXMF), with new path requests.</td><td>Propagation node keeps it; the recipient's app syncs every 30 minutes.</td></tr>
<tr><td style="color:var(--ma)"><b>Må:rse</b></td><td>A small path request through repeaters (and handhelds in relay role when no repeater is around). The recipient answers along the best-signal path. With the backbone, a host looks the recipient up in the DHT and only the recipient's area is asked.</td><td>Along the path, each hop acknowledged and resent up to 3 times.</td><td>Route repair at once; up to 3 path requests. Then mailbox (if a host can keep it) or carry (if people move).</td><td>Mailbox on a host or in a repeater's flash, delivered as soon as the recipient is back.</td></tr>
</table></div>
<h3>What is left out</h3>
<ul>
<li>Only direct messages are simulated, not channel messages (which every design floods). Må:rse's regions and proof-of-work stamps matter there, and are not shown.</li>
<li>Newer Meshtastic versions can also remember the next hop for direct messages. The model uses the classic managed flood.</li>
<li>Real timing, the capture effect, antenna heights and terrain are approximated, not computed.</li>
<li>Reticulum on a computer can use many more interfaces than LoRa. Here it runs over the same radios as the others, with announces limited the way its announce cap does.</li>
</ul>
<h3>What a listener hears</h3>
<p>Not simulated, but part of the comparison. This is what someone with a radio receiver can read from a direct message without any key:</p>
<div class="tablewrap"><table>
<tr><th></th><th>Readable without a key</th></tr>
<tr><td style="color:var(--mt)"><b>Meshtastic</b></td><td>Sender and recipient node numbers, packet id, channel hash and hop counts (the header is not encrypted).</td></tr>
<tr><td style="color:var(--mc)"><b>MeshCore</b></td><td>Short hashes of the recipient and the sender, and the path of repeater ids.</td></tr>
<tr><td style="color:var(--rn)"><b>Reticulum</b></td><td>The recipient's destination hash. No sender address.</td></tr>
<tr><td style="color:var(--ma)"><b>Må:rse</b></td><td>The recipient's 16-byte address and the path. The sender is never in the header.</td></tr>
</table></div>
<p class="muted small">Source code: <a href="https://github.com/trixxor/maarse/tree/main/docs/simulator">docs/simulator</a> (engine.js is the model, app.js the page). Found something unfair to one of the designs? Please open an issue.</p>`;
}

// ---------- start ----------
function redrawAll() { if (state.race) drawRaceFrame(state.race.step, 0); if (state.sc === "sandbox") drawEditor(); if (labBuilt) for (const k of Object.keys(S.LAB)) drawChart(k, labSeries[k] || null); }
const fromHash = location.hash.slice(1);
if (UI[fromHash]) state.sc = fromHash;
buildPicker();
selectScenario(state.sc);
})();
