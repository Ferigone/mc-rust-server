//! Carvers (`net.minecraft.world.level.levelgen.carver`): caves, canyons and the nether caves, applied after the surface stage.
//!
//! Every chunk is carved by the carvers that start in the 17x17 chunks around it (`ChunkGenerator.applyCarvers`); each start chunk
//! reseeds a legacy random source with `setLargeFeatureSeed`, so the result is independent of the order chunks are generated in.

use crate::biome_climate::BiomeClimate;
use crate::biome_source::BiomeSource;
use crate::climate::Sampler;
use crate::mth;
use crate::noise_chunk::NoiseChunk;
use crate::random::{LegacyRandomSource, RandomSource};
use crate::surface::{Anchor, BlockQuery, GenContext, SurfaceChunk, SurfaceRule, SurfaceSystem};
use anyhow::{anyhow, bail, Context, Result};
use mc_data::Blocks;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

// ------------------------------------------------------------------------------------------------ Mth trigonometry

/// `Mth.SIN`: the sine table of the 65536 angles
fn sin_table() -> &'static [f32; 65536] {
    static TABLE: OnceLock<Box<[f32; 65536]>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut t = Box::new([0f32; 65536]);
        for (i, v) in t.iter_mut().enumerate() {
            *v = (i as f64 * std::f64::consts::PI * 2.0 / 65536.0).sin() as f32;
        }
        t
    })
}

/// `Mth.sin(float)`
pub fn sin(x: f32) -> f32 {
    sin_table()[((x * 10430.378f32) as i32 & 65535) as usize]
}

/// `Mth.cos(float)`
pub fn cos(x: f32) -> f32 {
    sin_table()[((x * 10430.378f32 + 16384.0f32) as i32 & 65535) as usize]
}

const PI_F: f32 = std::f32::consts::PI;

// ------------------------------------------------------------------------------------------------ value providers

/// `FloatProvider` (the kinds the vanilla carvers use)
#[derive(Clone, Debug)]
pub enum FloatProvider {
    Constant(f32),
    Uniform { min: f32, max: f32 },
    Trapezoid { min: f32, max: f32, plateau: f32 },
}

fn float(v: &Value) -> Result<f32> {
    match v {
        Value::Number(n) => n.to_string().parse::<f32>().map_err(|e| anyhow!("bad float {n}: {e}")),
        _ => bail!("expected a number, got {v}"),
    }
}

fn float_field(v: &Value, k: &str) -> Result<f32> {
    float(v.get(k).ok_or_else(|| anyhow!("missing {k}"))?)
}

impl FloatProvider {
    pub fn parse(v: &Value) -> Result<Self> {
        if v.is_number() {
            return Ok(Self::Constant(float(v)?));
        }
        match v.get("type").and_then(Value::as_str) {
            Some("minecraft:constant") => Ok(Self::Constant(float_field(v, "value")?)),
            Some("minecraft:uniform") => Ok(Self::Uniform { min: float_field(v, "min_inclusive")?, max: float_field(v, "max_exclusive")? }),
            Some("minecraft:trapezoid") => Ok(Self::Trapezoid { min: float_field(v, "min")?, max: float_field(v, "max")?, plateau: float_field(v, "plateau")? }),
            other => bail!("unsupported float provider {other:?}"),
        }
    }

    pub fn sample(&self, r: &mut dyn RandomSource) -> f32 {
        match *self {
            Self::Constant(v) => v,
            // Mth.randomBetween
            Self::Uniform { min, max } => r.next_float() * (max - min) + min,
            Self::Trapezoid { min, max, plateau } => {
                let span = max - min;
                let a = (span - plateau) / 2.0;
                let b = span - a;
                let first = r.next_float() * b;
                min + first + r.next_float() * a
            }
        }
    }
}

/// `HeightProvider`
#[derive(Clone, Debug)]
pub enum HeightProvider {
    Constant(Anchor),
    Uniform { min: Anchor, max: Anchor },
    Trapezoid { min: Anchor, max: Anchor, plateau: i32 },
}

