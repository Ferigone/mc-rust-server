//! Light engine (milestone M4): block light and sky light of a chunk, computed from the blocks of the 3x3 chunks around it.
//!
//! Vanilla (`net.minecraft.world.level.lighting`) maintains light incrementally; the result it converges to is a
//! fixpoint that is defined by a few rules, which are implemented here directly:
//!
//! * **Which sections store light** (`LayerLightSectionStorage.updateSectionStatus`): a light section stores data when it
//!   or any of its 26 neighbouring sections is non-empty (`!hasOnlyAir`); light sections exist from one below to one above
//!   the block sections. Light never enters other sections.
//! * **Propagation** (`LightEngine`, `BlockLightEngine.propagateIncrease`): a lit cell of level `L > 1` gives its
//!   neighbour `L - max(1, lightBlock(neighbour))` when that is higher than the neighbour's level, unless the face shapes
//!   of the two blocks occlude each other (`shapeOccludes`, only blocks with `canOcclude && useShapeForLightOcclusion`).
//! * **Block light** starts at the light emission of every block.
//! * **Sky light** starts at 15 in every cell at or above the column's lowest sky source (`ChunkSkyLightSources`): the cell
//!   above the first edge, scanning down from above the highest filled section, where the lower block has a non-zero
//!   `lightBlock` or the face shapes occlude.
//!
//! `tools/light_check.sh` compares the result with the light vanilla stored in the region files of a world it generated.

use crate::world::{Chunk, Dimension, Palette, World};
use mc_data::props::{flag, Aabb, BlockProps};
use mc_protocol::packets::play::LightData;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

/// Directions in `Direction.values()` order: DOWN, UP, NORTH, SOUTH, WEST, EAST.
const DIRS: [(i32, i32, i32); 6] = [(0, -1, 0), (0, 1, 0), (0, 0, -1), (0, 0, 1), (-1, 0, 0), (1, 0, 0)];
/// Axis of each direction (0 = x, 1 = y, 2 = z).
const AXIS: [usize; 6] = [1, 1, 2, 2, 0, 0];

/// Side of the window (3 chunks) in cells.
const W: usize = 48;

/// The light of one light section: sky and block light as nibble arrays (`DataLayer` layout: `y << 8 | z << 4 | x`).
pub struct Layer {
    pub sky: Box<[u8; 2048]>,
    pub block: Box<[u8; 2048]>,
}

/// Light of a chunk: one entry per light section (from one below the world), `None` when the section stores no light.
pub struct ChunkLight {
    pub layers: Vec<Option<Layer>>,
}

impl ChunkLight {
    /// The masks and arrays of the chunk packet: layers with data go to the sky / block masks, stored layers that are
    /// dark to the "empty" masks (`ClientboundLightUpdatePacketData`).
    pub fn to_light_data(&self) -> LightData {
        let words = self.layers.len().div_ceil(64);
        let mut l = LightData { sky_mask: vec![0; words], block_mask: vec![0; words], empty_sky_mask: vec![0; words], empty_block_mask: vec![0; words], ..Default::default() };
        for (i, layer) in self.layers.iter().enumerate() {
            let Some(layer) = layer else { continue };
            let (w, bit) = (i / 64, 1u64 << (i % 64));
            if layer.sky.iter().any(|&v| v != 0) {
                l.sky_mask[w] |= bit;
                l.sky.push(*layer.sky);
            } else {
                l.empty_sky_mask[w] |= bit;
            }
            if layer.block.iter().any(|&v| v != 0) {
                l.block_mask[w] |= bit;
                l.block.push(*layer.block);
            } else {
                l.empty_block_mask[w] |= bit;
            }
        }
        l
    }
}

pub struct LightEngine<'a> {
    props: &'a BlockProps,
    palette: &'a Palette,
    dim: Dimension,
    /// Cache of "do these two blocks occlude light between them in this direction".
    occlusion: RefCell<HashMap<(u16, u16, u8), bool>>,
}

impl<'a> LightEngine<'a> {
    pub fn new(props: &'a BlockProps, palette: &'a Palette, dim: Dimension) -> Self {
        Self { props, palette, dim, occlusion: RefCell::new(HashMap::new()) }
    }

