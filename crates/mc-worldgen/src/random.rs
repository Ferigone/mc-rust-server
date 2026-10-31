//! `RandomSource` implementations (`LegacyRandomSource`, `XoroshiroRandomSource`) and their positional factories.

use crate::mth;
use md5::{Digest, Md5};

/// `BitRandomSource.FLOAT_MULTIPLIER` / `XoroshiroRandomSource.FLOAT_UNIT`
const FLOAT_UNIT: f32 = 5.9604645E-8;
/// `1.110223E-16F` widened to double, exactly as Java does when it multiplies a double by this float constant
const DOUBLE_UNIT: f64 = 1.110223E-16_f32 as f64;

const GOLDEN_RATIO_64: i64 = -7046029254386353131;
const SILVER_RATIO_64: i64 = 7640891576956012809;

/// `net.minecraft.util.RandomSource`
pub trait RandomSource {
    fn fork(&mut self) -> Box<dyn RandomSource>;
    fn fork_positional(&mut self) -> PositionalRandomFactory;
    fn set_seed(&mut self, seed: i64);
    fn next_int(&mut self) -> i32;
    fn next_int_bound(&mut self, bound: i32) -> i32;
    fn next_long(&mut self) -> i64;
    fn next_boolean(&mut self) -> bool;
    fn next_float(&mut self) -> f32;
    fn next_double(&mut self) -> f64;
    fn next_gaussian(&mut self) -> f64;
    /// `consumeCount`: advances the generator like `count` calls of `nextInt()` (`nextLong()` for Xoroshiro)
    fn consume_count(&mut self, count: i32) {
        for _ in 0..count {
            self.next_int();
        }
    }
    fn next_int_between_inclusive(&mut self, lo: i32, hi: i32) -> i32 {
        self.next_int_bound(hi - lo + 1) + lo
    }
    fn triangle(&mut self, center: f64, spread: f64) -> f64 {
        let a = self.next_double();
        let b = self.next_double();
        center + spread * (a - b)
    }
}

/// `MarsagliaPolarGaussian`
#[derive(Default, Clone)]
struct Gaussian {
    next_next: f64,
    have_next: bool,
}

impl Gaussian {
    fn next(&mut self, mut next_double: impl FnMut() -> f64) -> f64 {
        if self.have_next {
            self.have_next = false;
            return self.next_next;
        }
        loop {
            let a = 2.0 * next_double() - 1.0;
            let b = 2.0 * next_double() - 1.0;
            let s = mth::square(a) + mth::square(b);
            if s >= 1.0 || s == 0.0 {
                continue;
            }
            let m = (-2.0 * s.ln() / s).sqrt();
            self.next_next = b * m;
            self.have_next = true;
            return a * m;
        }
    }
}

/// `LegacyRandomSource`: `java.util.Random`'s 48-bit linear congruential generator.
#[derive(Clone)]
pub struct LegacyRandomSource {
    seed: i64,
    gaussian: Gaussian,
}

impl LegacyRandomSource {
    const MASK: i64 = 281474976710655;
    const MULTIPLIER: i64 = 25214903917;

    pub fn new(seed: i64) -> Self {
        let mut r = Self { seed: 0, gaussian: Gaussian::default() };
        r.set_seed(seed);
        r
    }

    /// `BitRandomSource.next`
    pub fn next_bits(&mut self, bits: u32) -> i32 {
        self.seed = self.seed.wrapping_mul(Self::MULTIPLIER).wrapping_add(11) & Self::MASK;
        (self.seed >> (48 - bits)) as i32
    }
}

