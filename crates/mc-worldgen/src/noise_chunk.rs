//! `NoiseChunk`: evaluates the wired noise router for one chunk with trilinear interpolation over the noise cells, the per-cell
//! cache, the aquifer and the ore veins, and produces the terrain blocks like `NoiseBasedChunkGenerator.doFill`.

use crate::density::{clamp, clamped_map, create_ap2, Ctx, Df, FlatTable, Mapper, MarkerKind, Node, Op, Visitor};
use crate::mth;
use crate::random::PositionalRandomFactory;
use crate::random_state::{GeneratorSettings, NoiseRouter, NoiseSettings, RandomState, TerrainBlocks};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::Arc;

/// `NoiseChunk.NoiseInterpolator` state
#[derive(Default)]
pub struct InterpState {
    slice0: Vec<Vec<f64>>,
    slice1: Vec<Vec<f64>>,
    n000: f64,
    n001: f64,
    n100: f64,
    n101: f64,
    n010: f64,
    n011: f64,
    n110: f64,
    n111: f64,
    v_xz00: f64,
    v_xz10: f64,
    v_xz01: f64,
    v_xz11: f64,
    v_z0: f64,
    v_z1: f64,
    value: f64,
}

impl InterpState {
    fn new(cell_count_y: usize, cell_count_xz: usize) -> Self {
        let slice = vec![vec![0.0; cell_count_y + 1]; cell_count_xz + 1];
        Self { slice0: slice.clone(), slice1: slice, ..Default::default() }
    }
}

/// The mutable state of the chunk that the wrapped nodes read (`NoiseChunk` fields).
pub struct ChunkState {
    pub cell_width: i32,
    pub cell_height: i32,
    cell_count_xz: i32,
    cell_count_y: i32,
    cell_noise_min_y: i32,
    cell_start_x: Cell<i32>,
    cell_start_y: Cell<i32>,
    cell_start_z: Cell<i32>,
    in_cell_x: Cell<i32>,
    in_cell_y: Cell<i32>,
    in_cell_z: Cell<i32>,
    interpolating: Cell<bool>,
    filling_cell: Cell<bool>,
    interpolators: RefCell<Vec<InterpState>>,
    cell_caches: RefCell<Vec<Vec<f64>>>,
}

impl ChunkState {
    /// The chunk as a function context at its current position.
    pub fn ctx(&self) -> Ctx<'_> {
        Ctx { x: self.cell_start_x.get() + self.in_cell_x.get(), y: self.cell_start_y.get() + self.in_cell_y.get(), z: self.cell_start_z.get() + self.in_cell_z.get(), chunk: Some(self) }
    }

    /// `NoiseInterpolator.compute` for the chunk context
    pub fn interpolated_value(&self, id: usize) -> f64 {
        assert!(self.interpolating.get(), "Trying to sample interpolator outside the interpolation loop");
        let it = self.interpolators.borrow();
        let s = &it[id];
        if self.filling_cell.get() {
            mth::lerp3(
                self.in_cell_x.get() as f64 / self.cell_width as f64,
                self.in_cell_y.get() as f64 / self.cell_height as f64,
                self.in_cell_z.get() as f64 / self.cell_width as f64,
                s.n000,
                s.n100,
                s.n010,
                s.n110,
                s.n001,
                s.n101,
                s.n011,
                s.n111,
            )
        } else {
            s.value
        }
    }

    /// `CacheAllInCell.compute` for the chunk context: the cached value at the current in-cell position, if it is inside the cell
    pub fn cell_cache_lookup(&self, id: usize) -> Option<f64> {
        assert!(self.interpolating.get(), "Trying to sample interpolator outside the interpolation loop");
        let (x, y, z) = (self.in_cell_x.get(), self.in_cell_y.get(), self.in_cell_z.get());
        if x >= 0 && y >= 0 && z >= 0 && x < self.cell_width && y < self.cell_height && z < self.cell_width {
            Some(self.cell_caches.borrow()[id][(((self.cell_height - 1 - y) * self.cell_width + x) * self.cell_width + z) as usize])
        } else {
            None
        }
    }

    /// `NoiseChunk.forIndex`: sets the in-cell position for index `i` of a cell array
    fn set_in_cell_from_index(&self, i: i32) {
        let z = i.rem_euclid(self.cell_width);
        let t = i.div_euclid(self.cell_width);
        self.in_cell_z.set(z);
        self.in_cell_x.set(t.rem_euclid(self.cell_width));
        self.in_cell_y.set(self.cell_height - 1 - t.div_euclid(self.cell_width));
    }
}

