//! Compares the surface stage (biomes, `BiomeManager` zoom, `SurfaceSystem`, `SurfaceRules`, badlands / iceberg extensions) and the
//! carvers with the real Mojang classes: blocks and `WORLD_SURFACE_WG` heightmap of the first chunk found for every biome.
//! The reference comes from `tools/terrain/.../SurfaceRef.java` (`tools/verify_surface.sh`, `tools/verify_carvers.sh`); skipped when it is missing.

use mc_data::{BlockProps, Blocks};
use mc_worldgen::biome_manager::{BiomeManager, ChunkBiomes};
use mc_worldgen::carver::{apply_carvers, CarveEnv, CarveStates, CarverSet, CarvingMask};
use mc_worldgen::biome_source::BiomeSource;
use mc_worldgen::noise_chunk::NoiseChunk;
use mc_worldgen::random_state::{GeneratorSettings, NoiseSettings, RandomState, TerrainBlocks, WorldgenData};
use mc_worldgen::surface::{GameBlockQuery, GenContext, SurfaceChunk, SurfaceRule, SurfaceStates, SurfaceSystem};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
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

struct Reader<'a>(&'a [u8], usize);

impl Reader<'_> {
    fn take(&mut self, n: usize) -> &[u8] {
        let s = &self.0[self.1..self.1 + n];
        self.1 += n;
        s
    }
    fn i32_be(&mut self) -> i32 {
        i32::from_be_bytes(self.take(4).try_into().unwrap())
    }
}

#[test]
fn surface_matches_the_real_surface_system() {
    compare("surface_manifest.txt", false);
}

#[test]
fn carvers_match_the_real_carvers() {
    compare("carver_manifest.txt", true);
}

