//! Compares every function of the wired noise router with the real Mojang `NoiseRouter`, bit for bit, at fixed positions.
//! The reference comes from `tools/terrain/.../RouterRef.java` (`tools/verify_terrain.sh`); skipped when it is missing.

use mc_worldgen::density::Ctx;
use mc_worldgen::random_state::{GeneratorSettings, RandomState, WorldgenData};
use std::collections::HashMap;
use std::path::Path;

#[test]
fn router_functions_match_the_real_ones() {
    let work = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference/work");
    let generated = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference/gen/generated");
    let Ok(text) = std::fs::read_to_string(work.join("router_ref.txt")) else {
        eprintln!("router_ref.txt not found (run tools/verify_terrain.sh): skipped");
        return;
    };
    let reference: HashMap<&str, i64> = text.lines().map(|l| l.split_once('\t').map(|(k, v)| (k, v.parse().unwrap())).unwrap()).collect();
    let data = WorldgenData::load(&generated.join("data/minecraft/worldgen")).unwrap();
    let configs: Vec<(&str, i64)> = vec![
        ("minecraft:overworld", 12345),
        ("minecraft:overworld", -4172144997902289642),
        ("minecraft:amplified", 777),
        ("minecraft:large_biomes", 4242),
        ("minecraft:nether", 12345),
        ("minecraft:end", 12345),
        ("minecraft:caves", 99),
        ("minecraft:floating_islands", 99),
    ];
    let (mut compared, mut bad) = (0, Vec::new());
    for (name, seed) in configs {
        let settings = GeneratorSettings::parse(&data.noise_settings[name]).unwrap();
        let state = RandomState::new(&data, &settings, seed).unwrap();
        let r = &state.router;
        let fields = [
            ("barrier", &r.barrier),
            ("floodedness", &r.fluid_level_floodedness),
            ("spread", &r.fluid_level_spread),
            ("lava", &r.lava),
            ("temperature", &r.temperature),
            ("vegetation", &r.vegetation),
            ("continents", &r.continents),
            ("erosion", &r.erosion),
            ("depth", &r.depth),
            ("ridges", &r.ridges),
            ("initial", &r.initial_density_without_jaggedness),
            ("final", &r.final_density),
            ("vein_toggle", &r.vein_toggle),
            ("vein_ridged", &r.vein_ridged),
            ("vein_gap", &r.vein_gap),
        ];
        for i in 0..48i32 {
            let (mut x, mut z, mut y) = ((i * 7919) % 6001 - 3000, (i * 104729) % 6007 - 3000, (i * 31) % 380 - 60);
            if i < 6 {
                x = i * 16;
                z = -i * 16;
                y = i * 50 - 40;
            }
            for (field, f) in &fields {
                let key = format!("{name}@{seed}.{i}.{field}");
                let expected = reference[key.as_str()];
                let actual = f.compute(&Ctx::point(x, y, z)).to_bits() as i64;
                compared += 1;
                if expected != actual {
                    bad.push(format!("{key} at ({x},{y},{z}): vanilla {} Rust {}", f64::from_bits(expected as u64), f64::from_bits(actual as u64)));
                }
            }
        }
    }
    assert!(compared > 0);
    assert!(bad.is_empty(), "{} of {compared} router values differ, first ones:\n{}", bad.len(), bad.iter().take(10).cloned().collect::<Vec<_>>().join("\n"));
    eprintln!("all {compared} router values are bit-identical");
}
