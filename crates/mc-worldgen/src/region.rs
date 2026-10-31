//! `WorldGenRegion` for the feature stage: the 3x3 chunks around the chunk being decorated, readable and (within one chunk of the centre)
//! writable, with the heightmaps `ProtoChunk.setBlockState` keeps up to date.

use crate::surface::BlockQuery;

/// `Heightmap.Types`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeightmapKind {
    WorldSurfaceWg,
    OceanFloorWg,
    WorldSurface,
    OceanFloor,
    MotionBlocking,
    MotionBlockingNoLeaves,
}

const KINDS: usize = 6;

impl HeightmapKind {
    fn index(self) -> usize {
        self as usize
    }

    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "WORLD_SURFACE_WG" => Self::WorldSurfaceWg,
            "OCEAN_FLOOR_WG" => Self::OceanFloorWg,
            "WORLD_SURFACE" => Self::WorldSurface,
            "OCEAN_FLOOR" => Self::OceanFloor,
            "MOTION_BLOCKING" => Self::MotionBlocking,
            "MOTION_BLOCKING_NO_LEAVES" => Self::MotionBlockingNoLeaves,
            _ => return None,
        })
    }

    /// The predicate that makes a block count for the heightmap.
    fn matches(self, q: &dyn BlockQuery, state: u16) -> bool {
        match self {
            Self::WorldSurfaceWg | Self::WorldSurface => !q.is_air(state),
            Self::OceanFloorWg | Self::OceanFloor => q.blocks_motion(state),
            Self::MotionBlocking => q.blocks_motion(state) || q.has_fluid(state),
            Self::MotionBlockingNoLeaves => (q.blocks_motion(state) || q.has_fluid(state)) && !q.is_leaves(state),
        }
    }

    /// Whether `ProtoChunk.setBlockState` keeps the heightmap up to date during the feature step (the `_WG` ones are frozen).
    fn live(self) -> bool {
        !matches!(self, Self::WorldSurfaceWg | Self::OceanFloorWg)
    }
}

/// One chunk of the region: blocks `(y - min_y) << 8 | z << 4 | x` and the first free y of every heightmap, `x + z * 16`.
pub struct RegionChunk {
    pub chunk_x: i32,
    pub chunk_z: i32,
    pub blocks: Vec<u16>,
    heightmaps: [[i32; 256]; KINDS],
}

impl RegionChunk {
    /// Primes all heightmaps from the blocks (`Heightmap.primeHeightmaps`; the `_WG` ones hold the state of the end of the carver step).
    pub fn new(chunk_x: i32, chunk_z: i32, min_y: i32, height: i32, blocks: Vec<u16>, q: &dyn BlockQuery) -> Self {
        let kinds = [
            HeightmapKind::WorldSurfaceWg,
            HeightmapKind::OceanFloorWg,
            HeightmapKind::WorldSurface,
            HeightmapKind::OceanFloor,
            HeightmapKind::MotionBlocking,
            HeightmapKind::MotionBlockingNoLeaves,
        ];
        let mut heightmaps = [[min_y; 256]; KINDS];
        for z in 0..16usize {
            for x in 0..16usize {
                for kind in kinds {
                    for y in (0..height).rev() {
                        if kind.matches(q, blocks[((y as usize) << 8) | (z << 4) | x]) {
                            heightmaps[kind.index()][x + z * 16] = min_y + y + 1;
                            break;
                        }
                    }
                }
            }
        }
        Self { chunk_x, chunk_z, blocks, heightmaps }
    }
}

/// The region.
pub struct Region<'a> {
    pub center: (i32, i32),
    pub min_y: i32,
    pub height: i32,
    /// `generatingStep.blockStateWriteRadius()`
    pub write_radius: i32,
    chunks: Vec<RegionChunk>,
    q: &'a dyn BlockQuery,
    biome_fn: &'a dyn Fn(i32, i32, i32) -> &'static str,
    air: u16,
    void_air: u16,
}

impl<'a> Region<'a> {
    pub fn new(
        center: (i32, i32),
        min_y: i32,
        height: i32,
        chunks: Vec<RegionChunk>,
        q: &'a dyn BlockQuery,
        biome_fn: &'a dyn Fn(i32, i32, i32) -> &'static str,
        air: u16,
        void_air: u16,
    ) -> Self {
        Self { center, min_y, height, write_radius: 1, chunks, q, biome_fn, air, void_air }
    }

    pub fn query(&self) -> &dyn BlockQuery {
        self.q
    }

