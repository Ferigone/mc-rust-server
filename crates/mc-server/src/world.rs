//! World: chunks, sections, the superflat generator, Anvil persistence and chunk packet encoding.
//!
//! The section and paletted container format is reconstructed from `LevelChunkSection.write`,
//! `PalettedContainer` (strategies SECTION_STATES / SECTION_BIOMES) and `SimpleBitStorage`.

use crate::anvil::RegionStorage;
use crate::chunk_io;
use crate::light::LightEngine;
use crate::level::{FlatSettings, LevelData};
use mc_data::props::{flag, BlockProps};
use mc_data::GameData;
use mc_protocol::frame::FrameEncoder;
use mc_protocol::nbt::{fastutil_open_hash_order, Compound, Tag};
use mc_protocol::packets::play::{self, BlockEntityInfo, LightData};
use mc_protocol::Put;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{info, warn};

pub const SECTION_BLOCKS: usize = 4096;

/// Dimension info (from `dimension_type`).
#[derive(Debug, Clone, Copy)]
pub struct Dimension {
    pub min_y: i32,
    pub height: i32,
}
impl Dimension {
    pub fn sections(&self) -> usize {
        (self.height / 16) as usize
    }
    /// Light sections: all sections + 1 below + 1 above.
    pub fn light_sections(&self) -> usize {
        self.sections() + 2
    }
    pub fn min_section(&self) -> i32 {
        self.min_y >> 4
    }
}

/// State/biome constants needed for encoding.
#[derive(Debug, Clone)]
pub struct Palette {
    pub air: u16,
    pub cave_air: u16,
    pub void_air: u16,
    /// Number of block states (global palette bits = ceillog2).
    pub block_state_count: u32,
    /// Number of biomes in the registry (global biome palette bits = ceillog2).
    pub biome_count: u32,
    /// Per state: whether `BlockState.getFluidState()` is non-empty (indexed by state ID; missing entries are false).
    pub fluid: Arc<Vec<bool>>,
}
impl Palette {
    pub fn has_fluid(&self, s: u16) -> bool {
        self.fluid.get(s as usize).copied().unwrap_or(false)
    }
    pub fn is_air(&self, s: u16) -> bool {
        s == self.air || s == self.cave_air || s == self.void_air
    }
    fn global_block_bits(&self) -> u32 {
        ceillog2(self.block_state_count)
    }
    fn global_biome_bits(&self) -> u32 {
        ceillog2(self.biome_count)
    }
}

pub fn ceillog2(v: u32) -> u32 {
    if v <= 1 {
        0
    } else {
        32 - (v - 1).leading_zeros()
    }
}

#[derive(Clone)]
pub enum Blocks {
    Uniform(u16),
    Full(Box<[u16; SECTION_BLOCKS]>),
}

/// Biomes of a section: a 4x4x4 grid of biome IDs.
#[derive(Clone)]
pub enum Biomes {
    Uniform(u16),
    Cells(Box<[u16; 64]>),
}

#[derive(Clone)]
pub struct Section {
    pub blocks: Blocks,
    /// Palette order as stored on disk (state IDs); used to reproduce vanilla's palette order.
    pub block_palette: Option<Vec<u16>>,
    pub biomes: Biomes,
    pub biome_palette: Option<Vec<u16>>,
    pub sky_light: Option<Box<[u8; 2048]>>,
    pub block_light: Option<Box<[u8; 2048]>>,
    /// Vanilla quirk: `LevelChunkSection.recalcBlockCounts` (run when a section is loaded from disk) adds every block
    /// with a non-empty fluid state twice to `nonEmptyBlockCount`, while later `setBlockState` calls only track
    /// non-air blocks. The extra count from the load is kept here so the value sent to clients stays identical.
    pub fluid_extra: i32,
}

impl Section {
    pub fn air(air: u16, biome: u16) -> Self {
        Self { blocks: Blocks::Uniform(air), block_palette: None, biomes: Biomes::Uniform(biome), biome_palette: None, sky_light: None, block_light: None, fluid_extra: 0 }
    }
    pub fn get(&self, x: usize, y: usize, z: usize) -> u16 {
        match &self.blocks {
            Blocks::Uniform(s) => *s,
            Blocks::Full(b) => b[(y << 8) | (z << 4) | x],
        }
    }
    pub fn set(&mut self, x: usize, y: usize, z: usize, state: u16) {
        match &mut self.blocks {
            Blocks::Uniform(s) if *s == state => {}
            Blocks::Uniform(s) => {
                let mut full = Box::new([*s; SECTION_BLOCKS]);
                full[(y << 8) | (z << 4) | x] = state;
                self.blocks = Blocks::Full(full);
            }
            Blocks::Full(b) => b[(y << 8) | (z << 4) | x] = state,
        }
    }
    /// `LevelChunkSection.hasOnlyAir()`: the (quirky, see `fluid_extra`) non-empty count is zero.
    pub fn is_all_air(&self, p: &Palette) -> bool {
        self.fluid_extra == 0
            && match &self.blocks {
                Blocks::Uniform(v) => p.is_air(*v),
                Blocks::Full(b) => b.iter().all(|&v| p.is_air(v)),
            }
    }

    /// `LevelChunkSection.recalcBlockCounts()` for the current blocks: non-air blocks plus blocks holding a fluid.
    pub fn recalc_fluid_extra(&mut self, p: &Palette) {
        self.fluid_extra = match &self.blocks {
            Blocks::Uniform(v) => if p.has_fluid(*v) { SECTION_BLOCKS as i32 } else { 0 },
            Blocks::Full(b) => b.iter().filter(|&&v| p.has_fluid(v)).count() as i32,
        };
    }
}

#[derive(Clone)]
pub struct BlockEntity {
    pub type_id: i32,
    pub x: i32,
    pub y: i32,
    pub z: i32,
    /// The complete stored compound (including `id`, `x`, `y`, `z`).
    pub nbt: Compound,
}

