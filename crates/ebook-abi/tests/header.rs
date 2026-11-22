//! `include/ebook_abi.h` 与 Rust 侧的一致性。
//!
//! 两件事：
//! 1. 头文件真的能被 C11 编译（`-Wall -Wextra -Werror`），且
//!    `_Static_assert(sizeof(X) == size_of::<X>())` 逐字段对得上——
//!    结构体加字段而忘了改版本，这里会红。
//! 2. 头文件声明的函数集合与 `src/` 里 `pub extern "C" fn` 的集合相等。
//!    漏一个、拼错一个、删了源码没删声明，都会红。

use std::collections::BTreeSet;

const HEADER: &str = include_str!("../include/ebook_abi.h");

/// 头文件里声明可以出现的返回类型（声明行以它们开头）。
const RETURN_TYPES: [&str; 16] = [
    "const char",
    "const uint8_t",
    "uint8_t",
    "uint32_t",
    "uint64_t",
    "size_t",
    "int32_t",
    "void",
    "float",
    "EbEngine",
    "EbPageInfo",
    "EbLoc",
    "EbSelectionRect",
    "EbLink",
    "EbScrollChapter",
    "EbScrollLoc",
];

fn rust_symbols() -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for source in [
        include_str!("../src/lib.rs"),
        include_str!("../src/plain.rs"),
    ] {
        for line in source.lines() {
            let Some(rest) = line.split(r#"extern "C" fn "#).nth(1) else {
                continue;
            };
            let name = rest.split(['(', '<']).next().unwrap_or("").trim();
            if !name.is_empty() {
                out.insert(name.to_string());
            }
        }
    }
    out
}

fn header_symbols() -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in HEADER.lines() {
        let line = line.trim();
        if !RETURN_TYPES.iter().any(|kind| line.starts_with(kind)) {
            continue;
        }
        // 声明行：`<type> <ptr>* name(`，名字一定以 eb_ 开头。
        for token in line.split([' ', '*', '(']) {
            let token = token.trim();
            if token.starts_with("eb_") && !token.is_empty() {
                out.insert(token.to_string());
            }
        }
    }
    out
}

#[test]
fn header_declares_exactly_the_exported_symbols() {
    let rust = rust_symbols();
    let header = header_symbols();
    let missing: Vec<_> = rust.difference(&header).cloned().collect();
    let stale: Vec<_> = header.difference(&rust).cloned().collect();
    assert!(missing.is_empty(), "头文件缺这些导出: {missing:?}");
    assert!(stale.is_empty(), "头文件声明了不存在的导出: {stale:?}");
    assert!(
        rust.len() > 40,
        "只扫到 {} 个导出，扫描逻辑坏了",
        rust.len()
    );
}

/// `pub extern "C" fn` 只有带上 `#[no_mangle]` 才会以 C 名字导出。文档注释
/// 里的 `#[no_mangle]` 只是文字：它必须真的是紧挨着函数的一项属性。头文件
/// 测试只比对名字，抓不到这个，所以在这里扫一遍源码。
#[test]
fn every_c_export_is_no_mangle() {
    for (file, source) in [
        ("src/lib.rs", include_str!("../src/lib.rs")),
        ("src/plain.rs", include_str!("../src/plain.rs")),
    ] {
        let lines: Vec<&str> = source.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            let declaration = line.trim_start();
            if !(declaration.starts_with("pub extern \"C\" fn ")
                || declaration.starts_with("pub unsafe extern \"C\" fn "))
            {
                continue;
            }
            let mut no_mangle = false;
            for prev in lines[..i].iter().rev() {
                let prev = prev.trim();
                if prev.starts_with("#[") {
                    if prev == "#[no_mangle]" {
                        no_mangle = true;
                    }
                    continue;
                }
                if prev.is_empty() || prev.starts_with("//") {
                    continue;
                }
                break;
            }
            assert!(
                no_mangle,
                "{file}:{}: this C export has no #[no_mangle] attribute:\n{}",
                i + 1,
                line
            );
        }
    }
}

#[test]
fn header_version_matches_the_library() {
    let declared = HEADER
        .lines()
        .find_map(|line| line.trim().strip_prefix("#define EB_ABI_VERSION "))
        .and_then(|value| value.trim().parse::<u32>().ok())
        .expect("头文件没有 EB_ABI_VERSION");
    assert_eq!(declared, ebook_abi::ABI_VERSION, "结构布局版本漂移");
}

