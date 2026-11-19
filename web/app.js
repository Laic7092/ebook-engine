import init, {
  eb_anchor,
  eb_asset,
  eb_chapter_images,
  eb_font,
  eb_font_count,
  eb_font_desc,
  eb_font_face,
  eb_font_face_bytes,
  eb_font_face_count,
  eb_font_reset,
  eb_hit,
  eb_layout,
  eb_link_at,
  eb_progress,
  eb_resolve_link,
  eb_open,
  eb_selection,
  eb_toc,
} from "./pkg/ebook_web.js";
import {
  decodeAnchor,
  decodeHit,
  decodeImages,
  decodeLayout,
  decodeLink,
  decodeProgress,
  decodeResolvedLink,
  decodeSelection,
  decodeToc,
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
const tocEl = document.getElementById("toc");
const tocToggleEl = document.getElementById("toc-toggle");

let book = null;
let layout = null;
let chapter = 0;
let page = 0;
let busy = false;
// 最近一次排版那一章的图片表（{id,href,mime}）与按 href 缓存的 ImageBitmap。
let chapterImages = [];
const imageCache = new Map();
let tocItems = [];

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
    let parsed = { bold: false, italic: false, family: null };
    try {
      parsed = JSON.parse(eb_font_desc(id));
    } catch (error) {
      parsed = { bold: false, italic: false, family: null };
    }
    fontDescs.set(id, parsed);
  }
  return fontDescs.get(id);
}

