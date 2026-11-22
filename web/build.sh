#!/usr/bin/env bash
# build.sh — 编译 wasm 并生成 web/pkg。
#
# 跟随 CARGO_TARGET_DIR：并行 worktree 与 CI 各自指定构建目录时，
# wasm-bindgen 仍然能找到产物。
set -euo pipefail
cd "$(dirname "$0")/.."
# 和 xtask 一致：尊重 CARGO_TARGET_DIR，否则每个 worktree 都得建符号链接。
TARGET_DIR="${CARGO_TARGET_DIR:-target}"
# 默认只带 epub；需要内置英文断字时用 FEATURES=epub,hyphen-en。
FEATURES="${FEATURES:-epub}"
# CLI 必须与 `wasm-bindgen` crate 的精确 pin 同版本（crates/ebook-web/Cargo.toml）。
want="$(scripts/versions.sh wasm-bindgen)"
got="$(wasm-bindgen --version | awk '{print $NF}')"
if [ "$got" != "$want" ]; then
	echo "wasm-bindgen $got ≠ $want：先 cargo install wasm-bindgen-cli --version $want --locked" >&2
	exit 1
fi
cargo build -p ebook-web --release --target wasm32-unknown-unknown --features "$FEATURES"
rm -rf web/pkg
wasm-bindgen --target web --out-dir web/pkg "$TARGET_DIR/wasm32-unknown-unknown/release/ebook_web.wasm"
# 生产用的拉丁回退字体；示例书与 demo.txt 不再进部署包。
cp fixtures/font/test.ttf web/latin.ttf
