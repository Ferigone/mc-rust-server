#!/usr/bin/env bash
# Differential test: runs the REAL vanilla 1.21.1 server next to the Rust server and compares packets for the same events.
#
# REQUIRES acceptance of the Mojang EULA (https://aka.ms/MinecraftEULA). The script writes eula=true into the working directory
# ONLY when you set MC_EULA_ACCEPTED=true, i.e. you knowingly confirm that you accept the EULA.
#   MC_EULA_ACCEPTED=true tools/vanilla_compare.sh
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
if [ "${MC_EULA_ACCEPTED:-}" != "true" ]; then
  echo "This test runs the vanilla server, which requires accepting the Mojang EULA." >&2
  echo "Read https://aka.ms/MinecraftEULA and, if you agree, run: MC_EULA_ACCEPTED=true $0" >&2
  exit 2
fi
[ -f reference/dl/server.jar ] || tools/fetch_reference.sh
cargo build -q --release -p mc-server -p mc-probe
W="$ROOT/reference/vanilla-compare"; rm -rf "$W"; mkdir -p "$W/vanilla" "$W/rust" "$W/dumps"
VP=25570; RP=25571
echo "eula=true" > "$W/vanilla/eula.txt"
S='online-mode=false
level-type=minecraft\:flat
generate-structures=false
level-seed=12345
enforce-secure-profile=false
view-distance=10
simulation-distance=10
spawn-protection=0
gamemode=creative
force-gamemode=true'
{ echo "$S"; echo "server-port=$VP"; } > "$W/vanilla/server.properties"
{ echo "online-mode=false"; echo "level-seed=12345"; echo "view-distance=10"; echo "simulation-distance=10"; echo "gamemode=creative"; echo "force-gamemode=true"; echo "server-port=$RP"; } > "$W/rust/server.properties"
# Every test player starts at its own fixed position (player data read by both servers) instead of a random spawn
# position, so the test cases cannot interfere with each other's block edits.
i=0
for n in ZedA ChatA CmdA LstA Brk1 Brk2 Brk3 Plc1 Plc2 Plc3 Bes1 Bes2 Bes3 Mul1 Mul2 Mul3; do
  for d in vanilla rust; do python3 -I "$ROOT/tools/make_playerdata.py" "$W/$d/world" "$n" "$((8 * i)).5" -60 0.5; done
  i=$((i + 1))
done
# stdin through a FIFO so that a console command can be sent after the start
mkfifo "$W/vanilla/stdin"; exec 9<>"$W/vanilla/stdin"
# exec: the background PID must be java itself, otherwise the EXIT trap kills only the subshell and java keeps the port
(cd "$W/vanilla" && exec java -Xmx1G -jar "$ROOT/reference/dl/server.jar" nogui >server.out 2>&1 <stdin) & VPID=$!
"$ROOT/target/release/mc-server" --dir "$W/rust" >"$W/rust/server.log" 2>&1 & RPID=$!
trap 'kill $VPID $RPID 2>/dev/null || true' EXIT
for _ in $(seq 1 240); do grep -q 'Done (' "$W/vanilla/server.out" 2>/dev/null && break; sleep 0.5; done
if ! grep -q 'Done (' "$W/vanilla/server.out" 2>/dev/null; then
  echo "The vanilla server did not start (port $VP in use / world lock?). Last lines:" >&2; tail -5 "$W/vanilla/server.out" >&2; exit 3
fi
# Random ticks (grass growth, leaf decay, ...) are block behaviour that is not ported yet (M7); without this rule the vanilla
# server sends block changes that no test action caused and the packet sequences cannot be compared.
echo "gamerule randomTickSpeed 0" >&9
for _ in $(seq 1 40); do grep -q 'randomTickSpeed' "$W/vanilla/server.out" 2>/dev/null && break; sleep 0.25; done
for _ in $(seq 1 40); do (echo > "/dev/tcp/127.0.0.1/$RP") 2>/dev/null && break; sleep 0.25; done
P="$ROOT/target/release/mc-probe"; D="$W/dumps"
run() { # tag port nick case extra...
  local tag=$1 port=$2 nick=$3 case=$4; shift 4
  "$P" join --port "$port" --name "$nick" --secs 5 "$@" --dump "$D/${tag}_${case}.bin" >/dev/null 2>&1 || true
}
for t in "vanilla $VP" "rust $RP"; do set -- $t; tag=$1; port=$2
  "$P" status --port "$port" 2>/dev/null | sed -n 1p > "$D/status_$tag.txt"
  run $tag $port "ZedA"  join
  run $tag $port "ChatA" chat --chat hello
  run $tag $port "CmdA"  cmd  --command "foo bar"
  run $tag $port "LstA"  list --command list
  run $tag $port "OldA"  p700 --protocol 700
  run $tag $port "OldA"  p760 --protocol 760
  for k in 1 2 3; do
    run $tag $port "Brk$k" break$k  --break below
    run $tag $port "Plc$k" place$k  --place below
    run $tag $port "Bes$k" beside$k --place beside
  done
  for k in 1 2 3; do run $tag $port "Mul$k" multi$k --break below --place beside; done
done
python3 -I tools/vanilla_compare.py "$D"
