#!/usr/bin/env python3
"""Porównanie zrzutów pakietów: prawdziwy serwer vanilla 1.21.1 vs serwer Rust. Użycie: vanilla_compare.py <katalog_zrzutów>"""
import os, re, struct, sys, json, collections
D = sys.argv[1]
exec(open(os.path.join(os.path.dirname(__file__), "cmp_dumps.py")).read().split("a, b = load")[0].replace('open("crates/mc-protocol/src/ids.rs")', 'open(os.path.join(os.path.dirname(__file__), "..", "crates/mc-protocol/src/ids.rs"))'))
sys.path.insert(0, os.path.dirname(__file__))

def pk(tag, case, name, st=2):
    return [b for s, i, b in load(f"{D}/{tag}_{case}.bin") if s == st and nm(s, i) == name]
def eid(tag, case): return struct.unpack(">i", pk(tag, case, "login")[0][:4])[0]

results = []   # (kategoria, opis, status, szczegół)
def rec(cat, desc, ok, detail=""): results.append((cat, desc, "OK" if ok else "RÓŻNICA", detail))

# --- status
sv = open(f"{D}/status_vanilla.txt").read().split("STATUS_JSON ", 1)[1].strip()
sr = open(f"{D}/status_rust.txt").read().split("STATUS_JSON ", 1)[1].strip()
jv, jr = json.loads(sv), json.loads(sr)
jv.pop("favicon", None); jr.pop("favicon", None)
rec("bajty identyczne", "status JSON (wartości i kolejność kluczy, bez bajtów ikony)", json.dumps(jv) == json.dumps(jr))

# --- login / configuration / join
for name in ["login_compression", "game_profile"]:
    rec("bajty identyczne", f"login/{name}", pk("vanilla", "join", name, 0) == pk("rust", "join", name, 0))
for name in ["custom_payload", "update_enabled_features", "select_known_packs", "registry_data", "finish_configuration"]:
    rec("bajty identyczne", f"configuration/{name}", pk("vanilla", "join", name, 1) == pk("rust", "join", name, 1), f"{len(pk('rust','join',name,1))} pak.")
for name in ["change_difficulty", "player_abilities", "set_carried_item", "server_data", "player_info_update", "initialize_border",
             "set_default_spawn_position", "game_event", "ticking_state", "ticking_step", "container_set_content", "set_health", "set_experience"]:
    v, r = pk("vanilla", "join", name), pk("rust", "join", name)
    rec("bajty identyczne", f"play/{name}", v == r, "" if v == r else f"vanilla={len(v)} pak. rust={len(r)} pak.")
ev, er = eid("vanilla", "join"), eid("rust", "join")
def norm_login(b):
    # eid(4) | hardcore(1) | liczba wymiarów | wymiary... | reszta; kolejność wymiarów (HashSet) zmienia się między uruchomieniami JVM
    n, i = varint(b, 5); dims = []
    for _ in range(n):
        l, i = varint(b, i); dims.append(b[i:i+l]); i += l
    return b[4:5] + b"".join(sorted(dims)) + b[i:]
lv, lr = norm_login(pk("vanilla", "join", "login")[0]), norm_login(pk("rust", "join", "login")[0])
rec("identyczne po zamaskowaniu ID encji i kolejności wymiarów (HashSet)", "play/login", lv == lr, "" if lv == lr else f"vanilla={lv.hex()} rust={lr.hex()}")
rec("identyczne po zamaskowaniu ID encji", "play/entity_event", pk("vanilla", "join", "entity_event")[0][4:] == pk("rust", "join", "entity_event")[0][4:])
def mine(tag, name, e):
    out = []
    for b in pk(tag, "join", name):
        x, i = varint(b, 0)
        if x == e: out.append(b[i:])
    return out
rec("identyczne po zamaskowaniu ID encji", "play/set_entity_data (własny gracz)", mine("vanilla", "set_entity_data", ev) == mine("rust", "set_entity_data", er))
pv, pr = pk("vanilla", "join", "player_position")[0], pk("rust", "join", "player_position")[0]
rec("identyczne po zamaskowaniu pól losowych", "play/player_position (bez losowych x,z)", pv[8:16] + pv[24:] == pr[8:16] + pr[24:])
av, ar = mine("vanilla", "update_attributes", ev), mine("rust", "update_attributes", er)
def attrset(b):
    r = b[0]; out = {}; i = 1
    for _ in range(r):
        a, i = varint(b, i); out[a] = b[i:i+8]; i += 8; m, i = varint(b, i)
        for _ in range(m):  # AttributeModifier: identyfikator (napis), wartość (f64), operacja (VarInt)
            n, i = varint(b, i); i += n + 8; _op, i = varint(b, i)
    return out
