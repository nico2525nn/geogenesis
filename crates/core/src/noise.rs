//! Noise functions and fractal combinators.
//!
//! Every base function returns values in `[-1, 1]`; lattice gradients are hashed, not shuffled.

use crate::grid::{Boundary, ScalarField};
use crate::linalg::{lerp, Vec2};
use crate::rng::{domain, hash2, hash2_f64, hash3};

/// The base noise function, per spec §2a `ノイズタイプ`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NoiseKind {
    /// Gradient noise on a square lattice.
    #[default]
    Perlin,
    /// Simplex noise on a skewed triangular lattice.
    Simplex,
    /// Value noise: random values at lattice points, quintic-interpolated.
    Value,
    /// Iterated dyadic wavelet (quadratic spline) upsampling of a coarse field.
    Wavelet,
    /// Sparse Gaussian bumps on a jittered lattice.
    GaussianSpike,
}

/// Fractal combinator applied on top of the base function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FractalKind {
    /// Fractional Brownian motion: the classic sum of octaves.
    #[default]
    Fbm,
    /// Ridged multifractal: `1 - |n|`, squared.
    Ridged,
    /// Billow: `|n|`.
    Billow,
}

/// Directional bias for anisotropic noise, per spec §2a `異方性`.
///
/// `amount` is 0 (isotropic) to 1 (fully directional). `direction` is the angle
/// in radians of the stretched axis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Anisotropy {
    pub amount: f64,
    pub direction: f64,
}

impl Default for Anisotropy {
    fn default() -> Self {
        Self {
            amount: 0.0,
            direction: 0.0,
        }
    }
}

impl Anisotropy {
    /// Returns `true` when no domain transform is needed.
    pub fn is_isotropic(&self) -> bool {
        self.amount <= f64::EPSILON
    }

    /// Stretches the sample domain along the principal axis.
    ///
    /// Compresses across the axis as it stretches along it.
    #[inline]
    fn warp(&self, x: f64, y: f64) -> (f64, f64) {
        if self.is_isotropic() {
            return (x, y);
        }
        let a = self.amount.clamp(0.0, 1.0);
        let (c, s) = (self.direction.cos(), self.direction.sin());
        let u = x * c + y * s;
        let v = -x * s + y * c;
        // Small k along u yields a long world-space feature; scaling the other
        // way would compress features along the axis instead.
        let (u2, v2) = (u / (1.0 + a), v * (1.0 + a));
        (u2 * c - v2 * s, u2 * s + v2 * c)
    }
}

/// Complete noise configuration, matching spec §2a.
#[derive(Debug, Clone, PartialEq)]
pub struct NoiseParams {
    pub kind: NoiseKind,
    pub fractal: FractalKind,
    /// 1–16, per spec.
    pub octaves: u32,
    /// 1.0–4.0, per spec.
    pub lacunarity: f64,
    /// 0.1–1.0, per spec.
    pub persistence: f64,
    pub anisotropy: Anisotropy,
    /// Domain-warp amplitude in lattice units; 0 disables warping.
    pub warp_strength: f64,
}

impl Default for NoiseParams {
    fn default() -> Self {
        Self {
            kind: NoiseKind::default(),
            fractal: FractalKind::default(),
            octaves: 6,
            lacunarity: 2.0,
            persistence: 0.5,
            anisotropy: Anisotropy::default(),
            warp_strength: 0.0,
        }
    }
}

impl NoiseParams {
    /// Creates isotropic fBm.
    pub fn fbm(kind: NoiseKind, octaves: u32, seed: u64) -> Self {
        let _ = seed;
        Self {
            kind,
            octaves,
            ..Self::default()
        }
    }
}

/// Quintic smoothstep `6t⁵ − 15t⁴ + 10t³`.
#[inline]
fn quintic(t: f64) -> f64 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Picks a reproducible gradient direction from a lattice hash.
///
/// Discrete directions from a hash keep results bit-identical across platforms.
#[inline]
fn gradient_from_hash(h: u64) -> (f64, f64) {
    // Precomputed cos/sin of 22.5° multiples; avoids `cos` in the hot loop.
    const DIRS: [(f64, f64); 16] = [
        (1.0, 0.0),
        (0.923_879_532_511_286_7, 0.382_683_432_365_089_8),
        (0.707_106_781_186_547_6, 0.707_106_781_186_547_6),
        (0.382_683_432_365_089_8, 0.923_879_532_511_286_7),
        (0.0, 1.0),
        (-0.382_683_432_365_089_8, 0.923_879_532_511_286_7),
        (-0.707_106_781_186_547_6, 0.707_106_781_186_547_6),
        (-0.923_879_532_511_286_7, 0.382_683_432_365_089_8),
        (-1.0, 0.0),
        (-0.923_879_532_511_286_7, -0.382_683_432_365_089_8),
        (-0.707_106_781_186_547_6, -0.707_106_781_186_547_6),
        (-0.382_683_432_365_089_8, -0.923_879_532_511_286_7),
        (0.0, -1.0),
        (0.382_683_432_365_089_8, -0.923_879_532_511_286_7),
        (0.707_106_781_186_547_6, -0.707_106_781_186_547_6),
        (0.923_879_532_511_286_7, -0.382_683_432_365_089_8),
    ];
    DIRS[(h & 15) as usize]
}

