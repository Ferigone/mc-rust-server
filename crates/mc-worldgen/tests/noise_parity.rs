//! Compares the Rust random sources and noise functions with the real Mojang classes, bit for bit.
//! The reference values come from `tools/noise/NoiseRef.java` (`tools/verify_noise.sh`); the call sequence below must stay in sync
//! with it. The test is skipped when the reference file has not been generated.

use mc_worldgen::noise::{BlendedNoise, ImprovedNoise, NormalNoise, PerlinNoise, SimplexNoise};
use mc_worldgen::random::{LegacyRandomSource, RandomSource, XoroshiroRandomSource};
use std::path::Path;

const SEEDS: [i64; 8] = [0, 1, -1, 12345, 0x0123456789ABCDEF, i64::MIN, i64::MAX, 2718281828459045];
const NOISE_SEEDS: [i64; 3] = [0, 12345, -1];
const POINTS: [[f64; 3]; 8] = [
    [0.0, 0.0, 0.0],
    [0.5, 0.5, 0.5],
    [1.234, -5.678, 9.1011],
    [100.1, 64.7, -200.3],
    [-1000.001, 12.5, 3333.3],
    [3.3554432E7 + 0.5, 0.0, 0.0],
    [-0.0001, -0.0001, -0.0001],
    [12345.6789, -64.0, -9876.54321],
];
const BLOCKS: [[i32; 3]; 5] = [[0, 0, 0], [100, 64, -100], [-17, -5, 333], [1234, 200, -4321], [-30000000, -64, 30000000]];

#[derive(Default)]
struct Out(Vec<(String, i64)>);

impl Out {
    fn e(&mut self, k: String, v: i64) {
        self.0.push((k, v));
    }
    fn d(&mut self, k: String, v: f64) {
        self.e(k, v.to_bits() as i64);
    }
    fn f(&mut self, k: String, v: f32) {
        self.e(k, v.to_bits() as i32 as i64);
    }
}

fn make(kind: &str, seed: i64) -> Box<dyn RandomSource> {
    if kind == "legacy" { Box::new(LegacyRandomSource::new(seed)) } else { Box::new(XoroshiroRandomSource::new(seed)) }
}

fn random(o: &mut Out, kind: &str, seed: i64) {
    let mut r = make(kind, seed);
    let p = format!("rnd.{kind}.{seed}.");
    o.e(format!("{p}int"), r.next_int() as i64);
    o.e(format!("{p}int7"), r.next_int_bound(7) as i64);
    o.e(format!("{p}int16"), r.next_int_bound(16) as i64);
    o.e(format!("{p}int256"), r.next_int_bound(256) as i64);
    o.e(format!("{p}int1000003"), r.next_int_bound(1000003) as i64);
    o.e(format!("{p}long"), r.next_long());
    o.e(format!("{p}bool"), r.next_boolean() as i64);
    o.f(format!("{p}float"), r.next_float());
    o.d(format!("{p}double"), r.next_double());
    o.d(format!("{p}gauss1"), r.next_gaussian());
    o.d(format!("{p}gauss2"), r.next_gaussian());
    o.d(format!("{p}gauss3"), r.next_gaussian());
    o.e(format!("{p}between"), r.next_int_between_inclusive(-5, 5) as i64);
    o.d(format!("{p}triangle"), r.triangle(1.0, 2.0));
    let mut fk = r.fork();
    o.e(format!("{p}fork.long"), fk.next_long());
    let pf = r.fork_positional();
    o.e(format!("{p}pos.at1"), pf.at(1, -2, 3).next_long());
    o.e(format!("{p}pos.at2"), pf.at(-30000000, 64, 30000000).next_long());
    o.e(format!("{p}pos.hash1"), pf.from_hash_of("minecraft:test").next_long());
    o.e(format!("{p}pos.hash2"), pf.from_hash_of("octave_-7").next_long());
    o.e(format!("{p}pos.hash3"), pf.from_hash_of("\u{fc}n\u{ef}:\u{1F48E}").next_long());
    o.e(format!("{p}pos.seed"), pf.from_seed(5).next_long());
    r.set_seed(seed.wrapping_add(1));
    o.e(format!("{p}reseed"), r.next_long());
    r.consume_count(10);
    o.e(format!("{p}consume"), r.next_long());
}

