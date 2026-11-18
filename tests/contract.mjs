import { readFileSync, readdirSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { decodeLayout } from "../web/decode.js";
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
  if (major !== 1 || minor !== 2) {
    throw new Error(`${name}: IR version ${major}.${minor}, want 1.2`);
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
    header: { major: 1, minor: 2 },
    version: { major: 1, minor: 2 },
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
