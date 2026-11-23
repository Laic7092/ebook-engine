# 设计

README 说“只有排版是核心”。本文把这个论证展开：为什么边界要这样切、
哪些不变量必须成立、以及这些选择换来了什么可测试性。

---

## 1. 分层与依赖方向

```
ebook-format ──> ebook-ir <── ebook-layout <── ebook-font
                       ^
                       └── ebook-render <── ebook-raster
                       └── ebook-web / ebook-cli
```

依赖只指向 `ebook-ir`，没有反向调用：

- `format` 解容器、解析 XHTML/CSS、做级联，产出**已计算样式**的 `StyledDoc`。
- `layout` 只认 `StyledDoc`、`LayoutEnv`、`PageConstraint`，产出 `LayoutResult`。
- `render` 只认 `&LayoutResult`，把 op 画到 `Surface`。
- `font` 是入参：`FontBook` 同时提供 `TextMeasurer` 与 `Resources`。
- `web` / `cli` 是组装层：读字节、选适配器、注入字体、调用排版与渲染。

没有 trait object 反向调用排版层。`Surface` 是渲染层的出口，`TextMeasurer`
与 `ResourceView` 是排版层的入口；两者都由组装层实现或注入。

### 为什么级联在格式层

排版只需要“计算后的值”。把选择器、继承、`!important`、层叠顺序都留在格式层，
排版层就永远看不到 CSS 语法，也不会在排版过程中重新求值样式。代价是
`StyledDoc` 必须携带完整的 `ComputedStyle`；回报是排版输入可以被手工构造，
随机文档测试不需要真实 HTML。

### 为什么字体在格式层之外、排版之内作为入参

字体 fallback 链因平台而异。一旦排版层自己去“找字体”，同一本书在不同平台
会量出不同行宽，分页、书签、跨端同步全部失效。这是正确性问题，不是性能问题。

因此：

- `FontBook` 由宿主创建并注入。
- 排版层只调用 `TextMeasurer`，不读字体文件、不写缓存。
- native 与 wasm 用同一份字体字节，度量与光栅化来自同一个 `FontBook`。

---

## 2. 不变量清单

以下不变量由 `crates/ebook-layout/tests/{invariants,properties,text_layout,pagination}.rs`
与 `xtask` 的 golden/pixel/parity/contract 共同覆盖。改排版时先看这张表。

### 2.1 页与 op 的覆盖关系

1. `pages[i].index == i`。
2. `pages[0].ops.start == 0`；后一页的 `ops.start` 等于前一页 `start + len`。
3. `pages[0].lines.start == 0`；行切片同样连续。
4. 所有页的 op 并集恰好覆盖 `display.ops`，行并集恰好覆盖 `runs.lines`。
5. 每页至少有一行；空文档可以没有页。
6. `page.lines` 里的每一行都落在 `[origin.y, origin.y + size.h]` 内（容差 1e-3）。
7. 页 `origin.x == 0`；`origin.y` 单调不减。
8. 行按 `rect.y` 单调不减。
9. 行的 `src.start` 单调不减（源文顺序）。

### 2.2 坐标与尺寸

10. 所有 op 坐标是**排版流坐标**，不是页面内坐标。
11. `Page.origin` 只由分页器写；改分页只移动 origin。
12. `Page.size` 是内容区尺寸，`Page.inset` 是页边距；
    `box_size()` = `size + inset`。
13. `content_size` 每个轴至少 `1.0`，极小页面不会除零或产生负尺寸。
14. 行盒不会因为负 margin 跑到页外；越界时宁可裁掉视觉，也不移动分页基线。

### 2.3 确定性与编解码

15. 同一个 `(StyledDoc, LayoutEnv, PageConstraint)` 重复排版逐字节相同。
16. `codec::to_bytes(layout(...))` 重复调用逐字节相同。
17. `decode(to_bytes(ir)) == ir`；再编码仍然是同一串字节。
18. 所有公开集合的遍历顺序固定：`Vec` / 固定数组，禁止 `HashMap` / `HashSet`。
19. 浮点运算顺序固定；不使用并行归约、不依赖迭代器实现细节以外的顺序。

### 2.4 边界与错误

