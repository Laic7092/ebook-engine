# Changelog

本项目遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本号遵循 `0.x` 阶段的语义：`0.1.0` 是第一条可运行的完整链路。

## [Unreleased]

### Added

- **排版前段缓存（`Layout::layout_cached` + `FlowCache`）**：把「盒树 + 断行 +
  整形」与「分页 + 组装」切成两级。前段的键只含调用方版本号、可用宽度、行数
  预算，**刻意不含页高与 `Mode`**——分页依赖页高，断行只依赖宽度。`Engine` 已
  接线（`(spine, style, font_rev)` 递增 `flow_rev`），所以手机地址栏收起、旋屏、
  分页 ↔ 滚动切模这些「同宽不同高」的重排不再重跑断行：`en.epub` 一章
  `0.855 ms → 0.132 ms`。硬约束是「缓存只省时间、不改输出」：带缓存的 IR 与不带
  缓存逐字节相同（`crates/ebook-layout/tests/flow_cache.rs`、
  `crates/ebook-reader/tests/flow_cache.rs`），换宽度 / 换字号 / 换行数预算 /
  改 `rev` 都必须不命中。
- **`Surface::finish()`**：一页或一次 `paint_window` 画完后的收尾点，默认空实现；
  `paint` / `paint_window` 在返回前调用一次，`ScaledSurface` 会穿到内层后端。
- **同色字形批处理（`RgbaSurface::set_glyph_batching`，默认关闭）**：同色字形先
  累积成 1 字节/像素覆盖率，换色或遇非字形 op 时一次合成；重叠取 `max`，因此
  结果与绘制顺序无关（这是按瓦片/按任务并行光栅、以及以后换 GPU 后端的前提）。
  默认关闭是测量结论：正文的墨稀疏且几乎不重叠，批处理多一遍目标缓冲往返，
  `480x640` 整页从 `0.49 ms` 变成 `0.64 ms`。两种模式的语义都有测试锁定。
- **连续滚动会话（跨章）**：`Engine::scroll_begin` 之后全部用全书流坐标，
  章节起点（`scroll_chapter` / `scroll_chapters`）、总高（`scroll_total_height`）、
  y → 章（`scroll_spine_at`）、跨章可见带（`scroll_slice`，章缝不错行）、
  跨章命中（`scroll_hit`）、行/锚点 → y（`scroll_line_y` / `scroll_anchor_y`）、
  进度（`scroll_progress`）。高度是懒量的，一章排不出来按 0 高记、不拖累后面的章；
  章节 IR 有 4 章 LRU。宿主不再维护“章节 top 表 / 切片命名 / 按章调度排版”。
  C ABI：`eb_plain_scroll_*`（含 `_band_raw` 落盘与 `_band_bytes` 内存瓦片），
  协议面 `Request::Scroll*`（tag 追加在末尾，老 tag 不变）。新增 `EbScrollChapter` /
  `EbScrollLoc`（`eb_struct_size` kind 5/6）。
- **滚动只画窗口里的 op**：`paint_window` 按 `Surface::scale()` 把 surface 高换算
  成流坐标，窗口外的字形连光栅查询都不做；此前每个瓦片都把整章的字形过一遍，
  代价只随章高增长（2x 下 9010pt 章：128pt 与 1024pt 切片都是 ~90ms，一整页只要
  ~6ms）。瓦片起点同时吸到设备像素栅格，滚动时字形亚像素相位不变，光栅缓存能命中。
  回归测试用计数 `Resources` 断言窗口外不查字形。

- C ABI 有了头文件：`crates/ebook-abi/include/ebook_abi.h`，两个面
  （`eb_open`/`eb_request` 与 `eb_plain_*`）、`repr(C)` 结构体、`EQP1` 位图格式
  与所有权/生命周期都在里面。`cargo test -p ebook-abi --test header` 用
  `-Wall -Wextra -Werror` 真编译一遍，并用 `_Static_assert` 对齐 Rust 的
  `size_of`；头文件与 `pub extern "C" fn` 的符号集合必须相等，漏一个就红。