/// Collects the cache objects `NoiseChunk.wrap` creates.
struct Builder {
    cell_width: i32,
    cell_height: i32,
    cell_count_xz: i32,
    cell_count_y: i32,
    first_noise_x: i32,
    first_noise_z: i32,
    noise_size_xz: i32,
    interp: Vec<InterpState>,
    interp_fillers: Vec<Df>,
    cell_values: Vec<Vec<f64>>,
    cell_fillers: Vec<Df>,
}

impl Visitor for Builder {
    /// `NoiseChunk.wrap` / `wrapNew` (with the empty blender)
    fn apply(&mut self, n: Df) -> Df {
        match &*n {
            Node::Marker { kind, wrapped } => match kind {
                MarkerKind::Interpolated => {
                    let id = self.interp.len();
                    self.interp.push(InterpState::new(self.cell_count_y as usize, self.cell_count_xz as usize));
                    self.interp_fillers.push(wrapped.clone());
                    Arc::new(Node::Interpolated { id, wrapped: wrapped.clone() })
                }
                MarkerKind::FlatCache => {
                    let size = (self.noise_size_xz + 1) as usize;
                    let mut values = vec![0.0; size * size];
                    for i in 0..size {
                        let x = (self.first_noise_x + i as i32) << 2;
                        for j in 0..size {
                            let z = (self.first_noise_z + j as i32) << 2;
                            values[i * size + j] = wrapped.compute(&Ctx::point(x, 0, z));
                        }
                    }
                    Arc::new(Node::FlatCache { table: FlatTable { values, size, first_noise_x: self.first_noise_x, first_noise_z: self.first_noise_z }, wrapped: wrapped.clone() })
                }
                MarkerKind::Cache2D => Arc::new(Node::Cache2D { wrapped: wrapped.clone() }),
                MarkerKind::CacheOnce => Arc::new(Node::CacheOnce { wrapped: wrapped.clone() }),
                MarkerKind::CacheAllInCell => {
                    let id = self.cell_values.len();
                    self.cell_values.push(vec![0.0; (self.cell_width * self.cell_width * self.cell_height) as usize]);
                    self.cell_fillers.push(wrapped.clone());
                    Arc::new(Node::CacheAllInCell { id, wrapped: wrapped.clone() })
                }
            },
            Node::HolderRef(f) => f.clone(),
            _ => n,
        }
    }
}

/// `Aquifer.FluidStatus`
#[derive(Clone, Copy)]
struct FluidStatus {
    level: i32,
    fluid: u16,
}

/// `BlockPos.asLong` and friends
fn pos_as_long(x: i32, y: i32, z: i32) -> i64 {
    ((x as i64 & 0x3FF_FFFF) << 38) | (y as i64 & 0xFFF) | ((z as i64 & 0x3FF_FFFF) << 12)
}
fn pos_x(l: i64) -> i32 {
    (l >> 38) as i32
}
fn pos_y(l: i64) -> i32 {
    ((l << 52) >> 52) as i32
}
fn pos_z(l: i64) -> i32 {
    ((l << 26) >> 38) as i32
}

/// `DimensionType.WAY_BELOW_MIN_Y`
const WAY_BELOW_MIN_Y: i32 = -32512;

struct NoiseAquifer {
    barrier: Df,
    floodedness: Df,
    spread: Df,
    lava_noise: Df,
    erosion: Df,
    depth: Df,
    random: PositionalRandomFactory,
    cache: RefCell<Vec<Option<FluidStatus>>>,
    location_cache: RefCell<Vec<i64>>,
    min_grid_x: i32,
    min_grid_y: i32,
    min_grid_z: i32,
    grid_size_x: i32,
    grid_size_z: i32,
    should_schedule_fluid_update: Cell<bool>,
}

const SURFACE_SAMPLING_OFFSETS_IN_CHUNKS: [[i32; 2]; 13] = [[0, 0], [-2, -1], [-1, -1], [0, -1], [1, -1], [-3, 0], [-2, 0], [-1, 0], [1, 0], [-2, 1], [-1, 1], [0, 1], [1, 1]];

fn similarity(a: i32, b: i32) -> f64 {
    1.0 - (b.wrapping_sub(a)).abs() as f64 / 25.0
}

enum Aquifer {
    Disabled,
    Noise(Box<NoiseAquifer>),
}

