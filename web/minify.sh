#!/usr/bin/env bash
# web/minify.sh — 把 web/ 压成可直接部署的静态包（默认 web/dist）。
#
# 设计约束：
#   * 不 bundle：JS 逐文件 minify，文件名与相对 import 保持不变，
#     pkg/ebook_web.js 里 new URL('...wasm', import.meta.url) 依旧有效。
#   * 不改源码：只读 web/，写 web/dist。
#   * 工具可降级：找不到 esbuild 时 JS/CSS 原样拷贝，脚本不失败。
#
# 用法：
#   ./web/minify.sh                   # → web/dist
#   ./web/minify.sh --out /tmp/site   # 换输出目录
#   ./web/minify.sh --wasm-opt        # 需要 binaryen 的 wasm-opt
#   ./web/minify.sh --precompress     # 额外生成 .gz（有 brotli 时也生成 .br）
#   ./web/minify.sh --serve 8000      # 构建后起本地静态服务
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT/web"
OUT="$SRC/dist"
WASM_OPT=0
PRECOMPRESS=0
SERVE=""

usage() {
  cat >&2 <<'EOF'
用法: web/minify.sh [选项]

  --out DIR          输出目录（默认 web/dist）
  --wasm-opt         用 wasm-opt -Oz 再压 wasm（需 binaryen）
  --precompress      生成 .gz / .br 预压缩副本
  --serve [PORT]     构建完后在输出目录起 python3 -m http.server
  -h, --help         显示本帮助
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --out)
      [ $# -ge 2 ] || { usage; exit 2; }
      OUT="$2"; shift 2 ;;
    --out=*) OUT="${1#*=}"; shift ;;
    --wasm-opt) WASM_OPT=1; shift ;;
    --precompress) PRECOMPRESS=1; shift ;;
    --serve)
      if [ $# -ge 2 ] && [[ "$2" != -* ]]; then SERVE="$2"; shift 2; else SERVE=8000; shift; fi ;;
    --serve=*) SERVE="${1#*=}"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "未知参数: $1" >&2; usage; exit 2 ;;
  esac
done

command -v rsync >/dev/null || { echo "需要 rsync" >&2; exit 1; }
command -v node >/dev/null || { echo "需要 node" >&2; exit 1; }

