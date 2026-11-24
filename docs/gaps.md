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

1. **格式/样式语义**：选择器、`white-space` 之外的 CSS 细节、表格、行内背景/边框。
2. **资源层**：native 图片解码只有 PNG/JPEG（GIF/WebP 缺）；可变字体/彩色字体/子集化缺。
3. **宿主/架构**：没有增量重排；翻页动画与跟手滑动仍在 JS 且基本不可用（G-19）。

另有一批是**明确不做**的（竖排、flex/grid、PDF、DRM），见第 5 节。

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
- **仍未修复**：`text-transform: capitalize`、
  `vw` / `vh` / `ex` / `ch` 等视口/相对单位（IR 没有视口上下文）。
  （`pt`/`pc`/`in`/`cm`/`mm`、`currentColor`、8 位 hex、`rgb()` / `hsl()`
  的百分比与 alpha、`@import` 都已支持；导入按文件内的相对路径解析，
  环与自导入会停，最多 8 层 / 32 个文件。）
- **影响**：`font-size: 3vw` 之类的声明被整条丢弃，回落到默认字号；
  `capitalize` 静默不生效。
- **证据**：`crates/ebook-format/src/css.rs` 的 `parse_len` / `parse_color`；
  `crates/ebook-ir/src/doc.rs` 的 `TextTransform` 没有 `Capitalize` 变体。
- **修复**：`capitalize` 成本最低（只加 IR 枚举变体）；视口单位要在
  `ComputedStyle` 之外引入视口上下文，可放到最后。

### G-03 选择器缺函数式伪类和伪元素

- **状态**：部分
- **已支持**：tag / id / class / 属性（`[a]`、`=`、`~=`、`|=`、`^=`、`$=`、
  `*=`、`i` 标志）/ 后代 / `>` / `+` / `~` / `:first-child` / `:last-child`。
- **仍未修复**：`:not()` / 其它函数式伪类、伪元素（`::before` / `::after`）；
  这些仍整条规则丢弃。
- **影响**：`:not(...)`、`::first-letter` 等写法丢样式，按状态类选择器写的
  章节/封面微调会失效。
- **证据**：`crates/ebook-format/src/css.rs` 的 `parse_selector`（遇到 `(` 返回
  `None`）/ `matches`；`ElemKey::attrs` 保存元素的全部属性。
- **验收**：每种新增组合都要有 CSS 级联测试，不只看单条声明。

### G-04 背景、边框只解析不输出

- **状态**：部分
- **已修复**：块级盒的 `background` / `border` 现在会产出 `Op::Rect` /
  `Op::Border`。背景按 `background-clip: border-box` 铺满边框盒，边框在背景之上；
  跨页的边框盒按页切段，首段保留上边框、末段保留下边框、中间段只留左右。
  折叠外边距后的盒顶也按实际内容位置计算，不会把外边距涂进背景。
- **仍未修复**：
  - 行内元素（`span` / `em`）的背景与边框：它们跨行，需要按行盒切段。
  - 根/`body` 背景没有按 CSS 的 canvas 传播规则铺满页边距；目前只铺内容盒。
  - `border-radius`、`border-style`、逐边颜色；IR 的 `BorderBox` 只有宽度和单色。
  - `line-through` / `overline` 没有字段（`underline` 已支持）。
- **证据**：`crates/ebook-layout/src/flow.rs` 的 `block` 末尾发装饰，
  `crates/ebook-layout/src/lib.rs` 的 `split_decoration` 按页切段；
  `crates/ebook-layout/tests/decorations.rs` 覆盖单页与跨页。
- **验收**：golden 里出现 `rect`/`border` op；pixel 基线覆盖有边框的块。

### G-05 表格没有真正的排版语义

- **状态**：部分
- **已修复**：列表 marker。`<ul>` / `<ol>` 在 UA 样式里设置 `ListStyle`，
  `li` 继承（`list-style-type: none` 会关掉），布局层在首行内容外侧画
  `•` / `1.`；`<ol start>` 生效，marker 不并进 `LineBox.glyphs`（不参与选区）。
- **仍未修复**：
  - 表格仍是 `Display::Block`：`Tag::Table` / `Tr` / `Td` 都按普通块排，
    没有列宽、行高与跨行跨列，表格变成一列。
  - 列表只支持直接 `ul/ol > li`；`list-style-position: inside`、
    `lower-alpha` / roman 的实际字形（`ListStyle` 只有 `ordered: bool`）、
    `<li value>` 与嵌套计数的复杂组合都还没做。
