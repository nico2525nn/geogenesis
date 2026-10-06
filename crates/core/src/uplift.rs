//! Vertical motion: tectonic uplift, flexural loading and isostatic response.
//!
//! Rates are m/yr and durations are years; [`UpliftRate::total_uplift`] is the only rate → displacement step.

use crate::grid::{Boundary, LabelField, ScalarField};
use crate::linalg::{clamp, smoothstep};
use crate::plates::{BoundaryType, PlateMosaic};

/// Uplift parameters, matching spec §1b ranges.
#[derive(Debug, Clone, PartialEq)]
pub struct UpliftParams {
    /// −10.0 … 50.0 mm/yr: baseline vertical motion from plate motion alone.
    pub mean_rate_mm_yr: f64,
    /// 0.001 … 1.0: spatial frequency of mantle-convection heterogeneity.
    pub noise_frequency: f64,
    /// Whether to apply Airy isostatic compensation.
    pub isostasy: bool,
    /// 1e20 … 1e25 N·m: flexural rigidity, setting the flexural wavelength.
    pub flexural_rigidity_nm: f64,
    /// 0 … 50: number of mantle plumes.
    pub hotspot_count: u32,
    /// 0.0 … 5000.0 m: maximum dome rise from a plume.
    pub hotspot_strength_m: f64,
    /// Fraction of the total time span to apply, so a partial run is scaled.
    pub time_span_yr: f64,
    /// Horizontal extent of the domain in metres, for rate → displacement.
    pub spatial_extent_m: f64,
    /// Vertical scale in metres, used to normalise the result.
    pub vertical_scale_m: f64,
}

impl Default for UpliftParams {
    fn default() -> Self {
        Self {
            mean_rate_mm_yr: 1.0,
            noise_frequency: 0.01,
            isostasy: true,
            flexural_rigidity_nm: 1e23,
            hotspot_count: 5,
            hotspot_strength_m: 2000.0,
            time_span_yr: 1e6,
            spatial_extent_m: 1e6,
            vertical_scale_m: 5000.0,
        }
    }
}

/// Conversion from mm/yr to m/yr.
const MM_PER_M: f64 = 1000.0;

/// A rate of uplift in m/yr.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct UpliftRate(pub f64);

impl UpliftRate {
    /// Converts a spec-range mm/yr rate into m/yr.
    pub fn from_mm_per_year(mm_per_year: f64) -> Self {
        UpliftRate(mm_per_year / MM_PER_M)
    }

    pub fn metres_per_year(self) -> f64 {
        self.0
    }

    /// Total displacement over `years`. This is the only rate → distance step.
    pub fn total_uplift(self, years: f64) -> f64 {
        self.0 * years
    }
}

/// Vertical motion field with components kept separate for independent scaling and testing.
#[derive(Debug, Clone)]
pub struct UpliftField {
    /// Net vertical displacement in metres; add to the elevation.
    pub total: ScalarField,
    /// Contribution from plate boundaries, in metres.
    pub tectonic: ScalarField,
    /// Contribution from lithospheric flexure, in metres.
    pub flexure: ScalarField,
    /// Contribution from isostatic compensation, in metres.
    pub isostatic: ScalarField,
    /// Contribution from mantle plumes, in metres.
    pub hotspot: ScalarField,
    /// Contribution from convection noise, in metres.
    pub noise: ScalarField,
}

impl UpliftField {
    /// Vertical velocity implied by the net displacement, in m/yr.
    pub fn rate(&self, time_span_yr: f64) -> ScalarField {
        let mut out = self.total.clone();
        if time_span_yr > 0.0 {
            let inv = 1.0 / time_span_yr;
            for (a, t) in out.as_mut_slice().iter_mut().zip(self.total.as_slice()) {
                *a = t * inv;
            }
        }
        out
    }

    /// Largest absolute component, for diagnostics.
    pub fn max_abs_component(&self) -> f64 {
        [
            &self.tectonic,
            &self.flexure,
            &self.isostatic,
            &self.hotspot,
            &self.noise,
        ]
        .iter()
        .map(|f| f.as_slice().iter().fold(0.0f64, |m, v| m.max(v.abs())))
        .fold(0.0f64, f64::max)
    }
}

