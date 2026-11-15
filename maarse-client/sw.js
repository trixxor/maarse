// Service worker for the Må:rse web client: caches the app shell so it can be opened offline.
// Bump VERSION when the files change; the client then shows "New version – reload".
"use strict";
const VERSION = "maarse-v1";
const SHELL = ["./", "index.html", "manifest.webmanifest", "icon.svg", "icon-192.png", "icon-512.png"];

self.addEventListener("install", e => {
  // No skipWaiting here: the new version waits until the user presses "reload".
  e.waitUntil(caches.open(VERSION).then(c => c.addAll(SHELL.map(u => new Request(u, { cache: "reload" })))));
});

self.addEventListener("activate", e => {
  e.waitUntil(caches.keys()
    .then(keys => Promise.all(keys.filter(k => k.startsWith("maarse-") && k !== VERSION).map(k => caches.delete(k))))
    .then(() => self.clients.claim()));
});

self.addEventListener("message", e => { if (e.data === "skipWaiting") self.skipWaiting(); });

self.addEventListener("fetch", e => {
  const req = e.request;
  const url = new URL(req.url);
  if (req.method !== "GET" || url.origin !== location.origin || url.pathname.endsWith("/ws")) return;
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
