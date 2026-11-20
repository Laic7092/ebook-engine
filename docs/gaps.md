# 已知缺口与兼容性审计

> 审计对象：`master` @ `acf8694`；G-19 / G-20 / G-21 基于 `2b1ba3e` 复核。
>
> 范围：当前仓库的静态代码审计，重点看**真实 EPUB 会碰到的底层语义**，
> 不是完整的 EPUB/CSS 一致性测试报告。严重度是工程判断，不是规范结论。
>
> 维护方式：修复一项就更新对应条目的「状态」和证据；新增缺口按 `G-xx` 编号追加。
> 编号按追加顺序，不一定按章节位置排列。

## 0. 结论

**确定性内核没有明显“底层洞”**：排版层不碰 I/O、不持全局状态、IR 禁止哈希容器，
native/wasm 有逐字节 parity，错误输入有 fuzz 和 limits 兜底。

真正的缺口集中在三层：

1. **格式/样式语义**：CSS 单位、选择器、`white-space`、表格/列表、背景/边框。
2. **资源层**：native 图片只解 PNG（JPEG 仍缺）；WOFF/WOFF2 已支持，可变字体/彩色字体/子集化仍缺。
3. **宿主/架构**：Web 打开书会泄漏字节，没有增量重排，`max_lines` 拦截偏晚；
   主线程仍包办解析/光栅/绘制/解码（G-21），页级像素在 Web 上另起一套实现（G-20），
   翻页动画与跟手滑动只存在于 JS 且基本不可用（G-19）。

另外有一批是**明确不做**的（竖排、flex/grid、PDF、DRM、scroll/tile），
见第 6 节，不应和缺陷混在一起。

---

## 1. 严重度

| 级别 | 含义 |
|---|---|
| **P0** | 直接导致正文错误、内容丢失、整类书排版不可读 |
| **P1** | 真实 EPUB 常见，造成明显排版偏差或兼容性下降 |
| **P2** | 正确性不一定错，但性能、内存或架构上会撞墙 |
| **X** | 明确不做，不是本轮缺陷 |

状态统一用：`未修复` / `部分` / `不修（X）`。

---

## 2. P0：会直接影响正文

### G-01 `white-space: pre` 的换行和 tab 丢失

- **状态**：已修复
- **修复记录**：`WhiteSpace` 追加 `PreLine` / `BreakSpaces` 变体；`flatten()` 按模式
  分别处理空格折叠、硬换行与 tab 展开（`tab-size: 8`）；`Pre` 不软换行，
  `PreWrap` / `BreakSpaces` / `PreLine` 保留硬换行。覆盖测试：
  `crates/ebook-layout/tests/whitespace.rs`；`fixtures/typography.epub` 的 `<pre>`
  使 `tests/golden/pixels.txt` 的 page 1 基线更新。
  剩余简化：`BreakSpaces` 与 `PreWrap` 共享实现，没有单独的空格悬挂宽度。
- **现象**：`<pre>`、`white-space: pre`、`pre-wrap` 里的 `\n` / `\r` / `\t`
  在 `flow.rs` 的 `flatten()` 里被无条件替换成空格。
- **影响**：代码块、诗歌、ASCII 图、保留格式的表格全部塌成一行。
  这是内容级错误，不只是样式偏差。
- **证据**：
  - `crates/ebook-layout/src/flow.rs`：`flatten()` 把换行/制表符映射为空格。
  - `crates/ebook-format/src/css.rs`：`white-space` 只映射 `pre` / `pre-wrap` /
    `nowrap`，`pre-line` 和 `break-spaces` 落到 `Normal`。
- **修复**：在 `flatten()` 保留换行信息（例如生成 `Break` 或显式 `\n` 字符），
  按 `WhiteSpace` 的四种模式分别处理：是否折叠空格、是否保留换行、是否允许软换行。
- **验收**：至少补一个 `pre`、一个 `pre-line`、一个带 tab 的 golden。

### G-02 CSS 长度单位和颜色函数覆盖不足

- **状态**：部分
- **已修复**：`parse_len` 支持 `pt` / `pc` / `in` / `cm` / `mm`（换算为 px）；
  `currentColor` 在 `color` / `background-color` 上真正求值；颜色补齐
  `#rgba` / `#rrggbbaa`、`rgb()` / `rgba()` 的百分比分量与 alpha。
  测试在 `crates/ebook-format/src/css.rs` 的 `mod tests`。