- C ABI 版本检查：`EB_ABI_VERSION` / `eb_abi_version()` /
  `eb_plain_abi_version()`，外加 `eb_struct_size(kind)`（`EbPageInfo` /
  `EbLoc` / `EbSelectionRect` / `EbLink`），宿主启动时能拒绝布局不一致的库。
- 标量面补齐（`eb_plain_*`）：书目元数据（`language` / `creator` / `publisher` /
  `identifier` / `date` / `description` / `cover_href` / `cover_mime` /
  `cover_len`）、spine 的 `id` / `media_type` / `linear`、TOC 的
  `href` / `fragment`、选区（`select` + `selection_rect_count` / `_rect` /
  `_text` / `_start` / `_end`）、链接解析（`resolve_link` /
  `resolved_fragment`）、`fragment_line`、`set_limits`。
- `Engine::page(index)`：单页元数据按下标直取，不再为了第一页把整章页表
  拉出来（`eb_plain_page_info` 因此从 O(页数) 变成 O(1)）。
- 发版/CI 去重：`1.100.0` 之前抄在三个 workflow 里，还配了一个“检查抄得对不对”
  的步骤。现在 `.github/actions/rust/action.yml` 一份，版本从
  `rust-toolchain.toml` / `crates/ebook-web/Cargo.toml` 经 `scripts/versions.sh`
  读；CI 直接跑 `scripts/check.sh`（本地与 CI 同一份闸门），不再各写一遍
  fmt/clippy/test/xtask。`scripts/check.sh` 里“golden 缺失就自动 bless”的兜底
  删掉了（快照已入库，自动覆盖只会掩盖回归）。
- release 矩阵扩到 Android（`arm64` / `armv7` / `x64`，用 runner 自带的 NDK
  clang）与 Windows（`x64`，MSVC 的 `ebook_abi.lib`）；每个 tar 里多带一份
  `ebook_abi.h`。
- 滚动模式：`Mode::Scroll` 把整章排成一页（`Page.origin == (0, 0)`，
  `Page.size.h` 是内容高度），`paint_window` 按内容 `y` 取瓦片。与分页共用同一份
  断行/块布局，`Page.origin` 与页切片契约不变。`Engine::layout_scroll` /
  `content_height` / `bitmap_window`，C ABI `eb_plain_layout_scroll` /
  `eb_plain_content_height` / `eb_plain_page_slice_raw`。

- 块级背景/边框：布局层现在会为 `background` / `border` 产出 `Op::Rect` /
  `Op::Border`。背景铺满边框盒、边框在背景之上；跨页盒按页切段，首段保留上边框、
  末段保留下边框、中间段只留左右。折叠外边距后的盒顶按实际内容位置计算。
- 内嵌字体支持 WOFF / WOFF2：`FontBook` 在加载入口用纯 Rust 的 `wuff` 解包成
  sfnt，再复用现有 harfrust 整形、read-fonts 度量与光栅链路；Web 端「加载本地字体」
  也接受 `.woff` / `.woff2`。解包失败通过 `FontBook` 诊断与 `eb_font_error` 暴露，
  不再静默丢弃。
- 阅读器翻页动画：淡入淡出与跟手滑动，设置面板新增「翻页动画」开关。
- **CSS `hsl()` / `hsla()`**：色相接受裸数字与 `deg`、负角绕回；饱和度/亮度必须是
  百分比，alpha 与 `rgb()` 共用规则（数字或百分比、越界钳制）。
- **CSS 一般兄弟选择器 `~`**：按 `prev_element` 链匹配任意前置元素兄弟，文本节点
  不打断；`p ~ p` 不再整条丢弃。
