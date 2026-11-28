use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use ebook_font::FontBook;
use ebook_ir::{
    codec, EdgesF, Fit, ImageData, ImageId, LayoutResult, RectF, Resources, Rgba8, Surface,
};
use ebook_layout::{Layout, LayoutEnv, NoImageSizes, PageConstraint, StubMeasurer};
use ebook_raster::RgbaSurface;

// 512 KiB 是原始预算。EPUB 容错（ZIP64、编码回退、病态输入限额）约 +21 KB；
// WOFF/WOFF2 的 Brotli 解码器约 +105 KB（WOFF1 复用 ebook-format 已有的
// miniz_oxide，不再额外链 flate2）；read-fonts 替换 ttf-parser 约 -11 KB。
//
// Rust 1.100.0 实测：core 326,698 B，core+epub 706,186 B。全文搜索
// （ebook-search + wasm 搜索面板 + ABI 变体）让 core+epub 比 0.4.1 的
// 679,015 B 增加约 21 KB；0.6.0 的版式档案、页级/行级 DP 与孤字修复再加约
// 6.5 KB（量表在 `quality` feature 后面，不进 wasm）。预算从 680 KiB 抬到 700 KiB。
// 构成上 code 占 gzip 的 73%、data 占 25%；其中 epub 适配器独占约 450 KB
// （zune-jpeg ~30 KB、wuff+brotli ~25 KB、roxmltree ~12 KB）。这些零件都在
// Web 上真干活（引擎自己解 JPEG、自己解 woff2），做条件编译会掉功能，换不来
// 1% 的体积，所以这里抬高预算而不是加 feature。
// 顺带记录已排除的调参项：opt-level="s" 反而 +33 KB；wasm-opt -Oz 只 -0.3% gzip。
const GZIP_BUDGET: usize = 700 * 1024;
fn target_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target"))
}

fn wasm_path() -> PathBuf {
    wasm_path_in(&target_dir())
}

fn wasm_path_in(dir: &Path) -> PathBuf {
    dir.join("wasm32-unknown-unknown/release/ebook_web.wasm")
}

/// 不带 feature 的 wasm 单独一个 target 目录：size/parity 在 lean 上跑，
/// app/size full 在默认 target 上跑，避免来回切换 feature 触发重复编译。
fn lean_dir() -> PathBuf {
    target_dir().join("xtask-lean")
}

fn parity_dir() -> PathBuf {
    target_dir().join("parity")
}

fn feature_key(features: &[&str]) -> String {
    if features.is_empty() {
        String::from("lean")
    } else {
        features.join("+")
    }
}

/// wasm 只在 Rust 源码/清单变化后才需要重编；用最新 mtime 做廉价判断。
///
/// `tests/` / `examples/` / `benches/` 不进 wasm 产物：改一个测试不该触发
/// 一整遍 wasm release 重编（那是分钟级的，和测试本身无关）。
fn newest_source_time() -> SystemTime {
    fn visit(dir: &Path, newest: &mut Option<SystemTime>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() {
                let skipped = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| matches!(name, "tests" | "examples" | "benches"))
                    .unwrap_or(false);
                if skipped {
                    continue;
                }
                visit(&path, newest);
            } else if kind.is_file() {
                if let Ok(time) = entry.metadata().and_then(|meta| meta.modified()) {
                    match *newest {
                        Some(current) if time <= current => {}
                        _ => *newest = Some(time),
                    }
                }
            }
        }
    }

    let mut newest = None;
    for path in [
        "Cargo.toml",
        "Cargo.lock",
        "rust-toolchain.toml",
        "build.rs",
    ] {
        if let Ok(time) = std::fs::metadata(path).and_then(|meta| meta.modified()) {
            match newest {
                Some(current) if time <= current => {}
                _ => newest = Some(time),
            }
        }
    }
    visit(Path::new("crates"), &mut newest);
    newest.unwrap_or(SystemTime::UNIX_EPOCH)
}

