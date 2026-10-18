// Makes Soba's page cross-origin isolated, so the threaded build can render
// its sound in an AudioWorklet, on the browser's audio thread (#375). That
// takes two response headers, and GitHub Pages can't be told to send them:
// this service worker adds them to the page as it's served. index.html's
// sobaIsolate() registers it and loads the page once more, through it.
//
// Only the page itself needs them. Everything else it loads comes from this
// same site, so passes untouched, and so does an embed (?patch=): its parent
// page isn't isolated, so it couldn't be either.

self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", (event) => event.waitUntil(self.clients.claim()));

self.addEventListener("fetch", (event) => {
    const request = event.request;
    if (request.mode !== "navigate" || new URL(request.url).searchParams.has("patch")) return;
    event.respondWith(fetch(request).then((response) => {
        // A redirect is followed by the browser, and asked for again here
        if (response.type === "opaqueredirect" || response.status === 0) return response;
        const headers = new Headers(response.headers);
        headers.set("Cross-Origin-Opener-Policy", "same-origin");
        headers.set("Cross-Origin-Embedder-Policy", "require-corp");
        return new Response(response.body, { status: response.status, statusText: response.statusText, headers });
    }));
});
