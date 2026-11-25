use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// 单个条目解压后的上限。压缩炸弹要么在这里被拒绝，要么变成一个可报告的缺失，
/// 而不是把进程内存吃干。
const MAX_ENTRY_BYTES: usize = 64 * 1024 * 1024;

/// 中央目录条目上限。正常 EPUB 远小于这个值；上限只用来挡住病态归档。
const MAX_ENTRIES: usize = 1_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZipError {
    Truncated,
    BadSignature,
    Unsupported,
}

pub struct Entry {
    name: String,
    method: u16,
    compressed: u64,
    /// 中央目录里的解压后大小。高度估计、预算与“值不值得读”都只看它，
    /// 不需要先把整个条目解压出来。
    uncompressed: u64,
    offset: u64,
}

pub struct ZipArchive<'a> {
    data: &'a [u8],
    entries: Vec<Entry>,
    /// 精确名字 → 条目下标。避免每读一个资产都线性扫一遍中央目录。
    exact: BTreeMap<String, usize>,
    /// ASCII 小写名字 → 条目下标，保持旧的“大小写不敏感兜底”语义。
    folded: BTreeMap<String, usize>,
    /// 名字试图越出包根目录（`..`、绝对路径、NUL）而被拒绝的条目。
    rejected: Vec<String>,
}

