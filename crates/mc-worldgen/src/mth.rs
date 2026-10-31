//! The parts of `net.minecraft.util.Mth` the generators use (same operation order, so results are bit-identical).

/// `Mth.floor`: Java's `(int)` cast saturates and maps NaN to 0, like Rust's `as`.
pub fn floor(v: f64) -> i32 {
    let i = v as i32;
    if v < i as f64 { i.wrapping_sub(1) } else { i }
}

/// `Mth.lfloor`
pub fn lfloor(v: f64) -> i64 {
    let i = v as i64;
    if v < i as f64 { i.wrapping_sub(1) } else { i }
}

pub fn lerp(t: f64, a: f64, b: f64) -> f64 {
    a + t * (b - a)
}

pub fn lerp2(t1: f64, t2: f64, v00: f64, v10: f64, v01: f64, v11: f64) -> f64 {
    lerp(t2, lerp(t1, v00, v10), lerp(t1, v01, v11))
}

#[allow(clippy::too_many_arguments)]
pub fn lerp3(t1: f64, t2: f64, t3: f64, v000: f64, v100: f64, v010: f64, v110: f64, v001: f64, v101: f64, v011: f64, v111: f64) -> f64 {
    lerp(t3, lerp2(t1, t2, v000, v100, v010, v110), lerp2(t1, t2, v001, v101, v011, v111))
}

pub fn smoothstep(x: f64) -> f64 {
    x * x * x * (x * (x * 6.0 - 15.0) + 10.0)
}

pub fn smoothstep_derivative(x: f64) -> f64 {
    30.0 * x * x * (x - 1.0) * (x - 1.0)
}

pub fn clamped_lerp(a: f64, b: f64, t: f64) -> f64 {
    if t < 0.0 {
        a
    } else if t > 1.0 {
        b
    } else {
        lerp(t, a, b)
    }
}

pub fn square(v: f64) -> f64 {
    v * v
}

/// `Mth.getSeed(int, int, int)` (position hash used by the positional random factories).
pub fn get_seed(x: i32, y: i32, z: i32) -> i64 {
    let mut seed = (x.wrapping_mul(3129871) as i64) ^ (z as i64).wrapping_mul(116129781) ^ (y as i64);
    seed = seed.wrapping_mul(seed).wrapping_mul(42317861).wrapping_add(seed.wrapping_mul(11));
    seed >> 16
}

/// `String.hashCode()` (UTF-16 code units).
pub fn java_string_hash(s: &str) -> i32 {
    s.encode_utf16().fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(c as i32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_and_seed_hash_follow_java() {
        // Java: (int) saturates, NaN is 0; and for -1e300 `MIN_VALUE - 1` wraps around to MAX_VALUE
        assert_eq!((floor(-0.5), floor(0.5), floor(-1.0), floor(f64::NAN), floor(1e300), floor(-1e300)), (-1, 0, -1, 0, i32::MAX, i32::MAX));
        assert_eq!((lfloor(-0.5), lfloor(2.5)), (-1, 2));
        // "minecraft:test".hashCode() and a string with a surrogate pair
        assert_eq!(java_string_hash("abc"), 96354);
        assert_eq!(java_string_hash(""), 0);
        assert_eq!(java_string_hash("\u{1F48E}"), 0xD83D * 31 + 0xDC8E);
    }
}
