//! `SurfaceSystem` and `SurfaceRules`: replaces the stone of the freshly generated terrain with the surface blocks of the biomes
//! (grass, sand, terracotta bands, ice, bedrock, ...), driven by the `surface_rule` of the noise settings.
//!
//! The rule tree is "compiled" once per `RandomState` (noises and random factories are resolved up front); the lazily cached
//! conditions of the Java classes are cached per column / per block with epoch counters, which does not change any value.

use crate::biome_climate::BiomeClimate;
use crate::biomes as b;
use crate::mth;
use crate::noise::NormalNoise;
use crate::noise_chunk::NoiseChunk;
use crate::random::{PositionalRandomFactory, RandomSource};
use crate::random_state::RandomState;
use anyhow::{anyhow, bail, Result};
use mc_data::Blocks;
use serde_json::Value;
use std::cell::Cell;
use std::sync::Arc;

/// `DimensionType.WAY_BELOW_MIN_Y`
const WAY_BELOW_MIN_Y: i32 = -2032 << 4;

/// What the surface stage needs to know about block states (the caller's numbering).
pub trait BlockQuery {
    fn is_air(&self, state: u16) -> bool;
    /// `!state.getFluidState().isEmpty()`
    fn has_fluid(&self, state: u16) -> bool;
    /// `a.is(b.getBlock())`
    fn same_block(&self, a: u16, b: u16) -> bool;
}

/// `BlockQuery` backed by the generated game data (`blocks.json` and `block_props.tsv`).
pub struct GameBlockQuery<'a> {
    pub blocks: &'a Blocks,
    pub props: &'a mc_data::BlockProps,
}

impl BlockQuery for GameBlockQuery<'_> {
    fn is_air(&self, state: u16) -> bool {
        self.props.state(state as u32).is_some_and(|s| s.has(mc_data::props::flag::AIR))
    }

    fn has_fluid(&self, state: u16) -> bool {
        self.props.state(state as u32).is_some_and(|s| s.has(mc_data::props::flag::FLUID))
    }

    fn same_block(&self, a: u16, b: u16) -> bool {
        match (self.blocks.block_of_state(a as u32), self.blocks.block_of_state(b as u32)) {
            (Some(x), Some(y)) => std::ptr::eq(x, y),
            _ => false,
        }
    }
}

/// The block states the surface system itself places.
#[derive(Clone, Copy, Debug)]
pub struct SurfaceStates {
    pub white_terracotta: u16,
    pub orange_terracotta: u16,
    pub terracotta: u16,
    pub yellow_terracotta: u16,
    pub brown_terracotta: u16,
    pub red_terracotta: u16,
    pub light_gray_terracotta: u16,
    pub packed_ice: u16,
    pub snow_block: u16,
    pub water: u16,
    pub air: u16,
}

impl SurfaceStates {
    pub fn resolve(blocks: &Blocks) -> Result<Self> {
        let st = |n: &str| blocks.default_state(n).map(|s| s as u16).ok_or_else(|| anyhow!("unknown block {n}"));
        Ok(Self {
            white_terracotta: st("minecraft:white_terracotta")?,
            orange_terracotta: st("minecraft:orange_terracotta")?,
            terracotta: st("minecraft:terracotta")?,
            yellow_terracotta: st("minecraft:yellow_terracotta")?,
            brown_terracotta: st("minecraft:brown_terracotta")?,
            red_terracotta: st("minecraft:red_terracotta")?,
            light_gray_terracotta: st("minecraft:light_gray_terracotta")?,
            packed_ice: st("minecraft:packed_ice")?,
            snow_block: st("minecraft:snow_block")?,
            water: st("minecraft:water")?,
            air: st("minecraft:air")?,
        })
    }
}

/// `WorldGenerationContext`: the vertical range vertical anchors resolve against.
#[derive(Clone, Copy, Debug)]
pub struct GenContext {
    pub min_y: i32,
    pub height: i32,
}

#[derive(Clone, Copy, Debug)]
pub enum Anchor {
    Absolute(i32),
    AboveBottom(i32),
    BelowTop(i32),
}

impl Anchor {
    pub fn parse(v: &Value) -> Result<Self> {
        if let Some(y) = v.get("absolute").and_then(Value::as_i64) {
            Ok(Anchor::Absolute(y as i32))
        } else if let Some(y) = v.get("above_bottom").and_then(Value::as_i64) {
            Ok(Anchor::AboveBottom(y as i32))
        } else if let Some(y) = v.get("below_top").and_then(Value::as_i64) {
            Ok(Anchor::BelowTop(y as i32))
        } else {
            bail!("bad vertical anchor {v}")
        }
    }