/// Heightmaps stored with the chunk (only the two the client reads).
#[derive(Clone)]
pub struct Heightmaps {
    pub world_surface: Vec<i64>,
    pub motion_blocking: Vec<i64>,
}

#[derive(Clone)]
pub struct Chunk {
    pub sections: Vec<Section>,
    pub heightmaps: Option<Heightmaps>,
    pub block_entities: Vec<BlockEntity>,
    /// Light comes from the stored arrays (a loaded chunk with `isLightOn`) rather than being computed.
    pub stored_light: bool,
    /// The chunk was stored without light (`isLightOn=0`): vanilla relights it when it loads it (see `light.rs`).
    pub relit: bool,
    /// Stored light of the two light sections outside the block range (section Y, sky, block); used to verify the light engine.
    pub edge_light: Vec<(i32, Option<Box<[u8; 2048]>>, Option<Box<[u8; 2048]>>)>,
    /// The stored compound without `sections`, preserved for saving.
    pub raw: Option<Compound>,
    pub modified: bool,
    /// Cached packet frame (including coordinates); invalidated when a block changes.
    pub cached: Option<Arc<Vec<u8>>>,
}

impl Chunk {
    pub fn empty(dim: &Dimension, palette: &Palette, biome: u16) -> Chunk {
        Chunk {
            sections: (0..dim.sections()).map(|_| Section::air(palette.air, biome)).collect(),
            heightmaps: None,
            block_entities: Vec::new(),
            stored_light: false,
            relit: false,
            edge_light: Vec::new(),
            raw: None,
            modified: false,
            cached: None,
        }
    }

    pub fn get(&self, dim: &Dimension, x: usize, y: i32, z: usize) -> u16 {
        let sy = (y - dim.min_y) as usize;
        self.sections[sy >> 4].get(x, sy & 15, z)
    }

    /// Light-section index (1-based over chunk sections) of the highest section with non-air blocks.
    fn top_light_index(&self, p: &Palette) -> Option<usize> {
        self.sections.iter().rposition(|s| !s.is_all_air(p)).map(|i| i + 1)
    }
}

/// Context needed to build a packet.
pub struct EncodeCtx<'a> {
    pub dim: Dimension,
    pub palette: &'a Palette,
    pub props: &'a BlockProps,
}

// ------------------------------------------------------------------ encoding

/// Packs values into `long`s like `SimpleBitStorage` (values never straddle a long boundary).
pub fn pack_bits(values: impl Iterator<Item = u32>, count: usize, bits: u32) -> Vec<i64> {
    let per_long = (64 / bits) as usize;
    let mut out = vec![0i64; count.div_ceil(per_long)];
    for (i, v) in values.enumerate() {
        out[i / per_long] |= ((v as u64) << ((i % per_long) as u32 * bits)) as i64;
    }
    out
}

/// Inverse of [`pack_bits`].
pub fn unpack_bits(longs: &[i64], count: usize, bits: u32) -> Vec<u32> {
    let per_long = (64 / bits) as usize;
    let mask = if bits >= 32 { u32::MAX as u64 } else { (1u64 << bits) - 1 };
    (0..count).map(|i| (((longs.get(i / per_long).copied().unwrap_or(0) as u64) >> ((i % per_long) as u32 * bits)) & mask) as u32).collect()
}

/// How the palette entries of a container are ordered.
enum PaletteOrder<'a> {
    /// First appearance in index order.
    FirstSeen,
    /// The given state first, then first appearance. A vanilla section starts as a palette holding only air,
    /// so after more blocks are added air has index 0.
    Front(u16),
    /// The stored palette order (entries still in use), then new entries in first-appearance order.
    Hint(&'a [u16]),
}

fn write_container(w: &mut Vec<u8>, values: &[u16], kind: ContainerKind, global_bits: u32, order: PaletteOrder<'_>) {
    let max_pal = kind.max_indirect_entries();
    let mut seen: Vec<u16> = Vec::new();
    let mut overflow = false;
    for &v in values {
        if !seen.contains(&v) {
            if seen.len() == max_pal {
                overflow = true;
                break;
            }
            seen.push(v);
        }
    }
    if !overflow && seen.len() == 1 {
        w.put_u8(0);
        w.put_varint(seen[0] as i32);
        w.put_varint(0); // empty long array (ZeroBitStorage)
        return;
    }
    if overflow {
        // global palette: values are written directly, without a palette list
        w.put_u8(global_bits as u8);
        let longs = pack_bits(values.iter().map(|&v| v as u32), values.len(), global_bits);
        w.put_long_array(&longs);
        return;
    }
    let palette: Vec<u16> = match order {
        PaletteOrder::FirstSeen => seen,
        PaletteOrder::Front(f) => {
            let mut p = seen;
            if let Some(pos) = p.iter().position(|&x| x == f) {
                let v = p.remove(pos);
                p.insert(0, v);
            }
            p
        }
        PaletteOrder::Hint(h) => {
            let mut p: Vec<u16> = h.iter().copied().filter(|x| seen.contains(x)).collect();
            for v in seen {
                if !p.contains(&v) {
                    p.push(v);
                }
            }
            p
        }
    };
    let bits = kind.indirect_bits(palette.len());
    w.put_u8(bits as u8);
    w.put_varint(palette.len() as i32);
    for p in &palette {
        w.put_varint(*p as i32);
    }
    let idx: Vec<u32> = values.iter().map(|v| palette.iter().position(|p| p == v).unwrap() as u32).collect();
    let longs = pack_bits(idx.into_iter(), values.len(), bits);
    w.put_long_array(&longs);
}

