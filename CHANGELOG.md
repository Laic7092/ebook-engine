# Changelog

本项目遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本号遵循 `0.x` 阶段的语义：`0.1.0` 是第一条可运行的完整链路。

## [Unreleased]

### Added

- 阅读器翻页动画：淡入淡出与跟手滑动，设置面板新增「翻页动画」开关。
- 排版 Worker（`web/layout-worker.js`）与 `eb_use_layout`：整章 `Layout` 在 worker
  执行，主线程只装回 IR 供命中/选区/链接查询；worker 失败或超时自动退回主线程。

**IR 1.2 / 1.3 / 1.4**

- `Glyph.ch`：字形到原文字节的映射，命中测试与选区不再靠宽度反推。
- `Op::Link`：链接热区按行发射，与 `LineBox` 同形；渲染默认不画，SVG 输出 `<a>`。
- `Issue` / `IssueCode`：可恢复问题的诊断流（缺字、缺图、尺寸依赖内容、超限），
  与 `Op::Unresolved` / `DirtyBlock` 互补。
- `Limits`（`max_nodes` / `max_depth` / `max_text_bytes` / `max_pages` /
  `max_lines` / `max_image_pixels`）与 `LayoutResult.truncated`：超限返回合法截断 IR。
- `LayoutStats` 与 `LayoutResult::stats` / `stats_with_doc`；CLI `--stats` 打印页数、
  行数、字形数、op 数、IR 字节与估算内存。
- IR 1.4：`GlyphRun.weight` / `style` / `underline` 与 `TextStyle.underline`。
  文字样式不再依赖 FontBook 是否命中；宿主没装字体时也能用系统字体合成粗体/斜体。
- `eb_glyph_bitmap`：Web 端按 `GlyphRun` 的 gid 取 FontBook 光栅位图，用
  `drawImage`/WebGL 自己画字；不再把粗体/斜体/字距交给浏览器合成。
- 移动端手势：左右 30% 轻点翻页，中间轻点切换工具栏；触屏横向滑动翻页。
- PWA：`manifest.webmanifest`、Service Worker 应用壳、192/512 图标；可安装、离线打开。
- 字体缓存：CJK 字体写入 Cache Storage（`ebook-fonts-v1`），第二次冷启动不再下载。
- 本地预览字体换成 PingFang SC Regular 的 GB2312 一级+二级子集：TTF 2.2 MB，gzip 1.5 MB；
  优先 gzip + `DecompressionStream`，缺字整段回退系统字体。
- 阅读设置：字号、粗体、行距、字距、页边距、对齐方式；目录顶部的章节下拉框移除。
- `eb_layout_style`：行距/字距/对齐作为独立排版入参，不再只能跟着字号等比缩放。

**排版**

- `Layout::layout_with`（带 `LayoutOptions`）与 `Layout::layout_parallel`：
  一次调用内的 `thread::scope`，输出与串行逐字节相同；`parallel` feature 在 wasm 关闭。
- `Reader`：命中、最近行回退、行/页查询、进度、锚点、链接、越界夹取。
- `Selection` / `SelectionRect` / `Reader::selection_text`：跨行跨页选区，矩形是页内坐标，
  反向选择自动 normalize，原文按 UTF-8 边界吸附。
- `TextMeasurer::shape` 与 `ShapeGlyph`：同字体文本整形成字形序列（默认逐字实现，
  `FontBook` 用 rustybuzz：kerning / ligature / 簇首字节偏移）。
- 双向文本：`unicode-bidi` 按 UAX #9 求段内级，按簇做视觉重排；`GlyphRun.dir` 与
  `LineBox.dir` 分离。
- 断字：`Hyphenator` 接口 + 内置英文 ushyphmax（Liang 算法），由宿主通过
  `TextMeasurer::hyphenator()` 注入；没有词典时 `hyphens: auto` 等同 `none`。
  内置 `English` / `en.pat` 由 `hyphen-en` feature 控制，默认不链入 wasm。
- `TextRules`：成对标点挤压、行末标点悬挂、中西文自动间距三个独立开关，默认全关；
  出处与浏览器差异见 [docs/text-rules.md](docs/text-rules.md)。
- 行内图片与 ruby：不再静默丢节点；尺寸未知时占位并记诊断。

**缓存与并发**

- 新增 `ebook-cache`：以输入指纹为键的有界 LRU（`LayoutCache`），
  `CacheStats { hits, misses, evictions }`；指纹覆盖文档/样式/文本/图片/字体/约束/策略版本。
- 确定性并行：任务只写自己的结果槽，按输入下标拼回；没有线程池、全局队列或 `thread_local`。

**格式层**