    pub fn resolve(&self, g: GenContext) -> i32 {
        match *self {
            Anchor::Absolute(y) => y,
            Anchor::AboveBottom(o) => g.min_y + o,
            Anchor::BelowTop(o) => g.height - 1 + g.min_y - o,
        }
    }
}

/// A compiled `SurfaceRules.ConditionSource`.
enum Cond {
    Biome(Vec<String>),
    Noise { id: usize, noise: Arc<NormalNoise>, min: f64, max: f64 },
    Gradient { factory: PositionalRandomFactory, true_at_and_below: i32, false_at_and_above: i32 },
    YAbove { anchor: i32, surface_depth_multiplier: i32, add_stone_depth: bool },
    Water { offset: i32, surface_depth_multiplier: i32, add_stone_depth: bool },
    Temperature,
    Steep { id: usize },
    Not(Box<Cond>),
    Hole,
    AbovePreliminarySurface,
    StoneDepth { offset: i32, add_surface_depth: bool, secondary_depth_range: i32, ceiling: bool },
}

/// A compiled `SurfaceRules.RuleSource`.
enum Rule {
    Bandlands,
    State(u16),
    Sequence(Vec<Rule>),
    Test(Cond, Box<Rule>),
}

/// The rule tree plus the number of cached conditions in it.
pub struct SurfaceRule {
    rule: Rule,
    cache_slots: usize,
}

impl SurfaceRule {
    /// Compiles the `surface_rule` JSON of a noise setting.
    pub fn compile(json: &Value, state: &RandomState, blocks: &Blocks, range: GenContext) -> Result<Self> {
        let slots = Cell::new(0usize);
        let env = CompileEnv { state, blocks, range, slots: &slots };
        let rule = compile_rule(json, &env)?;
        Ok(Self { rule, cache_slots: slots.get() })
    }
}

struct CompileEnv<'a> {
    state: &'a RandomState,
    blocks: &'a Blocks,
    range: GenContext,
    slots: &'a Cell<usize>,
}

impl CompileEnv<'_> {
    fn slot(&self) -> usize {
        let id = self.slots.get();
        self.slots.set(id + 1);
        id
    }
}

fn int(v: &Value, k: &str) -> Result<i32> {
    v.get(k).and_then(Value::as_i64).map(|i| i as i32).ok_or_else(|| anyhow!("surface rule: missing integer {k}"))
}

fn boolean(v: &Value, k: &str) -> Result<bool> {
    v.get(k).and_then(Value::as_bool).ok_or_else(|| anyhow!("surface rule: missing boolean {k}"))
}

fn double(v: &Value, k: &str) -> Result<f64> {
    v.get(k).and_then(Value::as_f64).ok_or_else(|| anyhow!("surface rule: missing number {k}"))
}

fn compile_rule(v: &Value, env: &CompileEnv) -> Result<Rule> {
    let ty = v.get("type").and_then(Value::as_str).ok_or_else(|| anyhow!("surface rule without a type: {v}"))?;
    Ok(match ty {
        "minecraft:bandlands" => Rule::Bandlands,
        "minecraft:block" => {
            let s = v.get("result_state").ok_or_else(|| anyhow!("block rule without result_state"))?;
            let name = s.get("Name").and_then(Value::as_str).ok_or_else(|| anyhow!("block state without a name"))?;
            let props: Vec<(String, String)> = s
                .get("Properties")
                .and_then(Value::as_object)
                .map(|o| o.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_owned())).collect())
                .unwrap_or_default();
            let id = env.blocks.state_id(name, props.iter().map(|(k, v)| (k.as_str(), v.as_str()))).ok_or_else(|| anyhow!("unknown block state {name}"))?;
            Rule::State(id as u16)
        }
        "minecraft:sequence" => {
            let list = v.get("sequence").and_then(Value::as_array).ok_or_else(|| anyhow!("sequence without a list"))?;
            Rule::Sequence(list.iter().map(|r| compile_rule(r, env)).collect::<Result<_>>()?)
        }
        "minecraft:condition" => {
            let cond = compile_cond(v.get("if_true").ok_or_else(|| anyhow!("condition rule without if_true"))?, env)?;
            Rule::Test(cond, Box::new(compile_rule(v.get("then_run").ok_or_else(|| anyhow!("condition rule without then_run"))?, env)?))
        }
        other => bail!("unknown surface rule {other}"),
    })
}

