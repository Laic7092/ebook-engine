/* ebook_abi.h — ebook-engine 的 C ABI。
 *
 * 手写声明，与 `crates/ebook-abi/src` 下的 `pub extern "C" fn` 一一对应：
 * `tests/header.rs` 会扫 Rust 源码里的 `pub extern "C" fn`，这里漏一个、
 * 或者参数形状对不上，测试就红。CI 的 release 资产（`libebook_abi.a`）就是
 * 按这份头文件链接的。
 *
 * 两个面：
 *
 *   1. postcard 面 —— `eb_open` / `eb_request` / `eb_close`。一条入口收发
 *      `Request`/`Response` 枚举（postcard 编码，schema 见
 *      `crates/ebook-abi/src/protocol.rs`）。适合能读返回缓冲区的宿主（cgo、
 *      Swift、Rust）。
 *   2. 标量面 —— `eb_plain_*`。参数与结果都是标量/字符串/结构体，像素与封面
 *      由宿主给路径、库写文件。适合只能拿 `Pointer` 不读内存的宿主（Asuka 的
 *      `extern`，JNI 的薄包装）。
 *
 * 三者共用的约定：
 *
 *   - **panic 不出边界**。所有入口 `catch_unwind`；panic 变成 `Error` 响应或
 *     负返回值，不会让进程 abort。因此 native 构建必须是 `panic = "unwind"`
 *     （cargo 的 `native` profile 已经这么配）。
 *   - **缓冲区归调用方**。`eb_alloc` / `eb_request` 返回的指针必须
 *     `eb_free(ptr, len)`。
 *   - **返回的 `const char *` 归库**，不要 free。有效期内是 NUL 结尾的 UTF-8：
 *       * `eb_version` / `eb_plain_version`：进程生命周期
 *       * `eb_plain_last_error` / `eb_last_error`：直到下一次失败的调用
 *       * 其余 `eb_plain_*` 字符串：直到后续 64 次取字符串（环形缓冲）；
 *         要长期保存就当场 `strdup`
 *   - **整数句柄**：`eb_plain_open` 返回 `>= 1`，失败 `-1`；其余入口对无效
 *     句柄返回 0 / `-1` / 空串 / `ok == 0` 的结构体，不 panic。
 *   - **线程**：句柄表、字符串环、错误缓冲都是进程级互斥；任意线程调用安全，
 *     同一本书上的调用串行化。返回的字符串不是 TLS，可以跨线程用（在生命周期
 *     内）。
 */

#ifndef EBOOK_ABI_H
#define EBOOK_ABI_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ---------------------------------------------------------------- 版本 */

/* 结构布局版本，对应 `ebook_abi::ABI_VERSION`。任何 `repr(C)` 结构体加字段、
 * 改字段顺序或改既有函数签名都会 +1。宿主启动时：
 *
 *     if (eb_abi_version() != EB_ABI_VERSION) { 拒绝加载 }
 *
 * 只新增函数不算破坏性变更。 */
#define EB_ABI_VERSION 1

/* `eb_struct_size` 的 kind。宿主可用它做二次校验（头文件与库同源）。 */
#define EB_STRUCT_PAGE_INFO 1
#define EB_STRUCT_LOC 2
#define EB_STRUCT_SELECTION_RECT 3
#define EB_STRUCT_LINK 4

/* 库版本串，如 "ebook-abi 0.2.1"。静态内存，永不失效。 */
const char *eb_version(void);
/* 结构布局版本；应与 EB_ABI_VERSION 相等。 */
uint32_t eb_abi_version(void);
/* 第 kind 个 `repr(C)` 结构体的字节数；未知 kind 返回 0。 */
size_t eb_struct_size(uint32_t kind);

/* ---------------------------------------------------------------- 缓冲区 */

/* 分配 len 字节交给宿主写请求；len 为 0 返回 NULL。 */
uint8_t *eb_alloc(size_t len);
/* 释放 eb_alloc / eb_request 返回的缓冲区。len 必须与当初一致。 */
void eb_free(uint8_t *ptr, size_t len);

