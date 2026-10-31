#!/usr/bin/env python3
"""NBT viewer: level.dat (gzip) or a chunk from an .mca file.  nbtdump.py level.dat | nbtdump.py r.0.0.mca [cx cz]"""
import gzip, struct, sys, zlib

def rd(b, i, t):
    if t == 1: return b[i], i+1
    if t == 2: return struct.unpack(">h", b[i:i+2])[0], i+2
    if t == 3: return struct.unpack(">i", b[i:i+4])[0], i+4
    if t == 4: return struct.unpack(">q", b[i:i+8])[0], i+8
    if t == 5: return struct.unpack(">f", b[i:i+4])[0], i+4
    if t == 6: return struct.unpack(">d", b[i:i+8])[0], i+8
    if t == 7:
        n = struct.unpack(">i", b[i:i+4])[0]; return ("bytes", n), i+4+n
    if t == 8:
        n = struct.unpack(">H", b[i:i+2])[0]; return b[i+2:i+2+n].decode("utf-8", "replace"), i+2+n
    if t == 9:
        et = b[i]; n = struct.unpack(">i", b[i+1:i+5])[0]; i += 5; out = []
        for _ in range(n):
            v, i = rd(b, i, et); out.append(v)
        return out, i
    if t == 10:
        d = {}
        while b[i] != 0:
            tt = b[i]; n = struct.unpack(">H", b[i+1:i+3])[0]; k = b[i+3:i+3+n].decode(); v, i = rd(b, i+3+n, tt); d[k] = v
        return d, i+1
    if t == 11:
        n = struct.unpack(">i", b[i:i+4])[0]; return ("ints", n), i+4+4*n
    if t == 12:
        n = struct.unpack(">i", b[i:i+4])[0]; return ("longs", n), i+4+8*n
    raise Exception(f"tag {t}")

def parse(b):
    t = b[0]; n = struct.unpack(">H", b[1:3])[0]
    return rd(b, 3+n, t)[0]

def show(v, ind=0, maxlist=3):
    p = "  " * ind
    if isinstance(v, dict):
        for k, x in v.items():
            if isinstance(x, (dict, list)) and x:
                print(f"{p}{k}:"); show(x, ind+1)
            else: print(f"{p}{k}: {x if not isinstance(x, (dict, list)) else x}")
    elif isinstance(v, list):
        for j, x in enumerate(v[:maxlist]):
            print(f"{p}- [{j}]"); show(x, ind+1) if isinstance(x, (dict, list)) else print(f"{p}  {x}")
        if len(v) > maxlist: print(f"{p}... (+{len(v)-maxlist} elements, {len(v)} in total)")

def region_chunk(path, cx, cz):
    d = open(path, "rb").read()
    idx = (cx & 31) + (cz & 31) * 32
    ent = struct.unpack(">I", d[idx*4:idx*4+4])[0]
    off, cnt = (ent >> 8) * 4096, ent & 0xFF
    if not ent: return None
    ln, ct = struct.unpack(">IB", d[off:off+5]); data = d[off+5:off+4+ln]
    comp = {1: gzip.decompress, 2: zlib.decompress, 3: lambda x: x}.get(ct)
    if comp is None: raise Exception(f"compression {ct}")
    return parse(comp(data)), ct, cnt

if __name__ == "__main__":
    p = sys.argv[1]
    if p.endswith(".dat"): show(parse(gzip.decompress(open(p, "rb").read())))
    else:
        cx, cz = (int(sys.argv[2]), int(sys.argv[3])) if len(sys.argv) > 3 else (0, 0)
        r = region_chunk(p, cx, cz); print("compression", r[1], "sectors", r[2]); show(r[0])