impl HeightProvider {
    pub fn parse(v: &Value) -> Result<Self> {
        match v.get("type").and_then(Value::as_str) {
            None => Ok(Self::Constant(Anchor::parse(v)?)),
            Some("minecraft:constant") => Ok(Self::Constant(Anchor::parse(&v["value"])?)),
            Some("minecraft:uniform") => Ok(Self::Uniform { min: Anchor::parse(&v["min_inclusive"])?, max: Anchor::parse(&v["max_inclusive"])? }),
            Some("minecraft:trapezoid") => Ok(Self::Trapezoid {
                min: Anchor::parse(&v["min_inclusive"])?,
                max: Anchor::parse(&v["max_inclusive"])?,
                plateau: v.get("plateau").and_then(Value::as_i64).unwrap_or(0) as i32,
            }),
            other => bail!("unsupported height provider {other:?}"),
        }
    }

    pub fn sample(&self, r: &mut dyn RandomSource, g: GenContext) -> i32 {
        match *self {
            Self::Constant(a) => a.resolve(g),
            Self::Trapezoid { min, max, plateau } => {
                let (lo, hi) = (min.resolve(g), max.resolve(g));
                if lo > hi {
                    return lo;
                }
                let span = hi - lo;
                if plateau >= span {
                    r.next_int_bound(hi - lo + 1) + lo
                } else {
                    let a = (span - plateau) / 2;
                    let b = span - a;
                    let first = r.next_int_bound(b + 1);
                    lo + first + r.next_int_bound(a + 1)
                }
            }
            Self::Uniform { min, max } => {
                let (lo, hi) = (min.resolve(g), max.resolve(g));
                if lo > hi { lo } else { r.next_int_bound(hi - lo + 1) + lo }
            }
        }
    }
}

// ------------------------------------------------------------------------------------------------ configurations

/// `CarverConfiguration`
#[derive(Clone, Debug)]
pub struct CarverCommon {
    pub probability: f32,
    y: HeightProvider,
    y_scale: FloatProvider,
    lava_level: Anchor,
    /// `replaceable`, by block state ID
    replaceable: std::sync::Arc<Vec<bool>>,
}

/// `CaveCarverConfiguration`
#[derive(Clone, Debug)]
pub struct CaveConfig {
    common: CarverCommon,
    horizontal_radius_multiplier: FloatProvider,
    vertical_radius_multiplier: FloatProvider,
    floor_level: FloatProvider,
}

/// `CanyonCarverConfiguration`
#[derive(Clone, Debug)]
pub struct CanyonConfig {
    common: CarverCommon,
    vertical_rotation: FloatProvider,
    distance_factor: FloatProvider,
    thickness: FloatProvider,
    width_smoothness: i32,
    horizontal_radius_factor: FloatProvider,
    vertical_radius_default_factor: f32,
    vertical_radius_center_factor: f32,
}

/// `ConfiguredWorldCarver`
#[derive(Clone, Debug)]
pub enum Carver {
    Cave(CaveConfig),
    NetherCave(CaveConfig),
    Canyon(CanyonConfig),
}

use crate::tags::replaceable_states;

fn parse_common(root: &Path, blocks: &Blocks, c: &Value) -> Result<CarverCommon> {
    Ok(CarverCommon {
        probability: float_field(c, "probability")?,
        y: HeightProvider::parse(&c["y"])?,
        y_scale: FloatProvider::parse(&c["yScale"])?,
        lava_level: Anchor::parse(&c["lava_level"])?,
        replaceable: std::sync::Arc::new(replaceable_states(root, blocks, &c["replaceable"])?),
    })
}

fn parse_cave(root: &Path, blocks: &Blocks, c: &Value) -> Result<CaveConfig> {
    Ok(CaveConfig {
        common: parse_common(root, blocks, c)?,
        horizontal_radius_multiplier: FloatProvider::parse(&c["horizontal_radius_multiplier"])?,
        vertical_radius_multiplier: FloatProvider::parse(&c["vertical_radius_multiplier"])?,
        floor_level: FloatProvider::parse(&c["floor_level"])?,
    })
}