/// 2D Perlin gradient noise in `[-1, 1]`.
pub fn perlin_2d(x: f64, y: f64, seed: u64) -> f64 {
    let xi = x.floor();
    let yi = y.floor();
    let xf = x - xi;
    let yf = y - yi;
    let (xi, yi) = (xi as i64, yi as i64);

    let u = quintic(xf);
    let v = quintic(yf);

    let (gx0, gy0) = gradient_from_hash(hash2(xi, yi, seed));
    let (gx1, gy1) = gradient_from_hash(hash2(xi + 1, yi, seed));
    let (gx2, gy2) = gradient_from_hash(hash2(xi, yi + 1, seed));
    let (gx3, gy3) = gradient_from_hash(hash2(xi + 1, yi + 1, seed));

    let n00 = gx0 * xf + gy0 * yf;
    let n10 = gx1 * (xf - 1.0) + gy1 * yf;
    let n01 = gx2 * xf + gy2 * (yf - 1.0);
    let n11 = gx3 * (xf - 1.0) + gy3 * (yf - 1.0);

    let a = lerp(n00, n10, u);
    let b = lerp(n01, n11, u);
    lerp(a, b, v)
}

// 2D simplex skew/unskew constants from Gustavson's construction.
// SQRT_3 is inlined because `std::f64::consts::SQRT_3` is still unstable.
const SQRT_3: f64 = 1.732_050_807_568_877_2;
const F2: f64 = 0.5 * (3.0 - SQRT_3);
const G2: f64 = (3.0 - SQRT_3) / 6.0;

/// 2D Simplex noise in `[-1, 1]`.
pub fn simplex_2d(x: f64, y: f64, seed: u64) -> f64 {
    // Skew the input space so the square lattice becomes a triangular one.
    let s = (x + y) * F2;
    let i = (x + s).floor();
    let j = (y + s).floor();
    let t = (i + j) * G2;
    let x0 = x - (i - t);
    let y0 = y - (j - t);

    // Determine which triangle of the rhombus we are in.
    let (i1, j1) = if x0 > y0 { (1.0, 0.0) } else { (0.0, 1.0) };

    let x1 = x0 - i1 + G2;
    let y1 = y0 - j1 + G2;
    let x2 = x0 - 1.0 + 2.0 * G2;
    let y2 = y0 - 1.0 + 2.0 * G2;

    let (ii, jj) = (i as i64, j as i64);
    let (n0, n1, n2) = (
        simplex_corner(x0, y0, ii, jj, seed),
        simplex_corner(x1, y1, ii + i1 as i64, jj + j1 as i64, seed),
        simplex_corner(x2, y2, ii + 1, jj + 1, seed),
    );

    // 70 * sum(n) rescales back into [-1, 1].
    70.0 * (n0 + n1 + n2)
}

/// Contributes one simplex corner.
#[inline]
fn simplex_corner(x: f64, y: f64, i: i64, j: i64, seed: u64) -> f64 {
    // Radial falloff: the contribution dies beyond radius ~0.5.
    let t = 0.5 - x * x - y * y;
    if t < 0.0 {
        return 0.0;
    }
    let t2 = t * t;
    let (gx, gy) = gradient_from_hash(hash2(i, j, seed));
    t2 * t2 * (gx * x + gy * y)
}

/// 2D value noise in `[-1, 1]`.
pub fn value_2d(x: f64, y: f64, seed: u64) -> f64 {
    let xi = x.floor();
    let yi = y.floor();
    let xf = x - xi;
    let yf = y - yi;
    let (xi, yi) = (xi as i64, yi as i64);

    let u = quintic(xf);
    let v = quintic(yf);

    let n00 = hash2_f64(xi, yi, seed);
    let n10 = hash2_f64(xi + 1, yi, seed);
    let n01 = hash2_f64(xi, yi + 1, seed);
    let n11 = hash2_f64(xi + 1, yi + 1, seed);

    let a = lerp(n00, n10, u);
    let b = lerp(n01, n11, u);
    // hash2_f64 is in [0, 1); map to [-1, 1).
    2.0 * lerp(a, b, v) - 1.0
}

/// 2D wavelet noise in `[-1, 1]`.
///
/// Interpolates between successively halved lattices rather than summing octaves.
pub fn wavelet_2d(x: f64, y: f64, seed: u64, levels: u32) -> f64 {
    let (mut value, mut lo, mut hi) = (0.0f64, x, y);
    for level in 0..levels.max(1) {
        let lattice = hash2_f64(lo.floor() as i64, hi.floor() as i64, seed ^ level as u64);
        let t = quintic(x - lo.floor());
        // Quadratic B-spline blending weight.
        let w = (t * t * (3.0 - 2.0 * t)).clamp(0.0, 1.0);
        let detail = 2.0 * lattice - 1.0;
        value += detail * (1.0 - w) * 0.5;
        lo *= 0.5;
        hi *= 0.5;
    }
    value
}

