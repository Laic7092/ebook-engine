import init, {
  eb_open,
  eb_font,
  eb_font_count,
  eb_font_desc,
  eb_font_reset,
  eb_hit,
  eb_layout,
  eb_link_at,
  eb_progress,
  eb_selection,
} from "./pkg/ebook_web.js";
import {
  decodeHit,
  decodeLayout,
  decodeLink,
  decodeProgress,
  decodeSelection,
} from "./decode.js";
import { createSelection } from "./selection.js";

const PAGE_W = 720;
const PAGE_H = 960;
const MARGIN = 54;
const MIN_SCALE = 0.55;
const BOOK_URLS = ["./en.epub", "../fixtures/en.epub", "fixtures/en.epub"];
const FONT_URLS = ["./test.ttf", "../fixtures/font/test.ttf", "fixtures/font/test.ttf"];
const FONT_FAMILY = "ebook-demo";
const FONT_FALLBACK =
  '"ebook-demo", "Noto Serif CJK SC", "Songti SC", "Source Han Serif SC", "Noto Sans CJK SC", Georgia, serif';

const canvas = document.getElementById("page");
const ctx = canvas.getContext("2d");
const titleEl = document.getElementById("book-title");
const metaEl = document.getElementById("book-meta");
const chapterEl = document.getElementById("chapter");
const statusEl = document.getElementById("status");
const labelEl = document.getElementById("page-label");
const progressEl = document.getElementById("progress");
const prevEl = document.getElementById("prev");
const nextEl = document.getElementById("next");
const fileEl = document.getElementById("file");
const fontFileEl = document.getElementById("font-file");
const builtinEl = document.getElementById("builtin-font");
const fitEl = document.getElementById("fit");
const bannerEl = document.getElementById("banner");

let book = null;
let layout = null;
let chapter = 0;
let page = 0;
let busy = false;

// 交互查询只对最近一次排版的章节有效；回调里再取 layout/page，避免闭包过期。
const selection = createSelection({
  canvas,
  page: () => (layout ? { index: page, info: layout.pages[page], layout } : null),
  hit: (index, x, y) => {
    try {
      return decodeHit(eb_hit(index, x, y));
    } catch (error) {
      return null;
    }
  },
  select: (a, b) => decodeSelection(eb_selection(a.page, a.line, a.byte, b.page, b.line, b.byte)),
  linkAt: (index, x, y) => {
    try {
      return decodeLink(eb_link_at(index, x, y));
    } catch (error) {
      return null;
    }
  },
  onLink: (href) => {
    followLink(href);
  },
  setStatus,
  onChange: () => {
    if (layout) {
      render();
    }
  },
});

function colorOf(color) {
  return `rgba(${color.r},${color.g},${color.b},${color.a / 255})`;
}

const fontDescs = new Map();

function fontDesc(id) {
  if (!fontDescs.has(id)) {
    let parsed = { bold: false, italic: false };
    try {
      parsed = JSON.parse(eb_font_desc(id));
    } catch (error) {
      parsed = { bold: false, italic: false };
    }
    fontDescs.set(id, parsed);
  }
  return fontDescs.get(id);
}

function fontOf(size, id) {
  const desc = id === undefined ? { bold: false, italic: false } : fontDesc(id);
  const parts = [];
  if (desc.italic) {
    parts.push("italic");
  }
  if (desc.bold) {
    parts.push("bold");
  }
  parts.push(`${size}px ${FONT_FALLBACK}`);
  return parts.join(" ");
}

function shifted(rect, ox, oy) {
  return { x: rect.x - ox, y: rect.y - oy, w: rect.w, h: rect.h };
}

function drawRect(rect, color) {
  ctx.fillStyle = colorOf(color);
  ctx.fillRect(rect.x, rect.y, rect.w, rect.h);
}

function drawBorder(border, ox, oy) {
  const r = shifted(border.rect, ox, oy);
  const w = border.width;
  ctx.fillStyle = colorOf(border.color);
  const parts = [
    { x: r.x, y: r.y, w: r.w, h: w.top },
    { x: r.x, y: r.y + r.h - w.bottom, w: r.w, h: w.bottom },
    { x: r.x, y: r.y, w: w.left, h: r.h },
    { x: r.x + r.w - w.right, y: r.y, w: w.right, h: r.h },
  ];
  for (const part of parts) {
    if (part.w > 0 && part.h > 0) {
      ctx.fillRect(part.x, part.y, part.w, part.h);
    }
  }
}

