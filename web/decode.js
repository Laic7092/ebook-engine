const decoder = new TextDecoder();

class Reader {
  constructor(bytes, offset) {
    this.bytes = bytes;
    this.view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    this.pos = offset;
  }

  u8() {
    const value = this.bytes[this.pos];
    this.pos += 1;
    return value;
  }

  rawU16(offset) {
    return this.view.getUint16(offset, true);
  }

  f32() {
    const value = this.view.getFloat32(this.pos, true);
    this.pos += 4;
    return value;
  }

  varint() {
    let value = 0;
    let shift = 0;
    for (let i = 0; i < 5; i += 1) {
      const byte = this.u8();
      value += (byte & 0x7f) * 2 ** shift;
      if ((byte & 0x80) === 0) {
        return value;
      }
      shift += 7;
    }
    throw new Error("bad varint");
  }

  string() {
    const len = this.varint();
    const start = this.pos;
    this.pos += len;
    return decoder.decode(this.bytes.subarray(start, this.pos));
  }

  option(read) {
    const tag = this.u8();
    if (tag === 0) {
      return null;
    }
    if (tag === 1) {
      return read(this);
    }
    throw new Error("bad option");
  }
}

function readVec(r, read) {
  const len = r.varint();
  const out = new Array(len);
  for (let i = 0; i < len; i += 1) {
    out[i] = read(r);
  }
  return out;
}

function readPoint(r) {
  return { x: r.f32(), y: r.f32() };
}

function readSize(r) {
  return { w: r.f32(), h: r.f32() };
}

function readRect(r) {
  return { x: r.f32(), y: r.f32(), w: r.f32(), h: r.f32() };
}

function readColor(r) {
  return { r: r.u8(), g: r.u8(), b: r.u8(), a: r.u8() };
}

function readSpan(r) {
  return { start: r.varint(), len: r.varint() };
}

function readSidesF(r) {
  return { top: r.f32(), right: r.f32(), bottom: r.f32(), left: r.f32() };
}

function readAnchor(r) {
  return {
    spine: r.varint(),
    path: readVec(r, (x) => x.varint()),
    offset: r.varint(),
  };
}

function readEdges(r) {
  return { top: r.f32(), right: r.f32(), bottom: r.f32(), left: r.f32() };
}

function readPage(r) {
  return {
    index: r.varint(),
    origin: readPoint(r),
    size: readSize(r),
    inset: readEdges(r),
    ops: readSpan(r),
    lines: readSpan(r),
    anchor: r.option(readAnchor),
  };
}

function readGlyph(r) {
  return { gid: r.varint(), dx: r.f32(), dy: r.f32(), ch: r.varint() };
}

function readGlyphRun(r) {
  return {
    font: r.varint(),
    size: r.f32(),
    color: readColor(r),
    origin: readPoint(r),
    glyphs: readSpan(r),
    text: r.string(),
    dir: r.varint(),
    rotate: r.varint(),
    weight: r.varint(),
    style: r.varint(),
    underline: r.u8() === 1,
  };
}

function readBorderBox(r) {
  return { rect: readRect(r), width: readSidesF(r), color: readColor(r) };
}

function readUnresolved(r) {
  return { node: r.varint(), rect: readRect(r), reason: r.varint() };
}

function readOp(r) {
  const tag = r.varint();
  if (tag === 0) {
    return { type: "glyphRun", run: readGlyphRun(r) };
  }
  if (tag === 1) {
    return { type: "rect", rect: readRect(r), color: readColor(r) };
  }
  if (tag === 2) {
    return { type: "border", border: readBorderBox(r) };
  }
  if (tag === 3) {
    return { type: "image", id: r.varint(), rect: readRect(r), fit: r.varint() };
  }
  if (tag === 4) {
    return { type: "unresolved", unresolved: readUnresolved(r) };
  }
  if (tag === 5) {
    return { type: "clip", rect: readRect(r), ops: readVec(r, readOp) };
  }
  if (tag === 6) {
    return { type: "link", rect: readRect(r), href: r.string() };
  }
  throw new Error(`bad op tag ${tag}`);
}

