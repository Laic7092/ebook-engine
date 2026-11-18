use ab_glyph_rasterizer::{point, Point, Rasterizer};
use ebook_ir::{FontDesc, FontId, FontSpec, FontStyle, GlyphBitmap, ImageData, ImageId, Resources};
use ebook_layout::TextMeasurer;
use ttf_parser::Face;

const SYNTH_FLAG: u16 = 0x8000;
const SYNTH_BOLD: u16 = 0x1000;
const SYNTH_ITALIC: u16 = 0x2000;
const SYNTH_BASE_MASK: u16 = 0x0fff;
const SYNTH_SHEAR: f32 = 0.22;

#[derive(Debug, Default)]
struct Metrics {
    desc: FontDesc,
    bytes: Vec<u8>,
    units_per_em: f32,
    ascender: f32,
    descender: f32,
    advances: Vec<u16>,
    cmap: Vec<(u32, u16)>,
}

impl Metrics {
    fn parse(desc: FontDesc, bytes: Vec<u8>) -> Option<Self> {
        let face = Face::parse(&bytes, 0).ok()?;
        let units_per_em = face.units_per_em() as f32;
        if units_per_em <= 0.0 {
            return None;
        }
        let ascender = face.ascender() as f32;
        let descender = face.descender() as f32;
        let advances = (0..face.number_of_glyphs())
            .map(|gid| {
                face.glyph_hor_advance(ttf_parser::GlyphId(gid))
                    .unwrap_or(0)
            })
            .collect();
        let mut cmap = Vec::new();
        if let Some(table) = face.tables().cmap {
            for subtable in table.subtables {
                if !subtable.is_unicode() {
                    continue;
                }
                subtable.codepoints(|code| {
                    if let Some(gid) = subtable.glyph_index(code) {
                        cmap.push((code, gid.0));
                    }
                });
            }
        }
        cmap.sort_unstable_by_key(|entry| entry.0);
        cmap.dedup_by_key(|entry| entry.0);
        Some(Self {
            desc,
            bytes,
            units_per_em,
            ascender,
            descender,
            advances,
            cmap,
        })
    }

    fn glyph(&self, ch: char) -> u16 {
        match self
            .cmap
            .binary_search_by_key(&(ch as u32), |entry| entry.0)
        {
            Ok(index) => self.cmap[index].1,
            Err(_) => 0,
        }
    }

    fn advance(&self, ch: char) -> f32 {
        let gid = self.glyph(ch);
        if gid == 0 {
            return 0.0;
        }
        self.advances.get(gid as usize).copied().unwrap_or(0) as f32 / self.units_per_em
    }
}

#[derive(Debug, Default)]
pub struct FontBook {
    faces: Vec<Metrics>,
}