fn improved(o: &mut Out, kind: &str, seed: i64) {
    let n = ImprovedNoise::new(&mut *make(kind, seed));
    let p = format!("improved.{kind}.{seed}.");
    o.d(format!("{p}xo"), n.xo);
    o.d(format!("{p}yo"), n.yo);
    o.d(format!("{p}zo"), n.zo);
    for (i, q) in POINTS.iter().enumerate() {
        o.d(format!("{p}{i}.noise"), n.noise(q[0], q[1], q[2]));
        o.d(format!("{p}{i}.scaled"), n.noise_with_scale(q[0], q[1], q[2], 0.5, 1.0));
        let mut der = [0.0; 3];
        o.d(format!("{p}{i}.deriv"), n.noise_with_derivative(q[0], q[1], q[2], &mut der));
        o.d(format!("{p}{i}.dx"), der[0]);
        o.d(format!("{p}{i}.dy"), der[1]);
        o.d(format!("{p}{i}.dz"), der[2]);
    }
}

fn perlin(o: &mut Out, name: &str, n: &PerlinNoise, p: &str) {
    o.d(format!("{p}{name}.max"), n.max_broken_value(0.0));
    for (i, q) in POINTS.iter().enumerate() {
        o.d(format!("{p}{name}.{i}"), n.get_value(q[0], q[1], q[2]));
    }
}

fn noises(o: &mut Out, seed: i64) {
    let p = format!("noise.{seed}.");
    perlin(o, "perlin", &PerlinNoise::create(&mut XoroshiroRandomSource::new(seed), -7, &[1.0, 1.0, 0.0, 1.5, 0.5]), &p);
    perlin(o, "perlin_octaves", &PerlinNoise::create_from_octaves(&mut XoroshiroRandomSource::new(seed), &[-4, -3, -1, 0]), &p);
    let octaves: Vec<i32> = (-15..=0).collect();
    perlin(o, "perlin_legacy_blended", &PerlinNoise::create_legacy_for_blended_noise(&mut XoroshiroRandomSource::new(seed), &octaves), &p);
    let nn = NormalNoise::create(&mut XoroshiroRandomSource::new(seed), -6, &[1.0, 1.0, 0.5, 0.0, 0.25]);
    o.d(format!("{p}normal.max"), nn.max_value());
    let nl = NormalNoise::create_legacy_nether_biome(&mut LegacyRandomSource::new(seed), -7, &[1.0, 1.0]);
    o.d(format!("{p}normal_legacy.max"), nl.max_value());
    let sx = SimplexNoise::new(&mut LegacyRandomSource::new(seed));
    for (i, q) in POINTS.iter().enumerate() {
        o.d(format!("{p}normal.{i}"), nn.get_value(q[0], q[1], q[2]));
        o.d(format!("{p}normal_legacy.{i}"), nl.get_value(q[0], q[1], q[2]));
        o.d(format!("{p}simplex2.{i}"), sx.get_value_2d(q[0], q[2]));
        o.d(format!("{p}simplex3.{i}"), sx.get_value_3d(q[0], q[1], q[2]));
    }
    let bn = BlendedNoise::new(&mut XoroshiroRandomSource::new(seed), 0.25, 0.125, 80.0, 160.0, 8.0);
    o.d(format!("{p}blended.min"), bn.min_value());
    o.d(format!("{p}blended.max"), bn.max_value());
    for (i, b) in BLOCKS.iter().enumerate() {
        o.d(format!("{p}blended.{i}"), bn.compute(b[0], b[1], b[2]));
    }
}

#[test]
fn matches_the_real_mojang_classes() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference/work/noise_ref.txt");
    let Ok(text) = std::fs::read_to_string(path) else {
        eprintln!("noise_ref.txt not found (run tools/verify_noise.sh): skipped");
        return;
    };
    let mut o = Out::default();
    for s in SEEDS {
        random(&mut o, "legacy", s);
        random(&mut o, "xoroshiro", s);
    }
    for s in NOISE_SEEDS {
        improved(&mut o, "xoroshiro", s);
        improved(&mut o, "legacy", s);
        noises(&mut o, s);
    }
    let reference: Vec<(&str, i64)> = text.lines().map(|l| l.split_once('\t').map(|(k, v)| (k, v.parse().unwrap())).unwrap()).collect();
    assert_eq!(o.0.len(), reference.len(), "number of values");
    let mut bad = Vec::new();
    for ((k, v), (rk, rv)) in o.0.iter().zip(&reference) {
        assert_eq!(k, rk, "sequence out of sync");
        if v != rv {
            bad.push(format!("{k}: rust {v:#x} vanilla {rv:#x}"));
        }
    }
    assert!(bad.is_empty(), "{} of {} values differ, first ones:\n{}", bad.len(), reference.len(), bad.iter().take(12).cloned().collect::<Vec<_>>().join("\n"));
}
