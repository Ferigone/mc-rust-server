# Container image of the Rust Minecraft 1.21.1 server (used by Railway, see docs/DEPLOY.md).
#
# The vanilla data the server needs (registries, block states, tags, world generation JSON) is NOT part of the repository: the first stage
# downloads the official 1.21.1 server from Mojang, verifies its SHA-1 and runs Mojang's own data generator (tools/fetch_reference.sh).
# By building this image you confirm that you accept the Minecraft EULA (https://aka.ms/MinecraftEULA).

# --- 1. the vanilla data (needs a JDK: data generator, remapped classes for the block / item property tables)
FROM eclipse-temurin:21-jdk-jammy AS data
RUN apt-get update \
 && apt-get install -y --no-install-recommends curl python3 unzip ca-certificates \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY tools ./tools
RUN tools/fetch_reference.sh

# --- 2. the server binary
FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release --locked -p mc-server

# --- 3. runtime: no JDK, no toolchain
FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/mc-server /usr/local/bin/mc-server
# only what the server reads (no assets)
COPY --from=data /src/reference/gen/generated/reports /opt/mc-data/reports
COPY --from=data /src/reference/gen/generated/data /opt/mc-data/data
COPY docker/entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh
ENV MC_DATA_DIR=/opt/mc-data \
    MC_DIR=/data \
    RUST_LOG=info
# the world, server.properties and player data live in /data: mount a volume there (Railway rejects the VOLUME instruction;
# with plain Docker use -v NAME:/data)
WORKDIR /data
EXPOSE 25565
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