- **仍未修复**：`hsl()`、`@import`、`text-transform: capitalize`、`vw` / `vh` 等
  视口单位（IR 没有视口上下文）。
- **现象**：`parse_len` 只认 `px` / `em` / `rem` / `%`，不认 `pt` / `mm` /
  `cm` / `in` / `vw` / `vh` / `ex` / `ch`。
- **影响**：EPUB 里非常常见的 `font-size: 12pt`、`margin: 1pc` 会被当成不支持
  的声明直接丢弃，整本书回落到默认字号/间距。
- **证据**：
  - `crates/ebook-format/src/css.rs`：`parse_len`、`font_size`。
  - `value_supported` 对不认识的单位返回 `false`，最终只留一条 unsupported 记录。
- **相关**：
  - `color: currentColor` 在 `value_supported` 里被接受，但 `parse_color` 不处理。
  - 颜色只覆盖少量命名色和 `#rgb` / `#rrggbb`，没有 `rgb()/%`、`hsl()`、
    8 位 hex、alpha。
  - `@import` 在 `parse_level` 的 `_ => {}` 分支被直接跳过。
  - `text-transform: capitalize` 被接受，但 `TextTransform` 没有对应变体，实际不生效。
- **修复**：先补 `pt/mm/cm/in` 和 `currentColor`，这两项收益最大；再补颜色函数与
  `@import`（后者需要格式层解析 CSS 之间的依赖）。
- **验收**：`font-size: 12pt` 的 golden 必须和 `16px` 区分开。

### G-03 选择器只支持 tag / id / class / 后代

- **状态**：部分
- **已修复**：子代 `>`、相邻兄弟 `+`、`:first-child` / `:last-child`（伪类按
  class 级计入 specificity，文本节点不参与兄弟计数）。测试在
  `crates/ebook-format/src/css.rs` 的 `mod tests`。
- **仍未修复**：`~`、属性选择器、`:not()` / 其它伪类、伪元素；这些仍整条规则丢弃。
- **现象**：`parse_selector` 遇到 `>`、`+`、`~`、`:`、`[`、`]`、`(`、`)`
  直接返回 `None`；`matches` 只做祖先链匹配。
- **影响**：以下常见写法整条规则被丢掉：
  - `div > p`
  - `h1 + p`
  - `p:first-child`
  - `[epub\:type="chapter"]`
  - `:not(...)`
  很多书的段首缩进、标题间距、图片选择依赖这些选择器。
- **证据**：`crates/ebook-format/src/css.rs` 的 `parse_selector` / `matches`。
- **修复**：至少补子代 `>`、相邻兄弟 `+`、`:first-child` / `:last-child`；
  属性选择器按 EPUB 常见程度排在后面。
- **验收**：每种新增组合都要有 CSS 级联测试，不只看单条声明。

### G-04 背景、边框只解析不输出

- **状态**：未修复
- **现象**：`ComputedStyle.background` / `border` 会从 CSS 读进来，
  但 `ebook-layout` 从不生成 `Op::Rect` / `Op::Border`。
- **影响**：背景色、边框、分隔线不显示；`border-radius` 仍被跳过。
  `text-decoration: underline` 已支持（`TextStyle.underline` → `GlyphRun.underline`），
  `line-through` / `overline` 还没有字段。IR、SVG、RGBA 光栅、Canvas 解码器
  其实都已经支持 `Rect` / `Border`，缺的是布局层的生产者。
- **证据**：
  - `crates/ebook-ir/src/layout.rs`：`Op::Rect`、`Op::Border`。
  - `crates/ebook-layout/src/lib.rs`：组装阶段只 push `GlyphRun`、`Image`、
    `Unresolved`、`Link`、`Clip`。
  - `crates/ebook-format/src/css.rs`：`background-color` / `background`、
    `border*` 已解析。
- **修复**：在块级盒完成后发射背景/边框 op；`line-through` / `overline`
  需要再扩展 IR。
- **验收**：golden 里出现 `rect`/`border` op；pixel 基线覆盖有边框的块。

### G-05 表格和列表没有真正的排版语义

- **状态**：未修复
- **现象**：
  - `Tag::Table` / `Tr` / `Td` 在 UA 样式里全部被设为 `Display::Block`。
  - `ListStyle` 在 IR 和 CSS 里存在，但布局层完全不读 `style.list`，
    不生成 `•` / `1.` 项目符号。
- **影响**：表格变成一列普通块；有序/无序列表只有缩进没有 marker。
  非虚构类 EPUB 的目录、参数表、步骤列表都会明显变形。
