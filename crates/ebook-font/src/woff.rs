//! WOFF / WOFF2 容器解包。
//!
//! 字体加载入口统一在这里把 Web 字体还原成普通 sfnt（TTF/OTF）。上层仍然只
//! 看到一份 sfnt 字节：harfrust 整形、ttf-parser 度量、ab_glyph_rasterizer
//! 光栅都不需要知道原始容器是 woff 还是 woff2。
//!
//! 只做容器解码，不改字体内容。调用方给浏览器 `FontFace` 的原始字节因此不受
//! 影响：浏览器原生支持 woff/woff2，没必要把解包后的 sfnt 再传一遍。

use std::error::Error;
use std::fmt;

/// Web 字体容器版本。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WoffKind {
    Woff,
    Woff2,
}

impl WoffKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Woff => "WOFF",
            Self::Woff2 => "WOFF2",
        }
    }

    fn signature(self) -> &'static [u8; 4] {
        match self {
            Self::Woff => b"wOFF",
            Self::Woff2 => b"wOF2",
        }
    }
}

/// 字体加载失败的原因。解析失败不再只是 `None`：调用方可以把它记进诊断，
/// 而不是让一份坏 woff 字体静默消失。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FontLoadError {
    /// WOFF/WOFF2 容器或压缩流损坏，无法还原成 sfnt。
    Woff { kind: WoffKind },
    /// 已经不是 Web 字体容器，但也不是可用的 TTF/OTF。
    InvalidFont,
}

impl fmt::Display for FontLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Woff { kind } => write!(
                f,
                "{} 解包失败（容器损坏、压缩流错误或表越界）",
                kind.name()
            ),
            Self::InvalidFont => write!(f, "不是可解析的 TTF/OTF/WOFF/WOFF2 字体"),
        }
    }
}

impl Error for FontLoadError {}

/// 若是 WOFF/WOFF2 就解包成 sfnt；普通 TTF/OTF 原样返回，不复制。
pub(crate) fn unpack(bytes: Vec<u8>) -> Result<Vec<u8>, FontLoadError> {
    let Some(kind) = detect(&bytes) else {
        return Ok(bytes);
    };
    let result = match kind {
        WoffKind::Woff => wuff::decompress_woff1_with_custom_z(&bytes, &mut decompress_z),
        WoffKind::Woff2 => {
            wuff::decompress_woff2_with_custom_brotli(&bytes, &mut decompress_brotli)
        }
    };
    result.map_err(|_| FontLoadError::Woff { kind })
}

fn detect(bytes: &[u8]) -> Option<WoffKind> {
    let signature = bytes.get(..4)?;
    [WoffKind::Woff, WoffKind::Woff2]
        .into_iter()
        .find(|kind| signature == kind.signature().as_slice())
}

/// WOFF1 的每个表用 zlib 单独压缩。输出上限由 wuff 从 WOFF 表目录算出，
/// 直接作为解压上限；解压后长度与表目录不一致时 wuff 会再校验一次。
fn decompress_z(compressed_data: &[u8], size_hint: usize) -> Result<Vec<u8>, Box<dyn Error>> {
    miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(compressed_data, size_hint).map_err(
        |error| {
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("zlib: {error:?}"),
            )) as Box<dyn Error>
        },
    )
}

/// WOFF2 的整块表数据用 Brotli 压缩。`expected_size` 来自表目录，是输出缓冲
/// 的硬上限；流若需要更多空间，Brotli 会停在 `NeedsMoreOutput`，不会无限扩张。
fn decompress_brotli(
    compressed_data: &[u8],
    expected_size: usize,
) -> Result<Vec<u8>, Box<dyn Error>> {
    use brotli_decompressor::{BrotliDecompressStream, BrotliResult, BrotliState, StandardAlloc};

    let mut output = vec![0u8; expected_size];
    let mut available_in = compressed_data.len();
    let mut input_offset = 0usize;
    let mut available_out = output.len();
    let mut output_offset = 0usize;
    let mut total_out = 0usize;
    let mut state = BrotliState::new(
        StandardAlloc::default(),
        StandardAlloc::default(),
        StandardAlloc::default(),
    );
    let result = BrotliDecompressStream(
        &mut available_in,
        &mut input_offset,
        compressed_data,
        &mut available_out,
        &mut output_offset,
        &mut output,
        &mut total_out,
        &mut state,
    );
    if !matches!(result, BrotliResult::ResultSuccess) || output_offset != expected_size {
        return Err(Box::new(wuff::WuffErr::GenericError));
    }
    Ok(output)
}
