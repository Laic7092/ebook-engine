// 主线程的引擎客户端：wasm 在 `engine-worker.js` 里，这里只发 RPC。
//
// 每个方法都返回 Promise。宿主关心的是「结果」而不是「谁算的」，所以这层
// 也把引擎的字节协议（页元数据、位图头）解成普通对象，调用方不碰 DataView。

const DEFAULT_TIMEOUT_MS = 15000;

export function createEngine(options = {}) {
  const url = options.url || new URL("./engine-worker.js", import.meta.url);
  const timeoutMs = options.timeoutMs || DEFAULT_TIMEOUT_MS;
  let worker = null;
  let seq = 0;
  let broken = null;
  const pending = new Map();

  function failAll(error) {
    broken = error instanceof Error ? error : new Error(String(error));
    for (const [id, entry] of pending) {
      pending.delete(id);
      clearTimeout(entry.timer);
      entry.reject(broken);
    }
  }

  function start() {
    if (worker || broken) {
      if (broken) {
        throw broken;
      }
      return worker;
    }
    if (typeof Worker !== "function") {
      failAll(new Error("当前环境不支持 Web Worker"));
      throw broken;
    }
    try {
      worker = new Worker(url, { type: "module" });
    } catch (error) {
      failAll(error);
      throw broken;
    }
    worker.addEventListener("message", (event) => {
      const msg = event.data || {};
      const entry = pending.get(msg.id);
      if (!entry) {
        return;
      }
      pending.delete(msg.id);
      clearTimeout(entry.timer);
      if (msg.ok) {
        entry.resolve(msg.value);
      } else {
        entry.reject(new Error(msg.error || "引擎调用失败"));
      }
    });
    worker.addEventListener("error", (event) => {
      failAll((event && event.message) || "引擎 worker 加载失败");
    });
    return worker;
  }

  function send(type, payload, transfer) {
    const thread = start();
    const id = ++seq;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        if (!pending.has(id)) {
          return;
        }
        pending.delete(id);
        reject(new Error(`引擎调用超时：${type}`));
      }, timeoutMs);
      pending.set(id, { resolve, reject, timer });
      try {
        thread.postMessage({ id, type, ...payload }, transfer || []);
      } catch (error) {
        pending.delete(id);
        clearTimeout(timer);
        reject(error);
      }
    });
  }

  function toBytes(value) {
    if (!value) {
      return new Uint8Array(0);
    }
    if (value instanceof Uint8Array) {
      return value;
    }
    if (value instanceof ArrayBuffer) {
      return new Uint8Array(value);
    }
    return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
  }

  function pages(value) {
    if (!value) {
      return { count: 0, values: new Float32Array(0) };
    }
    return value;
  }

  return {
    /** 启动 worker 并实例化 wasm（首屏用它预热，失败会抛）。 */
    ready() {
      return send("ready", {});
    },
    /** 开书。位图/字体也都发生在这里，返回书的元数据。 */
    open({ kind = "epub", bytes, text, name, loadEmbedded = false } = {}) {
      if (kind === "txt") {
        return send("open", { kind, text: text || "", name, loadEmbedded });
      }
      // 必须复制：调用方之后还要拿这份字节写 IndexedDB，transfer 会把
      // 它的 ArrayBuffer 置空（detached）。
      const data = toBytes(bytes).slice();
      return send("open", { kind, bytes: data, loadEmbedded }, [data.buffer]);
    },
    font(bytes) {
      // 字体只管注册，调用方可能还要写缓存，所以不 transfer（克隆会复制）。
      return send("font", { bytes: toBytes(bytes) });
    },
    /** 注册书里内嵌的全部字体，返回 `{loaded, error}`。 */
    fontFaces() {
      return send("fontFaces", {});
    },
    fontFace(index) {
      return send("fontFace", { index });
    },
    fontReset() {
      return send("fontReset", {});
    },
    fontCount() {
      return send("fontCount", {});
    },
    fontFaceCount() {
      return send("fontFaceCount", {});
    },
    fontError() {
      return send("fontError", {});
    },
    layout(params) {
      return send("layout", params).then(pages);
    },
    pageMeta() {
      return send("pageMeta", {}).then(pages);
    },
    /**
     * 整页位图。返回 `ImageData`（无 `ImageData` 的环境退化成同形状对象），
     * 页号越界/未排版时为 `null`。
     */
    async bitmap(page, dpr, ink = 0, paper = 0, bold = false) {
      const bytes = await send("bitmap", { page, dpr, ink, paper, bold });
      if (!bytes || bytes.length < 8) {
        return null;
      }
      const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
      const width = view.getUint32(0, true);
      const height = view.getUint32(4, true);
      if (!width || !height || bytes.length < 8 + width * height * 4) {
        return null;
      }
      const data = new Uint8ClampedArray(bytes.buffer, bytes.byteOffset + 8, width * height * 4);
      if (typeof ImageData === "function") {
        return new ImageData(data, width, height);
      }
      return { width, height, data };
    },
    hit(page, x, y) {
      return send("hit", { page, x, y });
    },
    selection(a, b) {
      return send("selection", { a, b });
    },
    linkAt(page, x, y) {
      return send("linkAt", { page, x, y });
    },
    progress(page, line = 0, byte = 0) {
      return send("progress", { page, line, byte });
    },
    toc() {
      return send("toc", {});
    },
    /** 当前章的 `#fragment` → 行号，没有这个锚点时为 `null`。 */
    fragmentLine(fragment) {
      return send("fragmentLine", { fragment });
    },
    resolveLink(href) {
      return send("resolveLink", { href });
    },
    lastError() {
      return send("lastError", {});
    },
    fingerprint(bytes, name) {
      return send("fingerprint", { bytes: toBytes(bytes), name });
    },
    dispose() {
      const thread = worker;
      worker = null;
      failAll(new Error("引擎已关闭"));
      if (thread && typeof thread.terminate === "function") {
        thread.terminate();
      }
    },
  };
}
