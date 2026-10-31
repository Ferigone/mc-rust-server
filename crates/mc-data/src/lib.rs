//! Dane gry wczytywane z wyjścia OFICJALNEGO data generatora Mojang (`tools/fetch_reference.sh`).
//! Żadne ID nie jest wpisane ręcznie: rejestry, stany bloków, tagi i wpisy datapacków
//! pochodzą z plików wygenerowanych przez serwer vanilla 1.21.1.

pub mod blocks;
pub mod dynamic;
pub mod nbtjson;
pub mod tags;

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub use blocks::Blocks;
pub use dynamic::{DynamicRegistry, SYNCHRONIZED_REGISTRIES};

/// Rejestr statyczny (wbudowany): nazwa -> protocol_id.
#[derive(Debug, Default, Clone)]
pub struct StaticRegistry {
    pub protocol_id: i32,
    pub ids: HashMap<String, i32>,
    pub names: Vec<String>,
}

pub struct GameData {
    pub root: PathBuf,
    pub registries: HashMap<String, StaticRegistry>,
    pub blocks: Blocks,
    /// 11 rejestrów synchronizowanych w kolejności `RegistryDataLoader.SYNCHRONIZED_REGISTRIES`.
    pub dynamic: Vec<DynamicRegistry>,
    /// Tagi gotowe do pakietu `update_tags` (rejestr -> [(tag, [id])]).
    pub tags: Vec<(String, Vec<(String, Vec<i32>)>)>,
}

impl GameData {
    /// `root` = katalog `generated` z data generatora (zawiera `reports/` i `data/`).
    pub fn load(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        let reports = root.join("reports");
        let registries = load_registries(&reports.join("registries.json")).context("registries.json")?;
        let blocks = Blocks::load(&reports.join("blocks.json")).context("blocks.json")?;
        let dynamic = dynamic::load_all(&root.join("data/minecraft"))?;
        let tags = tags::build(&root.join("data/minecraft/tags"), &registries, &dynamic)?;
        Ok(Self { root, registries, blocks, dynamic, tags })
    }

    pub fn registry_id(&self, registry: &str, name: &str) -> Option<i32> {
        self.registries.get(registry)?.ids.get(name).copied()
    }

    pub fn dynamic_registry(&self, name: &str) -> Option<&DynamicRegistry> {
        self.dynamic.iter().find(|r| r.name == name)
    }

    /// ID wpisu w rejestrze dynamicznym = jego indeks (kolejność ładowania vanilla).
    pub fn dynamic_id(&self, registry: &str, name: &str) -> Option<i32> {
        self.dynamic_registry(registry)?.entries.iter().position(|e| e.id == name).map(|i| i as i32)
    }
}

fn load_registries(path: &Path) -> Result<HashMap<String, StaticRegistry>> {
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
    let mut out = HashMap::new();
    for (name, reg) in v.as_object().context("registries must be an object")? {
        let mut r = StaticRegistry { protocol_id: reg["protocol_id"].as_i64().unwrap_or(-1) as i32, ..Default::default() };
        let entries = reg["entries"].as_object().context("entries")?;
        r.names = vec![String::new(); entries.len()];
        for (en, e) in entries {
            let id = e["protocol_id"].as_i64().context("protocol_id")? as i32;
            r.ids.insert(en.clone(), id);
            if (id as usize) < r.names.len() {
                r.names[id as usize] = en.clone();
            }
        }
        out.insert(name.clone(), r);
    }
    Ok(out)
}
