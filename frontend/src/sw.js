// Keeps the pages, styles and scripts for when the network is gone. Always asks
// the network first, since file names stay the same across versions, and only
// falls back to the saved copy offline. Never saves API answers, which hold
// personal data and need a session.

const CACHE = "firn-shell";

self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", (event) => event.waitUntil(self.clients.claim()));

self.addEventListener("fetch", (event) => {
  const url = new URL(event.request.url);
  if (event.request.method !== "GET" || url.origin !== location.origin || url.pathname.startsWith("/api/")) return;
  event.respondWith(
    (async () => {
      try {
        const response = await fetch(event.request);
        if (response.ok) {
          const cache = await caches.open(CACHE);
          await cache.put(event.request, response.clone());
        }
        return response;
      } catch (error) {
        const saved = await caches.match(event.request, { ignoreSearch: true });
        if (saved) return saved;
        throw error;
      }
    })(),
  );
});
