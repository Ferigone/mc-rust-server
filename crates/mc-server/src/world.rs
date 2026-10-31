//! World: chunks, sections, the superflat generator, chunk packet encoding.
//!
//! The section and paletted container format reconstructed from `LevelChunkSection.write`,
//! `PalettedContainer` (strategie SECTION_STATES / SECTION_BIOMES) i `SimpleBitStorage`.

use mc_protocol::frame::FrameEncoder;
use mc_protocol::nbt::{Compound, Tag};
use mc_protocol::packets::play::{self, LightData};
use mc_protocol::Put;
use std::collections::HashMap;
use std::sync::Arc;

pub const SECTION_BLOCKS: usize = 4096;

/// Informacje o wymiarze (z `dimension_type`).
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
}

/// State/biome constants needed for encoding.
#[derive(Debug, Clone)]
pub struct Palette {
    pub air: u16,
    pub cave_air: u16,
    pub void_air: u16,
    /// Number of block states (global palette bits = ceillog2).
    pub block_state_count: u32,
    /// Number of biomes in the registry (global biome palette bits = ceillog2); used for non-uniform biomes.
    #[allow(dead_code)]
    pub biome_count: u32,
}
impl Palette {
    pub fn is_air(&self, s: u16) -> bool {
        s == self.air || s == self.cave_air || s == self.void_air
    }
    fn global_block_bits(&self) -> u32 {
        ceillog2(self.block_state_count)
    }
    #[allow(dead_code)]
    fn global_biome_bits(&self) -> u32 {
        ceillog2(self.biome_count)
    }
}

fn ceillog2(v: u32) -> u32 {
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

#[derive(Clone)]
pub struct Section {
    pub blocks: Blocks,
    /// Biome of the whole section (ID in the `minecraft:worldgen/biome` registry). The 4x4x4 grid is uniform for now.
    pub biome: u16,
}

impl Section {
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
}

#[derive(Clone)]
pub struct Chunk {
    pub sections: Vec<Section>,
    /// Cached packet frame (including coordinates); invalidated when a block changes.
    cached: Option<Arc<Vec<u8>>>,
}

impl Chunk {
    pub fn get(&self, dim: &Dimension, x: usize, y: i32, z: usize) -> u16 {
        let sy = (y - dim.min_y) as usize;
        self.sections[sy >> 4].get(x, sy & 15, z)
    }
}

/// Context needed to build a packet.
pub struct EncodeCtx<'a> {
    pub dim: Dimension,
    pub palette: &'a Palette,
}

// ------------------------------------------------------------------ kodowanie

/// Packs values into `long`s like `SimpleBitStorage` (values never straddle a long boundary).
fn pack_bits(values: impl Iterator<Item = u32>, count: usize, bits: u32) -> Vec<i64> {
    let per_long = (64 / bits) as usize;
    let mut out = vec![0i64; count.div_ceil(per_long)];
    for (i, v) in values.enumerate() {
        out[i / per_long] |= ((v as u64) << ((i % per_long) as u32 * bits)) as i64;
    }
    out
}

