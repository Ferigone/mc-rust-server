#!/usr/bin/env bash
# Runs the REAL surface stage and the carvers (tools/terrain/.../SurfaceRef.java with the "carve" argument: ChunkGenerator.applyCarvers, AIR step) for the
# first chunk found for every biome around the origin, in several noise settings and seeds, and writes blocks + WORLD_SURFACE_WG heightmaps to
# reference/work/carver_*.bin plus a manifest; crates/mc-worldgen/tests/surface_parity.rs (test `carvers_match_the_real_carvers`) compares the Rust port.
# Requires: tools/fetch_reference.sh and tools/dump_props.sh.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
W="$ROOT/reference/work"
"$ROOT/tools/remap.sh" >/dev/null
"$ROOT/tools/dump_props.sh" >/dev/null
CP="$W/server-named.jar:$(find "$W/bundle/META-INF/libraries" -name '*.jar' | tr '\n' ':')"
CLS="$W/terrain-classes"; mkdir -p "$CLS"
javac -encoding UTF-8 -proc:none -nowarn -d "$CLS" -cp "$CP" "$ROOT"/tools/terrain/net/minecraft/world/level/levelgen/*.java 2>"$CLS/javac.log" || { grep -v "Picked up" "$CLS/javac.log" >&2; exit 2; }
# name settings seed biome-source min_y height max-ring-in-chunks step-in-chunks
CONFIGS=(
  "ow1 minecraft:overworld 12345 overworld -64 384 300 3"
  "ow2 minecraft:overworld -4172144997902289642 overworld -64 384 300 3"
  "amp minecraft:amplified 777 overworld -64 384 300 3"
  "large minecraft:large_biomes 4242 overworld -64 384 600 6"
  "nether minecraft:nether 12345 nether 0 256 300 3"
  "nether2 minecraft:nether -77 nether 0 256 300 3"
  "end minecraft:end 12345 end 0 256 400 3"
  "caves minecraft:caves 99 overworld -64 192 300 3"
  "islands minecraft:floating_islands 99 overworld 0 256 300 3"
)
MANIFEST="$W/carver_manifest.txt"; : > "$MANIFEST"
cd "$W"
PIDS=()
for c in "${CONFIGS[@]}"; do
  set -- $c
  name=$1; shift
  rm -f "$W/carver_$name.bin"
  ( java -Xmx1500m -cp "$CLS:$CP" net.minecraft.world.level.levelgen.SurfaceRef "$W/carver_$name.bin" "$@" carve 2>&1 | grep -E "RESULT|Exception|Error" || true ) &
  PIDS+=($!)
  echo "carver_$name.bin $* carve" >> "$MANIFEST"
  # at most 4 JVMs at a time
  if [ "${#PIDS[@]}" -ge 4 ]; then wait "${PIDS[0]}"; PIDS=("${PIDS[@]:1}"); fi
done
wait
for c in "${CONFIGS[@]}"; do set -- $c; [ -f "$W/carver_$1.bin" ] || { echo "reference for $1 was not generated" >&2; exit 3; }; done
cd "$ROOT" && cargo test --release -q -p mc-worldgen --test surface_parity carvers -- --nocapture 2>&1 | grep -E "^test |test result|panicked|chunks|differ|identical" || true
