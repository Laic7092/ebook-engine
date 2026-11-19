// Web 入口：书架 / 阅读页切换、导入、直接打开 ?book= 示例书。
//
// 目录结构：
//   db.js        IndexedDB（没有时退化成内存）
//   library.js   书架渲染与删除
//   reader.js    阅读页、排版、目录、字体、进度
//   text.js      文件格式识别与 TXT 编码检测

import init from "./pkg/ebook_web.js";
import * as store from "./db.js";
import { createLibrary } from "./library.js";
import { createReader, DEFAULT_SETTINGS } from "./reader.js";
import { decodeText, detectFormat, fingerprint, titleFromName } from "./text.js";

const SETTINGS_KEY = "reader";

const libraryView = document.getElementById("library-view");
const readerView = document.getElementById("reader-view");
const bannerEl = document.getElementById("banner");
const statusEl = document.getElementById("status");
const fileEl = document.getElementById("file");
const dragHintEl = document.getElementById("drag-hint");

let currentView = "library";
let settings = { ...DEFAULT_SETTINGS };
let statusTimer = 0;

function registerServiceWorker() {
  if (typeof navigator === "undefined" || !navigator.serviceWorker) {
    return;
  }
  if (!/^https?:$/.test(window.location.protocol)) {
    return;
  }
  navigator.serviceWorker.register("./sw.js").catch(() => {
    // 预览环境可能没有 SW；阅读功能不依赖它。
  });
}

function setStatus(text) {
  if (!statusEl) {
    return;
  }
  statusEl.textContent = text || "";
  clearTimeout(statusTimer);
  if (text && text !== "就绪" && text !== "ready") {
    statusTimer = setTimeout(() => {
      statusEl.textContent = "";
    }, 3600);
  }
}

function setBanner(message) {
  if (!bannerEl) {
    return;
  }
  bannerEl.textContent = message || "";
  bannerEl.hidden = !message;
}

function showView(name) {
  currentView = name;
  if (libraryView) {
    libraryView.hidden = name !== "library";
  }
  if (readerView) {
    readerView.hidden = name !== "reader";
  }
  if (document.body) {
    document.body.classList.toggle("reading", name === "reader");
    if (name === "library") {
      document.body.classList.remove("toc-open", "settings-open", "chrome-hidden");
    }
  }
  if (document.title) {
    document.title = name === "reader" ? "阅读 · ebook" : "ebook · 书架";
  }
}

const library = createLibrary({
  store,
  onOpen: (record) => {
    openRecord(record).catch((error) => {
      setBanner(`打不开《${record.title || record.name || "Untitled"}》：${error.message || error}`);
    });
  },
  onImport: () => {
    if (fileEl) {
      fileEl.click();
    }
  },
  onError: (message) => setBanner(message),
});

const reader = createReader({
  store,
  settings,
  onStatus: (text) => {
    if (text && text !== "ready") {
      setStatus(text);
    }
  },
  onSettingsChange: (next) => {
    settings = next;
    store.putSetting(SETTINGS_KEY, settings).catch(() => {});
  },
  onLoaded: (info, record) => {
    if (info.title) {
      record.title = info.title;
    }
    if (info.creator) {
      record.author = info.creator;
    }
    record.lang = info.language || record.lang;
    record.spineCount = Array.isArray(info.spine) ? info.spine.length : record.spineCount;
  },
  onBack: () => {
    showView("library");
    library.refresh().catch(() => {});
  },
});

function makeRecord(name, bytes) {
  const format = detectFormat(name, bytes);
  const now = Date.now();
  const id = fingerprint(bytes, name);
  return {
    id,
    title: titleFromName(name),
    name,
    format,
    size: bytes.length,
    added: now,
    updated: now,
  };
}

async function openRecord(record, bytes, { persist = true } = {}) {
  const previous = currentView;
  setBanner("");
  showView("reader");
  try {
    const info = await reader.open(record, bytes);
    if (info.title) {
      record.title = info.title;
    }
    if (info.creator) {
      record.author = info.creator;
    }
    record.lang = info.language || record.lang;
    record.spineCount = Array.isArray(info.spine) ? info.spine.length : record.spineCount;
    record.updated = Date.now();
    if (persist) {
      const existing = await Promise.resolve(store.getBook(record.id)).catch(() => null);
      const merged = existing ? { ...existing, ...record, added: existing.added || record.added } : record;
      if (bytes) {
        await store.putBook(merged, bytes).catch(() => {});
      } else {
        await store.putBookMeta(merged).catch(() => {});
      }
      await library.refresh().catch(() => {});
      record = merged;
    }
    return record;
  } catch (error) {
    showView(previous);
    throw error;
  }
}

async function importFile(file, { open = true } = {}) {
  if (!file) {
    return null;
  }
  setBanner("");
  setStatus(`正在导入 ${file.name}`);
  const bytes = new Uint8Array(await file.arrayBuffer());
  if (!bytes.length) {
    throw new Error("文件为空");
  }
  const format = detectFormat(file.name, bytes);
  if (format === "txt" && !decodeText(bytes).trim()) {
    throw new Error("TXT 内容为空");
  }
  const record = makeRecord(file.name, bytes);
  const existing = await Promise.resolve(store.getBook(record.id)).catch(() => null);
  if (existing) {
    record.added = existing.added || record.added;
  }
  if (open) {
    const saved = await openRecord(record, bytes, { persist: true });
    setStatus(`已加入书架：${saved.title}`);
    return saved;
  }
  await store.putBook(record, bytes);
  await library.refresh();
  setStatus(`已加入书架：${record.title}`);
  return record;
}

