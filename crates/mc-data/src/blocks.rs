//! Block states from `blocks.json` (the official report): state IDs, properties, default state,
//! and lookups in both directions (name + properties <-> state ID).

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct BlockInfo {
    pub name: String,
    pub default_state: u32,
    pub min_state: u32,
    pub max_state: u32,
    /// Properties in report order: (name, possible values).
    pub properties: Vec<(String, Vec<String>)>,
}

#[derive(Debug, Default)]
pub struct Blocks {
    pub list: Vec<BlockInfo>,
    pub by_name: HashMap<String, usize>,
    /// state id -> block index
    state_to_block: Vec<u32>,
    /// state id -> properties of that state (empty for blocks without properties)
    state_props: Vec<Vec<(String, String)>>,
    /// block index -> (properties sorted by name, joined) -> state id
    lookup: HashMap<(u32, String), u32>,
    pub state_count: u32,
}

fn props_key<'a>(props: impl Iterator<Item = (&'a str, &'a str)>) -> String {
    let mut v: Vec<(&str, &str)> = props.collect();
    v.sort();
    let mut s = String::new();
    for (k, val) in v {
        s.push_str(k);
        s.push('=');
        s.push_str(val);
        s.push(',');
    }
    s
}

impl Blocks {
    pub fn load(path: &Path) -> Result<Self> {
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
        let obj = v.as_object().context("blocks must be an object")?;
        let mut list = Vec::with_capacity(obj.len());
        let mut max_state = 0u32;
        // (block name, [(state id, properties)])
        let mut raw_states: Vec<Vec<(u32, Vec<(String, String)>)>> = Vec::with_capacity(obj.len());
        for (name, b) in obj {
            let states = b["states"].as_array().context("states")?;
            let mut min = u32::MAX;
            let mut max = 0;
            let mut def = None;
            let mut sts = Vec::with_capacity(states.len());
            for s in states {
                let id = s["id"].as_u64().context("state id")? as u32;
                min = min.min(id);
                max = max.max(id);
                if s["default"].as_bool().unwrap_or(false) {
                    def = Some(id);
                }
                let props: Vec<(String, String)> = s
                    .get("properties")
                    .and_then(|p| p.as_object())
                    .map(|p| p.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_owned())).collect())
                    .unwrap_or_default();
                sts.push((id, props));
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
            raw_states.push(sts);
        }
        // Block order = registry order; sort by the minimum state ID (states are contiguous per block).
        let mut order: Vec<usize> = (0..list.len()).collect();
        order.sort_by_key(|&i| list[i].min_state);
        let list: Vec<BlockInfo> = order.iter().map(|&i| list[i].clone()).collect();
        let raw_states: Vec<_> = order.iter().map(|&i| raw_states[i].clone()).collect();

        let mut by_name = HashMap::new();
        let mut state_to_block = vec![0u32; max_state as usize + 1];
        let mut state_props: Vec<Vec<(String, String)>> = vec![Vec::new(); max_state as usize + 1];
        let mut lookup = HashMap::new();
        for (i, b) in list.iter().enumerate() {
            by_name.insert(b.name.clone(), i);
            for s in b.min_state..=b.max_state {
                state_to_block[s as usize] = i as u32;
            }
            for (id, props) in &raw_states[i] {
                lookup.insert((i as u32, props_key(props.iter().map(|(k, v)| (k.as_str(), v.as_str())))), *id);
                state_props[*id as usize] = props.clone();
            }
        }
        Ok(Self { list, by_name, state_to_block, state_props, lookup, state_count: max_state + 1 })
    }

    pub fn default_state(&self, name: &str) -> Option<u32> {
        self.by_name.get(name).map(|&i| self.list[i].default_state)
    }

    pub fn block_of_state(&self, state: u32) -> Option<&BlockInfo> {
        self.state_to_block.get(state as usize).map(|&i| &self.list[i as usize])
    }

    /// Properties of a state as (name, value) pairs in report order.
    pub fn properties_of(&self, state: u32) -> &[(String, String)] {
        self.state_props.get(state as usize).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// State ID for a block name and explicit properties; missing properties use the block's defaults,
    /// like `BlockState` deserialization in vanilla (`Properties` only override the default state).
    pub fn state_id<'a>(&self, name: &str, props: impl Iterator<Item = (&'a str, &'a str)>) -> Option<u32> {
        let &bi = self.by_name.get(name)?;
        let info = &self.list[bi];
        let mut merged: Vec<(String, String)> = self.state_props[info.default_state as usize].clone();
        for (k, v) in props {
            match merged.iter_mut().find(|(mk, _)| mk == k) {
                Some(slot) => slot.1 = v.to_owned(),
                None => return None, // unknown property for this block
            }
        }
        self.lookup.get(&(bi as u32, props_key(merged.iter().map(|(k, v)| (k.as_str(), v.as_str()))))).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs only when the reference data has been generated (tools/fetch_reference.sh).
    fn load() -> Option<Blocks> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference/gen/generated/reports/blocks.json");
        p.exists().then(|| Blocks::load(&p).unwrap())
    }

    #[test]
    fn name_and_properties_roundtrip() {
        let Some(b) = load() else { return };
        // grass_block[snowy=false] is the default state with ID 9 (verified in the vanilla chunk comparison)
        assert_eq!(b.state_id("minecraft:grass_block", [("snowy", "false")].into_iter()), Some(9));
        assert_eq!(b.state_id("minecraft:grass_block", std::iter::empty()), Some(9));
        assert_eq!(b.state_id("minecraft:grass_block", [("snowy", "true")].into_iter()), Some(8));
        assert_eq!(b.state_id("minecraft:air", std::iter::empty()), Some(0));
        assert_eq!(b.state_id("minecraft:no_such_block", std::iter::empty()), None);
        for id in [0u32, 8, 9, 79, 1000, 5000, 26683] {
            let blk = b.block_of_state(id).unwrap();
            let props = b.properties_of(id).to_vec();
            let back = b.state_id(&blk.name, props.iter().map(|(k, v)| (k.as_str(), v.as_str())));
            assert_eq!(back, Some(id), "{} {:?}", blk.name, props);
        }
    }
}