fn compile_cond(v: &Value, env: &CompileEnv) -> Result<Cond> {
    let ty = v.get("type").and_then(Value::as_str).ok_or_else(|| anyhow!("surface condition without a type: {v}"))?;
    Ok(match ty {
        "minecraft:biome" => Cond::Biome(
            v.get("biome_is")
                .and_then(Value::as_array)
                .ok_or_else(|| anyhow!("biome condition without biome_is"))?
                .iter()
                .map(|b| b.as_str().map(str::to_owned).ok_or_else(|| anyhow!("bad biome name")))
                .collect::<Result<_>>()?,
        ),
        "minecraft:noise_threshold" => {
            let key = v.get("noise").and_then(Value::as_str).ok_or_else(|| anyhow!("noise condition without noise"))?;
            Cond::Noise { id: env.slot(), noise: env.state.get_or_create_noise(key)?, min: double(v, "min_threshold")?, max: double(v, "max_threshold")? }
        }
        "minecraft:vertical_gradient" => Cond::Gradient {
            factory: env.state.get_or_create_random_factory(v.get("random_name").and_then(Value::as_str).ok_or_else(|| anyhow!("gradient without random_name"))?),
            true_at_and_below: Anchor::parse(&v["true_at_and_below"])?.resolve(env.range),
            false_at_and_above: Anchor::parse(&v["false_at_and_above"])?.resolve(env.range),
        },
        "minecraft:y_above" => Cond::YAbove {
            anchor: Anchor::parse(&v["anchor"])?.resolve(env.range),
            surface_depth_multiplier: int(v, "surface_depth_multiplier")?,
            add_stone_depth: boolean(v, "add_stone_depth")?,
        },
        "minecraft:water" => Cond::Water {
            offset: int(v, "offset")?,
            surface_depth_multiplier: int(v, "surface_depth_multiplier")?,
            add_stone_depth: boolean(v, "add_stone_depth")?,
        },
        "minecraft:temperature" => Cond::Temperature,
        "minecraft:steep" => Cond::Steep { id: env.slot() },
        "minecraft:not" => Cond::Not(Box::new(compile_cond(v.get("invert").ok_or_else(|| anyhow!("not without invert"))?, env)?)),
        "minecraft:hole" => Cond::Hole,
        "minecraft:above_preliminary_surface" => Cond::AbovePreliminarySurface,
        "minecraft:stone_depth" => Cond::StoneDepth {
            offset: int(v, "offset")?,
            add_surface_depth: boolean(v, "add_surface_depth")?,
            secondary_depth_range: int(v, "secondary_depth_range")?,
            ceiling: match v.get("surface_type").and_then(Value::as_str) {
                Some("ceiling") => true,
                Some("floor") => false,
                other => bail!("bad surface_type {other:?}"),
            },
        },
        other => bail!("unknown surface condition {other}"),
    })
}

// ------------------------------------------------------------------------------------------------ the chunk being built

/// The blocks of a chunk during the surface stage plus its `WORLD_SURFACE_WG` heightmap.
pub struct SurfaceChunk {
    pub chunk_x: i32,
    pub chunk_z: i32,
    pub min_y: i32,
    pub height: i32,
    /// `(y - min_y) << 8 | z << 4 | x`
    pub blocks: Vec<u16>,
    /// `Heightmap.getFirstAvailable` for `WORLD_SURFACE_WG`, indexed `x + z * 16` (absolute y)
    pub world_surface_wg: [i32; 256],
    /// positions of fluids placed by the surface stage (`markPosForPostprocessing`)
    pub post_process: Vec<(i32, i32, i32)>,
}

impl SurfaceChunk {
    /// Wraps the output of `NoiseChunk::fill`; the heightmap is "highest non-air block + 1" (what the fill loop maintains).
    pub fn new(chunk_x: i32, chunk_z: i32, min_y: i32, height: i32, blocks: Vec<u16>, q: &dyn BlockQuery) -> Self {
        let mut hm = [min_y; 256];
        for z in 0..16usize {
            for x in 0..16usize {
                for y in (0..height).rev() {
                    if !q.is_air(blocks[((y as usize) << 8) | (z << 4) | x]) {
                        hm[x + z * 16] = min_y + y + 1;
                        break;
                    }
                }
            }
        }
        Self { chunk_x, chunk_z, min_y, height, blocks, world_surface_wg: hm, post_process: Vec::new() }
    }

    pub(crate) fn get(&self, x: i32, y: i32, z: i32, air: u16) -> u16 {
        if y < self.min_y || y >= self.min_y + self.height {
            return air; // VOID_AIR
        }
        self.blocks[(((y - self.min_y) as usize) << 8) | ((z & 15) << 4) as usize | (x & 15) as usize]
    }

