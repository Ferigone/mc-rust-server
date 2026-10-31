#!/usr/bin/env python3
"""Takes a level_chunk_with_light packet apart and compares vanilla vs Rust for common chunks."""
import struct, sys
exec(open("tools/cmp_dumps.py").read().split("a, b = load")[0])

class R:
    def __init__(s, b): s.b, s.i = b, 0
    def take(s, n): v = s.b[s.i:s.i+n]; s.i += n; return v
    def u8(s): return s.take(1)[0]
    def vi(s):
        v, s.i = varint(s.b, s.i); return v
    def i32(s): return struct.unpack(">i", s.take(4))[0]
    def i64(s): return struct.unpack(">q", s.take(8))[0]

def skip_nbt(r):
    def payload(t):
        if t == 1: r.take(1)
        elif t == 2: r.take(2)
        elif t == 3 or t == 5: r.take(4)
        elif t == 4 or t == 6: r.take(8)
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

def parse(body):
    r = R(body); x, z = r.i32(), r.i32()
    h0 = r.i; skip_nbt(r); hm = body[h0:r.i]
    n = r.vi(); sec = r.take(n)
    nbe = r.vi(); be = []
    for _ in range(nbe): r.u8(); r.take(2); r.vi(); skip_nbt(r)
    masks = []
    for _ in range(4):
        k = r.vi(); masks.append([r.i64() & 0xFFFFFFFFFFFFFFFF for _ in range(k)])
    sky = []; k = r.vi()
    for _ in range(k): ln = r.vi(); sky.append(r.take(ln))
    blk = []; k = r.vi()
    for _ in range(k): ln = r.vi(); blk.append(r.take(ln))
    return dict(x=x, z=z, hm=hm, sec=sec, be=nbe, masks=masks, sky=sky, blk=blk, end=r.i == len(body))

def bits(m): return [i for w, v in enumerate(m) for i in range(64) if v >> i & 1]

if __name__ == "__main__":
    v = load(sys.argv[1]); r_ = load(sys.argv[2])
    def chunks(p): return {(parse(b)["x"], parse(b)["z"]): parse(b) for st, pid, b in p if st == 2 and nm(st, pid) == "level_chunk_with_light"}
    cv, cr = chunks(v), chunks(r_)
    common = sorted(set(cv) & set(cr))
    k = common[0] if common else None
    print("common chunks:", len(common), "example:", k)
    a, b = cv[k], cr[k]
    print("heightmap NBT:", "IDENTICAL" if a["hm"] == b["hm"] else f"DIFFERENT vanilla={a['hm'].hex()[:60]}.. rust={b['hm'].hex()[:60]}..")
    print("sections (bytes):", "IDENTICAL" if a["sec"] == b["sec"] else f"DIFFERENT vanilla={len(a['sec'])}B rust={len(b['sec'])}B")
    print("block entities:", a["be"], b["be"])
    for nme, i in (("sky_mask", 0), ("block_mask", 1), ("empty_sky_mask", 2), ("empty_block_mask", 3)):
        print(f"  {nme}: vanilla={bits(a['masks'][i])} rust={bits(b['masks'][i])}")
    print("  tablice sky: vanilla", len(a["sky"]), "rust", len(b["sky"]), "| block: vanilla", len(a["blk"]), "rust", len(b["blk"]))
    ok = [all(x == b2 for x, b2 in zip(a["sky"], b["sky"]))] if a["sky"] and b["sky"] else []
    print("  pierwsze tablice sky identyczne:", a["sky"][:1] == b["sky"][:1])
    stats = {"hm": 0, "sec": 0, "masks": 0, "sky": 0}
    for key in common:
        x, y = cv[key], cr[key]
        stats["hm"] += x["hm"] == y["hm"]; stats["sec"] += x["sec"] == y["sec"]
        stats["masks"] += x["masks"] == y["masks"]; stats["sky"] += x["sky"] == y["sky"] and x["blk"] == y["blk"]
    print("identical in all common chunks:", stats, "of", len(common))
