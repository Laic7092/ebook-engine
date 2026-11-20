# 已知缺口与兼容性审计

> 审计对象：`master` @ `685bd4f`。
>
> 范围：当前仓库的静态代码审计，重点看**真实 EPUB 会碰到的底层语义**，
> 不是完整的 EPUB/CSS 一致性测试报告。严重度是工程判断，不是规范结论。
>
> 维护方式：修复一项就更新对应条目的「状态」并精简证据；已修复的条目直接删除，
> 新增缺口按 `G-xx` 编号追加（编号不重排，历史条目删除后编号会出现空洞）。

## 0. 结论

**确定性内核没有明显“底层洞”**：排版层不碰 I/O、不持全局状态、IR 禁止哈希容器，
native/wasm 有逐字节 parity，错误输入有 fuzz 和 limits 兜底。

缺口集中在三层：

1. **格式/样式语义**：选择器、`white-space` 之外的 CSS 细节、表格/列表、背景/边框。
2. **资源层**：native 图片只解 PNG（JPEG 缺）；可变字体/彩色字体/子集化缺。
3. **宿主/架构**：Web 打开书泄漏字节，没有增量重排，`max_lines` 拦截偏晚；
   光栅已统一到引擎（页面像素只有一套实现），但主线程仍包办解析/光栅/查询
   （G-21），翻页动画与跟手滑动仍在 JS 且基本不可用（G-19）。

另有一批是**明确不做**的（竖排、flex/grid、PDF、DRM、scroll/tile），见第 5 节。

## 1. 严重度

| 级别 | 含义 |
|---|---|
| **P0** | 直接导致正文错误、内容丢失、整类书排版不可读 |
| **P1** | 真实 EPUB 常见，造成明显排版偏差或兼容性下降 |
| **P2** | 正确性不一定错，但性能、内存或架构上会撞墙 |

状态统一用：`未修复` / `部分`。

---

## 2. P0：会直接影响正文

### G-02 CSS 长度单位和颜色函数覆盖不足

- **状态**：部分
- **仍未修复**：`hsl()`、`@import`、`text-transform: capitalize`、
  `vw` / `vh` / `ex` / `ch` 等视口/相对单位（IR 没有视口上下文）。
  （`pt`/`pc`/`in`/`cm`/`mm`、`currentColor`、8 位 hex、`rgb()` 百分比与 alpha 已支持。）
- **影响**：`font-size: 3vw` 之类的声明被整条丢弃，回落到默认字号；
  `@import` 的书丢掉外部样式表；`capitalize` 静默不生效。
- **证据**：`crates/ebook-format/src/css.rs` 的 `parse_len` / `parse_color` /
  `parse_level`（`@import` 落到 `_ => {}`）；`crates/ebook-ir/src/doc.rs` 的
  `TextTransform` 没有 `Capitalize` 变体。
- **修复**：`hsl()` 与 `capitalize` 成本最低；`@import` 需要格式层解析 CSS 间依赖；
  视口单位要在 `ComputedStyle` 之外引入视口上下文，可放到最后。

### G-03 选择器只支持 tag / id / class / 后代 / `>` / `+` / `:first-child` / `:last-child`

- **状态**：部分
- **仍未修复**：`~`、属性选择器、`:not()` / 其它伪类、伪元素；这些仍整条规则丢弃。
- **影响**：`[epub\:type="chapter"]`、`:not(...)`、`p ~ p` 等常见写法丢样式，
  段首缩进、标题间距、图片选择常依赖它们。
- **证据**：`crates/ebook-format/src/css.rs` 的 `parse_selector`（遇到未支持的
  符号返回 `None`）/ `matches`。
- **验收**：每种新增组合都要有 CSS 级联测试，不只看单条声明。

### G-04 背景、边框只解析不输出

- **状态**：未修复
- **现象**：`ComputedStyle.background` / `border` 会从 CSS 读进来，但
  `ebook-layout` 从不生成 `Op::Rect` / `Op::Border`。
- **影响**：背景色、边框、分隔线不显示；`border-radius` 被跳过；
  `line-through` / `overline` 没有字段（`underline` 已支持，IR/光栅/Canvas 侧的
  `Rect`/`Border` 也已具备，缺的只是布局层的生产者）。
- **证据**：`crates/ebook-ir/src/layout.rs`（`Op::Rect` / `Op::Border`）；
  `crates/ebook-layout/src/lib.rs` 的组装阶段只 push `GlyphRun` / `Image` /
  `Unresolved` / `Link` / `Clip`。