/// The global fluid picker of `NoiseBasedChunkGenerator.createFluidPicker`
struct GlobalFluid {
    sea_level: i32,
    default_fluid: u16,
    lava: u16,
}

impl GlobalFluid {
    fn compute(&self, y: i32) -> FluidStatus {
        if y < (-54).min(self.sea_level) {
            FluidStatus { level: -54, fluid: self.lava }
        } else {
            FluidStatus { level: self.sea_level, fluid: self.default_fluid }
        }
    }
}

fn status_at(s: FluidStatus, y: i32, air: u16) -> u16 {
    if y < s.level { s.fluid } else { air }
}

/// `OreVeinifier`
struct OreVeinifier {
    toggle: Df,
    ridged: Df,
    gap: Df,
    random: PositionalRandomFactory,
}

struct VeinType {
    ore: u16,
    raw_ore_block: u16,
    filler: u16,
    min_y: i32,
    max_y: i32,
}

pub struct NoiseChunk {
    pub st: ChunkState,
    settings: NoiseSettings,
    blocks: TerrainBlocks,
    global_fluid: GlobalFluid,
    interp_fillers: Vec<Df>,
    cell_fillers: Vec<Df>,
    initial_density: Df,
    block_density: Df,
    aquifer: Aquifer,
    ore: Option<OreVeinifier>,
    prelim: RefCell<HashMap<i64, i32>>,
    first_cell_x: i32,
    first_cell_z: i32,
}

impl NoiseChunk {
    /// `NoiseChunk` constructor: `noise` is the settings clamped to the chunk's height range.
    pub fn new(state: &RandomState, settings: &GeneratorSettings, noise: NoiseSettings, blocks: TerrainBlocks, min_block_x: i32, min_block_z: i32) -> Self {
        let cell_width = noise.cell_width();
        let cell_height = noise.cell_height();
        let cell_count_xz = 16 / cell_width;
        let cell_count_y = noise.height.div_euclid(cell_height);
        let cell_noise_min_y = noise.min_y.div_euclid(cell_height);
        let first_cell_x = min_block_x.div_euclid(cell_width);
        let first_cell_z = min_block_z.div_euclid(cell_width);
        let first_noise_x = min_block_x >> 2;
        let first_noise_z = min_block_z >> 2;
        let noise_size_xz = (cell_count_xz * cell_width) >> 2;
        let mut builder = Builder {
            cell_width,
            cell_height,
            cell_count_xz,
            cell_count_y,
            first_noise_x,
            first_noise_z,
            noise_size_xz,
            interp: Vec::new(),
            interp_fillers: Vec::new(),
            cell_values: Vec::new(),
            cell_fillers: Vec::new(),
        };
        let router: NoiseRouter = state.router.map_all(&mut builder);
        let global_fluid = GlobalFluid { sea_level: settings.sea_level, default_fluid: blocks.default_fluid, lava: blocks.lava };

        let aquifer = if !settings.aquifers_enabled {
            Aquifer::Disabled
        } else {
            let chunk_x = min_block_x.div_euclid(16);
            let chunk_z = min_block_z.div_euclid(16);
            let grid = |v: i32, d: i32| v.div_euclid(d);
            let min_grid_x = grid(chunk_x * 16, 16) - 1;
            let max_grid_x = grid(chunk_x * 16 + 15, 16) + 1;
            let min_grid_y = grid(noise.min_y, 12) - 1;
            let max_grid_y = grid(noise.min_y + noise.height, 12) + 1;
            let min_grid_z = grid(chunk_z * 16, 16) - 1;
            let max_grid_z = grid(chunk_z * 16 + 15, 16) + 1;
            let grid_size_x = max_grid_x - min_grid_x + 1;
            let grid_size_y = max_grid_y - min_grid_y + 1;
            let grid_size_z = max_grid_z - min_grid_z + 1;
            let count = (grid_size_x * grid_size_y * grid_size_z) as usize;
            Aquifer::Noise(Box::new(NoiseAquifer {
                barrier: router.barrier.clone(),
                floodedness: router.fluid_level_floodedness.clone(),
                spread: router.fluid_level_spread.clone(),
                lava_noise: router.lava.clone(),
                erosion: router.erosion.clone(),
                depth: router.depth.clone(),
                random: state.aquifer_random.clone(),
                cache: RefCell::new(vec![None; count]),
                location_cache: RefCell::new(vec![i64::MAX; count]),
                min_grid_x,
                min_grid_y,
                min_grid_z,
                grid_size_x,
                grid_size_z,
                should_schedule_fluid_update: Cell::new(false),
            }))
        };

        // DensityFunctions.cacheAllInCell(add(finalDensity, beardifier)).mapAll(wrap)
        let marker = Arc::new(Node::Marker {
            kind: MarkerKind::CacheAllInCell,
            wrapped: create_ap2(Op::Add, router.final_density.clone(), Arc::new(Node::Beardifier)),
        });
        let block_density = Mapper::new(&mut builder).map_all(&marker);

        let ore = if settings.ore_veins_enabled {
            Some(OreVeinifier {
                toggle: router.vein_toggle.clone(),
                ridged: router.vein_ridged.clone(),
                gap: router.vein_gap.clone(),
                random: state.ore_random.clone(),
            })
        } else {
            None
        };

        let st = ChunkState {
            cell_width,
            cell_height,
            cell_count_xz,
            cell_count_y,
            cell_noise_min_y,
            cell_start_x: Cell::new(0),
            cell_start_y: Cell::new(0),
            cell_start_z: Cell::new(0),
            in_cell_x: Cell::new(0),
            in_cell_y: Cell::new(0),
            in_cell_z: Cell::new(0),
            interpolating: Cell::new(false),
            filling_cell: Cell::new(false),
            interpolators: RefCell::new(std::mem::take(&mut builder.interp)),
            cell_caches: RefCell::new(std::mem::take(&mut builder.cell_values)),
        };
        Self {
            st,
            settings: noise,
            blocks,
            global_fluid,
            interp_fillers: std::mem::take(&mut builder.interp_fillers),
            cell_fillers: std::mem::take(&mut builder.cell_fillers),
            initial_density: router.initial_density_without_jaggedness.clone(),
            block_density,
            aquifer,
            ore,
            prelim: RefCell::new(HashMap::new()),
            first_cell_x,
            first_cell_z,
        }
    }