- 引擎 Worker（`web/engine-worker.js`）：wasm 实例只存在于 worker 里，开书、注册
  字体、排版、整页光栅、命中/选区/链接查询全在那边；主线程只发 RPC，拿页元数据
  （`eb_page_meta`）、位图（`eb_page_bitmap`，`transfer`）和查询结果。开书只解析
  一次，字体只注册一次，整份 IR 不再穿过线程边界。

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
- `eb_page_bitmap`：整页光栅由引擎完成（`paint()` + `ScaledSurface` + `RgbaSurface`），
  Web 端只 `putImageData`。字形光栅、图片解码、主题调色、下划线都不再由 JS 承担；
  粗/斜体是绘制期的 face 替换，不改 IR。
- `eb_page_meta` / `eb_fragment_line`：页盒子、行区间、本章进度与锚点行号直接来自
  引擎，宿主不再解码 IR。
- PNG + JPEG 图片解码（`zune-jpeg`），按归档路径做全书图片缓存。
- 移动端手势：左右 30% 轻点翻页，中间轻点切换工具栏；触屏横向滑动翻页。
- PWA：`manifest.webmanifest`、Service Worker 应用壳、192/512 图标；可安装、离线打开。
- 字体缓存：CJK 字体写入 Cache Storage（`ebook-fonts-v1`），第二次冷启动不再下载。
- 本地预览字体换成 PingFang SC Regular 的 GB2312 一级+二级子集：TTF 2.2 MB，gzip 1.5 MB；
  优先 gzip + `DecompressionStream`，缺字整段回退系统字体。
- 阅读设置：字号、粗体、行距、字距、页边距、对齐方式；目录顶部的章节下拉框移除。
- `eb_layout_style`：行距/字距/对齐作为独立排版入参，不再只能跟着字号等比缩放。

### Fixed

- **C ABI 出图是原子写**：`eb_plain_page_raw` / `_page_slice_raw` /
  `_scroll_band_raw` / `_scroll_slice_raw` 先写同目录的 `{path}.{pid}.{seq}.part`
  再 `rename`，宿主按「文件存在」判断瓦片可用时不会再读到只写了一半的 `EQP1`
  （滚动条带被取消/并发时那条一屏纸色的空白带）。
- **滚动章高按视口向上取整**：短章（题记、单段小节）至少占一屏，章尾是纸色，
  下一章从整屏边界开始；`tops` / 总高 / `scroll_spine_at` / `scroll_progress`
  都跟这张高度表走。排版失败的章仍是 0。
- **主题墨色只换默认字色**：`ink != 0` 现在只替换正文主导色（按文本字节数取），
  书里显式写的 `color`（链接蓝、`em` 红、警告框）保留。`set_ink` 的旧语义还在，
  需要整页一种颜色时仍可用；native 与 wasm 的 `page_bitmap` 都改了，C 头文件也
  补了 `ink`/`paper` 的说明。
- **`<rp>` 回退括号不再进入正文**：引擎自己处理注音，真实 XHTML 里的回退括号
  不会和基字、`rt` 一起排出来。
- **`text-indent` 按段落方向镜像**：RTL 首行从右边缩进；`text-align: end` 也不
  再把缩进算两次（LTR / RTL 都修）。
- **README 自相矛盾**：末尾还写着「缺字时该段退回浏览器系统字体」，与「现状与
  限制」和 `Op::Unresolved{FontMissing}` 的契约相反。回退路径已经删除：平台
  fallback 链不同会让同一本书在不同端分页不同，书签与跨端同步全部失效。

### Changed

- 渲染热路径不再为每个 `GlyphRun` 复制 `text`：`Surface` 新增
  `glyph_run_shifted`（默认实现兼容旧后端），`paint` 把平移交给后端，
  `RgbaSurface` 直接按平移后的 origin 绘制；`ScaledSurface` 只保留一次
  size/origin 复制。
- 排版侧少几次分配：flow → 组装直接 `move` run 文本；`line_link` 在文档无链接
  时短路、同一内联 owner 只查一次祖先链；`line_entries` 不再按字符分配
  `owner_chain`；`fit_hyphen` 用前缀和代替逐候选重算行宽。