function readLineBox(r) {
  return {
    rect: readRect(r),
    baseline: r.f32(),
    dir: r.varint(),
    src: readSpan(r),
    anchor: r.option(readAnchor),
    glyphs: readSpan(r),
    node: r.varint(),
  };
}

function readIssue(r) {
  return { node: r.varint(), code: r.varint(), detail: r.option((x) => x.string()) };
}

function readDirty(r) {
  return { node: r.varint(), rect: readRect(r), reason: r.varint() };
}

function readBlock(r) {
  return {
    node: r.varint(),
    rect: readRect(r),
    anchor: r.option(readAnchor),
    lines: r.varint(),
    depth: r.varint(),
  };
}

function readLayout(r) {
  return {
    version: { major: r.varint(), minor: r.varint() },
    spine: r.varint(),
    pages: readVec(r, readPage),
    display: {
      ops: readVec(r, readOp),
      glyphs: readVec(r, readGlyph),
    },
    runs: {
      lines: readVec(r, readLineBox),
    },
    blocks: readVec(r, readBlock),
    dirty: readVec(r, readDirty),
    issues: readVec(r, readIssue),
    truncated: r.u8() !== 0,
  };
}

export function decodeLayout(input) {
  const bytes = input instanceof Uint8Array ? input : new Uint8Array(input);
  if (bytes.length < 8) {
    throw new Error("layout too short");
  }
  if (bytes[0] !== 0x45 || bytes[1] !== 0x42 || bytes[2] !== 0x49 || bytes[3] !== 0x52) {
    throw new Error("bad layout magic");
  }
  const reader = new Reader(bytes, 8);
  const header = { major: reader.rawU16(4), minor: reader.rawU16(6) };
  const layout = readLayout(reader);
  if (reader.pos !== bytes.length) {
    throw new Error(`layout trailing bytes: ${bytes.length - reader.pos}`);
  }
  if (layout.version.major !== header.major || layout.version.minor !== header.minor) {
    throw new Error(`layout version mismatch: header=${header.major}.${header.minor}`);
  }
  return { header, ...layout };
}

// ---- 交互查询（wasm 导出的 JSON 信封）------------------------------------
// `eb_hit` / `eb_link_at` / `eb_selection` / `eb_progress` 走 JSON，不走 postcard：
// 查询是低频的人机交互，字段名比字节数重要。校验集中在这里，UI 侧只消费结果。

function parseJson(input, what) {
  const value = typeof input === "string" ? JSON.parse(input) : input;
  if (value === null || typeof value !== "object") {
    throw new Error(`${what}: not an object`);
  }
  return value;
}

function readLocation(value, what) {
  const keys = ["page", "line", "byte"];
  for (const key of keys) {
    if (!Number.isInteger(value[key]) || value[key] < 0) {
      throw new Error(`${what}: bad ${key} ${value[key]}`);
    }
  }
  return { page: value.page, line: value.line, byte: value.byte };
}

export function decodeHit(input) {
  const value = parseJson(input, "hit");
  return value.location === null ? null : readLocation(value.location, "hit");
}

export function decodeLink(input) {
  const value = parseJson(input, "link");
  if (value.href !== null && typeof value.href !== "string") {
    throw new Error("link: href is not a string");
  }
  return value.href;
}

