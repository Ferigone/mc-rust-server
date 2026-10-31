# mc-rust-server

A port of the **Minecraft: Java Edition 1.21 / 1.21.1** server (protocol **767**) to Rust. The goal is 1:1 behaviour
with the vanilla server, so that an unmodified Java 1.21 client can connect and play.

> **Status: milestone 1 of about 12 (see [docs/ROADMAP.md](docs/ROADMAP.md)).** This is a foundation, not a complete port.
> Vanilla is ~4,000 classes and ~470,000 lines of code; this document states exactly what works and has been verified,
> and what is still missing. It has not been tested with a real Minecraft client (none is available in the build
> environment); it has been tested against Mojang's own decoders and in a differential test against the real vanilla
> server (see below).

## What works

| Area | State |
|---|---|
| Handshake, status (server list, ping), zlib compression, AES-128-CFB8 | ✅ |
| Offline login (UUID `OfflinePlayer:<name>` like vanilla) and **online** login (RSA-1024, Mojang session) | ✅ (online mode verified against a mock session server, not against real Mojang) |
| Configuration: brand, feature flags, known packs, 11 synchronized registries, 514 tags | ✅ |
| Registries with full NBT for clients without a matching pack (e.g. 1.21 client vs 1.21.1 server) | ✅ |
| Play: Login, position, chunk batches (`PlayerChunkSender` algorithm), chunk tracking view (`ChunkTrackingView`) | ✅ |
| Keep-alive, chat (unsigned), `/list`, visibility and movement of other players (`ServerEntity.sendChanges`) | ✅ |
| Block breaking/placing (simplified), superflat world, 20 TPS | ⚠️ simplified |
| `server.properties` (same keys and defaults as vanilla) | ✅ read/write |

## What is still missing (honestly)

Normal world generation, loading/saving Anvil worlds (`.mca`), `level.dat`, player data, a full light engine
(only simplified sky light exists), non-player entities (mobs, items, AI), physics/collisions, inventory and crafting
(recipes are sent empty), block behaviour (placement states, redstone, fluids), commands other than `/list`
(and the command tree), ops/whitelist/bans, signed chat, advancements, scoreboards, Nether/End, RCON/query.
Everything is itemised in [docs/ROADMAP.md](docs/ROADMAP.md).

### Deliberate deviations from vanilla (until they are removed)

* `enforce-secure-profile` is ignored (always "false"): chat is unsigned.
* In survival mode a block break on `STOP_DESTROY_BLOCK` is accepted without verifying the mining time.
* A placed block gets its default state (no `getStateForPlacement`), item → block by name; collision with players is computed as a full cube.
* No movement speed validation (anti-cheat) apart from rejecting NaN/infinity/out-of-world values.
* No persistence: the world and inventories are empty after a restart; the spawn is fixed at (0.5, −60, 0.5).
* The server brand is `vanilla`.

## Building and running

Requirements: Rust ≥ 1.85, Java 21 (reference tooling only), `curl`, `python3`.

```bash
tools/fetch_reference.sh      # downloads the 1.21.1 server from Mojang (SHA1-verified), runs the OFFICIAL data generator
cargo build --release
mkdir -p run && ./target/release/mc-server --dir run   # creates run/server.properties
```

`mc-server` looks for the data in `reference/gen/generated` (or `--data <dir>` / `MC_DATA_DIR`).
For offline mode set `online-mode=false` in `run/server.properties`.

**Mojang files are not part of this repository** (`reference/` is in `.gitignore`): the scripts download them from
Mojang's servers onto the user's machine. Running the original server requires accepting the Mojang EULA — only
`tools/vanilla_compare.sh` runs it, and only with an explicit `MC_EULA_ACCEPTED=true`; no tool accepts anything on your behalf.

## Reverse-engineering tooling

| Script | What it does |
|---|---|
| `tools/fetch_reference.sh` | Downloads `server.jar` and the official name mappings, verifies SHA1, runs `net.minecraft.data.Main --all` (registries, packet IDs, block states, commands, data packs). |
| `tools/decompile.sh` | Extracts the server from the bundler, applies the Mojang mappings (tiny-remapper) and decompiles with Vineflower into `reference/src` (4053 files with official names). |
| `tools/gen_packet_ids.py` | Generates `crates/mc-protocol/src/ids.rs` from the `packets.json` report — **no packet ID is typed by hand**. |
| `tools/verify.sh` | Feeds a packet dump from the Rust server through the **real** Mojang codecs (see below). |
| `tools/e2e.sh` | Starts the server and bots and runs the whole verification with one command. |
| `tools/vanilla_compare.sh` | Differential test against the real vanilla server (requires `MC_EULA_ACCEPTED=true`). |
| `tools/cmp_dumps.py`, `tools/cmp_chunk.py` | Compare packet dumps / take a chunk apart piece by piece. |

