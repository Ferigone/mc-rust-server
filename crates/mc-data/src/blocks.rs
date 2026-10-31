//! Stany bloków z `blocks.json` (oficjalny raport): ID stanów, właściwości, stan domyślny.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct BlockInfo {
    pub name: String,
    pub default_state: u32,
    pub min_state: u32,
    pub max_state: u32,
    /// Właściwości w kolejności z raportu: (nazwa, możliwe wartości).
    pub properties: Vec<(String, Vec<String>)>,
}

#[derive(Debug, Default)]
pub struct Blocks {
    pub list: Vec<BlockInfo>,
    pub by_name: HashMap<String, usize>,
    /// state id -> indeks bloku
    state_to_block: Vec<u32>,
    pub state_count: u32,
}

impl Blocks {
    pub fn load(path: &Path) -> Result<Self> {
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
        let obj = v.as_object().context("blocks must be an object")?;
        let mut list = Vec::with_capacity(obj.len());
        let mut max_state = 0u32;
        for (name, b) in obj {
            let states = b["states"].as_array().context("states")?;
            let mut min = u32::MAX;
            let mut max = 0;
            let mut def = None;
            for s in states {
                let id = s["id"].as_u64().context("state id")? as u32;
                min = min.min(id);
                max = max.max(id);
                if s["default"].as_bool().unwrap_or(false) {
                    def = Some(id);
                }
            }
            let properties = b
                .get("properties")
                .and_then(|p| p.as_object())
                .map(|p| {
                    p.iter()
                        .map(|(k, vals)| (k.clone(), vals.as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_owned()).collect()))
                        .collect()
                })
                .unwrap_or_default();
            max_state = max_state.max(max);
            list.push(BlockInfo { name: name.clone(), default_state: def.context("no default state")?, min_state: min, max_state: max, properties });
        }
        // Kolejność bloków = kolejność rejestru; sortujemy po minimalnym ID stanu (stany są ciągłe per blok).
        list.sort_by_key(|b| b.min_state);
        let mut by_name = HashMap::new();
        let mut state_to_block = vec![0u32; max_state as usize + 1];
        for (i, b) in list.iter().enumerate() {
            by_name.insert(b.name.clone(), i);
            for s in b.min_state..=b.max_state {
                state_to_block[s as usize] = i as u32;
            }
        }
        Ok(Self { list, by_name, state_to_block, state_count: max_state + 1 })
    }

    pub fn default_state(&self, name: &str) -> Option<u32> {
        self.by_name.get(name).map(|&i| self.list[i].default_state)
    }

    pub fn block_of_state(&self, state: u32) -> Option<&BlockInfo> {
        self.state_to_block.get(state as usize).map(|&i| &self.list[i as usize])
    }
}