function drawUnresolved(item, ox, oy) {
  const r = shifted(item.rect, ox, oy);
  ctx.save();
  ctx.fillStyle = "rgba(255,245,245,0.65)";
  ctx.fillRect(r.x, r.y, r.w, r.h);
  ctx.setLineDash([5, 4]);
  ctx.strokeStyle = "#c23d3d";
  ctx.lineWidth = 1;
  ctx.strokeRect(r.x + 0.5, r.y + 0.5, Math.max(0, r.w - 1), Math.max(0, r.h - 1));
  ctx.setLineDash([]);
  const names = ["image pending", "font missing", "size from content"];
  const text = names[item.reason] || "unresolved";
  ctx.fillStyle = "#9d3333";
  ctx.font = fontOf(11);
  ctx.textAlign = "left";
  ctx.fillText(text, r.x + 6, r.y + 16);
  ctx.restore();
}

function drawImage(item, ox, oy) {
  const r = shifted(item.rect, ox, oy);
  ctx.save();
  ctx.beginPath();
  ctx.rect(r.x, r.y, r.w, r.h);
  ctx.clip();
  ctx.strokeStyle = "#d0d0d0";
  ctx.lineWidth = 1;
  ctx.beginPath();
  const first = Math.floor((r.x + r.y) / 8) * 8;
  const last = r.x + r.y + r.w + r.h;
  for (let line = first; line <= last; line += 8) {
    ctx.moveTo(line - r.y, r.y);
    ctx.lineTo(line - (r.y + r.h), r.y + r.h);
  }
  ctx.stroke();
  ctx.strokeStyle = "#999999";
  ctx.setLineDash([4, 4]);
  ctx.strokeRect(r.x + 0.5, r.y + 0.5, Math.max(0, r.w - 1), Math.max(0, r.h - 1));
  ctx.setLineDash([]);
  ctx.restore();
}

function drawRun(run, ox, oy) {
  if (!run.text) {
    return;
  }
  ctx.save();
  ctx.font = fontOf(run.size, run.font);
  ctx.fillStyle = colorOf(run.color);
  ctx.textAlign = "left";
  ctx.textBaseline = "alphabetic";
  ctx.direction = run.dir === 1 ? "rtl" : "ltr";
  ctx.fontKerning = "none";
  const x = run.origin.x - ox;
  const y = run.origin.y - oy;
  const chars = Array.from(run.text);
  const glyphs = layout.display.glyphs.slice(run.glyphs.start, run.glyphs.start + run.glyphs.len);
  let advance = 0;
  for (const glyph of glyphs) {
    advance += glyph.dx;
  }
  const paint = (fn) => {
    if (run.rotate === 1) {
      ctx.save();
      ctx.translate(x, y);
      ctx.rotate(Math.PI / 2);
      fn(0, 0);
      ctx.restore();
    } else {
      fn(x, y);
    }
  };
  if (glyphs.length === chars.length && Math.abs(ctx.measureText(run.text).width - advance) > 0.35) {
    paint((startX, startY) => {
      let cursor = startX;
      for (let i = 0; i < chars.length; i += 1) {
        ctx.fillText(chars[i], cursor, startY);
        cursor += glyphs[i].dx;
      }
    });
  } else {
    paint((startX, startY) => ctx.fillText(run.text, startX, startY));
  }
  ctx.restore();
}

function drawOp(op, ox, oy) {
  if (op.type === "glyphRun") {
    drawRun(op.run, ox, oy);
  } else if (op.type === "rect") {
    drawRect(shifted(op.rect, ox, oy), op.color);
  } else if (op.type === "border") {
    drawBorder(op.border, ox, oy);
  } else if (op.type === "image") {
    drawImage(op, ox, oy);
  } else if (op.type === "unresolved") {
    drawUnresolved(op.unresolved, ox, oy);
  } else if (op.type === "clip") {
    const r = shifted(op.rect, ox, oy);
    ctx.save();
    ctx.beginPath();
    ctx.rect(r.x, r.y, r.w, r.h);
    ctx.clip();
    for (const child of op.ops) {
      drawOp(child, ox, oy);
    }
    ctx.restore();
  }
}