impl<'a> ZipArchive<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self, ZipError> {
        let eocd = find_eocd(data).ok_or(ZipError::BadSignature)?;
        let directory = read_directory_info(data, eocd)?;
        let count = usize::try_from(directory.count).unwrap_or(usize::MAX);
        let cd = usize::try_from(directory.offset).map_err(|_| ZipError::Unsupported)?;
        let cd_size = usize::try_from(directory.size).map_err(|_| ZipError::Unsupported)?;
        let cd_end = cd.checked_add(cd_size).ok_or(ZipError::Truncated)?;
        if cd_size > 0 && cd_end > data.len() {
            return Err(ZipError::Truncated);
        }
        if cd_size == 0 && count == 0 {
            return Ok(Self {
                data,
                entries: Vec::new(),
                exact: BTreeMap::new(),
                folded: BTreeMap::new(),
                rejected: Vec::new(),
            });
        }
        if cd >= data.len() {
            return Err(ZipError::Truncated);
        }
        let mut entries = Vec::with_capacity(count.min(4096));
        let mut rejected = Vec::new();
        let mut p = cd;
        loop {
            // 有中央目录大小时以大小为准；少数流的 EOCD 条目数是 0，但目录本身
            // 完整。只有大小缺失时才退回条目数，避免整本书被一个坏字段拖垮。
            if cd_size > 0 {
                if p >= cd_end {
                    break;
                }
            } else if entries.len() + rejected.len() >= count {
                break;
            }
            if entries.len() + rejected.len() >= MAX_ENTRIES {
                return Err(ZipError::Unsupported);
            }
            if read_u32(data, p)? != 0x0201_4b50 {
                return Err(ZipError::BadSignature);
            }
            let method = read_u16(data, p + 10)?;
            let compressed32 = read_u32(data, p + 20)?;
            let uncompressed32 = read_u32(data, p + 24)?;
            let name_len = read_u16(data, p + 28)? as usize;
            let extra_len = read_u16(data, p + 30)? as usize;
            let comment_len = read_u16(data, p + 32)? as usize;
            let offset32 = read_u32(data, p + 42)?;
            let name_start = p.checked_add(46).ok_or(ZipError::Truncated)?;
            let name_end = name_start
                .checked_add(name_len)
                .ok_or(ZipError::Truncated)?;
            let extra_end = name_end.checked_add(extra_len).ok_or(ZipError::Truncated)?;
            let raw_name = data.get(name_start..name_end).ok_or(ZipError::Truncated)?;
            let extra = data.get(name_end..extra_end).ok_or(ZipError::Truncated)?;
            let (compressed, uncompressed, offset) =
                read_entry_sizes(compressed32, uncompressed32, offset32, extra)?;

            // 单个非 UTF-8 文件名不应该让整本书打不开。能用的条目继续保留，
            // 不能表示成 `&str` 的条目静默跳过（读取时会表现为缺失资产）。
            if let Ok(raw_name) = core::str::from_utf8(raw_name) {
                if let Some(name) = normalize(raw_name) {
                    entries.push(Entry {
                        name,
                        method,
                        compressed,
                        uncompressed,
                        offset,
                    });
                } else {
                    rejected.push(raw_name.to_string());
                }
            }
            p = extra_end
                .checked_add(comment_len)
                .ok_or(ZipError::Truncated)?;
            if p > data.len() {
                return Err(ZipError::Truncated);
            }
        }

        let mut exact = BTreeMap::new();
        let mut folded = BTreeMap::new();
        for (index, entry) in entries.iter().enumerate() {
            exact.entry(entry.name.clone()).or_insert(index);
            folded
                .entry(entry.name.to_ascii_lowercase())
                .or_insert(index);
        }
        Ok(Self {
            data,
            entries,
            exact,
            folded,
            rejected,
        })
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|e| e.name.as_str())
    }

    /// 被拒绝的路径（zip path traversal 防护的痕迹）。
    pub fn rejected(&self) -> &[String] {
        &self.rejected
    }

    pub fn has(&self, name: &str) -> bool {
        lookup(&self.entries, &self.exact, &self.folded, name).is_some()
    }

    /// 条目的解压后大小；只看中央目录，不触碰压缩数据。
    pub fn uncompressed_size(&self, name: &str) -> Option<u64> {
        let name = normalize(name)?;
        lookup(&self.entries, &self.exact, &self.folded, &name).map(|entry| entry.uncompressed)
    }

    pub fn read(&self, name: &str) -> Option<Vec<u8>> {
        let raw = self.raw_entry(name)?;
        let entry = self.entry(name)?;
        match entry.method {
            0 => Some(raw.to_vec()),
            8 => miniz_oxide::inflate::decompress_to_vec_with_limit(raw, MAX_ENTRY_BYTES).ok(),
            _ => None,
        }
    }

    /// 只解压前 `max` 字节。图片的固有尺寸/MIME 只在前几十 KB 里，没必要为了
    /// 一个 `image_size()` 把整张图（可能是 20MB 的 JPEG）解压出来。
    pub fn read_prefix(&self, name: &str, max: usize) -> Option<Vec<u8>> {
        if max == 0 {
            return None;
        }
        let raw = self.raw_entry(name)?;
        let entry = self.entry(name)?;
        match entry.method {
            0 => Some(raw[..raw.len().min(max)].to_vec()),
            8 => {
                use miniz_oxide::inflate::stream::{inflate, InflateState};
                use miniz_oxide::{DataFormat, MZFlush, MZStatus};
                let mut state = InflateState::new_boxed(DataFormat::Raw);
                let mut out = alloc::vec![0u8; max];
                let result = inflate(&mut state, raw, &mut out, MZFlush::Finish);
                match result.status {
                    Ok(MZStatus::Ok | MZStatus::StreamEnd) if result.bytes_written > 0 => {
                        out.truncate(result.bytes_written);
                        Some(out)
                    }
                    // 输出缓冲刚好被填满时 miniz 会报 Buf（还没看到流结束），
                    // 但已经写出的前缀正是调用方要的。
                    Err(miniz_oxide::MZError::Buf) if result.bytes_written > 0 => {
                        out.truncate(result.bytes_written);
                        Some(out)
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn entry(&self, name: &str) -> Option<&Entry> {
        let name = normalize(name)?;
        lookup(&self.entries, &self.exact, &self.folded, &name)
    }

    fn raw_entry(&self, name: &str) -> Option<&[u8]> {
        let entry = self.entry(name)?;
        let compressed = usize::try_from(entry.compressed).ok()?;
        if compressed > MAX_ENTRY_BYTES {
            return None;
        }
        let local = usize::try_from(entry.offset).ok()?;
        if read_u32(self.data, local).ok()? != 0x0403_4b50 {
            return None;
        }
        let name_len = read_u16(self.data, local + 26).ok()? as usize;
        let extra_len = read_u16(self.data, local + 28).ok()? as usize;
        let start = local
            .checked_add(30)?
            .checked_add(name_len)?
            .checked_add(extra_len)?;
        let end = start.checked_add(compressed)?;
        self.data.get(start..end)
    }

    pub fn read_text(&self, name: &str) -> Option<String> {
        self.read(name).map(|bytes| decode_text(&bytes))
    }
}

/// 文本条目的解码。
///
/// EPUB 规范要求 UTF-8，但真实书里常见带 BOM 的 UTF-8、UTF-16，以及
/// GBK/Big5/Shift_JIS 等 CJK 旧编码。决策顺序与浏览器一致：
///
/// 1. BOM；
/// 2. 无 BOM 的 UTF-16 字节特征；
/// 3. 合法 UTF-8；
/// 4. XML 声明 / HTML `<meta charset>` 里的编码标签；
/// 5. `chardetng`（Firefox 的编码探测器）猜测（仅 native）；
/// 6. 最后才是 UTF-8 lossy。
///
/// 任何一步都不失败；最坏情况是替换字符，而不是整章/整本打不开。
pub fn decode_text(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return decode_utf16(rest, true);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return decode_utf16(rest, false);
    }
    // 无 BOM 的 UTF-16 必须放在 UTF-8 之前：NUL 是合法 UTF-8，直接
    // `from_utf8` 会把 UTF-16 当成带 NUL 的 UTF-8。
    if is_utf16(bytes, true) {
        return decode_utf16(bytes, true);
    }
    if is_utf16(bytes, false) {
        return decode_utf16(bytes, false);
    }
    if let Ok(text) = core::str::from_utf8(bytes) {
        return text.to_string();
    }
    decode_legacy(bytes)
}

#[cfg(not(target_arch = "wasm32"))]
fn decode_legacy(bytes: &[u8]) -> String {
    if let Some(label) = declared_label(bytes) {
        if let Some(encoding) = encoding_rs::Encoding::for_label(&label) {
            return encoding.decode_without_bom_handling(bytes).0.into_owned();
        }
    }
    let mut detector = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Deny);
    detector.feed(bytes, true);
    let encoding = detector.guess(None, chardetng::Utf8Detection::Deny);
    encoding.decode_without_bom_handling(bytes).0.into_owned()
}

#[cfg(target_arch = "wasm32")]
fn decode_legacy(bytes: &[u8]) -> String {
    // wasm 不打包完整编码表（体积预算）。宿主可以用 `TextDecoder` 先解码。
    match declared_label(bytes).as_deref() {
        Some(b"iso-8859-1" | b"latin1" | b"latin-1") => decode_latin1(bytes),
        Some(b"windows-1252" | b"cp1252") => decode_windows_1252(bytes),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

fn is_utf16(bytes: &[u8], little_endian: bool) -> bool {
    if bytes.len() < 4 || !bytes.len().is_multiple_of(2) {
        return false;
    }
    let sample = bytes.len().min(256);
    let mut zeros_expected = 0usize;
    let mut pairs = 0usize;
    let mut i = 0usize;
    while i < sample {
        let (first, second) = (bytes[i], bytes[i + 1]);
        let zero_at_expected = if little_endian {
            second == 0
        } else {
            first == 0
        };
        if zero_at_expected {
            zeros_expected += 1;
        }
        pairs += 1;
        i += 2;
    }
    // UTF-16 的 ASCII 文本至少有一半的码元高字节为 0；阈值留足非 ASCII 空间。
    pairs >= 2 && zeros_expected * 4 >= pairs * 3
}

fn decode_utf16(bytes: &[u8], little_endian: bool) -> String {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            if little_endian {
                u16::from_le_bytes([pair[0], pair[1]])
            } else {
                u16::from_be_bytes([pair[0], pair[1]])
            }
        })
        .collect();
    let mut out = String::new();
    for decoded in char::decode_utf16(units) {
        out.push(decoded.unwrap_or('\u{FFFD}'));
    }
    out
}

/// XML 声明或 `<meta charset>` 里声明的编码标签。标签本身是 ASCII，所以可以
/// 在原始字节上扫描，不受正文编码影响。
fn declared_label(bytes: &[u8]) -> Option<Vec<u8>> {
    let head = &bytes[..bytes.len().min(1024)];
    xml_encoding_label(head).or_else(|| meta_charset_label(head))
}

fn find_ascii(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    hay.windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle))
}