impl Carver {
    pub fn parse(root: &Path, blocks: &Blocks, v: &Value) -> Result<Self> {
        let c = v.get("config").ok_or_else(|| anyhow!("carver without config"))?;
        match v.get("type").and_then(Value::as_str) {
            Some("minecraft:cave") => Ok(Self::Cave(parse_cave(root, blocks, c)?)),
            Some("minecraft:nether_cave") => Ok(Self::NetherCave(parse_cave(root, blocks, c)?)),
            Some("minecraft:canyon") => {
                let s = &c["shape"];
                Ok(Self::Canyon(CanyonConfig {
                    common: parse_common(root, blocks, c)?,
                    vertical_rotation: FloatProvider::parse(&c["vertical_rotation"])?,
                    distance_factor: FloatProvider::parse(&s["distance_factor"])?,
                    thickness: FloatProvider::parse(&s["thickness"])?,
                    width_smoothness: s["width_smoothness"].as_i64().ok_or_else(|| anyhow!("width_smoothness"))? as i32,
                    horizontal_radius_factor: FloatProvider::parse(&s["horizontal_radius_factor"])?,
                    vertical_radius_default_factor: float_field(s, "vertical_radius_default_factor")?,
                    vertical_radius_center_factor: float_field(s, "vertical_radius_center_factor")?,
                }))
            }
            other => bail!("unsupported carver {other:?}"),
        }
    }

    fn common(&self) -> &CarverCommon {
        match self {
            Self::Cave(c) | Self::NetherCave(c) => &c.common,
            Self::Canyon(c) => &c.common,
        }
    }

    /// `isStartChunk`: `random.nextFloat() <= probability`
    pub fn is_start_chunk(&self, r: &mut dyn RandomSource) -> bool {
        r.next_float() <= self.common().probability
    }
}

/// The configured carvers of the data pack and the carver list of every biome (`carvers.air`).
pub struct CarverSet {
    carvers: HashMap<String, Carver>,
    by_biome: HashMap<String, Vec<String>>,
}

impl CarverSet {
    /// `root` = the `generated` directory of the data generator.
    pub fn load(root: &Path, blocks: &Blocks) -> Result<Self> {
        let worldgen = root.join("data/minecraft/worldgen");
        let mut carvers = HashMap::new();
        for e in std::fs::read_dir(worldgen.join("configured_carver"))? {
            let path = e?.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let name = format!("minecraft:{}", path.file_stem().unwrap().to_string_lossy());
            let v: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
            carvers.insert(name.clone(), Carver::parse(root, blocks, &v).with_context(|| name.clone())?);
        }
        let mut by_biome = HashMap::new();
        for e in std::fs::read_dir(worldgen.join("biome"))? {
            let path = e?.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let name = format!("minecraft:{}", path.file_stem().unwrap().to_string_lossy());
            let v: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
            let list: Vec<String> = match v.get("carvers").and_then(|c| c.get("air")) {
                Some(Value::String(s)) => vec![s.clone()],
                Some(Value::Array(a)) => a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect(),
                _ => Vec::new(),
            };
            by_biome.insert(name, list);
        }
        Ok(Self { carvers, by_biome })
    }

    /// The carvers of a biome in their list order (the order is part of the seeding).
    pub fn for_biome(&self, biome: &str) -> Vec<&Carver> {
        self.by_biome.get(biome).map(|l| l.iter().filter_map(|n| self.carvers.get(n)).collect()).unwrap_or_default()
    }
}

// ------------------------------------------------------------------------------------------------ carving

/// `CarvingMask`: the blocks already carved in this chunk
pub struct CarvingMask {
    min_y: i32,
    bits: Vec<bool>,
}

impl CarvingMask {
    pub fn new(height: i32, min_y: i32) -> Self {
        Self { min_y, bits: vec![false; 256 * height as usize] }
    }

    fn index(&self, x: i32, y: i32, z: i32) -> usize {
        ((x & 15) | ((z & 15) << 4) | ((y - self.min_y) << 8)) as usize
    }

    fn get(&self, x: i32, y: i32, z: i32) -> bool {
        self.bits[self.index(x, y, z)]
    }

