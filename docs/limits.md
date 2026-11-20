# 限额与默认值

限额分两层：`ebook_ir::Limits`（可配置，随 `LayoutOptions` 传入，字段为 0 时用默认值）
和各层内部的固定常量（不可配）。这张表是**实际行为**的汇总，改任何默认值都要
同步本文与 `crates/ebook-layout/tests/limits.rs`。

## 1. 排版与 IR

### 1.1 `Limits`（`LayoutOptions.limits`）

| 字段 | 默认 | 超限行为 |
|---|---:|---|
| `max_nodes` | `1_048_576` | 空结果 + `LimitExceeded("max_nodes")`，`truncated` |
| `max_depth` | `256` | 空结果 + `LimitExceeded("max_depth")`，`truncated` |
| `max_text_bytes` | `20_000_000` | 空结果 + `LimitExceeded("max_text_bytes")`，`truncated` |
| `max_pages` | `100_000` | 保留已计划的页，丢尾部；`truncated` |
| `max_lines` | `2_000_000` | 保留已产出的行前缀；`truncated` |
| `max_image_pixels` | `67_108_864` | 超预算的图降为 `SizeFromContent`；`truncated` |

零值统一由 `Limits::resolved()` 换成默认值；`Limits::default()` 就是上表。
超限结果仍是合法 IR（页覆盖、行不越界、编解码往返成立）。

### 1.2 固定常量与几何边界

| 项 | 默认/上限 | 超限行为 | 位置 |
|---|---:|---|---|
| 页面尺寸 | `600 × 800` | 无上限；由调用方给 | `PageConstraint::default` |
| 页边距 | `0` | 负值会缩小内容区 | `EdgesF::ZERO` |
| 内容区最小尺寸 | `1.0 px` / 轴 | 夹到 1.0，不会出现 0 或负尺寸 | `PageConstraint::content_size` |
| 块嵌套深度 | `128` | 更深的块直接当空块，不报错、不产生内容 | `flow.rs` |
| 单块行序号/行数 | `u16::MAX = 65535` | 超过后行数统计截断；分页仍继续 | `flow.rs` |
| `NodeId` | `u32` | arena 下标，越界 panic（调用方违约） | `ebook-ir::doc` |
| `ImageId` | `u16 = 65535` | 超出无法编码；格式层应控制 | `ebook-ir::doc` |
| `Span.start/len` | `u32` | 文本/op 索引；越界切片 panic | `ebook-ir::layout` |
| 页号 | `u32` | 越界查询 `None` / 渲染 `NoSuchPage` | `LayoutResult::page` |
| 页数 | `100_000`（`max_pages`） | 超出截断，见 1.1 | `paginate.rs` |
| 断字词长 | `65535` 字节（`u16` 偏移） | 更长的词不拆（按无词典处理） | `hyphen/en.rs` |

“单行可表示”的上限来自 `u32` span（`Span.start/len`）；`max_text_bytes` 在断行之前
生效，所以单行不可能先碰到 `u32::MAX`。

## 2. 字体

| 项 | 默认/上限 | 超限行为 | 位置 |
|---|---:|---|---|
| 字体栈长度 | `8` 个 family | 第 9 个 `push` 返回 `false`，保留前 8 个 | `FONT_STACK_MAX` |
| family 名长度 | `32` 字节 UTF-8 | 在字符边界截断，不报错 | `FONT_FAMILY_MAX` |
| 字重 | `0..=1000` | CSS 解析时夹到 `1..=1000`；`0` 视为 `400` | `css.rs` / `FontBook::add_face` |
| 合成字体的 face 下标 | `0..=4095`（12 bit） | 超出后不合成，直接用基础 face | `SYNTH_BASE_MASK` |
| 字形位图尺寸 | `65535 × 65535` | 超出返回 `None`，不栅格化 | `FontBook::rasterize_at` |
| 字形 left/top | `i16` 范围 | 超出返回 `None` | 同上 |
| 字号 | `> 0` 且有限 | 非有限或 `<= 0` 返回 `None` | 同上 |

`FontId` 的 bit 15 是合成标记，bit 12..14 是合成粗体/斜体，bit 0..11 是基础
face 下标。`FontId::NONE = u16::MAX`。

## 3. 格式层

