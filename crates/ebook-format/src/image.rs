//! 图片头解析与最小 PNG 解码。
//!
//! 排版只需要**固有尺寸**，渲染只需要**像素**。格式层因此只做两件事：
//! [`image_size`] 从文件头读出宽高，[`decode_png`] 把 PNG 解成 RGBA。
//! 两者都不引第三方图像库：头解析是纯字节读取，PNG 走已有的 `miniz_oxide`。
//!
//! 已知范围：PNG 只支持 8 位、非隔行、颜色类型 0/2/3/4/6（灰度、RGB、调色板、
//! 灰度+alpha、RGBA）。其它格式只给尺寸，不给像素——渲染后端会画占位。

use alloc::vec::Vec;

use ebook_ir::{ImageData, SizeF};

/// 从文件头读取图片固有尺寸。支持 PNG / JPEG / GIF / WebP。
///
/// 截断或无法识别的输入返回 `None`，不 panic。尺寸为 0 也按无效处理。
pub fn image_size(bytes: &[u8]) -> Option<SizeF> {
    let size = png_size(bytes)
        .or_else(|| jpeg_size(bytes))
        .or_else(|| gif_size(bytes))
        .or_else(|| webp_size(bytes))?;
    if size.w <= 0.0 || size.h <= 0.0 {
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
                for chunk in body.chunks_exact(3) {
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
            for index in 0..count {
                let g = pixels[index];
                rgba.extend_from_slice(&[g, g, g, 255]);
            }
        }
        2 => {
            for index in 0..count {
                let at = index * 3;
                rgba.extend_from_slice(&[pixels[at], pixels[at + 1], pixels[at + 2], 255]);
            }
        }
        3 => {
            for index in 0..count {
                let entry = pixels[index] as usize;
                let rgb = palette.get(entry).copied().unwrap_or([0, 0, 0]);
                let a = alpha.get(entry).copied().unwrap_or(255);
                rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], a]);
            }
        }
        4 => {
            for index in 0..count {
                let at = index * 2;
                let g = pixels[at];
                rgba.extend_from_slice(&[g, g, g, pixels[at + 1]]);
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
