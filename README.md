# mc-rust-server

A port of the **Minecraft: Java Edition 1.21 / 1.21.1** server (protocol **767**) to Rust. The goal is 1:1 behaviour
with the vanilla server, so that an unmodified Java 1.21 client can connect and play.

> **Status: milestone 5 of about 12 is under way (world generation: terrain shape, biomes, surface rules and carvers are done and run in the server; features / structures are not) (see [docs/ROADMAP.md](docs/ROADMAP.md)).** This is a foundation, not a complete port.
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
| Block breaking/placing (simplified), 20 TPS | ⚠️ simplified |
| **Light engine** (block + sky light, section storage rules, face-shape occlusion) and real heightmaps for generated / edited chunks | ✅ identical to the vanilla server on every stored layer of a vanilla-generated world (see below); incremental `light_update` packets after edits: not yet |
| **Terrain shape** (`crates/mc-worldgen`): random sources, all noise functions, the data-driven density function graph with splines, the wired `NoiseRouter`, `NoiseChunk` (cell interpolation, caches), the aquifer (water / lava lakes) and ore veins, for every vanilla noise setting (overworld, amplified, large biomes, nether, end, caves, floating islands) | ✅ bit-identical to the Mojang classes: 878 random / noise values, 5,760 router values, 34 generated chunks; wired into the server (see below) |
| **Biome source** (`crates/mc-worldgen`): `Climate` (quantized parameters, the parameter-space R-tree with its thread-local shortcut, the sampler, the world spawn search), the overworld parameter table (`OverworldBiomeBuilder`, 7,593 entries), the nether preset and the End island biomes | ✅ identical to the Mojang classes: all list entries, 160,000 R-tree searches including exact ties, 79,300 biomes sampled from 13 seeds / settings, 9 spawn searches; wired into the server (see below) |
| **Surface stage** (`crates/mc-worldgen`): `SurfaceSystem` and `SurfaceRules` compiled from the `surface_rule` JSON (all 11 conditions, bandlands, bedrock floor / roof, noise thresholds), clay bands, eroded badlands pillars, frozen-ocean icebergs, the `BiomeManager` zoom (SHA-256 seed, fiddled distances), the biome temperature (`PerlinSimplexNoise`, frozen modifier), the `WORLD_SURFACE_WG` heightmap | ✅ identical to the Mojang classes: 223 chunks (the first chunk of each of 63 biomes, 9 noise settings / seeds incl. nether and End), every block and the heightmap; wired into the server (see below) |
| **Carvers** (`crates/mc-worldgen/src/carver.rs`): `CaveWorldCarver`, `CanyonWorldCarver`, `NetherWorldCarver`, `CarvingMask`, the 17x17-chunk `applyCarvers` loop with `setLargeFeatureSeed`, float/height providers, `Mth.sin/cos` tables, the re-grassing of dirt below a cave roof (`SurfaceSystem.topMaterial`) | ✅ identical to the Mojang classes: 223 chunks of 9 settings (overworld, amplified, large biomes, nether, ...) after surface and carvers |
| **World generation in the server** (`crates/mc-server/src/worldgen.rs`): new worlds with `level-type` normal / large_biomes / amplified (and worlds of the vanilla server with a noise generator) get the ported terrain, biomes, surface and carvers (caves, canyons) for every chunk that is not stored in the region files; generation runs on background threads, a chunk is sent once it and its eight neighbours exist; the initial world spawn follows `MinecraftServer.setInitialSpawn` (`Climate` spawn search, then the first suitable column of the chunk spiral) | ✅ works (the probe joins a generated world and decodes the chunks); the terrain is exact only up to the surface stage, see "What is still missing" |
| Block placement uses the real item→block mapping, replaceable blocks, and the real collision shapes against player hitboxes | ✅ (default block state only until `getStateForPlacement`, M7) |
| **Worlds on disk**: Anvil regions (`.mca`, zlib/gzip/LZ4/uncompressed, external `.mcc`), `level.dat`, `playerdata/*.dat` (position, game mode, health, food, XP, inventory), stored heightmaps/light/block entities, `level-name`/`level-seed`, autosave, graceful save on Ctrl+C/SIGTERM | ✅ loads worlds of the vanilla server and writes worlds the vanilla server loads (see below) |
| `server.properties` (same keys and defaults as vanilla) | ✅ read/write |

