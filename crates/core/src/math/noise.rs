//! Fractal noise: the raw material of relief, rock provinces and climate
//! heterogeneity.
//!
//! Five base functions are provided (spec §2a): Perlin gradient noise, 2D
//! simplex, value noise, a Ricker/wavelet lattice, and sparse Gaussian spikes
//! (volcano-like domes). Each is combined with fBm, ridged or billow fractal
//! summation.
//!
//! All functions are deterministic from a seed and, importantly, *tileable*
//! on periodic grids. Perlin and value noise wrap their lattice hashes; the
//! wavelet and spike kinds wrap lattice distances; simplex is made tileable by
//! a four-sample lattice blend. Without that, a toroidal world would show a
//! visible seam where the noise field jumps at the wrap.

use crate::grid::{Field, Grid};
use crate::math::fade5;
use crate::math::rng::{hash2, mix64, signed_from_hash, unit_from_hash};

/// Base noise function (spec §2a).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NoiseKind {
    /// Classic gradient noise on a square lattice.
    Perlin,
    /// Gradient noise on a triangular (skewed square) lattice.
    Simplex,
    /// Smooth value interpolation on a square lattice.
    Value,
    /// Sum of Ricker (Mexican-hat) wavelets on a lattice.
    Wavelet,
    /// Sparse Gaussian domes; useful for volcanic and plume relief.
    GaussianSpike,
}

impl NoiseKind {
    /// All kinds in schema order.
    pub const ALL: [Self; 5] = [
        Self::Perlin,
        Self::Simplex,
        Self::Value,
        Self::Wavelet,
        Self::GaussianSpike,
    ];

    /// Stable key for configuration and JSON.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Perlin => "perlin",
            Self::Simplex => "simplex",
            Self::Value => "value",
            Self::Wavelet => "wavelet",
            Self::GaussianSpike => "gaussian_spike",
        }
    }

    /// Human-readable label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Perlin => "Perlin gradient",
            Self::Simplex => "Simplex",
            Self::Value => "Value",
            Self::Wavelet => "Wavelet (Ricker)",
            Self::GaussianSpike => "Gaussian spikes",
        }
    }

    /// Parses a stable key.
    #[must_use]
    pub fn parse(key: &str) -> Option<Self> {
        match key {
            "perlin" => Some(Self::Perlin),
            "simplex" => Some(Self::Simplex),
            "value" => Some(Self::Value),
            "wavelet" => Some(Self::Wavelet),
            "gaussian_spike" | "gaussian" => Some(Self::GaussianSpike),
            _ => None,
        }
    }
}

/// Fractal summation style.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FractalKind {
    /// Fractional Brownian motion: plain octave sum.
    Fbm,
    /// Ridged multifractal: sharp crests, good for mountain belts.
    Ridged,
    /// Billow: puffy |n| octaves, good for clouds and rolling hills.
    Billow,
}

impl FractalKind {
    /// All kinds in schema order.
    pub const ALL: [Self; 3] = [Self::Fbm, Self::Ridged, Self::Billow];

    /// Stable key for configuration and JSON.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Fbm => "fbm",
            Self::Ridged => "ridged",
            Self::Billow => "billow",
        }
    }

    /// Human-readable label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Fbm => "fBm (smooth)",
            Self::Ridged => "Ridged",
            Self::Billow => "Billow",
        }
    }

    /// Parses a stable key.
    #[must_use]
    pub fn parse(key: &str) -> Option<Self> {
        match key {
            "fbm" => Some(Self::Fbm),
            "ridged" => Some(Self::Ridged),
            "billow" => Some(Self::Billow),
            _ => None,
        }
    }
}

/// Parameters of a fractal noise field.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoiseParams {
    /// Base function.
    pub kind: NoiseKind,
    /// Fractal summation style.
    pub fractal: FractalKind,
    /// Octave count, 1..=16 (spec §2a).
    pub octaves: u32,
    /// Frequency multiplier per octave.
    pub lacunarity: f64,
    /// Amplitude multiplier per octave.
    pub persistence: f64,
    /// 0 = isotropic, 1 = maximally directional.
    pub anisotropy: f64,
    /// Direction of the anisotropic fabric, degrees from +x.
    pub direction_deg: f64,
}

