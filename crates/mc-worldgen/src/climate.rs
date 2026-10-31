//! `net.minecraft.world.level.biome.Climate`: quantized climate parameters, the parameter-space R-tree and the sampler.
//!
//! The R-tree is built with the same sort / bucketize / cost rules as the Mojang class, because its shape decides the winner when two
//! entries are equally far away from a target point (and the thread-local "last result" shortcut makes the winner depend on the search history,
//! exactly as in the vanilla server).

use crate::density::{Ctx, Df};
use crate::mth;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// `Climate.quantizeCoord`: `(long)(f * 10000.0F)`, with the float multiplication of the JVM.
pub fn quantize(f: f32) -> i64 {
    (f * 10000.0f32) as i64
}

/// `Climate.unquantizeCoord`
pub fn unquantize(l: i64) -> f32 {
    l as f32 / 10000.0f32
}

/// `Climate.Parameter`: a closed interval of quantized values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Parameter {
    pub min: i64,
    pub max: i64,
}

impl Parameter {
    /// `Parameter.point`
    pub fn point(v: f32) -> Self {
        Self::span(v, v)
    }

    /// `Parameter.span(float, float)`
    pub fn span(min: f32, max: f32) -> Self {
        assert!(min <= max, "min > max: {min} {max}");
        Self { min: quantize(min), max: quantize(max) }
    }

    /// `Parameter.span(Parameter, Parameter)`
    pub fn span_of(a: Parameter, b: Parameter) -> Self {
        assert!(a.min <= b.max, "min > max: {a:?} {b:?}");
        Self { min: a.min, max: b.max }
    }

    /// `Parameter.distance(long)`
    pub fn distance(&self, v: i64) -> i64 {
        let above = v - self.max;
        let below = self.min - v;
        if above > 0 { above } else { below.max(0) }
    }

    /// `Parameter.span(@Nullable Parameter)`: the smallest interval containing both.
    fn union(&self, other: Option<Parameter>) -> Parameter {
        match other {
            None => *self,
            Some(o) => Parameter { min: self.min.min(o.min), max: self.max.max(o.max) },
        }
    }
}

/// `Climate.ParameterPoint`: temperature, humidity, continentalness, erosion, depth, weirdness and the offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParameterPoint {
    pub temperature: Parameter,
    pub humidity: Parameter,
    pub continentalness: Parameter,
    pub erosion: Parameter,
    pub depth: Parameter,
    pub weirdness: Parameter,
    pub offset: i64,
}

impl ParameterPoint {
    /// `Climate.parameters(Parameter x6, float offset)`
    pub fn new(
        temperature: Parameter,
        humidity: Parameter,
        continentalness: Parameter,
        erosion: Parameter,
        depth: Parameter,
        weirdness: Parameter,
        offset: f32,
    ) -> Self {
        Self { temperature, humidity, continentalness, erosion, depth, weirdness, offset: quantize(offset) }
    }

    /// `Climate.parameters(float x7)`
    pub fn points(t: f32, h: f32, c: f32, e: f32, d: f32, w: f32, offset: f32) -> Self {
        Self::new(Parameter::point(t), Parameter::point(h), Parameter::point(c), Parameter::point(e), Parameter::point(d), Parameter::point(w), offset)
    }

    /// `ParameterPoint.fitness`
    pub fn fitness(&self, t: &TargetPoint) -> i64 {
        sq(self.temperature.distance(t.temperature))
            + sq(self.humidity.distance(t.humidity))
            + sq(self.continentalness.distance(t.continentalness))
            + sq(self.erosion.distance(t.erosion))
            + sq(self.depth.distance(t.depth))
            + sq(self.weirdness.distance(t.weirdness))
            + sq(self.offset)
    }

    /// `ParameterPoint.parameterSpace`
    pub fn parameter_space(&self) -> [Parameter; 7] {
        [
            self.temperature,
            self.humidity,
            self.continentalness,
            self.erosion,
            self.depth,
            self.weirdness,
            Parameter { min: self.offset, max: self.offset },
        ]
    }
}

fn sq(v: i64) -> i64 {
    v.wrapping_mul(v)
}

/// `Climate.TargetPoint`: the quantized climate at a position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TargetPoint {
    pub temperature: i64,
    pub humidity: i64,
    pub continentalness: i64,
    pub erosion: i64,
    pub depth: i64,
    pub weirdness: i64,
}

impl TargetPoint {
    /// `Climate.target(float x6)`
    pub fn from_floats(t: f32, h: f32, c: f32, e: f32, d: f32, w: f32) -> Self {
        Self {
            temperature: quantize(t),
            humidity: quantize(h),
            continentalness: quantize(c),
            erosion: quantize(e),
            depth: quantize(d),
            weirdness: quantize(w),
        }
    }