impl RandomSource for LegacyRandomSource {
    fn fork(&mut self) -> Box<dyn RandomSource> {
        Box::new(LegacyRandomSource::new(self.next_long()))
    }
    fn fork_positional(&mut self) -> PositionalRandomFactory {
        PositionalRandomFactory::Legacy { seed: self.next_long() }
    }
    fn set_seed(&mut self, seed: i64) {
        self.seed = (seed ^ Self::MULTIPLIER) & Self::MASK;
        self.gaussian.have_next = false;
    }
    fn next_int(&mut self) -> i32 {
        self.next_bits(32)
    }
    fn next_int_bound(&mut self, bound: i32) -> i32 {
        assert!(bound > 0, "Bound must be positive");
        if bound & (bound - 1) == 0 {
            return ((bound as i64).wrapping_mul(self.next_bits(31) as i64) >> 31) as i32;
        }
        loop {
            let bits = self.next_bits(31);
            let val = bits % bound;
            if bits.wrapping_sub(val).wrapping_add(bound - 1) >= 0 {
                return val;
            }
        }
    }
    fn next_long(&mut self) -> i64 {
        let hi = self.next_bits(32);
        let lo = self.next_bits(32);
        ((hi as i64) << 32).wrapping_add(lo as i64)
    }
    fn next_boolean(&mut self) -> bool {
        self.next_bits(1) != 0
    }
    fn next_float(&mut self) -> f32 {
        self.next_bits(24) as f32 * FLOAT_UNIT
    }
    fn next_double(&mut self) -> f64 {
        let hi = self.next_bits(26) as i64;
        let lo = self.next_bits(27) as i64;
        (((hi << 27) + lo) as f64) * DOUBLE_UNIT
    }
    fn next_gaussian(&mut self) -> f64 {
        let mut g = std::mem::take(&mut self.gaussian);
        let v = g.next(|| self.next_double());
        self.gaussian = g;
        v
    }
}

/// `WorldgenRandom` over a `XoroshiroRandomSource`: the class extends `LegacyRandomSource`, so every method is the legacy algorithm
/// on top of `next(bits)`, which here takes the high bits of the inner generator's `nextLong()`. `setSeed` only reseeds the inner
/// generator (the cached Gaussian of the wrapper survives it, as in vanilla).
pub struct WorldgenRandom {
    inner: XoroshiroRandomSource,
    gaussian: Gaussian,
}

impl WorldgenRandom {
    pub fn new(seed: i64) -> Self {
        Self { inner: XoroshiroRandomSource::new(seed), gaussian: Gaussian::default() }
    }

    fn next(&mut self, bits: u32) -> i32 {
        ((self.inner.next_long() as u64) >> (64 - bits)) as i32
    }

    /// `setDecorationSeed(worldSeed, x, z)`: returns the decoration seed
    pub fn set_decoration_seed(&mut self, seed: i64, x: i32, z: i32) -> i64 {
        self.set_seed(seed);
        let a = self.next_long() | 1;
        let b = self.next_long() | 1;
        let r = (x as i64).wrapping_mul(a).wrapping_add((z as i64).wrapping_mul(b)) ^ seed;
        self.set_seed(r);
        r
    }

    /// `setFeatureSeed(decorationSeed, index, step)`
    pub fn set_feature_seed(&mut self, decoration_seed: i64, index: i32, step: i32) {
        self.set_seed(decoration_seed.wrapping_add(index as i64).wrapping_add(10000i64.wrapping_mul(step as i64)));
    }
}

impl RandomSource for WorldgenRandom {
    fn fork(&mut self) -> Box<dyn RandomSource> {
        self.inner.fork()
    }
    fn fork_positional(&mut self) -> PositionalRandomFactory {
        self.inner.fork_positional()
    }
    fn set_seed(&mut self, seed: i64) {
        self.inner.set_seed(seed);
    }
    fn next_int(&mut self) -> i32 {
        self.next(32)
    }
    fn next_int_bound(&mut self, bound: i32) -> i32 {
        assert!(bound > 0, "Bound must be positive");
        if bound & (bound - 1) == 0 {
            return ((bound as i64).wrapping_mul(self.next(31) as i64) >> 31) as i32;
        }
        loop {
            let bits = self.next(31);
            let val = bits % bound;
            if bits.wrapping_sub(val).wrapping_add(bound - 1) >= 0 {
                return val;
            }
        }
    }
    fn next_long(&mut self) -> i64 {
        let hi = self.next(32);
        let lo = self.next(32);
        ((hi as i64) << 32).wrapping_add(lo as i64)
    }
    fn next_boolean(&mut self) -> bool {
        self.next(1) != 0
    }
    fn next_float(&mut self) -> f32 {
        self.next(24) as f32 * FLOAT_UNIT
    }
    fn next_double(&mut self) -> f64 {
        let hi = self.next(26) as i64;
        let lo = self.next(27) as i64;
        (((hi << 27) + lo) as f64) * DOUBLE_UNIT
    }
    fn next_gaussian(&mut self) -> f64 {
        let mut g = std::mem::take(&mut self.gaussian);
        let v = g.next(|| self.next_double());
        self.gaussian = g;
        v
    }
}