#[derive(Clone, Copy)]
enum ContainerKind {
    /// SECTION_STATES: 1..=4 bits -> linear (always 4 bits), 5..=8 -> hashmap, more -> global
    States,
    /// SECTION_BIOMES: 1..=3 bits -> linear, more -> global
    Biomes,
}
impl ContainerKind {
    fn max_indirect_entries(self) -> usize {
        match self {
            ContainerKind::States => 256,
            ContainerKind::Biomes => 8,
        }
    }
    fn indirect_bits(self, n: usize) -> u32 {
        let b = ceillog2(n as u32);
        match self {
            ContainerKind::States => b.max(4),
            ContainerKind::Biomes => b.max(1),
        }
    }
}

fn encode_section(w: &mut Vec<u8>, s: &Section, p: &Palette) {
    let non_empty: i16 = match &s.blocks {
        Blocks::Uniform(v) => {
            if p.is_air(*v) {
                0
            } else {
                4096
            }
        }
        Blocks::Full(b) => b.iter().filter(|&&v| !p.is_air(v)).count() as i16,
    };
    w.put_i16(non_empty.wrapping_add(s.fluid_extra as i16));
    match &s.blocks {
        Blocks::Uniform(v) => {
            w.put_u8(0);
            w.put_varint(*v as i32);
            w.put_varint(0);
        }
        Blocks::Full(b) => {
            let order = match &s.block_palette {
                Some(h) => PaletteOrder::Hint(h),
                None => PaletteOrder::Front(p.air),
            };
            write_container(w, &b[..], ContainerKind::States, p.global_block_bits(), order)
        }
    }
    match &s.biomes {
        Biomes::Uniform(b) => {
            w.put_u8(0);
            w.put_varint(*b as i32);
            w.put_varint(0);
        }
        Biomes::Cells(cells) => {
            let order = match &s.biome_palette {
                Some(h) => PaletteOrder::Hint(h),
                None => PaletteOrder::FirstSeen,
            };
            write_container(w, &cells[..], ContainerKind::Biomes, p.global_biome_bits(), order)
        }
    }
}

/// `Heightmap.Types` the client reads: the predicate that decides which block counts.
#[derive(Clone, Copy)]
enum HeightmapType {
    /// `WORLD_SURFACE`: not air
    WorldSurface,
    /// `MOTION_BLOCKING`: blocks motion or holds a fluid
    MotionBlocking,
}

/// Heightmap values (offset above `min_y`): one above the first block from the top that matches the type's predicate.
/// Loaded chunks use their stored heightmaps; this is used for generated and edited chunks.
fn computed_heightmap(chunk: &Chunk, dim: &Dimension, p: &Palette, props: &BlockProps, ty: HeightmapType) -> Vec<i64> {
    let matches = |s: u16| match ty {
        HeightmapType::WorldSurface => !p.is_air(s),
        HeightmapType::MotionBlocking => props.state(s as u32).is_some_and(|st| st.has(flag::BLOCKS_MOTION) || st.has(flag::FLUID)),
    };
    let mut vals = [0u32; 256];
    for z in 0..16usize {
        for x in 0..16usize {
            let mut h = 0;
            'col: for sy in (0..dim.sections()).rev() {
                let sec = &chunk.sections[sy];
                if let Blocks::Uniform(v) = sec.blocks {
                    if !matches(v) {
                        continue;
                    }
                }
                for ly in (0..16usize).rev() {
                    if matches(sec.get(x, ly, z)) {
                        h = (sy * 16 + ly + 1) as u32;
                        break 'col;
                    }
                }
            }
            vals[x + z * 16] = h;
        }
    }
    pack_bits(vals.iter().copied(), 256, ceillog2(dim.height as u32 + 1))
}

fn empty_light(n: usize) -> LightData {
    LightData { sky_mask: vec![0; n.div_ceil(64)], empty_sky_mask: vec![0; n.div_ceil(64)], block_mask: vec![0; n.div_ceil(64)], empty_block_mask: vec![0; n.div_ceil(64)], ..Default::default() }
}

/// Simple sky light for generated chunks: 15 above the highest opaque block of a column, 0 below it.
/// (The full vanilla light engine: see the roadmap.) Matches vanilla for superflat worlds.
fn computed_light(chunk: &Chunk, dim: &Dimension, p: &Palette) -> LightData {
    let n = dim.light_sections();
    let mut light = empty_light(n);
    // height of the first opaque block in the column (index relative to min_y, exclusive)
    let mut top = [0usize; 256];
    for z in 0..16usize {
        for x in 0..16usize {
            let mut h = 0;
            'col: for sy in (0..dim.sections()).rev() {
                for ly in (0..16usize).rev() {
                    if !p.is_air(chunk.sections[sy].get(x, ly, z)) {
                        h = sy * 16 + ly + 1;
                        break 'col;
                    }
                }
            }
            top[x + z * 16] = h;
        }
    }
    // The vanilla light engine keeps layers only for non-empty sections and their vertical neighbours:
    // from the section below the world to one above the highest non-empty one (measured on the real 1.21.1 server).
    // Above that the client assumes full sky light without data.
    let top_nonempty = chunk.sections.iter().rposition(|s| !s.is_all_air(p));
    let active_end = top_nonempty.map_or(1, |t| t + 2).min(n - 1); // light section index, inclusive
    for ls in 0..=active_end {
        // ls = 0 -> section below the world; ls = n-1 -> above
        let mut arr = [0u8; 2048];
        let mut any = false;
        for z in 0..16usize {
            for x in 0..16usize {
                let t = top[x + z * 16] as i32;
                for ly in 0..16usize {
                    let rel_y = (ls as i32 - 1) * 16 + ly as i32; // y relative to min_y
                    if rel_y >= t {
                        let idx = (ly << 8) | (z << 4) | x;
                        arr[idx >> 1] |= 0x0F << ((idx & 1) * 4);
                        any = true;
                    }
                }
            }
        }
        if any {
            light.sky_mask[ls / 64] |= 1 << (ls % 64);
            light.sky.push(arr);
        } else {
            light.empty_sky_mask[ls / 64] |= 1 << (ls % 64);
        }
        // no block light sources: the layer exists but is empty
        light.empty_block_mask[ls / 64] |= 1 << (ls % 64);
    }
    light
}

