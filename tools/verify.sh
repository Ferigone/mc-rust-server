#!/usr/bin/env bash
# Verifies a packet dump (mc-probe --dump) with the real Mojang decoders. Requires tools/decompile.sh (server-named.jar).
# Usage: tools/verify.sh <dump.bin> [full]
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
W="$ROOT/reference/work"
[ -f "$W/server-named.jar" ] || { echo "Run tools/fetch_reference.sh and tools/decompile.sh first" >&2; exit 1; }
CP="$W/server-named.jar:$(find "$W/bundle/META-INF/libraries" -name '*.jar' | tr '\n' ':')"
OUT="$ROOT/reference/work/verify-classes"
mkdir -p "$OUT"
rm -f "$OUT"/Verify*.class
if ! javac -proc:none -nowarn -d "$OUT" -cp "$CP" "$ROOT/tools/verify/net/minecraft/tags/TagsAccess.java" "$ROOT/tools/verify/Verify.java" 2>"$OUT/javac.log"; then
  grep -v "Picked up" "$OUT/javac.log" >&2; echo "Harness compilation failed" >&2; exit 2
fi
# working directory outside the repository (Mojang creates logs/ here)
ARGS=(); for a in "$@"; do case "$a" in *.bin) ARGS+=("$(realpath "$a")");; *) ARGS+=("$a");; esac; done
cd "$W" && java -cp "$OUT:$CP" Verify "${ARGS[@]}" 2>&1 | grep -v "Picked up"