    fn set(&mut self, x: i32, y: i32, z: i32) {
        let i = self.index(x, y, z);
        self.bits[i] = true;
    }

    /// Number of carved blocks.
    pub fn count(&self) -> usize {
        self.bits.iter().filter(|&&b| b).count()
    }
}

/// The block states the carvers place or look for.
#[derive(Clone, Copy, Debug)]
pub struct CarveStates {
    pub grass_block: u16,
    pub mycelium: u16,
    pub dirt: u16,
    pub lava: u16,
    pub cave_air: u16,
    pub air: u16,
}

impl CarveStates {
    pub fn resolve(blocks: &Blocks) -> Result<Self> {
        let st = |n: &str| blocks.default_state(n).map(|s| s as u16).ok_or_else(|| anyhow!("unknown block {n}"));
        Ok(Self {
            grass_block: st("minecraft:grass_block")?,
            mycelium: st("minecraft:mycelium")?,
            dirt: st("minecraft:dirt")?,
            lava: st("minecraft:lava")?,
            cave_air: st("minecraft:cave_air")?,
            air: st("minecraft:air")?,
        })
    }
}

/// Everything a carver needs besides the chunk and the mask (`CarvingContext` and the arguments of `carve`).
pub struct CarveEnv<'a> {
    pub range: GenContext,
    pub noise_chunk: &'a NoiseChunk,
    pub system: &'a SurfaceSystem,
    pub rule: &'a SurfaceRule,
    pub biome_at: &'a dyn Fn(i32, i32, i32) -> &'static str,
    pub climate: &'a dyn Fn(&str) -> Option<BiomeClimate>,
    pub query: &'a dyn BlockQuery,
    pub states: CarveStates,
}

type Skip<'a> = &'a dyn Fn(f64, f64, f64, i32) -> bool;

/// `WorldCarver.getRange()`
const RANGE: i32 = 4;

impl Carver {
    fn carve(&self, env: &CarveEnv, chunk: &mut SurfaceChunk, r: &mut dyn RandomSource, start: (i32, i32), mask: &mut CarvingMask) {
        match self {
            Carver::Cave(c) => carve_caves(env, c, false, chunk, r, start, mask),
            Carver::NetherCave(c) => carve_caves(env, c, true, chunk, r, start, mask),
            Carver::Canyon(c) => carve_canyon(env, c, chunk, r, start, mask),
        }
    }
}

/// `WorldCarver.canReach`
fn can_reach(chunk: &SurfaceChunk, x: f64, z: f64, step: i32, steps: i32, thickness: f32) -> bool {
    let mid_x = (chunk.chunk_x * 16 + 8) as f64;
    let mid_z = (chunk.chunk_z * 16 + 8) as f64;
    let dx = x - mid_x;
    let dz = z - mid_z;
    let remaining = (steps - step) as f64;
    let reach = (thickness + 2.0f32 + 16.0f32) as f64;
    dx * dx + dz * dz - remaining * remaining <= reach * reach
}

