//! The whole generation pipeline of one dimension: terrain, surface, carvers (per chunk) and the feature decoration (per 3x3 chunks).
//! The server and the verification tests run exactly this code.

use crate::biome_climate::BiomeClimate;
use crate::biome_manager::{BiomeManager, ChunkBiomes};
use crate::biome_source::BiomeSource;
use crate::carver::{apply_carvers, CarveEnv, CarveStates, CarverSet, CarvingMask};
use crate::decoration::{Decorator, FeatureRegistry};
use crate::feature::{block_state, Env};
use crate::noise_chunk::NoiseChunk;
use crate::random_state::{GeneratorSettings, NoiseSettings, RandomState, TerrainBlocks, WorldgenData};
use crate::region::{Region, RegionChunk};
use crate::surface::{GameBlockQuery, GenContext, SurfaceChunk, SurfaceRule, SurfaceStates, SurfaceSystem};
use crate::tags::BlockTags;
use anyhow::{anyhow, Context, Result};
use mc_data::GameData;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Which biome source a dimension uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BiomeSourceKind {
    Overworld,
    Nether,
    End,
}

/// The biome cells of a chunk of the area around the chunk being generated.
pub type GridFn<'a> = &'a dyn Fn(i32, i32) -> Arc<ChunkBiomes>;

pub struct NoiseGenerator {
    pub min_y: i32,
    pub height: i32,
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
    pub decorator: Decorator,
    pub features: FeatureRegistry,
    game: Arc<GameData>,
    air: u16,
    void_air: u16,
}

impl NoiseGenerator {
    /// `settings_name` is a noise setting of the data pack (`minecraft:overworld`, `minecraft:amplified`, ...); `min_y` / `height` the
    /// dimension's build range.
    pub fn new(game: Arc<GameData>, min_y: i32, height: i32, seed: i64, settings_name: &str, kind: BiomeSourceKind) -> Result<Self> {
        let worldgen_dir = game.root.join("data/minecraft/worldgen");
        let data = WorldgenData::load(&worldgen_dir).context("loading the worldgen data")?;
        let json = data.noise_settings.get(settings_name).ok_or_else(|| anyhow!("unknown noise settings {settings_name}"))?;
        let settings = GeneratorSettings::parse(json)?;
        let state = RandomState::new(&data, &settings, seed)?;
        let st = |n: &str| game.blocks.default_state(n).map(|s| s as u16).ok_or_else(|| anyhow!("missing block {n}"));
        let default_block = block_state(&game.blocks, &settings.default_block)?;
        let terrain_blocks = TerrainBlocks {
            air: st("minecraft:air")?,
            lava: st("minecraft:lava")?,
            default_block,
            default_fluid: block_state(&game.blocks, &settings.default_fluid)?,
            copper_ore: st("minecraft:copper_ore")?,
            raw_copper_block: st("minecraft:raw_copper_block")?,
            granite: st("minecraft:granite")?,
            deepslate_iron_ore: st("minecraft:deepslate_iron_ore")?,
            raw_iron_block: st("minecraft:raw_iron_block")?,
            tuff: st("minecraft:tuff")?,
        };
        // NoiseSettings.clampToHeightAccessor
        let nmin = settings.noise.min_y.max(min_y);
        let nheight = (settings.noise.min_y + settings.noise.height).min(min_y + height) - nmin;
        let noise = NoiseSettings { min_y: nmin, height: nheight, ..settings.noise };
        let range = GenContext { min_y: min_y.max(settings.noise.min_y), height: height.min(settings.noise.height) };
        let rule = SurfaceRule::compile(&settings.surface_rule, &state, &game.blocks, range)?;
        let carvers = CarverSet::load(&game.root, &game.blocks)?;
        let carve_states = CarveStates::resolve(&game.blocks)?;
        let system = SurfaceSystem::new(&state, default_block, settings.sea_level, SurfaceStates::resolve(&game.blocks)?)?;
        let source = match kind {
            BiomeSourceKind::Overworld => BiomeSource::overworld(),
            BiomeSourceKind::Nether => BiomeSource::nether(),
            BiomeSourceKind::End => BiomeSource::End,
        };
        let features = FeatureRegistry::load(&worldgen_dir)?;
        let tags = BlockTags::new(&game.root);
        let env = Env { blocks: &game.blocks, tags: &tags, configured: &features.configured };
        let decorator = Decorator::new(&features, &source.possible_biomes(), &env)?;
        Ok(Self {
            min_y,
            height,
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
            decorator,
            features,
            air: st("minecraft:air")?,
            void_air: st("minecraft:void_air")?,
            game,
        })
    }

