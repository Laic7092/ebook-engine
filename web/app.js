import init, { eb_open, eb_font, eb_layout } from "./pkg/ebook_web.js";
import { decodeLayout } from "./decode.js";

const PAGE_W = 720;
const PAGE_H = 960;
const MARGIN = 54;
const BOOK_URLS = ["./en.epub", "../fixtures/en.epub", "fixtures/en.epub"];
const FONT_URLS = ["./test.ttf", "../fixtures/font/test.ttf", "fixtures/font/test.ttf"];
const FONT_FAMILY = "ebook-demo";

const canvas = document.getElementById("page");
const ctx = canvas.getContext("2d");
const titleEl = document.getElementById("book-title");
const metaEl = document.getElementById("book-meta");
const chapterEl = document.getElementById("chapter");
const statusEl = document.getElementById("status");
const labelEl = document.getElementById("page-label");
const prevEl = document.getElementById("prev");
const nextEl = document.getElementById("next");
const fileEl = document.getElementById("file");

let book = null;
let layout = null;
let chapter = 0;
let page = 0;
let busy = false;

function colorOf(color) {
  return `rgba(${color.r},${color.g},${color.b},${color.a / 255})`;
}

function fontOf(size) {
  return `${size}px "${FONT_FAMILY}"`;
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
  ctx.font = fontOf(run.size);
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
  canvas.style.width = `${w}px`;
  canvas.style.height = `${h}px`;
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
  labelEl.textContent = `chapter ${chapter + 1} / ${book.spine.length} · page ${shown + 1} / ${layout.pages.length}`;
  prevEl.disabled = shown === 0 && chapter === 0;
  nextEl.disabled = shown === layout.pages.length - 1 && chapter === book.spine.length - 1;
}

function setStatus(text) {
  statusEl.textContent = text;
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
  chapter = Math.max(0, Math.min(index, book.spine.length - 1));
  const bytes = eb_layout(chapter, PAGE_W, PAGE_H, MARGIN);
  if (!bytes.length) {
    throw new Error("empty layout");
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
  const buffer = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
  const face = new FontFace(FONT_FAMILY, buffer);
  await face.load();
  document.fonts.add(face);
  await document.fonts.ready;
  eb_font(bytes);
}

async function boot() {
  busy = true;
  setStatus("loading wasm");
  await init();
  setStatus("loading font");
  const fontBytes = await fetchFirst(FONT_URLS);
  await loadFont(fontBytes);
  setStatus("loading book");
  const bookBytes = await fetchFirst(BOOK_URLS);
  openBook(bookBytes);
  setStatus("ready");
  busy = false;
}

prevEl.addEventListener("click", prevPage);
nextEl.addEventListener("click", nextPage);
chapterEl.addEventListener("change", () => {
  layoutChapter(Number(chapterEl.value), 0);
});
fileEl.addEventListener("change", async () => {
  const file = fileEl.files && fileEl.files[0];
  if (!file) {
    return;
  }
  busy = true;
  setStatus("opening file");
  try {
    openBook(new Uint8Array(await file.arrayBuffer()));
    setStatus("ready");
  } catch (error) {
    setStatus(String(error.message || error));
  } finally {
    busy = false;
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
  setStatus(String(error.message || error));
  console.error(error);
});