impl FontBook {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, bytes: Vec<u8>) -> Option<FontId> {
        self.add_face(FontDesc::default(), bytes)
    }

    pub fn add_face(&mut self, desc: FontDesc, bytes: Vec<u8>) -> Option<FontId> {
        let mut desc = desc;
        if desc.weight == 0 {
            desc.weight = 400;
        }
        let metrics = Metrics::parse(desc, bytes)?;
        let id = FontId(self.faces.len() as u16);
        self.faces.push(metrics);
        Some(id)
    }

    pub fn add_with_desc(&mut self, desc: FontDesc, bytes: Vec<u8>) -> Option<FontId> {
        self.add_face(desc, bytes)
    }

    pub fn add_styled(
        &mut self,
        family: &str,
        weight: u16,
        style: FontStyle,
        bytes: Vec<u8>,
    ) -> Option<FontId> {
        self.add_face(
            FontDesc {
                family: family.into(),
                weight,
                style,
            },
            bytes,
        )
    }

    pub fn len(&self) -> usize {
        self.faces.len()
    }

    pub fn is_empty(&self) -> bool {
        self.faces.is_empty()
    }

    pub fn desc(&self, id: FontId) -> Option<&FontDesc> {
        self.faces
            .get(self.real_index(id)?)
            .map(|metrics| &metrics.desc)
    }

    pub fn select(&self, spec: &FontSpec) -> FontId {
        if self.faces.is_empty() {
            return spec.font;
        }
        let mut chosen = None;
        for family in spec.families.iter() {
            if let Some(index) = self.best(Some(family.as_str()), None, spec.weight, spec.style) {
                chosen = Some(index);
                break;
            }
        }
        let index = chosen
            .or_else(|| self.best(None, None, spec.weight, spec.style))
            .unwrap_or(0);
        self.synthesize(FontId(index as u16), spec.weight, spec.style)
    }

    pub fn select_char(&self, spec: &FontSpec, ch: char) -> FontId {
        if self.faces.is_empty() {
            return spec.font;
        }
        if !spec.families.is_empty() {
            for family in spec.families.iter() {
                if let Some(index) =
                    self.best(Some(family.as_str()), Some(ch), spec.weight, spec.style)
                {
                    return self.synthesize(FontId(index as u16), spec.weight, spec.style);
                }
            }
            return self.select(spec);
        }
        match self.best(None, Some(ch), spec.weight, spec.style) {
            Some(index) => self.synthesize(FontId(index as u16), spec.weight, spec.style),
            None => self.select(spec),
        }
    }

    pub fn base_font(&self, id: FontId) -> FontId {
        FontId(self.real_index(id).unwrap_or(id.0 as usize) as u16)
    }

    pub fn is_synthetic(&self, id: FontId) -> bool {
        id.0 & SYNTH_FLAG != 0
    }

    pub fn is_synthetic_bold(&self, id: FontId) -> bool {
        self.is_synthetic(id) && id.0 & SYNTH_BOLD != 0
    }

    pub fn is_synthetic_italic(&self, id: FontId) -> bool {
        self.is_synthetic(id) && id.0 & SYNTH_ITALIC != 0
    }

    fn synthesize(&self, base: FontId, weight: u16, style: FontStyle) -> FontId {
        if base.0 > SYNTH_BASE_MASK {
            return base;
        }
        let Some(metrics) = self.faces.get(base.0 as usize) else {
            return base;
        };
        let bold = weight >= 600 && metrics.desc.weight < 600;
        let italic = matches!(style, FontStyle::Italic | FontStyle::Oblique)
            && metrics.desc.style == FontStyle::Normal;
        if !bold && !italic {
            return base;
        }
        let mut bits = SYNTH_FLAG | base.0;
        if bold {
            bits |= SYNTH_BOLD;
        }
        if italic {
            bits |= SYNTH_ITALIC;
        }
        FontId(bits)
    }

    fn real_index(&self, id: FontId) -> Option<usize> {
        if id.0 & SYNTH_FLAG != 0 {
            let base = (id.0 & SYNTH_BASE_MASK) as usize;
            if self.faces.get(base).is_some() {
                return Some(base);
            }
            return None;
        }
        let index = id.0 as usize;
        if self.faces.get(index).is_some() {
            Some(index)
        } else {
            None
        }
    }

    fn best(
        &self,
        family: Option<&str>,
        ch: Option<char>,
        weight: u16,
        style: FontStyle,
    ) -> Option<usize> {
        let mut best: Option<(usize, u16, u16, i32)> = None;
        for (index, metrics) in self.faces.iter().enumerate() {
            if let Some(family) = family {
                if !metrics.desc.family.eq_ignore_ascii_case(family) {
                    continue;
                }
            }
            if let Some(ch) = ch {
                if metrics.glyph(ch) == 0 {
                    continue;
                }
            }
            let rank = style_rank(metrics.desc.style, style);
            let distance = metrics.desc.weight.abs_diff(weight);
            let tie = if weight >= 500 {
                -(metrics.desc.weight as i32)
            } else {
                metrics.desc.weight as i32
            };
            let key = (rank, distance, tie);
            let replace = match best {
                Some((_, best_rank, best_distance, best_tie)) => {
                    key < (best_rank, best_distance, best_tie)
                }
                None => true,
            };
            if replace {
                best = Some((index, rank, distance, tie));
            }
        }
        best.map(|(index, _, _, _)| index)
    }

    fn face(&self, id: FontId) -> Option<Face<'_>> {
        let metrics = self.faces.get(self.real_index(id)?)?;
        Face::parse(&metrics.bytes, 0).ok()
    }

    pub fn rasterize(&self, id: FontId, gid: u16, px: f32) -> Option<GlyphBitmap> {
        self.rasterize_at(id, gid, px, 0.0, 0.0)
    }

    pub fn rasterize_at(
        &self,
        id: FontId,
        gid: u16,
        px: f32,
        dx: f32,
        dy: f32,
    ) -> Option<GlyphBitmap> {
        if !px.is_finite() || px <= 0.0 || !dx.is_finite() || !dy.is_finite() {
            return None;
        }
        let index = self.real_index(id)?;
        let face = self.face(FontId(index as u16))?;
        let shear = if self.is_synthetic_italic(id) {
            SYNTH_SHEAR
        } else {
            0.0
        };
        let mut bounds = Bounds::new(shear);
        face.outline_glyph(ttf_parser::GlyphId(gid), &mut bounds)?;
        if !bounds.any {
            return None;
        }
        let scale = px / face.units_per_em() as f32;
        let left_edge = (bounds.min_x * scale + dx).floor();
        let right_edge = (bounds.max_x * scale + dx).ceil();
        let top_edge = (bounds.max_y * scale - dy).ceil();
        let bottom_edge = (bounds.min_y * scale - dy).floor();
        let width = right_edge - left_edge;
        let height = top_edge - bottom_edge;
        if !(width >= 1.0) || !(height >= 1.0) {
            return None;
        }
        if width > u16::MAX as f32 || height > u16::MAX as f32 {
            return None;
        }
        let left = left_edge as i32;
        let top = -(top_edge as i32);
        if left < i16::MIN as i32
            || left > i16::MAX as i32
            || top < i16::MIN as i32
            || top > i16::MAX as i32
        {
            return None;
        }
        let w = width as usize;
        let h = height as usize;
        let mut rasterizer = Rasterizer::new(w, h);
        let mut outline = RasterOutline {
            rasterizer: &mut rasterizer,
            scale,
            shear,
            dx,
            dy,
            left: left_edge,
            top: top_edge,
            start: point(0.0, 0.0),
            current: point(0.0, 0.0),
        };
        face.outline_glyph(ttf_parser::GlyphId(gid), &mut outline)?;
        drop(outline);
        let mut alpha = vec![0u8; w * h];
        rasterizer.for_each_pixel(|index, coverage| {
            alpha[index] = (coverage.clamp(0.0, 1.0) * 255.0).round() as u8;
        });
        let mut w = w;
        let mut left = left;
        if self.is_synthetic_bold(id) {
            let radius = 1usize;
            let padded_w = w + radius * 2;
            let mut padded = vec![0u8; padded_w * h];
            for y in 0..h {
                for x in 0..w {
                    let value = alpha[y * w + x];
                    if value == 0 {
                        continue;
                    }
                    let row = y * padded_w;
                    for shift in 0..=radius * 2 {
                        let target = &mut padded[row + x + shift];
                        if value > *target {
                            *target = value;
                        }
                    }
                }
            }
            alpha = padded;
            w = padded_w;
            left -= radius as i32;
        }
        Some(GlyphBitmap {
            w: w as u16,
            h: h as u16,
            left: left as i16,
            top: top as i16,
            alpha,
        })
    }
}