    fn light_block(&self, state: u16) -> u8 {
        self.props.state(state as u32).map_or(15, |p| p.light_block)
    }

    /// `LightEngine.isEmptyShape`
    fn empty_shape(&self, state: u16) -> bool {
        self.props.state(state as u32).is_none_or(|p| !p.has(flag::CAN_OCCLUDE) || !p.has(flag::USE_SHAPE_FOR_LIGHT_OCCLUSION))
    }

    fn face_shape(&self, state: u16, dir: usize) -> &[Aabb] {
        if self.empty_shape(state) {
            &[]
        } else {
            self.props.face_occlusion_shape(state as u32, dir)
        }
    }

    /// `LightEngine.shapeOccludes`: do the facing sides of `from` (towards `dir`) and `to` together cover the whole face?
    fn occludes(&self, from: u16, to: u16, dir: usize) -> bool {
        if self.empty_shape(from) && self.empty_shape(to) {
            return false;
        }
        let key = (from, to, dir as u8);
        if let Some(&v) = self.occlusion.borrow().get(&key) {
            return v;
        }
        let v = faces_cover(self.face_shape(from, dir), self.face_shape(to, dir ^ 1), AXIS[dir]);
        self.occlusion.borrow_mut().insert(key, v);
        v
    }

    /// `ChunkSkyLightSources.isEdgeOccluded` for the edge between an upper and the block below it.
    fn edge_occluded(&self, upper: u16, lower: u16) -> bool {
        self.light_block(lower) != 0 || self.occludes(upper, lower, 0)
    }

    /// `ChunkSkyLightSources.findLowestSourceY` for one column: the y of the lowest cell lit directly by the sky
    /// (`i32::MIN` when the whole column is open).
    fn lowest_source_y(&self, chunk: &Chunk, x: usize, z: usize) -> i32 {
        let Some(top) = chunk.sections.iter().rposition(|s| !s.is_all_air(self.palette)) else { return i32::MIN };
        let air = self.palette.air;
        let mut upper_y = self.dim.min_y + (top as i32 + 1) * 16;
        let mut upper = air;
        for si in (0..=top).rev() {
            let sec = &chunk.sections[si];
            if sec.is_all_air(self.palette) {
                upper = air;
                upper_y = self.dim.min_y + si as i32 * 16;
                continue;
            }
            for ly in (0..16).rev() {
                let lower = sec.get(x, ly, z);
                if self.edge_occluded(upper, lower) {
                    return upper_y;
                }
                upper = lower;
                upper_y -= 1;
            }
        }
        i32::MIN
    }

