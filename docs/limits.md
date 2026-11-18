# 限额与默认值

当前版本没有统一的 `Limits` 结构。限额分散在各层的常量与边界检查里；这张表是
**实际行为**的汇总，改任何默认值都要同步本文与相关测试/golden。

## 1. 排版与 IR

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
| 页数 | 无硬上限 | 受内存限制 | `paginate.rs` |

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
| ZIP 压缩方法 | `0` stored / `8` deflate | 其他方法 `read` 返回 `None` → 章节 `Missing` | `zip.rs` |
| ZIP 文件名 | 必须 UTF-8 | 非 UTF-8 → `Malformed("zip")` | `zip.rs` |
| EPUB mimetype | 必须以 `application/epub+zip` 开头 | 不匹配时 probe 降为 `Maybe` | `epub.rs` |
| spine 下标 | `0..len` | 越界 `FormatError::OutOfRange` | `chapter` |
| 章节文件 | 必须存在且可解压 | 缺失 `Missing("chapter")` | `epub.rs` |
| 文本编码 | UTF-8（lossy 解码） | 非法字节替换为 U+FFFD | `txt.rs` / `zip.rs` |
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
| IR 版本 | `1.1` | 见 [api.md](api.md#33-版本与兼容策略) |
| wasm gzip 预算 | `512 KiB` | `xtask size`；core 与 core+epub 分别卡 |
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