- **修复**：在块级盒完成后发射背景/边框 op；`line-through` 需扩展 IR。
- **验收**：golden 里出现 `rect`/`border` op；pixel 基线覆盖有边框的块。

### G-05 表格和列表没有真正的排版语义

- **状态**：未修复
- **现象**：`Tag::Table` / `Tr` / `Td` 在 UA 样式里全是 `Display::Block`；
  `ListStyle` 在 IR 和 CSS 里存在，但布局层不读 `style.list`，不生成 `•` / `1.`。
- **影响**：表格变成一列普通块；有序/无序列表只有缩进没有 marker。非虚构类
  EPUB 的目录、参数表、步骤列表明显变形。
- **证据**：`crates/ebook-format/src/css.rs` 的 `ua_apply`；
  `crates/ebook-ir/src/doc.rs` 的 `ListStyle`（布局层无引用）。
- **修复**：表格先支持单层 `tr/td` 的列宽与行高；列表先支持 `decimal` / `disc`
  marker，`start` 按 `ListStyle.start` 递增。

### G-06 图片解码缺 GIF / WebP

- **状态**：部分
- **仍未修复**：GIF / WebP 的像素解码（绘制已经统一到引擎，不再有第二套实现）
- **影响**：GIF / WebP 的图在 native 与 Web 上都是占位框。JPEG 已由
  `zune-jpeg` 解码（`decode_image` 按魔数分发），PNG 走自带的 `miniz_oxide`。
- **证据**：`crates/ebook-format/src/image.rs` 的 `decode_image` / `decode_jpeg`。
- **修复**：按需接 GIF（首帧）/ WebP，同样要求 native-wasm 逐字节一致。
- **验收**：GIF/WebP 在 `--ppm` 与 `eb_page_bitmap` 两条路径都能出图。

### G-07 容器层：`encryption.xml` 字体混淆

- **状态**：部分
- **仍未修复**：不处理 `META-INF/encryption.xml`，DRM / IDPF/Adobe 混淆的字体
  会以原始加密字节交给 `read-fonts`，解析失败。内容解密仍是 X。
- **影响**：商业 EPUB 的混淆字体丢失。
- **证据**：`crates/ebook-format/src/epub.rs` 没有 encryption.xml 分支。
- **修复**：只做 `font/obfuscation`（IDPF + Adobe 两种算法都不难，不需要 DRM）。

### G-08 字体：可变字体、彩色字体、子集化

- **状态**：部分
- **仍未修复**：
  - 只读字体默认实例，没有 `font-variation-settings` / variation axis。
  - 不支持 COLR/CPAL、CBDT/CBLC、sbix、SVG-in-OpenType 等彩色字体。
  - 没有字体子集化：CJK 字体整份留在内存，Web 还下载整份 CJK TTF。
  - `serif` / `sans-serif` 只是字符串，没有 generic family → 具体字体的映射。
  - 混淆字体见 G-07。
- **影响**：可变字体字重/字宽不生效；emoji/图标字体不显示；CJK 书内存与加载时间偏高。
- **证据**：`crates/ebook-font/src/lib.rs` 的 `Metrics::parse` / `shape` / `rasterize`；
  `docs/limits.md` 的 `MAX_FONT_BYTES = 32MB`。
- **修复**：可变字体需要把 `font-weight`/`font-style` 映射到 axis（至少
  `wght`/`wdth`/`ital`）；子集化是独立一条线，影响内存和加载。

---

## 3. P1：真实书兼容性与排版细节

### G-09 RTL、`unicode-bidi` 与逐元素语言

- **状态**：部分
- **现象**：
  - `text-align: start/end` 直接映射成左/右，没有按 `direction: rtl` 翻转。
  - CSS `unicode-bidi`（`bidi-override` / `isolate`）不解析；`<bdi>` / `<bdo>` 只是普通 `Span`。
  - 只有 `doc.meta.lang` 取第一个 `lang`，后续元素的 `lang` 不影响 `locl`、断字或字体选择。
- **影响**：阿拉伯语/希伯来语书的对齐和嵌套方向可能错；多语言书的逐段语言特性不生效。
- **证据**：`crates/ebook-layout/src/flow.rs` 的 `emit_line`；`css.rs` 的属性表；
  `xhtml.rs` 的 `lang` 处理。
- **修复**：先修 `start/end` 的 direction 感知；其余需要把语言/方向放进
  `ComputedStyle` / `TextStyle`。

### G-10 fixed-layout、SVG/MathML、`@page`

