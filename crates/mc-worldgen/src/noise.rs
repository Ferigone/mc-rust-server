//! Noise functions (`net.minecraft.world.level.levelgen.synth`): `ImprovedNoise`, `PerlinNoise`, `NormalNoise`,
//! `SimplexNoise` and `BlendedNoise`. Floating point operations keep the order of the Java code so results are bit-identical.

use crate::mth::{self, clamped_lerp, lerp2, lerp3, smoothstep, smoothstep_derivative};
use crate::random::{LegacyRandomSource, RandomSource, XoroshiroRandomSource};

/// `SimplexNoise.GRADIENT`
pub const GRADIENT: [[i32; 3]; 16] = [
    [1, 1, 0],
    [-1, 1, 0],
    [1, -1, 0],
    [-1, -1, 0],
    [1, 0, 1],
    [-1, 0, 1],
    [1, 0, -1],
    [-1, 0, -1],
    [0, 1, 1],
    [0, -1, 1],
    [0, 1, -1],
    [0, -1, -1],
    [1, 1, 0],
    [0, -1, 1],
    [-1, 1, 0],
    [0, -1, -1],
];

fn dot(g: &[i32; 3], x: f64, y: f64, z: f64) -> f64 {
    g[0] as f64 * x + g[1] as f64 * y + g[2] as f64 * z
}

/// `ImprovedNoise`: classic Perlin noise with a random permutation and a random offset.
pub struct ImprovedNoise {
    p: [u8; 256],
    pub xo: f64,
    pub yo: f64,
    pub zo: f64,
}

impl ImprovedNoise {
    pub fn new(random: &mut dyn RandomSource) -> Self {
        let xo = random.next_double() * 256.0;
        let yo = random.next_double() * 256.0;
        let zo = random.next_double() * 256.0;
        let mut p = [0u8; 256];
        for (i, v) in p.iter_mut().enumerate() {
            *v = i as u8;
        }
        for i in 0..256usize {
            let j = random.next_int_bound(256 - i as i32) as usize;
            p.swap(i, i + j);
        }
        Self { p, xo, yo, zo }
    }

    fn p(&self, i: i32) -> i32 {
        self.p[(i & 0xFF) as usize] as i32
    }

    pub fn noise(&self, x: f64, y: f64, z: f64) -> f64 {
        self.noise_with_scale(x, y, z, 0.0, 0.0)
    }

    /// `noise(x, y, z, yScale, yMax)`
    pub fn noise_with_scale(&self, x: f64, y: f64, z: f64, y_scale: f64, y_max: f64) -> f64 {
        let (dx, dy, dz) = (x + self.xo, y + self.yo, z + self.zo);
        let (ix, iy, iz) = (mth::floor(dx), mth::floor(dy), mth::floor(dz));
        let (fx, fy, fz) = (dx - ix as f64, dy - iy as f64, dz - iz as f64);
        let y_offset = if y_scale != 0.0 {
            let t = if y_max >= 0.0 && y_max < fy { y_max } else { fy };
            mth::floor(t / y_scale + 1.0E-7_f32 as f64) as f64 * y_scale
        } else {
            0.0
        };
        self.sample_and_lerp(ix, iy, iz, fx, fy - y_offset, fz, fy)
    }

