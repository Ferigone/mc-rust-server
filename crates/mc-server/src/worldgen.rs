//! The chunk generator of the server: the ported world generation pipeline (noise terrain, biomes, surface) behind a thread pool.
//!
//! Everything in `crates/mc-worldgen` is verified against the Mojang classes; this module only glues it to the server's chunk model.
//! Missing stages (features, structures) are listed in `docs/ROADMAP.md`: generated terrain has caves and canyons but no trees or villages yet.

use crate::world::Dimension;
use anyhow::Result;
use mc_data::GameData;
use mc_worldgen::biome_manager::ChunkBiomes;
pub use mc_worldgen::generator::BiomeSourceKind;
use mc_worldgen::generator::NoiseGenerator;
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

/// A generated chunk before it is converted to the server's chunk model.
pub struct Generated {
    /// block state IDs, `(y - min_y) << 8 | z << 4 | x`
    pub blocks: Vec<u16>,
    pub biomes: Arc<ChunkBiomes>,
}

pub struct Generator {
    noise: NoiseGenerator,
    biome_cache: Mutex<HashMap<(i32, i32), Arc<ChunkBiomes>>>,
}

impl Generator {
    /// `settings_name` is a noise setting of the data pack (`minecraft:overworld`, `minecraft:amplified`, ...).
    pub fn new(game: Arc<GameData>, dim: Dimension, seed: i64, settings_name: &str, kind: BiomeSourceKind) -> Result<Self> {
        let noise = NoiseGenerator::new(game, dim.min_y, dim.height, seed, settings_name, kind)?;
        Ok(Self { noise, biome_cache: Mutex::new(HashMap::new()) })
    }

    /// The biome cells of a chunk (cached: every chunk is needed for itself and as a neighbour of eight others).
    fn biomes_of(&self, cx: i32, cz: i32) -> Arc<ChunkBiomes> {
        if let Some(b) = self.biome_cache.lock().unwrap().get(&(cx, cz)) {
            return b.clone();
        }
        let grid = Arc::new(self.noise.fill_biomes(cx, cz));
        let mut cache = self.biome_cache.lock().unwrap();
        if cache.len() > 4096 {
            cache.clear();
        }
        cache.insert((cx, cz), grid.clone());
        grid
    }

    /// `Climate.Sampler.findSpawnPosition` (the block x / z of the best spot for the world spawn)
    pub fn find_spawn_position(&self) -> (i32, i32) {
        self.noise.sampler().find_spawn_position()
    }

    /// Terrain shape, biomes, surface and carvers of one chunk.
    pub fn generate(&self, cx: i32, cz: i32) -> Generated {
        let grid = |x: i32, z: i32| self.biomes_of(x, z);
        let blocks = self.noise.carved_chunk(cx, cz, &grid);
        Generated { blocks, biomes: self.biomes_of(cx, cz) }
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

    fn generator(seed: i64) -> Option<(Generator, Arc<GameData>)> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference/gen/generated");
        let game = Arc::new(GameData::load(&root).ok()?);
        let g = Generator::new(game.clone(), Dimension { min_y: -64, height: 384 }, seed, "minecraft:overworld", BiomeSourceKind::Overworld).ok()?;
        Some((g, game))
    }

    /// `cargo test --release -p mc-server worldgen -- --ignored --nocapture`: chunk generation speed
    #[test]
    #[ignore]
    fn generation_speed() {
        let Some((g, _)) = generator(12345) else { return };
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
        let Some((g, game)) = generator(12345) else { return };
        let c = g.generate(3, -2);
        let bedrock = game.blocks.default_state("minecraft:bedrock").unwrap() as u16;
        // y = -64 is the lowest layer: bedrock below the bedrock_floor gradient (y -64 is always bedrock)
        assert!((0..256).all(|i| c.blocks[i] == bedrock), "the lowest layer is bedrock");
        let air = game.blocks.default_state("minecraft:air").unwrap() as u16;
        assert!(c.blocks.iter().any(|&b| b == air) && c.blocks.iter().any(|&b| b != air));
    }
}
