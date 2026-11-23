# API 契约

本文只写**对外承诺**，不写实现细节。三份契约（`StyledDoc`、`LayoutResult`、
`Renderer`）的任何形状变化都属于破坏性变更；其余 crate 的公开函数可以增删，
但下面列出的语义（越界返回什么、空输入产出什么、错误如何分类）保持稳定。

- `ebook-ir`：数据结构、IR 编解码、渲染 trait
- `ebook-layout`：纯排版入口、字体度量与图片尺寸注入、限额、命中/选区/阅读器外观
- `ebook-cache`：以输入指纹为键的有界 LRU 排版缓存
- `ebook-format`：字节源、格式适配器、章节读取与诊断
- `ebook-font`：真实字体度量与光栅化
- `ebook-render`：op 遍历、SVG 后端
- `ebook-raster`：整数混合光栅后端
- `ebook-web`：wasm 导出与 JS 字节契约

坐标、层级与不变量另见 [design.md](design.md)；硬上限见 [limits.md](limits.md)。

---

## 1. 数据流

```
ByteSource ──format──> StyledDoc ──layout──> LayoutResult ──render──> Surface / 像素
```

三段之间只传值，没有回调，也没有 trait object 反向调用排版层。`TextMeasurer` 与
`ResourceView` 是排版入参，由调用方注入；排版层永远不自己去找字体或读文件。

---

## 2. `ebook-ir`：数据结构

### 2.1 `StyledDoc`

`StyledDoc` 是格式层的输出、排版层的输入。它包含：

| 字段 | 语义 |
|---|---|
| `meta: DocMeta` | 章节序号 `spine`、标题、语言、书写方向 |
| `nodes: Vec<Node>` | 节点表；`NodeId` 就是下标，`u32` |
| `children: Vec<NodeId>` | 所有节点的子节点，按父节点切段 |
| `text: String` | 唯一的文本缓冲；`Span` 指向它 |
| `images: Vec<ImageMeta>` | 图片固有尺寸、MIME、alt |
| `root: NodeId` | 遍历起点 |

构造契约：

- `nodes`、`children`、`text` 都是 arena；节点之间的父子关系由
  `Node.first_child` / `Node.child_len` 指向 `children` 的一段。
- `NodeKind::Text(Span)` 的 `Span` 必须落在 `text` 的 UTF-8 char 边界内。
- `Node.src` 是原始文档锚点（`spine` + `path: Vec<u16>` + `offset`），
  用于 CFI / 选区。排版层只携带，不解释。
- `Display::None` 的节点不产生盒，也不产生 op。
- `StyledDoc::node` / `kids` / `text_of` 是**信任调用方**的索引访问：
  越界会 panic。格式层必须保证索引有效。

### 2.2 `LayoutResult`

```
LayoutResult
├── version: IrVersion       // 1.4
├── spine: u32               // 与 StyledDoc.meta.spine 一致
├── pages: Vec<Page>
├── display: DisplayList { ops: Vec<Op>, glyphs: Vec<Glyph> }
├── runs: RunMap { lines: Vec<LineBox> }
├── dirty: Vec<DirtyBlock>
├── issues: Vec<Issue>       // 可恢复问题的诊断流（见 2.3）
└── truncated: bool          // 因 Limits 截断过；结果仍是合法 IR
```

| 结构 | 约定 |
|---|---|
| `Page.ops` / `Page.lines` | 指向 `display.ops` / `runs.lines` 的连续切片 |
| `Page.origin` | 该页在**排版流**里的起点；只移动它，不改 op 坐标 |
| `Page.size` | 内容区尺寸（不含 `inset`） |
| `Page.inset` | 页边距；`box_size()` = 内容尺寸 + 四边 |
| `Page.anchor` | 页首锚点，可能为空 |
| `GlyphRun.glyphs` | 指向 `display.glyphs` 的连续切片 |
| `GlyphRun.weight` / `style` / `underline` | 文字样式随 run 走；宿主没装字体也能合成粗体/斜体/下划线 |
| `LineBox.src` | 对应 `StyledDoc.text` 的源区间，用于选区；整行只有行内图片时该区间没有意义 |
| `DirtyBlock` | 需要宿主重排/换字体的块，`reason` 见 2.3 |
| `Issue` | 排版过程中记录的可恢复问题；只记录，不拦截 |
| `truncated` | `true` 表示命中 `Limits`，内容不完整；页覆盖与行不越界仍然成立 |

页是按顺序切出来的，不是过滤结果。正常排版下：

- `pages[i].index == i`
- `pages[0].ops.start == 0`，后一页的 `start` 等于前一页的 `start + len`
- 所有页的 op 并集恰好覆盖 `display.ops`，行同理
- 每页至少有一行；空文档可以没有页

### 2.3 问题码表（`UnresolvedReason` / `IssueCode`）

IR 里有两个互补的枚举：

- `UnresolvedReason` 挂在 op 与 `DirtyBlock` 上，告诉**渲染层/宿主**这块内容为什么
  画不出来；变体顺序就是 postcard/JS 解码的 tag。
- `IssueCode` 挂在 `LayoutResult.issues` 上，是给**诊断面板**看的分类；它不影响
  渲染，也不改变几何。

| tag | `UnresolvedReason` | 含义 | 宿主应该做什么 |
|---:|---|---|---|
| 0 | `ImagePending` | 图片固有尺寸未知，先按占位盒排版 | 图片到达后带尺寸重排；不要由渲染层猜 |
| 1 | `FontMissing` | 字体不覆盖该字符（`glyph_id == 0`） | 换字体、回退字体或标记缺字 |
| 2 | `SizeFromContent` | 尺寸依赖内容，当前无法结算 | 提供尺寸后重排 |

| tag | `IssueCode` | 何时出现 |
|---:|---|---|
| 0 | `Other` | 保留给宿主/适配器，排版层当前不产 |
| 1 | `MissingFont` | 保留；排版层用 `MissingGlyph` 表达缺字 |
| 2 | `MissingGlyph` | 对应 `UnresolvedReason::FontMissing` 的占位 |
| 3 | `MissingImage` | 对应 `UnresolvedReason::ImagePending` 的占位 |
| 4 | `SizeFromContent` | 对应 `UnresolvedReason::SizeFromContent` |
| 5 | `UnsupportedCss` | 保留给格式层的诊断映射 |
| 6 | `LimitExceeded` | `detail` 写命中了哪条限额（`max_nodes` / `max_pages` / ...） |