## Running it

* Locally: `tools/fetch_reference.sh` (downloads the official server from Mojang and runs its data generator into the git-ignored `reference/`), then `cargo run --release -p mc-server`.
* In a container / on Railway: see [docs/DEPLOY.md](docs/DEPLOY.md) (`Dockerfile`, `railway.toml`).

## What is still missing (honestly)

The last stages of normal world generation: features (trees, ores, flowers, lakes, ...), structures (villages, strongholds, ...)
and the `Beardifier`: new normal worlds already have the exact terrain, biomes, surface blocks, caves and canyons of vanilla, but no trees, ores or villages yet,
and the Nether and End are not reachable. Chunk generation is also not optimised yet (about 30 chunks per second on 4 cores). `light_update` packets after block edits (edited chunks are saved without light and relit by vanilla), non-player entities (mobs, items, AI), physics/collisions, inventory and crafting
(recipes are sent empty), block behaviour (placement states, redstone, fluids), commands other than `/list`
(and the command tree), ops/whitelist/bans, signed chat, advancements, scoreboards, Nether/End, RCON/query.
Everything is itemised in [docs/ROADMAP.md](docs/ROADMAP.md).

### Deliberate deviations from vanilla (until they are removed)

* `enforce-secure-profile` is ignored (always "false"): chat is unsigned.
* In survival mode a block break on `STOP_DESTROY_BLOCK` is accepted without verifying the mining time.
* A placed block gets its default state (no `getStateForPlacement`, no `canSurvive`); only players are checked for collision (other entities: M6).
* No movement speed validation (anti-cheat) apart from rejecting NaN/infinity/out-of-world values.
* A chunk edited by the Rust server is saved without its light and heightmaps and with `isLightOn=0` (vanilla relights it on load); nothing else of a chunk (entities, ticks, structures, POI) is changed or dropped. Clients get no `light_update` packets after a block edit yet (they update their own light locally); chunks sent afterwards have the correct light.
* Only the overworld is loaded; stored block/fluid ticks are kept but not executed yet (vanilla executes them right after loading: leaf decay, flowing water, ...).
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
| `tools/world_compare.sh`, `tools/world_compare.py` | World persistence test: vanilla generates a world, both servers load it and their chunk packets are compared; then the Rust server edits and saves the world and vanilla loads it (requires `MC_EULA_ACCEPTED=true`). |
| `tools/remap.sh` | Produces `server-named.jar`, the vanilla server with Mojang's official names (shared by the tools below). |
| `tools/verify_features.sh` | Runs the real `FeatureSorter` and `WorldgenRandom` (`FeatureRef.java`) and compares the feature order per decoration step and the random streams. |
| `tools/verify_carvers.sh` | Runs the real surface stage and carvers (`SurfaceRef.java` with `carve`, 9 settings, 223 chunks) and compares the Rust port. |
| `tools/worldgen_smoke.sh` | Starts a fresh normal world and lets the bot join (chunk packets of generated terrain decode, no panic). |
| `tools/terrain_check.sh` | Generates again the chunks of a world made by the vanilla server and compares the blocks (`mc-server --check-terrain`). |
| `tools/verify_surface.sh` | Runs the real surface stage (`tools/terrain/.../SurfaceRef.java`, 9 settings, 223 chunks) and compares blocks and heightmaps with the Rust port. |
| `tools/verify_terrain.sh` | Runs the real `NoiseChunk` fill loop (`tools/terrain/.../TerrainRef.java`), the real noise router (`RouterRef.java`) and the real biome sources (`BiomeRef.java`) for 9 / 13 configurations and compares the Rust port block by block / value by value. |
| `tools/verify_noise.sh` | Runs the real random sources and noise functions (`tools/noise/NoiseRef.java`) and compares them bit for bit with `crates/mc-worldgen`. |
| `tools/light_check.sh` | Recomputes the light of every chunk of a vanilla-generated world and compares it with the light vanilla stored (also `mc-server --check-light`). |
| `tools/dump_props.sh` | Runs the **real** block and item classes and dumps per-state facts (hardness, blast resistance, light emission / opacity, collision / outline / occlusion shapes, 18 flags such as `isAir`, "holds a fluid", replaceable) and per-item facts (stack size, durability, placed block) into `reference/gen/generated/reports/{block_props,item_props}.tsv`. Run by `tools/fetch_reference.sh`. |
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
Every test player starts at a fixed position from pre-seeded player data (both servers read it), and vanilla random ticks are
disabled with `gamerule randomTickSpeed 0` (random ticks are block behaviour, M7), so the runs are deterministic: 5 of 5
consecutive runs match. Latest result: **36 / 36 matching**:

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