/// `render <书> <页号> <输出.bmp> [lang]`：把某一页真的画出来，用眼睛判断。
///
/// 数字说得清的只有几何；「这一页看着松不松」只能看。BMP 是因为它不用编解码器：
/// 24 位、自底向上、行按 4 字节对齐，三十行就能写完，WSL 和 Windows 都能打开。
fn render_page() {
    let args: Vec<String> = std::env::args().skip(2).collect();
    if args.len() < 3 {
        panic!("用法：render <书> <页号> <输出.bmp> [lang]");
    }
    let (path, page, out) = (&args[0], &args[1], &args[2]);
    let lang = args.get(3).cloned();
    let bytes = std::fs::read(path).expect("读不到文件");
    let name = std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut doc = report_doc_for(&name, bytes);
    doc.meta.lang = lang;
    let font = std::fs::read(std::env::var("PREVIEW_FONT").unwrap_or_else(|_| PIXEL_FONT.into()))
        .expect("字体");
    let mut fonts = FontBook::new();
    fonts.add(font).expect("字体");
    let env = LayoutEnv {
        measurer: &fonts,
        resources: &NoImageSizes,
    };
    let constraint = PageConstraint::new(600.0, 800.0).with_margin(EdgesF::all(36.0));
    let ir = Layout::new().layout(&doc, &env, &constraint);
    // `worst` = 段间距被撑得最开的那一页：翻页时最容易看出「这页松」的地方。
    let index: u32 = if page == "worst" {
        let mut worst = (0u32, 0.0f32);
        for (i, p) in ir.pages.iter().enumerate() {
            let span = p.lines.as_range();
            let mut sum = 0.0;
            let mut n = 0;
            for k in span.start + 1..span.end {
                let gap = ir.runs.lines[k].rect.y
                    - (ir.runs.lines[k - 1].rect.y + ir.runs.lines[k - 1].rect.h);
                if gap > 0.5 {
                    sum += gap;
                    n += 1;
                }
            }
            if n > 0 && sum / n as f32 > worst.1 {
                worst = (i as u32, sum / n as f32);
            }
        }
        eprintln!("段间距最开的是第 {} 页（平均 {:.1}px）", worst.0, worst.1);
        worst.0
    } else {
        page.parse().expect("页号")
    };
    let page = ir.page(index).expect("页号超范围");
    let mut surface = RgbaSurface::from_page(page);
    surface.render_page(&ir, index, &mut fonts).expect("render");
    let (w, h) = (surface.width() as usize, surface.height() as usize);
    let mut bmp = Vec::with_capacity(54 + w * h * 3);
    let row = w * 3 + (4 - (w * 3) % 4) % 4;
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&((54 + row * h) as u32).to_le_bytes());
    bmp.extend_from_slice(&[0, 0, 0, 0]);
    bmp.extend_from_slice(&54u32.to_le_bytes());
    bmp.extend_from_slice(&40u32.to_le_bytes());
    bmp.extend_from_slice(&(w as i32).to_le_bytes());
    bmp.extend_from_slice(&(h as i32).to_le_bytes());
    bmp.extend_from_slice(&1u16.to_le_bytes());
    bmp.extend_from_slice(&24u16.to_le_bytes());
    bmp.extend_from_slice(&[0; 24]);
    for y in (0..h).rev() {
        let line = &surface.pixels()[y * w..(y + 1) * w];
        for px in line {
            bmp.extend_from_slice(&[px.b, px.g, px.r]);
        }
        bmp.extend(core::iter::repeat_n(0u8, row - w * 3));
    }
    std::fs::create_dir_all(std::path::Path::new(out).parent().unwrap()).ok();
    std::fs::write(out, &bmp).expect("写文件");
    eprintln!("{out}: {}×{}，第 {index} 页", w, h);
}

/// `orphans <书> [lang]`：把量表上剩下的孤字一条条列出来，并说明为什么没修掉。
///
/// 报表上只有一个数，但要修它得知道「上一行退到哪儿、退完还放不放得下」。这个
/// 命令就是为了回答那个问题，省得靠猜。顺带汇报越出版心的行——那是另一种毛病
/// （挤压/悬挂/压缩没兜住），一起看才不会互相掩盖。
fn orphan_probe() {
    let args: Vec<String> = std::env::args().skip(2).collect();
    let path = args.first().expect("用法：orphans <书> [lang]");
    let mut doc = {
        let bytes = std::fs::read(path).expect("读不到文件");
        let name = std::path::Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        report_doc_for(&name, bytes)
    };
    doc.meta.lang = args.get(1).cloned();
    let ir = layout_doc(&doc);
    let text = &doc.text;
    let edge = ir.pages.first().map(|page| page.size.w).unwrap_or(0.0);
    let report = ebook_layout::quality::analyze(&ir, &StubMeasurer);
    let mut over = 0;
    for (index, line) in ir.runs.lines.iter().enumerate() {
        if line.rect.right() <= edge + 1.0 {
            continue;
        }
        over += 1;
        if over <= 3 {
            let src = &text[line.src.start as usize..line.src.end() as usize];
            eprintln!(
                "越界 {:.1}px 第{index}行 {:?}",
                line.rect.right() - edge,
                src
            );
        }
    }
    for &index in &report.orphan_lines {
        let index = index as usize;
        let line = &ir.runs.lines[index];
        let src = &text[line.src.start as usize..line.src.end() as usize];
        let prev_tail: String = if index == 0 {
            String::new()
        } else {
            let prev = &ir.runs.lines[index - 1];
            text[prev.src.start as usize..prev.src.end() as usize]
                .chars()
                .rev()
                .take(10)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect()
        };
        let prev_w = index
            .checked_sub(1)
            .map(|prev| ir.runs.lines[prev].rect.w)
            .unwrap_or(0.0);
        eprintln!(
            "第{index}行 末行={src:?} 宽{:.1}  上一行宽{prev_w:.1} 结尾…{prev_tail}",
            line.rect.w
        );
    }
    eprintln!("共 {} 个孤字，{over} 行越界", report.orphans);
}