    #[allow(clippy::too_many_arguments)]
    fn sample_and_lerp(&self, x: i32, y: i32, z: i32, fx: f64, fy: f64, fz: f64, fy_raw: f64) -> f64 {
        let p0 = self.p(x);
        let p1 = self.p(x + 1);
        let p00 = self.p(p0 + y);
        let p01 = self.p(p0 + y + 1);
        let p10 = self.p(p1 + y);
        let p11 = self.p(p1 + y + 1);
        let g = |h: i32, a: f64, b: f64, c: f64| dot(&GRADIENT[(h & 15) as usize], a, b, c);
        let d000 = g(self.p(p00 + z), fx, fy, fz);
        let d100 = g(self.p(p10 + z), fx - 1.0, fy, fz);
        let d010 = g(self.p(p01 + z), fx, fy - 1.0, fz);
        let d110 = g(self.p(p11 + z), fx - 1.0, fy - 1.0, fz);
        let d001 = g(self.p(p00 + z + 1), fx, fy, fz - 1.0);
        let d101 = g(self.p(p10 + z + 1), fx - 1.0, fy, fz - 1.0);
        let d011 = g(self.p(p01 + z + 1), fx, fy - 1.0, fz - 1.0);
        let d111 = g(self.p(p11 + z + 1), fx - 1.0, fy - 1.0, fz - 1.0);
        let sx = smoothstep(fx);
        let sy = smoothstep(fy_raw);
        let sz = smoothstep(fz);
        lerp3(sx, sy, sz, d000, d100, d010, d110, d001, d101, d011, d111)
    }

    /// `noiseWithDerivative`: returns the value and adds the gradient to `derivative`.
    pub fn noise_with_derivative(&self, x: f64, y: f64, z: f64, derivative: &mut [f64; 3]) -> f64 {
        let (dx, dy, dz) = (x + self.xo, y + self.yo, z + self.zo);
        let (ix, iy, iz) = (mth::floor(dx), mth::floor(dy), mth::floor(dz));
        let (fx, fy, fz) = (dx - ix as f64, dy - iy as f64, dz - iz as f64);
        let p0 = self.p(ix);
        let p1 = self.p(ix + 1);
        let p00 = self.p(p0 + iy);
        let p01 = self.p(p0 + iy + 1);
        let p10 = self.p(p1 + iy);
        let p11 = self.p(p1 + iy + 1);
        let g000 = &GRADIENT[(self.p(p00 + iz) & 15) as usize];
        let g100 = &GRADIENT[(self.p(p10 + iz) & 15) as usize];
        let g010 = &GRADIENT[(self.p(p01 + iz) & 15) as usize];
        let g110 = &GRADIENT[(self.p(p11 + iz) & 15) as usize];
        let g001 = &GRADIENT[(self.p(p00 + iz + 1) & 15) as usize];
        let g101 = &GRADIENT[(self.p(p10 + iz + 1) & 15) as usize];
        let g011 = &GRADIENT[(self.p(p01 + iz + 1) & 15) as usize];
        let g111 = &GRADIENT[(self.p(p11 + iz + 1) & 15) as usize];
        let d000 = dot(g000, fx, fy, fz);
        let d100 = dot(g100, fx - 1.0, fy, fz);
        let d010 = dot(g010, fx, fy - 1.0, fz);
        let d110 = dot(g110, fx - 1.0, fy - 1.0, fz);
        let d001 = dot(g001, fx, fy, fz - 1.0);
        let d101 = dot(g101, fx - 1.0, fy, fz - 1.0);
        let d011 = dot(g011, fx, fy - 1.0, fz - 1.0);
        let d111 = dot(g111, fx - 1.0, fy - 1.0, fz - 1.0);
        let (sx, sy, sz) = (smoothstep(fx), smoothstep(fy), smoothstep(fz));
        let comp = |c: usize| lerp3(sx, sy, sz, g000[c] as f64, g100[c] as f64, g010[c] as f64, g110[c] as f64, g001[c] as f64, g101[c] as f64, g011[c] as f64, g111[c] as f64);
        let (gx, gy, gz) = (comp(0), comp(1), comp(2));
        let ddx = lerp2(sy, sz, d100 - d000, d110 - d010, d101 - d001, d111 - d011);
        let ddy = lerp2(sz, sx, d010 - d000, d011 - d001, d110 - d100, d111 - d101);
        let ddz = lerp2(sx, sy, d001 - d000, d101 - d100, d011 - d010, d111 - d110);
        let (sdx, sdy, sdz) = (smoothstep_derivative(fx), smoothstep_derivative(fy), smoothstep_derivative(fz));
        derivative[0] += gx + sdx * ddx;
        derivative[1] += gy + sdy * ddy;
        derivative[2] += gz + sdz * ddz;
        lerp3(sx, sy, sz, d000, d100, d010, d110, d001, d101, d011, d111)
    }
}

