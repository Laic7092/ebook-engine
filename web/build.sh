#!/usr/bin/env bash
# build.sh — 编译 wasm 并生成 web/pkg。
#
# 跟随 CARGO_TARGET_DIR：并行 worktree 与 CI 各自指定构建目录时，
# wasm-bindgen 仍然能找到产物。
set -euo pipefail
cd "$(dirname "$0")/.."

target_dir=${CARGO_TARGET_DIR:-target}

cargo build -p ebook-web --release --target wasm32-unknown-unknown --features epub
rm -rf web/pkg
wasm-bindgen --target web --out-dir web/pkg "$target_dir/wasm32-unknown-unknown/release/ebook_web.wasm"
cp fixtures/en.epub web/en.epub
cp fixtures/font/test.ttf web/test.ttf