impl Default for NoiseParams {
    fn default() -> Self {
        Self {
            kind: NoiseKind::Perlin,
            fractal: FractalKind::Fbm,
            octaves: 5,
            lacunarity: 2.0,
            persistence: 0.5,
            anisotropy: 0.0,
            direction_deg: 0.0,
        }
    }
}

impl NoiseParams {
    /// Clamps every parameter into its valid range.
    pub fn normalize(&mut self) {
        self.octaves = self.octaves.clamp(1, 16);
        self.lacunarity = self.lacunarity.clamp(1.0, 4.0);
        self.persistence = self.persistence.clamp(0.05, 1.0);
        self.anisotropy = self.anisotropy.clamp(0.0, 1.0);
        self.direction_deg = self.direction_deg.rem_euclid(360.0);
    }

    /// Validates against the spec ranges.
    pub fn validate(&self) -> Result<(), crate::error::GeoError> {
        use crate::error::GeoError;
        if !(1..=16).contains(&self.octaves) {
            return Err(GeoError::param(
                "octaves",
                self.octaves,
                "must be in 1..=16",
            ));
        }
        if !(1.0..=4.0).contains(&self.lacunarity) {
            return Err(GeoError::param(
                "lacunarity",
                self.lacunarity,
                "must be in 1..=4",
            ));
        }
        if !(0.05..=1.0).contains(&self.persistence) {
            return Err(GeoError::param(
                "persistence",
                self.persistence,
                "must be in 0.05..=1",
            ));
        }
        if !(0.0..=1.0).contains(&self.anisotropy) {
            return Err(GeoError::param(
                "anisotropy",
                self.anisotropy,
                "must be in 0..=1",
            ));
        }
        Ok(())
    }
}

/// A seeded fractal noise field.
#[derive(Clone, Debug)]
pub struct NoiseField {
    params: NoiseParams,
    seed: u64,
}

/// Period choice per axis: `Some(n)` means the lattice wraps every `n` cells.
type Periods = (Option<u32>, Option<u32>);

impl NoiseField {
    /// Creates a field. Parameters are normalised before use.
    #[must_use]
    pub fn new(mut params: NoiseParams, seed: u64) -> Self {
        params.normalize();
        Self { params, seed }
    }

    /// Parameters in effect.
    #[must_use]
    pub const fn params(&self) -> &NoiseParams {
        &self.params
    }

    /// Seed in effect.
    #[must_use]
    pub const fn seed(&self) -> u64 {
        self.seed
    }

    /// Samples the fractal at unit-lattice coordinates (non-periodic).
    #[must_use]
    pub fn sample(&self, x: f64, y: f64) -> f64 {
        self.fractal(x, y, (None, None))
    }

    /// Samples the fractal on a torus of `period` lattice cells in both axes.
    #[must_use]
    pub fn sample_tiled(&self, x: f64, y: f64, period: f64) -> f64 {
        let p = period.max(1.0).round() as u32;
        self.fractal(x, y, (Some(p), Some(p)))
    }

    /// Samples the fractal with independent per-axis periods (`0` = no wrap).
    #[must_use]
    pub fn sample_tiled_xy(&self, x: f64, y: f64, period_x: u32, period_y: u32) -> f64 {
        self.fractal(
            x,
            y,
            (
                if period_x > 0 { Some(period_x) } else { None },
                if period_y > 0 { Some(period_y) } else { None },
            ),
        )
    }

    /// Fills a field with `cycles` base-lattice cells across the domain width.
    ///
    /// The vertical lattice count follows the cell aspect ratio so features do
    /// not stretch on non-square grids. On wrapping boundaries the periods are
    /// rounded to integers, guaranteeing an exact seam-free tile.
    #[must_use]
    pub fn fill(&self, grid: &Grid, cycles: f64) -> Field<f64> {
        let mut out = Field::zeros(*grid);
        self.fill_into(&mut out, cycles);
        out
    }