async function fetchFirst(urls) {
  let lastError = null;
  for (const url of urls) {
    try {
      const response = await fetch(url, { cache: "no-store" });
      if (!response.ok) {
        throw new Error(`${url}: ${response.status}`);
      }
      return new Uint8Array(await response.arrayBuffer());
    } catch (error) {
      lastError = error;
    }
  }
  throw lastError || new Error("fetch failed");
}

function resolveBookUrl(path) {
  const base = new URL("./", window.location.href);
  return new URL(path, base).href;
}

async function openFromQuery(query) {
  const requested = query.get("book");
  if (!requested) {
    return false;
  }
  setStatus("正在加载示例书");
  const bytes = await fetchFirst([requested, resolveBookUrl(requested)]);
  const name = requested.split("/").pop() || "sample.epub";
  const record = makeRecord(name, bytes);
  const spine = Number(query.get("spine") || 0);
  if (Number.isInteger(spine) && spine > 0) {
    record.spine = spine;
  }
  await openRecord(record, bytes, { persist: false });
  setStatus("就绪");
  return true;
}

function bindFileInput() {
  if (!fileEl) {
    return;
  }
  fileEl.addEventListener("change", () => {
    const file = fileEl.files && fileEl.files[0];
    if (!file) {
      return;
    }
    importFile(file).catch((error) => {
      setBanner(`导入 ${file.name} 失败：${error.message || error}`);
      if (currentView === "reader" && !reader.isOpen()) {
        showView("library");
      }
    });
    fileEl.value = "";
  });
}

function bindDragAndDrop() {
  document.addEventListener("dragover", (event) => {
    event.preventDefault();
    if (dragHintEl) {
      dragHintEl.hidden = false;
    }
  });
  document.addEventListener("dragleave", () => {
    if (dragHintEl) {
      dragHintEl.hidden = true;
    }
  });
  document.addEventListener("drop", (event) => {
    event.preventDefault();
    if (dragHintEl) {
      dragHintEl.hidden = true;
    }
    const file = event.dataTransfer && event.dataTransfer.files && event.dataTransfer.files[0];
    if (!file) {
      return;
    }
    importFile(file).catch((error) => {
      setBanner(`导入 ${file.name} 失败：${error.message || error}`);
    });
  });
  let resizeTimer = 0;
  window.addEventListener("resize", () => {
    clearTimeout(resizeTimer);
    resizeTimer = setTimeout(() => {
      if (reader.isOpen()) {
        reader.refresh();
      }
    }, 120);
  });
}

function bindKeyboard() {
  window.addEventListener("keydown", (event) => {
    if (currentView !== "reader") {
      return;
    }
    const key = event.key;
    if (key === "Escape") {
      const body = document.body;
      if (body && body.classList.contains("toc-open")) {
        body.classList.remove("toc-open");
      } else if (body && body.classList.contains("settings-open")) {
        body.classList.remove("settings-open");
      } else if ((globalThis.window.innerWidth || 1200) < 700 && body && !body.classList.contains("chrome-hidden")) {
        reader.toggleChrome();
      } else {
        reader.close();
        showView("library");
        library.refresh().catch(() => {});
      }
      return;
    }
    if (key === "ArrowRight" || key === "PageDown" || key === " ") {
      event.preventDefault();
      reader.nextPage();
    } else if (key === "ArrowLeft" || key === "PageUp") {
      event.preventDefault();
      reader.prevPage();
    } else if (key === "t" || key === "T") {
      if (document.body) {
        document.body.classList.toggle("toc-open");
        document.body.classList.remove("settings-open");
      }
    }
  });
}

async function boot() {
  setBanner("");
  if (window.location.protocol === "file:") {
    setBanner(
      "index.html 被直接打开了。ES 模块和 wasm 不能从 file:// 加载：在仓库根目录执行 python3 -m http.server 8765，再打开 http://localhost:8765/web/",
    );
    return;
  }
  try {
    setStatus("正在加载 wasm");
    await init();
  } catch (error) {
    setBanner(`wasm 加载失败：${error.message || error}。请确认 web/pkg/ 已构建（./web/build.sh），并且通过 http 访问。`);
    return;
  }

  try {
    const saved = await store.getSetting(SETTINGS_KEY);
    if (saved && typeof saved === "object") {
      // 旧版存的是 fontScale 百分比，迁移到真实字号；fit 选项已经删除。
      const legacySize = Number(saved.fontScale)
        ? Math.round(DEFAULT_SETTINGS.fontSize * Number(saved.fontScale))
        : DEFAULT_SETTINGS.fontSize;
      settings = {
        fontSize: Number(saved.fontSize) || legacySize,
        bold: Boolean(saved.bold),
        lineHeight: Number(saved.lineHeight) || DEFAULT_SETTINGS.lineHeight,
        letterSpacing: Number(saved.letterSpacing) || 0,
        margin: Number(saved.margin) || DEFAULT_SETTINGS.margin,
        align: saved.align || DEFAULT_SETTINGS.align,
        theme: saved.theme || DEFAULT_SETTINGS.theme,
        animation: saved.animation || DEFAULT_SETTINGS.animation,
      };
    }
  } catch (error) {
    settings = { ...DEFAULT_SETTINGS };
  }
  reader.setSettings(settings);

  bindFileInput();
  bindDragAndDrop();
  bindKeyboard();
  registerServiceWorker();

  const query = new URLSearchParams(window.location.search || "");
  try {
    const direct = await openFromQuery(query);
    if (!direct) {
      showView("library");
      await library.refresh();
      setStatus("就绪");
    }
  } catch (error) {
    showView("library");
    await library.refresh().catch(() => {});
    setBanner(`打开失败：${error.message || error}`);
  }
}

boot().catch((error) => {
  setBanner(String(error.message || error));
  console.error(error);
});