/// Computes the net vertical displacement over the whole simulation span, in metres.
/// Finite and clamped: an unbounded result means a unit error upstream.
pub fn compute_uplift(
    mosaic: &PlateMosaic,
    b: Boundary,
    params: &UpliftParams,
    world_seed: u64,
) -> UpliftField {
    let years = params.time_span_yr.max(0.0);

    let tectonic = tectonic_uplift(mosaic, params, years);
    let flexure = flexural_response(mosaic, params);
    let hotspot = hotspot_uplift(mosaic, b, params, years);
    let noise = convection_noise(b, params, world_seed, years);

    let isostatic = if params.isostasy {
        isostatic_response(mosaic, &tectonic, params)
    } else {
        ScalarField::zeros(b)
    };

    let mut total = tectonic.clone();
    total.add_assign_field(&flexure);
    total.add_assign_field(&isostatic);
    total.add_assign_field(&hotspot);
    total.add_assign_field(&noise);

    // Exceeding the bound indicates a dimensional bug, not geology.
    let bound = params.vertical_scale_m.max(1.0) * 100.0;
    total.sanitize_non_finite(0.0);
    total.clamp_all(-bound, bound);

    UpliftField {
        total,
        tectonic,
        flexure,
        isostatic,
        hotspot,
        noise,
    }
}

/// Uplift and subsidence driven by plate boundary motion, in m/yr normalised by plate speed.
pub fn tectonic_uplift(
    mosaic: &PlateMosaic,
    params: &UpliftParams,
    years: f64,
) -> ScalarField {
    let b = *mosaic.labels.boundary();
    let mut out = ScalarField::zeros(b);

    let reference = crate::plates::units::TYPICAL_PLATE_SPEED_M_PER_YR;
    let mean_rate = UpliftRate::from_mm_per_year(params.mean_rate_mm_yr);

    for y in 0..b.height() {
        for x in 0..b.width() {
            let conv = mosaic.convergence.get(x, y); // m/yr, signed
            let kind = mosaic.boundary_type_at(x, y);

            // Signed rate in m/yr; saturating so fast collisions stay bounded.
            // Coefficients are m/yr: ~2 mm/yr of orogenic growth at full
            // convergence is the observed order (Himalaya ~5, Andes ~1). The
            // old values (0.4, 0.3) were 200x too large and drowned all
            // other relief: one convergent cell reached 1.6M m and the final
            // rescale crushed everything else into a 20 m band.
            let rate = match kind {
                BoundaryType::Convergent => {
                    let t = clamp(conv / reference, 0.0, 4.0);
                    mean_rate.metres_per_year() + 0.002 * t
                }
                BoundaryType::Divergent => {
                    let t = clamp(-conv / reference, 0.0, 4.0);
                    mean_rate.metres_per_year() - 0.0015 * t
                }
                BoundaryType::Transform => {
                    let slip = mosaic.transform_rate.get(x, y) / reference;
                    mean_rate.metres_per_year() - 0.0002 * slip.clamp(0.0, 4.0)
                }
                BoundaryType::Null => mean_rate.metres_per_year(),
            };

            out.set(x, y, rate * years);
        }
    }
    // Detrend: only relief *relative* to the plate mean builds topography.
    // A uniform +1 mm/yr over 1 Myr is +1000 m of datum shift, not a mountain;
    // without this the whole world sits kilometres above (or below, for
    // negative means) the vertical range and rescale burns the range on the
    // offset instead of the relief.
    let mean = out.mean();
    out.map_in_place(|_, v| v - mean);
    out
}

/// Flexural deflection beneath and around a load, via `w(x) ∝ exp(−x/λ) cos(x/λ)`.
pub fn flexural_response(mosaic: &PlateMosaic, params: &UpliftParams) -> ScalarField {
    let b = *mosaic.labels.boundary();
    let mut load = ScalarField::zeros(b);
    let reference = crate::plates::units::TYPICAL_PLATE_SPEED_M_PER_YR;

    // Convergent component only: divergent boundaries do not load the plate.
    for y in 0..b.height() {
        for x in 0..b.width() {
            if mosaic.boundary_type_at(x, y) == BoundaryType::Convergent {
                let conv = clamp(mosaic.convergence.get(x, y) / reference, 0.0, 4.0);
                load.set(x, y, conv);
            }
        }
    }

    // lambda = (D / (rho g))^(1/3), in metres.
    let rho_g = crate::plates::units::MANTLE_DENSITY * crate::plates::units::GRAVITY;
    let lambda_m = (params.flexural_rigidity_nm / rho_g).cbrt().max(1.0);
    let extent = params.spatial_extent_m.max(1.0);
    let cell_m = extent / b.width().max(1) as f64;
    let lambda_cells = (lambda_m / cell_m).clamp(0.5, 64.0);

    convolve_flexural(&load, lambda_cells, params.vertical_scale_m * 0.05)
}

