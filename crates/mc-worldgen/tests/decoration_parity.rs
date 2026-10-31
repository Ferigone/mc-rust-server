//! Compares the feature decoration with the real Mojang classes: the 3x3 chunks around the first chunk found for every biome, after the real noise,
//! surface, carver and feature stages. The reference comes from `tools/terrain/.../DecorateRef.java` (`tools/verify_features.sh`) and places exactly
//! the placed features listed by `examples/supported_features.rs`; skipped when it is missing.

use mc_data::GameData;
use mc_worldgen::biome_manager::ChunkBiomes;
use mc_worldgen::generator::{BiomeSourceKind, NoiseGenerator};
use mc_worldgen::surface::{BlockQuery, GameBlockQuery};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::{Arc, Mutex};

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

/// `ProtoChunk.getBlockState` answers plain air for every block of a section that holds only air-class blocks.
fn vanilla_view(blocks: &[u16], q: &dyn BlockQuery, air: u16) -> Vec<u16> {
    let mut out = blocks.to_vec();
    for section in out.chunks_mut(4096) {
        if section.iter().all(|&b| q.is_air(b)) {
            section.fill(air);
        }
    }
    out
}

#[derive(Default)]
struct Stats {
    centres: usize,
    chunks: usize,
    changed_blocks: usize,
    /// block name -> number of blocks the decoration placed (vanilla's view)
    placed: BTreeMap<String, usize>,
    bad: Vec<String>,
    pairs: BTreeMap<(String, String), usize>,
}

