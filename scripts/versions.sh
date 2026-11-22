#!/usr/bin/env bash
# versions.sh — 外部工具版本的唯一出处。
#
# 能推导的版本不在这里写死，从各自的权威文件读：
#
#   toolchain      ← rust-toolchain.toml 的 `channel`
#   targets        ← rust-toolchain.toml 的 `targets`
#   wasm-bindgen   ← crates/ebook-web/Cargo.toml 的 `wasm-bindgen = "=x.y.z"`
#
# 只有“我们自己装的工具”（woff / esbuild）是字面量。workflow、setup.sh、
# generate.sh 都从这里取，别再各写一份——之前 `1.100.0` 抄了三个 workflow，
# 还配了一个“检查抄得对不对”的步骤，纯属自己给自己找事。
#
# 用法：
#   scripts/versions.sh toolchain      # 1.100.0
#   scripts/versions.sh targets        # wasm32-unknown-unknown
#   scripts/versions.sh wasm-bindgen   # 0.2.129
#   scripts/versions.sh woff           # 0.6.4
#   scripts/versions.sh esbuild        # 0.25.x
#   scripts/versions.sh all            # KEY=VALUE 行，可 eval
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

fail() {
	echo "versions.sh: $*" >&2
	exit 1
}

toolchain() {
	local channel
	channel="$(sed -n 's/^channel = "\(.*\)"$/\1/p' rust-toolchain.toml)"
	[ -n "$channel" ] || fail "rust-toolchain.toml 里没有 channel"
	printf '%s\n' "$channel"
}

targets() {
	local list
	# ["a", "b"] → a b：去掉引号与逗号，交给 rustup 按空白拆。
	list="$(sed -n 's/^targets = \[\(.*\)\]$/\1/p' rust-toolchain.toml | tr -d '",')"
	[ -n "$list" ] || fail "rust-toolchain.toml 里没有 targets"
	printf '%s\n' "$list"
}

wasm_bindgen() {
	local version
	version="$(sed -n 's/^wasm-bindgen = "=\(.*\)"$/\1/p' crates/ebook-web/Cargo.toml)"
	[ -n "$version" ] || fail "crates/ebook-web/Cargo.toml 里没有精确 pin 的 wasm-bindgen"
	printf '%s\n' "$version"
}

case "${1:-}" in
toolchain) toolchain ;;
targets) targets ;;
wasm-bindgen) wasm_bindgen ;;
woff) echo "0.6.4" ;;
esbuild) echo "0.25.x" ;;
all)
	printf 'TOOLCHAIN=%s\n' "$(toolchain)"
	printf 'TARGETS=%s\n' "$(targets)"
	printf 'WASM_BINDGEN=%s\n' "$(wasm_bindgen)"
	printf 'WOFF=%s\n' "0.6.4"
	printf 'ESBUILD=%s\n' "0.25.x"
	;;
-h | --help)
	sed -n '2,22p' "$0" | sed 's/^# \{0,1\}//'
	;;
*)
	echo "usage: scripts/versions.sh [toolchain|targets|wasm-bindgen|woff|esbuild|all]" >&2
	exit 2
	;;
esac