/* ---------------------------------------------------------------- postcard 面 */

/* 不透明句柄，内部是 `Mutex<Engine>`。 */
typedef struct EbEngine EbEngine;

/* 打开一本书（EPUB 或纯文本，按魔数识别）。失败返回 NULL，原因见
 * eb_last_error。`name` 可以为 NULL。 */
EbEngine *eb_open(const uint8_t *bytes, size_t len, const char *name);
/* 关掉并释放句柄；只能关一次。 */
void eb_close(EbEngine *engine);

/* 上次打开失败的细节（UTF-8，**不带**结尾 0；`*len` 是字节数）。
 * 空串表示没有错误。指针在下一次失败前有效，不要 free。 */
const uint8_t *eb_last_error(size_t *len);

/* 收一个 postcard `Request`，回一个 postcard `Response`。
 *
 * 返回的缓冲区用 eb_free 释放；`*out_len == 0` 时返回 NULL（只有请求本身
 * 不是合法 postcard 才会发生——业务错误也会编码成 `Response::Error`）。
 *
 * 线格式（postcard 是 varint 长度前缀 + 字段顺序，不含字段名）：
 *
 *   Request   tag 0..15 对应
 *             Layout / Bitmap / Hit / LinkAt / Selection / Progress /
 *             FragmentLine / ResolveLink / RegisterFont / RegisterFontFace /
 *             BaseFontSize / Book / Toc / Pages / Limits / Ping
 *   Response  tag 0..13 对应
 *             Ok / Pong / Book / Toc / Pages / Bitmap / Hit / Link /
 *             Selection / Progress / Line / Target / Font / Number / Error
 *
 * 字段类型与顺序见 `crates/ebook-abi/src/protocol.rs`；同一份 schema 也被
 * `tests/ffi.rs` 逐字段走一遍。标量面能做的事这里都能做（还多一个
 * `Selection` 的矩形数组），选它只是宿主能不能读内存的区别。 */
uint8_t *eb_request(EbEngine *engine, const uint8_t *req, size_t len, size_t *out_len);

/* ---------------------------------------------------------------- 标量面 */

/* 一页的元数据。按值回传，所以是 `repr(C)`。
 * width/height 是内容盒（视口减留白），不含留白。 */
typedef struct EbPageInfo {
  int32_t ok;        /* 1 有，0 没有 */
  float width;
  float height;
  float progress;    /* 本章内进度 0..1 */
} EbPageInfo;

/* 文本位置。page == -1 表示没有。 */
typedef struct EbLoc {
  int32_t page;
  uint32_t line;
  uint32_t byte;
  float progress;    /* 全书进度 0..1（-1 未知） */
} EbLoc;

/* 选区里的一块矩形。ok == 0 表示没有这一块。 */
typedef struct EbSelectionRect {
  int32_t ok;
  uint32_t page;
  float x;
  float y;
  float w;
  float h;
} EbSelectionRect;

/* 链接解析结果。ok == 0 表示解不出。外部链接也算解出（external == 1）。 */
typedef struct EbLink {
  int32_t ok;
  int32_t spine;     /* -1：不在阅读顺序里 */
  int32_t external;  /* 1 外部链接，0 书内 */
} EbLink;

/* 标量面的版本检查（与上面两个同义，只是给不用 postcard 面的宿主）。 */
const char *eb_plain_version(void);
uint32_t eb_plain_abi_version(void);

/* 最近一次失败的原因（NUL 结尾，"" 没有）。在下一次失败前有效。 */
const char *eb_plain_last_error(void);

/* 打开一本书，返回句柄（>= 1）；失败 -1，原因见 eb_plain_last_error。 */
int32_t eb_plain_open(const uint8_t *data, size_t len, const char *name);
/* 关掉并释放句柄。0 好，-1 没有这个句柄。 */
int32_t eb_plain_close(int32_t id);

