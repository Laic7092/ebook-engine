// 字形图集回归：确认 Web 端不再用 fillText，而是按 IR 的 gid 画 FontBook 位图。
// 运行：./web/build.sh && bun web/tests/atlas.mjs

import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import * as wasm from "../pkg/ebook_web.js";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const ORIGIN = "http://test.local/";

const drawn = [];
const drawnImages = [];
const fills = [];
const ctxStub = {
  font: "10px sans-serif",
  fillStyle: "",
  strokeStyle: "",
  lineWidth: 1,
  textAlign: "left",
  textBaseline: "alphabetic",
  direction: "ltr",
  fontKerning: "auto",
  setTransform() {},
  save() {},
  restore() {},
  translate() {},
  rotate() {},
  beginPath() {},
  moveTo() {},
  lineTo() {},
  stroke() {},
  setLineDash() {},
  strokeRect() {},
  fillRect(x, y, w, h) {
    fills.push({ x, y, w, h, color: this.fillStyle });
  },
  rect() {},
  clip() {},
  fillText(text, x, y) {
    drawn.push({ text, x, y, font: this.font });
  },
  strokeText() {},
  drawImage(bitmap, x, y, w, h) {
    drawnImages.push({ bitmap, x, y, w, h });
  },
  measureText(text) {
    return { width: Array.from(text).length * 9 };
  },
};

function makeElement(tag = "div") {
  const listeners = new Map();
  const element = {
    tag,
    id: "",
    textContent: "",
    className: "",
    hidden: false,
    value: "",
    checked: false,
    disabled: false,
    type: "",
    files: null,
    width: 0,
    height: 0,
    style: {},
    children: [],
    classList: { add() {}, remove() {}, toggle() {} },
    addEventListener(type, handler) {
      if (!listeners.has(type)) {
        listeners.set(type, []);
      }
      listeners.get(type).push(handler);
    },
    dispatchEvent(event) {
      for (const handler of listeners.get(event.type) || []) {
        handler(event);
      }
      return true;
    },
    append(child) {
      this.children.push(child);
    },
    replaceChildren() {
      this.children = [];
    },
    getContext() {
      if (!element.__ctx) {
        element.__ctx = Object.create(ctxStub);
        element.__ctx.canvas = element;
        element.__ctx.createImageData = (w, h) => ({
          width: w,
          height: h,
          data: new Uint8ClampedArray(w * h * 4),
        });
        element.__ctx.putImageData = (image) => {
          element.__image = image;
        };
        element.__ctx.drawImage = (bitmap, ...args) => {
          if (args.length >= 8) {
            const [sx, sy, sw, sh, x, y, w, h] = args;
            drawnImages.push({
              bitmap,
              sx,
              sy,
              sw,
              sh,
              x,
              y,
              w,
              h,
              meta: bitmap.__sprites ? bitmap.__sprites.get(`${sx},${sy}`) : null,
            });
            return;
          }
          const [x, y, w, h] = args;
          drawnImages.push({ bitmap, x, y, w, h });
        };
      }
      return element.__ctx;
    },
  };
  return element;
}

const elements = new Map();
globalThis.document = {
  getElementById(id) {
    if (!elements.has(id)) {
      const element = makeElement();
      element.id = id;
      elements.set(id, element);
    }
    return elements.get(id);
  },
  createElement: (tag) => makeElement(tag),
  fonts: { add() {}, ready: Promise.resolve() },
  addEventListener() {},
  body: makeElement("body"),
};

globalThis.window = {
  location: {
    href: `${ORIGIN}web/`,
    search: "?book=../fixtures/style.epub&atlas=1",
    protocol: "http:",
  },
  innerHeight: 667,
  innerWidth: 375,
  devicePixelRatio: 2,
  addEventListener() {},
};

globalThis.FontFace = class {
  constructor(family, buffer) {
    this.family = family;
    this.buffer = buffer;
  }
  load() {
    return Promise.resolve(this);
  }
};

globalThis.Event = class {
  constructor(type) {
    this.type = type;
  }
  preventDefault() {}
};

globalThis.createImageBitmap = async () => ({ width: 12, height: 6, close() {} });

globalThis.fetch = async (input) => {
  const raw = input && input.href ? input.href : input;
  const parsed = new URL(String(raw), globalThis.window.location.href);
  const path = parsed.protocol === "file:" ? parsed.pathname : resolve(ROOT, parsed.pathname.slice(1));
  try {
    const bytes = readFileSync(path);
    return new Response(bytes, {
      status: 200,
      headers: { "content-type": path.endsWith(".wasm") ? "application/wasm" : "application/octet-stream" },
    });
  } catch (error) {
    return new Response("missing", { status: 404 });
  }
};

const failures = [];
function check(name, condition, detail) {
  if (condition) {
    console.log(`  ok   ${name}`);
    return;
  }
  console.log(`  FAIL ${name}${detail ? `: ${detail}` : ""}`);
  failures.push(name);
}

const settle = () => new Promise((done) => setTimeout(done, 250));
await import(`${ROOT}/web/app.js`);
for (let i = 0; i < 8; i += 1) {
  await settle();
}

check("glyphs are drawn through drawImage", drawnImages.length >= 10, `drawImage ${drawnImages.length}`);
check("atlas path does not fall back to fillText", drawn.length === 0, `fillText ${drawn.length}`);
const atlasCanvases = new Set(drawnImages.filter((item) => item.meta).map((item) => item.bitmap));
check(
  "glyphs share a small set of atlas canvases",
  atlasCanvases.size >= 1 && atlasCanvases.size <= 3,
  `${atlasCanvases.size} canvases for ${drawnImages.filter((item) => item.meta).length} glyphs`,
);


const body = drawnImages.find((item) => item.meta && item.meta.font === 0 && item.meta.gid > 0);
const bold = drawnImages.find((item) => item.meta && item.meta.font === 0x9000);
check("body glyphs use the regular face", Boolean(body));
check("heading/strong use a synthetic bold face", Boolean(bold));

function ink(packed) {
  if (!packed || packed.length < 8) {
    return 0;
  }
  let count = 0;
  for (let i = 8; i < packed.length; i += 1) {
    if (packed[i]) {
      count += 1;
    }
  }
  return count;
}
if (body) {
  const px = body.meta.size;
  const gid = body.meta.gid;
  const regular = ink(wasm.eb_glyph_bitmap(0, gid, px, 0, 0));
  const heavy = ink(wasm.eb_glyph_bitmap(0x9000, gid, px, 0, 0));
  check("synthetic bold has more ink than regular", heavy > regular * 1.15, `${regular} -> ${heavy}`);
}

check(
  "link glyphs are tinted browser blue",
  drawnImages.some(
    (item) =>
      item.meta &&
      item.meta.color &&
      item.meta.color.r === 26 &&
      item.meta.color.g === 95 &&
      item.meta.color.b === 180,
  ),
);
check(
  "link underline is painted",
  fills.some((fill) => String(fill.color).includes("26,95,180")),
  fills.map((fill) => fill.color).filter(Boolean).slice(0, 4).join(" | "),
);

if (failures.length > 0) {
  console.error(`atlas test failed: ${failures.join(", ")}`);
  process.exit(1);
}
console.log("atlas ok");