- **证据**：
  - `crates/ebook-format/src/css.rs`：`ua_apply` 的 `Tag::Table | Tag::Tr | Tag::Td`。
  - `crates/ebook-ir/src/doc.rs`：`ListStyle`；布局层无引用。
- **修复**：表格至少先支持单层 `tr/td` 的列宽与行高；列表先支持 `decimal` /
  `disc` 两类 marker，`start` 按 `ListStyle.start` 递增。

### G-06 native 图片解码只有 PNG

- **状态**：部分
- **已修复**：`image_size` / `image_mime` 支持 SVG（根标签 `width` / `height` 的
  绝对单位、`viewBox` 回退与宽高比补齐）；测试在
  `crates/ebook-format/tests/image.rs`。仍不做 SVG 光栅化。
- **仍未修复**：JPEG / GIF / WebP 的像素解码；native 光栅后端仍只解 PNG。
- **现象**：`image_size` 能从 PNG/JPEG/GIF/WebP 头读尺寸，但
  `decode_png` 只解 8 位、非隔行 PNG；native 光栅后端只调用 `decode_png`。
- **影响**：JPEG（最常见的 EPUB 图片）、GIF、WebP、SVG、AVIF 在 native
  后端只能画占位。Web 端靠浏览器 `createImageBitmap` 绕过了一部分，
  但 native CLI / 光栅后端没有。
- **证据**：
  - `crates/ebook-format/src/image.rs`：`decode_png` 的范围注释。
  - `crates/ebook-cli/src/main.rs`：`CliResources::image` 只调 `decode_png`。
  - `crates/ebook-font/src/lib.rs`：`impl Resources for FontBook` 的
    `image()` 永远返回 `None`。
- **修复**：优先接 JPEG（封面和插图绝大多数是 JPEG）；SVG 至少先解析
  `width` / `height` / `viewBox` 拿固有尺寸。第三方解码器要评估
  体积/许可/native-wasm 双端一致性。
- **验收**：JPEG 在 `--ppm` 和 `--svg` 两条路径都能出图。

### G-07 EPUB 容器层：加密、编码、ZIP64

- **状态**：部分
- **已修复**：`ZipArchive` 支持 ZIP64 EOCD 与中央目录扩展字段；单个非 UTF-8
  文件名只跳过该条目，不再让整包失败；`read_text` 按浏览器顺序解码：BOM →
  无 BOM UTF-16 → 合法 UTF-8 → XML/`<meta charset>` 声明 → `chardetng`
  （Firefox 的编码探测器，仅 native）。native 构建用 `encoding_rs` 覆盖
  GBK / Big5 / Shift_JIS / EUC-KR / windows-125x 等全部 WHATWG 编码；
  wasm 为体积预算只内置 BOM/UTF-16/UTF-8/latin-1/windows-1252，其余旧编码
  仍需宿主先用 `TextDecoder` 解码。container 缺失/坏路径会回退扫描 `.opf`；
  XML 非法控制字符替换为 U+FFFD 后继续解析。测试见 `zip.rs` 单测与
  `tests/robust.rs`（含 GBK 章节）。
- **仍未修复**：
  - 不处理 `META-INF/encryption.xml`：DRM、IDPF/Adobe 字体混淆的字体
    会以原始加密字节交给字体解析（`read-fonts`），解析失败。
- **影响**：商业 EPUB 的混淆字体仍会丢；无声明且探测器也判不准的极短文本
  仍可能乱码（但不会报错或白屏）。
- **证据**：
  - `crates/ebook-format/src/zip.rs`：EOCD / central directory / 文本解码。
  - `crates/ebook-format/src/epub.rs`：没有 encryption.xml 分支。
  - `docs/limits.md`：文本编码与 ZIP64 行。
- **修复**：继续处理 `encryption.xml` 的 `font/obfuscation`（IDPF 算法不复杂，
  且不需要 DRM 解密）。

### G-08 字体：WOFF、可变字体、彩色字体、子集化

- **状态**：部分
- **已修复**：`@font-face` 与本地字体在 `FontBook` 加载入口用 `wuff` 解包
  WOFF/WOFF2 为 sfnt，再交给 harfrust/read-fonts；解包失败通过
  `FontBook::diagnostics` / `eb_font_error` 暴露。