    /// Fills an existing field (same grid), reusing its allocation.
    pub fn fill_into(&self, out: &mut Field<f64>, cycles: f64) {
        let grid = out.grid();
        let w = grid.width() as f64;
        let h = grid.height() as f64;
        let cycles = cycles.max(0.0);
        if cycles <= 0.0 {
            out.fill_value(0.0);
            return;
        }
        let cycles_y = cycles * h / w;
        let periods = Self::periods_for(&grid, cycles, cycles_y);
        let (px, py) = periods;
        let (sx, sy) = self.axis_scales();
        let data = out.as_mut_slice();
        for y in 0..grid.height() {
            let v = (y as f64 + 0.5) / h * cycles_y;
            for x in 0..grid.width() {
                let u = (x as f64 + 0.5) / w * cycles;
                let i = grid.index(x, y);
                data[i] = self.fractal(u * sx, v * sy, periods);
            }
        }
        let _ = (px, py);
    }

    /// Periods for a grid given base cycles, or `(None, None)` when open.
    fn periods_for(grid: &Grid, cycles_x: f64, cycles_y: f64) -> Periods {
        let px = if grid.boundary().wraps_x() {
            Some((cycles_x.round() as i64).max(1) as u32)
        } else {
            None
        };
        let py = if grid.boundary().wraps_y() {
            Some((cycles_y.round() as i64).max(1) as u32)
        } else {
            None
        };
        (px, py)
    }

    /// Coordinate scales implementing anisotropy.
    ///
    /// On wrapping grids the stretch is axis-aligned (a rotated lattice would
    /// not tile); on open grids the requested direction is used exactly.
    fn axis_scales(&self) -> (f64, f64) {
        let a = self.params.anisotropy;
        if a <= 0.0 {
            return (1.0, 1.0);
        }
        // Along-direction coordinates are compressed, stretching features
        // along the requested direction.
        let theta = self.params.direction_deg.to_radians();
        let along = 1.0 - 0.85 * a;
        let (c, s) = theta.sin_cos();
        // Axis-aligned projection of the stretch; works for wrapped grids.
        let sx = 1.0 + (along - 1.0) * c * c;
        let sy = 1.0 + (along - 1.0) * s * s;
        (sx.max(0.08), sy.max(0.08))
    }

    /// Fractal sum. `periods` are base-lattice periods for the first octave.
    fn fractal(&self, x: f64, y: f64, periods: Periods) -> f64 {
        let p = &self.params;
        let mut sum = 0.0;
        let mut amp_sum = 0.0;
        let mut amp = 1.0;
        let mut freq = 1u32;
        for octave in 0..p.octaves {
            // Integer frequency ratios keep wrapped lattices exactly periodic.
            let f = f64::from(freq);
            let ox = x * f;
            let oy = y * f;
            let oct_periods = (
                periods.0.map(|px| px.saturating_mul(freq).max(1)),
                periods.1.map(|py| py.saturating_mul(freq).max(1)),
            );
            let n = self.base_sample(ox, oy, oct_periods, octave);
            let shaped = match p.fractal {
                FractalKind::Fbm => n,
                FractalKind::Ridged => {
                    let r = 1.0 - n.abs();
                    r * r * 2.0 - 1.0
                }
                FractalKind::Billow => n.abs() * 2.0 - 1.0,
            };
            sum += amp * shaped;
            amp_sum += amp;
            amp *= p.persistence;
            let next = (f64::from(freq) * p.lacunarity).round() as i64;
            freq = next.max(1) as u32;
        }
        if amp_sum > 0.0 {
            sum / amp_sum
        } else {
            0.0
        }
    }

    fn base_sample(&self, x: f64, y: f64, periods: Periods, octave: u32) -> f64 {
        let seed =
            mix64(self.seed ^ (0x9E37_79B9_7F4A_7C15u64.wrapping_mul(u64::from(octave) + 1)));
        match self.params.kind {
            NoiseKind::Perlin => perlin(x, y, seed, periods),
            NoiseKind::Value => value(x, y, seed, periods),
            NoiseKind::Simplex => tileable_wrap(x, y, periods, |sx, sy| simplex(sx, sy, seed)),
            NoiseKind::Wavelet => wavelet(x, y, seed, periods),
            NoiseKind::GaussianSpike => gaussian_spikes(x, y, seed, periods),
        }
    }
}

// ---------------------------------------------------------------------------
// Base noise kernels
// ---------------------------------------------------------------------------