- **状态**：未修复
- **现象**：不处理 `rendition` / fixed-layout / `page-progression-direction`；
  内联 SVG / MathML 落到 `Tag::Other`；不支持 `@page`、页眉页脚、running header。
- **影响**：绘本、漫画、教材、技术书明显变形。
- **证据**：`epub.rs` 的 package 解析；`css.rs` 的 at-rule 分支。
- **修复**：fixed-layout 需要独立页面模型，应单独立项；SVG 先只拿尺寸再考虑路径渲染。

### G-11 没有真正的增量重排

- **状态**：未修复
- **现象**：`DirtyBlock` 只作为诊断输出；`ebook-cache` 缓存整份 `LayoutResult` 的
  LRU，没有「只重排某块」的入口。
- **影响**：改字号、改窗口、图片迟到时整章重排；长章节/低端设备会卡。
- **证据**：`crates/ebook-layout/src/flow.rs` 的 `dirty.push(...)`；
  `crates/ebook-cache/src/lib.rs` 的 `LayoutCache::layout`。
- **修复**：先把 flow 的「计划/断行/回填」拆成可独立重算的块，再让 cache 按块失效。

### G-12 `max_lines` 在 flow 之后才拦截

- **状态**：未修复
- **现象**：`Layout::run` 先完整执行 `flow::flow`，之后才检查 `max_lines`；
  `max_pages` 也不减少前面的断行工作。
- **影响**：异常大的文档可以先用排版把 CPU/内存吃满，再返回截断结果。
- **证据**：`crates/ebook-layout/src/lib.rs` 的 `run`。
- **修复**：把行数/字形数预算传进 `flow`，在断行循环内提前停止，并保证截断后的
  行/字形区间仍连续。

### G-13 Web 打开书会泄漏整本书字节

- **状态**：未修复
- **现象**：`open_book` 用 `Box::leak(bytes.to_vec())` 把书变成 `'static`；
  换书时旧字节不释放。字体、资产、IR 也有多处 `to_vec` / `clone`。
- **影响**：长时间使用、频繁换书时 wasm 线性内存只涨不降，最终 OOM。
- **证据**：`crates/ebook-web/src/lib.rs` 的 `open_book`、`register_font`、
  `asset_bytes`、`layout_cached`。
- **修复**：让 `EpubReader` 拥有 `Vec<u8>`，或至少提供 `close_book()` 释放旧书。

### G-14 SVG 后端不是精确参考渲染

- **状态**：部分
- **现象**：`ebook-render::SvgSurface::glyph_run` 只写
  `<text font-family="sans-serif">` + 原始字符串，忽略 `Glyph` 数组、advance、
  kerning、bidi 重排；`image()` 只画占位。`ebook-cli` 的 `CliSvg` 能内联真实
  图片，但文字仍是 `<text>`。
- **影响**：SVG 输出不能用来验证排版几何；bidi/两端对齐/字距失真。
- **证据**：`crates/ebook-render/src/svg.rs`、`crates/ebook-cli/src/svg.rs`。
- **修复**：要么把 glyph 转成 `<path>`，要么在文档里明确它只是「可读预览」。

### G-15 parsed face 没有缓存

- **状态**：部分
- **仍未修复**：`face()` 在缓存未命中时仍逐次 `read_fonts::FontRef::from_index`
  （整形用的 harfrust `Font` 已随 `Metrics` 复用，二者共享同一份字体字节）。
- **影响**：度量/轮廓相关的重复解析带来额外 CPU 开销，大字体明显。
- **证据**：`crates/ebook-font/src/lib.rs` 的 `face()`。
- **修复**：给每个 `Metrics` 缓存 parsed face，绑定 `FontBook` 生命周期，保持无全局状态。

### G-19 翻页动画与跟手滑动只在宿主，跟手路径基本不可用

