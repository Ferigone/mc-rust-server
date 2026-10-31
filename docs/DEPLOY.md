# Deploying the server (Railway or any Docker host)

The repository contains a `Dockerfile`, `docker/entrypoint.sh` and a `railway.toml`. The image
builds the server from source and generates the vanilla data it needs (registries, block states, tags, world generation JSON) at build
time: it downloads the official Minecraft 1.21.1 server from Mojang, checks its SHA-1 and runs Mojang's own data generator. Nothing from
Mojang is stored in the repository. **By building the image you accept the Minecraft EULA (https://aka.ms/MinecraftEULA).**

## Railway

1. **New project** -> *Deploy from GitHub repo* -> `Ferigone/mc-rust-server`, branch `master`. Railway reads `railway.toml` and builds the
   `Dockerfile`. The first build takes roughly 10-15 minutes (data generator + a fat-LTO release build); later builds reuse the cached layers
   of the data stage.
2. **Volume**: add a volume to the service and mount it at `/data`. The world, `server.properties` and the player data live there; without
   a volume they are lost on every redeploy.
3. **TCP proxy**: Minecraft is plain TCP, not HTTP. In the service -> *Settings* -> *Networking* -> *TCP Proxy*, add a proxy for the
   application port `25565`. Railway shows an address like `something.proxy.rlwy.net:12345`.
4. **Connect**: Minecraft Java Edition **1.21.1** (or 1.21) -> *Multiplayer* -> *Add Server* -> that address. Other versions are rejected
   (protocol 767 only).

Give the service at least 1 GB of memory and 2 vCPUs: chunks are generated on all available cores (about 25 chunks per second on 4 cores; the
first join waits a few seconds for the spawn area).

### Variables

All optional; they are written to `server.properties` on every start, so change a variable and redeploy.

| Variable | Default | Meaning |
|---|---|---|
| `ONLINE_MODE` | `true` | Players are verified with Mojang's session servers. `false` lets anybody join under any name (use it only to find out whether a login problem comes from the authentication). |
| `MC_PORT` | `25565` | Port the server listens on (the TCP proxy must point at it). |
| `VIEW_DISTANCE` / `SIMULATION_DISTANCE` | `8` / `6` | Chunks around the player. |
| `ENFORCE_SECURE_PROFILE` | `false` | Chat signatures are not implemented yet, so this stays off. |
| `MOTD`, `MAX_PLAYERS`, `GAMEMODE`, `DIFFICULTY`, `HARDCORE`, `PVP`, `SPAWN_PROTECTION` | vanilla defaults | As in `server.properties`. |
| `LEVEL_NAME`, `LEVEL_SEED`, `LEVEL_TYPE` | `world`, random, `minecraft:normal` | `LEVEL_TYPE`: `minecraft:normal`, `minecraft:large_biomes`, `minecraft:amplified`, `minecraft:flat`. Only used when the world is created. |

## Anywhere else

```
docker build -t mc-rust-server .
docker run -p 25565:25565 -v mc-data:/data -e ONLINE_MODE=false mc-rust-server
```

## What you will see

This is a work in progress, not a finished port (see the README). The terrain, biomes, surface blocks, caves and canyons are generated exactly
like vanilla (verified against the real server classes); there are no trees, ores, flowers, structures or mobs yet, the Nether and End are not
reachable, and **the server has never been tested with the real game client** - the first connection from a real client is the experiment.
Server logs (Railway: *Deployments* -> *View logs*) show every connection and any error; please send them if a login or a chunk fails.
