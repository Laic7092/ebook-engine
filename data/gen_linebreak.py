#!/usr/bin/env python3
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SOURCE = ROOT / "data" / "unicode" / "LineBreak.txt"
TARGET = ROOT / "crates" / "ebook-layout" / "src" / "linebreak_table.rs"

LINE = re.compile(r"^([0-9A-Fa-f]+)(?:\.\.([0-9A-Fa-f]+))?\s*;\s*([A-Z0-9]{2})\s*(?:#\s*(\w+))?")

DEFAULTS = (
    (0x3400, 0x4DBF, "ID"),
    (0x4E00, 0x9FFF, "ID"),
    (0xF900, 0xFAFF, "ID"),
    (0x20000, 0x2FFFD, "ID"),
    (0x30000, 0x3FFFD, "ID"),
    (0x1F000, 0x1FAFF, "ID"),
    (0x1FC00, 0x1FFFD, "ID"),
    (0x20A0, 0x20CF, "PR"),
)

RESOLVE = {
    "AI": "AL",
    "SG": "AL",
    "XX": "AL",
    "CJ": "NS",
}

KEEP = {
    "AK", "AL", "AP", "AS", "B2", "BA", "BB", "BK", "CB", "CL", "CM", "CP",
    "CR", "EB", "EM", "EX", "GL", "H2", "H3", "HH", "HL", "HY", "ID", "IN",
    "IS", "JL", "JT", "JV", "LF", "NL", "NS", "NU", "OP", "PO", "PR", "QU",
    "QI", "QF", "RI", "SP", "SY", "VF", "VI", "WJ", "ZW", "ZWJ",
}

NAMES = {
    "AK": "Ak", "AL": "Al", "AP": "Ap", "AS": "As", "B2": "B2", "BA": "Ba",
    "BB": "Bb", "BK": "Bk", "CB": "Cb", "CL": "Cl", "CM": "Cm", "CP": "Cp",
    "CR": "Cr", "EB": "Eb", "EM": "Em", "EX": "Ex", "GL": "Gl", "H2": "H2",
    "H3": "H3", "HH": "Hh", "HL": "Hl", "HY": "Hy", "ID": "Id", "IN": "In",
    "IS": "Is", "JL": "Jl", "JT": "Jt", "JV": "Jv", "LF": "Lf", "NL": "Nl",
    "NS": "Ns", "NU": "Nu", "OP": "Op", "PO": "Po", "PR": "Pr", "QU": "Qu",
    "QI": "Qi", "QF": "Qf", "RI": "Ri", "SP": "Sp", "SY": "Sy", "VF": "Vf",
    "VI": "Vi", "WJ": "Wj", "ZW": "Zw", "ZWJ": "Zwj",
}


def parse():
    entries = []
    for raw in SOURCE.read_text(encoding="utf-8").splitlines():
        match = LINE.match(raw)
        if not match:
            continue
        start = int(match.group(1), 16)
        end = int(match.group(2), 16) if match.group(2) else start
        entries.append((start, end, match.group(3), match.group(4)))
    entries.sort()
    for left, right in zip(entries, entries[1:]):
        if left[1] >= right[0]:
            raise SystemExit(f"overlapping source ranges: {left} {right}")
    return entries


def fill_defaults(entries):
    extra = []
    for start, end, cls in DEFAULTS:
        cursor = start
        for entry_start, entry_end, _, _ in entries:
            if entry_end < cursor:
                continue
            if entry_start > end:
                break
            if entry_start > cursor:
                extra.append((cursor, min(entry_start - 1, end), cls, None))
            cursor = max(cursor, entry_end + 1)
            if cursor > end:
                break
        if cursor <= end:
            extra.append((cursor, end, cls, None))
    return entries + extra


def resolve(cls, category):
    if cls == "SA":
        return "CM" if category in ("Mn", "Mc") else "AL"
    if cls == "QU":
        if category == "Pi":
            return "QI"
        if category == "Pf":
            return "QF"
    return RESOLVE.get(cls, cls)


def compile_ranges(entries):
    segments = []
    for start, end, cls, category in sorted(entries):
        resolved = resolve(cls, category)
        if resolved not in KEEP:
            raise SystemExit(f"unhandled class: {resolved}")
        segments.append((start, end, resolved))
    merged = []
    for start, end, cls in segments:
        if cls == "AL":
            continue
        if merged and merged[-1][2] == cls and merged[-1][1] + 1 == start:
            merged[-1] = (merged[-1][0], end, cls)
        else:
            merged.append((start, end, cls))
    return merged


def render(ranges):
    variants = sorted(KEEP, key=lambda cls: NAMES[cls].lower())
    lines = []
    lines.append("pub(crate) const RANGE_COUNT: usize = %d;" % len(ranges))
    lines.append("")
    lines.append("#[derive(Clone, Copy, Debug, PartialEq, Eq)]")
    lines.append("pub(crate) enum Lb {")
    for cls in variants:
        lines.append("    %s," % NAMES[cls])
    lines.append("}")
    lines.append("")
    lines.append("pub(crate) static RANGES: &[(u32, u32, Lb)] = &[")
    for start, end, cls in ranges:
        lines.append("    (0x%04X, 0x%04X, Lb::%s)," % (start, end, NAMES[cls]))
    lines.append("];")
    lines.append("")
    lines.append("pub(crate) fn lb_of(c: char) -> Lb {")
    lines.append("    let cp = c as u32;")
    lines.append("    let mut lo = 0usize;")
    lines.append("    let mut hi = RANGE_COUNT;")
    lines.append("    while lo < hi {")
    lines.append("        let mid = (lo + hi) / 2;")
    lines.append("        let (start, end, class) = RANGES[mid];")
    lines.append("        if cp < start {")
    lines.append("            hi = mid;")
    lines.append("        } else if cp > end {")
    lines.append("            lo = mid + 1;")
    lines.append("        } else {")
    lines.append("            return class;")
    lines.append("        }")
    lines.append("    }")
    lines.append("    Lb::Al")
    lines.append("}")
    lines.append("")
    return "\n".join(lines)


def main():
    entries = parse()
    ranges = compile_ranges(fill_defaults(entries))
    text = render(ranges)
    TARGET.write_text(text, encoding="utf-8")
    print(f"source ranges: {len(entries)}")
    print(f"table ranges: {len(ranges)}")
    print(f"table bytes: {len(text.encode('utf-8'))}")


if __name__ == "__main__":
    main()
