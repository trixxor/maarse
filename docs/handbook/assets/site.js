/* Må:rse handbook shell: sidebar, theme, deep-dive toggle, copy buttons, prev/next.
   The pages are plain HTML and fully readable without this script (all deep dives show). */
(function () {
  "use strict";

  // Every page of the handbook: [file, title, group, keywords]
  var PAGES = [
    ["index.html", "Start here", "Start", "home overview reader paths status legend"],
    ["what-is-marse.html", "What is Må:rse?", "Start", "tldr why pros cons compare meshtastic meshcore reticulum"],
    ["get-started.html", "Get started", "Start", "install host flash board android first node web client"],
    ["roles.html", "Roles and devices", "Roles and devices", "client relay repeater anchor host server role lab auto forest"],
    ["role-client.html", "Client node", "Roles and devices", "phone handheld never forwards battery quiet"],
    ["role-relay.html", "Relay", "Roles and devices", "forest mode edge bridge carry auto handheld"],
    ["role-repeater.html", "Repeater", "Roles and devices", "fixed node roof hill mast solar battery manage administrator"],
    ["role-anchor.html", "Anchor", "Roles and devices", "long route legs 12 hops backbone motorway"],
    ["role-host.html", "Host", "Roles and devices", "pc phone service gateway web client post office wifi modem"],
    ["role-server.html", "Server", "Roles and devices", "maarse-server dht relay nat dashboard vps debian"],
    ["hardware.html", "Radio boards", "Roles and devices", "heltec v4 xiao nrf52840 esp32 wismesh tag battery flash"],
    ["how-it-works.html", "How messages travel", "Understand", "routing path request flood per hop ack backbone dht packet airtime"],
    ["mailboxes-and-carry.html", "Mailboxes and carry", "Understand", "post office offline deposit carry forest relay delivered notice"],
    ["privacy-and-security.html", "Privacy and security", "Understand", "encryption identity key seal ratchet metadata threat trust boundary"],
    ["architecture.html", "Architecture", "Understand", "crates host node client firmware layers roles anchor repeater"],
    ["messages.html", "Messages", "Features", "direct message delivered ratchet compression receipt overhead"],
    ["channels-and-regions.html", "Channels and regions", "Features", "public hashtag private region key group"],
    ["files-and-voice.html", "Files, pictures and voice", "Features", "file picture codec2 voice call opus chunk limit"],
    ["pages.html", "Pages", "Features", "maarse:// micron page bbs board"],
    ["alerts.html", "Public alerts", "Features", "warning bulletin issuer certificate cell broadcast"],
    ["sensors.html", "Sensor nodes", "Features", "bme280 recipe designer alarm owner sleep"],
    ["linked-devices.html", "Linked devices", "Features", "account master phone pc one identity"],
    ["backbone-and-trunks.html", "Backbone and trunks", "Features", "tcp udp dht trunk kiss wifi internet link"],
    ["meshcore.html", "MeshCore", "Features", "gateway bridge access identity"],
    ["apps.html", "Apps and clients", "Features", "android web client pwa windows tray service languages"],
    ["operations.html", "Operations and diagnostics", "Features", "firmware update ping crawl dht check range test status mqtt home assistant"],
    ["simulation.html", "Simulation results", "Evidence", "sim denmark mesh lab numbers airtime delivered carry mail trunks"],
    ["status.html", "What works today", "Evidence", "status released working copy simulated bench field verification"],
    ["decisions.html", "Decisions", "Evidence", "adr rejected shelved measured ota load control"],
    ["glossary.html", "Glossary", "Evidence", "lora sf airtime duty cycle hop flood path dht announce"]
  ];

  var root = document.documentElement;
  var here = location.pathname.split("/").pop() || "index.html";

  // Theme
  var theme = localStorage.getItem("maarse-docs-theme");
  if (!theme) theme = matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
  root.setAttribute("data-theme", theme);

  // Deep dives: hidden by default for a simple first read, remembered per browser
  var detail = localStorage.getItem("maarse-docs-detail") || "off";
  root.setAttribute("data-detail", detail);

  function el(tag, attrs, text) {
    var e = document.createElement(tag);
    for (var k in (attrs || {})) e.setAttribute(k, attrs[k]);
    if (text) e.textContent = text;
    return e;
  }

  // Top bar
  var bar = el("div", { "class": "topbar" });
  var menu = el("button", { "class": "btn menu-btn", type: "button", "aria-label": "Menu" }, "Menu");
  menu.onclick = function () {
    root.setAttribute("data-menu", root.getAttribute("data-menu") === "open" ? "" : "open");
  };
  var brand = el("a", { "class": "brand", href: "index.html" }, "Må:rse");
  brand.appendChild(el("small", {}, "handbook"));
  var dBtn = el("button", { "class": "btn", type: "button", title: "Show or hide the detailed sections" });
  var tBtn = el("button", { "class": "btn", type: "button", title: "Light or dark" });
  function paintDetail() {
    var on = root.getAttribute("data-detail") === "on";
    dBtn.setAttribute("aria-pressed", on ? "true" : "false");
    dBtn.textContent = on ? "Deep dives: shown" : "Deep dives: hidden";
  }
  function paintTheme() { tBtn.textContent = root.getAttribute("data-theme") === "dark" ? "Light" : "Dark"; }
  dBtn.onclick = function () {
    var next = root.getAttribute("data-detail") === "on" ? "off" : "on";
    root.setAttribute("data-detail", next);
    localStorage.setItem("maarse-docs-detail", next);
    paintDetail();
  };
  tBtn.onclick = function () {
    var next = root.getAttribute("data-theme") === "dark" ? "light" : "dark";
    root.setAttribute("data-theme", next);
    localStorage.setItem("maarse-docs-theme", next);
    paintTheme();
  };
  paintDetail(); paintTheme();
  bar.append(menu, brand, el("span", { "class": "spacer" }), dBtn, tBtn);

  // Sidebar
  var side = el("nav", { "class": "sidebar", "aria-label": "Handbook" });
  var search = el("input", { type: "search", placeholder: "Filter pages", "aria-label": "Filter pages" });
  side.appendChild(search);
  var groups = {};
  var items = [];
  PAGES.forEach(function (p) {
    if (!groups[p[2]]) {
      var h = el("h2", {}, p[2]);
      var ul = el("ul");
      side.append(h, ul);
      groups[p[2]] = ul;
    }
    var li = el("li");
    var a = el("a", { href: p[0] }, p[1]);
    if (p[0] === here) a.setAttribute("aria-current", "page");
    li.appendChild(a);
    groups[p[2]].appendChild(li);
    items.push({ li: li, hay: (p[1] + " " + p[3]).toLowerCase() });
  });
  search.oninput = function () {
    var q = search.value.trim().toLowerCase();
    items.forEach(function (i) { i.li.hidden = q && i.hay.indexOf(q) < 0; });
  };

  // Layout: wrap the existing <main>
  var main = document.querySelector("main");
  var layout = el("div", { "class": "layout" });
  main.parentNode.insertBefore(bar, main.parentNode.firstChild);
  main.parentNode.insertBefore(layout, main);
  layout.append(side, main);

  // Hint shown while deep dives are hidden
  var deeps = document.querySelectorAll(".deep");
  if (deeps.length) {
    var hint = el("div", { "class": "detail-hint" });
    hint.textContent = "This page has " + deeps.length + " deep-dive section" + (deeps.length > 1 ? "s" : "") + " that are hidden. ";
    var show = el("a", { href: "#" }, "Show them");
    show.onclick = function (e) { e.preventDefault(); dBtn.click(); };
    hint.appendChild(show);
    hint.appendChild(document.createTextNode("."));
    var art = document.querySelector("article");
    var tl = art.querySelector(".tldr");
    art.insertBefore(hint, tl ? tl.nextSibling : null);
  }

  // Open a deep dive automatically when the URL points inside it
  function revealTarget() {
    var id = location.hash.slice(1);
    if (!id) return;
    var t = document.getElementById(id);
    if (t && t.closest(".deep") && root.getAttribute("data-detail") === "off") {
      root.setAttribute("data-detail", "on");
      paintDetail();
      t.scrollIntoView();
    }
  }
  window.addEventListener("hashchange", revealTarget);
  revealTarget();

  // Copy buttons
  document.querySelectorAll("pre").forEach(function (pre) {
    if (pre.classList.contains("nocopy")) return;
    var b = el("button", { "class": "btn copy", type: "button" }, "Copy");
    b.onclick = function () {
      var txt = pre.querySelector("code") ? pre.querySelector("code").innerText : pre.innerText;
      navigator.clipboard.writeText(txt).then(function () {
        b.textContent = "Copied";
        setTimeout(function () { b.textContent = "Copy"; }, 1500);
      });
    };
    pre.appendChild(b);
  });

  // Previous / next
  var idx = PAGES.findIndex(function (p) { return p[0] === here; });
  if (idx >= 0) {
    var pager = el("div", { "class": "pager" });
    var prev = idx > 0 ? PAGES[idx - 1] : null;
    var next = idx < PAGES.length - 1 ? PAGES[idx + 1] : null;
    var l = el("span"), r = el("span");
    if (prev) { var pa = el("a", { href: prev[0] }, "← " + prev[1]); l.appendChild(pa); }
    if (next) { var na = el("a", { href: next[0] }, next[1] + " →"); r.appendChild(na); }
    pager.append(l, r);
    document.querySelector("article").appendChild(pager);
  }
})();
