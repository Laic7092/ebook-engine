use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use ebook_font::FontBook;
use ebook_ir::{codec, EdgesF, LayoutResult};
use ebook_layout::{Layout, LayoutEnv, NoImageSizes, PageConstraint, StubMeasurer};
use ebook_raster::RgbaSurface;

const GZIP_BUDGET: usize = 512 * 1024;
const WASM: &str = "target/wasm32-unknown-unknown/release/ebook_web.wasm";
const PARITY_DIR: &str = "target/parity";
const PIXEL_GOLDEN: &str = "tests/golden/pixels.txt";
const PIXEL_BOOK: &str = "fixtures/typography.epub";
const PIXEL_FONT: &str = "fixtures/font/test.ttf";

fn main() -> ExitCode {
    let cmd = std::env::args().nth(1).unwrap_or_else(|| String::from("all"));
    match cmd.as_str() {
        "golden" => golden(false),
        "bless" => golden(true),
        "size" => size(),
        "parity" => parity(),
        "pixel" => pixel(false),
        "pixel-bless" => pixel(true),
        "contract" => contract(),
        "all" => {
            let ok = [
                golden(false),
                contract(),
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
                "usage: xtask [all|golden|bless|contract|pixel|pixel-bless|size|parity]"
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
    let env = LayoutEnv { measurer: &StubMeasurer, resources: &NoImageSizes };
    let constraint = PageConstraint::new(600.0, 800.0).with_margin(EdgesF::all(24.0));
    let ir = Layout::new().layout(&doc, &env, &constraint);
    codec::to_bytes(&ir).expect("encode")
}

fn demo_ir() -> Vec<u8> {
    ir_for("demo.txt", std::fs::read("fixtures/demo.txt").expect("fixtures/demo.txt"))
}

fn epub_ir() -> Vec<u8> {
    ir_for("demo.epub", std::fs::read("fixtures/demo.epub").expect("fixtures/demo.epub"))
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
    let env = LayoutEnv { measurer: &fonts, resources: &NoImageSizes };
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
    let path = PathBuf::from(WASM);
    let raw = std::fs::metadata(&path)
        .map_err(|e| format!("missing {WASM}: {e}"))?
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
    let dir = Path::new(PARITY_DIR);
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let pkg = dir.join("js");
    let _ = std::fs::remove_dir_all(&pkg);

    let out = Command::new("wasm-bindgen")
        .args(["--target", "nodejs", "--out-dir"])
        .arg(&pkg)
        .arg(WASM)
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
    let out = Command::new("gzip").arg("-9").arg("-c").arg(path).output().ok()?;
    if out.status.success() {
        Some(out.stdout.len())
    } else {
        None
    }
}
