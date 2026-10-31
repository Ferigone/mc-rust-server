# mc-rust-server

Port serwera **Minecraft: Java Edition 1.21 / 1.21.1** (protokół **767**) do Rusta. Docelowo: zachowanie 1:1
z serwerem vanilla, tak by zwykły klient Java 1.21 łączył się bez modyfikacji.

> **Stan: kamień milowy 1 z ok. 12 (patrz [docs/ROADMAP.md](docs/ROADMAP.md)).** To fundament, nie kompletny port.
> Vanilla to ~4 000 klas i ~470 000 linii kodu; poniżej opisano dokładnie, co już działa i jest zweryfikowane,
> a czego jeszcze nie ma. Nie testowano z prawdziwym klientem Minecrafta (w środowisku budowy go nie ma) —
> tylko z oryginalnymi dekoderami Mojang i w teście różnicowym z prawdziwym serwerem vanilla (patrz niżej).

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
* Postawiony blok dostaje stan domyślny (bez `getStateForPlacement`), przedmiot → blok po nazwie; kolizja z graczem liczona jak z pełną kostką.
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
na maszynę użytkownika. Uruchamianie oryginalnego serwera wymaga zaakceptowania EULA Mojang — jedynie `tools/vanilla_compare.sh` go uruchamia
i robi to wyłącznie przy jawnym `MC_EULA_ACCEPTED=true`; żadne narzędzie niczego nie akceptuje w Twoim imieniu.

## Narzędzia REA (inżynieria odwrotna)

| Skrypt | Co robi |
|---|---|
| `tools/fetch_reference.sh` | Pobiera `server.jar` i mapowania oficjalnych nazw, weryfikuje SHA1, uruchamia `net.minecraft.data.Main --all` (rejestry, ID pakietów, stany bloków, komendy, datapacki). |
| `tools/decompile.sh` | Wyjmuje serwer z bundlera, nakłada mapowania Mojang (tiny-remapper) i dekompiluje Vineflowerem do `reference/src` (4053 pliki z oficjalnymi nazwami). |
| `tools/gen_packet_ids.py` | Generuje `crates/mc-protocol/src/ids.rs` z raportu `packets.json` — **żadne ID pakietu nie jest wpisane ręcznie**. |
| `tools/verify.sh` | Przepuszcza zrzut pakietów serwera Rust przez **prawdziwe** kodeki Mojang (patrz niżej). |
| `tools/e2e.sh` | Uruchamia serwer, boty i całą weryfikację jednym poleceniem. |
| `tools/vanilla_compare.sh` | Test różnicowy z prawdziwym serwerem vanilla (wymaga `MC_EULA_ACCEPTED=true`). |
| `tools/cmp_dumps.py`, `tools/cmp_chunk.py` | Porównanie zrzutów pakietów / rozbiór chunka część po części. |

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

## Test różnicowy z prawdziwym serwerem vanilla

`MC_EULA_ACCEPTED=true tools/vanilla_compare.sh` uruchamia **oryginalny serwer vanilla 1.21.1** obok serwera Rust
(ten sam seed, tryb kreatywny, płaski świat, offline), wykonuje te same zdarzenia botem i porównuje pakiety.
Skrypt zapisze `eula=true` tylko przy jawnej zmiennej `MC_EULA_ACCEPTED=true` (zgoda właściciela na EULA Mojang).
Ostatni wynik: **36 / 36 zgodnych**:

* **Bajt w bajt identyczne:** status JSON (kolejność kluczy zmierzona na vanilla), kompresja, `game_profile`, brand,
  feature flags, known packs, **wszystkie 11 pakietów `registry_data`**, `finish_configuration`, trudność, uprawnienia,
  wybrany slot, `server_data`, `player_info_update` (w tym pusty pakiet), granica świata, spawn, `game_event`,
  `ticking_state/step`, zawartość ekwipunku, zdrowie, doświadczenie, **wszystkie chunki** (heightmapy, sekcje wraz z kolejnością
  palety, maski i tablice światła — 109/109), czat, komunikat o nieznanej komendzie (z kontekstem `<--[HERE]`), `/list`,
  odrzucenie klienta ze złym protokołem (obie gałęzie komunikatu).
* **Identyczne po zamaskowaniu pól losowych/zależnych od sesji:** `login` (ID encji, kolejność wymiarów z `HashSet`),
  `entity_event`, metadane własne gracza, `player_position` (pozycja startowa jest losowa w promieniu `spawnRadius`).
* **Zachowanie świata:** łamanie bloku, stawianie bloku (udane i odrzucone przez kolizję z graczem), kilka zmian w jednej
  sekcji (`section_blocks_update`) — te same pakiety w tej samej kolejności co w vanilla: echo slotu, dwa jawne
  `block_update`, rozgłoszenie zmiany, potwierdzenie sekwencji na początku ticka.

Różnice zmierzone testem, które **nie** są jeszcze usunięte: `update_recipes` (ok. 108 KB w vanilla, u nas pusty),
`commands` (drzewo komend), `recipe` (książka receptur), `update_advancements`, moby i pozostałe encje, `update_tags`
(ta sama zawartość ID co w vanilla — sprawdzone `tools/verify.sh` — ale inna kolejność zapisu z `HashMap`),
losowe mechaniki (np. trawa przykryta kamieniem zamienia się w ziemię przy losowym ticku).

Test ujawnił i naprawił m.in.: kolejność kluczy NBT (emulacja iteracji `java.util.HashMap`), strukturę komunikatów błędów komend,
liczby zamiast tekstów w `/list`, obcinanie warstw światła (vanilla trzyma je tylko do sekcji nad najwyższą niepustą),
kolejność palety w sekcji, hashed seed (SHA-256), listę wymiarów, `ticking_*`, atrybuty gracza, stałą granicy świata.

## Architektura

* `crates/mc-protocol` — typy protokołu, NBT, ramkowanie (kompresja + szyfrowanie), pakiety.
* `crates/mc-data` — dane z oficjalnego generatora: rejestry, stany bloków, wpisy datapacków w kolejności
  vanilla (`TreeMap` po `ResourceLocation`), tagi.
* `crates/mc-server` — sieć (tokio), wątek gry 20 TPS (jak główny wątek vanilla: jedna pętla, deterministycznie),
  świat, uwierzytelnianie.
* `crates/mc-probe` — bot do testów.

Ramki pakietów są budowane (i kompresowane) raz i współdzielone między odbiorcami (`Arc`); chunki są cache'owane
jako gotowe ramki; zapis do gniazda scala wiele ramek w jeden `write`.