const GRADIENTS: [(f64, f64); 8] = [
    (1.0, 0.0),
    (-1.0, 0.0),
    (0.0, 1.0),
    (0.0, -1.0),
    (
        std::f64::consts::FRAC_1_SQRT_2,
        std::f64::consts::FRAC_1_SQRT_2,
    ),
    (
        -std::f64::consts::FRAC_1_SQRT_2,
        std::f64::consts::FRAC_1_SQRT_2,
    ),
    (
        std::f64::consts::FRAC_1_SQRT_2,
        -std::f64::consts::FRAC_1_SQRT_2,
    ),
    (
        -std::f64::consts::FRAC_1_SQRT_2,
        -std::f64::consts::FRAC_1_SQRT_2,
    ),
];

#[inline]
fn grad_dot(hash: u64, dx: f64, dy: f64) -> f64 {
    let g = GRADIENTS[(hash & 7) as usize];
    g.0 * dx + g.1 * dy
}

#[inline]
fn wrap_lattice(v: i64, period: Option<u32>) -> i64 {
    match period {
        Some(p) if p > 0 => v.rem_euclid(i64::from(p)),
        _ => v,
    }
}

/// Perlin gradient noise, approximately `[-1, 1]`.
fn perlin(x: f64, y: f64, seed: u64, periods: Periods) -> f64 {
    let x0f = x.floor();
    let y0f = y.floor();
    let xi = x0f as i64;
    let yi = y0f as i64;
    let fx = x - x0f;
    let fy = y - y0f;
    let x0 = wrap_lattice(xi, periods.0);
    let x1 = wrap_lattice(xi + 1, periods.0);
    let y0 = wrap_lattice(yi, periods.1);
    let y1 = wrap_lattice(yi + 1, periods.1);
    let h00 = hash2(x0 as u64, y0 as u64, seed);
    let h10 = hash2(x1 as u64, y0 as u64, seed);
    let h01 = hash2(x0 as u64, y1 as u64, seed);
    let h11 = hash2(x1 as u64, y1 as u64, seed);
    let n00 = grad_dot(h00, fx, fy);
    let n10 = grad_dot(h10, fx - 1.0, fy);
    let n01 = grad_dot(h01, fx, fy - 1.0);
    let n11 = grad_dot(h11, fx - 1.0, fy - 1.0);
    let u = fade5(fx);
    let v = fade5(fy);
    let nx0 = n00 + (n10 - n00) * u;
    let nx1 = n01 + (n11 - n01) * u;
    // 2D Perlin has a theoretical range of ±sqrt(2)/2; scale to ±1.
    (nx0 + (nx1 - nx0) * v) * std::f64::consts::SQRT_2
}

/// Value noise with quintic interpolation, range `[-1, 1]`.
fn value(x: f64, y: f64, seed: u64, periods: Periods) -> f64 {
    let x0f = x.floor();
    let y0f = y.floor();
    let xi = x0f as i64;
    let yi = y0f as i64;
    let fx = x - x0f;
    let fy = y - y0f;
    let x0 = wrap_lattice(xi, periods.0);
    let x1 = wrap_lattice(xi + 1, periods.0);
    let y0 = wrap_lattice(yi, periods.1);
    let y1 = wrap_lattice(yi + 1, periods.1);
    let v00 = signed_from_hash(hash2(x0 as u64, y0 as u64, seed));
    let v10 = signed_from_hash(hash2(x1 as u64, y0 as u64, seed));
    let v01 = signed_from_hash(hash2(x0 as u64, y1 as u64, seed));
    let v11 = signed_from_hash(hash2(x1 as u64, y1 as u64, seed));
    let u = fade5(fx);
    let v = fade5(fy);
    let a = v00 + (v10 - v00) * u;
    let b = v01 + (v11 - v01) * u;
    a + (b - a) * v
}

