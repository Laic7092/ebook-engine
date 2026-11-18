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

function readPage(r) {
  return {
    index: r.varint(),
    origin: readPoint(r),
    size: readSize(r),
    ops: readSpan(r),
    lines: readSpan(r),
    anchor: r.option(readAnchor),
  };
}

function readGlyph(r) {
  return { gid: r.varint(), dx: r.f32(), dy: r.f32() };
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
  };
}

function readBorderBox(r) {
  return { rect: readRect(r), width: readSidesF(r), color: readColor(r) };
}

function readUnresolved(r) {
  return { node: r.varint(), rect: readRect(r), reason: r.varint() };
}

function readLinkTarget(r) {
  const tag = r.varint();
  if (tag === 0) {
    return { type: "spine", spine: r.varint() };
  }
  if (tag === 1) {
    return { type: "external", href: r.string() };
  }
  throw new Error("bad link target");
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
    return { type: "link", rect: readRect(r), target: readLinkTarget(r) };
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
  };
}

function readDirty(r) {
  return { node: r.varint(), rect: readRect(r), reason: r.varint() };
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
    dirty: readVec(r, readDirty),
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
  return { header, ...layout };
}
