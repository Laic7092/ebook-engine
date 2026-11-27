// 引擎 worker：整个 wasm 实例（BOOK / FONT / LAST）只活在这个线程里。
//
// 主线程不 import wasm 了：开书、注册字体、排版、整页光栅、命中/选区/链接
// 查询都在这里完成，回去的只有页元数据、位图和已经校验过的对象。这样开书
// 只解析一次、字体只注册一次，切章时主线程没有解析/排版/光栅的长任务。
//
// 消息按到达顺序串行处理（`chain`）：排版和取位图之间不会插进别的请求，
// 否则位图可能读到上一章的 `LAST`。

import init, {
  eb_base_font_size,
  eb_font,
  eb_font_count,
  eb_font_error,
  eb_font_face,
  eb_font_face_count,
  eb_font_reset,
  eb_fragment_line,
  eb_hit,
  eb_last_error,
  eb_layout_style,
  eb_link_at,
  eb_open,
  eb_open_text,
  eb_page_bitmap,
  eb_page_meta,
  eb_progress,
  eb_resolve_link,
  eb_search,
  eb_search_locate,
  eb_selection,
  eb_toc,
} from "./pkg/ebook_web.js";
import {
  decodeHit,
  decodeLink,
  decodeProgress,
  decodeResolvedLink,
  decodeSearch,
  decodeSelection,
  decodeToc,
} from "./decode.js";
import { fingerprint } from "./text.js";

let ready = null;

function ensure() {
  if (!ready) {
    ready = init();
  }
  return ready;
}

function loadEmbeddedFaces() {
  const count = eb_font_face_count();
  let loaded = 0;
  let error = "";
  for (let index = 0; index < count; index += 1) {
    if (eb_font_face(index) === 0xffffffff) {
      error = eb_font_error() || error;
      continue;
    }
    loaded += 1;
  }
  return { loaded, error };
}

function bytesOf(value) {
  if (!value) {
    return new Uint8Array(0);
  }
  if (value instanceof Uint8Array) {
    return value;
  }
  if (value instanceof ArrayBuffer) {
    return new Uint8Array(value);
  }
  return new Uint8Array(value.buffer || value, value.byteOffset || 0, value.byteLength);
}

/** 页元数据：`[count] + 每页 9 个 f32`，主线程按 stride 9 读。 */
function readPages(buffer) {
  const bytes = new Uint8Array(buffer);
  if (bytes.length < 4) {
    return { count: 0, values: new Float32Array(0) };
  }
  const count = new DataView(bytes.buffer, bytes.byteOffset, 4).getUint32(0, true);
  const stride = 9;
  const values = new Float32Array(count * stride);
  const view = new DataView(bytes.buffer, bytes.byteOffset + 4, count * stride * 4);
  for (let index = 0; index < values.length; index += 1) {
    values[index] = view.getFloat32(index * 4, true);
  }
  return { count, values };
}

function layout(params) {
  const bytes = eb_layout_style(
    params.spine,
    params.width,
    params.height,
    params.left,
    params.right,
    params.top,
    params.bottom,
    params.fontScale,
    params.lineScale,
    params.letter,
    params.align,
  );
  if (!bytes || !bytes.length) {
    throw new Error(eb_last_error() || "排版失败");
  }
  return readPages(eb_page_meta());
}

/** 整页 RGBA：把引擎返回的那一块原样 `transfer` 回主线程。 */
function bitmap(msg) {
  const bytes = eb_page_bitmap(msg.page, msg.dpr, msg.ink || 0, msg.paper || 0, Boolean(msg.bold));
  if (!bytes || bytes.length < 8) {
    return { value: null };
  }
  if (bytes.byteOffset === 0 && bytes.byteLength === bytes.buffer.byteLength) {
    return { value: bytes, transfer: [bytes.buffer] };
  }
  const copy = bytes.slice();
  return { value: copy, transfer: [copy.buffer] };
}

const handlers = {
  /** 预热：只保证 wasm 实例化完成。 */
  ready() {
    return { value: true };
  },
  open(msg) {
    const info =
      msg.kind === "txt"
        ? eb_open_text(msg.text || "", msg.name || "untitled.txt")
        : eb_open(bytesOf(msg.bytes));
    const book = JSON.parse(info);
    if (book.error) {
      throw new Error(book.error);
    }
    let faces = { loaded: 0, error: "" };
    if (msg.loadEmbedded) {
      faces = loadEmbeddedFaces();
    }
    return {
      value: {
        book,
        baseFontSize: Number(eb_base_font_size()) || 0,
        faces,
      },
    };
  },
  font(msg) {
    const id = eb_font(bytesOf(msg.bytes));
    if (id === 0xffffffff) {
      throw new Error(eb_font_error() || "字体无法解析");
    }
    return { value: id };
  },
  fontFaces() {
    return { value: loadEmbeddedFaces() };
  },
  fontFace(msg) {
    const id = eb_font_face(msg.index);
    return { value: id === 0xffffffff ? null : id, error: eb_font_error() };
  },
  fontReset() {
    eb_font_reset();
    return { value: true };
  },
  fontCount() {
    return { value: eb_font_count() };
  },
  fontFaceCount() {
    return { value: eb_font_face_count() };
  },
  fontError() {
    return { value: eb_font_error() };
  },
  layout(msg) {
    const value = layout(msg);
    return { value, transfer: [value.values.buffer] };
  },
  pageMeta() {
    const value = readPages(eb_page_meta());
    return { value, transfer: [value.values.buffer] };
  },
  bitmap,
  hit(msg) {
    return { value: decodeHit(eb_hit(msg.page, msg.x, msg.y)) };
  },
  selection(msg) {
    return {
      value: decodeSelection(
        eb_selection(msg.a.page, msg.a.line, msg.a.byte, msg.b.page, msg.b.line, msg.b.byte),
      ),
    };
  },
  linkAt(msg) {
    return { value: decodeLink(eb_link_at(msg.page, msg.x, msg.y)) };
  },
  progress(msg) {
    return { value: decodeProgress(eb_progress(msg.page, msg.line || 0, msg.byte || 0)) };
  },
  toc() {
    return { value: decodeToc(eb_toc()) };
  },
  fragmentLine(msg) {
    const line = eb_fragment_line(msg.fragment);
    return { value: line === 0xffffffff ? null : line };
  },
  search(msg) {
    return { value: decodeSearch(eb_search(msg.query || "", msg.limit || 0)) };
  },
  searchLocate(msg) {
    const line = eb_search_locate(msg.node, msg.offset);
    return { value: line === 0xffffffff ? null : line };
  },
  resolveLink(msg) {
    return { value: decodeResolvedLink(eb_resolve_link(msg.href)) };
  },
  lastError() {
    return { value: eb_last_error() };
  },
  fingerprint(msg) {
    return { value: fingerprint(bytesOf(msg.bytes), msg.name || "") };
  },
};

function reply(id, body, transfer) {
  self.postMessage({ id, ...body }, transfer || []);
}

async function handle(msg) {
  const handler = handlers[msg.type];
  if (!handler) {
    reply(msg.id, { ok: false, error: `未知的引擎调用：${msg.type}` });
    return;
  }
  try {
    await ensure();
    const result = (await handler(msg)) || {};
    reply(msg.id, { ok: true, value: result.value, error: result.error }, result.transfer);
  } catch (error) {
    const message = String((error && error.message) || error || "引擎调用失败");
    reply(msg.id, { ok: false, error: message });
  }
}

let chain = Promise.resolve();

self.onmessage = (event) => {
  const msg = event.data || {};
  chain = chain.then(() => handle(msg)).catch(() => {});
};
