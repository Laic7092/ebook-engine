# ebook-engine

电子书渲染引擎。Web 优先，跨平台，可 Worker 并行。

**只有排版是核心。** 格式层与渲染层是适配器，可拆、可替换、可 mock。

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

分页属于排版，因为断点会反过来影响孤行寡行与段落切分。`orphans` / `widows` / `break-before` / `break-after` 都在分页器里结算：先求最满的断点，再按约束往回退，退不动就退回最满断点。约束冲突时宁可难看，也不产生空页。

op 坐标是**排版流坐标**，不是页面内坐标。`Page.origin` 表示这一页在流里的起点，渲染时整体减去它。所以改分页只移动 `origin`，一个 op 的坐标都不用重写——这正是「渲染层不改坐标」能成立的前提。

---

## 验证

纯函数换来的可验证性，是这个架构真正的收益：

| 手段 | 作用 |
|---|---|
| golden IR | 比字节流，不比像素。改算法时 diff 直接指出影响面。 |
| 跨目标比对 | native 与 wasm 各跑一遍，逐字节相同。 |
| 不变量 fuzz | 行不重叠、y 单调、op 切片连续、重复排版幂等。 |
| 差分测试 | headless 浏览器用 `column-count` 排同一份 XHTML，比行数与断点。 |
| 往返测试 | 锚点 ↔ 源码偏移双向可逆。 |

`DisplayList` 里禁止 `HashMap` / `HashSet`。遍历顺序决定字节流，序列化必须确定。

---

## 不做

PDF；DRM；flex / grid；浏览器级 CSS 兼容；把 HTML 直接塞进 DOM 当渲染方案。

最后一条尤其重要：DOM 方案的排版由浏览器决定，跨端一致、分页控制、增量重排全部不受控，也拿不到可测试的中间表示。

---

## 跑

```
cargo test --workspace
cargo run -p xtask -- golden
cargo run -p xtask -- size
cargo run -p xtask -- parity
cargo run -p ebook-cli -- layout fixtures/demo.epub --spine 0 --stats --svg page.svg
```

`golden` 比字节；`size` 卡预算；`parity` 用同一份输入分别跑 native 与 wasm，逐字节比对 IR。