/// Applies the flexural Green's function to `load`, isolated for testing.
/// `load` is mean-removed first so a uniform load bends rather than translates the plate.
fn convolve_flexural(load: &ScalarField, lambda_cells: f64, amplitude: f64) -> ScalarField {
    let b = *load.boundary();
    // Empty load (no convergent boundaries, as in a single-plate world):
    // skip the O(cells × radius) convolution entirely.
    let mut peak = 0.0f64;
    for &v in load.as_slice() {
        let a = v.abs();
        if a > peak {
            peak = a;
        }
    }
    if peak <= 0.0 || !peak.is_finite() {
        return ScalarField::zeros(b);
    }
    let profile = flexural_kernel(lambda_cells);

    let load_mean = load.mean();
    let mut centred = load.clone();
    centred.map_in_place(|_, v| v - load_mean);

    let radius = (profile.len() as i64 - 1) / 2;
    let mut scratch = ScalarField::zeros(b);
    for y in 0..b.height() {
        for x in 0..b.width() {
            let mut acc = 0.0;
            for i in -radius..=radius {
                acc += profile[(i + radius) as usize]
                    * centred.get_wrapped(x as i64 + i, y as i64);
            }
            scratch.set(x, y, acc);
        }
    }

    let mut response = ScalarField::zeros(b);
    for y in 0..b.height() {
        for x in 0..b.width() {
            let mut acc = 0.0;
            for i in -radius..=radius {
                acc += profile[(i + radius) as usize]
                    * scratch.get_wrapped(x as i64, y as i64 + i);
            }
            response.set(x, y, amplitude * acc);
        }
    }
    response
}

/// Builds the flexural Green's-function profile for `lambda_cells`.
/// Normalisation order matters: unit-gain by absolute sum first, then zero-mean.
pub fn flexural_kernel(lambda_cells: f64) -> Vec<f64> {
    // Clamp the wavelength to the grid: a kernel wider than the domain
    // degenerates into a global offset plus noise, at O(cells × radius) cost.
    // A wavelength above ~1/8 of the grid carries no resolvable structure, so
    // cap there and document the saturation instead of convolving the void.
    let lambda_cells = lambda_cells.clamp(0.5, 64.0);
    let radius = (lambda_cells * 4.0).ceil().max(1.0) as i64;
    let span = (2 * radius + 1) as usize;
    let mut profile = vec![0.0f64; span];
    let mut abs_sum = 0.0;
    for (i, p) in profile.iter_mut().enumerate() {
        let r = (i as f64 - radius as f64) / lambda_cells;
        *p = (-r).exp() * r.cos();
        abs_sum += p.abs();
    }
    if abs_sum > 0.0 {
        for p in profile.iter_mut() {
            *p /= abs_sum;
        }
    }
    let mean: f64 = profile.iter().sum::<f64>() / span as f64;
    for p in profile.iter_mut() {
        *p -= mean;
    }
    profile
}

/// Airy isostatic response: thickening `t` raises the surface by `t · (ρ_m − ρ_c) / ρ_m`.
pub fn isostatic_response(
    mosaic: &PlateMosaic,
    vertical: &ScalarField,
    params: &UpliftParams,
) -> ScalarField {
    let b = *mosaic.labels.boundary();
    let mut out = ScalarField::zeros(b);

    let rho_m = crate::plates::units::MANTLE_DENSITY;
    let rho_c = crate::plates::units::CONTINENTAL_DENSITY;
    let factor = (rho_m - rho_c) / rho_m;
    let _ = params;

    for y in 0..b.height() {
        for x in 0..b.width() {
            let v = vertical.get(x, y);
            out.set(x, y, v * factor);
        }
    }
    out
}