fn style_rank(found: FontStyle, wanted: FontStyle) -> u16 {
    if found == wanted {
        return 0;
    }
    match wanted {
        FontStyle::Normal => match found {
            FontStyle::Oblique => 1,
            FontStyle::Italic => 2,
            FontStyle::Normal => 0,
        },
        FontStyle::Italic => match found {
            FontStyle::Oblique => 1,
            FontStyle::Normal => 2,
            FontStyle::Italic => 0,
        },
        FontStyle::Oblique => match found {
            FontStyle::Italic => 1,
            FontStyle::Normal => 2,
            FontStyle::Oblique => 0,
        },
    }
}

struct Bounds {
    shear: f32,
    min_x: f32,
    max_x: f32,
    min_y: f32,
    max_y: f32,
    any: bool,
}

impl Bounds {
    fn new(shear: f32) -> Self {
        Self {
            shear,
            min_x: f32::INFINITY,
            max_x: f32::NEG_INFINITY,
            min_y: f32::INFINITY,
            max_y: f32::NEG_INFINITY,
            any: false,
        }
    }

    fn add(&mut self, x: f32, y: f32) {
        let x = x + self.shear * y;
        self.min_x = self.min_x.min(x);
        self.max_x = self.max_x.max(x);
        self.min_y = self.min_y.min(y);
        self.max_y = self.max_y.max(y);
        self.any = true;
    }
}

impl ttf_parser::OutlineBuilder for Bounds {
    fn move_to(&mut self, x: f32, y: f32) {
        self.add(x, y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.add(x, y);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.add(x1, y1);
        self.add(x, y);
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.add(x1, y1);
        self.add(x2, y2);
        self.add(x, y);
    }

    fn close(&mut self) {}
}

struct RasterOutline<'a> {
    rasterizer: &'a mut Rasterizer,
    scale: f32,
    shear: f32,
    dx: f32,
    dy: f32,
    left: f32,
    top: f32,
    start: Point,
    current: Point,
}

impl RasterOutline<'_> {
    fn map(&self, x: f32, y: f32) -> Point {
        let px = (x + self.shear * y) * self.scale + self.dx;
        let py = y * self.scale - self.dy;
        point(px - self.left, self.top - py)
    }
}

