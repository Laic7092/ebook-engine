//! 图片头解析与最小 PNG 解码。
//!
//! 排版只需要**固有尺寸**，渲染只需要**像素**。格式层因此只做两件事：
//! [`image_size`] 从文件头读出宽高，[`decode_png`] 把 PNG 解成 RGBA。
//! 两者都不引第三方图像库：头解析是纯字节读取，PNG 走已有的 `miniz_oxide`。
//!
//! 已知范围：PNG 只支持 8 位、非隔行、颜色类型 0/2/3/4/6（灰度、RGB、调色板、
//! 灰度+alpha、RGBA）。其它格式只给尺寸，不给像素——渲染后端会画占位。
//! SVG 也只在文件前部读根标签的 `width`/`height`/`viewBox`，不做光栅化。

use alloc::vec::Vec;

use ebook_ir::{ImageData, SizeF};

/// 从文件头读取图片固有尺寸。支持 PNG / JPEG / GIF / WebP / SVG。
///
/// 截断或无法识别的输入返回 `None`，不 panic。尺寸为 0 也按无效处理。
pub fn image_size(bytes: &[u8]) -> Option<SizeF> {
    let size = png_size(bytes)
        .or_else(|| jpeg_size(bytes))
        .or_else(|| gif_size(bytes))
        .or_else(|| webp_size(bytes))
        .or_else(|| svg_size(bytes))?;
    if size.w <= 0.0 || size.h <= 0.0 || !size.w.is_finite() || !size.h.is_finite() {
        None
    } else {
        Some(size)
    }
}

/// 从文件头猜 MIME。找不到时返回 `None`，由宿主按 `Book::asset` 的声明兜底。
pub fn image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF8") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if svg_tag(bytes).is_some() {
        Some("image/svg+xml")
    } else {
        None
    }
}

fn be_u32(bytes: &[u8], at: usize) -> Option<u32> {
    let slice = bytes.get(at..at + 4)?;
    Some(u32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn be_u16(bytes: &[u8], at: usize) -> Option<u16> {
    let slice = bytes.get(at..at + 2)?;
    Some(u16::from_be_bytes([slice[0], slice[1]]))
}

fn le_u16(bytes: &[u8], at: usize) -> Option<u16> {
    let slice = bytes.get(at..at + 2)?;
    Some(u16::from_le_bytes([slice[0], slice[1]]))
}

fn le_u24(bytes: &[u8], at: usize) -> Option<u32> {
    let slice = bytes.get(at..at + 3)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], 0]))
}

fn png_size(bytes: &[u8]) -> Option<SizeF> {
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return None;
    }
    // 第一个 chunk 必须是 IHDR：长度(4) 类型(4) 宽(4) 高(4)。
    if bytes.get(12..16)? != b"IHDR" {
        return None;
    }
    let w = be_u32(bytes, 16)?;
    let h = be_u32(bytes, 20)?;
    Some(SizeF::new(w as f32, h as f32))
}

/// JPEG：扫描到 SOFn（C0..CF，跳过 DHT/DAC/RSTn），读高/宽。
fn jpeg_size(bytes: &[u8]) -> Option<SizeF> {
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return None;
    }
    let mut i = 2usize;
    while i + 1 < bytes.len() {
        if bytes[i] != 0xFF {
            i += 1;
            continue;
        }
        let mut marker = bytes[i + 1];
        // 允许 0xFF 填充字节。
        while marker == 0xFF && i + 2 < bytes.len() {
            i += 1;
            marker = bytes[i + 1];
        }
        i += 2;
        // 无长度的独立标记。
        if marker == 0x01 || (0xD0..=0xD9).contains(&marker) {
            continue;
        }
        let len = be_u16(bytes, i)? as usize;
        if len < 2 {
            return None;
        }
        let is_sof = matches!(
            marker,
            0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF
        );
        if is_sof {
            // 精度(1) 高(2) 宽(2)
            let h = be_u16(bytes, i + 3)?;
            let w = be_u16(bytes, i + 5)?;
            if w == 0 || h == 0 {
                return None;
            }
            return Some(SizeF::new(w as f32, h as f32));
        }
        i = i.checked_add(len)?;
    }
    None
}

fn gif_size(bytes: &[u8]) -> Option<SizeF> {
    if bytes.len() < 10 || !bytes.starts_with(b"GIF8") {
        return None;
    }
    let w = le_u16(bytes, 6)?;
    let h = le_u16(bytes, 8)?;
    Some(SizeF::new(w as f32, h as f32))
}

