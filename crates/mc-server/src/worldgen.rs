//! The chunk generator of the server: the ported world generation pipeline (noise terrain, biomes, surface) behind a thread pool.
//!
//! Everything in `crates/mc-worldgen` is verified against the Mojang classes; this module only glues it to the server's chunk model.
//! Missing stages (features, structures) are listed in `docs/ROADMAP.md`: generated terrain has caves and canyons but no trees or villages yet.

use crate::world::Dimension;
use anyhow::{anyhow, Context, Result};
use mc_data::GameData;
use mc_worldgen::biome_climate::BiomeClimate;
use mc_worldgen::biome_manager::{BiomeManager, ChunkBiomes};
use mc_worldgen::carver::{apply_carvers, CarveEnv, CarveStates, CarverSet, CarvingMask};
use mc_worldgen::biome_source::BiomeSource;
use mc_worldgen::noise_chunk::NoiseChunk;
use mc_worldgen::random_state::{GeneratorSettings, NoiseSettings, RandomState, TerrainBlocks, WorldgenData};
use mc_worldgen::surface::{GameBlockQuery, GenContext, SurfaceChunk, SurfaceRule, SurfaceStates, SurfaceSystem};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

/// Which biome source a dimension uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BiomeSourceKind {
    Overworld,
    Nether,
    End,
}

/// A generated chunk before it is converted to the server's chunk model.
pub struct Generated {
    /// block state IDs, `(y - min_y) << 8 | z << 4 | x`
    pub blocks: Vec<u16>,
    pub biomes: Arc<ChunkBiomes>,
}

pub struct Generator {
    dim: Dimension,
    pub seed: i64,
    settings: GeneratorSettings,
    noise: NoiseSettings,
    terrain_blocks: TerrainBlocks,
    state: RandomState,
    source: BiomeSource,
    system: SurfaceSystem,
    rule: SurfaceRule,
    range: GenContext,
    carvers: CarverSet,
    carve_states: CarveStates,
    climates: HashMap<String, BiomeClimate>,
    game: Arc<GameData>,
    biome_cache: Mutex<HashMap<(i32, i32), Arc<ChunkBiomes>>>,
}

fn block_state(game: &GameData, v: &Value) -> Result<u16> {
    let name = v["Name"].as_str().ok_or_else(|| anyhow!("block state without a name"))?;
    let props: Vec<(String, String)> = v
        .get("Properties")
        .and_then(Value::as_object)
        .map(|o| o.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_owned())).collect())
        .unwrap_or_default();
    game.blocks.state_id(name, props.iter().map(|(k, v)| (k.as_str(), v.as_str()))).map(|s| s as u16).ok_or_else(|| anyhow!("unknown block state {name}"))
}

impl Generator {
    /// `settings_name` is a noise setting of the data pack (`minecraft:overworld`, `minecraft:amplified`, ...).
    pub fn new(game: Arc<GameData>, dim: Dimension, seed: i64, settings_name: &str, kind: BiomeSourceKind) -> Result<Self> {
        let data = WorldgenData::load(&game.root.join("data/minecraft/worldgen")).context("loading the worldgen data")?;
        let json = data.noise_settings.get(settings_name).ok_or_else(|| anyhow!("unknown noise settings {settings_name}"))?;
        let settings = GeneratorSettings::parse(json)?;
        let state = RandomState::new(&data, &settings, seed)?;
        let st = |n: &str| game.blocks.default_state(n).map(|s| s as u16).ok_or_else(|| anyhow!("missing block {n}"));
        let default_block = block_state(&game, &settings.default_block)?;
        let terrain_blocks = TerrainBlocks {
            air: st("minecraft:air")?,
            lava: st("minecraft:lava")?,
            default_block,
            default_fluid: block_state(&game, &settings.default_fluid)?,
            copper_ore: st("minecraft:copper_ore")?,
            raw_copper_block: st("minecraft:raw_copper_block")?,
            granite: st("minecraft:granite")?,
            deepslate_iron_ore: st("minecraft:deepslate_iron_ore")?,
            raw_iron_block: st("minecraft:raw_iron_block")?,
            tuff: st("minecraft:tuff")?,
        };
        // NoiseSettings.clampToHeightAccessor
        let min = settings.noise.min_y.max(dim.min_y);
        let height = (settings.noise.min_y + settings.noise.height).min(dim.min_y + dim.height) - min;
        let noise = NoiseSettings { min_y: min, height, ..settings.noise };
        let range = GenContext { min_y: dim.min_y.max(settings.noise.min_y), height: dim.height.min(settings.noise.height) };
        let rule = SurfaceRule::compile(&settings.surface_rule, &state, &game.blocks, range)?;
        let carvers = CarverSet::load(&game.root, &game.blocks)?;
        let carve_states = CarveStates::resolve(&game.blocks)?;
        let system = SurfaceSystem::new(&state, default_block, settings.sea_level, SurfaceStates::resolve(&game.blocks)?)?;
        let source = match kind {
            BiomeSourceKind::Overworld => BiomeSource::overworld(),
            BiomeSourceKind::Nether => BiomeSource::nether(),
            BiomeSourceKind::End => BiomeSource::End,
        };
        Ok(Self {
            dim,
            seed,
            settings,
            noise,
            terrain_blocks,
            state,
            source,
            system,
            rule,
            range,
            carvers,
            carve_states,
            climates: data.biomes,
            game,
            biome_cache: Mutex::new(HashMap::new()),
        })
    }

