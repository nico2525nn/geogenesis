//! Deterministic numerical foundations: RNG, noise, small vector types and
//! scalar helpers shared by every subsystem.
//!
//! Nothing in here depends on the grid or on higher-level state, so it is safe
//! to use from generation, simulation and tests alike.

pub mod noise;
pub mod rng;
pub mod vec2;

pub use vec2::Vec2;

/// Clamp `v` into `[lo, hi]` (panics in debug if `lo > hi`).
#[inline]
#[must_use]
pub fn clamp(v: f64, lo: f64, hi: f64) -> f64 {
    debug_assert!(lo <= hi);
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

/// Clamp an integer into `[lo, hi]`.
#[inline]
#[must_use]
pub fn clamp_usize(v: usize, lo: usize, hi: usize) -> usize {
    v.clamp(lo, hi)
}

/// Linear interpolation, `t = 0` gives `a`.
#[inline]
#[must_use]
pub fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// Hermite smoothstep between two edges; `0` below `edge0`, `1` above `edge1`.
#[inline]
#[must_use]
pub fn smoothstep(edge0: f64, edge1: f64, x: f64) -> f64 {
    if edge0 == edge1 {
        return if x < edge0 { 0.0 } else { 1.0 };
    }
    let t = clamp((x - edge0) / (edge1 - edge0), 0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Quintic smoothstep, the C2 variant used by Perlin's fade curve.
#[inline]
#[must_use]
pub fn fade5(t: f64) -> f64 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Replace non-finite values with `fallback`, leaving finite values untouched.
#[inline]
pub fn finite_or(v: f64, fallback: f64) -> f64 {
    if v.is_finite() {
        v
    } else {
        fallback
    }
}

/// The error function, needed for Gaussian kernels and diffusion profiles.
///
/// Abramowitz & Stegun 7.1.26 with a maximum absolute error of 1.5e-7, which is
/// far below anything the terrain simulation can resolve.
#[must_use]
pub fn erf(x: f64) -> f64 {
    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let x = x.abs();
    let t = 1.0 / (1.0 + 0.327_591_1 * x);
    let y = 1.0
        - (((((1.061_405_429 * t - 1.453_152_027) * t) + 1.421_413_741) * t - 0.284_496_736) * t
            + 0.254_829_592)
            * t
            * (-x * x).exp();
    sign * y
}

/// Complementary error function, `1 - erf(x)`.
#[inline]
#[must_use]
pub fn erfc(x: f64) -> f64 {
    1.0 - erf(x)
}

/// Standard normal CDF, used by threshold and probability helpers.
#[inline]
#[must_use]
pub fn normal_cdf(z: f64) -> f64 {
    0.5 * erfc(-z / std::f64::consts::SQRT_2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_bounds() {
        assert_eq!(clamp(-5.0, 0.0, 10.0), 0.0);
        assert_eq!(clamp(15.0, 0.0, 10.0), 10.0);
        assert_eq!(clamp(4.0, 0.0, 10.0), 4.0);
    }

    #[test]
    fn smoothstep_is_flat_at_the_edges() {
        assert_eq!(smoothstep(0.0, 1.0, -1.0), 0.0);
        assert_eq!(smoothstep(0.0, 1.0, 2.0), 1.0);
        assert!((smoothstep(0.0, 1.0, 0.5) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn erf_matches_reference_values() {
        // Reference values from the standard tables.
        for (x, want) in [
            (0.0, 0.0),
            (0.5, 0.520_499_877),
            (1.0, 0.842_700_793),
            (2.0, 0.995_322_265),
        ] {
            assert!((erf(x) - want).abs() < 2e-7, "erf({x}) = {}", erf(x));
        }
        assert!((erf(-0.5) + erf(0.5)).abs() < 1e-12);
    }

    #[test]
    fn finite_or_replaces_only_invalid_values() {
        assert_eq!(finite_or(3.0, 0.0), 3.0);
        assert_eq!(finite_or(f64::NAN, 7.0), 7.0);
        assert_eq!(finite_or(f64::INFINITY, 7.0), 7.0);
    }
}
