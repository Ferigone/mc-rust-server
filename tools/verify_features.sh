#!/usr/bin/env bash
# Runs the REAL feature decoration classes (tools/terrain/.../FeatureRef.java) and compares the Rust port: the order of the placed features
# per decoration step (FeatureSorter), WorldgenRandom, and (as the stages are ported) the features themselves.
# Requires: tools/fetch_reference.sh and tools/dump_props.sh.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
W="$ROOT/reference/work"
"$ROOT/tools/remap.sh" >/dev/null
CP="$W/server-named.jar:$(find "$W/bundle/META-INF/libraries" -name '*.jar' | tr '\n' ':')"
CLS="$W/terrain-classes"; mkdir -p "$CLS"
javac -encoding UTF-8 -proc:none -nowarn -d "$CLS" -cp "$CP" "$ROOT"/tools/terrain/net/minecraft/world/level/levelgen/*.java 2>"$CLS/javac.log" || { grep -v "Picked up" "$CLS/javac.log" >&2; exit 2; }
rm -f "$W/feature_ref.txt"
(cd "$W" && java -cp "$CLS:$CP" net.minecraft.world.level.levelgen.FeatureRef "$W/feature_ref.txt" 2>&1 | grep -E "RESULT|Exception|Error" || true)
[ -f "$W/feature_ref.txt" ] || { echo "the feature reference was not generated" >&2; exit 3; }
cd "$ROOT" && cargo test --release -q -p mc-worldgen --test feature_parity -- --nocapture 2>&1 | grep -E "^test |test result|panicked|features in order|identical|differ" || true
