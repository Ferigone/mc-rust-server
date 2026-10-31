#!/usr/bin/env bash
# Narzędzie REA: pobiera oficjalny serwer Minecraft 1.21.1 (protokół 767) od Mojang,
# weryfikuje SHA1 i uruchamia OFICJALNY generator danych (net.minecraft.data.Main).
# Wynik ląduje w ./reference/ (w .gitignore) - nie commitujemy plików Mojang.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REF="$ROOT/reference"
VERSION_URL="https://piston-meta.mojang.com/v1/packages/018296bcb207bea6fd25b05f08eee3942dcec406/1.21.1.json"

mkdir -p "$REF/dl" "$REF/gen"
cd "$REF/dl"

if [ ! -f version.json ]; then
  curl -fsS --retry 3 -o version.json "$VERSION_URL"
fi

read_field() { python3 -I -c "import json,sys; d=json.load(open('version.json'))['downloads'][sys.argv[1]]; print(d[sys.argv[2]])" "$1" "$2"; }

fetch() { # name file
  local url sha
  url=$(read_field "$1" url); sha=$(read_field "$1" sha1)
  if [ -f "$2" ] && [ "$(sha1sum "$2" | cut -d' ' -f1)" = "$sha" ]; then return; fi
  echo "[fetch] $2"
  curl -fsS --retry 3 -o "$2" "$url"
  [ "$(sha1sum "$2" | cut -d' ' -f1)" = "$sha" ] || { echo "SHA1 mismatch: $2" >&2; exit 1; }
}

fetch server server.jar
fetch server_mappings server_mappings.txt

if [ ! -d "$REF/gen/generated/reports" ]; then
  echo "[gen] uruchamiam oficjalny data generator (--all)"
  (cd "$REF/gen" && java -DbundlerMainClass=net.minecraft.data.Main -jar "$REF/dl/server.jar" --all --output "$REF/gen/generated" >/dev/null)
fi
echo "OK: $REF/gen/generated/{reports,data,assets}"
