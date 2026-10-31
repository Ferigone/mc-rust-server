#!/usr/bin/env bash
# Weryfikuje zrzut pakietów (mc-probe --dump) prawdziwymi dekoderami Mojang. Wymaga tools/decompile.sh (server-named.jar).
# Użycie: tools/verify.sh <zrzut.bin> [full]
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
W="$ROOT/reference/work"
[ -f "$W/server-named.jar" ] || { echo "Najpierw tools/fetch_reference.sh i tools/decompile.sh" >&2; exit 1; }
CP="$W/server-named.jar:$(find "$W/bundle/META-INF/libraries" -name '*.jar' | tr '\n' ':')"
OUT="$ROOT/reference/work/verify-classes"
mkdir -p "$OUT"
rm -f "$OUT"/Verify*.class
if ! javac -proc:none -nowarn -d "$OUT" -cp "$CP" "$ROOT/tools/verify/net/minecraft/tags/TagsAccess.java" "$ROOT/tools/verify/Verify.java" 2>"$OUT/javac.log"; then
  grep -v "Picked up" "$OUT/javac.log" >&2; echo "Kompilacja harnessu nie powiodła się" >&2; exit 2
fi
# katalog roboczy poza repozytorium (Mojang tworzy tu logs/)
ARGS=(); for a in "$@"; do case "$a" in *.bin) ARGS+=("$(realpath "$a")");; *) ARGS+=("$a");; esac; done
cd "$W" && java -cp "$OUT:$CP" Verify "${ARGS[@]}" 2>&1 | grep -v "Picked up"