- 仓库只保留源码：字体、EPUB 夹具、PWA 图标、iOS 启动图移出 git，由
  `scripts/setup.sh` / `scripts/generate.sh` 重建。新增 `fixtures/make_misc_epubs.py`
  与 `scripts/gen-icons.py`，所有生成物写进 `.gitignore`；CI 在测试前跑
  `scripts/generate.sh --skip-web`。
- Web 端 CJK 回退字体从 GB2312 子集 TTF（`cjk.ttf` + gzip）换成完整
  PingFang SC Regular WOFF2（`web/cjk.woff2`，约 5 MB）。woff2 自身已压缩，
  去掉 `DecompressionStream` 解压与 `.gz` 兜底分支，Cache Storage 直接存原始字节；
  `_headers` 与 Service Worker 缓存名同步更新。

### Removed

- `web/` 的 Canvas2D 绘制栈：字形 atlas、`drawOp`/`drawRun`/`drawGlyphRun`、
  `FontFace` 注册、章节图片解码。对应 ABI 一并下掉：`eb_glyph_bitmap`、
  `eb_font_desc`、`eb_font_face_bytes`、`eb_asset`、`eb_chapter_images`、
  `eb_use_layout`（主线程不再有交互状态副本，也就不需要装回）。

### Fixed

- C ABI 标量句柄表的 use-after-free：`with_engine` 原来在释放句柄表锁后才解引用
  裸指针，并发 `eb_plain_close` 可以在两步之间释放对象。现在表里存 `Arc<EbEngine>`，
  查表即克隆强引用，close 只摘表。
- Web 打开书泄漏整本字节：`open_book` 不再 `Box::leak`，书字节所有权登记在
  `BOOK_BYTES`，换书 / 新增的 `close_book()` / `eb_close_book` 会先丢掉借用者再
  按原分配回收。
- 解码后的 IR 遇到越界 span 会 panic（`release` 是 `panic = "abort"`，等于杀进程）：
  `Span::end` 改为饱和，`DisplayList::op_slice` / `glyph_slice` 越界返回空切片。
- 单个块的内联片段超过 65535 时 `Ch.piece` 的 `u16` 回绕，样式/链接会指到错误
  片段：片段下标改为 `u32`，并加 65537 片段的回归用例。
- `text-align: start/end` 没有按 `direction: rtl` 翻转，RTL 正文对齐是反的。
- `max_lines` 在 flow 之后才拦截，20MB 文本仍会排完整章：预算现在传进断行循环，
  超限立即收手（`truncated` + 单条 `LimitExceeded("max_lines")`）。
- `text-transform` 改变字符个数时（`ß` → `SS`）`LineBox.src` 末尾少算原文字节，
  选区会缺字符。
- `eb_last_error` 每次失败都 `mem::forget` 一条 `Vec<u8>`：现在错误串是原地
  复用的缓冲，连续失败不换指针、也不泄漏。
- `eb_version` 写死 `ebook-abi 0.1.0`，与 workspace 的 `0.2.1` 脱节：现在由
  `CARGO_PKG_VERSION` 编进来。
- 标量面返回的字符串指针原来是 thread-local，跨线程用（或换线程再取）就失效，
  只靠「调用都在主线程」的口头约定：现在是进程级、带 `Mutex` 的环形缓冲，
  任意线程调用都安全，寿命写进头文件（后续 64 次取字符串）。
- 无效句柄上的字符串 getter 返回 `NULL`：现在统一返回空串，宿主不必逐处判空。
- 外部链接的 `EbLink.spine` 沿用 reader 的 `0`（「相对当前章」没意义）：标量面
  统一说 `-1`，宿主只看 `spine` 也不会误跳第 1 章。
- `eb_plain_select` 的端点越界会返回 0 块矩形并留下一份空选区：现在给 `-2`
  并清空，不留下对不上号的矩形表。

- `web/selection.js` 漏解构 `onSwipeStart` / `onSwipeMove` / `onSwipeEnd` /
  `onSwipeCancel`，横向滑动一按就抛 `ReferenceError`。