/// `WorldCarver.carveEllipsoid`
#[allow(clippy::too_many_arguments)]
fn carve_ellipsoid(
    env: &CarveEnv,
    common: &CarverCommon,
    nether: bool,
    chunk: &mut SurfaceChunk,
    (x, y, z): (f64, f64, f64),
    h_radius: f64,
    v_radius: f64,
    mask: &mut CarvingMask,
    skip: Skip,
) -> bool {
    let mid_x = (chunk.chunk_x * 16 + 8) as f64;
    let mid_z = (chunk.chunk_z * 16 + 8) as f64;
    let limit = 16.0 + h_radius * 2.0;
    if (x - mid_x).abs() > limit || (z - mid_z).abs() > limit {
        return false;
    }
    let (min_x, min_z) = (chunk.chunk_x * 16, chunk.chunk_z * 16);
    let x0 = i32::max(mth::floor(x - h_radius) - min_x - 1, 0);
    let x1 = i32::min(mth::floor(x + h_radius) - min_x, 15);
    let y0 = i32::max(mth::floor(y - v_radius) - 1, env.range.min_y + 1);
    let y1 = i32::min(mth::floor(y + v_radius) + 1, env.range.min_y + env.range.height - 1 - 7);
    let z0 = i32::max(mth::floor(z - h_radius) - min_z - 1, 0);
    let z1 = i32::min(mth::floor(z + h_radius) - min_z, 15);
    let mut carved = false;
    for lx in x0..=x1 {
        let bx = min_x + lx;
        let dx = ((bx as f64) + 0.5 - x) / h_radius;
        for lz in z0..=z1 {
            let bz = min_z + lz;
            let dz = ((bz as f64) + 0.5 - z) / h_radius;
            if dx * dx + dz * dz >= 1.0 {
                continue;
            }
            let mut hit_grass = false;
            let mut by = y1;
            while by > y0 {
                let dy = ((by as f64) - 0.5 - y) / v_radius;
                if !skip(dx, dy, dz, by) && !mask.get(lx, by, lz) {
                    mask.set(lx, by, lz);
                    carved |= if nether {
                        carve_block_nether(env, common, chunk, (bx, by, bz))
                    } else {
                        carve_block(env, common, chunk, (bx, by, bz), &mut hit_grass)
                    };
                }
                by -= 1;
            }
        }
    }
    carved
}

/// `WorldCarver.carveBlock`
fn carve_block(env: &CarveEnv, common: &CarverCommon, chunk: &mut SurfaceChunk, (x, y, z): (i32, i32, i32), hit_grass: &mut bool) -> bool {
    let (q, st) = (env.query, env.states);
    let state = chunk.get(x, y, z, st.air);
    if q.same_block(state, st.grass_block) || q.same_block(state, st.mycelium) {
        *hit_grass = true;
    }
    if !common.replaceable[state as usize] {
        return false;
    }
    // getCarveState
    let carve = if y <= common.lava_level.resolve(env.range) { Some(st.lava) } else { env.noise_chunk.aquifer_substance_at(x, y, z, 0.0) };
    let Some(carve) = carve else { return false };
    chunk.set_block(x, y, z, carve, q, st.air, false);
    if env.noise_chunk.should_schedule_fluid_update() && q.has_fluid(carve) {
        chunk.post_process.push((x, y, z));
    }
    if *hit_grass {
        let below = chunk.get(x, y - 1, z, st.air);
        if q.same_block(below, st.dirt) {
            if let Some(top) = env.system.top_material(env.rule, env.noise_chunk, chunk, env.biome_at, env.climate, (x, y - 1, z), q.has_fluid(carve)) {
                chunk.set_block(x, y - 1, z, top, q, st.air, true);
            }
        }
    }
    true
}

/// `NetherWorldCarver.carveBlock`
fn carve_block_nether(env: &CarveEnv, common: &CarverCommon, chunk: &mut SurfaceChunk, (x, y, z): (i32, i32, i32)) -> bool {
    let st = env.states;
    let state = chunk.get(x, y, z, st.air);
    if !common.replaceable[state as usize] {
        return false;
    }
    let new = if y <= env.range.min_y + 31 { st.lava } else { st.cave_air };
    chunk.set_block(x, y, z, new, env.query, st.air, false);
    true
}

// ---- caves

/// `CaveWorldCarver.shouldSkip`
fn cave_should_skip(dx: f64, dy: f64, dz: f64, floor: f64) -> bool {
    if dy <= floor { true } else { dx * dx + dy * dy + dz * dz >= 1.0 }
}

/// `CaveWorldCarver.getThickness` / `NetherWorldCarver.getThickness`
fn cave_thickness(r: &mut dyn RandomSource, nether: bool) -> f32 {
    if nether {
        let a = r.next_float() * 2.0f32;
        return (a + r.next_float()) * 2.0f32;
    }
    let a = r.next_float() * 2.0f32;
    let mut t = a + r.next_float();
    if r.next_int_bound(10) == 0 {
        let f = r.next_float() * r.next_float() * 3.0f32 + 1.0f32;
        t *= f;
    }
    t
}

