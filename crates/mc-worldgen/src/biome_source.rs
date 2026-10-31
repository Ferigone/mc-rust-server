//! The biome sources of the vanilla dimensions: `MultiNoiseBiomeSource` (overworld, nether), `TheEndBiomeSource` and `FixedBiomeSource`.

use crate::biomes as b;
use crate::climate::{ParameterList, Sampler};
use crate::density::Ctx;
use crate::overworld_biomes::{nether_list, OverworldBiomeBuilder};

/// `BiomeSource`
pub enum BiomeSource {
    MultiNoise(ParameterList<&'static str>),
    End,
    Fixed(&'static str),
}

impl BiomeSource {
    /// `multi_noise` with the `minecraft:overworld` preset
    pub fn overworld() -> Self {
        BiomeSource::MultiNoise(OverworldBiomeBuilder::overworld_list())
    }

    /// `multi_noise` with the `minecraft:nether` preset
    pub fn nether() -> Self {
        BiomeSource::MultiNoise(nether_list())
    }

    /// `BiomeSource.possibleBiomes()`: the distinct biomes in order of first appearance.
    pub fn possible_biomes(&self) -> Vec<&'static str> {
        let all: Vec<&'static str> = match self {
            BiomeSource::MultiNoise(list) => list.values().iter().map(|(_, b)| *b).collect(),
            BiomeSource::End => vec![b::THE_END, b::END_HIGHLANDS, b::END_MIDLANDS, b::SMALL_END_ISLANDS, b::END_BARRENS],
            BiomeSource::Fixed(biome) => vec![*biome],
        };
        let mut seen = std::collections::HashSet::new();
        all.into_iter().filter(|b| seen.insert(*b)).collect()
    }

    /// `getNoiseBiome(quartX, quartY, quartZ, sampler)` (a biome cell is 4x4x4 blocks)
    pub fn get_noise_biome(&self, qx: i32, qy: i32, qz: i32, sampler: &Sampler) -> &'static str {
        match self {
            BiomeSource::MultiNoise(list) => list.find_value(&sampler.sample(qx, qy, qz)),
            BiomeSource::Fixed(b) => b,
            BiomeSource::End => {
                let (bx, by, bz) = (qx << 2, qy << 2, qz << 2);
                let (sx, sz) = (bx >> 4, bz >> 4);
                if (sx as i64) * (sx as i64) + (sz as i64) * (sz as i64) <= 4096 {
                    return b::THE_END;
                }
                let x = sx.wrapping_mul(2).wrapping_add(1).wrapping_mul(8);
                let z = sz.wrapping_mul(2).wrapping_add(1).wrapping_mul(8);
                let erosion = sampler.erosion.compute(&Ctx::point(x, by, z));
                if erosion > 0.25 {
                    b::END_HIGHLANDS
                } else if erosion >= -0.0625 {
                    b::END_MIDLANDS
                } else if erosion < -0.21875 {
                    b::SMALL_END_ISLANDS
                } else {
                    b::END_BARRENS
                }
            }
        }
    }
}