`truncated` 与 `IssueCode` 的关系：

- `truncated == true` 一定伴随至少一条 `IssueCode::LimitExceeded`，`detail` 是限额名。
- 头部限额（`max_nodes` / `max_depth` / `max_text_bytes`）直接返回**空结果**：
  没有页、没有 op、没有行，`truncated == true`。
- 产出侧限额（`max_pages` / `max_lines`）返回已产出部分的**前缀**：页覆盖、
  行不越界、每行仍在自己的页高内都成立；`display.glyphs` 会截到仍被行引用的
  最大下标，不留悬空字形。
- `max_image_pixels` 超预算的图片降级为 `Op::Unresolved{SizeFromContent}` 并记
  一条 `LimitExceeded`，同样置 `truncated`；其余内容继续排。
- `truncated == false` 时仍可能有 `MissingGlyph` / `MissingImage` 占位：
  “不完整”与“超预算”是两件事。

`UnresolvedReason` 出现在两处：

- `Op::Unresolved(Unresolved { node, rect, reason })`：画一个虚线占位框。
- `DirtyBlock { node, rect, reason }`：告诉宿主哪些块需要增量重排。

渲染层只负责把占位画出来，**不允许**自己决定字体或图片尺寸。

### 2.4 `Op`

| tag | 变体 | 坐标 |
|---:|---|---|
| 0 | `GlyphRun(GlyphRun)` | 基线起点，排版流坐标 |
| 1 | `Rect { rect, color }` | 左上角，排版流坐标 |
| 2 | `Border(BorderBox)` | 外边盒，排版流坐标 |
| 3 | `Image { id, rect, fit }` | 内容盒，排版流坐标 |
| 4 | `Unresolved(Unresolved)` | 占位盒，排版流坐标 |
| 5 | `Clip { rect, ops }` | 裁剪矩形 + 嵌套 op |
| 6 | `Link { rect, href }` | 链接热区，与行盒同形，排版流坐标 |

`Rect` / `Border` 由布局层为**块级** `background` / `border` 生成；跨页的盒会在
每一页出现一段（首段带上边框、末段带下边框），不会因为页边界被截断成一条边。
行内元素的背景/边框仍未产 op。

`Link` 的语义：

- 排版层只在带 `Node.link` 的行上发射，一行一个（换行就把链接切成多个热区）。
- `rect` 是**行盒矩形**（`LineBox.rect`），不是文字的墨迹范围；点行间空白不算命中。
- `href` 是原始属性值，排版层不解析、不改写、不拼基址。
- 链接文字默认是蓝色并带下划线：颜色由格式层 UA 样式写入 `GlyphRun.color`，
  下划线由 `GlyphRun.underline` 携带；书里的 CSS 可以覆盖。
- 热区本身不改颜色；光栅保持透明，SVG 后端输出 `<a>`；命中查询用
  `Reader::link_at`（页内坐标）。

`Fit`：`Contain=0`、`Cover=1`、`Fill=2`。
`Rotate`：`None=0`、`Cw90=1`。
`Dir`：`Ltr=0`、`Rtl=1`。

### 2.5 越界与空结果语义

| API | 输入 | 返回 |
|---|---|---|
| `LayoutResult::page(i)` | 任意 `u32` | 越界 `None` |
| `LayoutResult::page_of_line(l)` | 任意行号 | 没有页包含它则 `None` |
| `LayoutResult::anchor_at(p)` | 任意点 | 没有命中行或行无锚点则 `None` |
| `RunMap::line_at(p)` | 任意点 | 没有命中则 `None`；边界闭区间 |
| `DisplayList::op_slice(span)` | 有效 span | 切片；`span` 越界会 panic |
| `DisplayList::glyph_slice(span)` | 有效 span | 切片；`span` 越界会 panic |
| `StyledDoc::node(id)` | 有效 id | 引用；越界会 panic |
| `Renderer::render(ir, page, ...)` | 任意页号 | 越界 `Err(RenderError::NoSuchPage(page))` |
| `Reader::hit(page, p)` | 任意页号/坐标 | 页不存在或页内没有行才 `None`；落空时退回本页最近的行 |
| `Reader::link_at(page, p)` | 任意页号/坐标 | 没命中 `Op::Link` 则 `None` |
| `Reader::progress(loc)` | 任意位置 | 夹到合法位置后返回 `0..=1`；空文档 `0` |
| `Reader::page_progress(page)` | 任意页号 | 页不存在或空文档 `0` |
| `Reader::locate(anchor)` | 任意锚点 | 找不到则 `None` |
| `Reader::clamp(loc)` | 任意位置 | 夹到最近合法位置；空文档返回 `Location::default()` |
| `Reader::selection_rects(sel)` | 任意选区 | 空选区返回空 `Vec`；矩形是页内坐标 |
| `Reader::selection_text(doc, sel)` | 任意选区 | 空选区/空文档返回空串 |
| `LayoutCache::layout(doc, env, c)` | 任意输入 | 总返回 `&LayoutResult`；容量 0 按 1 处理 |
| `Hyphenator::breaks(word, out)` | 任意词 | 非字母词得到空 `out`；偏移在 `(0, word.len())`、升序 |
| `IssueCode` / `Issue.detail` | 任意 | `detail` 是 `Option<String>`；限额名写在这里 |

排版结果是**全函数**：`ebook-layout::layout` 不返回 `Result`，任何文档都会得到
一个可序列化的 `LayoutResult`。空文档得到零页或最多一页；极小页面被夹到
内容尺寸至少 `1.0 × 1.0`。

---

## 3. `ebook-ir::codec`：字节契约

### 3.1 编码

```
偏移 0..4   MAGIC = "EBIR"
偏移 4..6   major: u16 little-endian
偏移 6..8   minor: u16 little-endian
偏移 8..    postcard 编码的 LayoutResult 主体
```

主体里还会再编码一次 `LayoutResult.version`。JS 解码器要求 body 版本与 header
一致；Rust 的 `read_header` 只检查 major，`decode` 依赖 postcard 结构体形状。