/// `PerlinNoise.wrap`
pub fn wrap(v: f64) -> f64 {
    v - mth::lfloor(v / 3.3554432E7 + 0.5) as f64 * 3.3554432E7
}

/// `PerlinNoise`: several octaves of `ImprovedNoise`.
pub struct PerlinNoise {
    levels: Vec<Option<ImprovedNoise>>,
    first_octave: i32,
    amplitudes: Vec<f64>,
    lowest_freq_value_factor: f64,
    lowest_freq_input_factor: f64,
    max_value: f64,
}

impl PerlinNoise {
    /// `create(random, firstOctave, amplitudes)`: every octave gets its own random from the positional factory
    /// (`octave_<n>`).
    pub fn create(random: &mut dyn RandomSource, first_octave: i32, amplitudes: &[f64]) -> Self {
        Self::build(random, first_octave, amplitudes.to_vec(), true)
    }

    /// `create(random, IntStream octaves)`
    pub fn create_from_octaves(random: &mut dyn RandomSource, octaves: &[i32]) -> Self {
        let (first, amps) = make_amplitudes(octaves);
        Self::build(random, first, amps, true)
    }

    /// `createLegacyForBlendedNoise`: octaves drawn one after the other from the same random.
    pub fn create_legacy_for_blended_noise(random: &mut dyn RandomSource, octaves: &[i32]) -> Self {
        let (first, amps) = make_amplitudes(octaves);
        Self::build(random, first, amps, false)
    }

    /// `createLegacyForLegacyNetherBiome`
    pub fn create_legacy_for_legacy_nether_biome(random: &mut dyn RandomSource, first_octave: i32, amplitudes: &[f64]) -> Self {
        Self::build(random, first_octave, amplitudes.to_vec(), false)
    }

    fn build(random: &mut dyn RandomSource, first_octave: i32, amplitudes: Vec<f64>, new_init: bool) -> Self {
        let n = amplitudes.len() as i32;
        let neg = -first_octave;
        let mut levels: Vec<Option<ImprovedNoise>> = (0..n).map(|_| None).collect();
        if new_init {
            let factory = random.fork_positional();
            for i in 0..n {
                if amplitudes[i as usize] != 0.0 {
                    let octave = first_octave + i;
                    levels[i as usize] = Some(ImprovedNoise::new(&mut *factory.from_hash_of(&format!("octave_{octave}"))));
                }
            }
        } else {
            let first = ImprovedNoise::new(random);
            if neg >= 0 && neg < n && amplitudes[neg as usize] != 0.0 {
                levels[neg as usize] = Some(first);
            }
            for i in (0..neg).rev() {
                if i < n {
                    if amplitudes[i as usize] != 0.0 {
                        levels[i as usize] = Some(ImprovedNoise::new(random));
                    } else {
                        random.consume_count(262);
                    }
                } else {
                    random.consume_count(262);
                }
            }
            let non_null = levels.iter().filter(|l| l.is_some()).count();
            let non_zero = amplitudes.iter().filter(|&&a| a != 0.0).count();
            assert_eq!(non_null, non_zero, "Failed to create correct number of noise levels for given non-zero amplitudes");
            assert!(neg >= n - 1, "Positive octaves are temporarily disabled");
        }
        let lowest_freq_input_factor = 2.0f64.powi(-neg);
        let lowest_freq_value_factor = 2.0f64.powi(n - 1) / (2.0f64.powi(n) - 1.0);
        let mut s = Self { levels, first_octave, amplitudes, lowest_freq_value_factor, lowest_freq_input_factor, max_value: 0.0 };
        s.max_value = s.edge_value(2.0);
        s
    }

    pub fn max_value(&self) -> f64 {
        self.max_value
    }

    pub fn first_octave(&self) -> i32 {
        self.first_octave
    }

    pub fn amplitudes(&self) -> &[f64] {
        &self.amplitudes
    }