    // ---------------------------------------------------------------- interpolation loop (NoiseChunk public API)

    fn fill_slice(&self, first: bool, cell_x: i32) {
        let st = &self.st;
        st.cell_start_x.set(cell_x * st.cell_width);
        st.in_cell_x.set(0);
        for z in 0..=st.cell_count_xz {
            st.cell_start_z.set((self.first_cell_z + z) * st.cell_width);
            st.in_cell_z.set(0);
            for (i, filler) in self.interp_fillers.iter().enumerate() {
                for y in 0..=st.cell_count_y {
                    st.cell_start_y.set((y + st.cell_noise_min_y) * st.cell_height);
                    st.in_cell_y.set(0);
                    let v = filler.compute(&st.ctx());
                    let mut it = st.interpolators.borrow_mut();
                    let slice = if first { &mut it[i].slice0 } else { &mut it[i].slice1 };
                    slice[z as usize][y as usize] = v;
                }
            }
        }
    }

    pub fn initialize_for_first_cell_x(&self) {
        assert!(!self.st.interpolating.get(), "Staring interpolation twice");
        self.st.interpolating.set(true);
        self.fill_slice(true, self.first_cell_x);
    }

    pub fn advance_cell_x(&self, cell_x: i32) {
        self.fill_slice(false, self.first_cell_x + cell_x + 1);
        self.st.cell_start_x.set((self.first_cell_x + cell_x) * self.st.cell_width);
    }

    pub fn select_cell_yz(&self, cell_y: i32, cell_z: i32) {
        let st = &self.st;
        for it in st.interpolators.borrow_mut().iter_mut() {
            let (y, z) = (cell_y as usize, cell_z as usize);
            it.n000 = it.slice0[z][y];
            it.n001 = it.slice0[z + 1][y];
            it.n100 = it.slice1[z][y];
            it.n101 = it.slice1[z + 1][y];
            it.n010 = it.slice0[z][y + 1];
            it.n011 = it.slice0[z + 1][y + 1];
            it.n110 = it.slice1[z][y + 1];
            it.n111 = it.slice1[z + 1][y + 1];
        }
        st.filling_cell.set(true);
        st.cell_start_y.set((cell_y + st.cell_noise_min_y) * st.cell_height);
        st.cell_start_z.set((self.first_cell_z + cell_z) * st.cell_width);
        for (id, filler) in self.cell_fillers.iter().enumerate() {
            let n = (st.cell_width * st.cell_width * st.cell_height) as usize;
            for i in 0..n {
                st.set_in_cell_from_index(i as i32);
                let v = filler.compute(&st.ctx());
                st.cell_caches.borrow_mut()[id][i] = v;
            }
        }
        st.filling_cell.set(false);
    }