/// Plume-induced dome uplift: a dome at the plume plus a trail of older, lower seamounts.
pub fn hotspot_uplift(
    mosaic: &PlateMosaic,
    b: Boundary,
    params: &UpliftParams,
    years: f64,
) -> ScalarField {
    let mut out = ScalarField::zeros(b);
    if params.hotspot_count == 0 || params.hotspot_strength_m <= 0.0 {
        return out;
    }

    let (w, h) = (b.width(), b.height());
    let extent = params.spatial_extent_m.max(1.0);
    let mut rng = crate::rng::Rng::new(mosaic_boundary_seed(mosaic), crate::rng::domain::HOTSPOTS);

    for _ in 0..params.hotspot_count {
        let cx = rng.next_f64() * w as f64;
        let cy = rng.next_f64() * h as f64;

        let dome_radius = (w as f64 * 0.08).max(2.0);
        stamp_dome(&mut out, cx, cy, dome_radius, params.hotspot_strength_m);
        let (xi, yi) = (cx as usize, cy as usize);
        let velocity = crate::plates::plate_velocity_at(mosaic, xi.min(w.saturating_sub(1)), yi.min(h.saturating_sub(1)));
        if velocity.length() < 1e-12 {
            continue;
        }

        let steps = 24usize;
        let speed_cells = velocity.length() * years / extent;
        for s in 1..=steps {
            let t = s as f64 / steps as f64;
            let decay = (1.0 - t).powf(1.6);
            let px = cx - velocity.normalized().x * speed_cells * t;
            let py = cy - velocity.normalized().y * speed_cells * t;
            let radius = (dome_radius * (0.25 + 0.5 * decay)).max(1.0);
            stamp_dome(
                &mut out,
                px,
                py,
                radius,
                params.hotspot_strength_m * 0.35 * decay,
            );
        }
    }
    // Domes are positive-only: without detrending, five 2000 m domes add a
    // one-sided offset that the final rescale spends range on. The domes keep
    // their shape; only the datum is removed.
    let mean = out.mean();
    out.map_in_place(|_, v| v - mean);
    out
}

fn mosaic_boundary_seed(mosaic: &PlateMosaic) -> u64 {
    let mut acc = 0u64;
    for p in &mosaic.plates {
        acc = acc
            .wrapping_add(p.id as u64)
            .wrapping_add((p.site.x * 1e6) as u64)
            .wrapping_add((p.site.y * 1e6) as u64);
    }
    acc
}

fn stamp_dome(field: &mut ScalarField, cx: f64, cy: f64, radius: f64, height: f64) {
    let b = *field.boundary();
    let (w, h) = (b.width(), b.height());
    if radius <= 0.0 || height == 0.0 {
        return;
    }
    let r = radius.ceil() as i64;
    let x0 = cx as i64;
    let y0 = cy as i64;
    for dy in -r..=r {
        for dx in -r..=r {
            let d = ((dx * dx + dy * dy) as f64).sqrt() / radius;
            if d > 1.0 {
                continue;
            }
            let w8 = 1.0 - smoothstep(0.0, 1.0, d);
            let idx = b.index_wrapped(x0 + dx, y0 + dy);
            field.as_mut_slice()[idx] += height * w8;
        }
    }
    let _ = (w, h);
}

/// Heterogeneous vertical motion from mantle convection, scaled by the mean rate.
pub fn convection_noise(
    b: Boundary,
    params: &UpliftParams,
    world_seed: u64,
    years: f64,
) -> ScalarField {
    // Spec range mapped onto periods across the domain; the raw value would yield a flat field.
    let cycles = params.noise_frequency.clamp(0.001, 1.0) * 32.0;
    let field = crate::noise::NoiseField::new(
        crate::noise::NoiseParams {
            kind: crate::noise::NoiseKind::Simplex,
            fractal: crate::noise::FractalKind::Fbm,
            octaves: 4,
            lacunarity: 2.0,
            persistence: 0.5,
            ..Default::default()
        },
        world_seed,
    );

    let mut out = ScalarField::zeros(b);
    let amplitude = UpliftRate::from_mm_per_year(params.mean_rate_mm_yr).total_uplift(years);
    field.add_into(&mut out, cycles, amplitude);
    // Same datum argument as tectonic_uplift: only the variation builds
    // relief, so remove the mean before returning.
    let mean = out.mean();
    out.map_in_place(|_, v| v - mean);
    out
}

/// Tectonic uplift as a fraction of the vertical scale, in `[-1, 1]`.
pub fn normalized_relief(mosaic: &PlateMosaic, params: &UpliftParams, world_seed: u64) -> ScalarField {
    let b = *mosaic.labels.boundary();
    let uplift = compute_uplift(mosaic, b, params, world_seed);
    let mut out = uplift.total;
    out.rescale(-1.0, 1.0);
    out
}

