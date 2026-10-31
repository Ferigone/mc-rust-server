//! `RandomState` (the seeded noise instances and the wired `NoiseRouter`), the noise settings and the loading of the worldgen JSON.

use crate::density::{Blended, DensityRegistry, Df, Mapper, Node, NoiseHolder, Visitor, end_island_noise};
use crate::noise::{BlendedNoise, NormalNoise};
use crate::random::{LegacyRandomSource, PositionalRandomFactory, RandomSource, XoroshiroRandomSource};
use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// `NormalNoise.NoiseParameters`
#[derive(Clone, Debug)]
pub struct NoiseParams {
    pub first_octave: i32,
    pub amplitudes: Vec<f64>,
}

/// The worldgen JSON of the data pack (`data/minecraft/worldgen`).
pub struct WorldgenData {
    pub density: DensityRegistry,
    pub noises: HashMap<String, NoiseParams>,
    pub noise_settings: HashMap<String, Value>,
}

fn collect_json(dir: &Path, prefix: &str, out: &mut HashMap<String, Value>) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            collect_json(&path, &format!("{prefix}{name}/"), out)?;
        } else if let Some(stem) = name.strip_suffix(".json") {
            let v: Value = serde_json::from_slice(&std::fs::read(&path)?).with_context(|| path.display().to_string())?;
            out.insert(format!("minecraft:{prefix}{stem}"), v);
        }
    }
    Ok(())
}

impl WorldgenData {
    /// `dir` = `<generated>/data/minecraft/worldgen`
    pub fn load(dir: &Path) -> Result<Self> {
        let (mut df, mut noises, mut settings) = (HashMap::new(), HashMap::new(), HashMap::new());
        collect_json(&dir.join("density_function"), "", &mut df)?;
        collect_json(&dir.join("noise"), "", &mut noises)?;
        collect_json(&dir.join("noise_settings"), "", &mut settings)?;
        let noises = noises
            .into_iter()
            .map(|(k, v)| {
                let params = NoiseParams {
                    first_octave: v["firstOctave"].as_i64().ok_or_else(|| anyhow!("{k}: firstOctave"))? as i32,
                    amplitudes: v["amplitudes"].as_array().ok_or_else(|| anyhow!("{k}: amplitudes"))?.iter().map(|a| a.as_f64().unwrap_or(0.0)).collect(),
                };
                Ok((k, params))
            })
            .collect::<Result<HashMap<_, _>>>()?;
        Ok(Self { density: DensityRegistry::new(df), noises, noise_settings: settings })
    }
}

/// `NoiseSettings`
#[derive(Clone, Copy, Debug)]
pub struct NoiseSettings {
    pub min_y: i32,
    pub height: i32,
    pub size_horizontal: i32,
    pub size_vertical: i32,
}

impl NoiseSettings {
    pub fn cell_width(&self) -> i32 {
        self.size_horizontal << 2
    }
    pub fn cell_height(&self) -> i32 {
        self.size_vertical << 2
    }
}

/// The block states the terrain stage can produce, as IDs of the caller's block state numbering.
#[derive(Clone, Copy, Debug)]
pub struct TerrainBlocks {
    pub air: u16,
    pub lava: u16,
    pub default_block: u16,
    pub default_fluid: u16,
    pub copper_ore: u16,
    pub raw_copper_block: u16,
    pub granite: u16,
    pub deepslate_iron_ore: u16,
    pub raw_iron_block: u16,
    pub tuff: u16,
}

/// `NoiseGeneratorSettings` (the part the terrain stage needs).
pub struct GeneratorSettings {
    pub sea_level: i32,
    pub aquifers_enabled: bool,
    pub ore_veins_enabled: bool,
    pub legacy_random_source: bool,
    pub noise: NoiseSettings,
    pub default_block: Value,
    pub default_fluid: Value,
    pub router: Value,
}

impl GeneratorSettings {
    pub fn parse(v: &Value) -> Result<Self> {
        let n = &v["noise"];
        let int = |x: &Value, k: &str| x[k].as_i64().map(|i| i as i32).ok_or_else(|| anyhow!("missing {k}"));
        Ok(Self {
            sea_level: int(v, "sea_level")?,
            aquifers_enabled: v["aquifers_enabled"].as_bool().unwrap_or(false),
            ore_veins_enabled: v["ore_veins_enabled"].as_bool().unwrap_or(false),
            legacy_random_source: v["legacy_random_source"].as_bool().unwrap_or(false),
            noise: NoiseSettings { min_y: int(n, "min_y")?, height: int(n, "height")?, size_horizontal: int(n, "size_horizontal")?, size_vertical: int(n, "size_vertical")? },
            default_block: v["default_block"].clone(),
            default_fluid: v["default_fluid"].clone(),
            router: v["noise_router"].clone(),
        })
    }
}

/// `NoiseRouter`
pub struct NoiseRouter {
    pub barrier: Df,
    pub fluid_level_floodedness: Df,
    pub fluid_level_spread: Df,
    pub lava: Df,
    pub temperature: Df,
    pub vegetation: Df,
    pub continents: Df,
    pub erosion: Df,
    pub depth: Df,
    pub ridges: Df,
    pub initial_density_without_jaggedness: Df,
    pub final_density: Df,
    pub vein_toggle: Df,
    pub vein_ridged: Df,
    pub vein_gap: Df,
}