export function decodeSelection(input) {
  const value = parseJson(input, "selection");
  const rects = Array.isArray(value.rects) ? value.rects : [];
  const out = [];
  for (const rect of rects) {
    if (!Number.isInteger(rect.page) || rect.page < 0) {
      throw new Error(`selection: bad page ${rect.page}`);
    }
    for (const key of ["x", "y", "w", "h"]) {
      if (typeof rect[key] !== "number" || !Number.isFinite(rect[key])) {
        throw new Error(`selection: bad ${key} ${rect[key]}`);
      }
    }
    if (rect.w < 0 || rect.h < 0) {
      throw new Error("selection: negative rect");
    }
    out.push({ page: rect.page, x: rect.x, y: rect.y, w: rect.w, h: rect.h });
  }
  const start = value.start === undefined || value.start === null
    ? null
    : readLocation(value.start, "selection start");
  const end = value.end === undefined || value.end === null
    ? null
    : readLocation(value.end, "selection end");
  return { start, end, rects: out, text: typeof value.text === "string" ? value.text : "" };
}

export function decodeProgress(input) {
  const value = parseJson(input, "progress");
  if (typeof value.progress !== "number" || !Number.isFinite(value.progress)) {
    throw new Error(`progress: bad value ${value.progress}`);
  }
  return Math.min(1, Math.max(0, value.progress));
}

// ---- 资源 / 目录 / fragment ----------------------------------------------
// `eb_toc` 返回裸数组；`eb_anchor` / `eb_resolve_link`
// 返回带字段名的对象。校验仍然集中在这里。

function parseList(input, what) {
  const value = typeof input === "string" ? JSON.parse(input) : input;
  if (!Array.isArray(value)) {
    throw new Error(`${what}: not an array`);
  }
  return value;
}

function optionalString(value, what, key) {
  if (value === null || value === undefined) {
    return null;
  }
  if (typeof value !== "string") {
    throw new Error(`${what}: bad ${key}`);
  }
  return value;
}

function nonNegativeInt(value, what, key) {
  if (!Number.isInteger(value) || value < 0) {
    throw new Error(`${what}: bad ${key} ${value}`);
  }
  return value;
}

export function decodeToc(input) {
  return parseList(input, "toc").map((entry) => {
    if (typeof entry.label !== "string") {
      throw new Error("toc: label is not a string");
    }
    const spine = entry.spine === null || entry.spine === undefined
      ? null
      : nonNegativeInt(entry.spine, "toc", "spine");
    return {
      label: entry.label,
      spine,
      fragment: optionalString(entry.fragment, "toc", "fragment"),
      depth: nonNegativeInt(entry.depth, "toc", "depth"),
    };
  });
}

export function decodeResolvedLink(input) {
  const value = parseJson(input, "resolve");
  if (value.target === null || value.target === undefined) {
    return null;
  }
  const target = value.target;
  return {
    spine: nonNegativeInt(target.spine, "resolve", "spine"),
    fragment: optionalString(target.fragment, "resolve", "fragment"),
    external: target.external === true,
  };
}

export function decodeSearch(input) {
  const value = parseJson(input, "search");
  if (!Array.isArray(value.hits)) {
    throw new Error("search: hits is not an array");
  }
  const stringField = (item, key, index) => {
    if (typeof item[key] !== "string") {
      throw new Error(`search[${index}]: bad ${key}`);
    }
    return item[key];
  };
  const hits = value.hits.map((hit, index) => ({
    spine: nonNegativeInt(hit.spine, `search[${index}]`, "spine"),
    node: nonNegativeInt(hit.node, `search[${index}]`, "node"),
    offset: nonNegativeInt(hit.offset, `search[${index}]`, "offset"),
    end: nonNegativeInt(hit.end, `search[${index}]`, "end"),
    chapter: optionalString(hit.chapter, `search[${index}]`, "chapter"),
    before: stringField(hit, "before", index),
    text: stringField(hit, "text", index),
    after: stringField(hit, "after", index),
    ellipsisBefore: hit.ellipsis_before === true,
    ellipsisAfter: hit.ellipsis_after === true,
  }));
  return {
    hits,
    truncated: value.truncated === true,
    scanned: nonNegativeInt(value.scanned, "search", "scanned"),
    skipped: nonNegativeInt(value.skipped, "search", "skipped"),
  };
}