function render() {
  if (!layout || !layout.pages.length) {
    return;
  }
  const shown = Math.max(0, Math.min(page, layout.pages.length - 1));
  page = shown;
  const info = layout.pages[shown];
  const dpr = Math.min(window.devicePixelRatio || 1, 2);
  const w = (info.size.w || PAGE_W) + info.inset.left + info.inset.right;
  const h = (info.size.h || PAGE_H) + info.inset.top + info.inset.bottom;
  canvas.width = Math.round(w * dpr);
  canvas.height = Math.round(h * dpr);
  const scale = fitEl && fitEl.checked ? Math.min(1, (window.innerHeight - 150) / h) : 1;
  const shown_w = Math.round(w * Math.max(scale, MIN_SCALE));
  const shown_h = Math.round(h * Math.max(scale, MIN_SCALE));
  canvas.style.width = `${shown_w}px`;
  canvas.style.height = `${shown_h}px`;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.fillStyle = "#ffffff";
  ctx.fillRect(0, 0, w, h);
  ctx.fontKerning = "none";
  ctx.textBaseline = "alphabetic";
  const ox = info.origin.x - info.inset.left;
  const oy = info.origin.y - info.inset.top;
  const start = info.ops.start;
  const end = start + info.ops.len;
  for (let i = start; i < end; i += 1) {
    drawOp(layout.display.ops[i], ox, oy);
  }
  selection.draw(ctx, shown);
  labelEl.textContent = `chapter ${chapter + 1} / ${book.spine.length} · page ${shown + 1} / ${layout.pages.length}`;
  if (progressEl) {
    progressEl.textContent = `全书 ${(bookProgress(shown) * 100).toFixed(1)}%`;
  }
  prevEl.disabled = shown === 0 && chapter === 0;
  nextEl.disabled = shown === layout.pages.length - 1 && chapter === book.spine.length - 1;
}

// 全书进度 = (已完成章节 + 本章进度) / 章节数。本章进度来自 IR 的行号，
// 不是像素，所以换页面尺寸也稳定。
function bookProgress(shown) {
  const info = layout.pages[shown];
  const inChapter = decodeProgress(eb_progress(shown, info.lines.start, 0));
  const total = book && book.spine.length ? book.spine.length : 1;
  return Math.min(1, Math.max(0, (chapter + inChapter) / total));
}

// 链接跳转：宿主可以通过 window.ebookResolveLink(href) 接管解析，
// 返回 { spine, page } 或 { spine, path, offset }（Anchor 形状）；否则退化：
// 跨文件的按 spine href 找到目标章节并从页首开始，同章锚点暂不支持（格式层还没
// 把 id → Anchor 的映射带进来）。
function followLink(href) {
  if (!href || !book) {
    return;
  }
  const host = typeof window !== "undefined" ? window.ebookResolveLink : null;
  if (typeof host === "function") {
    let target = null;
    try {
      target = host(href);
    } catch (error) {
      target = null;
    }
    if (target && typeof target === "object") {
      if (Number.isInteger(target.page)) {
        layoutChapter(Number(target.spine || 0), target.page);
        return;
      }
      if (Array.isArray(target.path)) {
        jumpToAnchor(target);
        return;
      }
    }
    setStatus(`链接未解析：${href}`);
    return;
  }
  const [rawPath, fragment] = splitHref(href);
  if (!rawPath) {
    // 同章锚点：#fragment 需要宿主把 fragment 解析成 Anchor 才能 locate。
    setStatus(`章节内锚点需要宿主 resolveLink：${href}`);
    return;
  }
  const index = spineIndexOfPath(rawPath);
  if (index === null) {
    setStatus(`外链或未知目标：${href}`);
    return;
  }
  if (index === chapter) {
    setStatus(fragment ? `章节内锚点需要宿主 resolveLink：${href}` : href);
    return;
  }
  layoutChapter(index, 0);
  if (fragment) {
    setStatus(`已切到第 ${index + 1} 章；锚点 ${fragment} 需要宿主 resolveLink`);
  }
}

function jumpToAnchor(anchor) {
  const index = Number(anchor.spine || 0);
  if (index !== chapter) {
    layoutChapter(index, 0);
  }
  const line = locateAnchor(anchor);
  if (line === null) {
    setStatus("锚点不在本章");
    return;
  }
  const target = layout.pages.findIndex(
    (info) => line >= info.lines.start && line < info.lines.start + info.lines.len,
  );
  if (target >= 0) {
    page = target;
    selection.clear();
    render();
  }
}

