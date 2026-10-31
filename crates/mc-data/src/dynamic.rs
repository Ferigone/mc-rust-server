//! Dynamic registries synchronized with the client (`RegistryDataLoader.SYNCHRONIZED_REGISTRIES`).
//!
//! Entry order = vanilla loading order: `MultiPackResourceManager.listResources` returns
//! a `TreeMap<ResourceLocation, ..>` keyed by the full file path (with prefix and `.json`),
//! compared by path first, then by namespace. This determines entry IDs
//! (np. ID biomu w chunkach, ID typu wymiaru w pakiecie Login).

use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;

/// (registry name, data pack directory), in the order they are sent to the client.
pub const SYNCHRONIZED_REGISTRIES: &[(&str, &str)] = &[
    ("minecraft:worldgen/biome", "worldgen/biome"),
    ("minecraft:chat_type", "chat_type"),
    ("minecraft:trim_pattern", "trim_pattern"),
    ("minecraft:trim_material", "trim_material"),
    ("minecraft:wolf_variant", "wolf_variant"),
    ("minecraft:painting_variant", "painting_variant"),
    ("minecraft:dimension_type", "dimension_type"),
    ("minecraft:damage_type", "damage_type"),
    ("minecraft:banner_pattern", "banner_pattern"),
    ("minecraft:enchantment", "enchantment"),
    ("minecraft:jukebox_song", "jukebox_song"),
];

#[derive(Debug, Clone)]
pub struct DynamicEntry {
    /// np. `minecraft:plains`
    pub id: String,
    /// JSON content from the data pack (complete, DIRECT codec).
    pub json: Value,
}

#[derive(Debug, Clone)]
pub struct DynamicRegistry {
    pub name: String,
    pub dir: String,
    pub entries: Vec<DynamicEntry>,
}

impl DynamicRegistry {
    /// Value sent in `registry_data` when the known packs do not match.
    /// Biomes use `Biome.NETWORK_CODEC`: only the climate settings and `effects`.
    pub fn network_json(&self, e: &DynamicEntry) -> Value {
        if self.name == "minecraft:worldgen/biome" {
            let mut m = serde_json::Map::new();
            for k in ["has_precipitation", "temperature", "temperature_modifier", "downfall", "effects"] {
                if let Some(v) = e.json.get(k) {
                    m.insert(k.to_owned(), v.clone());
                }
            }
            Value::Object(m)
        } else {
            e.json.clone()
        }
    }
}

pub fn load_all(data_minecraft: &Path) -> Result<Vec<DynamicRegistry>> {
    SYNCHRONIZED_REGISTRIES.iter().map(|(name, dir)| load_one(data_minecraft, name, dir)).collect()
}

fn load_one(root: &Path, name: &str, dir: &str) -> Result<DynamicRegistry> {
    let base = root.join(dir);
    let mut files: Vec<(String, std::path::PathBuf)> = Vec::new();
    collect(&base, &base, &mut files)?;
    // TreeMap key: "<prefix>/<path>.json" compared as a Java String (UTF-16, ASCII here).
    files.sort_by(|a, b| a.0.encode_utf16().cmp(b.0.encode_utf16()));
    let mut entries = Vec::with_capacity(files.len());
    for (rel, path) in files {
        let id = format!("minecraft:{}", rel.strip_suffix(".json").unwrap());
        let json: Value = serde_json::from_slice(&std::fs::read(&path)?).with_context(|| path.display().to_string())?;
        entries.push(DynamicEntry { id, json });
    }
    Ok(DynamicRegistry { name: name.to_owned(), dir: dir.to_owned(), entries })
}

fn collect(base: &Path, dir: &Path, out: &mut Vec<(String, std::path::PathBuf)>) -> Result<()> {
    for e in std::fs::read_dir(dir).with_context(|| dir.display().to_string())? {
        let e = e?;
        let p = e.path();
        if e.file_type()?.is_dir() {
            collect(base, &p, out)?;
        } else if p.extension().is_some_and(|x| x == "json") {
            let rel = p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
            out.push((rel, p));
        }
    }
    Ok(())
}