The field layout of every packet was reconstructed from the decompiled code (`net.minecraft.network.protocol.*`),
not from memory; the same holds for the join sequence (`PlayerList.placeNewPlayer`), configuration
(`ServerConfigurationPacketListenerImpl`), login and chunk sending.

## Verification

`mc-probe` is a protocol bot (`status`, `join`, `mock-session`). `tools/e2e.sh` records the raw packets sent by the
Rust server and feeds them through the **original Mojang classes** loaded from the server jar (`tools/verify/Verify.java`):

* every login/configuration/play packet is decoded with the real codec and must consume the whole buffer,
* registries are loaded through the client path (`RegistryDataLoader.load` from the network) and compared **object by
  object** (codec → JSON) with the registries loaded by the vanilla server — also when entries are sent with full NBT,
* the order and IDs of registry entries are identical to the vanilla loader,
* **514 tags in 13 registries** have the same IDs as `TagNetworkSerialization` computes on the real data,
* every chunk is read with `LevelChunkSection.read` and its blocks/biome are checked as real `BlockState`/`Biome`,
* block IDs in the tests come from the `blocks.json` generated by Mojang, not from the Rust server code.

In addition: unit tests (`cargo test --workspace`) with vectors confirmed on real Java (offline UUID, `BlockPos.asLong`,
`ChunkTrackingView` chunk counts) and a load test: 100 concurrent bots (~110 chunks each) — all of them passed every
check; the server used about 3 s of CPU and ~114 MiB of RAM at peak (release build, 4 cores; superflat world, so this
is not a comparison with terrain generation).

## Differential test against the real vanilla server

`MC_EULA_ACCEPTED=true tools/vanilla_compare.sh` starts the **original vanilla 1.21.1 server** next to the Rust server
(same seed, creative mode, flat world, offline), performs the same events with a bot and compares the packets.
The script writes `eula=true` only when `MC_EULA_ACCEPTED=true` is set explicitly (the owner's acceptance of the Mojang EULA).
Latest result: **36 / 36 matching**:

* **Byte-for-byte identical:** status JSON (key order measured on vanilla), compression, `game_profile`, brand,
  feature flags, known packs, **all 11 `registry_data` packets**, `finish_configuration`, difficulty, abilities,
  selected slot, `server_data`, `player_info_update` (including the empty packet), world border, spawn, `game_event`,
  `ticking_state/step`, inventory contents, health, experience, **all chunks** (heightmaps, sections including palette
  order, light masks and arrays — 109/109), chat, the unknown-command message (with the `<--[HERE]` context), `/list`,
  rejection of a client with the wrong protocol (both message branches).
* **Identical after masking random/session-specific fields:** `login` (entity ID, dimension order from a `HashSet`),
  `entity_event`, the player's own metadata, `player_position` (the start position is random within `spawnRadius`).
* **World behaviour:** block breaking, block placing (successful and rejected because of collision with the player), several
  changes in one section (`section_blocks_update`) — the same packets in the same order as vanilla: slot echo, two explicit
  `block_update` packets, change broadcast, sequence acknowledgement at the start of the tick.

Differences measured by the test that are **not** removed yet: `update_recipes` (~108 KB in vanilla, empty here),
`commands` (command tree), `recipe` (recipe book), `update_advancements`, mobs and other entities, `update_tags`
(same ID content as vanilla — checked by `tools/verify.sh` — but a different `HashMap` write order), and random mechanics
(e.g. grass covered by stone turns into dirt on a random tick).

The test found and fixed, among others: NBT key order (emulation of `java.util.HashMap` iteration), the structure of
command error messages, numbers instead of strings in `/list`, light layer truncation (vanilla keeps them only up to the
section above the highest non-empty one), section palette order, hashed seed (SHA-256), the dimension list, `ticking_*`,
player attributes and the world border constant.

## Architecture

* `crates/mc-protocol` — protocol types, NBT, framing (compression + encryption), packets.
* `crates/mc-data` — data from the official generator: registries, block states, data-pack entries in vanilla
  order (`TreeMap` by `ResourceLocation`), tags.
* `crates/mc-server` — networking (tokio), a 20 TPS game thread (like the vanilla main thread: a single deterministic
  loop), world, authentication.
* `crates/mc-probe` — test bot.

Packet frames are built (and compressed) once and shared between recipients (`Arc`); chunks are cached as ready-made
frames; socket writes coalesce many frames into one `write`.

## Language policy

This is an open-source project: documentation, code comments, log messages, commit messages, pull request titles and
descriptions are written in English.
