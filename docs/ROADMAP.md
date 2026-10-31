# Mapa drogowa portu

Rozmiar vanilla 1.21.1 (zdekompilowane, `net.minecraft.*`): **4053 pliki, ~470 000 linii**. Podział (linie):

| Podsystem | Pliki | Linie |
|---|---:|---:|
| `world/entity` (encje, moby, AI, gracz) | 594 | 94 049 |
| `world/level/block` (zachowania bloków, encje bloków) | 418 | 58 221 |
| `world/level/levelgen` (generowanie świata) | 418 | 42 113 |
| `world/item` (przedmioty, komponenty) | 255 | 24 946 |
| `network` (protokół) | 380 | 23 899 |
| `server/commands` + `commands` (Brigadier) | 208 | 26 227 |
| `world/level/storage` (zapis świata) | 145 | 10 259 |
| `server/level` | 38 | 9 889 |
| `advancements` | 111 | 8 078 |
| `world/level/chunk` | 53 | 7 317 |
| `world/inventory` | 60 | 6 282 |
| `nbt` | 40 | 5 829 |
| `world/level/lighting` | 15 | 2 304 |

Metoda dla każdego etapu: **(1)** odczytać zdekompilowany kod z oficjalnymi nazwami, **(2)** przenieść logikę,
**(3)** udowodnić zgodność testem różnicowym względem prawdziwych klas Mojang (jak `tools/verify/Verify.java`:
harness w Javie uruchamia oryginalny kod na tych samych wejściach i porównuje wyniki), **(4)** dopiero potem dalej.
Tam, gdzie logika jest *sterowana danymi* (density functions, noise settings, loot tables, enchantmenty, receptury,
advancementy), portujemy interpreter danych, nie każdą klasę osobno.

## Etapy

**M1 ✅ Fundament sieciowy** (ten commit): handshake/status/login (offline+online)/configuration/play (dołączanie,
chunki, ruch graczy, czat), weryfikacja prawdziwymi kodekami.

**M2 — Świat na dysku.** Czytnik/zapis Anvil (`.mca`, zlib/LZ4/brak), NBT chunków ↔ sekcje (nazwa+właściwości bloku ↔
ID stanu z `blocks.json`), `level.dat`, `playerdata/*.dat`, `level-name`/`level-seed`. Wynik: wczytanie istniejącego
świata serwera Java 1:1 (gotowy teren bez generatora). Weryfikacja: round-trip na świecie wygenerowanym przez Mojang.

**M3 — Tabele właściwości bloków i przedmiotów.** `blocks.json` nie zawiera twardości, kolizji, przezroczystości
dla światła, emisji, płynów. Mały program w Javie (uruchamiający prawdziwe klasy `Block`/`BlockState` po `Bootstrap`)
zrzuca je per stan do pliku, z którego generowane są tablice Rusta. To samo dla przedmiotów (komponenty domyślne,
maksymalny stack).

**M4 — Silnik światła.** Port `world/level/lighting` (BFS sky/block light, `DataLayer`), pełne heightmapy
(`MOTION_BLOCKING`, `OCEAN_FLOOR`, …) zamiast uproszczonych. Zastępuje uproszczone światło nieba.

**M5 — Generowanie świata** (największe ryzyko zgodności seedów). Generatory liczb losowych
(`LegacyRandomSource`, `XoroshiroRandomSource`), szumy (`ImprovedNoise`, `PerlinNoise`, `SimplexNoise`,
`BlendedNoise`), interpreter grafu density functions i `NoiseRouter` (dane w JSON), biome source (multi-noise),
surface rules, karvery, features/placed features, struktury. Weryfikacja: ten sam seed → te same bloki co prawdziwy
`NoiseBasedChunkGenerator` w harnessie Javy, chunk po chunku.

**M6 — Encje i fizyka.** `Entity`/`LivingEntity`/`Player`, AABB i `VoxelShape` (kolizje), walidacja ruchu
(`ServerGamePacketListenerImpl.handleMovePlayer`: „moved too quickly/wrongly"), atrybuty, efekty, obrażenia,
śmierć/odrodzenie, metadane i śledzenie encji, przedmioty na ziemi, orby.

**M7 — Zachowania bloków.** `getStateForPlacement`, aktualizacje sąsiadów, losowe ticki, płyny, redstone,
drzwi/skrzynie/piece itd. (encje bloków + NBT), kopanie w przetrwaniu z prawdziwymi czasami (po M3).

**M8 — Przedmioty, ekwipunek, crafting.** Komponenty danych (57 typów) i ich kodeki, menu/kontenery
(`world/inventory`), receptury (`ClientboundUpdateRecipesPacket` + książka receptur), loot tables (interpreter danych),
enchantmenty (efekty sterowane danymi).

**M9 — Komendy i administracja.** Port Brigadier, drzewo komend (`commands.json` → pakiet `commands`), wszystkie komendy,
`ops.json`/`whitelist.json`/`banned-*.json`, poziomy uprawnień, konsola serwera.

**M10 — Moby i AI.** Cele (goal selectors), nawigacja/pathfinding, spawnowanie, handel, rozmnażanie, bossowie.

**M11 — Wymiary i struktury.** Nether/End, portale, struktury z szablonów (`StructureTemplate`, jigsaw).

**M12 — Reszta infrastruktury.** Podpisany czat (`enforce-secure-profile`), advancementy, tablice wyników, boss bary,
mapy, statystyki, resource pack, RCON/query, transfery, wielowątkowe generowanie/ładowanie chunków (rayon).

## Weryfikacja czarnoskrzynkowa (wymaga zgody użytkownika)

Najlepszy test „1:1" to uruchomienie **prawdziwego serwera vanilla** obok serwera Rust i porównywanie sekwencji pakietów
dla identycznych zdarzeń. Wymaga to zaakceptowania EULA Mojang (`eula=true`) przez właściciela środowiska — narzędzia
w tym repo tego nie robią same. Do czasu zgody całą weryfikację wykonuje się na klasach Mojang (bez uruchamiania serwera),
co pokrywa kodeki, rejestry, tagi i chunki, ale nie zachowanie gry w czasie.