// 与 IR 的 `LayoutResult::location_of` 同一套语义：同 spine + path，offset 落在
// 哪一行 `anchor.offset .. anchor.offset + src.len` 里就是哪一行，超出夹到末尾。
function locateAnchor(anchor) {
  if (!layout) {
    return null;
  }
  let last = null;
  for (let index = 0; index < layout.runs.lines.length; index += 1) {
    const line = layout.runs.lines[index];
    const start = line.anchor;
    if (!start || start.spine !== anchor.spine) {
      continue;
    }
    if (!samePath(start.path, anchor.path)) {
      continue;
    }
    const byte = anchor.offset - start.offset;
    if (byte >= 0 && byte <= line.src.len) {
      return index;
    }
    last = index;
  }
  return last;
}

function samePath(a, b) {
  return Array.isArray(a) && Array.isArray(b) && a.length === b.length && a.every((v, i) => v === b[i]);
}

function splitHref(href) {
  const hash = href.indexOf("#");
  if (hash < 0) {
    return [href, ""];
  }
  return [href.slice(0, hash), href.slice(hash + 1)];
}

function spineIndexOfPath(path) {
  const clean = (value) => value.replace(/^\.\//, "").toLowerCase();
  const want = clean(path);
  if (!want) {
    return null;
  }
  const current = book.spine[chapter];
  const dir = current ? current.href.replace(/[^\/]*$/, "") : "";
  for (let index = 0; index < book.spine.length; index += 1) {
    const href = clean(book.spine[index].href);
    if (href === want || href === clean(dir + want) || href.endsWith(`/${want}`)) {
      return index;
    }
  }
  return null;
}

function setStatus(text) {
  statusEl.textContent = text;
}

function showError(message) {
  setStatus(message);
  if (!bannerEl) {
    return;
  }
  bannerEl.textContent = message;
  bannerEl.hidden = false;
}

function clearError() {
  if (bannerEl) {
    bannerEl.hidden = true;
    bannerEl.textContent = "";
  }
}

function fillChapters() {
  chapterEl.replaceChildren();
  book.spine.forEach((item, index) => {
    const option = document.createElement("option");
    option.value = String(index);
    option.textContent = `${index + 1}. ${item.id || item.href}`;
    chapterEl.append(option);
  });
  chapterEl.value = String(chapter);
}

function updateBook() {
  titleEl.textContent = book.title || "untitled";
  metaEl.textContent = book.language || book.lang || "";
  fillChapters();
}

function layoutChapter(index, targetPage) {
  selection.clear();
  chapter = Math.max(0, Math.min(index, book.spine.length - 1));
  const bytes = eb_layout(chapter, PAGE_W, PAGE_H, MARGIN);
  if (!bytes.length) {
    throw new Error(`chapter ${chapter + 1} 排版失败`);
  }
  layout = decodeLayout(bytes);
  page = targetPage === "last" ? layout.pages.length - 1 : Number(targetPage || 0);
  chapterEl.value = String(chapter);
  render();
}

function openBook(bytes) {
  const info = JSON.parse(eb_open(bytes));
  if (info.error) {
    throw new Error(info.error);
  }
  clearError();
  book = info;
  chapter = 0;
  updateBook();
  layoutChapter(0, 0);
}

function nextPage() {
  if (!layout) {
    return;
  }
  if (page + 1 < layout.pages.length) {
    selection.clear();
    page += 1;
    render();
  } else if (chapter + 1 < book.spine.length) {
    layoutChapter(chapter + 1, 0);
  }
}

function prevPage() {
  if (!layout) {
    return;
  }
  if (page > 0) {
    selection.clear();
    page -= 1;
    render();
  } else if (chapter > 0) {
    layoutChapter(chapter - 1, "last");
  }
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

async function loadFont(bytes) {
  eb_font(bytes);
  const buffer = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
  try {
    const face = new FontFace(FONT_FAMILY, buffer);
    await face.load();
    document.fonts.add(face);
    await document.fonts.ready;
  } catch (error) {
    setStatus("font loaded for layout only");
  }
  fontDescs.clear();
}

async function relayout() {
  if (!book) {
    return;
  }
  busy = true;
  try {
    layoutChapter(chapter, page);
  } catch (error) {
    setStatus(String(error.message || error));
  } finally {
    busy = false;
  }
}

async function boot() {
  busy = true;
  clearError();
  if (window.location.protocol === "file:") {
    busy = false;
    showError(
      "index.html 被直接打开了。ES 模块和 wasm 不能从 file:// 加载：在仓库根目录执行 python3 -m http.server 8765，再打开 http://localhost:8765/web/",
    );
    return;
  }
  try {
    setStatus("loading wasm");
    try {
      await init();
    } catch (error) {
      showError(`wasm 加载失败：${error.message || error}。请确认 web/pkg/ 已构建（./web/build.sh），并且是通过 http 访问。`);
      return;
    }
    const query = new URLSearchParams(window.location.search);
    const requested = query.get("book");
    const urls = requested ? [requested, resolveBookUrl(requested)] : BOOK_URLS;
    setStatus("loading book");
    try {
      const bookBytes = await fetchFirst(urls);
      openBook(bookBytes);
      setStatus("ready");
    } catch (error) {
      setStatus("pick an epub file to start");
    }
  } finally {
    busy = false;
  }
}

function resolveBookUrl(path) {
  const base = new URL("./", window.location.href);
  return new URL(path, base).href;
}

prevEl.addEventListener("click", prevPage);
nextEl.addEventListener("click", nextPage);
chapterEl.addEventListener("change", () => {
  layoutChapter(Number(chapterEl.value), 0);
});
async function loadBookFile(file) {
  busy = true;
  setStatus(`opening ${file.name}`);
  try {
    openBook(new Uint8Array(await file.arrayBuffer()));
    setStatus(`${book.spine.length} chapters`);
  } catch (error) {
    showError(`打不开 ${file.name}：${error.message || error}`);
  } finally {
    busy = false;
  }
}

fileEl.addEventListener("change", () => {
  const file = fileEl.files && fileEl.files[0];
  if (file) {
    loadBookFile(file);
  }
});

fontFileEl.addEventListener("change", async () => {
  const file = fontFileEl.files && fontFileEl.files[0];
  if (!file) {
    return;
  }
  busy = true;
  setStatus(`loading ${file.name}`);
  try {
    await loadFont(new Uint8Array(await file.arrayBuffer()));
    setStatus(`fonts: ${eb_font_count()}`);
    await relayout();
  } catch (error) {
    setStatus(String(error.message || error));
  } finally {
    busy = false;
  }
});

builtinEl.addEventListener("change", async () => {
  busy = true;
  try {
    if (builtinEl.checked) {
      setStatus("loading builtin font");
      await loadFont(await fetchFirst(FONT_URLS));
      setStatus(`fonts: ${eb_font_count()}`);
    } else {
      eb_font_reset();
      fontDescs.clear();
      setStatus("browser fonts");
    }
    await relayout();
  } catch (error) {
    builtinEl.checked = false;
    setStatus(String(error.message || error));
  } finally {
    busy = false;
  }
});

fitEl.addEventListener("change", () => {
  selection.clear();
  if (layout) {
    render();
  }
});

document.addEventListener("dragover", (event) => {
  event.preventDefault();
});

document.addEventListener("drop", (event) => {
  event.preventDefault();
  const file = event.dataTransfer && event.dataTransfer.files && event.dataTransfer.files[0];
  if (!file) {
    return;
  }
  if (/\.(ttf|otf)$/i.test(file.name)) {
    fontFileEl.files = event.dataTransfer.files;
    fontFileEl.dispatchEvent(new Event("change"));
  } else {
    loadBookFile(file);
  }
});

window.addEventListener("resize", () => {
  // 缩放会改变画布 CSS 尺寸，旧选区的页内坐标换算前提变了，直接清掉。
  selection.clear();
  if (layout) {
    render();
  }
});
window.addEventListener("keydown", (event) => {
  if (busy) {
    return;
  }
  if (event.key === "ArrowRight" || event.key === "PageDown" || event.key === " ") {
    event.preventDefault();
    nextPage();
  } else if (event.key === "ArrowLeft" || event.key === "PageUp") {
    event.preventDefault();
    prevPage();
  }
});
boot().catch((error) => {
  showError(String(error.message || error));
  console.error(error);
});