    /// Computes the light of chunk (cx, cz). `get` returns loaded, fully generated chunks. The result is exact when the
    /// chunk and its eight neighbours are present (otherwise it is what vanilla computes for the chunks that are present).
    pub fn light_chunk<'c>(&self, cx: i32, cz: i32, get: &dyn Fn(i32, i32) -> Option<&'c Chunk>) -> ChunkLight {
        let n_sec = self.dim.sections();
        let ls_n = n_sec + 2; // light sections
        let h = ls_n * 16;
        let chunks: Vec<Option<&Chunk>> = (-2..=2).flat_map(|dx| (-2..=2).map(move |dz| (dx, dz))).map(|(dx, dz)| get(cx + dx, cz + dz)).collect();
        let at5 = |dx: i32, dz: i32| ((dx + 2) * 5 + (dz + 2)) as usize;
        let nonempty: Vec<Vec<bool>> =
            chunks.iter().map(|c| c.map_or_else(Vec::new, |c| c.sections.iter().map(|s| !s.is_all_air(self.palette)).collect())).collect();

        // which light sections of the 3x3 chunks store light
        let mut stored = vec![false; 9 * ls_n];
        for dx in -1..=1i32 {
            for dz in -1..=1i32 {
                if chunks[at5(dx, dz)].is_none() {
                    continue;
                }
                for ls in 0..ls_n {
                    'search: for ddx in -1..=1 {
                        for ddz in -1..=1 {
                            let ne = &nonempty[at5(dx + ddx, dz + ddz)];
                            if ne.is_empty() {
                                continue;
                            }
                            for dls in -1..=1i32 {
                                let si = ls as i32 + dls - 1;
                                if si >= 0 && (si as usize) < n_sec && ne[si as usize] {
                                    stored[((dx + 1) * 3 + dz + 1) as usize * ls_n + ls] = true;
                                    break 'search;
                                }
                            }
                        }
                    }
                }
            }
        }

        // dense window: blocks and "this cell stores light"
        let cells = W * W * h;
        let mut states = vec![self.palette.air; cells];
        let mut cell_stored = vec![false; cells];
        let idx = |x: usize, y: usize, z: usize| (y * W + z) * W + x;
        for dx in -1..=1i32 {
            for dz in -1..=1i32 {
                let Some(chunk) = chunks[at5(dx, dz)] else { continue };
                let (bx, bz) = (((dx + 1) * 16) as usize, ((dz + 1) * 16) as usize);
                for ls in 0..ls_n {
                    if !stored[((dx + 1) * 3 + dz + 1) as usize * ls_n + ls] {
                        continue;
                    }
                    for ly in 0..16 {
                        for lz in 0..16 {
                            for lx in 0..16 {
                                let i = idx(bx + lx, ls * 16 + ly, bz + lz);
                                cell_stored[i] = true;
                                if ls >= 1 && ls <= n_sec {
                                    states[i] = chunk.sections[ls - 1].get(lx, ly, lz);
                                }
                            }
                        }
                    }
                }
            }
        }

        // ---- block light
        // `BlockLightEngine.propagateLightSources` enqueues only the emitters of the chunk being lit. When the chunk is relit
        // (stored without light), light that would flow in from the emitters of lit neighbours is therefore missing, as in vanilla.
        let relit: Vec<bool> = (-1..=1).flat_map(|dx| (-1..=1).map(move |dz| (dx, dz))).map(|(dx, dz)| chunks[at5(dx, dz)].is_some_and(|c| c.relit)).collect();
        let target_relit = relit[4];
        let mut block = vec![0u8; cells];
        let mut buckets: [Vec<u32>; 16] = Default::default();
        for i in 0..cells {
            if cell_stored[i] && (!target_relit || relit[((i % W) / 16) * 3 + (i / W % W) / 16]) {
                let e = self.props.state(states[i] as u32).map_or(0, |p| p.light_emission);
                if e > 0 {
                    block[i] = e;
                    buckets[e as usize].push(i as u32);
                }
            }
        }
        self.flood(&mut block, &mut buckets, &states, &cell_stored, h);

        // ---- sky light
        let mut sky = vec![0u8; cells];
        let mut buckets: [Vec<u32>; 16] = Default::default();
        let base_y = self.dim.min_y - 16; // world y of window row 0
        for dx in -1..=1i32 {
            for dz in -1..=1i32 {
                let Some(chunk) = chunks[at5(dx, dz)] else { continue };
                let (bx, bz) = (((dx + 1) * 16) as usize, ((dz + 1) * 16) as usize);
                for lz in 0..16 {
                    for lx in 0..16 {
                        let lowest = self.lowest_source_y(chunk, lx, lz);
                        for ls in 0..ls_n {
                            if !stored[((dx + 1) * 3 + dz + 1) as usize * ls_n + ls] {
                                continue;
                            }
                            for ly in 0..16 {
                                let y = base_y + (ls * 16 + ly) as i32;
                                if y >= lowest {
                                    let i = idx(bx + lx, ls * 16 + ly, bz + lz);
                                    sky[i] = 15;
                                    buckets[15].push(i as u32);
                                }
                            }
                        }
                    }
                }
            }
        }
        self.flood(&mut sky, &mut buckets, &states, &cell_stored, h);

        // ---- the middle chunk
        let layers = (0..ls_n)
            .map(|ls| {
                if !stored[4 * ls_n + ls] {
                    return None;
                }
                let mut l = Layer { sky: Box::new([0; 2048]), block: Box::new([0; 2048]) };
                for ly in 0..16 {
                    for lz in 0..16 {
                        for lx in 0..16 {
                            let i = idx(16 + lx, ls * 16 + ly, 16 + lz);
                            let n = (ly << 8) | (lz << 4) | lx;
                            l.sky[n >> 1] |= sky[i] << ((n & 1) * 4);
                            l.block[n >> 1] |= block[i] << ((n & 1) * 4);
                        }
                    }
                }
                Some(l)
            })
            .collect();
        ChunkLight { layers }
    }

    /// Max-propagation of light levels (a bucket queue from level 15 down), see the module documentation.
    fn flood(&self, lv: &mut [u8], buckets: &mut [Vec<u32>; 16], states: &[u16], cell_stored: &[bool], h: usize) {
        let strides: [isize; 6] = [-((W * W) as isize), (W * W) as isize, -(W as isize), W as isize, -1, 1];
        for level in (2..=15usize).rev() {
            let cur = std::mem::take(&mut buckets[level]);
            for &c in &cur {
                let c = c as usize;
                if lv[c] as usize != level {
                    continue;
                }
                let (x, z, y) = (c % W, (c / W) % W, c / (W * W));
                for (d, &(dx, dy, dz)) in DIRS.iter().enumerate() {
                    let (nx, ny, nz) = (x as i32 + dx, y as i32 + dy, z as i32 + dz);
                    if nx < 0 || nz < 0 || ny < 0 || nx >= W as i32 || nz >= W as i32 || ny >= h as i32 {
                        continue;
                    }
                    let n = (c as isize + strides[d]) as usize;
                    if !cell_stored[n] || lv[n] as usize + 1 >= level {
                        continue;
                    }
                    let new = level as i32 - self.light_block(states[n]).max(1) as i32;
                    if new <= lv[n] as i32 || self.occludes(states[c], states[n], d) {
                        continue;
                    }
                    lv[n] = new as u8;
                    if new > 1 {
                        buckets[new as usize].push(n as u32);
                    }
                }
            }
        }
    }
}

