//! Configured features (`net.minecraft.world.level.levelgen.feature`) and the rule tests / providers they use.

use crate::mth;
use crate::random::RandomSource;
use crate::region::{HeightmapKind, Region};
use crate::surface::BlockQuery;
use crate::tags::BlockTags;
use anyhow::{anyhow, bail, Result};
use mc_data::Blocks;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

const PI_F: f32 = std::f32::consts::PI;

/// `IntProvider`
#[derive(Clone, Debug)]
pub enum IntProvider {
    Constant(i32),
    Uniform { min: i32, max: i32 },
    BiasedToBottom { min: i32, max: i32 },
    Clamped { source: Box<IntProvider>, min: i32, max: i32 },
}

fn int_field(v: &Value, k: &str) -> Result<i32> {
    v.get(k).and_then(Value::as_i64).map(|i| i as i32).ok_or_else(|| anyhow!("missing integer {k} in {v}"))
}

impl IntProvider {
    pub fn parse(v: &Value) -> Result<Self> {
        if let Some(i) = v.as_i64() {
            return Ok(Self::Constant(i as i32));
        }
        match v.get("type").and_then(Value::as_str) {
            Some("minecraft:constant") => Ok(Self::Constant(int_field(v, "value")?)),
            Some("minecraft:uniform") => Ok(Self::Uniform { min: int_field(v, "min_inclusive")?, max: int_field(v, "max_inclusive")? }),
            Some("minecraft:biased_to_bottom") => Ok(Self::BiasedToBottom { min: int_field(v, "min_inclusive")?, max: int_field(v, "max_inclusive")? }),
            Some("minecraft:clamped") => Ok(Self::Clamped {
                source: Box::new(Self::parse(&v["source"])?),
                min: int_field(v, "min_inclusive")?,
                max: int_field(v, "max_inclusive")?,
            }),
            other => bail!("unsupported int provider {other:?}"),
        }
    }

    pub fn sample(&self, r: &mut dyn RandomSource) -> i32 {
        match self {
            Self::Constant(v) => *v,
            // Mth.randomBetweenInclusive
            Self::Uniform { min, max } => r.next_int_bound(max - min + 1) + min,
            Self::BiasedToBottom { min, max } => {
                let inner = r.next_int_bound(max - min + 1) + 1;
                min + r.next_int_bound(inner)
            }
            Self::Clamped { source, min, max } => source.sample(r).clamp(*min, *max),
        }
    }
}

// ------------------------------------------------------------------------------------------------ rule tests

/// `RuleTest`
#[derive(Clone)]
pub enum RuleTest {
    AlwaysTrue,
    /// `block_match` / `tag_match`: a flag per block state
    States(Arc<Vec<bool>>),
    BlockStateMatch(u16),
    /// `random_block_match`: block states and the probability (a random float is drawn only for matching blocks)
    RandomStates(Arc<Vec<bool>>, f32),
    RandomBlockState(u16, f32),
}

pub fn block_state(blocks: &Blocks, v: &Value) -> Result<u16> {
    let name = v.get("Name").and_then(Value::as_str).ok_or_else(|| anyhow!("block state without a name: {v}"))?;
    let props: Vec<(String, String)> = v
        .get("Properties")
        .and_then(Value::as_object)
        .map(|o| o.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_owned())).collect())
        .unwrap_or_default();
    blocks.state_id(name, props.iter().map(|(k, v)| (k.as_str(), v.as_str()))).map(|s| s as u16).ok_or_else(|| anyhow!("unknown block state {name}"))
}

fn block_states(blocks: &Blocks, name: &str) -> Result<Vec<bool>> {
    let info = blocks.by_name.get(name).map(|&i| &blocks.list[i]).ok_or_else(|| anyhow!("unknown block {name}"))?;
    let mut out = vec![false; blocks.state_count as usize];
    for s in info.min_state..=info.max_state {
        out[s as usize] = true;
    }
    Ok(out)
}

fn probability(v: &Value) -> Result<f32> {
    match v.get("probability") {
        Some(Value::Number(n)) => n.to_string().parse::<f32>().map_err(|e| anyhow!("bad probability: {e}")),
        _ => bail!("missing probability"),
    }
}

