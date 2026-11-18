use std::fs;
use std::process::ExitCode;

use ebook_format::{open, MemorySource};
use ebook_ir::{codec, EdgesF, Location, NoResources, PointF, Renderer, SizeF};
use ebook_layout::{Layout, LayoutEnv, NoImageSizes, PageConstraint, Reader, StubMeasurer};
use ebook_render::{SvgRenderer, SvgSurface};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("layout") => layout(&args[1..]),
        _ => {
            eprintln!(
                "usage: ebook layout <file> [--width N] [--height N] [--margin N] [--page N] [--spine N] [--ir PATH] [--svg PATH] [--stats] [--hit X,Y] [--select P:L:B-P:L:B]"
            );
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
    let mut hit_at: Option<(f32, f32)> = None;
    let mut select_at: Option<String> = None;

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
            "--hit" => {
                hit_at = Some(parse_point(need(args, i + 1, arg)?)?);
                i += 1;
            }
            "--select" => {
                select_at = Some(need(args, i + 1, arg)?.to_string());
                i += 1;
            }
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

    if let Some((x, y)) = hit_at {
        print_hit(&ir, page, x, y);
    }
    if let Some(spec) = select_at.as_deref() {
        print_selection(&ir, &doc, spec)?;
    }

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
        let s = ir.stats_with_doc(&doc);
        println!(
            "pages={} lines={} glyphs={} ops={} dirty={} ir={} bytes nodes={} text_bytes={} layout_bytes={} truncated={}",
            s.pages,
            s.lines,
            s.glyphs,
            s.ops,
            ir.dirty.len(),
            bytes.len(),
            s.nodes,
            s.text_bytes,
            s.layout_bytes,
            ir.truncated
        );
    }

    Ok(())
}

fn need<'a>(args: &'a [String], i: usize, flag: &str) -> Result<&'a str, String> {
    args.get(i)
        .map(String::as_str)
        .ok_or_else(|| format!("missing value for {flag}"))
}

fn parse_point(value: &str) -> Result<(f32, f32), String> {
    let (x, y) = value
        .split_once(',')
        .ok_or_else(|| format!("{value}: want X,Y"))?;
    let x = x.parse().map_err(|_| format!("{value}: bad X"))?;
    let y = y.parse().map_err(|_| format!("{value}: bad Y"))?;
    Ok((x, y))
}

fn parse_location(value: &str) -> Result<Location, String> {
    let mut parts = value.split(':');
    let mut next = |what: &str| -> Result<u32, String> {
        parts
            .next()
            .ok_or_else(|| format!("{value}: missing {what}"))?
            .parse()
            .map_err(|_| format!("{value}: bad {what}"))
    };
    let location = Location {
        page: next("page")?,
        line: next("line")?,
        byte: next("byte")?,
    };
    if parts.next().is_some() {
        return Err(format!("{value}: want P:L:B"));
    }
    Ok(location)
}

/// `--hit X,Y`：页内坐标 → 原文位置。调试输出走 stdout，不影响 IR/像素。
fn print_hit(ir: &ebook_ir::LayoutResult, page: u32, x: f32, y: f32) {
    let reader = Reader::new(ir);
    let point = PointF::new(x, y);
    match reader.hit(page, point) {
        Some(location) => {
            println!(
                "hit page {} ({x:.1},{y:.1}) -> page {} line {} byte {} progress {:.4}",
                page,
                location.page,
                location.line,
                location.byte,
                reader.progress(location)
            );
            if let Some(anchor) = reader.anchor_at(page, point) {
                println!(
                    "  anchor spine {} path {:?} offset {}",
                    anchor.spine, anchor.path, anchor.offset
                );
            }
            if let Some(href) = reader.link_at(page, point) {
                println!("  link {href}");
            }
        }
        None => println!("hit page {} ({x:.1},{y:.1}) -> miss", page),
    }
}

/// `--select P:L:B-P:L:B`：打印分页矩形与原文。
fn print_selection(
    ir: &ebook_ir::LayoutResult,
    doc: &ebook_ir::StyledDoc,
    spec: &str,
) -> Result<(), String> {
    let (start, end) = spec
        .split_once('-')
        .ok_or_else(|| format!("{spec}: want P:L:B-P:L:B"))?;
    let a = parse_location(start)?;
    let b = parse_location(end)?;
    let reader = Reader::new(ir);
    let selection = reader.selection(a, b);
    let (from, to) = selection.normalize();
    println!(
        "select page {} line {} byte {} .. page {} line {} byte {} empty {}",
        from.page,
        from.line,
        from.byte,
        to.page,
        to.line,
        to.byte,
        selection.is_empty()
    );
    let rects = reader.selection_rects(&selection);
    for rect in &rects {
        println!(
            "  rect page {} x {:.2} y {:.2} w {:.2} h {:.2}",
            rect.page, rect.rect.x, rect.rect.y, rect.rect.w, rect.rect.h
        );
    }
    let text = reader.selection_text(doc, &selection);
    println!("  text {:?}", text);
    Ok(())
}
