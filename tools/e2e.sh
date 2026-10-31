#!/usr/bin/env bash
# End-to-end test: starts the Rust server, connects bots (mc-probe) and verifies the packet dumps
# with the REAL Mojang decoders (requires: tools/fetch_reference.sh + tools/decompile.sh).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
cargo build -q -p mc-server -p mc-probe
BIN="$ROOT/target/debug"
DATA="$ROOT/reference/gen/generated"
RUN="$(mktemp -d)"
PORT="${PORT:-25597}"
printf 'online-mode=false\nserver-port=%s\n' "$PORT" > "$RUN/server.properties"
"$BIN/mc-server" --dir "$RUN" >"$RUN/server.log" 2>&1 &
SRV=$!
trap 'kill $SRV 2>/dev/null || true; rm -rf "$RUN"' EXIT
for _ in $(seq 1 80); do (echo > "/dev/tcp/127.0.0.1/$PORT") 2>/dev/null && break; sleep 0.25; done

echo "### status";             "$BIN/mc-probe" status --port "$PORT"
echo "### join (known packs)"; "$BIN/mc-probe" join --port "$PORT" --name Alice --data "$DATA" --secs 2 --chat "e2e" --dump "$RUN/known.bin" | tail -1
echo "### join (full NBT)";   "$BIN/mc-probe" join --port "$PORT" --name Bob --data "$DATA" --secs 2 --no-known-packs --dump "$RUN/full.bin" | tail -1
echo "### two players"
"$BIN/mc-probe" join --port "$PORT" --name Carol --secs 4 --chat "dwa" --dump "$RUN/two.bin" >/dev/null &
sleep 1.2
"$BIN/mc-probe" join --port "$PORT" --name Dave --secs 2 --walk >/dev/null
wait %2 2>/dev/null || true
sleep 3

if [ -f "$ROOT/reference/work/server-named.jar" ]; then
  for f in known:known full:full two:known; do
    echo "### verification with the real Mojang decoders: ${f%%:*}"
    mode=""; [ "${f##*:}" = full ] && mode=full
    "$ROOT/tools/verify.sh" "$RUN/${f%%:*}.bin" $mode | sed 's/^\[[0-9:]*\] \[main\/INFO\]: \[STDOUT\]: //' | grep -E "FAIL|RESULT" || true
  done
else
  echo "(skipped Mojang verification: run tools/decompile.sh)"
fi
echo "### OK"