/// `front`: a state placed at the start of the palette if present. A vanilla section starts as a palette
/// holding only air, so after more blocks are added air has index 0.
fn write_container(w: &mut Vec<u8>, values: &[u16], bits_cfg: ContainerKind, global_bits: u32, front: Option<u16>) {
    let mut palette: Vec<u16> = Vec::new();
    let mut overflow = false;
    let max_pal = bits_cfg.max_indirect_entries();
    for &v in values {
        if !palette.contains(&v) {
            if palette.len() == max_pal {
                overflow = true;
                break;
            }
            palette.push(v);
        }
    }
    if !overflow && palette.len() == 1 {
        w.put_u8(0);
        w.put_varint(palette[0] as i32);
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
    if let Some(f) = front {
        if let Some(pos) = palette.iter().position(|&p| p == f) {
            let v = palette.remove(pos);
            palette.insert(0, v);
        }
    }
    let bits = bits_cfg.indirect_bits(palette.len());
    w.put_u8(bits as u8);
    w.put_varint(palette.len() as i32);
    for p in &palette {
        w.put_varint(*p as i32);
    }
    let idx: Vec<u32> = values.iter().map(|v| palette.iter().position(|p| p == v).unwrap() as u32).collect();
    let longs = pack_bits(idx.into_iter(), values.len(), bits);
    w.put_long_array(&longs);
}

#[allow(dead_code)]
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
    w.put_i16(non_empty);
    match &s.blocks {
        Blocks::Uniform(v) => {
            w.put_u8(0);
            w.put_varint(*v as i32);
            w.put_varint(0);
        }
        Blocks::Full(b) => write_container(w, &b[..], ContainerKind::States, p.global_block_bits(), Some(p.air)),
    }
    // biomes: 64 cells (4x4x4), uniform here
    w.put_u8(0);
    w.put_varint(s.biome as i32);
    w.put_varint(0);
}

fn heightmap(chunk: &Chunk, dim: &Dimension, p: &Palette) -> Vec<i64> {
    // WORLD_SURFACE and MOTION_BLOCKING: for superflat (only full blocks) "first non-air from the top" is enough.
    let mut vals = [0u32; 256];
    for z in 0..16usize {
        for x in 0..16usize {
            let mut h = 0;
            'col: for sy in (0..dim.sections()).rev() {
                let sec = &chunk.sections[sy];
                if let Blocks::Uniform(v) = sec.blocks {
                    if p.is_air(v) {
                        continue;
                    }
                }
                for ly in (0..16usize).rev() {
                    if !p.is_air(sec.get(x, ly, z)) {
                        h = (sy * 16 + ly + 1) as u32;
                        break 'col;
                    }
                }
            }
            vals[x + z * 16] = h;
        }
    }
    let bits = ceillog2(dim.height as u32 + 1);
    pack_bits(vals.iter().copied(), 256, bits)
}