    /// `BlockColumn.setBlock`: `ProtoChunk.setBlockState` (heightmap update included) and, for fluids, `markPosForPostprocessing`.
    fn set(&mut self, x: i32, y: i32, z: i32, state: u16, q: &dyn BlockQuery, air: u16) {
        self.set_block(x, y, z, state, q, air, true);
    }

    /// `ProtoChunk.setBlockState` (heightmap update included); `mark_fluid` adds fluids to the post-processing list.
    pub(crate) fn set_block(&mut self, x: i32, y: i32, z: i32, state: u16, q: &dyn BlockQuery, air: u16, mark_fluid: bool) {
        if y < self.min_y || y >= self.min_y + self.height {
            return;
        }
        self.blocks[(((y - self.min_y) as usize) << 8) | ((z & 15) << 4) as usize | (x & 15) as usize] = state;
        // Heightmap.update
        let idx = ((x & 15) + (z & 15) * 16) as usize;
        let first_free = self.world_surface_wg[idx];
        if y > first_free - 2 {
            if !q.is_air(state) {
                if y >= first_free {
                    self.world_surface_wg[idx] = y + 1;
                }
            } else if first_free - 1 == y {
                let mut found = self.min_y;
                let mut i = y - 1;
                while i >= self.min_y {
                    if !q.is_air(self.get(x, i, z, air)) {
                        found = i + 1;
                        break;
                    }
                    i -= 1;
                }
                self.world_surface_wg[idx] = found;
            }
        }
        if mark_fluid && q.has_fluid(state) {
            self.post_process.push((x, y, z));
        }
    }

    /// The blocks as `ChunkAccess.getBlockState` reports them: `LevelChunkSection.hasOnlyAir()` sections answer plain `air` for every
    /// position, even where they hold `cave_air` (a vanilla quirk: the stored section keeps the `cave_air`).
    pub fn vanilla_view(&self, q: &dyn BlockQuery, air: u16) -> Vec<u16> {
        let mut out = self.blocks.clone();
        for section in out.chunks_mut(4096) {
            if section.iter().all(|&b| q.is_air(b)) {
                section.fill(air);
            }
        }
        out
    }

    /// `ChunkAccess.getHeight(WORLD_SURFACE_WG, x, z)`: the y of the highest block (the first available y minus one)
    pub fn height_at(&self, lx: i32, lz: i32) -> i32 {
        self.world_surface_wg[(lx + lz * 16) as usize] - 1
    }
}

// ------------------------------------------------------------------------------------------------ the system

/// `SurfaceSystem`
pub struct SurfaceSystem {
    default_block: u16,
    sea_level: i32,
    clay_bands: Vec<u16>,
    clay_bands_offset_noise: Arc<NormalNoise>,
    badlands_pillar_noise: Arc<NormalNoise>,
    badlands_pillar_roof_noise: Arc<NormalNoise>,
    badlands_surface_noise: Arc<NormalNoise>,
    iceberg_pillar_noise: Arc<NormalNoise>,
    iceberg_pillar_roof_noise: Arc<NormalNoise>,
    iceberg_surface_noise: Arc<NormalNoise>,
    surface_noise: Arc<NormalNoise>,
    surface_secondary_noise: Arc<NormalNoise>,
    noise_random: PositionalRandomFactory,
    states: SurfaceStates,
}

/// `Math.round(double)`
fn java_round(x: f64) -> i64 {
    let f = x.floor();
    (if x - f >= 0.5 { f + 1.0 } else { f }) as i64
}

impl SurfaceSystem {
    pub fn new(state: &RandomState, default_block: u16, sea_level: i32, states: SurfaceStates) -> Result<Self> {
        let n = |k: &str| state.get_or_create_noise(k);
        let noise_random = state.random.clone();
        let mut bands_random = noise_random.from_hash_of("minecraft:clay_bands");
        Ok(Self {
            default_block,
            sea_level,
            clay_bands: generate_bands(&mut *bands_random, &states),
            clay_bands_offset_noise: n("minecraft:clay_bands_offset")?,
            surface_noise: n("minecraft:surface")?,
            surface_secondary_noise: n("minecraft:surface_secondary")?,
            badlands_pillar_noise: n("minecraft:badlands_pillar")?,
            badlands_pillar_roof_noise: n("minecraft:badlands_pillar_roof")?,
            badlands_surface_noise: n("minecraft:badlands_surface")?,
            iceberg_pillar_noise: n("minecraft:iceberg_pillar")?,
            iceberg_pillar_roof_noise: n("minecraft:iceberg_pillar_roof")?,
            iceberg_surface_noise: n("minecraft:iceberg_surface")?,
            noise_random,
            states,
        })
    }