    pub fn source(&self) -> &BiomeSource {
        &self.source
    }

    pub fn sampler(&self) -> &crate::climate::Sampler {
        &self.state.sampler
    }

    pub fn range(&self) -> GenContext {
        self.range
    }

    /// `ChunkAccess.fillBiomesFromNoise`
    pub fn fill_biomes(&self, cx: i32, cz: i32) -> ChunkBiomes {
        ChunkBiomes::fill(cx, cz, self.min_y, self.height, |qx, qy, qz| self.source.get_noise_biome(qx, qy, qz, &self.state.sampler))
    }

    /// Terrain, surface and carvers of one chunk: the blocks `(y - min_y) << 8 | z << 4 | x`. `grid` gives the biome cells of the
    /// chunks around (the surface reads one chunk further than the chunk itself).
    pub fn carved_chunk(&self, cx: i32, cz: i32, grid: GridFn) -> Vec<u16> {
        let tb = self.terrain_blocks;
        let quart_source = |qx: i32, qy: i32, qz: i32| grid(qx >> 2, qz >> 2).get_noise_biome(qx, qy, qz);
        let manager = BiomeManager::new(&quart_source, self.seed);
        let noise_chunk = NoiseChunk::new(&self.state, &self.settings, self.noise, tb, cx * 16, cz * 16);
        let filled = noise_chunk.fill(cx * 16, cz * 16);
        let mut all = vec![tb.air; 16 * 16 * self.height as usize];
        let offset = ((self.noise.min_y - self.min_y) as usize) * 256;
        all[offset..offset + filled.len()].copy_from_slice(&filled);
        let query = GameBlockQuery { blocks: &self.game.blocks, props: &self.game.block_props };
        let mut chunk = SurfaceChunk::new(cx, cz, self.min_y, self.height, all, &query);
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
        let mut mask = CarvingMask::new(self.height, self.min_y);
        apply_carvers(&env, &mut chunk, &mut mask, self.seed, &self.source, &self.state.sampler, &self.carvers);
        chunk.blocks
    }

    /// The feature decoration of chunk `center`: `carved` are the carved blocks of the 3x3 chunks around it (any order). The features write into
    /// all nine; the changed chunks are returned. `grid` gives the biome cells of the chunks around.
    pub fn decorate(&self, center: (i32, i32), carved: Vec<(i32, i32, Vec<u16>)>, grid: GridFn) -> Vec<(i32, i32, Vec<u16>)> {
        let query = GameBlockQuery { blocks: &self.game.blocks, props: &self.game.block_props };
        let chunks: Vec<RegionChunk> = carved.into_iter().map(|(cx, cz, blocks)| RegionChunk::new(cx, cz, self.min_y, self.height, blocks, &query)).collect();
        let quart_source = |qx: i32, qy: i32, qz: i32| grid(qx >> 2, qz >> 2).get_noise_biome(qx, qy, qz);
        let manager = BiomeManager::new(&quart_source, self.seed);
        let biome_fn = |x: i32, y: i32, z: i32| manager.get_biome(x, y, z);
        let mut region = Region::new(center, self.min_y, self.height, chunks, &query, &biome_fn, self.air, self.void_air);
        // the biomes of the 3x3 chunks that the biome source can produce
        let possible: HashSet<&'static str> = self.source.possible_biomes().into_iter().collect();
        let mut biomes: HashSet<&'static str> = HashSet::new();
        for dx in -1..=1 {
            for dz in -1..=1 {
                biomes.extend(grid(center.0 + dx, center.1 + dz).all_biomes().into_iter().filter(|b| possible.contains(b)));
            }
        }
        self.decorator.decorate(&mut region, self.seed, self.range, &biomes);
        region.into_chunks().into_iter().map(|c| (c.chunk_x, c.chunk_z, c.blocks)).collect()
    }
}