/* ---- 书目 ---- */

const char *eb_plain_title(int32_t id);        /* "" 没有 */
const char *eb_plain_language(int32_t id);     /* dc:language，退回 lang */
const char *eb_plain_creator(int32_t id);      /* dc:creator */
const char *eb_plain_publisher(int32_t id);
const char *eb_plain_identifier(int32_t id);   /* 通常是 ISBN/UUID */
const char *eb_plain_date(int32_t id);         /* 原始字符串，不解析 */
const char *eb_plain_description(int32_t id);
const char *eb_plain_cover_href(int32_t id);   /* 归档内路径，"" 没有 */

/* ---- 封面 ----
 *
 * 封面字节只走文件：宿主给路径，库写 PNG/JPEG 原始字节。
 * 扩展名/解码方式由 eb_plain_cover_mime 决定。 */
const char *eb_plain_cover_mime(int32_t id);              /* "image/png" / "" */
size_t eb_plain_cover_len(int32_t id);                    /* 0 没有封面 */
int32_t eb_plain_cover_write(int32_t id, const char *path); /* 0 好，-1 没有，-3 写失败 */

/* ---- 阅读顺序（spine）---- */

uint32_t eb_plain_spine_count(int32_t id);
const char *eb_plain_spine_href(int32_t id, uint32_t index);        /* 归档内路径 */
const char *eb_plain_spine_id(int32_t id, uint32_t index);          /* manifest id */
const char *eb_plain_spine_media_type(int32_t id, uint32_t index);
int32_t eb_plain_spine_linear(int32_t id, uint32_t index);          /* 1 在阅读顺序里 */
/* 最近一次排版的章节下标（-1 没有）。 */
int32_t eb_plain_spine(int32_t id);

/* ---- 目录（TOC）---- */

uint32_t eb_plain_toc_count(int32_t id);
const char *eb_plain_toc_title(int32_t id, uint32_t index);
const char *eb_plain_toc_href(int32_t id, uint32_t index);      /* 不含 fragment */
const char *eb_plain_toc_fragment(int32_t id, uint32_t index);  /* 不带 '#'，"" 没有 */
int32_t eb_plain_toc_spine(int32_t id, uint32_t index);         /* -1 不在阅读顺序里 */
uint32_t eb_plain_toc_depth(int32_t id, uint32_t index);        /* 0 起算 */

/* ---- 字体 ----
 *
 * 排版前至少注册一个字体；没有字体时页照样出，但位图上没有字。
 * 返回字体 id（>= 0）；-1 句柄/参数错，-2 字体解析失败。 */
int32_t eb_plain_register_font(int32_t id, const uint8_t *bytes, size_t len);
/* 注册字体集合（TTC/OTC）里的第 index 个面，先注册整份文件。 */
int32_t eb_plain_register_font_face(int32_t id, uint32_t index);
uint32_t eb_plain_font_count(int32_t id);

/* ---- 排版 ----
 *
 * 一次调用带全参数：ABI 不保存 viewport/style。
 * width/height 是视口（含留白），top/right/bottom/left 是留白，
 * font_scale/line_scale 是倍率，letter_px/word_px 是额外字距/词距（点），
 * align：0 原书、1 左、2 中、3 右、4 两端。
 * 0 好；-1 句柄无效；-2 排版失败（原因见 eb_plain_last_error）。 */
int32_t eb_plain_layout(int32_t id, uint32_t spine, float width, float height,
                        float top, float right, float bottom, float left,
                        float font_scale, float line_scale, float letter_px,
                        float word_px, uint32_t align);

/* 同上，但把整章排成一页滚动流：height 只当视口高度（不切页），
 * top/bottom 应传 0（滚动容器自己留白）。
 * 排完 eb_plain_content_height 是内容总长，eb_plain_page_slice_raw 按 y 取瓦片。 */
