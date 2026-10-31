#!/usr/bin/env python3
"""Compares two clientbound packet dumps from mc-probe (--dump): the vanilla server vs the Rust server.

Dump format: [state u8][len u32 BE][VarInt id + body]; states: 0 login, 1 config, 2 play.
Usage: cmp_dumps.py vanilla.bin rust.bin [--show NAME ...] [--hex]
"""
import re, struct, sys, collections

def varint(b, i=0):
    v = s = 0
    while True:
        c = b[i]; i += 1
        v |= (c & 0x7F) << s; s += 7
        if not c & 0x80: break
    return v, i

def load(path):
    data = open(path, "rb").read(); i = 0; out = []
    while i < len(data):
        st = data[i]; n = struct.unpack(">I", data[i+1:i+5])[0]; p = data[i+5:i+5+n]; i += 5 + n
        pid, j = varint(p); out.append((st, pid, p[j:]))
    return out

# names from the generated ids.rs
names = {}
cur = None
for line in open("crates/mc-protocol/src/ids.rs"):
    m = re.match(r"\s+pub fn (\w+)_(clientbound|serverbound)\(id: i32\)", line)
    if m: cur = m.group(1) if m.group(2) == "clientbound" else None
    m = re.match(r"\s+(\d+) => \"(\w+)\",", line)
    if m and cur: names[(cur, int(m.group(1)))] = m.group(2)
STATE = {0: "login", 1: "configuration", 2: "play"}
def nm(st, pid): return names.get((STATE[st], pid), f"?{pid}")

a, b = load(sys.argv[1]), load(sys.argv[2])
show = []
if "--show" in sys.argv:
    show = [x for x in sys.argv[sys.argv.index("--show")+1:] if not x.startswith("--")]
hexmode = "--hex" in sys.argv

def summarize(p):
    c = collections.OrderedDict()
    for st, pid, body in p: c.setdefault((st, nm(st, pid)), []).append(body)
    return c
ca, cb = summarize(a), summarize(b)

print(f"{'state/packet':45} {'vanilla':>8} {'rust':>6}  identical bytes?")
for key in list(ca.keys()) + [k for k in cb if k not in ca]:
    la, lb = ca.get(key, []), cb.get(key, [])
    same = ""
    if la and lb:
        same = "YES (all)" if la == lb else ("first YES" if la[0] == lb[0] else "NO")
    mark = "  <-- missing in vanilla" if not la else ("  <-- missing in Rust" if not lb else "")
    print(f"{STATE[key[0]]+'/'+key[1]:45} {len(la):8} {len(lb):6}  {same}{mark}")

print("\nORDER (first occurrences, without chunks):")
def order(p):
    seen = []; 
    for st, pid, _ in p:
        k = f"{STATE[st][0]}:{nm(st,pid)}"
        if k not in seen and "level_chunk" not in k and "chunk_batch" not in k: seen.append(k)
    return seen
oa, ob = order(a), order(b)
print(" vanilla:", " ".join(oa)); print(" rust   :", " ".join(ob))

for s in show:
    for st, pid, body in a:
        if nm(st, pid) == s: print(f"\n[vanilla] {s} ({len(body)} B): {body[:200].hex() if hexmode else body[:200]!r}"); break
    for st, pid, body in b:
        if nm(st, pid) == s: print(f"[rust   ] {s} ({len(body)} B): {body[:200].hex() if hexmode else body[:200]!r}"); break