- **影响**：非虚构类 EPUB 的参数表、跨列表格明显变形；列表已经可用。
- **证据**：`crates/ebook-format/src/css.rs` 的 `ua_apply` / `apply`
  （`list-style*`）；`crates/ebook-layout/src/flow.rs` 的 `list_marker`；
  `crates/ebook-layout/tests/lists.rs`。
- **修复**：表格先支持单层 `tr/td` 的列宽与行高；其余按宿主反馈再扩。

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
- **仍未修复**：
  - CSS `unicode-bidi`（`bidi-override` / `isolate`）不解析；`<bdi>` / `<bdo>` 只是普通 `Span`。
  - 只有 `doc.meta.lang` 取第一个 `lang`，后续元素的 `lang` 不影响 `locl`、断字或字体选择。
- **已修复**：`text-align: start/end` 按 `direction: rtl` 翻转（`emit_line` 读
  `base_style.dir`）；`tests/robustness.rs` 的 `rtl_start_and_end_are_mirrored` 覆盖。
- **影响**：阿拉伯语/希伯来语的嵌套方向可能错；多语言书的逐段语言特性不生效。
- **证据**：`crates/ebook-layout/src/flow.rs` 的 `emit_line`；`css.rs` 的属性表；
  `xhtml.rs` 的 `lang` 处理。
- **修复**：其余需要把语言/方向放进 `ComputedStyle` / `TextStyle`。

### G-10 fixed-layout、SVG/MathML、`@page`

- **状态**：未修复
- **现象**：不处理 `rendition` / fixed-layout / `page-progression-direction`；
  内联 SVG / MathML 落到 `Tag::Other`；不支持 `@page`、页眉页脚、running header。
- **影响**：绘本、漫画、教材、技术书明显变形。
- **证据**：`epub.rs` 的 package 解析；`css.rs` 的 at-rule 分支。
- **修复**：fixed-layout 需要独立页面模型，应单独立项；SVG 先只拿尺寸再考虑路径渲染。

### G-11 没有真正的增量重排

- **状态**：部分
- **已修复（阶段切分）**：`Layout::layout_cached` + `FlowCache`（`crates/ebook-layout/src/cache.rs`）
  把「盒树 + 断行 + 整形」与「分页 + 组装」切成两级：前段的键只含调用方版本号、
  可用宽度、行数预算，**刻意不含页高与 `Mode`**。`Engine` 已接线（`build_layout`
  用 `(spine, style, font_rev)` 递增 `flow_rev`），所以以下场景不再重跑断行：
  手机地址栏收起、旋屏、分页 ↔ 滚动切模。实测（`en.epub` 1655 字 + 真实字体）：
  同宽换页高 `0.855 ms → 0.132 ms`（省 85%）。不变量：同一输入下带缓存的 IR
  与不带缓存逐字节相同（`crates/ebook-layout/tests/flow_cache.rs`、
  `crates/ebook-reader/tests/flow_cache.rs`）。
- **仍未修复**：没有「只重排某一块」的入口。按块失效需要一个 field-complete 的
  块级内容哈希（`Node` / `ComputedStyle` 全字段）或块级 `StyledDoc` 缓存，漏一个
  字段就是静默错排，代价与收益不成比例。
- **别做（实测结论）**：块级缓存解不了「改字号」——字号参与整形，所有块的键都变。
  `en.epub` 一章 `layout` 共 0.49 ms，其中整形只占 23%（0.114 ms），其余是盒树 /
  断行 / 组装，而这些在宽度或字号变化时本来就无法复用。真正卡交互的是 G-19 的
  翻页动画，不是排版。
- **证据**：`crates/ebook-layout/src/flow.rs` 的 `dirty.push(...)`；
  `crates/ebook-cache/src/lib.rs` 的 `LayoutCache::layout`（整份 IR 的 LRU，键含页高）。
- **修复**：确实需要块级入口时，先把 `flow` 的「计划/断行/回填」拆成可独立重算的
  块，再让 cache 按块失效；`FlowCache` 的两级形状可以直接沿用。

### G-13 Web 宿主的内存拷贝与预算

- **状态**：部分
- **已修复**：`open_book` 不再 `Box::leak`。书字节的所有权登记在 `BOOK_BYTES`，
  换书/`close_book()` 时先丢掉借用者再按原样回收；`eb_close_book` 已导出。
  回归用例见 `crates/ebook-web/src/lib.rs` 的 `interaction_json_shapes`。