int32_t eb_plain_layout_scroll(int32_t id, uint32_t spine, float width, float height,
                               float top, float right, float bottom, float left,
                               float font_scale, float line_scale, float letter_px,
                               float word_px, uint32_t align);

/* 排出来的页数；滚动模式下是 1。 */
uint32_t eb_plain_page_count(int32_t id);
/* 滚动模式的内容高度（本章流坐标总长）；分页模式/没排版时 0。 */
float eb_plain_content_height(int32_t id);
/* 一页的逻辑尺寸与本章进度。ok == 0 表示没有这一页。 */
EbPageInfo eb_plain_page_info(int32_t id, uint32_t page);

/* ---- 像素 ----
 *
 * 位图一律写成 EQP1 文件（`eb_plain_page_raw` / `eb_plain_page_slice_raw`）：
 *
 *   "EQP1" | u32le width | u32le height | width*height*4 bytes RGBA8
 *
 * 行优先，直通 alpha（非预乘）。`scale` 是设备像素比，`ink`/`paper` 是
 * 0xRRGGBBAA 主题色（0 = 不修正 / 白纸），`bold` 全局加粗。
 * 位图覆盖整个视口（含留白），不是内容盒。
 * 返回：0 好，-1 句柄/路径错，-2 光栅化失败，-3 写文件失败。 */
int32_t eb_plain_page_raw(int32_t id, uint32_t page, float scale, uint32_t ink,
                          uint32_t paper, bool bold, const char *path);

/* 滚动流的一个窗口：y/height 是点（y 在 [0, content_height) 里）。
 * 同一个 y 再来一次得到逐字节相同的文件。 */
int32_t eb_plain_page_slice_raw(int32_t id, uint32_t page, float y, float height,
                                float scale, uint32_t ink, uint32_t paper,
                                bool bold, const char *path);

/* ---- 位置与交互 ---- */

/* 页内点（点坐标）→ 原文位置；page == -1 表示没有命中。 */
EbLoc eb_plain_hit(int32_t id, uint32_t page, float x, float y);
/* 位置 → 全书进度 0..1（-1 未知）。 */
float eb_plain_progress(int32_t id, uint32_t page, uint32_t line, uint32_t byte);
/* 页内点 → 链接地址（"" 没有）。 */
const char *eb_plain_link_at(int32_t id, uint32_t page, float x, float y);
/* 当前章 "#fragment" 的行号；-1 本章没有这个锚点。 */
int32_t eb_plain_fragment_line(int32_t id, const char *fragment);

/* 选区。端点按 (page, line, byte) 给，顺序无所谓（库会规范化）。
 * 返回矩形块数（>= 0）；-1 句柄/排版缺失，-2 端点越界。
 * 结果留在句柄上，用下面几个取；下一次 eb_plain_select 覆盖。 */
int32_t eb_plain_select(int32_t id, uint32_t a_page, uint32_t a_line, uint32_t a_byte,
                        uint32_t b_page, uint32_t b_line, uint32_t b_byte);
uint32_t eb_plain_selection_rect_count(int32_t id);
EbSelectionRect eb_plain_selection_rect(int32_t id, uint32_t index);
const char *eb_plain_selection_text(int32_t id);   /* "" 没有 */
EbLoc eb_plain_selection_start(int32_t id);        /* page == -1 没有 */
EbLoc eb_plain_selection_end(int32_t id);

/* 在最近一次排版的那一章里解析书内链接。ok == 0 表示解不出。
 * fragment 单独取，在下一次解析前有效。 */
EbLink eb_plain_resolve_link(int32_t id, const char *href);
const char *eb_plain_resolved_fragment(int32_t id); /* 不带 '#'，"" 没有 */

/* 资源上限：单页像素与书级图片缓存（解码后的 RGBA 字节数）。
 * 默认 4096×4096 / 64 MiB。0 好，-1 句柄无效。 */
int32_t eb_plain_set_limits(int32_t id, uint64_t max_page_pixels, uint64_t image_cache_bytes);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* EBOOK_ABI_H */