/// `Shapes.faceShapeOccludes` on two face shapes (boxes that span the whole `axis`): both are empty -> false, otherwise
/// true when their union covers the entire unit square of the face.
fn faces_cover(a: &[Aabb], b: &[Aabb], axis: usize) -> bool {
    if a.is_empty() && b.is_empty() {
        return false;
    }
    const EPS: f64 = 1.0E-7;
    let (u, v) = match axis {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    };
    let breakpoints = |c: usize| -> Vec<f64> {
        let mut p = vec![0.0, 1.0];
        for bx in a.iter().chain(b) {
            p.push(bx[c].clamp(0.0, 1.0));
            p.push(bx[c + 3].clamp(0.0, 1.0));
        }
        p.sort_by(|x, y| x.partial_cmp(y).unwrap());
        p.dedup_by(|x, y| (*x - *y).abs() < EPS);
        p
    };
    let (pu, pv) = (breakpoints(u), breakpoints(v));
    for iu in 0..pu.len() - 1 {
        let cu = (pu[iu] + pu[iu + 1]) / 2.0;
        for iv in 0..pv.len() - 1 {
            let cv = (pv[iv] + pv[iv + 1]) / 2.0;
            let covered = a.iter().chain(b).any(|bx| bx[u] <= cu && cu <= bx[u + 3] && bx[v] <= cv && cv <= bx[v + 3]);
            if !covered {
                return false;
            }
        }
    }
    true
}

