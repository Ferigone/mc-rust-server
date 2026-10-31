# mc-rust-server

Port serwera **Minecraft: Java Edition 1.21 / 1.21.1** (protokół **767**) do Rusta. Docelowo: zachowanie 1:1
z serwerem vanilla, tak by zwykły klient Java 1.21 łączył się bez modyfikacji.

> **Stan: kamień milowy 1 z ok. 12 (patrz [docs/ROADMAP.md](docs/ROADMAP.md)).** To fundament, nie kompletny port.
> Vanilla to ~4 000 klas i ~470 000 linii kodu; poniżej opisano dokładnie, co już działa i jest zweryfikowane,
> a czego jeszcze nie ma. Nie testowano z prawdziwym klientem Minecrafta (w środowisku budowy go nie ma) —
> wyłącznie z oryginalnymi dekoderami Mojang (patrz „Weryfikacja").

## Co działa

| Obszar | Stan |
|---|---|
| Handshake, status (lista serwerów, ping), kompresja zlib, AES-128-CFB8 | ✅ |
| Login offline (UUID `OfflinePlayer:<nick>` jak vanilla) i **online** (RSA-1024, sesja Mojang) | ✅ (online sprawdzone na atrapie serwera sesji, nie na prawdziwym Mojangu) |
| Configuration: brand, feature flags, known packs, 11 rejestrów synchronizowanych, 514 tagów | ✅ |
| Rejestry z pełnym NBT dla klientów bez pasującej paczki (np. klient 1.21 vs serwer 1.21.1) | ✅ |
| Play: Login, pozycja, chunki w paczkach (algorytm `PlayerChunkSender`), widok chunków (`ChunkTrackingView`) | ✅ |
| Keepalive, czat (niepodpisany), `/list`, widoczność i ruch innych graczy (`ServerEntity.sendChanges`) | ✅ |
| Łamanie/stawianie bloków (uproszczone), świat superflat, 20 TPS | ⚠️ uproszczone |
| `server.properties` (te same klucze i domyślne co vanilla) | ✅ odczyt/zapis |

## Czego jeszcze nie ma (uczciwie)

Generator świata normalnego, wczytywanie/zapis świata Anvil (`.mca`), `level.dat`, dane graczy, silnik światła
(jest uproszczone światło nieba), encje poza graczami (moby, przedmioty, AI), fizyka/kolizje, ekwipunek i crafting
(receptury wysyłane puste), zachowania bloków (stany przy stawianiu, redstone, płyny), komendy poza `/list`
(i drzewo komend), ops/whitelist/bany, podpisany czat, zdobycia, tablice wyników, Nether/End, RCON/query. Wszystko
rozpisane w [docs/ROADMAP.md](docs/ROADMAP.md).

### Świadome odstępstwa od vanilla (do czasu ich usunięcia)

* `enforce-secure-profile` jest ignorowane (zawsze „false"): czat jest niepodpisany.
* W trybie przetrwania łamanie bloku po `STOP_DESTROY_BLOCK` jest przyjmowane bez weryfikacji czasu kopania.
* Postawiony blok dostaje stan domyślny (bez `getStateForPlacement`), przedmiot → blok po nazwie.
* Brak walidacji prędkości ruchu (anty-cheat) poza odrzuceniem NaN/nieskończoności/wartości poza światem.
* Brak trwałości: po restarcie świat i ekwipunki są puste; spawn stały (0.5, −60, 0.5).
* Brand serwera to `vanilla`.

## Budowanie i uruchomienie

Wymagane: Rust ≥ 1.85, Java 21 (tylko do narzędzi referencyjnych), `curl`, `python3`.

```bash
tools/fetch_reference.sh      # pobiera serwer 1.21.1 od Mojang (SHA1), uruchamia OFICJALNY data generator
cargo build --release
mkdir -p run && ./target/release/mc-server --dir run   # utworzy run/server.properties
```

`mc-server` szuka danych w `reference/gen/generated` (albo `--data <katalog>` / `MC_DATA_DIR`).
Dla trybu offline ustaw w `run/server.properties`: `online-mode=false`.

**Pliki Mojang nie są w repozytorium** (`reference/` jest w `.gitignore`): skrypty pobierają je z serwerów Mojang
na maszynę użytkownika. Uruchamianie oryginalnego serwera wymaga zaakceptowania EULA Mojang — narzędzia tego repo
**nie** uruchamiają serwera vanilla i niczego nie akceptują w Twoim imieniu.

## Narzędzia REA (inżynieria odwrotna)

| Skrypt | Co robi |
|---|---|
| `tools/fetch_reference.sh` | Pobiera `server.jar` i mapowania oficjalnych nazw, weryfikuje SHA1, uruchamia `net.minecraft.data.Main --all` (rejestry, ID pakietów, stany bloków, komendy, datapacki). |
| `tools/decompile.sh` | Wyjmuje serwer z bundlera, nakłada mapowania Mojang (tiny-remapper) i dekompiluje Vineflowerem do `reference/src` (4053 pliki z oficjalnymi nazwami). |
| `tools/gen_packet_ids.py` | Generuje `crates/mc-protocol/src/ids.rs` z raportu `packets.json` — **żadne ID pakietu nie jest wpisane ręcznie**. |
| `tools/verify.sh` | Przepuszcza zrzut pakietów serwera Rust przez **prawdziwe** kodeki Mojang (patrz niżej). |
| `tools/e2e.sh` | Uruchamia serwer, boty i całą weryfikację jednym poleceniem. |

Układ pól każdego pakietu został odtworzony ze zdekompilowanego kodu (`net.minecraft.network.protocol.*`),
a nie z pamięci; to samo dotyczy sekwencji dołączania (`PlayerList.placeNewPlayer`), konfiguracji
(`ServerConfigurationPacketListenerImpl`), logowania i wysyłki chunków.

## Weryfikacja

`mc-probe` to bot protokołu (`status`, `join`, `mock-session`). `tools/e2e.sh` zapisuje surowe pakiety wysłane przez
serwer Rust i przepuszcza je przez **oryginalne klasy Mojang** wczytane z jara serwera (`tools/verify/Verify.java`):

* każdy pakiet login/configuration/play dekodowany jest prawdziwym kodekiem i musi zużyć cały bufor,
* rejestry ładowane są ścieżką klienta (`RegistryDataLoader.load` z sieci) i porównywane **obiekt po obiekcie**
  (kodek → JSON) z rejestrami wczytanymi przez serwer vanilla — także gdy wpisy idą z pełnym NBT,
* kolejność i ID wpisów rejestrów są identyczne z loaderem vanilla,
* **514 tagów w 13 rejestrach** ma identyczne ID jak `TagNetworkSerialization` na prawdziwych danych,
* każdy chunk czytany jest przez `LevelChunkSection.read`, a bloki/biom sprawdzane jako prawdziwe `BlockState`/`Biome`,
* ID bloków w testach pochodzą z `blocks.json` wygenerowanego przez Mojang, nie z kodu serwera Rust.

Dodatkowo: testy jednostkowe (`cargo test --workspace`) z wektorami potwierdzonymi na prawdziwej Javie
(UUID offline, `BlockPos.asLong`, liczby chunków `ChunkTrackingView`) oraz test obciążeniowy: 100 równoległych botów
(po ~110 chunków każdy) — wszystkie zaliczyły wszystkie sprawdzenia; serwer zużył ok. 3 s CPU i ~114 MiB RAM
w szczycie (build release, 4 rdzenie; świat superflat, więc to nie jest porównanie z generowaniem terenu).

## Architektura

* `crates/mc-protocol` — typy protokołu, NBT, ramkowanie (kompresja + szyfrowanie), pakiety.
* `crates/mc-data` — dane z oficjalnego generatora: rejestry, stany bloków, wpisy datapacków w kolejności
  vanilla (`TreeMap` po `ResourceLocation`), tagi.
* `crates/mc-server` — sieć (tokio), wątek gry 20 TPS (jak główny wątek vanilla: jedna pętla, deterministycznie),
  świat, uwierzytelnianie.
* `crates/mc-probe` — bot do testów.

Ramki pakietów są budowane (i kompresowane) raz i współdzielone między odbiorcami (`Arc`); chunki są cache'owane
jako gotowe ramki; zapis do gniazda scala wiele ramek w jeden `write`.