| 函数 | 语义 |
|---|---|
| `to_bytes(&T)` / `encode(&T, &mut Vec)` | 写 header + postcard；失败 `CodecError::Encode` |
| `decode::<T>(bytes)` | 校验 header 后解码；失败 `CodecError::Decode` |
| `read_header(bytes)` | 只读 header，返回发现的版本 |
| `hash_of(bytes)` | FNV-1a 64，给缓存键/日志用，不参与解码 |

### 3.2 `CodecError`

| 变体 | 触发条件 |
|---|---|
| `TooShort` | 少于 8 字节 |
| `BadMagic` | 前 4 字节不是 `EBIR` |
| `Version { found, expected }` | major 不同 |
| `Encode` | postcard 序列化失败 |
| `Decode` | postcard 反序列化失败 |

### 3.3 版本与兼容策略

当前 `IR_VERSION = 1.4`。

- **major 表示字节布局。** 增删字段、改字段顺序、增删/重排枚举变体、改整数宽度，
  必须升 major。旧解码器遇到不同 major 直接拒绝。
- **minor 表示相容的字段增补。** 新字段只在结构体**末尾追加**，旧版本语义不变：
  1.2 加 `Glyph.ch` / `Op::Link` / `Issue` / `Limits`（`Limits` 不进字节流），
  1.3 加 `LayoutResult.truncated`（`LayoutStats` 只是内存里的估算，不序列化）；
  1.4 给 `GlyphRun` 末尾追加 `weight` / `style` / `underline`，给 `TextStyle`
  末尾追加 `underline`。
  `read_header` 不因 minor 不同而拒绝。
- **但 postcard 是位置编码，不是自描述格式。** 旧解码器无法“跳过未知字段”，
  所以 minor 增补后 native 与 JS 必须**一起**升级；`decode` 依赖结构体形状，
  JS 侧额外把 header 与 body 的 `(major, minor)` 逐字段比对。把 minor 当
  “解码器可以不管”的信号是错误的。
- **重命名/重新排序/改语义必须升 major。** 仅改文档措辞不需要。
- **JS 侧是手写解码器。** 改 `crates/ebook-ir` 的结构体必须同步改
  `web/decode.js`、`web/encode.js`，并让 `xtask contract` 通过。contract 的做法
  是把 golden IR 解出来再编回去，要求逐字节还原。
- wasm 导出 `eb_version()` 返回 `(major << 16) | minor`，宿主可以据此提示不兼容。

---

## 4. `ebook-layout`：纯排版

### 4.1 入口

```rust
pub fn layout(doc: &StyledDoc, env: &LayoutEnv<'_>, c: &PageConstraint) -> LayoutResult

impl Layout {
    pub fn layout(&mut self, doc, env, c) -> LayoutResult;
    pub fn layout_with(&mut self, doc, env, c, opts: &LayoutOptions) -> LayoutResult;
    pub fn layout_parallel(&mut self, doc, env, c, workers: usize,
                           opts: &LayoutOptions) -> LayoutResult;
}
```