impl RuleTest {
    pub fn parse(env: &Env, v: &Value) -> Result<Self> {
        match v.get("predicate_type").and_then(Value::as_str) {
            Some("minecraft:always_true") => Ok(Self::AlwaysTrue),
            Some("minecraft:block_match") => Ok(Self::States(Arc::new(block_states(env.blocks, v["block"].as_str().ok_or_else(|| anyhow!("block_match without block"))?)?))),
            Some("minecraft:tag_match") => Ok(Self::States(env.tags.states(env.blocks, v["tag"].as_str().ok_or_else(|| anyhow!("tag_match without tag"))?)?)),
            Some("minecraft:blockstate_match") => Ok(Self::BlockStateMatch(block_state(env.blocks, &v["block_state"])?)),
            Some("minecraft:random_block_match") => {
                Ok(Self::RandomStates(Arc::new(block_states(env.blocks, v["block"].as_str().ok_or_else(|| anyhow!("random_block_match without block"))?)?), probability(v)?))
            }
            Some("minecraft:random_blockstate_match") => Ok(Self::RandomBlockState(block_state(env.blocks, &v["block_state"])?, probability(v)?)),
            other => bail!("unsupported rule test {other:?}"),
        }
    }

    /// `RuleTest.test(state, random)`
    pub fn test(&self, state: u16, r: &mut dyn RandomSource) -> bool {
        match self {
            Self::AlwaysTrue => true,
            Self::States(s) => s[state as usize],
            Self::BlockStateMatch(s) => state == *s,
            Self::RandomStates(s, p) => s[state as usize] && r.next_float() < *p,
            Self::RandomBlockState(s, p) => state == *s && r.next_float() < *p,
        }
    }
}

/// What parsing a feature needs.
pub struct Env<'a> {
    pub blocks: &'a Blocks,
    pub tags: &'a BlockTags,
    /// `configured_feature` registry: `minecraft:name` -> JSON
    pub configured: &'a HashMap<String, Value>,
}

// ------------------------------------------------------------------------------------------------ features

/// `OreConfiguration.TargetBlockState`
#[derive(Clone)]
pub struct OreTarget {
    pub rule: RuleTest,
    pub state: u16,
}

/// `OreConfiguration`
#[derive(Clone)]
pub struct OreConfig {
    pub size: i32,
    pub discard_chance_on_air_exposure: f32,
    pub targets: Vec<OreTarget>,
}

/// A configured feature the port can place.
#[derive(Clone)]
pub enum Feature {
    Ore(OreConfig),
    ScatteredOre(OreConfig),
}

impl Feature {
    /// A `configured_feature` entry (`{type, config}`).
    pub fn parse(env: &Env, v: &Value) -> Result<Self> {
        let config = &v["config"];
        match v.get("type").and_then(Value::as_str) {
            Some("minecraft:ore") => Ok(Self::Ore(parse_ore(env, config)?)),
            Some("minecraft:scattered_ore") => Ok(Self::ScatteredOre(parse_ore(env, config)?)),
            other => bail!("unsupported feature {other:?}"),
        }
    }

    /// `Feature.place(config, level, generator, random, origin)`
    pub fn place(&self, region: &mut Region, r: &mut dyn RandomSource, origin: (i32, i32, i32)) -> bool {
        if !region.ensure_can_write(origin.0, origin.2) {
            return false;
        }
        match self {
            Self::Ore(c) => place_ore(region, r, origin, c),
            Self::ScatteredOre(c) => place_scattered_ore(region, r, origin, c),
        }
    }
}

fn parse_ore(env: &Env, c: &Value) -> Result<OreConfig> {
    let targets = c["targets"]
        .as_array()
        .ok_or_else(|| anyhow!("ore without targets"))?
        .iter()
        .map(|t| Ok(OreTarget { rule: RuleTest::parse(env, &t["target"])?, state: block_state(env.blocks, &t["state"])? }))
        .collect::<Result<Vec<_>>>()?;
    let discard = match c.get("discard_chance_on_air_exposure") {
        Some(Value::Number(n)) => n.to_string().parse::<f32>()?,
        _ => 0.0,
    };
    Ok(OreConfig { size: int_field(c, "size")?, discard_chance_on_air_exposure: discard, targets })
}

