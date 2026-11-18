import { readFileSync, readdirSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { decodeLayout } from "../web/decode.js";
import { encodeLayout } from "../web/encode.js";

const HERE = dirname(fileURLToPath(import.meta.url));
const GOLDEN_DIR = resolve(HERE, "golden");

function compare(name, bytes) {
  const layout = decodeLayout(bytes);
  const again = encodeLayout(layout);
  if (again.length !== bytes.length) {
    return `length ${again.length} != ${bytes.length}`;
  }
  for (let i = 0; i < bytes.length; i += 1) {
    if (again[i] !== bytes[i]) {
      return `byte ${i}: ${again[i]} != ${bytes[i]}`;
    }
  }
  checkSpans(name, layout);
  return null;
}

function checkSpans(name, layout) {
  const ops = layout.display.ops.length;
  const lines = layout.runs.lines.length;
  let cursor = 0;
  for (const page of layout.pages) {
    if (page.ops.start !== cursor && page.ops.len > 0) {
      throw new Error(`${name}: page ${page.index} ops start ${page.ops.start} != ${cursor}`);
    }
    cursor += page.ops.len;
    if (page.lines.start + page.lines.len > lines) {
      throw new Error(`${name}: page ${page.index} lines overflow`);
    }
    if (page.lines.len === 0) {
      throw new Error(`${name}: page ${page.index} has no lines`);
    }
  }
  if (cursor !== ops) {
    throw new Error(`${name}: pages cover ${cursor} ops of ${ops}`);
  }
  let end = 0;
  for (const page of layout.pages) {
    if (page.lines.start < end) {
      throw new Error(`${name}: page ${page.index} lines overlap`);
    }
    end = page.lines.start + page.lines.len;
  }
  if (end !== lines) {
    throw new Error(`${name}: pages cover ${end} lines of ${lines}`);
  }
}

const names = process.argv.length > 2
  ? process.argv.slice(2)
  : readdirSync(GOLDEN_DIR).filter((n) => n.endsWith(".ir")).sort().map((n) => resolve(GOLDEN_DIR, n));

let failed = 0;
for (const path of names) {
  const bytes = readFileSync(path);
  const problem = compare(path, bytes);
  if (problem) {
    console.error(`contract broken: ${path}: ${problem}`);
    failed += 1;
  } else {
    console.log(`contract ok: ${path} (${bytes.length} bytes round-trip)`);
  }
}

if (failed > 0) {
  process.exit(1);
}