    /// `SurfaceSystem.getSurfaceDepth`
    fn surface_depth(&self, x: i32, z: i32) -> i32 {
        let n = self.surface_noise.get_value(x as f64, 0.0, z as f64);
        (n * 2.75 + 3.0 + self.noise_random.at(x, 0, z).next_double() * 0.25) as i32
    }

    fn surface_secondary(&self, x: i32, z: i32) -> f64 {
        self.surface_secondary_noise.get_value(x as f64, 0.0, z as f64)
    }

    /// `SurfaceSystem.getBand`
    fn band(&self, x: i32, y: i32, z: i32) -> u16 {
        let offset = java_round(self.clay_bands_offset_noise.get_value(x as f64, 0.0, z as f64) * 4.0) as i32;
        let len = self.clay_bands.len() as i32;
        self.clay_bands[((y + offset + len) % len) as usize]
    }

    /// `SurfaceSystem.buildSurface`. `biome_at(x, y, z)` is `BiomeManager.getBiome`, `climate` gives the climate of a biome.
    #[allow(clippy::too_many_arguments)]
    pub fn build_surface(
        &self,
        chunk: &mut SurfaceChunk,
        noise_chunk: &NoiseChunk,
        rule: &SurfaceRule,
        biome_at: &dyn Fn(i32, i32, i32) -> &'static str,
        climate: &dyn Fn(&str) -> Option<BiomeClimate>,
        q: &dyn BlockQuery,
        use_legacy_random: bool,
    ) {
        let air = self.states.air;
        let (min_x, min_z) = (chunk.chunk_x * 16, chunk.chunk_z * 16);
        let mut ctx = Context::new(self, noise_chunk, biome_at, climate, rule.cache_slots);
        for lx in 0..16 {
            for lz in 0..16 {
                let (x, z) = (min_x + lx, min_z + lz);
                let top = chunk.height_at(lx, lz) + 1;
                let biome = biome_at(x, if use_legacy_random { 0 } else { top }, z);
                if biome == b::ERODED_BADLANDS {
                    self.eroded_badlands_extension(chunk, x, z, top, q);
                }
                let top = chunk.height_at(lx, lz) + 1;
                ctx.update_xz(x, z);
                let mut stone_above = 0;
                let mut water_height = i32::MIN;
                let mut floor_y = i32::MAX;
                let min_build = chunk.min_y;
                let mut y = top;
                while y >= min_build {
                    let state = chunk.get(x, y, z, air);
                    if q.is_air(state) {
                        stone_above = 0;
                        water_height = i32::MIN;
                    } else if q.has_fluid(state) {
                        if water_height == i32::MIN {
                            water_height = y + 1;
                        }
                    } else {
                        if floor_y >= y {
                            floor_y = WAY_BELOW_MIN_Y;
                            let mut y2 = y - 1;
                            while y2 >= min_build - 1 {
                                let s2 = chunk.get(x, y2, z, air);
                                if q.is_air(s2) || q.has_fluid(s2) {
                                    floor_y = y2 + 1;
                                    break;
                                }
                                y2 -= 1;
                            }
                        }
                        stone_above += 1;
                        let stone_below = y - floor_y + 1;
                        ctx.update_y(stone_above, stone_below, water_height, x, y, z);
                        if state == self.default_block {
                            if let Some(s) = ctx.try_apply(&rule.rule, chunk, x, y, z) {
                                chunk.set(x, y, z, s, q, air);
                            }
                        }
                    }
                    y -= 1;
                }
                if biome == b::FROZEN_OCEAN || biome == b::DEEP_FROZEN_OCEAN {
                    let min_surface = ctx.min_surface_level();
                    if let Some(c) = climate(biome) {
                        self.frozen_ocean_extension(min_surface, &c, chunk, x, z, top, q);
                    }
                }
            }
        }
    }

    /// `SurfaceSystem.topMaterial`: what the surface rules put at `(x, y, z)` (used by the carvers to re-grass the dirt below a cave roof).
    #[allow(clippy::too_many_arguments)]
    pub fn top_material(
        &self,
        rule: &SurfaceRule,
        noise_chunk: &NoiseChunk,
        chunk: &SurfaceChunk,
        biome_at: &dyn Fn(i32, i32, i32) -> &'static str,
        climate: &dyn Fn(&str) -> Option<BiomeClimate>,
        (x, y, z): (i32, i32, i32),
        has_fluid: bool,
    ) -> Option<u16> {
        let mut ctx = Context::new(self, noise_chunk, biome_at, climate, rule.cache_slots);
        ctx.update_xz(x, z);
        ctx.update_y(1, 1, if has_fluid { y + 1 } else { i32::MIN }, x, y, z);
        ctx.try_apply(&rule.rule, chunk, x, y, z)
    }