    fn chunk_index(&self, cx: i32, cz: i32) -> Option<usize> {
        self.chunks.iter().position(|c| c.chunk_x == cx && c.chunk_z == cz)
    }

    /// The chunks back, e.g. to store them.
    pub fn into_chunks(self) -> Vec<RegionChunk> {
        self.chunks
    }

    pub fn chunk(&self, cx: i32, cz: i32) -> Option<&RegionChunk> {
        self.chunk_index(cx, cz).map(|i| &self.chunks[i])
    }

    pub fn is_outside_build_height(&self, y: i32) -> bool {
        y < self.min_y || y >= self.min_y + self.height
    }

    /// `getBlockState`: `void_air` outside the build height; the chunks beyond the 3x3 area are only at the `STRUCTURE_STARTS` status during
    /// the feature step: all air
    pub fn get_block(&self, x: i32, y: i32, z: i32) -> u16 {
        if self.is_outside_build_height(y) {
            return self.void_air;
        }
        match self.chunk(x >> 4, z >> 4) {
            Some(c) => c.blocks[(((y - self.min_y) as usize) << 8) | (((z & 15) << 4) | (x & 15)) as usize],
            None => self.air,
        }
    }

    /// `WorldGenRegion.ensureCanWrite`
    pub fn ensure_can_write(&self, x: i32, z: i32) -> bool {
        (self.center.0 - (x >> 4)).abs() <= self.write_radius && (self.center.1 - (z >> 4)).abs() <= self.write_radius
    }

    /// `WorldGenRegion.setBlock` -> `ProtoChunk.setBlockState`: the live heightmaps are updated like `Heightmap.update`. Returns whether the block was written.
    pub fn set_block(&mut self, x: i32, y: i32, z: i32, state: u16) -> bool {
        if !self.ensure_can_write(x, z) {
            return false;
        }
        if self.is_outside_build_height(y) {
            return true; // ProtoChunk answers VOID_AIR without changing anything
        }
        self.write(x, y, z, state, true);
        true
    }

    /// `LevelChunkSection.setBlockState` directly (bulk writers): no heightmap update. The caller checked `ensure_can_write`.
    pub fn set_block_raw(&mut self, x: i32, y: i32, z: i32, state: u16) {
        self.write(x, y, z, state, false);
    }

    fn write(&mut self, x: i32, y: i32, z: i32, state: u16, update_heightmaps: bool) {
        let min_y = self.min_y;
        let i = self.chunk_index(x >> 4, z >> 4).unwrap_or_else(|| panic!("chunk of ({x}, {y}, {z}) is outside of the region"));
        let q = self.q;
        let block_index = (((y - min_y) as usize) << 8) | (((z & 15) << 4) | (x & 15)) as usize;
        self.chunks[i].blocks[block_index] = state;
        if !update_heightmaps {
            return;
        }
        let col = ((x & 15) + (z & 15) * 16) as usize;
        for kind in [HeightmapKind::WorldSurface, HeightmapKind::OceanFloor, HeightmapKind::MotionBlocking, HeightmapKind::MotionBlockingNoLeaves] {
            debug_assert!(kind.live());
            let chunk = &mut self.chunks[i];
            let first_free = chunk.heightmaps[kind.index()][col];
            if y <= first_free - 2 {
                continue;
            }
            if kind.matches(q, state) {
                if y >= first_free {
                    chunk.heightmaps[kind.index()][col] = y + 1;
                }
            } else if first_free - 1 == y {
                let mut found = min_y;
                let mut yy = y - 1;
                while yy >= min_y {
                    let s = chunk.blocks[(((yy - min_y) as usize) << 8) | (((z & 15) << 4) | (x & 15)) as usize];
                    if kind.matches(q, s) {
                        found = yy + 1;
                        break;
                    }
                    yy -= 1;
                }
                chunk.heightmaps[kind.index()][col] = found;
            }
        }
    }

    /// `WorldGenLevel.getHeight(type, x, z)`: the y of the first free block above the column (`ChunkAccess.getHeight + 1`)
    pub fn height(&self, kind: HeightmapKind, x: i32, z: i32) -> i32 {
        match self.chunk(x >> 4, z >> 4) {
            Some(c) => c.heightmaps[kind.index()][((x & 15) + (z & 15) * 16) as usize],
            None => self.min_y,
        }
    }

    /// `WorldGenLevel.getBiome(pos)` (the zoomed biome)
    pub fn biome_at(&self, x: i32, y: i32, z: i32) -> &'static str {
        (self.biome_fn)(x, y, z)
    }
}