case "$OUT" in
  /*) ;;
  *) OUT="$ROOT/$OUT" ;;
esac
if [ "$OUT" = "$SRC" ] || [ "$OUT" = "$ROOT" ]; then
  echo "--out 不能是 web/ 或仓库根目录" >&2
  exit 1
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# esbuild：优先 PATH 里的，其次 npx 缓存/下载，都没有就跳过压缩。
ESBUILD=""
if command -v esbuild >/dev/null 2>&1; then
  ESBUILD="$(command -v esbuild)"
elif command -v npx >/dev/null 2>&1 && npx --yes esbuild --version >/dev/null 2>&1; then
  printf '#!/usr/bin/env bash\nexec npx --yes esbuild "$@"\n' > "$TMP/esbuild"
  chmod +x "$TMP/esbuild"
  ESBUILD="$TMP/esbuild"
fi
[ -n "$ESBUILD" ] || echo "警告: 找不到 esbuild，JS/CSS 将原样拷贝" >&2

echo "==> 复制静态资源 → $OUT"
# 输出目录若在 web/ 里（默认如此），复制时要排掉它自己。
EXCLUDES=(--exclude '/dist/' --exclude '/tests/' --exclude '/build.sh'
  --exclude '/minify.sh' --exclude '/node_modules/' --exclude '*.d.ts'
  --exclude '.DS_Store')
case "$OUT" in
  "$SRC"/*) EXCLUDES+=(--exclude "/${OUT#"$SRC"/}/");;
esac
rsync -a --delete "${EXCLUDES[@]}" "$SRC/" "$OUT/"

if [ -n "$ESBUILD" ]; then
  echo "==> minify JS"
  shopt -s nullglob
  js=("$SRC"/*.js "$SRC"/pkg/*.js)
  shopt -u nullglob
  if [ ${#js[@]} -gt 0 ]; then
    "$ESBUILD" "${js[@]}" \
      --outdir="$OUT" --minify --charset=utf8 --log-level=warning
  fi
fi

echo "==> minify HTML"
ESBUILD_BIN="$ESBUILD" node - "$OUT/index.html" <<'NODE'
const { readFileSync, writeFileSync } = require("node:fs");
const { spawnSync } = require("node:child_process");

const file = process.argv[2];
const esbuild = process.env.ESBUILD_BIN || "";
const before = readFileSync(file, "utf8");
let html = before;

// 内联 <style> 交给 esbuild 的 CSS loader；失败就保留原文。
if (esbuild) {
  html = html.replace(/(<style\b[^>]*>)([\s\S]*?)(<\/style>)/gi, (m, open, css, close) => {
    const r = spawnSync(
      esbuild,
      ["--loader=css", "--minify", "--charset=utf8", "--log-level=warning"],
      { input: css, encoding: "utf8" },
    );
    if (r.status !== 0) {
      console.error("警告: 内联 CSS 压缩失败，保留原文");
      return m;
    }
    return open + r.stdout.trim() + close;
  });
}

// 只处理 pre/textarea 之外的区域：去注释、标签之间的缩进压成一个空格。
// 保留一个空格是为了不改变行内元素之间的换行间距。
html = html
  .split(/(<(?:pre|textarea)\b[^>]*>[\s\S]*?<\/(?:pre|textarea)>)/i)
  .map((part, i) =>
    i % 2
      ? part
      : part.replace(/<!--(?!\[if)[\s\S]*?-->/g, "").replace(/>[ \t\r\n]+</g, "> <"),
  )
  .join("");

writeFileSync(file, html);
const after = Buffer.byteLength(html);
console.log(`  index.html ${Buffer.byteLength(before)} → ${after} bytes`);
NODE

if [ "$WASM_OPT" = 1 ]; then
  wasm="$OUT/pkg/ebook_web_bg.wasm"
  if [ ! -f "$wasm" ]; then
    echo "警告: 未找到 $wasm，跳过 wasm-opt" >&2
  elif ! command -v wasm-opt >/dev/null 2>&1; then
    echo "错误: --wasm-opt 需要 wasm-opt（Debian: apt install binaryen；macOS: brew install binaryen）" >&2
    exit 1
  else
    echo "==> wasm-opt -Oz"
    # 新版 binaryen 支持 --all-features；老版本会失败，退回普通 -Oz。
    if ! wasm-opt -Oz --all-features -o "$TMP/out.wasm" "$wasm" 2>"$TMP/wasm-opt.log"; then
      if ! wasm-opt -Oz -o "$TMP/out.wasm" "$wasm"; then
        cat "$TMP/wasm-opt.log" >&2
        exit 1
      fi
    fi
    mv "$TMP/out.wasm" "$wasm"
  fi
fi

if [ "$PRECOMPRESS" = 1 ]; then
  echo "==> 预压缩 .gz/.br"
  while IFS= read -r f; do
    gzip -9 -n -c "$f" > "$f.gz"
    if command -v brotli >/dev/null 2>&1; then
      brotli -q 11 -f -o "$f.br" "$f"
    fi
  done < <(find "$OUT" -type f \
    \( -name '*.html' -o -name '*.js' -o -name '*.mjs' -o -name '*.css' \
       -o -name '*.wasm' -o -name '*.json' -o -name '*.svg' -o -name '*.epub' -o -name '*.txt' \) \
    ! -name '*.gz' ! -name '*.br')
  command -v brotli >/dev/null 2>&1 || echo "提示: 未安装 brotli，只生成了 .gz" >&2
fi

echo "==> 体积"
bytes() { stat -c%s "$1"; }
for f in index.html app.js reader.js db.js text.js pkg/ebook_web.js pkg/ebook_web_bg.wasm latin.ttf cjk.ttf.gz cjk.ttf; do
  s="$SRC/$f"
  [ -f "$s" ] || continue
  o="$OUT/$f"
  if [ -f "$o" ]; then
    sb="$(bytes "$s")"; ob="$(bytes "$o")"
    printf '  %-26s %9s → %9s  (%d%%)\n' "$f" "$sb" "$ob" "$((ob * 100 / sb))"
  else
    printf '  %-26s %9s →   已剔除\n' "$f" "$(bytes "$s")"
  fi
done
printf '  %-26s %9s\n' "输出总计" "$(du -sb "$OUT" | cut -f1)"

if [ -n "$SERVE" ]; then
  echo "==> http://localhost:$SERVE/ （Ctrl-C 停止）"
  cd "$OUT"
  exec python3 -m http.server "$SERVE"
fi
