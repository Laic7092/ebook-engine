#!/usr/bin/env python3
"""生成 iOS 主屏启动图（apple-touch-startup-image）和 180px 图标。

iOS 从主屏启动 PWA 时，在页面首帧之前会显示自己的启动图；没有
apple-touch-startup-image 就是纯白，这是「冷启动白屏」最直接的来源。
这里用应用背景色 + 居中图标生成一组常见 iPhone / iPad 尺寸，另加一张
无边界的 fallback，保证任何设备都不会白屏。

运行：
  python3 scripts/gen-splash.py
"""

from __future__ import annotations

import struct
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WEB = ROOT / "web"
ICON = WEB / "icon-512.png"
SPLASH = WEB / "splash"
BG = (0xF6, 0xF4, 0xEF)  # 与 manifest background_color / CSS --bg 一致

# (文件名, 物理宽, 物理高, CSS 宽, CSS 高, device-pixel-ratio)
DEVICES = [
    ("iphone-se-1", 640, 1136, 320, 568, 2),
    ("iphone-8", 750, 1334, 375, 667, 2),
    ("iphone-8-plus", 1242, 2208, 414, 736, 3),
    ("iphone-x", 1125, 2436, 375, 812, 3),
    ("iphone-xr", 828, 1792, 414, 896, 2),
    ("iphone-xs-max", 1242, 2688, 414, 896, 3),
    ("iphone-12-mini", 1080, 2340, 360, 780, 3),
    ("iphone-12", 1170, 2532, 390, 844, 3),
    ("iphone-12-pro-max", 1284, 2778, 428, 926, 3),
    ("iphone-15-pro", 1179, 2556, 393, 852, 3),
    ("iphone-15-pro-max", 1290, 2796, 430, 932, 3),
    ("iphone-16-pro", 1206, 2622, 402, 874, 3),
    ("iphone-16-pro-max", 1320, 2868, 440, 956, 3),
    ("iphone-air", 1260, 2736, 420, 912, 3),
    ("ipad-9-7", 1536, 2048, 768, 1024, 2),
    ("ipad-10-2", 1620, 2160, 810, 1080, 2),
    ("ipad-pro-10-5", 1668, 2224, 834, 1112, 2),
    ("ipad-air-10-9", 1640, 2360, 820, 1180, 2),
    ("ipad-pro-12-9", 2048, 2732, 1024, 1366, 2),
]

# 没有 media 匹配时的兜底；纯色背景 + 很小的图标，拉伸/裁切都不会露白。
FALLBACK = ("default", 1290, 2796)


def paeth(a: int, b: int, c: int) -> int:
    p = a + b - c
    pa = abs(p - a)
    pb = abs(p - b)
    pc = abs(p - c)
    if pa <= pb and pa <= pc:
        return a
    if pb <= pc:
        return b
    return c


def read_png(path: Path) -> tuple[int, int, list[bytearray]]:
    data = path.read_bytes()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError(f"{path}: not a PNG")
    pos = 8
    width = height = 0
    idat = bytearray()
    while pos < len(data):
        length = struct.unpack(">I", data[pos : pos + 4])[0]
        kind = data[pos + 4 : pos + 8]
        body = data[pos + 8 : pos + 8 + length]
        pos += 12 + length
        if kind == b"IHDR":
            width, height, depth, color, comp, filt, interlace = struct.unpack(
                ">IIBBBBB", body
            )
            if (depth, color, comp, filt, interlace) != (8, 6, 0, 0, 0):
                raise ValueError("只支持 8-bit RGBA 非隔行 PNG")
        elif kind == b"IDAT":
            idat += body
        elif kind == b"IEND":
            break
    raw = zlib.decompress(bytes(idat))
    bpp = 4
    stride = width * bpp
    rows: list[bytearray] = []
    prev = bytearray(stride)
    offset = 0
    for _ in range(height):
        f = raw[offset]
        offset += 1
        line = bytearray(raw[offset : offset + stride])
        offset += stride
        if f == 1:
            for i in range(bpp, stride):
                line[i] = (line[i] + line[i - bpp]) & 0xFF
        elif f == 2:
            for i in range(stride):
                line[i] = (line[i] + prev[i]) & 0xFF
        elif f == 3:
            for i in range(stride):
                left = line[i - bpp] if i >= bpp else 0
                line[i] = (line[i] + ((left + prev[i]) >> 1)) & 0xFF
        elif f == 4:
            for i in range(stride):
                left = line[i - bpp] if i >= bpp else 0
                upleft = prev[i - bpp] if i >= bpp else 0
                line[i] = (line[i] + paeth(left, prev[i], upleft)) & 0xFF
        elif f != 0:
            raise ValueError(f"未知 PNG filter {f}")
        rows.append(line)
        prev = line
    return width, height, rows