    pub fn update_for_y(&self, y: i32, t: f64) {
        self.st.in_cell_y.set(y - self.st.cell_start_y.get());
        for it in self.st.interpolators.borrow_mut().iter_mut() {
            it.v_xz00 = mth::lerp(t, it.n000, it.n010);
            it.v_xz10 = mth::lerp(t, it.n100, it.n110);
            it.v_xz01 = mth::lerp(t, it.n001, it.n011);
            it.v_xz11 = mth::lerp(t, it.n101, it.n111);
        }
    }

    pub fn update_for_x(&self, x: i32, t: f64) {
        self.st.in_cell_x.set(x - self.st.cell_start_x.get());
        for it in self.st.interpolators.borrow_mut().iter_mut() {
            it.v_z0 = mth::lerp(t, it.v_xz00, it.v_xz10);
            it.v_z1 = mth::lerp(t, it.v_xz01, it.v_xz11);
        }
    }

    pub fn update_for_z(&self, z: i32, t: f64) {
        self.st.in_cell_z.set(z - self.st.cell_start_z.get());
        for it in self.st.interpolators.borrow_mut().iter_mut() {
            it.value = mth::lerp(t, it.v_z0, it.v_z1);
        }
    }

    pub fn stop_interpolation(&self) {
        assert!(self.st.interpolating.get(), "Staring interpolation twice");
        self.st.interpolating.set(false);
    }

    pub fn swap_slices(&self) {
        for it in self.st.interpolators.borrow_mut().iter_mut() {
            std::mem::swap(&mut it.slice0, &mut it.slice1);
        }
    }

    // ---------------------------------------------------------------- block state rule

    /// `NoiseChunk.getInterpolatedState`: the aquifer rule, then the ore veins; `None` means the default block.
    pub fn interpolated_state(&self) -> Option<u16> {
        let ctx = self.st.ctx();
        let density = self.block_density.compute(&ctx);
        if let Some(s) = self.aquifer_substance(&ctx, density) {
            return Some(s);
        }
        self.ore.as_ref().and_then(|o| self.vein(o, &ctx))
    }

    /// Whether the last fluid decision asks for a scheduled fluid update (`Aquifer.shouldScheduleFluidUpdate`)
    pub fn should_schedule_fluid_update(&self) -> bool {
        match &self.aquifer {
            Aquifer::Disabled => false,
            Aquifer::Noise(a) => a.should_schedule_fluid_update.get(),
        }
    }

    fn vein(&self, o: &OreVeinifier, ctx: &Ctx<'_>) -> Option<u16> {
        let b = &self.blocks;
        let toggle = o.toggle.compute(ctx);
        let y = ctx.y;
        let kind = if toggle > 0.0 {
            VeinType { ore: b.copper_ore, raw_ore_block: b.raw_copper_block, filler: b.granite, min_y: 0, max_y: 50 }
        } else {
            VeinType { ore: b.deepslate_iron_ore, raw_ore_block: b.raw_iron_block, filler: b.tuff, min_y: -60, max_y: -8 }
        };
        let abs = toggle.abs();
        let to_top = kind.max_y - y;
        let from_bottom = y - kind.min_y;
        if from_bottom >= 0 && to_top >= 0 {
            let edge = to_top.min(from_bottom);
            let roundoff = clamped_map(edge as f64, 0.0, 20.0, -0.2, 0.0);
            if abs + roundoff < 0.4f32 as f64 {
                return None;
            }
            let mut r = o.random.at(ctx.x, y, ctx.z);
            if r.next_float() > 0.7f32 {
                return None;
            }
            if o.ridged.compute(ctx) >= 0.0 {
                return None;
            }
            let richness = clamped_map(abs, 0.4f32 as f64, 0.6f32 as f64, 0.1f32 as f64, 0.3f32 as f64);
            if (r.next_float() as f64) < richness && o.gap.compute(ctx) > (-0.3f32) as f64 {
                return Some(if r.next_float() < 0.02f32 { kind.raw_ore_block } else { kind.ore });
            }
            return Some(kind.filler);
        }
        None
    }

    // ---------------------------------------------------------------- aquifer