/// `CaveWorldCarver.carve` (the nether variant differs in the cave bound, the thickness, the y scale and `carveBlock`)
fn carve_caves(env: &CarveEnv, cfg: &CaveConfig, nether: bool, chunk: &mut SurfaceChunk, r: &mut dyn RandomSource, start: (i32, i32), mask: &mut CarvingMask) {
    let common = &cfg.common;
    let reach = (RANGE * 2 - 1) * 16;
    let bound = if nether { 10 } else { 15 };
    // nextInt(nextInt(nextInt(bound) + 1) + 1)
    let a = r.next_int_bound(bound);
    let b = r.next_int_bound(a + 1);
    let count = r.next_int_bound(b + 1);
    for _ in 0..count {
        let x = (start.0 * 16 + r.next_int_bound(16)) as f64;
        let y = common.y.sample(r, env.range) as f64;
        let z = (start.1 * 16 + r.next_int_bound(16)) as f64;
        let h_mul = cfg.horizontal_radius_multiplier.sample(r) as f64;
        let v_mul = cfg.vertical_radius_multiplier.sample(r) as f64;
        let floor = cfg.floor_level.sample(r) as f64;
        let skip = move |dx: f64, dy: f64, dz: f64, _y: i32| cave_should_skip(dx, dy, dz, floor);
        let mut tunnels = 1;
        if r.next_int_bound(4) == 0 {
            let y_scale = common.y_scale.sample(r) as f64;
            let thickness = 1.0f32 + r.next_float() * 6.0f32;
            // createRoom
            let h = 1.5 + (sin((std::f64::consts::PI / 2.0) as f32) * thickness) as f64;
            carve_ellipsoid(env, common, nether, chunk, (x + 1.0, y, z), h, h * y_scale, mask, &skip);
            tunnels += r.next_int_bound(4);
        }
        for _ in 0..tunnels {
            let yaw = r.next_float() * (std::f64::consts::PI * 2.0) as f32;
            let pitch = (r.next_float() - 0.5f32) / 4.0f32;
            let thickness = cave_thickness(r, nether);
            let steps = reach - r.next_int_bound(reach / 4);
            let seed = r.next_long();
            let tunnel = Tunnel { nether, h_mul, v_mul, y_scale: if nether { 5.0 } else { 1.0 } };
            create_tunnel(env, common, &tunnel, chunk, seed, (x, y, z), thickness, (yaw, pitch), (0, steps), mask, &skip);
        }
    }
}

struct Tunnel {
    nether: bool,
    h_mul: f64,
    v_mul: f64,
    y_scale: f64,
}

/// `CaveWorldCarver.createTunnel`
#[allow(clippy::too_many_arguments)]
fn create_tunnel(
    env: &CarveEnv,
    common: &CarverCommon,
    t: &Tunnel,
    chunk: &mut SurfaceChunk,
    seed: i64,
    (mut x, mut y, mut z): (f64, f64, f64),
    thickness: f32,
    (mut yaw, mut pitch): (f32, f32),
    (first_step, steps): (i32, i32),
    mask: &mut CarvingMask,
    skip: Skip,
) {
    let mut rnd = LegacyRandomSource::new(seed);
    let split_step = rnd.next_int_bound(steps / 2) + steps / 4;
    let steep = rnd.next_int_bound(6) == 0;
    let mut yaw_acc = 0.0f32;
    let mut pitch_acc = 0.0f32;
    let half_pi = (std::f64::consts::PI / 2.0) as f32;
    let mut step = first_step;
    while step < steps {
        let h = 1.5 + (sin(PI_F * step as f32 / steps as f32) * thickness) as f64;
        let v = h * t.y_scale;
        let cos_pitch = cos(pitch);
        x += (cos(yaw) * cos_pitch) as f64;
        y += sin(pitch) as f64;
        z += (sin(yaw) * cos_pitch) as f64;
        pitch *= if steep { 0.92f32 } else { 0.7f32 };
        pitch += pitch_acc * 0.1f32;
        yaw += yaw_acc * 0.1f32;
        pitch_acc *= 0.9f32;
        yaw_acc *= 0.75f32;
        pitch_acc += (rnd.next_float() - rnd.next_float()) * rnd.next_float() * 2.0f32;
        yaw_acc += (rnd.next_float() - rnd.next_float()) * rnd.next_float() * 4.0f32;
        if step == split_step && thickness > 1.0f32 {
            for sign in [-1.0f32, 1.0f32] {
                let child_seed = rnd.next_long();
                let child_thickness = rnd.next_float() * 0.5f32 + 0.5f32;
                let child_yaw = if sign < 0.0 { yaw - half_pi } else { yaw + half_pi };
                let child = Tunnel { y_scale: 1.0, ..*t };
                create_tunnel(env, common, &child, chunk, child_seed, (x, y, z), child_thickness, (child_yaw, pitch / 3.0f32), (step, steps), mask, skip);
            }
            return;
        }
        if rnd.next_int_bound(4) != 0 {
            if !can_reach(chunk, x, z, step, steps, thickness) {
                return;
            }
            carve_ellipsoid(env, common, t.nether, chunk, (x, y, z), h * t.h_mul, v * t.v_mul, mask, skip);
        }
        step += 1;
    }
}