fn webp_size(bytes: &[u8]) -> Option<SizeF> {
    if bytes.len() < 16 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return None;
    }
    let kind = bytes.get(12..16)?;
    let data = bytes.get(20..)?;
    match kind {
        b"VP8X" => {
            // flags(4) 画布宽-1(3) 画布高-1(3)
            let w = le_u24(data, 4)? + 1;
            let h = le_u24(data, 7)? + 1;
            Some(SizeF::new(w as f32, h as f32))
        }
        b"VP8L" => {
            // 0x2f + 14 位宽-1 + 14 位高-1（小端位流）
            if data.first().copied()? != 0x2f {
                return None;
            }
            let bits = le_u32(data, 1)?;
            let w = (bits & 0x3FFF) + 1;
            let h = ((bits >> 14) & 0x3FFF) + 1;
            Some(SizeF::new(w as f32, h as f32))
        }
        b"VP8 " => {
            // 3 字节起始码 + 宽(2) 高(2)，宽高各 14 位有效。
            if data.get(3..6)? != [0x9d, 0x01, 0x2a] {
                return None;
            }
            let w = (le_u16(data, 6)? & 0x3FFF) as u32;
            let h = (le_u16(data, 8)? & 0x3FFF) as u32;
            Some(SizeF::new(w as f32, h as f32))
        }
        _ => None,
    }
}

fn le_u32(bytes: &[u8], at: usize) -> Option<u32> {
    let slice = bytes.get(at..at + 4)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

// ---------------------------------------------------------------------------
// SVG 固有尺寸
// ---------------------------------------------------------------------------

/// 只扫描文件前部。SVG 的根标签通常就在开头，没必要为防御性解析读完整文件。
const SVG_SCAN_LIMIT: usize = 8 * 1024;

/// 找到根 `<svg>` 标签，返回 `<svg` 与结尾 `>` 之间的属性文本。
///
/// 跳过 BOM、前导空白、XML 声明、注释与 DOCTYPE；遇到别的根元素就放弃。
fn svg_tag(bytes: &[u8]) -> Option<&[u8]> {
    let bytes = &bytes[..bytes.len().min(SVG_SCAN_LIMIT)];
    let mut i = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        3
    } else {
        0
    };
    loop {
        i = skip_ascii_space(bytes, i);
        let rest = bytes.get(i..)?;
        if rest.starts_with(b"<?") {
            // XML 声明或其它处理指令。
            i = find_bytes(bytes, i + 2, b"?>")?.checked_add(2)?;
        } else if rest.starts_with(b"<!--") {
            i = find_bytes(bytes, i + 4, b"-->")?.checked_add(3)?;
        } else if starts_with_ignore_ascii_case(rest, b"<!DOCTYPE") {
            // DOCTYPE 可能带内部子集，`>` 不一定就是结尾。
            i = find_doctype_end(bytes, i + 9)?.checked_add(1)?;
        } else if rest.starts_with(b"<svg") {
            let boundary = *bytes.get(i + 4)?;
            if !boundary.is_ascii_whitespace() && boundary != b'>' && boundary != b'/' {
                return None;
            }
            let end = find_tag_end(bytes, i + 4)?;
            return bytes.get(i + 4..end);
        } else {
            return None;
        }
    }
}

fn skip_ascii_space(bytes: &[u8], mut i: usize) -> usize {
    while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    i
}