    /// `TargetPoint.toParameterArray`: the offset of a target is always 0.
    pub fn to_array(&self) -> [i64; 7] {
        [self.temperature, self.humidity, self.continentalness, self.erosion, self.depth, self.weirdness, 0]
    }
}

// ------------------------------------------------------------------------------------------------ the R-tree

const CHILDREN_PER_NODE: usize = 6;
const DIMENSIONS: usize = 7;

enum Kind {
    Leaf(usize),
    Sub(Vec<u32>),
}

struct RNode {
    space: [Parameter; DIMENSIONS],
    kind: Kind,
}

/// `Climate.RTree` (an arena instead of linked nodes)
struct RTree {
    id: u64,
    nodes: Vec<RNode>,
    root: u32,
}

static NEXT_TREE_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    /// `RTree.lastResult`, a `ThreadLocal` per tree: the leaf found by the previous search on this thread.
    static LAST_RESULT: RefCell<HashMap<u64, u32>> = RefCell::new(HashMap::new());
}

fn mid(p: &Parameter) -> i64 {
    (p.min + p.max) / 2
}

impl RTree {
    fn create<T>(values: &[(ParameterPoint, T)]) -> RTree {
        assert!(!values.is_empty(), "Need at least one value to build the search tree.");
        let mut nodes: Vec<RNode> = values
            .iter()
            .enumerate()
            .map(|(i, (p, _))| RNode { space: p.parameter_space(), kind: Kind::Leaf(i) })
            .collect();
        let leaves: Vec<u32> = (0..nodes.len() as u32).collect();
        let root = build(&mut nodes, DIMENSIONS, leaves);
        RTree { id: NEXT_TREE_ID.fetch_add(1, Ordering::Relaxed), nodes, root }
    }

    /// `RTree.search` (with the thread-local shortcut); returns the index of the winning value.
    fn search(&self, target: &TargetPoint) -> usize {
        let t = target.to_array();
        let last = LAST_RESULT.with(|m| m.borrow().get(&self.id).copied());
        let found = self.search_node(self.root, &t, last).expect("a non-empty tree always has a result");
        LAST_RESULT.with(|m| m.borrow_mut().insert(self.id, found));
        match self.nodes[found as usize].kind {
            Kind::Leaf(i) => i,
            Kind::Sub(_) => unreachable!(),
        }
    }

    fn distance(&self, node: u32, t: &[i64; DIMENSIONS]) -> i64 {
        let n = &self.nodes[node as usize];
        let mut d = 0i64;
        for i in 0..DIMENSIONS {
            d += sq(n.space[i].distance(t[i]));
        }
        d
    }

    /// `Leaf.search` / `SubTree.search`
    fn search_node(&self, node: u32, t: &[i64; DIMENSIONS], last: Option<u32>) -> Option<u32> {
        match &self.nodes[node as usize].kind {
            Kind::Leaf(_) => Some(node),
            Kind::Sub(children) => {
                let mut best = match last {
                    None => i64::MAX,
                    Some(l) => self.distance(l, t),
                };
                let mut result = last;
                for &child in children {
                    let d = self.distance(child, t);
                    if best > d {
                        let leaf = self.search_node(child, t, result).expect("a subtree always finds a leaf");
                        let leaf_d = if child == leaf { d } else { self.distance(leaf, t) };
                        if best > leaf_d {
                            best = leaf_d;
                            result = Some(leaf);
                        }
                    }
                }
                result
            }
        }
    }
}

/// `RTree.buildParameterSpace`
fn parameter_space_of(nodes: &[RNode], children: &[u32]) -> [Parameter; DIMENSIONS] {
    assert!(!children.is_empty(), "SubTree needs at least one child");
    let mut space: [Option<Parameter>; DIMENSIONS] = [None; DIMENSIONS];
    for &c in children {
        for i in 0..DIMENSIONS {
            space[i] = Some(nodes[c as usize].space[i].union(space[i]));
        }
    }
    space.map(|p| p.unwrap())
}

fn new_subtree(nodes: &mut Vec<RNode>, children: Vec<u32>) -> u32 {
    let space = parameter_space_of(nodes, &children);
    nodes.push(RNode { space, kind: Kind::Sub(children) });
    (nodes.len() - 1) as u32
}