    pub fn get_value(&self, x: f64, y: f64, z: f64) -> f64 {
        self.get_value_full(x, y, z, 0.0, 0.0, false)
    }

    pub fn get_value_full(&self, x: f64, y: f64, z: f64, y_scale: f64, y_max: f64, fix_y: bool) -> f64 {
        let mut total = 0.0;
        let mut input = self.lowest_freq_input_factor;
        let mut value = self.lowest_freq_value_factor;
        for (i, level) in self.levels.iter().enumerate() {
            if let Some(n) = level {
                let v = n.noise_with_scale(wrap(x * input), if fix_y { -n.yo } else { wrap(y * input) }, wrap(z * input), y_scale * input, y_max * input);
                total += self.amplitudes[i] * v * value;
            }
            input *= 2.0;
            value /= 2.0;
        }
        total
    }

    pub fn max_broken_value(&self, y_scale: f64) -> f64 {
        self.edge_value(y_scale + 2.0)
    }

    fn edge_value(&self, noise_value: f64) -> f64 {
        let mut total = 0.0;
        let mut value = self.lowest_freq_value_factor;
        for (i, level) in self.levels.iter().enumerate() {
            if level.is_some() {
                total += self.amplitudes[i] * noise_value * value;
            }
            value /= 2.0;
        }
        total
    }

    /// `getOctaveNoise(i)`: octaves counted from the highest frequency
    pub fn octave_noise(&self, i: usize) -> Option<&ImprovedNoise> {
        self.levels[self.levels.len() - 1 - i].as_ref()
    }
}

/// `PerlinNoise.makeAmplitudes`: the sorted set of octaves becomes (first octave, amplitudes of 1.0 where present)
fn make_amplitudes(octaves: &[i32]) -> (i32, Vec<f64>) {
    let mut sorted: Vec<i32> = octaves.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    assert!(!sorted.is_empty(), "Need some octaves!");
    let neg = -sorted[0];
    let last = *sorted.last().unwrap();
    let total = neg + last + 1;
    assert!(total >= 1, "Total number of octaves needs to be >= 1");
    let mut amps = vec![0.0; total as usize];
    for o in sorted {
        amps[(o + neg) as usize] = 1.0;
    }
    (-neg, amps)
}

/// `NormalNoise`: the sum of two `PerlinNoise` instances, scaled to a fixed expected deviation.
pub struct NormalNoise {
    value_factor: f64,
    first: PerlinNoise,
    second: PerlinNoise,
    max_value: f64,
}

impl NormalNoise {
    const INPUT_FACTOR: f64 = 1.0181268882175227;

    pub fn create(random: &mut dyn RandomSource, first_octave: i32, amplitudes: &[f64]) -> Self {
        let first = PerlinNoise::create(random, first_octave, amplitudes);
        let second = PerlinNoise::create(random, first_octave, amplitudes);
        Self::finish(first, second, amplitudes)
    }

    pub fn create_legacy_nether_biome(random: &mut dyn RandomSource, first_octave: i32, amplitudes: &[f64]) -> Self {
        let first = PerlinNoise::create_legacy_for_legacy_nether_biome(random, first_octave, amplitudes);
        let second = PerlinNoise::create_legacy_for_legacy_nether_biome(random, first_octave, amplitudes);
        Self::finish(first, second, amplitudes)
    }

    fn finish(first: PerlinNoise, second: PerlinNoise, amplitudes: &[f64]) -> Self {
        let (mut min, mut max) = (i32::MAX, i32::MIN);
        for (i, &a) in amplitudes.iter().enumerate() {
            if a != 0.0 {
                min = min.min(i as i32);
                max = max.max(i as i32);
            }
        }
        let value_factor = 0.16666666666666666 / (0.1 * (1.0 + 1.0 / ((max - min + 1) as f64)));
        let max_value = (first.max_value() + second.max_value()) * value_factor;
        Self { value_factor, first, second, max_value }
    }

    pub fn max_value(&self) -> f64 {
        self.max_value
    }