- **仍未修复**：`register_font` / `asset_bytes` / `layout_cached` 仍有多处
  `to_vec` / `clone`（字体会整份复制，`layout_cached` 同时持有 `doc` 与 `ir`）；
  宿主没有显式内存总预算。
- **影响**：频繁换字体/图片或同时保留多章 IR 时，wasm 线性内存峰值偏高。
- **证据**：`crates/ebook-web/src/lib.rs` 的 `register_font`、`asset_bytes`、
  `layout_cached`。
- **修复**：`FontBook` 接受 `Vec<u8>` 所有权；`layout_cached` 用 `move` 而不是
  clone；宿主层加总预算（见 G-16）。

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

- **状态**：部分
- **已修复**：`text-indent` 按段落方向镜像（LTR 从左边扣、RTL 从右边扣）；
  `<rp>` 回退括号不再进入正文。
- `text-emphasis`（中文着重号）不支持，CJK 正文常见。
- `ruby`：布局层有 `NodeKind::Ruby`，但格式层没有把 `ruby/rt` 折成它，
  真实 XHTML 的注音不会排到基字上方；也没有 `ruby-position` / 复杂 ruby。
- `text-align: justify` 对 CJK 把余量平均加到每个字间隙，不是标点/字间分开处理。
- `opacity`、`transform`、`filter`、`box-shadow` 不支持。
- `page-break-*` 与 `break-*` 只支持一部分，`left/right/recto/verso` 没有奇偶页处理。
- `linear="no"` 只记录在 `SpineItem`，阅读顺序没有真正跳过。

### G-22 脏 XHTML 的容错边界

- **状态**：部分（容错层已建立，仍在扩展）
- **仍未覆盖**：完整的 HTML5 树构造算法（表格 foster parenting、`<select>` 插入
  模式、模板内容）、`<frameset>`、表单控件语义。这些结构只保证「不白屏」，
  不保证与浏览器逐节点一致。
- **证据**：`crates/ebook-format/src/html_repair.rs`；`tests/robust.rs`；
  `crates/ebook-format/examples/audit.rs`。
- **验收**：坏章节只影响自己；公开语料 + 变异 fuzz 下无 panic、无整本失败。

### G-26 没有章节流入口：宿主无法追加/替换一章文本

- **状态**：未修复（宿主 Quire 的在线书源需要）
- **场景**：宿主按章从书源取正文，没有整本字节。现在只能把 3 章拼成临时 TXT，
  `eb_plain_close` + `eb_plain_open` 重开。
- **代价**：两套章号（窗口内 `spine` vs 全书下标，Quire 有 22 处换算）；换窗
  必须重排并丢 `FlowCache`/滚动列；目录两份；页缓存要带窗口基址；窗口要落盘，
  标题不像「第…章」时还得宿主补。
- **需求**：
  1. `eb_plain_new_book(title)`：0 章，书名由参数给。
  2. `eb_plain_append_chapter(doc, title, text)`：尾部追加，返回章数。
  3. `eb_plain_replace_chapter(doc, spine, title, text)`：换一章，章数与 `spine` 不变。
  4. `eb_plain_chapter_rev(doc, spine) -> u64`：该章内容版本，替换/追加后变化，
     宿主用它做页位图的键。
  5. 可选：`eb_plain_clear_chapters`、`eb_plain_insert_chapter`（后者移动
     `spine`，优先级低）。postcard 面加同义变体。
- **语义**：UTF-8；标题由宿主给（包成 `H1`），`text` 每行一段，不跑分章规则；
  替换后该章 `layout`/`scroll`/`FlowCache` 失效、`scroll_total_height` 重算；
  单章受 `max_text_bytes`，总量不限（逐章懒解析）；非法 `spine`/空文本/超限
  返回空结果或 `-2`，不 panic；只加新函数。
- **与 G-11 无关**：G-11 是同一文本的重排缓存，这里是内容本身会变。
- **证据**：`quire-asuka/reader.asuka`（`openOnline`/`switchWindow`/`windowBase`）、
  `remote/online.asuka`（`materialize`）。
- **验收**：append 后章数 +1 且旧章不变；replace 后该章 IR 与 rev 变、其他章
  rev 不变；滚动列追加后总高增长、旧 band 逐字节不变；无文件也能排版；
  native/wasm parity；空/超长/越界不 panic。

### G-27 打开大书要整本进内存

- **状态**：未修复（宿主 Quire 在 iOS 导入 300MB EPUB 时卡死）
- **现状**：`eb_plain_open` 收的是 `bytes`，内部 `to_vec()`；Asuka 侧
  `engine.open(path)` 先把整本 `fs.read` 进来。打开一本 300MB 的书至少两份
  全量拷贝，且发生在主线程，再叠加 ZIP 解析。