/// `RTree.sort`: ordered by the midpoint of `dim`, ties broken by the following dimensions (cyclically).
fn sort_nodes(nodes: &[RNode], list: &mut [u32], n: usize, dim: usize, absolute: bool) {
    list.sort_by(|&a, &b| {
        for k in 0..n {
            let d = (dim + k) % n;
            let (ma, mb) = (mid(&nodes[a as usize].space[d]), mid(&nodes[b as usize].space[d]));
            let (ka, kb) = if absolute { (ma.abs(), mb.abs()) } else { (ma, mb) };
            let ord = ka.cmp(&kb);
            if ord != std::cmp::Ordering::Equal {
                return ord;
            }
        }
        std::cmp::Ordering::Equal
    });
}

/// `RTree.bucketize`: groups of `6^floor(log6(size - 0.01))` consecutive nodes.
fn bucketize(nodes: &mut Vec<RNode>, list: &[u32]) -> Vec<u32> {
    let mut buckets = Vec::new();
    let mut current: Vec<u32> = Vec::new();
    let per_bucket = 6f64.powf(((list.len() as f64 - 0.01).ln() / 6f64.ln()).floor()) as usize;
    for &n in list {
        current.push(n);
        if current.len() >= per_bucket {
            buckets.push(new_subtree(nodes, std::mem::take(&mut current)));
        }
    }
    if !current.is_empty() {
        buckets.push(new_subtree(nodes, current));
    }
    buckets
}

/// `RTree.cost`
fn cost(space: &[Parameter; DIMENSIONS]) -> i64 {
    space.iter().map(|p| (p.max - p.min).abs()).sum()
}

/// `RTree.build`
fn build(nodes: &mut Vec<RNode>, n: usize, mut children: Vec<u32>) -> u32 {
    assert!(!children.is_empty(), "Need at least one child to build a node");
    if children.len() == 1 {
        return children[0];
    }
    if children.len() <= CHILDREN_PER_NODE {
        let key = |nodes: &[RNode], c: u32| -> i64 { (0..n).map(|i| mid(&nodes[c as usize].space[i]).abs()).sum() };
        let snapshot: Vec<(u32, i64)> = children.iter().map(|&c| (c, key(nodes, c))).collect();
        let mut sorted = snapshot;
        sorted.sort_by_key(|&(_, k)| k);
        children = sorted.into_iter().map(|(c, _)| c).collect();
        return new_subtree(nodes, children);
    }
    let mut best_cost = i64::MAX;
    let mut best_dim = usize::MAX;
    let mut best_buckets: Vec<u32> = Vec::new();
    for dim in 0..n {
        sort_nodes(nodes, &mut children, n, dim, false);
        let buckets = bucketize(nodes, &children);
        let total: i64 = buckets.iter().map(|&b| cost(&nodes[b as usize].space)).sum();
        if best_cost > total {
            best_cost = total;
            best_dim = dim;
            best_buckets = buckets;
        }
    }
    sort_nodes(nodes, &mut best_buckets, n, best_dim, true);
    let built: Vec<u32> = best_buckets
        .iter()
        .map(|&b| {
            let kids = match &nodes[b as usize].kind {
                Kind::Sub(c) => c.clone(),
                Kind::Leaf(_) => unreachable!(),
            };
            build(nodes, n, kids)
        })
        .collect();
    new_subtree(nodes, built)
}

/// `Climate.ParameterList`: the (parameter point, value) entries plus the search index.
pub struct ParameterList<T> {
    values: Vec<(ParameterPoint, T)>,
    index: RTree,
}

impl<T> ParameterList<T> {
    pub fn new(values: Vec<(ParameterPoint, T)>) -> Self {
        let index = RTree::create(&values);
        Self { values, index }
    }

    pub fn values(&self) -> &[(ParameterPoint, T)] {
        &self.values
    }

    /// `findValue` / `findValueIndex`: the R-tree search (history dependent on exact ties, like the Java `ThreadLocal`).
    pub fn find_value(&self, target: &TargetPoint) -> &T {
        &self.values[self.index.search(target)].1
    }

    /// `findValueBruteForce`: the first entry with the smallest fitness.
    pub fn find_value_brute_force(&self, target: &TargetPoint) -> &T {
        let mut best = (self.values[0].0.fitness(target), &self.values[0].1);
        for (p, v) in &self.values[1..] {
            let f = p.fitness(target);
            if f < best.0 {
                best = (f, v);
            }
        }
        best.1
    }
}

// ------------------------------------------------------------------------------------------------ the sampler

/// `Climate.Sampler`: the six climate functions evaluated at quart positions.
pub struct Sampler {
    pub temperature: Df,
    pub humidity: Df,
    pub continentalness: Df,
    pub erosion: Df,
    pub depth: Df,
    pub weirdness: Df,
    pub spawn_target: Vec<ParameterPoint>,
}

