//! Density functions (`net.minecraft.world.level.levelgen.DensityFunctions`): the node types, the JSON decoding with the same
//! wrapping rules as the Java codecs, the visitor (`mapAll`) machinery and `compute`.
//!
//! The structure follows the Java classes closely on purpose: where the Java code folds constants into `MulOrAdd`, recomputes bounds
//! in `mapAll` or short-circuits `min`/`max`, the same happens here, so results are bit-identical.

use crate::mth;
use crate::noise::{BlendedNoise, NormalNoise, SimplexNoise};
use crate::noise_chunk::ChunkState;
use crate::random::{LegacyRandomSource, RandomSource};
use crate::spline::{CubicSpline, SplineBounds, SplineCoordinate};
use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

pub type Df = Arc<Node>;

/// `Math.min` / `Math.max` for doubles (NaN propagates, -0.0 < 0.0)
pub fn jmin(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        return f64::NAN;
    }
    if a == 0.0 && b == 0.0 {
        return if a.is_sign_negative() { a } else { b };
    }
    if a < b { a } else { b }
}

pub fn jmax(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        return f64::NAN;
    }
    if a == 0.0 && b == 0.0 {
        return if a.is_sign_positive() { a } else { b };
    }
    if a > b { a } else { b }
}

/// `Mth.clamp(double, double, double)`
pub fn clamp(v: f64, lo: f64, hi: f64) -> f64 {
    if v < lo { lo } else { jmin(v, hi) }
}

/// `Mth.clampedMap`
pub fn clamped_map(v: f64, from_lo: f64, from_hi: f64, to_lo: f64, to_hi: f64) -> f64 {
    mth::clamped_lerp(to_lo, to_hi, (v - from_lo) / (from_hi - from_lo))
}

/// `DensityFunction.NoiseHolder`
#[derive(Clone)]
pub struct NoiseHolder {
    pub key: String,
    pub noise: Option<Arc<NormalNoise>>,
}

