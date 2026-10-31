//! Placed features (`PlacedFeature`) and the placement modifiers (`net.minecraft.world.level.levelgen.placement`).

use crate::carver::HeightProvider;
use crate::feature::{Env, Feature, IntProvider};
use crate::random::RandomSource;
use crate::region::Region;
use crate::surface::GenContext;
use anyhow::{anyhow, bail, Result};
use serde_json::Value;

/// A `PlacementModifier` the port knows.
#[derive(Clone)]
pub enum Modifier {
    /// `count`: the position is repeated `count` times
    Count(IntProvider),
    /// `in_square`: a random column of the chunk
    InSquare,
    /// `height_range`
    HeightRange(HeightProvider),
    /// `biome`: only where the biome at the position has this placed feature
    Biome,
    /// `rarity_filter`: on average once every `chance` times
    RarityFilter(i32),
}

impl Modifier {
    pub fn parse(v: &Value) -> Result<Self> {
        match v.get("type").and_then(Value::as_str) {
            Some("minecraft:count") => Ok(Self::Count(IntProvider::parse(&v["count"])?)),
            Some("minecraft:in_square") => Ok(Self::InSquare),
            Some("minecraft:height_range") => Ok(Self::HeightRange(HeightProvider::parse(&v["height"])?)),
            Some("minecraft:biome") => Ok(Self::Biome),
            Some("minecraft:rarity_filter") => Ok(Self::RarityFilter(v["chance"].as_i64().ok_or_else(|| anyhow!("rarity_filter without chance"))? as i32)),
            other => bail!("unsupported placement modifier {other:?}"),
        }
    }
}

/// What the placement needs besides the region and the random source.
pub struct PlaceCtx<'a> {
    /// `WorldGenerationContext` (anchors of the height providers)
    pub range: GenContext,
    /// `BiomeGenerationSettings.hasFeature(topFeature)`: does the biome have the placed feature with this key?
    pub biome_has_feature: &'a dyn Fn(&str, &str) -> bool,
}

/// A placed feature the port can place.
#[derive(Clone)]
pub struct PlacedFeature {
    pub name: String,
    /// the canonical JSON (what makes two entries equal)
    pub key: String,
    pub modifiers: Vec<Modifier>,
    pub feature: Feature,
}

impl PlacedFeature {
    /// `json` is the `placed_feature` entry; an error means the port cannot place it (yet).
    pub fn parse(env: &Env, name: &str, json: &Value) -> Result<Self> {
        let feature = match &json["feature"] {
            Value::String(f) => {
                let cf = env.configured.get(f).ok_or_else(|| anyhow!("unknown configured feature {f}"))?;
                Feature::parse(env, cf)?
            }
            inline @ Value::Object(_) => Feature::parse(env, inline)?,
            other => bail!("bad feature reference {other}"),
        };
        let modifiers = json["placement"].as_array().ok_or_else(|| anyhow!("placed feature without placement"))?.iter().map(Modifier::parse).collect::<Result<Vec<_>>>()?;
        Ok(Self { name: name.to_owned(), key: serde_json::to_string(json)?, modifiers, feature })
    }

    /// `PlacedFeature.placeWithBiomeCheck`: the modifiers are applied position by position (depth first, like the lazy streams of the
    /// Java code), so the random draws of a modifier and of the feature interleave exactly as in vanilla.
    pub fn place_with_biome_check(&self, region: &mut Region, ctx: &PlaceCtx, r: &mut dyn RandomSource, origin: (i32, i32, i32)) -> bool {
        self.run(0, region, ctx, r, origin)
    }

    fn run(&self, idx: usize, region: &mut Region, ctx: &PlaceCtx, r: &mut dyn RandomSource, pos: (i32, i32, i32)) -> bool {
        let Some(m) = self.modifiers.get(idx) else {
            return self.feature.place(region, r, pos);
        };
        match m {
            Modifier::Count(p) => {
                let n = p.sample(r);
                let mut placed = false;
                for _ in 0..n.max(0) {
                    placed |= self.run(idx + 1, region, ctx, r, pos);
                }
                placed
            }
            Modifier::InSquare => {
                let x = r.next_int_bound(16) + pos.0;
                let z = r.next_int_bound(16) + pos.2;
                self.run(idx + 1, region, ctx, r, (x, pos.1, z))
            }
            Modifier::HeightRange(h) => {
                let y = h.sample(r, ctx.range);
                self.run(idx + 1, region, ctx, r, (pos.0, y, pos.2))
            }
            Modifier::Biome => {
                let biome = region.biome_at(pos.0, pos.1, pos.2);
                if (ctx.biome_has_feature)(biome, &self.key) {
                    self.run(idx + 1, region, ctx, r, pos)
                } else {
                    false
                }
            }
            Modifier::RarityFilter(chance) => {
                if r.next_float() < 1.0f32 / *chance as f32 {
                    self.run(idx + 1, region, ctx, r, pos)
                } else {
                    false
                }
            }
        }
    }
}
