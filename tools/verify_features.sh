#!/usr/bin/env bash
# Runs the REAL feature decoration classes and compares the Rust port:
#  1. tools/terrain/.../FeatureRef.java: the order of the placed features per decoration step (FeatureSorter) and WorldgenRandom;
#  2. tools/terrain/.../DecorateRef.java: ChunkGenerator.applyBiomeDecoration on the 3x3 chunks around the first chunk found for every biome (real noise,
#     surface and carver stages first), placing exactly the placed features the port implements (examples/supported_features.rs writes the list).
#     The result goes to reference/work/decorate_*.bin plus a manifest; crates/mc-worldgen/tests/decoration_parity.rs compares the port.
# FEATURE_CONFIGS="ow1 nether" restricts step 2 to some configurations (ow1 ow2 amp large nether nether2 end caves islands).
# Requires: tools/fetch_reference.sh and tools/dump_props.sh.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
W="$ROOT/reference/work"
"$ROOT/tools/remap.sh" >/dev/null
"$ROOT/tools/dump_props.sh" >/dev/null
CP="$W/server-named.jar:$(find "$W/bundle/META-INF/libraries" -name '*.jar' | tr '\n' ':')"
CLS="$W/terrain-classes"; mkdir -p "$CLS"
javac -encoding UTF-8 -proc:none -nowarn -d "$CLS" -cp "$CP" "$ROOT"/tools/terrain/net/minecraft/world/level/levelgen/*.java 2>"$CLS/javac.log" || { grep -v "Picked up" "$CLS/javac.log" >&2; exit 2; }

# 1. feature order + WorldgenRandom
rm -f "$W/feature_ref.txt"
(cd "$W" && java -cp "$CLS:$CP" net.minecraft.world.level.levelgen.FeatureRef "$W/feature_ref.txt" 2>&1 | grep -E "RESULT|Exception|Error" || true)
[ -f "$W/feature_ref.txt" ] || { echo "the feature reference was not generated" >&2; exit 3; }

# 2. the decoration itself
(cd "$ROOT" && cargo run --release -q -p mc-worldgen --example supported_features -- "$ROOT/reference/gen/generated" "$W/decorate_supported.txt" --unsupported | head -1)
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
# FEATURE_CONFIGS="ow1 nether" restricts the run to some of the configurations (faster iteration)
if [ -n "${FEATURE_CONFIGS:-}" ]; then
  SELECTED=()
  for c in "${CONFIGS[@]}"; do for n in $FEATURE_CONFIGS; do [ "${c%% *}" = "$n" ] && SELECTED+=("$c"); done; done
  CONFIGS=("${SELECTED[@]}")
fi
MANIFEST="$W/decorate_manifest.txt"; : > "$MANIFEST"
cd "$W"
PIDS=()
for c in "${CONFIGS[@]}"; do
  set -- $c
  name=$1; shift
  rm -f "$W/decorate_$name.bin"
  ( java -Xmx1500m -cp "$CLS:$CP" net.minecraft.world.level.levelgen.DecorateRef "$W/decorate_$name.bin" "$@" "$W/decorate_supported.txt" 2>&1 | grep -E "RESULT|Exception|Error" || true ) &
  PIDS+=($!)
  echo "decorate_$name.bin $*" >> "$MANIFEST"
  # at most 4 JVMs at a time
  if [ "${#PIDS[@]}" -ge 4 ]; then wait "${PIDS[0]}"; PIDS=("${PIDS[@]:1}"); fi
done
wait
for c in "${CONFIGS[@]}"; do set -- $c; [ -f "$W/decorate_$1.bin" ] || { echo "reference for $1 was not generated" >&2; exit 3; }; done
cd "$ROOT" && cargo test --release -q -p mc-worldgen --test feature_parity --test decoration_parity -- --nocapture 2>&1 | grep -E "^test |test result|panicked|features in order|identical|differ|compared|placed by" || true
