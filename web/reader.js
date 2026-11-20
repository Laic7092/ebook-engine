// 阅读页：把最近一次 IR 画到 canvas，并维护章节/页码/目录/字体/进度。
//
// 这个模块只面向已经打开的书。书架视图、导入与持久化由 app.js / library.js
// 负责；这里通过 options.store 读写进度和字体缓存，方便测试替换。

import { createEngine } from "./engine-client.js";
import { createSelection } from "./selection.js";
import { decodeText } from "./text.js";

const PAGE_W = 720;
const PAGE_H = 960;
const FONT_URLS = ["./latin.ttf", "../fixtures/font/test.ttf", "fixtures/font/test.ttf"];
const CJK_FONT_URLS = [
  // 随仓库提供（PingFang SC Regular，WOFF2 单文件），本地服务器直接读，不再等 CDN。
  // woff2 自身已是 Brotli 压缩，不需要再叠加 gzip。
  "./cjk.woff2?v=5",
];
const FONT_CACHE = "ebook-fonts-v1";
// 字体没就绪时先画系统字体，超过这个时间就不再等；通常缓存命中会低于它。
const FONT_WAIT_MS = 60;
// 翻页动画：淡入淡出 / 跟手滑动。
const FADE_MS = 180;
const SLIDE_MS = 240;
const SLIDE_EASE = "cubic-bezier(0.22,0.61,0.36,1)";
const SWIPE_COMMIT_RATIO = 0.22;

const NARROW = typeof window !== "undefined" && (window.innerWidth || 1200) < 700;

export const DEFAULT_SETTINGS = {
  // 真实字号，单位 CSS px；页面容器不随字号变化。
  fontSize: 18,
  bold: false,
  lineHeight: 1.5,
  letterSpacing: 0,
  // 手机默认留白小一些，避免 390px 宽只剩十几个汉字；桌面保持较松的页边距。
  margin: NARROW ? 30 : 48,
  align: "book",
  theme: "light",
  animation: "slide",
};

const ALIGN_CODES = { book: 0, start: 1, center: 2, end: 3, justify: 4 };
// 引擎页元数据每页 9 个 f32：w,h,left,right,top,bottom,line_start,line_len,progress。
const PAGE_META_STRIDE = 9;
const ALIGNS = Object.keys(ALIGN_CODES);

function element(id) {
  return document.getElementById(id);
}

function clamp(value, min, max) {
  return Math.max(min, Math.min(max, value));
}

function toBytes(value) {
  if (!value) {
    return null;
  }
  if (value instanceof Uint8Array) {
    return value;
  }
  if (value instanceof ArrayBuffer) {
    return new Uint8Array(value);
  }
  if (ArrayBuffer.isView(value)) {
    return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
  }
  return new Uint8Array(value);
}

function splitHref(href) {
  const hash = href.indexOf("#");
  if (hash < 0) {
    return [href, ""];
  }
  return [href.slice(0, hash), href.slice(hash + 1)];
}

