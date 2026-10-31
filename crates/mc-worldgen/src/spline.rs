//! `net.minecraft.util.CubicSpline` with 32-bit float arithmetic exactly like the Java code.

/// The value bounds of a spline coordinate.
pub trait SplineBounds {
    fn min_value(&self) -> f32;
    fn max_value(&self) -> f32;
}

/// A coordinate of a spline: yields a float for a context of type `C`.
pub trait SplineCoordinate<C>: SplineBounds {
    fn apply(&self, ctx: &C) -> f32;
}

pub enum CubicSpline<I> {
    Constant(f32),
    Multipoint { coordinate: I, locations: Vec<f32>, values: Vec<CubicSpline<I>>, derivatives: Vec<f32>, min: f32, max: f32 },
}

/// `Mth.lerp(float, float, float)`
fn lerp(t: f32, a: f32, b: f32) -> f32 {
    a + t * (b - a)
}

fn jmin(a: f32, b: f32) -> f32 {
    if a.is_nan() || b.is_nan() {
        return f32::NAN;
    }
    if a == 0.0 && b == 0.0 {
        return if a.is_sign_negative() { a } else { b };
    }
    if a < b { a } else { b }
}

fn jmax(a: f32, b: f32) -> f32 {
    if a.is_nan() || b.is_nan() {
        return f32::NAN;
    }
    if a == 0.0 && b == 0.0 {
        return if a.is_sign_positive() { a } else { b };
    }
    if a > b { a } else { b }
}

fn linear_extend(x: f32, locations: &[f32], value: f32, derivatives: &[f32], i: usize) -> f32 {
    let d = derivatives[i];
    if d == 0.0 { value } else { value + d * (x - locations[i]) }
}

/// `Mth.binarySearch(0, n, i -> x < locations[i]) - 1`
fn find_interval_start(locations: &[f32], x: f32) -> i32 {
    let (mut start, mut len) = (0i32, locations.len() as i32);
    while len > 0 {
        let half = len / 2;
        let mid = start + half;
        if x < locations[mid as usize] {
            len = half;
        } else {
            start = mid + 1;
            len -= half + 1;
        }
    }
    start - 1
}

impl<I: SplineBounds> CubicSpline<I> {
    /// `Multipoint.create`: computes the value bounds
    pub fn multipoint(coordinate: I, locations: Vec<f32>, values: Vec<CubicSpline<I>>, derivatives: Vec<f32>) -> Self {
        assert!(locations.len() == values.len() && locations.len() == derivatives.len(), "All lengths must be equal");
        assert!(!locations.is_empty(), "Cannot create a multipoint spline with no points");
        let last = locations.len() - 1;
        let mut min = f32::INFINITY;
        let mut max = f32::NEG_INFINITY;
        let cmin = coordinate.min_value();
        let cmax = coordinate.max_value();
        if cmin < locations[0] {
            let a = linear_extend(cmin, &locations, values[0].min_value(), &derivatives, 0);
            let b = linear_extend(cmin, &locations, values[0].max_value(), &derivatives, 0);
            min = jmin(min, jmin(a, b));
            max = jmax(max, jmax(a, b));
        }
        if cmax > locations[last] {
            let a = linear_extend(cmax, &locations, values[last].min_value(), &derivatives, last);
            let b = linear_extend(cmax, &locations, values[last].max_value(), &derivatives, last);
            min = jmin(min, jmin(a, b));
            max = jmax(max, jmax(a, b));
        }
        for v in &values {
            min = jmin(min, v.min_value());
            max = jmax(max, v.max_value());
        }
        for i in 0..last {
            let (x0, x1) = (locations[i], locations[i + 1]);
            let dx = x1 - x0;
            let (v0, v1) = (&values[i], &values[i + 1]);
            let (v0min, v0max, v1min, v1max) = (v0.min_value(), v0.max_value(), v1.min_value(), v1.max_value());
            let (d0, d1) = (derivatives[i], derivatives[i + 1]);
            if d0 != 0.0 || d1 != 0.0 {
                let s0 = d0 * dx;
                let s1 = d1 * dx;
                let lo = jmin(v0min, v1min);
                let hi = jmax(v0max, v1max);
                let a = s0 - v1max + v0min;
                let b = s0 - v1min + v0max;
                let c = -s1 + v1min - v0max;
                let d = -s1 + v1max - v0min;
                let e = jmin(a, c);
                let f = jmax(b, d);
                min = jmin(min, lo + 0.25 * e);
                max = jmax(max, hi + 0.25 * f);
            }
        }
        CubicSpline::Multipoint { coordinate, locations, values, derivatives, min, max }
    }

