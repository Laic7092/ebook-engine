# Changelog

本项目遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本号遵循 `0.x` 阶段的语义：`0.1.0` 是第一条可运行的完整链路。

## [Unreleased]

### Added

- `scripts/check.sh`：fmt、clippy（只报告 warning）、test、`xtask all` 一键闸门。
- `scripts/bench.sh` 与 `scripts/bench_baseline.txt`：layout / raster / IR codec 的
  中位耗时基线，超过 `BENCH_TOL`（默认 1.15×）退出 1。
- `xtask bench`：`std::time::Instant` 采样，输出 `name<TAB>median_ns`。
- `.github/workflows/ci.yml`：stable + wasm32，缓存 `~/.cargo` 与 `target`，
  跑 fmt / clippy / test / `xtask all` / `xtask size`。
- `docs/api.md`、`docs/design.md`、`docs/limits.md`：公开契约、不变量与限额。

### Changed

- README 增加 CI badge、脚本入口、验证矩阵与“不做”边界。
- clippy 暂不启用 `-D warnings`，warning 数量作为指标；crate 源码统一清理
  留给后续提交。

## [0.1.0] - 2026-11-18

首条完整链路：TXT / EPUB → 纯排版 → IR → SVG / 光栅 / Canvas2D。
本条目归纳到此为止的 23 条历史提交。

### Added

- **IR 契约**：`StyledDoc`（DOM + 已计算样式 + 原始锚点）、`LayoutResult`
  （`Page[]` + `DisplayList` + `RunMap` + `DirtyBlock`）、`Renderer`
  （`&LayoutResult` → 像素）。
- **IR 编解码**：`EBIR` magic + major/minor header + postcard 主体；`hash_of`
  提供 FNV-1a 缓存键；IR 版本 `1.1`。
- **IR op**：`GlyphRun`、`Rect`、`Border`、`Image`、`Unresolved`、`Clip`；
  裁剪 op 支持嵌套；`Fit::{Contain, Cover, Fill}`。
- **显式不确定性**：`UnresolvedReason::{ImagePending, FontMissing,
  SizeFromContent}` 同时进入 op 与 `DirtyBlock`，渲染层不猜尺寸/字形。
- **纯排版**：盒树、行盒构造、断行、分页、增量重排入口；`layout` 是全函数。
- **分页**：`orphans` / `widows` / `break-before` / `break-after` 在分页器结算；
  约束冲突时退回最满断点，不产生空页。
- **文本排版**：两端对齐、文本缩进、字距/词距、`text-transform`、
  `white-space`、`vertical-align`、完整 margin collapsing。
- **断行**：UAX #14 成对规则；属性表由 `data/unicode/LineBreak.txt` 生成，
  不引第三方断行 crate。
- **格式层**：TXT 适配器；EPUB 适配器（ZIP stored/deflate、`container.xml`、
  OPF、spine、XHTML、CSS 级联、命名实体、锚点映射）。
- **字体**：`ttf-parser` 读取真实 advance / ascent / descent / cmap；
  face 匹配、family 栈、字重/样式选择、合成粗体与斜体、亚像素定位。
- **字形光栅化**：`ab_glyph_rasterizer` 生成 alpha 位图，同一份字体字节同时
  服务度量与绘制。
- **渲染**：共用 `paint()` op 遍历；SVG 参考后端；RGBA8 光栅后端（整数混合、
  逐字节跨平台一致、`to_ppm()`）；Canvas2D 浏览器后端。
- **页边距**：`Page.inset` 携带页边距，后端按页面盒尺寸分配画布。
- **web**：wasm-bindgen 导出、postcard worker 协议、浏览器 EPUB 阅读器、
  可选字体加载、字体描述供 canvas 选择字重/样式、适配视口。
- **验证工具**：`xtask golden`（逐字节 IR）、`xtask contract`（JS 解码器
  解出再编回）、`xtask pixel`（逐页 FNV）、`xtask size`（gzip 预算）、
  `xtask parity`（native vs wasm 逐字节）、`xtask app`（DOM stub 端到端）。
- **不变量测试**：60 个随机文档 × 3 种页面尺寸，覆盖页覆盖、行不越界、
  y/页序单调、重复排版幂等、编解码往返不变；空文档与极小页面保持可序列化。

### Changed

- 文本从“字符 + 宽度”改为 glyph run：`GlyphRun` 持有 `text`、`glyphs` span、
  `font`、`size`、`dir`、`rotate`，选区与 Canvas2D 共用同一份数据。
- 分页结果从“页内坐标”改为“排版流坐标 + `Page.origin`”；改分页只移动
  origin，op 坐标不重写。
- 度量从 stub 改为注入式 `TextMeasurer`：`FontBook` 给真实度量，
  `StubMeasurer` 只用于确定性测试。
- 浏览器端从固定字体改为“可选字体 + 浏览器字体回退”，并暴露字体描述，
  避免量出来的和画出来的不是同一款字。
- 失败语义从静默空结果改为显式错误/`Unresolved`：打开失败、渲染失败、
  缺字都不再吞掉。
- `Li`/链接等 dead variant 清理，IR 只保留实际会产出的形状。

### Fixed

- 命名实体（`&hellip;`、`&rsquo;` 等）现在会展开，真实 EPUB 不再因为实体
  解析失败而打不开；未知实体保留原文本，不丢内容。
- 光栅后端渲染前清屏，并正确应用裁剪栈；重复渲染不再叠上一页的像素。
- 图片/字体缺失时产确定性占位与 `DirtyBlock`，不再由渲染层临时猜值。
- 页边距与页面盒尺寸一致：`from_page` 使用 `Page.box_size()`，内容不再被
  边距裁掉。
- XHTML/CSS 解析对未知属性、未命名实体和损坏输入更宽容，错误分类为
  `Unsupported` / `Missing` / `Malformed` / `OutOfRange`，不再统一吞成空文档。
- 浏览器阅读器对打开/排版/字体失败给出可见错误，并补上端到端 App 测试。
