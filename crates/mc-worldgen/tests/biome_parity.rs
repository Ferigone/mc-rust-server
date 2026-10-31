//! Compares the biome source with the real Mojang classes: the preset parameter lists entry by entry, R-tree searches on
//! synthetic targets (many exact ties), biomes of the real `MultiNoiseBiomeSource` / `TheEndBiomeSource` at sampled quart positions
//! (climate values and biome names) and the world spawn search. The reference comes from `tools/terrain/.../BiomeRef.java`
//! (`tools/verify_terrain.sh`); skipped when it is missing.

use mc_worldgen::biome_source::BiomeSource;
use mc_worldgen::climate::{Parameter, ParameterList, ParameterPoint, TargetPoint};
use mc_worldgen::overworld_biomes::{nether_list, OverworldBiomeBuilder};
use mc_worldgen::random_state::{GeneratorSettings, RandomState, WorldgenData};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

fn p(x: &Parameter) -> String {
    format!("{},{}", x.min, x.max)
}

fn describe(pt: &ParameterPoint, biome: &str) -> String {
    format!("{},{},{},{},{},{},{},{}", p(&pt.temperature), p(&pt.humidity), p(&pt.continentalness), p(&pt.erosion), p(&pt.depth), p(&pt.weirdness), pt.offset, biome)
}

/// The kind of biome source each reference configuration uses (the order of the arguments of `BiomeRef`).
const CONFIGS: &[(&str, i64, &str)] = &[
    ("minecraft:overworld", 12345, "overworld"),
    ("minecraft:overworld", -4172144997902289642, "overworld"),
    ("minecraft:overworld", 0, "overworld"),
    ("minecraft:overworld", -1, "overworld"),
    ("minecraft:overworld", 987654321, "overworld"),
    ("minecraft:amplified", 777, "overworld"),
    ("minecraft:large_biomes", 4242, "overworld"),
    ("minecraft:caves", 99, "overworld"),
    ("minecraft:floating_islands", 99, "overworld"),
    ("minecraft:nether", 12345, "nether"),
    ("minecraft:nether", -77, "nether"),
    ("minecraft:end", 12345, "end"),
    ("minecraft:end", 31337, "end"),
];

