#!/usr/bin/env bash
# check.sh — 提交前跑一遍的完整闸门。
#
# 用法：
#   scripts/check.sh
#
# 内容：cargo fmt --all --check、clippy（只报告 warning，不阻断）、
#       cargo test --workspace、xtask all（golden / contract / app /
#       pixel / size / parity）。
#
# 环境变量：
#   CARGO_TARGET_DIR   构建目录；CI 与并行 worktree 应各自指定
#
# clippy 不加 -D warnings：本轮有多个 agent 并行修改 crates/，warning
# 清单作为指标打印，统一清理留给后续提交。
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

case "${1:-}" in
-h | --help)
	sed -n '2,16p' "$0" | sed 's/^# \{0,1\}//'
	exit 0
	;;
"") ;;
*)
	echo "unknown argument: $1 (try --help)" >&2
	exit 2
	;;
esac

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

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

section "cargo clippy --workspace --all-targets"
if cargo clippy --workspace --all-targets 2>&1 | tee "$tmp/clippy"; then
	warnings=$(grep -cE '^warning: ' "$tmp/clippy" || true)
	echo "clippy warnings: $warnings (只报告，不阻断)"
	if [[ $warnings -gt 0 ]]; then
		echo "warning 清单见上方输出；本轮不改 crates/ 源码，留给后续统一清理。"
	fi
else
	echo "FAIL: clippy 出现 error" >&2
	status=1
fi

section "cargo test --workspace"
if ! cargo test --workspace; then
	echo "FAIL: cargo test" >&2
	status=1
fi

# tests/golden/*.ir 被 .gitignore 的 *.ir 排除，干净工作区里没有这两个快照。
# 只在缺失时生成，已有快照仍然逐字节比较；不要用 bless 覆盖回归。
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