    fn aquifer_substance(&self, ctx: &Ctx<'_>, density: f64) -> Option<u16> {
        let air = self.blocks.air;
        match &self.aquifer {
            Aquifer::Disabled => {
                if density > 0.0 {
                    None
                } else {
                    Some(status_at(self.global_fluid.compute(ctx.y), ctx.y, air))
                }
            }
            Aquifer::Noise(a) => self.noise_aquifer_substance(a, ctx, density),
        }
    }

    fn noise_aquifer_substance(&self, a: &NoiseAquifer, ctx: &Ctx<'_>, density: f64) -> Option<u16> {
        let (x, y, z) = (ctx.x, ctx.y, ctx.z);
        let air = self.blocks.air;
        let lava = self.blocks.lava;
        if density > 0.0 {
            a.should_schedule_fluid_update.set(false);
            return None;
        }
        let global = self.global_fluid.compute(y);
        if status_at(global, y, air) == lava {
            a.should_schedule_fluid_update.set(false);
            return Some(lava);
        }
        let gx = (x - 5).div_euclid(16);
        let gy = (y + 1).div_euclid(12);
        let gz = (z - 5).div_euclid(16);
        let (mut d0, mut d1, mut d2) = (i32::MAX, i32::MAX, i32::MAX);
        let (mut l0, mut l1, mut l2) = (0i64, 0i64, 0i64);
        for dx in 0..=1 {
            for dy in -1..=1 {
                for dz in 0..=1 {
                    let (cx, cy, cz) = (gx + dx, gy + dy, gz + dz);
                    let index = a.index(cx, cy, cz);
                    let cached = a.location_cache.borrow()[index];
                    let loc = if cached != i64::MAX {
                        cached
                    } else {
                        let mut r = a.random.at(cx, cy, cz);
                        let px = cx * 16 + r.next_int_bound(10);
                        let py = cy * 12 + r.next_int_bound(9);
                        let pz = cz * 16 + r.next_int_bound(10);
                        let l = pos_as_long(px, py, pz);
                        a.location_cache.borrow_mut()[index] = l;
                        l
                    };
                    let (ox, oy, oz) = (pos_x(loc) - x, pos_y(loc) - y, pos_z(loc) - z);
                    let dist = ox * ox + oy * oy + oz * oz;
                    if d0 >= dist {
                        l2 = l1;
                        l1 = l0;
                        l0 = loc;
                        d2 = d1;
                        d1 = d0;
                        d0 = dist;
                    } else if d1 >= dist {
                        l2 = l1;
                        l1 = loc;
                        d2 = d1;
                        d1 = dist;
                    } else if d2 >= dist {
                        l2 = loc;
                        d2 = dist;
                    }
                }
            }
        }
        let s0 = self.aquifer_status(a, l0);
        let sim01 = similarity(d0, d1);
        let block0 = status_at(s0, y, air);
        if sim01 <= 0.0 {
            // FLOWING_UPDATE_SIMULARITY = similarity(10^2, 12^2)
            a.should_schedule_fluid_update.set(sim01 >= similarity(100, 144));
            return Some(block0);
        }
        if self.blocks_is_water(block0) && status_at(self.global_fluid.compute(y - 1), y - 1, air) == lava {
            a.should_schedule_fluid_update.set(true);
            return Some(block0);
        }
        let mut barrier = f64::NAN;
        let s1 = self.aquifer_status(a, l1);
        let p01 = sim01 * self.pressure(a, ctx, &mut barrier, s0, s1);
        if density + p01 > 0.0 {
            a.should_schedule_fluid_update.set(false);
            return None;
        }
        let s2 = self.aquifer_status(a, l2);
        let sim02 = similarity(d0, d2);
        if sim02 > 0.0 {
            let p02 = sim01 * sim02 * self.pressure(a, ctx, &mut barrier, s0, s2);
            if density + p02 > 0.0 {
                a.should_schedule_fluid_update.set(false);
                return None;
            }
        }
        let sim12 = similarity(d1, d2);
        if sim12 > 0.0 {
            let p12 = sim01 * sim12 * self.pressure(a, ctx, &mut barrier, s1, s2);
            if density + p12 > 0.0 {
                a.should_schedule_fluid_update.set(false);
                return None;
            }
        }
        a.should_schedule_fluid_update.set(true);
        Some(block0)
    }

    /// `BlockState.is(Blocks.WATER)`: the only fluids around are lava, the default fluid and air
    fn blocks_is_water(&self, b: u16) -> bool {
        b != self.blocks.lava && b != self.blocks.air
    }