/// `Xoroshiro128PlusPlus`
#[derive(Clone)]
pub struct Xoroshiro128PlusPlus {
    lo: i64,
    hi: i64,
}

impl Xoroshiro128PlusPlus {
    pub fn new(lo: i64, hi: i64) -> Self {
        if lo | hi == 0 {
            return Self { lo: GOLDEN_RATIO_64, hi: SILVER_RATIO_64 };
        }
        Self { lo, hi }
    }

    pub fn next_long(&mut self) -> i64 {
        let (lo, mut hi) = (self.lo, self.hi);
        let result = lo.wrapping_add(hi).rotate_left(17).wrapping_add(lo);
        hi ^= lo;
        self.lo = lo.rotate_left(49) ^ hi ^ (hi << 21);
        self.hi = hi.rotate_left(28);
        result
    }
}

/// `RandomSupport.mixStafford13`
pub fn mix_stafford13(mut v: i64) -> i64 {
    v = (v ^ ((v as u64) >> 30) as i64).wrapping_mul(-4658895280553007687);
    v = (v ^ ((v as u64) >> 27) as i64).wrapping_mul(-7723592293110705685);
    v ^ ((v as u64) >> 31) as i64
}

/// `RandomSupport.upgradeSeedTo128bit` (the seed `(lo, hi)` after mixing)
pub fn upgrade_seed_to_128bit(seed: i64) -> (i64, i64) {
    let lo = seed ^ SILVER_RATIO_64;
    let hi = lo.wrapping_add(GOLDEN_RATIO_64);
    (mix_stafford13(lo), mix_stafford13(hi))
}

/// `RandomSupport.seedFromHashOf`: the two big-endian longs of the MD5 digest of the UTF-8 string.
pub fn seed_from_hash_of(s: &str) -> (i64, i64) {
    let digest = Md5::digest(s.as_bytes());
    (i64::from_be_bytes(digest[0..8].try_into().unwrap()), i64::from_be_bytes(digest[8..16].try_into().unwrap()))
}

/// `XoroshiroRandomSource`
#[derive(Clone)]
pub struct XoroshiroRandomSource {
    rng: Xoroshiro128PlusPlus,
    gaussian: Gaussian,
}

impl XoroshiroRandomSource {
    pub fn new(seed: i64) -> Self {
        let (lo, hi) = upgrade_seed_to_128bit(seed);
        Self::from_128bit(lo, hi)
    }

    pub fn from_128bit(lo: i64, hi: i64) -> Self {
        Self { rng: Xoroshiro128PlusPlus::new(lo, hi), gaussian: Gaussian::default() }
    }

    fn next_bits(&mut self, bits: u32) -> i64 {
        ((self.rng.next_long() as u64) >> (64 - bits)) as i64
    }
}