fn xml_encoding_label(head: &[u8]) -> Option<Vec<u8>> {
    let start = head.iter().position(|byte| !byte.is_ascii_whitespace())?;
    if !head[start..].starts_with(b"<?xml") {
        return None;
    }
    let end = find_ascii(&head[start..], b"?>")? + start;
    let decl = &head[start..end];
    let at = find_ascii(decl, b"encoding")?;
    let mut i = at + b"encoding".len();
    while i < decl.len() && decl[i].is_ascii_whitespace() {
        i += 1;
    }
    if decl.get(i) != Some(&b'=') {
        return None;
    }
    i += 1;
    while i < decl.len() && decl[i].is_ascii_whitespace() {
        i += 1;
    }
    let quote = *decl.get(i)?;
    if quote == b'"' || quote == b'\'' {
        i += 1;
        let label_start = i;
        while i < decl.len() && decl[i] != quote {
            i += 1;
        }
        (i > label_start).then(|| decl[label_start..i].to_ascii_lowercase())
    } else {
        let label_start = i;
        while i < decl.len() && !decl[i].is_ascii_whitespace() && !matches!(decl[i], b'>' | b'?') {
            i += 1;
        }
        (i > label_start).then(|| decl[label_start..i].to_ascii_lowercase())
    }
}