- TOC（EPUB3 nav / EPUB2 NCX）、章节链接、资源清单（图片/字体/样式）、
  书目元数据（作者/出版者/语言/日期/标识符）。
- 格式层诊断：`EpubIssueCode`（缺 spine 项、缺资源、XHTML 损坏、未支持 CSS、
  nav/ncx 损坏、重复 id、越界路径、重复 manifest、TOC 目标缺失）。
- `ebook-format::txt::TxtReader`：TXT 按“第 N 章 / Chapter N”标题行分章，
  超长文本按行兜底切分；spine、TOC 与逐章 `StyledDoc` 和 EPUB 同构。
- `ebook-web` 新增 `eb_open_text` / `eb_open_file`：纯文本走同一条排版、
  目录、选中与进度链路；`book_json` 补齐作者、出版者、标识符、日期、简介与封面路径。

**Web 阅读器**

- 书架：IndexedDB 持久化书籍字节、元数据与阅读进度；支持导入、搜索、删除，
  IndexedDB 不可用时自动退回内存。
- 阅读页：EPUB / TXT 共用目录、章节选择、进度条、选区复制、链接跳转，
  新增真实字号（px）、页边距、明亮/护眼/夜间主题设置，设置同样持久化。字号只改文档字形与行高，不改变页面容器；移除“页面铺满屏幕”开关，阅读区始终按可视区域排版。
- Canvas 阅读器按 `GlyphRun` 的 weight/style 生成字体串，并绘制 `underline`；
  `<a>` 使用浏览器默认链接蓝与下划线，标题/strong/em 不再丢失粗细和斜体。
- 移动优先响应式界面：书架 2–6 列自适应；阅读页按真实可视区域排版并铺满，
  顶部/底部工具栏、移动端底部面板、桌面抽屉、移动端工具栏自动隐藏和面板遮罩统一重做；工具栏改为覆盖层，显隐不触发重排。
- TXT 编码检测：BOM、严格 UTF-8，再按替换字符/控制字符评分选择
  GB18030 / Big5 / Shift_JIS / EUC-KR / windows-1252。
- `web/minify.sh`：生成可部署的 `web/dist`。JS 逐文件 minify 而不 bundle，
  保持相对 import 与 wasm URL；HTML 内联 CSS 压缩、去注释与缩进压缩；
  `--wasm-opt` / `--precompress` / `--serve` 按需开启。

**质量与文档**

- `scripts/check.sh`：fmt、clippy `-D warnings`、test、`xtask all` 一键闸门。
- `scripts/bench.sh` 与 `scripts/bench_baseline.txt`：layout / raster / IR codec 的
  中位耗时基线，超过 `BENCH_TOL`（默认 1.15×）退出 1；基线记录采集机器与 rustc 版本。
- `xtask bench`：`std::time::Instant` 采样，输出 `name<TAB>median_ns`。
- `.github/workflows/ci.yml`：stable + wasm32，缓存 `~/.cargo` 与 `target`，
  跑 fmt / clippy `-D warnings` / test / `xtask all` / `xtask size`。
- `docs/api.md`、`docs/design.md`、`docs/limits.md`、`docs/text-rules.md`：
  公开契约、两遍式设计、限额与中文文本政策。
- `crates/ebook-layout/tests/fuzz.rs`（1000 份随机文档）、
  `crates/ebook-raster/tests/fuzz.rs`（1000 页随机 op）、
  `crates/ebook-format/tests/fuzz_like.rs`（1000 次变异）：
  不 panic、不越界、重复执行逐字节/逐像素相同。

### Changed

- 整形后端从 `rustybuzz` 换成 `harfrust`（HarfBuzz 15.2 算法移植，基于
  `read-fonts`）：`FontBook` 在 `Metrics::parse` 时建好 `harfrust::Font` 并随
  `FontBook` 复用，shaping plan/layout 缓存不再每次 `shape` 重建；字体字节用
  `Arc<Vec<u8>>` 与 ttf-parser 共享，不额外复制。公共 `TextMeasurer::shape`
  契约（逻辑序、簇首字节偏移、返回宽等于字形宽之和）不变。
- Web 阅读器交互优化：canvas 尺寸不变时不再重设 `width/height`；当前页 ±1
  预渲染到位图缓存，翻页/跟手滑动优先搬图；选区改画在独立 overlay canvas 上，
  拖动选区不再重绘正文页。
- TXT 适配器改为按行建模：认出来的章节标题行包成 `H1`，其余每个非空行单独包成
  `P`，不再把连续行合并成一个段落（换行即分段，空行不产生节点）。