impl RandomSource for XoroshiroRandomSource {
    fn fork(&mut self) -> Box<dyn RandomSource> {
        let lo = self.rng.next_long();
        let hi = self.rng.next_long();
        Box::new(XoroshiroRandomSource::from_128bit(lo, hi))
    }
    fn fork_positional(&mut self) -> PositionalRandomFactory {
        let lo = self.rng.next_long();
        let hi = self.rng.next_long();
        PositionalRandomFactory::Xoroshiro { lo, hi }
    }
    fn set_seed(&mut self, seed: i64) {
        let (lo, hi) = upgrade_seed_to_128bit(seed);
        self.rng = Xoroshiro128PlusPlus::new(lo, hi);
        self.gaussian.have_next = false;
    }
    fn next_int(&mut self) -> i32 {
        self.rng.next_long() as i32
    }
    fn next_int_bound(&mut self, bound: i32) -> i32 {
        assert!(bound > 0, "Bound must be positive");
        let b = bound as i64;
        let mut r = self.next_int() as u32 as i64;
        let mut m = r * b;
        let mut low = m & 0xFFFF_FFFF;
        if low < b {
            let threshold = (!bound).wrapping_add(1) as u32 % bound as u32;
            while low < threshold as i64 {
                r = self.next_int() as u32 as i64;
                m = r * b;
                low = m & 0xFFFF_FFFF;
            }
        }
        (m >> 32) as i32
    }
    fn next_long(&mut self) -> i64 {
        self.rng.next_long()
    }
    fn next_boolean(&mut self) -> bool {
        self.rng.next_long() & 1 != 0
    }
    fn next_float(&mut self) -> f32 {
        self.next_bits(24) as f32 * FLOAT_UNIT
    }
    fn next_double(&mut self) -> f64 {
        self.next_bits(53) as f64 * DOUBLE_UNIT
    }
    fn next_gaussian(&mut self) -> f64 {
        let mut g = std::mem::take(&mut self.gaussian);
        let v = g.next(|| self.next_double());
        self.gaussian = g;
        v
    }
    fn consume_count(&mut self, count: i32) {
        for _ in 0..count {
            self.rng.next_long();
        }
    }
}

/// `PositionalRandomFactory`
#[derive(Clone, Debug)]
pub enum PositionalRandomFactory {
    Legacy { seed: i64 },
    Xoroshiro { lo: i64, hi: i64 },
}

impl PositionalRandomFactory {
    pub fn at(&self, x: i32, y: i32, z: i32) -> Box<dyn RandomSource> {
        let pos = mth::get_seed(x, y, z);
        match *self {
            Self::Legacy { seed } => Box::new(LegacyRandomSource::new(pos ^ seed)),
            Self::Xoroshiro { lo, hi } => Box::new(XoroshiroRandomSource::from_128bit(pos ^ lo, hi)),
        }
    }

    pub fn from_hash_of(&self, name: &str) -> Box<dyn RandomSource> {
        match *self {
            Self::Legacy { seed } => Box::new(LegacyRandomSource::new(mth::java_string_hash(name) as i64 ^ seed)),
            Self::Xoroshiro { lo, hi } => {
                let (a, b) = seed_from_hash_of(name);
                Box::new(XoroshiroRandomSource::from_128bit(a ^ lo, b ^ hi))
            }
        }
    }

    pub fn from_seed(&self, seed: i64) -> Box<dyn RandomSource> {
        match *self {
            Self::Legacy { .. } => Box::new(LegacyRandomSource::new(seed)),
            Self::Xoroshiro { lo, hi } => Box::new(XoroshiroRandomSource::from_128bit(seed ^ lo, seed ^ hi)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_matches_java_util_random() {
        // well-known values of `new java.util.Random(0)`
        let mut r = LegacyRandomSource::new(0);
        assert_eq!(r.next_int(), -1155484576);
        assert_eq!(r.next_int(), -723955400);
        let mut g = LegacyRandomSource::new(0);
        assert_eq!(g.next_gaussian(), 0.8025330637390305);
    }

    #[test]
    fn xoroshiro_zero_seed_state_is_replaced() {
        // an all-zero state is not allowed: it becomes (golden ratio, silver ratio)
        let mut a = Xoroshiro128PlusPlus::new(0, 0);
        let mut b = Xoroshiro128PlusPlus::new(GOLDEN_RATIO_64, SILVER_RATIO_64);
        assert_eq!(a.next_long(), b.next_long());
    }

    #[test]
    fn bounded_ints_stay_in_range() {
        for seed in [1, 2, 3] {
            let mut l = LegacyRandomSource::new(seed);
            let mut x = XoroshiroRandomSource::new(seed);
            for bound in [1, 2, 3, 7, 16, 100, 1000003, i32::MAX] {
                for _ in 0..200 {
                    assert!((0..bound).contains(&l.next_int_bound(bound)));
                    assert!((0..bound).contains(&x.next_int_bound(bound)));
                }
            }
        }
    }
}