/// Light of a loaded chunk, reproducing what the vanilla light engine sends (measured on real worlds):
/// layers exist for light sections up to one above the highest non-empty section found in the 3x3 chunk
/// neighbourhood; stored non-zero arrays are sent as data, missing ones as empty layers, except that sections
/// above the chunk's own highest non-empty section carry full sky light.
fn stored_light(chunk: &Chunk, neighbourhood_top: Option<usize>, dim: &Dimension, p: &Palette) -> LightData {
    let n = dim.light_sections();
    let mut light = empty_light(n);
    let Some(t) = neighbourhood_top else { return light };
    let own_top = chunk.top_light_index(p);
    for i in 0..=(t + 1).min(n - 1) {
        let sec = if i >= 1 { chunk.sections.get(i - 1) } else { None };
        let set = |m: &mut Vec<u64>| m[i / 64] |= 1 << (i % 64);
        match sec.and_then(|s| s.sky_light.as_ref()) {
            Some(a) => {
                set(&mut light.sky_mask);
                light.sky.push(**a);
            }
            None if own_top.is_some_and(|o| i > o) => {
                set(&mut light.sky_mask);
                light.sky.push([0xFF; 2048]);
            }
            None => set(&mut light.empty_sky_mask),
        }
        match sec.and_then(|s| s.block_light.as_ref()) {
            Some(a) => {
                set(&mut light.block_mask);
                light.block.push(**a);
            }
            None => set(&mut light.empty_block_mask),
        }
    }
    light
}

/// `BlockEntity.getUpdateTag` for the types that override it with `saveCustomOnly`/`saveWithoutMetadata`
/// (the stored data without `id`/`x`/`y`/`z`, the data components and the `keepPacked` flag that only
/// `LevelChunk` adds when it serializes a chunk); every other type sends an empty tag (encoded as TAG_End).
fn block_entity_update_tag(id: &str, nbt: &Compound) -> Option<Compound> {
    const WITH_DATA: &[&str] = &[
        "minecraft:banner",
        "minecraft:beacon",
        "minecraft:conduit",
        "minecraft:decorated_pot",
        "minecraft:jigsaw",
        "minecraft:sign",
        "minecraft:hanging_sign",
        "minecraft:skull",
        "minecraft:mob_spawner",
        "minecraft:structure_block",
        "minecraft:end_gateway",
    ];
    if !WITH_DATA.contains(&id) {
        return None;
    }
    let mut c = Compound(nbt.0.iter().filter(|(k, _)| !matches!(k.as_str(), "id" | "x" | "y" | "z" | "keepPacked" | "components")).cloned().collect());
    if id == "minecraft:mob_spawner" {
        c.remove("SpawnPotentials");
    }
    if c.is_empty() {
        None
    } else {
        Some(c)
    }
}

/// `Vec3i.hashCode()` of a `BlockPos`.
fn block_pos_hash(x: i32, y: i32, z: i32) -> i32 {
    y.wrapping_add(z.wrapping_mul(31)).wrapping_mul(31).wrapping_add(x)
}

/// Builds the `level_chunk_with_light` payload for chunk (cx, cz).
pub fn build_chunk_payload(chunk: &Chunk, cx: i32, cz: i32, ctx: &EncodeCtx<'_>, light: &LightData) -> Vec<u8> {
    let mut sections = Vec::with_capacity(8192);
    for s in &chunk.sections {
        encode_section(&mut sections, s, ctx.palette);
    }
    let (ws, mb) = match &chunk.heightmaps {
        Some(h) => (h.world_surface.clone(), h.motion_blocking.clone()),
        None => {
            (
                computed_heightmap(chunk, &ctx.dim, ctx.palette, ctx.props, HeightmapType::WorldSurface),
                computed_heightmap(chunk, &ctx.dim, ctx.palette, ctx.props, HeightmapType::MotionBlocking),
            )
        }
    };
    let mut hmc = Compound::new();
    // The client only reads WORLD_SURFACE and MOTION_BLOCKING (`Heightmap.Types.sendToClient`).
    hmc.put("MOTION_BLOCKING", Tag::LongArray(mb));
    hmc.put("WORLD_SURFACE", Tag::LongArray(ws));
    // Block entities in the iteration order of the chunk's fastutil map (keys are BlockPos).
    let hashes: Vec<i32> = chunk.block_entities.iter().map(|b| block_pos_hash(b.x, b.y, b.z)).collect();
    let bes: Vec<BlockEntityInfo> = fastutil_open_hash_order(&hashes)
        .into_iter()
        .map(|i| {
            let b = &chunk.block_entities[i];
            let id = b.nbt.get_str("id").unwrap_or("");
            BlockEntityInfo {
                packed_xz: ((((b.x & 15) << 4) | (b.z & 15)) & 0xFF) as u8,
                y: b.y as i16,
                type_id: b.type_id,
                tag: block_entity_update_tag(id, &b.nbt).map(Tag::Compound),
            }
        })
        .collect();
    play::level_chunk_with_light(cx, cz, &hmc, &sections, &bes, light)
}

// -------------------------------------------------------------------- world

/// What serves chunks that are not stored on disk.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fallback {
    /// The superflat template (for flat worlds).
    Flat,
    /// Empty air (for worlds whose generator is not ported yet).
    Void,
}

/// Parameters for creating a world that does not exist yet.
pub struct NewWorld {
    pub name: String,
    pub seed: i64,
    pub game_type: i32,
    pub difficulty: u8,
    pub hardcore: bool,
}

