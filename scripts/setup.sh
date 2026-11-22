#!/usr/bin/env bash
# setup.sh — 干净 checkout 的一次性初始化。
#
# 做四件事：
#   1. 补 wasm32 编译目标和 wasm-bindgen-cli（版本必须与 Cargo.toml 一致）；
#   2. 安装字体转换工具 woff（缺 fonttools 时也能生成 WOFF/WOFF2）；
#   3. 调 scripts/generate.sh 重建不进入 git 的字体、EPUB、图标、启动图；
#   4. 打印下一步命令。
#
# 用法：
#   scripts/setup.sh                 # 初始化（中文字体走 GitHub 代理下载）
#   scripts/setup.sh --skip-cjk      # 不下 5 MB 的中文字体
#   scripts/setup.sh --skip-web      # 不生成图标/启动图
#
# 环境变量：
#   CARGO_TARGET_DIR  构建目录
#   GH_PROXY          GitHub 代理前缀，默认 https://gh-proxy.com/
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# 版本只有一个出处：rust-toolchain.toml / crates/ebook-web/Cargo.toml /
# scripts/versions.sh。这里不再抄一遍。
WASM_BINDGEN_VERSION="$(scripts/versions.sh wasm-bindgen)"
WOFF_VERSION="$(scripts/versions.sh woff)"

say() { printf '\n==> %s\n' "$*"; }
need() {
	command -v "$1" >/dev/null 2>&1 || {
		echo "缺少命令: $1" >&2
		exit 1
	}
}

need cargo
need python3
need curl

if ! command -v rustup >/dev/null 2>&1; then
	echo "建议安装 rustup：https://rustup.rs" >&2
else
	# target 列表来自 rust-toolchain.toml，这里不再写死。
	for target in $(scripts/versions.sh targets); do
		if ! rustup target list --installed 2>/dev/null | grep -qx "$target"; then
			say "安装 $target"
			rustup target add "$target"
		fi
	done
fi

BINDGEN=""
if command -v wasm-bindgen >/dev/null 2>&1; then
	BINDGEN="$(command -v wasm-bindgen)"
elif [ -x "$HOME/.cargo/bin/wasm-bindgen" ]; then
	BINDGEN="$HOME/.cargo/bin/wasm-bindgen"
fi
if [ -z "$BINDGEN" ] || ! "$BINDGEN" --version | grep -q " $WASM_BINDGEN_VERSION$"; then
	say "安装 wasm-bindgen-cli $WASM_BINDGEN_VERSION"
	cargo install wasm-bindgen-cli --version "$WASM_BINDGEN_VERSION" --locked
fi

if ! command -v woff >/dev/null 2>&1 && [ ! -x "$HOME/.cargo/bin/woff" ]; then
	if ! python3 -c 'import fontTools' >/dev/null 2>&1; then
		say "安装 woff $WOFF_VERSION（WOFF / WOFF2 转换）"
		cargo install woff --version "$WOFF_VERSION" --features binary --locked
	fi
fi

say "生成二进制资源"
scripts/generate.sh "$@"

cat <<'EOF'

==> 下一步
  cargo test --workspace                     # Rust 测试
  ./web/build.sh                             # 编译 wasm 到 web/pkg
  python3 -m http.server                     # 打开 http://localhost:8000/web/
  ./web/minify.sh                            # 生成可部署的 web/dist
EOF