/// `Mth.ceil(float)`
fn ceil(f: f32) -> i32 {
    let i = f as i32;
    if f > i as f32 { i + 1 } else { i }
}

/// `Math.round(float)`
fn round(f: f32) -> i32 {
    let fl = f.floor();
    (if f - fl >= 0.5 { fl + 1.0 } else { fl }) as i32
}

/// `Feature.isAdjacentToAir`
fn is_adjacent_to_air(get: &dyn Fn(i32, i32, i32) -> u16, q: &dyn BlockQuery, (x, y, z): (i32, i32, i32)) -> bool {
    [(0, -1, 0), (0, 1, 0), (0, 0, -1), (0, 0, 1), (-1, 0, 0), (1, 0, 0)].iter().any(|(dx, dy, dz)| q.is_air(get(x + dx, y + dy, z + dz)))
}

/// `OreFeature.canPlaceOre`
fn can_place_ore(
    state: u16,
    get: &dyn Fn(i32, i32, i32) -> u16,
    q: &dyn BlockQuery,
    r: &mut dyn RandomSource,
    cfg: &OreConfig,
    target: &OreTarget,
    pos: (i32, i32, i32),
) -> bool {
    if !target.rule.test(state, r) {
        return false;
    }
    // shouldSkipAirCheck
    let skip = if cfg.discard_chance_on_air_exposure <= 0.0 {
        true
    } else if cfg.discard_chance_on_air_exposure >= 1.0 {
        false
    } else {
        r.next_float() >= cfg.discard_chance_on_air_exposure
    };
    skip || !is_adjacent_to_air(get, q, pos)
}

/// `OreFeature.place`
fn place_ore(region: &mut Region, r: &mut dyn RandomSource, (ox, oy, oz): (i32, i32, i32), cfg: &OreConfig) -> bool {
    let size = cfg.size;
    let f5 = r.next_float() * PI_F;
    let f6 = size as f32 / 8.0f32;
    let i7 = ceil((size as f32 / 16.0f32 * 2.0f32 + 1.0f32) / 2.0f32);
    let d8 = ox as f64 + (f5 as f64).sin() * f6 as f64;
    let d9 = ox as f64 - (f5 as f64).sin() * f6 as f64;
    let d10 = oz as f64 + (f5 as f64).cos() * f6 as f64;
    let d11 = oz as f64 - (f5 as f64).cos() * f6 as f64;
    let d13 = (oy + r.next_int_bound(3) - 2) as f64;
    let d14 = (oy + r.next_int_bound(3) - 2) as f64;
    let i15 = ox - ceil(f6) - i7;
    let i16 = oy - 2 - i7;
    let i17 = oz - ceil(f6) - i7;
    let i18 = 2 * (ceil(f6) + i7);
    let i19 = 2 * (2 + i7);
    for x in i15..=i15 + i18 {
        for z in i17..=i17 + i18 {
            if i16 <= region.height(HeightmapKind::OceanFloorWg, x, z) {
                return do_place_ore(region, r, cfg, (d8, d9, d10, d11, d13, d14), (i15, i16, i17, i18, i19));
            }
        }
    }
    false
}

