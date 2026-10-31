#!/usr/bin/env python3
"""Konwertuje oficjalne mapowania Mojang (format ProGuard, named -> obf) do Tiny v2 (official -> named).

Użycie: proguard2tiny.py server_mappings.txt out.tiny
Tiny-remapper potrzebuje deskryptorów w przestrzeni nazw źródłowej ("official" = obfuskowane),
więc nazwy klas w deskryptorach tłumaczymy named -> obf.
"""
import re
import sys

PRIM = {"int": "I", "long": "J", "boolean": "Z", "byte": "B", "short": "S",
        "char": "C", "float": "F", "double": "D", "void": "V"}


def main(src, dst):
    classes = {}  # named(dotted) -> obf(dotted)
    cur = None
    entries = []  # (named_class, kind, line)
    with open(src, encoding="utf-8") as f:
        for raw in f:
            line = raw.rstrip("\n")
            if not line or line.startswith("#"):
                continue
            if not line.startswith(" "):
                m = re.match(r"^(\S+) -> (\S+):$", line)
                named, obf = m.group(1), m.group(2)
                classes[named] = obf
                cur = named
            else:
                entries.append((cur, line.strip()))

    def to_desc(jtype):
        dims = 0
        while jtype.endswith("[]"):
            dims += 1
            jtype = jtype[:-2]
        if jtype in PRIM:
            d = PRIM[jtype]
        else:
            d = "L" + classes.get(jtype, jtype).replace(".", "/") + ";"
        return "[" * dims + d

    members = {}  # named class -> list of lines
    for cls, e in entries:
        m = re.match(r"^(?:\d+:\d+:)?(\S+) (\S+?)(\((.*)\))?(?::\d+:\d+)? -> (\S+)$", e)
        if not m:
            continue
        rtype, name, paren, args, obf = m.group(1), m.group(2), m.group(3), m.group(4), m.group(5)
        if paren is None:  # pole
            members.setdefault(cls, []).append(f"\tf\t{to_desc(rtype)}\t{obf}\t{name}")
        else:
            params = "".join(to_desc(a) for a in args.split(",")) if args else ""
            members.setdefault(cls, []).append(f"\tm\t({params}){to_desc(rtype)}\t{obf}\t{name}")

    with open(dst, "w", encoding="utf-8") as out:
        out.write("tiny\t2\t0\tofficial\tnamed\n")
        for named, obf in classes.items():
            out.write(f"c\t{obf.replace('.', '/')}\t{named.replace('.', '/')}\n")
            for line in members.get(named, []):
                out.write(line + "\n")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