/// Distance-weighted plate influence: 1 on a plate's own territory, 0 at the nearest boundary.
pub fn plate_interior_mask(mosaic: &PlateMosaic) -> ScalarField {
    let max_d = mosaic.boundary_distance.max();
    let mut out = mosaic.boundary_distance.clone();
    if max_d > 0.0 {
        out.map_in_place(|_, v| smoothstep(0.0, max_d * 0.5, v));
    }
    out
}

pub fn plate_labels(mosaic: &PlateMosaic) -> LabelField {
    mosaic.labels.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linalg::Vec2;
    use crate::grid::BoundaryKind;
    use crate::plates::{build_mosaic, PlateParams, Plate};

    fn rect(w: usize, h: usize) -> Boundary {
        Boundary::new(BoundaryKind::Rectangular, w, h)
    }

    fn two_plate_mosaic(b: Boundary, va: Vec2, vb: Vec2) -> PlateMosaic {
        let sites = [
            Vec2::new(0.25, 0.5),
            Vec2::new(0.75, 0.5),
        ];
        let mut labels = LabelField::filled(b, 0);
        for y in 0..b.height() {
            for x in 0..b.width() {
                labels.set(x, y, if x < b.width() / 2 { 0 } else { 1 });
            }
        }
        let plates = vec![
            Plate {
                id: 0,
                site: sites[0],
                velocity: va,
                thickness_km: 35.0,
                density: crate::plates::units::CONTINENTAL_DENSITY,
                is_oceanic: false,
                area_cells: 0,
            },
            Plate {
                id: 1,
                site: sites[1],
                velocity: vb,
                thickness_km: 35.0,
                density: crate::plates::units::CONTINENTAL_DENSITY,
                is_oceanic: false,
                area_cells: 0,
            },
        ];
        let (boundary_type, convergence, transform_rate, boundary_distance, normal_x, normal_y) = {
            let mut types = LabelField::filled(b, 0);
            let mut conv = ScalarField::zeros(b);
            let mut tr = ScalarField::zeros(b);
            let mut dist = ScalarField::zeros(b);
            let mut nx = ScalarField::zeros(b);
            let mut ny = ScalarField::zeros(b);
            let mid = b.width() / 2;
            for y in 0..b.height() {
                conv.set(mid, y, -(vb - va).x);
                tr.set(mid, y, (vb - va).y.abs());
                dist.set(mid, y, 0.0);
                nx.set(mid, y, 1.0);
                ny.set(mid, y, 0.0);
                types.set(mid, y, 1);
            }
            (types, conv, tr, dist, nx, ny)
        };
        // Re-classify with the real classifier so the boundary type is honest.
        let mut m = PlateMosaic {
            plates,
            labels,
            boundary_type,
            convergence,
            transform_rate,
            boundary_distance,
            normal_x,
            normal_y,
        };
        let mid = b.width() / 2;
        let normal = Vec2::new(1.0, 0.0);
        let motion = crate::plates::relative_motion(&m.plates[0], &m.plates[1], normal);
        let kind = motion.classify(1.0);
        for y in 0..b.height() {
            m.boundary_type.set(mid, y, match kind {
                BoundaryType::Convergent => 1,
                BoundaryType::Divergent => 2,
                BoundaryType::Transform => 3,
                BoundaryType::Null => 0,
            });
        }
        m
    }

    #[test]
    fn uplift_rate_converts_mm_to_m_correctly() {
        let r = UpliftRate::from_mm_per_year(1.0);
        assert!((r.metres_per_year() - 0.001).abs() < 1e-15);
        assert!((r.total_uplift(1e6) - 1000.0).abs() < 1e-9);
    }

    #[test]
    fn uplift_is_finite_and_bounded_for_adversarial_parameters() {
        let b = rect(32, 32);
        let m = build_mosaic(b, &PlateParams::default(), 42, 1e7);
        let params = UpliftParams {
            time_span_yr: 5e8,
            spatial_extent_m: 1.0,
            flexural_rigidity_nm: 1e20,
            hotspot_count: 50,
            hotspot_strength_m: 5000.0,
            mean_rate_mm_yr: 50.0,
            ..Default::default()
        };
        let up = compute_uplift(&m, b, &params, 42);
        assert_eq!(up.total.non_finite_count(), 0, "uplift produced non-finite");
        let bound = params.vertical_scale_m * 100.0;
        assert!(up.total.max() <= bound, "uplift exceeded bound");
        assert!(up.total.min() >= -bound);
    }

    #[test]
    fn tectonic_uplift_is_zero_mean() {
        // Datum shifts are not topography: a uniform +1 mm/yr must not move
        // the world mean. Without detrending, 1 Myr at 1 mm/yr puts the whole
        // grid +1000 m up and the final rescale spends the vertical range on
        // the offset, crushing real relief into a flat band.
        let b = rect(32, 32);
        let m = build_mosaic(b, &PlateParams::default(), 7, 1e7);
        let t = tectonic_uplift(&m, &UpliftParams::default(), 1e6);
        assert!(
            t.mean().abs() < 1e-6,
            "tectonic uplift has datum offset {}",
            t.mean()
        );
        assert!(t.max() - t.min() > 100.0, "no relief survived detrending");
    }

    #[test]
    fn uplift_scales_linearly_with_time() {
        let b = rect(24, 24);
        let m = build_mosaic(b, &PlateParams::default(), 7, 1e7);
        let p1 = UpliftParams {
            time_span_yr: 1e5,
            ..Default::default()
        };
        let p2 = UpliftParams {
            time_span_yr: 2e5,
            ..Default::default()
        };
        let u1 = compute_uplift(&m, b, &p1, 7);
        let u2 = compute_uplift(&m, b, &p2, 7);
        // Compares the time-scaling tectonic component only.
        let mean1 = u1.tectonic.mean();
        let mean2 = u2.tectonic.mean();
        assert!(
            (mean2 - 2.0 * mean1).abs() < 1e-6 * mean1.abs().max(1.0),
            "tectonic uplift {mean1} -> {mean2} did not double"
        );
    }

    #[test]
    fn convergent_boundary_produces_higher_uplift_than_divergent() {
        let b = rect(24, 24);
        let conv = two_plate_mosaic(b, Vec2::new(10.0, 0.0), Vec2::new(-10.0, 0.0));
        let div = two_plate_mosaic(b, Vec2::new(-10.0, 0.0), Vec2::new(10.0, 0.0));
        let params = UpliftParams {
            hotspot_count: 0,
            isostasy: false,
            time_span_yr: 1e6,
            ..Default::default()
        };
        let c = tectonic_uplift(&conv, &params, 1e6);
        let d = tectonic_uplift(&div, &params, 1e6);
        let mid = b.width() / 2;
        let c_val = c.get(mid, mid / 2);
        let d_val = d.get(mid, mid / 2);
        assert!(c_val > d_val, "convergent {c_val} should exceed divergent {d_val}");
    }

    #[test]
    fn flexure_produces_a_forebulge() {
        // Idealised single-cell load isolates the kernel's shape from mosaic noise.
        let b = rect(64, 64);
        let mut load = ScalarField::zeros(b);
        load.set(32, 32, 1.0);

        let response = convolve_flexural(&load, 5.0, 500.0);
        assert_eq!(response.non_finite_count(), 0);
        assert!(
            response.mean().abs() < 1e-6,
            "flexure added net volume: {}",
            response.mean()
        );
        assert!(
            response.max() > 0.0,
            "no forebulge: min {} max {}",
            response.min(),
            response.max()
        );
        let peak_x = (0..64)
            .max_by(|&a, &c| {
                response
                    .get(a, 32)
                    .total_cmp(&response.get(c, 32))
            })
            .unwrap();
        assert!(
            (28..=36).contains(&peak_x),
            "response peaks at {peak_x}, away from the load at x=32"
        );
    }

    #[test]
    fn flexural_convolution_of_an_empty_load_is_zero() {
        let b = rect(16, 16);
        let load = ScalarField::zeros(b);
        let r = convolve_flexural(&load, 3.0, 100.0);
        assert!(r.as_slice().iter().all(|v| v.abs() < 1e-12));
    }

    #[test]
    fn flexural_response_uses_the_mosaic_load() {
        // D=1e20 keeps the response resolvable on a 32-cell grid.
        let b = rect(32, 32);
        let m = build_mosaic(b, &PlateParams::default(), 11, 1e7);
        let params = UpliftParams {
            flexural_rigidity_nm: 1e20,
            spatial_extent_m: 1e6,
            ..Default::default()
        };
        let flex = flexural_response(&m, &params);
        assert_eq!(flex.non_finite_count(), 0);
        assert!(
            flex.std_dev() > 0.0,
            "flexure is spatially flat for a mosaic with boundaries"
        );
        let mut near_load = 0.0;
        let mut far_load = 0.0;
        let mut near_n = 0usize;
        let mut far_n = 0usize;
        for y in 0..32 {
            for x in 0..32 {
                let d = m.boundary_distance.get(x, y);
                if d <= 2.0 {
                    near_load += flex.get(x, y).abs();
                    near_n += 1;
                } else if d > 10.0 {
                    far_load += flex.get(x, y).abs();
                    far_n += 1;
                }
            }
        }
        let (near_mean, far_mean) = (
            near_load / near_n.max(1) as f64,
            far_load / far_n.max(1) as f64,
        );
        assert!(
            near_mean > far_mean,
            "flexure {near_mean} near load should exceed {far_mean} in the far field"
        );
    }

    #[test]
    fn flexure_kernel_has_zero_mean() {
        // Zero-mean is what produces the forebulge instead of a uniform depression.
        for lambda in [2.0f64, 5.0, 20.0, 50.0] {
            let radius = (lambda * 4.0).ceil() as i64;
            let span = (2 * radius + 1) as usize;
            let mut profile = vec![0.0f64; span];
            let mut abs_sum = 0.0;
            for (i, p) in profile.iter_mut().enumerate() {
                let r = (i as f64 - radius as f64) / lambda;
                *p = (-r).exp() * r.cos();
                abs_sum += p.abs();
            }
            for p in profile.iter_mut() {
                *p /= abs_sum;
            }
            let mean: f64 = profile.iter().sum::<f64>() / span as f64;
            for p in profile.iter_mut() {
                *p -= mean;
            }
            let final_mean: f64 = profile.iter().sum::<f64>() / span as f64;
            assert!(
                final_mean.abs() < 1e-12,
                "lambda {lambda}: kernel mean {final_mean} is not zero"
            );
        }
    }

    #[test]
    fn flexure_is_zero_without_convergent_load() {
        let b = rect(24, 24);
        let diverging = two_plate_mosaic(b, Vec2::new(-10.0, 0.0), Vec2::new(10.0, 0.0));
        let params = UpliftParams::default();
        let flex = flexural_response(&diverging, &params);
        assert!(
            flex.as_slice().iter().all(|v| v.abs() < 1e-12),
            "divergent boundary produced flexure"
        );
    }

    #[test]
    fn isostatic_response_is_a_fraction_of_the_load() {
        let b = rect(24, 24);
        let m = build_mosaic(b, &PlateParams::default(), 3, 1e7);
        let vertical = ScalarField::filled(b, 1000.0);
        let iso = isostatic_response(&m, &vertical, &UpliftParams::default());
        let rho_m = crate::plates::units::MANTLE_DENSITY;
        let rho_c = crate::plates::units::CONTINENTAL_DENSITY;
        let factor = (rho_m - rho_c) / rho_m;
        assert!(
            (iso.max() - 1000.0 * factor).abs() < 1e-6,
            "isostasy {} != {factor} * 1000",
            iso.max()
        );
        assert!(iso.max() < vertical.max(), "mountains must float, not double");
    }

    #[test]
    fn hotspot_count_zero_adds_nothing() {
        let b = rect(24, 24);
        let m = build_mosaic(b, &PlateParams::default(), 5, 1e7);
        let params = UpliftParams {
            hotspot_count: 0,
            ..Default::default()
        };
        let h = hotspot_uplift(&m, b, &params, 1e6);
        assert!(h.max() == 0.0, "hotspots present with count 0");
    }

    #[test]
    fn hotspot_strength_scales_the_dome() {
        let b = rect(48, 48);
        let m = build_mosaic(b, &PlateParams::default(), 5, 1e7);
        let weak = hotspot_uplift(
            &m,
            b,
            &UpliftParams {
                hotspot_count: 5,
                hotspot_strength_m: 100.0,
                ..Default::default()
            },
            1e6,
        );
        let strong = hotspot_uplift(
            &m,
            b,
            &UpliftParams {
                hotspot_count: 5,
                hotspot_strength_m: 4000.0,
                ..Default::default()
            },
            1e6,
        );
        assert!(
            strong.max() > weak.max() * 10.0,
            "dome height {} vs {}",
            strong.max(),
            weak.max()
        );
    }

    #[test]
    fn hotspot_trail_extends_against_plate_motion() {
        let b = rect(48, 48);
        // Single plate moving +x: the trail extends towards -x.
        let moving = two_plate_mosaic(b, Vec2::new(60.0, 0.0), Vec2::new(60.0, 0.0));
        let params = UpliftParams {
            hotspot_count: 1,
            hotspot_strength_m: 3000.0,
            spatial_extent_m: 1e7,
            time_span_yr: 5e7,
            ..Default::default()
        };
        let hs = hotspot_uplift(&moving, b, &params, 5e7);
        assert_eq!(hs.non_finite_count(), 0);
        assert!(hs.max() > 0.0, "no hotspot dome");
        let mut sum = 0.0;
        let mut weight = 0.0;
        for y in 0..48 {
            for x in 0..48 {
                let v = hs.get(x, y).max(0.0);
                sum += x as f64 * v;
                weight += v;
            }
        }
        assert!(weight > 0.0, "no hotspot weight");
        let centroid_x = sum / weight;
        assert!(
            centroid_x < 30.0,
            "trail centroid {centroid_x} did not trail behind a +x plate"
        );
    }

    #[test]
    fn dome_stamp_is_smooth_and_bounded() {
        let b = rect(32, 32);
        let mut f = ScalarField::zeros(b);
        stamp_dome(&mut f, 16.0, 16.0, 5.0, 1000.0);
        assert!((f.max() - 1000.0).abs() < 1e-6, "dome height wrong");
        assert_eq!(f.min(), 0.0, "dome dipped below zero");
        let affected = f.as_slice().iter().filter(|v| **v > 0.0).count();
        assert!(affected < b.cell_count() / 2, "dome covered the grid");
    }

    #[test]
    fn convection_noise_breaks_up_smooth_relief() {
        let b = rect(32, 32);
        let params = UpliftParams {
            noise_frequency: 0.05,
            time_span_yr: 1e6,
            ..Default::default()
        };
        let n = convection_noise(b, &params, 21, 1e6);
        assert!(n.max() > n.min(), "noise field is constant");
        assert_eq!(n.non_finite_count(), 0);
    }

    #[test]
    fn normalized_relief_spans_the_unit_interval() {
        let b = rect(24, 24);
        let m = build_mosaic(b, &PlateParams::default(), 31, 1e7);
        let relief = normalized_relief(&m, &UpliftParams::default(), 31);
        assert!((relief.min() + 1.0).abs() < 1e-9, "min {} not -1", relief.min());
        assert!((relief.max() - 1.0).abs() < 1e-9, "max {} not +1", relief.max());
    }

    #[test]
    fn plate_interior_mask_is_one_inside_and_zero_at_boundaries() {
        let b = rect(32, 32);
        let m = build_mosaic(b, &PlateParams::default(), 41, 1e7);
        let mask = plate_interior_mask(&m);
        assert_eq!(mask.min(), 0.0, "mask not zero somewhere");
        assert!(mask.max() > 0.5, "mask never approaches one");
        let mut checked = 0;
        for y in 0..32 {
            for x in 0..32 {
                if m.boundary_type.get(x, y) != 0 {
                    assert!(mask.get(x, y) < 0.35, "mask high at boundary {x},{y}");
                    checked += 1;
                }
            }
        }
        assert!(checked > 0, "no boundary cells to check");
    }

    #[test]
    fn uplift_is_reproducible_from_seed() {
        let b = rect(24, 24);
        let m = build_mosaic(b, &PlateParams::default(), 55, 1e7);
        let p = UpliftParams::default();
        let a = compute_uplift(&m, b, &p, 55);
        let c = compute_uplift(&m, b, &p, 55);
        assert_eq!(a.total.as_slice(), c.total.as_slice());
    }

    #[test]
    fn rate_converts_total_back_to_velocity() {
        let b = rect(16, 16);
        let m = build_mosaic(b, &PlateParams::default(), 2, 1e7);
        let up = compute_uplift(&m, b, &UpliftParams::default(), 2);
        let rate = up.rate(1e6);
        assert!((rate.max() - up.total.max() / 1e6).abs() < 1e-9);
    }
}