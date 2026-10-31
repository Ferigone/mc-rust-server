#!/usr/bin/env bash
# Smoke test of the world generator inside the server: a fresh normal world, a bot joins, the chunk packets must decode and nothing may panic.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
cargo build -q --release -p mc-server -p mc-probe
D="$ROOT/reference/worldgen-smoke"; rm -rf "$D"; mkdir -p "$D"
printf '%s\n' "online-mode=false" "level-seed=12345" "view-distance=6" "gamemode=creative" "server-port=25583" > "$D/server.properties"
"$ROOT/target/release/mc-server" --dir "$D" > "$D/server.log" 2>&1 & SPID=$!
trap 'kill $SPID 2>/dev/null || true' EXIT
for _ in $(seq 1 60); do (echo > /dev/tcp/127.0.0.1/25583) 2>/dev/null && break; sleep 0.5; done
# (the probe's own flat-world checks are expected to fail on terrain: only the decoding checks matter here)
"$ROOT/target/release/mc-probe" join --port 25583 --name Explorer --secs 25 > "$D/probe.txt" 2>&1 || true
grep -E "chunk: packet fully decoded|chunk: 24 sections|play: chunks received|play/login: packet fully decoded" "$D/probe.txt"
if grep -q "panicked" "$D/server.log"; then echo "the server panicked:" >&2; grep -A3 panicked "$D/server.log" >&2; exit 1; fi
grep -q "\[OK \] chunk: packet fully decoded" "$D/probe.txt" || { echo "chunk packets did not decode" >&2; exit 1; }
grep "World spawn" "$D/server.log" | sed 's/\x1b\[[0-9;]*m//g'
echo "OK: the generated world was served"