fn starts_with_ignore_ascii_case(bytes: &[u8], prefix: &[u8]) -> bool {
    bytes
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

fn find_bytes(bytes: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    let mut i = from;
    while i.checked_add(needle.len())? <= bytes.len() {
        if bytes.get(i..i + needle.len())? == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// 找标签结尾的 `>`，引号内的 `>` 不算。
fn find_tag_end(bytes: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    let mut quote = None;
    while let Some(&b) = bytes.get(i) {
        match quote {
            Some(q) => {
                if b == q {
                    quote = None;
                }
            }
            None => match b {
                b'\'' | b'"' => quote = Some(b),
                b'>' => return Some(i),
                _ => {}
            },
        }
        i += 1;
    }
    None
}

fn find_doctype_end(bytes: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    let mut quote = None;
    let mut depth = 0usize;
    while let Some(&b) = bytes.get(i) {
        match quote {
            Some(q) => {
                if b == q {
                    quote = None;
                }
            }
            None => match b {
                b'\'' | b'"' => quote = Some(b),
                b'[' => depth += 1,
                b']' => depth = depth.saturating_sub(1),
                b'>' if depth == 0 => return Some(i),
                _ => {}
            },
        }
        i += 1;
    }
    None
}

/// 在属性文本里找某个属性值，属性顺序任意、名字大小写不敏感、单双引号都支持。
fn svg_attr<'a>(tag: &'a [u8], wanted: &[u8]) -> Option<&'a [u8]> {
    let mut i = 0;
    while i < tag.len() {
        while tag
            .get(i)
            .is_some_and(|b| b.is_ascii_whitespace() || *b == b'/')
        {
            i += 1;
        }
        if i >= tag.len() {
            break;
        }
        let name_start = i;
        while tag
            .get(i)
            .is_some_and(|b| !b.is_ascii_whitespace() && *b != b'=' && *b != b'/' && *b != b'>')
        {
            i += 1;
        }
        let name = tag.get(name_start..i)?;
        i = skip_ascii_space(tag, i);
        let value = if tag.get(i) == Some(&b'=') {
            i += 1;
            i = skip_ascii_space(tag, i);
            match tag.get(i).copied() {
                Some(q @ (b'\'' | b'"')) => {
                    i += 1;
                    let start = i;
                    while tag.get(i).is_some_and(|b| *b != q) {
                        i += 1;
                    }
                    let end = i;
                    i = i.checked_add(1)?;
                    Some(tag.get(start..end)?)
                }
                Some(_) => {
                    let start = i;
                    while tag
                        .get(i)
                        .is_some_and(|b| !b.is_ascii_whitespace() && *b != b'>' && *b != b'/')
                    {
                        i += 1;
                    }
                    Some(tag.get(start..i)?)
                }
                None => None,
            }
        } else {
            None
        };
        if name.eq_ignore_ascii_case(wanted) {
            return value;
        }
        if i == name_start {
            // 兜底推进，避免空属性名让循环原地打转。
            i += 1;
        }
    }
    None
}

/// 解析 SVG 的绝对长度，按 CSS px 换算。百分比、负数和未知单位都算无效。
fn svg_length(value: &[u8]) -> Option<f32> {
    let text = core::str::from_utf8(value).ok()?.trim();
    let bytes = text.as_bytes();
    let mut i = 0;
    if matches!(bytes.get(i), Some(b'+' | b'-')) {
        i += 1;
    }
    let mut digits = 0;
    while bytes.get(i).is_some_and(u8::is_ascii_digit) {
        i += 1;
        digits += 1;
    }
    if bytes.get(i) == Some(&b'.') {
        i += 1;
        while bytes.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return None;
    }
    if matches!(bytes.get(i), Some(b'e' | b'E')) {
        let mut j = i + 1;
        if matches!(bytes.get(j), Some(b'+' | b'-')) {
            j += 1;
        }
        let exp_start = j;
        while bytes.get(j).is_some_and(u8::is_ascii_digit) {
            j += 1;
        }
        if j > exp_start {
            i = j;
        }
    }
    let number = text.get(..i)?.parse::<f64>().ok()?;
    let unit = text.get(i..)?.trim();
    let scale = if unit.is_empty() || unit.eq_ignore_ascii_case("px") {
        1.0
    } else if unit.eq_ignore_ascii_case("pt") {
        4.0 / 3.0
    } else if unit.eq_ignore_ascii_case("pc") {
        16.0
    } else if unit.eq_ignore_ascii_case("in") {
        96.0
    } else if unit.eq_ignore_ascii_case("cm") {
        96.0 / 2.54
    } else if unit.eq_ignore_ascii_case("mm") {
        96.0 / 25.4
    } else {
        return None;
    };
    let px = number * scale;
    if !px.is_finite() || px <= 0.0 {
        return None;
    }
    Some(px as f32)
}

fn svg_viewbox(value: &[u8]) -> Option<SizeF> {
    let text = core::str::from_utf8(value).ok()?;
    let mut numbers = [0.0f64; 4];
    let mut count = 0usize;
    for token in text.split(|c: char| c.is_ascii_whitespace() || c == ',') {
        if token.is_empty() {
            continue;
        }
        if count == numbers.len() {
            return None;
        }
        numbers[count] = token.parse::<f64>().ok()?;
        count += 1;
    }
    if count != numbers.len() || !numbers.iter().all(|n| n.is_finite()) {
        return None;
    }
    let (w, h) = (numbers[2], numbers[3]);
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    Some(SizeF::new(w as f32, h as f32))
}

fn svg_size(bytes: &[u8]) -> Option<SizeF> {
    let tag = svg_tag(bytes)?;
    let width = svg_attr(tag, b"width").and_then(svg_length);
    let height = svg_attr(tag, b"height").and_then(svg_length);
    let viewbox = svg_attr(tag, b"viewBox").and_then(svg_viewbox);
    let size = match (width, height) {
        (Some(w), Some(h)) => SizeF::new(w, h),
        // 只有一边有单位时，借用 viewBox 的宽高比补另一边。
        (Some(w), None) => {
            let vb = viewbox?;
            SizeF::new(w, w * vb.h / vb.w)
        }
        (None, Some(h)) => {
            let vb = viewbox?;
            SizeF::new(h * vb.w / vb.h, h)
        }
        (None, None) => viewbox?,
    };
    (size.w.is_finite() && size.h.is_finite()).then_some(size)
}

// ---------------------------------------------------------------------------
// PNG 解码
// ---------------------------------------------------------------------------

struct Header {
    width: u32,
    height: u32,
    depth: u8,
    color: u8,
    interlace: u8,
}

/// 把 8 位、非隔行 PNG 解成 RGBA8。范围外的 PNG 返回 `None`。
pub fn decode_png(bytes: &[u8]) -> Option<ImageData> {
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return None;
    }
    let mut header: Option<Header> = None;
    let mut idat: Vec<u8> = Vec::new();
    let mut palette: Vec<[u8; 3]> = Vec::new();
    let mut alpha: Vec<u8> = Vec::new();
    let mut i = 8usize;
    while i + 8 <= bytes.len() {
        let len = be_u32(bytes, i)? as usize;
        let kind = bytes.get(i + 4..i + 8)?;
        let body = bytes.get(i + 8..i + 8 + len)?;
        match kind {
            b"IHDR" => {
                if body.len() < 13 {
                    return None;
                }
                header = Some(Header {
                    width: be_u32(body, 0)?,
                    height: be_u32(body, 4)?,
                    depth: body[8],
                    color: body[9],
                    interlace: body[12],
                });
            }
            b"PLTE" => {
                for chunk in body.chunks(3).take_while(|chunk| chunk.len() == 3) {
                    palette.push([chunk[0], chunk[1], chunk[2]]);
                }
            }
            b"tRNS" => alpha = body.to_vec(),
            b"IDAT" => idat.extend_from_slice(body),
            b"IEND" => break,
            _ => {}
        }
        i = i.checked_add(12 + len)?;
    }
    let header = header?;
    if header.depth != 8 || header.interlace != 0 {
        return None;
    }
    if header.width == 0 || header.height == 0 {
        return None;
    }
    if header.width > 16384 || header.height > 16384 {
        return None;
    }
    let channels = match header.color {
        0 => 1,
        2 => 3,
        3 => 1,
        4 => 2,
        6 => 4,
        _ => return None,
    };
    let stride = header.width as usize * channels;
    let expected = (stride + 1) * header.height as usize;
    let raw = miniz_oxide::inflate::decompress_to_vec_zlib(&idat).ok()?;
    if raw.len() < expected {
        return None;
    }
    let mut pixels = reconstuct(&raw, header.height as usize, stride, channels);
    let mut rgba = Vec::with_capacity(header.width as usize * header.height as usize * 4);
    let count = header.width as usize * header.height as usize;
    match header.color {
        0 => {
            for &g in pixels.iter().take(count) {
                rgba.extend_from_slice(&[g, g, g, 255]);
            }
        }
        2 => {
            for px in pixels.as_chunks::<3>().0.iter().take(count) {
                rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
        }
        3 => {
            for &entry in pixels.iter().take(count) {
                let entry = entry as usize;
                let rgb = palette.get(entry).copied().unwrap_or([0, 0, 0]);
                let a = alpha.get(entry).copied().unwrap_or(255);
                rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], a]);
            }
        }
        4 => {
            for px in pixels.as_chunks::<2>().0.iter().take(count) {
                rgba.extend_from_slice(&[px[0], px[0], px[0], px[1]]);
            }
        }
        6 => rgba.append(&mut pixels),
        _ => return None,
    }
    Some(ImageData {
        w: header.width,
        h: header.height,
        rgba,
    })
}