20. `layout` 是全函数：空文档、极小页面、极深嵌套都返回可序列化结果。
21. 越界查询返回 `None`，不 panic：`page`、`page_of_line`、`line_at`、`anchor_at`。
22. span 切片与节点索引是信任调用方：越界 panic 属于调用方违约。
23. 渲染不存在的页返回 `NoSuchPage`，不绘制半页。
24. 缺字产 `FontMissing`，不猜字形；图片尺寸未知产 `ImagePending`，不猜尺寸。
25. `truncated == true` 时 1–14 仍然成立：截断只保留前缀，不留悬空 op/行/字形区间。
26. 只有一行、且该行连同比页内起点偏移都装不进内容区时，允许它溢出页面；
    其它任何情况下行都不越界（`tests/fuzz.rs` 把这条写成了断言）。

> 已知缺陷（1.4）：整行只有行内图片时，`LineBox.src` 从 0 算起（占位字符
> `U+FFFC` 按 3 字节计），不指向真实原文。选区会跳过这类行；修复需要改
> `flow` 的源区间计算。`tests/fuzz.rs` 暂时把 `Image` 行从 src 单调检查里排除。

---

## 3. 为什么 `DisplayList` 禁止 `HashMap` / `HashSet`

IR 是 native 与 wasm 之间的**字节契约**。`postcard` 按字段声明顺序编码
`Vec`，而 `HashMap` 的迭代顺序取决于哈希种子和容量历史：

- 同样内容的两个 `HashMap`，插入顺序不同，序列化字节就可能不同。
- 跨进程、跨平台、跨编译器版本，哈希实现可能不同。
- golden 与 parity 都要求逐字节相同，任何迭代顺序抖动都会变成假失败。
- 缓存键是 IR 字节的哈希；顺序抖动会让缓存永远不命中。

所以 IR 里只允许：

- `Vec`：插入顺序即语义顺序（页、op、行、dirty 块）。
- 固定数组：字体栈、字体名、边的四方向。
- 需要按 key 查找时，用排序后的 `Vec` + 二分，或者直接用下标（`NodeId`）。

这条禁令不是风格问题，是“字节流必须确定”的直接推论。`LayoutResult` /
`DisplayList` 里一旦出现哈希容器，parity 与 golden 就不再是可测事实。

---

## 4. 为什么 op 坐标是排版流坐标

分页会移动内容：今天第 3 页的第一行，改字号后可能落到第 4 页。如果 op 存的是
页面内坐标，每次重排都要重写所有后续 op 的坐标，渲染层与排版层就会出现两套
“页面偏移”逻辑。

流坐标把这两个概念拆开：

- 排版层在一条连续流里排所有内容，op 只记录“在流里的位置”。
- 分页器决定每页从流的哪个 `y` 开始，写进 `Page.origin`。
- 渲染时只做一次平移：`paint()` 把 `op - (origin - inset)` 交给后端。

于是：

- 改分页只改 `Page.origin` 和切片边界，一个 op 都不用重写。
- “渲染层不改坐标”不是纪律，而是数据结构允许它只做一次平移。
- 同一份 IR 可以被任意后端按页渲染，后端不需要知道分页算法。
- 页内坐标只在 `paint()` 里出现一次，SVG / 光栅 / Canvas 不会各自实现一套。

`Page.inset` 也参与这次平移：内容区坐标加上页边距，才落到页面盒坐标。

---

## 5. 分页为什么在排版层

断行决定分页，分页又反过来影响孤行寡行与段落切分。如果把分页放到渲染层：

- 孤行寡行需要回退断点，渲染层必须理解块结构。
- 强制分页会改变后续页的 `origin`，渲染层必须能改排版结果。
- 选区与 CFI 依赖 `RunMap`，页边界必须和行边界一起结算。

当前算法：

1. 把行与块级 op 展平成 `Unit` 序列，每个 unit 有 `y`、`h`、块号、行号、
   `orphans`、`widows`、`break-before/after`。
2. 从当前页顶开始，找最满的断点。
3. `resolve` 按约束往回退；退到段首仍不满足，就整段推到下一页。
4. 约束冲突时退回最满断点。宁可难看，也不产生空页。
5. 页首 `origin.y` 夹到 `上一页 origin + 1.0`，保证页序单调。

