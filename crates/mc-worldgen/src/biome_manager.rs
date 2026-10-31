//! `BiomeManager`: the "voronoi zoom" that turns the 4x4x4 biome cells into per-block biomes, fiddled with a hash of the world seed.

use sha2::{Digest, Sha256};

/// `BiomeManager.obfuscateSeed`: the first 8 bytes (little endian) of SHA-256 over the seed as a little-endian long
pub fn obfuscate_seed(seed: i64) -> i64 {
    let hash = Sha256::digest(seed.to_le_bytes());
    i64::from_le_bytes(hash[0..8].try_into().unwrap())
}

/// `LinearCongruentialGenerator.next`
fn lcg_next(seed: i64, salt: i64) -> i64 {
    let s = seed.wrapping_mul(seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407));
    s.wrapping_add(salt)
}

fn fiddle(v: i64) -> f64 {
    let d = ((v >> 24).rem_euclid(1024)) as f64 / 1024.0;
    (d - 0.5) * 0.9
}

fn fiddled_distance(seed: i64, x: i32, y: i32, z: i32, dx: f64, dy: f64, dz: f64) -> f64 {
    let mut s = lcg_next(seed, x as i64);
    s = lcg_next(s, y as i64);
    s = lcg_next(s, z as i64);
    s = lcg_next(s, x as i64);
    s = lcg_next(s, y as i64);
    s = lcg_next(s, z as i64);
    let fx = fiddle(s);
    s = lcg_next(s, seed);
    let fy = fiddle(s);
    s = lcg_next(s, seed);
    let fz = fiddle(s);
    let sq = |v: f64| v * v;
    sq(dz + fz) + sq(dy + fy) + sq(dx + fx)
}

/// `BiomeManager.getBiome(BlockPos)`: the quart position (`x >> 2`, ...) whose biome applies to the block.
pub fn zoomed_quart(biome_zoom_seed: i64, x: i32, y: i32, z: i32) -> (i32, i32, i32) {
    let (px, py, pz) = (x.wrapping_sub(2), y.wrapping_sub(2), z.wrapping_sub(2));
    let (qx, qy, qz) = (px >> 2, py >> 2, pz >> 2);
    let (fx, fy, fz) = ((px & 3) as f64 / 4.0, (py & 3) as f64 / 4.0, (pz & 3) as f64 / 4.0);
    let mut best = 0;
    let mut best_d = f64::INFINITY;
    for i in 0..8 {
        let (ox, oy, oz) = (i & 4 == 0, i & 2 == 0, i & 1 == 0);
        let cx = if ox { qx } else { qx + 1 };
        let cy = if oy { qy } else { qy + 1 };
        let cz = if oz { qz } else { qz + 1 };
        let d = fiddled_distance(
            biome_zoom_seed,
            cx,
            cy,
            cz,
            if ox { fx } else { fx - 1.0 },
            if oy { fy } else { fy - 1.0 },
            if oz { fz } else { fz - 1.0 },
        );
        if best_d > d {
            best = i;
            best_d = d;
        }
    }
    (if best & 4 == 0 { qx } else { qx + 1 }, if best & 2 == 0 { qy } else { qy + 1 }, if best & 1 == 0 { qz } else { qz + 1 })
}

/// `BiomeManager`: per-block biomes on top of a source of 4x4x4 biome cells.
pub struct BiomeManager<'a> {
    biome_zoom_seed: i64,
    source: &'a dyn Fn(i32, i32, i32) -> &'static str,
}

impl<'a> BiomeManager<'a> {
    /// `source(quartX, quartY, quartZ)` is `NoiseBiomeSource.getNoiseBiome`; `world_seed` is the plain world seed.
    pub fn new(source: &'a dyn Fn(i32, i32, i32) -> &'static str, world_seed: i64) -> Self {
        Self { biome_zoom_seed: obfuscate_seed(world_seed), source }
    }

    /// `getBiome(BlockPos)`
    pub fn get_biome(&self, x: i32, y: i32, z: i32) -> &'static str {
        let (qx, qy, qz) = zoomed_quart(self.biome_zoom_seed, x, y, z);
        (self.source)(qx, qy, qz)
    }
}

/// The biome cells of one chunk (`ChunkAccess.fillBiomesFromNoise`), filled in the order of the vanilla server.
pub struct ChunkBiomes {
    min_quart_y: i32,
    quart_height: i32,
    /// section by section, `x << 4 | y << 2 | z` inside a section
    cells: Vec<&'static str>,
}

impl ChunkBiomes {
    /// Fills the chunk (`min_y` and `height` in blocks) from `get_noise_biome(quartX, quartY, quartZ)`.
    pub fn fill(chunk_x: i32, chunk_z: i32, min_y: i32, height: i32, mut get_noise_biome: impl FnMut(i32, i32, i32) -> &'static str) -> Self {
        let (qx0, qz0) = (chunk_x * 4, chunk_z * 4);
        let sections = height >> 4;
        let mut cells = Vec::with_capacity((sections * 64) as usize);
        for s in 0..sections {
            let qy0 = (min_y >> 2) + s * 4;
            for x in 0..4 {
                for y in 0..4 {
                    for z in 0..4 {
                        cells.push(get_noise_biome(qx0 + x, qy0 + y, qz0 + z));
                    }
                }
            }
        }
        Self { min_quart_y: min_y >> 2, quart_height: height >> 2, cells }
    }

    /// Every distinct biome of the chunk (`PalettedContainer.getAll`).
    pub fn all_biomes(&self) -> Vec<&'static str> {
        let mut seen = std::collections::HashSet::new();
        self.cells.iter().copied().filter(|b| seen.insert(*b)).collect()
    }

    /// `ChunkAccess.getNoiseBiome(quartX, quartY, quartZ)`: the quart y is clamped to the chunk's range.
    pub fn get_noise_biome(&self, qx: i32, qy: i32, qz: i32) -> &'static str {
        let y = qy.clamp(self.min_quart_y, self.min_quart_y + self.quart_height - 1) - self.min_quart_y;
        let (section, ly) = (y >> 2, y & 3);
        self.cells[(section * 64 + (qx & 3) * 16 + ly * 4 + (qz & 3)) as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lcg_matches_the_java_formula() {
        // x * (x * 6364136223846793005 + 1442695040888963407) + salt
        assert_eq!(lcg_next(0, 5), 5);
        assert_eq!(lcg_next(1, 0), 6364136223846793005i64.wrapping_add(1442695040888963407));
    }

    #[test]
    fn fiddle_stays_in_range() {
        for v in [i64::MIN, -1, 0, 1, 1 << 24, i64::MAX, 0x1234_5678_9ABC_DEF0] {
            let f = fiddle(v);
            assert!((-0.45..=0.45).contains(&f), "{f}");
        }
    }
}