fn meta_charset_label(head: &[u8]) -> Option<Vec<u8>> {
    // 只认 `<meta` 之后第一次出现的 charset，避免正文里的单词误命中。
    let meta = find_ascii(head, b"<meta")?;
    let at = find_ascii(&head[meta..], b"charset")? + meta;
    let mut i = at + b"charset".len();
    while i < head.len() && head[i].is_ascii_whitespace() {
        i += 1;
    }
    if head.get(i) != Some(&b'=') {
        return None;
    }
    i += 1;
    while i < head.len() && head[i].is_ascii_whitespace() {
        i += 1;
    }
    let quote = *head.get(i)?;
    let label_start;
    if quote == b'"' || quote == b'\'' {
        i += 1;
        label_start = i;
        while i < head.len() && head[i] != quote {
            i += 1;
        }
    } else {
        label_start = i;
        while i < head.len()
            && !head[i].is_ascii_whitespace()
            && !matches!(head[i], b';' | b'"' | b'\'' | b'>')
        {
            i += 1;
        }
    }
    (i > label_start).then(|| head[label_start..i].to_ascii_lowercase())
}

#[cfg(target_arch = "wasm32")]
fn decode_latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&byte| byte as char).collect()
}

#[cfg(target_arch = "wasm32")]
fn decode_windows_1252(bytes: &[u8]) -> String {
    const HIGH: [char; 32] = [
        '\u{20AC}', '\u{FFFD}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}',
        '\u{2021}', '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{FFFD}',
        '\u{017D}', '\u{FFFD}', '\u{FFFD}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}',
        '\u{2022}', '\u{2013}', '\u{2014}', '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}',
        '\u{0153}', '\u{FFFD}', '\u{017E}', '\u{0178}',
    ];
    bytes
        .iter()
        .map(|&byte| match byte {
            0x80..=0x9F => HIGH[(byte - 0x80) as usize],
            _ => byte as char,
        })
        .collect()
}

