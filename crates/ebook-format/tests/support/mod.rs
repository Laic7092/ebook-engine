//! 测试用的最小 zip / epub 构造器。只写 stored（未压缩）条目，够用且可逐字节控制。
#![allow(dead_code)]

use ebook_format::MemorySource;

fn push_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

/// 把 `(name, bytes)` 列表写成一个 stored zip。
pub fn stored_zip(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, bytes) in entries {
        let offset = out.len() as u32;
        let name_len = name.len() as u16;
        let size = bytes.len() as u32;

        push_u32(&mut out, 0x0403_4b50);
        push_u16(&mut out, 20);
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
        push_u16(&mut central, 20);
        push_u16(&mut central, 20);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u32(&mut central, 0);
        push_u32(&mut central, size);
        push_u32(&mut central, size);
        push_u16(&mut central, name_len);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u32(&mut central, 0);
        push_u32(&mut central, offset);
        central.extend_from_slice(name.as_bytes());
    }

    let central_offset = out.len() as u32;
    out.extend_from_slice(&central);
    push_u32(&mut out, 0x0605_4b50);
    push_u16(&mut out, 0);
    push_u16(&mut out, 0);
    push_u16(&mut out, entries.len() as u16);
    push_u16(&mut out, entries.len() as u16);
    push_u32(&mut out, central.len() as u32);
    push_u32(&mut out, central_offset);
    push_u16(&mut out, 0);
    out
}

pub fn container(opf_path: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="{opf_path}" media-type="application/oebps-package+xml"/></rootfiles>
</container>"#
    )
}

/// 组装一本最小 epub。`extras` 里的路径都是归档路径。
pub fn epub(opf_path: &str, opf: &str, extras: &[(&str, &[u8])]) -> Vec<u8> {
    let mut entries: Vec<(&str, Vec<u8>)> = vec![
        ("mimetype", b"application/epub+zip".to_vec()),
        ("META-INF/container.xml", container(opf_path).into_bytes()),
        (opf_path, opf.as_bytes().to_vec()),
    ];
    for (name, bytes) in extras {
        entries.push((name, bytes.to_vec()));
    }
    stored_zip(&entries)
}

pub fn source(bytes: Vec<u8>) -> MemorySource {
    let mut src = MemorySource::new();
    src.insert("book.epub", bytes);
    src
}

/// 带 EPUB3 nav、链接、媒体查询、@font-face、图片与完整元数据的样书。
pub fn sample_epub3() -> Vec<u8> {
    let opf = r##"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="bookid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Toc Book</dc:title>
    <dc:creator>Ada Lovelace</dc:creator>
    <dc:publisher>Analytical Press</dc:publisher>
    <dc:identifier id="bookid">urn:uuid:toc-book</dc:identifier>
    <dc:language>en</dc:language>
    <dc:date>1843-01-01</dc:date>
    <dc:description>A book about a table of contents.</dc:description>
    <meta name="cover" content="cover-img"/>
  </metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="c1" href="text/ch1.xhtml" media-type="application/xhtml+xml"/>
    <item id="c2" href="text/ch2.xhtml" media-type="application/xhtml+xml"/>
    <item id="css" href="style.css" media-type="text/css"/>
    <item id="img" href="img/pic.png" media-type="image/png"/>
    <item id="cover-img" href="img/cover.png" media-type="image/png"/>
    <item id="font" href="font/book.ttf" media-type="font/ttf"/>
  </manifest>
  <spine>
    <itemref idref="c1"/>
    <itemref idref="c2"/>
  </spine>
</package>"##;

    let nav = r##"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<head><title>Contents</title></head>
<body>
  <nav epub:type="toc"><h1>Contents</h1>
    <ol>
      <li><a href="text/ch1.xhtml">One</a>
        <ol><li><a href="text/ch1.xhtml#sec1">Section 1</a></li></ol>
      </li>
      <li><a href="text/ch2.xhtml#top">Two &amp; a half</a></li>
      <li><span>Group</span>
        <ol><li><a href="text/ch2.xhtml">Sub</a></li></ol>
      </li>
    </ol>
  </nav>
  <nav epub:type="landmarks"><ol><li><a href="text/ch2.xhtml">not the toc</a></li></ol></nav>
</body></html>"##;

    let ch1 = r##"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>One</title><link rel="stylesheet" href="../style.css"/></head>
<body>
  <h1 id="sec1">One</h1>
  <p id="sec1">Duplicate id on purpose.
    <a href="#sec1">self</a>
    <a href="../text/ch2.xhtml#top">next</a>
    <a href="https://example.com/x">outside</a>
    <a href="missing.xhtml">gone</a>
  </p>
  <p class="styled" style="color: #123456 !important; font: italic bold 12px/1.4 serif; border-radius: 3px; page-break-before: always;">Styled shorthand</p>
</body></html>"##;

    let ch2 = r##"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>Two</title></head>
<body><h1 id="top">Two</h1><p>Second chapter.</p></body></html>"##;

    let css = r##"@charset "utf-8";
@font-face {
  font-family: "Book Serif";
  font-weight: bold;
  font-style: italic;
  src: url("font/book.ttf") format("truetype");
}
@media print { body { color: #000000; } }
@media screen { body { color: #111111; } }
body { margin: 0 auto; }
p { box-shadow: 0 0 1px #000; }
.styled { color: #000000; text-decoration: underline; }
"##;

    epub(
        "OEBPS/content.opf",
        opf,
        &[
            ("OEBPS/nav.xhtml", nav.as_bytes()),
            ("OEBPS/text/ch1.xhtml", ch1.as_bytes()),
            ("OEBPS/text/ch2.xhtml", ch2.as_bytes()),
            ("OEBPS/style.css", css.as_bytes()),
            ("OEBPS/img/pic.png", b"\x89PNG pic"),
            ("OEBPS/img/cover.png", b"\x89PNG cover"),
            ("OEBPS/font/book.ttf", b"FONTBYTES"),
        ],
    )
}
