use alloc::string::String;
use alloc::vec::Vec;

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
}

impl<'a> ZipArchive<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self, ZipError> {
        let eocd = find_eocd(data).ok_or(ZipError::BadSignature)?;
        let count = read_u16(data, eocd + 10)? as usize;
        let cd = read_u32(data, eocd + 16)? as usize;
        let mut entries = Vec::with_capacity(count);
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
            let name = core::str::from_utf8(raw_name)
                .map_err(|_| ZipError::Unsupported)?
                .into();
            entries.push(Entry {
                name,
                method,
                compressed,
                offset,
            });
            p += 46 + name_len + extra_len + comment_len;
        }
        Ok(Self { data, entries })
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|e| e.name.as_str())
    }

    pub fn has(&self, name: &str) -> bool {
        find(&self.entries, name).is_some()
    }

    pub fn read(&self, name: &str) -> Option<Vec<u8>> {
        let entry = find(&self.entries, name)?;
        let local = entry.offset as usize;
        if read_u32(self.data, local).ok()? != 0x0403_4b50 {
            return None;
        }
        let name_len = read_u16(self.data, local + 26).ok()? as usize;
        let extra_len = read_u16(self.data, local + 28).ok()? as usize;
        let start = local + 30 + name_len + extra_len;
        let end = start + entry.compressed as usize;
        let raw = self.data.get(start..end)?;
        match entry.method {
            0 => Some(raw.to_vec()),
            8 => miniz_oxide::inflate::decompress_to_vec(raw).ok(),
            _ => None,
        }
    }

    pub fn read_text(&self, name: &str) -> Option<String> {
        self.read(name)
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
    }
}

fn find<'e>(entries: &'e [Entry], name: &str) -> Option<&'e Entry> {
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
