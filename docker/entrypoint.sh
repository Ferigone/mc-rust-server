#!/bin/sh
# Writes server.properties from environment variables (every start, so changing a variable and redeploying takes effect), then starts the server.
# Variables (all optional):
#   ONLINE_MODE (true)        verify players with Mojang's session servers; "false" lets anybody join under any name
#   MC_PORT (25565)           port the server listens on (point the Railway TCP proxy at it)
#   MOTD, MAX_PLAYERS, GAMEMODE, DIFFICULTY, HARDCORE, PVP, LEVEL_NAME, LEVEL_SEED, LEVEL_TYPE,
#   VIEW_DISTANCE (8), SIMULATION_DISTANCE (6), SPAWN_PROTECTION, ENFORCE_SECURE_PROFILE (false)
set -eu

DIR="${MC_DIR:-/data}"
mkdir -p "$DIR"
PROPS="$DIR/server.properties"
touch "$PROPS"

# setprop KEY VALUE: replaces the line (the file uses Java's escaping for ":" "=" and "\")
setprop() {
  esc=$(printf '%s' "$2" | sed 's/\\/\\\\/g; s/:/\\:/g; s/=/\\=/g')
  awk -F= -v k="$1" '$1 != k' "$PROPS" > "$PROPS.tmp" || true
  printf '%s=%s\n' "$1" "$esc" >> "$PROPS.tmp"
  mv "$PROPS.tmp" "$PROPS"
}
# opt KEY VAR [DEFAULT]: writes KEY from the environment variable VAR, or DEFAULT when it is unset; nothing when both are missing
opt() {
  eval "val=\${$2:-}"
  if [ -n "$val" ]; then setprop "$1" "$val"; elif [ "$#" -ge 3 ]; then setprop "$1" "$3"; fi
}

opt server-port MC_PORT 25565
opt online-mode ONLINE_MODE true
opt enforce-secure-profile ENFORCE_SECURE_PROFILE false
opt view-distance VIEW_DISTANCE 8
opt simulation-distance SIMULATION_DISTANCE 6
opt motd MOTD
opt max-players MAX_PLAYERS
opt gamemode GAMEMODE
opt difficulty DIFFICULTY
opt hardcore HARDCORE
opt pvp PVP
opt level-name LEVEL_NAME
opt level-seed LEVEL_SEED
opt level-type LEVEL_TYPE
opt spawn-protection SPAWN_PROTECTION

echo "mc-server: data $MC_DATA_DIR, world directory $DIR"
grep -E '^(server-port|online-mode|view-distance|level-name|level-seed|level-type)=' "$PROPS" | sed 's/^/  /'
exec mc-server --data "$MC_DATA_DIR" --dir "$DIR"
