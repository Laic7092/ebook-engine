# ebook-engine

[![CI](https://github.com/OWNER/ebook-engine/actions/workflows/ci.yml/badge.svg)](https://github.com/OWNER/ebook-engine/actions/workflows/ci.yml)
<!-- 发布到 GitHub 时把 OWNER 换成实际组织/用户名。 -->

电子书渲染引擎。Web 优先，跨平台，可 Worker 并行。

**只有排版是核心。** 格式层与渲染层是适配器，可拆、可替换、可 mock。

接口契约见 [docs/api.md](docs/api.md)，设计论证与不变量见
[docs/design.md](docs/design.md)，限额与默认值见 [docs/limits.md](docs/limits.md)。

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

度量只认字体入参：`TextMeasurer` 由外部注入，`FontBook` 用 `ttf-parser` 给出真实的 advance / ascent / descent / cmap，同一份字体字节同时也是 `Resources` 的光栅化来源。`ebook-layout` 到今天依然只依赖 `ebook-ir` 一个 crate。

断行按 UAX #14，属性表由 `data/unicode/LineBreak.txt` 生成（`data/gen_linebreak.py`），不引第三方 crate。只实现成对可判定的规则，需要上下文的规则明确不装。

两端对齐、孤行寡行、块边界与强制分页都在排版层结算。缺字不猜：产 `Op::Unresolved{FontMissing}` 与 `DirtyBlock`，由宿主决定换字体还是换字。

分页属于排版，因为断点会反过来影响孤行寡行与段落切分。`orphans` / `widows` / `break-before` / `break-after` 都在分页器里结算：先求最满的断点，再按约束往回退，退不动就退回最满断点。约束冲突时宁可难看，也不产生空页。

op 坐标是**排版流坐标**，不是页面内坐标。`Page.origin` 表示这一页在流里的起点，渲染时整体减去它。所以改分页只移动 `origin`，一个 op 的坐标都不用重写——这正是「渲染层不改坐标」能成立的前提。

---

## 三个后端

同一份 IR，三种画法。彼此是替代关系，不是备份关系。

| 后端 | 位置 | 用途 |
|---|---|---|
| SVG | `ebook-render` | 参考实现，输出可以直接看，也方便和浏览器排同一份 XHTML 做差分 |
| 光栅 | `ebook-raster` | 整数混合，跨平台逐字节一致；`to_ppm()` 可以落到文件 |
| Canvas2D | `web/` | 浏览器阅读器，按 `GlyphRun.text` 画，文本可选中 |

`paint()` 是三个后端共用的 op 遍历，坐标只在这里被平移到页面内，后端自己不碰坐标。

---

## 验证

纯函数换来的可验证性，是这个架构真正的收益：

| 手段 | 命令 | 失守时说明什么 |
|---|---|---|
| 一键闸门 | `scripts/check.sh` | fmt、clippy（只报告 warning）、test、`xtask all` 任一失败。 |
| golden IR | `xtask golden` | 排版结果变了。字节 diff 直接指出变化面。 |
| ABI 契约 | `xtask contract` | JS 解码器与 Rust 结构体字段不再一致。做法是把 golden IR 解出来再编回去，要求逐字节还原。 |
| 像素基线 | `xtask pixel` | 渲染结果变了。逐页 FNV 摘要，`pixel-bless` 显式更新。 |
| 跨目标比对 | `xtask parity` | native 与 wasm 不再逐字节相同。 |
| 浏览器应用 | `xtask app` | 读者 App 的翻页/打开/字体路径坏了。用 bun + DOM stub 跑端到端。 |
| 体积预算 | `xtask size` | core 或 core+epub 的 wasm gzip 超过 512 KiB。 |
| 性能回归 | `scripts/bench.sh` | layout / raster / IR codec 的中位耗时超过基线 1.15×。 |
| 不变量 fuzz | `cargo test` | 60 个随机文档 × 3 种页面尺寸：页覆盖、行不越界、y 单调、页序单调、重复排版幂等、编解码往返不变。 |

`DisplayList` 里禁止 `HashMap` / `HashSet`。遍历顺序决定字节流，序列化必须确定。

`tests/golden/*.ir` 是本地快照（`.gitignore` 排除 `*.ir`）。`scripts/check.sh` 与 CI 只在缺失时生成，已有快照仍然逐字节比较；不要用 `bless` 覆盖回归。

IR 是 wasm 与 JS 之间唯一的接口，而 JS 侧的解码器是手写的，所以 `contract` 这项不是锦上添花：它把「改了 Rust 结构体，忘了改 JS」从静默错位变成一条带字节偏移的失败。

---

## 不做

PDF；DRM；flex / grid；浏览器级 CSS 兼容；把 HTML 直接塞进 DOM 当渲染方案；滚动 / 瓦片模式（scroll / tile）。

DOM 方案尤其不能做：排版由浏览器决定，跨端一致、分页控制、增量重排全部不受控，也拿不到可测试的中间表示。滚动 / 瓦片模式会把分页从唯一模型降级成可选视图，`Page.origin` 与页切片契约随之失效，所以本轮明确不做。

---

## 跑

```
scripts/check.sh                 # fmt + clippy(报告) + test + xtask all
scripts/bench.sh                 # 对比 scripts/bench_baseline.txt，超 1.15× 退出 1
scripts/bench.sh --update        # 在目标机器上重新取基线
cargo test --workspace
cargo run -p xtask -- all
cargo run -p xtask -- golden | bless | contract | pixel | pixel-bless | size | parity | app | bench
cargo run -p ebook-cli -- layout fixtures/demo.epub --spine 0 --stats --svg page.svg
cargo run -p ebook-raster --example page
./web/build.sh && python3 -m http.server
```

最后一条打开 `http://localhost:8000/web/` 就是阅读器：选 EPUB、翻页、换章。`fixtures/en.epub` 是自带的英文样书，字体用 `fixtures/font/test.ttf`，浏览器侧用同一份字体做绘制，量出来的和画出来的不该有漂移。中日韩书需要自带一个覆盖 CJK 的字体，仓库里那个只覆盖拉丁。

`golden` 比字节；`size` 卡预算；`parity` 用同一份输入分别跑 native 与 wasm，逐字节比对 IR。`bench` 输出 `name<TAB>median_ns`，可直接 diff；CI 跑 fmt、clippy（warning 只作为指标打印，不加 `-D warnings`）、test、`xtask all` 与 `xtask size`。