    pub fn get_value(&self, x: f64, y: f64, z: f64) -> f64 {
        let (x2, y2, z2) = (x * Self::INPUT_FACTOR, y * Self::INPUT_FACTOR, z * Self::INPUT_FACTOR);
        (self.first.get_value(x, y, z) + self.second.get_value(x2, y2, z2)) * self.value_factor
    }
}

/// `SimplexNoise`
pub struct SimplexNoise {
    p: [i32; 512],
    pub xo: f64,
    pub yo: f64,
    pub zo: f64,
}

const SQRT_3: f64 = 1.7320508075688772;
const F2: f64 = 0.5 * (SQRT_3 - 1.0);
const G2: f64 = (3.0 - SQRT_3) / 6.0;

impl SimplexNoise {
    pub fn new(random: &mut dyn RandomSource) -> Self {
        let xo = random.next_double() * 256.0;
        let yo = random.next_double() * 256.0;
        let zo = random.next_double() * 256.0;
        let mut p = [0i32; 512];
        for (i, v) in p.iter_mut().enumerate().take(256) {
            *v = i as i32;
        }
        for i in 0..256usize {
            let j = random.next_int_bound(256 - i as i32) as usize;
            p.swap(i, i + j);
        }
        Self { p, xo, yo, zo }
    }

    fn p(&self, i: i32) -> i32 {
        self.p[(i & 0xFF) as usize]
    }

    fn corner_noise_3d(index: i32, x: f64, y: f64, z: f64, base: f64) -> f64 {
        let mut t = base - x * x - y * y - z * z;
        if t < 0.0 {
            0.0
        } else {
            t *= t;
            t * t * dot(&GRADIENT[index as usize], x, y, z)
        }
    }

    pub fn get_value_2d(&self, x: f64, y: f64) -> f64 {
        let s = (x + y) * F2;
        let i = mth::floor(x + s);
        let j = mth::floor(y + s);
        let t = (i + j) as f64 * G2;
        let x0 = x - (i as f64 - t);
        let y0 = y - (j as f64 - t);
        let (i1, j1) = if x0 > y0 { (1, 0) } else { (0, 1) };
        let x1 = x0 - i1 as f64 + G2;
        let y1 = y0 - j1 as f64 + G2;
        let x2 = x0 - 1.0 + 2.0 * G2;
        let y2 = y0 - 1.0 + 2.0 * G2;
        let ii = i & 0xFF;
        let jj = j & 0xFF;
        let gi0 = self.p(ii + self.p(jj)) % 12;
        let gi1 = self.p(ii + i1 + self.p(jj + j1)) % 12;
        let gi2 = self.p(ii + 1 + self.p(jj + 1)) % 12;
        let n0 = Self::corner_noise_3d(gi0, x0, y0, 0.0, 0.5);
        let n1 = Self::corner_noise_3d(gi1, x1, y1, 0.0, 0.5);
        let n2 = Self::corner_noise_3d(gi2, x2, y2, 0.0, 0.5);
        70.0 * (n0 + n1 + n2)
    }