rec("zgodne semantycznie (kolejność z HashMap Javy)", "play/update_attributes gracza (te same atrybuty i wartości)", bool(av) and bool(ar) and attrset(av[0]) == attrset(ar[0]))

# --- chunki
def chunks(tag):
    d = {}
    for b in pk(tag, "join", "level_chunk_with_light"):
        d[struct.unpack(">ii", b[:8])] = b
    return d
cv, cr = chunks("vanilla"), chunks("rust")
common = sorted(set(cv) & set(cr))
same = sum(1 for k in common if cv[k] == cr[k])
rec("bajty identyczne", "play/level_chunk_with_light (heightmapy, sekcje, światło)", bool(common) and same == len(common), f"{same}/{len(common)} wspólnych chunków")

# --- czat, komendy
def strip_ts(b):
    k = b.index(b"witaj") + 5
    return b[:k] + b[k+8:]
rec("bajty identyczne", "czat: player_chat (poza znacznikiem czasu serwera)", strip_ts(pk("vanilla", "chat", "player_chat")[0]) == strip_ts(pk("rust", "chat", "player_chat")[0]))
rec("bajty identyczne", "komenda nieznana: 2x system_chat (komunikat + kontekst)", pk("vanilla", "cmd", "system_chat") == pk("rust", "cmd", "system_chat"))
rec("bajty identyczne", "/list: system_chat", pk("vanilla", "list", "system_chat") == pk("rust", "list", "system_chat"))
# --- wersja protokołu
for p in (700, 760):
    rec("bajty identyczne", f"odrzucenie klienta z protokołem {p}", pk("vanilla", f"p{p}", "login_disconnect", 0) == pk("rust", f"p{p}", "login_disconnect", 0))
# --- bloki
keep = {"block_update", "block_changed_ack", "container_set_slot", "section_blocks_update"}
def norm(tag, case):
    out = []
    for s, i, b in load(f"{D}/{tag}_{case}.bin"):
        if s == 2 and nm(s, i) in keep:
            n = nm(s, i)
            out.append((n, b[8:] if n == "block_update" else (b[8:9] if n == "section_blocks_update" else b)))
    return out
def trim(lst):  # vanilla czasem dodaje zmiany z losowych ticków (np. trawa -> ziemia); odetnij ogon po ack
    names = [n for n, _ in lst]
    return lst[: names.index("block_changed_ack") + 1] if "block_changed_ack" in names else lst
for case, desc in (("break", "łamanie bloku w kreatywnym"), ("place", "stawianie bloku (odrzucone: kolizja z graczem)"), ("beside", "stawianie bloku (udane)"), ("multi", "łamanie + stawianie w jednym ticku (section_blocks_update)")):
    ok = all(trim(norm("vanilla", f"{case}{k}")) == trim(norm("rust", f"{case}{k}")) for k in (1, 2, 3) if os.path.exists(f"{D}/vanilla_{case}{k}.bin"))
    rec("kolejność i bajty zgodne (poza pozycją)", f"{desc}: container_set_slot/block_update/ack", ok)

# --- raport
w = max(len(r[1]) for r in results)
cat = None
for c, d, s, det in results:
    if c != cat: print(f"\n## {c}"); cat = c
    print(f"  [{'OK ' if s == 'OK' else 'RÓŻNICA'}] {d:{w}}  {det}")
bad = [r for r in results if r[2] != "OK"]
print(f"\nWYNIK: {len(results) - len(bad)} / {len(results)} zgodnych z vanilla")
print("""
Znane różnice (jeszcze niezaimplementowane): update_recipes (ok. 108 KB w vanilla), commands, recipe (książka receptur),
update_advancements, moby i pozostałe encje (add_entity, move_entity_*, set_equipment...), update_tags (inna kolejność z
HashMap — zawartość identyczna wg tools/verify.sh), losowe: pozycja startowa, czas gry, ID encji.""")
sys.exit(1 if bad else 0)