pub struct World {
    pub seed: i64,
    pub dim: Dimension,
    pub palette: Palette,
    pub spawn: (i32, i32, i32),
    pub level: Option<LevelData>,
    pub fallback: Fallback,
    data: Arc<GameData>,
    biome_ids: HashMap<String, u16>,
    template: Arc<Chunk>,
    void: Arc<Chunk>,
    /// Light of the flat template chunk (identical for every chunk of a superflat world), computed once.
    template_light: Option<LightData>,
    chunks: HashMap<(i32, i32), Chunk>,
    /// Chunks known not to exist on disk (avoids re-reading region files).
    absent: HashSet<(i32, i32)>,
    storage: Option<RegionStorage>,
    dir: Option<PathBuf>,
    dirty: HashSet<(i32, i32)>,
}

impl World {
    /// A superflat world kept in memory (no persistence); used by tests.
    #[cfg(test)]
    pub fn flat(dim: Dimension, palette: Palette, layers: &[u16], biome: u16, seed: i64, data: Arc<GameData>) -> Self {
        let template = Arc::new(Self::flat_template(&dim, &palette, layers, biome));
        let void = Arc::new(Chunk::empty(&dim, &palette, biome));
        let spawn_y = dim.min_y + layers.len() as i32;
        Self {
            seed,
            dim,
            palette,
            spawn: (0, spawn_y, 0),
            level: None,
            fallback: Fallback::Flat,
            data,
            biome_ids: HashMap::new(),
            template,
            void,
            template_light: None,
            chunks: HashMap::new(),
            absent: HashSet::new(),
            storage: None,
            dir: None,
            dirty: HashSet::new(),
        }
    }

    fn flat_template(dim: &Dimension, palette: &Palette, layers: &[u16], biome: u16) -> Chunk {
        let mut chunk = Chunk::empty(dim, palette, biome);
        for (i, &state) in layers.iter().enumerate() {
            for z in 0..16 {
                for x in 0..16 {
                    chunk.sections[i >> 4].set(x, i & 15, z, state);
                }
            }
        }
        chunk
    }

    /// Opens (or creates) the world in `dir`. Chunks come from the region files; ungenerated ones from the fallback.
    pub fn open(dir: PathBuf, data: Arc<GameData>, dim: Dimension, palette: Palette, new_world: NewWorld) -> anyhow::Result<Self> {
        let biome_ids: HashMap<String, u16> = data
            .dynamic_registry("minecraft:worldgen/biome")
            .map(|r| r.entries.iter().enumerate().map(|(i, e)| (e.id.clone(), i as u16)).collect())
            .unwrap_or_default();
        let level_path = dir.join("level.dat");
        let mut level = LevelData::load(&level_path)?;
        let flat_cfg: Option<FlatSettings>;
        let (seed, spawn);
        match &level {
            Some(l) => {
                if l.data_version() != 0 && l.data_version() != crate::level::DATA_VERSION && l.data_version() != 3953 {
                    warn!("level.dat has DataVersion {} (this server supports 1.21/1.21.1 = 3953/3955); upgrading older worlds is not implemented", l.data_version());
                }
                flat_cfg = l.flat_settings();
                seed = l.seed();
                spawn = l.spawn();
            }
            None => {
                flat_cfg = Some(FlatSettings::default());
                seed = new_world.seed;
                let layers_h: i32 = FlatSettings::default().layers.iter().map(|l| l.1).sum();
                spawn = (0, dim.min_y + layers_h, 0);
                let l = LevelData::new_flat(&new_world.name, seed, new_world.game_type, new_world.difficulty, new_world.hardcore, spawn, flat_cfg.as_ref().unwrap());
                std::fs::create_dir_all(&dir)?;
                l.save(&level_path)?;
                info!("Created a new flat world in {}", dir.display());
                level = Some(l);
            }
        }
        let fallback = if flat_cfg.is_some() { Fallback::Flat } else { Fallback::Void };
        if fallback == Fallback::Void {
            warn!("This world uses the noise generator, which is not ported yet: chunks that are not stored in the region files are served empty");
        }
        // flat template from the generator settings
        let cfg = flat_cfg.unwrap_or_default();
        let mut layers: Vec<u16> = Vec::new();
        for (block, height) in &cfg.layers {
            let st = data.blocks.default_state(block).unwrap_or(0) as u16;
            for _ in 0..*height {
                layers.push(st);
            }
        }
        let plains = *biome_ids.get(&cfg.biome).or_else(|| biome_ids.get("minecraft:plains")).unwrap_or(&0);
        let template = Arc::new(Self::flat_template(&dim, &palette, &layers, plains));
        let void = Arc::new(Chunk::empty(&dim, &palette, plains));
        Ok(Self {
            seed,
            dim,
            palette,
            spawn,
            level,
            fallback,
            biome_ids,
            data,
            template,
            void,
            template_light: None,
            chunks: HashMap::new(),
            absent: HashSet::new(),
            storage: Some(RegionStorage::new(dir.join("region"))),
            dir: Some(dir),
            dirty: HashSet::new(),
        })
    }

    fn load_chunk(&mut self, cx: i32, cz: i32) -> Option<Chunk> {
        let storage = self.storage.as_mut()?;
        match storage.read(cx, cz) {
            Ok(Some(nbt)) => match chunk_io::chunk_from_nbt(&nbt, &self.dim, &self.palette, &self.data, &self.biome_ids) {
                Ok(c) => c,
                Err(e) => {
                    warn!("Could not read chunk ({cx}, {cz}): {e}");
                    None
                }
            },
            Ok(None) => None,
            Err(e) => {
                warn!("Could not read region data for chunk ({cx}, {cz}): {e}");
                None
            }
        }
    }