- **现象**：
  - WOFF/WOFF2 已解包，但 `encryption.xml` 的字体混淆仍未处理（见 G-07）。
  - 只读字体默认实例，没有 `font-variation-settings` / variation axis。
  - 不支持 COLR/CPAL、CBDT/CBLC、sbix、SVG-in-OpenType 等彩色字体。
  - 没有字体子集化：CJK 字体整份留在内存，Web 还会下载整份 CJK TTF。
  - `serif` / `sans-serif` 只是字符串，没有 generic family 到具体字体的映射。
- **影响**：WOFF 内嵌字体已可用；可变字体字重/字宽不生效；
  emoji/图标字体不显示；CJK 书内存和加载时间偏高。
- **证据**：
  - `crates/ebook-format/src/epub.rs`：`collect_fonts` 直接使用原始字节。
  - `crates/ebook-font/src/lib.rs`：`Metrics::parse` / `shape` / `rasterize`。
  - `docs/limits.md`：`MAX_FONT_BYTES = 32MB`，没有子集化。
- **修复**：WOFF 解包和子集化是两条独立线；前者影响兼容性，后者影响内存/加载。
  可变字体需要把 `font-weight`/`font-style` 映射到 axis，至少覆盖 `wght`/`wdth`/`ital`。

---

## 3. P1：真实书兼容性与排版细节

### G-09 RTL、`unicode-bidi` 与逐元素语言

- **状态**：部分
- **现象**：
  - `text-align: start/end` 在 `flow.rs` 里直接映射成左/右，没有按
    `direction: rtl` 翻转。
  - CSS `unicode-bidi`（`bidi-override` / `isolate`）不解析；`<bdi>` / `<bdo>`
    只是普通 `Span`。
  - `xhtml.rs` 只在 `doc.meta.lang` 为空时取第一个 `lang`，后续元素的
    `lang` 不影响 `locl`、断字或字体选择。
- **影响**：阿拉伯语/希伯来语书的对齐和嵌套方向可能错；多语言书的
  逐段语言特性不生效。
- **证据**：`crates/ebook-layout/src/flow.rs` 的 `emit_line` 对齐逻辑；
  `crates/ebook-format/src/css.rs` 的属性表；`crates/ebook-format/src/xhtml.rs`
  的 `lang` 处理。
- **修复**：先修 `start/end` 的 direction 感知；`unicode-bidi` 和逐元素 `lang`
  需要把语言/方向放进 `ComputedStyle` 或 `TextStyle`。

### G-10 EPUB 版式特性：fixed-layout、SVG/MathML、`@page`

- **状态**：未修复 / 部分
- **现象**：
  - 不处理 `rendition` / fixed-layout / `page-progression-direction`。
  - 内联 SVG / MathML 落到 `Tag::Other`，没有矢量布局或数学排版。
  - 不支持 `@page`、页眉页脚、running header。
- **影响**：绘本、漫画、教材、技术书等依赖固定版式或 SVG 的书会明显变形。
- **证据**：`crates/ebook-format/src/epub.rs` 的 package 解析；
  `crates/ebook-format/src/css.rs` 的 at-rule 分支；`Tag::Other` 的 UA 默认块级。
- **修复**：fixed-layout 需要独立的页面模型，工作量大于普通流式排版，
  应单独立项；SVG 可以先只拿尺寸，再考虑路径渲染。

### G-11 没有真正的增量重排

- **状态**：未修复
- **现象**：`DirtyBlock` 只作为诊断输出；`ebook-cache` 缓存的是整份
  `LayoutResult` 的 LRU，没有“只重排某块/某段”的入口。
- **影响**：改字号、改窗口、图片迟到时仍然整章重排；长章节/低端设备会卡。
- **证据**：
  - `crates/ebook-layout/src/flow.rs`：`dirty.push(...)`。
  - `crates/ebook-cache/src/lib.rs`：`LayoutCache::layout` 整份命中/未命中。
  - `README.md` 说“增量重排入口”，但当前只有标记，没有消费者。
- **修复**：先在 `flow` 里把“计划/断行/回填”拆成可独立重算的块，
  再让 cache 按块失效。不要先做“局部重排”再补不变量。

### G-12 `max_lines` 在 flow 之后才拦截

- **状态**：未修复
- **现象**：`Layout::run` 先完整执行 `flow::flow`，之后在组装阶段才检查
  `max_lines`。`max_pages` 也只影响分页计划，不会减少前面的断行工作。
- **影响**：恶意或异常大的文档可以先用排版把 CPU/内存吃满，再返回截断结果。
- **证据**：`crates/ebook-layout/src/lib.rs` 的 `run`；`README.md` 的
  “现状与限制”已承认。