// ---- canyons

/// `CanyonWorldCarver.carve`
fn carve_canyon(env: &CarveEnv, cfg: &CanyonConfig, chunk: &mut SurfaceChunk, r: &mut dyn RandomSource, start: (i32, i32), mask: &mut CarvingMask) {
    let reach = (RANGE * 2 - 1) * 16;
    let x = (start.0 * 16 + r.next_int_bound(16)) as f64;
    let y = cfg.common.y.sample(r, env.range);
    let z = (start.1 * 16 + r.next_int_bound(16)) as f64;
    let yaw = r.next_float() * (std::f64::consts::PI * 2.0) as f32;
    let pitch = cfg.vertical_rotation.sample(r);
    let y_scale = cfg.common.y_scale.sample(r) as f64;
    let thickness = cfg.thickness.sample(r);
    let steps = (reach as f32 * cfg.distance_factor.sample(r)) as i32;
    let seed = r.next_long();
    do_carve_canyon(env, cfg, chunk, seed, (x, y as f64, z), thickness, (yaw, pitch), steps, y_scale, mask);
}

/// `CanyonWorldCarver.doCarve`
#[allow(clippy::too_many_arguments)]
fn do_carve_canyon(
    env: &CarveEnv,
    cfg: &CanyonConfig,
    chunk: &mut SurfaceChunk,
    seed: i64,
    (mut x, mut y, mut z): (f64, f64, f64),
    thickness: f32,
    (mut yaw, mut pitch): (f32, f32),
    steps: i32,
    y_scale: f64,
    mask: &mut CarvingMask,
) {
    let mut rnd = LegacyRandomSource::new(seed);
    // initWidthFactors
    let mut factors = vec![0f32; env.range.height as usize];
    let mut current = 1.0f32;
    for (i, f) in factors.iter_mut().enumerate() {
        if i == 0 || rnd.next_int_bound(cfg.width_smoothness) == 0 {
            let a = rnd.next_float();
            current = 1.0f32 + a * rnd.next_float();
        }
        *f = current * current;
    }
    let min_y = env.range.min_y;
    let skip = move |dx: f64, dy: f64, dz: f64, by: i32| -> bool {
        let idx = (by - min_y) as usize;
        (dx * dx + dz * dz) * factors[idx - 1] as f64 + dy * dy / 6.0 >= 1.0
    };
    let mut yaw_acc = 0.0f32;
    let mut pitch_acc = 0.0f32;
    for step in 0..steps {
        let mut h = 1.5 + (sin(step as f32 * PI_F / steps as f32) * thickness) as f64;
        let mut v = h * y_scale;
        h *= cfg.horizontal_radius_factor.sample(&mut rnd) as f64;
        // updateVerticalRadius
        let t = 1.0f32 - (0.5f32 - step as f32 / steps as f32).abs() * 2.0f32;
        let factor = cfg.vertical_radius_default_factor + cfg.vertical_radius_center_factor * t;
        let jitter = rnd.next_float() * (1.0f32 - 0.75f32) + 0.75f32;
        v = factor as f64 * v * jitter as f64;
        let cos_pitch = cos(pitch);
        let sin_pitch = sin(pitch);
        x += (cos(yaw) * cos_pitch) as f64;
        y += sin_pitch as f64;
        z += (sin(yaw) * cos_pitch) as f64;
        pitch *= 0.7f32;
        pitch += pitch_acc * 0.05f32;
        yaw += yaw_acc * 0.05f32;
        pitch_acc *= 0.8f32;
        yaw_acc *= 0.5f32;
        pitch_acc += (rnd.next_float() - rnd.next_float()) * rnd.next_float() * 2.0f32;
        yaw_acc += (rnd.next_float() - rnd.next_float()) * rnd.next_float() * 4.0f32;
        if rnd.next_int_bound(4) != 0 {
            if !can_reach(chunk, x, z, step, steps, thickness) {
                return;
            }
            carve_ellipsoid(env, &cfg.common, false, chunk, (x, y, z), h, v, mask, &skip);
        }
    }
}

