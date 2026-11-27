// Offline shell cache. Speech recognition itself still needs the internet.
var CACHE = "sokhan-pwa-v1";
var ASSETS = [
  "./",
  "index.html",
  "style.css",
  "app.js",
  "halfspace.js",
  "manifest.webmanifest",
  "icon-512.png",
  "apple-touch-icon.png",
];

self.addEventListener("install", function (e) {
  e.waitUntil(
    caches
      .open(CACHE)
      .then(function (c) {
        return c.addAll(ASSETS);
      })
      .then(function () {
        return self.skipWaiting();
      })
  );
});

self.addEventListener("activate", function (e) {
  e.waitUntil(
    caches
      .keys()
      .then(function (keys) {
        return Promise.all(
          keys
            .filter(function (k) {
              return k !== CACHE;
            })
            .map(function (k) {
              return caches.delete(k);
            })
        );
      })
      .then(function () {
        return self.clients.claim();
      })
  );
});

// Network first (so updates arrive), cache as the offline fallback.
self.addEventListener("fetch", function (e) {
  if (e.request.method !== "GET") return;
  e.respondWith(
    fetch(e.request)
      .then(function (res) {
        var copy = res.clone();
        caches.open(CACHE).then(function (c) {
          c.put(e.request, copy);
        });
        return res;
      })
      .catch(function () {
        return caches.match(e.request);
      })
  );
});
