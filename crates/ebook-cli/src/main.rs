mod svg;

use std::fs;
use std::process::ExitCode;

use ebook_font::FontBook;
use ebook_format::epub::EpubReader;
use ebook_format::{open, Book, ChapterAnchor, MemorySource};
use ebook_ir::{
    codec, EdgesF, GlyphBitmap, ImageData, ImageId, ImageMeta, Location, PointF, Resources, SizeF,
    StyledDoc,
};
use ebook_layout::{
    Layout, LayoutEnv, PageConstraint, Reader, ResourceView, StubMeasurer, TextMeasurer,
};
use ebook_render::paint;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("layout") => layout(&args[1..]),
        _ => {
            eprintln!(
                "usage: ebook layout <file> [--width N] [--height N] [--margin N] [--page N] [--spine N] [--ir PATH] [--svg PATH] [--ppm PATH] [--stats] [--hit X,Y] [--select P:L:B-P:L:B] [--anchor ID]"
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
    let mut ppm_out: Option<String> = None;
    let mut want_stats = false;
    let mut hit_at: Option<(f32, f32)> = None;
    let mut select_at: Option<String> = None;
    let mut anchor_at: Option<String> = None;

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
            "--ppm" => {
                ppm_out = Some(need(args, i + 1, arg)?.to_string());
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
            "--anchor" => {
                anchor_at = Some(need(args, i + 1, arg)?.to_string());
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
    src.insert(&name, data.clone());
    let opened = open(&src).map_err(|e| format!("format: {e:?}"))?;
    // EPUB 的图片字节只能从 zip 读；txt 没有资产。
    let reader = if opened.adapter_name() == "epub" {
        Some(EpubReader::open(&data).map_err(|e| format!("epub: {e:?}"))?)
    } else {
        None
    };
    let (doc, anchors) = match &reader {
        Some(reader) => {
            let (doc, _, anchors) = reader
                .chapter_with_anchors(spine)
                .map_err(|e| format!("chapter: {e:?}"))?;
            (doc, anchors)
        }
        None => (
            opened
                .chapter(&src, spine)
                .map_err(|e| format!("chapter: {e:?}"))?,
            Vec::new(),
        ),
    };

    // 内嵌字体是排版入参：family/weight/style 原样搬进 FontBook。
    let mut fonts = load_fonts(&opened.book);
    if !fonts.is_empty() {
        fonts.set_language(doc.meta.lang.as_deref());
    }
    let media = ChapterMedia::new(reader.as_ref(), &doc);
    let stub = StubMeasurer;
    let measurer: &dyn TextMeasurer = if fonts.is_empty() { &stub } else { &fonts };
    let env = LayoutEnv {
        measurer,
        resources: &media,
    };
    let constraint = PageConstraint::new(width, height).with_margin(EdgesF::all(margin));
    let ir = Layout::new().layout(&doc, &env, &constraint);
    let bytes = codec::to_bytes(&ir).map_err(|e| format!("encode: {e:?}"))?;

    if let Some(fragment) = anchor_at.as_deref() {
        print_anchor(&anchors, fragment);
    }

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
        let mut surface = svg::CliSvg::new(size, media.data_uris());
        let mut res = CliResources {
            font: &mut fonts,
            media: &media,
        };
        paint(&ir, page, &mut surface, &mut res).map_err(|e| format!("render: {e:?}"))?;
        fs::write(&out, surface.finish()).map_err(|e| format!("{out}: {e}"))?;
    }

    if let Some(out) = ppm_out {
        let info = ir
            .page(page)
            .ok_or_else(|| format!("page {page} does not exist"))?;
        let mut surface = ebook_raster::RgbaSurface::from_page(info);
        let mut res = CliResources {
            font: &mut fonts,
            media: &media,
        };
        surface
            .render_page(&ir, page, &mut res)
            .map_err(|e| format!("raster: {e:?}"))?;
        fs::write(&out, surface.to_ppm()).map_err(|e| format!("{out}: {e}"))?;
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

fn print_anchor(anchors: &[ChapterAnchor], fragment: &str) {
    match anchors.iter().find(|entry| entry.id == fragment) {
        Some(entry) => println!(
            "anchor #{} -> spine {} path {:?} offset {}",
            fragment, entry.anchor.spine, entry.anchor.path, entry.anchor.offset
        ),
        None => println!("anchor #{fragment} -> miss"),
    }
}

/// 把书里声明的内嵌字体装进 `FontBook`。坏字体跳过，不阻止排版，但要留下诊断。
fn load_fonts(book: &Book) -> FontBook {
    let mut fonts = FontBook::new();
    for face in book.fonts() {
        if let Err(error) =
            fonts.try_add_styled(&face.family, face.weight, face.style, face.bytes.clone())
        {
            eprintln!("font {} ({}): {error}", face.href, face.family);
        }
    }
    fonts
}

/// 一章的图片表：`src`/MIME 来自 `ImageMeta`，尺寸优先用格式层填好的固有尺寸，
/// 缺了再按 `src` 从 zip 读头解析。
struct ChapterMedia<'a> {
    reader: Option<&'a EpubReader<'a>>,
    images: Vec<(Option<String>, Option<String>)>,
    sizes: Vec<Option<SizeF>>,
}

impl<'a> ChapterMedia<'a> {
    fn new(reader: Option<&'a EpubReader<'a>>, doc: &StyledDoc) -> Self {
        let images: Vec<(Option<String>, Option<String>)> = doc
            .images
            .iter()
            .map(|meta: &ImageMeta| (meta.src.clone(), meta.mime.clone()))
            .collect();
        let mut sizes = Vec::with_capacity(images.len());
        for meta in &doc.images {
            let mut size = meta.size;
            if size.is_none() {
                if let (Some(reader), Some(src)) = (reader, meta.src.as_deref()) {
                    if let Some((bytes, _)) = reader.asset(src) {
                        size = ebook_format::image::image_size(bytes);
                    }
                }
            }
            sizes.push(size);
        }
        Self {
            reader,
            images,
            sizes,
        }
    }

    fn source(&self, id: ImageId) -> Option<(&[u8], &str)> {
        let (src, _) = self.images.get(id as usize)?;
        let src = src.as_deref()?;
        self.reader?.asset(src)
    }

    /// `--svg` 用的内联数据：`(id, mime, base64)`。
    fn data_uris(&self) -> Vec<(ImageId, String, String)> {
        let mut out = Vec::new();
        for (index, (_, mime)) in self.images.iter().enumerate() {
            let Some((bytes, declared)) = self.source(index as ImageId) else {
                continue;
            };
            let mime = mime.clone().unwrap_or_else(|| declared.to_string());
            out.push((index as ImageId, mime, svg::base64(bytes)));
        }
        out
    }
}

impl ResourceView for ChapterMedia<'_> {
    fn image_size(&self, id: ImageId) -> Option<SizeF> {
        self.sizes.get(id as usize).copied().flatten()
    }
}

/// 光栅后端要字形，SVG 后端只要图片；两套共用同一份 zip/PNG 实现。
struct CliResources<'a, 'm> {
    font: &'a mut FontBook,
    media: &'a ChapterMedia<'m>,
}

impl Resources for CliResources<'_, '_> {
    fn glyph_alpha(&mut self, font: ebook_ir::FontId, gid: u16, px: f32) -> Option<GlyphBitmap> {
        self.font.rasterize(font, gid, px)
    }

    fn glyph_alpha_at(
        &mut self,
        font: ebook_ir::FontId,
        gid: u16,
        px: f32,
        dx: f32,
        dy: f32,
    ) -> Option<GlyphBitmap> {
        self.font.rasterize_at(font, gid, px, dx, dy)
    }

    fn image(&mut self, id: ImageId) -> Option<ImageData> {
        let (bytes, _) = self.media.source(id)?;
        ebook_format::image::decode_image(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ebook_ir::{Fit, FontSpec, Op};

    fn fixture() -> Vec<u8> {
        std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/images.epub"
        ))
        .expect("fixtures/images.epub")
    }

    #[test]
    fn images_reach_layout_svg_and_raster() {
        let data = fixture();
        let reader = EpubReader::open(&data).expect("open");
        let (doc, _, anchors) = reader.chapter_with_anchors(0).expect("chapter");
        assert!(anchors.iter().any(|entry| entry.id == "note"));

        let media = ChapterMedia::new(Some(&reader), &doc);
        let mut fonts = load_fonts(reader.book());
        fonts.set_language(doc.meta.lang.as_deref());
        let spec = FontSpec::new(18.0, 400, ebook_ir::FontStyle::Normal).with_family("Book Serif");
        let chosen = fonts.select(&spec);
        assert_eq!(
            fonts.desc(chosen).map(|desc| desc.family.as_str()),
            Some("Book Serif"),
            "the embedded @font-face must not fall back"
        );

        let env = LayoutEnv {
            measurer: &fonts,
            resources: &media,
        };
        let constraint = PageConstraint::new(600.0, 800.0).with_margin(EdgesF::all(24.0));
        let ir = Layout::new().layout(&doc, &env, &constraint);

        fn collect(ops: &[Op], out: &mut Vec<(ImageId, ebook_ir::RectF, Fit)>) {
            for op in ops {
                match op {
                    Op::Image { id, rect, fit } => out.push((*id, *rect, *fit)),
                    Op::Clip { ops, .. } => collect(ops, out),
                    _ => {}
                }
            }
        }
        let mut images = Vec::new();
        collect(&ir.display.ops, &mut images);
        assert_eq!(images.len(), 2, "{images:?}");
        assert_eq!((images[0].1.w, images[0].1.h), (12.0, 6.0));
        assert_eq!((images[1].1.w, images[1].1.h), (4.0, 10.0));
        assert!(images.iter().all(|(_, _, fit)| *fit == Fit::Contain));

        let page = ir.page(0).expect("page");
        let size = page.box_size();
        let mut surface = svg::CliSvg::new(size, media.data_uris());
        let mut res = CliResources {
            font: &mut fonts,
            media: &media,
        };
        paint(&ir, 0, &mut surface, &mut res).expect("svg");
        let svg = surface.finish();
        assert_eq!(svg.matches("<image ").count(), 2, "{svg}");
        assert!(svg.contains("xlink:href=\"data:image/png;base64,"));
        assert!(svg.contains("preserveAspectRatio=\"xMidYMid meet\""));

        // 光栅后端用同一份 zip/PNG 资源画出真实像素（左半红、右半蓝）。
        let mut raster = ebook_raster::RgbaSurface::from_page(page);
        let mut res = CliResources {
            font: &mut fonts,
            media: &media,
        };
        raster.render_page(&ir, 0, &mut res).expect("raster");
        let ox = page.origin.x - page.inset.left;
        let oy = page.origin.y - page.inset.top;
        let first = images[0].1;
        let left = raster.pixel(
            (first.x - ox + 1.0) as u32,
            (first.y - oy + first.h / 2.0) as u32,
        );
        let right = raster.pixel(
            (first.x - ox + first.w - 1.0) as u32,
            (first.y - oy + first.h / 2.0) as u32,
        );
        let left = left.expect("left pixel");
        let right = right.expect("right pixel");
        assert!(left.r > 150 && left.g < 100, "left is red: {left:?}");
        assert!(right.b > 150 && right.r < 100, "right is blue: {right:?}");
    }

    #[test]
    fn anchor_fragments_print_the_node_path() {
        let data = fixture();
        let reader = EpubReader::open(&data).expect("open");
        let (_, _, anchors) = reader.chapter_with_anchors(0).expect("chapter");
        let note = anchors
            .iter()
            .find(|entry| entry.id == "note")
            .expect("note");
        assert_eq!(note.anchor.spine, 0);
        assert!(!note.anchor.path.is_empty());
    }
}