fn compare(manifest_file: &str, carve: bool) {
    let work = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference/work");
    let generated = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference/gen/generated");
    let Ok(manifest) = std::fs::read_to_string(work.join(manifest_file)) else {
        eprintln!("{manifest_file} not found (run tools/verify_surface.sh / tools/verify_carvers.sh): skipped");
        return;
    };
    let blocks = Blocks::load(&generated.join("reports/blocks.json")).unwrap();
    let props = BlockProps::load(&generated.join("reports/block_props.tsv")).unwrap();
    let query = GameBlockQuery { blocks: &blocks, props: &props };
    let states = SurfaceStates::resolve(&blocks).unwrap();
    let data = WorldgenData::load(&generated.join("data/minecraft/worldgen")).unwrap();
    let st = |n: &str| blocks.default_state(n).unwrap_or_else(|| panic!("missing {n}")) as u16;
    let carvers = CarverSet::load(&generated, &blocks).unwrap();
    let carve_states = CarveStates::resolve(&blocks).unwrap();
    let mut carved_blocks = 0usize;
    let name = |id: u16| blocks.block_of_state(id as u32).map_or("?".to_owned(), |b| b.name.clone());
    let (mut total, mut bad) = (0usize, Vec::new());
    let mut placed: BTreeMap<String, usize> = BTreeMap::new();
    let mut biomes_seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut pairs: BTreeMap<(String, String), usize> = BTreeMap::new();
    for line in manifest.lines() {
        // file settings seed kind min_y height max_ring step
        let f: Vec<&str> = line.split_whitespace().collect();
        let (file, settings_name, seed, kind) = (f[0], f[1], f[2].parse::<i64>().unwrap(), f[3]);
        let (acc_min_y, acc_height) = (f[4].parse::<i32>().unwrap(), f[5].parse::<i32>().unwrap());
        let (max_ring, step) = (f[6].parse::<i32>().unwrap(), f[7].parse::<usize>().unwrap());
        let settings = GeneratorSettings::parse(&data.noise_settings[settings_name]).unwrap();
        let state = RandomState::new(&data, &settings, seed).unwrap();
        let source = match kind {
            "nether" => BiomeSource::nether(),
            "end" => BiomeSource::End,
            _ => BiomeSource::overworld(),
        };
        // the reference picks the first chunk of every biome with this scan; replay it (same chunks, same R-tree search history)
        let mut chosen: Vec<(i32, i32, &'static str)> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut ring = 0;
        while ring <= max_ring {
            let mut dx = -ring;
            while dx <= ring {
                let mut dz = -ring;
                while dz <= ring {
                    if dx.abs().max(dz.abs()) == ring {
                        let b = source.get_noise_biome(dx * 4 + 2, 16, dz * 4 + 2, &state.sampler);
                        if seen.insert(b) {
                            chosen.push((dx, dz, b));
                        }
                    }
                    dz += step as i32;
                }
                dx += step as i32;
            }
            ring += step as i32;
        }
        let reference = std::fs::read(work.join(file)).unwrap();
        let mut r = Reader(&reference, 0);
        let count = r.i32_be() as usize;
        assert_eq!(count, chosen.len(), "{file}: the scan finds {} chunks, the reference has {count}", chosen.len());

        let gen_range = GenContext { min_y: acc_min_y.max(settings.noise.min_y), height: acc_height.min(settings.noise.height) };
        let default_block = block_state(&blocks, &settings.default_block);
        let rule = SurfaceRule::compile(&settings.surface_rule, &state, &blocks, gen_range).unwrap();
        let system = SurfaceSystem::new(&state, default_block, settings.sea_level, states).unwrap();
        let tb = TerrainBlocks {
            air: st("minecraft:air"),
            lava: st("minecraft:lava"),
            default_block,
            default_fluid: block_state(&blocks, &settings.default_fluid),
            copper_ore: st("minecraft:copper_ore"),
            raw_copper_block: st("minecraft:raw_copper_block"),
            granite: st("minecraft:granite"),
            deepslate_iron_ore: st("minecraft:deepslate_iron_ore"),
            raw_iron_block: st("minecraft:raw_iron_block"),
            tuff: st("minecraft:tuff"),
        };
        // NoiseSettings.clampToHeightAccessor
        let min = settings.noise.min_y.max(acc_min_y);
        let height = (settings.noise.min_y + settings.noise.height).min(acc_min_y + acc_height) - min;
        let ns = NoiseSettings { min_y: min, height, ..settings.noise };
        let climate = |n: &str| data.biomes.get(n).cloned();
        for &(cx, cz, centre) in &chosen {
            let (rcx, rcz) = (r.i32_be(), r.i32_be());
            let len = r.take(1)[0] as usize;
            let rname = String::from_utf8(r.take(len).to_vec()).unwrap();
            assert_eq!((rcx, rcz, rname.as_str()), (cx, cz, centre), "{file}: chunk list differs from the reference");
            let expected: Vec<u16> = r.take(16 * 16 * acc_height as usize * 2).chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect();
            let expected_hm: Vec<i32> = r.take(256 * 4).chunks_exact(4).map(|b| i32::from_le_bytes(b.try_into().unwrap())).collect();

            // biomes of the 3x3 neighbourhood
            let mut grids: HashMap<(i32, i32), ChunkBiomes> = HashMap::new();
            for dx in -1..=1 {
                for dz in -1..=1 {
                    let g = ChunkBiomes::fill(cx + dx, cz + dz, acc_min_y, acc_height, |qx, qy, qz| source.get_noise_biome(qx, qy, qz, &state.sampler));
                    grids.insert((cx + dx, cz + dz), g);
                }
            }
            let quart_source = |qx: i32, qy: i32, qz: i32| grids[&(qx >> 2, qz >> 2)].get_noise_biome(qx, qy, qz);
            let manager = BiomeManager::new(&quart_source, seed);

            let nc = NoiseChunk::new(&state, &settings, ns, tb, cx * 16, cz * 16);
            let filled = nc.fill(cx * 16, cz * 16);
            let mut all = vec![tb.air; 16 * 16 * acc_height as usize];
            let offset = ((ns.min_y - acc_min_y) as usize) * 256;
            all[offset..offset + filled.len()].copy_from_slice(&filled);
            let mut chunk = SurfaceChunk::new(cx, cz, acc_min_y, acc_height, all, &query);
            let biome_at = |x: i32, y: i32, z: i32| manager.get_biome(x, y, z);
            system.build_surface(&mut chunk, &nc, &rule, &biome_at, &climate, &query, settings.legacy_random_source);
            if carve {
                // ChunkGenerator.applyCarvers: the biome manager asks the biome source directly (no chunk grids)
                let direct = |qx: i32, qy: i32, qz: i32| source.get_noise_biome(qx, qy, qz, &state.sampler);
                let carve_manager = BiomeManager::new(&direct, seed);
                let carve_biome_at = |x: i32, y: i32, z: i32| carve_manager.get_biome(x, y, z);
                let env = CarveEnv { range: gen_range, noise_chunk: &nc, system: &system, rule: &rule, biome_at: &carve_biome_at, climate: &climate, query: &query, states: carve_states };
                let mut mask = CarvingMask::new(acc_height, acc_min_y);
                apply_carvers(&env, &mut chunk, &mut mask, seed, &source, &state.sampler, &carvers);
                carved_blocks += mask.count();
            }

            // the reference reads ChunkAccess.getBlockState
            let observed = chunk.vanilla_view(&query, tb.air);
            total += 1;
            *biomes_seen.entry(centre.to_owned()).or_default() += 1;
            for &s in &observed {
                *placed.entry(name(s)).or_default() += 1;
            }
            let diffs: Vec<usize> = (0..expected.len()).filter(|&k| expected[k] != observed[k]).collect();
            for &k in &diffs {
                *pairs.entry((name(expected[k]), name(observed[k]))).or_default() += 1;
            }
            let hm_ok = (0..256).all(|i| chunk.height_at(i & 15, i >> 4) == expected_hm[i as usize]);
            if !diffs.is_empty() || !hm_ok {
                let k = diffs.first().copied().unwrap_or(0);
                bad.push(format!(
                    "{file} {settings_name}@{seed} chunk ({cx},{cz}) [{centre}]: {} blocks differ{}, first at (x={}, y={}, z={}): vanilla {} Rust {}; heightmap {}",
                    diffs.len(),
                    "",
                    cx * 16 + (k & 15) as i32,
                    (k >> 8) as i32 + acc_min_y,
                    cz * 16 + ((k >> 4) & 15) as i32,
                    name(expected[k]),
                    name(observed[k]),
                    if hm_ok { "identical" } else { "DIFFERENT" }
                ));
            }
        }
        assert_eq!(r.1, reference.len(), "{file}: trailing data");
    }
    assert!(total > 0);
    eprintln!("{total} chunks compared, {} block types occur, {} biomes at the chunk centres", placed.len(), biomes_seen.len());
    if carve {
        eprintln!("{carved_blocks} blocks were carved");
        assert!(carved_blocks > 10_000, "the carvers should have carved many blocks, got {carved_blocks}");
    }
    if !pairs.is_empty() {
        let mut list: Vec<_> = pairs.iter().collect();
        list.sort_by_key(|e| std::cmp::Reverse(*e.1));
        eprintln!("most frequent differences (vanilla -> Rust):");
        for ((a, b), n) in list.iter().take(12) {
            eprintln!("  {n:8}  {a} -> {b}");
        }
    }
    assert!(bad.is_empty(), "{} of {total} chunks differ from the real surface system:\n{}", bad.len(), bad.join("\n"));
    eprintln!("all {total} chunks (blocks and WORLD_SURFACE_WG heightmaps) are identical to the real {}", if carve { "surface system and carvers" } else { "surface system" });
}
