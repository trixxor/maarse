// Service worker for the Må:rse web client: caches the app shell so it can be opened offline.
// Bump VERSION when the files change; the client then shows "New version – reload".
"use strict";
const VERSION = "maarse-v13";
// The map libraries, style, fonts and sprites are cached on their own, so a new app version keeps them. The map file itself
// (denmark.pmtiles, read with Range requests, answers 206 which cannot be cached) and /api/map/ are never cached.
const MAP_CACHE = "maarse-map-v1";
const MAP_NOCACHE = ["/map/denmark.pmtiles", "/map/installed.json"];
const SHELL = ["./", "index.html", "i18n.js", "manifest.webmanifest", "icon.svg", "icon-192.png", "icon-512.png"];

self.addEventListener("install", e => {
  // No skipWaiting here: the new version waits until the user presses "reload".
  e.waitUntil(caches.open(VERSION).then(c => c.addAll(SHELL.map(u => new Request(u, { cache: "reload" })))));
});

self.addEventListener("activate", e => {
  e.waitUntil(caches.keys()
    .then(keys => Promise.all(keys.filter(k => k.startsWith("maarse-") && k !== VERSION && k !== MAP_CACHE).map(k => caches.delete(k))))
    .then(() => self.clients.claim()));
});

self.addEventListener("message", e => { if (e.data === "skipWaiting") self.skipWaiting(); });

self.addEventListener("fetch", e => {
  const req = e.request;
  const url = new URL(req.url);
  if (req.method !== "GET" || url.origin !== location.origin || url.pathname.endsWith("/ws")) return;
  if (url.pathname.startsWith("/api/")) return;   // never cached, also not /api/map/
  if (url.pathname.startsWith("/map/")) {
    if (req.headers.has("range") || MAP_NOCACHE.includes(url.pathname)) return;
    // Network first so a new host version wins; the cached copy is used when the host cannot be reached.
    e.respondWith(fetch(req).then(res => {
      if (res.ok && res.status === 200) { const copy = res.clone(); caches.open(MAP_CACHE).then(c => c.put(req, copy)).catch(() => {}); }
      return res;
    }).catch(() => caches.match(req, { cacheName: MAP_CACHE }).then(r => r || Response.error())));
    return;
  }
  if (req.mode === "navigate") {
    // Network first, so an updated host always wins; otherwise the cached shell.
    e.respondWith(fetch(req).catch(() => caches.match("index.html", { cacheName: VERSION })));
    return;
  }
  e.respondWith(caches.match(req, { cacheName: VERSION }).then(r => r || fetch(req)));
});

// Click on a notification: bring the window to the front.
self.addEventListener("notificationclick", e => {
  e.notification.close();
  e.waitUntil(self.clients.matchAll({ type: "window", includeUncontrolled: true }).then(list => {
    for (const c of list) if ("focus" in c) return c.focus();
    return self.clients.openWindow("./");
  }));
});
