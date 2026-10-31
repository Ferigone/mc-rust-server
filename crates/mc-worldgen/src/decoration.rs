//! Biome decoration (`ChunkGenerator.applyBiomeDecoration`): the feature order per decoration step (`FeatureSorter`), and the
//! placed features of the data pack.

use crate::feature::Env;
use crate::placement::{PlaceCtx, PlacedFeature};
use crate::random::{RandomSource, WorldgenRandom};
use crate::region::Region;
use crate::surface::GenContext;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

/// The placed features of the data pack and the feature lists of the biomes.
pub struct FeatureRegistry {
    /// `minecraft:name` -> JSON
    pub placed: HashMap<String, Value>,
    /// biome -> the placed feature names of every decoration step
    pub biome_features: HashMap<String, Vec<Vec<String>>>,
    /// `minecraft:name` -> JSON of the configured features
    pub configured: HashMap<String, Value>,
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
        let configured = read_dir_json(&dir.join("configured_feature"))?.into_iter().collect();
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
        Ok(Self { placed, biome_features, configured })
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
    pub keys: Vec<String>,
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

/// The decoration of a dimension: the sorted features per step, the placed features the port can place, and what each biome has.
pub struct Decorator {
    steps: Vec<StepFeatures>,
    /// compiled placed features by key (those the port cannot place are missing and skipped)
    placed: HashMap<String, Arc<PlacedFeature>>,
    /// biome -> per step the indices (into `steps[step]`) of its features
    biome_indices: HashMap<String, Vec<Vec<usize>>>,
    /// biome -> the keys of all its placed features (`BiomeGenerationSettings.hasFeature`)
    biome_keys: HashMap<String, HashSet<String>>,
    /// placed feature names the port cannot place yet, with the reason
    pub unsupported: Vec<(String, String)>,
}

impl Decorator {
    /// `possible_biomes` = `BiomeSource.possibleBiomes()`
    pub fn new(reg: &FeatureRegistry, possible_biomes: &[&str], env: &Env) -> Result<Self> {
        let steps = build_features_per_step(reg, possible_biomes)?;
        let mut placed = HashMap::new();
        let mut unsupported = Vec::new();
        let mut names: Vec<&String> = reg.placed.keys().collect();
        names.sort();
        for name in names {
            match PlacedFeature::parse(env, name, &reg.placed[name]) {
                Ok(p) => {
                    placed.entry(p.key.clone()).or_insert_with(|| Arc::new(p));
                }
                Err(e) => unsupported.push((name.clone(), format!("{e:#}"))),
            }
        }
        let mut biome_indices = HashMap::new();
        let mut biome_keys = HashMap::new();
        for biome in possible_biomes {
            let lists = reg.biome_features.get(*biome).ok_or_else(|| anyhow!("unknown biome {biome}"))?;
            let mut per_step = Vec::new();
            let mut keys = HashSet::new();
            for (step, names) in lists.iter().enumerate() {
                let mut idx = Vec::new();
                for n in names {
                    let key = reg.key_of(n)?;
                    if let Some(i) = steps.get(step).and_then(|s| s.keys.iter().position(|k| *k == key)) {
                        idx.push(i);
                    }
                    keys.insert(key);
                }
                per_step.push(idx);
            }
            biome_indices.insert(biome.to_string(), per_step);
            biome_keys.insert(biome.to_string(), keys);
        }
        Ok(Self { steps, placed, biome_indices, biome_keys, unsupported })
    }

    /// The names of the placed features the port places, sorted (the Java reference places exactly these).
    pub fn supported_names(&self, reg: &FeatureRegistry) -> Vec<String> {
        let mut v: Vec<String> = reg.placed.keys().filter(|n| reg.key_of(n).is_ok_and(|k| self.placed.contains_key(&k))).cloned().collect();
        v.sort();
        v
    }

    /// `ChunkGenerator.applyBiomeDecoration` (without structures): `biomes` are the biomes of the 3x3 chunks around the centre that
    /// the biome source can produce.
    pub fn decorate(&self, region: &mut Region, world_seed: i64, range: GenContext, biomes: &HashSet<&'static str>) {
        let (cx, cz) = region.center;
        let origin = (cx * 16, region.min_y, cz * 16);
        let mut random = WorldgenRandom::new(1);
        let decoration_seed = random.set_decoration_seed(world_seed, origin.0, origin.2);
        let biome_has_feature = |biome: &str, key: &str| self.biome_keys.get(biome).is_some_and(|k| k.contains(key));
        let ctx = PlaceCtx { range, biome_has_feature: &biome_has_feature };
        for step in 0..self.steps.len() {
            // the sorted indices of the features of the biomes around
            let mut indices: BTreeSet<usize> = BTreeSet::new();
            for biome in biomes {
                if let Some(per_step) = self.biome_indices.get(*biome) {
                    if let Some(list) = per_step.get(step) {
                        indices.extend(list.iter().copied());
                    }
                }
            }
            for index in indices {
                let Some(feature) = self.placed.get(&self.steps[step].keys[index]) else { continue };
                random.set_feature_seed(decoration_seed, index as i32, step as i32);
                let r: &mut dyn RandomSource = &mut random;
                feature.place_with_biome_check(region, &ctx, r, origin);
            }
        }
    }
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
        FeatureRegistry { placed, biome_features, configured: HashMap::new() }
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
