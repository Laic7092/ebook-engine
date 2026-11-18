#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build -p ebook-web --release --target wasm32-unknown-unknown --features epub
rm -rf web/pkg
wasm-bindgen --target web --out-dir web/pkg target/wasm32-unknown-unknown/release/ebook_web.wasm
cp fixtures/en.epub web/en.epub
cp fixtures/font/test.ttf web/test.ttf
