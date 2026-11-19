// 阅读页：把最近一次 IR 画到 canvas，并维护章节/页码/目录/字体/进度。
//
// 这个模块只面向已经打开的书。书架视图、导入与持久化由 app.js / library.js
// 负责；这里通过 options.store 读写进度和字体缓存，方便测试替换。

import {
  eb_anchor,
  eb_asset,
  eb_base_font_size,
  eb_chapter_images,
  eb_font,
  eb_font_count,
  eb_font_desc,
  eb_font_face,
  eb_font_face_bytes,
  eb_font_face_count,
  eb_font_reset,
  eb_glyph_bitmap,
  eb_hit,
  eb_last_error,
  eb_layout_style,
  eb_link_at,
  eb_open,
  eb_open_text,
  eb_progress,
  eb_resolve_link,
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
import { decodeText } from "./text.js";

const PAGE_W = 720;
const PAGE_H = 960;
const FONT_URLS = ["./latin.ttf", "../fixtures/font/test.ttf", "fixtures/font/test.ttf"];
const FONT_FAMILY = "ebook-demo";
const CJK_FONT_URLS = [
  // 随仓库提供，本地服务器直接读，不再等 CDN。下面的远程地址只作兜底。
  "./cjk.ttf.gz?v=4",
  "./cjk.ttf?v=4",
];
const FONT_FALLBACK =
  '"ebook-demo", "PingFang SC", "Hiragino Sans GB", "Noto Sans CJK SC", "Source Han Sans SC", "Microsoft YaHei", Georgia, serif';
const FIT_NAMES = ["contain", "cover", "fill"];
// 测试环境没有真正的 canvas，用 `?atlas=0` 回到 fillText，仍能检查排版/交互。
const ATLAS_DISABLED = /(?:^|[?&])atlas=0(?:&|$)/.test(window.location.search || "");
// 字形不再一 gid 一张 canvas：写进固定大小的 atlas，按颜色分组。
const GLYPH_ATLAS_SIZE = 2048;
const GLYPH_ATLAS_MAX = 3;
const GLYPH_ATLAS_PAD = 1;
const GLYPH_SCRATCH = 256;
const GLYPH_CACHE_MAX = 16384;
const FONT_CACHE = "ebook-fonts-v1";
// 字体没就绪时先画系统字体，超过这个时间就不再等；通常缓存命中会低于它。
const FONT_WAIT_MS = 60;

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
};

const ALIGN_CODES = { book: 0, start: 1, center: 2, end: 3, justify: 4 };
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

function colorOf(color) {
  return `rgba(${color.r},${color.g},${color.b},${color.a / 255})`;
}

