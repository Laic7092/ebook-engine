// 排版 worker：把整章 `Layout` 从主线程挪走。
//
// 主线程仍然负责打开书、注册字体（命中/选区/字形位图都要用），这里复制一份
// BOOK / FontBook，只接收排版请求并返回 IR 字节。两边字体注册顺序必须一致，
// 否则 IR 里的 font id 在主线程取不到对应字形。

import init, {
  eb_font,
  eb_font_face,
  eb_font_face_count,
  eb_font_reset,
  eb_last_error,
  eb_layout_style,
  eb_open,
  eb_open_text,
} from "./pkg/ebook_web.js";

let ready = null;

function ensure() {
  if (!ready) {
    ready = init();
  }
  return ready;
}

function openError(info) {
  try {
    const value = JSON.parse(info);
    return value && value.error ? String(value.error) : "";
  } catch (error) {
    return "";
  }
}

function loadEmbeddedFaces() {
  const count = eb_font_face_count();
  for (let index = 0; index < count; index += 1) {
    eb_font_face(index);
  }
}

self.onmessage = async (event) => {
  const msg = event.data || {};
  try {
    await ensure();
    if (msg.type === "open") {
      const info =
        msg.kind === "txt"
          ? eb_open_text(msg.text || "", msg.name || "untitled.txt")
          : eb_open(new Uint8Array(msg.bytes));
      const error = openError(info);
      if (error) {
        throw new Error(error);
      }
      if (msg.loadEmbedded) {
        loadEmbeddedFaces();
      }
      self.postMessage({ type: "open", id: msg.id });
      return;
    }
    if (msg.type === "font") {
      eb_font(new Uint8Array(msg.bytes));
      return;
    }
    if (msg.type === "font-reset") {
      eb_font_reset();
      return;
    }
    if (msg.type === "layout") {
      const bytes = eb_layout_style(
        msg.spine,
        msg.width,
        msg.height,
        msg.left,
        msg.right,
        msg.top,
        msg.bottom,
        msg.fontScale,
        msg.lineScale,
        msg.letter,
        msg.align,
      );
      if (!bytes || !bytes.length) {
        self.postMessage({
          type: "layout",
          id: msg.id,
          error: eb_last_error() || "排版失败",
        });
        return;
      }
      self.postMessage({ type: "layout", id: msg.id, bytes }, [bytes.buffer]);
      return;
    }
  } catch (error) {
    const message = String((error && error.message) || error || "worker error");
    if (msg.type === "layout") {
      self.postMessage({ type: "layout", id: msg.id, error: message });
    } else {
      self.postMessage({ type: "fatal", id: msg.id, error: message });
    }
  }
};