/// Simple sky light propagation: 15 above the highest opaque block of a column, 0 below it.
/// (The full vanilla light engine: see the roadmap.)
fn skylight(chunk: &Chunk, dim: &Dimension, p: &Palette) -> LightData {
    let n = dim.light_sections();
    let mut light = LightData::default();
    light.sky_mask = vec![0; n.div_ceil(64)];
    light.empty_sky_mask = vec![0; n.div_ceil(64)];
    light.block_mask = vec![0; n.div_ceil(64)];
    light.empty_block_mask = vec![0; n.div_ceil(64)];
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
    let top_nonempty = (0..dim.sections()).rev().find(|&sy| {
        let sec = &chunk.sections[sy];
        match &sec.blocks {
            Blocks::Uniform(v) => !p.is_air(*v),
            Blocks::Full(b) => b.iter().any(|&v| !p.is_air(v)),
        }
    });
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

/// Builds the `level_chunk_with_light` packet payload for chunk (cx, cz).
pub fn build_chunk_payload(chunk: &Chunk, cx: i32, cz: i32, ctx: &EncodeCtx<'_>) -> Vec<u8> {
    let mut sections = Vec::with_capacity(8192);
    for s in &chunk.sections {
        encode_section(&mut sections, s, ctx.palette);
    }
    let hm = heightmap(chunk, &ctx.dim, ctx.palette);
    let mut hmc = Compound::new();
    // The client only reads WORLD_SURFACE and MOTION_BLOCKING (`Heightmap.Types.sendToClient`).
    hmc.put("MOTION_BLOCKING", Tag::LongArray(hm.clone()));
    hmc.put("WORLD_SURFACE", Tag::LongArray(hm));
    let light = skylight(chunk, &ctx.dim, ctx.palette);
    play::level_chunk_with_light(cx, cz, &hmc, &sections, &[], &light)
}

// -------------------------------------------------------------------- world

pub struct World {
    pub seed: i64,
    pub dim: Dimension,
    pub palette: Palette,
    template: Arc<Chunk>,
    modified: HashMap<(i32, i32), Chunk>,
    pub spawn: (i32, i32, i32),
}

impl World {
    /// Superflat: bedrock, 2x dirt, grass_block from `min_y` (the vanilla default preset), plains biome.
    pub fn flat(dim: Dimension, palette: Palette, layers: &[u16], biome: u16, seed: i64) -> Self {
        let mut sections: Vec<Section> = (0..dim.sections()).map(|_| Section { blocks: Blocks::Uniform(palette.air), biome }).collect();
        for (i, &state) in layers.iter().enumerate() {
            let sy = i; // fewer than 16 layers: everything is in section 0
            for z in 0..16 {
                for x in 0..16 {
                    sections[sy >> 4].set(x, sy & 15, z, state);
                }
            }
        }
        let spawn_y = dim.min_y + layers.len() as i32;
        Self { seed, dim, palette, template: Arc::new(Chunk { sections, cached: None }), modified: HashMap::new(), spawn: (0, spawn_y, 0) }
    }

    /// `BiomeManager.obfuscateSeed`: Guava `Hashing.sha256().hashLong(seed).asLong()`
    /// (SHA-256 of the 8 little-endian bytes, the first 8 bytes read as a little-endian long).
    pub fn hashed_seed(&self) -> i64 {
        use sha2::{Digest, Sha256};
        let h = Sha256::digest(self.seed.to_le_bytes());
        i64::from_le_bytes(h[..8].try_into().unwrap())
    }

    /// Highest non-air block + 1 in a column (the equivalent of `PlayerRespawnLogic.getOverworldRespawnPos`
    /// for a world without fluids/leaves); `None` when the column is empty.
    pub fn surface_y(&self, x: i32, z: i32) -> Option<i32> {
        let ch = self.chunk(x >> 4, z >> 4);
        for sy in (0..self.dim.sections()).rev() {
            for ly in (0..16usize).rev() {
                if !self.palette.is_air(ch.sections[sy].get((x & 15) as usize, ly, (z & 15) as usize)) {
                    return Some(self.dim.min_y + (sy * 16 + ly) as i32 + 1);
                }
            }
        }
        None
    }

    pub fn chunk(&self, cx: i32, cz: i32) -> &Chunk {
        self.modified.get(&(cx, cz)).unwrap_or(&self.template)
    }

    pub fn get_block(&self, x: i32, y: i32, z: i32) -> u16 {
        if y < self.dim.min_y || y >= self.dim.min_y + self.dim.height {
            return self.palette.void_air;
        }
        self.chunk(x >> 4, z >> 4).get(&self.dim, (x & 15) as usize, y, (z & 15) as usize)
    }

    /// Sets a block; returns the previous state. Outside the world: no change.
    pub fn set_block(&mut self, x: i32, y: i32, z: i32, state: u16) -> Option<u16> {
        if y < self.dim.min_y || y >= self.dim.min_y + self.dim.height {
            return None;
        }
        let key = (x >> 4, z >> 4);
        let template = self.template.clone();
        let chunk = self.modified.entry(key).or_insert_with(|| (*template).clone());
        let sy = (y - self.dim.min_y) as usize;
        let old = chunk.sections[sy >> 4].get((x & 15) as usize, sy & 15, (z & 15) as usize);
        chunk.sections[sy >> 4].set((x & 15) as usize, sy & 15, (z & 15) as usize, state);
        chunk.cached = None;
        Some(old)
    }

    /// A ready-to-send frame (compressed), cached per chunk.
    pub fn chunk_frame(&mut self, cx: i32, cz: i32, enc: &mut FrameEncoder) -> Arc<Vec<u8>> {
        if let Some(c) = self.modified.get(&(cx, cz)) {
            if let Some(f) = &c.cached {
                return f.clone();
            }
        }
        let ctx = EncodeCtx { dim: self.dim, palette: &self.palette };
        let payload = build_chunk_payload(self.chunk(cx, cz), cx, cz, &ctx);
        let frame = Arc::new(enc.encode(&payload));
        if let Some(c) = self.modified.get_mut(&(cx, cz)) {
            c.cached = Some(frame.clone());
        }
        frame
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pal() -> Palette {
        Palette { air: 0, cave_air: 10000, void_air: 10001, block_state_count: 26684, biome_count: 64 }
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
    fn heightmap_bits_for_384() {
        // Mth.ceillog2(384 + 1) = 9 -> 7 values per long -> 37 longs
        let dim = Dimension { min_y: -64, height: 384 };
        let w = World::flat(dim, pal(), &[7, 10, 10, 9], 1, 12345);
        let hm = heightmap(w.chunk(0, 0), &dim, &pal());
        assert_eq!(hm.len(), 37);
        // height 4 in every column: 7 values of 9 bits in the first long
        let expect: u64 = (0..7).fold(0u64, |a, i| a | (4u64 << (i * 9)));
        assert_eq!(hm[0] as u64, expect);
    }

    #[test]
    fn flat_world_blocks_and_light() {
        let dim = Dimension { min_y: -64, height: 384 };
        let w = World::flat(dim, pal(), &[7, 10, 10, 9], 1, 12345);
        assert_eq!(w.get_block(5, -64, 5), 7);
        assert_eq!(w.get_block(5, -61, 5), 9);
        assert_eq!(w.get_block(5, -60, 5), 0);
        assert_eq!(w.spawn, (0, -60, 0));
        let l = skylight(w.chunk(0, 0), &dim, &pal());
        // as measured on the vanilla server: sky_mask={1,2}, empty_sky={0}, empty_block={0,1,2}
        assert_eq!(l.sky_mask[0], 0b110);
        assert_eq!(l.empty_sky_mask[0], 0b001);
        assert_eq!(l.block_mask[0], 0);
        assert_eq!(l.empty_block_mask[0], 0b111);
        assert_eq!(l.sky.len(), 2);
        // in section 1 the first 4 layers (y -64..-61) are dark, the higher ones are lit
        let arr = &l.sky[0];
        assert_eq!(arr[(3 << 8) >> 1] & 0x0F, 0); // y=3 (−61) ciemne
        assert_eq!(arr[(4 << 8) >> 1] & 0x0F, 0x0F); // y=4 (−60) jasne
    }

    #[test]
    fn paletted_container_roundtrip_shape() {
        let mut vals = [0u16; 4096];
        vals[0] = 7;
        vals[1] = 9;
        let mut w = Vec::new();
        write_container(&mut w, &vals, ContainerKind::States, 15, None);
        // 3 palette entries -> 4 bits, linear palette: bits(1) + len(1) + 3 + array length(2) + 256 longs
        assert_eq!(w[0], 4);
        assert_eq!(w[1], 3);
        let mut r = mc_protocol::Reader::new(&w[2..]);
        assert_eq!((r.varint().unwrap(), r.varint().unwrap(), r.varint().unwrap()), (7, 9, 0)); // order of first appearance
        assert_eq!(r.varint().unwrap(), 256);
    }

    /// Value measured on the real vanilla 1.21.1 server (Login packet, `hashedSeed` field) for `level-seed=12345`.
    #[test]
    fn hashed_seed_matches_vanilla() {
        let dim = Dimension { min_y: -64, height: 384 };
        let w = World::flat(dim, pal(), &[7, 10, 10, 9], 1, 12345);
        assert_eq!(w.hashed_seed(), 293737985876514017);
        assert_eq!(w.surface_y(3, 3), Some(-60));
    }

    #[test]
    fn set_block_creates_override_and_invalidates_cache() {
        let dim = Dimension { min_y: -64, height: 384 };
        let mut w = World::flat(dim, pal(), &[7, 10, 10, 9], 1, 12345);
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
}