impl ttf_parser::OutlineBuilder for RasterOutline<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        let p = self.map(x, y);
        self.start = p;
        self.current = p;
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.map(x, y);
        self.rasterizer.draw_line(self.current, p);
        self.current = p;
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let start = self.current;
        let control = self.map(x1, y1);
        let end = self.map(x, y);
        // 在设备空间自己细分，而不是交给栅格器的固定容差：`ab_glyph_rasterizer`
        // 的四边形细分只有 3 段起步，24px 的圆环会因此少约 2% 的墨。
        let excess = distance(start, control) + distance(control, end) - distance(start, end);
        let steps = curve_steps(excess);
        let mut previous = start;
        for i in 1..=steps {
            let t = i as f32 / steps as f32;
            let p = quad_at(start, control, end, t);
            self.rasterizer.draw_line(previous, p);
            previous = p;
        }
        self.current = end;
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let start = self.current;
        let control1 = self.map(x1, y1);
        let control2 = self.map(x2, y2);
        let end = self.map(x, y);
        let excess =
            distance(start, control1) + distance(control1, control2) + distance(control2, end)
                - distance(start, end);
        let steps = curve_steps(excess);
        let mut previous = start;
        for i in 1..=steps {
            let t = i as f32 / steps as f32;
            let p = cubic_at(start, control1, control2, end, t);
            self.rasterizer.draw_line(previous, p);
            previous = p;
        }
        self.current = end;
    }

    fn close(&mut self) {
        self.rasterizer.draw_line(self.current, self.start);
        self.current = self.start;
    }
}

fn distance(a: Point, b: Point) -> f32 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    (dx * dx + dy * dy).sqrt()
}

/// 按弦高误差选折线段数：四边形在参数 1/2 处离弦最远，约为控制多边形
/// 超出量的四分之一；取 0.01 像素的目标误差，再夹到 [1, 64]。
fn curve_steps(excess: f32) -> usize {
    if !(excess > 0.0) {
        return 1;
    }
    let steps = (excess * 200.0).sqrt().ceil() as usize;
    steps.clamp(1, 64)
}

fn quad_at(a: Point, b: Point, c: Point, t: f32) -> Point {
    let u = 1.0 - t;
    point(
        u * u * a.x + 2.0 * u * t * b.x + t * t * c.x,
        u * u * a.y + 2.0 * u * t * b.y + t * t * c.y,
    )
}

fn cubic_at(a: Point, b: Point, c: Point, d: Point, t: f32) -> Point {
    let u = 1.0 - t;
    point(
        u * u * u * a.x + 3.0 * u * u * t * b.x + 3.0 * u * t * t * c.x + t * t * t * d.x,
        u * u * u * a.y + 3.0 * u * u * t * b.y + 3.0 * u * t * t * c.y + t * t * t * d.y,
    )
}

impl TextMeasurer for FontBook {
    fn advance(&self, ch: char, font: FontId, size: f32) -> f32 {
        match self
            .real_index(font)
            .and_then(|index| self.faces.get(index))
        {
            Some(metrics) => metrics.advance(ch) * size,
            None => 0.0,
        }
    }

    fn ascent(&self, font: FontId, size: f32) -> f32 {
        match self
            .real_index(font)
            .and_then(|index| self.faces.get(index))
        {
            Some(metrics) => metrics.ascender / metrics.units_per_em * size,
            None => size * 0.8,
        }
    }

    fn descent(&self, font: FontId, size: f32) -> f32 {
        match self
            .real_index(font)
            .and_then(|index| self.faces.get(index))
        {
            Some(metrics) => metrics.descender.abs() / metrics.units_per_em * size,
            None => size * 0.2,
        }
    }

    fn covers(&self, ch: char, font: FontId) -> bool {
        self.real_index(font)
            .and_then(|index| self.faces.get(index))
            .map(|metrics| metrics.glyph(ch) != 0)
            .unwrap_or(false)
    }

    fn glyph_id(&self, ch: char, font: FontId) -> u16 {
        self.real_index(font)
            .and_then(|index| self.faces.get(index))
            .map(|metrics| metrics.glyph(ch))
            .unwrap_or(0)
    }

    fn select(&self, spec: &FontSpec) -> FontId {
        FontBook::select(self, spec)
    }

    fn select_char(&self, spec: &FontSpec, ch: char) -> FontId {
        FontBook::select_char(self, spec, ch)
    }
}

impl Resources for FontBook {
    fn glyph_alpha(&mut self, font: FontId, gid: u16, px: f32) -> Option<GlyphBitmap> {
        self.rasterize(font, gid, px)
    }

    fn glyph_alpha_at(
        &mut self,
        font: FontId,
        gid: u16,
        px: f32,
        dx: f32,
        dy: f32,
    ) -> Option<GlyphBitmap> {
        self.rasterize_at(font, gid, px, dx, dy)
    }

    fn image(&mut self, _id: ImageId) -> Option<ImageData> {
        None
    }
}