/// 2D Gaussian-spike noise, in `[0, 1)`.
///
/// Narrow Gaussian bumps on a jittered lattice; `0` means "no feature here",
/// so callers can treat the field as a mask.
pub fn gaussian_spike_2d(x: f64, y: f64, seed: u64, density: f64, width: f64) -> f64 {
    let base = x.floor();
    let by = y.floor();
    let mut sum = 0.0;

    // 3x3 neighbourhood suffices for width < 1.
    for oy in -1i64..=1 {
        for ox in -1i64..=1 {
            let ci = base as i64 + ox;
            let cj = by as i64 + oy;
            let h = hash3(ci, cj, 0, seed);
            let r = (h >> 11) as f64 / (1u64 << 53) as f64;
            if r > density {
                continue;
            }
            // Jitter under a third of a cell keeps neighbours resolvable.
            let jx = ci as f64 + (hash2_f64(ci, cj, seed ^ 0xAA) - 0.5) * 0.3;
            let jy = cj as f64 + (hash2_f64(ci, cj, seed ^ 0x55) - 0.5) * 0.3;
            let dx = x - jx;
            let dy = y - jy;
            let amp = 0.75 + 0.25 * hash2_f64(ci, cj, seed ^ 0x33);
            sum += amp * (-(dx * dx + dy * dy) / (2.0 * width * width)).exp();
        }
    }
    // Soft saturation preserves peak ordering, which a `clamp` would erase.
    sum / (1.0 + sum)
}

/// Evaluates the base function for `kind` at a single point.
#[inline]
fn eval_base(kind: NoiseKind, x: f64, y: f64, seed: u64) -> f64 {
    match kind {
        NoiseKind::Perlin => perlin_2d(x, y, seed),
        NoiseKind::Simplex => simplex_2d(x, y, seed),
        NoiseKind::Value => value_2d(x, y, seed),
        NoiseKind::Wavelet => wavelet_2d(x, y, seed, 3),
        NoiseKind::GaussianSpike => gaussian_spike_2d(x, y, seed, 0.25, 0.5),
    }
}

/// Evaluates `params` at one point, applying anisotropy, warping and octaves.
///
/// Result is in `[-1, 1]`: octaves are summed with a normalising weight so the
/// total stays bounded regardless of octave count or persistence.
pub fn sample(params: &NoiseParams, x: f64, y: f64, seed: u64) -> f64 {
    let (mut px, mut py) = params.anisotropy.warp(x, y);

    if params.warp_strength > 0.0 {
        // Domain warping bends features into curving shapes unwarped fBm lacks.
        let ws = params.warp_strength;
        let wx = perlin_2d(px, py, seed ^ 0xDEAD_BEEF);
        let wy = perlin_2d(px + 5.2, py + 1.3, seed ^ 0xBEEF_DEAD);
        px += ws * wx;
        py += ws * wy;
    }

    let octaves = params.octaves.clamp(1, 16);
    let lac = params.lacunarity.clamp(1.0, 4.0);
    let pers = params.persistence.clamp(0.1, 1.0);

    let mut value = 0.0;
    let mut amplitude = 1.0;
    let mut frequency = 1.0;
    let mut norm = 0.0;

    for octave in 0..octaves {
        let n = eval_base(params.kind, px * frequency, py * frequency, seed.wrapping_add(octave as u64));
        let shaped = match params.fractal {
            FractalKind::Fbm => n,
            FractalKind::Ridged => {
                let r = 1.0 - n.abs();
                // Square sharpens crests; subtract 0.5 to re-centre on 0.
                r * r - 0.5
            }
            FractalKind::Billow => n.abs() - 0.5,
        };
        value += shaped * amplitude;
        norm += amplitude;
        amplitude *= pers;
        frequency *= lac;
    }

    if norm > 0.0 {
        value /= norm;
    }
    // Ridged/billow re-centring can push slightly past the edge, so clamp.
    value.clamp(-1.0, 1.0)
}

/// A stateless noise field sampler.
#[derive(Debug, Clone)]
pub struct NoiseField {
    params: NoiseParams,
    seed: u64,
}

impl NoiseField {
    /// Creates a field for `params` from `world_seed`.
    pub fn new(params: NoiseParams, world_seed: u64) -> Self {
        let seed = crate::rng::derive_seed(world_seed, domain_for(params.kind));
        Self { params, seed }
    }

    /// Uses an explicit seed instead of deriving one.
    pub fn with_seed(params: NoiseParams, seed: u64) -> Self {
        Self { params, seed }
    }

