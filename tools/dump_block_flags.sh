#!/usr/bin/env bash
# Runs the real Mojang block classes and writes reference/work/block_flags.txt ("<state> <isAir> <hasFluid>").
# Used by the `fluid_rule_matches_vanilla` test in mc-data. Requires tools/decompile.sh (server-named.jar).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
W="$ROOT/reference/work"
[ -f "$W/server-named.jar" ] || { echo "Run tools/fetch_reference.sh and tools/decompile.sh first" >&2; exit 1; }
CP="$W/server-named.jar:$(find "$W/bundle/META-INF/libraries" -name '*.jar' | tr '\n' ':')"
OUT="$W/verify-classes"
mkdir -p "$OUT"
javac -proc:none -nowarn -d "$OUT" -cp "$CP" "$ROOT/tools/verify/BlockFlags.java" 2>"$OUT/javac.log" || { grep -v "Picked up" "$OUT/javac.log" >&2; exit 2; }
cd "$W" && java -cp "$OUT:$CP" BlockFlags "$W/block_flags.txt" 2>&1 | grep -v "Picked up" | grep -E "RESULT|Exception|Error" || true
