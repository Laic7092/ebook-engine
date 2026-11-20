import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import * as store from "../web/db.js";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const ORIGIN = "http://test.local/";

const drawn = [];
const fills = [];
const strokes = [];
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
    drawn.push({ text, x, y, font: this.font, color: this.fillStyle });
  },
  strokeText(text, x, y) {
    strokes.push({ text, x, y, font: this.font, width: this.lineWidth, color: this.strokeStyle });
  },
  measureText(text) {
    return { width: Array.from(text).length * 9 };
  },
};

function makeElement(id) {
  const listeners = new Map();
  return {
    id,
    textContent: "",
    hidden: false,
    value: "",
    checked: false,
    disabled: false,
    files: null,
    width: 0,
    height: 0,
    style: {},
    children: [],
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
}

const elements = new Map();

globalThis.document = {
  getElementById(id) {
    if (!elements.has(id)) {
      elements.set(id, makeElement(id));
    }
    return elements.get(id);
  },
  createElement: (tag) => makeElement(tag),
  fonts: { add() {}, ready: Promise.resolve() },
  addEventListener() {},
};

// 应用测试只关心 JS 状态机；5 MB 中文字体的下载/解包会拖慢并阻塞事件循环。
// 字体解析由 ebook-font / web/tests 单独覆盖，这里显式走系统字体回退。
globalThis.window = {
  location: { href: `${ORIGIN}web/`, search: "?book=../fixtures/en.epub&atlas=0&nofont=1", protocol: "http:" },
  innerHeight: 1000,
  innerWidth: 1400,
  devicePixelRatio: 2,
  addEventListener() {},
};

globalThis.FontFace = class {
  constructor(family) {
    this.family = family;
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

globalThis.fetch = async (input) => {
  const raw = input && input.href ? input.href : input;
  const parsed = new URL(String(raw), globalThis.window.location.href);
  const path = parsed.protocol === "file:" ? parsed.pathname : resolve(ROOT, parsed.pathname.slice(1));
  try {
    const bytes = readFileSync(path);
    const type = path.endsWith(".wasm") ? "application/wasm" : "application/octet-stream";
    return new Response(bytes, { status: 200, headers: { "content-type": type } });
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

function text(id) {
  return globalThis.document.getElementById(id).textContent;
}

function banner() {
  const element = globalThis.document.getElementById("banner");
  return element.hidden ? "" : element.textContent;
}

function fileLike(relative, name) {
  const bytes = readFileSync(resolve(ROOT, relative));
  return {
    name: name || relative.split("/").pop(),
    arrayBuffer: async () => bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.length),
  };
}

function uploadBytes(bytes, name) {
  const input = globalThis.document.getElementById("file");
  input.files = [
    {
      name,
      arrayBuffer: async () => bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.length),
    },
  ];
  input.dispatchEvent({ type: "change" });
}

function upload(relative, name) {
  const bytes = readFileSync(resolve(ROOT, relative));
  uploadBytes(new Uint8Array(bytes), name || relative.split("/").pop());
}

function buttonsOf(element, out = []) {
  if (element.className && String(element.className).includes("toc-entry")) {
    out.push(element);
  }
  for (const child of element.children || []) {
    buttonsOf(child, out);
  }
  return out;
}

const settle = () => new Promise((done) => setTimeout(done, 250));

await import(`${ROOT}/web/app.js`);
await settle();
await settle();

check("boot draws the requested book", drawn.length > 200, `drawn ${drawn.length}`);
check("boot fills the title", text("book-title").length > 0, text("book-title"));
check("boot leaves no banner", banner() === "", banner());
const pageCanvas = globalThis.document.getElementById("page");
check("boot sizes the canvas", pageCanvas.width > 0);
check(
  "page fills the reading area instead of a fixed 720px",
  parseFloat(pageCanvas.style.width) >= globalThis.window.innerWidth - 1,
  pageCanvas.style.width,
);
check(
  "page fills the full viewport height",
  parseFloat(pageCanvas.style.height) >= globalThis.window.innerHeight - 1,
  pageCanvas.style.height,
);
check(
  "boot loads the builtin font",
  globalThis.document.getElementById("builtin-font").checked === true,
  "builtin font not loaded, layout would use stub advances",
);
check(
  "boot draws whole words instead of single characters",
  drawn.some((item) => /^[A-Za-z]{3,}$/.test(item.text)),
  drawn.slice(0, 8).map((item) => item.text).join("|"),
);

// 字号是文档字号，不是整页缩放：容器尺寸不能变，字形大小要变。
const beforeWidth = pageCanvas.style.width;
const beforeHeight = pageCanvas.style.height;
const fontSize = globalThis.document.getElementById("font-size");
fontSize.value = "30";
drawn.length = 0;
fontSize.dispatchEvent({ type: "change" });
await settle();
await settle();
check(
  "font size keeps the page container fixed",
  pageCanvas.style.width === beforeWidth && pageCanvas.style.height === beforeHeight,
  `${beforeWidth}x${beforeHeight} -> ${pageCanvas.style.width}x${pageCanvas.style.height}`,
);
check(
  "font size changes the drawn glyph size",
  drawn.some((item) => (parseFloat(item.font) || 0) >= 24),
  drawn.map((item) => item.font).slice(0, 3).join(" | "),
);
fontSize.value = "18";
fontSize.dispatchEvent({ type: "change" });
await settle();
await settle();

// 行距、字距、对齐、粗体都走同一套设置，排版后页面容器仍然不能变。
const lineHeight = globalThis.document.getElementById("line-height");
const letterSpacing = globalThis.document.getElementById("letter-spacing");
const textAlign = globalThis.document.getElementById("text-align");
const boldToggle = globalThis.document.getElementById("font-bold");
lineHeight.value = "1.6";
lineHeight.dispatchEvent({ type: "change" });
await settle();
letterSpacing.value = "1";
letterSpacing.dispatchEvent({ type: "change" });
await settle();
textAlign.value = "justify";
textAlign.dispatchEvent({ type: "change" });
await settle();
boldToggle.checked = true;
boldToggle.dispatchEvent({ type: "change" });
await settle();
check(
  "line/letter/align/bold keep the page container fixed",
  pageCanvas.style.width === beforeWidth && pageCanvas.style.height === beforeHeight,
  `${beforeWidth}x${beforeHeight} -> ${pageCanvas.style.width}x${pageCanvas.style.height}`,
);
check("line/letter/align/bold leave no banner", banner() === "", banner());
globalThis.document.getElementById("settings-reset").dispatchEvent({ type: "click" });
await settle();
await settle();

drawn.length = 0;
upload("fixtures/zh.epub");
await settle();
await settle();
check("upload swaps the book", text("book-title") !== "The Lantern at the End of Winter", text("book-title"));
check(
  "upload of a CJK book does not keep the latin-only builtin font",
  globalThis.document.getElementById("builtin-font").checked === false,
  "latin font left registered, CJK would render as missing glyphs",
);
check(
  "upload lays out the new chapter",
  drawn.reduce((sum, item) => sum + Array.from(item.text).length, 0) > 50,
  `drawn ${drawn.length} chunks`,
);
check("upload leaves no banner", banner() === "", banner());
check(
  "upload draws non-ascii text",
  drawn.some((item) => /[^\x00-\x7f]/.test(item.text)),
  drawn.map((item) => item.text).join("").slice(0, 40),
);

const shelf = globalThis.document.getElementById("shelf");
check("import adds the book to the shelf", shelf.children.length === 1, `cards ${shelf.children.length}`);

// TXT：按标题行分章，走 eb_open_text + TxtReader，不经过 ZIP。
drawn.length = 0;
upload("fixtures/demo.txt");
await settle();
await settle();
check("txt upload opens", text("book-title") === "demo", text("book-title"));
const txtToc = buttonsOf(globalThis.document.getElementById("toc"));
check(
  "txt upload is chapter-split by heading",
  txtToc.some((button) => /第二章/.test(button.textContent)),
  txtToc.map((button) => button.textContent).join(" | "),
);
check(
  "txt upload draws chinese text",
  drawn.some((item) => /[\u4e00-\u9fff]/.test(item.text)),
  drawn.map((item) => item.text).join("").slice(0, 40),
);
check("txt upload joins the shelf", shelf.children.length === 2, `cards ${shelf.children.length}`);

drawn.length = 0;
const before = text("book-title");
uploadBytes(new TextEncoder().encode("this is not a zip archive"), "broken.epub");
await settle();
await settle();
check("broken file reports instead of silently doing nothing", banner().length > 0, "no banner");
check("broken file keeps the previous book", text("book-title") === before, text("book-title"));
check("broken file is not added to the shelf", shelf.children.length === 2, `cards ${shelf.children.length}`);

// UA 默认样式：h1/strong 要加粗、em 要斜体、a 要蓝色下划线。
// 这本书没有内嵌字体，FontBook 里没有 face，刚好覆盖 font id = 0 的情况。
drawn.length = 0;
fills.length = 0;
strokes.length = 0;
upload("fixtures/style.epub");
await settle();
await settle();
// 没有字体时中文会被拆成单字 run，按字判断，不假设整词在同一个 run 里。
function drawnWith(word, pattern) {
  return drawn.some(
    (item) =>
      Array.from(word).some((ch) => item.text.includes(ch)) && pattern.test(item.font),
  );
}
function strokedWith(word) {
  return strokes.some((item) => Array.from(word).some((ch) => item.text.includes(ch)));
}
const styleRuns = drawn.map((item) => `${item.text}|${item.font}|${item.color}`).join("\n");
check(
  "heading is stroked to fake bold without a bold face",
  strokedWith("标题"),
  strokes.map((item) => `${item.text}|${item.width}`).join(" | "),
);
check("strong is stroked to fake bold", strokedWith("加粗"), strokes.length);
check("em is drawn italic", drawnWith("斜体", /\bitalic\b/), styleRuns.slice(0, 200));
const linkBlue = "rgba(26,95,180,1)";
check(
  "link text is blue",
  drawn.some(
    (item) =>
      Array.from("链接").some((ch) => item.text.includes(ch)) && item.color === linkBlue,
  ),
  drawn.filter((item) => /[链接]/.test(item.text)).map((item) => item.color).join(" | "),
);
check(
  "link gets an underline",
  fills.some((item) => item.color === linkBlue),
  fills.map((item) => item.color).join(" | "),
);

// 存储层的内存兜底：测试环境没有 indexedDB，但 API 仍必须完整可用。
const testId = "store-test-book";
await store.putBook({ id: testId, title: "Store Test", format: "txt", size: 3 }, new Uint8Array([1, 2, 3]));
check("store lists metadata", (await store.listBooks()).some((book) => book.id === testId));
check("store returns bytes", (await store.getBookData(testId)).byteLength === 3);
await store.putProgress(testId, { spine: 1, line: 2, percent: 0.5 });
check("store returns progress", (await store.getProgress(testId)).percent === 0.5);
await store.putSetting("reader", { theme: "sepia" });
check("store returns settings", (await store.getSetting("reader")).theme === "sepia");
await store.putAsset("font:test", new Uint8Array([9, 8]).buffer);
check("store caches assets", (await store.getAsset("font:test")).byteLength === 2);
await store.removeBook(testId);
check("store deletes metadata and bytes", !(await store.getBook(testId)) && !(await store.getBookData(testId)));

if (failures.length > 0) {
  console.error(`app test failed: ${failures.join(", ")}`);
  process.exit(1);
}
console.log("app ok");