/// 规范化归档内路径：反斜杠当分隔符，去掉 `.`，`..` 只能退到包根为止，
/// 越过根目录、绝对路径和含 NUL 的名字一律拒绝。
fn normalize(name: &str) -> Option<String> {
    if name.contains('\0') {
        return None;
    }
    let mut parts: Vec<&str> = Vec::new();
    for segment in name.split(['/', '\\']) {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

fn lookup<'e>(
    entries: &'e [Entry],
    exact: &BTreeMap<String, usize>,
    folded: &BTreeMap<String, usize>,
    name: &str,
) -> Option<&'e Entry> {
    if let Some(&index) = exact.get(name) {
        return entries.get(index);
    }
    let lower = name.to_ascii_lowercase();
    folded.get(&lower).and_then(|&index| entries.get(index))
}

struct DirectoryInfo {
    count: u64,
    offset: u64,
    size: u64,
}

fn read_directory_info(data: &[u8], eocd: usize) -> Result<DirectoryInfo, ZipError> {
    let count16 = read_u16(data, eocd + 10)? as u64;
    let size32 = read_u32(data, eocd + 12)?;
    let offset32 = read_u32(data, eocd + 16)?;
    if count16 != u16::MAX as u64 && size32 != u32::MAX && offset32 != u32::MAX {
        return Ok(DirectoryInfo {
            count: count16,
            offset: offset32 as u64,
            size: size32 as u64,
        });
    }

    // ZIP64：EOCD 前面紧邻 locator，locator 指向 ZIP64 EOCD。
    if eocd < 20 {
        return Err(ZipError::Truncated);
    }
    let locator = eocd - 20;
    if read_u32(data, locator)? != 0x0706_4b50 {
        return Err(ZipError::Unsupported);
    }
    let zip64 = usize::try_from(read_u64(data, locator + 8)?).map_err(|_| ZipError::Unsupported)?;
    if read_u32(data, zip64)? != 0x0606_4b50 {
        return Err(ZipError::BadSignature);
    }
    // ZIP64 EOCD 固定头到 offset 字段末尾共 56 字节。
    if zip64.checked_add(56).is_none_or(|end| end > data.len()) {
        return Err(ZipError::Truncated);
    }
    Ok(DirectoryInfo {
        count: read_u64(data, zip64 + 32)?,
        offset: read_u64(data, zip64 + 48)?,
        size: read_u64(data, zip64 + 40)?,
    })
}

/// 中央目录条目里的 size/offset 可能是 ZIP64 扩展字段里的 64 位值。
fn read_entry_sizes(
    compressed32: u32,
    uncompressed32: u32,
    offset32: u32,
    extra: &[u8],
) -> Result<(u64, u64, u64), ZipError> {
    if compressed32 != u32::MAX && uncompressed32 != u32::MAX && offset32 != u32::MAX {
        return Ok((compressed32 as u64, uncompressed32 as u64, offset32 as u64));
    }
    let mut p = 0usize;
    while p + 4 <= extra.len() {
        let id = u16::from_le_bytes([extra[p], extra[p + 1]]);
        let size = u16::from_le_bytes([extra[p + 2], extra[p + 3]]) as usize;
        let body_start = p + 4;
        let body_end = body_start.checked_add(size).ok_or(ZipError::Truncated)?;
        let body = extra.get(body_start..body_end).ok_or(ZipError::Truncated)?;
        if id == 0x0001 {
            let mut q = 0usize;
            let mut compressed = compressed32 as u64;
            let mut uncompressed = uncompressed32 as u64;
            let mut offset = offset32 as u64;
            // ZIP64 扩展字段的顺序固定为 uncompressed, compressed, offset，
            // 只包含“对应 32 位字段为 0xFFFF_FFFF 的那些项”。
            if uncompressed32 == u32::MAX {
                uncompressed = read_u64_slice(body, q)?;
                q += 8;
            }
            if compressed32 == u32::MAX {
                compressed = read_u64_slice(body, q)?;
                q += 8;
            }
            if offset32 == u32::MAX {
                offset = read_u64_slice(body, q)?;
            }
            return Ok((compressed, uncompressed, offset));
        }
        p = body_end;
    }
    Err(ZipError::Unsupported)
}