impl NoiseHolder {
    pub fn value(&self, x: f64, y: f64, z: f64) -> f64 {
        self.noise.as_ref().map_or(0.0, |n| n.get_value(x, y, z))
    }
    pub fn max_value(&self) -> f64 {
        self.noise.as_ref().map_or(2.0, |n| n.max_value())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Op {
    Add,
    Mul,
    Min,
    Max,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MapKind {
    Abs,
    Square,
    Cube,
    HalfNegative,
    QuarterNegative,
    Squeeze,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MarkerKind {
    Interpolated,
    FlatCache,
    Cache2D,
    CacheOnce,
    CacheAllInCell,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShiftKind {
    A,
    B,
    Both,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rarity {
    Type1,
    Type2,
}

impl Rarity {
    fn get(self, v: f64) -> f64 {
        match self {
            // QuantizedSpaghettiRarity.getSpaghettiRarity3D
            Rarity::Type1 => {
                if v < -0.5 {
                    0.75
                } else if v < 0.0 {
                    1.0
                } else if v < 0.5 {
                    1.5
                } else {
                    2.0
                }
            }
            // QuantizedSpaghettiRarity.getSphaghettiRarity2D
            Rarity::Type2 => {
                if v < -0.75 {
                    0.5
                } else if v < -0.5 {
                    0.75
                } else if v < 0.5 {
                    1.0
                } else if v < 0.75 {
                    2.0
                } else {
                    3.0
                }
            }
        }
    }
    fn max_rarity(self) -> f64 {
        match self {
            Rarity::Type1 => 2.0,
            Rarity::Type2 => 3.0,
        }
    }
}

/// `DensityFunctions.Spline.Coordinate`
pub struct Coordinate(pub Df);

impl SplineBounds for Coordinate {
    fn min_value(&self) -> f32 {
        self.0.min_value() as f32
    }
    fn max_value(&self) -> f32 {
        self.0.max_value() as f32
    }
}

impl<'a> SplineCoordinate<Ctx<'a>> for Coordinate {
    fn apply(&self, ctx: &Ctx<'a>) -> f32 {
        self.0.compute(ctx) as f32
    }
}

/// The parameters of `old_blended_noise` and its (seeded) noise.
pub struct Blended {
    pub params: [f64; 5],
    pub noise: Arc<BlendedNoise>,
}

pub enum Node {
    Constant(f64),
    Ap2 { op: Op, a: Df, b: Df, min: f64, max: f64 },
    MulOrAdd { add: bool, input: Df, min: f64, max: f64, arg: f64 },
    Mapped { kind: MapKind, input: Df, min: f64, max: f64 },
    Clamp { input: Df, min: f64, max: f64 },
    YClampedGradient { from_y: i32, to_y: i32, from_v: f64, to_v: f64 },
    Noise { noise: NoiseHolder, xz: f64, y: f64 },
    Shift { kind: ShiftKind, noise: NoiseHolder },
    ShiftedNoise { sx: Df, sy: Df, sz: Df, xz: f64, y: f64, noise: NoiseHolder },
    RangeChoice { input: Df, min_incl: f64, max_excl: f64, in_range: Df, out_range: Df },
    Spline(CubicSpline<Coordinate>),
    WeirdScaled { input: Df, noise: NoiseHolder, mapper: Rarity },
    OldBlendedNoise(Blended),
    EndIslands(Arc<SimplexNoise>),
    BlendAlpha,
    BlendOffset,
    BlendDensity(Df),
    Beardifier,
    /// `DensityFunctions.Marker` before the `NoiseChunk` replaced it
    Marker { kind: MarkerKind, wrapped: Df },
    /// `DensityFunctions.HolderHolder`
    HolderRef(Df),
    // ---- the replacements `NoiseChunk.wrap` creates (state lives in the `ChunkState`)
    Interpolated { id: usize, wrapped: Df },
    FlatCache { table: FlatTable, wrapped: Df },
    CacheAllInCell { id: usize, wrapped: Df },
    Cache2D { wrapped: Df },
    CacheOnce { wrapped: Df },
}

/// `NoiseChunk.FlatCache`: the wrapped function sampled once per 4x4 columns (at y = 0) over the chunk and its border.
pub struct FlatTable {
    pub values: Vec<f64>,
    pub size: usize,
    pub first_noise_x: i32,
    pub first_noise_z: i32,
}

/// `DensityFunction.FunctionContext`: a plain position, or the `NoiseChunk` itself (then `chunk` is set and the position is the
/// chunk's current block position).
#[derive(Clone, Copy)]
pub struct Ctx<'a> {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub chunk: Option<&'a ChunkState>,
}

impl<'a> Ctx<'a> {
    pub fn point(x: i32, y: i32, z: i32) -> Ctx<'static> {
        Ctx { x, y, z, chunk: None }
    }
}

fn mapped_transform(kind: MapKind, v: f64) -> f64 {
    match kind {
        MapKind::Abs => v.abs(),
        MapKind::Square => v * v,
        MapKind::Cube => v * v * v,
        MapKind::HalfNegative => {
            if v > 0.0 { v } else { v * 0.5 }
        }
        MapKind::QuarterNegative => {
            if v > 0.0 { v } else { v * 0.25 }
        }
        MapKind::Squeeze => {
            let c = clamp(v, -1.0, 1.0);
            c / 2.0 - c * c * c / 24.0
        }
    }
}

/// `Mapped.create`
pub fn create_mapped(kind: MapKind, input: Df) -> Df {
    let in_min = input.min_value();
    let a = mapped_transform(kind, in_min);
    let b = mapped_transform(kind, input.max_value());
    let (min, max) = if kind != MapKind::Abs && kind != MapKind::Square { (a, b) } else { (jmax(0.0, in_min), jmax(a, b)) };
    Arc::new(Node::Mapped { kind, input, min, max })
}

/// `TwoArgumentSimpleFunction.create`
pub fn create_ap2(op: Op, a: Df, b: Df) -> Df {
    let (amin, bmin, amax, bmax) = (a.min_value(), b.min_value(), a.max_value(), b.max_value());
    let min = match op {
        Op::Add => amin + bmin,
        Op::Mul => {
            if amin > 0.0 && bmin > 0.0 {
                amin * bmin
            } else if amax < 0.0 && bmax < 0.0 {
                amax * bmax
            } else {
                jmin(amin * bmax, amax * bmin)
            }
        }
        Op::Min => jmin(amin, bmin),
        Op::Max => jmax(amin, bmin),
    };
    let max = match op {
        Op::Add => amax + bmax,
        Op::Mul => {
            if amin > 0.0 && bmin > 0.0 {
                amax * bmax
            } else if amax < 0.0 && bmax < 0.0 {
                amin * bmin
            } else {
                jmax(amin * bmin, amax * bmax)
            }
        }
        Op::Min => jmin(amax, bmax),
        Op::Max => jmax(amax, bmax),
    };
    if op == Op::Mul || op == Op::Add {
        if let Node::Constant(c) = &*a {
            return Arc::new(Node::MulOrAdd { add: op == Op::Add, input: b, min, max, arg: *c });
        }
        if let Node::Constant(c) = &*b {
            return Arc::new(Node::MulOrAdd { add: op == Op::Add, input: a, min, max, arg: *c });
        }
    }
    Arc::new(Node::Ap2 { op, a, b, min, max })
}

/// `EndIslandDensityFunction.getHeightValue`
fn end_island_height(noise: &SimplexNoise, x: i32, z: i32) -> f32 {
    let (hx, hz) = (x / 2, z / 2);
    let (rx, rz) = (x % 2, z % 2);
    let mut v = 100.0f32 - sqrt_f32((x.wrapping_mul(x).wrapping_add(z.wrapping_mul(z))) as f32) * 8.0;
    v = clamp_f32(v, -100.0, 80.0);
    for dx in -12..=12i32 {
        for dz in -12..=12i32 {
            let a = (hx + dx) as i64;
            let b = (hz + dz) as i64;
            if a * a + b * b > 4096 && noise.get_value_2d(a as f64, b as f64) < (-0.9f32) as f64 {
                let strength = (((a as f32).abs() * 3439.0f32) + ((b as f32).abs() * 147.0f32)) % 13.0f32 + 9.0f32;
                let fx = (rx - dx * 2) as f32;
                let fz = (rz - dz * 2) as f32;
                let mut s = 100.0f32 - sqrt_f32(fx * fx + fz * fz) * strength;
                s = clamp_f32(s, -100.0, 80.0);
                v = if v.is_nan() || s.is_nan() { f32::NAN } else if v > s { v } else { s };
            }
        }
    }
    v
}

/// `Mth.sqrt(float)`
fn sqrt_f32(v: f32) -> f32 {
    (v as f64).sqrt() as f32
}

/// `Mth.clamp(float, float, float)`
fn clamp_f32(v: f32, lo: f32, hi: f32) -> f32 {
    if v < lo {
        lo
    } else if v.is_nan() || hi.is_nan() {
        f32::NAN
    } else if v < hi {
        v
    } else {
        hi
    }
}

impl Node {
    pub fn min_value(&self) -> f64 {
        match self {
            Node::Constant(v) => *v,
            Node::Ap2 { min, .. } | Node::MulOrAdd { min, .. } | Node::Mapped { min, .. } | Node::Clamp { min, .. } => *min,
            Node::YClampedGradient { from_v, to_v, .. } => jmin(*from_v, *to_v),
            Node::Noise { .. } | Node::Shift { .. } | Node::ShiftedNoise { .. } => -self.max_value(),
            Node::RangeChoice { in_range, out_range, .. } => jmin(in_range.min_value(), out_range.min_value()),
            Node::Spline(s) => s.min_value() as f64,
            Node::WeirdScaled { .. } => 0.0,
            Node::OldBlendedNoise(b) => b.noise.min_value(),
            Node::EndIslands(_) => -0.84375,
            Node::BlendAlpha => 1.0,
            Node::BlendOffset | Node::Beardifier => 0.0,
            Node::BlendDensity(_) => f64::NEG_INFINITY,
            Node::Marker { wrapped, .. }
            | Node::HolderRef(wrapped)
            | Node::Interpolated { wrapped, .. }
            | Node::FlatCache { wrapped, .. }
            | Node::CacheAllInCell { wrapped, .. }
            | Node::Cache2D { wrapped }
            | Node::CacheOnce { wrapped } => wrapped.min_value(),
        }
    }

    pub fn max_value(&self) -> f64 {
        match self {
            Node::Constant(v) => *v,
            Node::Ap2 { max, .. } | Node::MulOrAdd { max, .. } | Node::Mapped { max, .. } | Node::Clamp { max, .. } => *max,
            Node::YClampedGradient { from_v, to_v, .. } => jmax(*from_v, *to_v),
            Node::Noise { noise, .. } | Node::ShiftedNoise { noise, .. } => noise.max_value(),
            Node::Shift { noise, .. } => noise.max_value() * 4.0,
            Node::RangeChoice { in_range, out_range, .. } => jmax(in_range.max_value(), out_range.max_value()),
            Node::Spline(s) => s.max_value() as f64,
            Node::WeirdScaled { noise, mapper, .. } => mapper.max_rarity() * noise.max_value(),
            Node::OldBlendedNoise(b) => b.noise.max_value(),
            Node::EndIslands(_) => 0.5625,
            Node::BlendAlpha => 1.0,
            Node::BlendOffset | Node::Beardifier => 0.0,
            Node::BlendDensity(_) => f64::INFINITY,
            Node::Marker { wrapped, .. }
            | Node::HolderRef(wrapped)
            | Node::Interpolated { wrapped, .. }
            | Node::FlatCache { wrapped, .. }
            | Node::CacheAllInCell { wrapped, .. }
            | Node::Cache2D { wrapped }
            | Node::CacheOnce { wrapped } => wrapped.max_value(),
        }
    }

    pub fn compute(&self, c: &Ctx<'_>) -> f64 {
        match self {
            Node::Constant(v) => *v,
            Node::Ap2 { op, a, b, .. } => {
                let x = a.compute(c);
                match op {
                    Op::Add => x + b.compute(c),
                    Op::Mul => {
                        if x == 0.0 {
                            0.0
                        } else {
                            x * b.compute(c)
                        }
                    }
                    Op::Min => {
                        if x < b.min_value() {
                            x
                        } else {
                            jmin(x, b.compute(c))
                        }
                    }
                    Op::Max => {
                        if x > b.max_value() {
                            x
                        } else {
                            jmax(x, b.compute(c))
                        }
                    }
                }
            }
            Node::MulOrAdd { add, input, arg, .. } => {
                let v = input.compute(c);
                if *add { v + arg } else { v * arg }
            }
            Node::Mapped { kind, input, .. } => mapped_transform(*kind, input.compute(c)),
            Node::Clamp { input, min, max } => clamp(input.compute(c), *min, *max),
            Node::YClampedGradient { from_y, to_y, from_v, to_v } => clamped_map(c.y as f64, *from_y as f64, *to_y as f64, *from_v, *to_v),
            Node::Noise { noise, xz, y } => noise.value(c.x as f64 * xz, c.y as f64 * y, c.z as f64 * xz),
            Node::Shift { kind, noise } => {
                let (a, b, d) = match kind {
                    ShiftKind::A => (c.x as f64, 0.0, c.z as f64),
                    ShiftKind::B => (c.z as f64, c.x as f64, 0.0),
                    ShiftKind::Both => (c.x as f64, c.y as f64, c.z as f64),
                };
                noise.value(a * 0.25, b * 0.25, d * 0.25) * 4.0
            }
            Node::ShiftedNoise { sx, sy, sz, xz, y, noise } => {
                let x = c.x as f64 * xz + sx.compute(c);
                let yy = c.y as f64 * y + sy.compute(c);
                let z = c.z as f64 * xz + sz.compute(c);
                noise.value(x, yy, z)
            }
            Node::RangeChoice { input, min_incl, max_excl, in_range, out_range } => {
                let v = input.compute(c);
                if v >= *min_incl && v < *max_excl { in_range.compute(c) } else { out_range.compute(c) }
            }
            Node::Spline(s) => s.apply(c) as f64,
            Node::WeirdScaled { input, noise, mapper } => {
                let r = mapper.get(input.compute(c));
                r * noise.value(c.x as f64 / r, c.y as f64 / r, c.z as f64 / r).abs()
            }
            Node::OldBlendedNoise(b) => b.noise.compute(c.x, c.y, c.z),
            Node::EndIslands(n) => (end_island_height(n, c.x / 8, c.z / 8) as f64 - 8.0) / 128.0,
            Node::BlendAlpha => 1.0,
            Node::BlendOffset | Node::Beardifier => 0.0,
            // the empty blender returns the density unchanged
            Node::BlendDensity(input) => input.compute(c),
            Node::Marker { wrapped, .. } | Node::HolderRef(wrapped) | Node::Cache2D { wrapped } | Node::CacheOnce { wrapped } => wrapped.compute(c),
            Node::Interpolated { id, wrapped } => match c.chunk {
                None => wrapped.compute(c),
                Some(st) => st.interpolated_value(*id),
            },
            Node::FlatCache { table, wrapped } => {
                // the table is read for every kind of context; positions outside of it are computed directly
                let qx = (c.x >> 2) - table.first_noise_x;
                let qz = (c.z >> 2) - table.first_noise_z;
                let n = table.size as i32;
                if qx >= 0 && qz >= 0 && qx < n && qz < n { table.values[(qx * n + qz) as usize] } else { wrapped.compute(c) }
            }
            Node::CacheAllInCell { id, wrapped } => match c.chunk {
                None => wrapped.compute(c),
                Some(st) => match st.cell_cache_lookup(*id) {
                    Some(v) => v,
                    None => wrapped.compute(c),
                },
            },
        }
    }
}

// ------------------------------------------------------------------------------------------------ visitors

/// `DensityFunction.Visitor`
pub trait Visitor {
    fn apply(&mut self, n: Df) -> Df;
    fn visit_noise(&mut self, h: &NoiseHolder) -> NoiseHolder {
        h.clone()
    }
}

/// `mapAll` with a memo for the shared functions (the graph is a DAG: every reference to a registered function leads to the same
/// node, and its mapped version is created only once).
pub struct Mapper<'v> {
    pub visitor: &'v mut dyn Visitor,
    memo: HashMap<usize, Df>,
}

impl<'v> Mapper<'v> {
    pub fn new(visitor: &'v mut dyn Visitor) -> Self {
        Self { visitor, memo: HashMap::new() }
    }

    fn shared(&mut self, f: &Df) -> Df {
        let key = Arc::as_ptr(f) as usize;
        if let Some(m) = self.memo.get(&key) {
            return m.clone();
        }
        let m = self.map_all(f);
        self.memo.insert(key, m.clone());
        m
    }

    pub fn map_all(&mut self, node: &Df) -> Df {
        match &**node {
            Node::Constant(_)
            | Node::YClampedGradient { .. }
            | Node::BlendAlpha
            | Node::BlendOffset
            | Node::Beardifier
            | Node::EndIslands(_)
            | Node::OldBlendedNoise(_) => self.visitor.apply(node.clone()),
            Node::Ap2 { op, a, b, .. } => {
                let (a, b) = (self.map_all(a), self.map_all(b));
                let n = create_ap2(*op, a, b);
                self.visitor.apply(n)
            }
            Node::MulOrAdd { add, input, arg, .. } => {
                let i = self.map_all(input);
                let (lo, hi) = (i.min_value(), i.max_value());
                let (min, max) = if *add {
                    (lo + arg, hi + arg)
                } else if *arg >= 0.0 {
                    (lo * arg, hi * arg)
                } else {
                    (hi * arg, lo * arg)
                };
                Arc::new(Node::MulOrAdd { add: *add, input: i, min, max, arg: *arg })
            }
            Node::Mapped { kind, input, .. } => {
                let i = self.map_all(input);
                create_mapped(*kind, i)
            }
            Node::Clamp { input, min, max } => Arc::new(Node::Clamp { input: self.map_all(input), min: *min, max: *max }),
            Node::Noise { noise, xz, y } => {
                let n = self.visitor.visit_noise(noise);
                self.visitor.apply(Arc::new(Node::Noise { noise: n, xz: *xz, y: *y }))
            }
            Node::Shift { kind, noise } => {
                let n = self.visitor.visit_noise(noise);
                self.visitor.apply(Arc::new(Node::Shift { kind: *kind, noise: n }))
            }
            Node::ShiftedNoise { sx, sy, sz, xz, y, noise } => {
                let (sx, sy, sz) = (self.map_all(sx), self.map_all(sy), self.map_all(sz));
                let n = self.visitor.visit_noise(noise);
                self.visitor.apply(Arc::new(Node::ShiftedNoise { sx, sy, sz, xz: *xz, y: *y, noise: n }))
            }
            Node::RangeChoice { input, min_incl, max_excl, in_range, out_range } => {
                let (input, in_range, out_range) = (self.map_all(input), self.map_all(in_range), self.map_all(out_range));
                self.visitor.apply(Arc::new(Node::RangeChoice { input, min_incl: *min_incl, max_excl: *max_excl, in_range, out_range }))
            }
            Node::Spline(s) => {
                let mapped = s.map_all(&mut |c: &Coordinate| Coordinate(self.shared(&c.0)));
                self.visitor.apply(Arc::new(Node::Spline(mapped)))
            }
            Node::WeirdScaled { input, noise, mapper } => {
                let input = self.map_all(input);
                let n = self.visitor.visit_noise(noise);
                self.visitor.apply(Arc::new(Node::WeirdScaled { input, noise: n, mapper: *mapper }))
            }
            Node::BlendDensity(input) => {
                let i = self.map_all(input);
                self.visitor.apply(Arc::new(Node::BlendDensity(i)))
            }
            Node::Marker { kind, wrapped } => {
                let w = self.map_all(wrapped);
                self.visitor.apply(Arc::new(Node::Marker { kind: *kind, wrapped: w }))
            }
            Node::HolderRef(f) => {
                let w = self.shared(f);
                self.visitor.apply(Arc::new(Node::HolderRef(w)))
            }
            // nodes that only exist after `NoiseChunk.wrap` are never mapped again
            Node::Interpolated { .. } | Node::FlatCache { .. } | Node::CacheAllInCell { .. } | Node::Cache2D { .. } | Node::CacheOnce { .. } => {
                node.clone()
            }
        }
    }
}

// ------------------------------------------------------------------------------------------------ decoding

/// The registered density functions (`worldgen/density_function/**.json`), decoded lazily.
pub struct DensityRegistry {
    json: HashMap<String, Value>,
    built: RefCell<HashMap<String, Df>>,
}

impl DensityRegistry {
    pub fn new(json: HashMap<String, Value>) -> Self {
        Self { json, built: RefCell::new(HashMap::new()) }
    }

    /// A registry entry (decoded with `DensityFunction.DIRECT_CODEC`).
    pub fn get(&self, key: &str) -> Result<Df> {
        if let Some(d) = self.built.borrow().get(key) {
            return Ok(d.clone());
        }
        let v = self.json.get(key).ok_or_else(|| anyhow!("unknown density function {key}"))?;
        let d = self.decode_direct(v).map_err(|e| anyhow!("{key}: {e}"))?;
        self.built.borrow_mut().insert(key.to_owned(), d.clone());
        Ok(d)
    }

    /// `DensityFunction.HOLDER_HELPER_CODEC`: a reference or an inline definition, wrapped in a `HolderHolder` either way.
    pub fn decode_holder(&self, v: &Value) -> Result<Df> {
        let inner = match v {
            Value::String(s) => self.get(s)?,
            other => self.decode_direct(other)?,
        };
        Ok(Arc::new(Node::HolderRef(inner)))
    }

    /// A holder whose value is used directly (the spline coordinate): the function itself.
    fn decode_value(&self, v: &Value) -> Result<Df> {
        match v {
            Value::String(s) => self.get(s),
            other => self.decode_direct(other),
        }
    }

    /// `DensityFunctions.DIRECT_CODEC`: a number is a constant, an object is dispatched on its `type`.
    pub fn decode_direct(&self, v: &Value) -> Result<Df> {
        match v {
            Value::Number(n) => Ok(Arc::new(Node::Constant(n.as_f64().ok_or_else(|| anyhow!("bad number"))?))),
            Value::Object(o) => {
                let ty = o.get("type").and_then(Value::as_str).ok_or_else(|| anyhow!("missing type"))?;
                let ty = ty.strip_prefix("minecraft:").unwrap_or(ty);
                self.decode_typed(ty, v)
            }
            _ => bail!("unexpected density function value {v}"),
        }
    }

    fn f64_of(v: &Value, key: &str) -> Result<f64> {
        v.get(key).and_then(Value::as_f64).ok_or_else(|| anyhow!("missing number {key}"))
    }

    fn noise_of(v: &Value, key: &str) -> Result<NoiseHolder> {
        match v.get(key) {
            Some(Value::String(s)) => Ok(NoiseHolder { key: s.clone(), noise: None }),
            _ => bail!("missing noise reference {key}"),
        }
    }

    fn arg(&self, v: &Value, key: &str) -> Result<Df> {
        self.decode_holder(v.get(key).ok_or_else(|| anyhow!("missing {key}"))?)
    }

    fn decode_typed(&self, ty: &str, v: &Value) -> Result<Df> {
        let node = match ty {
            "constant" => Node::Constant(Self::f64_of(v, "argument")?),
            "add" => return Ok(create_ap2(Op::Add, self.arg(v, "argument1")?, self.arg(v, "argument2")?)),
            "mul" => return Ok(create_ap2(Op::Mul, self.arg(v, "argument1")?, self.arg(v, "argument2")?)),
            "min" => return Ok(create_ap2(Op::Min, self.arg(v, "argument1")?, self.arg(v, "argument2")?)),
            "max" => return Ok(create_ap2(Op::Max, self.arg(v, "argument1")?, self.arg(v, "argument2")?)),
            "abs" => return Ok(create_mapped(MapKind::Abs, self.arg(v, "argument")?)),
            "square" => return Ok(create_mapped(MapKind::Square, self.arg(v, "argument")?)),
            "cube" => return Ok(create_mapped(MapKind::Cube, self.arg(v, "argument")?)),
            "half_negative" => return Ok(create_mapped(MapKind::HalfNegative, self.arg(v, "argument")?)),
            "quarter_negative" => return Ok(create_mapped(MapKind::QuarterNegative, self.arg(v, "argument")?)),
            "squeeze" => return Ok(create_mapped(MapKind::Squeeze, self.arg(v, "argument")?)),
            "clamp" => Node::Clamp {
                input: self.decode_direct(v.get("input").ok_or_else(|| anyhow!("missing input"))?)?,
                min: Self::f64_of(v, "min")?,
                max: Self::f64_of(v, "max")?,
            },
            "y_clamped_gradient" => Node::YClampedGradient {
                from_y: Self::f64_of(v, "from_y")? as i32,
                to_y: Self::f64_of(v, "to_y")? as i32,
                from_v: Self::f64_of(v, "from_value")?,
                to_v: Self::f64_of(v, "to_value")?,
            },
            "noise" => Node::Noise { noise: Self::noise_of(v, "noise")?, xz: Self::f64_of(v, "xz_scale")?, y: Self::f64_of(v, "y_scale")? },
            "shift_a" => Node::Shift { kind: ShiftKind::A, noise: Self::noise_of(v, "argument")? },
            "shift_b" => Node::Shift { kind: ShiftKind::B, noise: Self::noise_of(v, "argument")? },
            "shift" => Node::Shift { kind: ShiftKind::Both, noise: Self::noise_of(v, "argument")? },
            "shifted_noise" => Node::ShiftedNoise {
                sx: self.arg(v, "shift_x")?,
                sy: self.arg(v, "shift_y")?,
                sz: self.arg(v, "shift_z")?,
                xz: Self::f64_of(v, "xz_scale")?,
                y: Self::f64_of(v, "y_scale")?,
                noise: Self::noise_of(v, "noise")?,
            },
            "range_choice" => Node::RangeChoice {
                input: self.arg(v, "input")?,
                min_incl: Self::f64_of(v, "min_inclusive")?,
                max_excl: Self::f64_of(v, "max_exclusive")?,
                in_range: self.arg(v, "when_in_range")?,
                out_range: self.arg(v, "when_out_of_range")?,
            },
            "spline" => Node::Spline(self.decode_spline(v.get("spline").ok_or_else(|| anyhow!("missing spline"))?)?),
            "weird_scaled_sampler" => Node::WeirdScaled {
                input: self.arg(v, "input")?,
                noise: Self::noise_of(v, "noise")?,
                mapper: match v.get("rarity_value_mapper").and_then(Value::as_str) {
                    Some("type_1") => Rarity::Type1,
                    Some("type_2") => Rarity::Type2,
                    other => bail!("unknown rarity_value_mapper {other:?}"),
                },
            },
            "old_blended_noise" => {
                let params = [
                    Self::f64_of(v, "xz_scale")?,
                    Self::f64_of(v, "y_scale")?,
                    Self::f64_of(v, "xz_factor")?,
                    Self::f64_of(v, "y_factor")?,
                    Self::f64_of(v, "smear_scale_multiplier")?,
                ];
                Node::OldBlendedNoise(Blended { params, noise: Arc::new(BlendedNoise::create_unseeded(params[0], params[1], params[2], params[3], params[4])) })
            }
            "end_islands" => Node::EndIslands(Arc::new(end_island_noise(0))),
            "blend_alpha" => Node::BlendAlpha,
            "blend_offset" => Node::BlendOffset,
            "blend_density" => Node::BlendDensity(self.arg(v, "argument")?),
            "beardifier" => Node::Beardifier,
            "interpolated" => Node::Marker { kind: MarkerKind::Interpolated, wrapped: self.arg(v, "argument")? },
            "flat_cache" => Node::Marker { kind: MarkerKind::FlatCache, wrapped: self.arg(v, "argument")? },
            "cache_2d" => Node::Marker { kind: MarkerKind::Cache2D, wrapped: self.arg(v, "argument")? },
            "cache_once" => Node::Marker { kind: MarkerKind::CacheOnce, wrapped: self.arg(v, "argument")? },
            "cache_all_in_cell" => Node::Marker { kind: MarkerKind::CacheAllInCell, wrapped: self.arg(v, "argument")? },
            other => bail!("unsupported density function type {other}"),
        };
        Ok(Arc::new(node))
    }

    fn decode_spline(&self, v: &Value) -> Result<CubicSpline<Coordinate>> {
        match v {
            Value::Number(n) => Ok(CubicSpline::Constant(n.as_f64().ok_or_else(|| anyhow!("bad spline value"))? as f32)),
            Value::Object(_) => {
                let coordinate = Coordinate(self.decode_value(v.get("coordinate").ok_or_else(|| anyhow!("missing coordinate"))?)?);
                let points = v.get("points").and_then(Value::as_array).ok_or_else(|| anyhow!("missing points"))?;
                let (mut locations, mut values, mut derivatives) = (Vec::new(), Vec::new(), Vec::new());
                for p in points {
                    locations.push(Self::f64_of(p, "location")? as f32);
                    derivatives.push(Self::f64_of(p, "derivative")? as f32);
                    values.push(self.decode_spline(p.get("value").ok_or_else(|| anyhow!("missing value"))?)?);
                }
                Ok(CubicSpline::multipoint(coordinate, locations, values, derivatives))
            }
            _ => bail!("bad spline {v}"),
        }
    }
}

/// `new EndIslandDensityFunction(seed)`: the island noise comes from a legacy random advanced by 17292 calls.
pub fn end_island_noise(seed: i64) -> SimplexNoise {
    let mut r = LegacyRandomSource::new(seed);
    r.consume_count(17292);
    SimplexNoise::new(&mut r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(v: f64) -> Df {
        Arc::new(Node::Constant(v))
    }

    #[test]
    fn java_min_max_handle_nan_and_signed_zero() {
        assert!(jmin(f64::NAN, 1.0).is_nan() && jmax(1.0, f64::NAN).is_nan());
        assert!(jmin(0.0, -0.0).is_sign_negative() && jmin(-0.0, 0.0).is_sign_negative());
        assert!(jmax(0.0, -0.0).is_sign_positive() && jmax(-0.0, 0.0).is_sign_positive());
        assert_eq!((jmin(1.0, 2.0), jmax(1.0, 2.0)), (1.0, 2.0));
    }

    #[test]
    fn constants_are_folded_into_mul_or_add_with_the_right_bounds() {
        let noise = Arc::new(Node::YClampedGradient { from_y: 0, to_y: 10, from_v: -1.0, to_v: 3.0 });
        let m = create_ap2(Op::Mul, c(2.0), noise.clone());
        assert!(matches!(&*m, Node::MulOrAdd { add: false, arg, .. } if *arg == 2.0));
        assert_eq!((m.min_value(), m.max_value()), (-2.0, 6.0));
        let a = create_ap2(Op::Add, noise.clone(), c(-0.5));
        assert!(matches!(&*a, Node::MulOrAdd { add: true, .. }));
        assert_eq!((a.min_value(), a.max_value()), (-1.5, 2.5));
        // min / max never fold; two non-constants stay an Ap2
        assert!(matches!(&*create_ap2(Op::Min, c(1.0), noise.clone()), Node::Ap2 { .. }));
        assert!(matches!(&*create_ap2(Op::Mul, noise.clone(), noise), Node::Ap2 { .. }));
    }

    #[test]
    fn mapped_functions_and_clamped_gradient() {
        let ctx = Ctx::point(0, 5, 0);
        let g = Arc::new(Node::YClampedGradient { from_y: 0, to_y: 10, from_v: 0.0, to_v: 1.0 });
        assert_eq!(g.compute(&ctx), 0.5);
        assert_eq!(g.compute(&Ctx::point(0, -100, 0)), 0.0);
        assert_eq!(g.compute(&Ctx::point(0, 100, 0)), 1.0);
        assert_eq!(create_mapped(MapKind::HalfNegative, c(-4.0)).compute(&ctx), -2.0);
        assert_eq!(create_mapped(MapKind::QuarterNegative, c(-4.0)).compute(&ctx), -1.0);
        assert_eq!(create_mapped(MapKind::Squeeze, c(2.0)).compute(&ctx), 0.5 - 1.0 / 24.0);
        // abs / square have non-negative bounds
        let sq = create_mapped(MapKind::Square, g);
        assert_eq!((sq.min_value(), sq.max_value()), (0.0, 1.0));
    }

    #[test]
    fn range_choice_and_min_max_shortcuts() {
        let ctx = Ctx::point(0, 0, 0);
        let rc = Arc::new(Node::RangeChoice { input: c(0.5), min_incl: 0.0, max_excl: 0.5, in_range: c(1.0), out_range: c(2.0) });
        assert_eq!(rc.compute(&ctx), 2.0); // the upper bound is exclusive
        let mn = create_ap2(Op::Min, c(5.0), c(3.0));
        let mx = create_ap2(Op::Max, c(5.0), c(3.0));
        assert_eq!((mn.compute(&ctx), mx.compute(&ctx)), (3.0, 5.0));
    }

    #[test]
    fn json_references_are_wrapped_like_the_java_codecs() {
        let mut json = HashMap::new();
        json.insert("minecraft:zero".to_owned(), serde_json::json!(0.0));
        json.insert("minecraft:both".to_owned(), serde_json::json!({"type": "minecraft:add", "argument1": "minecraft:zero", "argument2": 1.5}));
        let reg = DensityRegistry::new(json);
        let both = reg.get("minecraft:both").unwrap();
        // the arguments are HolderHolders, so nothing is folded yet ...
        assert!(matches!(&*both, Node::Ap2 { .. }));
        assert_eq!(both.compute(&Ctx::point(0, 0, 0)), 1.5);
        // ... but mapping it (as NoiseChunk.wrap does) unwraps them and folds the constants
        struct Unwrap;
        impl Visitor for Unwrap {
            fn apply(&mut self, n: Df) -> Df {
                if let Node::HolderRef(f) = &*n { f.clone() } else { n }
            }
        }
        let mapped = Mapper::new(&mut Unwrap).map_all(&both);
        assert!(matches!(&*mapped, Node::MulOrAdd { add: true, .. }));
        assert_eq!(mapped.compute(&Ctx::point(0, 0, 0)), 1.5);
    }
}