    fn pressure(&self, a: &NoiseAquifer, ctx: &Ctx<'_>, barrier: &mut f64, s1: FluidStatus, s2: FluidStatus) -> f64 {
        let y = ctx.y;
        let air = self.blocks.air;
        let lava = self.blocks.lava;
        let b1 = status_at(s1, y, air);
        let b2 = status_at(s2, y, air);
        let b1_lava = b1 == lava;
        let b2_lava = b2 == lava;
        let b1_water = self.blocks_is_water(b1);
        let b2_water = self.blocks_is_water(b2);
        if (b1_lava && b2_water) || (b1_water && b2_lava) {
            return 2.0;
        }
        let diff = (s1.level - s2.level).abs();
        if diff == 0 {
            return 0.0;
        }
        let mid = 0.5 * (s1.level + s2.level) as f64;
        let above = y as f64 + 0.5 - mid;
        let half = diff as f64 / 2.0;
        let margin = half - above.abs();
        let gradient = if above > 0.0 {
            let m = 0.0 + margin;
            if m > 0.0 { m / 1.5 } else { m / 2.5 }
        } else {
            let m = 3.0 + margin;
            if m > 0.0 { m / 3.0 } else { m / 10.0 }
        };
        let noise = if !(gradient < -2.0) && !(gradient > 2.0) {
            if barrier.is_nan() {
                *barrier = a.barrier.compute(ctx);
            }
            *barrier
        } else {
            0.0
        };
        2.0 * (noise + gradient)
    }

    fn aquifer_status(&self, a: &NoiseAquifer, loc: i64) -> FluidStatus {
        let (x, y, z) = (pos_x(loc), pos_y(loc), pos_z(loc));
        let index = a.index(x.div_euclid(16), y.div_euclid(12), z.div_euclid(16));
        if let Some(s) = a.cache.borrow()[index] {
            return s;
        }
        let s = self.compute_fluid(a, x, y, z);
        a.cache.borrow_mut()[index] = Some(s);
        s
    }

    fn compute_fluid(&self, a: &NoiseAquifer, x: i32, y: i32, z: i32) -> FluidStatus {
        let global = self.global_fluid.compute(y);
        let mut min_surface = i32::MAX;
        let above = y + 12;
        let below = y - 12;
        let mut surface_fluid_here = false;
        for off in SURFACE_SAMPLING_OFFSETS_IN_CHUNKS {
            let sx = x + (off[0] << 4);
            let sz = z + (off[1] << 4);
            let level = self.preliminary_surface_level(sx, sz);
            let surface = level.wrapping_add(8);
            let here = off[0] == 0 && off[1] == 0;
            if here && below > surface {
                return global;
            }
            let high_enough = above > surface;
            if high_enough || here {
                let f = self.global_fluid.compute(surface);
                if status_at(f, surface, self.blocks.air) != self.blocks.air {
                    if here {
                        surface_fluid_here = true;
                    }
                    if high_enough {
                        return f;
                    }
                }
            }
            min_surface = jmin_i32(min_surface, level);
        }
        let surface_level = self.surface_level(a, x, y, z, global, min_surface, surface_fluid_here);
        FluidStatus { level: surface_level, fluid: self.fluid_type(a, x, y, z, global, surface_level) }
    }

    fn surface_level(&self, a: &NoiseAquifer, x: i32, y: i32, z: i32, global: FluidStatus, min_surface: i32, surface_fluid: bool) -> i32 {
        let p = Ctx::point(x, y, z);
        let (v1, v2);
        // OverworldBiomeBuilder.isDeepDarkRegion
        if a.erosion.compute(&p) < (-0.225f32) as f64 && a.depth.compute(&p) > 0.9f32 as f64 {
            v1 = -1.0;
            v2 = -1.0;
        } else {
            let d = min_surface.wrapping_add(8).wrapping_sub(y);
            let f = if surface_fluid { clamped_map(d as f64, 0.0, 64.0, 1.0, 0.0) } else { 0.0 };
            let flood = clamp(a.floodedness.compute(&p), -1.0, 1.0);
            let m1 = mth_map(f, 1.0, 0.0, -0.3, 0.8);
            let m2 = mth_map(f, 1.0, 0.0, -0.8, 0.4);
            v1 = flood - m2;
            v2 = flood - m1;
        }
        if v2 > 0.0 {
            global.level
        } else if v1 > 0.0 {
            self.randomized_fluid_surface_level(a, x, y, z, min_surface)
        } else {
            WAY_BELOW_MIN_Y
        }
    }