    /// Loads the chunk from disk if needed. Returns whether a real (stored or modified) chunk exists.
    pub fn ensure(&mut self, cx: i32, cz: i32) -> bool {
        if self.chunks.contains_key(&(cx, cz)) {
            return true;
        }
        if self.absent.contains(&(cx, cz)) {
            return false;
        }
        match self.load_chunk(cx, cz) {
            Some(c) => {
                self.chunks.insert((cx, cz), c);
                true
            }
            None => {
                self.absent.insert((cx, cz));
                false
            }
        }
    }

    fn fallback_chunk(&self) -> &Chunk {
        match self.fallback {
            Fallback::Flat => &self.template,
            Fallback::Void => &self.void,
        }
    }

    /// A chunk that is already loaded (no disk access, no fallback).
    pub fn loaded_chunk(&self, cx: i32, cz: i32) -> Option<&Chunk> {
        self.chunks.get(&(cx, cz))
    }

    pub fn chunk(&mut self, cx: i32, cz: i32) -> &Chunk {
        self.ensure(cx, cz);
        match self.chunks.get(&(cx, cz)) {
            Some(c) => c,
            None => self.fallback_chunk(),
        }
    }

    /// Highest non-empty section of a stored chunk as a light-section index; `None` for absent chunks.
    fn light_top_of(&mut self, cx: i32, cz: i32) -> Option<usize> {
        if !self.ensure(cx, cz) {
            return None;
        }
        let p = self.palette.clone();
        self.chunks.get(&(cx, cz)).and_then(|c| c.top_light_index(&p))
    }

    pub fn get_block(&mut self, x: i32, y: i32, z: i32) -> u16 {
        if y < self.dim.min_y || y >= self.dim.min_y + self.dim.height {
            return self.palette.void_air;
        }
        let dim = self.dim;
        self.chunk(x >> 4, z >> 4).get(&dim, (x & 15) as usize, y, (z & 15) as usize)
    }

    /// Highest non-air block + 1 in a column (the equivalent of `PlayerRespawnLogic.getOverworldRespawnPos`
    /// for a world without fluids/leaves); `None` when the column is empty.
    pub fn surface_y(&mut self, x: i32, z: i32) -> Option<i32> {
        let (dim, pal) = (self.dim, self.palette.clone());
        let ch = self.chunk(x >> 4, z >> 4);
        for sy in (0..dim.sections()).rev() {
            for ly in (0..16usize).rev() {
                if !pal.is_air(ch.sections[sy].get((x & 15) as usize, ly, (z & 15) as usize)) {
                    return Some(dim.min_y + (sy * 16 + ly) as i32 + 1);
                }
            }
        }
        None
    }

    /// Sets a block; returns the previous state. Outside the world: no change.
    pub fn set_block(&mut self, x: i32, y: i32, z: i32, state: u16) -> Option<u16> {
        if y < self.dim.min_y || y >= self.dim.min_y + self.dim.height {
            return None;
        }
        let key = (x >> 4, z >> 4);
        if !self.ensure(key.0, key.1) {
            let base = self.fallback_chunk().clone();
            self.chunks.insert(key, base);
            self.absent.remove(&key);
        }
        let chunk = self.chunks.get_mut(&key).unwrap();
        let sy = (y - self.dim.min_y) as usize;
        let old = chunk.sections[sy >> 4].get((x & 15) as usize, sy & 15, (z & 15) as usize);
        if old != state {
            chunk.sections[sy >> 4].set((x & 15) as usize, sy & 15, (z & 15) as usize, state);
            chunk.cached = None;
            chunk.modified = true;
            // the stored light is out of date: from now on the light engine computes this chunk's light
            chunk.stored_light = false;
            self.dirty.insert(key);
            // light spreads at most 15 blocks, so chunks within reach of the edit change too
            for dx in -1..=1 {
                for dz in -1..=1 {
                    let (kx, kz) = (key.0 + dx, key.1 + dz);
                    let dist_x = (kx * 16 - x).max(x - (kx * 16 + 15)).max(0);
                    let dist_z = (kz * 16 - z).max(z - (kz * 16 + 15)).max(0);
                    if dist_x.max(dist_z) <= 15 {
                        if let Some(c) = self.chunks.get_mut(&(kx, kz)) {
                            c.cached = None;
                            c.stored_light = false;
                        }
                    }
                }
            }
        }
        Some(old)
    }

    /// A ready-to-send frame (compressed), cached per chunk.
    pub fn chunk_frame(&mut self, cx: i32, cz: i32, enc: &mut FrameEncoder) -> Arc<Vec<u8>> {
        let real = self.ensure(cx, cz);
        if real {
            if let Some(f) = self.chunks.get(&(cx, cz)).and_then(|c| c.cached.clone()) {
                return f;
            }
        }
        // Stored light is only trusted when the chunk and all its loaded neighbours are lit: vanilla relights an unlit
        // neighbour when it loads it, which changes the light of the chunks around it as well.
        let mut stored = real && self.chunks.get(&(cx, cz)).is_some_and(|c| c.stored_light);
        if stored {
            for dx in -1..=1 {
                for dz in -1..=1 {
                    self.ensure(cx + dx, cz + dz);
                    if self.chunks.get(&(cx + dx, cz + dz)).is_some_and(|c| !c.stored_light) {
                        stored = false;
                    }
                }
            }
        }
        let neighbourhood_top = if stored {
            let mut top: Option<usize> = None;
            for dx in -1..=1 {
                for dz in -1..=1 {
                    if let Some(t) = self.light_top_of(cx + dx, cz + dz) {
                        top = Some(top.map_or(t, |o| o.max(t)));
                    }
                }
            }
            top
        } else {
            None
        };
        // chunks without trusted stored light: the light engine (the void fallback keeps the simple "sky above" light)
        let light = if stored {
            None
        } else {
            Some(self.engine_light(cx, cz, real))
        };
        let ctx = EncodeCtx { dim: self.dim, palette: &self.palette, props: &self.data.block_props };
        let chunk = match self.chunks.get(&(cx, cz)) {
            Some(c) => c,
            None => self.fallback_chunk(),
        };
        let light = match light {
            Some(l) => l,
            None => stored_light(chunk, neighbourhood_top, &ctx.dim, &self.palette),
        };
        let payload = build_chunk_payload(chunk, cx, cz, &ctx, &light);
        let frame = Arc::new(enc.encode(&payload));
        if real {
            if let Some(c) = self.chunks.get_mut(&(cx, cz)) {
                c.cached = Some(frame.clone());
            }
        }
        frame
    }