    pub fn get_value_3d(&self, x: f64, y: f64, z: f64) -> f64 {
        let s = (x + y + z) * 0.3333333333333333;
        let i = mth::floor(x + s);
        let j = mth::floor(y + s);
        let k = mth::floor(z + s);
        let t = (i + j + k) as f64 * 0.16666666666666666;
        let x0 = x - (i as f64 - t);
        let y0 = y - (j as f64 - t);
        let z0 = z - (k as f64 - t);
        let (i1, j1, k1, i2, j2, k2) = if x0 >= y0 {
            if y0 >= z0 {
                (1, 0, 0, 1, 1, 0)
            } else if x0 >= z0 {
                (1, 0, 0, 1, 0, 1)
            } else {
                (0, 0, 1, 1, 0, 1)
            }
        } else if y0 < z0 {
            (0, 0, 1, 0, 1, 1)
        } else if x0 < z0 {
            (0, 1, 0, 0, 1, 1)
        } else {
            (0, 1, 0, 1, 1, 0)
        };
        let x1 = x0 - i1 as f64 + 0.16666666666666666;
        let y1 = y0 - j1 as f64 + 0.16666666666666666;
        let z1 = z0 - k1 as f64 + 0.16666666666666666;
        let x2 = x0 - i2 as f64 + 0.3333333333333333;
        let y2 = y0 - j2 as f64 + 0.3333333333333333;
        let z2 = z0 - k2 as f64 + 0.3333333333333333;
        let x3 = x0 - 1.0 + 0.5;
        let y3 = y0 - 1.0 + 0.5;
        let z3 = z0 - 1.0 + 0.5;
        let ii = i & 0xFF;
        let jj = j & 0xFF;
        let kk = k & 0xFF;
        let gi0 = self.p(ii + self.p(jj + self.p(kk))) % 12;
        let gi1 = self.p(ii + i1 + self.p(jj + j1 + self.p(kk + k1))) % 12;
        let gi2 = self.p(ii + i2 + self.p(jj + j2 + self.p(kk + k2))) % 12;
        let gi3 = self.p(ii + 1 + self.p(jj + 1 + self.p(kk + 1))) % 12;
        let n0 = Self::corner_noise_3d(gi0, x0, y0, z0, 0.6);
        let n1 = Self::corner_noise_3d(gi1, x1, y1, z1, 0.6);
        let n2 = Self::corner_noise_3d(gi2, x2, y2, z2, 0.6);
        let n3 = Self::corner_noise_3d(gi3, x3, y3, z3, 0.6);
        32.0 * (n0 + n1 + n2 + n3)
    }
}

/// `BlendedNoise`: the legacy terrain noise (`old_blended_noise`).
pub struct BlendedNoise {
    min_limit: PerlinNoise,
    max_limit: PerlinNoise,
    main: PerlinNoise,
    xz_multiplier: f64,
    y_multiplier: f64,
    xz_factor: f64,
    y_factor: f64,
    smear_scale_multiplier: f64,
    max_value: f64,
}

impl BlendedNoise {
    pub fn new(random: &mut dyn RandomSource, xz_scale: f64, y_scale: f64, xz_factor: f64, y_factor: f64, smear_scale_multiplier: f64) -> Self {
        let octaves16: Vec<i32> = (-15..=0).collect();
        let octaves8: Vec<i32> = (-7..=0).collect();
        let min_limit = PerlinNoise::create_legacy_for_blended_noise(random, &octaves16);
        let max_limit = PerlinNoise::create_legacy_for_blended_noise(random, &octaves16);
        let main = PerlinNoise::create_legacy_for_blended_noise(random, &octaves8);
        let xz_multiplier = 684.412 * xz_scale;
        let y_multiplier = 684.412 * y_scale;
        let max_value = min_limit.max_broken_value(y_multiplier);
        Self { min_limit, max_limit, main, xz_multiplier, y_multiplier, xz_factor, y_factor, smear_scale_multiplier, max_value }
    }

    /// `BlendedNoise.createUnseeded`
    pub fn create_unseeded(xz_scale: f64, y_scale: f64, xz_factor: f64, y_factor: f64, smear: f64) -> Self {
        Self::new(&mut XoroshiroRandomSource::new(0), xz_scale, y_scale, xz_factor, y_factor, smear)
    }

    pub fn min_value(&self) -> f64 {
        -self.max_value
    }

    pub fn max_value(&self) -> f64 {
        self.max_value
    }

