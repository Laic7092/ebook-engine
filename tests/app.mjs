import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import * as store from "../web/db.js";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const ORIGIN = "http://test.local/";

// 绘制已经全部在引擎里：测试只关心「一页位图被放进 #page」以及墨量变化，
// 不再记录 fillText / fillRect 的细节。
const paints = [];

function inkPixels(image) {
  const data = image && image.data ? image.data : null;
  if (!data) {
    return 0;
  }
  let ink = 0;
  for (let i = 0; i < data.length; i += 4) {
    if (data[i] !== 255 || data[i + 1] !== 255 || data[i + 2] !== 255) {
      ink += 1;
    }
  }
  return ink;
}

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
  clearRect() {},
  putImageData(image, x, y) {
    paints.push({ width: image.width, height: image.height, x, y, ink: inkPixels(image), image });
  },
  drawImage() {},
  fillText() {},
  strokeText() {},
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
  location: { href: `${ORIGIN}web/`, search: "?book=../fixtures/en.epub&nofont=1", protocol: "http:" },
  innerHeight: 1000,
  innerWidth: 1400,
  devicePixelRatio: 2,
  addEventListener() {},
};

// 浏览器里 ImageData 是内置的；bun 未必有，这里兜一个最小实现。
if (typeof globalThis.ImageData !== "function") {
  globalThis.ImageData = class {
    constructor(data, width, height) {
      this.data = data;
      this.width = width;
      this.height = height;
    }
  };
}

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

check("boot paints a whole page", paints.length > 0, `paints ${paints.length}`);
check(
  "painted page has ink",
  paints.some((paint) => paint.ink > 0),
  paints.map((paint) => paint.ink).join(","),
);
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
  "painted bitmap matches the canvas size",
  paints.some((paint) => paint.width === pageCanvas.width && paint.height === pageCanvas.height),
  `${paints[0] && paints[0].width}x${paints[0] && paints[0].height} vs ${pageCanvas.width}x${pageCanvas.height}`,
);

// 字号是文档字号，不是整页缩放：容器尺寸不能变，字形大小要变。
const beforeWidth = pageCanvas.style.width;
const beforeHeight = pageCanvas.style.height;
const fontSize = globalThis.document.getElementById("font-size");
fontSize.value = "30";
paints.length = 0;
fontSize.dispatchEvent({ type: "change" });
await settle();
await settle();
check(
  "font size keeps the page container fixed",
  pageCanvas.style.width === beforeWidth && pageCanvas.style.height === beforeHeight,
  `${beforeWidth}x${beforeHeight} -> ${pageCanvas.style.width}x${pageCanvas.style.height}`,
);
const bigPage = paints[paints.length - 1];
check(
  "font size changes the painted page",
  Boolean(bigPage) && bigPage.ink > 0,
  bigPage ? `ink ${bigPage.ink}` : "no paint",
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

paints.length = 0;
upload("fixtures/zh.epub");
await settle();
await settle();
check("upload swaps the book", text("book-title") !== "The Lantern at the End of Winter", text("book-title"));
check(
  "upload of a CJK book does not keep the latin-only builtin font",
  globalThis.document.getElementById("builtin-font").checked === false,
  "latin font left registered, CJK would render as missing glyphs",
);
// 测试环境用 `nofont=1` 跳过 5 MB 中文字体：没有字形时引擎画白页，
// 页数/状态仍然要推进。中文落墨由 `crates/ebook-web/tests` 的 Rust 用例覆盖。
check(
  "upload lays out the new chapter",
  paints.length > 0,
  `paints ${paints.length}`,
);
check("upload leaves no banner", banner() === "", banner());

const shelf = globalThis.document.getElementById("shelf");
check("import adds the book to the shelf", shelf.children.length === 1, `cards ${shelf.children.length}`);

// TXT：按标题行分章，走 eb_open_text + TxtReader，不经过 ZIP。
paints.length = 0;
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
  "txt upload paints the chapter",
  paints.length > 0,
  `paints ${paints.length}`,
);
check("txt upload joins the shelf", shelf.children.length === 2, `cards ${shelf.children.length}`);
// 全文搜索：输入关键词 → 结果列表；结果项带章名与高亮上下文。
const searchInput = globalThis.document.getElementById("search-input");
const searchResults = globalThis.document.getElementById("search-results");
searchInput.value = "雪";
searchInput.dispatchEvent({ type: "input" });
await settle();
check(
  "full-text search returns hits",
  (searchResults.children || []).length > 0,
  `results ${(searchResults.children || []).length}`,
);
check(
  "search summary reports a count",
  /条结果/.test(text("search-summary")),
  text("search-summary"),
);
searchInput.value = "";
searchInput.dispatchEvent({ type: "input" });
await settle();


paints.length = 0;
const before = text("book-title");
uploadBytes(new TextEncoder().encode("this is not a zip archive"), "broken.epub");
await settle();
await settle();
check("broken file reports instead of silently doing nothing", banner().length > 0, "no banner");
check("broken file keeps the previous book", text("book-title") === before, text("book-title"));
check("broken file is not added to the shelf", shelf.children.length === 2, `cards ${shelf.children.length}`);

// UA 默认样式（粗体合成、斜体、链接蓝色 + 下划线）现在由引擎绘制，
// 覆盖在 `crates/ebook-raster/tests` 与 `crates/ebook-web/tests`；
// 这里只确认一份全是中文的样式书也能打开并翻页。
paints.length = 0;
upload("fixtures/style.epub");
await settle();
await settle();
check(
  "style book opens and paints",
  paints.length > 0 && banner() === "",
  `paints ${paints.length}, banner ${banner()}`,
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
// 阅读器会留下 `setTimeout`（进度防抖、chrome 自动隐藏、字体请求超时），
// 不显式退出的话 bun 会一直等这些定时器，`xtask app` 就卡住了。
process.exit(0);
