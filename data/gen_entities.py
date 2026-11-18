from html.entities import name2codepoint

items = sorted(name2codepoint.items())
lines = [
    "pub(crate) const NAMED: [(&str, char); %d] = [" % len(items),
]
for name, code in items:
    lines.append('    ("%s", \'\\u{%x}\'),' % (name, code))
lines.append("];")
open("crates/ebook-format/src/entities.rs", "w").write("\n".join(lines) + "\n")
print("entities %d" % len(items))
