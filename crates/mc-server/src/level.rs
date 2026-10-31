//! `level.dat`: the world's global data (seed, spawn, time, game rules, generator settings).
//! The original compound is kept and only known fields are updated on save, so unknown data
//! (data packs, dragon fight, custom boss bars, ...) round-trips unchanged.

use anyhow::{bail, Context, Result};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use mc_protocol::nbt::{Compound, Tag};
use mc_protocol::Reader;
use std::io::{Read, Write};
use std::path::Path;

pub const DATA_VERSION: i32 = 3955;

#[derive(Debug, Clone)]
pub struct FlatSettings {
    /// (block name, height) from the bottom up.
    pub layers: Vec<(String, i32)>,
    pub biome: String,
}

impl Default for FlatSettings {
    /// The default superflat preset: bedrock, 2x dirt, grass_block in a plains biome.
    fn default() -> Self {
        Self {
            layers: vec![("minecraft:bedrock".into(), 1), ("minecraft:dirt".into(), 2), ("minecraft:grass_block".into(), 1)],
            biome: "minecraft:plains".into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LevelData {
    /// The whole root compound (contains `Data`).
    pub root: Compound,
}

impl LevelData {
    pub fn data(&self) -> &Compound {
        self.root.get_compound("Data").expect("level.dat has Data")
    }
    pub fn data_mut(&mut self) -> &mut Compound {
        match self.root.0.iter_mut().find(|(k, _)| k == "Data") {
            Some((_, Tag::Compound(c))) => c,
            _ => unreachable!("level.dat has Data"),
        }
    }

    pub fn load(path: &Path) -> Result<Option<Self>> {
        if !path.exists() {
            return Ok(None);
        }
        let mut raw = Vec::new();
        GzDecoder::new(std::fs::File::open(path)?).read_to_end(&mut raw).with_context(|| format!("reading {}", path.display()))?;
        match Tag::read_named(&mut Reader::new(&raw))? {
            Some((_, Tag::Compound(root))) if root.get_compound("Data").is_some() => Ok(Some(Self { root })),
            _ => bail!("{} has no Data compound", path.display()),
        }
    }

    /// Saves like vanilla: write `level.dat_new`, keep the previous file as `level.dat_old`, then rename.
    pub fn save(&self, path: &Path) -> Result<()> {
        let mut raw = Vec::new();
        Tag::Compound(self.root.clone()).write_named("", &mut raw);
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        enc.write_all(&raw)?;
        let bytes = enc.finish()?;
        let dir = path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir)?;
        let new = dir.join("level.dat_new");
        let old = dir.join("level.dat_old");
        std::fs::write(&new, bytes)?;
        if path.exists() {
            let _ = std::fs::remove_file(&old);
            std::fs::rename(path, &old)?;
        }
        std::fs::rename(&new, path)?;
        Ok(())
    }

    pub fn seed(&self) -> i64 {
        self.data().get_compound("WorldGenSettings").and_then(|w| w.get_i64("seed")).unwrap_or(0)
    }
    pub fn spawn(&self) -> (i32, i32, i32) {
        let d = self.data();
        (d.get_i32("SpawnX").unwrap_or(0), d.get_i32("SpawnY").unwrap_or(64), d.get_i32("SpawnZ").unwrap_or(0))
    }
    pub fn spawn_angle(&self) -> f32 {
        self.data().get_f64("SpawnAngle").unwrap_or(0.0) as f32
    }
    pub fn game_time(&self) -> i64 {
        self.data().get_i64("Time").unwrap_or(0)
    }
    pub fn day_time(&self) -> i64 {
        self.data().get_i64("DayTime").unwrap_or(0)
    }
    pub fn game_type(&self) -> i32 {
        self.data().get_i32("GameType").unwrap_or(0)
    }
    pub fn difficulty(&self) -> u8 {
        self.data().get_i32("Difficulty").unwrap_or(2) as u8
    }
    pub fn difficulty_locked(&self) -> bool {
        self.data().get_bool("DifficultyLocked").unwrap_or(false)
    }
    pub fn hardcore(&self) -> bool {
        self.data().get_bool("hardcore").unwrap_or(false)
    }
    pub fn raining(&self) -> bool {
        self.data().get_bool("raining").unwrap_or(false)
    }
    pub fn thundering(&self) -> bool {
        self.data().get_bool("thundering").unwrap_or(false)
    }
    pub fn data_version(&self) -> i32 {
        self.data().get_i32("DataVersion").unwrap_or(0)
    }

    /// Game rules are stored as strings (`"true"`, `"10"`).
    pub fn rule_bool(&self, name: &str, default: bool) -> bool {
        match self.data().get_compound("GameRules").and_then(|g| g.get_str(name)) {
            Some("true") => true,
            Some("false") => false,
            _ => default,
        }
    }
    pub fn rule_int(&self, name: &str, default: i32) -> i32 {
        self.data().get_compound("GameRules").and_then(|g| g.get_str(name)).and_then(|s| s.parse().ok()).unwrap_or(default)
    }

    /// Generator settings of the overworld when it is a flat world.
    pub fn flat_settings(&self) -> Option<FlatSettings> {
        let generator = self
            .data()
            .get_compound("WorldGenSettings")?
            .get_compound("dimensions")?
            .get_compound("minecraft:overworld")?
            .get_compound("generator")?;
        if generator.get_str("type") != Some("minecraft:flat") {
            return None;
        }
        let settings = generator.get_compound("settings")?;
        let layers = settings
            .get_list("layers")?
            .iter()
            .filter_map(|l| {
                let c = l.as_compound()?;
                Some((c.get_str("block")?.to_owned(), c.get_i32("height")?))
            })
            .collect();
        Some(FlatSettings { layers, biome: settings.get_str("biome").unwrap_or("minecraft:plains").to_owned() })
    }

    /// The overworld's noise generator: (noise settings name, biome source type or multi-noise preset).
    pub fn noise_generator(&self) -> Option<(String, String)> {
        let generator = self
            .data()
            .get_compound("WorldGenSettings")?
            .get_compound("dimensions")?
            .get_compound("minecraft:overworld")?
            .get_compound("generator")?;
        if generator.get_str("type") != Some("minecraft:noise") {
            return None;
        }
        let settings = generator.get_str("settings")?.to_owned();
        let source = generator.get_compound("biome_source")?;
        let kind = match source.get_str("type")? {
            "minecraft:multi_noise" => source.get_str("preset")?.to_owned(),
            other => other.to_owned(),
        };
        Some((settings, kind))
    }

    /// Whether the overworld uses a normal (noise based) generator.
    #[cfg(test)]
    pub fn is_noise_world(&self) -> bool {
        self.data()
            .get_compound("WorldGenSettings")
            .and_then(|w| w.get_compound("dimensions"))
            .and_then(|d| d.get_compound("minecraft:overworld"))
            .and_then(|o| o.get_compound("generator"))
            .and_then(|g| g.get_str("type"))
            == Some("minecraft:noise")
    }

    /// A new normal world's `level.dat`: like a superflat one but with the noise generator in the overworld.
    pub fn new_noise(name: &str, seed: i64, game_type: i32, difficulty: u8, hardcore: bool, spawn: (i32, i32, i32), settings: &str) -> Self {
        let mut level = Self::new_flat(name, seed, game_type, difficulty, hardcore, spawn, &FlatSettings::default());
        let generator = Compound::new()
            .with("type", Tag::string("minecraft:noise"))
            .with("settings", Tag::string(settings))
            .with("biome_source", Tag::Compound(Compound::new().with("type", Tag::string("minecraft:multi_noise")).with("preset", Tag::string("minecraft:overworld"))));
        let overworld = Compound::new().with("generator", Tag::Compound(generator)).with("type", Tag::string("minecraft:overworld"));
        fn child<'a>(c: &'a mut Compound, key: &str) -> &'a mut Compound {
            match c.0.iter_mut().find(|(k, _)| k == key) {
                Some((_, Tag::Compound(c))) => c,
                _ => unreachable!("level.dat of a new world has {key}"),
            }
        }
        let wgs = child(level.data_mut(), "WorldGenSettings");
        wgs.put("generate_features", Tag::Byte(1));
        let dims = child(wgs, "dimensions");
        dims.put("minecraft:overworld", Tag::Compound(overworld));
        level
    }

