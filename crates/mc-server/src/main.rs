//! Minecraft Java 1.21.1 (protocol 767) server written in Rust: a port of the vanilla server.

mod anvil;
mod chunk_io;
mod auth;
mod config;
mod game;
mod level;
mod light;
mod net;
mod playerdata;
mod prepare;
mod world;

use anyhow::{Context, Result};
use config::{Properties, Settings};
use mc_data::GameData;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, AtomicU64};
use std::sync::{Arc, Mutex};
use tracing::info;

fn find_data_dir(arg: Option<PathBuf>) -> Result<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(a) = arg {
        candidates.push(a);
    }
    if let Ok(e) = std::env::var("MC_DATA_DIR") {
        candidates.push(e.into());
    }
    for base in ["reference/gen/generated", "../reference/gen/generated", "../../reference/gen/generated"] {
        candidates.push(base.into());
    }
    for c in &candidates {
        if c.join("reports/registries.json").exists() {
            return Ok(c.clone());
        }
    }
    anyhow::bail!(
        "Reference data not found. Run `tools/fetch_reference.sh` (downloads the server from Mojang and runs the official data generator) \
         or point to the directory with --data <dir> / MC_DATA_DIR."
    )
}

fn main() -> Result<()> {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into())).init();

    let mut data_arg = None;
    let mut run_dir: Option<PathBuf> = None;
    let mut check_light = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--data" => data_arg = args.next().map(PathBuf::from),
            "--dir" => run_dir = args.next().map(PathBuf::from),
            "nogui" | "--nogui" => {}
            "--check-light" => check_light = true,
            "-h" | "--help" => {
                println!("mc-server [--data <generated dir>] [--dir <server dir>]");
                return Ok(());
            }
            other => anyhow::bail!("unknown argument: {other}"),
        }
    }
    // Resolve the data path relative to the current directory before changing it.
    let data_dir = find_data_dir(data_arg)?.canonicalize()?;
    if let Some(d) = &run_dir {
        std::fs::create_dir_all(d)?;
        std::env::set_current_dir(d)?;
    }

    let props = Properties::load_or_create(Path::new("server.properties"))?;
    let settings = Settings::from(&props);
    info!("Starting minecraft server version {} (protocol {})", net::VERSION_NAME, mc_protocol::ids::PROTOCOL_VERSION);

    let t0 = std::time::Instant::now();
    let data = Arc::new(GameData::load(&data_dir).context("loading game data")?);
    let config = prepare::ConfigPackets::build(&data);
    info!(
        "Game data loaded in {} ms ({} blocks, {} states, {} static registries)",
        t0.elapsed().as_millis(),
        data.blocks.list.len(),
        data.blocks.state_count,
        data.registries.len()
    );

    // --- world: an existing Anvil world in `level-name`, or a new flat world (the normal generator is not ported yet)
    let dim_json = &data.dynamic_registry("minecraft:dimension_type").unwrap().entries.iter().find(|e| e.id == "minecraft:overworld").unwrap().json;
    let dim = world::Dimension { min_y: dim_json["min_y"].as_i64().unwrap() as i32, height: dim_json["height"].as_i64().unwrap() as i32 };
    let st = |n: &str| data.blocks.default_state(n).unwrap_or_else(|| panic!("missing block {n}"));
    let palette = world::Palette {
        air: st("minecraft:air") as u16,
        cave_air: st("minecraft:cave_air") as u16,
        void_air: st("minecraft:void_air") as u16,
        block_state_count: data.blocks.state_count,
        biome_count: data.dynamic_registry("minecraft:worldgen/biome").unwrap().entries.len() as u32,
        fluid: std::sync::Arc::new((0..data.blocks.state_count).map(|s| data.block_props.state(s).is_some_and(|p| p.has(mc_data::props::flag::FLUID))).collect()),
    };
    let new_world = world::NewWorld {
        name: settings.level_name.clone(),
        seed: config::parse_seed(&settings.level_seed),
        game_type: settings.game_mode as i32,
        difficulty: settings.difficulty,
        hardcore: settings.hardcore,
    };
    let mut world = world::World::open(PathBuf::from(&settings.level_name), data.clone(), dim, palette, new_world)?;
    if let Some(level) = world.level.as_mut() {
        // DedicatedServer.forceDifficulty: server.properties wins on every start (hardcore forces HARD)
        let hardcore = level.hardcore();
        level.data_mut().put("Difficulty", mc_protocol::nbt::Tag::Byte(if hardcore { 3 } else { settings.difficulty as i8 }));
    }

    if check_light {
        // verification mode (tools/light_check.sh): recompute the light of the stored chunks and compare it with vanilla's
        let ok = light::check_world(&mut world, &data.block_props, 64);
        std::process::exit(if ok { 0 } else { 1 });
    }

    let ids = game::Ids {
        dimension_type: data.dynamic_id("minecraft:dimension_type", "minecraft:overworld").unwrap(),
        player_entity_type: data.registry_id("minecraft:entity_type", "minecraft:player").unwrap(),
        chat_type: data.dynamic_id("minecraft:chat_type", "minecraft:chat").unwrap(),
        overworld: "minecraft:overworld",
        attr_movement_speed: data.registry_id("minecraft:attribute", "minecraft:generic.movement_speed").unwrap(),
        attr_entity_range: data.registry_id("minecraft:attribute", "minecraft:player.entity_interaction_range").unwrap(),
        attr_block_range: data.registry_id("minecraft:attribute", "minecraft:player.block_interaction_range").unwrap(),
    };

    // Server icon: vanilla requires a 64x64 PNG (otherwise it skips it). Note: vanilla re-encodes the PNG through
    // ImageIO, we send the file unchanged: the image is identical, the base64 bytes may differ.
    let favicon = std::fs::read("server-icon.png").ok().and_then(|b| {
        let ok = b.len() > 24 && &b[..8] == b"\x89PNG\r\n\x1a\n" && u32::from_be_bytes(b[16..20].try_into().unwrap()) == 64 && u32::from_be_bytes(b[20..24].try_into().unwrap()) == 64;
        if !ok {
            tracing::warn!("server-icon.png must be a 64x64 PNG image, skipping it");
        }
        ok.then(|| format!("data:image/png;base64,{}", base64(&b)))
    });
    let rsa = if settings.online_mode {
        info!("Generating the RSA key pair (online-mode)");
        Some(auth::ServerKey::generate()?)
    } else {
        tracing::warn!("**** SERVER IS RUNNING IN OFFLINE/INSECURE MODE!");
        None
    };

    let (tx, rx) = std::sync::mpsc::channel();
    let shared = Arc::new(net::Shared {
        settings,
        config,
        rsa,
        events: tx,
        online: AtomicI32::new(0),
        status: Mutex::new(net::StatusInfo::default()),
        favicon,
        next_session: AtomicU64::new(1),
    });

    let game = game::Game::new(shared.clone(), data.clone(), ids, world);
    let game_thread = std::thread::Builder::new().name("game".into()).spawn(move || game.run(rx))?;

    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().thread_name("net").build()?;
    let result: Result<()> = rt.block_on(async {
        info!("Done ({} ms)! For help, type \"help\"", t0.elapsed().as_millis());
        tokio::select! {
            r = net::serve(shared.clone()) => r,
            _ = stop_signal() => { info!("Stopping server"); Ok(()) }
        }
    });
    let _ = shared.events.send(net::GameEvent::Shutdown);
    let _ = game_thread.join();
    result
}

/// Resolves on Ctrl+C or, on Unix, SIGTERM (what `docker stop` and service managers send), so the world is saved.
async fn stop_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}
