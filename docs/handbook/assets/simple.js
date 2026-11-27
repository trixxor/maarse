/* Simple role pictures: one tiny looping story per role. No controls, no numbers.
   <div class="simple" data-role="client|relay|repeater|anchor"></div> */
(function () {
  "use strict";
  var NS = "http://www.w3.org/2000/svg", LOOP = 8;
  function s(tag, a, text) { var e = document.createElementNS(NS, tag); for (var k in (a || {})) e.setAttribute(k, a[k]); if (text != null) e.textContent = text; return e; }
  function v(n) { return getComputedStyle(document.documentElement).getPropertyValue(n).trim(); }
  function lerp(a, b, t) { return a + (b - a) * Math.max(0, Math.min(1, t)); }
  function ease(t) { t = Math.max(0, Math.min(1, t)); return t * t * (3 - 2 * t); }

  var SCENES = {
    client: { color: "--c4", name: "Client", tag: "your phone",
      steps: [[0, "You send a message to your friend."], [2.2, "The phone in the middle is a client. It does not pass messages on."], [5.2, "So the message stops here. A client only looks after its own."]] },
    relay: { color: "--c2", name: "Relay", tag: "a handheld that helps out",
      steps: [[0, "You send a message to your friend."], [2.2, "The relay waits a moment, in case a repeater is near."], [4.6, "Then it passes the message on."], [6.4, "Delivered."]] },
    repeater: { color: "--c1", name: "Repeater", tag: "a tower on a hill",
      steps: [[0, "You send a message to your friend."], [2.2, "The repeater passes it on straight away."], [5, "Delivered. It does this all day, for everybody."]] },
    anchor: { color: "--c3", name: "Anchor", tag: "links far away places",
      steps: [[0, "Your friend is in another town, far beyond the radio."], [1.8, "The anchor is on the internet. It takes the message there."], [4.6, "And hands it to a radio in your friend's town."], [6.6, "Delivered."]] }
  };

  function person(g, x, y, col, label, C) {
    var p = s("g", { transform: "translate(" + x + "," + y + ")" });
    p.appendChild(s("circle", { cx: 0, cy: -22, r: 11, fill: col }));
    p.appendChild(s("path", { d: "M-17 18 Q-17 -6 0 -6 Q17 -6 17 18 Z", fill: col }));
    p.appendChild(s("rect", { x: 10, y: -2, width: 9, height: 15, rx: 2, fill: C.panel, stroke: col, "stroke-width": 2 }));
    p.appendChild(s("text", { x: 0, y: 40, "text-anchor": "middle", "class": "sp-label" }, label));
    g.appendChild(p); return p;
  }
  function tower(g, x, y, col, label, C, waves) {
    var p = s("g", { transform: "translate(" + x + "," + y + ")" });
    p.appendChild(s("path", { d: "M0 -26 L16 18 L-16 18 Z", fill: col }));
    p.appendChild(s("path", { d: "M-8 18 L0 -4 L8 18", fill: "none", stroke: C.panel, "stroke-width": 2 }));
    if (waves !== false) [12, 22].forEach(function (r, i) { p.appendChild(s("path", { d: "M" + (-r * .75) + " " + (-26 - r * .35) + " A" + r + " " + r + " 0 0 1 " + (r * .75) + " " + (-26 - r * .35), fill: "none", stroke: col, "stroke-width": 2.4, opacity: .75 - i * .25, "stroke-linecap": "round" })); });
    p.appendChild(s("text", { x: 0, y: 40, "text-anchor": "middle", "class": "sp-label" }, label));
    g.appendChild(p); return p;
  }
  function envelope(parent, C) {
    var e = s("g"); e.appendChild(s("rect", { x: -13, y: -9, width: 26, height: 18, rx: 3, fill: "#fff", stroke: C.text, "stroke-width": 2 }));
    e.appendChild(s("path", { d: "M-13 -9 L0 3 L13 -9", fill: "none", stroke: C.text, "stroke-width": 2, "stroke-linejoin": "round" })); parent.appendChild(e); return e;
  }

  function init(host) {
    var kind = host.getAttribute("data-role"), def = SCENES[kind]; if (!def) return;
    var C = { panel: v("--panel"), text: v("--text"), muted: v("--muted"), line: v("--line"), good: v("--good"), bad: v("--bad"), role: v(def.color) };
    var W = 640, H = 200, svg = s("svg", { viewBox: "0 0 " + W + " " + H, "class": "sp-svg", role: "img", "aria-label": def.name + ": " + def.tag });
    var cap = document.createElement("div"); cap.className = "sp-cap";
    var head = document.createElement("div"); head.className = "sp-head"; head.innerHTML = "<strong>" + def.name + "</strong> <span>" + def.tag + "</span>";
    host.innerHTML = ""; host.append(head, svg, cap);
    var env, mark, mid, A, B, toMid, toEnd, hold = 0;

    if (kind === "anchor") {
      // two towns, an anchor on the internet in between
      [[70, 100, 118], [W - 70, 100, 118]].forEach(function (c) { svg.appendChild(s("circle", { cx: c[0], cy: 112, r: 62, fill: "none", stroke: C.line, "stroke-width": 2, "stroke-dasharray": "5 6" })); });
      svg.appendChild(s("text", { x: 70, y: 188, "text-anchor": "middle", "class": "sp-note" }, "your town"));
      svg.appendChild(s("text", { x: W - 70, y: 188, "text-anchor": "middle", "class": "sp-note" }, "your friend’s town"));
      svg.appendChild(s("ellipse", { cx: W / 2, cy: 46, rx: 78, ry: 24, fill: "none", stroke: C.line, "stroke-width": 2 }));
      svg.appendChild(s("text", { x: W / 2, y: 50, "text-anchor": "middle", "class": "sp-note" }, "the internet"));
      A = person(svg, 70, 112, v("--c4"), "You", C); B = person(svg, W - 70, 112, v("--c4"), "Friend", C);
      mid = tower(svg, 190, 108, C.role, "Anchor", C); tower(svg, W - 190, 108, v("--c1"), "Repeater", C, true);
      svg.appendChild(s("path", { d: "M190 82 Q" + W / 2 + " 20 " + (W - 190) + " 82", fill: "none", stroke: C.line, "stroke-width": 2, "stroke-dasharray": "4 5" }));
      env = envelope(svg, C); mark = s("text", { x: W - 70, y: 66, "text-anchor": "middle", "class": "sp-mark" }, ""); svg.appendChild(mark);
    } else {
      svg.appendChild(s("path", { d: "M70 150 Q" + W / 2 + " 196 " + (W - 70) + " 150", fill: "none", stroke: C.line, "stroke-width": 2, "stroke-dasharray": "5 6" }));
      svg.appendChild(s("text", { x: W / 2, y: 190, "text-anchor": "middle", "class": "sp-note" }, "too far to reach each other"));
      A = person(svg, 90, 100, v("--c4"), "You", C); B = person(svg, W - 90, 100, v("--c4"), "Friend", C);
      if (kind === "repeater") mid = tower(svg, W / 2, 96, C.role, "Repeater", C);
      else mid = person(svg, W / 2, 100, C.role, def.name, C);
      if (kind === "relay") { var ring = s("circle", { cx: W / 2, cy: 100, r: 30, fill: "none", stroke: C.role, "stroke-width": 2.5, "stroke-dasharray": "4 4" }); svg.insertBefore(ring, mid); }
      env = envelope(svg, C); mark = s("text", { x: W - 90, y: 54, "text-anchor": "middle", "class": "sp-mark" }, ""); svg.appendChild(mark);
    }
    var clock = s("text", { x: W / 2, y: 58, "text-anchor": "middle", "class": "sp-mark", fill: C.muted }, ""); svg.appendChild(clock);

    function frame(t) {
      var x, y, op = 1, m = "", mc = C.good, ck = "", pulse = 1;
      var ax = A.transform.baseVal.getItem(0).matrix.e + 22, bx = B.transform.baseVal.getItem(0).matrix.e - 22, mx = W / 2, my = kind === "repeater" ? 84 : 92;
      if (kind === "client") {
        if (t < 2) { x = lerp(ax, mx - 26, ease(t / 2)); y = my; }
        else if (t < 5.2) { x = mx - 26; y = my; ck = "✕"; mc = C.bad; if (t > 2.2) pulse = 1 + 0.12 * Math.sin((t - 2.2) * 12) * Math.max(0, 1 - (t - 2.2)); }
        else { x = mx - 26; y = my; ck = "✕"; op = Math.max(0, 1 - (t - 5.2) / 1.2); }
        if (t > 5.2) { m = "?"; mc = C.muted; }
        if (t > 2.2) m = t > 5.2 ? "?" : "";
      } else if (kind === "relay") {
        if (t < 2) { x = lerp(ax, mx - 26, ease(t / 2)); y = my; }
        else if (t < 4.6) { x = mx - 26; y = my; ck = t < 3.1 ? "⏳" : t < 3.9 ? "⏳" : "⏳"; }
        else if (t < 6.4) { x = lerp(mx + 26, bx, ease((t - 4.6) / 1.6)); y = my; if (t < 4.7) x = mx + 26; }
        else { x = bx; y = my; m = "✓"; }
      } else if (kind === "repeater") {
        if (t < 2) { x = lerp(ax, mx - 22, ease(t / 2)); y = my + 4; }
        else if (t < 4.6) { x = lerp(mx + 22, bx, ease((t - 2.1) / 2.5)); y = my + 4; }
        else { x = bx; y = my + 4; m = "✓"; }
        if (t > 2 && t < 2.5) pulse = 1.2;
      } else { // anchor
        var ax2 = ax, p1 = 190 - 20, p2 = W - 190 + 20;
        if (t < 1.8) { x = lerp(ax2, p1, ease(t / 1.8)); y = 108; }
        else if (t < 4.6) { var u = ease((t - 1.8) / 2.8); x = lerp(190, W - 190, u); y = 82 - 56 * Math.sin(Math.PI * u); }
        else if (t < 6.4) { x = lerp(p2, bx, ease((t - 4.6) / 1.8)); y = 108; }
        else { x = bx; y = 108; m = "✓"; }
      }
      if (kind === "repeater" || kind === "relay" || kind === "client" || kind === "anchor") env.setAttribute("transform", "translate(" + x + "," + y + ") scale(" + pulse + ")");
      env.setAttribute("opacity", op);
      mark.textContent = m; mark.setAttribute("fill", mc);
      clock.textContent = ck; clock.setAttribute("fill", mc);
      var txt = def.steps[0][1]; def.steps.forEach(function (st) { if (t >= st[0]) txt = st[1]; }); if (cap.textContent !== txt) cap.textContent = txt;
    }
    var reduce = window.matchMedia && matchMedia("(prefers-reduced-motion: reduce)").matches, t0 = performance.now(), vis = true;
    if (typeof IntersectionObserver !== "undefined") new IntersectionObserver(function (es) { vis = es[0].isIntersecting; }, { threshold: .1 }).observe(host);
    if (reduce) { frame(7); return; }
    (function tick(now) { if (vis && !document.hidden) frame(((now - t0) / 1000) % LOOP); requestAnimationFrame(tick); })(t0);
  }
  window.SimpleRoles = { init: init };
  function all() { document.querySelectorAll(".simple[data-role]").forEach(init); }
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", all); else all();
})();
