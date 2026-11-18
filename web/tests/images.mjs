// 浏览器 App 的资源/目录回归：用假 canvas 驱动 web/app.js。
//
// 这个文件放在 web/ 下（允许改动的路径），不依赖 tests/app.mjs。运行：
//   ./web/build.sh && bun web/tests/images.mjs

import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const ORIGIN = "http://test.local/";

const drawn = [];
const drawnImages = [];
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
  fillRect() {},
  rect() {},
  clip() {},
  fillText(text, x, y) {
    drawn.push({ text, x, y, font: this.font });
  },
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
    classList: {
      add() {},
      remove() {},
      toggle() {},
    },
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
      return ctxStub;
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
  fonts: {
    add() {},
    ready: Promise.resolve(),
  },
  addEventListener() {},
  body: makeElement("body"),
};

globalThis.window = {
  location: {
    href: `${ORIGIN}web/`,
    search: "?book=../fixtures/images.epub",
    protocol: "http:",
  },
  innerHeight: 1000,
  innerWidth: 1400,
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

// 只读 PNG 的 IHDR；drawImage 的调用次数与目标 rect 才是断言对象。
globalThis.createImageBitmap = async (blob) => {
  const bytes = new Uint8Array(await blob.arrayBuffer());
  if (bytes[0] === 0x89 && bytes[1] === 0x50) {
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    return { width: view.getUint32(16), height: view.getUint32(20), close() {} };
  }
  return { width: 12, height: 6, close() {} };
};

globalThis.fetch = async (input) => {
  const raw = input && input.href ? input.href : input;
  const parsed = new URL(String(raw), globalThis.window.location.href);
  const path = parsed.protocol === "file:" ? parsed.pathname : resolve(ROOT, parsed.pathname.slice(1));
  try {
    const bytes = readFileSync(path);
    return new Response(bytes, { status: 200, headers: { "content-type": path.endsWith(".wasm") ? "application/wasm" : "application/octet-stream" } });
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

const settle = () => new Promise((done) => setTimeout(done, 200));

await import(`${ROOT}/web/app.js`);
for (let i = 0; i < 6; i += 1) {
  await settle();
}

// ---- 图片 ---------------------------------------------------------------
check(
  "the embedded font family is used for drawing",
  drawn.some((item) => /Book Serif/.test(item.font)),
  drawn[0] && drawn[0].font,
);
check("two images were drawn", drawnImages.length === 2, `drawn ${drawnImages.length}`);
check(
  "the wide image keeps its 12x6 rect",
  drawnImages.some((image) => Math.round(image.w) === 12 && Math.round(image.h) === 6),
  JSON.stringify(drawnImages.map((image) => [image.w, image.h])),
);
check(
  "the inline image keeps its 4x10 rect",
  drawnImages.some((image) => Math.round(image.w) === 4 && Math.round(image.h) === 10),
  JSON.stringify(drawnImages.map((image) => [image.w, image.h])),
);

// ---- 目录 ---------------------------------------------------------------
const toc = globalThis.document.getElementById("toc");
function buttonsOf(element, out = []) {
  if (element.className && String(element.className).includes("toc-entry")) {
    out.push(element);
  }
  for (const child of element.children || []) {
    buttonsOf(child, out);
  }
  return out;
}
const tocButtons = buttonsOf(toc);
check("toc has the three nav entries", tocButtons.length === 3, `toc ${tocButtons.length}`);
check(
  "the nested fragment entry is present",
  tocButtons.some((button) => /A note in chapter one/.test(button.textContent)),
  tocButtons.map((button) => button.textContent).join(" | "),
);

const chapterSelect = globalThis.document.getElementById("chapter");
check("starts in chapter 1", chapterSelect.value === "0", chapterSelect.value);
const second = tocButtons.find((button) => /^Two:/.test(button.textContent));
check("the second chapter has a toc entry", Boolean(second));
if (second) {
  drawnImages.length = 0;
  second.dispatchEvent({ type: "click" });
  for (let i = 0; i < 4; i += 1) {
    await settle();
  }
  check("clicking it changes the chapter", chapterSelect.value === "1", chapterSelect.value);
  check("the second chapter draws its image", drawnImages.length === 1, `drawn ${drawnImages.length}`);
}

if (failures.length > 0) {
  console.error(`images app test failed: ${failures.join(", ")}`);
  process.exit(1);
}
console.log("images app ok");
