use core::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use ab_glyph_rasterizer::{point, Point, Rasterizer};
use ebook_ir::{FontDesc, FontId, FontSpec, FontStyle, GlyphBitmap, ImageData, ImageId, Resources};
#[cfg(feature = "hyphen-en")]
use ebook_layout::English;
use ebook_layout::{shape_per_char, Hyphenator, ShapeGlyph, TextMeasurer};
use read_fonts::{
    model::glyph::outline::PathSink,
    ps::cff::CffFontRef,
    tables::{
        cmap::{CmapSubtable, EncodingRecord, PlatformId},
        glyf::{
            outline::{Outline, OutlineContext, PathContourStart, ScaleF32},
            Glyf,
        },
        gvar::GlyphVariationData,
        loca::{Loca, LocaGlyph},
        os2::SelectionFlags,
    },
    types::{F2Dot14, Fixed, GlyphId, Point as FontPoint},
    FontRef, TableProvider,
};

mod woff;
pub use woff::{FontLoadError, WoffKind};

const SYNTH_FLAG: u16 = 0x8000;
const SYNTH_BOLD: u16 = 0x1000;
const SYNTH_ITALIC: u16 = 0x2000;
const SYNTH_BASE_MASK: u16 = 0x0fff;
const SYNTH_SHEAR: f32 = 0.22;

/// 光栅缓存上限。超限就整体清空，而不是逐条淘汰：字位图比键大得多，
/// 记 LRU 顺序的收益不如直接把生命周期切成小段。清空后仍会重新填，
/// 长会话的内存因此有硬上限。
const RASTER_CACHE_MAX: usize = 4096;

/// 整形缓存上限。同理，整段 run 的代价高于几个键，超限整体清空。
const SHAPE_CACHE_MAX: usize = 1024;

/// 光栅缓存的键。`FontId` 用逻辑 id 而不是 base face：合成粗体/斜体
/// 走同一份轮廓，但增益/切变之后的位图不同，不能共用条目。浮点按 bit
/// 入键，保证命中判定与逐字节结果一样严格（`-0.0`/`0.0` 也分开）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct RasterKey {
    font: FontId,
    gid: u16,
    px: u32,
    dx: u32,
    dy: u32,
}

impl RasterKey {
    fn new(font: FontId, gid: u16, px: f32, dx: f32, dy: f32) -> Self {
        Self {
            font,
            gid,
            px: px.to_bits(),
            dx: dx.to_bits(),
            dy: dy.to_bits(),
        }
    }
}

/// 整形缓存的键。文本和语言都在里面：同样的字节在不同 `locl` 语言下
/// 可能得到不同的字形；字号按 bit 入键，避免 `-0.0`/`0.0` 之类混用。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ShapeKey {
    font: FontId,
    size: u32,
    text: String,
    lang: Option<String>,
}

impl ShapeKey {
    fn new(font: FontId, size: f32, text: &str, lang: Option<&str>) -> Self {
        Self {
            font,
            size: size.to_bits(),
            text: text.to_owned(),
            lang: lang.map(str::to_owned),
        }
    }
}

/// 一次失败的字体加载。`FontBook` 保留最近所有失败，宿主可以取走显示；
/// 失败不会阻止其它字体或排版，只是不再静默丢弃。
#[derive(Clone, Debug, PartialEq)]
pub struct FontDiagnostic {
    pub desc: FontDesc,
    pub error: FontLoadError,
}

impl fmt::Display for FontDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.desc.family.is_empty() {
            write!(f, "字体加载失败: {}", self.error)
        } else {
            write!(
                f,
                "字体 {:?}（weight {}）加载失败: {}",
                self.desc.family, self.desc.weight, self.error
            )
        }
    }
}

struct Metrics {
    desc: FontDesc,
    /// 度量、cmap 与轮廓都从这份字节读取；harfrust 也共享同一个 Arc，
    /// 不会为整形再复制一遍几十 MB 的 CJK 字体。
    bytes: Arc<Vec<u8>>,
    /// read-fonts 的 owning `Font`。它自带 shaping/layout 缓存，
    /// 必须在 `FontBook` 生命周期内复用；每次 `shape` 重建会丢掉这份缓存。
    /// `None` 表示 read-fonts 读不了这份数据，此时退回逐字。
    hb_font: Option<harfrust::Font>,
    units_per_em: f32,
    ascender: f32,
    descender: f32,
    advances: Vec<u16>,
    cmap: Vec<(u32, u16)>,
}

