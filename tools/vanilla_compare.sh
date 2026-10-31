#!/usr/bin/env bash
# Test różnicowy: uruchamia PRAWDZIWY serwer vanilla 1.21.1 obok serwera Rust i porównuje pakiety dla tych samych zdarzeń.
#
# WYMAGA zgody na EULA Mojang (https://aka.ms/MinecraftEULA). Skrypt zapisze eula=true w katalogu roboczym
# TYLKO gdy ustawisz MC_EULA_ACCEPTED=true, czyli świadomie potwierdzisz, że akceptujesz EULA.
#   MC_EULA_ACCEPTED=true tools/vanilla_compare.sh
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
if [ "${MC_EULA_ACCEPTED:-}" != "true" ]; then
  echo "Ten test uruchamia serwer vanilla, co wymaga zaakceptowania EULA Mojang." >&2
  echo "Przeczytaj https://aka.ms/MinecraftEULA i, jeśli się zgadzasz, uruchom: MC_EULA_ACCEPTED=true $0" >&2
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
  echo "Serwer vanilla nie wystartował (zajęty port $VP / blokada świata?). Ostatnie linie:" >&2; tail -5 "$W/vanilla/server.out" >&2; exit 3
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