def resize_rgba(
    src_w: int, src_h: int, rows: list[bytearray], dst_w: int, dst_h: int
) -> list[bytearray]:
    """盒式降采样，保留 alpha 预乘，避免图标边缘发黑。"""
    out: list[bytearray] = []
    for y in range(dst_h):
        sy0 = y * src_h // dst_h
        sy1 = max(sy0 + 1, (y + 1) * src_h // dst_h)
        line = bytearray(dst_w * 4)
        for x in range(dst_w):
            sx0 = x * src_w // dst_w
            sx1 = max(sx0 + 1, (x + 1) * src_w // dst_w)
            r = g = b = a = 0
            n = 0
            for sy in range(sy0, sy1):
                row = rows[sy]
                base = sx0 * 4
                end = sx1 * 4
                for i in range(base, end, 4):
                    alpha = row[i + 3]
                    r += row[i] * alpha
                    g += row[i + 1] * alpha
                    b += row[i + 2] * alpha
                    a += alpha
                    n += 1
            off = x * 4
            if a:
                line[off] = r // a
                line[off + 1] = g // a
                line[off + 2] = b // a
                line[off + 3] = a // n
        out.append(line)
    return out


def composite_on_bg(
    rows: list[bytearray], width: int, height: int
) -> list[bytearray]:
    """RGBA → RGB，透明部分合成到应用背景色。"""
    out: list[bytearray] = []
    bg = bytes(BG)
    for y in range(height):
        src = rows[y]
        line = bytearray(bg * width)
        for x in range(width):
            sr, sg, sb, sa = src[x * 4 : x * 4 + 4]
            if not sa:
                continue
            off = x * 3
            if sa == 255:
                line[off : off + 3] = bytes((sr, sg, sb))
            else:
                inv = 255 - sa
                line[off] = (sr * sa + line[off] * inv) // 255
                line[off + 1] = (sg * sa + line[off + 1] * inv) // 255
                line[off + 2] = (sb * sa + line[off + 2] * inv) // 255
        out.append(line)
    return out


def chunk(kind: bytes, body: bytes) -> bytes:
    return (
        struct.pack(">I", len(body))
        + kind
        + body
        + struct.pack(">I", zlib.crc32(kind + body) & 0xFFFFFFFF)
    )


def write_png(path: Path, width: int, height: int, rows: list[bytearray]) -> None:
    raw = b"".join(b"\x00" + bytes(row) for row in rows)
    data = b"\x89PNG\r\n\x1a\n"
    data += chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
    data += chunk(b"IDAT", zlib.compress(raw, 9))
    data += chunk(b"IEND", b"")
    path.write_bytes(data)


def make_splash(
    icon_w: int,
    icon_h: int,
    icon_rows: list[bytearray],
    width: int,
    height: int,
    path: Path,
) -> None:
    icon_px = max(48, int(min(width, height) * 0.20))
    scaled = resize_rgba(icon_w, icon_h, icon_rows, icon_px, icon_px)
    x0 = (width - icon_px) // 2
    y0 = (height - icon_px) // 2
    bg = bytes(BG)
    rows: list[bytearray] = []
    for y in range(height):
        # 整行先铺背景；图标区域再按 alpha 合成。
        line = bytearray(bg * width)
        if y0 <= y < y0 + icon_px:
            src = scaled[y - y0]
            base = x0 * 3
            for x in range(icon_px):
                sr, sg, sb, sa = src[x * 4 : x * 4 + 4]
                if not sa:
                    continue
                off = base + x * 3
                if sa == 255:
                    line[off : off + 3] = bytes((sr, sg, sb))
                else:
                    inv = 255 - sa
                    line[off] = (sr * sa + line[off] * inv) // 255
                    line[off + 1] = (sg * sa + line[off + 1] * inv) // 255
                    line[off + 2] = (sb * sa + line[off + 2] * inv) // 255
        rows.append(line)
    write_png(path, width, height, rows)


def main() -> None:
    if not ICON.exists():
        raise SystemExit(f"缺少 {ICON}")
    SPLASH.mkdir(parents=True, exist_ok=True)
    icon_w, icon_h, icon_rows = read_png(ICON)

    total = 0
    for name, w, h, _cw, _ch, _dpr in DEVICES:
        path = SPLASH / f"{name}.png"
        make_splash(icon_w, icon_h, icon_rows, w, h, path)
        total += path.stat().st_size
    name, w, h = FALLBACK
    path = SPLASH / f"{name}.png"
    make_splash(icon_w, icon_h, icon_rows, w, h, path)
    total += path.stat().st_size

    # 180px 是 iOS 主屏图标的标准尺寸；iOS 会自己切圆角，先铺背景色。
    icon = resize_rgba(icon_w, icon_h, icon_rows, 180, 180)
    write_png(WEB / "icon-180.png", 180, 180, composite_on_bg(icon, 180, 180))

    print(f"{len(DEVICES) + 1} splash images, {total / 1024:.1f} KiB")


if __name__ == "__main__":
    main()