    fn eroded_badlands_extension(&self, chunk: &mut SurfaceChunk, x: i32, z: i32, height: i32, q: &dyn BlockQuery) {
        let air = self.states.air;
        let v = f64::min(
            (self.badlands_surface_noise.get_value(x as f64, 0.0, z as f64) * 8.25).abs(),
            self.badlands_pillar_noise.get_value(x as f64 * 0.2, 0.0, z as f64 * 0.2) * 15.0,
        );
        if v <= 0.0 {
            return;
        }
        let roof = (self.badlands_pillar_roof_noise.get_value(x as f64 * 0.75, 0.0, z as f64 * 0.75) * 1.5).abs();
        let top = 64.0 + f64::min(v * v * 2.5, (roof * 50.0).ceil() + 24.0);
        let top_y = mth::floor(top);
        if height <= top_y {
            let mut y = top_y;
            while y >= chunk.min_y {
                let s = chunk.get(x, y, z, air);
                if q.same_block(s, self.default_block) {
                    break;
                }
                if q.same_block(s, self.states.water) {
                    return;
                }
                y -= 1;
            }
            let mut y = top_y;
            while y >= chunk.min_y && q.is_air(chunk.get(x, y, z, air)) {
                chunk.set(x, y, z, self.default_block, q, air);
                y -= 1;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn frozen_ocean_extension(&self, min_surface: i32, biome: &BiomeClimate, chunk: &mut SurfaceChunk, x: i32, z: i32, top: i32, q: &dyn BlockQuery) {
        let air = self.states.air;
        let v = f64::min(
            (self.iceberg_surface_noise.get_value(x as f64, 0.0, z as f64) * 8.25).abs(),
            self.iceberg_pillar_noise.get_value(x as f64 * 1.28, 0.0, z as f64 * 1.28) * 15.0,
        );
        if v <= 1.8 {
            return;
        }
        let roof = (self.iceberg_pillar_roof_noise.get_value(x as f64 * 1.17, 0.0, z as f64 * 1.17) * 1.5).abs();
        let mut ice_top = f64::min(v * v * 1.2, (roof * 40.0).ceil() + 14.0);
        if biome.should_melt_frozen_ocean_iceberg_slightly(x, 63, z) {
            ice_top -= 2.0;
        }
        let sea = self.sea_level as f64;
        let ice_bottom;
        if ice_top > 2.0 {
            ice_bottom = sea - ice_top - 7.0;
            ice_top += sea;
        } else {
            ice_top = 0.0;
            ice_bottom = 0.0;
        }
        let ice_top_copy = ice_top;
        let mut random = self.noise_random.at(x, 0, z);
        let max_snow = 2 + random.next_int_bound(4);
        let snow_min_y = self.sea_level + 18 + random.next_int_bound(10);
        let mut snow = 0;
        let mut y = i32::max(top, (ice_top as i32).wrapping_add(1));
        while y >= min_surface {
            let s = chunk.get(x, y, z, air);
            if q.is_air(s) && y < ice_top_copy as i32 && random.next_double() > 0.01
                || q.same_block(s, self.states.water) && y > ice_bottom as i32 && y < self.sea_level && ice_bottom != 0.0 && random.next_double() > 0.15
            {
                if snow <= max_snow && y > snow_min_y {
                    chunk.set(x, y, z, self.states.snow_block, q, air);
                    snow += 1;
                } else {
                    chunk.set(x, y, z, self.states.packed_ice, q, air);
                }
            }
            y -= 1;
        }
    }
}

/// `SurfaceSystem.generateBands`
fn generate_bands(random: &mut dyn RandomSource, s: &SurfaceStates) -> Vec<u16> {
    let mut bands = vec![s.terracotta; 192];
    let mut i = 0usize;
    while i < bands.len() {
        i += random.next_int_bound(5) as usize + 1;
        if i < bands.len() {
            bands[i] = s.orange_terracotta;
        }
        i += 1;
    }
    make_bands(random, &mut bands, 1, s.yellow_terracotta);
    make_bands(random, &mut bands, 2, s.brown_terracotta);
    make_bands(random, &mut bands, 1, s.red_terracotta);
    let count = random.next_int_between_inclusive(9, 15);
    let mut done = 0;
    let mut j = 0usize;
    while done < count && j < bands.len() {
        bands[j] = s.white_terracotta;
        if j >= 2 && random.next_boolean() {
            bands[j - 1] = s.light_gray_terracotta;
        }
        if j + 1 < bands.len() && random.next_boolean() {
            bands[j + 1] = s.light_gray_terracotta;
        }
        done += 1;
        j += random.next_int_bound(16) as usize + 4;
    }
    bands
}

fn make_bands(random: &mut dyn RandomSource, bands: &mut [u16], base: i32, state: u16) {
    let n = random.next_int_between_inclusive(6, 15);
    for _ in 0..n {
        let width = base + random.next_int_bound(3);
        let start = random.next_int_bound(bands.len() as i32) as usize;
        let mut k = 0;
        while start + k < bands.len() && (k as i32) < width {
            bands[start + k] = state;
            k += 1;
        }
    }
}

// ------------------------------------------------------------------------------------------------ rule evaluation

/// `SurfaceRules.Context`
struct Context<'a> {
    system: &'a SurfaceSystem,
    noise_chunk: &'a NoiseChunk,
    biome_at: &'a dyn Fn(i32, i32, i32) -> &'static str,
    climate: &'a dyn Fn(&str) -> Option<BiomeClimate>,
    epoch_xz: u64,
    epoch_y: u64,
    block_x: i32,
    block_z: i32,
    surface_depth: i32,
    surface_secondary: Option<f64>,
    min_surface_level: Option<i32>,
    last_prelim_cell: Option<(i32, i32)>,
    prelim: [i32; 4],
    block_y: i32,
    water_height: i32,
    stone_depth_below: i32,
    stone_depth_above: i32,
    biome: Option<&'static str>,
    cold: Option<bool>,
    cache: Vec<(u64, bool)>,
}

impl<'a> Context<'a> {
    fn new(
        system: &'a SurfaceSystem,
        noise_chunk: &'a NoiseChunk,
        biome_at: &'a dyn Fn(i32, i32, i32) -> &'static str,
        climate: &'a dyn Fn(&str) -> Option<BiomeClimate>,
        slots: usize,
    ) -> Self {
        Self {
            system,
            noise_chunk,
            biome_at,
            climate,
            epoch_xz: 1,
            epoch_y: 1,
            block_x: 0,
            block_z: 0,
            surface_depth: 0,
            surface_secondary: None,
            min_surface_level: None,
            last_prelim_cell: None,
            prelim: [0; 4],
            block_y: 0,
            water_height: 0,
            stone_depth_below: 0,
            stone_depth_above: 0,
            biome: None,
            cold: None,
            cache: vec![(0, false); slots],
        }
    }

    fn update_xz(&mut self, x: i32, z: i32) {
        self.epoch_xz += 1;
        self.epoch_y += 1;
        self.block_x = x;
        self.block_z = z;
        self.surface_depth = self.system.surface_depth(x, z);
        self.surface_secondary = None;
        self.min_surface_level = None;
    }

    fn update_y(&mut self, stone_above: i32, stone_below: i32, water_height: i32, _x: i32, y: i32, _z: i32) {
        self.epoch_y += 1;
        self.biome = None;
        self.cold = None;
        self.block_y = y;
        self.water_height = water_height;
        self.stone_depth_below = stone_below;
        self.stone_depth_above = stone_above;
    }

    fn surface_secondary(&mut self) -> f64 {
        if let Some(v) = self.surface_secondary {
            return v;
        }
        let v = self.system.surface_secondary(self.block_x, self.block_z);
        self.surface_secondary = Some(v);
        v
    }

    /// `Context.getMinSurfaceLevel`
    fn min_surface_level(&mut self) -> i32 {
        if let Some(v) = self.min_surface_level {
            return v;
        }
        let (cx, cz) = (self.block_x >> 4, self.block_z >> 4);
        if self.last_prelim_cell != Some((cx, cz)) {
            self.last_prelim_cell = Some((cx, cz));
            let nc = self.noise_chunk;
            self.prelim[0] = nc.preliminary_surface_level(cx << 4, cz << 4);
            self.prelim[1] = nc.preliminary_surface_level((cx + 1) << 4, cz << 4);
            self.prelim[2] = nc.preliminary_surface_level(cx << 4, (cz + 1) << 4);
            self.prelim[3] = nc.preliminary_surface_level((cx + 1) << 4, (cz + 1) << 4);
        }
        let level = mth::floor(mth::lerp2(
            ((self.block_x & 15) as f32 / 16.0f32) as f64,
            ((self.block_z & 15) as f32 / 16.0f32) as f64,
            self.prelim[0] as f64,
            self.prelim[1] as f64,
            self.prelim[2] as f64,
            self.prelim[3] as f64,
        ));
        let v = level + self.surface_depth - 8;
        self.min_surface_level = Some(v);
        v
    }

    fn biome(&mut self) -> &'static str {
        if let Some(b) = self.biome {
            return b;
        }
        let b = (self.biome_at)(self.block_x, self.block_y, self.block_z);
        self.biome = Some(b);
        b
    }

    fn try_apply(&mut self, rule: &Rule, chunk: &SurfaceChunk, x: i32, y: i32, z: i32) -> Option<u16> {
        match rule {
            Rule::Bandlands => Some(self.system.band(x, y, z)),
            Rule::State(s) => Some(*s),
            Rule::Sequence(rules) => {
                for r in rules {
                    if let Some(s) = self.try_apply(r, chunk, x, y, z) {
                        return Some(s);
                    }
                }
                None
            }
            Rule::Test(cond, then) => {
                if self.test(cond, chunk) {
                    self.try_apply(then, chunk, x, y, z)
                } else {
                    None
                }
            }
        }
    }

    fn test(&mut self, cond: &Cond, chunk: &SurfaceChunk) -> bool {
        match cond {
            Cond::Biome(names) => {
                let b = self.biome();
                names.iter().any(|n| n == b)
            }
            Cond::Noise { id, noise, min, max } => {
                let (epoch, v) = self.cache[*id];
                if epoch == self.epoch_xz {
                    return v;
                }
                let n = noise.get_value(self.block_x as f64, 0.0, self.block_z as f64);
                let v = n >= *min && n <= *max;
                self.cache[*id] = (self.epoch_xz, v);
                v
            }
            Cond::Gradient { factory, true_at_and_below, false_at_and_above } => {
                let y = self.block_y;
                if y <= *true_at_and_below {
                    true
                } else if y >= *false_at_and_above {
                    false
                } else {
                    let t = mth::lerp((y as f64 - *true_at_and_below as f64) / (*false_at_and_above as f64 - *true_at_and_below as f64), 1.0, 0.0);
                    (factory.at(self.block_x, y, self.block_z).next_float() as f64) < t
                }
            }
            Cond::YAbove { anchor, surface_depth_multiplier, add_stone_depth } => {
                self.block_y + if *add_stone_depth { self.stone_depth_above } else { 0 } >= anchor.wrapping_add(self.surface_depth.wrapping_mul(*surface_depth_multiplier))
            }
            Cond::Water { offset, surface_depth_multiplier, add_stone_depth } => {
                self.water_height == i32::MIN
                    || self.block_y + if *add_stone_depth { self.stone_depth_above } else { 0 }
                        >= self.water_height.wrapping_add(*offset).wrapping_add(self.surface_depth.wrapping_mul(*surface_depth_multiplier))
            }
            Cond::Temperature => {
                if let Some(c) = self.cold {
                    return c;
                }
                let biome = self.biome();
                let v = (self.climate)(biome).map(|c| c.cold_enough_to_snow(self.block_x, self.block_y, self.block_z)).unwrap_or(false);
                self.cold = Some(v);
                v
            }
            Cond::Steep { id } => {
                let (epoch, v) = self.cache[*id];
                if epoch == self.epoch_xz {
                    return v;
                }
                let (x, z) = (self.block_x & 15, self.block_z & 15);
                let (z0, z1) = (i32::max(z - 1, 0), i32::min(z + 1, 15));
                let v = if chunk.height_at(x, z1) >= chunk.height_at(x, z0) + 4 {
                    true
                } else {
                    let (x0, x1) = (i32::max(x - 1, 0), i32::min(x + 1, 15));
                    chunk.height_at(x0, z) >= chunk.height_at(x1, z) + 4
                };
                self.cache[*id] = (self.epoch_xz, v);
                v
            }
            Cond::Not(inner) => !self.test(inner, chunk),
            Cond::Hole => self.surface_depth <= 0,
            Cond::AbovePreliminarySurface => self.block_y >= self.min_surface_level(),
            Cond::StoneDepth { offset, add_surface_depth, secondary_depth_range, ceiling } => {
                let depth = if *ceiling { self.stone_depth_below } else { self.stone_depth_above };
                let surface = if *add_surface_depth { self.surface_depth } else { 0 };
                let secondary = if *secondary_depth_range == 0 {
                    0
                } else {
                    let s = self.surface_secondary();
                    (mth::lerp((s - -1.0) / (1.0 - -1.0), 0.0, *secondary_depth_range as f64)) as i32
                };
                depth <= 1 + *offset + surface + secondary
            }
        }
    }
}
