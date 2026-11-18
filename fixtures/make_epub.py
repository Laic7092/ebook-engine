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
    <dc:title>\u96ea\u591c</dc:title>
    <dc:language>zh-Hans</dc:language>
    <dc:identifier id="bookid">urn:uuid:snow-night</dc:identifier>
  </metadata>
  <manifest>
    <item id="css" href="style.css" media-type="text/css"/>
    <item id="c1" href="ch1.xhtml" media-type="application/xhtml+xml"/>
    <item id="c2" href="ch2.xhtml" media-type="application/xhtml+xml"/>
    <item id="png" href="pixel.png" media-type="image/png"/>
  </manifest>
  <spine>
    <itemref idref="c1"/>
    <itemref idref="c2"/>
  </spine>
</package>
"""

CSS = """@charset "utf-8";
body { font-size: 18px; line-height: 1.7; color: #222222; margin: 0 12px; }
h1 { font-size: 2em; text-align: center; margin: 1em 0 1.2em; }
p { text-indent: 2em; margin: 0 0 0.5em; text-align: justify; }
p.lead { text-indent: 0; font-size: 1.1em; color: #555555; }
#note { border: 1px solid #cccccc; padding: 8px; text-indent: 0; }
em { color: #990000; }
"""

CH1 = """<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>\u4e00</title><link rel="stylesheet" type="text/css" href="style.css"/></head>
<body>
<h1>\u7b2c\u4e00\u7ae0 \u96ea\u591c</h1>
<p class="lead">\u90a3\u5e74\u51ac\u5929\u7684\u96ea\u4e0b\u5f97\u6781\u5927\uff0c\u50cf\u662f\u8981\u628a\u6574\u5ea7\u5c71\u57ce\u90fd\u57cb\u8fdb\u4e00\u7247\u5bc2\u9759\u91cc\u3002</p>
<p>\u8857\u4e0a\u7684\u706f\u4e00\u76cf\u4e00\u76cf\u4eae\u8d77\u6765\uff0c\u53c8\u88ab\u98ce\u538b\u5f97\u4f4e\u4f4e\u7684\uff0c\u5149\u6655\u5728\u96ea\u96fe\u91cc\u6563\u6210\u4e00\u56e2\u6a21\u7cca\u7684\u6a59\u8272\u3002</p>
<p>\u6c88\u781a\u80cc\u7740\u4e66\u7bb1\u4ece\u957f\u8857\u5c3d\u5934\u8d70\u6765\uff0c\u9774\u5e95\u9677\u8fdb\u96ea\u91cc\uff0c\u53d1\u51fa\u7ec6\u788e\u7684\u58f0\u54cd\u3002\u4ed6\u5df2\u7ecf\u8d70\u4e86\u6574\u6574\u4e09\u5929\uff0c\u4ece\u5317\u8fb9\u7684\u6e21\u53e3\u4e00\u8def\u5411\u5357\uff0c\u53ea\u4e3a\u4e86\u8d76\u5728\u5e74\u5173\u4e4b\u524d\uff0c\u628a\u8fd9\u7bb1\u4e66\u9001\u5230\u57ce\u897f\u7684\u65e7\u4e66\u9662\u3002</p>
<p>\u4ed6\u7ad9\u5728\u95e8\u524d\uff0c\u72b9\u8c6b\u4e86\u5f88\u4e45\uff0c<em>\u6700\u7ec8\u8fd8\u662f\u62ac\u624b\u6572\u4e86\u4e09\u4e0b</em>\u3002</p>
<p><img src="pixel.png" alt="\u63d2\u56fe"/></p>
<p id="note">\u91cc\u9762\u6ca1\u6709\u4eba\u5e94\u58f0\u3002\u98ce\u4ece\u5c4b\u6a90\u4e0a\u5377\u4e0b\u6765\uff0c\u628a\u5eca\u4e0b\u7684\u706f\u7b3c\u5439\u5f97\u6447\u6447\u6643\u6643\u3002</p>
</body>
</html>
"""

CH2 = """<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>\u4e8c</title><link rel="stylesheet" type="text/css" href="style.css"/></head>
<body>
<h1>\u7b2c\u4e8c\u7ae0 \u65e7\u4e66</h1>
<p>\u95e8\u662f\u81ea\u5df1\u5f00\u7684\u3002</p>
<p>\u6c88\u781a\u540e\u6765\u56de\u60f3\uff0c\u59cb\u7ec8\u8bf4\u4e0d\u6e05\u90a3\u6247\u95e8\u7a76\u7adf\u662f\u600e\u4e48\u5f00\u7684\u2014\u2014\u6ca1\u6709\u811a\u6b65\u58f0\uff0c\u6ca1\u6709\u95e8\u8f74\u7684\u8f7b\u54cd\u3002</p>
<p class="lead">\u9662\u5b50\u6bd4\u5916\u9762\u66f4\u6697\u3002</p>
<p>\u4ed6\u6cbf\u7740\u56de\u5eca\u5f80\u91cc\u8d70\uff0c\u811a\u4e0b\u4e0d\u6562\u7528\u529b\u3002\u8d70\u5230\u7b2c\u4e09\u8fdb\u9662\u5b50\u7684\u65f6\u5019\uff0c\u5ffd\u7136\u542c\u89c1\u4e86\u7ffb\u4e66\u7684\u58f0\u97f3\u3002</p>
</body>
</html>
"""


def png(width: int, height: int) -> bytes:
    raw = b"".join(b"\x00" + b"\xc0\x40\x40" * width for _ in range(height))

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
    out = HERE / "demo.epub"
    files = [
        ("META-INF/container.xml", CONTAINER.encode()),
        ("OEBPS/content.opf", OPF.encode()),
        ("OEBPS/style.css", CSS.encode()),
        ("OEBPS/ch1.xhtml", CH1.encode()),
        ("OEBPS/ch2.xhtml", CH2.encode()),
        ("OEBPS/pixel.png", png(8, 8)),
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