    /// Light of a chunk computed by the light engine from the blocks of the 3x3 chunks around it. Missing neighbours count as
    /// superflat template chunks in a flat world and as absent otherwise.
    fn engine_light(&mut self, cx: i32, cz: i32, real: bool) -> LightData {
        let flat = self.fallback == Fallback::Flat;
        if !real {
            match self.fallback {
                Fallback::Void => return computed_light(&self.void, &self.dim, &self.palette),
                Fallback::Flat => {
                    if let Some(l) = &self.template_light {
                        return l.clone();
                    }
                }
            }
        }
        for dx in -2..=2 {
            for dz in -2..=2 {
                self.ensure(cx + dx, cz + dz);
            }
        }
        let engine = LightEngine::new(&self.data.block_props, &self.palette, self.dim);
        let template: &Chunk = &self.template;
        let chunks = &self.chunks;
        let light = if real {
            engine.light_chunk(cx, cz, &|x, z| chunks.get(&(x, z)).or(if flat { Some(template) } else { None })).to_light_data()
        } else {
            // a chunk of the superflat world surrounded by superflat chunks
            engine.light_chunk(cx, cz, &|_, _| Some(template)).to_light_data()
        };
        if !real {
            self.template_light = Some(light.clone());
        }
        light
    }

    fn write_chunk(&mut self, key: (i32, i32), game_time: i64) -> bool {
        let Some(chunk) = self.chunks.get(&key) else { return true };
        let nbt = chunk_io::chunk_to_nbt(chunk, key.0, key.1, &self.dim, &self.palette, &self.data, &self.biome_ids, game_time);
        match self.storage.as_mut() {
            Some(st) => match st.write(key.0, key.1, &nbt) {
                Ok(()) => true,
                Err(e) => {
                    warn!("Could not save chunk ({}, {}): {e}", key.0, key.1);
                    false
                }
            },
            None => true,
        }
    }

    /// Writes all modified chunks to the region files and `level.dat`. Returns the number of chunks saved.
    pub fn save(&mut self, game_time: i64, day_time: i64) -> usize {
        let mut saved = 0;
        let keys: Vec<(i32, i32)> = self.dirty.iter().copied().collect();
        for key in keys {
            if self.write_chunk(key, game_time) {
                self.dirty.remove(&key);
                if let Some(c) = self.chunks.get_mut(&key) {
                    c.modified = false;
                }
                saved += 1;
            }
        }
        if let (Some(level), Some(dir)) = (self.level.as_mut(), self.dir.as_ref()) {
            let d = level.data_mut();
            d.put("Time", Tag::Long(game_time));
            d.put("DayTime", Tag::Long(day_time));
            let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
            d.put("LastPlayed", Tag::Long(now_ms));
            if let Err(e) = level.save(&dir.join("level.dat")) {
                warn!("Could not save level.dat: {e}");
            }
        }
        saved
    }

    /// Drops chunks nobody needs from memory (saving modified ones first).
    pub fn evict(&mut self, keep: &HashSet<(i32, i32)>, game_time: i64) {
        let drop: Vec<(i32, i32)> = self.chunks.keys().filter(|k| !keep.contains(k)).copied().collect();
        for k in drop {
            if self.dirty.contains(&k) {
                if !self.write_chunk(k, game_time) {
                    continue; // keep it in memory and retry on the next save
                }
                self.dirty.remove(&k);
            }
            self.chunks.remove(&k);
        }
        if self.absent.len() > 20_000 {
            self.absent.clear();
        }
    }

    /// Whether the overworld generator is a flat one (the `isFlat` flag of the Login packet).
    pub fn is_flat(&self) -> bool {
        self.fallback == Fallback::Flat
    }

