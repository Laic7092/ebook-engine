// PWA 缓存：应用壳 + wasm + 第一次读到的字体/书。
//
// 关键取舍：
//   * 导航请求 cache-first。iOS 从主屏冷启动时先拿缓存里的 index.html，
//     再后台更新；网络优先会白白等一次往返（这是之前 1s 白屏的主因）。
//   * 安装时只要有任一资源失败就整体失败，PWA 会彻底不生效；这里逐项
//     容错，字体这种非核心资源干脆留到运行时再缓存。
//   * 预缓存用 cache: "no-cache"：index.html / JS 会回源校验，wasm 这种
//     immutable 资源仍然直接命中 HTTP 缓存，不会重复下载 1.5 MB。

// 发布时如果改了 app.js / reader.js 等未带哈希的文件，记得同时改 CACHE_NAME：
// 这些资源是 cache-first，浏览器不会主动发现它们变了。
const CACHE_NAME = "ebook-v10";
const FONT_CACHE = "ebook-fonts-v1";
const INDEX_URL = new URL("./index.html", self.location.href).href;
const CORE = [
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
  "./manifest.webmanifest",
  "./icon-192.png",
  "./icon-512.png",
];
// latin.ttf 有 760 KB，只有打开无内嵌字体的英文书才用得到；
// 首次使用时由 fetch 的 cache-first 分支写入，不拖慢安装。
// cjk 字体同理（首次约 1.5 MB gzip），由 reader.js / fetch 缓存。

self.addEventListener("install", (event) => {
  event.waitUntil(
    (async () => {
      const cache = await caches.open(CACHE_NAME);
      const results = await Promise.all(
        CORE.map((path) => {
          const url = new URL(path, self.location.href).href;
          return cache
            .add(new Request(url, { cache: "no-cache" }))
            .then(() => true)
            .catch((error) => {
              console.warn("[sw] precache failed:", path, error);
              return false;
            });
        }),
      );
      // 一个都没成功说明网络/部署有问题，让浏览器稍后重试；
      // 个别图标失败则不影响应用壳可用。
      if (!results.some(Boolean)) {
        throw new Error("precache failed");
      }
      await self.skipWaiting();
    })(),
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

  if (request.mode === "navigate") {
    // 同步注册后台更新，保证返回缓存后 SW 不会被立刻回收。
    const update = fetch(request)
      .then(async (response) => {
        if (response && response.ok) {
          // 固定用 index.html 当缓存键，避免每个 URL 各存一份 HTML。
          const cache = await caches.open(CACHE_NAME);
          await cache.put(INDEX_URL, response.clone()).catch(() => {});
        }
        return response;
      })
      .catch(() => null);
    event.waitUntil(update);
    event.respondWith(
      (async () => {
        const cache = await caches.open(CACHE_NAME);
        // 导航 URL 可能带 query（例如从 Safari 打开分享链接），统一回退到应用壳。
        const cached =
          (await cache.match(request, { ignoreSearch: true })) ||
          (await cache.match(INDEX_URL));
        if (cached) {
          // 页面立刻用缓存渲染，网络更新在后台完成，下次启动生效。
          return cached;
        }
        const fresh = await update;
        if (fresh) {
          return fresh;
        }
        return new Response("离线，且应用壳尚未缓存。", {
          status: 503,
          headers: { "content-type": "text/plain; charset=utf-8" },
        });
      })(),
    );
    return;
  }

  // 静态资源 cache-first：尤其是 wasm / 字体，首屏之后不再走网络。
  event.respondWith(handleAsset(request));
});

async function handleAsset(request) {
  const cache = await caches.open(CACHE_NAME);
  const cached = await cache.match(request, { ignoreSearch: true });
  if (cached) {
    return cached;
  }
  const response = await fetch(request);
  if (response && response.ok) {
    // 写到当前版本的壳缓存；下次新 SW 安装时会整体换掉。
    cache.put(request, response.clone()).catch(() => {});
  }
  return response;
}