- **状态**：未修复
- **现象**：
  - 动画全在 `web/reader.js`（`playFade` / `playSlide` / `beginSwipe` /
    `applySwipeDrag` / `endSwipe`），CSS `transform` + `setTimeout` 状态机驱动；
    引擎没有页级位图出口。
  - `beginSwipe` 要求目标页位图已在 `pageSnapshots` 里，否则 `return false`；
    `web/selection.js` 收到 false 后置 `swipeBlocked`，move 不产生位移，只在抬手
    按 `SWIPE_MIN = 44px` 翻一次。预取是 `requestIdleCallback` + 120/90ms、
    一次一页，起手前几乎命中不了；切章更不可能跟手。
  - 鼠标/触控板没有跟手路径：`pointerDown` 对非 `touch` 立即 `begin()` 进选区，
    swipe 分支不可达；也没有 `wheel` 监听。
  - 手势仲裁：`LONG_PRESS_MS = 320` 优先于横滑判定，慢速横滑先变选区；
    `SWIPE_START = 12px` 没有从 dx 里扣除，起手跳 12px。
  - 收尾用 `setTimeout` 而非 `transitionend`，`animating` 期间 `turnPage` 直接
    return；后台节流或掉帧时状态机挂住，overlay 不清理、输入被吞。
  - overlay 是裸 `<canvas>`，没有 `#page` 的主题 `filter`，滑动过程颜色会跳。
  - 内存：`pageSnapshots` 留 4 张全 DPR 全页 canvas（≈11.9MB/张），滑动再叠
    2 张 overlay（字形 atlas 已经随引擎绘制一起删掉了）。
  - `applySwipeDrag` 没有 rAF 合并；`tests/app.mjs` 没有 pointer 序列测试。
  - overlay 的主题滤镜靠 `getComputedStyle(#page).filter` 复制，不再有自己的绘制。
- **影响**：真机「拖不动，松手才翻」；快速连续翻页被吞；主题下闪色；移动端接近
  canvas 内存上限时首帧卡顿甚至白页。
- **证据**：`web/reader.js` 的 `snapshotPage` / `placeOverlay` / `beginAnimation` /
  `beginSwipe` / `endSwipe` / `prefetchNeighbor`；`web/selection.js` 的
  `pointerDown` / `pointerMove` / `pointerUp`；`web/index.html` 的 `#page` 样式。
- **修复**：① 宿主侧：起手永不拒绝（目标页缺失时只滑当前页 + 边缘阴影），
  `pointerdown` 立即预取邻页，收尾改 `transitionend` + timer 兜底，`pointermove`
  用 rAF 合并并扣掉起始阈值，overlay 继承主题，快照改 `ImageBitmap` 并降 DPR，
  缓存改成「当前页 ±1 显式槽位」。② 引擎位图出口已经就位（`eb_page_bitmap`），
  动画只搬两张位图。
- **验收**：滑到一半时旧页 transform ≈ dx、新页 ≈ dx + 页宽；目标页未缓存时同样
  有位移；sepia/dark 无闪色；`xtask app` 的 pointer 序列测试覆盖以上三条。

---

## 4. P2：工程与鲁棒性

### G-16 宿主资源与内存边界

- **状态**：部分
- **现象**：Web 端没有显式内存预算；CJK 字体、整本 zip、所有章节 IR 可能同时
  留在内存。`MAX_FONT_BYTES = 32MB` 只拦单字体。
- **影响**：移动端 wasm 容易触顶；字体 + 图片 + IR 的峰值不可控。
- **修复**：宿主层加总预算（字体/图片/IR），超限时降级（只留当前章 IR、
  图片按需解码、字体子集化）。

### G-18 其他已知小项

- **状态**：未修复 / 部分
- `text-emphasis`（中文着重号）不支持，CJK 正文常见。
- `ruby` 只处理直接 `ruby/rt`，没有 `rp` / `ruby-position` / 复杂 ruby。
- `text-align: justify` 对 CJK 把余量平均加到每个字间隙，不是标点/字间分开处理。
- `text-indent` 在 RTL 下不镜像。
- `opacity`、`transform`、`filter`、`box-shadow` 不支持。
- `page-break-*` 与 `break-*` 只支持一部分，`left/right/recto/verso` 没有奇偶页处理。
- `linear="no"` 只记录在 `SpineItem`，阅读顺序没有真正跳过。

### G-21 主线程仍承担解析、光栅、绘制、解码、查询

- **状态**：未修复
- **现象**：
  - `layout-worker` 只接 `layout`；开书在主线程 `eb_open` 一遍，worker 又解析一遍，
    且 `workerBook` 是结构化克隆整本字节（没有 `transfer`）。
  - 每章主线程做两次 IR 解码：`eb_use_layout` → `adopt_layout` 里 `codec::decode`
    并 `chapter_with_anchors` **重新解析本章 XHTML/CSS**；随后 `decodeLayout` 又把
    同一份字节解成 JS 对象图（`ops` / `glyphs` / `lines` 每项一个对象）。
  - 整页绘制、首见字形的 `eb_glyph_bitmap` 光栅化与 atlas 写入、页快照拷贝、
    idle 里的 `renderPageCanvas`，全在主线程。
  - 字体解析两份：`eb_font`（主线程）与 worker 的 `eb_font`；不再有
    `new FontFace` 那一份。
  - `eb_hit` / `eb_selection` / `eb_link_at` / `eb_progress` 走 JSON 信封 + JS 二次
    解码，选区拖动每个 `pointermove` 一次，没有 rAF 合并。
  - worker 8s 超时后回退主线程排版。
  - 导入时 `text.js` 的 `fingerprint` 在 JS 里逐字节循环；IndexedDB 整本存取。
