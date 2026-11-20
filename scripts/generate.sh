#!/usr/bin/env bash
# generate.sh — 生成所有不进入 git 的二进制资源。
#
# 仓库只提交代码与文本数据；字体、EPUB 夹具、图标、启动图都在这里重建。
# 依赖：python3（标准库）、curl；woff/woff2 需要 `woff` CLI 或 fonttools。
#
# 用法：
#   scripts/generate.sh                # 缺什么补什么（中文字体走 GitHub 代理）
#   scripts/generate.sh --force        # 全部重建
#   scripts/generate.sh --skip-cjk     # 不下 5 MB 的中文字体
#   scripts/generate.sh --skip-web     # 不生成图标/启动图（也会跳过中文字体）
#   scripts/generate.sh --web-only     # 只生成 web/ 部署要的资源（字体/图标/启动图）
#
# 环境变量：
#   GH_PROXY    GitHub 代理前缀，默认 https://gh-proxy.com/；设为空字符串可直连
#   PYTHON      Python 解释器，默认 python3
#   WOFF        woff CLI 路径，默认从 PATH 查找；也可用 fonttools 兜底
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

FORCE=0
WITH_CJK=1
SKIP_WEB=0
WEB_ONLY=0
for arg in "$@"; do
	case "$arg" in
	--force) FORCE=1 ;;
	--skip-cjk) WITH_CJK=0 ;;
	--skip-web) SKIP_WEB=1; WITH_CJK=0 ;;
	--web-only) WEB_ONLY=1 ;;
	-h | --help)
		sed -n '2,19p' "$0" | sed 's/^# \{0,1\}//'
		exit 0
		;;
	*)
		echo "未知参数: $arg（--help 查看用法）" >&2
		exit 2
		;;
	esac
done

PYTHON="${PYTHON:-python3}"
GH_PROXY="${GH_PROXY-https://gh-proxy.com/}"
FONT_DIR="$ROOT/fixtures/font"
FONT="$FONT_DIR/test.ttf"
CACHE="$ROOT/.cache"

say() { printf '==> %s\n' "$*"; }

command -v curl >/dev/null 2>&1 || {
	echo "缺少命令: curl" >&2
	exit 1
}
command -v "$PYTHON" >/dev/null 2>&1 || {
	echo "缺少命令: $PYTHON" >&2
	exit 1
}

mkdir -p "$FONT_DIR" "$CACHE"

# GitHub 默认走代理，国内直连 release/raw 经常超时。
github_url() {
	case "$1" in
	https://github.com/* | https://raw.githubusercontent.com/*)
		if [ -n "$GH_PROXY" ]; then
			printf '%s%s' "$GH_PROXY" "$1"
		else
			printf '%s' "$1"
		fi
		;;
	*) printf '%s' "$1" ;;
	esac
}

try_download() {
	local url="$1" out="$2" real tmp
	real="$(github_url "$url")"
	tmp="$out.tmp.$$"
	mkdir -p "$(dirname "$out")"
	say "下载 $(basename "$out") ← $real"
	if curl -fL --retry 2 --connect-timeout 20 --progress-bar -o "$tmp" "$real"; then
		mv "$tmp" "$out"
		return 0
	fi
	rm -f "$tmp"
	echo "下载失败: $real" >&2
	return 1
}

# 依次尝试多个镜像，全部失败才退出。
download() {
	local out="$1"
	shift
	local url
	for url in "$@"; do
		try_download "$url" "$out" && return 0
	done
	echo "无法下载 $(basename "$out")；可设置 GH_PROXY，或手动放到该路径" >&2
	exit 1
}

# ---------------------------------------------------------------- 字体

if [ "$FORCE" = 1 ] || [ ! -s "$FONT" ]; then
	# 只下 757 KB 的 ttf，不拉整个 5 MB release 压缩包。
	download "$FONT" \
		"https://cdn.jsdelivr.net/npm/dejavu-fonts-ttf@2.37.3/ttf/DejaVuSans.ttf"
fi

# web-only 只需要 latin.ttf 与 cjk.woff2：不生成测试字体、不要求 woff CLI。
if [ "$WEB_ONLY" = 0 ]; then
	WOFF="${WOFF:-}"
	if [ -z "$WOFF" ]; then
		if command -v woff >/dev/null 2>&1; then
			WOFF="$(command -v woff)"
		elif [ -x "$HOME/.cargo/bin/woff" ]; then
			WOFF="$HOME/.cargo/bin/woff"
		elif "$PYTHON" -c 'import fontTools' >/dev/null 2>&1; then
			WOFF="python-fonttools"
		else
			echo "缺少 woff/woff2 转换工具。运行 scripts/setup.sh，或：" >&2
			echo "  cargo install woff --version 0.6.4 --features binary --locked" >&2
			exit 1
		fi
	fi

	make_woff() {
		local out="$1"
		if [ "$FORCE" = 1 ] || [ ! -s "$out" ] || [ "$FONT" -nt "$out" ]; then
			if [ "$WOFF" = "python-fonttools" ]; then
				"$PYTHON" - "$FONT" "$out" <<-'PY'
	import sys
	from fontTools.ttLib import TTFont

	font = TTFont(sys.argv[1])
	font.flavor = "woff2" if sys.argv[2].endswith(".woff2") else "woff"
	font.save(sys.argv[2])
	print(f"wrote {sys.argv[2]}")
	PY
			else
				"$WOFF" "$FONT" "$out"
			fi
		fi
	}

	say "生成 WOFF / WOFF2 测试字体"
	make_woff "$FONT_DIR/test.woff"
	make_woff "$FONT_DIR/test.woff2"