- **需求**：`eb_plain_open_path(path)`（或 `open` 的路径变体）：按需
  `pread`/mmap，不持有整本字节；加 `eb_plain_probe(path)` 只读
  OPF/封面/目录/章数/文本字节数，不解析全书；无文件也能排版/光栅。
- **验收**：300MB EPUB 打开时宿主常驻内存不随文件线性增长；`probe` 在
  一秒内返回元数据；native/wasm 对同一文件 IR 逐字节一致。
- **关系**：与 G-26 是同一个「书是章节流，不是一个 blob」的两端；G-26 是在线
  端，本条是本地端。

---

## 5. 明确不做（不是缺陷）

| 项 | 原因 |
|---|---|
| 竖排 / `WritingMode::Vertical*` | 字段保留，纵向布局是独立工程 |
| flex / grid / 绝对定位 / float | 不追求浏览器级 CSS 兼容 |
| SVG 逐字形精确渲染 | SVG 后端是可读预览：文本按 `GlyphRun.text` 输出，不复现 kerning/bidi/几何；几何校验用 RGBA / 像素基线 |
| PDF 输出 | 当前只有 SVG / RGBA / Canvas |
| DRM 解密 | 只考虑 `encryption.xml` 的字体混淆，不做内容解密 |
| 完整的 UAX #14 上下文规则 | 只实现成对可判定部分，已写在 `docs/design.md` |
| 非英文断字词典 | `English` 已是 `hyphen-en` 可选 feature；其他语言由宿主注入 |

---

## 6. 建议修复顺序

1. **G-19**：先让跟手滑动可用（起手不拒绝、预取、`transitionend`、rAF、主题
   一致性、`ImageBitmap` + 降 DPR）。唯一「用户已经能感觉到坏掉」的项，成本低。
2. **G-03 / G-02**：`:not()` / 伪元素、`capitalize`。成本低，直接改善真实书样式。
3. **G-05**：单层表格（列表 marker 已完成）。
4. **G-06 JPEG / G-07 字体混淆**：决定「图片和字体能不能显示」。
5. **G-11**：真正的增量重排。内核改动，放在语义修复之后。
6. **G-09 / G-10 / G-08**：剩余 RTL 语义、fixed-layout、可变/彩色字体。
   工作量大，单独排期。
7. **G-26**：章节流入口（append / replace / chapter_rev）。宿主现在用
   `close`/`open` + 临时 TXT，换窗必须重排；只加新函数，等有需求再排。

---

## 7. 已确认的稳固点

这些不是「没问题」，而是当前审计没有发现底层缺陷：

- 排版 / IR / 光栅 / 字体 / 渲染层无 `unsafe`；仅 `ebook-reader` 的
  `Engine`（自引用书字节）与 C ABI / wasm 边界有 `unsafe`，两处都注明安全前提。
- `LayoutResult` 的 `op_slice` / `glyph_slice` 对坏 span 退化为空切片，
  `Span::end` 饱和；解码后的 IR 不会因为越界索引 panic（`release` 是 abort）。
- `DisplayList` 禁止 `HashMap` / `HashSet`，遍历顺序固定。
- 排版层不碰 I/O、不取时钟/随机、不持全局可变状态。
- native / wasm 有 `xtask parity` 逐字节比对。
- 坏 ZIP / 坏 XHTML / 坏字体有 fuzz 和越界保护；ZIP 路径穿越与单条目解压有上限。
- `Limits` 超限仍返回合法 IR，行/字形/页切片保持连续；`max_lines` 在断行阶段
  就提前停止，不再先排完整章。
- `hyphen-en` 已改成 opt-in，不再默认把 `en.pat` 打进 wasm。
- 页 → 像素只有一套实现：`paint()` + `RgbaSurface`，native（`--ppm`）与 wasm
  （`eb_page_bitmap`）同源，`ScaledSurface` 负责 DPR。
- wasm 实例只在 worker 里创建一次：开书/字体/排版/光栅/查询都在那个线程，主线程
  拿页元数据与位图，IR 不过线程边界。

---

## 8. 如何更新这份文档

1. 修复条目后：能整条删就删（编号不复用）；只剩尾巴的，只留「仍未修复」。
2. 发现新缺口时追加 `G-xx`，不要重排已有编号。
3. 某条目因设计变更变成「不做」，移到第 5 节并删掉原条目。
4. 与 `docs/limits.md` 重叠的数值不要在这里重复抄，链接过去即可。