#[test]
fn struct_sizes_match_the_header_kinds() {
    use std::mem::size_of;
    assert_eq!(
        ebook_abi::eb_struct_size(ebook_abi::plain::EB_STRUCT_PAGE_INFO),
        size_of::<ebook_abi::plain::EbPageInfo>()
    );
    assert_eq!(
        ebook_abi::eb_struct_size(ebook_abi::plain::EB_STRUCT_LOC),
        size_of::<ebook_abi::plain::EbLoc>()
    );
    assert_eq!(
        ebook_abi::eb_struct_size(ebook_abi::plain::EB_STRUCT_SELECTION_RECT),
        size_of::<ebook_abi::plain::EbSelectionRect>()
    );
    assert_eq!(
        ebook_abi::eb_struct_size(ebook_abi::plain::EB_STRUCT_LINK),
        size_of::<ebook_abi::plain::EbLink>()
    );
    assert_eq!(
        ebook_abi::eb_struct_size(ebook_abi::plain::EB_STRUCT_SCROLL_CHAPTER),
        size_of::<ebook_abi::plain::EbScrollChapter>()
    );
    assert_eq!(
        ebook_abi::eb_struct_size(ebook_abi::plain::EB_STRUCT_SCROLL_LOC),
        size_of::<ebook_abi::plain::EbScrollLoc>()
    );
    assert_eq!(ebook_abi::eb_struct_size(0), 0, "未知 kind 应为 0");
}

#[test]
fn header_compiles_and_agrees_with_rust_layout() {
    use std::mem::size_of;

    let dir = std::env::temp_dir().join(format!("eb-abi-header-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("建临时目录");
    let probe = dir.join("probe.c");
    let version = HEADER
        .lines()
        .find_map(|line| line.trim().strip_prefix("#define EB_ABI_VERSION "))
        .map(str::trim)
        .unwrap_or_default();
    let source = format!(
        r#"#include "ebook_abi.h"

_Static_assert(EB_ABI_VERSION == {version}, "ABI version");
_Static_assert(EB_STRUCT_PAGE_INFO == 1, "kind 1");
_Static_assert(EB_STRUCT_LOC == 2, "kind 2");
_Static_assert(EB_STRUCT_SELECTION_RECT == 3, "kind 3");
_Static_assert(EB_STRUCT_LINK == 4, "kind 4");
_Static_assert(EB_STRUCT_SCROLL_CHAPTER == 5, "kind 5");
_Static_assert(EB_STRUCT_SCROLL_LOC == 6, "kind 6");

_Static_assert(sizeof(EbPageInfo) == {page_info}, "EbPageInfo");
_Static_assert(sizeof(EbLoc) == {loc}, "EbLoc");
_Static_assert(sizeof(EbSelectionRect) == {rect}, "EbSelectionRect");
_Static_assert(sizeof(EbLink) == {link}, "EbLink");
_Static_assert(sizeof(EbScrollChapter) == {scroll_chapter}, "EbScrollChapter");
_Static_assert(sizeof(EbScrollLoc) == {scroll_loc}, "EbScrollLoc");

_Static_assert(sizeof(float) == 4, "f32");
_Static_assert(sizeof(uint64_t) == 8, "u64");

void probe_types(void) {{
  EbEngine *engine = eb_open((const uint8_t *)0, 0, (const char *)0);
  eb_close(engine);
  EbPageInfo info = eb_plain_page_info(-1, 0);
  EbLoc loc = eb_plain_hit(-1, 0, 0.0f, 0.0f);
  EbSelectionRect box = eb_plain_selection_rect(-1, 0);
  EbLink link = eb_plain_resolve_link(-1, (const char *)0);
  EbScrollChapter chapter = eb_plain_scroll_chapter(-1, 0);
  EbScrollLoc hit = eb_plain_scroll_hit(-1, 0.0f, 0.0f);
  size_t len = 0;
  uint8_t *bytes = eb_plain_scroll_band_bytes(-1, 0.0f, 1.0f, 1.0f, 0, 0, false, &len);
  (void)info; (void)loc; (void)box; (void)link; (void)chapter; (void)hit; (void)bytes;
}}
"#,
        version = version,
        page_info = size_of::<ebook_abi::plain::EbPageInfo>(),
        loc = size_of::<ebook_abi::plain::EbLoc>(),
        rect = size_of::<ebook_abi::plain::EbSelectionRect>(),
        link = size_of::<ebook_abi::plain::EbLink>(),
        scroll_chapter = size_of::<ebook_abi::plain::EbScrollChapter>(),
        scroll_loc = size_of::<ebook_abi::plain::EbScrollLoc>(),
    );
    std::fs::write(&probe, source).expect("写 probe.c");

    let include = concat!(env!("CARGO_MANIFEST_DIR"), "/include");
    let output = if cfg!(windows) {
        std::process::Command::new("cl")
            .args(["/nologo", "/std:c11", "/W4", "/WX", "/c"])
            .arg(&probe)
            .arg(format!("/I{include}"))
            .arg(format!("/Fo{}.obj", dir.join("probe").display()))
            .output()
    } else {
        let compiler = std::env::var("CC").unwrap_or_else(|_| "cc".to_string());
        std::process::Command::new(compiler)
            .args(["-std=c11", "-Wall", "-Wextra", "-Werror", "-fsyntax-only"])
            .arg("-I")
            .arg(include)
            .arg(&probe)
            .output()
    }
    .expect("C 编译器没跑起来（设 CC 指定它）");

    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        output.status.success(),
        "头文件编译失败:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
