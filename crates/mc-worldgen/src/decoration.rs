//! Biome decoration (`ChunkGenerator.applyBiomeDecoration`): the feature order per decoration step (`FeatureSorter`), and the
//! placed features of the data pack.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

/// The placed features of the data pack and the feature lists of the biomes.
pub struct FeatureRegistry {
    /// `minecraft:name` -> JSON
    pub placed: HashMap<String, Value>,
    /// biome -> the placed feature names of every decoration step
    pub biome_features: HashMap<String, Vec<Vec<String>>>,
}

fn read_dir_json(dir: &Path) -> Result<Vec<(String, Value)>> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).with_context(|| dir.display().to_string())? {
        let path = e?.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let name = format!("minecraft:{}", path.file_stem().unwrap().to_string_lossy());
        out.push((name, serde_json::from_slice(&std::fs::read(&path)?)?));
    }
    Ok(out)
}

impl FeatureRegistry {
    /// `dir` = `<generated>/data/minecraft/worldgen`
    pub fn load(dir: &Path) -> Result<Self> {
        let placed = read_dir_json(&dir.join("placed_feature"))?.into_iter().collect();
        let mut biome_features = HashMap::new();
        for (name, v) in read_dir_json(&dir.join("biome"))? {
            let steps = v["features"].as_array().ok_or_else(|| anyhow!("{name}: features"))?;
            let lists = steps
                .iter()
                .map(|step| match step {
                    Value::Array(a) => a.iter().map(|f| f.as_str().map(str::to_owned).ok_or_else(|| anyhow!("{name}: inline placed feature"))).collect::<Result<Vec<_>>>(),
                    other => bail!("{name}: bad feature step {other}"),
                })
                .collect::<Result<Vec<_>>>()?;
            biome_features.insert(name, lists);
        }
        Ok(Self { placed, biome_features })
    }

    /// `PlacedFeature` is a record: two entries with the same configured feature and the same modifiers are equal, so they share one index.
    fn key_of(&self, name: &str) -> Result<String> {
        let v = self.placed.get(name).ok_or_else(|| anyhow!("unknown placed feature {name}"))?;
        Ok(serde_json::to_string(v)?)
    }
}

/// `FeatureSorter.StepFeatureData`: the placed features of one decoration step in their final order.
#[derive(Debug, Default, Clone)]
pub struct StepFeatures {
    /// the registry name of every feature (the first one met when several entries are equal)
    pub names: Vec<String>,
    keys: Vec<String>,
}

impl StepFeatures {
    /// `indexMapping`: the position of a placed feature in this step
    pub fn index_of(&self, reg: &FeatureRegistry, name: &str) -> Option<usize> {
        let key = reg.key_of(name).ok()?;
        self.keys.iter().position(|k| *k == key)
    }
}

/// `FeatureSorter.buildFeaturesPerStep(biomes, features, true)`
pub fn build_features_per_step(reg: &FeatureRegistry, biomes: &[&str]) -> Result<Vec<StepFeatures>> {
    // feature index per record (the order the features are met in), and the data of every feature
    let mut index_of_key: HashMap<String, usize> = HashMap::new();
    let mut data: Vec<(usize, String, String)> = Vec::new(); // (step, name, key) by feature index
    // (step, index) -> successors; both ordered like the TreeMaps of the Java code
    let mut graph: BTreeMap<(usize, usize), BTreeSet<(usize, usize)>> = BTreeMap::new();
    let mut max_steps = 0;
    for biome in biomes {
        let steps = reg.biome_features.get(*biome).ok_or_else(|| anyhow!("unknown biome {biome}"))?;
        max_steps = max_steps.max(steps.len());
        let mut list: Vec<(usize, usize)> = Vec::new();
        for (step, names) in steps.iter().enumerate() {
            for name in names {
                let key = reg.key_of(name)?;
                let next = index_of_key.len();
                let idx = *index_of_key.entry(key.clone()).or_insert_with(|| {
                    data.push((step, name.clone(), key));
                    next
                });
                list.push((step, idx));
            }
        }
        for k in 0..list.len() {
            let set = graph.entry(list[k]).or_default();
            if k + 1 < list.len() {
                set.insert(list[k + 1]);
            }
        }
    }
    // depth-first search: the reverse post-order is the order
    let mut visited: BTreeSet<(usize, usize)> = BTreeSet::new();
    let mut in_progress: BTreeSet<(usize, usize)> = BTreeSet::new();
    let mut order: Vec<(usize, usize)> = Vec::new();
    fn dfs(
        graph: &BTreeMap<(usize, usize), BTreeSet<(usize, usize)>>,
        visited: &mut BTreeSet<(usize, usize)>,
        in_progress: &mut BTreeSet<(usize, usize)>,
        out: &mut Vec<(usize, usize)>,
        node: (usize, usize),
    ) -> bool {
        if visited.contains(&node) {
            return false;
        }
        if in_progress.contains(&node) {
            return true;
        }
        in_progress.insert(node);
        if let Some(next) = graph.get(&node) {
            for &n in next {
                if dfs(graph, visited, in_progress, out, n) {
                    return true;
                }
            }
        }
        in_progress.remove(&node);
        visited.insert(node);
        out.push(node);
        false
    }
    for &node in graph.keys() {
        if !in_progress.is_empty() {
            bail!("DFS bork: iteration finished with a non-empty in-progress set");
        }
        if !visited.contains(&node) && dfs(&graph, &mut visited, &mut in_progress, &mut order, node) {
            bail!("Feature order cycle found");
        }
    }
    order.reverse();
    let mut out = vec![StepFeatures::default(); max_steps];
    for (step, idx) in order {
        let (_, name, key) = &data[idx];
        out[step].names.push(name.clone());
        out[step].keys.push(key.clone());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry(biomes: &[(&str, &[&[&str]])]) -> FeatureRegistry {
        let mut placed = HashMap::new();
        let mut biome_features = HashMap::new();
        for (b, steps) in biomes {
            let mut lists = Vec::new();
            for step in *steps {
                for f in *step {
                    placed.insert(f.to_string(), serde_json::json!({ "feature": f, "placement": [] }));
                }
                lists.push(step.iter().map(|f| f.to_string()).collect());
            }
            biome_features.insert(b.to_string(), lists);
        }
        FeatureRegistry { placed, biome_features }
    }

    #[test]
    fn features_of_two_biomes_are_merged_in_a_consistent_order() {
        // a and c have to come before b in the first biome, c before d in the second one
        let reg = registry(&[("x", &[&["a", "c", "b"]]), ("y", &[&["c", "d"], &["e"]])]);
        let steps = build_features_per_step(&reg, &["x", "y"]).unwrap();
        assert_eq!(steps.len(), 2);
        let pos = |n: &str| steps[0].names.iter().position(|x| x == n).unwrap();
        assert!(pos("a") < pos("c") && pos("c") < pos("b") && pos("c") < pos("d"));
        assert_eq!(steps[1].names, vec!["e"]);
    }

    #[test]
    fn contradicting_orders_are_a_cycle() {
        let reg = registry(&[("x", &[&["a", "b"]]), ("y", &[&["b", "a"]])]);
        assert!(build_features_per_step(&reg, &["x", "y"]).is_err());
    }
}
