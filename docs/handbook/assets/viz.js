/* Må:rse handbook visualisations. No dependencies. Everything is drawn as inline SVG
   and follows the page theme through CSS variables.

   Viz.toa(bytes)              LoRa airtime in ms (SF9, 125 kHz, CR 4/5, preamble 16, explicit header, CRC)
   Viz.bars(el, spec)          grouped horizontal bars
   Viz.stack(el, spec)         100 % stacked bars
   Viz.frames(el, spec)        byte anatomy of packets, drawn to one scale
   Viz.mesh(el, opts)          flood against routed delivery, with real airtimes
   Viz.roleLab(el)             what a node does with a message, by role and battery
   Viz.timeline(el, steps)     a played step list
*/
(function () {
  "use strict";
  var NS = "http://www.w3.org/2000/svg";
  function s(tag, attrs, text) {
    var e = document.createElementNS(NS, tag);
    for (var k in (attrs || {})) e.setAttribute(k, attrs[k]);
    if (text != null) e.textContent = text;
    return e;
  }
  function h(tag, attrs, text) {
    var e = document.createElement(tag);
    for (var k in (attrs || {})) e.setAttribute(k, attrs[k]);
    if (text != null) e.textContent = text;
    return e;
  }
  function clear(el) { while (el.firstChild) el.removeChild(el.firstChild); }
  function fmt(v, unit, dec) {
    var d = dec == null ? (Math.abs(v) >= 100 ? 0 : 1) : dec;
    var t = (Math.round(v * Math.pow(10, d)) / Math.pow(10, d)).toLocaleString("en-US", { maximumFractionDigits: d });
    return t + (unit || "");
  }

  /* LoRa time on air, the Semtech AN1200.13 formula used by maarse-core/src/airtime.rs */
  function toa(bytes, o) {
    o = o || {};
    var sf = o.sf || 9, bw = o.bw || 125000, cr = o.cr || 1, pre = o.pre == null ? 16 : o.pre;
    var tsym = Math.pow(2, sf) / bw * 1000;
    var de = (sf >= 11 && bw === 125000) ? 1 : 0;
    var n = Math.ceil((8 * bytes - 4 * sf + 28 + 16) / (4 * (sf - 2 * de)));
    var pay = 8 + Math.max(n * (cr + 4), 0);
    return (pre + 4.25 + pay) * tsym;
  }

  /* ---------- grouped bars ---------- */
  function bars(el, spec) {
    clear(el);
    var W = 760, left = 190, right = 90, rowH = 22, gap = 6, gpad = 26;
    var groups = spec.groups, series = spec.series || [];
    var max = spec.max || 0;
    groups.forEach(function (g) { g.bars.forEach(function (b) { if (b.value > max) max = b.value; }); });
    max = max * 1.04;
    var H = 8;
    groups.forEach(function (g) { H += gpad + g.bars.length * (rowH + gap) + 10; });
    var svg = s("svg", { viewBox: "0 0 " + W + " " + H, role: "img", "aria-label": spec.title || "chart", "class": "viz-svg" });
    var y = 8;
    groups.forEach(function (g) {
      svg.appendChild(s("text", { x: 0, y: y + 16, "class": "v-group" }, g.label));
      if (g.note) svg.appendChild(s("text", { x: W, y: y + 16, "text-anchor": "end", "class": "v-note" }, g.note));
      y += gpad;
      g.bars.forEach(function (b) {
        var w = Math.max(1, (W - left - right) * b.value / max);
        var si = b.series != null ? b.series : 0;
        svg.appendChild(s("text", { x: left - 8, y: y + 15, "text-anchor": "end", "class": "v-label" }, b.label));
        var r = s("rect", { x: left, y: y, width: w, height: rowH, rx: 4, "class": "v-bar c" + (si + 1) + (b.dim ? " dim" : "") });
        r.appendChild(s("title", {}, b.label + ": " + fmt(b.value, spec.unit) + (b.note ? " (" + b.note + ")" : "")));
        svg.appendChild(r);
        svg.appendChild(s("text", { x: left + w + 6, y: y + 15, "class": "v-val" }, fmt(b.value, spec.unit) + (b.tag ? "  " + b.tag : "")));
        y += rowH + gap;
      });
      y += 10;
    });
    el.appendChild(svg);
    if (series.length) {
      var lg = h("div", { "class": "v-legend" });
      series.forEach(function (n, i) {
        var it = h("span", { "class": "v-key" });
        it.appendChild(h("i", { "class": "sw c" + (i + 1) }));
        it.appendChild(document.createTextNode(n));
        lg.appendChild(it);
      });
      el.appendChild(lg);
    }
  }

  /* ---------- 100 % stacked bars ---------- */
  function stack(el, spec) {
    clear(el);
    var W = 760, left = 150, rowH = 28, gap = 10;
    var keys = spec.keys;
    var H = spec.rows.length * (rowH + gap) + 10;
    var svg = s("svg", { viewBox: "0 0 " + W + " " + H, role: "img", "aria-label": spec.title || "chart", "class": "viz-svg" });
    spec.rows.forEach(function (r, i) {
      var y = 4 + i * (rowH + gap);
      var total = 0; keys.forEach(function (k) { total += r.values[k] || 0; });
      svg.appendChild(s("text", { x: left - 8, y: y + 19, "text-anchor": "end", "class": "v-label" }, r.label));
      var x = left, span = W - left - 4;
      keys.forEach(function (k, ki) {
        var v = r.values[k] || 0; if (!v) return;
        var w = span * v / total;
        var rect = s("rect", { x: x, y: y, width: Math.max(0.5, w), height: rowH, "class": "v-bar c" + (ki + 1) });
        rect.appendChild(s("title", {}, k + ": " + fmt(100 * v / total, " %") + " of airtime"));
        svg.appendChild(rect);
        if (w > 38) svg.appendChild(s("text", { x: x + w / 2, y: y + 19, "text-anchor": "middle", "class": "v-in" }, Math.round(100 * v / total) + "%"));
        x += w;
      });
    });
    el.appendChild(svg);
    var lg = h("div", { "class": "v-legend" });
    keys.forEach(function (k, i) {
      var it = h("span", { "class": "v-key" });
      it.appendChild(h("i", { "class": "sw c" + (i + 1) }));
      it.appendChild(document.createTextNode(k));
      lg.appendChild(it);
    });
    el.appendChild(lg);
  }

  /* ---------- packet anatomy ---------- */
  function frames(el, spec) {
    clear(el);
    var W = 760, left = 150, right = 120, rowH = 34, gap = 22;
    var max = spec.scale || 255;
    var H = spec.rows.length * (rowH + gap) + 30;
    var svg = s("svg", { viewBox: "0 0 " + W + " " + H, role: "img", "aria-label": spec.title || "packet anatomy", "class": "viz-svg" });
    var span = W - left - right;
    spec.rows.forEach(function (r, i) {
      var y = 6 + i * (rowH + gap), x = left, total = 0;
      svg.appendChild(s("text", { x: left - 8, y: y + 21, "text-anchor": "end", "class": "v-label" }, r.label));
      r.parts.forEach(function (p) {
        var w = span * p.bytes / max; total += p.bytes;
        var rect = s("rect", { x: x, y: y, width: Math.max(1, w), height: rowH, "class": "v-bar c" + (p.c || 1) + (p.data ? " data" : "") });
        rect.appendChild(s("title", {}, p.label + ": " + p.bytes + " bytes"));
        svg.appendChild(rect);
        if (w > 30) svg.appendChild(s("text", { x: x + w / 2, y: y + 21, "text-anchor": "middle", "class": "v-in" }, p.short || p.bytes));
        x += w;
      });
      svg.appendChild(s("text", { x: x + 8, y: y + 21, "class": "v-val" }, total + " B · " + fmt(toa(total) / 1000, " s", 2)));
    });
    var ky = 6 + spec.rows.length * (rowH + gap) - 4;
    svg.appendChild(s("text", { x: left, y: ky + 14, "class": "v-note" }, spec.caption || "Drawn to one scale. Time is the airtime at SF9, 125 kHz."));
    el.appendChild(svg);
    if (spec.legend) {
      var lg = h("div", { "class": "v-legend" });
      spec.legend.forEach(function (l) {
        var it = h("span", { "class": "v-key" });
        it.appendChild(h("i", { "class": "sw c" + l[0] }));
        it.appendChild(document.createTextNode(l[1]));
        lg.appendChild(it);
      });
      el.appendChild(lg);
    }
  }

  /* ---------- the mesh player ---------- */
  var NODES = [
    ["S", 55, 160, "sender"], ["a", 165, 70], ["b", 170, 165], ["c", 160, 260],
    ["d", 285, 50], ["e", 295, 150], ["f", 285, 250],
    ["g", 415, 95], ["h", 425, 205],
    ["i", 540, 55], ["j", 548, 150], ["k", 540, 255],
    ["l", 660, 100], ["m", 665, 215], ["R", 755, 160, "receiver"]
  ];
  var RANGE = 140;
  function buildGraph() {
    var n = NODES.map(function (a, i) { return { id: i, name: a[0], x: a[1], y: a[2], kind: a[3] || "node", nb: [] }; });
    for (var i = 0; i < n.length; i++) for (var j = i + 1; j < n.length; j++) {
      var d = Math.hypot(n[i].x - n[j].x, n[i].y - n[j].y);
      if (d <= RANGE) { n[i].nb.push(j); n[j].nb.push(i); }
    }
    return n;
  }
  function bfsPath(g, from, to, banned) {
    var prev = {}; prev[from] = -1; var q = [from];
    while (q.length) {
      var u = q.shift();
      if (u === to) break;
      g[u].nb.forEach(function (v) {
        if (v in prev || (banned && banned.indexOf(v) >= 0)) return;
        prev[v] = u; q.push(v);
      });
    }
    if (!(to in prev)) return null;
    var p = []; for (var c = to; c !== -1; c = prev[c]) p.unshift(c);
    return p;
  }
  function floodWaves(g, from, stopAt) {
    var seen = {}; seen[from] = 0; var order = [[from]]; var cur = [from];
    while (cur.length) {
      var nxt = [];
      cur.forEach(function (u) {
        if (u === stopAt) return;
        g[u].nb.forEach(function (v) { if (!(v in seen)) { seen[v] = seen[u] + 1; nxt.push(v); } });
      });
      if (nxt.length) order.push(nxt);
      cur = nxt;
    }
    return order;
  }

  function mesh(el) {
    clear(el);
    var g = buildGraph();
    var S = 0, R = g.length - 1;
    var path = bfsPath(g, S, R); // the route a path request would find
    var hops = path.length - 1;
    var TEXT = 22; // bytes of text, as in the airtime report
    var szFlood = 18 + 64 + 1 + TEXT;                // a flooded message has no path
    var szData = 18 + 1 + 2 * hops + 64 + 1 + TEXT;   // source-routed
    var szAck = 18 + 1 + 2 * hops;
    var szReq = 18 + 1 + 16 + 5;                      // empty trail; each repeater adds 2 bytes
    var szResp = 18 + 1 + 2 * hops + 1 + 2 * hops;
    var tFlood = toa(szFlood), tData = toa(szData), tAck = toa(szAck), tReq = toa(szReq + 2), tResp = toa(szResp);

    var wrap = h("div", { "class": "mesh" });
    var ctrl = h("div", { "class": "mesh-ctrl" });
    var bFlood = h("button", { "class": "btn", type: "button" }, "Send by flood");
    var bFirst = h("button", { "class": "btn", type: "button" }, "Send by route: first message");
    var bNext = h("button", { "class": "btn", type: "button" }, "Send by route: next message");
    var bLoss = h("button", { "class": "btn", type: "button" }, "Lose a packet on hop 3");
    var bReset = h("button", { "class": "btn", type: "button" }, "Reset");
    ctrl.append(bFlood, bFirst, bNext, bLoss, bReset);

    var svg = s("svg", { viewBox: "0 0 810 320", "class": "viz-svg mesh-svg", role: "img", "aria-label": "A small mesh of fifteen nodes between a sender and a receiver" });
    var linkL = s("g"), ringL = s("g"), nodeL = s("g"), dotL = s("g");
    svg.append(linkL, ringL, nodeL, dotL);
    g.forEach(function (a) { a.nb.forEach(function (b) { if (b > a.id) linkL.appendChild(s("line", { x1: a.x, y1: a.y, x2: g[b].x, y2: g[b].y, "class": "m-link" })); }); });
    var nodeEls = g.map(function (a) {
      var c = s("g", { "class": "m-node " + a.kind, transform: "translate(" + a.x + "," + a.y + ")" });
      c.appendChild(s("circle", { r: a.kind === "node" ? 11 : 15 }));
      c.appendChild(s("text", { y: a.kind === "node" ? 4 : 5, "text-anchor": "middle" }, a.kind === "sender" ? "S" : a.kind === "receiver" ? "R" : ""));
      nodeL.appendChild(c); return c;
    });
    svg.appendChild(s("text", { x: 55, y: 196, "text-anchor": "middle", "class": "v-note" }, "sender"));
    svg.appendChild(s("text", { x: 755, y: 196, "text-anchor": "middle", "class": "v-note" }, "receiver"));

    var stats = h("div", { "class": "mesh-stats" });
    var out = h("div", { "class": "mesh-out", "aria-live": "polite" });
    var total = h("div", { "class": "mesh-total" });
    wrap.append(ctrl, svg, stats, out, total);
    el.appendChild(wrap);

    var tx = 0, air = 0, busy = false, msgs = { flood: 0, route: 0 }, routeKnown = false, cum = { flood: 0, route: 0 };
    var timers = [];
    function later(fn, ms) { timers.push(setTimeout(fn, ms)); }
    function stopAll() { timers.forEach(clearTimeout); timers = []; busy = false; clear(ringL); clear(dotL); nodeEls.forEach(function (n) { n.classList.remove("hot", "lost", "got"); }); linkL.childNodes.forEach(function (l) { l.classList.remove("on"); }); }
    function lights(i, cls) { nodeEls[i].classList.add(cls || "hot"); later(function () { nodeEls[i].classList.remove(cls || "hot"); }, 700); }
    function pulse(i, kind) {
      var c = s("circle", { cx: g[i].x, cy: g[i].y, r: 8, "class": "m-ring " + (kind || "") });
      ringL.appendChild(c);
      var t0 = performance.now();
      (function step(t) { var k = (t - t0) / 800; if (k >= 1) { c.remove(); return; } c.setAttribute("r", 8 + k * 60); c.setAttribute("opacity", 0.7 * (1 - k)); requestAnimationFrame(step); })(t0);
    }
    function hit(a, b, kind) { // a unicast hop
      var d = s("circle", { r: 5, "class": "m-dot " + (kind || "") }); dotL.appendChild(d);
      var t0 = performance.now(), D = 520;
      (function step(t) { var k = Math.min(1, (t - t0) / D); d.setAttribute("cx", g[a].x + (g[b].x - g[a].x) * k); d.setAttribute("cy", g[a].y + (g[b].y - g[a].y) * k); if (k < 1) requestAnimationFrame(step); else d.remove(); })(t0);
    }
    function count(n, ms) { tx += n; air += n * ms; paint(); }
    function paint() {
      stats.innerHTML = "";
      [["Transmissions", tx], ["Airtime used", fmt(air / 1000, " s", 2)]].forEach(function (p) {
        var b = h("div", { "class": "stat" }); b.appendChild(h("strong", {}, String(p[1]))); b.appendChild(h("span", {}, p[0])); stats.appendChild(b);
      });
      total.innerHTML = "";
      var rows = [];
      if (msgs.flood) rows.push(["Flood, " + msgs.flood + " message" + (msgs.flood > 1 ? "s" : ""), cum.flood, 1]);
      if (msgs.route) rows.push(["Route, " + msgs.route + " message" + (msgs.route > 1 ? "s" : ""), cum.route, 2]);
      if (rows.length) {
        var mx = Math.max.apply(null, rows.map(function (r) { return r[1]; }));
        rows.forEach(function (r) {
          var line = h("div", { "class": "tot-row" });
          line.appendChild(h("span", { "class": "tot-l" }, r[0]));
          var bar = h("span", { "class": "tot-b" }); var f = h("i", { "class": "c" + r[2] }); f.style.width = (100 * r[1] / mx) + "%"; bar.appendChild(f); line.appendChild(bar);
          line.appendChild(h("span", { "class": "tot-v" }, fmt(r[1] / 1000, " s", 1)));
          total.appendChild(line);
        });
      }
    }
    function say(t) { out.textContent = t; }
    function lock() { if (busy) return false; busy = true; return true; }
    function unlock() { busy = false; }

    function doFlood() {
      if (!lock()) return;
      var waves = floodWaves(g, S, R), n = 0;
      say("A flood: every node that hears the message repeats it once, so the whole mesh transmits.");
      waves.forEach(function (w, wi) {
        later(function () { w.forEach(function (u) { if (u === R) { lights(u, "got"); return; } pulse(u, "f"); lights(u); n++; }); count(w.filter(function (u) { return u !== R; }).length, tFlood); }, wi * 700);
      });
      later(function () { msgs.flood++; cum.flood += n * tFlood; paint(); say("Flood: " + n + " transmissions of " + fmt(tFlood, " ms", 0) + " each = " + fmt(n * tFlood / 1000, " s", 1) + " of airtime for this one message, and the same again for the next one."); unlock(); }, waves.length * 700 + 200);
    }
    function discover(done) {
      var waves = floodWaves(g, S, R), n = 0;
      say("No route yet. The sender floods a small path request. Each repeater adds itself to it.");
      waves.forEach(function (w, wi) {
        later(function () { w.forEach(function (u) { if (u === R) { lights(u, "got"); return; } pulse(u, "q"); lights(u); n++; }); count(w.filter(function (u) { return u !== R; }).length, tReq); }, wi * 700);
      });
      var t = waves.length * 700 + 150;
      later(function () {
        say("The receiver picked the best of the copies and answers along it. Only " + hops + " transmissions.");
        for (var i = path.length - 1; i > 0; i--) (function (k) { later(function () { hit(path[k], path[k - 1], "r"); count(1, tResp); }, (path.length - 1 - k) * 520); })(i);
      }, t);
      later(function () { routeKnown = true; cum.route += n * tReq + hops * tResp; paint(); done(); }, t + hops * 520 + 200);
    }
    function sendRoute(lose) {
      var steps = path.length - 1, ms = 560;
      say(lose ? "Data and acknowledgement follow the route. A truck blocks hop 3." : "Data follows the route. Only the nodes on it transmit, and each hop is confirmed.");
      for (var i = 0; i < steps; i++) (function (k) {
        later(function () { hit(path[k], path[k + 1], "d"); count(1, tData); lights(path[k]); }, k * ms);
      })(i);
      var lostAt = lose ? Math.min(2, steps - 1) : -1;
      if (lose) {
        later(function () { nodeEls[path[lostAt + 1]].classList.add("lost"); say("Hop 3 lost the packet. The sender of that hop heard no forward within 3 s and sends the same packet again."); }, lostAt * ms + 300);
        later(function () { nodeEls[path[lostAt + 1]].classList.remove("lost"); hit(path[lostAt], path[lostAt + 1], "d"); count(1, tData); }, lostAt * ms + 1500);
      }
      var extra = lose ? 1500 : 0;
      later(function () { lights(R, "got"); say("Delivered. The receiver sends an end-to-end acknowledgement back along the route."); }, steps * ms + extra);
      for (var j = steps; j > 0; j--) (function (k) {
        later(function () { hit(path[k], path[k - 1], "a"); count(1, tAck); }, steps * ms + extra + 300 + (steps - k) * 400);
      })(j);
      later(function () {
        msgs.route++; cum.route += steps * tData + steps * tAck + (lose ? tData : 0); paint();
        say("Route: " + steps + " data + " + steps + " acknowledgement transmissions" + (lose ? " + 1 resend of the lost hop" : "") + " = " + fmt((steps * tData + steps * tAck + (lose ? tData : 0)) / 1000, " s", 1) + ". Compare with " + fmt(n15() * tFlood / 1000, " s", 1) + " for a flood.");
        unlock();
      }, steps * ms + extra + 300 + steps * 400 + 200);
    }
    function n15() { return g.length - 1; }
    function reset() { stopAll(); tx = 0; air = 0; msgs = { flood: 0, route: 0 }; cum = { flood: 0, route: 0 }; routeKnown = false; paint(); say("Pick a button. The airtimes are real (SF9, 125 kHz, a 22-byte text message); the 15-node layout is only an illustration."); }
    bFlood.onclick = doFlood;
    bFirst.onclick = function () { if (!lock()) return; routeKnown = false; discover(function () { busy = false; lock(); sendRoute(false); }); };
    bNext.onclick = function () { if (!routeKnown) { say("There is no route yet. Press the first-message button."); return; } if (!lock()) return; sendRoute(false); };
    bLoss.onclick = function () { if (!routeKnown) { say("There is no route yet. Press the first-message button."); return; } if (!lock()) return; sendRoute(true); };
    bReset.onclick = reset;
    reset();
    var info = h("p", { "class": "v-note small" }, "Route here: " + hops + " hops. Packet sizes: flooded message " + szFlood + " B, routed message " + szData + " B, acknowledgement " + szAck + " B, path request " + szReq + " B (+2 B per repeater).");
    el.appendChild(info);
  }

  /* ---------- the role lab ---------- */
  var ROLES = {
    client: { name: "Client", fwd: "never", text: "A client sends and receives its own messages and never forwards for anyone. A phone comes and goes, so nobody may depend on it." },
    relay: { name: "Relay", fwd: "up to 4 hops", text: "A relay forwards like a repeater, but waits two slots longer so a real repeater wins, and only for packets that have gone fewer than 4 hops. It is what a handheld becomes in a forest." },
    repeater: { name: "Repeater", fwd: "yes", text: "A repeater forwards floods and routed packets. It is a fixed node that is on most of the time." },
    anchor: { name: "Anchor", fwd: "yes, and splits long paths", text: "An anchor forwards, and when a path request has gone too far it starts a new leg, so a route can be longer than 12 hops." }
  };
  var ENERGY = [
    ["Full", "3.90 V or USB power", { client: 0, relay: 1, repeater: 1, anchor: 1 }, "forwards everything"],
    ["Ok", "3.60 to 3.90 V", { relay: 1, repeater: 1, anchor: 1 }, "forwards everything, a little later in floods"],
    ["Low", "3.40 to 3.60 V", { relay: 2, repeater: 2, anchor: 2 }, "forwards only direct messages and path requests"],
    ["Critical", "below 3.40 V", { relay: 0, repeater: 0, anchor: 0 }, "forwards nothing"]
  ];
  function roleLab(el) {
    clear(el);
    var role = "repeater", en = 0, kind = "msg";
    var box = h("div", { "class": "lab" });
    var row1 = h("div", { "class": "lab-row" }), row2 = h("div", { "class": "lab-row" }), row3 = h("div", { "class": "lab-row" });
    function seg(label, items, get, set, host) {
      var l = h("span", { "class": "lab-l" }, label); host.appendChild(l);
      var grp = h("span", { "class": "seg" });
      items.forEach(function (it) {
        var b = h("button", { type: "button", "class": "btn" }, it[1]);
        b.onclick = function () { set(it[0]); paint(); };
        b._k = it[0]; grp.appendChild(b);
      });
      host.appendChild(grp); return grp;
    }
    var gRole = seg("Middle node is a", Object.keys(ROLES).map(function (k) { return [k, ROLES[k].name]; }), null, function (v) { role = v; }, row1);
    var gEn = seg("Battery", ENERGY.map(function (e, i) { return [i, e[0]]; }), null, function (v) { en = v; }, row2);
    var gKind = seg("Packet", [["msg", "Direct message (routed)"], ["flood", "Channel message (flood)"], ["req", "Path request"]], null, function (v) { kind = v; }, row3);
    var svg = s("svg", { viewBox: "0 0 760 190", "class": "viz-svg", role: "img", "aria-label": "A sender, a middle node and a receiver that cannot hear each other directly" });
    var A = [70, 95], M = [380, 95], B = [690, 95];
    svg.appendChild(s("line", { x1: A[0], y1: A[1], x2: M[0], y2: M[1], "class": "m-link on" }));
    svg.appendChild(s("line", { x1: M[0], y1: M[1], x2: B[0], y2: B[1], "class": "m-link on" }));
    svg.appendChild(s("path", { d: "M 70 150 Q 380 195 690 150", "class": "m-skip", fill: "none" }));
    svg.appendChild(s("text", { x: 380, y: 183, "text-anchor": "middle", "class": "v-note" }, "too far apart to hear each other"));
    function node(p, label, cls) {
      var g2 = s("g", { "class": "m-node " + cls, transform: "translate(" + p[0] + "," + p[1] + ")" });
      g2.appendChild(s("circle", { r: 20 })); g2.appendChild(s("text", { y: 5, "text-anchor": "middle" }, label)); svg.appendChild(g2); return g2;
    }
    node(A, "S", "sender"); var mNode = node(M, "?", "mid"); node(B, "R", "receiver");
    var verdict = h("div", { "class": "lab-verdict", "aria-live": "polite" });
    var detail = h("p", { "class": "lab-text" });
    var play = h("button", { type: "button", "class": "btn lab-play" }, "Send it");
    box.append(row1, row2, row3, svg, play, verdict, detail); el.appendChild(box);
    var dot = null, timer = null;
    function outcome() {
      var e = ENERGY[en][2][role]; if (e == null) e = (role === "client" ? 0 : 1);
      if (role === "client") return { ok: false, why: "A client never forwards. The message stops at the middle node, so S and R cannot talk through it." };
      if (e === 0) return { ok: false, why: "The battery is critical (below 3.40 V). The node forwards nothing, so it protects its last charge. The route is repaired another way or the message waits." };
      if (e === 2 && kind === "flood") return { ok: false, why: "At low battery a node forwards only direct messages and path requests. A channel flood stops here." };
      var late = role === "relay" ? "It waits two slots (about 400 ms at SF9) before sending, so a real repeater would have gone first." : (en === 1 ? "It waits a little longer than a full node, about 150 ms." : "");
      var extra = "";
      if (role === "relay") extra = " It forwards only while the packet has gone fewer than 4 hops.";
      if (role === "anchor" && kind === "req") extra = " If the path request has gone far already, the anchor starts a new leg here instead of dropping it.";
      return { ok: true, why: "The middle node forwards the packet. " + late + extra };
    }
    function paint() {
      [gRole, gEn, gKind].forEach(function (grp, gi) { Array.prototype.forEach.call(grp.children, function (b) { var sel = gi === 0 ? role : gi === 1 ? en : kind; b.setAttribute("aria-pressed", String(b._k === sel)); }); });
      mNode.lastChild.textContent = ROLES[role].name.charAt(0);
      var o = outcome();
      verdict.className = "lab-verdict " + (o.ok ? "ok" : "no");
      verdict.textContent = o.ok ? "Gets through" : "Stops at the middle node";
      detail.textContent = ROLES[role].text + " Battery: " + ENERGY[en][0] + " (" + ENERGY[en][1] + "), " + ENERGY[en][3] + ". " + o.why;
    }
    play.onclick = function () {
      if (dot) { dot.remove(); clearTimeout(timer); }
      var o = outcome();
      dot = s("circle", { r: 7, cx: A[0], cy: A[1], "class": "m-dot d" }); svg.appendChild(dot);
      var t0 = performance.now(), toMid = 1000, wait = role === "relay" ? 700 : 150, toEnd = 1000;
      (function step(t) {
        var e = t - t0, x;
        if (e < toMid) x = A[0] + (M[0] - A[0]) * e / toMid;
        else if (!o.ok) { dot.setAttribute("class", "m-dot lost"); return; }
        else if (e < toMid + wait) x = M[0];
        else if (e < toMid + wait + toEnd) x = M[0] + (B[0] - M[0]) * (e - toMid - wait) / toEnd;
        else { dot.setAttribute("class", "m-dot got"); return; }
        dot.setAttribute("cx", x); requestAnimationFrame(step);
      })(t0);
    };
    paint();
  }

  /* ---------- a played step list ---------- */
  function timeline(el, steps) {
    clear(el);
    var wrap = h("div", { "class": "tl" });
    var list = h("ol", { "class": "tl-list" });
    var items = steps.map(function (st, i) {
      var li = h("li", { "class": "tl-item" });
      li.appendChild(h("span", { "class": "tl-t" }, st.t));
      var body = h("div", { "class": "tl-body" });
      body.appendChild(h("strong", {}, st.title));
      body.appendChild(h("span", {}, st.text));
      if (st.pkt) body.appendChild(h("code", { "class": "tl-pkt" }, st.pkt));
      li.appendChild(body);
      li.onclick = function () { show(i); };
      list.appendChild(li); return li;
    });
    var ctl = h("div", { "class": "tl-ctl" });
    var pb = h("button", { type: "button", "class": "btn" }, "Play");
    var pv = h("button", { type: "button", "class": "btn" }, "Back");
    var nx = h("button", { type: "button", "class": "btn" }, "Next");
    ctl.append(pv, pb, nx);
    wrap.append(ctl, list); el.appendChild(wrap);
    var cur = -1, tm = null;
    function show(i) { cur = Math.max(0, Math.min(steps.length - 1, i)); items.forEach(function (li, k) { li.classList.toggle("on", k === cur); li.classList.toggle("done", k < cur); }); }
    function stop() { clearInterval(tm); tm = null; pb.textContent = "Play"; }
    pb.onclick = function () { if (tm) { stop(); return; } if (cur >= steps.length - 1) cur = -1; pb.textContent = "Pause"; show(cur + 1); tm = setInterval(function () { if (cur >= steps.length - 1) { stop(); return; } show(cur + 1); }, 1800); };
    nx.onclick = function () { stop(); show(cur + 1); };
    pv.onclick = function () { stop(); show(cur - 1); };
    show(0);
  }


  /* ---------- radio areas joined by the backbone ---------- */
  function backbone(el) {
    clear(el);
    var wrap = h("div", { "class": "mesh" });
    var ctrl = h("div", { "class": "mesh-ctrl" });
    var b1 = h("button", { "class": "btn", type: "button" }, "Send A to B with the DHT");
    var b2 = h("button", { "class": "btn", type: "button" }, "Same message if every area repeated it");
    var br = h("button", { "class": "btn", type: "button" }, "Reset");
    ctrl.append(b1, b2, br);
    var svg = s("svg", { viewBox: "0 0 780 360", "class": "viz-svg mesh-svg", role: "img", "aria-label": "Three radio areas joined by an internet backbone" });
    var areas = [
      { name: "Area A", host: [120, 215], nodes: [[45, 150], [100, 120], [175, 150], [55, 270], [185, 275]], tx: 0 },
      { name: "Area B", host: [660, 215], nodes: [[585, 150], [640, 120], [715, 150], [600, 275], [730, 270]], tx: 0 },
      { name: "Area C", host: [390, 285], nodes: [[330, 250], [450, 250], [345, 318], [440, 318]], tx: 0 }
    ];
    var cloud = [390, 60];
    svg.appendChild(s("ellipse", { cx: cloud[0], cy: cloud[1], rx: 120, ry: 38, "class": "bb-cloud" }));
    svg.appendChild(s("text", { x: cloud[0], y: cloud[1] - 2, "text-anchor": "middle", "class": "v-group" }, "Internet backbone"));
    svg.appendChild(s("text", { x: cloud[0], y: cloud[1] + 16, "text-anchor": "middle", "class": "v-note" }, "TCP links and the DHT lookup table"));
    var ripL = s("g"), dotL = s("g"); var nodeEls = [];
    areas.forEach(function (a, ai) {
      var zone = s("circle", { cx: a.host[0], cy: a.host[1] - 5 + (ai === 2 ? -20 : 0), r: ai === 2 ? 78 : 112, "class": "bb-zone" }); svg.appendChild(zone);
      a.nodes.forEach(function (n) { svg.appendChild(s("line", { x1: n[0], y1: n[1], x2: a.host[0], y2: a.host[1], "class": "m-link" })); });
      svg.appendChild(s("line", { x1: a.host[0], y1: a.host[1], x2: cloud[0] + (ai - 1) * 90, y2: cloud[1] + 36, "class": "m-link on" }));
      svg.appendChild(s("text", { x: a.host[0], y: a.host[1] + (ai === 2 ? 52 : 82), "text-anchor": "middle", "class": "v-group" }, a.name));
    });
    svg.append(ripL);
    areas.forEach(function (a) {
      a.els = a.nodes.map(function (n) { var g = s("g", { "class": "m-node", transform: "translate(" + n[0] + "," + n[1] + ")" }); g.appendChild(s("circle", { r: 9 })); svg.appendChild(g); return g; });
      var hg = s("g", { "class": "m-node host", transform: "translate(" + a.host[0] + "," + a.host[1] + ")" }); hg.appendChild(s("rect", { x: -14, y: -14, width: 28, height: 28, rx: 5 })); hg.appendChild(s("text", { y: 5, "text-anchor": "middle" }, "H")); svg.appendChild(hg); a.hostEl = hg;
    });
    svg.appendChild(dotL);
    var stats = h("div", { "class": "mesh-stats" });
    var out = h("div", { "class": "mesh-out", "aria-live": "polite" });
    wrap.append(ctrl, svg, stats, out); el.appendChild(wrap);
    var timers = [], busy = false;
    function later(f, ms) { timers.push(setTimeout(f, ms)); }
    function ring(x, y) {
      var c = s("circle", { cx: x, cy: y, r: 8, "class": "m-ring f" }); ripL.appendChild(c);
      var t0 = performance.now(); (function st(t) { var k = (t - t0) / 800; if (k >= 1) { c.remove(); return; } c.setAttribute("r", 8 + k * 55); c.setAttribute("opacity", .7 * (1 - k)); requestAnimationFrame(st); })(t0);
    }
    function dot(p, q, cls, ms) {
      var d = s("circle", { r: 6, "class": "m-dot " + (cls || "") }); dotL.appendChild(d);
      var t0 = performance.now(); (function st(t) { var k = Math.min(1, (t - t0) / ms); d.setAttribute("cx", p[0] + (q[0] - p[0]) * k); d.setAttribute("cy", p[1] + (q[1] - p[1]) * k); if (k < 1) requestAnimationFrame(st); else d.remove(); })(t0);
    }
    function paint() {
      stats.innerHTML = "";
      areas.forEach(function (a) { var b = h("div", { "class": "stat" }); b.appendChild(h("strong", {}, String(a.tx))); b.appendChild(h("span", {}, "Radio transmissions in " + a.name)); stats.appendChild(b); });
    }
    function say(t) { out.textContent = t; }
    function speak(ai, n, ms) { // radio transmissions inside one area
      var a = areas[ai]; var k = Math.min(n, a.nodes.length);
      for (var i = 0; i < k; i++) (function (i) { later(function () { ring(a.nodes[i][0], a.nodes[i][1]); a.tx++; paint(); }, i * 250); })(i);
    }
    function reset() { timers.forEach(clearTimeout); timers = []; busy = false; areas.forEach(function (a) { a.tx = 0; }); ripL.innerHTML = ""; dotL.innerHTML = ""; paint(); say("Press a button. The layout is an illustration."); }
    b1.onclick = function () {
      if (busy) return; reset(); busy = true;
      var A = areas[0], B = areas[1];
      say("1. The sender's radio reaches the host of its own area.");
      speak(0, 1); later(function () { ring(A.host[0], A.host[1]); A.tx++; paint(); dot(A.nodes[0], A.host, "d", 500); }, 300);
      later(function () { say("2. The host asks the DHT which host can reach the recipient. Only a lookup on the internet, no radio."); dot(A.host, [cloud[0] - 90, cloud[1] + 36], "r", 700); }, 1500);
      later(function () { dot([cloud[0] - 90, cloud[1] + 36], A.host, "r", 700); }, 2300);
      later(function () { say("3. The message goes straight to that host over TCP. Area C is not asked."); dot(A.host, [cloud[0] - 90, cloud[1] + 36], "d", 600); }, 3200);
      later(function () { dot([cloud[0] - 90, cloud[1] + 36], [cloud[0] + 90, cloud[1] + 36], "d", 700); }, 3800);
      later(function () { dot([cloud[0] + 90, cloud[1] + 36], B.host, "d", 600); }, 4500);
      later(function () { say("4. Only area B's radio is woken, once, to reach the recipient. Area C used no airtime at all."); ring(B.host[0], B.host[1]); B.tx++; paint(); }, 5200);
      later(function () { ring(B.nodes[2][0], B.nodes[2][1]); B.tx++; paint(); busy = false; }, 5600);
    };
    b2.onclick = function () {
      if (busy) return; reset(); busy = true;
      say("Without a lookup, every connected area repeats the message into its own radios.");
      areas.forEach(function (a, ai) { later(function () { ring(a.host[0], a.host[1]); a.tx++; paint(); }, 200 + ai * 150); speak(ai, a.nodes.length, 0); });
      later(function () { busy = false; say("Every area transmits, including the one that has nothing to do with the message. That is what the DHT avoids."); }, 1800);
    };
    br.onclick = reset;
    reset();
  }

  window.Viz = { backbone: backbone, toa: toa, bars: bars, stack: stack, frames: frames, mesh: mesh, roleLab: roleLab, timeline: timeline };
})();
