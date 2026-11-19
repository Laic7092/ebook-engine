#!/usr/bin/env bash
# check.sh — 提交前跑一遍的完整闸门。
#
# 用法：
#   scripts/check.sh
#
# 内容：cargo fmt --all --check、cargo clippy -- -D warnings、
#       cargo test --workspace、xtask all（golden / contract / app /
#       pixel / size / parity）。
#
# 环境变量：
#   CARGO_TARGET_DIR   构建目录；CI 与并行 worktree 应各自指定
#
# clippy 与 CI 一致，按 -D warnings 阻断：warning 也是一次回归。
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

case "${1:-}" in
-h | --help)
	sed -n '2,15p' "$0" | sed 's/^# \{0,1\}//'
	exit 0
	;;
"") ;;
*)
	echo "unknown argument: $1 (try --help)" >&2
	exit 2
	;;
esac

section() {
	printf '\n==> %s\n' "$*"
}

status=0

section "cargo fmt --all --check"
if cargo fmt --all --check; then
	echo "ok"
else
	echo "FAIL: rustfmt 有未格式化的文件" >&2
	status=1
fi

section "cargo clippy --workspace --all-targets -- -D warnings"
if cargo clippy --workspace --all-targets -- -D warnings; then
	echo "ok"
else
	echo "FAIL: clippy 有 warning 或 error" >&2
	status=1
fi

section "cargo test --workspace"
if ! cargo test --workspace; then
	echo "FAIL: cargo test" >&2
	status=1
fi

section "optional hyphen-en feature"
if ! cargo clippy -p ebook-layout --all-targets --features hyphen-en -- -D warnings; then
	echo "FAIL: clippy (ebook-layout, hyphen-en)" >&2
	status=1
fi
if ! cargo clippy -p ebook-font --all-targets --features hyphen-en -- -D warnings; then
	echo "FAIL: clippy (ebook-font, hyphen-en)" >&2
	status=1
fi
if ! cargo test -p ebook-layout --features hyphen-en --test hyphen; then
	echo "FAIL: cargo test (ebook-layout, hyphen-en)" >&2
	status=1
fi
if ! cargo test -p ebook-font --features hyphen-en --test embedded; then
	echo "FAIL: cargo test (ebook-font, hyphen-en)" >&2
	status=1
fi

# tests/golden/*.ir 已入库（.gitignore 的 `!tests/golden/*.ir` 放行）。
# 这里只为“本地刚 checkout、快照被误删”兜底：缺失时生成，已有快照仍然
# 逐字节比较；不要用 bless 覆盖回归。
section "prepare golden IR"
if [[ ! -f tests/golden/demo.ir || ! -f tests/golden/epub.ir ]]; then
	echo "tests/golden/*.ir 未入库，先生成本地快照"
	if ! cargo run -p xtask -- bless; then
		echo "FAIL: xtask bless" >&2
		status=1
	fi
else
	echo "ok"
fi

section "cargo run -p xtask -- all"
if ! cargo run -p xtask -- all; then
	echo "FAIL: xtask all" >&2
	status=1
fi

section "done"
if ((status != 0)); then
	echo "scripts/check.sh: FAILED" >&2
	exit 1
fi
echo "scripts/check.sh: all checks passed"