    pub fn min_value(&self) -> f32 {
        match self {
            CubicSpline::Constant(v) => *v,
            CubicSpline::Multipoint { min, .. } => *min,
        }
    }

    pub fn max_value(&self) -> f32 {
        match self {
            CubicSpline::Constant(v) => *v,
            CubicSpline::Multipoint { max, .. } => *max,
        }
    }

    pub fn apply<C>(&self, ctx: &C) -> f32
    where
        I: SplineCoordinate<C>,
    {
        match self {
            CubicSpline::Constant(v) => *v,
            CubicSpline::Multipoint { coordinate, locations, values, derivatives, .. } => {
                let x = coordinate.apply(ctx);
                let i = find_interval_start(locations, x);
                let last = locations.len() as i32 - 1;
                if i < 0 {
                    linear_extend(x, locations, values[0].apply(ctx), derivatives, 0)
                } else if i == last {
                    linear_extend(x, locations, values[last as usize].apply(ctx), derivatives, last as usize)
                } else {
                    let i = i as usize;
                    let (x0, x1) = (locations[i], locations[i + 1]);
                    let t = (x - x0) / (x1 - x0);
                    let (d0, d1) = (derivatives[i], derivatives[i + 1]);
                    let v0 = values[i].apply(ctx);
                    let v1 = values[i + 1].apply(ctx);
                    let a = d0 * (x1 - x0) - (v1 - v0);
                    let b = -d1 * (x1 - x0) + (v1 - v0);
                    lerp(t, v0, v1) + t * (1.0 - t) * lerp(t, a, b)
                }
            }
        }
    }

    /// `mapAll`: maps the coordinates and recomputes the bounds
    pub fn map_all(&self, f: &mut dyn FnMut(&I) -> I) -> CubicSpline<I> {
        match self {
            CubicSpline::Constant(v) => CubicSpline::Constant(*v),
            CubicSpline::Multipoint { coordinate, locations, values, derivatives, .. } => {
                let c = f(coordinate);
                let vals = values.iter().map(|v| v.map_all(f)).collect();
                CubicSpline::multipoint(c, locations.clone(), vals, derivatives.clone())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A coordinate that returns the context itself.
    struct Identity(f32, f32);
    impl SplineBounds for Identity {
        fn min_value(&self) -> f32 {
            self.0
        }
        fn max_value(&self) -> f32 {
            self.1
        }
    }
    impl SplineCoordinate<f32> for Identity {
        fn apply(&self, ctx: &f32) -> f32 {
            *ctx
        }
    }

    #[test]
    fn hermite_interpolation_and_linear_extension() {
        let s = CubicSpline::multipoint(Identity(-2.0, 3.0), vec![0.0, 1.0], vec![CubicSpline::Constant(0.0), CubicSpline::Constant(1.0)], vec![0.0, 0.0]);
        assert_eq!(s.apply(&0.5), 0.5);
        assert_eq!(s.apply(&0.25), 0.15625);
        // outside the points the spline continues with the end derivative (0 here: the end value)
        assert_eq!((s.apply(&-1.0), s.apply(&2.0)), (0.0, 1.0));
        // a derivative extends linearly
        let t = CubicSpline::multipoint(Identity(-2.0, 3.0), vec![0.0, 1.0], vec![CubicSpline::Constant(0.0), CubicSpline::Constant(1.0)], vec![0.5, 2.0]);
        assert_eq!((t.apply(&-1.0), t.apply(&2.0)), (-0.5, 3.0));
        // bounds include the linear extension
        assert!(t.min_value() <= -1.0 && t.max_value() >= 3.0);
    }

    #[test]
    fn interval_search_matches_java_binary_search() {
        let l = [0.0, 1.0, 2.0, 3.0];
        assert_eq!((find_interval_start(&l, -1.0), find_interval_start(&l, 0.0), find_interval_start(&l, 1.5), find_interval_start(&l, 3.0), find_interval_start(&l, 9.0)), (-1, 0, 1, 3, 3));
    }
}