fn find_eocd(data: &[u8]) -> Option<usize> {
    if data.len() < 22 {
        return None;
    }
    let lowest = data.len().saturating_sub(66_000);
    let mut i = data.len() - 22;
    loop {
        if read_u32(data, i).ok() == Some(0x0605_4b50) && plausible_eocd(data, i) {
            return Some(i);
        }
        if i == lowest {
            return None;
        }
        i -= 1;
    }
}

/// 防止把 ZIP 注释里的假 EOCD 当成目录起点：注释长度必须落在文件内，
/// 且非空中央目录的偏移必须指向 EOCD 之前。
fn plausible_eocd(data: &[u8], at: usize) -> bool {
    let Some(comment_len) = read_u16(data, at + 20).ok() else {
        return false;
    };
    if at
        .checked_add(22)
        .and_then(|v| v.checked_add(comment_len as usize))
        .is_none_or(|end| end > data.len())
    {
        return false;
    }
    let Some(count) = read_u16(data, at + 10).ok() else {
        return false;
    };
    if count == 0 || count == u16::MAX {
        return true;
    }
    let Some(offset) = read_u32(data, at + 16).ok() else {
        return false;
    };
    offset != u32::MAX && (offset as usize) < at
}

fn read_u16(data: &[u8], at: usize) -> Result<u16, ZipError> {
    let end = at.checked_add(2).ok_or(ZipError::Truncated)?;
    let bytes = data.get(at..end).ok_or(ZipError::Truncated)?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_u32(data: &[u8], at: usize) -> Result<u32, ZipError> {
    let end = at.checked_add(4).ok_or(ZipError::Truncated)?;
    let bytes = data.get(at..end).ok_or(ZipError::Truncated)?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn read_u64(data: &[u8], at: usize) -> Result<u64, ZipError> {
    let end = at.checked_add(8).ok_or(ZipError::Truncated)?;
    let bytes = data.get(at..end).ok_or(ZipError::Truncated)?;
    Ok(u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]))
}

fn read_u64_slice(data: &[u8], at: usize) -> Result<u64, ZipError> {
    let end = at.checked_add(8).ok_or(ZipError::Truncated)?;
    let bytes = data.get(at..end).ok_or(ZipError::Truncated)?;
    Ok(u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]))
}

#[cfg(test)]
mod tests {
    use super::{decode_text, normalize, ZipArchive};

    #[test]
    fn traversal_is_rejected() {
        assert_eq!(
            normalize("OEBPS/ch1.xhtml").as_deref(),
            Some("OEBPS/ch1.xhtml")
        );
        assert_eq!(
            normalize("./OEBPS/./ch1.xhtml").as_deref(),
            Some("OEBPS/ch1.xhtml")
        );
        assert_eq!(
            normalize("OEBPS/../ch1.xhtml").as_deref(),
            Some("ch1.xhtml")
        );
        assert_eq!(
            normalize("/OEBPS/ch1.xhtml").as_deref(),
            Some("OEBPS/ch1.xhtml")
        );
        assert_eq!(normalize("../evil"), None);
        assert_eq!(normalize(".."), None);
        assert_eq!(normalize(""), None);
        assert_eq!(normalize("a\0b"), None);
    }

    #[test]
    fn text_decoding_handles_boms_and_legacy_encodings() {
        assert_eq!(decode_text(b"\xEF\xBB\xBFhello"), "hello");
        assert_eq!(decode_text(&[0xFF, 0xFE, b'h', 0, b'i', 0]), "hi");
        assert_eq!(decode_text(&[0xFE, 0xFF, 0, b'h', 0, b'i']), "hi");
        assert_eq!(
            decode_text(b"<?xml version=\"1.0\" encoding=\"iso-8859-1\"?><p>caf\xE9</p>"),
            "<?xml version=\"1.0\" encoding=\"iso-8859-1\"?><p>caf\u{e9}</p>"
        );
        assert_eq!(
            decode_text(b"<?xml version=\"1.0\" encoding=\"windows-1252\"?>\x93hi\x94"),
            "<?xml version=\"1.0\" encoding=\"windows-1252\"?>\u{201c}hi\u{201d}"
        );
    }

