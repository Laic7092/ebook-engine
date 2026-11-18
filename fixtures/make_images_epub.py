"""Generate fixtures/images.epub: two PNGs, an embedded @font-face, a nested TOC.

Run from the repo root: python3 fixtures/make_images_epub.py
The archive is deterministic (fixed timestamps) so re-running it is a no-op diff.
"""

import struct
import zipfile
import zlib
from pathlib import Path

HERE = Path(__file__).parent

CONTAINER = """<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>
"""

OPF = """<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="bookid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Pictures and Faces</dc:title>
    <dc:language>en</dc:language>
    <dc:identifier id="bookid">urn:uuid:pictures-and-faces</dc:identifier>
  </metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="css" href="style.css" media-type="text/css"/>
    <item id="c1" href="text/ch1.xhtml" media-type="application/xhtml+xml"/>
    <item id="c2" href="text/ch2.xhtml" media-type="application/xhtml+xml"/>
    <item id="wide" href="pics/wide.png" media-type="image/png"/>
    <item id="tall" href="pics/tall.png" media-type="image/png"/>
    <item id="font" href="font/test.ttf" media-type="font/ttf"/>
  </manifest>
  <spine>
    <itemref idref="c1"/>
    <itemref idref="c2"/>
  </spine>
</package>
"""

CSS = """@charset "utf-8";
@font-face { font-family: "Book Serif"; src: url(font/test.ttf) format("truetype"); font-weight: normal; font-style: normal; }
body { font-family: "Book Serif", serif; font-size: 18px; line-height: 1.7; color: #20242a; margin: 0 18px; }
h1 { font-size: 1.7em; margin: 1em 0 1em; }
p { margin: 0 0 0.6em; text-indent: 0; }
figure { margin: 1em 0; }
img { max-width: 100%; }
"""

NAV = """<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<head><title>Contents</title></head>
<body>
<nav epub:type="toc">
  <h1>Contents</h1>
  <ol>
    <li><a href="text/ch1.xhtml">One: The Wide Picture</a>
      <ol>
        <li><a href="text/ch1.xhtml#note">A note in chapter one</a></li>
      </ol>
    </li>
    <li><a href="text/ch2.xhtml#target">Two: The Tall Picture</a></li>
  </ol>
</nav>
</body>
</html>
"""

CH1 = """<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>One</title><link rel="stylesheet" type="text/css" href="../style.css"/></head>
<body>
<h1 id="top">One: The Wide Picture</h1>
<p>Before the picture there is a sentence, and after it there is another one so the line box has something to wrap around.</p>
<figure><img src="../pics/wide.png" alt="a wide picture"/></figure>
<p>An inline picture <img src="../pics/tall.png" alt="a tall picture"/> sits between two words.</p>
<p id="note">This paragraph has an id so the table of contents can jump to it. <a href="ch2.xhtml#target">Go to chapter two</a>.</p>
</body>
</html>
"""

CH2 = """<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>Two</title><link rel="stylesheet" type="text/css" href="../style.css"/></head>
<body>
<h1>Two: The Tall Picture</h1>
<p id="target">The target of the link. The tall picture lives here too, so both image ids exist in two chapters.</p>
<figure><img src="../pics/tall.png" alt="a tall picture"/></figure>
<p>Back to <a href="ch1.xhtml#note">the note</a>.</p>
</body>
</html>
"""


def png(width: int, height: int, pixels) -> bytes:
    raw = b"".join(
        b"\x00" + b"".join(struct.pack("BBB", *pixels(x, y)) for x in range(width))
        for y in range(height)
    )

    def chunk(tag: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data))
            + tag
            + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


def main() -> None:
    out = HERE / "images.epub"
    font = (HERE / "font/test.ttf").read_bytes()
    files = [
        ("META-INF/container.xml", CONTAINER.encode()),
        ("OEBPS/content.opf", OPF.encode()),
        ("OEBPS/nav.xhtml", NAV.encode()),
        ("OEBPS/style.css", CSS.encode()),
        ("OEBPS/text/ch1.xhtml", CH1.encode()),
        ("OEBPS/text/ch2.xhtml", CH2.encode()),
        # 12x6：左半红、右半蓝，方便断言 drawImage/解码方向。
        ("OEBPS/pics/wide.png", png(12, 6, lambda x, y: (220, 40, 40) if x < 6 else (40, 60, 220))),
        # 4x10：上绿下黄。
        ("OEBPS/pics/tall.png", png(4, 10, lambda x, y: (40, 180, 80) if y < 5 else (230, 200, 40))),
        ("OEBPS/font/test.ttf", font),
    ]
    with zipfile.ZipFile(out, "w") as z:
        info = zipfile.ZipInfo("mimetype", (1980, 1, 1, 0, 0, 0))
        z.writestr(info, "application/epub+zip", compress_type=zipfile.ZIP_STORED)
        for name, data in files:
            info = zipfile.ZipInfo(name, (1980, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            z.writestr(info, data)
    print(f"wrote {out} ({out.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
