#!/usr/bin/env bash
# Runs the REAL Mojang block / item classes and writes block_props.tsv and item_props.tsv (per-state hardness, light, shapes, ...)
# into reference/gen/generated/reports/ next to the official data-generator reports; the Rust server loads them from there.
# Requires beforehand: tools/fetch_reference.sh. Idempotent (skipped when the files exist; FORCE=1 regenerates).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
W="$ROOT/reference/work"
OUT="$ROOT/reference/gen/generated/reports"
if [ -f "$OUT/block_props.tsv" ] && [ -f "$OUT/item_props.tsv" ] && [ "${FORCE:-}" != 1 ]; then echo "OK: property tables exist"; exit 0; fi
"$ROOT/tools/remap.sh" >/dev/null
CP="$W/server-named.jar:$(find "$W/bundle/META-INF/libraries" -name '*.jar' | tr '\n' ':')"
CLS="$W/props-classes"; mkdir -p "$CLS"
javac -proc:none -nowarn -d "$CLS" -cp "$CP" "$ROOT/tools/props/PropsDump.java" 2>"$CLS/javac.log" || { grep -v "Picked up" "$CLS/javac.log" >&2; exit 2; }
# working directory outside the repository (Mojang creates logs/ here)
cd "$W" && java -cp "$CLS:$CP" PropsDump "$OUT" 2>&1 | grep -E "RESULT|Exception|Error" || true