分页结果是 `PageStart` 列表；`PageBuilder` 再把连续的 op/行切成 `Span`。
所以页是切片，不是过滤。

---

## 6. native / wasm parity 为什么可测

可测的前提是排版层没有平台相关的隐藏输入：

- 不碰 I/O：字体、图片尺寸、页面约束都是入参。
- 不持全局可变状态：没有字体库单例、没有 `thread_local` 缓存。
- 不取时钟、随机：没有时间戳、没有随机种子。
- 不出现 `#[cfg(target_*)]`：同一份源码编到 native 与 wasm。

在这个前提下，`xtask parity` 用同一份 `fixtures/demo.txt`：

1. native 直接 `layout` 得到 IR 字节。
2. wasm 侧通过 wasm-bindgen 调同一个 `layout` 得到 IR 字节。
3. 比较两串字节。

浮点是 IEEE-754 单精度，postcard 是固定布局，`Vec` 顺序固定，所以只要输入相同，
输出就应当逐字节相同。任何差异都说明有隐藏状态、平台分支或顺序抖动——
这正是这条测试要抓的东西。

`xtask contract` 是另一条独立防线：JS 手写解码器把 golden IR 解出来再编回去，
要求逐字节还原。它抓的是“Rust 结构体改了，JS 忘了改”，与 parity 抓的平台差异
不是同一类问题。

---

## 7. 不确定性必须显式

排版层不能“顺手猜一个值”。当前把不确定性分成三类：

| 不确定性 | IR 表现 | 宿主动作 |
|---|---|---|
| 字体缺字 | `Op::Unresolved{FontMissing}` + `DirtyBlock` | 换字体/回退/标记缺字 |
| 图片尺寸未加载 | 占位盒 + `ImagePending` + `DirtyBlock` | 带尺寸重排 |
| 尺寸依赖内容 | 占位 + `SizeFromContent` | 提供尺寸后重排 |

渲染层看到 `Unresolved` 只画虚线框，不补全内容。这样分页结果不依赖加载时机，
纯函数性成立，书签不会因为图片晚到而跳页。

---

## 8. 渲染层为什么只读

`Renderer::render` 接收 `&LayoutResult`，类型上就拿不到可变 IR。渲染层只能：

- 按页取 `Page.ops` 切片；
- 按 `GlyphRun.glyphs` 取字形切片；
- 通过 `Resources` 取字形位图或图片；
- 把 op 画到 `Surface`。

它不能改坐标、不能重新断行、不能补全未知尺寸。否则就会出现第二套排版逻辑，
golden test 失去意义，跨端一致性也无法证明。

光栅后端的“整数混合”与 SVG 后端的“可读预览”是替代关系，不是备份关系：
同一份 IR 可以用任意后端画，后端之间不共享布局逻辑。SVG 只便于人眼检查，
字形位置直接来自 `GlyphRun.text`，不复现 kerning/bidi，也不作为几何参考——
几何一致性用 RGBA 逐字节比对。

---

## 9. wasm 边界

`ebook-web` 只做三件事：把字节交给格式层、把 IR 字节交给 JS、把字体注册进
`FontBook`。IR 是唯一的数据契约：

- header `EBIR` + major/minor 固定 8 字节；
- body 是 postcard 位置编码；
- JS 解码器手写，字段顺序必须与 Rust 完全一致；
- `web/encode.js` 只用于 contract 的“解出来再编回去”，不参与阅读器主路径。

`web/reader.js` 只按 `GlyphRun.text` 画文本，按 `Page.origin/inset` 平移画布；
它不解释 CSS，也不重新断行。字体描述由 `eb_font_desc` 从 `FontBook` 取出，
交给 canvas 的 `font` 字符串，保证量出来的和画出来的用同一份字体。
`web/app.js` 负责书架/阅读页路由与导入，`web/library.js` 渲染书架，
`web/db.js` 封装 IndexedDB（不可用时退化成内存），TXT 的编码检测在
`web/text.js`，分章仍在 `ebook-format::txt::TxtReader`。界面是移动优先的
响应式布局：阅读页用实际可视区域作为排版约束，手机用底部面板，桌面用侧边抽屉，
移动端工具栏在阅读时自动隐藏，轻点页面显示或收起；工具栏是覆盖层，显隐不会改变排版约束或触发重排。

