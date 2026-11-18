import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { resolve } from "node:path";

const require = createRequire(import.meta.url);
const [pkg, textPath, irPath] = process.argv.slice(2);

const mod = require(resolve(pkg, "ebook_web.js"));
const text = readFileSync(textPath, "utf8");
const native = readFileSync(irPath);
const fromWasm = Buffer.from(mod.eb_layout(text, 600, 800, 24));

if (fromWasm.length !== native.length) {
  console.error(`parity length mismatch: wasm=${fromWasm.length} native=${native.length}`);
  process.exit(1);
}
for (let i = 0; i < native.length; i += 1) {
  if (fromWasm[i] !== native[i]) {
    console.error(`parity mismatch at byte ${i}`);
    process.exit(1);
  }
}
console.log(`parity ok: ${native.length} bytes identical`);
