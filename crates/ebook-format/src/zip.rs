use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// 单个条目解压后的上限。压缩炸弹要么在这里被拒绝，要么变成一个可报告的缺失，
/// 而不是把进程内存吃干。
const MAX_ENTRY_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZipError {
    Truncated,
    BadSignature,
    Unsupported,
}

pub struct Entry {
    name: String,
    method: u16,
    compressed: u32,
    offset: u32,
}

pub struct ZipArchive<'a> {
    data: &'a [u8],
    entries: Vec<Entry>,
    /// 名字试图越出包根目录（`..`、绝对路径、NUL）而被拒绝的条目。
    rejected: Vec<String>,
}

impl<'a> ZipArchive<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self, ZipError> {
        let eocd = find_eocd(data).ok_or(ZipError::BadSignature)?;
        let count = read_u16(data, eocd + 10)? as usize;
        let cd = read_u32(data, eocd + 16)? as usize;
        if cd >= data.len() {
            return Err(ZipError::Truncated);
        }
        let mut entries = Vec::with_capacity(count.min(4096));
        let mut rejected = Vec::new();
        let mut p = cd;
        for _ in 0..count {
            if read_u32(data, p)? != 0x0201_4b50 {
                return Err(ZipError::BadSignature);
            }
            let method = read_u16(data, p + 10)?;
            let compressed = read_u32(data, p + 20)?;
            let name_len = read_u16(data, p + 28)? as usize;
            let extra_len = read_u16(data, p + 30)? as usize;
            let comment_len = read_u16(data, p + 32)? as usize;
            let offset = read_u32(data, p + 42)?;
            let raw_name = data
                .get(p + 46..p + 46 + name_len)
                .ok_or(ZipError::Truncated)?;
            let raw_name = core::str::from_utf8(raw_name)
                .map_err(|_| ZipError::Unsupported)?
                .to_string();
            if let Some(name) = normalize(&raw_name) {
                entries.push(Entry {
                    name,
                    method,
                    compressed,
                    offset,
                });
            } else {
                rejected.push(raw_name);
            }
            p = p
                .checked_add(46 + name_len + extra_len + comment_len)
                .ok_or(ZipError::Truncated)?;
            if p > data.len() {
                return Err(ZipError::Truncated);
            }
        }
        Ok(Self {
            data,
            entries,
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
        lookup(&self.entries, name).is_some()
    }

    pub fn read(&self, name: &str) -> Option<Vec<u8>> {
        let name = normalize(name)?;
        let entry = lookup(&self.entries, &name)?;
        if entry.compressed as usize > MAX_ENTRY_BYTES {
            return None;
        }
        let local = entry.offset as usize;
        if read_u32(self.data, local).ok()? != 0x0403_4b50 {
            return None;
        }
        let name_len = read_u16(self.data, local + 26).ok()? as usize;
        let extra_len = read_u16(self.data, local + 28).ok()? as usize;
        let start = local
            .checked_add(30)?
            .checked_add(name_len)?
            .checked_add(extra_len)?;
        let end = start.checked_add(entry.compressed as usize)?;
        let raw = self.data.get(start..end)?;
        match entry.method {
            0 => Some(raw.to_vec()),
            8 => miniz_oxide::inflate::decompress_to_vec_with_limit(raw, MAX_ENTRY_BYTES).ok(),
            _ => None,
        }
    }

    pub fn read_text(&self, name: &str) -> Option<String> {
        self.read(name)
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
    }
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

fn lookup<'e>(entries: &'e [Entry], name: &str) -> Option<&'e Entry> {
    entries
        .iter()
        .find(|e| e.name == name)
        .or_else(|| entries.iter().find(|e| e.name.eq_ignore_ascii_case(name)))
}

fn find_eocd(data: &[u8]) -> Option<usize> {
    if data.len() < 22 {
        return None;
    }
    let lowest = data.len().saturating_sub(66_000);
    let mut i = data.len() - 22;
    loop {
        if read_u32(data, i).ok()? == 0x0605_4b50 {
            return Some(i);
        }
        if i == lowest {
            return None;
        }
        i -= 1;
    }
}

fn read_u16(data: &[u8], at: usize) -> Result<u16, ZipError> {
    let bytes = data.get(at..at + 2).ok_or(ZipError::Truncated)?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_u32(data: &[u8], at: usize) -> Result<u32, ZipError> {
    let bytes = data.get(at..at + 4).ok_or(ZipError::Truncated)?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

#[cfg(test)]
mod tests {
    use super::normalize;

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
}