#[test]
fn decoration_matches_the_real_decoration() {
    let work = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference/work");
    let generated = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference/gen/generated");
    let Ok(manifest) = std::fs::read_to_string(work.join("decorate_manifest.txt")) else {
        eprintln!("decorate_manifest.txt not found (run tools/verify_features.sh): skipped");
        return;
    };
    let game = Arc::new(GameData::load(&generated).unwrap());
    let query = GameBlockQuery { blocks: &game.blocks, props: &game.block_props };
    let air = game.blocks.default_state("minecraft:air").unwrap() as u16;
    let name = |id: u16| game.blocks.block_of_state(id as u32).map_or("?".to_owned(), |b| b.name.clone());
    let stats = Mutex::new(Stats::default());
    let threads = std::thread::available_parallelism().map_or(2, |n| n.get()).min(8);

    for line in manifest.lines() {
        // file settings seed kind min_y height max_ring step
        let f: Vec<&str> = line.split_whitespace().collect();
        let (file, settings_name, seed, kind) = (f[0], f[1], f[2].parse::<i64>().unwrap(), f[3]);
        let (min_y, height) = (f[4].parse::<i32>().unwrap(), f[5].parse::<i32>().unwrap());
        let (max_ring, step) = (f[6].parse::<i32>().unwrap(), f[7].parse::<i32>().unwrap());
        let source_kind = match kind {
            "nether" => BiomeSourceKind::Nether,
            "end" => BiomeSourceKind::End,
            _ => BiomeSourceKind::Overworld,
        };
        let generator = NoiseGenerator::new(game.clone(), min_y, height, seed, settings_name, source_kind).unwrap();

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
                        let b = generator.source().get_noise_biome(dx * 4 + 2, 16, dz * 4 + 2, generator.sampler());
                        if seen.insert(b) {
                            chosen.push((dx, dz, b));
                        }
                    }
                    dz += step;
                }
                dx += step;
            }
            ring += step;
        }
        let reference = std::fs::read(work.join(file)).unwrap();
        let mut r = Reader(&reference, 0);
        let count = r.i32_be() as usize;
        assert_eq!(count, chosen.len(), "{file}: the scan finds {} centres, the reference has {count}", chosen.len());
        let chunk_len = 16 * 16 * height as usize;
        // the reference file: read sequentially, compare in parallel
        let mut expected: Vec<(i32, i32, &str, Vec<Vec<u16>>)> = Vec::new();
        for &(cx, cz, centre) in &chosen {
            let (rcx, rcz) = (r.i32_be(), r.i32_be());
            let len = r.take(1)[0] as usize;
            let rname = String::from_utf8(r.take(len).to_vec()).unwrap();
            assert_eq!((rcx, rcz, rname.as_str()), (cx, cz, centre), "{file}: centre list differs from the reference");
            let chunks: Vec<Vec<u16>> =
                (0..9).map(|_| r.take(chunk_len * 2).chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect()).collect();
            expected.push((cx, cz, centre, chunks));
        }
        assert_eq!(r.1, reference.len(), "{file}: trailing data");

        let next = std::sync::atomic::AtomicUsize::new(0);
        std::thread::scope(|sc| {
            for _ in 0..threads {
                sc.spawn(|| loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some((cx, cz, centre, reference_chunks)) = expected.get(i) else { return };
                    let (cx, cz) = (*cx, *cz);
                    let cache: RefCell<HashMap<(i32, i32), Arc<ChunkBiomes>>> = RefCell::new(HashMap::new());
                    let grid = |x: i32, z: i32| -> Arc<ChunkBiomes> {
                        if let Some(g) = cache.borrow().get(&(x, z)) {
                            return g.clone();
                        }
                        let g = Arc::new(generator.fill_biomes(x, z));
                        cache.borrow_mut().insert((x, z), g.clone());
                        g
                    };
                    let mut carved = Vec::new();
                    for dx in -1..=1 {
                        for dz in -1..=1 {
                            carved.push((cx + dx, cz + dz, generator.carved_chunk(cx + dx, cz + dz, &grid)));
                        }
                    }
                    let decorated = generator.decorate((cx, cz), carved.clone(), &grid);
                    let mut local = Stats { centres: 1, ..Stats::default() };
                    for (k, (dx, dz)) in (-1..=1).flat_map(|dx| (-1..=1).map(move |dz| (dx, dz))).enumerate() {
                        let (_, _, before) = carved.iter().find(|c| c.0 == cx + dx && c.1 == cz + dz).unwrap();
                        let (_, _, after) = decorated.iter().find(|c| c.0 == cx + dx && c.1 == cz + dz).unwrap();
                        let observed = vanilla_view(after, &query, air);
                        let before_view = vanilla_view(before, &query, air);
                        let want = &reference_chunks[k];
                        local.chunks += 1;
                        for j in 0..want.len() {
                            if before_view[j] != want[j] {
                                local.changed_blocks += 1;
                                *local.placed.entry(name(want[j])).or_default() += 1;
                            }
                        }
                        let diffs: Vec<usize> = (0..want.len()).filter(|&j| want[j] != observed[j]).collect();
                        for &j in &diffs {
                            *local.pairs.entry((name(want[j]), name(observed[j]))).or_default() += 1;
                        }
                        if let Some(&j) = diffs.first() {
                            local.bad.push(format!(
                                "{file} {settings_name}@{seed} centre ({cx},{cz}) [{centre}] chunk ({},{}): {} blocks differ, first at (x={}, y={}, z={}): vanilla {} Rust {} (before the decoration: {})",
                                cx + dx,
                                cz + dz,
                                diffs.len(),
                                (cx + dx) * 16 + (j & 15) as i32,
                                (j >> 8) as i32 + min_y,
                                (cz + dz) * 16 + ((j >> 4) & 15) as i32,
                                name(want[j]),
                                name(observed[j]),
                                name(before_view[j])
                            ));
                        }
                    }
                    let mut total = stats.lock().unwrap();
                    total.centres += local.centres;
                    total.chunks += local.chunks;
                    total.changed_blocks += local.changed_blocks;
                    total.bad.extend(local.bad);
                    for (k, v) in local.placed {
                        *total.placed.entry(k).or_default() += v;
                    }
                    for (k, v) in local.pairs {
                        *total.pairs.entry(k).or_default() += v;
                    }
                });
            }
        });
        eprintln!("{file}: {} centres compared", chosen.len());
    }

    let stats = stats.into_inner().unwrap();
    assert!(stats.centres > 0);
    eprintln!("{} centres ({} chunks) compared; the decoration changed {} blocks", stats.centres, stats.chunks, stats.changed_blocks);
    let mut placed: Vec<_> = stats.placed.iter().collect();
    placed.sort_by_key(|e| std::cmp::Reverse(*e.1));
    eprintln!("blocks placed by the decoration: {}", placed.iter().take(14).map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", "));
    assert!(stats.changed_blocks > 50_000, "the decoration should place many blocks, got {}", stats.changed_blocks);
    if !stats.pairs.is_empty() {
        let mut list: Vec<_> = stats.pairs.iter().collect();
        list.sort_by_key(|e| std::cmp::Reverse(*e.1));
        eprintln!("most frequent differences (vanilla -> Rust):");
        for ((a, b), n) in list.iter().take(12) {
            eprintln!("  {n:8}  {a} -> {b}");
        }
    }
    let mut bad = stats.bad;
    bad.sort();
    assert!(bad.is_empty(), "{} chunks differ from the real decoration, first ones:\n{}", bad.len(), bad.iter().take(10).cloned().collect::<Vec<_>>().join("\n"));
    eprintln!("all {} chunks are identical to the real feature decoration", stats.chunks);
}