    #[test]
    fn zip64_eocd_and_extra_fields_are_accepted() {
        let archive = zip64_stored(&[("a.txt", b"hello")]);
        let zip = ZipArchive::parse(&archive).expect("zip64 parses");
        assert!(zip.has("a.txt"));
        assert_eq!(zip.read_text("a.txt").as_deref(), Some("hello"));
    }

    fn push_u16(out: &mut Vec<u8>, value: u16) {
        out.extend_from_slice(&value.to_le_bytes());
    }

    fn push_u32(out: &mut Vec<u8>, value: u32) {
        out.extend_from_slice(&value.to_le_bytes());
    }

    fn push_u64(out: &mut Vec<u8>, value: u64) {
        out.extend_from_slice(&value.to_le_bytes());
    }

    fn zip64_stored(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, bytes) in entries {
            let offset = out.len() as u64;
            let name_len = name.len() as u16;
            let size = bytes.len() as u32;
            push_u32(&mut out, 0x0403_4b50);
            push_u16(&mut out, 45);
            push_u16(&mut out, 0);
            push_u16(&mut out, 0);
            push_u16(&mut out, 0);
            push_u16(&mut out, 0);
            push_u32(&mut out, 0);
            push_u32(&mut out, size);
            push_u32(&mut out, size);
            push_u16(&mut out, name_len);
            push_u16(&mut out, 0);
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(bytes);

            push_u32(&mut central, 0x0201_4b50);
            push_u16(&mut central, 45);
            push_u16(&mut central, 45);
            push_u16(&mut central, 0);
            push_u16(&mut central, 0);
            push_u16(&mut central, 0);
            push_u16(&mut central, 0);
            push_u32(&mut central, 0);
            push_u32(&mut central, u32::MAX);
            push_u32(&mut central, u32::MAX);
            push_u16(&mut central, name_len);
            push_u16(&mut central, 28);
            push_u16(&mut central, 0);
            push_u16(&mut central, 0);
            push_u16(&mut central, 0);
            push_u32(&mut central, 0);
            push_u32(&mut central, u32::MAX);
            central.extend_from_slice(name.as_bytes());
            push_u16(&mut central, 0x0001);
            push_u16(&mut central, 24);
            push_u64(&mut central, size as u64);
            push_u64(&mut central, size as u64);
            push_u64(&mut central, offset);
        }
        let central_offset = out.len() as u64;
        let central_size = central.len() as u64;
        out.extend_from_slice(&central);
        let zip64_eocd = out.len() as u64;
        push_u32(&mut out, 0x0606_4b50);
        push_u64(&mut out, 44);
        push_u16(&mut out, 45);
        push_u16(&mut out, 45);
        push_u32(&mut out, 0);
        push_u32(&mut out, 0);
        push_u64(&mut out, entries.len() as u64);
        push_u64(&mut out, entries.len() as u64);
        push_u64(&mut out, central_size);
        push_u64(&mut out, central_offset);
        let locator = out.len();
        push_u32(&mut out, 0x0706_4b50);
        push_u32(&mut out, 0);
        push_u64(&mut out, zip64_eocd);
        push_u32(&mut out, 1);
        assert_eq!(out.len(), locator + 20);
        push_u32(&mut out, 0x0605_4b50);
        push_u16(&mut out, 0);
        push_u16(&mut out, 0);
        push_u16(&mut out, u16::MAX);
        push_u16(&mut out, u16::MAX);
        push_u32(&mut out, u32::MAX);
        push_u32(&mut out, u32::MAX);
        push_u16(&mut out, 0);
        out
    }
}