    /// A new flat world's `level.dat` (same fields vanilla writes for a superflat world).
    pub fn new_flat(name: &str, seed: i64, game_type: i32, difficulty: u8, hardcore: bool, spawn: (i32, i32, i32), flat: &FlatSettings) -> Self {
        fn s(x: impl Into<String>) -> Tag {
            Tag::string(x)
        }
        let layers: Vec<Tag> = flat
            .layers
            .iter()
            .map(|(b, h)| Tag::Compound(Compound::new().with("block", s(b.clone())).with("height", Tag::Int(*h))))
            .collect();
        let overworld = Compound::new()
            .with(
                "generator",
                Tag::Compound(
                    Compound::new()
                        .with("type", s("minecraft:flat"))
                        .with(
                            "settings",
                            Tag::Compound(
                                Compound::new()
                                    .with("features", Tag::Byte(0))
                                    .with("biome", s(flat.biome.clone()))
                                    .with("layers", Tag::List(layers))
                                    .with("structure_overrides", Tag::List(vec![s("minecraft:strongholds"), s("minecraft:villages")]))
                                    .with("lakes", Tag::Byte(0)),
                            ),
                        ),
                ),
            )
            .with("type", s("minecraft:overworld"));
        let nether = Compound::new()
            .with(
                "generator",
                Tag::Compound(
                    Compound::new()
                        .with("type", s("minecraft:noise"))
                        .with("settings", s("minecraft:nether"))
                        .with("biome_source", Tag::Compound(Compound::new().with("type", s("minecraft:multi_noise")).with("preset", s("minecraft:nether")))),
                ),
            )
            .with("type", s("minecraft:the_nether"));
        let end = Compound::new()
            .with(
                "generator",
                Tag::Compound(
                    Compound::new()
                        .with("type", s("minecraft:noise"))
                        .with("settings", s("minecraft:end"))
                        .with("biome_source", Tag::Compound(Compound::new().with("type", s("minecraft:the_end")))),
                ),
            )
            .with("type", s("minecraft:the_end"));
        let dims = Compound::new()
            .with("minecraft:overworld", Tag::Compound(overworld))
            .with("minecraft:the_nether", Tag::Compound(nether))
            .with("minecraft:the_end", Tag::Compound(end));
        let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
        let data = Compound::new()
            .with("DataVersion", Tag::Int(DATA_VERSION))
            .with("version", Tag::Int(19133))
            .with("LevelName", s(name))
            .with("LastPlayed", Tag::Long(now_ms))
            .with("Time", Tag::Long(0))
            .with("DayTime", Tag::Long(0))
            .with("SpawnX", Tag::Int(spawn.0))
            .with("SpawnY", Tag::Int(spawn.1))
            .with("SpawnZ", Tag::Int(spawn.2))
            .with("SpawnAngle", Tag::Float(0.0))
            .with("GameType", Tag::Int(game_type))
            .with("Difficulty", Tag::Byte(difficulty as i8))
            .with("DifficultyLocked", Tag::Byte(0))
            .with("hardcore", Tag::Byte(hardcore as i8))
            .with("allowCommands", Tag::Byte(0))
            .with("initialized", Tag::Byte(1))
            .with("raining", Tag::Byte(0))
            .with("rainTime", Tag::Int(0))
            .with("thundering", Tag::Byte(0))
            .with("thunderTime", Tag::Int(0))
            .with("clearWeatherTime", Tag::Int(0))
            .with("WasModded", Tag::Byte(0))
            .with("BorderCenterX", Tag::Double(0.0))
            .with("BorderCenterZ", Tag::Double(0.0))
            .with("BorderSize", Tag::Double(5.9999968E7))
            .with("BorderSizeLerpTarget", Tag::Double(5.9999968E7))
            .with("BorderSizeLerpTime", Tag::Long(0))
            .with("BorderSafeZone", Tag::Double(5.0))
            .with("BorderDamagePerBlock", Tag::Double(0.2))
            .with("BorderWarningBlocks", Tag::Double(5.0))
            .with("BorderWarningTime", Tag::Double(15.0))
            .with("WanderingTraderSpawnChance", Tag::Int(25))
            .with("WanderingTraderSpawnDelay", Tag::Int(24000))
            .with("ServerBrands", Tag::List(vec![s("vanilla")]))
            .with("CustomBossEvents", Tag::Compound(Compound::new()))
            .with("GameRules", Tag::Compound(Compound::new()))
            .with("DataPacks", Tag::Compound(Compound::new().with("Enabled", Tag::List(vec![s("vanilla")])).with("Disabled", Tag::List(vec![s("trade_rebalance"), s("bundle")]))))
            .with(
                "Version",
                Tag::Compound(Compound::new().with("Id", Tag::Int(DATA_VERSION)).with("Name", s("1.21.1")).with("Series", s("main")).with("Snapshot", Tag::Byte(0))),
            )
            .with(
                "WorldGenSettings",
                Tag::Compound(
                    Compound::new().with("seed", Tag::Long(seed)).with("generate_features", Tag::Byte(0)).with("bonus_chest", Tag::Byte(0)).with("dimensions", Tag::Compound(dims)),
                ),
            );
        Self { root: Compound::new().with("Data", Tag::Compound(data)) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_flat_roundtrip() {
        let d = std::env::temp_dir().join(format!("mc-level-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let ld = LevelData::new_flat("world", 12345, 1, 2, false, (0, -60, 0), &FlatSettings::default());
        ld.save(&d.join("level.dat")).unwrap();
        ld.save(&d.join("level.dat")).unwrap(); // second save keeps level.dat_old
        assert!(d.join("level.dat_old").exists());
        let back = LevelData::load(&d.join("level.dat")).unwrap().unwrap();
        assert_eq!(back.seed(), 12345);
        assert_eq!(back.spawn(), (0, -60, 0));
        assert_eq!(back.game_type(), 1);
        assert_eq!(back.flat_settings().unwrap().layers.len(), 3);
        assert!(!back.is_noise_world());
        assert!(back.rule_bool("doDaylightCycle", true));
    }

    #[test]
    fn new_noise_world_has_a_noise_generator() {
        let ld = LevelData::new_noise("world", 1, 0, 2, false, (0, 64, 0), "minecraft:amplified");
        assert!(ld.is_noise_world());
        assert!(ld.flat_settings().is_none());
        assert_eq!(ld.noise_generator(), Some(("minecraft:amplified".to_owned(), "minecraft:overworld".to_owned())));
        assert_eq!(ld.seed(), 1);
    }
}