- **修复**：把行数/字形数预算传进 `flow`，在断行循环内部提前停止，
  并保证截断后的行/字形区间仍然连续。

### G-13 Web 打开书会泄漏整本书字节

- **状态**：未修复
- **现象**：`open_book` 用 `Box::leak(bytes.to_vec())` 把书变成 `'static`；
  换书时旧的泄漏字节不会释放。字体、资产、IR 也有多处 `to_vec` / `clone`。
- **影响**：长时间使用、频繁换书时 wasm 线性内存只涨不降，最终 OOM。
- **证据**：`crates/ebook-web/src/lib.rs` 的 `open_book`、`register_font`、
  `asset_bytes`、`layout_cached`。
- **修复**：把 `EpubReader` 改成拥有 `Vec<u8>` 的版本，或者用
  `wasm-bindgen` 拷贝 + 显式 `drop`；至少提供 `close_book()` 释放旧书。

### G-14 SVG 后端不是精确参考渲染

- **状态**：部分
- **现象**：`ebook-render::SvgSurface::glyph_run` 只写
  `<text font-family="sans-serif">` + 原始字符串，忽略 `Glyph` 数组、
  advance、kerning、bidi 重排；`image()` 只画占位。
  `ebook-cli` 的 `CliSvg` 能内联真实图片，但文字仍是 `<text>`。
- **影响**：SVG 输出不能用来验证排版几何；bidi/两端对齐/字距在 SVG 里会失真。
- **证据**：`crates/ebook-render/src/svg.rs`、`crates/ebook-cli/src/svg.rs`。
- **修复**：要么把 glyph 转成 `<path>`（精确但文件大），要么在文档里明确
  它只是“可读预览”，不要当作 golden 的视觉参照。

### G-15 字形/Face 没有缓存

- **状态**：部分
- **已修复**：`FontBook` 内新增有界缓存——按 `(FontId, gid, px, dx, dy)` 缓存
  光栅结果（含 `None`），按 `(FontId, size, text, lang)` 缓存整形结果；
  绑定 `FontBook` 生命周期，`add*` / `set_language` 时失效。测试在
  `crates/ebook-font/tests/cache.rs`。
- **仍未修复**：`face()` 在缓存未命中时仍逐次 `read_fonts::FontRef::from_index`
  （整形用的 harfrust `Font` 已在 `Metrics::parse` 建好并随 `FontBook` 复用；
  度量/轮廓共用的 `read-fonts` 视图仍按需重建，二者共享同一份
  `Arc<Vec<u8>>` 字体字节）。
- **现象**：`rasterize` / `advance` 每次都重建 `FontRef`；native 光栅
  每次绘制都重新栅格化字形。
- **影响**：翻页、缩放、重复排版时 CPU 开销高；大字体尤其明显。
- **证据**：`crates/ebook-font/src/lib.rs` 的 `face()`、`shape()`、
  `rasterize_at()`；`crates/ebook-raster/src/lib.rs` 的 `draw_glyph`。
- **修复**：给 `FontBook` 内的每个 `Metrics` 缓存 parsed face（或
  `RawFaceTables`），再加一个按 `(font, gid, size, subpixel)` 键的 glyph cache。
  缓存要绑定 `FontBook` 生命周期，保持无全局状态。

### G-19 翻页动画与跟手滑动只在宿主，跟手路径基本不可用

- **状态**：未修复
- **现象**：
  - 翻页动画完全在 `web/reader.js`（`playFade` / `playSlide` / `beginSwipe` /
    `applySwipeDrag` / `endSwipe`），用 CSS `transform` + `setTimeout` 状态机驱动；
    引擎没有页级位图出口，只有 IR 与单字形位图。
  - `beginSwipe` 要求目标页位图已在 `pageSnapshots` 里，否则 `return false`；
    `web/selection.js` 收到 false 后置 `swipeBlocked`，此后 move 不产生位移，只在
    抬手时按 `SWIPE_MIN = 44px` 翻一次。预取是 `requestIdleCallback` + 120/90ms、
    一次一页（`prefetchNeighbor`），起手前几乎命中不了；切章更不可能跟手。
  - 鼠标 / 触控板没有跟手路径：`selection.js` 的 `pointerDown` 对非 `touch` 立即
    `begin()` 进入选区，swipe 分支不可达；也没有 `wheel` 监听。
  - 手势仲裁：`LONG_PRESS_MS = 320` 的定时器优先于横滑判定，慢速横滑先变成选区；
    `SWIPE_START = 12px` 没有从 dx 里扣除，起手会跳 12px。
  - 动画收尾用 `setTimeout` 而不是 `transitionend`，且 `animating` 期间 `turnPage`
    直接 return；后台节流或掉帧时状态机挂住，overlay 不清理、输入继续被吞。
  - overlay 是 `document.createElement("canvas")`，没有 `#page` 的主题 `filter`
    （`web/index.html` 的 sepia/dark），滑动过程颜色会跳。
  - 内存：`pageSnapshots` 留 4 张全 DPR 全页 canvas（390×844@3x ≈ 11.9MB/张），
    滑动时再叠 2 张全页 overlay，加上 3 张 2048² 字形 atlas。
  - `applySwipeDrag` 没有 rAF 合并；`tests/app.mjs` 没有任何 pointer 序列测试。