function shifted(rect, ox, oy) {
  return { x: rect.x - ox, y: rect.y - oy, w: rect.w, h: rect.h };
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

export function createReader(options) {
  const store = options.store;
  const onStatus = options.onStatus || (() => {});
  const onSettingsChange = options.onSettingsChange || (() => {});
  const onBack = options.onBack || (() => {});
  const onLoaded = options.onLoaded || (() => {});

  const canvas = element("page");
  const ctx = canvas.getContext("2d");
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
  let chapterImages = [];
  let tocItems = [];
  const imageCache = new Map();
  const fontDescs = new Map();
  let layoutToken = 0;
  let layoutDimsUsed = null;
  let baseFontSize = 18;
  let busy = false;
  let progressTimer = 0;
  let chromeTimer = 0;
  let hintTimer = 0;
  // 自己按 gid 画字：字形、粗体、斜体全部来自 FontBook，不再让浏览器合成。
  let atlasReady = false;
  let renderDpr = 1;
  const glyphSprites = new Map();
  const glyphAtlases = [];
  const retiredGlyphAtlases = [];
  let atlasClock = 0;
  // 字体状态按“这本书需要哪套字体”复用：同一套字体换书不再重置 FontBook，
  // atlas 与 font id 都保持有效，热开书省掉首帧的整页光栅化。
  let fontState = null;
  // 首屏已经画出来之后，字体后台加载不再往状态栏刷进度，避免“就绪”又被
  // “字体 1.2 / 1.5 MB”盖掉。
  let fontProgressVisible = true;
  let openToken = 0;

  function hasClass(name) {
    const list = document.body && document.body.classList;
    return Boolean(list && typeof list.contains === "function" && list.contains(name));
  }

  function setStatus(text) {
    statusEl.textContent = text;
    onStatus(text);
  }

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

  function fontOf(size, id, weight, style) {
    const desc = id === undefined ? { bold: false, italic: false, family: null } : fontDesc(id);
    const parts = [];
    if (style === 1 || style === 2 || desc.italic) {
      parts.push("italic");
    }
    // 粗细不交给 canvas 的 `bold`：iOS 对自定义/回退字体经常不合成。
    // 真正的粗体由 drawRun 用 strokeText 描边，保证每台设备都看得出区别。
    const family = desc.family ? `"${desc.family}", ` : "";
    parts.push(`${size}px ${family}${FONT_FALLBACK}`);
    return parts.join(" ");
  }

  function needsBoldStroke(id, weight) {
    const desc = id === undefined ? {} : fontDesc(id);
    const wanted = settings.bold ? 700 : Number(weight) || desc.weight || 400;
    const actual = Number(desc.weight) || 400;
    return wanted >= 600 && actual < 600;
  }

  // 全局粗体：run 已经要求粗体就保持；否则把 base face 换成 FontBook 的
  // synthetic bold，保留原有 synthetic italic 位。
  function boldFontOf(id, weight) {
    if (!settings.bold || (Number(weight) || 400) >= 600) {
      return id;
    }
    const desc = id === undefined ? {} : fontDesc(id);
    if ((Number(desc.weight) || 400) >= 600) {
      return id;
    }
    return 0x9000 | ((id || 0) & 0x2000) | ((id || 0) & 0x0fff);
  }

  function createGlyphAtlas() {
    const canvas =
      typeof OffscreenCanvas === "function"
        ? new OffscreenCanvas(GLYPH_ATLAS_SIZE, GLYPH_ATLAS_SIZE)
        : document.createElement("canvas");
    canvas.width = GLYPH_ATLAS_SIZE;
    canvas.height = GLYPH_ATLAS_SIZE;
    return {
      canvas,
      ctx: canvas.getContext("2d"),
      x: 0,
      y: 0,
      rowH: 0,
      lastUsed: ++atlasClock,
      scratch: null,
    };
  }

  function clearGlyphSprites() {
    glyphSprites.clear();
    glyphAtlases.length = 0;
    retiredGlyphAtlases.length = 0;
  }

  function enableGlyphAtlas() {
    atlasReady = !ATLAS_DISABLED;
    clearGlyphSprites();
  }

  // 简单的 shelf 装箱：每个字形按原尺寸写进 atlas，行满了换行。
  function atlasAlloc(atlas, w, h) {
    if (w + GLYPH_ATLAS_PAD > GLYPH_ATLAS_SIZE || h + GLYPH_ATLAS_PAD > GLYPH_ATLAS_SIZE) {
      return null;
    }
    if (atlas.x + w + GLYPH_ATLAS_PAD > GLYPH_ATLAS_SIZE) {
      atlas.x = 0;
      atlas.y += atlas.rowH + GLYPH_ATLAS_PAD;
      atlas.rowH = 0;
    }
    if (atlas.y + h + GLYPH_ATLAS_PAD > GLYPH_ATLAS_SIZE) {
      return null;
    }
    const sx = atlas.x;
    const sy = atlas.y;
    atlas.x += w + GLYPH_ATLAS_PAD;
    if (h > atlas.rowH) {
      atlas.rowH = h;
    }
    atlas.lastUsed = ++atlasClock;
    return [sx, sy];
  }

  function discardGlyphAtlas(atlas) {
    if (atlas.ctx && typeof atlas.ctx.clearRect === "function") {
      atlas.ctx.clearRect(0, 0, GLYPH_ATLAS_SIZE, GLYPH_ATLAS_SIZE);
    }
    atlas.x = 0;
    atlas.y = 0;
    atlas.rowH = 0;
    atlas.scratch = null;
    if (atlas.canvas.__sprites) {
      atlas.canvas.__sprites.clear();
    }
  }

  // 淘汰的 atlas 不能立刻擦：同一帧里可能已经有它的 sprite 排进 placed。
  // 先退休，等 render 把这一页画完再统一清。
  function flushRetiredGlyphAtlases() {
    if (!retiredGlyphAtlases.length) {
      return;
    }
    for (const atlas of retiredGlyphAtlases) {
      discardGlyphAtlas(atlas);
    }
    retiredGlyphAtlases.length = 0;
  }

  // 所有颜色共用同一个 atlas 池：canvas 里存的就是已经染好色的蒙版，
  // 换颜色不需要换图。满了按 LRU 淘汰单张，不整池清空。
  function pickGlyphAtlas(w, h) {
    if (w + GLYPH_ATLAS_PAD > GLYPH_ATLAS_SIZE || h + GLYPH_ATLAS_PAD > GLYPH_ATLAS_SIZE) {
      return null;
    }
    for (const atlas of glyphAtlases) {
      const pos = atlasAlloc(atlas, w, h);
      if (pos) {
        return { atlas, pos };
      }
    }
    if (glyphAtlases.length < GLYPH_ATLAS_MAX) {
      const atlas = createGlyphAtlas();
      glyphAtlases.push(atlas);
      const pos = atlasAlloc(atlas, w, h);
      return pos ? { atlas, pos } : null;
    }
    let victimIndex = 0;
    for (let i = 1; i < glyphAtlases.length; i += 1) {
      if (glyphAtlases[i].lastUsed < glyphAtlases[victimIndex].lastUsed) {
        victimIndex = i;
      }
    }
    const victim = glyphAtlases.splice(victimIndex, 1)[0];
    // 从索引里摘掉，但像素留到 render 末尾再擦。
    for (const [key, sprite] of glyphSprites) {
      if (sprite && sprite.atlas === victim) {
        glyphSprites.delete(key);
      }
    }
    retiredGlyphAtlases.push(victim);
    const atlas = createGlyphAtlas();
    glyphAtlases.push(atlas);
    const pos = atlasAlloc(atlas, w, h);
    return pos ? { atlas, pos } : null;
  }

  function writeGlyph(atlas, sx, sy, w, h, packed, color) {
    let image = atlas.scratch;
    if (!image || image.width < w || image.height < h) {
      const side = Math.max(GLYPH_SCRATCH, w, h);
      image = atlas.ctx.createImageData(side, side);
      atlas.scratch = image;
    }
    const out = image.data;
    const stride = image.width;
    const alphaScale = color.a / 255;
    for (let y = 0; y < h; y += 1) {
      const src = y * w;
      const dst = y * stride;
      for (let x = 0; x < w; x += 1) {
        const at = (dst + x) * 4;
        const alpha = packed[8 + src + x];
        out[at] = color.r;
        out[at + 1] = color.g;
        out[at + 2] = color.b;
        out[at + 3] = alpha ? Math.round(alpha * alphaScale) : 0;
      }
    }
    atlas.ctx.putImageData(image, sx, sy, 0, 0, w, h);
  }

  function evictSpriteEntries() {
    if (glyphSprites.size < GLYPH_CACHE_MAX) {
      return;
    }
    // Map 保持插入顺序：一次丢最旧的一小段，别整表清空。
    let drop = Math.max(1, GLYPH_CACHE_MAX >> 3);
    for (const key of glyphSprites.keys()) {
      glyphSprites.delete(key);
      drop -= 1;
      if (drop <= 0) {
        break;
      }
    }
  }

  // 缓存的是已经染好色的字形 alpha 蒙版，按 atlas 的 (font,gid,size,color) 复用。
  // 相位在 drawGlyphRun 里吸附到整数设备像素，所以同一字形只光栅化一次。
  function glyphSprite(font, gid, size, color) {
    if (color.a === 0) {
      return null;
    }
    const colorKey = `${color.r},${color.g},${color.b},${color.a}`;
    const key = `${font}|${gid}|${size}|${colorKey}`;
    if (glyphSprites.has(key)) {
      const cached = glyphSprites.get(key);
      if (cached && cached.atlas) {
        cached.atlas.lastUsed = ++atlasClock;
      }
      return cached;
    }
    let sprite = null;
    try {
      const packed = eb_glyph_bitmap(font, gid, size, 0, 0);
      if (packed && packed.length >= 8) {
        const view = new DataView(packed.buffer, packed.byteOffset, packed.byteLength);
        const w = view.getUint16(0, true);
        const h = view.getUint16(2, true);
        if (w > 0 && h > 0 && packed.length >= 8 + w * h) {
          const left = view.getInt16(4, true);
          const top = view.getInt16(6, true);
          const placed = pickGlyphAtlas(w, h);
          if (placed) {
            const [sx, sy] = placed.pos;
            writeGlyph(placed.atlas, sx, sy, w, h, packed, color);
            sprite = {
              atlas: placed.atlas,
              sx,
              sy,
              w,
              h,
              left,
              top,
              font,
              gid,
              size,
              color,
            };
            // 只给测试/调试看：确认指定位置画的是哪个 gid、哪种颜色。
            if (!placed.atlas.canvas.__sprites) {
              placed.atlas.canvas.__sprites = new Map();
            }
            placed.atlas.canvas.__sprites.set(`${sx},${sy}`, sprite);
          }
        }
      }
    } catch (error) {
      sprite = null;
    }
    evictSpriteEntries();
    glyphSprites.set(key, sprite);
    return sprite;
  }

  // 完全按 IR 的字形 id/advance/相位画；空格没有轮廓，只前进不落墨。
  function drawGlyphRun(run, glyphs, startX, startY, font) {
    const placed = [];
    const dpr = renderDpr || 1;
    const px = run.size * dpr;
    let penX = startX;
    let penY = startY;
    let missing = false;
    for (const glyph of glyphs) {
      // 字形吸附到整数设备像素。亚像素相位会把同一 gid 复制成几十个
      // sprite，换来的视觉收益远小于每页几百次额外的光栅化。
      const x = Math.floor(penX * dpr);
      const y = Math.floor((penY + glyph.dy) * dpr);
      const sprite = glyph.gid ? glyphSprite(font, glyph.gid, px, run.color) : null;
      // 子集字体缺字：整段退回浏览器系统字体，别让某个字静默消失。
      if (!glyph.gid) {
        missing = true;
      }
      if (sprite) {
        placed.push({
          sprite,
          x: x / dpr + sprite.left / dpr,
          y: y / dpr + sprite.top / dpr,
          w: sprite.w / dpr,
          h: sprite.h / dpr,
        });
      }
      penX += glyph.dx;
    }
    if (missing || !placed.length) {
      return false;
    }
    for (const item of placed) {
      const sprite = item.sprite;
      ctx.drawImage(
        sprite.atlas.canvas,
        sprite.sx,
        sprite.sy,
        sprite.w,
        sprite.h,
        item.x,
        item.y,
        item.w,
        item.h,
      );
    }
    return true;
  }

  function drawUnderline(x, y, width, size, color) {
    if (width <= 0) {
      return;
    }
    ctx.fillStyle = colorOf(color);
    ctx.fillRect(
      x,
      y + Math.max(1, size * 0.08),
      width,
      Math.max(1, size * 0.055),
    );
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
    const x = run.origin.x - ox;
    const y = run.origin.y - oy;
    const chars = Array.from(run.text);
    const glyphs = layout.display.glyphs.slice(run.glyphs.start, run.glyphs.start + run.glyphs.len);
    const underlineWidth = glyphs.reduce((sum, glyph) => sum + glyph.dx, 0);
    const paint = (fn) => {
      if (run.rotate === 1) {
        ctx.save();
        ctx.translate(x, y);
        ctx.rotate(Math.PI / 2);
        const result = fn(0, 0);
        ctx.restore();
        return result;
      }
      return fn(x, y);
    };

    ctx.save();
    if (atlasReady) {
      const drew = paint((startX, startY) => {
        if (!drawGlyphRun(run, glyphs, startX, startY, boldFontOf(run.font, run.weight))) {
          return false;
        }
        if (run.underline) {
          drawUnderline(startX, startY, underlineWidth, run.size, run.color);
        }
        return true;
      });
      if (drew) {
        ctx.restore();
        return;
      }
    }

    ctx.font = fontOf(run.size, run.font, run.weight, run.style);
    ctx.fillStyle = colorOf(run.color);
    ctx.textAlign = "left";
    ctx.textBaseline = "alphabetic";
    ctx.direction = run.dir === 1 ? "rtl" : "ltr";
    ctx.fontKerning = "auto";
    const boldStroke = needsBoldStroke(run.font, run.weight);
    if (boldStroke) {
      ctx.strokeStyle = ctx.fillStyle;
      ctx.lineWidth = Math.max(0.5, run.size * 0.045);
      ctx.lineJoin = "round";
    }
    const emit = (text, atX, atY) => {
      ctx.fillText(text, atX, atY);
      if (boldStroke) {
        ctx.strokeText(text, atX, atY);
      }
    };
    // 排版已经把 advance 结算进每个字形（两端对齐加在空格上），整串
    // `fillText` 会把空格打回字体原宽、行尾缩回去。按非空白连续块画：
    // 块内交给浏览器整形，块之间按字形 dx 前进。
    const split = run.dir !== 1 && glyphs.length === chars.length;
    paint((startX, startY) => {
      if (!split) {
        emit(run.text, startX, startY);
      } else {
        let cursor = startX;
        let wordStart = 0;
        for (let i = 0; i <= chars.length; i += 1) {
          if (i < chars.length && !/\s/u.test(chars[i])) {
            continue;
          }
          if (i > wordStart) {
            const word = chars.slice(wordStart, i).join("");
            let shaped = 0;
            for (let k = wordStart; k < i; k += 1) {
              shaped += glyphs[k].dx;
            }
            const measured = ctx.measureText(word).width;
            if (Math.abs(measured - shaped) <= Math.max(0.75, shaped * 0.02)) {
              emit(word, cursor, startY);
            } else {
              let charCursor = cursor;
              for (let k = wordStart; k < i; k += 1) {
                emit(chars[k], charCursor, startY);
                charCursor += glyphs[k].dx;
              }
            }
            cursor += shaped;
          }
          if (i < chars.length) {
            cursor += glyphs[i].dx;
          }
          wordStart = i + 1;
        }
      }
      if (run.underline) {
        drawUnderline(startX, startY, underlineWidth, run.size, run.color);
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

  function render() {
    if (!layout || !layout.pages.length) {
      return;
    }
    const shown = clamp(page, 0, layout.pages.length - 1);
    page = shown;
    const info = layout.pages[shown];
    const dpr = Math.min(window.devicePixelRatio || 1, 3);
    const w = (info.size.w || PAGE_W) + info.inset.left + info.inset.right;
    const h = (info.size.h || PAGE_H) + info.inset.top + info.inset.bottom;
    canvas.width = Math.round(w * dpr);
    canvas.height = Math.round(h * dpr);
    canvas.style.width = `${w}px`;
    canvas.style.height = `${h}px`;
    renderDpr = dpr;
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
    flushRetiredGlyphAtlases();
    selection.draw(ctx, shown);
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
  }

  function bookProgress(shown) {
    const info = layout.pages[shown];
    const inChapter = decodeProgress(eb_progress(shown, info.lines.start, 0));
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

  function fillToc() {
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

  function updateBook() {
    titleEl.textContent = book.title || currentRecord?.title || "untitled";
    metaEl.textContent = currentRecord?.author || book.creator || book.language || book.lang || "";
    fillToc();
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
    selection.clear();
    chapter = clamp(index, 0, book.spine.length - 1);
    const dims = layoutDims();
    layoutDimsUsed = dims;
    const token = ++layoutToken;
    const bytes = eb_layout_style(
      chapter,
      dims.width,
      dims.height,
      dims.left,
      dims.right,
      dims.top,
      dims.bottom,
      layoutFontScale(),
      settings.lineHeight,
      settings.letterSpacing,
      ALIGN_CODES[settings.align] ?? 0,
    );
    if (!bytes.length) {
      const reason = eb_last_error();
      throw new Error(`第 ${chapter + 1} 章排版失败${reason ? `：${reason}` : ""}`);
    }
    if (token !== layoutToken) {
      return;
    }
    layout = decodeLayout(bytes);
    page = resolveTarget(target);
    highlightToc();
    render();
    saveProgressSoon();
    loadChapterImages(token).catch(() => {});
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

  function nextPage() {
    if (!layout || busy) {
      return;
    }
    if (page + 1 < layout.pages.length) {
      selection.clear();
      page += 1;
      render();
      saveProgressSoon();
    } else if (chapter + 1 < book.spine.length) {
      layoutChapter(chapter + 1, 0).catch((error) => setStatus(String(error.message || error)));
    }
    if (!hasClass("chrome-hidden")) {
      scheduleChromeHide(1800);
    }
  }

  function prevPage() {
    if (!layout || busy) {
      return;
    }
    if (page > 0) {
      selection.clear();
      page -= 1;
      render();
      saveProgressSoon();
    } else if (chapter > 0) {
      layoutChapter(chapter - 1, "last").catch((error) => setStatus(String(error.message || error)));
    }
    if (!hasClass("chrome-hidden")) {
      scheduleChromeHide(1800);
    }
  }

  async function seekTo(fraction) {
    if (!book || !layout) {
      return;
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
    await jumpToAnchor(anchor);
  }

  async function jumpToAnchor(anchor) {
    const index = Number(anchor.spine || 0);
    if (index !== chapter) {
      await layoutChapter(index, 0);
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
      saveProgressSoon();
    }
  }

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
    setStatus,
    onChange: () => {
      if (layout) {
        render();
      }
    },
  });

  // ---- 图片 ---------------------------------------------------------------

  async function loadChapterImages(token) {
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
        imageCache.set(entry.href, null);
      }
    }
    if (changed && token === layoutToken) {
      render();
    }
  }

  // ---- 字体 ---------------------------------------------------------------

  async function loadEmbeddedFonts(info, token) {
    const faces = Array.isArray(info.fonts) ? info.fonts : [];
    if (!faces.length) {
      return false;
    }
    const count = eb_font_face_count();
    let loaded = 0;
    for (let index = 0; index < count; index += 1) {
      if (token !== openToken) {
        return false;
      }
      if (eb_font_face(index) === 0xffffffff) {
        continue;
      }
      loaded += 1;
      const family = faces[index] && faces[index].family;
      if (!family || typeof FontFace !== "function" || !document.fonts) {
        continue;
      }
      try {
        const bytes = eb_font_face_bytes(index);
        if (token !== openToken) {
          return false;
        }
        const face = new FontFace(family, bytes.slice().buffer);
        document.fonts.add(await face.load());
      } catch (error) {
        // 浏览器解析不了：布局仍用 FontBook 的量度。
      }
    }
    if (token !== openToken) {
      return false;
    }
    fontDescs.clear();
    if (loaded) {
      enableGlyphAtlas();
    }
    return loaded > 0;
  }

  async function loadFont(bytes, family = FONT_FAMILY) {
    eb_font(bytes);
    const buffer = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
    try {
      if (typeof FontFace === "function" && document.fonts) {
        const face = new FontFace(family, buffer);
        await face.load();
        document.fonts.add(face);
        if (document.fonts.ready) {
          await document.fonts.ready;
        }
      }
    } catch (error) {
      setStatus("字体只用于排版");
    }
    fontDescs.clear();
    enableGlyphAtlas();
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

  async function gunzipFont(url, bytes) {
    if (!/\.gz(?:$|\?)/i.test(url)) {
      return bytes;
    }
    if (typeof DecompressionStream !== "function") {
      throw new Error("浏览器不支持 gzip 解压");
    }
    const stream = new Blob([bytes]).stream().pipeThrough(new DecompressionStream("gzip"));
    return new Uint8Array(await new Response(stream).arrayBuffer());
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
        new Response(bytes, { headers: { "content-type": "font/ttf" } }),
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

  // 同步决定这本书需要哪套字体。字体变了才重置 FontBook 与 atlas；
  // 同一套字体换书则全部复用，热开书不再重付首帧光栅化。
  function planFonts(info, record) {
    const key = fontKeyFor(info, record);
    if (fontState && fontState.key === key && fontState.ok) {
      return { key, kind: fontState.kind, reuse: true };
    }
    const kind = key.startsWith("embedded:")
      ? "embedded"
      : key === "default:cjk"
        ? "cjk"
        : "latin";
    eb_font_reset();
    fontDescs.clear();
    clearGlyphSprites();
    atlasReady = false;
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
        // 字体 19 MB，HTTP 缓存不可靠；Cache Storage 首读之后直接本地命中。
        const cached = await cachedFontBytes(url);
        if (token !== openToken) {
          return false;
        }
        if (cached) {
          await loadFont(await gunzipFont(url, cached));
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
        const raw = await gunzipFont(url, bytes);
        if (token !== openToken) {
          return false;
        }
        await loadFont(raw);
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
      ok = await loadEmbeddedFonts(info, token);
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
    if (!["light", "sepia", "dark"].includes(next.theme)) {
      next.theme = "light";
    }
    return next;
  }

  async function applySettings(next, persist = true) {
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
          eb_font_reset();
          fontDescs.clear();
          clearGlyphSprites();
          atlasReady = false;
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
    busy = true;
    const token = ++openToken;
    try {
      setStatus("正在打开");
      const raw = bytes ? toBytes(bytes) : toBytes(await store.getBookData(record.id));
      if (!raw || !raw.length) {
        throw new Error("书籍数据不存在");
      }
      let info;
      if (record.format === "txt") {
        const text = decodeText(raw);
        if (!text.trim()) {
          throw new Error("TXT 内容为空");
        }
        info = JSON.parse(eb_open_text(text, record.name || record.title || "untitled.txt"));
      } else {
        info = JSON.parse(eb_open(raw));
      }
      if (info.error) {
        throw new Error(info.error);
      }
      book = info;
      currentRecord = record;
      baseFontSize = Number(eb_base_font_size()) || DEFAULT_SETTINGS.fontSize;
      chapter = 0;
      page = 0;
      layout = null;
      layoutDimsUsed = null;
      setChromeHidden(isNarrowView());
      chapterImages = [];
      imageCache.clear();
      const plan = planFonts(info, record);
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
      updateBook();
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
    layoutDimsUsed = null;
    chapterImages = [];
    imageCache.clear();
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