function fontOf(size, id) {
  const desc = id === undefined ? { bold: false, italic: false, family: null } : fontDesc(id);
  const parts = [];
  if (desc.italic) {
    parts.push("italic");
  }
  if (desc.bold) {
    parts.push("bold");
  }
  // 内嵌字体用 `@font-face` 的 family（浏览器侧已注册）；否则用宿主字体栈。
  const family = desc.family ? `"${desc.family}", ` : "";
  parts.push(`${size}px ${family}${FONT_FALLBACK}`);
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

// `Fit` 的编码与 IR 一致：0 = Contain、1 = Cover、2 = Fill。
const FIT_NAMES = ["contain", "cover", "fill"];

function drawImagePlaceholder(r) {
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

// 图片只画 IR 给出的 rect：Fit 是“在 rect 里怎么放”，不是重新排版。
// 字节来自 `eb_asset`，失败（缺图、解码失败、createImageBitmap 不可用）时
// 仍然画斜线占位，不白屏。
function drawImage(item, ox, oy) {
  const r = shifted(item.rect, ox, oy);
  if (r.w <= 0 || r.h <= 0) {
    return;
  }
  const entry = chapterImages.find((image) => image.id === item.id);
  const bitmap = entry && entry.href ? imageCache.get(entry.href) : null;
  if (!bitmap || !bitmap.width || !bitmap.height) {
    drawImagePlaceholder(r);
    return;
  }
  const fit = FIT_NAMES[item.fit] || "contain";
  ctx.save();
  ctx.beginPath();
  ctx.rect(r.x, r.y, r.w, r.h);
  ctx.clip();
  if (fit === "fill") {
    ctx.drawImage(bitmap, r.x, r.y, r.w, r.h);
  } else {
    const scale = fit === "cover"
      ? Math.max(r.w / bitmap.width, r.h / bitmap.height)
      : Math.min(r.w / bitmap.width, r.h / bitmap.height);
    const w = bitmap.width * scale;
    const h = bitmap.height * scale;
    ctx.drawImage(bitmap, r.x + (r.w - w) / 2, r.y + (r.h - h) / 2, w, h);
  }
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
  ctx.fontKerning = "auto";
  const x = run.origin.x - ox;
  const y = run.origin.y - oy;
  const chars = Array.from(run.text);
  const glyphs = layout.display.glyphs.slice(run.glyphs.start, run.glyphs.start + run.glyphs.len);
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
  // 排版已经把 advance 结算进每个字形（两端对齐加在空格上），所以整串
  // `fillText` 会把空格打回字体原宽、行尾缩回去。按“非空白连续块”画：块内
  // 交给浏览器整形（保留 kerning / 连字），块之间按字形 dx 前进，两端对齐
  // 的加宽由此精确复现。
  // RTL 与字形/字符不是 1:1 时退回整串绘制：顺序和簇都不可按字符硬切。
  const split = run.dir !== 1 && glyphs.length === chars.length;
  paint((startX, startY) => {
    if (!split) {
      ctx.fillText(run.text, startX, startY);
      return;
    }
    let cursor = startX;
    let wordStart = 0;
    for (let i = 0; i <= chars.length; i += 1) {
      if (i < chars.length && !/\s/u.test(chars[i])) {
        continue;
      }
      if (i > wordStart) {
        ctx.fillText(chars.slice(wordStart, i).join(""), cursor, startY);
        for (let k = wordStart; k < i; k += 1) {
          cursor += glyphs[k].dx;
        }
      }
      if (i < chars.length) {
        cursor += glyphs[i].dx;
      }
      wordStart = i + 1;
    }
  });
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

// 链接跳转：先用格式层的 `eb_resolve_link`（章节上下文里的 resolve_link_from）
// 把 href 解析成 {spine,fragment}，再切章 + 用 `eb_anchor` 查 fragment 锚点。
// 拿不到目标时才退化到宿主钩子 / 按 spine href 匹配。
function followLink(href) {
  if (!href || !book) {
    return;
  }
  let target = null;
  try {
    target = decodeResolvedLink(eb_resolve_link(href));
  } catch (error) {
    target = null;
  }
  if (target) {
    if (target.external) {
      setStatus(`外链：${href}`);
      return;
    }
    if (target.spine !== chapter) {
      layoutChapter(target.spine, 0);
    } else {
      page = 0;
      selection.clear();
      render();
    }
    if (target.fragment) {
      jumpToFragment(target.fragment);
    }
    return;
  }
  followLinkFallback(href);
}

function jumpToFragment(fragment) {
  let anchor = null;
  try {
    anchor = decodeAnchor(eb_anchor(fragment));
  } catch (error) {
    anchor = null;
  }
  if (!anchor) {
    setStatus(`找不到锚点 #${fragment}`);
    return;
  }
  jumpToAnchor(anchor);
}

// 宿主接管或按路径匹配。仅在 `eb_resolve_link` 拿不到目标时使用。
function followLinkFallback(href) {
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
    setStatus(`找不到锚点 #${fragment}`);
    return;
  }
  const index = spineIndexOfPath(rawPath);
  if (index === null) {
    setStatus(`外链或未知目标：${href}`);
    return;
  }
  layoutChapter(index, 0);
  if (fragment) {
    jumpToFragment(fragment);
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

// ---- 目录 -----------------------------------------------------------------
// `eb_toc` 返回扁平列表 + depth；这里按 depth 拼成嵌套 <ul>。
function fillToc() {
  if (!tocEl) {
    return;
  }
  let entries = [];
  try {
    entries = decodeToc(eb_toc());
  } catch (error) {
    entries = [];
  }
  tocEl.replaceChildren();
  tocItems = [];
  if (!entries.length) {
    return;
  }
  const root = document.createElement("ul");
  // levels 栈：每一项是一个缩进层，lastLi 是该层最后一个 <li>（嵌套列表挂在它下面）。
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
      button.addEventListener("click", () => goToToc(entry));
    }
    li.append(button);
    level.list.append(li);
    level.lastLi = li;
    tocItems.push({ entry, button });
  }
  tocEl.append(root);
  highlightToc();
}

function goToToc(entry) {
  if (entry.spine === null) {
    return;
  }
  layoutChapter(entry.spine, 0);
  if (entry.fragment) {
    jumpToFragment(entry.fragment);
  }
  closeTocOnNarrow();
}

function highlightToc() {
  for (const item of tocItems) {
    const active = item.entry.spine === chapter;
    item.button.className = active ? "toc-entry active" : "toc-entry";
  }
}

function closeTocOnNarrow() {
  if (typeof window !== "undefined" && window.innerWidth < 1000 && document.body) {
    document.body.classList.remove("toc-open");
  }
}

// ---- 图片 -----------------------------------------------------------------
// 图片字节来自 wasm 的 `eb_asset`，按 `ImageMeta.src`（归档路径）缓存。
// Fit 三态在 drawImage 里结算；这里只负责拿到 bitmap，失败保留占位。
async function loadChapterImages() {
  let entries = [];
  try {
    entries = decodeImages(eb_chapter_images());
  } catch (error) {
    entries = [];
  }
  chapterImages = entries;
  if (typeof createImageBitmap !== "function" || typeof Blob !== "function") {
    return;
  }
  let changed = false;
  for (const entry of entries) {
    if (!entry.href || imageCache.has(entry.href)) {
      continue;
    }
    try {
      const bytes = eb_asset(entry.href);
      if (!bytes || !bytes.length) {
        continue;
      }
      const blob = new Blob([bytes], { type: entry.mime || "application/octet-stream" });
      imageCache.set(entry.href, await createImageBitmap(blob));
      changed = true;
    } catch (error) {
      // 标记为“试过了”：同一章反复渲染时不要再重试坏图，继续画占位。
      imageCache.set(entry.href, null);
    }
  }
  if (changed) {
    render();
  }
}

// ---- 内嵌字体 ---------------------------------------------------------------
// wasm 侧的 FontBook 负责度量，这里再把同一份字节注册给浏览器的 FontFace，
// 否则量到的和画出来的不是同一张脸。
async function loadEmbeddedFonts(info) {
  const faces = Array.isArray(info.fonts) ? info.fonts : [];
  if (!faces.length) {
    return;
  }
  const count = eb_font_face_count();
  for (let index = 0; index < count; index += 1) {
    if (eb_font_face(index) === 0xffffffff) {
      continue;
    }
    const family = faces[index] && faces[index].family;
    if (!family || typeof FontFace !== "function" || !document.fonts) {
      continue;
    }
    try {
      const bytes = eb_font_face_bytes(index);
      const face = new FontFace(family, bytes.slice().buffer);
      document.fonts.add(await face.load());
    } catch (error) {
      // 浏览器解析不了（或者被 CSP 拦）：布局仍用 FontBook 的量度。
    }
  }
  fontDescs.clear();
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
  fillToc();
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
  highlightToc();
  render();
  // 图片解码是异步的；先按占位排版，拿到 bitmap 后只重画一次。
  loadChapterImages();
}

async function openBook(bytes) {
  const info = JSON.parse(eb_open(bytes));
  if (info.error) {
    throw new Error(info.error);
  }
  clearError();
  book = info;
  chapter = 0;
  // 先把内嵌字体装进 wasm 与浏览器，再排版，避免第一页用错字脸。
  await loadEmbeddedFonts(info);
  // 没有内嵌字体的书（仓库自带的 en.epub 就是）必须有一个真实量度，否则
  // 排版退回固定 advance 的 stub，而画布用宿主字体：量出来的和画出来的不是
  // 同一套宽度，词内就会裂开。内置字体只在这时兜底，不覆盖内嵌字体。
  if (eb_font_count() === 0) {
    try {
      await loadFont(await fetchFirst(FONT_URLS));
      builtinEl.checked = true;
    } catch (error) {
      setStatus("no font: layout falls back to fixed advances");
    }
  }
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
      await openBook(bookBytes);
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
    await openBook(new Uint8Array(await file.arrayBuffer()));
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

if (tocToggleEl) {
  tocToggleEl.addEventListener("click", () => {
    if (document.body) {
      document.body.classList.toggle("toc-open");
    }
  });
}

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