- **影响**：真机上「拖不动，松手才翻」；快速连续翻页被吞；主题下闪色；移动端
  接近 canvas 内存上限时首帧卡顿甚至白页。
- **证据**：`web/reader.js` 的 `snapshotPage` / `placeOverlay` / `beginAnimation` /
  `beginSwipe` / `endSwipe` / `schedulePrefetch` / `prefetchNeighbor`；
  `web/selection.js` 的 `pointerDown` / `pointerMove` / `pointerUp`；
  `web/index.html` 的 `#reader-stage` / `#page` 样式。
- **修复**：分两步。① 宿主侧：起手永不拒绝（目标页缺失时只滑当前页 + 边缘阴影），
  `pointerdown` 立即对邻页发起预取，收尾改 `transitionend` + timer 兜底，`pointermove`
  用 rAF 合并并扣掉起始阈值，overlay 继承主题（或由引擎烘焙），快照改 `ImageBitmap`
  并降 DPR，缓存从 4 条 LRU 改成「当前页 ±1 显式槽位」。② 引擎补页级位图（G-20），
  让动画只搬两张位图。
- **验收**：滑到一半时旧页 transform ≈ dx、新页 ≈ dx + 页宽；目标页未缓存时同样
  有位移；sepia/dark 无闪色；`xtask app` 的 pointer 序列测试覆盖以上三条。

---

## 4. P2：工程与鲁棒性

### G-16 宿主资源与内存边界

- **状态**：部分
- **现象**：Web 端没有显式内存预算；CJK 字体、整本 zip、所有章节 IR
  都可能同时留在内存。`MAX_FONT_BYTES = 32MB` 只拦单字体。
- **影响**：移动端 wasm 容易触顶；字体 + 图片 + IR 的峰值不可控。
- **修复**：宿主层加总预算（字体/图片/IR），超限时降级（例如只保留当前章 IR、
  图片按需解码、字体子集化）。

### G-17 复杂脚本的 measure/draw 一致性

- **状态**：部分
- **现象**：Web Canvas 在 `glyphs.length !== chars.length` 时退回整串
  `fillText`，实际字形由浏览器重新 shaping；native 则按 harfrust 的
  `Glyph.dx` 摆放。
- **影响**：复杂脚本（阿拉伯、印度语系）或字体回退时，浏览器显示和 IR
  几何可能漂移；`xtask parity` 只比 IR，不比浏览器像素。
- **修复**：短期在文档里标注“Canvas 路径不保证与 IR 逐像素一致”；
  长期要么用 path 绘制，要么把浏览器 shaping 结果回写 IR（代价很大）。

### G-18 其他已知小项

- **状态**：未修复 / 部分
- `text-emphasis`（中文着重号）不支持，CJK 正文常见。
- `ruby` 只处理直接 `ruby/rt`，没有 `rp` / `ruby-position` / 复杂 ruby。
- `text-align: justify` 对 CJK 是把余量平均加到每个字间隙，不是标点/字间
  分开处理；能用，但和纸书排版有差距。
- `text-indent` 在 RTL 下不镜像。
- `opacity`、`transform`、`filter`、`box-shadow` 不支持。
- `page-break-*` 的老属性和 `break-*` 都支持一部分，但 `left/right/recto/verso`
  在分页器里没有真正的奇偶页处理。
- `linear="no"` 只记录在 `SpineItem`，阅读顺序没有真正跳过。

### G-20 页 → 像素有两套实现，引擎光栅后端在 Web 上没用

