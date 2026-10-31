#!/usr/bin/env bash
# Runs the REAL Mojang random sources and noise functions (tools/noise/NoiseRef.java) and writes reference/work/noise_ref.txt;
# crates/mc-worldgen/tests/noise_parity.rs compares the Rust port with it bit for bit.
# Requires: tools/fetch_reference.sh (the remapped jar is produced by tools/remap.sh).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
W="$ROOT/reference/work"
"$ROOT/tools/remap.sh" >/dev/null
CP="$W/server-named.jar:$(find "$W/bundle/META-INF/libraries" -name '*.jar' | tr '\n' ':')"
CLS="$W/noise-classes"; mkdir -p "$CLS"
javac -encoding UTF-8 -proc:none -nowarn -d "$CLS" -cp "$CP" "$ROOT/tools/noise/NoiseRef.java" 2>"$CLS/javac.log" || { grep -v "Picked up" "$CLS/javac.log" >&2; exit 2; }
cd "$W" && java -cp "$CLS:$CP" NoiseRef "$W/noise_ref.txt" 2>&1 | grep -E "RESULT|Exception|Error" || true
cd "$ROOT" && cargo test -q -p mc-worldgen --test noise_parity 2>&1 | grep -E "^test |test result|panicked|mismatch" || true