/// `mc-server --check-light`: recomputes the light of every stored chunk of the world whose eight neighbours are stored too
/// and compares it with the light vanilla stored in the region files. Returns whether everything matched.
pub fn check_world(world: &mut World, props: &BlockProps, radius: i32) -> bool {
    let mut coords = Vec::new();
    for cx in -radius..=radius {
        for cz in -radius..=radius {
            if world.ensure(cx, cz) {
                coords.push((cx, cz));
            }
        }
    }
    let present: HashSet<(i32, i32)> = coords.iter().copied().collect();
    let engine = LightEngine::new(props, &world.palette, world.dim);
    let n_sec = world.dim.sections();
    let min_section = world.dim.min_section();
    let (mut chunks_checked, mut chunks_equal, mut layers, mut bad_layers, mut late_layers) = (0, 0, 0u64, 0u64, 0u64);
    let mut shown = 0;
    let mut engine_time = std::time::Duration::ZERO;
    let get = |x: i32, z: i32| world.loaded_chunk(x, z);
    for &(cx, cz) in &coords {
        let complete = (-1..=1).all(|dx| (-1..=1).all(|dz| present.contains(&(cx + dx, cz + dz))));
        let chunk = world.loaded_chunk(cx, cz).unwrap();
        if !complete || !chunk.stored_light {
            continue;
        }
        chunks_checked += 1;
        let t = std::time::Instant::now();
        let light = engine.light_chunk(cx, cz, &get);
        engine_time += t.elapsed();
        let mut chunk_ok = true;
        for (ls, computed) in light.layers.iter().enumerate() {
            let (sky, block): (Option<&[u8; 2048]>, Option<&[u8; 2048]>) = if ls >= 1 && ls <= n_sec {
                let s = &chunk.sections[ls - 1];
                (s.sky_light.as_deref(), s.block_light.as_deref())
            } else {
                let y = min_section + ls as i32 - 1;
                match chunk.edge_light.iter().find(|e| e.0 == y) {
                    Some(e) => (e.1.as_deref(), e.2.as_deref()),
                    None => (None, None),
                }
            };
            for (name, stored, ours) in [("sky", sky, computed.as_ref().map(|l| &*l.sky)), ("block", block, computed.as_ref().map(|l| &*l.block))] {
                layers += 1;
                let zero = [0u8; 2048];
                let (a, b) = (stored.unwrap_or(&zero), ours.unwrap_or(&zero));
                // vanilla stores a snapshot of its layers when the chunk is saved: a layer that only exists because a neighbouring
                // chunk was generated later is missing from the file (the engine then holds a layer of 15s above all terrain)
                let late_layer = name == "sky" && stored.is_none() && b.iter().all(|&v| v == 0xFF);
                if a != b && late_layer {
                    late_layers += 1;
                } else if a != b {
                    bad_layers += 1;
                    chunk_ok = false;
                    if shown < 8 {
                        shown += 1;
                        let diff: Vec<usize> = (0..4096).filter(|&n| (a[n >> 1] >> ((n & 1) * 4)) & 15 != (b[n >> 1] >> ((n & 1) * 4)) & 15).collect();
                        let n = diff[0];
                        println!(
                            "  MISMATCH chunk ({cx},{cz}) light section {ls} {name}: {} cells differ, e.g. ({}, {}, {}): vanilla {} Rust {}{}",
                            diff.len(),
                            n & 15,
                            n >> 8,
                            (n >> 4) & 15,
                            (a[n >> 1] >> ((n & 1) * 4)) & 15,
                            (b[n >> 1] >> ((n & 1) * 4)) & 15,
                            if computed.is_none() { " (section not stored by the engine)" } else { "" },
                        );
                    }
                }
            }
        }
        if chunk_ok {
            chunks_equal += 1;
        }
    }
    println!(
        "light check: {chunks_checked} chunks with a complete neighbourhood, {chunks_equal} identical; {layers} layers compared, {bad_layers} differ \
         (+ {late_layers} all-15 sky layers vanilla had not stored yet); light engine: {:.1} ms per chunk",
        engine_time.as_secs_f64() * 1000.0 / chunks_checked.max(1) as f64
    );
    bad_layers == 0 && chunks_checked > 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{Biomes, Blocks as WBlocks};
    use mc_data::Blocks;
    use std::path::Path;

    struct Fixture {
        props: BlockProps,
        blocks: Blocks,
        palette: Palette,
        dim: Dimension,
    }

    /// Runs only when the reference data has been generated (tools/fetch_reference.sh).
    fn fixture() -> Option<Fixture> {
        let reports = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference/gen/generated/reports");
        if !reports.join("block_props.tsv").exists() {
            return None;
        }
        let blocks = Blocks::load(&reports.join("blocks.json")).unwrap();
        let props = BlockProps::load(&reports.join("block_props.tsv")).unwrap();
        let st = |n: &str| blocks.default_state(n).unwrap() as u16;
        let palette = Palette {
            air: st("minecraft:air"),
            cave_air: st("minecraft:cave_air"),
            void_air: st("minecraft:void_air"),
            block_state_count: blocks.state_count,
            biome_count: 64,
            fluid: Default::default(),
        };
        Some(Fixture { props, blocks, palette, dim: Dimension { min_y: -64, height: 384 } })
    }

    impl Fixture {
        fn st(&self, name: &str) -> u16 {
            self.blocks.default_state(name).unwrap() as u16
        }

        /// A superflat chunk: bedrock, two dirt, grass (y = -64 .. -61).
        fn flat_chunk(&self) -> Chunk {
            let mut c = Chunk::empty(&self.dim, &self.palette, 0);
            for (i, n) in ["minecraft:bedrock", "minecraft:dirt", "minecraft:dirt", "minecraft:grass_block"].iter().enumerate() {
                for z in 0..16 {
                    for x in 0..16 {
                        c.sections[0].set(x, i, z, self.st(n));
                    }
                }
            }
            c
        }

        fn set(&self, c: &mut Chunk, x: usize, y: i32, z: usize, name: &str) {
            let sy = (y - self.dim.min_y) as usize;
            c.sections[sy >> 4].set(x, sy & 15, z, self.st(name));
        }

        /// Light of the middle chunk of a 3x3 window of `flat` neighbours around `center`.
        fn light(&self, center: &Chunk, flat: &Chunk) -> ChunkLight {
            let engine = LightEngine::new(&self.props, &self.palette, self.dim);
            engine.light_chunk(0, 0, &|x, z| Some(if (x, z) == (0, 0) { center } else { flat }))
        }
    }

    fn level(l: &ChunkLight, sky: bool, y: i32, x: usize, z: usize) -> u8 {
        let ls = (((y + 64) >> 4) + 1) as usize;
        let layer = l.layers[ls].as_ref().expect("section stores light");
        let n = (((y + 64) as usize & 15) << 8) | (z << 4) | x;
        let arr = if sky { &layer.sky } else { &layer.block };
        (arr[n >> 1] >> ((n & 1) * 4)) & 15
    }

    #[test]
    fn face_coverage() {
        let full = [0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        let lower_half = [0.0, 0.0, 0.0, 1.0, 0.5, 1.0];
        let upper_half = [0.0, 0.5, 0.0, 1.0, 1.0, 1.0];
        // looking along z (axis 2): the face spans x and y
        assert!(faces_cover(&[full], &[], 2));
        assert!(!faces_cover(&[], &[], 2));
        assert!(!faces_cover(&[lower_half], &[], 2));
        assert!(faces_cover(&[lower_half], &[upper_half], 2));
        assert!(!faces_cover(&[lower_half], &[lower_half], 2));
        // a post in the middle never covers the face
        assert!(!faces_cover(&[[0.375, 0.0, 0.0, 0.625, 1.0, 1.0]], &[], 2));
    }

    #[test]
    fn sky_light_of_a_superflat_chunk() {
        let Some(f) = fixture() else { return };
        let flat = f.flat_chunk();
        let l = f.light(&flat, &flat);
        // sections 0..=2 store light (the first has blocks, its neighbours are vertical neighbours of a non-empty section)
        assert!(l.layers[0].is_some() && l.layers[1].is_some() && l.layers[2].is_some() && l.layers[3].is_none());
        assert_eq!(level(&l, true, -61, 5, 5), 0); // inside the grass block
        assert_eq!(level(&l, true, -60, 5, 5), 15); // the open sky starts above it
        assert_eq!(level(&l, true, -40, 5, 5), 15); // still inside the stored sections 0..=2
        assert_eq!(level(&l, false, -60, 5, 5), 0);
    }

    #[test]
    fn torch_light_falls_off_by_one_per_block_and_stops_at_solid_blocks() {
        let Some(f) = fixture() else { return };
        let flat = f.flat_chunk();
        let mut c = f.flat_chunk();
        f.set(&mut c, 8, -60, 8, "minecraft:torch");
        let l = f.light(&c, &flat);
        assert_eq!(level(&l, false, -60, 8, 8), 14);
        assert_eq!(level(&l, false, -60, 9, 8), 13);
        assert_eq!(level(&l, false, -59, 8, 8), 13);
        assert_eq!(level(&l, false, -58, 9, 9), 14 - 4);
        assert_eq!(level(&l, false, -60, 15, 8), 14 - 7); // seven blocks away
        assert_eq!(level(&l, false, -60, 0, 8), 14 - 8);
        assert_eq!(level(&l, false, -57, 12, 10), 14 - (4 + 3 + 2)); // Manhattan distance 9
        // no light inside the ground
        assert_eq!(level(&l, false, -61, 8, 8), 0);
        // a wall of stone stops the light
        let mut walled = c.clone();
        for y in -60..-50 {
            for z in 0..16 {
                f.set(&mut walled, 10, y, z, "minecraft:stone");
            }
        }
        let l = f.light(&walled, &flat);
        assert_eq!(level(&l, false, -60, 9, 8), 13);
        assert_eq!(level(&l, false, -60, 11, 8), 0);
        // ... but light still gets around the top of the wall (y = -50): 2 blocks over, 10 up = distance 12 -> 2, one more -> 1
        assert_eq!(level(&l, false, -50, 10, 8), 14 - 12);
        assert_eq!(level(&l, false, -50, 11, 8), 14 - 13);
    }

    #[test]
    fn sky_light_spreads_one_level_per_block_under_an_overhang() {
        let Some(f) = fixture() else { return };
        let flat = f.flat_chunk();
        let mut c = f.flat_chunk();
        for z in 6..=10 {
            for x in 6..=10 {
                f.set(&mut c, x, -58, z, "minecraft:stone");
            }
        }
        let l = f.light(&c, &flat);
        assert_eq!(level(&l, true, -58, 8, 8), 0); // the stone itself
        assert_eq!(level(&l, true, -59, 5, 8), 15); // open column next to the platform
        assert_eq!(level(&l, true, -59, 6, 8), 14);
        assert_eq!(level(&l, true, -59, 8, 8), 12); // three blocks in from every side
        assert_eq!(level(&l, true, -57, 8, 8), 15); // on top of the platform
        // a glass roof lets everything through, leaves and water absorb one level per block
        let mut glass = f.flat_chunk();
        let mut leaves = f.flat_chunk();
        for z in 6..=10 {
            for x in 6..=10 {
                f.set(&mut glass, x, -58, z, "minecraft:glass");
                f.set(&mut leaves, x, -58, z, "minecraft:oak_leaves");
            }
        }
        // glass has no light opacity: the sky shines straight through
        assert_eq!(level(&f.light(&glass, &flat), true, -59, 8, 8), 15);
        // leaves absorb one level per block: every leaf cell gets 14 from the open sky above it, the cell below gets 13 from the
        // leaf above, which beats the 12 that arrives from the open columns at the sides (3 blocks away)
        let l = f.light(&leaves, &flat);
        assert_eq!(level(&l, true, -58, 6, 8), 14);
        assert_eq!(level(&l, true, -58, 8, 8), 14);
        assert_eq!(level(&l, true, -59, 8, 8), 13);
    }

    #[test]
    fn engine_matches_the_old_flat_world_light() {
        // the superflat chunk light the server used before the engine (verified against the vanilla server): sky 15 from the
        // first air block up in sections 0..=2, nothing else
        let Some(f) = fixture() else { return };
        let flat = f.flat_chunk();
        let data = f.light(&flat, &flat).to_light_data();
        assert_eq!(data.sky_mask, vec![0b110]); // sections 1 and 2 have sky data, section 0 (below the world) is dark
        assert_eq!(data.empty_sky_mask, vec![0b001]);
        assert_eq!(data.block_mask, vec![0]);
        assert_eq!(data.empty_block_mask, vec![0b111]);
        assert_eq!(data.sky.len(), 2);
        assert!(data.sky[1].iter().all(|&b| b == 0xFF));
        let _ = (WBlocks::Uniform(0), Biomes::Uniform(0));
    }
}