const BENCH_WARMUP: usize = 3;
const BENCH_ITERS: usize = 31;
const PIXEL_GOLDEN: &str = "tests/golden/pixels.txt";
const PIXEL_BOOK: &str = "fixtures/typography.epub";
const PIXEL_FONT: &str = "fixtures/font/test.ttf";

fn main() -> ExitCode {
    let cmd = std::env::args()
        .nth(1)
        .unwrap_or_else(|| String::from("all"));
    match cmd.as_str() {
        "golden" => golden(false),
        "report" => quality_report(&std::env::args().skip(2).collect::<Vec<_>>()),
        "bless" => golden(true),
        "size" => size(),
        "parity" => parity(),
        "pixel" => pixel(false),
        "pixel-bless" => pixel(true),
        "contract" => contract(),
        "app" => app(),
        "bench" => bench(),
        "orphans" => {
            orphan_probe();
            ExitCode::SUCCESS
        }
        "render" => {
            render_page();
            ExitCode::SUCCESS
        }
        "all" => {
            let ok = [
                golden(false),
                contract(),
                app(),
                pixel(false),
                size(),
                parity(),
            ]
            .iter()
            .all(|code| *code == ExitCode::SUCCESS);
            if ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        _ => {
            eprintln!(
                "usage: xtask [all|golden|bless|report|orphans|render|contract|app|pixel|pixel-bless|size|parity]"
            );
            ExitCode::from(2)
        }
    }
}

/// 打开一本 fixture 的第一章。`ir_for` 和 `report` 共用它，保证两边排的是
/// 同一个文档。
fn doc_for(name: &str, bytes: Vec<u8>) -> ebook_ir::StyledDoc {
    let mut src = ebook_format::MemorySource::new();
    src.insert(name, bytes);
    let opened = ebook_format::open(&src).expect("format");
    opened.chapter(&src, 0).expect("chapter")
}

/// 排版报告用的一章：取**文本最长**的那章。
///
/// 真书的第 0 章常常是封面、版权页或者只放一张图，排出来 0 行；而报告要的是
/// 有代表性的正文。`ir_for`（golden）仍然固定第 0 章，不受影响。
fn report_doc_for(name: &str, bytes: Vec<u8>) -> ebook_ir::StyledDoc {
    // TXT 入口只认 UTF-8，别的编码会被替换成 U+FFFD 而不报错。报告里如果满屏
    // 乱码，那些数字一个都不能信，所以这里至少喊一声。
    if name.ends_with(".txt") || name.ends_with(".text") {
        let text = String::from_utf8_lossy(&bytes);
        let broken = text.chars().filter(|ch| *ch == '\u{fffd}').count();
        let total = text.chars().count().max(1);
        if broken * 50 > total {
            eprintln!(
                "警告：{name} 有 {:.0}% 的字符解不出来（TXT 入口只认 UTF-8）。\
                 先用 iconv -f GB18030 -t UTF-8 转一份再看报告。",
                100.0 * broken as f64 / total as f64
            );
        }
    }
    let mut src = ebook_format::MemorySource::new();
    src.insert(name, bytes);
    let opened = ebook_format::open(&src).expect("format");
    let mut best: Option<ebook_ir::StyledDoc> = None;
    for index in 0..opened.book.spine.len() {
        let Ok(doc) = opened.chapter(&src, index) else {
            continue;
        };
        if best
            .as_ref()
            .is_none_or(|best| best.text.len() < doc.text.len())
        {
            best = Some(doc);
        }
    }
    best.expect("empty book")
}

fn layout_doc(doc: &ebook_ir::StyledDoc) -> LayoutResult {
    let env = LayoutEnv {
        measurer: &StubMeasurer,
        resources: &NoImageSizes,
    };
    let constraint = PageConstraint::new(600.0, 800.0).with_margin(EdgesF::all(24.0));
    Layout::new().layout(doc, &env, &constraint)
}

fn ir_for(name: &str, bytes: Vec<u8>) -> Vec<u8> {
    codec::to_bytes(&layout_doc(&doc_for(name, bytes))).expect("encode")
}

fn demo_ir() -> Vec<u8> {
    ir_for(
        "demo.txt",
        std::fs::read("fixtures/demo.txt").expect("fixtures/demo.txt"),
    )
}

fn epub_ir() -> Vec<u8> {
    ir_for(
        "demo.epub",
        std::fs::read("fixtures/demo.epub").expect("fixtures/demo.epub"),
    )
}

fn golden(bless: bool) -> ExitCode {
    let cases = [
        ("tests/golden/demo.ir", demo_ir()),
        ("tests/golden/epub.ir", epub_ir()),
    ];
    let mut ok = true;
    for (name, actual) in cases {
        let path = Path::new(name);
        if bless {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            std::fs::write(path, &actual).expect("write golden");
            println!("golden written: {name} ({} bytes)", actual.len());
            continue;
        }
        match std::fs::read(path) {
            Ok(expected) if expected == actual => {
                println!("golden ok: {name} ({} bytes)", actual.len());
            }
            Ok(expected) => {
                let at = expected
                    .iter()
                    .zip(actual.iter())
                    .position(|(a, b)| a != b)
                    .unwrap_or_else(|| expected.len().min(actual.len()));
                eprintln!(
                    "golden mismatch in {name} at byte {at}: expected {} bytes, got {}",
                    expected.len(),
                    actual.len()
                );
                ok = false;
            }
            Err(e) => {
                eprintln!("golden missing: {name}: {e}");
                ok = false;
            }
        }
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn pixel_ir() -> LayoutResult {
    let bytes = std::fs::read(PIXEL_BOOK).expect(PIXEL_BOOK);
    let font = std::fs::read(PIXEL_FONT).expect(PIXEL_FONT);
    let mut src = ebook_format::MemorySource::new();
    src.insert("typography.epub", bytes);
    let opened = ebook_format::open(&src).expect("format");
    let doc = opened.chapter(&src, 0).expect("chapter");
    let mut fonts = FontBook::new();
    fonts.add(font).expect("font");
    let env = LayoutEnv {
        measurer: &fonts,
        resources: &NoImageSizes,
    };
    let constraint = PageConstraint::new(480.0, 640.0).with_margin(EdgesF::all(36.0));
    Layout::new().layout(&doc, &env, &constraint)
}

fn fnv(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn pixel_digest() -> Vec<String> {
    let ir = pixel_ir();
    let mut fonts = FontBook::new();
    let font = std::fs::read(PIXEL_FONT).expect(PIXEL_FONT);
    fonts.add(font).expect("font");
    let mut lines = Vec::new();
    for index in 0..ir.pages.len() as u32 {
        let page = ir.page(index).expect("page");
        let mut surface = RgbaSurface::from_page(page);
        surface.render_page(&ir, index, &mut fonts).expect("render");
        let mut raw = Vec::with_capacity(surface.pixels().len() * 4);
        for pixel in surface.pixels() {
            raw.extend_from_slice(&[pixel.r, pixel.g, pixel.b, pixel.a]);
        }
        lines.push(format!(
            "page {} {}x{} {:016x}",
            page.index,
            surface.width(),
            surface.height(),
            fnv(&raw)
        ));
    }
    lines.push(format!("pages {}", ir.pages.len()));
    lines
}

/// 版面质量报告：同一本书换档案排两遍，把「尺子」的数字并排印出来。
///
/// 这是判断一次改动是不是「变好了」的地方——golden 只说明「变了」。
/// 一把尺子需要一块有足够信号的样张：现有 fixture 都只有十几行，页底空洞、
/// 行数抖动这些列全是 0。
///
/// 这里造一本 300 段的中文书稿（约 3.7 万字、67 页）：段落长度和标点位置都由
/// 固定种子的 LCG 决定，所以每次运行逐字节相同；用千字文的字凑，避免和真书
/// 内容撞车。
fn synthetic_book() -> Vec<u8> {
    const CHARS: &str = "天地玄黄宇宙洪荒日月盈昃辰宿列张寒来暑往秋收冬藏闰余成岁律吕调阳\
                         云腾致雨露结为霜金生丽水玉出昆冈剑号巨阙珠称夜光果珍李柰菜重芥姜";
    const PUNCT: &[char] = &['，', '。', '；', '：', '、', '？', '！'];
    const LENGTHS: &[usize] = &[12, 25, 40, 60, 90, 140, 220, 320];
    let mut seed = 0x2024_0101u64;
    let mut next = move || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) as usize
    };
    let chars: Vec<char> = CHARS.chars().filter(|ch| !ch.is_whitespace()).collect();
    let mut text = String::new();
    for paragraph in 0..300 {
        if paragraph > 0 {
            text.push_str("\n\n");
        }
        let length = LENGTHS[next() % LENGTHS.len()];
        for index in 0..length {
            text.push(chars[next() % chars.len()]);
            if index % (9 + next() % 11) == 0 && index + 1 < length {
                text.push(PUNCT[next() % PUNCT.len()]);
            }
        }
    }
    text.into_bytes()
}

fn quality_report(paths: &[String]) -> ExitCode {
    let default: Vec<String> = [
        "fixtures/demo.txt",
        "fixtures/demo.epub",
        "fixtures/typography.epub",
    ]
    .iter()
    .map(|path| path.to_string())
    .collect();
    let books: Vec<String> = if paths.is_empty() {
        default
    } else {
        paths.to_vec()
    };
    let synthetic = paths.is_empty();
    println!(
        "{:<22} {:<9} {}  版心",
        "书",
        "档案",
        ebook_layout::quality::HEADER
    );
    let mut books = books;
    if synthetic {
        books.push(String::from("<合成长书>"));
    }
    for path in &books {
        let Ok(bytes) = std::fs::read(path).or_else(|_| {
            if path == "<合成长书>" {
                Ok(synthetic_book())
            } else {
                Err(std::io::Error::from(std::io::ErrorKind::NotFound))
            }
        }) else {
            println!("{path}: 读不到");
            continue;
        };
        // 合成长书没有文件名，直接给它一个。
        let name = if path.starts_with('<') {
            String::from("zh-long.txt")
        } else {
            Path::new(path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(path)
                .to_string()
        };
        let mut doc = report_doc_for(&name, bytes);
        let zh = doc
            .meta
            .lang
            .as_deref()
            .is_some_and(|lang| lang.to_ascii_lowercase().starts_with("zh"));
        // 同一本书、同一版面，只换档案：这才是可比的两行。
        let profiles: &[(&str, Option<&str>)] = if zh {
            &[("generic", None), ("zh-hans", Some("zh"))]
        } else {
            &[("generic", None)]
        };
        for (label, lang) in profiles {
            doc.meta.lang = lang.map(str::to_string);
            let ir = layout_doc(&doc);
            let measured = ebook_layout::quality::analyze(&ir, &StubMeasurer);
            println!("{path:<22} {label:<9} {measured}  {}", measured.measure);
        }
    }
    ExitCode::SUCCESS
}

fn pixel(bless: bool) -> ExitCode {
    let actual = pixel_digest().join("\n");
    let path = Path::new(PIXEL_GOLDEN);
    if bless {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(path, format!("{actual}\n")).expect("write pixel golden");
        println!("pixel golden written: {PIXEL_GOLDEN}");
        return ExitCode::SUCCESS;
    }
    match std::fs::read_to_string(path) {
        Ok(expected) if expected.trim_end() == actual => {
            println!("pixel ok: {PIXEL_GOLDEN}");
            ExitCode::SUCCESS
        }
        Ok(expected) => {
            for (line, want) in actual.lines().zip(expected.lines()) {
                if line != want {
                    eprintln!("pixel mismatch:\n  golden: {want}\n  actual: {line}");
                    break;
                }
            }
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("pixel golden missing: {PIXEL_GOLDEN}: {e}");
            ExitCode::FAILURE
        }
    }
}

fn contract() -> ExitCode {
    let out = Command::new("bun").arg("tests/contract.mjs").output();
    match out {
        Ok(out) if out.status.success() => {
            print!("{}", String::from_utf8_lossy(&out.stdout));
            ExitCode::SUCCESS
        }
        Ok(out) => {
            eprint!("{}", String::from_utf8_lossy(&out.stderr));
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("bun: {e}");
            ExitCode::FAILURE
        }
    }
}

fn app() -> ExitCode {
    match build_web_pkg().and_then(|()| run_script("tests/app.mjs")) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn build_web_pkg() -> Result<(), String> {
    build_wasm(&["epub"], &target_dir())?;
    let pkg = Path::new("web/pkg");
    let source = wasm_path();
    let fresh = pkg.join("ebook_web.js").exists()
        && std::fs::metadata(pkg.join("ebook_web_bg.wasm"))
            .and_then(|meta| meta.modified())
            .ok()
            .zip(
                std::fs::metadata(&source)
                    .and_then(|meta| meta.modified())
                    .ok(),
            )
            .map(|(built, wasm)| built >= wasm)
            .unwrap_or(false);
    if fresh {
        return Ok(());
    }
    let _ = std::fs::remove_dir_all(pkg);
    std::fs::create_dir_all(pkg).map_err(|e| format!("web/pkg: {e}"))?;
    let out = Command::new("wasm-bindgen")
        .args(["--target", "web", "--out-dir"])
        .arg(pkg)
        .arg(source)
        .output()
        .map_err(|e| format!("wasm-bindgen: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    Ok(())
}

fn run_script(path: &str) -> Result<(), String> {
    let out = Command::new("bun")
        .arg(path)
        .output()
        .map_err(|e| format!("bun: {e}"))?;
    print!("{}", String::from_utf8_lossy(&out.stdout));
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).into_owned())
    }
}

fn build_wasm(features: &[&str], dir: &Path) -> Result<(), String> {
    let key = feature_key(features);
    let path = wasm_path_in(dir);
    let stamp = dir.join("xtask/wasm.stamp");
    let source_time = newest_source_time();
    let fresh = std::fs::read_to_string(&stamp)
        .map(|text| text.trim() == key)
        .unwrap_or(false)
        && std::fs::metadata(&path)
            .and_then(|meta| meta.modified())
            .map(|time| time >= source_time)
            .unwrap_or(false);
    if fresh {
        return Ok(());
    }

    let mut command =
        Command::new(std::env::var("CARGO").unwrap_or_else(|_| String::from("cargo")));
    command.args([
        "build",
        "-p",
        "ebook-web",
        "--release",
        "--target",
        "wasm32-unknown-unknown",
        "--target-dir",
    ]);
    command.arg(dir);
    if !features.is_empty() {
        command.arg("--features");
        command.arg(features.join(","));
    }
    match command.status() {
        Ok(s) if s.success() => {
            if let Some(parent) = stamp.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(stamp, key);
            Ok(())
        }
        Ok(_) => Err(String::from("wasm build failed")),
        Err(e) => Err(format!("cargo not available: {e}")),
    }
}

fn measure(features: &[&str], label: &str, dir: &Path) -> Result<usize, String> {
    build_wasm(features, dir)?;
    let path = wasm_path_in(dir);
    let raw = std::fs::metadata(&path)
        .map_err(|e| format!("missing {}: {e}", path.display()))?
        .len() as usize;
    match gzip_len(&path) {
        Some(gz) => {
            println!("{label}: raw {raw} B, gzip {gz} B");
            Ok(gz)
        }
        None => {
            println!("{label}: raw {raw} B, gzip unavailable");
            Ok(0)
        }
    }
}

fn size() -> ExitCode {
    // 先 epub 再 lean：与 app 的构建顺序一致，减少 feature 切换。
    let full = measure(&["epub"], "core+epub", &target_dir());
    let lean = measure(&[], "core", &lean_dir());
    match (lean, full) {
        (Ok(lean), Ok(full)) => {
            println!("epub adapter delta: {} B gzip", full.saturating_sub(lean));
            if lean > GZIP_BUDGET || full > GZIP_BUDGET {
                eprintln!("size budget exceeded (budget {GZIP_BUDGET} B gzip)");
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        }
        (Err(message), _) | (_, Err(message)) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn parity() -> ExitCode {
    match run_parity() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn run_parity() -> Result<(), String> {
    let lean = lean_dir();
    build_wasm(&[], &lean)?;
    let dir = parity_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let pkg = dir.join("js");
    let source = wasm_path_in(&lean);
    let fresh = pkg.join("ebook_web.js").exists()
        && std::fs::metadata(pkg.join("ebook_web_bg.wasm"))
            .and_then(|meta| meta.modified())
            .ok()
            .zip(
                std::fs::metadata(&source)
                    .and_then(|meta| meta.modified())
                    .ok(),
            )
            .map(|(built, wasm)| built >= wasm)
            .unwrap_or(false);
    if !fresh {
        let _ = std::fs::remove_dir_all(&pkg);
        let out = Command::new("wasm-bindgen")
            .args(["--target", "nodejs", "--out-dir"])
            .arg(&pkg)
            .arg(&source)
            .output()
            .map_err(|e| format!("wasm-bindgen: {e}"))?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).into_owned());
        }
    }

    let native = dir.join("native.ir");
    std::fs::write(&native, demo_ir()).map_err(|e| e.to_string())?;

    let out = Command::new("bun")
        .arg("tests/parity.mjs")
        .arg(&pkg)
        .arg("fixtures/demo.txt")
        .arg(&native)
        .output()
        .map_err(|e| format!("bun: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    print!("{}", String::from_utf8_lossy(&out.stdout));
    Ok(())
}

fn gzip_len(path: &Path) -> Option<usize> {
    let out = Command::new("gzip")
        .arg("-9")
        .arg("-c")
        .arg(path)
        .output()
        .ok()?;
    if out.status.success() {
        Some(out.stdout.len())
    } else {
        None
    }
}

fn bench() -> ExitCode {
    match run_bench() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn bench_iterations() -> usize {
    std::env::var("BENCH_ITERS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(BENCH_ITERS)
}

fn run_bench() -> Result<(), String> {
    let iterations = bench_iterations();
    println!("# xtask bench: warmup={BENCH_WARMUP} iters={iterations} (median ns; codec 16 reps)");

    let layout = bench_layout()?;
    report("layout/en-epub-ch0", &layout);
    let (encode, decode) = bench_codec()?;
    report("ir/encode", &encode);
    report("ir/decode", &decode);
    let raster = bench_raster()?;
    report("raster/page-480x640", &raster);
    bench_image()?;
    Ok(())
}

fn bench_layout() -> Result<Vec<u128>, String> {
    let bytes = std::fs::read("fixtures/en.epub").map_err(|e| format!("fixtures/en.epub: {e}"))?;
    let font = std::fs::read(PIXEL_FONT).map_err(|e| format!("{PIXEL_FONT}: {e}"))?;
    let mut src = ebook_format::MemorySource::new();
    src.insert("en.epub", bytes);
    let opened = ebook_format::open(&src).map_err(|e| format!("format: {e:?}"))?;
    let doc = opened
        .chapter(&src, 0)
        .map_err(|e| format!("chapter: {e:?}"))?;
    let mut fonts = FontBook::new();
    fonts
        .add(font)
        .ok_or_else(|| String::from("font parse failed"))?;
    let env = LayoutEnv {
        measurer: &fonts,
        resources: &NoImageSizes,
    };
    let constraint = PageConstraint::new(600.0, 800.0).with_margin(EdgesF::all(24.0));
    Ok(sample(1, || {
        let ir = Layout::new().layout(&doc, &env, &constraint);
        black_box(&ir);
    }))
}

fn bench_codec() -> Result<(Vec<u128>, Vec<u128>), String> {
    let bytes = std::fs::read("fixtures/en.epub").map_err(|e| format!("fixtures/en.epub: {e}"))?;
    let font = std::fs::read(PIXEL_FONT).map_err(|e| format!("{PIXEL_FONT}: {e}"))?;
    let mut src = ebook_format::MemorySource::new();
    src.insert("en.epub", bytes);
    let opened = ebook_format::open(&src).map_err(|e| format!("format: {e:?}"))?;
    let doc = opened
        .chapter(&src, 0)
        .map_err(|e| format!("chapter: {e:?}"))?;
    let mut fonts = FontBook::new();
    fonts
        .add(font)
        .ok_or_else(|| String::from("font parse failed"))?;
    let env = LayoutEnv {
        measurer: &fonts,
        resources: &NoImageSizes,
    };
    let constraint = PageConstraint::new(600.0, 800.0).with_margin(EdgesF::all(24.0));
    let ir = Layout::new().layout(&doc, &env, &constraint);
    let encoded = codec::to_bytes(&ir).map_err(|e| format!("encode: {e:?}"))?;
    let mut out = Vec::new();
    let encode = sample(16, || {
        codec::encode(&ir, &mut out).expect("encode");
        black_box(out.len());
    });
    let decode = sample(16, || {
        let decoded: LayoutResult = codec::decode(&encoded).expect("decode");
        black_box(&decoded);
    });
    Ok((encode, decode))
}

fn bench_raster() -> Result<Vec<u128>, String> {
    let ir = pixel_ir();
    let font = std::fs::read(PIXEL_FONT).map_err(|e| format!("{PIXEL_FONT}: {e}"))?;
    let mut fonts = FontBook::new();
    fonts
        .add(font)
        .ok_or_else(|| String::from("font parse failed"))?;
    let page = ir
        .page(0)
        .ok_or_else(|| String::from("pixel book has no page"))?;
    let mut surface = RgbaSurface::from_page(page);
    Ok(sample(1, || {
        surface.render_page(&ir, 0, &mut fonts).expect("render");
        black_box(surface.pixel(0, 0));
    }))
}

struct OneImage(Arc<ImageData>);

impl Resources for OneImage {
    fn glyph_alpha(
        &mut self,
        _font: ebook_ir::FontId,
        _gid: u16,
        _px: f32,
    ) -> Option<Arc<ebook_ir::GlyphBitmap>> {
        None
    }

    fn image(&mut self, _id: ImageId, _target: ebook_ir::SizeF) -> Option<Arc<ImageData>> {
        Some(self.0.clone())
    }
}

fn bench_image() -> Result<(), String> {
    // 3000×2000：6 MP，完整 RGBA 24 MB；页面实际只需要 1000 px 宽。
    let bytes = bench_png(3000, 2000, |x, y| {
        (
            (x * 255 / 2999) as u8,
            (y * 255 / 1999) as u8,
            ((x + y) & 0xff) as u8,
        )
    });
    let full = ebook_format::image::decode_image(&bytes)
        .ok_or_else(|| String::from("bench png decode failed"))?;
    let scaled = ebook_format::image::decode_image_scaled(&bytes, 1000, 667)
        .ok_or_else(|| String::from("bench png scaled decode failed"))?;
    println!(
        "# image/decode-full: output={} bytes, {}x{}",
        full.rgba.len(),
        full.w,
        full.h
    );
    println!(
        "# image/decode-scaled: output={} bytes, {}x{}",
        scaled.rgba.len(),
        scaled.w,
        scaled.h
    );

    let full_time = sample(1, || {
        let image = ebook_format::image::decode_image(&bytes).expect("decode");
        black_box(image.rgba.len());
    });
    report("image/decode-full-6mp", &full_time);
    let scaled_time = sample(1, || {
        let image = ebook_format::image::decode_image_scaled(&bytes, 1000, 667).expect("decode");
        black_box(image.rgba.len());
    });
    report("image/decode-scaled-6mp", &scaled_time);

    // 页面缓存里已经是小图时，双线性采样只面对小图；这里比较同一块
    // 480×320 目标区域从 24 MB 原图与 2 MB 缩略图取样的成本。
    let mut surface = RgbaSurface::new(480, 640);
    let rect = RectF::new(0.0, 0.0, 480.0, 320.0);
    let mut full_res = OneImage(Arc::new(full));
    let full_draw = sample(1, || {
        surface.clear(Rgba8::WHITE);
        surface.image(0, rect, Fit::Contain, &mut full_res);
        black_box(surface.pixel(240, 160));
    });
    report("image/draw-full-6mp", &full_draw);
    let mut scaled_res = OneImage(Arc::new(scaled));
    let scaled_draw = sample(1, || {
        surface.clear(Rgba8::WHITE);
        surface.image(0, rect, Fit::Contain, &mut scaled_res);
        black_box(surface.pixel(240, 160));
    });
    report("image/draw-scaled-6mp", &scaled_draw);
    Ok(())
}

/// 生成一张 RGB8 PNG。deflate 用 stored block，避免给 xtask 加压缩依赖；
/// 这里只关心解码/降采样的真实像素量，不关心文件大小。
fn bench_png(w: u32, h: u32, pixel: impl Fn(u32, u32) -> (u8, u8, u8)) -> Vec<u8> {
    let stride = 1 + w as usize * 3;
    let mut raw = Vec::with_capacity(stride * h as usize);
    for y in 0..h {
        raw.push(0);
        for x in 0..w {
            let (r, g, b) = pixel(x, y);
            raw.extend_from_slice(&[r, g, b]);
        }
    }
    let mut zlib = Vec::with_capacity(raw.len() + raw.len() / 65_535 * 5 + 16);
    zlib.extend_from_slice(&[0x78, 0x01]);
    let mut rest = &raw[..];
    while !rest.is_empty() {
        let n = rest.len().min(65_535);
        let last = n == rest.len();
        zlib.push(u8::from(last));
        zlib.extend_from_slice(&(n as u16).to_le_bytes());
        zlib.extend_from_slice(&(!(n as u16)).to_le_bytes());
        zlib.extend_from_slice(&rest[..n]);
        rest = &rest[n..];
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut png = Vec::with_capacity(zlib.len() + 128);
    png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    png_chunk(&mut png, b"IHDR", &ihdr);
    png_chunk(&mut png, b"IDAT", &zlib);
    png_chunk(&mut png, b"IEND", &[]);
    png
}

fn png_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc = 0xffff_ffffu32;
    crc = crc32(crc, kind);
    crc = crc32(crc, data);
    out.extend_from_slice(&(crc ^ 0xffff_ffff).to_be_bytes());
}

fn crc32(mut crc: u32, bytes: &[u8]) -> u32 {
    for &byte in bytes {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    crc
}

fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in bytes {
        a = (a + byte as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

fn sample(reps: usize, mut run: impl FnMut()) -> Vec<u128> {
    let iterations = bench_iterations();
    for _ in 0..BENCH_WARMUP {
        for _ in 0..reps {
            run();
        }
    }
    let mut samples = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let start = Instant::now();
        for _ in 0..reps {
            run();
        }
        samples.push(start.elapsed().as_nanos() / reps as u128);
    }
    samples
}

fn report(name: &str, samples: &[u128]) {
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let median = sorted.get(sorted.len() / 2).copied().unwrap_or(0);
    println!("{name}\t{median}");
}