#[allow(clippy::too_many_arguments)]
fn do_place_ore(region: &mut Region, r: &mut dyn RandomSource, cfg: &OreConfig, d: (f64, f64, f64, f64, f64, f64), i: (i32, i32, i32, i32, i32)) -> bool {
    let (d3, d4, d5, d6, d7, d8) = d;
    let (i9, i10, i11, i12, i13) = i;
    let mut placed = 0;
    let mut bits = vec![false; (i12 * i13 * i12).max(0) as usize];
    let size = cfg.size as usize;
    let mut a = vec![0f64; size * 4];
    for n in 0..size {
        let f20 = n as f32 / size as f32;
        let x = mth::lerp(f20 as f64, d3, d4);
        let y = mth::lerp(f20 as f64, d7, d8);
        let z = mth::lerp(f20 as f64, d5, d6);
        let d24 = r.next_double() * size as f64 / 16.0;
        let d25 = (((crate::carver::sin(PI_F * f20) + 1.0f32) as f64) * d24 + 1.0) / 2.0;
        a[n * 4] = x;
        a[n * 4 + 1] = y;
        a[n * 4 + 2] = z;
        a[n * 4 + 3] = d25;
    }
    for n in 0..size.saturating_sub(1) {
        if a[n * 4 + 3] <= 0.0 {
            continue;
        }
        for m in n + 1..size {
            if a[m * 4 + 3] <= 0.0 {
                continue;
            }
            let dx = a[n * 4] - a[m * 4];
            let dy = a[n * 4 + 1] - a[m * 4 + 1];
            let dz = a[n * 4 + 2] - a[m * 4 + 2];
            let dr = a[n * 4 + 3] - a[m * 4 + 3];
            if dr * dr > dx * dx + dy * dy + dz * dz {
                if dr > 0.0 {
                    a[m * 4 + 3] = -1.0;
                } else {
                    a[n * 4 + 3] = -1.0;
                }
            }
        }
    }
    for n in 0..size {
        let radius = a[n * 4 + 3];
        if radius < 0.0 {
            continue;
        }
        let (cx, cy, cz) = (a[n * 4], a[n * 4 + 1], a[n * 4 + 2]);
        let x0 = i32::max(mth::floor(cx - radius), i9);
        let y0 = i32::max(mth::floor(cy - radius), i10);
        let z0 = i32::max(mth::floor(cz - radius), i11);
        let x1 = i32::max(mth::floor(cx + radius), x0);
        let y1 = i32::max(mth::floor(cy + radius), y0);
        let z1 = i32::max(mth::floor(cz + radius), z0);
        for x in x0..=x1 {
            let dx = ((x as f64) + 0.5 - cx) / radius;
            if dx * dx >= 1.0 {
                continue;
            }
            for y in y0..=y1 {
                let dy = ((y as f64) + 0.5 - cy) / radius;
                if dx * dx + dy * dy >= 1.0 {
                    continue;
                }
                for z in z0..=z1 {
                    let dz = ((z as f64) + 0.5 - cz) / radius;
                    if dx * dx + dy * dy + dz * dz < 1.0 && !region.is_outside_build_height(y) {
                        let idx = (x - i9 + (y - i10) * i12 + (z - i11) * i12 * i13) as usize;
                        if bits[idx] {
                            continue;
                        }
                        bits[idx] = true;
                        if region.ensure_can_write(x, z) {
                            let state = region.get_block(x, y, z);
                            for target in &cfg.targets {
                                let q = region.query();
                                // BulkSectionAccess.getBlockState: air outside the build height (void_air is air as well)
                                let get = |gx: i32, gy: i32, gz: i32| region.get_block(gx, gy, gz);
                                if can_place_ore(state, &get, q, r, cfg, target, (x, y, z)) {
                                    region.set_block_raw(x, y, z, target.state);
                                    placed += 1;
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    placed > 0
}

/// `ScatteredOreFeature.place`
fn place_scattered_ore(region: &mut Region, r: &mut dyn RandomSource, (ox, oy, oz): (i32, i32, i32), cfg: &OreConfig) -> bool {
    let count = r.next_int_bound(cfg.size + 1);
    for n in 0..count {
        let spread = i32::min(n, 7) as f32;
        let mut offset = || round((r.next_float() - r.next_float()) * spread);
        let dx = offset();
        let dy = offset();
        let dz = offset();
        let pos = (ox + dx, oy + dy, oz + dz);
        let state = region.get_block(pos.0, pos.1, pos.2);
        for target in &cfg.targets {
            let q = region.query();
            let get = |gx: i32, gy: i32, gz: i32| region.get_block(gx, gy, gz);
            if can_place_ore(state, &get, q, r, cfg, target, pos) {
                region.set_block(pos.0, pos.1, pos.2, target.state);
                break;
            }
        }
    }
    true
}