- 拖选把每个 `pointermove` 都发一次引擎查询：现在合并到一个动画帧，晚到的结果
  按序号丢弃。
- PWA 冷启动：导航请求改为 cache-first + 后台回源，iOS 从主屏启动不再等
  一次网络往返（1s 白屏的主要来源）。预缓存逐项容错，单个图标失败不会让
  install 整体失败；`latin.ttf` 移出安装清单，首次用到时再缓存。
- iOS 启动图：按机型生成 `web/splash/*.png` 的 `apple-touch-startup-image`，
  并补 180px 主屏图标，避免页面首帧前的纯白闪屏。
- SW 在 wasm 就绪后立即注册；书架读取与 wasm 加载并行，开屏先出书。
- 未带哈希的 JS 改为 `Cache-Control: no-cache`，避免 Service Worker
  预缓存时被浏览器 HTTP 缓存挡住。发布时若改了 JS，仍需同步升 `CACHE_NAME`。

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
  `Arc<Vec<u8>>` 与度量/光栅共享，不额外复制。公共 `TextMeasurer::shape`
  契约（逻辑序、簇首字节偏移、返回宽等于字形宽之和）不变。
- 去掉 `ttf-parser`：cmap、upem、ascent/descent、advance 与 glyf/CFF 轮廓
  全部改由 `read-fonts`（harfrust 的底层）读取，光栅器仍是 `ab_glyph_rasterizer`；
  wasm gzip 反而小了约 11 KB。
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

- **重进章节时图片变占位或串图**：`web/reader.js` 在本章第一次 `render()` 之后才设置
  `chapterImages`，已解码过的图片不会触发重绘，因此第二次进入同一章时首帧仍用上一章的
  `id → href` 映射，画出错图或占位（例如封面翻到下一章再翻回来）。现在图片表在首帧前
  就位；`web/tests/images.mjs` 增加「进入第二章再返回第一章」的回归。图片本身没有
  被释放，`imageCache` 只在开书/关书时清空。
- **图片不再溢出页面**：块级与行内图片的尺寸超过可用宽度时按宽高比等比缩到容器内
  （只缩不放），并修正 `width`/`height` 只给一维时的另一边推导。此前排版层完全
  按固有尺寸出图，手机上宽度 1000+ px 的插图只能看到左边一块。`max-width` /
  `min-width` 等尺寸属性仍未进入级联，当前行为等同于阅读器默认的「适配页宽」。
- **脏 XHTML 不再整章失败**：新增 `html_repair` 容错层（tokenizer + HTML 树构造
  恢复），严格 XML 解析失败后自动修复并重试。覆盖裸属性值（`href=http://...`）、
  裸 `&`、未闭合 void 元素、错配结束标签、可选结束标签、重复属性、多根片段、
  大小写不敏感等真实世界的 HTML 写法；仍失败时退化成纯文本，保证每章至少能
  出字。OPF / container / nav / NCX 走同一入口，避免一本坏 OPF 导致整本打不开。
  诊断码新增 `RepairedXhtml` / `TextFallback`。
- 实体展开在 `&` 后 64 字节内没有分号、且窗口落在多字节字符中间时会 panic；
  现已退到字符边界并补回归测试。
- 文本解码按浏览器顺序补齐：BOM → 无 BOM UTF-16 → 合法 UTF-8 → XML/`<meta
  charset>` → `chardetng` → UTF-8 lossy。native 构建用 `encoding_rs` 支持
  GBK / Big5 / Shift_JIS / EUC-KR / windows-125x；wasm 为体积预算保留
  latin-1/windows-1252，其余旧编码仍由宿主先解码。
- Calibre 常见的 `<svg><image xlink:href="cover.jpeg"/></svg>` 封面现在识别为
  图片，并用 `width`/`height` 作为固有尺寸；纯图片封面不再排版出 0 页。
- `<!DOCTYPE html>` 之外的坏 XHTML（未闭合标签、裸属性等）不再被判成
  `Malformed("xhtml")`；超深/超限章节降级为可读的纯文本而不是白屏。
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