`Layout::new().layout(...)` 等价于 `layout(...)`。没有 I/O、没有全局状态、没有
时钟与随机数。同一个 `(doc, env, c)` 必须逐字节产出同一个 `LayoutResult`。
`layout_with` 只多一层预算；`layout_parallel` 的输出必须是串行结果的逐字节副本
（详见 [design.md §12](design.md#12-并发只在一次调用内)）。

### 4.2 `TextMeasurer`

| 方法 | 默认实现 | 契约 |
|---|---|---|
| `advance(ch, font, size)` | 无 | 返回逻辑宽度，单位 px |
| `ascent(font, size)` | 无 | 基线以上高度 |
| `descent(font, size)` | 无 | 基线以下高度 |
| `shape(text, font, size, out)` | 逐字 `advance`（`shape_per_char`） | 把一段**同字体**文本整形成字形序列，返回总前进宽度；字形按逻辑顺序追加，`ShapeGlyph.ch` 是簇首字符在该段文本内的字节偏移 |
| `x_height(font, size)` | `0.5 * size` | `vertical-align: middle` 用 |
| `covers(ch, font)` | `true` | 字体是否覆盖字符 |
| `glyph_id(ch, font)` | Unicode 低 16 位，超出用 `0xFFFD` | 缺字返回 0 |
| `hyphenator()` | `None` | 返回断字词典；`hyphens: auto` 只在非 `None` 时生效 |
| `select(spec)` | `spec.font` | 按字体栈选主字体 |
| `select_char(spec, ch)` | `select(spec)` | 按字符选能覆盖的字体 |

`shape` 的契约（实现真实整形时必须遵守）：

- `out` 只追加，不要求调用方先清空；`ch` 落在簇首字符的**字节边界**上。
- 字形按逻辑顺序：RTL 由实现内部翻回逻辑序，视觉重排由排版层的 `visual_order`
  统一做（按簇，不拆合字）。
- 返回的总宽必须等于 `out` 里字形前进宽度之和，否则量到的和画出来的会漂移。

`FontBook` 实现了真实版本（harfrust：kerning / ligature，`ch` 是簇首字节偏移）；
`StubMeasurer` 只用于测试与确定性 golden，走默认逐字实现，因此升级前后的
`StubMeasurer` 路径逐字节一致。

### 4.3 `ResourceView`

```rust
fn image_size(&self, id: ImageId) -> Option<SizeF>;
```

返回 `None` 表示图片尺寸未知。排版层不会等待加载，而是按占位尺寸排出
`Op::Unresolved{ImagePending}` 与 `DirtyBlock`。`NoImageSizes` 永远返回 `None`。

同一个查询也用于**块级与行内图片**的像素预算（`Limits::max_image_pixels`）：
资源层不知道尺寸的图片不参与累加（它已经按 `ImagePending` 降级了）。
`ResourceView` 不提供像素数据，只提供“固有尺寸”——真正的像素由渲染层的
`Resources::image` 取得，两层不交叉。

### 4.4 `PageConstraint`

| 字段 | 默认 | 语义 |
|---|---|---|
| `page: SizeF` | `600 × 800` | 页面盒尺寸 |
| `margin: EdgesF` | 0 | 四边页边距 |
| `writing_mode` | `HorizontalTb` | 当前只支持横排；纵向字段保留 |

`content_size()` = `page - margin`，每个轴单独夹到至少 `1.0`。
`Page.size` 存内容尺寸，`Page.inset` 存 margin，`Page.box_size()` 加回去。

### 4.5 分页语义

分页在排版层结算，因为断点反过来影响孤行寡行与段落切分：

1. 先找当前页最满的断点。
2. 按 `break-before` / `break-after` / `orphans` / `widows` 往回退。
3. 退不动就退回最满断点；约束冲突时宁可难看，也不产生空页。

输出保证见 [design.md 的不变量清单](design.md#2-不变量清单)。

### 4.6 `LayoutOptions` / `Limits`

```rust
pub enum Mode { Paged, Scroll }
pub struct LayoutOptions { pub limits: Limits, pub mode: Mode }
impl LayoutOptions {
    pub fn with_limits(limits: Limits) -> Self;
    pub fn scroll() -> Self;
}

pub struct Limits {
    pub max_nodes: u32,
    pub max_depth: u16,
    pub max_text_bytes: u32,
    pub max_pages: u32,
    pub max_lines: u32,
    pub max_image_pixels: u32,
}
```

`Limits` 的字段为 0 时由 `Limits::resolved()` 换成默认值；`LayoutOptions::default()`
就是“全部用默认”。默认值：

| 字段 | 默认 | 拦截位置 | 超限结果 |
|---|---:|---|---|
| `max_nodes` | `1_048_576` | flow 之前看 `doc.nodes.len()` | 空结果 + `LimitExceeded("max_nodes")` |
| `max_depth` | `256` | flow 之前按 parent 链记忆化算深度 | 空结果 + `LimitExceeded("max_depth")` |
| `max_text_bytes` | `20_000_000` | flow 之前看 `doc.text.len()` | 空结果 + `LimitExceeded("max_text_bytes")` |
| `max_pages` | `100_000` | 分页计划；需要开新页但已到上限 | 保留已计划的页，丢掉尾部 |
| `max_lines` | `2_000_000` | 组装侧累加行数 | 保留已产出的行前缀 |
| `max_image_pixels` | `67_108_864`（8192²） | 组装侧按 `ResourceView::image_size` 累加 | 超预算的图片降级为 `SizeFromContent` |

拦截顺序是固定的：先头部限额（O(1) 量级），再 flow，最后产出侧限额。所有超限
结果都是合法 IR：页覆盖、行不越界、编解码往返都成立；`truncated == true` 是唯一
的信号。逐字段的“0=默认 / 恰好等于 / 超一”测试在
`crates/ebook-layout/tests/limits.rs`。

`max_image_pixels` 是**像素数**不是字节数（`w × h`，按资源给出的固有尺寸）。

### 4.7 滚动模式

```rust
pub enum Mode { Paged, Scroll }

// ebook-render
pub fn paint_window(ir: &LayoutResult, page: u32, top: f32,
                    surface: &mut dyn Surface,
                    res: &mut dyn Resources) -> Result<(), RenderError>;
```

- `Mode::Scroll` 只改变“取窗口”：断行与块布局和 `Mode::Paged` 完全相同，
  所以滚动/分页切换不会让行或排版位置分裂。
- 整章产出一页：`Page.origin == (0, 0)`，`Page.size.h` 是内容实际高度
  （最后一行底边），`Page.ops` / `Page.lines` 覆盖整章。
- 上下 `PageConstraint.margin` 应传 0，左右保留；宿主在滚动容器上自己留
  上下白，瓦片之间才没有空白带。
- `paint_window(ir, 0, y, surface, res)` 把流坐标 `y` 画到 surface 的 y=0，
  surface 只需瓦片高；宽度是整页宽（含左右留白）。**窗口外的 op 直接跳过**
  （`Surface::scale()` 把 surface 高换算回流坐标）：取一枚 128pt 瓦片的代价
  只跟 128pt 有关，不跟章高有关。`top` 会吸到设备像素栅格，相邻瓦片的字形
  亚像素相位相同，滚动时光栅缓存才能命中。
- 命中仍用 `Reader::hit(0, x, y)`（`inset.top == 0` 时 `y` 就是流坐标）；
  `Reader::progress` 按行计算，滚动模式不需要新公式。
- 单章入口：`Engine::layout_scroll`、`content_height`、`bitmap_window`；
  C ABI：`eb_plain_layout_scroll`、`eb_plain_content_height`、
  `eb_plain_page_slice_raw`。

#### 4.7.1 连续滚动会话（跨章）

单章 API 只解决了“这一章怎么画”；跨章滚动还要章节起点、全书总高、跨章命中
和“目录/链接该滚到哪”。这些由引擎算，宿主不再自己维护章节 top 表：

```rust
// ebook-reader
engine.scroll_begin(viewport, style)?;      // 上下留白会被置 0
engine.scroll_chapter(i)?;                  // { spine, top, height, ok }，懒量 0..=i
engine.scroll_chapters()?;                  // 全书表，首次会排完全书
engine.scroll_total_height()?;              // 滚动容器 contentSize
engine.scroll_spine_at(y)?;                 // y → (章, 章首)
engine.scroll_slice(y, height, scale, ink, paper, bold)?;  // 可跨章的一段
engine.scroll_hit(x, y)?;                   // → { spine, line, byte, y }
engine.scroll_line_y(spine, line)?;         // 行 → 全书流坐标
engine.scroll_anchor_y(spine, "#frag")?;    // 目录/链接 → 全书流坐标
engine.scroll_progress(y)?;                 // 流坐标 → 0..1
```

- 高度是**懒**的：`scroll_chapter(i)` 只保证 `0..=i` 量过，可以分摊到多帧；
  `scroll_total_height()` / `scroll_chapters()` 才量全书。
- 一章排不出来不会拖垮整本书：该章 `height == 0`、`ok == false`，后面的章照排。
- 章节高按视口高向上取整：短章（题记、单段小节）也占满一屏，章尾是纸色，
  下一章从整屏边界开始。
- `scroll_slice` 自己找出涉及的章、分别光栅化再拼到同一张位图，章缝不会错行；
  内容之外是纸色。起点会吸到设备像素栅格。
- 章节 IR 有 LRU（当前章 + 3 章）；字体换了（`register_font`）后旧高度失效，
  `scroll_*` 会返回 `NoScroll`，宿主重新 `scroll_begin` 即可。
- C ABI：`eb_plain_scroll_begin` / `_chapter_count` / `_chapter` /
  `_total_height` / `_spine_at` / `_chapter_top` / `_band_raw` / `_band_bytes` /
  `_hit` / `_line_y` / `_anchor_y` / `_progress`；协议面是 `Request::Scroll*`
  （`ScrollBand` 直接回 `Response::Bitmap`，不需要落盘）。
- 并发与分页相同：一个 `Engine` 是 `&mut`，多个引擎可以同时排版/取瓦片；
  没有全局状态。取消一枚瓦片就是丢掉结果——修掉 O(章高) 之后它本来就是毫秒级。

### 4.8 `LayoutCache`（`ebook-cache`）

```rust
pub struct LayoutCache { /* private */ }
impl LayoutCache {
    pub fn new(capacity: usize) -> Self;
    pub fn with_policy_rev(self, rev: u32) -> Self;
    pub fn set_policy_rev(&mut self, rev: u32);
    pub fn set_fonts(&mut self, fonts: &FontSet);
    pub fn layout(&mut self, doc: &StyledDoc, env: &LayoutEnv<'_>,
                  c: &PageConstraint) -> &LayoutResult;
    pub fn stats(&self) -> CacheStats;   // hits / misses / evictions
    pub fn len / is_empty / capacity / clear
}
```

- 有界 LRU（`VecDeque + HashMap`），缓存完全在 IR 之外：`LayoutResult` 没有缓存字段。
- 键是**输入指纹**：文档结构 + 每个 `ComputedStyle` 字段 + 文本字节 + 图片元数据 +
  字体身份 + `PageConstraint` + 调用方 `policy_rev`。同一个输入永远得到同一个键，
  所以没有失效逻辑。
- `set_fonts` 把字体身份显式交给缓存：`LayoutEnv` 里只有 `TextMeasurer`，拿不到
  `FontSet`。忘了登记就意味着换字体不会使旧条目失效。
- `layout` 固定用默认 `Limits`（没有 `opts` 变体）；需要预算时先用
  `fingerprint(...)` + 自己持有结果。
- 缓存是 caller-owned：没有全局单例、没有 `thread_local`。

### 4.9 `Reader` / `Selection` / `SelectionRect`

```rust
pub struct Reader<'a> { /* &LayoutResult */ }
impl<'a> Reader<'a> {
    pub fn new(ir: &'a LayoutResult) -> Self;
    pub fn ir(&self) -> &'a LayoutResult;
    pub fn hit(&self, page: u32, p: PointF) -> Option<Location>;
    pub fn line_of(&self, loc: Location) -> Option<&LineBox>;
    pub fn page_of_line(&self, line: u32) -> Option<u32>;
    pub fn progress(&self, loc: Location) -> f32;
    pub fn page_progress(&self, page: u32) -> f32;
    pub fn anchor_at(&self, page: u32, p: PointF) -> Option<Anchor>;
    pub fn locate(&self, anchor: &Anchor) -> Option<Location>;
    pub fn link_at(&self, page: u32, p: PointF) -> Option<&str>;
    pub fn clamp(&self, loc: Location) -> Location;
    pub fn selection(&self, a: Location, b: Location) -> Selection;
    pub fn selection_rects(&self, sel: &Selection) -> Vec<SelectionRect>;
    pub fn selection_text(&self, doc: &StyledDoc, sel: &Selection) -> String;
}
```

`Location { page, line, byte }`：

- `line` 是**全局行号**（`runs.lines` 下标），不是页内行号；`page` 由行号反查，
  保证三元组自洽。两者不一致时以行号为准。
- `byte` 是行内 `LineBox.src` 的字节偏移，落在 UTF-8 字符边界上（选区会在
  该行范围内向外吸附）。
- `Location::default()` 是 `(0, 0, 0)`；空文档也返回它。

坐标：`hit` / `link_at` / `anchor_at` / `selection_rects` 里的 `PointF`、`RectF`
都是**页内坐标**（`Page` 左上角为原点）。IR 里的 op/行盒是排版流坐标，`Reader`
内部做一次与 `paint` 互逆的平移。

`Selection { start, end }` 可以反向；`normalize()` 按 `(line, byte)` 排序，
`is_empty()` 只看两端是否相同。`SelectionRect { page, rect }` 按页返回，同一页
可能有多个矩形（跨行/跨段）。RTL 行的矩形按视觉顺序（右到左）返回，`byte`
始终是逻辑序。

`progress` 用“行号 + 行内字节占比”，不用像素：同一内容在任意页面尺寸下进度相同，
页边界连续且单调不减。

### 4.10 `Hyphenator`

```rust
pub trait Hyphenator {
    fn breaks(&self, word: &str, out: &mut Vec<u16>);
    fn name(&self) -> &str;
}

#[cfg(feature = "hyphen-en")]
pub struct English; // ushyphmax（hyph-utf8）的 Liang 算法实现
```

排版层只定义接口，词典由宿主通过 `TextMeasurer::hyphenator()` 注入：

- `breaks` 把允许插入连字符的**字节偏移**写进 `out`（升序、在 `(0, word.len())`），
  非字母词得到空 `out`。用 `&mut Vec` 而不是返回切片，是为了保持 `&self` 与
  “无内部可变性”的约束。
- `hyphens: auto` + `Maybe`（`hyphens` 为真）且词典非 `None` 时，断行才能在词内
  下刀；没有词典时行为与 `hyphens: none` 相同。
- 换词典会改分页，所以它**不是**排版的默认政策：内置 `English` 也要宿主显式
  返回 `Some(&english)` 才生效（见 `crates/ebook-layout/tests/hyphen.rs`）。
  `English` 与 `en.pat` 还受 `ebook-layout` 的 `hyphen-en` feature 控制，默认不编译；
  `ebook-font` 需要时用 `ebook-font/hyphen-en` 打开。
- `name()` 用于缓存键与诊断；改词典必须改这个名字，否则缓存的旧分页会继续命中。

### 4.11 `Op::Link` 与 `Location` 的分工

| 问题 | 用什么 | 产出方 |
|---|---|---|
| 这一点在链接里吗？ | `Op::Link { rect, href }` + `Reader::link_at` | 排版层按行发射 |
| 这一点对应原文哪里？ | `Reader::hit` → `Location` | 排版层从字形与行盒反算 |
| 激活链接跳到哪？ | `href` 原文（宿主拼基址/解析 CFI） | 格式层原样保留 |
| 把位置写进书签？ | `Location` + `Anchor` | `Reader::locate` / `anchor_at` |

链接不参与分页几何：`Op::Link` 不占宽度、不推高行盒，只是行盒上的一个热区。
`Location` 也不含 href，它只描述“第几行、行内第几字节”。两者能互相定位，
因为 `Op::Link.rect` 与 `LineBox.rect` 同形。

---

## 5. `ebook-format`：格式适配器

### 5.1 `ByteSource`

```rust
trait ByteSource {
    fn read(&self, name: &str) -> Option<Vec<u8>>;
    fn names(&self) -> Vec<&str>;
    fn document(&self) -> Option<Vec<u8>>; // 单文件格式用
    fn first_name(&self) -> Option<&str>;
}
```

`MemorySource` 是测试与 CLI 用的实现。`document()` 只有在恰好一个文件时返回内容，
EPUB 适配器靠它判断“整个输入就是一个 epub 字节流”。

### 5.2 打开与读章

| API | 语义 |
|---|---|
| `adapters()` | 内置 `txt`、`epub` 两个适配器 |
| `open(src)` | 按 `Probe::Yes` → `Maybe` 选择；都不匹配返回 `Unsupported("no adapter")` |
| `Opened::chapter(src, index)` | 读第 `index` 章；越界 `OutOfRange` |
| `Opened::len()` / `is_empty()` | spine 长度 |
| `txt::TxtReader::from_text(text, name)` | 已解码 UTF-8 文本按标题行分章；章内标题行包成 `H1`，其余每个非空行单独包成 `P`；超长按行/字符边界兜底 |
| `TxtReader::chapter_with_anchors(index)` | 逐章 `StyledDoc`，锚点 spine 已改写为本章 |

`Probe`：`No`（明确不支持）、`Maybe`（可能是）、`Yes`（确认）。
EPUB 先看 `PK\x03\x04`，再确认 `mimetype` 是 `application/epub+zip`。

### 5.3 `FormatError`

| 变体 | 语义 | 调用方处理 |
|---|---|---|
| `Unsupported(&'static str)` | 没有适配器接受输入 | 提示格式不支持 |
| `Missing(&'static str)` | 容器里缺必需文件/章节 | 提示文件损坏或选错书 |
| `Malformed(&'static str)` | ZIP / OPF / XHTML / 编码错误 | 提示解析失败 |
| `OutOfRange` | spine 下标越界 | 夹到有效范围或提示章节不存在 |

这些错误都不含底层库的错误对象，保持 `no_std` 与可序列化日志的简洁性。

---

## 6. `ebook-font`：字体是入参

`FontBook` 不是全局单例，由调用方创建并传入 `LayoutEnv`。

| API | 语义 |
|---|---|
| `add(bytes)` | 解析失败返回 `None`；成功分配 `FontId` |
| `add_face(desc, bytes)` | 带 family / weight / style 注册 |
| `add_styled(family, weight, style, bytes)` | 便捷注册 |
| `len()` / `is_empty()` | 已注册 face 数 |
| `desc(id)` | 返回原始描述；合成字体返回基础 face 的描述 |
| `select(spec)` | 字体栈 → family 匹配 → 权重/样式最近 → 合成 |
| `select_char(spec, ch)` | 优先选覆盖该字符的 face |
| `base_font(id)` | 去掉合成标记后的基础 `FontId` |
| `is_synthetic*` | 是否合成了粗体/斜体 |
| `rasterize(id, gid, px)` | 失败返回 `None`（缺字、无效尺寸、位图越界） |

`FontId` 的位布局是 IR 的一部分：

```
bit 15     合成标记
bit 12..14 合成粗体/斜体
bit 0..11  基础 face 下标
```

超过 4095 个 face 时不再合成，直接返回基础 id。`FontId::NONE` 是 `u16::MAX`。

`FontBook` 同时实现 `TextMeasurer`（排版度量）与 `Resources`（光栅化），
同一份字体字节既量又画，避免 native 与 canvas 两套度量漂移。

---

## 7. `ebook-render` / `ebook-raster`：只读 IR

### 7.1 `Renderer` 与 `Surface`

```rust
trait Renderer {
    fn render(&mut self, ir: &LayoutResult, page: u32,
              surface: &mut dyn Surface, res: &mut dyn Resources)
        -> Result<(), RenderError>;
}
```

入参是不可变引用，类型上禁止改坐标。`paint()` 是共同实现：它只在这里把
**排版流坐标**平移成页面坐标。`paint_window(ir, page, top, surface, res)` 是它的
窗口版：把流坐标 `top` 画到 surface 的 y=0，surface 只占瓦片高，供滚动模式用；
它按 `surface.size().h / surface.scale()` 裁掉窗口外的 op（字形连光栅查询都不做）。

| `Surface` 方法 | 调用方保证 |
|---|---|
| `glyph_run(run, glyphs, res)` | `glyphs` 已按 `run.glyphs` 切好 |
| `glyph_run_shifted(run, glyphs, shift, res)` | 同上，且 `shift` 是加在 `run.origin` 上的平移；热路径后端应覆盖以避免复制 `GlyphRun.text` |
| `rect(rect, color)` | 坐标已平移到页面内 |
| `border(border)` | 同上 |
| `image(id, rect, fit, res)` | 同上；尺寸未知时 IR 已给占位盒 |
| `unresolved(u)` | 同上 |
| `push_clip(rect)` / `pop_clip()` | 嵌套顺序由 paint 维护 |
| `finish()` | 一页 / 一次 `paint_window` 画完时调用一次；默认空实现。后端如果把字形先累积成覆盖率再合成，就在这一步落进像素 —— `paint` 返回之后读像素的宿主不必自己收尾 |
| `scale()` | 默认 1.0；装饰型 surface（先乘比例再转给后端）覆盖它，`paint_window` 靠它把 surface 高换算成流坐标 |

`RenderError`：

| 变体 | 语义 |
|---|---|
| `NoSuchPage(u32)` | 页号不存在 |
| `EmptyDocument` | 保留给“没有任何页”的显式后端；当前 `paint` 返回 `NoSuchPage` |

### 7.2 后端

| 后端 | 类型 | 输出 |
|---|---|---|
| 可读预览 | `SvgRenderer` + `SvgSurface` | 可直接看的 SVG；文本用 `GlyphRun.text`，不复现 kerning/bidi/几何，几何校验以 RGBA 为准 |
| 光栅 | `RgbaSurface` | RGBA8，整数混合，跨平台逐字节一致 |
| Canvas2D | `web/reader.js` | 浏览器阅读器，文本可选中；wasm 在 `web/engine-worker.js` 里 |

`RgbaSurface::to_ppm()` 输出 P6 PPM，`to_rgba()` 输出裸 RGBA8；
`render_page()` 会先清成白色再画。`RgbaSurface::from_page(page)` 用
`page.box_size()` 决定画布大小。

字形有两种画法，默认即时混合：

| 模式 | 开关 | 语义 | 实测（480×640 整页） |
|---|---|---|---|
| 即时（默认） | `set_glyph_batching(false)` | 每个字形画完立刻混合；同色重叠会混合两次 | 0.49 ms |
| 批处理 | `set_glyph_batching(true)` | 同色字形先累积成 1 字节/像素覆盖率，换色或遇到非字形 op 时一次合成；重叠取 `max`，**结果与绘制顺序无关** | 0.64 ms |

默认关闭是测量结论：正文的墨稀疏且几乎不重叠，批处理多一遍目标缓冲往返，
反而慢 30%。打开它换的是顺序无关性（`max` 幂等且交换），这是按瓦片/按任务
并行光栅或换 GPU 后端的前提。两种模式的语义都有测试锁定：
`crates/ebook-raster/tests/render.rs` 的 `same_color_overlap_composites_once` /
`same_color_batching_is_order_independent`。

DPR 不进入 IR：`ScaledSurface` 装饰任何 `Surface`，把坐标、边框宽度、字形
advance 与**字号**一起乘 `scale`，因此字形按设备像素光栅化，而不是画完再放大。
`scale = 1` 与直接绘制逐字节相同（`crates/ebook-raster/tests/scale.rs`）。
`eb_page_bitmap` 就是“`ScaledSurface` + `RgbaSurface` + `paint`”这三件套。

线程模型：wasm 实例只创建在 worker 里（`web/engine-worker.js`），主线程通过
`web/engine-client.js` 发 RPC。开书/注册字体/排版/光栅/查询都在 worker 完成，
回来的只有页元数据（`eb_page_meta`）、位图（`eb_page_bitmap`，`ArrayBuffer`
`transfer`）与查询结果；IR 字节不过线程边界，主线程也不 import wasm。

---

## 8. `ebook-web`：wasm 导出

| 导出 | feature | 语义 |
|---|---|---|
| `eb_layout(text, w, h, margin)` | 默认 | 纯文本排版，返回 IR 字节 |
| `eb_layout_text` | `epub` | 同上，命名避开同名导出 |
| `eb_handle(bytes)` | 任意 | postcard 请求/响应（`Ping` / JSON 之外的二进制协议） |
| `eb_open(bytes)` | `epub` | 打开 EPUB，返回书目 JSON 或 `{"error":...}` |
| `eb_open_text(text, name)` | `epub` | 打开已解码的 TXT；按标题行分章，`name` 用于书名 |
| `eb_open_file(bytes, name)` | `epub` | 按 ZIP 魔数自动选择 EPUB / TXT |
| `eb_close_book()` | `epub` | 关掉当前书并回收它的字节；`eb_open*` 也会在开头自动释放上一本 |
| `eb_layout_chapter(bytes, name, spine, w, h, margin)` | `epub` | 无缓存地排版一章 |
| `eb_layout(spine, w, h, margin)` | `epub` | 排版已打开书籍的指定章 |
| `eb_layout_insets(spine, w, h, left, right, top, bottom, font_scale)` | `epub` | 四边独立页边距 + 只缩放字号/行高；页面容器不变 |
| `eb_layout_style(spine, w, h, left, right, top, bottom, font_scale, line_scale, letter_px, align)` | `epub` | 在四边页边距上再独立覆盖行距/字距/对齐；`align` 0=原书、1=左、2=中、3=右、4=两端 |
| `eb_page_meta()` | `epub` | 页级元数据：`[count:u32]` + 每页 9 个 f32（`w,h,left,right,top,bottom,line_start,line_len,progress`）；宿主不解析 IR 就能翻页/画进度 |
| `eb_fragment_line(fragment)` | `epub` | 当前章 `#fragment` 的行号，`u32::MAX` = 本章没有这个锚点 |
| `eb_base_font_size()` | `epub` | 当前书首章根节点字号；宿主用它把 px 字号换算成缩放比例 |
| `eb_font(bytes)` | `epub` | 注册字体，返回 id；失败 `u32::MAX` |
| `eb_font_reset()` / `eb_font_count()` | `epub` | 字体槽管理 |
| `eb_page_bitmap(page, scale, ink, paper, bold)` | `epub` | 引擎整页光栅：8 字节头（`w:u32, h:u32`）+ `w*h*4` RGBA；`scale` 是 DPR，`ink`/`paper` 是 `0xRRGGBBAA`（`0` = 不修正/白纸）；`ink` 只替换默认字色（作者显式 `color` 保留），`bold` 是全局加粗 |
| `eb_hit(page, x, y)` | 任意 | 页内坐标命中：`{"location":{...}|null}` |
| `eb_link_at(page, x, y)` | 任意 | 页内坐标上的链接：`{"href":"..."|"null"}` |
| `eb_selection(a_page,a_line,a_byte,b_page,b_line,b_byte)` | 任意 | 选区：`{start,end,rects:[{page,rect}],text}` |
| `eb_progress(page, line, byte)` | 任意 | 全书进度：`{"progress":0..1}` |
| `eb_toc()` | 任意 | 目录：`[{label,spine,fragment,depth}]`，`spine` 可为 `null` |
| `eb_resolve_link(href)` | 任意 | 在当前章上下文里解析书内链接：`{target:{spine,fragment,external}}` |
| `eb_version()` | 任意 | `(major << 16) | minor` |

`eb_hit` / `eb_link_at` / `eb_selection` / `eb_progress` 都作用在**最近一次**
`eb_layout*` 产生的 IR 上（宿主需先排版再查询），坐标是页内坐标；返回值是
JSON 字符串，字段与 `ebook_ir::Location` / `SelectionRect` 一一对应。没有加载过
IR 或页号越界时返回 `null` 字段，不 panic。

JS 侧解码器是手写的，IR 版本、枚举 tag、字段顺序都见本文第 2、3 节。
`web/pkg` 是构建产物，不提交；`web/build.sh` 与 `xtask app` 都会生成它。

---

## 9. `ebook-abi`：C ABI

头文件是 `crates/ebook-abi/include/ebook_abi.h`，release 资产（每个 target 一个
`libebook_abi-<target>.tar.gz`，里面是静态库 + 这个头文件）也带它。
`cargo test -p ebook-abi` 会把头文件用 `-Wall -Wextra -Werror` 真编译一遍，
并用 `_Static_assert` 对齐 Rust 侧的 `size_of`；头文件与 `pub extern "C" fn`
的符号集合必须完全相等。

### 9.1 两个面

| 面 | 入口 | 适合 |
|---|---|---|
| postcard | `eb_open` / `eb_request` / `eb_close` | 能读返回缓冲区的宿主：cgo、Swift、Rust |
| 标量 | `eb_plain_*` | 只能拿 `Pointer` 不读内存的宿主：Asuka 的 `extern`、JNI 薄包装 |

- postcard 面的 `Request` / `Response` 枚举与 wasm 侧同思路，schema 就是
  `crates/ebook-abi/src/protocol.rs`；新增能力只加枚举变体，不往 C 面加函数。
- 标量面每条能力一个函数，参数与结果都是标量/字符串/`repr(C)` 结构体，
  像素与封面由宿主给路径、库写文件。
- 两面共用一个 `Engine`：postcard 面是 `Mutex<Engine>` 的不透明指针，
  标量面是整数句柄 + 进程级句柄表。

### 9.2 所有权与生命周期

| 东西 | 归谁 | 有效期 |
|---|---|---|
| `eb_alloc` / `eb_request` 返回值 | 调用方 | 直到 `eb_free(ptr, len)` |
| 其余 `const char *` | 库 | 见下 |
| `eb_plain_open` 句柄 | 调用方 | 直到 `eb_plain_close` |

- `eb_version` / `eb_plain_version`：静态字符串，进程生命周期。
- `eb_last_error` / `eb_plain_last_error`：库内缓冲，直到**下一次失败的调用**
  （原地复用，连续失败不换地址，也不泄漏）。
- `eb_plain_*` 其余字符串：进程级环形缓冲（最近 64 条），直到后续 64 次取字符串。
  要长期保存就当场 `strdup`。
- 无效句柄不返回 `NULL` 而是空串/`0`/`ok == 0` 的结构体；关掉的句柄同理。
  `NULL` 只表示“参数错”（例如空的路径）。

### 9.3 版本与布局

- `EB_ABI_VERSION`（头文件）必须等于 `eb_abi_version()` / `eb_plain_abi_version()`。
  任何 `repr(C)` 结构体加字段、改字段顺序、改既有函数参数都要 +1；
  只加新函数不算破坏性变更。
- `eb_struct_size(kind)` 返回 `EbPageInfo` / `EbLoc` / `EbSelectionRect` /
  `EbLink` 的字节数（kind 1..4），宿主可用来二次校验头文件与库同源。
- 库版本串由 `CARGO_PKG_VERSION` 编进来（不再手写）。

### 9.4 位图落盘格式（`EQP1`）

```text
"EQP1" | u32le width | u32le height | width * height * 4 bytes RGBA8
```

行优先、直通 alpha（非预乘）。位图覆盖整个视口（含留白），不是内容盒。
写文件是原子的：先写同目录的 `{path}.{pid}.{seq}.part`，再 `rename` 到目标
路径；宿主用「文件存在」判断可用时不会读到半张位图。
`eb_plain_page_raw` 出一页；`eb_plain_page_slice_raw` 出滚动流的一片
（`y`/`height` 是点，`y` 在 `[0, eb_plain_content_height)` 里，同一 `y`
逐字节可复现）。`eb_plain_scroll_band_raw` 出**跨章**的一段（`y` 在
`[0, eb_plain_scroll_total_height)` 里），`eb_plain_scroll_band_bytes` 是它的
内存版（`eb_free` 释放）——能读内存的宿主不必为瓦片落盘。平台侧只读它：iOS 包成 `CGImage`，GTK 包成 `GdkMemoryTexture`。

### 9.5 句柄上的两份状态

标量面只能返回标量/结构体，所以两样东西留在句柄上：

- 最近一次选区（`eb_plain_select` → `eb_plain_selection_rect` / `_text` /
  `_start` / `_end`）。矩形数组按 `0..eb_plain_selection_rect_count` 下标取。
- 最近一次链接解析的 fragment（`eb_plain_resolve_link` →
  `eb_plain_resolved_fragment`）。

两者都在关书时丢掉，下一次同类调用覆盖。`ok == 0` 是不存在，不是错误。

### 9.6 错误

所有入口都 `catch_unwind`：panic 变成 `Response::Error` 或负返回值 + 错误串，
不会让宿主 abort。这条依赖 `native` profile 的 `panic = "unwind"`
（`Cargo.toml` 里已经这么配，`release` 的 `panic = "abort"` 只给 wasm/CLI 用）。

标量面的返回码：`0` 成功 / 继续；`-1` 句柄或参数错；`-2` 引擎侧失败
（排版、光栅化、字体解析、上限）；`-3` 写文件失败。
