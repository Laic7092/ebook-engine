# ebook-engine

[![CI](https://github.com/Laic7092/ebook-engine/actions/workflows/ci.yml/badge.svg)](https://github.com/Laic7092/ebook-engine/actions/workflows/ci.yml)
<!-- 发布到 GitHub 时把 OWNER 换成实际组织/用户名。 -->

电子书渲染引擎。Web 优先，跨平台，可 Worker 并行。

**只有排版是核心。** 格式层与渲染层是适配器，可拆、可替换、可 mock。

接口契约见 [docs/api.md](docs/api.md)，设计论证与不变量见
[docs/design.md](docs/design.md)，限额与默认值见 [docs/limits.md](docs/limits.md)，
中文文本政策见 [docs/text-rules.md](docs/text-rules.md)，已知缺口与兼容性审计见
[docs/gaps.md](docs/gaps.md)。

```
format  ──>  StyledDoc  ──>  layout  ──>  LayoutResult  ──>  render
```

`StyledDoc` 是 AST，`LayoutResult` 是字节码，render 是后端。三段之间只传数据，没有回调、没有 trait object 反向调用。

---

## 三个契约

改这三个东西都是破坏性变更，其余随便改。

| 契约 | 是什么 | 为什么必须是这个形状 |
|---|---|---|
| `StyledDoc` | DOM + **已计算样式** + 原始锚点 | 级联需要 DOM，排版只需要计算后的值。级联放格式层，排版就永远碰不到选择器。 |
| `LayoutResult` | `Page[]` + `DisplayList` + `RunMap` | 分页结果必须可序列化、可 diff、可跨端逐字节比对。`RunMap` 是选区与 CFI 的唯一来源。 |
| `Renderer` | `&LayoutResult` → 像素 | 入参是不可变引用，从类型上禁止改坐标。 |

`DisplayList` 里的 op 按页连续。页是 op 数组的一个切片，不是过滤结果。

---

## 四条禁令

只约束排版层。违反任何一条，「核心只有排版」就不成立。

- 不碰 I/O
- 不持全局可变状态（字体库、单例、thread_local）
- 不取时钟、不取随机
- 不出现 `#[cfg(target_*)]`

代价是排版层要接受一切不确定性为入参：字体字节、图片固有尺寸、页面约束。回报是四件事同时成立：

1. 多线程退化成任务队列，不需要共享内存、不需要 COOP/COEP。
2. 缓存键就是输入的哈希，没有失效逻辑。
3. native 与 wasm 的输出可以逐字节对比，平台无关成了可测事实而不是口号。
4. 排版可以脱离真实格式测试，也可以脱离 canvas 测试。

---

## 两条铁律

**渲染层不改坐标。** 只读 IR、只画。一旦允许渲染层裁剪或调整，就有了第二套排版逻辑，golden test 立刻失去意义。

**不确定性必须显式。** 图片没加载、字形缺失、尺寸依赖内容，都要在 IR 里留下占位与标记，不能由渲染层顺手猜一个值。否则排版结果依赖加载时机，纯函数性被打破，分页会跳。

---

## 边界

| 层 | 只做 | 绝不做 |
|---|---|---|
| format | 解容器、解析、级联、产出锚点映射 | 测文本、算行、决定分页 |
| layout | 盒树、断行、分页、增量重排 | 读文件、加载字体、写字形缓存到磁盘 |
| render | 把 op 画成像素 | 改坐标、补全未知尺寸、重新断行 |

字体是排版入参，不是排版过程中去「找」的东西。不同平台 fallback 链不同，会导致同一本书分页不同，书签与跨端同步全部失效。这是正确性问题，不是性能问题。

度量只认字体入参：`TextMeasurer` 由外部注入，`FontBook` 用 `read-fonts` 给出真实的 cmap / advance / ascent / descent 与 glyf/CFF 轮廓，同一份字体字节同时也是 `Resources` 的光栅化来源；WOFF/WOFF2 在字体加载入口解包成 sfnt，后续整形、度量与光栅链路不变。`TextMeasurer::shape` 默认逐字 advance，`FontBook` 用 harfrust（HarfBuzz 的 Rust 移植）做真正的整形（kerning / ligature、按簇回写字形与字节偏移）；双向文本由 `unicode-bidi` 按 UAX #9 求段内级，排版层按簇做视觉重排。`ebook-layout` 只依赖 `ebook-ir` 与 `unicode-bidi`，仍然不碰 I/O。

断行按 UAX #14，属性表由 `data/unicode/LineBreak.txt` 生成（`data/gen_linebreak.py`），不引第三方 crate。只实现成对可判定的规则，需要上下文的规则明确不装。断字（hyphenation）是可插接口：宿主通过 `TextMeasurer::hyphenator()` 注入词典，内置英文 ushyphmax 模式的 Liang 算法（`hyphen-en` feature，默认关闭，不把 `en.pat` 链进 wasm）；没有词典时 `hyphens: auto` 与 `none` 同行为。

中文排版政策是三个独立开关（`TextRules`：成对标点挤压 / 行末悬挂 / 中西文自动间距），默认全关，出处与浏览器差异见 [docs/text-rules.md](docs/text-rules.md)。行内图片与 ruby 不再静默丢失：尺寸未知时产占位与诊断，ruby 的注音参与行高但不改基文字形。

两端对齐、孤行寡行、块边界与强制分页都在排版层结算。缺字不猜：产 `Op::Unresolved{FontMissing}` 与 `DirtyBlock`，由宿主决定换字体还是换字。

分页属于排版，因为断点会反过来影响孤行寡行与段落切分。`orphans` / `widows` / `break-before` / `break-after` 都在分页器里结算：先求最满的断点，再按约束往回退，退不动就退回最满断点。约束冲突时宁可难看，也不产生空页。

op 坐标是**排版流坐标**，不是页面内坐标。`Page.origin` 表示这一页在流里的起点，渲染时整体减去它。所以改分页只移动 `origin`，一个 op 的坐标都不用重写——这正是「渲染层不改坐标」能成立的前提。

---

## 能力清单

以下都是已合入主干的现状，不是 roadmap：

| 领域 | 现状 |
|---|---|
| 格式 | TXT / EPUB（ZIP stored+deflate、container/OPF/spine、XHTML、CSS 级联、命名实体、锚点）；TOC（EPUB3 nav / EPUB2 NCX）、链接、资源清单、字体声明、书目元数据、格式层诊断（`EpubIssueCode`） |
| 排版 | 盒树、margin collapsing、UAX #14 断行、两端对齐、孤行寡行、强制分页、增量重排入口；`layout` 是全函数 |
| 文本 | `TextMeasurer::shape`（默认逐字，`FontBook` 用 harfrust）、按簇双向重排（UAX #9）、字距/词距、可选断字词典、`TextRules` 三个中文政策开关 |
| 替换元素 | 块级/行内图片（固有尺寸未知时占位）、ruby 注音 |
| 契约 | IR 1.4：`Glyph.ch`、`Op::Link`、`GlyphRun.weight/style/underline`、`Issue`/`IssueCode`、`LayoutResult.truncated`、`LayoutStats` |
| 限额 | `Limits` 六个字段（节点/深度/文本字节/页数/行数/图片像素），超限返回合法截断 IR |
| 交互 | `Reader`（命中、锚点、进度、链接、夹取）、`Selection`/`SelectionRect`（分页矩形与原文）、wasm 侧 JSON 查询 |
| 缓存 | `ebook-cache`：输入指纹为键的有界 LRU，命中/未命中/淘汰统计 |
| 并发 | `Layout::layout_parallel`：一次调用内的 `thread::scope`，输出与串行逐字节相同；wasm 走串行 |
| 渲染 | RGBA8 整数光栅（tone/线性光、嵌套裁剪、双线性图片、PNG+JPEG 图片）、SVG 导出（CLI）、Canvas2D 阅读器（只搬位图） |

## 现状与限制

完整的缺口清单、证据与建议修复顺序见 [docs/gaps.md](docs/gaps.md)。这里只列当前最影响使用的几条：

- 只做横排；`WritingMode` 字段保留。
- 文字只能来自已注册的字体：缺字（例如没装中文字体）时该段直接空白，
  不再退回浏览器系统字体（以前那条回退路径已删）。
- `layout_parallel` 并行的是分页单元映射，不是段落断行本身；真正按段并行需要改 `flow` 的内部拆分，留给后续。
- 缓存用默认 `Limits`，没有 `layout_with` 变体；字体身份由调用方 `set_fonts` 显式登记。
- 整行只有行内图片时，`LineBox.src` 不指向真实原文（图片占位字符按 3 字节计）；选区会跳过这类行，修复需要动 `flow` 的源区间计算。
- `max_lines` 的拦截在组装侧：flow 仍会为超出部分做完断行。
- 中文政策的格式层 CSS 解析（`text-spacing-trim` 等）还没接；开关由宿主显式设置。

---

## 三个后端

同一份 IR，三种画法。彼此是替代关系，不是备份关系。

| 后端 | 位置 | 用途 |
|---|---|---|
| SVG | `ebook-render` | 参考实现，输出可以直接看，也方便和浏览器排同一份 XHTML 做差分 |
| 光栅 | `ebook-raster` | 整数混合，跨平台逐字节一致；`to_ppm()` 可以落到文件 |
| Canvas2D | `web/` | 浏览器阅读器：wasm 在 worker 里，整页位图来自 `eb_page_bitmap`，主线程只做 `putImageData` + 交互 |

`paint()` 是三个后端共用的 op 遍历，坐标只在这里被平移到页面内，后端自己不碰坐标。

---

## 验证

纯函数换来的可验证性，是这个架构真正的收益：

| 手段 | 命令 | 失守时说明什么 |
|---|---|---|
| 一键闸门 | `scripts/check.sh` | fmt、clippy `-D warnings`、test、`xtask all` 任一失败。 |
| golden IR | `xtask golden` | 排版结果变了。字节 diff 直接指出变化面。 |
| ABI 契约 | `xtask contract` | JS 解码器与 Rust 结构体字段不再一致。做法是把 golden IR 解出来再编回去，要求逐字节还原。 |
| 像素基线 | `xtask pixel` | 渲染结果变了。逐页 FNV 摘要，`pixel-bless` 显式更新。 |
| 跨目标比对 | `xtask parity` | native 与 wasm 不再逐字节相同。 |
| 浏览器应用 | `xtask app` | 读者 App 的翻页/打开/字体路径坏了。用 bun + DOM stub 跑端到端。 |
| 体积预算 | `xtask size` | core 或 core+epub 的 wasm gzip 超过 544 KiB。 |
| 性能回归 | `scripts/bench.sh` | layout / raster / IR codec 的中位耗时超过基线 1.15×。 |
| 排版 fuzz | `cargo test -p ebook-layout --test fuzz` | 1000 份随机文档（结构/文本/样式/页面尺寸）：页覆盖、行不越界、y 与 src 单调、重复排版幂等、编解码往返不变。 |
| 格式 fuzz | `cargo test -p ebook-format --test fuzz_like` | 真实样书 1000 次确定性变异（截断/翻字节/搬块/插入/抹零），`open` 与 `chapter` 都不 panic。 |
| 光栅 fuzz | `cargo test -p ebook-raster --test fuzz` | 1000 页随机 rect/glyph/image/嵌套 clip：不越界、两次渲染逐像素相同、alpha 不变。 |
| 限额 | `cargo test -p ebook-layout --test limits` | 每个 `Limits` 字段的 0=默认 / 恰好等于 / 超一，截断结果仍是合法 IR。 |
| 缓存 | `cargo test -p ebook-cache` | 指纹覆盖每个样式字段、字体与约束；LRU 命中/淘汰；并行结果与串行逐字节相同。 |

`DisplayList` 里禁止 `HashMap` / `HashSet`。遍历顺序决定字节流，序列化必须确定。

`tests/golden/*.ir` 与 `tests/golden/pixels.txt` 都已入库（`.gitignore` 用 `!tests/golden/*.ir` 放行），CI 在干净 checkout 上直接逐字节比较。`scripts/check.sh` 只在缺失时才生成快照；不要用 `bless` 覆盖回归。

IR 是 wasm 与 JS 之间唯一的接口，而 JS 侧的解码器是手写的，所以 `contract` 这项不是锦上添花：它把「改了 Rust 结构体，忘了改 JS」从静默错位变成一条带字节偏移的失败。

---

## 不做

PDF；DRM；flex / grid；浏览器级 CSS 兼容；把 HTML 直接塞进 DOM 当渲染方案；滚动 / 瓦片模式（scroll / tile）。

DOM 方案尤其不能做：排版由浏览器决定，跨端一致、分页控制、增量重排全部不受控，也拿不到可测试的中间表示。滚动 / 瓦片模式会把分页从唯一模型降级成可选视图，`Page.origin` 与页切片契约随之失效，所以本轮明确不做。

---

## 跑

干净 checkout 先跑一次 `scripts/setup.sh`：补 wasm32 / wasm-bindgen / woff，并调用
`scripts/generate.sh` 重建不进入 git 的字体、EPUB 夹具、图标与启动图。仓库只提交
代码和文本数据；`fixtures/*.epub`、`fixtures/font/*`、`web/latin.ttf`、
`web/cjk.woff2`、`web/icon-*.png`、`web/splash/` 都是生成物。GitHub 下载默认走
`GH_PROXY`（默认 `https://gh-proxy.com/`），5 MB 的中文字体也走同一个代理。

```
scripts/setup.sh                 # 一次性初始化（DejaVu、夹具、图标、PingFang）
scripts/setup.sh --skip-cjk      # 不下 5 MB 的中文字体
scripts/generate.sh              # 只重建生成资源；--force / --skip-cjk / --skip-web
scripts/generate.sh --skip-web   # CI：不生成图标/启动图和中文字体
scripts/check.sh                 # fmt + clippy(-D warnings) + test + xtask all
scripts/bench.sh                 # 对比 scripts/bench_baseline.txt，超 1.15× 退出 1
scripts/bench.sh --update        # 在目标机器上重新取基线
cargo test --workspace
cargo run -p xtask -- all
cargo run -p xtask -- golden | bless | contract | pixel | pixel-bless | size | parity | app | bench
cargo run -p ebook-cli -- layout fixtures/demo.epub --spine 0 --stats --svg page.svg
cargo run -p ebook-raster --example page
./web/build.sh && python3 -m http.server
./web/minify.sh                  # 生成可部署的 web/dist（JS/HTML 压缩；--wasm-opt / --precompress 见脚本头）
npx wrangler pages deploy web/dist --project-name ebook-reader   # 发布到 Cloudflare Pages
FEATURES=epub,hyphen-en ./web/build.sh   # 需要内置英文断字时
```

最后一条打开 `http://localhost:8000/web/` 就是完整阅读器：书架默认从 IndexedDB 读取，支持导入 EPUB 与 TXT（UTF-8 / UTF-16 / GB18030 / Big5 / Shift_JIS），点开进入阅读页，翻页、目录、字号/粗体/行距/字距/边距/对齐/主题设置与阅读进度都会持久化。界面按移动优先设计，阅读页使用真实可视区域排版并铺满，手机用底部面板、桌面用侧边抽屉，移动端工具栏会自动隐藏，轻点屏幕中间显示或收起；左右两侧轻点或横向滑动翻页。`?book=<url>&spine=<n>` 可直接打开指定书与章节，方便截图与回归。`fixtures/en.epub` 只用于测试，不进入部署包。部署包里的拉丁回退字体是 `web/latin.ttf`（DejaVu Sans），中日韩字体是 `web/cjk.woff2`（PingFang SC Regular，约 5 MB 单文件）；woff2 本身已是 Brotli 压缩，不再叠加 gzip。`web/manifest.webmanifest` + `web/sw.js` 让应用可安装，并把 wasm、JS 和第一次读到的字体放进 Cache Storage；导航请求优先读应用壳、后台回源，iOS 从主屏冷启动不必再等网络。`web/splash/*.png` 是对应机型的 `apple-touch-startup-image`（`python3 scripts/gen-splash.py` 重新生成），避免首帧前的白屏。字体首次约 5 MB，之后冷启动直接命中本地缓存。缺字时该段退回浏览器系统字体，不会静默消失。

`golden` 比字节；`size` 卡预算；`parity` 用同一份输入分别跑 native 与 wasm，逐字节比对 IR。`bench` 输出 `name<TAB>median_ns`，可直接 diff；基线文件里写明采集机器与 rustc 版本。CI 跑 fmt、clippy `-D warnings`、test、`xtask all` 与 `xtask size`，并在干净 checkout 上直接用入库的 golden 快照。
