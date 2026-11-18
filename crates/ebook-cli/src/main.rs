use std::fs;
use std::process::ExitCode;

use ebook_format::{open, MemorySource};
use ebook_ir::{codec, EdgesF, NoResources, Renderer, SizeF};
use ebook_layout::{Layout, LayoutEnv, NoImageSizes, PageConstraint, StubMeasurer};
use ebook_render::{stats, SvgRenderer, SvgSurface};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("layout") => layout(&args[1..]),
        _ => {
            eprintln!("usage: ebook layout <file> [--width N] [--height N] [--margin N] [--page N] [--spine N] [--ir PATH] [--svg PATH] [--stats]");
            ExitCode::from(2)
        }
    }
}

fn layout(args: &[String]) -> ExitCode {
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let mut path: Option<String> = None;
    let mut width = 600.0f32;
    let mut height = 800.0f32;
    let mut margin = 24.0f32;
    let mut page = 0u32;
    let mut spine = 0usize;
    let mut ir_out: Option<String> = None;
    let mut svg_out: Option<String> = None;
    let mut want_stats = false;

    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "--width" => {
                width = need(args, i + 1, arg)?
                    .parse()
                    .map_err(|_| "bad width".to_string())?;
                i += 1;
            }
            "--height" => {
                height = need(args, i + 1, arg)?
                    .parse()
                    .map_err(|_| "bad height".to_string())?;
                i += 1;
            }
            "--margin" => {
                margin = need(args, i + 1, arg)?
                    .parse()
                    .map_err(|_| "bad margin".to_string())?;
                i += 1;
            }
            "--page" => {
                page = need(args, i + 1, arg)?
                    .parse()
                    .map_err(|_| "bad page".to_string())?;
                i += 1;
            }
            "--spine" => {
                spine = need(args, i + 1, arg)?
                    .parse()
                    .map_err(|_| "bad spine".to_string())?;
                i += 1;
            }
            "--ir" => {
                ir_out = Some(need(args, i + 1, arg)?.to_string());
                i += 1;
            }
            "--svg" => {
                svg_out = Some(need(args, i + 1, arg)?.to_string());
                i += 1;
            }
            "--stats" => want_stats = true,
            other if other.starts_with("--") => return Err(format!("unknown flag {other}")),
            other => path = Some(other.to_string()),
        }
        i += 1;
    }

    let path = path.ok_or_else(|| "missing input file".to_string())?;
    let data = fs::read(&path).map_err(|e| format!("{path}: {e}"))?;
    let name = std::path::Path::new(&path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("main.txt")
        .to_string();

    let mut src = MemorySource::new();
    src.insert(&name, data);
    let opened = open(&src).map_err(|e| format!("format: {e:?}"))?;
    let doc = opened
        .chapter(&src, spine)
        .map_err(|e| format!("chapter: {e:?}"))?;

    let env = LayoutEnv {
        measurer: &StubMeasurer,
        resources: &NoImageSizes,
    };
    let constraint = PageConstraint::new(width, height).with_margin(EdgesF::all(margin));
    let ir = Layout::new().layout(&doc, &env, &constraint);
    let bytes = codec::to_bytes(&ir).map_err(|e| format!("encode: {e:?}"))?;

    let wrote_ir = ir_out.is_some();
    if let Some(out) = ir_out {
        fs::write(&out, &bytes).map_err(|e| format!("{out}: {e}"))?;
    }

    if let Some(out) = svg_out {
        let size = ir.page(page).map(|p| p.box_size()).unwrap_or(SizeF::ZERO);
        let mut surface = SvgSurface::new(size);
        SvgRenderer
            .render(&ir, page, &mut surface, &mut NoResources)
            .map_err(|e| format!("render: {e:?}"))?;
        fs::write(&out, surface.finish()).map_err(|e| format!("{out}: {e}"))?;
    }

    if want_stats || !wrote_ir {
        let s = stats(&ir, bytes.len());
        println!(
            "pages={} lines={} glyphs={} ops={} dirty={} ir={} bytes",
            s.pages, s.lines, s.glyphs, s.ops, s.dirty, s.bytes
        );
    }

    Ok(())
}

fn need<'a>(args: &'a [String], i: usize, flag: &str) -> Result<&'a str, String> {
    args.get(i)
        .map(String::as_str)
        .ok_or_else(|| format!("missing value for {flag}"))
}