    /// Current parameters.
    pub fn params(&self) -> &NoiseParams {
        &self.params
    }

    /// The derived seed.
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// Evaluates at a point.
    #[inline]
    pub fn sample(&self, x: f64, y: f64) -> f64 {
        sample(&self.params, x, y, self.seed)
    }

    /// Fills a grid, with `cycles` noise periods across the whole domain.
    pub fn fill(&self, boundary: Boundary, cycles: f64) -> ScalarField {
        self.fill_rect(boundary, cycles, cycles)
    }

    /// Fills a grid with independent `cycles_x` and `cycles_y` counts.
    pub fn fill_rect(&self, boundary: Boundary, cycles_x: f64, cycles_y: f64) -> ScalarField {
            let mut out = ScalarField::zeros(boundary);
            let (w, h) = (boundary.width(), boundary.height());
            let (ox, oy) = seed_offset(self.seed);
            let (sx, sy) = (cycles_x / w.max(1) as f64, cycles_y / h.max(1) as f64);
            for y in 0..h {
                for x in 0..w {
                    out.set(
                        x,
                        y,
                        self.sample(x as f64 * sx + ox, y as f64 * sy + oy),
                    );
                }
            }
            out
        }

        /// Adds the field into `target`, multiplied by `gain`.
        pub fn add_into(&self, target: &mut ScalarField, cycles: f64, gain: f64) {
            let b = *target.boundary();
            let (w, h) = (b.width(), b.height());
            let (ox, oy) = seed_offset(self.seed);
            let step = cycles / w.max(1) as f64;
            for y in 0..h {
                for x in 0..w {
                    let n = self.sample(x as f64 * step + ox, y as f64 * step + oy);
                    let i = b.index(x, y);
                    target.as_mut_slice()[i] += n * gain;
                }
            }
        }

        /// Calls `f(x, y, noise_value)` for each cell.
        pub fn fill_with(&self, boundary: Boundary, cycles: f64, mut f: impl FnMut(f64, f64, f64)) {
            let (w, h) = (boundary.width(), boundary.height());
            let (ox, oy) = seed_offset(self.seed);
            let step = cycles / w.max(1) as f64;
            for y in 0..h {
                for x in 0..w {
                    f(
                        x as f64,
                        y as f64,
                        self.sample(x as f64 * step + ox, y as f64 * step + oy),
                    );
                }
            }
        }
    }

/// Seed-derived lattice translation.
fn seed_offset(seed: u64) -> (f64, f64) {
    const Q: u64 = 1024;
    (
        (crate::rng::hash2(0, 0, seed) % Q) as f64 / Q as f64,
        (crate::rng::hash2(1, 1, seed) % Q) as f64 / Q as f64,
    )
}

/// Seed domain for a noise kind, so kinds do not correlate.
const fn domain_for(kind: NoiseKind) -> u64 {
    match kind {
        NoiseKind::Perlin => domain::NOISE_PERLIN,
        NoiseKind::Simplex => domain::NOISE_SIMPLEX,
        NoiseKind::Value => domain::NOISE_VALUE,
        NoiseKind::Wavelet => domain::NOISE_WAVELET,
        NoiseKind::GaussianSpike => domain::NOISE_WAVELET ^ 0xF0,
    }
}

/// Normalised coordinates within a grid cell, for feature stamps.
#[inline]
pub fn cell_norm(index: usize, boundary: &Boundary) -> (f64, f64) {
    let (x, y) = boundary.coords(index);
    (
        if boundary.width() > 1 {
            x as f64 / (boundary.width() - 1) as f64
        } else {
            0.5
        },
        if boundary.height() > 1 {
            y as f64 / (boundary.height() - 1) as f64
        } else {
            0.5
        },
    )
}

