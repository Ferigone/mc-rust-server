//! Block tags (`data/minecraft/tags/block`) resolved to per-state flags.

use anyhow::{anyhow, bail, Context, Result};
use mc_data::Blocks;
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::{Arc, Mutex};

pub fn resolve_tag(root: &Path, name: &str, out: &mut BTreeSet<String>, depth: usize) -> Result<()> {
    if depth > 16 {
        bail!("tag recursion in {name}");
    }
    let path = root.join("data/minecraft/tags/block").join(format!("{}.json", name.trim_start_matches("minecraft:")));
    let v: Value = serde_json::from_slice(&std::fs::read(&path).with_context(|| path.display().to_string())?)?;
    for e in v["values"].as_array().ok_or_else(|| anyhow!("{name}: no values"))? {
        let id = match e {
            Value::String(s) => s.as_str(),
            Value::Object(o) => o.get("id").and_then(Value::as_str).ok_or_else(|| anyhow!("bad tag entry"))?,
            _ => bail!("bad tag entry"),
        };
        match id.strip_prefix('#') {
            Some(t) => resolve_tag(root, t, out, depth + 1)?,
            None => {
                out.insert(id.to_owned());
            }
        }
    }
    Ok(())
}

pub fn replaceable_states(root: &Path, blocks: &Blocks, v: &Value) -> Result<Vec<bool>> {
    let mut names = BTreeSet::new();
    let add = |s: &str, names: &mut BTreeSet<String>| -> Result<()> {
        match s.strip_prefix('#') {
            Some(t) => resolve_tag(root, t, names, 0),
            None => {
                names.insert(s.to_owned());
                Ok(())
            }
        }
    };
    match v {
        Value::String(s) => add(s, &mut names)?,
        Value::Array(list) => {
            for e in list {
                add(e.as_str().ok_or_else(|| anyhow!("bad replaceable entry"))?, &mut names)?;
            }
        }
        _ => bail!("bad replaceable {v}"),
    }
    let mut out = vec![false; blocks.state_count as usize];
    for n in &names {
        let info = blocks.by_name.get(n).map(|&i| &blocks.list[i]).ok_or_else(|| anyhow!("unknown block {n} in a carver tag"))?;
        for s in info.min_state..=info.max_state {
            out[s as usize] = true;
        }
    }
    Ok(out)
}


/// A block tag as a flag per block state ID, cached.
pub struct BlockTags {
    root: std::path::PathBuf,
    cache: Mutex<HashMap<String, Arc<Vec<bool>>>>,
}

impl BlockTags {
    /// `root` = the `generated` directory of the data generator
    pub fn new(root: &Path) -> Self {
        Self { root: root.to_path_buf(), cache: Mutex::new(HashMap::new()) }
    }

    /// `#minecraft:name` or `minecraft:name`: the states of all blocks in the tag
    pub fn states(&self, blocks: &Blocks, tag: &str) -> Result<Arc<Vec<bool>>> {
        let name = tag.trim_start_matches('#').to_owned();
        if let Some(t) = self.cache.lock().unwrap().get(&name) {
            return Ok(t.clone());
        }
        let v = Arc::new(replaceable_states(&self.root, blocks, &Value::String(format!("#{name}")))?);
        self.cache.lock().unwrap().insert(name, v.clone());
        Ok(v)
    }
}
