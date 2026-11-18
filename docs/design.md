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

光栅后端的“整数混合”与 SVG 后端的“参考实现”是替代关系，不是备份关系：
同一份 IR 可以用任意后端画，后端之间不共享布局逻辑。

---

## 9. wasm 边界

`ebook-web` 只做三件事：把字节交给格式层、把 IR 字节交给 JS、把字体注册进
`FontBook`。IR 是唯一的数据契约：

- header `EBIR` + major/minor 固定 8 字节；
- body 是 postcard 位置编码；
- JS 解码器手写，字段顺序必须与 Rust 完全一致；
- `web/encode.js` 只用于 contract 的“解出来再编回去”，不参与阅读器主路径。

`web/app.js` 只按 `GlyphRun.text` 画文本，按 `Page.origin/inset` 平移画布；
它不解释 CSS，也不重新断行。字体描述由 `eb_font_desc` 从 `FontBook` 取出，
交给 canvas 的 `font` 字符串，保证量出来的和画出来的用同一份字体。

---

## 10. 取舍与不做

- 只做横排。`WritingMode` 字段保留，纵向布局不在本轮。
- 只实现成对可判定的 UAX #14 规则；需要上下文的规则明确不装。
- 不做 flex / grid、浏览器级 CSS 兼容、DOM 直渲。
- 不做 PDF、DRM。
- 不做滚动/瓦片模式（scroll / tile）；分页是唯一的分页模型。
- 不引 criterion；bench 用 `std::time::Instant`，输出可 diff 的
  `name<TAB>ns`，见 [scripts/bench.sh](../scripts/bench.sh)。
