#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
# 和 xtask 一致：尊重 CARGO_TARGET_DIR，否则每个 worktree 都得建符号链接。
TARGET_DIR="${CARGO_TARGET_DIR:-target}"
cargo build -p ebook-web --release --target wasm32-unknown-unknown --features epub
rm -rf web/pkg
wasm-bindgen --target web --out-dir web/pkg "$TARGET_DIR/wasm32-unknown-unknown/release/ebook_web.wasm"
cp fixtures/en.epub web/en.epub
cp fixtures/font/test.ttf web/test.ttf
