#!/usr/bin/env bash
# Reverse-engineering tool: produces reference/work/server-named.jar, the vanilla 1.21.1 server with Mojang's OFFICIAL names
# (bundler extracted, ProGuard mappings -> Tiny, tiny-remapper). Idempotent. Used by tools/decompile.sh, tools/verify.sh
# and tools/dump_props.sh. Requires beforehand: tools/fetch_reference.sh
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REF="$ROOT/reference"
T="$REF/tools"
mkdir -p "$T" "$REF/work"
[ -f "$REF/dl/server.jar" ] || { echo "Run tools/fetch_reference.sh first" >&2; exit 1; }

dl() { [ -s "$2" ] && return; curl -fsS --retry 5 --retry-delay 5 -o "$2" "$1"; }
dl https://maven.fabricmc.net/net/fabricmc/tiny-remapper/0.10.4/tiny-remapper-0.10.4-fat.jar "$T/tiny-remapper.jar"

cd "$REF/work"
# 1) extract the actual jar from the bundler + libraries
if [ ! -f server-inner.jar ]; then
  unzip -o -q "$REF/dl/server.jar" 'META-INF/versions/*' 'META-INF/libraries/*' -d bundle
  cp bundle/META-INF/versions/*/server-*.jar server-inner.jar
fi
# 2) ProGuard mappings -> Tiny
[ -f mojang.tiny ] || python3 -I "$ROOT/tools/proguard2tiny.py" "$REF/dl/server_mappings.txt" mojang.tiny
# 3) remap obf -> named (classpath = server libraries, for correct inheritance resolution)
if [ ! -f server-named.jar ]; then
  java -jar "$T/tiny-remapper.jar" server-inner.jar server-named.jar mojang.tiny official named \
    $(find bundle/META-INF/libraries -name '*.jar') >/dev/null
fi
echo "OK: $REF/work/server-named.jar"