- 阅读器在书没有内嵌字体时**按语言取字体**：拉丁仍用仓库内置的 `test.ttf`，
  中日韩优先读仓库自带的 CJK 字体（后改为 Noto Sans CJK SC 的 GB2312 子集），
  本地服务器直接加载，不再等 CDN；远程 jsDelivr 只作兜底。
  必须是 TTF/OTF，`ttf-parser` / rustybuzz 读不了 woff2。`?font=<url>` 可换源、
  `?nofont=1` 可退回宿主字体栈；缓存查找有 2s 超时，
  存储被策略禁用时不会卡住首屏。
- Web 字形渲染改为共享 atlas（最多 3 张 2048²，按 LRU 换代），字形吸附到整数
  设备像素：同一 gid 不再因亚像素相位复制成几十张 sprite。翻页/切章少掉每页
  几百次 `OffscreenCanvas` + `putImageData`，桌面 Edge 实测翻页 15–30 ms → 2–8 ms，
  切章 45 ms → 27–32 ms。
- 字体加载与首章排版并行：缓存命中（≤60 ms）直接等字体，冷启动则先用系统字体
  出首屏，字体就绪后 relayout。同语言换书复用 `FontBook` 与 atlas，不再重置字体、
  重画第一页；字体请求去掉 `cache: "no-store"`，交回 HTTP 缓存与 Service Worker。
- 阅读器支持 `?book=<url>&spine=<n>` 深链，直接打开指定章节（也方便截图/回归）。
- IR 1.1 → 1.4：新字段只在结构体末尾追加；IR 仍是 native 与 wasm 之间唯一的字节契约，
  `web/decode.js` / `web/encode.js` 与 `xtask contract` 同步。
- 分页改为明确的两遍式：flow 只做断行，paginate 只做页起点与切片；
  `Page.origin` 仍是排版流坐标，改分页不重写 op。
- 整形从“逐字 advance”改为可选 `shape`：`StubMeasurer` 走默认实现，golden 不受影响。
- clippy 在全 workspace 清零并启用 `-D warnings`；少数属于并行分支路径的
  风格 warning 用带理由的包级 `allow` 收口。
- README 增加 CI badge、脚本入口、验证矩阵、能力清单与“不做”边界，
  并把文档入口补到 `docs/text-rules.md`。
- bench 基线按 rustybuzz 之后的实现重新采集。
- 断行表删掉未使用的 `RI` 类：UAX #14 的 LB30a 尚未实现，这个枚举没有规则可挂；
  `data/gen_linebreak.py` 把 `RI` 解析为 `AL`，表少 `0x1F1E6..0x1F1FF` 一个区间，
  脚本重跑与入库表逐字节一致。

### Removed

- 未合并分支 `wt-b`（用 `glyph_bounding_box` 代替 `outline_glyph` 求光栅边界）作废：
  `Bounds` 现已带 `shear` 参数服务合成斜体，而 `glyph_bounding_box` 不经过 shear，
  照原样合入会让斜体字形的边界算错。

### Fixed

- 目录里超长、无空格的章节名可以任意断行，不再把面板撑出横向滚动条。
- 目录/设置底部弹层关闭后只移出屏幕、阴影仍从视口底部漏出；关闭态现在同步
  隐藏，滑动动画结束后才彻底不可见。
- XHTML 带 `<!DOCTYPE html>`（pandoc / calibre / Sigil 的默认输出）不再被判成
  `Malformed("xhtml")`：解析时放行 DTD，实体仍由格式层展开。此前这类书每个
  章节都打不开（表现为「chapter N 排版失败」）。
- 浏览器阅读器不再给中日韩书自动装只覆盖拉丁的内置字体（否则整篇
  `font missing`）；换书时清空上一本注册的字体。量度字体与画布字体不一致时
  按字形 `dx` 逐字摆放，避免整词按自然宽绘制、下一词却按 `dx` 起笔造成的压字。
- `eb_layout` 失败时通过 `eb_last_error` 返回具体原因，JS 侧拼进错误消息，
  不再是笼统的「排版失败」。
- 浏览器阅读器首屏没装字体：排版退回固定 advance 的 stub，画布却用宿主字体，
  量出来的和画出来的不是同一套宽度，词内裂开、断行也与原生不一致。现在打开
  没有内嵌字体的书会先装内置字体；`drawRun` 不再按字符画，改为按空白切块
  （块内保留整形与 kerning，块间按字形 `dx` 前进），两端对齐的空格宽度
  因此精确复现。
- 选区文本按 UTF-8 边界向外吸附，不再切出半个字符。
- 超限结果不再产生悬空字形区间：截断后 `display.glyphs` 收敛到仍被行引用的最大下标。
- 链接/选区/进度的 wasm 查询在页号或位置越界时返回 `null` 字段，而不是 panic。

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
