use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Instant;

use ebook_font::FontBook;
use ebook_ir::{codec, EdgesF, LayoutResult};
use ebook_layout::{Layout, LayoutEnv, NoImageSizes, PageConstraint, StubMeasurer};
use ebook_raster::RgbaSurface;

// 内置 WOFF/WOFF2 解包后，Brotli 解码器本身约占 105 KB gzip；WOFF1 的 zlib
// 复用 ebook-format 已有的 miniz_oxide，不再额外链一套 flate2。预算相应上调。
const GZIP_BUDGET: usize = 640 * 1024;
fn target_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target"))
}

fn wasm_path() -> PathBuf {
    target_dir().join("wasm32-unknown-unknown/release/ebook_web.wasm")
}

fn parity_dir() -> PathBuf {
    target_dir().join("parity")
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
        "bless" => golden(true),
        "size" => size(),
        "parity" => parity(),
        "pixel" => pixel(false),
        "pixel-bless" => pixel(true),
        "contract" => contract(),
        "app" => app(),
        "bench" => bench(),
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
                "usage: xtask [all|golden|bless|contract|app|pixel|pixel-bless|size|parity|bench]"
            );
            ExitCode::from(2)
        }
    }
}

fn ir_for(name: &str, bytes: Vec<u8>) -> Vec<u8> {
    let mut src = ebook_format::MemorySource::new();
    src.insert(name, bytes);
    let opened = ebook_format::open(&src).expect("format");
    let doc = opened.chapter(&src, 0).expect("chapter");
    let env = LayoutEnv {
        measurer: &StubMeasurer,
        resources: &NoImageSizes,
    };
    let constraint = PageConstraint::new(600.0, 800.0).with_margin(EdgesF::all(24.0));
    let ir = Layout::new().layout(&doc, &env, &constraint);
    codec::to_bytes(&ir).expect("encode")
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
    match build_web_pkg()
        .and_then(|()| run_script("tests/app.mjs"))
        .and_then(|()| run_script("web/tests/images.mjs"))
        .and_then(|()| run_script("web/tests/atlas.mjs"))
    {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn build_web_pkg() -> Result<(), String> {
    build_wasm(&["epub"])?;
    let pkg = Path::new("web/pkg");
    let _ = std::fs::remove_dir_all(pkg);
    std::fs::create_dir_all(pkg).map_err(|e| format!("web/pkg: {e}"))?;
    let out = Command::new("wasm-bindgen")
        .args(["--target", "web", "--out-dir"])
        .arg(pkg)
        .arg(wasm_path())
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

fn build_wasm(features: &[&str]) -> Result<(), String> {
    let mut command =
        Command::new(std::env::var("CARGO").unwrap_or_else(|_| String::from("cargo")));
    command.args([
        "build",
        "-p",
        "ebook-web",
        "--release",
        "--target",
        "wasm32-unknown-unknown",
    ]);
    if !features.is_empty() {
        command.arg("--features");
        command.arg(features.join(","));
    }
    match command.status() {
        Ok(s) if s.success() => Ok(()),
        Ok(_) => Err(String::from("wasm build failed")),
        Err(e) => Err(format!("cargo not available: {e}")),
    }
}

fn measure(features: &[&str], label: &str) -> Result<usize, String> {
    build_wasm(features)?;
    let path = wasm_path();
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
    let lean = measure(&[], "core");
    let full = measure(&["epub"], "core+epub");
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
    build_wasm(&[])?;
    let dir = parity_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let pkg = dir.join("js");
    let _ = std::fs::remove_dir_all(&pkg);

    let out = Command::new("wasm-bindgen")
        .args(["--target", "nodejs", "--out-dir"])
        .arg(&pkg)
        .arg(wasm_path())
        .output()
        .map_err(|e| format!("wasm-bindgen: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
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
