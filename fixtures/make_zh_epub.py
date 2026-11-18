import zipfile

CHAPTERS = [
    ("chapter one", """第一章 雪夜

冬天的路很长，只有走近的人才看得见脚印。她沿着山脊往上走，天色一点一点暗下来。

山谷里铺着一层薄雪，河在雪下面慢慢地流。她把背包放下，拉起衣领，静静地听着。风很小，只有冰在树上碎裂的声音。

那盏灯她已经带了十一天。灯是旧的，黄铜，一边凹下去，玻璃上蒙着烟灰。它原本属于她父亲，他守着东边的码头守了三十年。码头关了以后，他把灯带回家，放在炉子上方的架子上，再也没有点过。

现在灯挂在她腰上，冷的，安静。她不知道它还能不能亮起来。她只知道那封信里写了它的名字。

“把灯带来，在开春之前。门会开着，但楼梯是黑的。”

她把这句话读了太多遍，纸在折痕处已经软了。落款只有一个字母。"""),
    ("chapter two", """第二章 楼梯

楼梯是黑的，这一点信上没有说谎。她扶着墙往上走，手指碰到的地方都是冷的石头。

上面有一扇门，门缝里透出一点光。她把灯举起来，灯是灭的，黑暗里只有它黄铜的轮廓。

她数了一段台阶，停下来听。有人在楼上走，很慢，像是怕吵醒什么。"""),
]

OPF = """<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id">
<metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
<dc:title>雪夜</dc:title><dc:identifier id="id">zh-sample</dc:identifier><dc:language>zh</dc:language>
</metadata>
<manifest>
{}
</manifest>
<spine>
{}
</spine>
</package>"""

with zipfile.ZipFile("fixtures/zh.epub", "w", zipfile.ZIP_DEFLATED) as archive:
    archive.writestr("mimetype", "application/epub+zip", zipfile.ZIP_STORED)
    archive.writestr(
        "META-INF/container.xml",
        """<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
<rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>""",
    )
    items = []
    spine = []
    for index, (_, body) in enumerate(CHAPTERS):
        name = f"c{index + 1}.xhtml"
        paragraphs = "".join(f"<p>{line}</p>" for line in body.split("\n\n"))
        archive.writestr(
            f"OEBPS/{name}",
            f"""<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><head><title>雪夜</title></head><body>{paragraphs}</body></html>""",
        )
        items.append(
            f'<item id="c{index + 1}" href="{name}" media-type="application/xhtml+xml"/>'
        )
        spine.append(f'<itemref idref="c{index + 1}"/>')
    archive.writestr("OEBPS/content.opf", OPF.format("\n".join(items), "\n".join(spine)))
print("fixtures/zh.epub")