| 项 | 默认/上限 | 超限行为 | 位置 |
|---|---:|---|---|
| ZIP EOCD 搜索窗口 | 最后 `66_000` 字节 | 找不到返回 `BadSignature` → `Malformed("zip")` | `zip.rs` |
| ZIP 中央目录条目 | `1_000_000` | 超出 `Unsupported` → `Malformed("zip")` | `zip.rs` |
| ZIP64 | EOCD / 中央目录 size+offset | 支持；字段损坏返回 `Truncated`/`Unsupported` | `zip.rs` |
| ZIP 压缩方法 | `0` stored / `8` deflate | 其他方法 `read` 返回 `None` → 章节 `Missing` | `zip.rs` |
| ZIP 文件名 | UTF-8 | 单个非 UTF-8 条目跳过，不再拖垮整包 | `zip.rs` |
| ZIP 单条目解压 | `64 MiB` | 超出返回 `None` → 对应资产/章节 `Missing` | `zip.rs` |
| EPUB mimetype | 必须以 `application/epub+zip` 开头 | 不匹配时 probe 降为 `Maybe` | `epub.rs` |
| container.xml | 缺失/坏/指向不存在的 OPF | 回退扫描归档内 `.opf`，并记 `MissingAsset` issue；找不到才报错 | `epub.rs` |
| 包文档/导航文档 | `16 MiB`；XML 深度 `128`；节点 `1_000_000` | 超限 `Malformed("opf")` 或跳过该导航源 | `epub.rs` |
| 章节 XHTML | `32 MiB`；解析器深度 `128`；构建深度 `64`；节点 `1_048_576` | 超限 `Malformed("xhtml")`，只失败该章并记 issue | `xhtml.rs` |
| 章节文本 | `20_000_000` 字节 | 超限 `Malformed("xhtml")` | `xhtml.rs` |
| 章节锚点路径总量 | `8_000_000` 项 | 超限 `Malformed("xhtml")` | `xhtml.rs` |
| 单章图片数 | `u16::MAX` | 超限 `Malformed("xhtml")`（`ImageId` 是 u16） | `xhtml.rs` |
| CSS 输入 | `16 MiB` | 在 UTF-8 边界截断 | `css.rs` |
| CSS 规则数 | `16_384` | 保留已解析前缀，记不支持项 | `css.rs` |
| CSS at-rule 嵌套 | `32` 层 | 更深分支跳过，记不支持项 | `css.rs` |
| CSS 选择器 | `4 KiB` / `32` 个复合选择器 | 整条规则丢弃 | `css.rs` |
| CSS 单规则体 | `4 MiB` | 整条规则跳过 | `css.rs` |
| CSS 声明数 | `2_048` / 规则 | 保留前缀 | `css.rs` |
| CSS 级联匹配预算 | `20_000_000` 步 | 用尽后停止应用样式表规则（仍保留行内样式） | `css.rs` |
| 内嵌字体数 | `1_024` | 超出记 `MissingAsset`，剩余跳过 | `epub.rs` |
| 目录深度/条目 | `64` 层 / `100_000` 条 | 更深/更多条目截断 | `epub.rs` |
| spine 下标 | `0..len` | 越界 `FormatError::OutOfRange` | `chapter` |
| 章节文件 | 必须存在且可解压 | 缺失 `Missing("chapter")` 并记 `MissingAsset` | `epub.rs` |
| 文本编码 | UTF-8（含 BOM）/ UTF-16 LE+BE / latin-1 / windows-1252 | 其它非法字节替换为 U+FFFD | `txt.rs` / `zip.rs` |
| XML 非法控制字符 | XML 1.0 合法字符集 | 替换为 U+FFFD 后继续解析 | `entities.rs` |
| CSS 未知属性/值 | 忽略 | 不影响打开 | `css.rs` |
| 未命名实体 | 保留原文本 | 例如 `&bogus;` 原样进入正文 | `entities.rs` |

## 4. 渲染

| 项 | 默认/上限 | 超限行为 | 位置 |
|---|---:|---|---|
| 画布尺寸 | 由 `Page.box_size()` 或调用方给 | `u32`；`width * height` 用 saturating 乘法 | `RgbaSurface::new` |
| 画布像素数 | 无硬上限 | 由内存决定；宿主应限制页面尺寸 | `RgbaSurface` |
| 裁剪栈 | 无硬上限 | 逐层求交；空交集后不再绘制 | `RgbaSurface` |
| SVG id | `u32` | `saturating_add`，极端数量下重复 | `SvgSurface` |
| 背景 | 白色（`Rgba8::WHITE`） | `render_page` 先清屏 | `RgbaSurface` |

`RgbaSurface::from_size` 对非正数取 0，对正数向上取整。画布过大时分配失败属于
调用方需要提前约束的输入，不是排版层的错误。

## 5. wasm 与构建

| 项 | 值 | 说明 |
|---|---:|---|
| IR magic | `EBIR` | 4 字节 |
| IR header | `8` 字节 | magic + major/minor（LE u16） |
| IR 版本 | `1.4` | 见 [api.md](api.md#33-版本与兼容策略) |
| wasm gzip 预算 | `680 KiB` | `xtask size`；core 与 core+epub 分别卡 |
| `wasm-bindgen` | `0.2.129` | 与 `ebook-web` 精确锁定 |
| wasm target | `wasm32-unknown-unknown` | `rust-toolchain.toml` |

## 6. 性能闸门

这些不是运行时限额，是 CI/本地闸门：

| 项 | 默认 | 重新基线 |
|---|---:|---|
| `xtask bench` 采样 | 31 次，中位数 | `BENCH_ITERS` |
| 独立进程重复 | 3 次，再取中位数 | `BENCH_COUNT` |
| 预热 | 3 次 | `BENCH_WARMUP`（代码常量） |
| bench 允许 slowdown | `1.15×` | `scripts/bench.sh --update` |
| golden / pixel | 逐字节 | `xtask bless` / `xtask pixel-bless` |
| IR 尺寸 | `512 KiB` gzip | `xtask size` 常量 |

bench 的绝对 ns 与 CPU、编译器版本强相关；基线只用于抓大幅回归，不追求跨机器
可比。重新基线后把 `scripts/bench_baseline.txt` 一起提交，并在 commit message
里写明原因。
