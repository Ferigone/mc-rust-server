#!/usr/bin/env python3
"""Writes a minimal vanilla-compatible player data file (<world>/playerdata/<uuid>.dat) for an offline-mode player.

Usage: make_playerdata.py <world_dir> <name> <x> <y> <z> [game_type]

Used by the differential tests to start every test player at a fixed, distinct position (instead of the random spawn
position), so that test cases cannot interfere with each other. Both the vanilla and the Rust server read the file.
"""
import gzip, hashlib, os, struct, sys, uuid

DATA_VERSION = 3955


def offline_uuid(name):
    """java.util.UUID.nameUUIDFromBytes("OfflinePlayer:" + name)"""
    h = bytearray(hashlib.md5(("OfflinePlayer:" + name).encode("utf-8")).digest())
    h[6] = (h[6] & 0x0F) | 0x30
    h[8] = (h[8] & 0x3F) | 0x80
    return uuid.UUID(bytes=bytes(h))


def s(v): b = v.encode("utf-8"); return struct.pack(">H", len(b)) + b
def tag(t, name, payload): return bytes([t]) + s(name) + payload
def f_list(t, items): return bytes([t]) + struct.pack(">i", len(items)) + b"".join(items)


def player_nbt(pos, game_type):
    c = b""
    c += tag(3, "DataVersion", struct.pack(">i", DATA_VERSION))
    c += tag(9, "Pos", f_list(6, [struct.pack(">d", v) for v in pos]))
    c += tag(9, "Motion", f_list(6, [struct.pack(">d", 0.0)] * 3))
    c += tag(9, "Rotation", f_list(5, [struct.pack(">f", 0.0)] * 2))
    c += tag(8, "Dimension", s("minecraft:overworld"))
    c += tag(1, "OnGround", b"\x01")
    c += tag(3, "playerGameType", struct.pack(">i", game_type))
    c += tag(5, "Health", struct.pack(">f", 20.0))
    c += tag(3, "foodLevel", struct.pack(">i", 20))
    c += tag(9, "Inventory", f_list(0, []))
    return tag(10, "", c + b"\x00")


def main():
    world, name, x, y, z = sys.argv[1], sys.argv[2], float(sys.argv[3]), float(sys.argv[4]), float(sys.argv[5])
    gm = int(sys.argv[6]) if len(sys.argv) > 6 else 1
    d = os.path.join(world, "playerdata")
    os.makedirs(d, exist_ok=True)
    with gzip.open(os.path.join(d, f"{offline_uuid(name)}.dat"), "wb") as f:
        f.write(player_nbt((x, y, z), gm))


main()