// ---- the 17x17 chunk loop

/// `WorldgenRandom.setLargeFeatureSeed`
fn set_large_feature_seed(r: &mut LegacyRandomSource, seed: i64, x: i32, z: i32) {
    r.set_seed(seed);
    let a = r.next_long();
    let b = r.next_long();
    r.set_seed((x as i64).wrapping_mul(a) ^ (z as i64).wrapping_mul(b) ^ seed);
}

/// `ChunkGenerator.applyCarvers` (the `AIR` step): every carver that starts in the 17x17 chunks around `chunk` and reaches it.
/// `source` / `sampler` are the biome source of the dimension (the carver lists are looked up at y = 0 of each chunk's corner).
pub fn apply_carvers(env: &CarveEnv, chunk: &mut SurfaceChunk, mask: &mut CarvingMask, world_seed: i64, source: &BiomeSource, sampler: &Sampler, set: &CarverSet) {
    let mut random = LegacyRandomSource::new(0);
    for dx in -8..=8 {
        for dz in -8..=8 {
            let (cx, cz) = (chunk.chunk_x + dx, chunk.chunk_z + dz);
            let biome = source.get_noise_biome(cx * 4, 0, cz * 4, sampler);
            for (i, carver) in set.for_biome(biome).into_iter().enumerate() {
                set_large_feature_seed(&mut random, world_seed.wrapping_add(i as i64), cx, cz);
                if carver.is_start_chunk(&mut random) {
                    carver.carve(env, chunk, &mut random, (cx, cz), mask);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_table_lookups() {
        assert_eq!(sin(0.0), 0.0);
        assert_eq!(cos(0.0), 1.0);
        assert_eq!(sin(std::f32::consts::FRAC_PI_2), 1.0);
        // the table has 65536 entries over a full turn: 10430.378 steps per radian
        assert!((sin(1.0) - 1.0f32.sin()).abs() < 1e-3);
        assert!((cos(2.0) - 2.0f32.cos()).abs() < 1e-3);
        assert_eq!(sin(-0.5), -sin(0.5));
    }

    #[test]
    fn trapezoid_and_uniform_providers() {
        let mut r = LegacyRandomSource::new(7);
        let t = FloatProvider::Trapezoid { min: 0.0, max: 6.0, plateau: 2.0 };
        for _ in 0..100 {
            let v = t.sample(&mut r);
            assert!((0.0..=6.0).contains(&v));
        }
        let u = FloatProvider::Uniform { min: 0.7, max: 1.4 };
        for _ in 0..100 {
            let v = u.sample(&mut r);
            assert!((0.7..1.4).contains(&v));
        }
    }

    #[test]
    fn large_feature_seed_is_deterministic() {
        let (mut a, mut b) = (LegacyRandomSource::new(1), LegacyRandomSource::new(99));
        set_large_feature_seed(&mut a, 12345, 3, -4);
        set_large_feature_seed(&mut b, 12345, 3, -4);
        assert_eq!(a.next_long(), b.next_long());
        set_large_feature_seed(&mut b, 12345, 4, -4);
        assert_ne!(a.next_long(), b.next_long());
    }
}
