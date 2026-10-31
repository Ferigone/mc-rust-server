#!/usr/bin/env bash
# Reverse-engineering tool: decompiles the vanilla 1.21.1 server with Mojang's OFFICIAL names.
# Output: reference/src/ (not tracked by git), used only as reference material for the port.
# Requires beforehand: tools/fetch_reference.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REF="$ROOT/reference"
T="$REF/tools"
mkdir -p "$T" "$REF/work" "$REF/src"

[ -f "$REF/dl/server.jar" ] || { echo "Run tools/fetch_reference.sh first" >&2; exit 1; }

dl() { [ -s "$2" ] && return; curl -fsS --retry 5 --retry-delay 5 -o "$2" "$1"; }
dl https://repo1.maven.org/maven2/org/vineflower/vineflower/1.10.1/vineflower-1.10.1.jar "$T/vineflower.jar"

# 1-3) bundler -> remapped jar with Mojang's official names (shared with the other tools)
"$ROOT/tools/remap.sh" >/dev/null
cd "$REF/work"

# 4) decompilation
if [ -z "$(ls -A "$REF/src" 2>/dev/null)" ]; then
  LIBS=$(find bundle/META-INF/libraries -name '*.jar' | sed 's/^/-e=/' | tr '\n' ' ')
  java -Xmx4g -jar "$T/vineflower.jar" -dgs=1 -hdc=0 -rsy=1 -ind='    ' $LIBS server-named.jar "$REF/src" >/dev/null 2>&1 || true
fi
echo "OK: $(find "$REF/src" -name '*.java' | wc -l) Java files in $REF/src"
