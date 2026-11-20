#!/usr/bin/env python3
"""把 TeX 英文断字模式编译成排版层可以直接 include_bytes! 的二进制表。

数据来源（按优先级）：

1. `--patterns` 指定的 `patterns.go`（默认 `~/layx/render/hyphen/en/patterns.go`），
   里面是 hyph-en-us 的 ushyphmax 模式，一行一条；
2. 也可以直接喂 `hyph-en-us.tex`（`--tex`），行为相同：`%` 注释、空行跳过。

输出布局（小端、零拷贝、`u8` 对齐）：

    magic          4 字节  b"HYPH"
    version        u8      = 1
    node_count     u32
    edge_count     u32
    weight_count   u32
    nodes          node_count * 6 字节
                   { edge_start: u16, edge_len: u8, weight_len: u8, weight_start: u16 }
    edges          edge_count * 3 字节  { label: u8, child: u16 }
    weights        weight_count 字节    每条模式的 0..9 权重

u16 偏移对 hyph-en-us 够用（8399 个节点、9430 条边、4 万多个权重），
生成时越界会直接报错，不会写出坏表。

模式文本里的数字是权重，字母与 `.`（词边界）是路径。同一节点只保留一份权重，
重复模式取逐位最大值，和 `~/layx/render/hyphen/en/en.go` 的 buildPatterns 一致。

用法：

    python3 data/gen_hyphen.py                # 默认从 ~/layx 生成
    python3 data/gen_hyphen.py --tex hyph-en-us.tex
"""

import argparse
import os
import struct
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DEFAULT_GO = os.path.expanduser("~/layx/render/hyphen/en/patterns.go")
DEFAULT_OUT = os.path.join(ROOT, "crates/ebook-layout/src/hyphen/en.pat")


def read_go_patterns(path):
    with open(path, encoding="utf-8") as handle:
        text = handle.read()
    marker = "const patternData = `"
    start = text.find(marker)
    if start < 0:
        raise SystemExit(f"{path}: no patternData string")
    start += len(marker)
    end = text.find("`", start)
    if end < 0:
        raise SystemExit(f"{path}: unterminated patternData string")
    return text[start:end].splitlines()


def read_tex_patterns(path):
    with open(path, encoding="utf-8") as handle:
        text = handle.read()
    # hyph-utf8 的 .tex 把模式包在 \patterns{ ... } 里，后面可能还有
    # \hyphenation{...}；只取 patterns 块，避免把 TeX 命令当模式。
    start = text.find("\\patterns{")
    if start >= 0:
        body = text[start + len("\\patterns{") :]
        end = body.find("\n}")
        if end >= 0:
            body = body[:end]
        text = body
    lines = []
    for line in text.splitlines():
        line = line.split("%", 1)[0].strip()
        if line:
            lines.append(line)
    return lines


def parse(lines):
    patterns = []
    for line in lines:
        line = line.strip()
        if not line:
            continue
        letters = []
        weights = [0]
        for char in line:
            if "0" <= char <= "9":
                weights[-1] = int(char)
            elif char == "." or ("a" <= char <= "z"):
                letters.append(char)
                weights.append(0)
            else:
                raise SystemExit(f"unexpected pattern character {char!r} in {line!r}")
        if letters:
            patterns.append(("".join(letters), weights))
    return patterns


class Node:
    __slots__ = ("children", "weights")

    def __init__(self):
        self.children = {}
        self.weights = None


def build(patterns):
    nodes = [Node()]
    for letters, weights in patterns:
        node = nodes[0]
        for char in letters:
            child = node.children.get(char)
            if child is None:
                child = len(nodes)
                node.children[char] = child
                nodes.append(Node())
            node = nodes[child]
        if node.weights is None:
            node.weights = list(weights)
        else:
            for index, weight in enumerate(weights):
                if weight > node.weights[index]:
                    node.weights[index] = weight
    return nodes


def serialize(nodes):
    edges = []
    weights = []
    records = []
    for node in nodes:
        edge_start = len(edges)
        for label in sorted(node.children):
            edges.append((label, node.children[label]))
        weight_start = len(weights)
        weight_len = 0
        if node.weights is not None:
            weights.extend(node.weights)
            weight_len = len(node.weights)
        records.append((edge_start, len(node.children), weight_len, weight_start))

    for name, count in (("edges", len(edges)), ("weights", len(weights))):
        if count > 0xFFFF:
            raise SystemExit(f"{name} do not fit u16: {count}")
    if len(records) > 0xFFFF:
        raise SystemExit(f"nodes do not fit u16: {len(records)}")

    out = bytearray()
    out += b"HYPH"
    out += bytes([2])
    out += struct.pack("<III", len(records), len(edges), len(weights))
    for edge_start, edge_len, weight_len, weight_start in records:
        if edge_len > 0xFF or weight_len > 0xFF:
            raise SystemExit("node degree/weight length does not fit u8")
        out += struct.pack("<HBBH", edge_start, edge_len, weight_len, weight_start)
    for label, child in edges:
        out += struct.pack("<BH", ord(label), child)
    out += bytes(weights)
    return bytes(out)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--patterns", default=DEFAULT_GO)
    parser.add_argument("--tex")
    parser.add_argument("--out", default=DEFAULT_OUT)
    args = parser.parse_args()

    if args.tex:
        lines = read_tex_patterns(args.tex)
    else:
        if not os.path.exists(args.patterns):
            raise SystemExit(f"pattern source missing: {args.patterns}")
        lines = read_go_patterns(args.patterns)

    patterns = parse(lines)
    nodes = build(patterns)
    blob = serialize(nodes)

    os.makedirs(os.path.dirname(args.out), exist_ok=True)
    with open(args.out, "wb") as handle:
        handle.write(blob)
    print(
        f"{args.out}: {len(patterns)} patterns, {len(nodes)} nodes, {len(blob)} bytes",
        file=sys.stderr,
    )


if __name__ == "__main__":
    main()
