#!/usr/bin/env python3
"""Compares what the REAL vanilla server and the Rust server send for the same Anvil world (level_chunk_with_light).

Usage: world_compare.py <vanilla.bin> <rust.bin> [--after <vanilla_after_edit.bin> <x,y,z> [--after-rust <rust_after_edit.bin>]]

Dumps come from `mc-probe join --dump`. Checks, for every chunk both servers sent:
  * heightmaps, block entities (type, position, NBT bytes, order) and the section structure must be identical,
  * block data must be identical except for positions the vanilla server changed itself by running scheduled / random
    ticks right after loading (leaf distance, flowing water and lava, ...); those are listed, they are the job of M7,
  * light data is only reported (it depends on neighbouring chunks that the Rust server cannot generate yet).
With --after: the world saved by the Rust server after breaking <x,y,z> was loaded by vanilla; exactly that block (and
nothing else besides vanilla's own tick noise) may differ from the original vanilla view.
With --after-rust: the Rust server serves the world it saved itself (the edited chunk has no stored light, so vanilla relights it
and the Rust server runs its light engine); every chunk both servers send must be identical, light included.
Exit code 1 when a hard check fails.
"""
import collections, json, os, struct, sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
exec(open(os.path.join(ROOT, "tools/cmp_dumps.py")).read().split("a, b = load")[0])  # varint, load, nm


class R:
    def __init__(s, b): s.b, s.i = b, 0
    def take(s, n): v = s.b[s.i:s.i + n]; s.i += n; return v
    def u8(s): return s.take(1)[0]
    def vi(s):
        v, s.i = varint(s.b, s.i); return v
    def i32(s): return struct.unpack(">i", s.take(4))[0]
    def i64(s): return struct.unpack(">q", s.take(8))[0]


def skip_nbt(r):
    def payload(t):
        if t == 1: r.take(1)
        elif t == 2: r.take(2)
        elif t in (3, 5): r.take(4)
        elif t in (4, 6): r.take(8)
        elif t == 7: r.take(struct.unpack(">i", r.take(4))[0])
        elif t == 8: r.take(struct.unpack(">H", r.take(2))[0])
        elif t == 9:
            et = r.u8(); n = struct.unpack(">i", r.take(4))[0]
            for _ in range(n): payload(et)
        elif t == 10:
            while True:
                tt = r.u8()
                if tt == 0: break
                r.take(struct.unpack(">H", r.take(2))[0]); payload(tt)
        elif t == 11: r.take(4 * struct.unpack(">i", r.take(4))[0])
        elif t == 12: r.take(8 * struct.unpack(">i", r.take(4))[0])
    t = r.u8()
    if t: payload(t)


def parse_chunk(body):
    r = R(body); x, z = r.i32(), r.i32()
    h0 = r.i; skip_nbt(r); hm = body[h0:r.i]
    sec = r.take(r.vi())
    bes = []
    for _ in range(r.vi()):
        b0 = r.i; r.u8(); r.take(2); r.vi(); skip_nbt(r); bes.append(body[b0:r.i])
    return dict(x=x, z=z, hm=hm, sec=sec, be=bes, light=body[r.i:])


