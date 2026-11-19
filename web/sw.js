// PWA 缓存：应用壳 + wasm + 第一次读到的字体/书。
// 字体 19 MB，HTTP 缓存不可靠时每次冷启动都会重新下载；这里 cache-first。

const CACHE_NAME = "ebook-v9";
const FONT_CACHE = "ebook-fonts-v1";
const SHELL = [
  "./index.html",
  "./app.js",
  "./reader.js",
  "./library.js",
  "./db.js",
  "./text.js",
  "./selection.js",
  "./layout-worker.js",
  "./decode.js",
  "./encode.js",
  "./pkg/ebook_web.js",
  "./pkg/ebook_web_bg.wasm",
  "./latin.ttf",
  "./manifest.webmanifest",
  "./icon-192.png",
  "./icon-512.png",
];

self.addEventListener("install", (event) => {
  event.waitUntil(
    caches
      .open(CACHE_NAME)
      .then((cache) => cache.addAll(SHELL))
      .then(() => self.skipWaiting()),
  );
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) =>
        Promise.all(
          keys
            .filter(
              (key) =>
                key.startsWith("ebook-") && key !== CACHE_NAME && key !== FONT_CACHE,
            )
            .map((key) => caches.delete(key)),
        ),
      )
      .then(() => self.clients.claim()),
  );
});

self.addEventListener("fetch", (event) => {
  const request = event.request;
  if (request.method !== "GET") {
    return;
  }
  const url = new URL(request.url);
  if (url.origin !== self.location.origin) {
    return;
  }

  // 页面导航走网络优先：HTML 更新能立刻生效；断网时回退到缓存壳。
  if (request.mode === "navigate") {
    event.respondWith(
      fetch(request)
        .then((response) => {
          if (response && response.ok) {
            const copy = response.clone();
            caches.open(CACHE_NAME).then((cache) => cache.put(request, copy)).catch(() => {});
          }
          return response;
        })
        .catch(() => caches.match("./index.html")),
    );
    return;
  }

  // 静态资源 cache-first：尤其是 cjk.ttf / wasm，首屏之后不再走网络。
  event.respondWith(
    caches.match(request, { ignoreSearch: true }).then((cached) => {
      if (cached) {
        return cached;
      }
      return fetch(request).then((response) => {
        if (response && response.ok) {
          const copy = response.clone();
          caches.open(CACHE_NAME).then((cache) => cache.put(request, copy)).catch(() => {});
        }
        return response;
      });
    }),
  );
});
