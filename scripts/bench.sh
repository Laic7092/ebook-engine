#!/usr/bin/env bash
# bench.sh — 粗粒度性能回归闸门。
#
# 用法：
#   scripts/bench.sh              # 与 scripts/bench_baseline.txt 比较
#   scripts/bench.sh --update     # 用本机结果重写基线
#
# 输出格式：每行 "name<TAB>median_ns"（由 `xtask bench` 生成，median 为
# N 次单独计时的中位数，前 3 次预热；脚本默认跑 3 个独立进程再取中位数）。
# 可直接 diff、可重放。
#
# 环境变量：
#   BENCH_TOL     允许的 slowdown 倍数（默认 1.15，即 15%）
#   BENCH_ITERS   每个 benchmark 的采样次数（默认 31）
#   BENCH_COUNT   独立进程重复次数，取各次中位数的中位数（默认 3）
#   CARGO_TARGET_DIR  构建目录；并行 worktree 应各自指定
#
# 重新基线：在目标机器（空闲、插电、固定 CPU governor）上运行
#   scripts/bench.sh --update
# 然后把 scripts/bench_baseline.txt 的改动一起提交。绝对 ns 与 CPU、
# 编译器版本强相关，这个闸门只抓大幅回归，不抓几个百分点的抖动。
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

baseline=scripts/bench_baseline.txt
tol=${BENCH_TOL:-1.15}

update=0
for arg in "$@"; do
	case "$arg" in
	--update | --bless) update=1 ;;
	-h | --help)
		sed -n '2,22p' "$0" | sed 's/^# \{0,1\}//'
		exit 0
		;;
	*)
		echo "unknown argument: $arg (try --help)" >&2
		exit 2
		;;
	esac
done

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

count=${BENCH_COUNT:-3}
if [[ ! $count =~ ^[1-9][0-9]*$ ]]; then
	echo "error: BENCH_COUNT 必须是正整数" >&2
	exit 2
fi

echo "==> xtask bench (release, BENCH_ITERS=${BENCH_ITERS:-31}, runs=$count)"
: >"$tmp/raw"
for ((run = 1; run <= count; run += 1)); do
	cargo run --release -q -p xtask -- bench >>"$tmp/raw"
done

# 每个 benchmark 在所有独立进程里各有一个中位数，再取这些中位数的中位数。
# 只保留 "name<TAB>number" 行，注释与空行忽略；awk 用插入排序保证确定性。
awk -F '\t' '
	NF == 2 && $1 !~ /^#/ && $2 ~ /^[0-9]+$/ {
		values[$1] = values[$1] " " $2
		seen[$1] = 1
	}
	END {
		for (name in seen) {
			n = split(values[name], a, " ")
			for (i = 2; i <= n; i += 1) {
				v = a[i]
				j = i - 1
				while (j > 0 && a[j] + 0 > v + 0) {
					a[j + 1] = a[j]
					j -= 1
				}
				a[j + 1] = v
			}
			print name "\t" a[int((n + 1) / 2)]
		}
	}
' "$tmp/raw" | sort >"$tmp/values"

# 给人工看的最终结果（也是可 diff 的机器格式）。
cat "$tmp/values"

if [[ ! -s $tmp/values ]]; then
	echo "error: xtask bench 没有输出任何可读结果" >&2
	exit 1
fi

if ((update)); then
	{
		echo "# ebook-engine benchmark baselines (median ns per iteration)."
		echo "# Captured on: $(uname -srm) / $(rustc --version)"
		echo "#"
		echo "# Refresh with: scripts/bench.sh --update"
		echo "# Coarse by design: absolute times depend on CPU and compiler; the"
		echo "# gate allows BENCH_TOL (default 1.15x) of slowdown."
		echo "# Format: name<TAB>median_ns (median of $count process medians)"
		cat "$tmp/values"
	} >"$baseline"
	echo "==> baseline written to $baseline"
	exit 0
fi

[[ -f $baseline ]] || {
	echo "error: $baseline not found; run scripts/bench.sh --update" >&2
	exit 1
}

awk -F '\t' 'NF == 2 && $1 !~ /^#/ { print $1 "\t" $2 }' "$baseline" >"$tmp/base"

status=0
checked=0
while IFS=$'\t' read -r name current; do
	base=$(awk -F '\t' -v n="$name" '$1 == n { print $2 }' "$tmp/base")
	if [[ -z $base ]]; then
		printf 'NEW   %s: %s ns (no baseline entry; run --update)\n' "$name" "$current"
		status=1
		continue
	fi
	checked=$((checked + 1))
	if ! awk -v name="$name" -v base="$base" -v cur="$current" -v tol="$tol" 'BEGIN {
		if (base + 0 <= 0) {
			printf "FAIL  %s: invalid baseline %s\n", name, base
			exit 1
		}
		ratio = cur / base
		if (ratio > tol) {
			printf "FAIL  %s: %d ns vs baseline %d ns (%.2fx slower; tolerance %.2fx)\n", name, cur, base, ratio, tol
			exit 1
		}
		if (ratio < 1 / tol) {
			printf "FAST  %s: %d ns vs baseline %d ns (%.2fx; consider --update)\n", name, cur, base, ratio
			exit 0
		}
		printf "OK    %s: %d ns vs baseline %d ns (%.2fx; tolerance %.2fx)\n", name, cur, base, ratio, tol
	}'; then
		status=1
	fi
done <"$tmp/values"

if ((checked == 0)); then
	echo "error: 没有任何 benchmark 与基线匹配" >&2
	exit 1
fi

# 基线里存在、这次却没有测到的条目，说明 benchmark 被删改，也按失败处理。
while IFS=$'\t' read -r name _; do
	if ! awk -F '\t' -v n="$name" '$1 == n { found = 1 } END { exit found ? 0 : 1 }' "$tmp/values"; then
		printf 'MISSING %s: baseline entry not measured\n' "$name"
		status=1
	fi
done <"$tmp/base"

if ((status != 0)); then
	echo "bench: FAILED" >&2
	exit 1
fi
echo "bench: OK ($checked benchmarks within ${tol}x of baseline)"
