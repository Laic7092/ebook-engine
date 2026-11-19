import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const ORIGIN = "http://test.local/";

const drawn = [];
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
  fillText(text, x, y) {
    drawn.push({ text, x, y, font: this.font });
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

globalThis.window = {
  location: { href: `${ORIGIN}web/`, search: "", protocol: "http:" },
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

const settle = () => new Promise((done) => setTimeout(done, 250));

await import(`${ROOT}/web/app.js`);
await settle();
await settle();

check("boot draws the requested book", drawn.length > 200, `drawn ${drawn.length}`);
check("boot fills the title", text("book-title").length > 0, text("book-title"));
check("boot leaves no banner", banner() === "", banner());
check("boot sizes the canvas", globalThis.document.getElementById("page").width > 0);
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

drawn.length = 0;
const before = text("book-title");
uploadBytes(new TextEncoder().encode("this is not a zip archive"), "broken.epub");
await settle();
await settle();
check("broken file reports instead of silently doing nothing", banner().length > 0, "no banner");
check("broken file keeps the previous book", text("book-title") === before, text("book-title"));

if (failures.length > 0) {
  console.error(`app test failed: ${failures.join(", ")}`);
  process.exit(1);
}
console.log("app ok");
