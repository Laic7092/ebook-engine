# API 契约

本文只写**对外承诺**，不写实现细节。三份契约（`StyledDoc`、`LayoutResult`、
`Renderer`）的任何形状变化都属于破坏性变更；其余 crate 的公开函数可以增删，
但下面列出的语义（越界返回什么、空输入产出什么、错误如何分类）保持稳定。

- `ebook-ir`：数据结构、IR 编解码、渲染 trait
- `ebook-layout`：纯排版入口、字体度量与图片尺寸注入
- `ebook-format`：字节源、格式适配器、章节读取
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
├── version: IrVersion       // 1.1
├── spine: u32               // 与 StyledDoc.meta.spine 一致
├── pages: Vec<Page>
├── display: DisplayList { ops: Vec<Op>, glyphs: Vec<Glyph> }
├── runs: RunMap { lines: Vec<LineBox> }
└── dirty: Vec<DirtyBlock>
```

| 结构 | 约定 |
|---|---|
| `Page.ops` / `Page.lines` | 指向 `display.ops` / `runs.lines` 的连续切片 |
| `Page.origin` | 该页在**排版流**里的起点；只移动它，不改 op 坐标 |
| `Page.size` | 内容区尺寸（不含 `inset`） |
| `Page.inset` | 页边距；`box_size()` = 内容尺寸 + 四边 |
| `Page.anchor` | 页首锚点，可能为空 |
| `GlyphRun.glyphs` | 指向 `display.glyphs` 的连续切片 |
| `LineBox.src` | 对应 `StyledDoc.text` 的源区间，用于选区 |
| `DirtyBlock` | 需要宿主重排/换字体的块，`reason` 同下 |

页是按顺序切出来的，不是过滤结果。正常排版下：

- `pages[i].index == i`
- `pages[0].ops.start == 0`，后一页的 `start` 等于前一页的 `start + len`
- 所有页的 op 并集恰好覆盖 `display.ops`，行同理
- 每页至少有一行；空文档可以没有页

### 2.3 问题码表（IssueCode / `UnresolvedReason`）

当前版本没有单独的 `IssueCode` 枚举。IR 用 `UnresolvedReason` 同时标记 op
与脏块，它承担的就是 IssueCode 的角色；其变体顺序就是 postcard/JS 解码的 tag：

| tag | 变体 | 含义 | 宿主应该做什么 |
|---:|---|---|---|
| 0 | `ImagePending` | 图片固有尺寸未知，先按占位盒排版 | 图片到达后带尺寸重排；不要由渲染层猜 |
| 1 | `FontMissing` | 字体不覆盖该字符（`glyph_id == 0`） | 换字体、回退字体或标记缺字 |
| 2 | `SizeFromContent` | 尺寸依赖内容，当前无法结算 | 提供尺寸后重排 |

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

当前 `IR_VERSION = 1.1`。

- **major 表示字节布局。** 增删字段、改字段顺序、增删/重排枚举变体、改整数宽度，
  都必须升 major。旧解码器遇到不同 major 直接拒绝。
- **minor 表示布局不变、语义澄清或文档修正。** `read_header` 不因 minor 不同而
  拒绝。由于 postcard 是位置编码而不是自描述格式，任何真实字段变化都不能只升
  minor。
- **JS 侧是手写解码器。** 改 `crates/ebook-ir` 的结构体必须同步改
  `web/decode.js`、`web/encode.js`，并让 `xtask contract` 通过。contract 的做法
  是把 golden IR 解出来再编回去，要求逐字节还原。
- wasm 导出 `eb_version()` 返回 `(major << 16) | minor`，宿主可以据此提示不兼容。

---

## 4. `ebook-layout`：纯排版

### 4.1 入口

```rust
pub fn layout(doc: &StyledDoc, env: &LayoutEnv<'_>, c: &PageConstraint) -> LayoutResult
```

`Layout::new().layout(...)` 等价。没有 I/O、没有全局状态、没有时钟与随机数。
同一个 `(doc, env, c)` 必须逐字节产出同一个 `LayoutResult`。

### 4.2 `TextMeasurer`

| 方法 | 默认实现 | 契约 |
|---|---|---|
| `advance(ch, font, size)` | 无 | 返回逻辑宽度，单位 px |
| `ascent(font, size)` | 无 | 基线以上高度 |
| `descent(font, size)` | 无 | 基线以下高度 |
| `x_height(font, size)` | `0.5 * size` | `vertical-align: middle` 用 |
| `covers(ch, font)` | `true` | 字体是否覆盖字符 |
| `glyph_id(ch, font)` | Unicode 低 16 位，超出用 `0xFFFD` | 缺字返回 0 |
| `select(spec)` | `spec.font` | 按字体栈选主字体 |
| `select_char(spec, ch)` | `select(spec)` | 按字符选能覆盖的字体 |

`FontBook` 实现了真实版本；`StubMeasurer` 只用于测试与确定性 golden。

### 4.3 `ResourceView`

```rust
fn image_size(&self, id: ImageId) -> Option<SizeF>;
```

返回 `None` 表示图片尺寸未知。排版层不会等待加载，而是按占位尺寸排出
`Op::Unresolved{ImagePending}` 与 `DirtyBlock`。`NoImageSizes` 永远返回 `None`。

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
**排版流坐标**平移成页面坐标。

| `Surface` 方法 | 调用方保证 |
|---|---|
| `glyph_run(run, glyphs, res)` | `glyphs` 已按 `run.glyphs` 切好 |
| `rect(rect, color)` | 坐标已平移到页面内 |
| `border(border)` | 同上 |
| `image(id, rect, fit, res)` | 同上；尺寸未知时 IR 已给占位盒 |
| `unresolved(u)` | 同上 |
| `push_clip(rect)` / `pop_clip()` | 嵌套顺序由 paint 维护 |

`RenderError`：

| 变体 | 语义 |
|---|---|
| `NoSuchPage(u32)` | 页号不存在 |
| `EmptyDocument` | 保留给“没有任何页”的显式后端；当前 `paint` 返回 `NoSuchPage` |

### 7.2 后端

| 后端 | 类型 | 输出 |
|---|---|---|
| 参考 | `SvgRenderer` + `SvgSurface` | 可直接看的 SVG；文本用 `GlyphRun.text` |
| 光栅 | `RgbaSurface` | RGBA8，整数混合，跨平台逐字节一致 |
| Canvas2D | `web/app.js` | 浏览器阅读器，文本可选中 |

`RgbaSurface::to_ppm()` 输出 P6 PPM；`render_page()` 会先清成白色再画。
`RgbaSurface::from_page(page)` 用 `page.box_size()` 决定画布大小。

---

## 8. `ebook-web`：wasm 导出

| 导出 | feature | 语义 |
|---|---|---|
| `eb_layout(text, w, h, margin)` | 默认 | 纯文本排版，返回 IR 字节 |
| `eb_layout_text` | `epub` | 同上，命名避开同名导出 |
| `eb_handle(bytes)` | 任意 | postcard 请求/响应（`Ping` / JSON 之外的二进制协议） |
| `eb_open(bytes)` | `epub` | 打开 EPUB，返回书目 JSON 或 `{"error":...}` |
| `eb_layout_chapter(bytes, name, spine, w, h, margin)` | `epub` | 无缓存地排版一章 |
| `eb_layout(spine, w, h, margin)` | `epub` | 排版已打开书籍的指定章 |
| `eb_font(bytes)` | `epub` | 注册字体，返回 id；失败 `u32::MAX` |
| `eb_font_reset()` / `eb_font_count()` | `epub` | 字体槽管理 |
| `eb_font_desc(id)` | `epub` | 返回给 canvas 的字体描述 JSON |
| `eb_version()` | 任意 | `(major << 16) | minor` |

JS 侧解码器是手写的，IR 版本、枚举 tag、字段顺序都见本文第 2、3 节。
`web/pkg` 是构建产物，不提交；`web/build.sh` 与 `xtask app` 都会生成它。