    /// `BiomeManager.obfuscateSeed`: Guava `Hashing.sha256().hashLong(seed).asLong()`
    /// (SHA-256 of the 8 little-endian bytes, the first 8 bytes read as a little-endian long).
    pub fn hashed_seed(&self) -> i64 {
        use sha2::{Digest, Sha256};
        let h = Sha256::digest(self.seed.to_le_bytes());
        i64::from_le_bytes(h[..8].try_into().unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pal() -> Palette {
        Palette { air: 0, cave_air: 10000, void_air: 10001, block_state_count: 26684, biome_count: 64, fluid: Default::default() }
    }

    fn no_data() -> Arc<GameData> {
        // The tests below do not touch the data tables; they build the world through the flat constructor.
        Arc::new(GameData::empty())
    }

    #[test]
    fn ceillog2_matches_java() {
        assert_eq!(ceillog2(26684), 15);
        assert_eq!(ceillog2(64), 6);
        assert_eq!(ceillog2(65), 7);
        assert_eq!(ceillog2(2), 1);
        assert_eq!(ceillog2(1), 0);
    }

    #[test]
    fn pack_unpack_roundtrip() {
        for bits in [1u32, 3, 4, 7, 9, 15] {
            let vals: Vec<u32> = (0..300u32).map(|i| i.wrapping_mul(2654435761u32).wrapping_add(i * i) & ((1 << bits) - 1)).collect();
            let longs = pack_bits(vals.iter().copied(), vals.len(), bits);
            assert_eq!(unpack_bits(&longs, vals.len(), bits), vals, "bits {bits}");
        }
    }

    #[test]
    fn heightmap_bits_for_384() {
        // Mth.ceillog2(384 + 1) = 9 -> 7 values per long -> 37 longs
        let dim = Dimension { min_y: -64, height: 384 };
        let mut w = World::flat(dim, pal(), &[7, 10, 10, 9], 1, 12345, no_data());
        let hm = computed_heightmap(w.chunk(0, 0), &dim, &pal(), &BlockProps::default(), HeightmapType::WorldSurface);
        assert_eq!(hm.len(), 37);
        // height 4 in every column: 7 values of 9 bits in the first long
        let expect: u64 = (0..7).fold(0u64, |a, i| a | (4u64 << (i * 9)));
        assert_eq!(hm[0] as u64, expect);
    }

    #[test]
    fn flat_world_blocks_and_light() {
        let dim = Dimension { min_y: -64, height: 384 };
        let mut w = World::flat(dim, pal(), &[7, 10, 10, 9], 1, 12345, no_data());
        assert_eq!(w.get_block(5, -64, 5), 7);
        assert_eq!(w.get_block(5, -61, 5), 9);
        assert_eq!(w.get_block(5, -60, 5), 0);
        assert_eq!(w.spawn, (0, -60, 0));
        let l = computed_light(w.chunk(0, 0), &dim, &pal());
        // as measured on the vanilla server: sky_mask={1,2}, empty_sky={0}, empty_block={0,1,2}
        assert_eq!(l.sky_mask[0], 0b110);
        assert_eq!(l.empty_sky_mask[0], 0b001);
        assert_eq!(l.block_mask[0], 0);
        assert_eq!(l.empty_block_mask[0], 0b111);
        assert_eq!(l.sky.len(), 2);
        // in section 1 the first 4 layers (y -64..-61) are dark, the higher ones are lit
        let arr = &l.sky[0];
        assert_eq!(arr[(3 << 8) >> 1] & 0x0F, 0); // y=3 (-61) dark
        assert_eq!(arr[(4 << 8) >> 1] & 0x0F, 0x0F); // y=4 (-60) lit
    }

    #[test]
    fn paletted_container_roundtrip_shape() {
        let mut vals = [0u16; 4096];
        vals[0] = 7;
        vals[1] = 9;
        let mut w = Vec::new();
        write_container(&mut w, &vals, ContainerKind::States, 15, PaletteOrder::FirstSeen);
        // 3 palette entries -> 4 bits, linear palette: bits(1) + len(1) + 3 + array length(2) + 256 longs
        assert_eq!(w[0], 4);
        assert_eq!(w[1], 3);
        let mut r = mc_protocol::Reader::new(&w[2..]);
        assert_eq!((r.varint().unwrap(), r.varint().unwrap(), r.varint().unwrap()), (7, 9, 0)); // order of first appearance
        assert_eq!(r.varint().unwrap(), 256);
        // a palette hint reorders entries (stored order), unused entries are dropped
        let mut w2 = Vec::new();
        write_container(&mut w2, &vals, ContainerKind::States, 15, PaletteOrder::Hint(&[0, 3, 9, 7]));
        let mut r2 = mc_protocol::Reader::new(&w2[2..]);
        assert_eq!((r2.varint().unwrap(), r2.varint().unwrap(), r2.varint().unwrap()), (0, 9, 7));
    }

    /// Value measured on the real vanilla 1.21.1 server (Login packet, `hashedSeed` field) for `level-seed=12345`.
    #[test]
    fn hashed_seed_matches_vanilla() {
        let dim = Dimension { min_y: -64, height: 384 };
        let mut w = World::flat(dim, pal(), &[7, 10, 10, 9], 1, 12345, no_data());
        assert_eq!(w.hashed_seed(), 293737985876514017);
        assert_eq!(w.surface_y(3, 3), Some(-60));
    }

    #[test]
    fn set_block_creates_override_and_invalidates_cache() {
        let dim = Dimension { min_y: -64, height: 384 };
        let mut w = World::flat(dim, pal(), &[7, 10, 10, 9], 1, 12345, no_data());
        let mut enc = FrameEncoder::new(None);
        let f1 = w.chunk_frame(2, 3, &mut enc);
        assert_eq!(w.set_block(33, -60, 50, 1), Some(0));
        assert_eq!(w.get_block(33, -60, 50), 1);
        assert_eq!(w.get_block(34, -60, 50), 0);
        let f2 = w.chunk_frame(2, 3, &mut enc);
        assert_ne!(*f1, *f2);
        // the neighbouring chunk is unchanged
        assert_eq!(w.get_block(0, -60, 0), 0);
    }

    #[test]
    fn block_entity_tags() {
        let nbt = Compound::new().with("id", Tag::string("minecraft:sign")).with("x", Tag::Int(1)).with("y", Tag::Int(2)).with("z", Tag::Int(3)).with("is_waxed", Tag::Byte(0));
        let t = block_entity_update_tag("minecraft:sign", &nbt).unwrap();
        assert_eq!(t.0.len(), 1);
        assert!(block_entity_update_tag("minecraft:chest", &nbt).is_none());
        // `keepPacked` (added by LevelChunk when saving) and the data components are not part of saveCustomOnly
        let stored = nbt.clone().with("keepPacked", Tag::Byte(0)).with("components", Tag::Compound(Compound::new()));
        assert_eq!(block_entity_update_tag("minecraft:sign", &stored).unwrap().0.len(), 1);
        // a spawner whose only custom data would be SpawnPotentials has nothing left to send
        let only_potentials = Compound::new().with("id", Tag::string("minecraft:mob_spawner")).with("SpawnPotentials", Tag::List(vec![]));
        assert!(block_entity_update_tag("minecraft:mob_spawner", &only_potentials).is_none());
    }
}