## World persistence test (milestone 2)

`MC_EULA_ACCEPTED=true tools/world_compare.sh`: the **real vanilla server** generates a world (seed 12345); then

1. vanilla and the Rust server load copies of it and send their chunks to a bot: on a freshly generated world **all 109
   chunks are identical byte for byte** (heightmaps, sections including palette order and the vanilla non-empty block
   count quirk, block entities including their order, light masks and arrays);
2. the Rust server breaks a block, shuts down on SIGTERM and saves; the **vanilla server loads the saved world without
   errors**, places the player at the saved position, and exactly the edited block differs.

Run against an older world (generated, then left running for a while), the remaining differences are all changes the vanilla
server makes by itself after loading: scheduled block/fluid ticks (leaf decay distance, flowing water and lava) and light
that spills in from chunks that only vanilla generates. Those belong to milestones M4/M5/M7.

## Biome source test (milestone 5, part 3)

`tools/verify_terrain.sh` also runs `tools/terrain/.../BiomeRef.java` with the **real** `Climate`, `MultiNoiseBiomeSource`,
`MultiNoiseBiomeSourceParameterList` and `TheEndBiomeSource`, and `crates/mc-worldgen/tests/biome_parity.rs` compares the Rust port
(**272,507 values, all identical**):

* the complete overworld (7,593 entries) and nether parameter lists, entry by entry, including the order that decides ties,
* 160,000 R-tree searches. The biome tables almost never produce a tie between different biomes, so besides the real lists the test
  searches 100,000 targets in randomly generated lists on a coarse grid (distinct values, clustered like vanilla's depth 0 / 1 pairs):
  about 19,700 of them end on an exact tie where the tree and a brute-force search disagree, and the Rust tree gives the vanilla answer every time,
* the climate target (six quantized values) and the biome at 79,300 quart positions for 13 seeds / settings (overworld, amplified,
  large biomes, caves, floating islands, nether, End): 63 different biomes are hit,
* `findSpawnPosition` for the 9 overworld-type settings,
* the temperature of every biome (`getTemperature`, incl. the frozen modifier and the height adjustment with `PerlinSimplexNoise`) as raw float
  bits at 400 positions each (64 biomes), plus `coldEnoughToSnow` / `shouldMeltFrozenOceanIcebergSlightly` on top of it.

The mutation checks show the test is sensitive: ignoring the thread-local "last result", taking the last instead of the first cheapest
split dimension, changing the sort of the buckets / of the small nodes, dropping the last sort tie-break, rounding instead of truncating
the quantization, or changing a single entry of the biome tables all fail it. Two mutations cannot fail it: `best >= leaf_d` in the
search (equivalent, a recursive search only returns a different leaf when it is strictly nearer) and `best >= d` (only prunes less).

Vanilla details reproduced: `quantizeCoord` truncates the *float* product (`-0.78F * 10000F` is `-7799.9995`, so the boundary is
`-7799`); the R-tree keeps the previous result of the thread on exact ties (so the answer depends on the search history, as in the
vanilla server); the End biome is derived from the erosion function evaluated at the centre of the 16x16 section.

## Surface stage test (milestone 5, part 4)

`tools/verify_surface.sh` runs the **real** surface stage (`tools/terrain/.../SurfaceRef.java`): `BiomeSource` for the 3x3 chunks around the target,
the `NoiseChunk` fill loop, `BiomeManager` (the zoom with the SHA-256 obfuscated seed) and `SurfaceSystem.buildSurface` with the `surface_rule` of
the noise settings. For each of 9 settings / seeds (overworld x2, amplified, large biomes, nether x2, End, caves, floating islands) it picks the first
chunk found for every biome in a spiral around the origin (223 chunks, 63 different biomes at the chunk centres) and writes every block and the
`WORLD_SURFACE_WG` heightmap. `crates/mc-worldgen/tests/surface_parity.rs` replays the same biome scan (so the R-tree search history is identical),
generates the chunks with the Rust port and compares: **all 223 chunks are identical** (43 different block types occur: grass, sand, red sand,
terracotta bands, snow, packed ice, gravel, mud, netherrack, soul sand, bedrock floor / roof, ...).

Mutation checks (not committed): changing the surface depth factor, the clay band offset or spacing, the steepness threshold, the eroded badlands
pillar height, the iceberg height or water chance, the preliminary-surface margin, the heightmap update rule, the `not` condition, the secondary
depth truncation, the stone depth below, the biome zoom fiddle or the frozen temperature modifier all fail the test (1 to 109 of the 223 chunks
differ). A shift of the snow temperature threshold by 0.01 is **not** seen by the chunks (no sampled column lands in that band), which is why the
biome test above compares the temperatures themselves.

Vanilla details reproduced: `ChunkAccess.getHeight` is the first free y minus one (the surface loop starts one above the highest block); the biome
of a column is looked up at the height of the heightmap (y = 0 for legacy random sources); the biome of a block is the nearest of eight fiddled
quart corners (`BiomeManager`), clamped to the chunk's quart range; `Math.round` for the clay bands; `WorldGenerationContext` takes the smaller of
the level height and the generator height (so the nether roof is at 127 in a 256 tall dimension).

## Terrain shape test (milestone 5, part 2)

`tools/verify_terrain.sh` builds the real `RandomState` and `NoiseChunk` from the vanilla data pack and runs vanilla's
`doFill` loop (without structure terrain adaptation) for **34 chunks of 9 configurations** (3 overworld seeds, amplified, large biomes,
nether, end, caves, floating islands); `crates/mc-worldgen/tests/terrain_parity.rs` generates the same chunks with the Rust port and
all block states are identical (stone, water, lava, air, copper / iron ore veins with tuff and granite). Because the block decision only
depends on the *sign* of the final density, a second test compares **every router function** (`barrier`, `temperature`, `vegetation`,
`continents`, `erosion`, `depth`, `ridges`, the densities, the vein functions, ...) at 48 positions in each of 8 settings: **5,760
values, bit-identical** (this is what the biome source will need). The tests are sensitive: breaking the aquifer similarity, the ore
vein threshold, the cell interpolation order, the `squeeze` formula or the shift scale makes them fail. This found two bugs on the
way: the key of the legacy shift noise (`minecraft:offset`), and that the block-level test alone cannot see the magnitude of a density.

Vanilla details reproduced: JSON arguments are wrapped in `HolderHolder`s so constants fold into `MulOrAdd` only when `NoiseChunk`
unwraps them; splines compute in 32-bit floats; a cell's density comes from the `lerp3` path while ore veins use the stepwise
`updateForY/X/Z` interpolation (they differ in the last bits); flat caches are read for every kind of context, while interpolators
and the in-cell cache only act for the chunk context.

## World generation primitives test (milestone 5, part 1)

`tools/verify_noise.sh` runs a fixed call sequence through the **real** Mojang `LegacyRandomSource`, `XoroshiroRandomSource` (with
`fork`, positional factories, `fromHashOf`, gaussian, reseeding), `ImprovedNoise` (value, scaled variant, derivative), `PerlinNoise`
(modern and legacy initialisation), `NormalNoise` (modern and legacy nether), `SimplexNoise` (2D and 3D) and `BlendedNoise` for several
seeds and 8 sample points, and `crates/mc-worldgen/tests/noise_parity.rs` replays it with the Rust port: **all 878 values are
bit-identical** (the test fails with 521 differences if a single float constant is widened wrongly, e.g. Java's `1.110223E-16F`).

## Light engine test (milestone 4)

`mc-server --check-light` (`tools/light_check.sh`) recomputes block and sky light for every chunk of a vanilla-generated world
that has all eight neighbours stored and compares it with the arrays vanilla stored: **225 / 225 chunks, all 11,700 layers
identical** (8.5 ms per chunk in a release build). 12 further sky layers exist only in the live vanilla engine, not in the
files, because a neighbouring chunk was generated after the chunk was saved; the captured vanilla packets contain exactly
those layers (all 15). `tools/world_compare.sh` additionally serves a world saved by the Rust server with both servers: the edited
chunk has no stored light, vanilla relights it, the Rust engine computes it, and **all 109 chunks were identical, light included**, on the world
that was generated when this was written. **Open issue:** on a later run with a freshly generated vanilla world, 1 of the 109 chunks (the edited one,
84 light cells at its south-east corner, up to 5 levels brighter in Rust) differed; it reproduces with the world generator disabled, so it is not
caused by the generator. The likely cause is that vanilla's *stored* light of a neighbouring chunk is stale for that corner (vanilla relights only
the edited chunk and takes its neighbours' stored arrays as given), while the Rust engine recomputes the whole 3x3 window from the blocks.
`tools/vanilla_compare.sh`: all superflat chunks (now lit by the engine) stay byte-identical.

The engine implements the fixpoint of the vanilla engine instead of its incremental queues: light is stored in a section when
the section or any of its 26 neighbours is non-empty, a lit cell gives its neighbour `level - max(1, lightBlock)` unless the
face shapes occlude, block light starts at the emission of every block, sky light at the cells at or above each column's
lowest sky source (`ChunkSkyLightSources`). Vanilla quirk reproduced: a chunk stored without light is relit from its own
emitters only, so light spilling in from the emitters of lit neighbours is missing there.

Quirks reproduced on the way: `LevelChunkSection.recalcBlockCounts` counts every block that holds a fluid twice
(`nonEmptyBlockCount` can exceed 4096), `keepPacked` and data components are not part of a block entity's update tag,
fastutil iteration order for block entities, `java.util.HashMap` order for NBT compounds, palette order of unchanged
sections.

## Carver test (milestone 5, part 6)

`tools/verify_carvers.sh` runs the real surface stage **and the real carvers** (`ChunkGenerator.applyCarvers`, the `AIR` step: `ConfiguredWorldCarver.carve` for
every carver that starts in the 17x17 chunks around the target, `CarvingContext`, `CarvingMask`, the aquifer of the chunk's `NoiseChunk`) for the same 223 chunks
of 9 settings as the surface test, and `crates/mc-worldgen/tests/surface_parity.rs` (`carvers_match_the_real_carvers`) compares the Rust port: **all 223 chunks are
identical** (blocks and heightmaps). Mutation checks (not committed): 15 of 17 changes fail the test (cave thickness, tunnel / canyon pitch and yaw rules, room size,
`canReach` margin, the top limit of the ellipsoid, the re-grassing below a roof, the large feature seed, the number of caves per start chunk, the sine table, the
tunnel split step, the canyon width smoothness, the nether lava level, the water height of the top material). Two cannot fail it: removing the `CarvingMask` check
(carving an already carved block does nothing, the mask is a speed-up) and `>=` instead of `>` in the cave ellipsoid (an exact tie of a sum of squares with 1.0).

Vanilla details reproduced: `Mth.sin` / `Mth.cos` are lookups in a 65536-entry table; the nether and overworld carvers use different `carveBlock`s (the nether one ignores
the aquifer and fills lava up to `minGenY + 31`); the biome manager of the carvers asks the biome source directly instead of the stored chunk biomes; and
`ChunkAccess.getBlockState` answers plain `air` for every position of a section that holds only air-class blocks, even where it stores `cave_air` (the test applies the
same view to the Rust chunk; the stored section keeps `cave_air`, which is what the server sends). This also found that a harness must bind the block tags after loading the
data pack (the carvers' `replaceable` sets are tags): without it the real carvers replace nothing.

## Feature decoration order test (milestone 5, part 7, first half)

Features are placed per decoration step in the order `FeatureSorter` computes, and every placed feature is seeded with its **index in that order**
(`WorldgenRandom.setFeatureSeed(decorationSeed, index, step)`), so the order has to be exact before any feature can match. `tools/verify_features.sh` runs the
real `FeatureSorter` and `WorldgenRandom` (`tools/terrain/.../FeatureRef.java`) and `crates/mc-worldgen/tests/feature_parity.rs` compares the Rust port:
**the sorted feature lists of all three dimensions (151 overworld, 37 nether and 5 End features over 11 / 10 / 11 steps) and the random streams are identical**
(209 values). Reproduced: `PlacedFeature` is a record, so entries with the same configured feature and modifiers share one index; the topological sort is a
depth-first search over `TreeMap`/`TreeSet`s ordered by (step, discovery index); `WorldgenRandom` extends the *legacy* random source, so over a Xoroshiro
source it uses the legacy `nextInt`/`nextFloat`/`nextDouble`/`nextLong` algorithms on `nextLong() >>> (64 - bits)`, and `setSeed` does not reset its cached Gaussian.

The placement modifiers, providers and the features themselves follow in the next stages.

## Generator integration check (milestone 5, part 5)

New worlds with `level-type=minecraft:normal` (and `large_biomes`, `amplified`) and chunks that a vanilla world does not store are generated by the
ported pipeline on background threads (`crates/mc-server/src/worldgen.rs`). `tools/worldgen_smoke.sh` starts a fresh normal world and lets the bot
join: the chunk packets decode and nothing panics. `tools/terrain_check.sh` (`mc-server --check-terrain`) generates again every chunk stored in a world made by the **real vanilla server** (289 chunks of
seed 12345) and compares the blocks; this is not an equality test, because vanilla also places features and structures, but it proves that seed,
coordinates and heights are right: **91.08 % of the 28.4 million blocks are identical** (90.31 % before the carvers were added) and the highest solid
non-foliage block of **97.93 %** of the columns is at the same height. The differences are what the missing stages produce: the underground blobs of
clay, tuff, andesite, diorite, granite, gravel and dirt, ores, sculk and moss (features), trees, and air from mineshafts, geodes and other structures.
`tools/vanilla_compare.sh` (superflat) still matches 38 / 38 against the vanilla server.

Generation speed is about 80 ms per chunk and core with the carvers (4 threads: about 25 chunks per second, a view distance of 8 fills in roughly 12 seconds);
unmodified generated chunks are not saved, they are generated again after they were unloaded, so later generator stages improve them without migration.

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