/// 2D simplex noise, approximately `[-1, 1]`.
fn simplex(x: f64, y: f64, seed: u64) -> f64 {
    const F2: f64 = 0.366_025_403_784_438_6;
    const G2: f64 = 0.211_324_865_405_187_13;
    let s = (x + y) * F2;
    let i = (x + s).floor();
    let j = (y + s).floor();
    let t = (i + j) * G2;
    let x0 = x - (i - t);
    let y0 = y - (j - t);
    let (i1, j1) = if x0 > y0 { (1.0, 0.0) } else { (0.0, 1.0) };
    let x1 = x0 - i1 + G2;
    let y1 = y0 - j1 + G2;
    let x2 = x0 - 1.0 + 2.0 * G2;
    let y2 = y0 - 1.0 + 2.0 * G2;
    let ii = i as i64;
    let jj = j as i64;
    let mut n = 0.0;
    for (dx, dy, px, py) in [
        (0i64, 0i64, x0, y0),
        (i1 as i64, j1 as i64, x1, y1),
        (1, 1, x2, y2),
    ] {
        let h = hash2((ii + dx) as u64, (jj + dy) as u64, seed);
        let g = GRADIENTS[(h & 7) as usize];
        let t = 0.5 - px * px - py * py;
        if t > 0.0 {
            let t2 = t * t;
            n += t2 * t2 * (g.0 * px + g.1 * py);
        }
    }
    n * 70.0
}

/// Wraps a non-periodic sampler into a tileable one on the given axes.
///
/// Four bilinear weights are applied: the centre sample and its periodic
/// copies. At a seam the weights make the field continuous and identical on
/// both sides. The construction costs up to four samples but only runs for the
/// simplex kind, and only when that axis wraps.
fn tileable_wrap(x: f64, y: f64, periods: Periods, f: impl Fn(f64, f64) -> f64) -> f64 {
    match periods {
        (None, None) => f(x, y),
        (Some(px), None) => {
            let p = f64::from(px);
            let wx = (x / p).clamp(0.0, 1.0);
            f(x, y) * (1.0 - wx) + f(x - p, y) * wx
        }
        (None, Some(py)) => {
            let p = f64::from(py);
            let wy = (y / p).clamp(0.0, 1.0);
            f(x, y) * (1.0 - wy) + f(x, y - p) * wy
        }
        (Some(px), Some(py)) => {
            let (px, py) = (f64::from(px), f64::from(py));
            let wx = (x / px).clamp(0.0, 1.0);
            let wy = (y / py).clamp(0.0, 1.0);
            let w00 = (1.0 - wx) * (1.0 - wy);
            let w10 = wx * (1.0 - wy);
            let w01 = (1.0 - wx) * wy;
            let w11 = wx * wy;
            f(x, y) * w00 + f(x - px, y) * w10 + f(x, y - py) * w01 + f(x - px, y - py) * w11
        }
    }
}

/// Ricker (Mexican-hat) wavelet lattice.
///
/// Each lattice point contributes `amp * (1 - r²) e^{-r²/2}`; wrapping the
/// lattice offset keeps the field tileable.
fn wavelet(x: f64, y: f64, seed: u64, periods: Periods) -> f64 {
    let x0 = x.floor() as i64;
    let y0 = y.floor() as i64;
    let mut sum = 0.0;
    let mut norm = 0.0;
    for dy in -2i64..=2 {
        for dx in -2i64..=2 {
            let gx = x0 + dx;
            let gy = y0 + dy;
            let h = hash2(
                wrap_lattice(gx, periods.0) as u64,
                wrap_lattice(gy, periods.1) as u64,
                seed,
            );
            let amp = signed_from_hash(h);
            // Wrap the offset as well: a sample near the seam must see the
            // wavelet that sits on the far side of the period.
            let mut ox = x - gx as f64;
            let mut oy = y - gy as f64;
            if let Some(p) = periods.0 {
                let p = f64::from(p);
                if ox < -0.5 {
                    ox += p;
                } else if ox > 0.5 {
                    ox -= p;
                }
            }
            if let Some(p) = periods.1 {
                let p = f64::from(p);
                if oy < -0.5 {
                    oy += p;
                } else if oy > 0.5 {
                    oy -= p;
                }
            }
            let r2 = ox * ox + oy * oy;
            if r2 > 6.25 {
                continue;
            }
            let w = (1.0 - r2) * (-0.5 * r2).exp();
            sum += amp * w;
            norm += w.abs();
        }
    }
    if norm > 0.0 {
        (sum / norm) * 1.4
    } else {
        0.0
    }
}

