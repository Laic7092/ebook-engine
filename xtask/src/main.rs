use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use ebook_ir::{codec, EdgesF};
use ebook_layout::{Layout, LayoutEnv, NoImageSizes, PageConstraint, StubMeasurer};

const GZIP_BUDGET: usize = 512 * 1024;
const WASM: &str = "target/wasm32-unknown-unknown/release/ebook_web.wasm";
const PARITY_DIR: &str = "target/parity";

fn main() -> ExitCode {
    let cmd = std::env::args().nth(1).unwrap_or_else(|| String::from("all"));
    match cmd.as_str() {
        "golden" => golden(false),
        "bless" => golden(true),
        "size" => size(),
        "parity" => parity(),
        "all" => {
            let ok = [
                golden(false),
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
            eprintln!("usage: xtask [all|golden|bless|size|parity]");
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
