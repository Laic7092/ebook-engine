// 文件识别与文本解码。
//
// EPUB 是 ZIP（PK\x03\x04）；其余交给 TXT。TXT 的编码没有元数据可依赖，
// 浏览器 `TextDecoder` 支持 UTF-8 / UTF-16 / GB18030 / Big5 / Shift_JIS，
// 这里按 BOM、严格 UTF-8、再按“替换字符与控制字符最少”挑选候选。

const BOMS = [
  { bytes: [0xef, 0xbb, 0xbf], encoding: "utf-8" },
  { bytes: [0xff, 0xfe], encoding: "utf-16le" },
  { bytes: [0xfe, 0xff], encoding: "utf-16be" },
];

const CANDIDATES = ["gb18030", "big5", "shift_jis", "euc-kr", "windows-1252"];

export function looksLikeZip(bytes) {
  return (
    bytes.length >= 4 &&
    bytes[0] === 0x50 &&
    bytes[1] === 0x4b &&
    bytes[2] === 0x03 &&
    bytes[3] === 0x04
  );
}

export function detectFormat(name, bytes) {
  const lower = String(name || "").toLowerCase();
  if (lower.endsWith(".txt") || lower.endsWith(".md")) {
    return "txt";
  }
  if (lower.endsWith(".epub")) {
    return "epub";
  }
  return looksLikeZip(bytes) ? "epub" : "txt";
}

export function titleFromName(name) {
  const base = String(name || "untitled").split(/[\\/]/).pop();
  const stem = base.replace(/\.[^.]+$/, "").replace(/[_-]+/g, " ").trim();
  return stem || base || "Untitled";
}

function bomLength(bytes) {
  for (const bom of BOMS) {
    if (bytes.length >= bom.bytes.length && bom.bytes.every((byte, i) => bytes[i] === byte)) {
      return bom;
    }
  }
  return null;
}

function decodeWith(bytes, encoding, fatal = false) {
  try {
    return new TextDecoder(encoding, { fatal }).decode(bytes);
  } catch (error) {
    return null;
  }
}

function penalty(text) {
  let score = 0;
  const sample = text.slice(0, 60000);
  for (const ch of sample) {
    const code = ch.codePointAt(0);
    if (ch === "\uFFFD") {
      score += 1000;
    } else if (code < 0x20 && ch !== "\n" && ch !== "\r" && ch !== "\t") {
      score += 80;
    } else if (code >= 0xe000 && code <= 0xf8ff) {
      score += 20;
    }
  }
  return score;
}

export function decodeText(bytes) {
  if (!bytes || !bytes.length) {
    return "";
  }
  const bom = bomLength(bytes);
  if (bom) {
    const decoded = decodeWith(bytes.subarray(bom.bytes.length), bom.encoding, false);
    if (decoded !== null) {
      return decoded;
    }
  }
  // 严格 UTF-8 先试：中文 TXT 绝大多数是 UTF-8，且失败即是明确信号。
  const utf8 = decodeWith(bytes, "utf-8", true);
  if (utf8 !== null) {
    return utf8;
  }
  let best = null;
  let bestScore = Infinity;
  for (const encoding of CANDIDATES) {
    const decoded = decodeWith(bytes, encoding, true);
    if (decoded === null) {
      continue;
    }
    const score = penalty(decoded);
    if (score < bestScore) {
      best = decoded;
      bestScore = score;
    }
  }
  if (best !== null) {
    return best;
  }
  return decodeWith(bytes, "utf-8", false) || "";
}

/// 文件内容指纹。用双 32 位 FNV-1a 覆盖完整字节，导入同一本书不会重复，
/// 也不需要 crypto.subtle（非安全上下文里不可用）。
export function fingerprint(bytes, name = "") {
  let a = 0x811c9dc5;
  let b = 0x9e3779b9;
  for (let i = 0; i < bytes.length; i += 1) {
    const byte = bytes[i];
    a ^= byte;
    a = Math.imul(a, 0x01000193);
    b ^= byte + i;
    b = Math.imul(b, 0x85ebca6b);
  }
  const tail = `${name}:${bytes.length}`;
  for (let i = 0; i < tail.length; i += 1) {
    b ^= tail.charCodeAt(i);
    b = Math.imul(b, 0xc2b2ae35);
  }
  return `${(a >>> 0).toString(16).padStart(8, "0")}${(b >>> 0).toString(16).padStart(8, "0")}`;
}

export function formatBytes(size) {
  if (!Number.isFinite(size) || size <= 0) {
    return "";
  }
  if (size < 1024) {
    return `${size} B`;
  }
  if (size < 1024 * 1024) {
    return `${(size / 1024).toFixed(1)} KB`;
  }
  return `${(size / (1024 * 1024)).toFixed(1)} MB`;
}