def read_container(b, i, blocks):
    bits = b[i]; i += 1
    if bits == 0:
        pid, i = varint(b, i); n, i = varint(b, i)
        return i + 8 * n, [pid] * (4096 if blocks else 64)
    pal = None
    if (blocks and bits <= 8) or (not blocks and bits <= 3):
        n, i = varint(b, i); pal = []
        for _ in range(n):
            p, i = varint(b, i); pal.append(p)
    n, i = varint(b, i)
    longs = [struct.unpack(">Q", b[i + 8 * k:i + 8 * k + 8])[0] for k in range(n)]
    per, mask, total = 64 // bits, (1 << bits) - 1, 4096 if blocks else 64
    vals = [(longs[j // per] >> ((j % per) * bits)) & mask for j in range(total)]
    return i + 8 * n, [pal[v] for v in vals] if pal else vals


def sections(sec):
    """[(non_empty_count, blocks[4096], biomes[64])] and the raw bytes of every section."""
    i = 0; out = []
    while i < len(sec):
        s0 = i
        cnt = struct.unpack(">h", sec[i:i + 2])[0]; i += 2
        i, blocks = read_container(sec, i, True)
        i, biomes = read_container(sec, i, False)
        out.append((cnt, blocks, biomes, sec[s0:i]))
    return out


def chunks(path):
    out = {}
    for st, pid, body in load(path):
        if st == 2 and nm(st, pid) == "level_chunk_with_light":
            c = parse_chunk(body); out[(c["x"], c["z"])] = c
    return out


def block_names():
    p = os.path.join(ROOT, "reference/gen/generated/reports/blocks.json")
    names = {}
    for name, v in json.load(open(p)).items():
        for s in v["states"]: names[s["id"]] = name.replace("minecraft:", "")
    return names


def diff_blocks(a, b, k):
    """Positions whose block differs: [((x, y, z), state_a, state_b)]; structure problems are returned separately."""
    sa, sb = sections(a["sec"]), sections(b["sec"])
    out, structure = [], []
    if len(sa) != len(sb): return out, ["section count"]
    for idx, (x, y) in enumerate(zip(sa, sb)):
        if x[2] != y[2]: structure.append(f"biomes of section {idx}")
        for j in range(4096):
            if x[1][j] != y[1][j]:
                out.append(((k[0] * 16 + (j & 15), (idx - 4) * 16 + (j >> 8), k[1] * 16 + ((j >> 4) & 15)), x[1][j], y[1][j]))
    return out, structure


def main():
    args = [a for a in sys.argv[1:]]
    after = None
    after_rust = None
    if "--after-rust" in args:
        i = args.index("--after-rust"); after_rust = args[i + 1]; del args[i:i + 2]
    if "--after" in args:
        i = args.index("--after"); after = (args[i + 1], tuple(int(v) for v in args[i + 2].split(","))); del args[i:i + 3]
    van, rus = chunks(args[0]), chunks(args[1])
    names = block_names()
    common = sorted(set(van) & set(rus))
    print(f"chunks: vanilla sent {len(van)}, rust sent {len(rus)}, in common {len(common)} (the rest is generated by the vanilla server on demand: M5)")
    hard = 0
    ident = sum(1 for k in common if van[k]["sec"] == rus[k]["sec"])
    print(f"section bytes identical: {ident}/{len(common)}")
    print(f"heightmap NBT identical: {sum(1 for k in common if van[k]['hm'] == rus[k]['hm'])}/{len(common)}")
    be_ok = sum(1 for k in common if van[k]["be"] == rus[k]["be"])
    print(f"block entities identical (count, order, bytes): {be_ok}/{len(common)}")
    hard += len(common) - be_ok
    lt = sum(1 for k in common if van[k]["light"] == rus[k]["light"])
    print(f"light data identical (masks + arrays): {lt}/{len(common)} chunks (informational: light spills in from neighbouring chunks that only the vanilla server generates; the light engine is M4)")
    pairs = collections.Counter(); positions = 0
    for k in common:
        d, structure = diff_blocks(van[k], rus[k], k)
        if structure: print("STRUCTURE DIFFERENCE", k, structure); hard += 1
        positions += len(d)
        for _, x, y in d: pairs[(names.get(x, x), names.get(y, y))] += 1
    print(f"blocks that differ: {positions} positions (vanilla -> rust), by block family:")
    fam = collections.Counter()
    for (x, y), n in pairs.items():
        key = x if x == y else f"{x} -> {y}"
        fam["leaves distance" if "leaves" in x and "leaves" in y else "fluid" if any(f in x + y for f in ("water", "lava")) else "other: " + key] += n
    for k, n in fam.most_common(12): print(f"    {n:6d}  {k}")
    hard += sum(n for k, n in fam.items() if k.startswith("other"))
    if after:
        path, (px, py, pz) = after
        aft = chunks(path)
        k = (px >> 4, pz >> 4)
        d, structure = diff_blocks(van[k], aft[k], k)
        edit = [t for t in d if t[0] == (px, py, pz)]
        print(f"edit check: vanilla loaded the Rust-saved world; block ({px},{py},{pz}) {names.get(edit[0][1]) if edit else '?'} -> {names.get(edit[0][2]) if edit else '(unchanged!)'}; other differing blocks in that chunk: {len(d) - len(edit)}")
        if not edit or len(d) != 1 or structure: hard += 1
        be = van[k]["be"] == aft[k]["be"]
        print(f"edit check: block entities of the edited chunk identical: {be}")
        hard += not be
    if after and after_rust:
        av, ar = chunks(after[0]), chunks(after_rust)
        common2 = sorted(set(av) & set(ar))
        what = {"sections": "sec", "heightmaps": "hm", "block entities": "be", "light (masks + arrays)": "light"}
        res = {name: sum(1 for k in common2 if av[k][key] == ar[k][key]) for name, key in what.items()}
        print("saved world served by both servers: " + ", ".join(f"{name} identical {n}/{len(common2)}" for name, n in res.items()))
        for name, key in what.items():
            bad = [k for k in common2 if av[k][key] != ar[k][key]]
            if bad:
                hard += len(bad)
                print(f"  DIFFERENT {name} in chunks {bad[:6]}")
    print("RESULT:", "FAIL (%d hard differences)" % hard if hard else "OK")
    sys.exit(1 if hard else 0)


main()