impl core::fmt::Debug for Metrics {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Metrics")
            .field("desc", &self.desc)
            .field("units_per_em", &self.units_per_em)
            .field("ascender", &self.ascender)
            .field("descender", &self.descender)
            .field("advances", &self.advances.len())
            .field("cmap", &self.cmap.len())
            .finish()
    }
}

impl Metrics {
    fn parse(desc: FontDesc, bytes: Vec<u8>) -> Result<Self, FontLoadError> {
        let bytes = woff::unpack(bytes)?;
        let bytes = Arc::new(bytes);
        let font =
            FontRef::from_index(bytes.as_slice(), 0).map_err(|_| FontLoadError::InvalidFont)?;
        // 与原实现一致：head/hhea/maxp 是必需表，head 的 upem 和 loca 格式
        // 还必须在合法范围内。
        let head = font.head().map_err(|_| FontLoadError::InvalidFont)?;
        let units_per_em = head.units_per_em();
        if !(16..=16384).contains(&units_per_em) || !matches!(head.index_to_loc_format(), 0 | 1) {
            return Err(FontLoadError::InvalidFont);
        }
        let units_per_em = units_per_em as f32;
        font.hhea().map_err(|_| FontLoadError::InvalidFont)?;
        let maxp = font.maxp().map_err(|_| FontLoadError::InvalidFont)?;
        if maxp.num_glyphs() == 0 {
            return Err(FontLoadError::InvalidFont);
        }
        let (ascender, descender) = line_metrics(&font).ok_or(FontLoadError::InvalidFont)?;
        // harfrust 与度量/轮廓共用同一份字节，只克隆 Arc，不拷贝。
        let source: Arc<dyn AsRef<[u8]> + Send + Sync> = bytes.clone();
        let hb_font = harfrust::Font::new(source, 0);
        let hmtx = font.hmtx().ok();
        let advances = (0..maxp.num_glyphs())
            .map(|gid| {
                hmtx.as_ref()
                    .and_then(|table| table.advance(GlyphId::new(gid as u32)))
                    .unwrap_or(0)
            })
            .collect();
        let cmap = build_cmap(&font);
        Ok(Self {
            desc,
            bytes,
            hb_font,
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
    /// 加载失败的字体。与 `faces` 分开，失败不会占用 `FontId`，也不会改变
    /// fallback 顺序；宿主可以读出来显示，避免坏 woff 静默消失。
    diagnostics: Vec<FontDiagnostic>,
    /// 文档语言（BCP-47 前缀，如 `zh` / `ja` / `en`）。
    ///
    /// 排版层的 `TextMeasurer::shape` 拿不到 `DocMeta`，所以语言由宿主在排版前
    /// 注入；没设置时按字形猜（见 [`guess_language`]），猜不到就不设，
    /// harfrust 会把脚本留成 `Zyyy`。
    lang: Option<String>,
    /// 内置英文断字词典（`hyphen-en` feature）。零大小类型，`hyphenator()` 借出去；
    /// 没有这个 feature 时字段不存在，`hyphenator()` 返回 `None`。
    #[cfg(feature = "hyphen-en")]
    english: English,
    /// 字形光栅缓存，绑定 `FontBook` 生命周期；不放进全局/`thread_local`。
    /// `RefCell` 让 `&self` 的 `Resources::glyph_alpha_at` 也能写缓存。
    rasters: RefCell<BTreeMap<RasterKey, Option<GlyphBitmap>>>,
    /// 整形缓存，值保存 `shape` 写入 `out` 的字形序列与返回的 advance。
    shapes: RefCell<BTreeMap<ShapeKey, (f32, Vec<ShapeGlyph>)>>,
}

impl FontBook {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, bytes: Vec<u8>) -> Option<FontId> {
        self.add_face(FontDesc::default(), bytes)
    }

    /// 可诊断版本的 [`Self::add`]。
    pub fn try_add(&mut self, bytes: Vec<u8>) -> Result<FontId, FontLoadError> {
        self.try_add_face(FontDesc::default(), bytes)
    }

    pub fn add_face(&mut self, desc: FontDesc, bytes: Vec<u8>) -> Option<FontId> {
        self.try_add_face(desc, bytes).ok()
    }

    /// 可诊断版本：失败时返回具体原因，同时记入 [`Self::diagnostics`]。
    pub fn try_add_face(
        &mut self,
        desc: FontDesc,
        bytes: Vec<u8>,
    ) -> Result<FontId, FontLoadError> {
        let mut desc = desc;
        if desc.weight == 0 {
            desc.weight = 400;
        }
        let metrics = match Metrics::parse(desc.clone(), bytes) {
            Ok(metrics) => metrics,
            Err(error) => {
                self.diagnostics.push(FontDiagnostic {
                    desc,
                    error: error.clone(),
                });
                return Err(error);
            }
        };
        let id = FontId(self.faces.len() as u16);
        self.faces.push(metrics);
        // 新 face 会改变后续 select/fallback 的候选集。旧逻辑 `FontId` 仍指向
        // 原来的 base face，本身不会串味，但候选集变化后旧缓存已不再代表
        // “当前 FontBook 的结果”，直接清空最稳。
        self.clear_caches();
        Ok(id)
    }

    /// 加载失败的字体，按发生顺序。成功的字体不在这里。
    pub fn diagnostics(&self) -> &[FontDiagnostic] {
        &self.diagnostics
    }

    /// 取走诊断。宿主通常在一次批量加载后调用一次，避免重复显示。
    pub fn take_diagnostics(&mut self) -> Vec<FontDiagnostic> {
        core::mem::take(&mut self.diagnostics)
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
        self.try_add_styled(family, weight, style, bytes).ok()
    }

    /// 可诊断版本的 [`Self::add_styled`]。
    pub fn try_add_styled(
        &mut self,
        family: &str,
        weight: u16,
        style: FontStyle,
        bytes: Vec<u8>,
    ) -> Result<FontId, FontLoadError> {
        self.try_add_face(
            FontDesc {
                family: family.into(),
                weight,
                style,
            },
            bytes,
        )
    }

    /// 设置文档语言，只影响 OpenType 的 `locl` 之类的语言相关替换。
    pub fn set_language(&mut self, lang: Option<&str>) {
        self.lang = lang.map(|value| value.to_ascii_lowercase());
        // 语言进整形键，理论上换语言会自然落到新键；但旧语言的条目不再有用，
        // 而且清空后不会让长会话因为来回切语言而积累两份缓存。
        self.shapes.borrow_mut().clear();
    }

    fn clear_caches(&self) {
        self.rasters.borrow_mut().clear();
        self.shapes.borrow_mut().clear();
    }

    pub fn language(&self) -> Option<&str> {
        self.lang.as_deref()
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

    fn face(&self, id: FontId) -> Option<FontRef<'_>> {
        let metrics = self.faces.get(self.real_index(id)?)?;
        FontRef::from_index(metrics.bytes.as_slice(), 0).ok()
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
        // 先只确认逻辑 id 有效，不解析 `Face`：命中缓存时不应再做一次字体解析。
        let index = self.real_index(id)?;
        let key = RasterKey::new(id, gid, px, dx, dy);
        if let Some(cached) = self.rasters.borrow().get(&key) {
            return cached.clone();
        }
        let result = self.rasterize_uncached(FontId(index as u16), id, gid, px, dx, dy);
        // `None` 也缓存：缺字、越界等失败结果同样会在翻页/缩放时重复询问。
        let mut cache = self.rasters.borrow_mut();
        if cache.len() >= RASTER_CACHE_MAX {
            cache.clear();
        }
        cache.insert(key, result.clone());
        result
    }

    fn rasterize_uncached(
        &self,
        base: FontId,
        id: FontId,
        gid: u16,
        px: f32,
        dx: f32,
        dy: f32,
    ) -> Option<GlyphBitmap> {
        let face = self.face(base)?;
        let shear = if self.is_synthetic_italic(id) {
            SYNTH_SHEAR
        } else {
            0.0
        };
        let mut bounds = Bounds::new(shear);
        draw_glyph(&face, gid, &mut bounds)?;
        if !bounds.any {
            return None;
        }
        let scale = px / face.head().ok()?.units_per_em() as f32;
        let left_edge = (bounds.min_x * scale + dx).floor();
        let right_edge = (bounds.max_x * scale + dx).ceil();
        let top_edge = (bounds.max_y * scale - dy).ceil();
        let bottom_edge = (bounds.min_y * scale - dy).floor();
        let width = right_edge - left_edge;
        let height = top_edge - bottom_edge;
        if !(width >= 1.0 && height >= 1.0) {
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
        draw_glyph(&face, gid, &mut outline)?;
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

/// 读出与原实现相同的 ascender/descender：OS/2 显式要求 typo 度量时
/// 优先用它，否则用 hhea；hhea 为 0 时再退回 typo/win 度量。
fn line_metrics(font: &FontRef<'_>) -> Option<(f32, f32)> {
    let hhea = font.hhea().ok()?;
    let os2 = font.os2().ok();
    // 原实现只在 OS/2 version >= 4 时认 USE_TYPO_METRICS。
    let use_typo = os2.as_ref().is_some_and(|table| {
        table.version() >= 4
            && table
                .fs_selection()
                .contains(SelectionFlags::USE_TYPO_METRICS)
    });
    let ascent = if use_typo {
        os2.as_ref().map(|table| table.s_typo_ascender() as i32)
    } else {
        None
    }
    .unwrap_or_else(|| {
        let mut value = hhea.ascender().to_i16() as i32;
        if value == 0 {
            if let Some(os2) = &os2 {
                value = os2.s_typo_ascender() as i32;
                if value == 0 {
                    value = os2.us_win_ascent() as i16 as i32;
                }
            }
        }
        value
    });
    let descent = if use_typo {
        os2.as_ref().map(|table| table.s_typo_descender() as i32)
    } else {
        None
    }
    .unwrap_or_else(|| {
        let mut value = hhea.descender().to_i16() as i32;
        if value == 0 {
            if let Some(os2) = &os2 {
                value = os2.s_typo_descender() as i32;
                if value == 0 {
                    value = -(os2.us_win_descent() as i16 as i32);
                }
            }
        }
        value
    });
    Some((ascent as f32, descent as f32))
}

fn is_unicode_subtable(record: &EncodingRecord, subtable: &CmapSubtable<'_>) -> bool {
    const WINDOWS_UNICODE_BMP: u16 = 1;
    const WINDOWS_UNICODE_FULL: u16 = 10;
    match record.platform_id() {
        PlatformId::Unicode => true,
        PlatformId::Windows if record.encoding_id() == WINDOWS_UNICODE_BMP => true,
        PlatformId::Windows => {
            record.encoding_id() == WINDOWS_UNICODE_FULL
                && matches!(
                    subtable,
                    CmapSubtable::Format12(_) | CmapSubtable::Format13(_)
                )
        }
        _ => false,
    }
}

/// 按原实现的语义枚举所有 Unicode cmap 子表，只保留非 0 映射；
/// 同一码点出现多次时保留排序后的第一条，随后查询走二分。
fn build_cmap(font: &FontRef<'_>) -> Vec<(u32, u16)> {
    let mut cmap = Vec::new();
    if let Ok(table) = font.cmap() {
        for record in table.encoding_records() {
            let Ok(subtable) = record.subtable(table.offset_data()) else {
                continue;
            };
            if !is_unicode_subtable(record, &subtable) {
                continue;
            }
            for (code, gid) in subtable.iter() {
                let gid = gid.to_u32();
                if gid != 0 && gid <= u16::MAX as u32 {
                    cmap.push((code, gid as u16));
                }
            }
        }
    }
    cmap.sort_unstable_by_key(|entry| entry.0);
    cmap.dedup_by_key(|entry| entry.0);
    cmap
}

/// 把 `&mut dyn PathSink` 包成 Sized，避免 glyf/CFF 两条路径按 pen 类型
/// 单态化，wasm 体积只留一份。`dx` 用来抵消 read-fonts glyf 按左侧边距
/// 平移坐标的约定，恢复原来的“字形原点”坐标。
struct DynSink<'a> {
    inner: &'a mut dyn PathSink,
    dx: f32,
}

impl PathSink for DynSink<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        self.inner.move_to(x + self.dx, y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.inner.line_to(x + self.dx, y);
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.inner.quad_to(cx0 + self.dx, cy0, x + self.dx, y);
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.inner
            .curve_to(cx0 + self.dx, cy0, cx1 + self.dx, cy1, x + self.dx, y);
    }

    fn close(&mut self) {
        self.inner.close();
    }
}

/// 只给 glyf 轮廓用的最小上下文：不读 gvar/HVAR/OS2/hmtx，因此与原来的
/// 静态字体行为一致，也不把变体度量的代码拉进来。
struct StaticOutline<'a> {
    glyf: Glyf<'a>,
    loca: Loca<'a>,
    units_per_em: u16,
}

impl<'a> OutlineContext<'a> for StaticOutline<'a> {
    fn glyph(&self, glyph: GlyphId) -> Option<LocaGlyph<'a>> {
        self.loca.get(glyph, &self.glyf)
    }

