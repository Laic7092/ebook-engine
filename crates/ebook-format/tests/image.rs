//! 图片头解析：PNG / JPEG / GIF / WebP 与截断输入。

use ebook_format::image::{decode_png, image_mime, image_size};

#[test]
fn png_ihdr_gives_the_size() {
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend_from_slice(&13u32.to_be_bytes());
    png.extend_from_slice(b"IHDR");
    png.extend_from_slice(&320u32.to_be_bytes());
    png.extend_from_slice(&200u32.to_be_bytes());
    png.extend_from_slice(&[8, 2, 0, 0, 0]);
    let size = image_size(&png).expect("png");
    assert_eq!((size.w, size.h), (320.0, 200.0));
    assert_eq!(image_mime(&png), Some("image/png"));
}

#[test]
fn jpeg_sof_gives_the_size() {
    // SOI, APP0（长度 16 的占位段）, SOF0（精度 8, 高 48, 宽 64）。
    let mut jpeg = vec![0xFF, 0xD8];
    jpeg.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x10]);
    jpeg.extend_from_slice(&[0u8; 14]);
    jpeg.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08]);
    jpeg.extend_from_slice(&48u16.to_be_bytes());
    jpeg.extend_from_slice(&64u16.to_be_bytes());
    jpeg.extend_from_slice(&[3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]);
    let size = image_size(&jpeg).expect("jpeg");
    assert_eq!((size.w, size.h), (64.0, 48.0));
    assert_eq!(image_mime(&jpeg), Some("image/jpeg"));
}

#[test]
fn gif_logical_screen_gives_the_size() {
    let mut gif = b"GIF89a".to_vec();
    gif.extend_from_slice(&31u16.to_le_bytes());
    gif.extend_from_slice(&17u16.to_le_bytes());
    gif.extend_from_slice(&[0, 0, 0]);
    let size = image_size(&gif).expect("gif");
    assert_eq!((size.w, size.h), (31.0, 17.0));
    assert_eq!(image_mime(&gif), Some("image/gif"));
}

#[test]
fn webp_vp8x_and_vp8l_give_the_size() {
    let mut vp8x = b"RIFF".to_vec();
    vp8x.extend_from_slice(&0u32.to_le_bytes());
    vp8x.extend_from_slice(b"WEBPVP8X");
    vp8x.extend_from_slice(&10u32.to_le_bytes()); // chunk size
    vp8x.extend_from_slice(&0u32.to_le_bytes()); // flags
    vp8x.extend_from_slice(&[0x3F, 0x00, 0x00]); // 画布宽 - 1 = 63
    vp8x.extend_from_slice(&[0x1F, 0x00, 0x00]); // 画布高 - 1 = 31
    let size = image_size(&vp8x).expect("vp8x");
    assert_eq!((size.w, size.h), (64.0, 32.0));

    let mut vp8l = b"RIFF".to_vec();
    vp8l.extend_from_slice(&0u32.to_le_bytes());
    vp8l.extend_from_slice(b"WEBPVP8L");
    vp8l.extend_from_slice(&5u32.to_le_bytes()); // chunk size
    vp8l.push(0x2F);
    // 宽 - 1 = 9，高 - 1 = 4，打包进 32 位小端。
    let bits: u32 = 9 | (4 << 14);
    vp8l.extend_from_slice(&bits.to_le_bytes());
    let size = image_size(&vp8l).expect("vp8l");
    assert_eq!((size.w, size.h), (10.0, 5.0));
    assert_eq!(image_mime(&vp8l), Some("image/webp"));
}

#[test]
fn truncated_headers_are_none_not_panics() {
    let png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR";
    assert_eq!(image_size(png), None);
    assert_eq!(image_size(&[0xFF, 0xD8, 0xFF]), None);
    assert_eq!(image_size(b"GIF89a\x01"), None);
    assert_eq!(image_size(b"RIFF\x00\x00\x00\x00WEBPVP8 "), None);
    assert_eq!(image_size(b""), None);
    assert_eq!(image_size(b"not an image"), None);
    // PNG 头里宽高为 0 也按无效处理。
    let mut zero = b"\x89PNG\r\n\x1a\n".to_vec();
    zero.extend_from_slice(&13u32.to_be_bytes());
    zero.extend_from_slice(b"IHDR");
    zero.extend_from_slice(&[0; 13]);
    assert_eq!(image_size(&zero), None);
}

#[test]
fn decoder_returns_pixels_for_the_fixture_png() {
    // fixtures/images.epub 里的 wide.png 是 12x6：左半红、右半蓝。
    let raw = fixture_epub();
    let reader = ebook_format::epub::EpubReader::open(&raw).expect("open");
    let (bytes, mime) = reader.asset("OEBPS/pics/wide.png").expect("wide.png");
    assert_eq!(mime, "image/png");
    let image = decode_png(bytes).expect("png decodes");
    assert_eq!((image.w, image.h), (12, 6));
    assert_eq!(&image.rgba[0..4], &[220, 40, 40, 255]);
    let last = image.rgba.len() - 4;
    assert_eq!(&image.rgba[last..], &[40, 60, 220, 255]);
}

#[test]
fn decoder_rejects_non_png_and_truncated_data() {
    assert!(decode_png(b"GIF89a").is_none());
    let raw = fixture_epub();
    let reader = ebook_format::epub::EpubReader::open(&raw).expect("open");
    let (bytes, _) = reader.asset("OEBPS/pics/wide.png").expect("wide.png");
    assert!(decode_png(&bytes[..bytes.len() / 2]).is_none());
}

fn fixture_epub() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/images.epub"
    ))
    .expect("fixtures/images.epub")
}