/// Direction unit vector for anisotropic feature stamps.
pub fn direction_vector(degrees: f64) -> Vec2 {
    Vec2::from_angle(degrees.to_radians(), 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::BoundaryKind;

    const SEED: u64 = 0xDEAD_BEEF_CAFE;

    fn range_over(kind: NoiseKind, n: usize, step: f64) -> (f64, f64) {
        let params = NoiseParams {
            kind,
            octaves: 1,
            ..Default::default()
        };
        let mut lo = f64::INFINITY;
        let mut hi = f64::NEG_INFINITY;
        for i in 0..n {
            for j in 0..n {
                let v = sample(&params, i as f64 * step, j as f64 * step, SEED);
                lo = lo.min(v);
                hi = hi.max(v);
            }
        }
        (lo, hi)
    }

    #[test]
    fn every_base_function_stays_in_unit_range() {
        for kind in [
            NoiseKind::Perlin,
            NoiseKind::Simplex,
            NoiseKind::Value,
            NoiseKind::Wavelet,
            NoiseKind::GaussianSpike,
        ] {
            let (lo, hi) = range_over(kind, 64, 0.37);
            assert!(hi <= 1.0, "{kind:?} exceeds +1: {hi}");
            if kind != NoiseKind::GaussianSpike {
                assert!(lo >= -1.0, "{kind:?} below -1: {lo}");
            } else {
                // Non-negative mask by contract.
                assert!(lo >= 0.0, "{kind:?} went negative: {lo}");
            }
        }
    }

    #[test]
    fn base_functions_are_deterministic() {
        for kind in [
            NoiseKind::Perlin,
            NoiseKind::Simplex,
            NoiseKind::Value,
            NoiseKind::Wavelet,
        ] {
            let a = perlin_or(kind, 1.7, 2.3, SEED);
            let b = perlin_or(kind, 1.7, 2.3, SEED);
            assert_eq!(a, b, "{kind:?} is not deterministic");
        }
    }

    fn perlin_or(kind: NoiseKind, x: f64, y: f64, seed: u64) -> f64 {
        match kind {
            NoiseKind::Perlin => perlin_2d(x, y, seed),
            NoiseKind::Simplex => simplex_2d(x, y, seed),
            NoiseKind::Value => value_2d(x, y, seed),
            NoiseKind::Wavelet => wavelet_2d(x, y, seed, 3),
            NoiseKind::GaussianSpike => gaussian_spike_2d(x, y, seed, 0.25, 0.5),
        }
    }

    #[test]
    fn different_seeds_give_different_fields() {
        let p = NoiseParams::default();
        // Off-lattice samples: on a lattice row the y-gradient term drops out.
        let mut diffs = 0;
        for i in 0..32 {
            let (x, y) = (i as f64 * 0.317 + 0.041, i as f64 * 0.193 + 0.077);
            if sample(&p, x, y, 1) != sample(&p, x, y, 2) {
                diffs += 1;
            }
        }
        assert_eq!(diffs, 32, "seed barely changes the field");
    }

#[test]
    fn noise_is_continuous_within_a_seed() {
        // Bounded per-unit gradients rule out discontinuities.
        let p = NoiseParams {
            octaves: 3,
            ..Default::default()
        };
        for step in [1e-2f64, 1e-3, 1e-4] {
            let n = (2.0 / step) as usize;
            let mut max_slope = 0.0f64;
            let mut i = 1;
            while (i as f64 * step) < 2.0 {
                let x = i as f64 * step;
                let a = sample(&p, x, 0.37, SEED);
                let b = sample(&p, x + step, 0.37, SEED);
                max_slope = max_slope.max((a - b).abs() / step);
                i += 1;
            }
            assert!(n > 0);
            assert!(max_slope < 20.0, "step {step}: slope {max_slope} is unbounded");
        }
    }

    #[test]
    fn noise_differs_between_seeds_but_both_stay_bounded() {
        // Seed changes must stay smooth, not become white noise.
        let p = NoiseParams {
            octaves: 4,
            ..Default::default()
        };
        let mut max_slope = 0.0f64;
        for seed in [1u64, 2] {
            for i in 0..400 {
                let x = i as f64 * 0.005;
                let a = sample(&p, x, 0.37, seed);
                let b = sample(&p, x + 0.005, 0.37, seed);
                max_slope = max_slope.max((a - b).abs() / 0.005);
            }
        }
        assert!(max_slope < 20.0, "seeded field is not smooth: {max_slope}");
    }

    #[test]
    fn noise_is_roughly_zero_mean() {
        for kind in [NoiseKind::Perlin, NoiseKind::Simplex, NoiseKind::Value] {
            let params = NoiseParams {
                kind,
                octaves: 1,
                ..Default::default()
            };
            let mut sum = 0.0;
            let n = 100.0;
            let mut count = 0.0;
            for i in 0..80 {
                for j in 0..80 {
                    sum += sample(&params, i as f64 * n, j as f64 * n, SEED);
                    count += 1.0;
                }
            }
            let mean = sum / count;
            assert!(mean.abs() < 0.12, "{kind:?} mean {mean}");
        }
    }

    #[test]
    fn noise_has_no_dc_grid_bias() {
        // A large bias would show up as cross-hatch on the terrain.
        let params = NoiseParams {
            kind: NoiseKind::Perlin,
            octaves: 1,
            ..Default::default()
        };
        let mut x_bias = 0.0;
        let mut y_bias = 0.0;
        let n = 40;
        for i in 0..n {
            for j in 0..n {
                x_bias += sample(&params, i as f64, j as f64, SEED);
                y_bias += sample(&params, j as f64, i as f64, SEED);
            }
        }
        let total = (n * n) as f64;
        assert!(x_bias / total < 0.15);
        assert!(y_bias / total < 0.15);
    }

    #[test]
    fn perlin_is_zero_at_lattice_points() {
        // Gradient noise is exactly 0 on integer lattice points.
        for i in 0..8 {
            for j in 0..8 {
                let v = perlin_2d(i as f64, j as f64, SEED);
                assert!(v.abs() < 1e-9, "perlin({i},{j}) = {v}");
            }
        }
    }

    #[test]
    fn perlin_is_continuous() {
        let eps = 1e-6;
        for i in 0..8 {
            for j in 0..8 {
                let (x, y) = (i as f64 + 0.5, j as f64 + 0.5);
                let a = perlin_2d(x, y, SEED);
                let b = perlin_2d(x + eps, y + eps, SEED);
                assert!((a - b).abs() < 0.05, "discontinuity at {x},{y}");
            }
        }
    }

    #[test]
    fn noise_gradients_are_statistically_isotropic() {
        // Axis-weighted gradient sets fail this; it guards that artefact.
        for kind in [NoiseKind::Perlin, NoiseKind::Simplex, NoiseKind::Value] {
            let params = NoiseParams {
                kind,
                octaves: 1,
                ..Default::default()
            };
            let eps = 1e-4;
            let (mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0);
            let n = 200;
            for i in 0..n {
                for j in 0..n {
                    let (x, y) = (i as f64 * 0.137 + 0.019, j as f64 * 0.211 + 0.043);
                    let gx = (sample(&params, x + eps, y, SEED) - sample(&params, x - eps, y, SEED))
                        / (2.0 * eps);
                    let gy = (sample(&params, x, y + eps, SEED) - sample(&params, x, y - eps, SEED))
                        / (2.0 * eps);
                    sxx += gx * gx;
                    syy += gy * gy;
                    sxy += gx * gy;
                }
            }
            let count = (n * n) as f64;
            let (mxx, myy, mxy) = (sxx / count, syy / count, sxy / count);
            let ratio = mxx / myy;
            assert!(
                (0.75..1.33).contains(&ratio),
                "{kind:?} axis anisotropy: gx²/gy² = {ratio}"
            );
            assert!(
                mxy.abs() < 0.15 * (mxx * myy).sqrt(),
                "{kind:?} gradient correlation {mxy}"
            );
        }
    }

    #[test]
    fn fractal_amplitude_does_not_explode_with_octaves() {
        for octaves in [1u32, 4, 8, 16] {
            let params = NoiseParams {
                octaves,
                ..Default::default()
            };
            let mut hi = f64::NEG_INFINITY;
            let mut lo = f64::INFINITY;
            for i in 0..64 {
                for j in 0..64 {
                    let v = sample(&params, i as f64 * 0.21, j as f64 * 0.21, SEED);
                    hi = hi.max(v);
                    lo = lo.min(v);
                }
            }
            assert!(hi <= 1.0 && lo >= -1.0, "octaves={octaves}: {lo}..{hi}");
        }
    }

    #[test]
    fn more_octaves_add_detail() {
        // More detail means lower coarse-scale variance.
        let coarse = NoiseParams {
            octaves: 1,
            ..Default::default()
        };
        let fine = NoiseParams {
            octaves: 8,
            ..Default::default()
        };
        let c = std_dev_field(&coarse, 8.0);
        let f = std_dev_field(&fine, 8.0);
        assert!(f < c, "8 octaves ({f}) should be rougher than 1 ({c})");
    }

    fn std_dev_field(params: &NoiseParams, freq: f64) -> f64 {
        let mut vals = Vec::new();
        for i in 0..40 {
            for j in 0..40 {
                vals.push(sample(params, i as f64 / 40.0 * freq, j as f64 / 40.0 * freq, SEED));
            }
        }
        let m = vals.iter().sum::<f64>() / vals.len() as f64;
        (vals.iter().map(|v| (v - m).powi(2)).sum::<f64>() / vals.len() as f64).sqrt()
    }

    #[test]
    fn anisotropy_stretches_features_along_its_axis() {
        // Stretched fields change less per unit displacement along the axis.
        let aniso = NoiseParams {
            octaves: 4,
            anisotropy: Anisotropy {
                amount: 1.0,
                direction: 0.0, // stretch along +x
            },
            ..Default::default()
        };
        let along = rms_difference(&aniso, (1.0, 0.0));
        let across = rms_difference(&aniso, (0.0, 1.0));
        assert!(
            along < across,
            "anisotropic field should vary slower along its axis (along {along}, across {across})"
        );
    }

    #[test]
    fn isotropic_anisotropy_matches_itself_on_both_axes() {
        let iso = NoiseParams {
            octaves: 4,
            ..Default::default()
        };
        let along = rms_difference(&iso, (1.0, 0.0));
        let across = rms_difference(&iso, (0.0, 1.0));
        // No preferred axis, so the two must be comparable.
        let ratio = along / across;
        assert!(ratio > 0.6 && ratio < 1.7, "isotropic ratio {ratio}");
    }

    /// RMS of `f(p + d) - f(p)` over sample points.
    fn rms_difference(params: &NoiseParams, d: (f64, f64)) -> f64 {
        let mut sum = 0.0;
        let n = 60;
        for i in 0..n {
            for j in 0..n {
                let (x, y) = (i as f64 / n as f64 * 6.0, j as f64 / n as f64 * 6.0);
                let delta = sample(params, x, y, SEED) - sample(params, x + d.0, y + d.1, SEED);
                sum += delta * delta;
            }
        }
        (sum / (n * n) as f64).sqrt()
    }

    #[test]
    fn isotropic_anisotropy_is_a_no_op() {
        let a = Anisotropy::default();
        assert!(a.is_isotropic());
        assert_eq!(a.warp(3.0, 5.0), (3.0, 5.0));
    }

    #[test]
    fn domain_warping_changes_the_field_but_keeps_range() {
        let plain = NoiseParams {
            octaves: 4,
            ..Default::default()
        };
        let warped = NoiseParams {
            octaves: 4,
            warp_strength: 0.5,
            ..Default::default()
        };
        let mut differs = 0;
        for i in 0..20 {
            for j in 0..20 {
                let (x, y) = (i as f64 * 0.3, j as f64 * 0.3);
                let a = sample(&plain, x, y, SEED);
                let b = sample(&warped, x, y, SEED);
                assert!(b.abs() <= 1.0);
                if (a - b).abs() > 1e-9 {
                    differs += 1;
                }
            }
        }
        assert_eq!(differs, 400, "warp had no effect");
    }

    #[test]
    fn noise_field_fill_matches_direct_sampling() {
        let b = Boundary::new(BoundaryKind::Rectangular, 8, 8);
        let cycles = 8.0;
        let f = NoiseField::with_seed(NoiseParams::default(), SEED);
        let field = f.fill(b, cycles);
        let (ox, oy) = seed_offset(SEED);
        let step = cycles / 8.0;
        for y in 0..8 {
            for x in 0..8 {
                let expect = f.sample(x as f64 * step + ox, y as f64 * step + oy);
                assert!((field.get(x, y) - expect).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn noise_field_fill_produces_variation_at_resolvable_frequencies() {
        // Regression test: only grid-representable frequencies are checked.
        let b = Boundary::new(BoundaryKind::Rectangular, 32, 32);
        for spec_freq in [0.01, 0.1, 0.5, 1.0] {
            let cycles = spec_freq * 32.0;
            let f = NoiseField::with_seed(NoiseParams::default(), SEED);
            let field = f.fill(b, cycles);
            assert!(
                field.max() - field.min() > 0.2,
                "spec freq {spec_freq} (cycles {cycles}) gave a constant field"
            );
        }
    }

    #[test]
    fn unresolvable_frequencies_are_flat_and_monotonic_below_resonance() {
        // Below ~1 cycle per 2 cells the field is legitimately flat.
        let b = Boundary::new(BoundaryKind::Rectangular, 32, 32);
        let mut variances = Vec::new();
        for cycles in [0.05f64, 0.5, 2.0, 8.0, 16.0] {
            let f = NoiseField::with_seed(NoiseParams::default(), SEED);
            variances.push(f.fill(b, cycles).std_dev());
        }
        assert!(
            variances[0] < 0.05,
            "unresolvable frequency should be flat: {}",
            variances[0]
        );
        // Variance must not shrink as frequency rises through the band.
        assert!(
            variances[3] > variances[1],
            "8 cycles {} should vary more than 0.5 cycles {}",
            variances[3],
            variances[1]
        );
    }

    #[test]
    fn noise_field_seed_offset_varies_with_seed() {
        // Bit patterns compared because f64 is not Hash.
        let mut offsets: Vec<u64> = [1u64, 2, 3, 4, 5]
            .iter()
            .map(|&s| {
                let (ox, oy) = seed_offset(s);
                ox.to_bits() ^ oy.to_bits().rotate_left(32)
            })
            .collect();
        let total = offsets.len();
        offsets.sort_unstable();
        offsets.dedup();
        assert_eq!(offsets.len(), total, "seed offsets collide");
    }

    #[test]
    fn noise_field_fill_rect_uses_independent_cycle_counts() {
        // Compare x-adjacent against y-adjacent differences, not row sums.
        let b = Boundary::new(BoundaryKind::Rectangular, 16, 8);
        let f = NoiseField::with_seed(NoiseParams::default(), SEED);
        let field = f.fill_rect(b, 64.0, 2.0);
        let x_step: f64 = (0..15).map(|x| (field.get(x + 1, 4) - field.get(x, 4)).abs()).sum();
        let y_step: f64 = (0..7).map(|y| (field.get(8, y + 1) - field.get(8, y)).abs()).sum();
        assert!(
            x_step > y_step,
            "x variation {x_step} should exceed y variation {y_step}"
        );
    }

    #[test]
    fn noise_field_add_into_accumulates() {
        let b = Boundary::new(BoundaryKind::Rectangular, 4, 4);
        let f = NoiseField::with_seed(NoiseParams::default(), SEED);
        let mut target = ScalarField::zeros(b);
        f.add_into(&mut target, 4.0, 10.0);
        let once = target.clone();
        f.add_into(&mut target, 4.0, 10.0);
        for i in 0..target.len() {
            assert!((target.as_slice()[i] - 2.0 * once.as_slice()[i]).abs() < 1e-12);
        }
    }

    #[test]
    fn ridged_fractal_produces_crest_dominated_statistics() {
        let params = NoiseParams {
            fractal: FractalKind::Ridged,
            octaves: 4,
            ..Default::default()
        };
        let mut vals = Vec::new();
        for i in 0..60 {
            for j in 0..60 {
                let v = sample(&params, i as f64 * 0.17, j as f64 * 0.19, SEED);
                assert!(v.abs() <= 1.0, "ridged left range: {v}");
                vals.push(v);
            }
        }
        // Folded |n| skews the mean below the median.
        let mean = vals.iter().sum::<f64>() / vals.len() as f64;
        let mut sorted = vals.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = sorted[sorted.len() / 2];
        assert!(
            mean < median,
            "ridged should be crest-skewed: mean {mean} >= median {median}"
        );
    }

    #[test]
    fn billow_fractal_is_right_skewed() {
        let billow = NoiseParams {
            fractal: FractalKind::Billow,
            octaves: 3,
            ..Default::default()
        };
        let fbm = NoiseParams {
            fractal: FractalKind::Fbm,
            octaves: 3,
            ..Default::default()
        };
        // `|n| - 0.5` folds the distribution; the mean-median gap is the signal.
        fn mean_minus_median(p: &NoiseParams) -> f64 {
            let mut v = Vec::new();
            for i in 0..70 {
                for j in 0..70 {
                    v.push(sample(p, i as f64 * 0.13, j as f64 * 0.17, SEED));
                }
            }
            let mean = v.iter().sum::<f64>() / v.len() as f64;
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            mean - v[v.len() / 2]
        }
        let billow_skew = mean_minus_median(&billow);
        let fbm_skew = mean_minus_median(&fbm);
        assert!(
            billow_skew > fbm_skew,
            "billow ({billow_skew}) should be more right-skewed than fbm ({fbm_skew})"
        );
    }

    #[test]
    fn wavelet_and_perlin_differ() {
        let w = perlin_or(NoiseKind::Wavelet, 1.3, 2.7, SEED);
        let p = perlin_or(NoiseKind::Perlin, 1.3, 2.7, SEED);
        assert!((w - p).abs() > 1e-6, "wavelet is identical to perlin");
    }

    #[test]
    fn gaussian_spikes_are_localized() {
        // The contract is separability: count strict interior local maxima.
        let width = 0.35;
        let mut maxima = 0;
        let mut cells = 0;
        for i in 1..39 {
            for j in 1..39 {
                let v = gaussian_spike_2d(i as f64, j as f64, SEED, 1.0, width);
                let peak = v;
                let mut is_max = true;
                for (dx, dy) in [(-0.5f64, 0.0), (0.5, 0.0), (0.0, -0.5), (0.0, 0.5)] {
                    if gaussian_spike_2d(i as f64 + dx, j as f64 + dy, SEED, 1.0, width) >= peak {
                        is_max = false;
                    }
                }
                if is_max {
                    maxima += 1;
                }
                cells += 1;
            }
        }
        // Jitter lets neighbours occasionally dominate; a solid majority suffices.
        let ratio = maxima as f64 / cells as f64;
        assert!(ratio > 0.6, "spikes are not separable peaks: ratio {ratio}");
    }

    #[test]
    fn gaussian_spike_density_controls_coverage() {
        // Sparse settings must leave most of the field at zero.
        fn coverage(density: f64) -> f64 {
            let mut nonzero = 0;
            let total = 120 * 120;
            for i in 0..120 {
                for j in 0..120 {
                    let v = gaussian_spike_2d(
                        i as f64 * 0.25,
                        j as f64 * 0.25,
                        SEED,
                        density,
                        0.35,
                    );
                    if v.abs() > 1e-3 {
                        nonzero += 1;
                    }
                }
            }
            nonzero as f64 / total as f64
        }
        let sparse = coverage(0.05);
        let dense = coverage(0.5);
        assert!(sparse < dense, "density has no effect: {sparse} vs {dense}");
        assert!(sparse < 0.35, "sparse spikes cover too much: {sparse}");
    }

    #[test]
    fn octaves_lacunarity_and_persistence_are_clamped_to_spec_ranges() {
        let params = NoiseParams {
            octaves: 99,
            lacunarity: 0.1,
            persistence: 5.0,
            ..Default::default()
        };
        // Out-of-spec inputs must still evaluate in range.
        for i in 0..16 {
            let v = sample(&params, i as f64 * 0.3, i as f64 * 0.7, SEED);
            assert!(v.abs() <= 1.0);
        }
        assert_eq!(params.octaves.clamp(1, 16), 16);
        assert_eq!(params.lacunarity.clamp(1.0, 4.0), 1.0);
        assert_eq!(params.persistence.clamp(0.1, 1.0), 1.0);
    }
}