export function createReader(options) {
  const store = options.store;
  const onStatus = options.onStatus || (() => {});
  const onSettingsChange = options.onSettingsChange || (() => {});
  const onBack = options.onBack || (() => {});
  const onLoaded = options.onLoaded || (() => {});

  const canvas = element("page");
  // 页面只有一个绘制者（引擎位图），这里只负责把位图放进 #page。
  const ctx = canvas.getContext("2d");
  const canCopy = Boolean(ctx && typeof ctx.drawImage === "function");
  const selectionCanvas = element("selection-layer");
  const selectionCtx = selectionCanvas ? selectionCanvas.getContext("2d") : null;
  const titleEl = element("book-title");
  const metaEl = element("book-meta");
  const statusEl = element("status");
  const labelEl = element("page-label");
  const progressEl = element("progress");
  const prevEl = element("prev");
  const nextEl = element("next");
  const seekEl = element("seek");
  const builtinEl = element("builtin-font");
  const fontFileEl = element("font-file");
  const tocEl = element("toc");
  const tocToggleEl = element("toc-toggle");
  const tocCloseEl = element("toc-close");
  const settingsToggleEl = element("settings-toggle");
  const settingsCloseEl = element("settings-close");
  const backEl = element("back");
  const themeEl = element("theme");
  const fontSizeEl = element("font-size");
  const fontSizeLabelEl = element("font-size-label");
  const boldEl = element("font-bold");
  const lineHeightEl = element("line-height");
  const lineHeightLabelEl = element("line-height-label");
  const letterSpacingEl = element("letter-spacing");
  const letterSpacingLabelEl = element("letter-spacing-label");
  const alignEl = element("text-align");
  const animationEl = element("page-animation");
  const marginEl = element("margin");
  const marginLabelEl = element("margin-label");
  const settingsResetEl = element("settings-reset");
  const mainEl = element("reader-main");
  const stageEl = element("reader-stage");
  const backdropEl = element("panel-backdrop");
  const hintEl = element("reader-hint");

  let settings = { ...DEFAULT_SETTINGS, ...(options.settings || {}) };
  let book = null;
  let currentRecord = null;
  let layout = null;
  let chapter = 0;
  let page = 0;
  let tocItems = [];
  let layoutToken = 0;
  let layoutDimsUsed = null;
  let baseFontSize = 18;
  let busy = false;
  let progressTimer = 0;
  let chromeTimer = 0;
  let hintTimer = 0;
  // 绘制完全在引擎里：这里只记当前 DPR，用来判定缓存是否还有效。
  let renderDpr = 1;
  // 字体状态按“这本书需要哪套字体”复用：同一套字体换书不再重置 FontBook，
  // atlas 与 font id 都保持有效，热开书省掉首帧的整页光栅化。
  let fontState = null;
  // 首屏已经画出来之后，字体后台加载不再往状态栏刷进度，避免“就绪”又被
  // “字体 1.2 / 1.5 MB”盖掉。
  let fontProgressVisible = true;
  let openToken = 0;
  // 翻页动画的临时画布、计时器与当前滑动状态。
  let animationEls = [];
  let animationTimer = 0;
  let animationToken = 0;
  let animating = false;
  let turn = null;
  // 一次翻页从取位图到动画结束：期间不再接受新的翻页请求。
  let turning = false;
  // 页面位图缓存：当前页 ±1 的离屏 canvas，翻页/滑动直接搬图，不再重画字形。
  const pageSnapshots = new Map();
  let prefetchTimer = 0;
  // 引擎在 worker 里：开书、字体、排版、光栅、查询都只发生在那边。
  // 主线程只拿页元数据、位图和查询结果。
  const engine = createEngine(options.engine);
  // 每次 render 自增：晚到的位图不能盖掉新页。
  let renderToken = 0;
  // 位图按页缓存成 canvas，翻页动画与预取共用。
  // （缓存在 clearPageCache 里随章节/尺寸失效。）

  function hasClass(name) {
    const list = document.body && document.body.classList;
    return Boolean(list && typeof list.contains === "function" && list.contains(name));
  }

  function setStatus(text) {
    statusEl.textContent = text;
    onStatus(text);
  }

  // 阅读区始终铺满整个 reader-view。顶栏/底栏只是覆盖层，显隐不改变
  // 排版尺寸；正文用上/下不对称的页边距避开工具栏，所以切换工具栏
  // 不会重排、不会跳页。旧实现把 chrome 放回 flex 流里，一显隐就改
  // 约束，必须 relayout——这里彻底去掉那条路径。
  function viewSize() {
    const stage = stageEl || mainEl;
    const width = stage && stage.clientWidth ? stage.clientWidth : window.innerWidth || 1200;
    const height = stage && stage.clientHeight ? stage.clientHeight : window.innerHeight || 900;
    return {
      width: Math.max(280, Math.round(width)),
      height: Math.max(320, Math.round(height)),
    };
  }

  function chromeInsets() {
    const top = element("reader-top");
    const bottom = element("reader-bottom");
    return {
      top: (top && top.offsetHeight) || 54,
      bottom: (bottom && bottom.offsetHeight) || 62,
    };
  }

  function layoutDims() {
    const side = clamp(settings.margin, 20, 120);
    const chrome = chromeInsets();
    const view = viewSize();
    return {
      width: view.width,
      height: view.height,
      left: side,
      right: side,
      // 工具栏覆盖在页面上，正文至少要让开它的高度；四边因此不对称。
      top: Math.max(side, chrome.top + 8),
      bottom: Math.max(side, chrome.bottom + 8),
    };
  }

  function layoutFontScale() {
    const base = baseFontSize > 0 ? baseFontSize : DEFAULT_SETTINGS.fontSize;
    return clamp(settings.fontSize / base, 0.5, 3.0);
  }

  function sameDims(a, b) {
    return Boolean(
      a &&
        b &&
        Math.abs(a.width - b.width) < 0.5 &&
        Math.abs(a.height - b.height) < 0.5 &&
        Math.abs(a.left - b.left) < 0.5 &&
        Math.abs(a.right - b.right) < 0.5 &&
        Math.abs(a.top - b.top) < 0.5 &&
        Math.abs(a.bottom - b.bottom) < 0.5,
    );
  }

  // 设备像素比：3 是分页位图的上限，再高只会吃内存。
  function pageDpr() {
    return Math.min(window.devicePixelRatio || 1, 3);
  }

  // 引擎整页位图：绘制全在 Rust 里，宿主不做字形缓存、不做图片解码、
  // 不做主题调色；这里只把 worker 送回来的 RGBA 变成 ImageData。
  function enginePageImage(index, dpr) {
    return engine
      .bitmap(index, dpr, 0, 0, Boolean(settings.bold))
      .catch(() => null);
  }

  // 主题滤镜写在 `#page` 上（sepia/dark），引擎只认纸色与墨色。
  // 动画快照是独立 canvas，必须自己带上同一个滤镜，否则滑动时会闪色。
  function pageFilter() {
    try {
      const style = window.getComputedStyle ? window.getComputedStyle(canvas) : null;
      return (style && style.filter) || "";
    } catch (error) {
      return "";
    }
  }

  async function render() {
    if (!layout || !layout.pages.length) {
      return;
    }
    const shown = clamp(page, 0, layout.pages.length - 1);
    page = shown;
    const info = layout.pages[shown];
    const dpr = pageDpr();
    const w = (info.size.w || PAGE_W) + info.inset.left + info.inset.right;
    const h = (info.size.h || PAGE_H) + info.inset.top + info.inset.bottom;
    const pixelW = Math.round(w * dpr);
    const pixelH = Math.round(h * dpr);
    // 尺寸没变就不要重设 width/height：那会重新分配整个后备缓冲。
    if (canvas.width !== pixelW || canvas.height !== pixelH) {
      canvas.width = pixelW;
      canvas.height = pixelH;
    }
    canvas.style.width = `${w}px`;
    canvas.style.height = `${h}px`;
    if (renderDpr !== dpr) {
      clearPageCache();
    }
    renderDpr = dpr;
    const token = ++renderToken;
    const cached = canCopy ? cachedPageCanvas(shown) : null;
    if (cached && cached.width === pixelW && cached.height === pixelH) {
      ctx.setTransform(1, 0, 0, 1, 0, 0);
      ctx.drawImage(cached, 0, 0);
    } else {
      const image = await enginePageImage(shown, dpr);
      // 期间又翻了页/换了章：这一张位图已经过期，交给更晚的 render。
      if (token !== renderToken || page !== shown) {
        return;
      }
      if (image) {
        ctx.setTransform(1, 0, 0, 1, 0, 0);
        ctx.putImageData(image, 0, 0);
      }
    }
    syncSelectionLayer(w, h, dpr);
    paintSelection();
    if (labelEl) {
      labelEl.textContent = `第 ${chapter + 1} 章 · ${shown + 1}/${layout.pages.length}`;
    }
    if (titleEl && book.title) {
      document.title = `${book.title} · ${shown + 1}`;
    }
    const percent = bookProgress(shown);
    if (progressEl) {
      progressEl.textContent = `${(percent * 100).toFixed(1)}%`;
    }
    if (seekEl) {
      seekEl.value = String(Math.round(percent * 1000));
    }
    prevEl.disabled = shown === 0 && chapter === 0;
    nextEl.disabled = shown === layout.pages.length - 1 && chapter === book.spine.length - 1;
    schedulePrefetch();
  }

  function pageCanvasKey(index) {
    return `${layoutToken}:${index}:${renderDpr}`;
  }

  function clearPageCache() {
    pageSnapshots.clear();
    if (prefetchTimer) {
      clearTimeout(prefetchTimer);
      prefetchTimer = 0;
    }
  }

  function cachedPageCanvas(index) {
    return pageSnapshots.get(pageCanvasKey(index)) || null;
  }

  function cachePageCanvas(index, snap) {
    if (!snap) {
      return;
    }
    pageSnapshots.set(pageCanvasKey(index), snap);
    while (pageSnapshots.size > 4) {
      const oldest = pageSnapshots.keys().next().value;
      pageSnapshots.delete(oldest);
    }
  }

  // 任意一页的离屏快照：一次引擎调用出一张位图，预取与滑动共用同一个出口。
  async function renderPageCanvas(index) {
    if (!canCopy || !layout || !layout.pages[index]) {
      return null;
    }
    const dpr = renderDpr || pageDpr();
    const image = await enginePageImage(index, dpr);
    if (!image) {
      return null;
    }
    const snap = document.createElement("canvas");
    snap.width = image.width;
    snap.height = image.height;
    const snapCtx = snap.getContext("2d");
    if (!snapCtx || typeof snapCtx.putImageData !== "function") {
      return null;
    }
    snapCtx.putImageData(image, 0, 0);
    return snap;
  }

  // 快照缓存缺失时现取一张（引擎一次调用，主线程不做光栅）。
  async function ensurePageCanvas(index) {
    const cached = cachedPageCanvas(index);
    if (cached) {
      return cached;
    }
    const snap = await renderPageCanvas(index);
    if (snap) {
      cachePageCanvas(index, snap);
    }
    return snap;
  }

  function schedulePrefetch() {
    if (!canCopy || !layout || animating || turn) {
      return;
    }
    if (prefetchTimer) {
      clearTimeout(prefetchTimer);
    }
    prefetchTimer = setTimeout(() => {
      prefetchTimer = 0;
      prefetchNeighbor();
    }, 120);
  }

  async function prefetchNeighbor() {
    if (!canCopy || !layout || animating || turn) {
      return;
    }
    const token = layoutToken;
    const index = [page + 1, page - 1].find(
      (candidate) =>
        candidate >= 0 &&
        candidate < layout.pages.length &&
        !cachedPageCanvas(candidate),
    );
    if (index === undefined) {
      return;
    }
    const run = async () => {
      if (token !== layoutToken || !layout || animating || turn) {
        return;
      }
      cachePageCanvas(index, await renderPageCanvas(index));
      // 一次只预渲染一页：首屏交互优先，另一页留给下一次空闲。
      if (
        [page + 1, page - 1].some(
          (candidate) =>
            candidate >= 0 &&
            candidate < layout.pages.length &&
            !cachedPageCanvas(candidate),
        )
      ) {
        prefetchTimer = setTimeout(() => {
          prefetchTimer = 0;
          prefetchNeighbor();
        }, 90);
      }
    };
    if (typeof requestIdleCallback === "function") {
      requestIdleCallback(run, { timeout: 800 });
    } else {
      setTimeout(run, 40);
    }
  }

  function syncSelectionLayer(width, height, dpr) {
    if (!selectionCanvas) {
      return;
    }
    const pixelW = Math.round(width * dpr);
    const pixelH = Math.round(height * dpr);
    if (selectionCanvas.width !== pixelW || selectionCanvas.height !== pixelH) {
      selectionCanvas.width = pixelW;
      selectionCanvas.height = pixelH;
    }
    selectionCanvas.style.width = `${width}px`;
    selectionCanvas.style.height = `${height}px`;
    selectionCanvas.style.left = `${canvas.offsetLeft || 0}px`;
    selectionCanvas.style.top = `${canvas.offsetTop || 0}px`;
    if (selectionCtx && typeof selectionCtx.setTransform === "function") {
      selectionCtx.setTransform(dpr, 0, 0, dpr, 0, 0);
    }
  }

  // 选区画在独立透明层上；拖动选区只清这一层，正文页不重绘。
  function paintSelection() {
    if (!selectionCanvas || !selectionCtx) {
      return;
    }
    if (typeof selectionCtx.clearRect === "function") {
      selectionCtx.save();
      selectionCtx.setTransform(1, 0, 0, 1, 0, 0);
      selectionCtx.clearRect(0, 0, selectionCanvas.width, selectionCanvas.height);
      selectionCtx.restore();
    }
    if (layout && layout.pages.length) {
      selection.draw(selectionCtx, clamp(page, 0, layout.pages.length - 1));
    }
  }

  function bookProgress(shown) {
    const info = layout.pages[shown];
    const inChapter = Number(info.progress) || 0;
    const total = book && book.spine.length ? book.spine.length : 1;
    return clamp((chapter + inChapter) / total, 0, 1);
  }

  function saveProgressSoon() {
    if (progressTimer) {
      clearTimeout(progressTimer);
    }
    progressTimer = setTimeout(saveProgress, 350);
  }

  function saveProgress() {
    if (progressTimer) {
      clearTimeout(progressTimer);
      progressTimer = 0;
    }
    if (!currentRecord || !layout || !layout.pages[page] || !book) {
      return;
    }
    const info = layout.pages[page];
    const record = {
      spine: chapter,
      line: info.lines.start,
      page,
      percent: bookProgress(page),
      updated: Date.now(),
    };
    Promise.resolve(store.putProgress(currentRecord.id, record)).catch(() => {});
  }

  function highlightToc() {
    for (const item of tocItems) {
      const active = item.entry.spine === chapter;
      item.button.className = active ? "toc-entry active" : "toc-entry";
    }
  }

  async function fillToc() {
    let entries = [];
    try {
      entries = await engine.toc();
    } catch (error) {
      entries = [];
    }
    tocEl.replaceChildren();
    tocItems = [];
    if (!entries.length) {
      return;
    }
    const root = document.createElement("ul");
    const levels = [{ depth: 0, list: root, lastLi: null }];
    for (const entry of entries) {
      while (levels.length > 1 && entry.depth < levels[levels.length - 1].depth) {
        levels.pop();
      }
      let level = levels[levels.length - 1];
      if (entry.depth > level.depth) {
        const sub = document.createElement("ul");
        if (level.lastLi) {
          level.lastLi.append(sub);
        } else {
          level.list.append(sub);
        }
        level = { depth: entry.depth, list: sub, lastLi: null };
        levels.push(level);
      }
      const li = document.createElement("li");
      const button = document.createElement("button");
      button.type = "button";
      button.className = "toc-entry";
      button.textContent = entry.label;
      if (entry.spine === null) {
        button.disabled = true;
      } else {
        button.addEventListener("click", () => {
          goToToc(entry);
        });
      }
      li.append(button);
      level.list.append(li);
      level.lastLi = li;
      tocItems.push({ entry, button });
    }
    tocEl.append(root);
    highlightToc();
  }

  async function updateBook() {
    titleEl.textContent = book.title || currentRecord?.title || "untitled";
    metaEl.textContent = currentRecord?.author || book.creator || book.language || book.lang || "";
    await fillToc();
  }

  async function goToToc(entry) {
    if (entry.spine === null) {
      return;
    }
    try {
      await layoutChapter(entry.spine, 0);
      if (entry.fragment) {
        await jumpToFragment(entry.fragment);
      }
      closePanelsAndRead();
    } catch (error) {
      setStatus(String(error.message || error));
    }
  }

  function closePanels() {
    if (document.body) {
      document.body.classList.remove("toc-open", "settings-open");
    }
  }

  function isNarrowView() {
    return (window.innerWidth || 1200) < 700;
  }

  function clearChromeTimer() {
    if (chromeTimer) {
      clearTimeout(chromeTimer);
      chromeTimer = 0;
    }
  }

  function hideHint() {
    if (hintTimer) {
      clearTimeout(hintTimer);
      hintTimer = 0;
    }
    if (hintEl) {
      hintEl.hidden = true;
    }
  }

  function showHint(text, delay = 2600) {
    if (!hintEl) {
      return;
    }
    hintEl.textContent = text;
    hintEl.hidden = false;
    clearTimeout(hintTimer);
    hintTimer = setTimeout(() => {
      hintTimer = 0;
      hintEl.hidden = true;
    }, delay);
  }

  function scheduleChromeHide(delay = 2800) {
    clearChromeTimer();
    if (!book || !isNarrowView()) {
      return;
    }
    chromeTimer = setTimeout(() => {
      chromeTimer = 0;
      if (hasClass("chrome-hidden") || hasClass("toc-open") || hasClass("settings-open")) {
        return;
      }
      setChromeHidden(true);
    }, delay);
  }

  function setChromeHidden(hidden) {
    if (!document.body) {
      return;
    }
    clearChromeTimer();
    document.body.classList.toggle("chrome-hidden", Boolean(hidden));
    if (hidden) {
      closePanels();
    }
    // 不 relayout：chrome 是覆盖层，页面几何完全不变。
  }

  function toggleChrome() {
    hideHint();
    if (hasClass("chrome-hidden")) {
      setChromeHidden(false);
      scheduleChromeHide(3200);
    } else {
      setChromeHidden(true);
    }
  }

  function closePanelsAndRead() {
    closePanels();
    if (book && isNarrowView()) {
      setChromeHidden(true);
    }
  }

  // ---- 章节 / 页码 ---------------------------------------------------------

  function resolveTarget(target) {
    if (!layout) {
      return 0;
    }
    const last = layout.pages.length - 1;
    if (target === "last") {
      return Math.max(0, last);
    }
    if (target && typeof target === "object") {
      if (Number.isInteger(target.line)) {
        const found = layout.pages.findIndex(
          (info) => target.line >= info.lines.start && target.line < info.lines.start + info.lines.len,
        );
        if (found >= 0) {
          return found;
        }
      }
      if (typeof target.ratio === "number") {
        return clamp(Math.round(target.ratio * last), 0, last);
      }
      if (Number.isInteger(target.page)) {
        return clamp(target.page, 0, last);
      }
    }
    if (target === undefined || target === null) {
      return 0;
    }
    return clamp(Number(target) || 0, 0, last);
  }

  async function layoutChapter(index, target) {
    if (animating || turn) {
      cancelAnimation();
    }
    clearPageCache();
    selection.clear();
    chapter = clamp(index, 0, book.spine.length - 1);
    const spine = chapter;
    const dims = layoutDims();
    layoutDimsUsed = dims;
    const token = ++layoutToken;
    let pages;
    try {
      pages = await engine.layout({
        spine,
        width: dims.width,
        height: dims.height,
        left: dims.left,
        right: dims.right,
        top: dims.top,
        bottom: dims.bottom,
        fontScale: layoutFontScale(),
        lineScale: settings.lineHeight,
        letter: settings.letterSpacing,
        align: ALIGN_CODES[settings.align] ?? 0,
      });
    } catch (error) {
      throw new Error(`第 ${spine + 1} 章排版失败：${error.message || error}`);
    }
    if (token !== layoutToken) {
      return;
    }
    layout = { pages: pagesFromMeta(pages) };
    page = resolveTarget(target);
    highlightToc();
    await render();
    saveProgressSoon();
  }

  // 引擎只送页盒子/行区间/进度，整份 IR 不再过线程。
  function pagesFromMeta(meta) {
    const pages = [];
    const values = (meta && meta.values) || [];
    const count = (meta && meta.count) || 0;
    for (let index = 0; index < count; index += 1) {
      const at = index * PAGE_META_STRIDE;
      pages.push({
        index,
        size: { w: values[at], h: values[at + 1] },
        inset: {
          left: values[at + 2],
          right: values[at + 3],
          top: values[at + 4],
          bottom: values[at + 5],
        },
        lines: { start: values[at + 6] | 0, len: values[at + 7] | 0 },
        progress: values[at + 8],
      });
    }
    return pages;
  }

  async function relayout(position) {
    if (!book || !layout) {
      return;
    }
    const keep = position || {
      line: layout.pages[page]?.lines.start ?? 0,
      ratio: layout.pages.length ? page / layout.pages.length : 0,
    };
    await layoutChapter(chapter, keep);
  }

  // ---- 翻页动画 -----------------------------------------------------------

  function snapshotPage() {
    if (!canvas.width || !canvas.height) {
      return null;
    }
    try {
      const snap = document.createElement("canvas");
      snap.width = canvas.width;
      snap.height = canvas.height;
      const snapCtx = snap.getContext("2d");
      if (!snapCtx || typeof snapCtx.drawImage !== "function") {
        return null;
      }
      snapCtx.drawImage(canvas, 0, 0);
      return snap;
    } catch (error) {
      return null;
    }
  }

  function pageCssWidth() {
    const width = parseFloat(canvas.style.width);
    return width > 0 ? width : canvas.clientWidth || canvas.width || 1;
  }

  // 把快照铺在 #page 上面；canvas 本身留在下面，动画结束再显示。
  function placeOverlay(snap) {
    const stage = stageEl || canvas.parentNode;
    if (!stage || !snap) {
      return null;
    }
    const width = pageCssWidth();
    const height = parseFloat(canvas.style.height) || canvas.clientHeight || canvas.height;
    snap.style.position = "absolute";
    snap.style.left = `${canvas.offsetLeft || 0}px`;
    snap.style.top = `${canvas.offsetTop || 0}px`;
    snap.style.width = `${width}px`;
    snap.style.height = `${height}px`;
    snap.style.background = "#fff";
    const filter = pageFilter();
    if (filter && filter !== "none") {
      snap.style.filter = filter;
    }
    snap.style.pointerEvents = "none";
    snap.style.zIndex = "2";
    snap.style.willChange = "transform, opacity";
    if (canvas.style.borderRadius) {
      snap.style.borderRadius = canvas.style.borderRadius;
    }
    if (typeof stage.appendChild === "function") {
      stage.appendChild(snap);
    } else if (typeof stage.append === "function") {
      stage.append(snap);
    } else {
      return null;
    }
    animationEls.push(snap);
    return snap;
  }

  function dropOverlays() {
    for (const el of animationEls) {
      if (el && typeof el.remove === "function") {
        el.remove();
      } else if (el && el.parentNode && typeof el.parentNode.removeChild === "function") {
        el.parentNode.removeChild(el);
      }
    }
    animationEls = [];
  }

  function resetCanvasStyle() {
    canvas.style.transform = "";
    canvas.style.transition = "";
    canvas.style.visibility = "";
  }

  function nextFrame(fn) {
    if (typeof requestAnimationFrame === "function") {
      requestAnimationFrame(() => requestAnimationFrame(fn));
    } else {
      setTimeout(fn, 0);
    }
  }

  // 所有动画都走同一个 token：新动画/取消会让旧回调自动作废，避免快速翻页时
  // 旧计时器把新快照删掉。
  function beginAnimation(ms, after) {
    const token = ++animationToken;
    animating = true;
    if (animationTimer) {
      clearTimeout(animationTimer);
    }
    animationTimer = setTimeout(() => {
      animationTimer = 0;
      if (token !== animationToken) {
        return;
      }
      animating = false;
      dropOverlays();
      resetCanvasStyle();
      if (after) {
        after();
      }
    }, ms);
  }

  function cancelAnimation() {
    animationToken += 1;
    if (animationTimer) {
      clearTimeout(animationTimer);
      animationTimer = 0;
    }
    animating = false;
    turn = null;
    dropOverlays();
    resetCanvasStyle();
  }

  function playFade(snap) {
    const el = placeOverlay(snap);
    if (!el) {
      return;
    }
    el.style.opacity = "1";
    el.style.transition = `opacity ${FADE_MS}ms ease`;
    nextFrame(() => {
      el.style.opacity = "0";
    });
    beginAnimation(FADE_MS + 60);
  }

  function playSlide(snap, direction, targetSnap) {
    const toSnap = targetSnap || snapshotPage();
    if (!toSnap) {
      return;
    }
    const oldEl = placeOverlay(snap);
    const newEl = placeOverlay(toSnap);
    if (!oldEl || !newEl) {
      dropOverlays();
      return;
    }
    const width = pageCssWidth();
    canvas.style.visibility = "hidden";
    oldEl.style.transform = "translate3d(0,0,0)";
    newEl.style.transform = `translate3d(${direction * width}px,0,0)`;
    nextFrame(() => {
      oldEl.style.transition = `transform ${SLIDE_MS}ms ${SLIDE_EASE}`;
      newEl.style.transition = `transform ${SLIDE_MS}ms ${SLIDE_EASE}`;
      oldEl.style.transform = `translate3d(${-direction * width}px,0,0)`;
      newEl.style.transform = "translate3d(0,0,0)";
    });
    beginAnimation(SLIDE_MS + 60);
  }

  async function animatePageTo(target, direction) {
    const mode = settings.animation;
    const from = page;
    const snap = mode === "none" ? null : snapshotPage();
    if (snap) {
      cachePageCanvas(from, snap);
    }
    // 目标位图在切换前就位：动画第一帧不会出现“空页”。
    const targetSnap = mode === "none" ? null : await ensurePageCanvas(target);
    if (page !== from) {
      return;
    }
    selection.clear();
    page = target;
    await render();
    saveProgressSoon();
    if (!snap || mode === "none") {
      return;
    }
    if (mode === "fade") {
      playFade(snap);
    } else {
      playSlide(snap, direction, targetSnap);
    }
  }

  async function animateChapterTo(index, direction) {
    const mode = settings.animation;
    const snap = mode === "none" ? null : snapshotPage();
    await layoutChapter(index, direction > 0 ? 0 : "last");
    if (!snap || mode === "none") {
      return;
    }
    if (mode === "fade") {
      playFade(snap);
    } else {
      playSlide(snap, direction);
    }
  }

  async function turnPage(direction) {
    if (!layout || busy || animating || turning) {
      return;
    }
    turning = true;
    try {
      if (direction > 0) {
        if (page + 1 < layout.pages.length) {
          await animatePageTo(page + 1, 1);
        } else if (chapter + 1 < book.spine.length) {
          await animateChapterTo(chapter + 1, 1);
        }
      } else if (page > 0) {
        await animatePageTo(page - 1, -1);
      } else if (chapter > 0) {
        await animateChapterTo(chapter - 1, -1);
      }
    } catch (error) {
      setStatus(String(error.message || error));
    } finally {
      turning = false;
    }
    if (!hasClass("chrome-hidden")) {
      scheduleChromeHide(1800);
    }
  }

  function nextPage() {
    return turnPage(1);
  }

  function prevPage() {
    return turnPage(-1);
  }

  // 跟手滑动：先抓当前页，再把目标页临时画到 canvas 上抓第二张快照，
  // 随后恢复原页。两张快照跟手指移动，松手时按位移+速度决定翻页或回弹。
  async function beginSwipe(direction) {
    if (!layout || busy || animating || turning || settings.animation !== "slide") {
      return false;
    }
    const forward = direction === "next";
    const target = forward ? page + 1 : page - 1;
    if (target < 0 || target >= layout.pages.length) {
      return false;
    }
    const from = page;
    const fromSnap = snapshotPage();
    if (!fromSnap) {
      return false;
    }
    // 邻页通常已经预取好；没有就现取一张（异步，主线程只等一次 RPC），
    // 手指已经开始动时最多晚一帧跟上。
    const toSnap = await ensurePageCanvas(target);
    if (!toSnap || page !== from) {
      return false;
    }
    cachePageCanvas(from, fromSnap);
    const oldEl = placeOverlay(fromSnap);
    const newEl = placeOverlay(toSnap);
    if (!oldEl || !newEl) {
      cancelAnimation();
      return false;
    }
    canvas.style.visibility = "hidden";
    turn = {
      direction: forward ? 1 : -1,
      target,
      oldEl,
      newEl,
      width: pageCssWidth(),
      dx: 0,
    };
    animating = true;
    applySwipeDrag(0);
    return true;
  }

  function applySwipeDrag(dx) {
    if (!turn) {
      return;
    }
    const width = Math.max(1, turn.width);
    const distance = turn.direction > 0 ? clamp(dx, -width, 0) : clamp(dx, 0, width);
    turn.dx = distance;
    turn.oldEl.style.transform = `translate3d(${distance}px,0,0)`;
    turn.newEl.style.transform =
      `translate3d(${distance + turn.direction * width}px,0,0)`;
  }

  function endSwipe(dx, dt) {
    if (!turn) {
      return;
    }
    const active = turn;
    const width = Math.max(1, active.width);
    const velocity = dx / Math.max(1, dt);
    const projected = active.dx + velocity * 120;
    const commit = Math.abs(projected) > width * SWIPE_COMMIT_RATIO;
    const finalDx = commit ? -active.direction * width : 0;
    active.oldEl.style.transition = `transform ${SLIDE_MS}ms ${SLIDE_EASE}`;
    active.newEl.style.transition = `transform ${SLIDE_MS}ms ${SLIDE_EASE}`;
    nextFrame(() => {
      active.oldEl.style.transform = `translate3d(${finalDx}px,0,0)`;
      active.newEl.style.transform =
        `translate3d(${finalDx + active.direction * width}px,0,0)`;
    });
    beginAnimation(SLIDE_MS + 40, () => {
      turn = null;
      if (commit) {
        page = active.target;
        selection.clear();
        render();
        saveProgressSoon();
      }
    });
    if (!hasClass("chrome-hidden")) {
      scheduleChromeHide(1800);
    }
  }

  function cancelSwipe() {
    cancelAnimation();
  }

  async function seekTo(fraction) {
    if (!book || !layout) {
      return;
    }
    if (animating || turn) {
      cancelAnimation();
    }
    const total = book.spine.length || 1;
    const pos = clamp(fraction, 0, 1) * total;
    const targetChapter = clamp(Math.floor(pos), 0, total - 1);
    const ratio = pos - targetChapter;
    if (targetChapter === chapter) {
      const last = layout.pages.length - 1;
      page = clamp(Math.round(ratio * last), 0, last);
      selection.clear();
      render();
      saveProgressSoon();
    } else {
      await layoutChapter(targetChapter, { ratio });
    }
  }

  // ---- 链接 / 锚点 ---------------------------------------------------------

  function spineIndexOfPath(path) {
    const clean = (value) => value.replace(/^\.\//, "").toLowerCase();
    const want = clean(path);
    if (!want) {
      return null;
    }
    const current = book.spine[chapter];
    const dir = current ? current.href.replace(/[^/]*$/, "") : "";
    for (let index = 0; index < book.spine.length; index += 1) {
      const href = clean(book.spine[index].href);
      if (href === want || href === clean(dir + want) || href.endsWith(`/${want}`)) {
        return index;
      }
    }
    return null;
  }

  async function followLink(href) {
    if (!href || !book) {
      return;
    }
    const target = await engine.resolveLink(href).catch(() => null);
    if (target) {
      if (target.external) {
        setStatus(`外链：${href}`);
        return;
      }
      if (target.spine !== chapter) {
        await layoutChapter(target.spine, 0);
      } else {
        page = 0;
        selection.clear();
        render();
        saveProgressSoon();
      }
      if (target.fragment) {
        await jumpToFragment(target.fragment);
      }
      return;
    }
    const [rawPath, fragment] = splitHref(href);
    if (!rawPath) {
      await jumpToFragment(fragment);
      return;
    }
    const index = spineIndexOfPath(rawPath);
    if (index === null) {
      setStatus(`外链或未知目标：${href}`);
      return;
    }
    await layoutChapter(index, 0);
    if (fragment) {
      await jumpToFragment(fragment);
    }
  }

  async function jumpToFragment(fragment) {
    const line = await engine.fragmentLine(fragment).catch(() => null);
    if (line === null || line === undefined) {
      setStatus(`找不到锚点 #${fragment}`);
      return;
    }
    await jumpToLine(line);
  }

  async function jumpToLine(line) {
    if (animating || turn) {
      cancelAnimation();
    }
    const target = layout.pages.findIndex(
      (info) => line >= info.lines.start && line < info.lines.start + info.lines.len,
    );
    if (target < 0) {
      setStatus("锚点不在本章");
      return;
    }
    page = target;
    selection.clear();
    await render();
    saveProgressSoon();
  }

  const selection = createSelection({
    canvas,
    page: () => (layout ? { index: page, info: layout.pages[page], layout } : null),
    hit: (index, x, y) => engine.hit(index, x, y).catch(() => null),
    select: (a, b) => engine.selection(a, b),
    linkAt: (index, x, y) => engine.linkAt(index, x, y).catch(() => null),
    onLink: (href) => {
      followLink(href).catch((error) => setStatus(String(error.message || error)));
    },
    onTap: (event) => {
      // 左/右 30% 翻页，中间 40% 显示/隐藏工具栏。
      const box = canvas.getBoundingClientRect();
      if (event && box && box.width > 0) {
        const ratio = (event.clientX - box.left) / box.width;
        if (ratio < 0.3) {
          prevPage();
          return;
        }
        if (ratio > 0.7) {
          nextPage();
          return;
        }
      }
      if (isNarrowView()) {
        toggleChrome();
      }
    },
    onSwipe: (direction) => {
      if (direction === "next") {
        nextPage();
      } else {
        prevPage();
      }
    },
    onSwipeStart: (direction) => beginSwipe(direction),
    onSwipeMove: (dx) => applySwipeDrag(dx),
    onSwipeEnd: (dx, dt) => endSwipe(dx, dt),
    onSwipeCancel: () => cancelSwipe(),
    setStatus,
    onChange: () => {
      paintSelection();
    },
  });

  // ---- 字体 ---------------------------------------------------------------

  // 内嵌字体在 worker 里注册：主线程一个字节都不用拿。
  async function loadEmbeddedFonts(token) {
    const faces = await engine.fontFaces().catch((error) => ({ error: String(error.message || error) }));
    if (token !== openToken) {
      return false;
    }
    const loaded = Number(faces && faces.loaded) || 0;
    const error = (faces && faces.error) || "";
    if (error) {
      // 有字体成功时不要用状态栏盖住正常进度，但诊断仍要让人看得到。
      if (loaded) {
        console.warn(error);
      } else {
        setStatus(error);
      }
    }
    return loaded > 0;
  }

  async function loadFont(bytes) {
    await engine.font(bytes);
  }

  async function fetchFirst(urls) {
    let lastError = null;
    for (const url of urls) {
      try {
        // 不再用 no-store：字体/示例书都带版本号，交给 HTTP 缓存与 SW。
        const response = await fetch(url);
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

  async function fetchFontBytes(url) {
    const controller = typeof AbortController === "function" ? new AbortController() : null;
    const timer = setTimeout(() => controller && controller.abort(), 180000);
    try {
      const response = await fetch(url, controller ? { signal: controller.signal } : {});
      if (!response.ok) {
        throw new Error(`字体 ${response.status}: ${url}`);
      }
      if (!response.body || typeof response.body.getReader !== "function") {
        return new Uint8Array(await response.arrayBuffer());
      }
      const total = Number(response.headers.get("content-length") || 0);
      const reader = response.body.getReader();
      const chunks = [];
      let received = 0;
      for (;;) {
        const { done, value } = await reader.read();
        if (done) {
          break;
        }
        chunks.push(value);
        received += value.length;
        const mb = (received / 1048576).toFixed(1);
        if (fontProgressVisible) {
          setStatus(total ? `字体 ${mb} / ${(total / 1048576).toFixed(1)} MB` : `字体 ${mb} MB`);
        }
      }
      const bytes = new Uint8Array(received);
      let offset = 0;
      for (const chunk of chunks) {
        bytes.set(chunk, offset);
        offset += chunk.length;
      }
      return bytes;
    } finally {
      clearTimeout(timer);
    }
  }

  async function cachedFontBytes(url) {
    if (typeof caches === "undefined") {
      return null;
    }
    try {
      const response = await caches.match(url);
      return response ? new Uint8Array(await response.arrayBuffer()) : null;
    } catch (error) {
      return null;
    }
  }

  async function cacheFontBytes(url, bytes) {
    if (typeof caches === "undefined") {
      return;
    }
    try {
      const cache = await caches.open(FONT_CACHE);
      await cache.put(
        url,
        new Response(bytes, { headers: { "content-type": "font/woff2" } }),
      );
    } catch (error) {
      // 配额/隐私模式失败不影响本次阅读。
    }
  }

  function isCjkInfo(info) {
    const sample = `${info.language || ""} ${info.title || ""}`;
    return (
      /(^|[-_ ])(zh|ja|ko)\b/i.test(sample) ||
      /[\u3040-\u30ff\u3400-\u9fff\uac00-\ud7af]/.test(sample)
    );
  }

  function fontKeyFor(info, record) {
    const faces = Array.isArray(info.fonts) ? info.fonts : [];
    if (faces.length) {
      return `embedded:${(record && record.id) || info.title || "book"}:${faces.length}`;
    }
    return isCjkInfo(info) ? "default:cjk" : "default:latin";
  }

  // 同步决定这本书需要哪套字体。字体变了才重置 FontBook；
  // 同一套字体换书则全部复用，热开书不再重付首帧光栅化。
  async function planFonts(info, record) {
    const key = fontKeyFor(info, record);
    if (fontState && fontState.key === key && fontState.ok) {
      return { key, kind: fontState.kind, reuse: true };
    }
    const kind = key.startsWith("embedded:")
      ? "embedded"
      : key === "default:cjk"
        ? "cjk"
        : "latin";
    await engine.fontReset();
    builtinEl.checked = false;
    fontProgressVisible = true;
    fontState = { key, kind, ok: false };
    return { key, kind, reuse: false };
  }

  async function loadRemoteFont(token) {
    const query = new URLSearchParams(window.location.search || "");
    if (query.get("nofont") === "1") {
      throw new Error("remote font disabled");
    }
    const override = query.get("font");
    const urls = override ? [override] : CJK_FONT_URLS;
    let lastError = null;
    for (const url of urls) {
      const remote = /^https?:/i.test(url);
      try {
        // 字体 5 MB 左右，HTTP 缓存不可靠；Cache Storage 首读之后直接本地命中。
        const cached = await cachedFontBytes(url);
        if (token !== openToken) {
          return false;
        }
        if (cached) {
          await loadFont(cached);
          if (fontProgressVisible) {
            setStatus("字体来自缓存");
          }
          return token === openToken;
        }
        if (fontProgressVisible) {
          setStatus(remote ? "正在下载字体" : "正在加载本地字体");
        }
        const bytes = await fetchFontBytes(url);
        if (token !== openToken) {
          return false;
        }
        // 先开始写 Cache Storage，再解析字体：首屏不额外等一次字体落盘。
        cacheFontBytes(url, bytes);
        if (token !== openToken) {
          return false;
        }
        await loadFont(bytes);
        return token === openToken;
      } catch (error) {
        lastError = error;
      }
    }
    throw lastError || new Error("没有可用字体");
  }

  async function prepareFonts(info, plan, token) {
    if (plan.reuse) {
      return true;
    }
    let ok = false;
    if (plan.kind === "embedded") {
      ok = await loadEmbeddedFonts(token);
    } else if (plan.kind === "cjk") {
      try {
        ok = await loadRemoteFont(token);
      } catch (error) {
        if (token === openToken) {
          setStatus("未找到中文字体：使用系统字体");
        }
      }
      if (ok && token === openToken) {
        // 这是中文排版字体，不是“内置拉丁字体”，复选框保持关闭。
        builtinEl.checked = false;
      }
    } else {
      try {
        const bytes = await fetchFirst(FONT_URLS);
        if (token !== openToken) {
          return false;
        }
        await loadFont(bytes);
        ok = token === openToken;
        if (ok) {
          builtinEl.checked = true;
        }
      } catch (error) {
        if (token === openToken) {
          setStatus("未找到内置字体：使用系统字体");
        }
      }
    }
    if (ok && token === openToken && fontState && fontState.key === plan.key) {
      fontState.ok = true;
    }
    return ok && token === openToken;
  }

  // ---- 设置 ---------------------------------------------------------------

  function applyTheme(theme) {
    if (!document.body) {
      return;
    }
    document.body.classList.remove("theme-light", "theme-sepia", "theme-dark");
    document.body.classList.add(`theme-${theme}`);
  }

  function syncSettingsControls() {
    applyTheme(settings.theme);
    if (themeEl) {
      themeEl.value = settings.theme;
    }
    if (fontSizeEl) {
      fontSizeEl.value = String(settings.fontSize);
    }
    if (fontSizeLabelEl) {
      fontSizeLabelEl.textContent = `${Math.round(settings.fontSize)}px`;
    }
    if (boldEl) {
      boldEl.checked = Boolean(settings.bold);
    }
    if (lineHeightEl) {
      lineHeightEl.value = String(settings.lineHeight);
    }
    if (lineHeightLabelEl) {
      lineHeightLabelEl.textContent = `${Number(settings.lineHeight).toFixed(2)}×`;
    }
    if (letterSpacingEl) {
      letterSpacingEl.value = String(settings.letterSpacing);
    }
    if (letterSpacingLabelEl) {
      letterSpacingLabelEl.textContent = `${Number(settings.letterSpacing).toFixed(1)}px`;
    }
    if (marginEl) {
      marginEl.value = String(settings.margin);
    }
    if (marginLabelEl) {
      marginLabelEl.textContent = `${Math.round(settings.margin)}px`;
    }
    if (alignEl) {
      alignEl.value = settings.align;
    }
    if (animationEl) {
      animationEl.value = settings.animation;
    }
  }

  function normalizeSettings(input) {
    const next = { ...settings, ...input };
    next.fontSize = clamp(
      Math.round(Number(next.fontSize) || DEFAULT_SETTINGS.fontSize),
      12,
      36,
    );
    next.margin = clamp(Number(next.margin) || DEFAULT_SETTINGS.margin, 20, 120);
    next.bold = Boolean(next.bold);
    next.lineHeight = clamp(
      Number(next.lineHeight) || DEFAULT_SETTINGS.lineHeight,
      0.9,
      2.4,
    );
    next.letterSpacing = clamp(
      Number(next.letterSpacing) || 0,
      -2,
      6,
    );
    next.align = ALIGNS.includes(next.align) ? next.align : "book";
    if (!["none", "fade", "slide"].includes(next.animation)) {
      next.animation = DEFAULT_SETTINGS.animation;
    }
    if (!["light", "sepia", "dark"].includes(next.theme)) {
      next.theme = "light";
    }
    return next;
  }

  async function applySettings(next, persist = true) {
    if (animating || turn) {
      cancelAnimation();
    }
    const merged = normalizeSettings(next);
    const layoutChanged =
      merged.fontSize !== settings.fontSize ||
      merged.margin !== settings.margin ||
      merged.lineHeight !== settings.lineHeight ||
      merged.letterSpacing !== settings.letterSpacing ||
      merged.align !== settings.align;
    settings = merged;
    syncSettingsControls();
    if (persist) {
      onSettingsChange({ ...settings });
    }
    if (layoutChanged && layout) {
      await relayout();
    } else if (layout) {
      render();
    }
  }

  function setSettings(next) {
    settings = normalizeSettings(next);
    syncSettingsControls();
  }

  // ---- 事件 ---------------------------------------------------------------

  // 选区/翻页期间不要把工具栏从用户脚下抽走。
  canvas.addEventListener("pointerdown", clearChromeTimer);
  prevEl.addEventListener("click", prevPage);
  nextEl.addEventListener("click", nextPage);
  if (seekEl) {
    seekEl.addEventListener("change", () => {
      seekTo(Number(seekEl.value) / 1000).catch((error) => setStatus(String(error.message || error)));
    });
  }
  if (tocToggleEl) {
    tocToggleEl.addEventListener("click", () => {
      if (document.body) {
        document.body.classList.toggle("toc-open");
        document.body.classList.remove("settings-open");
      }
    });
  }
  if (tocCloseEl) {
    tocCloseEl.addEventListener("click", closePanelsAndRead);
  }
  if (backdropEl) {
    backdropEl.addEventListener("click", closePanelsAndRead);
  }
  if (settingsToggleEl) {
    settingsToggleEl.addEventListener("click", () => {
      if (document.body) {
        document.body.classList.toggle("settings-open");
        document.body.classList.remove("toc-open");
      }
    });
  }
  if (settingsCloseEl) {
    settingsCloseEl.addEventListener("click", closePanelsAndRead);
  }
  for (const bar of [element("reader-top"), element("reader-bottom")]) {
    if (!bar) {
      continue;
    }
    bar.addEventListener("pointerdown", clearChromeTimer);
    bar.addEventListener("pointerup", () => scheduleChromeHide(4200));
  }
  if (backEl) {
    backEl.addEventListener("click", () => {
      close();
      onBack();
    });
  }
  if (themeEl) {
    themeEl.addEventListener("change", () => {
      applySettings({ theme: themeEl.value }).catch(() => {});
    });
  }
  if (fontSizeEl) {
    fontSizeEl.addEventListener("input", () => {
      if (fontSizeLabelEl) {
        fontSizeLabelEl.textContent = `${Math.round(Number(fontSizeEl.value))}px`;
      }
    });
    fontSizeEl.addEventListener("change", () => {
      applySettings({ fontSize: Number(fontSizeEl.value) }).catch((error) =>
        setStatus(String(error.message || error)),
      );
    });
  }
  if (boldEl) {
    boldEl.addEventListener("change", () => {
      applySettings({ bold: boldEl.checked }).catch((error) =>
        setStatus(String(error.message || error)),
      );
    });
  }
  if (lineHeightEl) {
    lineHeightEl.addEventListener("input", () => {
      if (lineHeightLabelEl) {
        lineHeightLabelEl.textContent = `${Number(lineHeightEl.value).toFixed(2)}×`;
      }
    });
    lineHeightEl.addEventListener("change", () => {
      applySettings({ lineHeight: Number(lineHeightEl.value) }).catch((error) =>
        setStatus(String(error.message || error)),
      );
    });
  }
  if (letterSpacingEl) {
    letterSpacingEl.addEventListener("input", () => {
      if (letterSpacingLabelEl) {
        letterSpacingLabelEl.textContent = `${Number(letterSpacingEl.value).toFixed(1)}px`;
      }
    });
    letterSpacingEl.addEventListener("change", () => {
      applySettings({ letterSpacing: Number(letterSpacingEl.value) }).catch((error) =>
        setStatus(String(error.message || error)),
      );
    });
  }
  if (alignEl) {
    alignEl.addEventListener("change", () => {
      applySettings({ align: alignEl.value }).catch((error) =>
        setStatus(String(error.message || error)),
      );
    });
  }
  if (animationEl) {
    animationEl.addEventListener("change", () => {
      applySettings({ animation: animationEl.value }).catch((error) =>
        setStatus(String(error.message || error)),
      );
    });
  }
  if (marginEl) {
    marginEl.addEventListener("input", () => {
      if (marginLabelEl) {
        marginLabelEl.textContent = `${Math.round(Number(marginEl.value))}px`;
      }
    });
    marginEl.addEventListener("change", () => {
      applySettings({ margin: Number(marginEl.value) }).catch((error) =>
        setStatus(String(error.message || error)),
      );
    });
  }
  if (settingsResetEl) {
    settingsResetEl.addEventListener("click", () => {
      applySettings({ ...DEFAULT_SETTINGS }).catch((error) =>
        setStatus(String(error.message || error)),
      );
    });
  }
  if (builtinEl) {
    builtinEl.addEventListener("change", async () => {
      if (!book || busy) {
        return;
      }
      busy = true;
      try {
        if (builtinEl.checked) {
          setStatus("正在加载内置字体");
          await loadFont(await fetchFirst(FONT_URLS));
          // 手动换过字体：下次开书重建 FontBook，不能沿用旧的字体身份。
          fontState = null;
        } else {
          await engine.fontReset();
          fontState = null;
          setStatus("使用系统字体");
        }
        await relayout();
      } catch (error) {
        builtinEl.checked = false;
        setStatus(String(error.message || error));
      } finally {
        busy = false;
      }
    });
  }
  if (fontFileEl) {
    fontFileEl.addEventListener("change", async () => {
      const file = fontFileEl.files && fontFileEl.files[0];
      if (!file) {
        return;
      }
      busy = true;
      try {
        setStatus(`正在加载 ${file.name}`);
        await loadFont(new Uint8Array(await file.arrayBuffer()));
        fontState = null;
        await relayout();
      } catch (error) {
        setStatus(String(error.message || error));
      } finally {
        busy = false;
      }
    });
  }

  // ---- 对外 API -----------------------------------------------------------

  async function open(record, bytes) {
    if (busy) {
      throw new Error("正在处理上一本书");
    }
    cancelAnimation();
    busy = true;
    const token = ++openToken;
    try {
      setStatus("正在打开");
      const raw = bytes ? toBytes(bytes) : toBytes(await store.getBookData(record.id));
      if (!raw || !raw.length) {
        throw new Error("书籍数据不存在");
      }
      // 开书只在 worker 里发生一次：BOOK 在那边成为唯一权威，主线程拿
      // 元数据（目录/章节/内嵌字体表）就够画界面了。
      let openedText = "";
      if (record.format === "txt") {
        openedText = decodeText(raw);
        if (!openedText.trim()) {
          throw new Error("TXT 内容为空");
        }
      }
      const opened = await engine.open(
        record.format === "txt"
          ? { kind: "txt", text: openedText, name: record.name || record.title || "untitled.txt" }
          : { kind: "epub", bytes: raw },
      );
      const info = opened.book;
      book = info;
      currentRecord = record;
      baseFontSize = Number(opened.baseFontSize) || DEFAULT_SETTINGS.fontSize;
      chapter = 0;
      page = 0;
      layout = null;
      layoutDimsUsed = null;
      setChromeHidden(isNarrowView());
      const plan = await planFonts(info, record);
      // 字体和首章排版并行：缓存命中通常能在 FONT_WAIT_MS 内完成，直接等；
      // 冷启动要下载 1.5 MB 时先用系统字体画一页，字体到齐后再 relayout。
      const fontReady = prepareFonts(info, plan, token).catch((error) => {
        if (token === openToken) {
          setStatus(String(error.message || error));
        }
        return false;
      });
      const fast = plan.reuse
        ? true
        : await Promise.race([
            fontReady,
            new Promise((resolve) => setTimeout(() => resolve("pending"), FONT_WAIT_MS)),
          ]);
      if (token !== openToken) {
        return info;
      }
      await updateBook();
      const progress = await Promise.resolve(store.getProgress(record.id)).catch(() => null);
      let target = 0;
      let startChapter = 0;
      if (progress && Number.isInteger(progress.spine)) {
        startChapter = clamp(progress.spine, 0, book.spine.length - 1);
        target = progress;
      } else if (record && Number.isInteger(record.spine) && record.spine > 0) {
        startChapter = clamp(record.spine, 0, book.spine.length - 1);
      }
      if (token !== openToken) {
        return info;
      }
      await layoutChapter(startChapter, target);
      fontProgressVisible = false;
      setStatus("就绪");
      onLoaded(info, record);
      if (!plan.reuse && fast === "pending") {
        fontReady
          .then((ready) => {
            if (!ready || token !== openToken || !book) {
              return undefined;
            }
            return relayout().then(() => {
              if (token === openToken) {
                setStatus("就绪");
              }
            });
          })
          .catch((error) => {
            if (token === openToken) {
              setStatus(String(error.message || error));
            }
          });
      }
      if (isNarrowView()) {
        showHint("轻点页面显示工具栏");
      }
      return info;
    } finally {
      busy = false;
    }
  }

  function close() {
    openToken += 1;
    cancelAnimation();
    clearChromeTimer();
    hideHint();
    saveProgress();
    selection.clear();
    closePanels();
    if (document.body) {
      document.body.classList.remove("chrome-hidden");
    }
    book = null;
    currentRecord = null;
    layout = null;
    clearPageCache();
    layoutDimsUsed = null;
  }

  function refresh() {
    if (!layout) {
      return;
    }
    if (!sameDims(layoutDims(), layoutDimsUsed)) {
      relayout().catch((error) => setStatus(String(error.message || error)));
      return;
    }
    render();
  }

  return {
    /** 预热引擎（wasm 在 worker 里实例化）。 */
    ready: () => engine.ready(),
    open,
    close,
    refresh,
    nextPage,
    prevPage,
    applySettings,
    setSettings,
    getSettings: () => ({ ...settings }),
    saveProgress,
    toggleChrome,
    isOpen: () => Boolean(book),
  };
}