fi

if [ "$FORCE" = 1 ] || [ ! -s "$ROOT/web/latin.ttf" ] || ! cmp -s "$FONT" "$ROOT/web/latin.ttf"; then
	say "复制 web/latin.ttf"
	cp "$FONT" "$ROOT/web/latin.ttf"
fi

# ---------------------------------------------------------------- 中文字体

if [ "$WITH_CJK" = 1 ] && { [ "$FORCE" = 1 ] || [ ! -s "$ROOT/web/cjk.woff2" ]; }; then
	# GitHub raw 默认走 GH_PROXY（https://gh-proxy.com/），jsDelivr 仅作兜底。
	download "$ROOT/web/cjk.woff2" \
		"https://github.com/refinec/PingFangSC/raw/main/woff2/PingFangSC-Regular.woff2" \
		"https://cdn.jsdelivr.net/gh/refinec/PingFangSC@main/woff2/PingFangSC-Regular.woff2"
	if [ "$(head -c 4 "$ROOT/web/cjk.woff2")" != "wOF2" ]; then
		rm -f "$ROOT/web/cjk.woff2"
		echo "下载到的不是 WOFF2 文件，已删除" >&2
		exit 1
	fi
fi

# ---------------------------------------------------------------- Unicode / 断字

if [ "$WEB_ONLY" = 0 ]; then
	LINEBREAK="$ROOT/data/unicode/LineBreak.txt"
	LINEBREAK_TABLE="$ROOT/crates/ebook-layout/src/linebreak_table.rs"
	if [ "$FORCE" = 1 ] || [ ! -s "$LINEBREAK" ]; then
		download "$LINEBREAK" "https://www.unicode.org/Public/18.0.0/ucd/LineBreak.txt"
	fi
	if [ "$FORCE" = 1 ] || [ ! -s "$LINEBREAK_TABLE" ] || [ "$LINEBREAK" -nt "$LINEBREAK_TABLE" ]; then
		say "生成 linebreak_table.rs"
		"$PYTHON" data/gen_linebreak.py
	fi

	HYPHEN_TEX="$CACHE/hyph-en-us.tex"
	HYPHEN_PAT="$ROOT/crates/ebook-layout/src/hyphen/en.pat"
	if [ "$FORCE" = 1 ] || [ ! -s "$HYPHEN_TEX" ]; then
		download "$HYPHEN_TEX" \
			"https://github.com/hyphenation/tex-hyphen/raw/master/hyph-utf8/tex/generic/hyph-utf8/patterns/tex/hyph-en-us.tex"
	fi
	if [ "$FORCE" = 1 ] || [ ! -s "$HYPHEN_PAT" ] || [ "$HYPHEN_TEX" -nt "$HYPHEN_PAT" ]; then
		say "生成英文断字表 en.pat"
		"$PYTHON" data/gen_hyphen.py --tex "$HYPHEN_TEX" --out "$HYPHEN_PAT"
	fi

	# ---------------------------------------------------------------- EPUB 夹具

	say "生成 EPUB 夹具"
	"$PYTHON" fixtures/make_epub.py
	"$PYTHON" fixtures/make_en_epub.py
	"$PYTHON" fixtures/make_zh_epub.py
	"$PYTHON" fixtures/make_images_epub.py
	"$PYTHON" fixtures/make_misc_epubs.py
fi

# ---------------------------------------------------------------- Web 图标

if [ "$SKIP_WEB" = 0 ]; then
	if [ "$FORCE" = 1 ] || [ ! -s "$ROOT/web/icon-512.png" ] || [ ! -s "$ROOT/web/icon-192.png" ] || [ ! -s "$ROOT/web/icon-180.png" ]; then
		say "生成 PWA 图标"
		"$PYTHON" scripts/gen-icons.py
	fi
	if [ "$FORCE" = 1 ] || [ ! -s "$ROOT/web/splash/default.png" ]; then
		say "生成 iOS 启动图"
		"$PYTHON" scripts/gen-splash.py
	fi
fi

say "完成"
