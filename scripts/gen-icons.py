#!/usr/bin/env python3
"""生成 PWA / Apple Touch 图标（icon-180/192/512.png）。

图标是一本摊开的书：深色底、米色书页、中间橙色书脊。所有形状都在代码里，
4x 超采样后降采样，避免小尺寸出现硬锯齿。只依赖 Python 标准库。

运行（仓库根目录）：

    python3 scripts/gen-icons.py

scripts/gen-splash.py 依赖这里生成的 icon-512.png。
"""

from __future__ import annotations

import struct
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WEB = ROOT / "web"

BG = (0x12, 0x15, 0x1C)
PAGE = (0xF7, 0xF4, 0xED)
EDGE = (0xE2, 0xDB, 0xCF)
SPINE = (0xB4, 0x53, 0x2A)
LINE = (0x96, 0x8B, 0x7B)

SS = 4  # 每个像素在每个方向上的采样数


def inside_round_rect(
    x: float, y: float, x0: float, y0: float, x1: float, y1: float, r: float
) -> bool:
    if x < x0 or x > x1 or y < y0 or y > y1:
        return False
    cx = x0 + r if x < x0 + r else (x1 - r if x > x1 - r else x)
    cy = y0 + r if y < y0 + r else (y1 - r if y > y1 - r else y)
    return (x - cx) ** 2 + (y - cy) ** 2 <= r * r


def color_at(x: float, y: float, size: float) -> tuple[int, int, int]:
    margin = 0.199 * size
    x0 = y0 = margin
    x1 = y1 = size - margin
    radius = 0.024 * size

    if not inside_round_rect(x, y, x0, y0, x1, y1, radius):
        return BG

    # 书页与封面边缘：左右各留一条浅色书口。
    edge = 0.037 * size
    if x < x0 + edge or x > x1 - edge:
        return EDGE

    # 书脊贯穿整个书页高度。
    spine = 0.047 * size
    if abs(x - size / 2) <= spine / 2:
        return SPINE

    # 每侧三条文字线。
    line_w = 0.164 * size
    line_h = 0.019 * size
    left = x0 + 0.072 * size
    right = x0 + 0.361 * size
    for ratio in (0.149, 0.280, 0.410):
        cy = y0 + ratio * size
        if abs(y - cy) <= line_h / 2 and (
            left <= x <= left + line_w or right <= x <= right + line_w
        ):
            return LINE

    return PAGE


def render(size: int) -> list[bytearray]:
    rows: list[bytearray] = []
    step = 1.0 / SS
    total = SS * SS
    for py in range(size):
        line = bytearray(size * 4)
        for px in range(size):
            r = g = b = 0
            for sy in range(SS):
                y = py + (sy + 0.5) * step
                for sx in range(SS):
                    x = px + (sx + 0.5) * step
                    cr, cg, cb = color_at(x, y, size)
                    r += cr
                    g += cg
                    b += cb
            off = px * 4
            line[off] = r // total
            line[off + 1] = g // total
            line[off + 2] = b // total
            line[off + 3] = 255
        rows.append(line)
    return rows


def chunk(kind: bytes, body: bytes) -> bytes:
    return (
        struct.pack(">I", len(body))
        + kind
        + body
        + struct.pack(">I", zlib.crc32(kind + body) & 0xFFFFFFFF)
    )


def write_png(path: Path, size: int, rows: list[bytearray], alpha: bool) -> None:
    color = 6 if alpha else 2
    if alpha:
        pixels = b"".join(b"\x00" + bytes(row) for row in rows)
    else:
        pixels = b"".join(
            b"\x00" + bytes(c for i in range(0, len(row), 4) for c in row[i : i + 3])
            for row in rows
        )
    data = b"\x89PNG\r\n\x1a\n"
    data += chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, color, 0, 0, 0))
    data += chunk(b"IDAT", zlib.compress(pixels, 9))
    data += chunk(b"IEND", b"")
    path.write_bytes(data)
    print(f"wrote {path.relative_to(ROOT)} ({len(data)} bytes)")


def main() -> None:
    WEB.mkdir(parents=True, exist_ok=True)
    # 180px 给 iOS 主屏用，保持不透明（透明会被系统合成成黑/白）。
    write_png(WEB / "icon-180.png", 180, render(180), alpha=False)
    write_png(WEB / "icon-192.png", 192, render(192), alpha=True)
    write_png(WEB / "icon-512.png", 512, render(512), alpha=True)


if __name__ == "__main__":
    main()