    fn randomized_fluid_surface_level(&self, a: &NoiseAquifer, x: i32, y: i32, z: i32, min_surface: i32) -> i32 {
        let gx = x.div_euclid(16);
        let gy = y.div_euclid(40);
        let gz = z.div_euclid(16);
        let center = gy * 40 + 20;
        let v = a.spread.compute(&Ctx::point(gx, gy, gz)) * 10.0;
        let q = mth::floor(v / 3.0) * 3;
        min_surface.min(center + q)
    }

    fn fluid_type(&self, a: &NoiseAquifer, x: i32, y: i32, z: i32, global: FluidStatus, surface_level: i32) -> u16 {
        let mut f = global.fluid;
        if surface_level <= -10 && surface_level != WAY_BELOW_MIN_Y && global.fluid != self.blocks.lava {
            let gx = x.div_euclid(64);
            let gy = y.div_euclid(40);
            let gz = z.div_euclid(64);
            let v = a.lava_noise.compute(&Ctx::point(gx, gy, gz));
            if v.abs() > 0.3 {
                f = self.blocks.lava;
            }
        }
        f
    }

    /// `NoiseChunk.preliminarySurfaceLevel`
    pub fn preliminary_surface_level(&self, x: i32, z: i32) -> i32 {
        let qx = (x >> 2) << 2;
        let qz = (z >> 2) << 2;
        let key = (qx as i64 & 0xFFFF_FFFF) | ((qz as i64 & 0xFFFF_FFFF) << 32);
        if let Some(v) = self.prelim.borrow().get(&key) {
            return *v;
        }
        let min_y = self.settings.min_y;
        let mut y = min_y + self.settings.height;
        let mut result = i32::MAX;
        while y >= min_y {
            if self.initial_density.compute(&Ctx::point(qx, y, qz)) > 0.390625 {
                result = y;
                break;
            }
            y -= self.st.cell_height;
        }
        self.prelim.borrow_mut().insert(key, result);
        result
    }

    /// `NoiseBasedChunkGenerator.doFill` for the whole chunk: returns the block state IDs, indexed `(y - min_y) << 8 | z << 4 | x`.
    pub fn fill(&self, min_block_x: i32, min_block_z: i32) -> Vec<u16> {
        let st = &self.st;
        let (cell_w, cell_h) = (st.cell_width, st.cell_height);
        let min_cell_y = self.settings.min_y.div_euclid(cell_h);
        let cell_count_y = self.settings.height.div_euclid(cell_h);
        let mut out = vec![self.blocks.air; (16 * 16 * self.settings.height) as usize];
        self.initialize_for_first_cell_x();
        for cell_x in 0..(16 / cell_w) {
            self.advance_cell_x(cell_x);
            for cell_z in 0..(16 / cell_w) {
                for cell_y in (0..cell_count_y).rev() {
                    self.select_cell_yz(cell_y, cell_z);
                    for iy in (0..cell_h).rev() {
                        let y = (min_cell_y + cell_y) * cell_h + iy;
                        self.update_for_y(y, iy as f64 / cell_h as f64);
                        for ix in 0..cell_w {
                            let x = min_block_x + cell_x * cell_w + ix;
                            self.update_for_x(x, ix as f64 / cell_w as f64);
                            for iz in 0..cell_w {
                                let z = min_block_z + cell_z * cell_w + iz;
                                self.update_for_z(z, iz as f64 / cell_w as f64);
                                let state = self.interpolated_state().unwrap_or(self.blocks.default_block);
                                out[(((y - self.settings.min_y) * 16 + (z & 15)) * 16 + (x & 15)) as usize] = state;
                            }
                        }
                    }
                }
            }
            self.swap_slices();
        }
        self.stop_interpolation();
        out
    }
}

impl NoiseAquifer {
    fn index(&self, x: i32, y: i32, z: i32) -> usize {
        ((y - self.min_grid_y) * self.grid_size_z + (z - self.min_grid_z)).wrapping_mul(self.grid_size_x).wrapping_add(x - self.min_grid_x) as usize
    }
}

fn jmin_i32(a: i32, b: i32) -> i32 {
    a.min(b)
}

/// `Mth.map(double, double, double, double, double)`
fn mth_map(v: f64, from_lo: f64, from_hi: f64, to_lo: f64, to_hi: f64) -> f64 {
    mth::lerp((v - from_lo) / (from_hi - from_lo), to_lo, to_hi)
}
