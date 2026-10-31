#!/usr/bin/env bash
# Narzędzie REA: dekompilacja serwera vanilla 1.21.1 z OFICJALNYMI nazwami Mojang.
# Wynik: reference/src/ (poza gitem) - służy wyłącznie jako materiał referencyjny do portowania.
# Wymaga wcześniej: tools/fetch_reference.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REF="$ROOT/reference"
T="$REF/tools"
mkdir -p "$T" "$REF/work" "$REF/src"

[ -f "$REF/dl/server.jar" ] || { echo "Najpierw uruchom tools/fetch_reference.sh" >&2; exit 1; }

dl() { [ -s "$2" ] && return; curl -fsS --retry 5 --retry-delay 5 -o "$2" "$1"; }
dl https://repo1.maven.org/maven2/org/vineflower/vineflower/1.10.1/vineflower-1.10.1.jar "$T/vineflower.jar"
dl https://maven.fabricmc.net/net/fabricmc/tiny-remapper/0.10.4/tiny-remapper-0.10.4-fat.jar "$T/tiny-remapper.jar"

# 1) wyjmij właściwy jar z bundlera + biblioteki
cd "$REF/work"
if [ ! -f server-inner.jar ]; then
  unzip -o -q "$REF/dl/server.jar" 'META-INF/versions/*' 'META-INF/libraries/*' -d bundle
  cp bundle/META-INF/versions/*/server-*.jar server-inner.jar
fi

# 2) mapowania ProGuard -> Tiny
[ -f mojang.tiny ] || python3 -I "$ROOT/tools/proguard2tiny.py" "$REF/dl/server_mappings.txt" mojang.tiny

# 3) remap obf -> named
if [ ! -f server-named.jar ]; then
  # classpath = biblioteki serwera (dla poprawnego rozwiązywania dziedziczenia)
  java -jar "$T/tiny-remapper.jar" server-inner.jar server-named.jar mojang.tiny official named \
    $(find bundle/META-INF/libraries -name '*.jar') >/dev/null
fi

# 4) dekompilacja
if [ -z "$(ls -A "$REF/src" 2>/dev/null)" ]; then
  LIBS=$(find bundle/META-INF/libraries -name '*.jar' | sed 's/^/-e=/' | tr '\n' ' ')
  java -Xmx4g -jar "$T/vineflower.jar" -dgs=1 -hdc=0 -rsy=1 -ind='    ' $LIBS server-named.jar "$REF/src" >/dev/null 2>&1 || true
fi
echo "OK: $(find "$REF/src" -name '*.java' | wc -l) plików .java w $REF/src"
