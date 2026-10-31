//! Compares the terrain stage (noise router + `NoiseChunk` + aquifer + ore veins) with the real Mojang `NoiseChunk`.
//! The reference blocks come from `tools/terrain/.../TerrainRef.java` (`tools/verify_terrain.sh`); skipped when they are missing.

use mc_data::Blocks;
use mc_worldgen::noise_chunk::NoiseChunk;
use mc_worldgen::random_state::{GeneratorSettings, NoiseSettings, RandomState, TerrainBlocks, WorldgenData};
use serde_json::Value;
use std::path::Path;

fn block_state(blocks: &Blocks, v: &Value) -> u16 {
    let name = v["Name"].as_str().unwrap();
    let props: Vec<(String, String)> = v
        .get("Properties")
        .and_then(Value::as_object)
        .map(|o| o.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned())).collect())
        .unwrap_or_default();
    blocks.state_id(name, props.iter().map(|(k, v)| (k.as_str(), v.as_str()))).unwrap_or_else(|| panic!("unknown block state {name}")) as u16
}

#[test]
fn terrain_matches_the_real_noise_chunk() {
    let work = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference/work");
    let generated = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference/gen/generated");
    let Ok(manifest) = std::fs::read_to_string(work.join("terrain_manifest.txt")) else {
        eprintln!("terrain_manifest.txt not found (run tools/verify_terrain.sh): skipped");
        return;
    };
    let blocks = Blocks::load(&generated.join("reports/blocks.json")).unwrap();
    let names: Vec<String> = blocks.list.iter().map(|b| b.name.clone()).collect();
    let _ = names;
    let data = WorldgenData::load(&generated.join("data/minecraft/worldgen")).unwrap();
    let st = |n: &str| blocks.default_state(n).unwrap_or_else(|| panic!("missing {n}")) as u16;
    let mut total_chunks = 0;
    let mut bad_chunks = Vec::new();
    for line in manifest.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        let (file, settings_name, seed) = (f[0], f[1], f[2].parse::<i64>().unwrap());
        let (acc_min_y, acc_height) = (f[3].parse::<i32>().unwrap(), f[4].parse::<i32>().unwrap());
        let settings = GeneratorSettings::parse(&data.noise_settings[settings_name]).unwrap();
        let tb = TerrainBlocks {
            air: st("minecraft:air"),
            lava: st("minecraft:lava"),
            default_block: block_state(&blocks, &settings.default_block),
            default_fluid: block_state(&blocks, &settings.default_fluid),
            copper_ore: st("minecraft:copper_ore"),
            raw_copper_block: st("minecraft:raw_copper_block"),
            granite: st("minecraft:granite"),
            deepslate_iron_ore: st("minecraft:deepslate_iron_ore"),
            raw_iron_block: st("minecraft:raw_iron_block"),
            tuff: st("minecraft:tuff"),
        };
        let state = RandomState::new(&data, &settings, seed).unwrap();
        // NoiseSettings.clampToHeightAccessor
        let min = settings.noise.min_y.max(acc_min_y);
        let height = (settings.noise.min_y + settings.noise.height).min(acc_min_y + acc_height) - min;
        let ns = NoiseSettings { min_y: min, height, ..settings.noise };
        let reference = std::fs::read(work.join(file)).unwrap();
        let chunk_bytes = 8 + (16 * 16 * height as usize) * 2;
        for (i, c) in f[5..].iter().enumerate() {
            let (cx, cz) = c.split_once(',').map(|(a, b)| (a.parse::<i32>().unwrap(), b.parse::<i32>().unwrap())).unwrap();
            let r = &reference[i * chunk_bytes..(i + 1) * chunk_bytes];
            assert_eq!((i32::from_be_bytes(r[0..4].try_into().unwrap()), i32::from_be_bytes(r[4..8].try_into().unwrap())), (cx, cz));
            let expected: Vec<u16> = r[8..].chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect();
            let nc = NoiseChunk::new(&state, &settings, ns, tb, cx * 16, cz * 16);
            let actual = nc.fill(cx * 16, cz * 16);
            total_chunks += 1;
            let diffs: Vec<usize> = (0..expected.len()).filter(|&k| expected[k] != actual[k]).collect();
            if !diffs.is_empty() {
                let k = diffs[0];
                let name = |id: u16| blocks.block_of_state(id as u32).map_or("?".to_owned(), |b| b.name.clone());
                bad_chunks.push(format!(
                    "{file} {settings_name} seed {seed} chunk ({cx},{cz}): {} of {} blocks differ, first at (x={}, y={}, z={}): vanilla {} Rust {}",
                    diffs.len(),
                    expected.len(),
                    k & 15,
                    (k >> 8) as i32 + ns.min_y,
                    (k >> 4) & 15,
                    name(expected[k]),
                    name(actual[k])
                ));
            }
        }
    }
    assert!(total_chunks > 0);
    assert!(bad_chunks.is_empty(), "{} of {} chunks differ from the real NoiseChunk:\n{}", bad_chunks.len(), total_chunks, bad_chunks.join("\n"));
    eprintln!("all {total_chunks} chunks identical to the real NoiseChunk");
}
