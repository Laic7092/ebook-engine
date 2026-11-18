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
    <dc:title>The Lantern at the End of Winter</dc:title>
    <dc:language>en</dc:language>
    <dc:identifier id="bookid">urn:uuid:lantern-winter</dc:identifier>
  </metadata>
  <manifest>
    <item id="css" href="style.css" media-type="text/css"/>
    <item id="c1" href="ch1.xhtml" media-type="application/xhtml+xml"/>
    <item id="c2" href="ch2.xhtml" media-type="application/xhtml+xml"/>
    <item id="c3" href="ch3.xhtml" media-type="application/xhtml+xml"/>
    <item id="mark" href="mark.png" media-type="image/png"/>
  </manifest>
  <spine>
    <itemref idref="c1"/>
    <itemref idref="c2"/>
    <itemref idref="c3"/>
  </spine>
</package>
"""

CSS = """@charset "utf-8";
body { font-size: 19px; line-height: 1.62; color: #20242a; margin: 0 18px; }
h1 { font-size: 1.9em; text-align: center; margin: 1.1em 0 1.25em; color: #243447; }
p { text-indent: 1.6em; margin: 0 0 0.62em; text-align: justify; }
p.lead { text-indent: 0; font-size: 1.08em; color: #4d5966; }
p.first { text-indent: 0; }
em { color: #8a3b24; }
blockquote { margin: 1em 2em; padding-left: 1em; border-left: 3px solid #b8c2cc; color: #49515b; }
#note { border: 1px solid #c9d2da; padding: 10px; background-color: #f4f7f9; text-indent: 0; }
figure { margin: 1.4em auto; text-align: center; }
img { width: 96px; height: 96px; }
"""

CH1 = """<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>One</title><link rel="stylesheet" type="text/css" href="style.css"/></head>
<body>
<h1>One: The Frozen Road</h1>
<p class="lead">The winter had been long, and the road north was marked only by the tracks of those who had walked it before and had not come back to say how far it went.</p>
<p>Mara reached the ridge at dusk. Below her the valley lay under a pale blue sheet of snow, and the river moved beneath it like a slow thought. She set down her pack, pulled the collar of her coat close, and listened. There was no wind. There was only the small, dry sound of ice settling in the trees.</p>
<p>She had carried the lantern for eleven days. It was old brass, dented at one side, and the glass was clouded with soot. It had belonged to her father, who had kept the light on the eastern pier for thirty years. When the harbor closed, he brought the lantern home and set it on the shelf above the stove. He never lit it again.</p>
<p>Now it hung from her belt, cold and quiet. She did not know whether it would burn. She only knew that the letter had asked for it by name, and that the letter had come from a place she had thought was empty.</p>
<blockquote>Bring the lantern, and come before the thaw. The door will be open, but the stair is dark.</blockquote>
<p>She had read the words so many times that the paper had softened at the folds. The signature was a single letter, <em>E</em>, written in a hand she almost recognized.</p>
<p>That night she slept in the hollow of a fallen pine. The stars were hard and bright. In the morning she found that someone had passed the same way before dawn: a line of footprints, small and even, leading down toward the valley.</p>
<figure><img src="mark.png" alt="A small brass mark"/></figure>
<p>She followed them until the snow began to fall again, and then the road vanished, and there was only the white country and the sound of her own breathing.</p>
</body>
</html>
"""

CH2 = """<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>Two</title><link rel="stylesheet" type="text/css" href="style.css"/></head>
<body>
<h1>Two: The House Without a Bell</h1>
<p class="first">By noon the house was in front of her, though she had not seen it from the ridge. It stood in a fold of the hills, low and gray, with a roof that carried the snow without complaint. There was no path to the door, no smoke from the chimney, no bell beside the frame.</p>
<p>The footprints ended at the threshold. Mara stood there for a long time, holding the lantern against her chest. The door was painted a dark green, almost black. When she touched it, it swung inward without a sound.</p>
<p>Inside, the air was warmer than it should have been. A narrow hall opened into a room full of books. They were stacked on the floor, on the chairs, on the window ledge, and in the fireplace, where no fire had been lit for years. Dust covered everything in a thin, even coat.</p>
<p>On the table lay a map of the coast, drawn in a careful hand. Someone had marked the eastern pier with a circle and written a date beside it. The date was tomorrow.</p>
<p id="note">Mara set the lantern on the table and opened the little door at its base. The wick was dry, but the oil cup was full. She found a match in her pocket, struck it, and held the flame to the wick until it caught.</p>
<p>The light did not reach very far. It made a small yellow room within the larger dark, and in that room the dust seemed to move. She sat down in the chair by the window and waited for the person who had written the letter.</p>
<p>She waited through the afternoon. She waited while the light outside turned from white to gray, and from gray to a deeper blue. She listened to the house. It did not creak. It did not settle. It held its breath with her.</p>
<p>At last, near midnight, she heard footsteps on the stair. They were small and even, the same steps she had followed through the snow. She did not turn around. She watched the reflection in the dark window and saw a shape come to rest behind her, carrying a lamp that gave no light.</p>
</body>
</html>
"""

CH3 = """<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>Three</title><link rel="stylesheet" type="text/css" href="style.css"/></head>
<body>
<h1>Three: The Thaw</h1>
<p class="lead">In the morning the river had risen. The ice along the banks was gray and hollow, and the water beneath it made a sound like pages turning.</p>
<p>The person on the stair was an old woman with a narrow face and eyes the color of the sea in winter. She did not introduce herself. She looked at the lantern, and then at Mara, and then at the map on the table, and something in her expression loosened.</p>
<p>They spoke until the sun came up. The woman told Mara about the pier, about the harbor, and about the night the light had gone out. She told her about the letter, which had waited for eleven years to be sent, and about the house, which had waited longer than that.</p>
<p>Mara listened. She did not ask the questions she had carried north with her. The answers, when they came, were smaller than she had expected and heavier than she had feared.</p>
<blockquote>A light is not kept for the people who can see it. It is kept for the ones who are still looking.</blockquote>
<p>When the thaw reached the valley, they walked down together. The snow pulled away from the road in long, wet strips. Birds returned to the trees. The river carried the broken ice toward the sea, and the sea received it without comment.</p>
<p>At the eastern pier, Mara set the lantern on the same stone her father had used. It was still warm. She opened the little door, filled the cup, and lit the wick. The flame stood up straight in the morning air.</p>
<p>Far out on the water, a boat turned toward the harbor. It had been gone a long time. Its sail was patched and pale, and the person at the tiller was waving, though no one on the shore could yet see who it was.</p>
<p>Mara stayed until the boat came in. Then she went home, and the road behind her was no longer only the tracks of people who did not come back.</p>
</body>
</html>
"""


def png(width: int, height: int) -> bytes:
    raw = b"".join(b"\x00" + b"\x8a\x5a\x32" * width for _ in range(height))

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
    out = HERE / "en.epub"
    files = [
        ("META-INF/container.xml", CONTAINER.encode()),
        ("OEBPS/content.opf", OPF.encode()),
        ("OEBPS/style.css", CSS.encode()),
        ("OEBPS/ch1.xhtml", CH1.encode()),
        ("OEBPS/ch2.xhtml", CH2.encode()),
        ("OEBPS/ch3.xhtml", CH3.encode()),
        ("OEBPS/mark.png", png(24, 24)),
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