- **状态**：未修复（架构）
- **现象**：`ebook-raster` 已有 `Surface::render_page` + `Tone`（墨色/伽马）+
  裁剪 + 图片，CLI 走 `--ppm`；Web 端另起一套 `drawOp` / `drawGlyphRun` /
  `glyphSprite` + 手写 atlas，字形按设备像素 floor，复杂脚本整串退回 `fillText`。
  `ebook-web` 只导出 IR 与单字形位图，没有「整页位图」入口。
- **影响**：主题、下划线、边框、图片 fit、字形相位要在两边各维护一遍；G-17 的
  几何漂移结构上不可避免；动画拿不到一次调用出一张位图的接口，只能靠 JS 预取整页绘制。
- **证据**：`crates/ebook-render/src/paint.rs`、`crates/ebook-raster/src/lib.rs` 的
  `render_page` / `set_tone` / `to_ppm`、`web/reader.js` 的 `drawOp` / `drawGlyphRun` /
  `glyphSprite` / `renderPageCanvas`。
- **修复**：在 `ebook-web` 增加 `eb_page_bitmap(spine, page, dpr, tone) -> RGBA` 与
  `eb_page_box(spine, page)`，在 `layout-worker` 内调用（顺手把 ±1 页位图一起产出并
  `transfer`）；Web 侧删掉 JS 绘制分支，只做 `drawImage` / `transform`。先量基准：
  1170×2532 单页 CPU 光栅若超过 8ms，则动画/预取走引擎位图（可降 DPR），可见页
  保留现有 Canvas 路径。
- **验收**：同一页引擎位图与 Canvas 路径的像素差异有上限；动画快照与静态页同源
  （不依赖 CSS `filter`）；native/wasm 共用同一个 `render_page`。

### G-21 主线程仍承担解析、光栅、绘制、解码、查询

- **状态**：未修复
- **现象**：
  - `layout-worker` 只接 `layout`；开书在主线程 `eb_open` 一遍，worker 又解析一遍，
    且 `workerBook` 是结构化克隆整本字节（没有 `transfer`）。
  - 每章主线程做两次 IR 解码：`eb_use_layout` → `adopt_layout` 里 `codec::decode`
    并 `chapter_with_anchors` **重新解析本章 XHTML/CSS**；随后 `decodeLayout` 又把
    同一份字节解成 JS 对象图（`ops` / `glyphs` / `lines` 每项一个对象）。
  - 整页绘制、首见字形的 `eb_glyph_bitmap` 光栅化与 atlas 写入、页快照拷贝、
    idle 里的 `renderPageCanvas` 预取，全在主线程。
  - 字体解析三份：`eb_font`（FontBook）、`new FontFace`（浏览器）、worker 的
    `eb_font`；内嵌字体还要 `eb_font_face_bytes` 再拷一份给 JS。
  - `eb_hit` / `eb_selection` / `eb_link_at` / `eb_progress` 走 JSON 信封 + JS 二次
    解码，选区拖动每个 `pointermove` 一次，没有 rAF 合并。
  - worker 8s 超时后回退主线程排版（`requestLayout` 的 `eb_layout_style` 分支）。
  - 导入时 `text.js` 的 `fingerprint` 在 JS 里逐字节循环；IndexedDB 整本
    `putBook` / `getBookData`。
- **影响**：切章有可见停顿与 GC 尖峰；预取绘制与手势在同一线程竞争；峰值内存与
  G-13 / G-16 叠加，移动端更容易触顶。
- **证据**：`crates/ebook-web/src/lib.rs` 的 `adopt_layout` / `layout_cached_style`；
  `web/layout-worker.js` 的头注释与 `onmessage`；`web/reader.js` 的 `open` /
  `layoutChapter` / `render` / `renderPageCanvas` / `glyphSprite` / `requestLayout`；
  `web/decode.js`；`web/text.js` 的 `fingerprint`。
- **修复**：① 交互状态常驻 worker（主线程只发 `hit` / `selection` RPC），删掉
  `adopt_layout` 的章节重解析；② 开书只解析一次（worker 权威 + `transfer` + 元信息
  回传）；③ 页级位图（G-20）接管绘制与光栅；④ `decodeLayout` 只解当前页 ±1 的 op
  切片；⑤ 查询改 postcard / typed array 并做 rAF 节流；⑥ `fingerprint` 挪 worker。
- **验收**：切章期间主线程无 >50ms 长任务；开书解析次数 1、字体解析次数 ≤2；
  `decodeLayout` 只为可见页生成对象图。