    /// The biome cells of a chunk (cached: every chunk is needed for itself and as a neighbour of eight others).
    fn biomes_of(&self, cx: i32, cz: i32) -> Arc<ChunkBiomes> {
        if let Some(b) = self.biome_cache.lock().unwrap().get(&(cx, cz)) {
            return b.clone();
        }
        let grid = Arc::new(ChunkBiomes::fill(cx, cz, self.dim.min_y, self.dim.height, |qx, qy, qz| self.source.get_noise_biome(qx, qy, qz, &self.state.sampler)));
        let mut cache = self.biome_cache.lock().unwrap();
        if cache.len() > 4096 {
            cache.clear();
        }
        cache.insert((cx, cz), grid.clone());
        grid
    }

    /// `Climate.Sampler.findSpawnPosition` (the block x / z of the best spot for the world spawn)
    pub fn find_spawn_position(&self) -> (i32, i32) {
        self.state.sampler.find_spawn_position()
    }

    /// Terrain shape, biomes and surface of one chunk.
    pub fn generate(&self, cx: i32, cz: i32) -> Generated {
        let tb = self.terrain_blocks;
        let (min_y, height) = (self.dim.min_y, self.dim.height);
        let mut grids: HashMap<(i32, i32), Arc<ChunkBiomes>> = HashMap::with_capacity(9);
        for dx in -1..=1 {
            for dz in -1..=1 {
                grids.insert((cx + dx, cz + dz), self.biomes_of(cx + dx, cz + dz));
            }
        }
        let quart_source = |qx: i32, qy: i32, qz: i32| grids[&(qx >> 2, qz >> 2)].get_noise_biome(qx, qy, qz);
        let manager = BiomeManager::new(&quart_source, self.seed);
        let noise_chunk = NoiseChunk::new(&self.state, &self.settings, self.noise, tb, cx * 16, cz * 16);
        let filled = noise_chunk.fill(cx * 16, cz * 16);
        let mut all = vec![tb.air; 16 * 16 * height as usize];
        let offset = ((self.noise.min_y - min_y) as usize) * 256;
        all[offset..offset + filled.len()].copy_from_slice(&filled);
        let query = GameBlockQuery { blocks: &self.game.blocks, props: &self.game.block_props };
        let mut chunk = SurfaceChunk::new(cx, cz, min_y, height, all, &query);
        let biome_at = |x: i32, y: i32, z: i32| manager.get_biome(x, y, z);
        let climate = |n: &str| self.climates.get(n).cloned();
        self.system.build_surface(&mut chunk, &noise_chunk, &self.rule, &biome_at, &climate, &query, self.settings.legacy_random_source);
        // carvers: the biome manager of the carving context asks the biome source directly
        let direct = |qx: i32, qy: i32, qz: i32| self.source.get_noise_biome(qx, qy, qz, &self.state.sampler);
        let carve_manager = BiomeManager::new(&direct, self.seed);
        let carve_biome_at = |x: i32, y: i32, z: i32| carve_manager.get_biome(x, y, z);
        let env = CarveEnv {
            range: self.range,
            noise_chunk: &noise_chunk,
            system: &self.system,
            rule: &self.rule,
            biome_at: &carve_biome_at,
            climate: &climate,
            query: &query,
            states: self.carve_states,
        };
        let mut mask = CarvingMask::new(height, min_y);
        apply_carvers(&env, &mut chunk, &mut mask, self.seed, &self.source, &self.state.sampler, &self.carvers);
        Generated { blocks: chunk.blocks, biomes: grids.remove(&(cx, cz)).unwrap() }
    }
}