/// 去掉每行开头的 filter 字节并做反滤波，返回紧密排列的采样。
/// `bpp` 是像素字节数（filter 的左邻距离）。
fn reconstuct(raw: &[u8], height: usize, stride: usize, bpp: usize) -> Vec<u8> {
    let mut out = alloc::vec![0u8; stride * height];
    for row in 0..height {
        let base = row * stride;
        let src = &raw[base + row..base + row + stride + 1];
        let filter = src[0];
        let line = &src[1..];
        for x in 0..stride {
            let left = if x >= bpp { out[base + x - bpp] } else { 0 };
            let up = if row > 0 { out[base - stride + x] } else { 0 };
            let upper_left = if row > 0 && x >= bpp {
                out[base - stride + x - bpp]
            } else {
                0
            };
            let value = match filter {
                0 => line[x],
                1 => line[x].wrapping_add(left),
                2 => line[x].wrapping_add(up),
                3 => line[x].wrapping_add(((left as u16 + up as u16) / 2) as u8),
                4 => line[x].wrapping_add(paeth(left, up, upper_left)),
                _ => line[x],
            };
            out[base + x] = value;
        }
    }
    out
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i16 + b as i16 - c as i16;
    let pa = (p - a as i16).abs();
    let pb = (p - b as i16).abs();
    let pc = (p - c as i16).abs();
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}
