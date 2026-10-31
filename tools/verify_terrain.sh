#!/usr/bin/env bash
# Runs the REAL NoiseChunk fill loop (tools/terrain/.../TerrainRef.java) for several noise settings, seeds and chunks and writes the
# block state IDs to reference/work/terrain_*.bin plus a manifest; crates/mc-worldgen/tests/terrain_parity.rs compares the Rust port.
# Requires: tools/fetch_reference.sh.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
W="$ROOT/reference/work"
"$ROOT/tools/remap.sh" >/dev/null
CP="$W/server-named.jar:$(find "$W/bundle/META-INF/libraries" -name '*.jar' | tr '\n' ':')"
CLS="$W/terrain-classes"; mkdir -p "$CLS"
javac -encoding UTF-8 -proc:none -nowarn -d "$CLS" -cp "$CP" "$ROOT"/tools/terrain/net/minecraft/world/level/levelgen/*.java 2>"$CLS/javac.log" || { grep -v "Picked up" "$CLS/javac.log" >&2; exit 2; }
MANIFEST="$W/terrain_manifest.txt"; : > "$MANIFEST"
# name settings seed min_y height chunks...
CONFIGS=(
  "ow1 minecraft:overworld 12345 -64 384 0,0 1,0 -1,0 0,1 0,-1 5,5 -12,7 30,-30 100,100"
  "ow2 minecraft:overworld -4172144997902289642 -64 384 0,0 3,-4 -9,-9 64,12 -50,80"
  "ow3 minecraft:overworld 0 -64 384 2,2 -7,13 21,-5"
  "amp minecraft:amplified 777 -64 384 0,0 6,6 -20,3"
  "large minecraft:large_biomes 4242 -64 384 0,0 9,-2"
  "nether minecraft:nether 12345 0 128 0,0 4,4 -10,6 25,-25"
  "end minecraft:end 12345 0 128 0,0 6,6 -3,10 20,20"
  "caves minecraft:caves 99 -64 192 0,0 5,-5"
  "islands minecraft:floating_islands 99 0 256 0,0 -4,8"
)
cd "$W"
for c in "${CONFIGS[@]}"; do
  set -- $c
  name=$1; shift
  rm -f "$W/terrain_$name.bin"
  java -cp "$CLS:$CP" net.minecraft.world.level.levelgen.TerrainRef "$W/terrain_$name.bin" "$@" 2>&1 | grep -E "RESULT|Exception|Error" || true
  [ -f "$W/terrain_$name.bin" ] || { echo "reference for $name was not generated" >&2; exit 3; }
  echo "terrain_$name.bin $*" >> "$MANIFEST"
done
# the router functions at fixed positions (finer than the block level: exact climate values, magnitudes of the densities)
java -cp "$CLS:$CP" net.minecraft.world.level.levelgen.RouterRef "$W/router_ref.txt" minecraft:overworld@12345 minecraft:overworld@-4172144997902289642 minecraft:amplified@777 minecraft:large_biomes@4242 minecraft:nether@12345 minecraft:end@12345 minecraft:caves@99 minecraft:floating_islands@99 2>&1 | grep -E "RESULT|Exception|Error" || true
# the biome source: parameter lists, R-tree searches, sampled biomes of the real MultiNoiseBiomeSource / TheEndBiomeSource, world spawn search
rm -f "$W/biome_ref.txt"
java -cp "$CLS:$CP" net.minecraft.world.level.levelgen.BiomeRef "$W/biome_ref.txt" minecraft:overworld@12345@overworld minecraft:overworld@-4172144997902289642@overworld minecraft:overworld@0@overworld minecraft:overworld@-1@overworld minecraft:overworld@987654321@overworld minecraft:amplified@777@overworld minecraft:large_biomes@4242@overworld minecraft:caves@99@overworld minecraft:floating_islands@99@overworld minecraft:nether@12345@nether minecraft:nether@-77@nether minecraft:end@12345@end minecraft:end@31337@end 2>&1 | grep -E "RESULT|Exception|Error" || true
[ -f "$W/biome_ref.txt" ] || { echo "the biome reference was not generated" >&2; exit 3; }
cd "$ROOT" && cargo test --release -q -p mc-worldgen --test terrain_parity --test router_parity --test biome_parity -- --nocapture 2>&1 | grep -E "^test |test result|panicked|chunk|differ|MISMATCH|settings" || true