    pub fn compute(&self, block_x: i32, block_y: i32, block_z: i32) -> f64 {
        let x = block_x as f64 * self.xz_multiplier;
        let y = block_y as f64 * self.y_multiplier;
        let z = block_z as f64 * self.xz_multiplier;
        let xf = x / self.xz_factor;
        let yf = y / self.y_factor;
        let zf = z / self.xz_factor;
        let smear = self.y_multiplier * self.smear_scale_multiplier;
        let smear_f = smear / self.y_factor;
        let mut min_v = 0.0;
        let mut max_v = 0.0;
        let mut main_v = 0.0;
        let mut scale = 1.0;
        for i in 0..8 {
            if let Some(n) = self.main.octave_noise(i) {
                main_v += n.noise_with_scale(wrap(xf * scale), wrap(yf * scale), wrap(zf * scale), smear_f * scale, yf * scale) / scale;
            }
            scale /= 2.0;
        }
        let t = (main_v / 10.0 + 1.0) / 2.0;
        let use_max_only = t >= 1.0;
        let use_min_only = t <= 0.0;
        scale = 1.0;
        for i in 0..16 {
            let wx = wrap(x * scale);
            let wy = wrap(y * scale);
            let wz = wrap(z * scale);
            let s = smear * scale;
            if !use_max_only {
                if let Some(n) = self.min_limit.octave_noise(i) {
                    min_v += n.noise_with_scale(wx, wy, wz, s, y * scale) / scale;
                }
            }
            if !use_min_only {
                if let Some(n) = self.max_limit.octave_noise(i) {
                    max_v += n.noise_with_scale(wx, wy, wz, s, y * scale) / scale;
                }
            }
            scale /= 2.0;
        }
        clamped_lerp(min_v / 512.0, max_v / 512.0, t) / 128.0
    }
}

/// `PerlinSimplexNoise`: octaves of 2D simplex noise (used for the temperature noise of the biomes)
pub struct PerlinSimplexNoise {
    levels: Vec<Option<SimplexNoise>>,
    highest_freq_value_factor: f64,
    highest_freq_input_factor: f64,
}

impl PerlinSimplexNoise {
    /// `octaves` are the (distinct) octave numbers, e.g. `[-2, -1, 0]`.
    pub fn new(random: &mut dyn RandomSource, octaves: &[i32]) -> Self {
        let mut set: Vec<i32> = octaves.to_vec();
        set.sort_unstable();
        set.dedup();
        assert!(!set.is_empty(), "Need some octaves!");
        let first = -set[0];
        let last = *set.last().unwrap();
        let total = first + last + 1;
        assert!(total >= 1, "Total number of octaves needs to be >= 1");
        let base = SimplexNoise::new(random);
        let mut levels: Vec<Option<SimplexNoise>> = (0..total).map(|_| None).collect();
        let mut base_slot = None;
        if last >= 0 && last < total && set.contains(&0) {
            base_slot = Some(last as usize);
        }
        for i in (last + 1)..total {
            if i >= 0 && set.contains(&(last - i)) {
                levels[i as usize] = Some(SimplexNoise::new(random));
            } else {
                random.consume_count(262);
            }
        }
        if last > 0 {
            let seed = (base.get_value_3d(base.xo, base.yo, base.zo) * (9.223372E18f32 as f64)) as i64;
            let mut r = LegacyRandomSource::new(seed);
            for i in (0..last).rev() {
                if i < total && set.contains(&(last - i)) {
                    levels[i as usize] = Some(SimplexNoise::new(&mut r));
                } else {
                    r.consume_count(262);
                }
            }
        }
        if let Some(slot) = base_slot {
            levels[slot] = Some(base);
        }
        Self {
            levels,
            highest_freq_input_factor: 2f64.powi(last),
            highest_freq_value_factor: 1.0 / (2f64.powi(total) - 1.0),
        }
    }

    /// `getValue(x, y, useNoiseOffsets)`
    pub fn get_value(&self, x: f64, y: f64, use_offsets: bool) -> f64 {
        let mut sum = 0.0;
        let mut input = self.highest_freq_input_factor;
        let mut value = self.highest_freq_value_factor;
        for level in &self.levels {
            if let Some(n) = level {
                let (ox, oy) = if use_offsets { (n.xo, n.yo) } else { (0.0, 0.0) };
                sum += n.get_value_2d(x * input + ox, y * input + oy) * value;
            }
            input /= 2.0;
            value *= 2.0;
        }
        sum
    }
}