/// A pool of worker threads that generate chunks in the background.
pub struct GenPool {
    jobs: Sender<(i32, i32)>,
    results: Receiver<(i32, i32, Generated)>,
    in_flight: HashSet<(i32, i32)>,
}

impl GenPool {
    pub fn new(generator: Arc<Generator>, threads: usize) -> Self {
        let (job_tx, job_rx) = channel::<(i32, i32)>();
        let (res_tx, res_rx) = channel();
        let job_rx = Arc::new(Mutex::new(job_rx));
        for i in 0..threads.max(1) {
            let (generator, job_rx, res_tx) = (generator.clone(), job_rx.clone(), res_tx.clone());
            std::thread::Builder::new()
                .name(format!("worldgen-{i}"))
                .spawn(move || loop {
                    let job = job_rx.lock().unwrap().recv();
                    let Ok((cx, cz)) = job else { return };
                    let g = generator.generate(cx, cz);
                    if res_tx.send((cx, cz, g)).is_err() {
                        return;
                    }
                })
                .expect("spawning a worldgen thread");
        }
        Self { jobs: job_tx, results: res_rx, in_flight: HashSet::new() }
    }

    /// Queues a chunk unless it is already being generated.
    pub fn request(&mut self, cx: i32, cz: i32) {
        if self.in_flight.insert((cx, cz)) {
            let _ = self.jobs.send((cx, cz));
        }
    }

    /// Finished chunks (non-blocking).
    pub fn poll(&mut self) -> Vec<(i32, i32, Generated)> {
        let mut out = Vec::new();
        while let Ok(r) = self.results.try_recv() {
            self.in_flight.remove(&(r.0, r.1));
            out.push(r);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::time::Instant;

    fn generator(seed: i64) -> Option<Generator> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference/gen/generated");
        let game = Arc::new(GameData::load(&root).ok()?);
        Generator::new(game, Dimension { min_y: -64, height: 384 }, seed, "minecraft:overworld", BiomeSourceKind::Overworld).ok()
    }

    /// `cargo test --release -p mc-server worldgen -- --ignored --nocapture`: chunk generation speed
    #[test]
    #[ignore]
    fn generation_speed() {
        let Some(g) = generator(12345) else { return };
        let t0 = Instant::now();
        let first = g.generate(0, 0);
        eprintln!("first chunk (incl. biome grids of the 3x3 area): {} ms", t0.elapsed().as_millis());
        assert_eq!(first.blocks.len(), 16 * 16 * 384);
        let t1 = Instant::now();
        let mut n = 0;
        for cx in 1..5 {
            for cz in 1..5 {
                g.generate(cx, cz);
                n += 1;
            }
        }
        eprintln!("{n} more chunks: {} ms each", t1.elapsed().as_millis() / n);
        // the same on 4 threads (a fresh area, so the biome grids are computed too)
        let t2 = Instant::now();
        let next = std::sync::atomic::AtomicI32::new(0);
        std::thread::scope(|sc| {
            for _ in 0..4 {
                sc.spawn(|| loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if i >= 64 {
                        return;
                    }
                    g.generate(20 + i % 8, 20 + i / 8);
                });
            }
        });
        eprintln!("64 chunks on 4 threads: {} ms in total, {} ms per chunk", t2.elapsed().as_millis(), t2.elapsed().as_millis() / 64);
    }

    #[test]
    fn generated_chunks_have_a_surface_and_bedrock() {
        let Some(g) = generator(12345) else { return };
        let c = g.generate(3, -2);
        let bedrock = g.game.blocks.default_state("minecraft:bedrock").unwrap() as u16;
        // y = -64 is the lowest layer: bedrock below the bedrock_floor gradient (y -64 is always bedrock)
        assert!((0..256).all(|i| c.blocks[i] == bedrock), "the lowest layer is bedrock");
        let air = g.game.blocks.default_state("minecraft:air").unwrap() as u16;
        assert!(c.blocks.iter().any(|&b| b == air) && c.blocks.iter().any(|&b| b != air));
    }
}
