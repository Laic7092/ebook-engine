#!/usr/bin/env python3
"""生成手工维护的小型 EPUB 测试夹具：entity / style / typography。

这些书原本是直接提交的二进制 zip。为了让仓库只保留可读源码，这里把
每本书的 XHTML/CSS 内联成字符串，运行时重新打包；zip 时间戳固定，
重复执行不会产生 diff。

运行（仓库根目录）：

    python3 fixtures/make_misc_epubs.py
"""

from __future__ import annotations

import base64
import zipfile
from pathlib import Path

HERE = Path(__file__).parent
STAMP = (1980, 1, 1, 0, 0, 0)

CONTAINER_ENTITY = (
    '<?xml version="1.0"?><container version="1.0" '
    'xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles>'
    '<rootfile full-path="OEBPS/content.opf" '
    'media-type="application/oebps-package+xml"/></rootfiles></container>'
)

ENTITY = {
    "META-INF/container.xml": CONTAINER_ENTITY,
    "OEBPS/content.opf": (
        '<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" '
        'version="3.0" unique-identifier="id"><metadata '
        'xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>entity</dc:title>'
        '<dc:identifier id="id">entity</dc:identifier>'
        "<dc:language>en</dc:language></metadata><manifest>"
        '<item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/>'
        '</manifest><spine><itemref idref="c1"/></spine></package>'
    ),
    "OEBPS/c1.xhtml": (
        '<?xml version="1.0" encoding="utf-8"?>\n'
        '<html xmlns="http://www.w3.org/1999/xhtml"><head><title>t</title>'
        "</head><body><p>a&nbsp;b&mdash;c&hellip;d&rsquo;e&amp;f &#160; "
        "numeric &bogus; unknown</p></body></html>"
    ),
}

CONTAINER_STYLE = """<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>
"""

STYLE = {
    "META-INF/container.xml": CONTAINER_STYLE,
    "OEBPS/content.opf": """<?xml version="1.0" encoding="UTF-8"?>
<package version="3.0" xmlns="http://www.idpf.org/2007/opf" unique-identifier="id" xml:lang="zh">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:identifier id="id">style-test</dc:identifier>
    <dc:title>样式测试</dc:title>
    <dc:language>zh-Hans</dc:language>
  </metadata>
  <manifest>
    <item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine><itemref idref="c1"/></spine>
</package>
""",
    "OEBPS/c1.xhtml": """<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" lang="zh">
<head><title>样式测试</title></head>
<body>
<h1>标题</h1>
<p>普通<strong>加粗</strong><em>斜体</em><a href="#end">链接</a></p>
<p id="end">结尾</p>
</body>
</html>
""",
}

CONTAINER_TYPOGRAPHY = """<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
<rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"""

# 16x12 的纯色小图，只用来断言图片占位与固有尺寸。
PIC_PNG = base64.b64decode(
    "iVBORw0KGgoAAAANSUhEUgAAABAAAAAMCAIAAADkharWAAAAFUlEQVR42mM4URFFEmIY1TCqATsC"
    "AI57M5D/RkrCAAAAAElFTkSuQmCC"
)

TYPOGRAPHY = {
    "META-INF/container.xml": CONTAINER_TYPOGRAPHY,
    "OEBPS/content.opf": """<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id">
<metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Typography</dc:title><dc:identifier id="id">typo</dc:identifier><dc:language>en</dc:language></metadata>
<manifest>
<item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/>
<item id="css" href="style.css" media-type="text/css"/>
<item id="pic" href="img/pic.png" media-type="image/png"/>
</manifest>
<spine><itemref idref="c1"/></spine></package>""",
    "OEBPS/style.css": """
body { margin: 0; }
p { margin: 1em 0; }
.narrow { width: 14em; text-align: justify; hyphens: auto; }
.wide { letter-spacing: 0.08em; }
table { border: 1px solid #888; }
td { border: 1px solid #888; padding: 2px; }
hr { border: 0; border-top: 1px solid #444; }
img { width: 120px; height: 80px; }
blockquote { margin: 1em 2em; }
""",
    "OEBPS/c1.xhtml": """<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><head><title>t</title>
<link rel="stylesheet" type="text/css" href="style.css"/></head><body>
<h2>Typographic conformance</h2>
<p>Regular text, then <b>bold text</b>, then <i>italic text</i>, then <em>emphasis</em> and <strong>strong</strong>.</p>
<p>Reference with a marker<sup>1</sup> and a footnote<sub>2</sub>, plus <u>underline</u>, <s>strikethrough</s>, <small>small</small> and <code>monospace</code>.</p>
<p class="wide">Letter spacing should widen this whole line without breaking the measure.</p>
<p class="narrow">Justified narrow column with hyphenation enabled: the extra space should not open rivers across the measure of this text block at all.</p>
<blockquote><p>Indented quotation, which is a block with its own margins.</p></blockquote>
<ul><li>first bullet</li><li>second bullet</li></ul>
<ol><li>first number</li><li>second number</li></ol>
<hr/>
<table><tr><td>cell a</td><td>cell b</td></tr><tr><td>cell c</td><td>cell d</td></tr></table>
<pre>preformatted
   keeps   spacing</pre>
<p><img src="img/pic.png" alt="picture"/></p>
<p>Line one<br/>line two after a break, and a <a href="#note">link to a note</a>.</p>
</body></html>""",
    "OEBPS/img/pic.png": PIC_PNG,
}


def write_epub(path: Path, files: dict[str, str | bytes]) -> None:
    with zipfile.ZipFile(path, "w") as archive:
        info = zipfile.ZipInfo("mimetype", STAMP)
        archive.writestr(info, "application/epub+zip", compress_type=zipfile.ZIP_STORED)
        for name, data in files.items():
            info = zipfile.ZipInfo(name, STAMP)
            info.compress_type = zipfile.ZIP_DEFLATED
            archive.writestr(info, data)
    print(f"wrote {path.relative_to(HERE)} ({path.stat().st_size} bytes)")


def main() -> None:
    write_epub(HERE / "entity.epub", ENTITY)
    write_epub(HERE / "style.epub", STYLE)
    write_epub(HERE / "typography.epub", TYPOGRAPHY)


if __name__ == "__main__":
    main()