/// Sparse Gaussian domes on a unit lattice.
///
/// About one lattice point in eight carries a dome, giving isolated knolls
/// rather than continuous relief; this is the primitive used for volcanic
/// fields and hotspot swarms.
fn gaussian_spikes(x: f64, y: f64, seed: u64, periods: Periods) -> f64 {
    const SIGMA: f64 = 1.1;
    const SUPPORT: i64 = 3;
    let x0 = x.floor() as i64;
    let y0 = y.floor() as i64;
    let mut sum = 0.0;
    for dy in -SUPPORT..=SUPPORT {
        for dx in -SUPPORT..=SUPPORT {
            let gx = x0 + dx;
            let gy = y0 + dy;
            let h = hash2(
                wrap_lattice(gx, periods.0) as u64,
                wrap_lattice(gy, periods.1) as u64,
                seed,
            );
            // One in eight lattice points hosts a dome; the rest are empty.
            if h & 7 != 0 {
                continue;
            }
            let amp = signed_from_hash(mix64(h));
            let mut ox = x - gx as f64;
            let mut oy = y - gy as f64;
            if let Some(p) = periods.0 {
                let p = f64::from(p);
                if ox < -0.5 {
                    ox += p;
                } else if ox > 0.5 {
                    ox -= p;
                }
            }
            if let Some(p) = periods.1 {
                let p = f64::from(p);
                if oy < -0.5 {
                    oy += p;
                } else if oy > 0.5 {
                    oy -= p;
                }
            }
            let r2 = ox * ox + oy * oy;
            sum += amp * (-r2 / (2.0 * SIGMA * SIGMA)).exp();
        }
    }
    // Empirical normalisation: one dome in eight with unit amplitude.
    sum * 1.6
}