    fn glyph_variation_data(&self, _glyph: GlyphId) -> Option<GlyphVariationData<'a>> {
        None
    }

    fn has_gvar(&self) -> bool {
        false
    }

    fn phantom_point_deltas(
        &self,
        _glyph: GlyphId,
    ) -> Option<[FontPoint<Fixed>; read_fonts::tables::glyf::PHANTOM_POINT_COUNT]> {
        None
    }

    fn units_per_em(&self) -> u16 {
        self.units_per_em
    }

    fn h_metrics(&self, _glyph: GlyphId) -> (i32, i32) {
        (0, 0)
    }

    fn h_line_metrics(&self) -> (i32, i32) {
        (0, 0)
    }
}

/// 按原实现的来源顺序取轮廓：先 glyf，再 CFF；不启用 CFF2/VARC。
/// 首点为二次离曲线点时从第二个点/首尾中点起步（ScanForward）。
fn draw_glyph(font: &FontRef<'_>, gid: u16, sink: &mut dyn PathSink) -> Option<()> {
    let glyph = GlyphId::new(gid as u32);
    let mut sink = DynSink {
        inner: sink,
        dx: 0.0,
    };
    if let (Ok(glyf), Ok(loca)) = (font.glyf(), font.loca(None)) {
        let context = StaticOutline {
            glyf,
            loca,
            units_per_em: font.head().ok()?.units_per_em(),
        };
        let mut outline = Outline::<ScaleF32>::new();
        let loaded = outline.load(&context, glyph, None).ok()?;
        // read-fonts 的 OutlineRef 已经按左侧边距平移；加回去得到字形原点坐标。
        sink.dx = loaded.phantom_points()[0].x;
        return loaded
            .to_path(PathContourStart::ScanForward, &mut sink)
            .then_some(());
    }
    let cff = CffFontRef::new_cff(
        font.cff().ok()?.offset_data().as_bytes(),
        0,
        Some(font.head().ok()?.units_per_em() as i32),
    )
    .ok()?;
    let coords: &[F2Dot14] = &[];
    let subfont = cff.subfont(cff.subfont_index(glyph)?, coords).ok()?;
    cff.draw(&subfont, glyph, coords, None, &mut sink).ok()?;
    Some(())
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

impl PathSink for Bounds {
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

impl PathSink for RasterOutline<'_> {
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
    // `!` 与 `<=` 对 NaN 的判定不同：这里把 NaN 也当作“没有超出量”，
    // 与原来的 `!(excess > 0.0)` 完全等价。
    if excess.is_nan() || excess <= 0.0 {
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

/// 从文本本身猜语言：只用于 `locl` 之类的替换，猜不到就算了。
///
/// 优先看假名/谚文（日/韩必须区分），再看汉字（默认按中文），最后拉丁字母按英文。
fn guess_language(text: &str) -> Option<&'static str> {
    let mut han = false;
    let mut latin = false;
    for ch in text.chars().take(64) {
        match ch as u32 {
            0x3040..=0x30FF | 0x31F0..=0x31FF => return Some("ja"),
            0xAC00..=0xD7A3 | 0x1100..=0x11FF | 0x3130..=0x318F => return Some("ko"),
            0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF => han = true,
            _ => {
                if ch.is_ascii_alphabetic() {
                    latin = true;
                }
            }
        }
    }
    if han {
        Some("zh")
    } else if latin {
        Some("en")
    } else {
        None
    }
}

impl TextMeasurer for FontBook {
    /// 用 harfrust（HarfBuzz 的 Rust 移植）整形：kerning、ligature 默认开，
    /// script/language 从入参或文本猜。
    ///
    /// 返回的字形按**逻辑顺序**（RTL run 内部会翻回去），`ch` 是簇首字符在
    /// `text` 里的字节偏移，和逐字实现保持同一套语义。
    fn shape(&self, text: &str, font: FontId, size: f32, out: &mut Vec<ShapeGlyph>) -> f32 {
        let Some(metrics) = self
            .real_index(font)
            .and_then(|index| self.faces.get(index))
        else {
            return shape_per_char(self, text, font, size, out);
        };
        if !size.is_finite() || size <= 0.0 {
            return 0.0;
        }
        // 键包含字体、字号、文本和当前注入的语言。语言同时影响 `locl`，
        // 文本本身又决定脚本/猜出来的语言，所以命中即可直接复用整段结果。
        let key = ShapeKey::new(font, size, text, self.lang.as_deref());
        if let Some((total, glyphs)) = self.shapes.borrow().get(&key) {
            out.extend_from_slice(glyphs);
            return *total;
        }
        // harfrust 的 `Font` 是 read-fonts 的 owning font，不按 px 建面，
        // 整形永远以 units-per-em 输出，下面统一乘 `size / upem`。
        let Some(hb_font) = metrics.hb_font.as_ref() else {
            return shape_per_char(self, text, font, size, out);
        };
        let shaper = harfrust::ShaperFont::new(hb_font);
        let mut buffer = harfrust::Buffer::new();
        buffer.push_str(text);
        // 语言优先级：宿主注入 > 文本猜。脚本交给 guess，猜不到就是 Zyyy。
        if let Some(lang) = self.lang.as_deref().or_else(|| guess_language(text)) {
            if let Ok(lang) = lang.parse::<harfrust::Language>() {
                buffer.set_language(Some(lang));
            }
        }
        buffer.guess_segment_properties();
        let rtl = buffer.direction() == harfrust::Direction::RightToLeft;
        // 字体损坏等情况下 harfrust 直接报错（HarfBuzz 会退回 dummy shaper），
        // 这里退回逐字 advance，排版不至于整段失败。
        if harfrust::shape(&shaper, &mut buffer, harfrust::ShapeOptions::new()).is_err() {
            return shape_per_char(self, text, font, size, out);
        }
        let scale = size / metrics.units_per_em;
        let mut total = 0.0f32;
        let mut glyphs: Vec<ShapeGlyph> = Vec::with_capacity(buffer.len());
        for (info, position) in buffer
            .glyph_infos()
            .iter()
            .zip(buffer.glyph_positions().iter())
        {
            let dx = position.x_advance as f32 * scale;
            let dy = position.y_offset as f32 * scale;
            total += dx;
            glyphs.push(ShapeGlyph {
                gid: info.glyph_id.min(u16::MAX as u32) as u16,
                dx,
                dy,
                ch: info.cluster,
            });
        }
        if rtl {
            // HarfBuzz 对 RTL 输出的是视觉顺序；IR 要逻辑顺序。
            glyphs.reverse();
        }
        out.extend_from_slice(&glyphs);
        let mut cache = self.shapes.borrow_mut();
        if cache.len() >= SHAPE_CACHE_MAX {
            cache.clear();
        }
        cache.insert(key, (total, glyphs));
        total
    }

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

    /// 宿主加载了字体，就已经明确接受“排版要用真实字体”：断字词典也在这里注入。
    /// 没装任何字体时 `layout` 走 `StubMeasurer`，`hyphens: auto` 不生效。
    /// 英文词典本身还受 `hyphen-en` feature 控制，默认不链入 `en.pat`。
    fn hyphenator(&self) -> Option<&dyn Hyphenator> {
        #[cfg(feature = "hyphen-en")]
        {
            Some(&self.english)
        }
        #[cfg(not(feature = "hyphen-en"))]
        {
            None
        }
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