- **影响**：切章有可见停顿与 GC 尖峰；预取绘制与手势争线程；峰值内存与
  G-13 / G-16 叠加，移动端更容易触顶。
- **证据**：`crates/ebook-web/src/lib.rs` 的 `adopt_layout` / `layout_cached_style`；
  `web/layout-worker.js`；`web/reader.js` 的 `open` / `layoutChapter` / `render` /
  `requestLayout`；`web/decode.js`；`web/text.js` 的 `fingerprint`。
- **修复**：① 交互状态常驻 worker（主线程只发 RPC），删掉 `adopt_layout` 的章节
  重解析；② 开书只解析一次（worker 权威 + `transfer`）；③ 位图在 worker 里生成并
  `transfer`；④ `decodeLayout` 只解当前页 ±1 的切片（绘制已经不需要 op 图）；
  ⑤ 查询改 postcard / typed array 并做 rAF 节流；⑥ `fingerprint` 挪 worker。
- **验收**：切章期间主线程无 >50ms 长任务；开书解析 1 次、字体解析 ≤2 次；
  `decodeLayout` 只为可见页生成对象图。

### G-22 脏 XHTML 的容错边界

- **状态**：部分（容错层已建立，仍在扩展）
- **仍未覆盖**：完整的 HTML5 树构造算法（表格 foster parenting、`<select>` 插入
  模式、模板内容）、`<frameset>`、表单控件语义。这些结构只保证「不白屏」，
  不保证与浏览器逐节点一致。
- **证据**：`crates/ebook-format/src/html_repair.rs`；`tests/robust.rs`；
  `crates/ebook-format/examples/audit.rs`。
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
   一致性、`ImageBitmap` + 降 DPR）。唯一「用户已经能感觉到坏掉」的项，成本低。
2. **G-03 / G-02**：剩余选择器、`hsl()`、`capitalize`。成本低，直接改善真实书样式。
3. **G-04 / G-05**：背景/边框 op、列表 marker、单层表格。IR 已有 `Rect`/`Border`。
4. **G-06 JPEG / G-07 字体混淆**：决定「图片和字体能不能显示」。
5. **G-21**：交互状态回 worker + 开书只解析一次。
   是宿主架构改造，最好先把单页光栅基准测出来再动手；做完能顺带消掉
   G-13 / G-16 的峰值内存。
6. **G-12 / G-11**：`max_lines` 前移、真正的增量重排。内核改动，放在语义修复之后。
7. **G-09 / G-10 / G-08 / G-14**：RTL、fixed-layout、可变/彩色字体、精确 SVG。
   工作量大，单独排期。

---

## 7. 已确认的稳固点

这些不是「没问题」，而是当前审计没有发现底层缺陷：

- 全仓库无 `unsafe`。
- `DisplayList` 禁止 `HashMap` / `HashSet`，遍历顺序固定。
- 排版层不碰 I/O、不取时钟/随机、不持全局可变状态。
- native / wasm 有 `xtask parity` 逐字节比对。
- 坏 ZIP / 坏 XHTML / 坏字体有 fuzz 和越界保护；ZIP 路径穿越与单条目解压有上限。
- `Limits` 超限仍返回合法 IR，行/字形/页切片保持连续。
- `hyphen-en` 已改成 opt-in，不再默认把 `en.pat` 打进 wasm。
- 页 → 像素只有一套实现：`paint()` + `RgbaSurface`，native（`--ppm`）与 wasm
  （`eb_page_bitmap`）同源，`ScaledSurface` 负责 DPR。

---

## 8. 如何更新这份文档

1. 修复条目后：能整条删就删（编号不复用）；只剩尾巴的，只留「仍未修复」。
2. 发现新缺口时追加 `G-xx`，不要重排已有编号。
3. 某条目因设计变更变成「不做」，移到第 5 节并删掉原条目。
4. 与 `docs/limits.md` 重叠的数值不要在这里重复抄，链接过去即可。
