import { readFileSync, readdirSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { decodeHit, decodeLayout, decodeLink, decodeProgress, decodeSelection } from "../web/decode.js";
import { encodeLayout } from "../web/encode.js";

const HERE = dirname(fileURLToPath(import.meta.url));
const GOLDEN_DIR = resolve(HERE, "golden");

function compare(name, bytes) {
  const layout = decodeLayout(bytes);
  const again = encodeLayout(layout);
  if (again.length !== bytes.length) {
    return `length ${again.length} != ${bytes.length}`;
  }
  for (let i = 0; i < bytes.length; i += 1) {
    if (again[i] !== bytes[i]) {
      return `byte ${i}: ${again[i]} != ${bytes[i]}`;
    }
  }
  checkSpans(name, layout);
  checkFields(name, layout);
  return null;
}

// 往返不验证字段含义：两侧字段顺序错得一样也会逐字节还原。
// 所以这里对新增字段的语义做最小断言，防止 decode/encode 一起跑偏。
function checkFields(name, layout) {
  const { major, minor } = layout.version;
  if (major !== 1 || minor !== 4) {
    throw new Error(`${name}: IR version ${major}.${minor}, want 1.4`);
  }

  const glyphs = layout.display.glyphs;
  for (let i = 0; i < layout.runs.lines.length; i += 1) {
    const line = layout.runs.lines[i];
    if (line.glyphs.start + line.glyphs.len > glyphs.length) {
      throw new Error(`${name}: line ${i} glyph span overflows ${glyphs.length} glyphs`);
    }
    for (let g = line.glyphs.start; g < line.glyphs.start + line.glyphs.len; g += 1) {
      const ch = glyphs[g].ch;
      if (ch >= line.src.len) {
        throw new Error(`${name}: line ${i} glyph ${g} ch ${ch} outside src ${line.src.len}`);
      }
    }
    if (line.node > 0xffffffff) {
      throw new Error(`${name}: line ${i} node ${line.node} out of range`);
    }
  }

  for (const op of layout.display.ops) {
    if (op.type !== "link") {
      continue;
    }
    if (typeof op.href !== "string" || op.href.length === 0) {
      throw new Error(`${name}: link op without href`);
    }
    if (!(op.rect.w > 0 && op.rect.h > 0)) {
      throw new Error(`${name}: link op with empty rect`);
    }
  }

  for (const issue of layout.issues) {
    if (!Number.isInteger(issue.code) || issue.code < 0 || issue.code > 6) {
      throw new Error(`${name}: issue with unknown code ${issue.code}`);
    }
    if (issue.detail !== null && typeof issue.detail !== "string") {
      throw new Error(`${name}: issue detail is not a string`);
    }
  }

  if (typeof layout.truncated !== "boolean") {
    throw new Error(`${name}: truncated flag is not a boolean`);
  }
}

function checkSpans(name, layout) {
  const ops = layout.display.ops.length;
  const lines = layout.runs.lines.length;
  let cursor = 0;
  for (const page of layout.pages) {
    if (page.ops.start !== cursor && page.ops.len > 0) {
      throw new Error(`${name}: page ${page.index} ops start ${page.ops.start} != ${cursor}`);
    }
    cursor += page.ops.len;
    if (page.lines.start + page.lines.len > lines) {
      throw new Error(`${name}: page ${page.index} lines overflow`);
    }
    if (page.lines.len === 0) {
      throw new Error(`${name}: page ${page.index} has no lines`);
    }
  }
  if (cursor !== ops) {
    throw new Error(`${name}: pages cover ${cursor} ops of ${ops}`);
  }
  let end = 0;
  for (const page of layout.pages) {
    if (page.lines.start < end) {
      throw new Error(`${name}: page ${page.index} lines overlap`);
    }
    end = page.lines.start + page.lines.len;
  }
  if (end !== lines) {
    throw new Error(`${name}: pages cover ${end} lines of ${lines}`);
  }
}

// 合成一份带 link op / 字形偏移 / 诊断条目的布局，验证 encode↔decode 对新增字段诚实。
// golden 里暂时没有 link（格式层还没填 `Node.link`），所以这一段是 link 解码的唯一覆盖。
function selfCheck() {
  const layout = {
    header: { major: 1, minor: 4 },
    version: { major: 1, minor: 4 },
    spine: 0,
    pages: [{
      index: 0,
      origin: { x: 0, y: 100 },
      size: { w: 80, h: 100 },
      inset: { top: 8, right: 8, bottom: 8, left: 8 },
      ops: { start: 0, len: 2 },
      lines: { start: 0, len: 1 },
      anchor: null,
    }],
    display: {
      ops: [
        {
          type: "glyphRun",
          run: {
            font: 2,
            size: 12,
            color: { r: 0, g: 0, b: 0, a: 255 },
            origin: { x: 10, y: 20 },
            glyphs: { start: 0, len: 1 },
            text: "x",
            dir: 0,
            rotate: 0,
            weight: 700,
            style: 1,
            underline: true,
          },
        },
        { type: "link", rect: { x: 10, y: 20, w: 8, h: 16 }, href: "ch2.xhtml#n" },
      ],
      glyphs: [{ gid: 7, dx: 8, dy: 0, ch: 3 }],
    },
    runs: {
      lines: [{
        rect: { x: 10, y: 20, w: 8, h: 16 },
        baseline: 32,
        dir: 0,
        src: { start: 5, len: 6 },
        anchor: { spine: 0, path: [1, 2], offset: 5 },
        glyphs: { start: 0, len: 1 },
        node: 9,
      }],
    },
    dirty: [],
    issues: [{ node: 4, code: 3, detail: "image pending" }],
    truncated: true,
  };
  const bytes = encodeLayout(layout);
  const again = decodeLayout(bytes);
  checkFields("synthetic", again);
  checkSpans("synthetic", again);
  const diff = (want, got, what) => {
    if (JSON.stringify(want) !== JSON.stringify(got)) {
      throw new Error(`synthetic: ${what} ${JSON.stringify(got)} != ${JSON.stringify(want)}`);
    }
  };
  diff(layout.display.ops, again.display.ops, "ops");
  diff(layout.display.glyphs, again.display.glyphs, "glyphs");
  diff(layout.runs.lines, again.runs.lines, "lines");
  diff(layout.issues, again.issues, "issues");
  diff(layout.truncated, again.truncated, "truncated");
  const reencoded = encodeLayout(again);
  if (reencoded.length !== bytes.length) {
    throw new Error(`synthetic: re-encode length ${reencoded.length} != ${bytes.length}`);
  }
  for (let i = 0; i < bytes.length; i += 1) {
    if (reencoded[i] !== bytes[i]) {
      throw new Error(`synthetic: re-encode byte ${i} differs`);
    }
  }
  console.log(`contract ok: synthetic link/issue round-trip (${bytes.length} bytes)`);
}

// 交互导出（eb_hit / eb_link_at / eb_selection / eb_progress）的往返检查。
// 需要 ./web/build.sh 产出的 wasm 包；没构建就跳过，不把 contract 变成构建前置。
async function interactionSelfCheck() {
  let wasm;
  try {
    wasm = await import(new URL("../web/pkg/ebook_web.js", import.meta.url).href);
  } catch (error) {
    console.log(`contract skip: web/pkg not built (${error.message.split("\n")[0]}) — run ./web/build.sh`);
    return null;
  }
  const raw = readFileSync(resolve(HERE, "../web/pkg/ebook_web_bg.wasm"));
  await wasm.default({ module_or_path: raw });

  const layout = decodeLayout(
    wasm.eb_layout_text(
      "hello world hello world hello world hello world hello world",
      200,
      300,
      40,
    ),
  );
  if (layout.runs.lines.length < 2) {
    throw new Error(`interaction: text layout has ${layout.runs.lines.length} line(s)`);
  }
  const pageOfLine = (line) => {
    const page = layout.pages.find((p) => line >= p.lines.start && line < p.lines.start + p.lines.len);
    if (!page) {
      throw new Error(`interaction: no page for line ${line}`);
    }
    return page.index;
  };

  // 行盒是流坐标，换成页内坐标才能喂给 eb_hit（流 = 页内 + origin - inset）。
  const firstPage = layout.pages[0];
  const toPage = (rect) => ({
    x: rect.x - firstPage.origin.x + firstPage.inset.left,
    y: rect.y - firstPage.origin.y + firstPage.inset.top,
  });
  const line0 = layout.runs.lines[0];
  const line0Page = toPage(line0.rect);
  const first = decodeHit(
    wasm.eb_hit(firstPage.index, line0Page.x + line0.rect.w / 2, line0Page.y + line0.rect.h / 2),
  );
  if (!first || first.page !== firstPage.index || first.line !== 0) {
    throw new Error(`interaction: hit on first line ${JSON.stringify(first)}`);
  }
  const miss = decodeHit(wasm.eb_hit(99, 0, 0));
  if (miss !== null) {
    throw new Error(`interaction: out-of-range hit ${JSON.stringify(miss)}`);
  }
  // txt 没有 <a href>，所以链接查询必须是 null 而不是空字符串。
  const href = decodeLink(
    wasm.eb_link_at(firstPage.index, line0Page.x + 2, line0Page.y + line0.rect.h / 2),
  );
  if (href !== null) {
    throw new Error(`interaction: unexpected link ${href}`);
  }

  const byte0 = first.byte;
  const byte1 = Math.min(line0.src.len, byte0 + 2);
  if (byte1 <= byte0) {
    throw new Error("interaction: first line too short for a selection");
  }
  const sel = decodeSelection(
    wasm.eb_selection(first.page, first.line, byte0, first.page, first.line, byte1),
  );
  if (sel.text.length !== byte1 - byte0 || sel.rects.length !== 1) {
    throw new Error(`interaction: selection ${JSON.stringify(sel)}`);
  }
  const rect = sel.rects[0];
  if (rect.page !== first.page || !(rect.w > 0 && rect.h > 0)) {
    throw new Error(`interaction: selection rect ${JSON.stringify(rect)}`);
  }
  const inside = decodeHit(wasm.eb_hit(rect.page, rect.x + rect.w / 2, rect.y + rect.h / 2));
  if (!inside || inside.line !== first.line || inside.byte < byte0 || inside.byte > byte1) {
    throw new Error(`interaction: hit inside selection rect ${JSON.stringify(inside)}`);
  }
  const reversed = decodeSelection(
    wasm.eb_selection(first.page, first.line, byte1, first.page, first.line, byte0),
  );
  if (reversed.text !== sel.text || JSON.stringify(reversed.rects) !== JSON.stringify(sel.rects)) {
    throw new Error("interaction: reversed selection differs from forward selection");
  }

  let prev = -1;
  for (let line = 0; line < layout.runs.lines.length; line += 1) {
    const progress = decodeProgress(wasm.eb_progress(pageOfLine(line), line, 0));
    if (progress < prev || progress < 0 || progress > 1) {
      throw new Error(`interaction: progress ${progress} at line ${line} (prev ${prev})`);
    }
    prev = progress;
  }
  const last = layout.runs.lines.length - 1;
  const end = decodeProgress(wasm.eb_progress(pageOfLine(last), last, layout.runs.lines[last].src.len));
  if (end !== 1) {
    throw new Error(`interaction: final progress ${end}`);
  }
  console.log(`contract ok: interaction hit/link/selection/progress (${layout.runs.lines.length} lines)`);
  return null;
}

// 选区交互模块（web/selection.js）不依赖 DOM 也能驱动：喂一个假 canvas 和
// hit/select/linkAt 回调，验证拖动→矩形→高亮、点击→链接、clear 后不残留。
async function selectionModuleSelfCheck() {
  const { createSelection } = await import(new URL("../web/selection.js", import.meta.url).href);
  const listeners = new Map();
  const canvas = {
    addEventListener(type, handler) {
      if (!listeners.has(type)) {
        listeners.set(type, []);
      }
      listeners.get(type).push(handler);
    },
    getBoundingClientRect() {
      return { left: 0, top: 0, width: 100, height: 100 };
    },
    setPointerCapture() {},
  };
  const fire = (type, event) => {
    for (const handler of listeners.get(type) || []) {
      handler({ button: 0, pointerId: 1, pointerType: "mouse", preventDefault() {}, ...event });
    }
  };
  let linked = null;
  let swiped = null;
  const page = () => ({
    index: 0,
    info: {
      size: { w: 100, h: 100 },
      inset: { top: 0, right: 0, bottom: 0, left: 0 },
    },
    layout: {},
  });
  const location = (x) => ({ page: 0, line: 0, byte: Math.max(0, Math.min(9, Math.floor(x / 10))) });
  const selection = createSelection({
    canvas,
    page,
    hit: (_index, x) => location(x),
    select: (a, b) => {
      const lo = Math.min(a.byte, b.byte);
      const hi = Math.max(a.byte, b.byte);
      return { rects: [{ page: 0, x: lo * 10, y: 0, w: (hi - lo) * 10, h: 10 }], text: "x".repeat(hi - lo) };
    },
    linkAt: () => "ch2.xhtml#n",
    onLink: (href) => {
      linked = href;
    },
    onSwipe: (direction) => {
      swiped = direction;
    },
    setStatus: () => {},
    onChange: () => {},
  });

  // 引擎查询是异步的（worker RPC），宿主只保证「一次拖动 = 一次查询」，
  // 所以这里按真实节奏等一轮事件循环再断言。
  const settle = () => new Promise((done) => setTimeout(done, 30));

  // 拖动：x=10 → byte 1，x=40 → byte 4。
  fire("pointerdown", { clientX: 10, clientY: 10 });
  await settle();
  fire("pointermove", { clientX: 40, clientY: 10 });
  await settle();
  fire("pointerup", { clientX: 40, clientY: 10 });
  await settle();
  if (!selection.active() || selection.text() !== "xxx") {
    throw new Error(`selection module: drag ${JSON.stringify({ active: selection.active(), text: selection.text() })}`);
  }
  let painted = 0;
  const ctx = {
    save() {},
    restore() {},
    fillRect(x, y, w, h) {
      painted += 1;
      if (x !== 10 || y !== 0 || w !== 30 || h !== 10) {
        throw new Error(`selection module: rect ${x},${y},${w},${h}`);
      }
    },
  };
  selection.draw(ctx, 0);
  if (painted !== 1) {
    throw new Error(`selection module: painted ${painted} rects`);
  }
  selection.draw(ctx, 1);
  if (painted !== 1) {
    throw new Error("selection module: rect leaked onto another page");
  }

  // 不动鼠标地按下/抬起 = 点击链接。
  selection.clear();
  if (selection.active()) {
    throw new Error("selection module: clear left rects behind");
  }
  fire("pointerdown", { clientX: 10, clientY: 10 });
  await settle();
  fire("pointerup", { clientX: 10, clientY: 10 });
  await settle();
  if (linked !== "ch2.xhtml#n") {
    throw new Error(`selection module: link click ${linked}`);
  }

  // 触屏横向滑动：左滑翻下一页，不应再触发点击。
  linked = null;
  swiped = null;
  fire("pointerdown", { clientX: 90, clientY: 10, pointerType: "touch" });
  fire("pointermove", { clientX: 40, clientY: 12, pointerType: "touch" });
  await settle();
  fire("pointerup", { clientX: 40, clientY: 12, pointerType: "touch" });
  await settle();
  if (swiped !== "next" || linked !== null) {
    throw new Error(`selection module: swipe ${swiped}, link ${linked}`);
  }
  console.log("contract ok: selection module drag/overlay/link-click/swipe");
}

const names = process.argv.length > 2
  ? process.argv.slice(2)
  : readdirSync(GOLDEN_DIR).filter((n) => n.endsWith(".ir")).sort().map((n) => resolve(GOLDEN_DIR, n));

let failed = 0;
try {
  selfCheck();
} catch (error) {
  console.error(`contract broken: ${error.message}`);
  failed += 1;
}
try {
  await interactionSelfCheck();
} catch (error) {
  console.error(`contract broken: ${error.message}`);
  failed += 1;
}
try {
  await selectionModuleSelfCheck();
} catch (error) {
  console.error(`contract broken: ${error.message}`);
  failed += 1;
}
for (const path of names) {
  const bytes = readFileSync(path);
  const problem = compare(path, bytes);
  if (problem) {
    console.error(`contract broken: ${path}: ${problem}`);
    failed += 1;
  } else {
    console.log(`contract ok: ${path} (${bytes.length} bytes round-trip)`);
  }
}

if (failed > 0) {
  process.exit(1);
}
