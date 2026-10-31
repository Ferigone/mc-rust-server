//! Compares the feature decoration infrastructure with the real Mojang classes: the order of the placed features per decoration step
//! (`FeatureSorter`) and `WorldgenRandom`. The reference comes from `tools/terrain/.../FeatureRef.java` (`tools/verify_features.sh`);
//! skipped when it is missing.

use mc_worldgen::biome_source::BiomeSource;
use mc_worldgen::decoration::{build_features_per_step, FeatureRegistry};
use mc_worldgen::random::{RandomSource, WorldgenRandom};
use std::collections::HashMap;
use std::path::Path;

#[test]
fn feature_order_and_worldgen_random_match_the_real_ones() {
    let work = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference/work");
    let generated = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference/gen/generated");
    let Ok(text) = std::fs::read_to_string(work.join("feature_ref.txt")) else {
        eprintln!("feature_ref.txt not found (run tools/verify_features.sh): skipped");
        return;
    };
    let reference: HashMap<&str, &str> = text.lines().map(|l| l.split_once('\t').unwrap()).collect();
    let mut bad: Vec<String> = Vec::new();
    let mut compared = 0usize;

    // FeatureSorter
    let reg = FeatureRegistry::load(&generated.join("data/minecraft/worldgen")).unwrap();
    for (name, source) in [("overworld", BiomeSource::overworld()), ("nether", BiomeSource::nether()), ("end", BiomeSource::End)] {
        let steps = build_features_per_step(&reg, &source.possible_biomes()).unwrap();
        let expected: usize = reference.keys().filter(|k| k.starts_with(&format!("order.{name}."))).count();
        assert_eq!(steps.iter().map(|s| s.names.len()).sum::<usize>(), expected, "{name}: number of sorted features");
        for (step, s) in steps.iter().enumerate() {
            for (i, f) in s.names.iter().enumerate() {
                compared += 1;
                let key = format!("order.{name}.{step}.{i}");
                if reference[key.as_str()] != f {
                    bad.push(format!("{key}: vanilla {} Rust {f}", reference[key.as_str()]));
                }
            }
        }
        eprintln!("{name}: {} decoration steps, {expected} features in order", steps.len());
    }

    // WorldgenRandom
    let mut n = 0;
    for (seed, x, z) in [(12345i64, 0, 0), (-4172144997902289642, 16, -32), (0, -48, 80), (987654321, 160, 160)] {
        let mut r = WorldgenRandom::new(1);
        let dec = r.set_decoration_seed(seed, x, z);
        let mut check = |key: String, actual: String| {
            compared += 1;
            if reference[key.as_str()] != actual {
                bad.push(format!("{key}: vanilla {} Rust {actual}", reference[key.as_str()]));
            }
        };
        check(format!("wgr.{n}"), dec.to_string());
        n += 1;
        for (index, step) in [(0, 0), (5, 3), (17, 9)] {
            r.set_feature_seed(dec, index, step);
            let mut v: Vec<String> = Vec::new();
            v.push(r.next_int_bound(10).to_string());
            v.push(r.next_int_bound(16).to_string());
            v.push(r.next_int_bound(3).to_string());
            v.push(r.next_int_bound(1000003).to_string());
            v.push((r.next_float().to_bits() as i32).to_string());
            v.push((r.next_double().to_bits() as i64).to_string());
            v.push(r.next_long().to_string());
            v.push(r.next_boolean().to_string());
            v.push(r.next_int().to_string());
            v.push((r.next_gaussian().to_bits() as i64).to_string());
            r.set_feature_seed(dec, index + 1, step);
            v.push((r.next_gaussian().to_bits() as i64).to_string());
            v.push((r.next_gaussian().to_bits() as i64).to_string());
            check(format!("wgr.{n}"), v.join(","));
            n += 1;
        }
    }
    assert!(bad.is_empty(), "{} of {compared} feature values differ, first ones:\n{}", bad.len(), bad.iter().take(10).cloned().collect::<Vec<_>>().join("\n"));
    eprintln!("all {compared} feature values are identical to the real server classes");
}