impl Sampler {
    /// `Sampler.sample` for a quart position (4 blocks).
    pub fn sample(&self, qx: i32, qy: i32, qz: i32) -> TargetPoint {
        let c = Ctx::point(qx << 2, qy << 2, qz << 2);
        TargetPoint::from_floats(
            self.temperature.compute(&c) as f32,
            self.humidity.compute(&c) as f32,
            self.continentalness.compute(&c) as f32,
            self.erosion.compute(&c) as f32,
            self.depth.compute(&c) as f32,
            self.weirdness.compute(&c) as f32,
        )
    }

    /// `Sampler.findSpawnPosition` (the block position's y is always 0).
    pub fn find_spawn_position(&self) -> (i32, i32) {
        if self.spawn_target.is_empty() {
            return (0, 0);
        }
        find_spawn_position(&self.spawn_target, self)
    }
}

/// `Climate.findSpawnPosition` (`SpawnFinder`)
pub fn find_spawn_position(targets: &[ParameterPoint], sampler: &Sampler) -> (i32, i32) {
    let mut result = spawn_fitness(targets, sampler, 0, 0);
    radial_search(targets, sampler, &mut result, 2048.0, 512.0);
    radial_search(targets, sampler, &mut result, 512.0, 32.0);
    (result.0, result.1)
}

fn radial_search(targets: &[ParameterPoint], sampler: &Sampler, result: &mut (i32, i32, i64), max_dist: f32, step: f32) {
    let mut angle = 0.0f32;
    let mut dist = step;
    let (cx, cz) = (result.0, result.1);
    while dist <= max_dist {
        let x = cx.wrapping_add(((angle as f64).sin() * dist as f64) as i32);
        let z = cz.wrapping_add(((angle as f64).cos() * dist as f64) as i32);
        let candidate = spawn_fitness(targets, sampler, x, z);
        if candidate.2 < result.2 {
            *result = candidate;
        }
        angle += step / dist;
        if angle as f64 > std::f64::consts::PI * 2.0 {
            angle = 0.0;
            dist += step;
        }
    }
}

fn spawn_fitness(targets: &[ParameterPoint], sampler: &Sampler, x: i32, z: i32) -> (i32, i32, i64) {
    let scale = mth::square(2500.0);
    let squared = ((x as i64) * (x as i64) + (z as i64) * (z as i64)) as f64 / scale;
    let penalty = (1.0e8f64 * (squared * squared)) as i64;
    let t = sampler.sample(x >> 2, 0, z >> 2);
    let flat = TargetPoint { depth: 0, ..t };
    let mut best = i64::MAX;
    for p in targets {
        best = best.min(p.fitness(&flat));
    }
    (x, z, penalty.wrapping_add(best))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantization_truncates_the_float_product() {
        assert_eq!(quantize(0.26666668), 2666);
        assert_eq!(quantize(-0.26666668), -2666);
        assert_eq!(quantize(-0.78), -7799); // -0.78F * 10000.0F is -7799.9995 in float arithmetic
        assert_eq!(quantize(1.0), 10000);
        assert_eq!(quantize(f32::NAN), 0);
        assert_eq!(unquantize(2666), 0.2666f32);
    }

    #[test]
    fn parameter_distance_is_zero_inside_and_linear_outside() {
        let p = Parameter { min: -100, max: 300 };
        assert_eq!((p.distance(-100), p.distance(0), p.distance(300)), (0, 0, 0));
        assert_eq!((p.distance(-150), p.distance(350)), (50, 50));
    }

    #[test]
    fn fitness_adds_the_squared_offset() {
        let pt = ParameterPoint::points(0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.1);
        let at = TargetPoint { temperature: 30, humidity: 0, continentalness: -40, erosion: 0, depth: 0, weirdness: 0 };
        assert_eq!(pt.fitness(&at), 30 * 30 + 40 * 40 + 1000 * 1000);
    }

    #[test]
    fn tree_search_equals_brute_force_when_there_are_no_ties() {
        // distinct points on a line: every target has a unique nearest entry
        let entries: Vec<(ParameterPoint, usize)> = (0..200usize).map(|i| (ParameterPoint::points(i as f32 / 100.0 - 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0), i)).collect();
        let list = ParameterList::new(entries);
        for t in -10000..=10000i64 {
            if t % 50 == 0 {
                continue; // exactly halfway between two entries
            }
            let target = TargetPoint { temperature: t, humidity: 1, continentalness: 2, erosion: 3, depth: 4, weirdness: 5 };
            assert_eq!(list.find_value(&target), list.find_value_brute_force(&target), "target {t}");
        }
    }
}