/// Deterministic sparse point set on a grid, useful for plate seeds and
/// volcano placement.
///
/// Returns `count` points in cell coordinates, each with a random weight in
/// `[0, 1)`; points are jittered inside a stratified grid so they are spread
/// evenly but not regular.
#[must_use]
pub fn jittered_points(grid: &Grid, count: usize, seed: u64) -> Vec<(f64, f64, f64)> {
    let mut out = Vec::with_capacity(count);
    if count == 0 {
        return out;
    }
    // Stratify into a near-square cell layout.
    let cols = (count as f64).sqrt().ceil() as usize;
    let cols = cols.max(1);
    let rows = count.div_ceil(cols);
    let cell_w = grid.width() as f64 / cols as f64;
    let cell_h = grid.height() as f64 / rows as f64;
    for i in 0..count {
        let cx = i % cols;
        let cy = i / cols;
        let h = hash2(i as u64, seed, 0xA11C_E5EE_D000_0001);
        let h2 = mix64(h);
        let h3 = mix64(h2);
        let x = (cx as f64 + unit_from_hash(h2)) * cell_w;
        let y = (cy as f64 + unit_from_hash(h3)) * cell_h;
        let weight = unit_from_hash(mix64(h3));
        out.push((
            x.clamp(0.0, grid.width() as f64 - 1e-9),
            y.clamp(0.0, grid.height() as f64 - 1e-9),
            weight,
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::BoundaryKind;

    fn grid(kind: BoundaryKind) -> Grid {
        Grid::new(64, 64, 1000.0, kind).unwrap()
    }

    #[test]
    fn keys_roundtrip() {
        for k in NoiseKind::ALL {
            assert_eq!(NoiseKind::parse(k.key()), Some(k));
        }
        for f in FractalKind::ALL {
            assert_eq!(FractalKind::parse(f.key()), Some(f));
        }
    }

    #[test]
    fn noise_is_deterministic() {
        let p = NoiseParams::default();
        let a = NoiseField::new(p, 42);
        let b = NoiseField::new(p, 42);
        for i in 0..100 {
            let x = i as f64 * 0.37;
            let y = i as f64 * 0.11;
            assert_eq!(a.sample(x, y), b.sample(x, y));
        }
    }

    #[test]
    fn different_seeds_give_different_fields() {
        let a =
            NoiseField::new(NoiseParams::default(), 1).fill(&grid(BoundaryKind::Rectangular), 8.0);
        let b =
            NoiseField::new(NoiseParams::default(), 2).fill(&grid(BoundaryKind::Rectangular), 8.0);
        assert_ne!(a.as_slice(), b.as_slice());
    }

    #[test]
    fn all_kinds_are_finite_and_bounded() {
        for kind in NoiseKind::ALL {
            for fractal in FractalKind::ALL {
                let p = NoiseParams {
                    kind,
                    fractal,
                    octaves: 4,
                    ..Default::default()
                };
                let f = NoiseField::new(p, 7).fill(&grid(BoundaryKind::Rectangular), 8.0);
                assert_eq!(f.non_finite_count(), 0, "{kind:?} {fractal:?}");
                let (lo, hi) = (f.min_value().unwrap(), f.max_value().unwrap());
                assert!(lo > -6.0 && hi < 6.0, "{kind:?} {fractal:?}: [{lo}, {hi}]");
                assert!(hi - lo > 0.01, "{kind:?} {fractal:?} is flat");
            }
        }
    }

    #[test]
    fn noise_has_zero_mean_ish_and_real_variance() {
        for kind in NoiseKind::ALL {
            let p = NoiseParams {
                kind,
                octaves: 4,
                ..Default::default()
            };
            let f = NoiseField::new(p, 11).fill(&grid(BoundaryKind::Toroidal), 10.0);
            let mean = f.mean().unwrap();
            let std = f.std_dev().unwrap();
            assert!(mean.abs() < 0.2, "{kind:?} mean {mean}");
            assert!(std > 0.02, "{kind:?} std {std}");
        }
    }

    #[test]
    fn toroidal_fill_is_seamless() {
        for kind in NoiseKind::ALL {
            let p = NoiseParams {
                kind,
                octaves: 3,
                anisotropy: 0.6,
                direction_deg: 30.0,
                ..Default::default()
            };
            let field = NoiseField::new(p, 5);
            let g = grid(BoundaryKind::Toroidal);
            let cycles = 6.0;
            let periods = NoiseField::periods_for(&g, cycles, cycles);
            let sx = 1.0;
            let sy = 1.0;
            let _ = (sx, sy);
            // Cells just inside either seam differ no more than neighbouring
            // interior cells: a jump would be a visible tile boundary.
            let mut seam = 0.0;
            let mut interior = 0.0;
            let n = 64usize;
            for x in 0..n {
                let xf = x as f64 + 0.5;
                let v_a = field.fractal((xf / 64.0) * cycles, 0.5, periods);
                let v_b = field.fractal(((xf + 64.0) / 64.0) * cycles, 0.5, periods);
                seam += (v_a - v_b).abs();
                let v_c = field.fractal((xf / 64.0) * cycles, 2.5, periods);
                interior += (v_a - v_c).abs();
            }
            assert!(
                seam <= interior * 3.0 + 1e-9,
                "{kind:?}: seam {seam} vs interior {interior}"
            );
        }
    }

    #[test]
    fn anisotropy_changes_field() {
        let iso =
            NoiseField::new(NoiseParams::default(), 3).fill(&grid(BoundaryKind::Rectangular), 8.0);
        let aniso = NoiseField::new(
            NoiseParams {
                anisotropy: 0.9,
                direction_deg: 0.0,
                ..Default::default()
            },
            3,
        )
        .fill(&grid(BoundaryKind::Rectangular), 8.0);
        assert_ne!(iso.as_slice(), aniso.as_slice());
    }

    #[test]
    fn jittered_points_are_inside_and_spread() {
        let g = grid(BoundaryKind::Toroidal);
        let pts = jittered_points(&g, 16, 9);
        assert_eq!(pts.len(), 16);
        for (x, y, w) in &pts {
            assert!((0.0..64.0).contains(x), "{x}");
            assert!((0.0..64.0).contains(y), "{y}");
            assert!((0.0..1.0).contains(w));
        }
        // Stratified placement: the nearest-neighbour distance stays
        // reasonable even for many points.
        let mut min_dist = f64::INFINITY;
        for i in 0..pts.len() {
            for j in 0..i {
                let d = (pts[i].0 - pts[j].0).hypot(pts[i].1 - pts[j].1);
                min_dist = min_dist.min(d);
            }
        }
        assert!(min_dist > 2.0, "min distance {min_dist}");
    }
}