impl NoiseRouter {
    pub fn decode(registry: &DensityRegistry, json: &Value) -> Result<Self> {
        let f = |name: &str| registry.decode_holder(json.get(name).ok_or_else(|| anyhow!("noise_router: missing {name}"))?);
        Ok(Self {
            barrier: f("barrier")?,
            fluid_level_floodedness: f("fluid_level_floodedness")?,
            fluid_level_spread: f("fluid_level_spread")?,
            lava: f("lava")?,
            temperature: f("temperature")?,
            vegetation: f("vegetation")?,
            continents: f("continents")?,
            erosion: f("erosion")?,
            depth: f("depth")?,
            ridges: f("ridges")?,
            initial_density_without_jaggedness: f("initial_density_without_jaggedness")?,
            final_density: f("final_density")?,
            vein_toggle: f("vein_toggle")?,
            vein_ridged: f("vein_ridged")?,
            vein_gap: f("vein_gap")?,
        })
    }

    /// `NoiseRouter.mapAll`
    pub fn map_all(&self, v: &mut dyn Visitor) -> NoiseRouter {
        let mut m = Mapper::new(v);
        NoiseRouter {
            barrier: m.map_all(&self.barrier),
            fluid_level_floodedness: m.map_all(&self.fluid_level_floodedness),
            fluid_level_spread: m.map_all(&self.fluid_level_spread),
            lava: m.map_all(&self.lava),
            temperature: m.map_all(&self.temperature),
            vegetation: m.map_all(&self.vegetation),
            continents: m.map_all(&self.continents),
            erosion: m.map_all(&self.erosion),
            depth: m.map_all(&self.depth),
            ridges: m.map_all(&self.ridges),
            initial_density_without_jaggedness: m.map_all(&self.initial_density_without_jaggedness),
            final_density: m.map_all(&self.final_density),
            vein_toggle: m.map_all(&self.vein_toggle),
            vein_ridged: m.map_all(&self.vein_ridged),
            vein_gap: m.map_all(&self.vein_gap),
        }
    }
}

/// `RandomState`: the world seed turned into positional randoms, the noise instances and the wired router.
pub struct RandomState {
    pub seed: i64,
    pub random: PositionalRandomFactory,
    pub aquifer_random: PositionalRandomFactory,
    pub ore_random: PositionalRandomFactory,
    pub router: NoiseRouter,
}

struct Wiring<'a> {
    random: &'a PositionalRandomFactory,
    noises: &'a HashMap<String, NoiseParams>,
    cache: &'a RefCell<HashMap<String, Arc<NormalNoise>>>,
    legacy: bool,
    seed: i64,
}

impl Wiring<'_> {
    fn instance(&self, key: &str) -> Arc<NormalNoise> {
        if let Some(n) = self.cache.borrow().get(key) {
            return n.clone();
        }
        let p = self.noises.get(key).unwrap_or_else(|| panic!("unknown noise {key}"));
        let n = Arc::new(NormalNoise::create(&mut *self.random.from_hash_of(key), p.first_octave, &p.amplitudes));
        self.cache.borrow_mut().insert(key.to_owned(), n.clone());
        n
    }
}

impl Visitor for Wiring<'_> {
    fn visit_noise(&mut self, h: &NoiseHolder) -> NoiseHolder {
        if self.legacy {
            // pre-1.18 worlds: the legacy nether biome noises for temperature / vegetation, a constant shift
            let legacy_instance = |offset: i64| LegacyRandomSource::new(self.seed.wrapping_add(offset));
            if h.key == "minecraft:temperature" {
                let n = NormalNoise::create_legacy_nether_biome(&mut legacy_instance(0), -7, &[1.0, 1.0]);
                return NoiseHolder { key: h.key.clone(), noise: Some(Arc::new(n)) };
            }
            if h.key == "minecraft:vegetation" {
                let n = NormalNoise::create_legacy_nether_biome(&mut legacy_instance(1), -7, &[1.0, 1.0]);
                return NoiseHolder { key: h.key.clone(), noise: Some(Arc::new(n)) };
            }
            if h.key == "minecraft:offset" {
                let n = NormalNoise::create(&mut *self.random.from_hash_of("minecraft:offset"), 0, &[0.0]);
                return NoiseHolder { key: h.key.clone(), noise: Some(Arc::new(n)) };
            }
        }
        NoiseHolder { key: h.key.clone(), noise: Some(self.instance(&h.key)) }
    }

    fn apply(&mut self, n: Df) -> Df {
        match &*n {
            Node::OldBlendedNoise(b) => {
                let mut random: Box<dyn RandomSource> =
                    if self.legacy { Box::new(LegacyRandomSource::new(self.seed)) } else { self.random.from_hash_of("minecraft:terrain") };
                let p = b.params;
                Arc::new(Node::OldBlendedNoise(Blended { params: p, noise: Arc::new(BlendedNoise::new(&mut *random, p[0], p[1], p[2], p[3], p[4])) }))
            }
            Node::EndIslands(_) => Arc::new(Node::EndIslands(Arc::new(end_island_noise(self.seed)))),
            _ => n,
        }
    }
}

impl RandomState {
    pub fn new(data: &WorldgenData, settings: &GeneratorSettings, seed: i64) -> Result<Self> {
        let mut base: Box<dyn RandomSource> =
            if settings.legacy_random_source { Box::new(LegacyRandomSource::new(seed)) } else { Box::new(XoroshiroRandomSource::new(seed)) };
        let random = base.fork_positional();
        let aquifer_random = random.from_hash_of("minecraft:aquifer").fork_positional();
        let ore_random = random.from_hash_of("minecraft:ore").fork_positional();
        let cache = RefCell::new(HashMap::new());
        let mut wiring = Wiring { random: &random, noises: &data.noises, cache: &cache, legacy: settings.legacy_random_source, seed };
        let router = NoiseRouter::decode(&data.density, &settings.router)?.map_all(&mut wiring);
        Ok(Self { seed, random, aquifer_random, ore_random, router })
    }
}