### G-22 脏 XHTML 的容错边界

- **状态**：已建立，仍在扩展
- **已修复**：格式层先用 `roxmltree` 严格解析；失败后走
  `crates/ebook-format/src/html_repair.rs` 的 HTML 容错层（tokenizer + 树构造
  恢复），支持裸属性值、裸 `&`、void 元素、错配/多余结束标签、可选结束标签、
  重复属性、多根/纯片段、大小写不敏感等。修复后仍失败则退化成纯文本，保证
  「每一章至少能出字」。OPF / container / nav / NCX 也走同一修复入口，避免
  一本坏 OPF 导致整本打不开。诊断码：`RepairedXhtml` / `TextFallback`。
- **仍未覆盖**：完整的 HTML5 树构造算法（例如表格 foster parenting、
  `<select>` 插入模式、模板内容）、`<frameset>`、表单控件语义。遇到这些
  结构时只保证「不白屏」，不保证与浏览器逐节点一致。
- **证据**：`crates/ebook-format/src/html_repair.rs`；`tests/robust.rs`；
  `crates/ebook-format/examples/audit.rs`（对目录逐章审计）。
- **验收**：坏章节只影响自己；公开语料 + 变异 fuzz 下无 panic、无整本失败。

---

## 5. 明确不做（不是缺陷）

| 项 | 原因 |
|---|---|
| 竖排 / `WritingMode::Vertical*` | 字段保留，纵向布局是独立工程 |
| flex / grid / 绝对定位 / float | 不追求浏览器级 CSS 兼容 |
| PDF 输出 | 当前只有 SVG / RGBA / Canvas |
| DRM 解密 | 只考虑 `encryption.xml` 的字体混淆，不做内容解密 |
| scroll / tile 模式 | 分页是唯一模型，`Page.origin` 依赖它 |
| 完整的 UAX #14 上下文规则 | 只实现成对可判定部分，已写在 `docs/design.md` |
| 非英文断字词典 | `English` 已是 `hyphen-en` 可选 feature；其他语言由宿主注入 |

---

## 6. 建议修复顺序

1. **G-19**：先让跟手滑动可用（起手不拒绝、预取、`transitionend`、rAF、主题
   一致性、`ImageBitmap` + 降 DPR）。这是当前唯一「用户已经能感觉到坏掉」的项，
   成本低，先做。
2. **G-01 / G-02 / G-03**：`white-space`、`pt` 单位、常用选择器。
   成本低，直接改善真实书的正文和字号。
3. **G-04 / G-05**：背景/边框 op、列表 marker、单层表格。
   IR 已经有 `Rect`/`Border`，先让布局层发射。
4. **G-06 / G-08**：JPEG 解码、WOFF 解包。
   这两项决定“图片和字体能不能显示”。
5. **G-20 / G-21**：页级位图 + 交互状态回 worker + 开书只解析一次。
   是宿主架构改造，最好在 G-20 的基准测出来之后再动手；做完能顺带消掉
   G-13 / G-16 的峰值内存。
6. **G-12 / G-11**：`max_lines` 前移、真正的增量重排。
   属于内核改动，放在语义修复之后。
7. **G-09 / G-10 / G-14**：RTL、fixed-layout、精确 SVG。
   工作量大，单独排期。

---

## 7. 已确认的稳固点

这些不是“没问题”，而是当前审计没有发现底层缺陷：

- 全仓库无 `unsafe`。
- `DisplayList` 禁止 `HashMap` / `HashSet`，遍历顺序固定。
- 排版层不碰 I/O、不取时钟/随机、不持全局可变状态。
- native / wasm 有 `xtask parity` 逐字节比对。
- 坏 ZIP / 坏 XHTML / 坏字体有 fuzz 和越界保护。
- ZIP 路径穿越和单条目解压有上限。
- `Limits` 超限仍返回合法 IR，且行/字形/页切片保持连续。
- `hyphen-en` 已改成 opt-in，不再默认把 `en.pat` 打进 wasm。

---

## 8. 如何更新这份文档

1. 修复条目时，把「状态」改为 `已修复`，补上对应测试命令。
2. 发现新缺口时追加 `G-xx`，不要重排已有编号。
3. 如果某个条目因为设计变更变成“不做”，移到第 5 节并在原条目写 `X`。
4. 与 `docs/limits.md` 重叠的数值不要在这里重复抄一遍，链接过去即可。