---

## 10. 两遍式：先断行，再分页

排版层内部只有两个阶段，中间只传一份扁平的 item 流：

```
StyledDoc ──flow──> [Item::Line | Item::Op] ──paginate──> Page[] + Span 切片
             断行                       分页
```

**第一遍（`flow`）：盒树 + 断行。** 这一遍只关心宽度：展开块/行内盒、整形文本、
按 UAX #14 找断点、结算两端对齐与行高，产出行盒（`y` 已经定下）与块级 op。
每个 item 带 `block`、`order`、`len`、`orphans`、`widows`、`force`，供第二遍决定
断点，但这些字段不进 IR。

**第二遍（`paginate`）：页起点 + 切片。** 这一遍只关心高度：把 item 映射成
`Unit`，从当前页顶贪心找最满断点，再按 `break-before/after` 与孤行寡行往回退；
退不动就退回最满断点。结果是一串 `PageStart { unit, y }`，`PageBuilder` 再把连续
的 op/行切成 `Span`。

为什么不合成一遍：

- 宽度与高度的输入是分开的。页面只改高度时，断行结果完全不变，只要重新分页。
- 孤行寡行需要**回退**断点，而回退只能在“已经知道自己有哪些行”之后做；一边断行
  一边分页就把回退变成了改行。
- 第一遍是纯函数，第二遍只需要 item 的 `y/h/block` 摘要。两遍之间没有回调，
  也没有“分页器回头让断行器重排一段”的隐式循环。
- 两遍都结束后才做产出侧限额（`max_pages` / `max_lines`）：截断只能发生在
  “已经有了合法前缀”的位置，否则页覆盖与行不越界就不再成立。

代价是 item 流会多占一份内存（每个 item 一条记录，不含字形与文本本体）。
对于电子书章节这个量级，换来的“改页高不重排文字”与“限额可截断”更值。

---

## 11. 缓存键 = 输入指纹

`ebook-cache` 的键不是“文档 id + 版本号”，而是**把所有排版输入哈希一遍**：

- 文档结构：节点数、每个节点的 `kind` / 全部 `ComputedStyle` 字段 / `parent` /
  `first_child` / `child_len` / 锚点 / 链接、`children` 顺序、`text` 字节、图片元数据、
  `meta`；
- 字体身份：`FontSet` 里每个字体的 family / weight / style / 字节长度 + 前 64 字节；
- 页面约束：`page` / `margin` / `writing_mode`；
- 调用方的 `policy_rev`。

