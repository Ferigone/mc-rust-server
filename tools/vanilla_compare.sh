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
(cd "$W/vanilla" && java -Xmx1G -jar "$ROOT/reference/dl/server.jar" nogui >server.out 2>&1 </dev/null) & VPID=$!
"$ROOT/target/release/mc-server" --dir "$W/rust" >"$W/rust/server.log" 2>&1 & RPID=$!
trap 'kill $VPID $RPID 2>/dev/null || true' EXIT
for _ in $(seq 1 240); do grep -q 'Done (' "$W/vanilla/server.out" 2>/dev/null && break; sleep 0.5; done
if ! grep -q 'Done (' "$W/vanilla/server.out" 2>/dev/null; then
  echo "The vanilla server did not start (port $VP in use / world lock?). Last lines:" >&2; tail -5 "$W/vanilla/server.out" >&2; exit 3
fi
for _ in $(seq 1 40); do (echo > "/dev/tcp/127.0.0.1/$RP") 2>/dev/null && break; sleep 0.25; done
P="$ROOT/target/release/mc-probe"; D="$W/dumps"
run() { # tag port nick case extra...
  local tag=$1 port=$2 nick=$3 case=$4; shift 4
  "$P" join --port "$port" --name "$nick" --secs 3 "$@" --dump "$D/${tag}_${case}.bin" >/dev/null 2>&1 || true
}
for t in "vanilla $VP" "rust $RP"; do set -- $t; tag=$1; port=$2
  "$P" status --port "$port" 2>/dev/null | sed -n 1p > "$D/status_$tag.txt"
  run $tag $port "ZedA"  join
  run $tag $port "ChatA" chat --chat witaj
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
