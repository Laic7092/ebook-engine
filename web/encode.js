const encoder = new TextEncoder();
const MAGIC = [0x45, 0x42, 0x49, 0x52];
const HEADER_LEN = 8;

class Writer {
  constructor() {
    this.bytes = new Uint8Array(1024);
    this.view = new DataView(this.bytes.buffer);
    this.pos = 0;
  }

  reserve(extra) {
    if (this.pos + extra <= this.bytes.length) {
      return;
    }
    let size = this.bytes.length * 2;
    while (size < this.pos + extra) {
      size *= 2;
    }
    const next = new Uint8Array(size);
    next.set(this.bytes.subarray(0, this.pos));
    this.bytes = next;
    this.view = new DataView(next.buffer);
  }

  u8(value) {
    this.reserve(1);
    this.bytes[this.pos] = value & 0xff;
    this.pos += 1;
  }

  rawU16(value) {
    this.reserve(2);
    this.view.setUint16(this.pos, value, true);
    this.pos += 2;
  }

  f32(value) {
    this.reserve(4);
    this.view.setFloat32(this.pos, Math.fround(value), true);
    this.pos += 4;
  }

  varint(value) {
    let rest = value >>> 0;
    if (value > 0xffffffff) {
      throw new Error(`varint too large: ${value}`);
    }
    this.reserve(5);
    while (rest >= 0x80) {
      this.bytes[this.pos] = (rest & 0x7f) | 0x80;
      this.pos += 1;
      rest = Math.floor(rest / 128);
    }
    this.bytes[this.pos] = rest;
    this.pos += 1;
  }

  string(value) {
    const raw = encoder.encode(value);
    this.varint(raw.length);
    this.reserve(raw.length);
    this.bytes.set(raw, this.pos);
    this.pos += raw.length;
  }

  option(value, write) {
    if (value === null || value === undefined) {
      this.u8(0);
      return;
    }
    this.u8(1);
    write(this, value);
  }

  vec(items, write) {
    this.varint(items.length);
    for (const item of items) {
      write(this, item);
    }
  }

  finish() {
    return this.bytes.subarray(0, this.pos);
  }
}

function writePoint(w, value) {
  w.f32(value.x);
  w.f32(value.y);
}

function writeSize(w, value) {
  w.f32(value.w);
  w.f32(value.h);
}

function writeRect(w, value) {
  w.f32(value.x);
  w.f32(value.y);
  w.f32(value.w);
  w.f32(value.h);
}

function writeColor(w, value) {
  w.u8(value.r);
  w.u8(value.g);
  w.u8(value.b);
  w.u8(value.a);
}

function writeSpan(w, value) {
  w.varint(value.start);
  w.varint(value.len);
}

function writeSidesF(w, value) {
  w.f32(value.top);
  w.f32(value.right);
  w.f32(value.bottom);
  w.f32(value.left);
}

function writeAnchor(w, value) {
  w.varint(value.spine);
  w.vec(value.path, (w2, v) => w2.varint(v));
  w.varint(value.offset);
}

function writePage(w, value) {
  w.varint(value.index);
  writePoint(w, value.origin);
  writeSize(w, value.size);
  writeSidesF(w, value.inset);
  writeSpan(w, value.ops);
  writeSpan(w, value.lines);
  w.option(value.anchor, writeAnchor);
}

function writeGlyph(w, value) {
  w.varint(value.gid);
  w.f32(value.dx);
  w.f32(value.dy);
  w.varint(value.ch);
}

function writeGlyphRun(w, value) {
  w.varint(value.font);
  w.f32(value.size);
  writeColor(w, value.color);
  writePoint(w, value.origin);
  writeSpan(w, value.glyphs);
  w.string(value.text);
  w.varint(value.dir);
  w.varint(value.rotate);
}

function writeBorderBox(w, value) {
  writeRect(w, value.rect);
  writeSidesF(w, value.width);
  writeColor(w, value.color);
}

function writeUnresolved(w, value) {
  w.varint(value.node);
  writeRect(w, value.rect);
  w.varint(value.reason);
}

function writeOp(w, value) {
  if (value.type === "glyphRun") {
    w.varint(0);
    writeGlyphRun(w, value.run);
    return;
  }
  if (value.type === "rect") {
    w.varint(1);
    writeRect(w, value.rect);
    writeColor(w, value.color);
    return;
  }
  if (value.type === "border") {
    w.varint(2);
    writeBorderBox(w, value.border);
    return;
  }
  if (value.type === "image") {
    w.varint(3);
    w.varint(value.id);
    writeRect(w, value.rect);
    w.varint(value.fit);
    return;
  }
  if (value.type === "unresolved") {
    w.varint(4);
    writeUnresolved(w, value.unresolved);
    return;
  }
  if (value.type === "clip") {
    w.varint(5);
    writeRect(w, value.rect);
    w.vec(value.ops, writeOp);
    return;
  }
  if (value.type === "link") {
    w.varint(6);
    writeRect(w, value.rect);
    w.string(value.href);
    return;
  }
  throw new Error(`bad op type ${value.type}`);
}

function writeLineBox(w, value) {
  writeRect(w, value.rect);
  w.f32(value.baseline);
  w.varint(value.dir);
  writeSpan(w, value.src);
  w.option(value.anchor, writeAnchor);
  writeSpan(w, value.glyphs);
  w.varint(value.node);
}

function writeIssue(w, value) {
  w.varint(value.node);
  w.varint(value.code);
  w.option(value.detail, (w2, v) => w2.string(v));
}

function writeDirty(w, value) {
  w.varint(value.node);
  writeRect(w, value.rect);
  w.varint(value.reason);
}

export function encodeLayout(layout) {
  const w = new Writer();
  for (const byte of MAGIC) {
    w.u8(byte);
  }
  w.rawU16(layout.header.major);
  w.rawU16(layout.header.minor);
  w.varint(layout.version.major);
  w.varint(layout.version.minor);
  w.varint(layout.spine);
  w.vec(layout.pages, writePage);
  w.vec(layout.display.ops, writeOp);
  w.vec(layout.display.glyphs, writeGlyph);
  w.vec(layout.runs.lines, writeLineBox);
  w.vec(layout.dirty, writeDirty);
  w.vec(layout.issues, writeIssue);
  return w.finish();
}

export { HEADER_LEN, MAGIC };