这个选择让缓存**没有失效逻辑**：任何一个会改变排版结果的字节变了，键就变了，
旧条目自然不再命中。反过来，只要键相同，输出就必须逐字节相同——这正是
[§2.3 确定性](design.md#23-确定性与编解码)那条不变量的另一面。

几个细节是刻意的：

- 所有 f32 按 `to_bits` 参与哈希，`-0.0` 与 `0.0` 不会被当成同一个值。
- 字体只取前 64 字节：足够区分不同字体文件，又不会为了键把几十 MB 重新哈希。
- 哈希是 FNV-1a。缓存是 caller-owned，键不落盘、不跨信任边界；这里不需要
  密码学哈希，碰撞的后果只是一次错误命中。
- 缓存键与缓存条目都不进 `LayoutResult`：IR 是 native/wasm 的字节契约，一旦
  带上缓存状态，golden 与 parity 就会依赖进程历史。
- 字体身份由调用方 `set_fonts` 显式登记，因为 `LayoutEnv` 只暴露 `TextMeasurer`。
  这是当前形状下的取舍：不改 `LayoutEnv` 的契约，代价是调用方要多一步。

---

## 12. 并发只在一次调用内

`Layout::layout_parallel(doc, env, c, workers, opts)` 的并行范围只有**一次调用**：

- `std::thread::scope` 在调用内建线程，任务完成后 join，返回前所有线程都已结束。
- 没有线程池、没有全局队列、没有 `OnceLock` / `lazy_static` / `thread_local`。
- 每个任务只写自己的结果槽；拼回按输入下标，不按完成顺序。
- `workers` 只是本次调用的上限，不是全局配置。
- wasm 关掉 `parallel` feature，`layout_parallel` 直接走串行路径，签名不变，
  输出逐字节相同。

这四条合起来保证：并行结果与串行结果**逐字节相同**。如果并行改变输出，那就说明
有共享状态或非确定顺序，parity/golden 会立刻抓到。

当前并行的是“item → 分页单元”的纯函数映射，不是段落断行本身。真正的段落级
并行需要先在 `flow` 里把断行拆成可独立的任务（计划/断行/回填三段），这属于
排版内核的改动，不在硬化轮次里。

---

## 13. IR 1.2–1.4 的字段为什么这么放

IR 是位置编码，字段顺序就是字节布局。1.2–1.4 的新字段都遵守同一条规则：
**只在结构体末尾追加**，让旧快照只多出一个后缀，而不是整段错位。

| 字段 | 放在哪 | 为什么不是别的放法 |
|---|---|---|
| `Glyph.ch` | `Glyph` 内部 | 命中与选区需要“每个字形对应原文哪个字节”。放进 `Glyph` 让字形数组仍然是唯一的坐标流；如果另开 `Vec<u32>` 映射，两个数组的切片就必须永远同步，任何一处筛字形都要改两处。 |
| `Op::Link` | `DisplayList.ops` 的一种 op | 链接是渲染/命中关心的事，不是排版几何。放进 op 数组就能跟着 `Page.ops` 一起切片；如果挂到 `LineBox` 上，就得给页再加一个“链接切片”，页覆盖不变量会多一条。 |
| `Issue` | `LayoutResult.issues` | 诊断不能混进 op，否则渲染层要认识“只记录不画”的变体。独立一个 `Vec` 让渲染完全不受影响，`IssueCode` 以后加变体也只动这一个枚举。 |
| `truncated` | `LayoutResult` 最后一个字段（`bool`） | `bool` 在 postcard 里是 1 字节，追加在末尾对旧 golden 的影响最小（每个文件 +1 字节）；放在结构体中间会让后面所有字段错位，等同于 major 变更。 |
| `GlyphRun.weight` / `style` / `underline` | `GlyphRun` 最后一个字段之后 | 文字样式必须跟着 run 走，不能只靠 FontBook：没装字体时 font id 全是 0，宿主仍要知道“这句是 bold / italic / underline”。追加在 `rotate` 之后，旧字段偏移不变。 |
| `TextStyle.underline` | `TextStyle` 最后一个字段之后 | 装饰线先只支持下划线；`line-through` / `overline` 还需要更多字段，等有真实需求再加。 |
| `Limits` | **不进 IR** | 限额是输入，不是输出。写进 IR 会让同一份文档在不同预算下产生不同字节，golden 无法稳定。 |
| `LayoutStats` | **不进 IR** | 它是 `size_of` + 容量 + 字符串长度的估算，用于诊断与预算；一旦序列化，分配器的容量差异会变成字节差异。 |
| `DirtyBlock` | 保持独立 | 增量重排要的是“哪块需要重来”，与 op/行都不是一一对应；保留独立数组才能在不改 IR 其余部分的前提下扩展原因。 |

代价是 `LayoutResult` 的字段顺序从此不能重排；收益是每次加字段都只需要同步
`web/decode.js` / `web/encode.js`，而不必重写所有 golden 与像素基线。

---

## 14. 取舍与不做

- 只做横排。`WritingMode` 字段保留，纵向布局不在本轮。
- 只实现成对可判定的 UAX #14 规则；需要上下文的规则明确不装。
- 不做 flex / grid、浏览器级 CSS 兼容、DOM 直渲。
- 不做 PDF、DRM。
- 滚动与分页共用同一份断行/块布局：`Mode::Scroll` 把整章排成一页，
  `paint_window` 按内容 y 取瓦片（窗口外的 op 不画、不查字形）；分页仍是默认模型。
- 跨章滚动不是宿主的事：`scroll_begin` 起的会话给出章节起点、全书总高、
  跨章可见带与流坐标命中，宿主只管“滚动容器 + 把位图贴上去”。
- 不引 criterion；bench 用 `std::time::Instant`，输出可 diff 的
  `name<TAB>ns`，见 [scripts/bench.sh](../scripts/bench.sh)。