#[test]
fn biome_source_matches_the_real_one() {
    let work = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference/work");
    let generated = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference/gen/generated");
    let Ok(text) = std::fs::read_to_string(work.join("biome_ref.txt")) else {
        eprintln!("biome_ref.txt not found (run tools/verify_terrain.sh): skipped");
        return;
    };
    let reference: HashMap<&str, &str> = text.lines().map(|l| l.split_once('\t').unwrap()).collect();
    let mut bad: Vec<String> = Vec::new();
    let mut compared = 0usize;

    // A: the parameter lists, entry by entry (order matters for the R-tree and for ties)
    let overworld_list = OverworldBiomeBuilder::overworld_list();
    let nether = nether_list();
    for (id, list) in [("overworld", &overworld_list), ("nether", &nether)] {
        let expected = reference.keys().filter(|k| k.starts_with(&format!("list.{id}."))).count();
        assert_eq!(list.values().len(), expected, "{id}: number of entries");
        for (i, (pt, biome)) in list.values().iter().enumerate() {
            compared += 1;
            let key = format!("list.{id}.{i}");
            let actual = describe(pt, biome);
            if reference[key.as_str()] != actual {
                bad.push(format!("{key}: vanilla {} Rust {actual}", reference[key.as_str()]));
            }
        }
    }
    eprintln!("parameter lists identical: overworld {} entries, nether {} entries", overworld_list.values().len(), nether.values().len());

    // B: R-tree searches against fresh lists, with the same synthetic targets
    let mut ties = 0usize;
    for (id, list) in [("overworld", OverworldBiomeBuilder::overworld_list()), ("nether", nether_list())] {
        let mut pools: Vec<BTreeSet<i64>> = vec![BTreeSet::new(); 6];
        for (pt, _) in list.values() {
            let ps = [pt.temperature, pt.humidity, pt.continentalness, pt.erosion, pt.depth, pt.weirdness];
            for d in 0..6 {
                pools[d].insert(ps[d].min);
                pools[d].insert(ps[d].max);
            }
        }
        let pools: Vec<Vec<i64>> = pools.into_iter().map(|s| s.into_iter().collect()).collect();
        let mut state: u64 = 0x9E3779B97F4A7C15;
        for i in 0..30000 {
            let mut t = [0i64; 6];
            for d in 0..6 {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                let r = (state >> 33) as i64;
                t[d] = if r & 3 == 0 { (r >> 2) % 24001 - 12000 } else { pools[d][((r >> 2) % pools[d].len() as i64) as usize] };
            }
            let target = TargetPoint { temperature: t[0], humidity: t[1], continentalness: t[2], erosion: t[3], depth: t[4], weirdness: t[5] };
            let tree = *list.find_value(&target);
            let brute = *list.find_value_brute_force(&target);
            if tree != brute {
                ties += 1;
            }
            compared += 1;
            let key = format!("tree.{id}.{i}");
            let actual = format!("{tree},{brute}");
            if reference[key.as_str()] != actual {
                bad.push(format!("{key}: vanilla {} Rust {actual}", reference[key.as_str()]));
            }
        }
    }
    eprintln!("R-tree: 60000 searches identical; {ties} of them end on a tie where the tree and brute force disagree (reproduced exactly)");
    assert!(ties > 0, "the synthetic targets should hit exact ties");

    // B2: random lists on a coarse grid (distinct values: exact ties between different entries are common)
    let grid: [i64; 7] = [-10000, -5000, -2500, 0, 2500, 5000, 10000];
    let mut rand_ties = 0usize;
    for (li, size) in [5usize, 40, 700, 300, 1500].into_iter().enumerate() {
        let mut state: u64 = 0x1234567 + li as u64;
        let mut next = || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            state >> 33
        };
        let mut entries: Vec<(ParameterPoint, usize)> = Vec::new();
        let mut prev: Option<([Parameter; 6], i64)> = None;
        for i in 0..size {
            let mut ps = [Parameter { min: 0, max: 0 }; 6];
            // lists 3 and 4: clusters of entries that differ in a single parameter (like the surface / depth pairs of the real table)
            let draw = next(); // (always consumed, like in BiomeRef)
            let clustered = li >= 3 && prev.is_some() && draw % 12 != 0;
            let changed = if clustered { (next() % 7) as usize } else { usize::MAX };
            for d in 0..6 {
                if clustered && changed != d {
                    ps[d] = prev.unwrap().0[d];
                    continue;
                }
                let a = grid[(next() % 7) as usize];
                let b = grid[(next() % 7) as usize];
                ps[d] = Parameter { min: a.min(b), max: a.max(b) };
            }
            let offset = if clustered && changed != 6 { prev.unwrap().1 } else { [0i64, 0, 0, 1000, 2500][(next() % 5) as usize] };
            prev = Some((ps, offset));
            entries.push((ParameterPoint { temperature: ps[0], humidity: ps[1], continentalness: ps[2], erosion: ps[3], depth: ps[4], weirdness: ps[5], offset }, i));
        }
        let list = ParameterList::new(entries);
        for i in 0..20000 {
            let mut t = [0i64; 6];
            for d in 0..6 {
                t[d] = (next() % 17) as i64 * 1250 - 10000;
            }
            let target = TargetPoint { temperature: t[0], humidity: t[1], continentalness: t[2], erosion: t[3], depth: t[4], weirdness: t[5] };
            let (tree, brute) = (*list.find_value(&target), *list.find_value_brute_force(&target));
            rand_ties += (tree != brute) as usize;
            compared += 1;
            let key = format!("rand.{li}.{i}");
            let actual = format!("{tree},{brute}");
            if reference[key.as_str()] != actual {
                bad.push(format!("{key}: vanilla {} Rust {actual}", reference[key.as_str()]));
            }
        }
    }
    eprintln!("random grid lists: 100000 searches identical; {rand_ties} end on ties between different entries where the tree disagrees with brute force");
    assert!(rand_ties > 100, "the grid targets should hit many ties");

    // C + D: the biome sources at sampled positions and the spawn search; one source per kind, queried in the order of the reference
    let data = WorldgenData::load(&generated.join("data/minecraft/worldgen")).unwrap();
    let (overworld, nether, end) = (BiomeSource::overworld(), BiomeSource::nether(), BiomeSource::End);
    let mut seen: HashSet<&'static str> = HashSet::new();
    for (cfg, (settings_name, seed, kind)) in CONFIGS.iter().enumerate() {
        let settings = GeneratorSettings::parse(&data.noise_settings[*settings_name]).unwrap();
        let state = RandomState::new(&data, &settings, *seed).unwrap();
        let sampler = &state.sampler;
        let source = match *kind {
            "nether" => &nether,
            "end" => &end,
            _ => &overworld,
        };
        let mut positions: Vec<(i32, i32, i32)> = Vec::new();
        for i in 0..2500i32 {
            positions.push(((i * 7919 + i * i * 13) % 3001 - 1500, (i * 31) % 112 - 32, (i * 104729 + i * i * 7) % 3001 - 1500));
        }
        for gx in 0..60 {
            for gz in 0..60 {
                positions.push(((gx - 30) * 24, 16, (gz - 30) * 24));
            }
        }
        for (n, (qx, qy, qz)) in positions.into_iter().enumerate() {
            let t = sampler.sample(qx, qy, qz);
            let biome = source.get_noise_biome(qx, qy, qz, sampler);
            seen.insert(biome);
            compared += 1;
            let key = format!("src.{cfg}.{n}");
            let actual = format!("{qx},{qy},{qz},{},{},{},{},{},{},{biome}", t.temperature, t.humidity, t.continentalness, t.erosion, t.depth, t.weirdness);
            if reference[key.as_str()] != actual {
                bad.push(format!("{key} ({settings_name}@{seed}): vanilla {} Rust {actual}", reference[key.as_str()]));
            }
        }
        if *kind == "overworld" {
            let (x, z) = sampler.find_spawn_position();
            compared += 1;
            let key = format!("spawn.{cfg}");
            let actual = format!("{x},0,{z}");
            if reference[key.as_str()] != actual {
                bad.push(format!("{key} ({settings_name}@{seed}): vanilla {} Rust {actual}", reference[key.as_str()]));
            }
        }
    }
    // E: the temperature of every biome at many positions (bit-exact), and the two thresholds built on it
    let mut names: Vec<&String> = data.biomes.keys().collect();
    names.sort();
    let ys = [-60, 40, 63, 80, 81, 90, 120, 200, 319];
    let (mut snow, mut melt) = (0usize, 0usize);
    for name in &names {
        let climate = &data.biomes[*name];
        for i in 0..400i32 {
            let x = (i * 7919 + i * i * 31) % 20001 - 10000;
            let z = (i * 104729 + i * i * 17) % 20001 - 10000;
            let y = ys[(i as usize) % ys.len()];
            let t = climate.temperature_at(x, y, z);
            let actual = format!("{},{},{}", t.to_bits() as i32, climate.cold_enough_to_snow(x, y, z), climate.should_melt_frozen_ocean_iceberg_slightly(x, y, z));
            snow += climate.cold_enough_to_snow(x, y, z) as usize;
            melt += climate.should_melt_frozen_ocean_iceberg_slightly(x, y, z) as usize;
            compared += 1;
            let key = format!("clim.{name}.{i}");
            if reference[key.as_str()] != actual {
                bad.push(format!("{key} at ({x},{y},{z}): vanilla {} Rust {actual}", reference[key.as_str()]));
            }
        }
    }
    eprintln!("biome temperatures: {} biomes x 400 positions identical ({snow} cold enough to snow, {melt} warm enough to melt icebergs)", names.len());
    assert!(snow > 1000 && melt > 1000, "the positions should straddle both thresholds");
    eprintln!("{} different biomes were sampled", seen.len());
    assert!(seen.len() >= 40, "the sampled positions should cover many biomes, got {}", seen.len());
    assert!(bad.is_empty(), "{} of {compared} biome values differ, first ones:\n{}", bad.len(), bad.iter().take(10).cloned().collect::<Vec<_>>().join("\n"));
    eprintln!("all {compared} biome values are identical to the real server classes");
}

#[test]
fn spawn_target_json_matches_the_builder() {
    let generated = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference/gen/generated");
    let Ok(data) = WorldgenData::load(&generated.join("data/minecraft/worldgen")) else {
        eprintln!("generated data not found: skipped");
        return;
    };
    let settings = GeneratorSettings::parse(&data.noise_settings["minecraft:overworld"]).unwrap();
    assert_eq!(settings.spawn_target, OverworldBiomeBuilder::new().spawn_target());
    let nether = GeneratorSettings::parse(&data.noise_settings["minecraft:nether"]).unwrap();
    assert!(nether.spawn_target.is_empty());
}
