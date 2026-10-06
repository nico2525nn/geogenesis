//! Rainfall: the spatial precipitation field that drives everything else.
//!
//! The formula from spec §3a is `rain(cell) = base × terrain corrections`, but
//! "terrain corrections" has to be unpacked: with elevation gain, windward
//! enhancement, leeward rain shadow, patchiness and seasonality each acting
//! multiplicatively, in mm/yr. The product structure matters because adding the
//! terms instead would let a negative shadow drive rainfall below zero.
//!
//! The output feeds fluvial erosion (runoff), chemical weathering (throughflow)
//! and the biome model (water availability).

use crate::grid::ScalarField;
use crate::linalg::clamp;

/// Rainfall parameters, matching spec §3a ranges.
#[derive(Debug, Clone, PartialEq)]
pub struct RainfallParams {
    /// 0.0 … 10,000 mm/yr.
    pub annual_mm: f64,
    /// 0.0 … 1.0: spatial patchiness of the base field.
    pub heterogeneity: f64,
    /// 0.0 … 360°: prevailing wind bearing, degrees from +x.
    pub wind_dir_deg: f64,
    /// 0.0 … 1.0: how strongly mountains dry their lee side.
    pub rain_shadow_strength: f64,
    /// −1.0 … 1.0: correlation of rainfall with elevation.
    pub elevation_dependence: f64,
    /// 0.0 … 1.0: 0 = uniform through the year, 1 = all rain in one season.
    pub seasonality: f64,
}

impl Default for RainfallParams {
    fn default() -> Self {
        Self {
            annual_mm: 1200.0,
            heterogeneity: 0.3,
            wind_dir_deg: 270.0,
            rain_shadow_strength: 0.7,
            elevation_dependence: 0.2,
            seasonality: 0.3,
        }
    }
}

/// Computes the annual precipitation field in mm/yr.
///
/// Pure function of `(elevation, params, seed)`: no simulation state, so it is
/// cheap to recompute whenever the surface changes.
pub fn precipitation(
    elevation: &ScalarField,
    params: &RainfallParams,
    seed: u64,
    cell_size_m: f64,
) -> ScalarField {
    let b = *elevation.boundary();
    let (w, h) = (b.width(), b.height());
    let mut out = ScalarField::zeros(b);

    let wind = (params.wind_dir_deg.to_radians().cos(), params.wind_dir_deg.to_radians().sin());
    let shadow = clamp(params.rain_shadow_strength, 0.0, 1.0);
    let elev_dep = clamp(params.elevation_dependence, -1.0, 1.0);
    let base = params.annual_mm.max(0.0);

    // Patchiness field: one value per cell, reproducible from the seed.
    let hetero_amp = clamp(params.heterogeneity, 0.0, 1.0);

    for y in 0..h {
        for x in 0..w {
            let z = elevation.get(x, y);

            // 1. Base patchiness: base × (1 ± hetero·noise).
            let n = crate::rng::hash2_f64(x as i64, y as i64, seed);
            let patch = 1.0 + hetero_amp * (2.0 * n - 1.0) * 0.5;

            // 2. Orographic lift: air rising over high ground condenses.
            // Scaled so elevation_dependence = 1 doubles rainfall over a
            // `orographic_scale` climb, and −1 halves it.
            let gain = 1.0 + elev_dep * (z / 3000.0).clamp(-1.0, 1.0);

            // 3. Rain shadow: compare against the terrain *upwind*.
            // A high upwind ridge casts a dry shadow whose strength falls off
            // with distance. Sample a short upwind fetch and take the maximum
            // elevation along it.
            let shade = upwind_relief(elevation, x, y, wind, cell_size_m, 8);
            let shadow_factor = 1.0 - shadow * (shade / 2000.0).clamp(0.0, 1.0);

            out.set(x, y, base * patch * gain * shadow_factor.max(0.0));
        }
    }
    out.sanitize_non_finite(base);
    out.clamp_all(0.0, 20_000.0);
    out
}

/// Maximum relief above the cell along the upwind fetch, in metres.
fn upwind_relief(
    elevation: &ScalarField,
    x: usize,
    y: usize,
    wind: (f64, f64),
    cell_size_m: f64,
    steps: usize,
) -> f64 {
    let z = elevation.get(x, y);
    let mut max_rise = 0.0;
    for i in 1..=steps {
        let px = x as f64 - wind.0 * i as f64;
        let py = y as f64 - wind.1 * i as f64;
        let (ix, iy) = (px.round() as i64, py.round() as i64);
        // Distance decay: a ridge 8 cells upwind shields less than one
        // immediately adjacent. Inverse-distance weighting is standard.
        let falloff = cell_size_m.max(1.0) * i as f64;
        let rise = (elevation.get_wrapped(ix, iy) - z) / falloff.max(1.0);
        if rise > max_rise {
            max_rise = rise;
        }
    }
    (max_rise * cell_size_m).max(0.0)
}

/// Fraction of the annual total falling in the wettest quarter.
///
/// Maps `seasonality` 0 → 1/4 (uniform) and 1 → ~0.9 (monsoon).
pub fn wet_season_fraction(seasonality: f64) -> f64 {
    0.25 + clamp(seasonality, 0.0, 1.0) * 0.65
}

/// Effective erosive rainfall: the part of the rain that runs off.
///
/// Only the excess over infiltration does geomorphic work. `infiltration_mm`
/// is the land-surface intake capacity (spec §3b `雨水浸透率` scaled to mm/yr).
pub fn effective_runoff_mm(rain_mm: f64, infiltration_mm: f64) -> f64 {
    (rain_mm - infiltration_mm).max(0.0)
}

/// Runoff field in mm/yr, after infiltration.
pub fn runoff_field(
    precipitation: &ScalarField,
    infiltration_mm: f64,
    soil_depth: &ScalarField,
) -> ScalarField {
    let b = *precipitation.boundary();
    let mut out = ScalarField::zeros(b);
    for i in 0..b.cell_count() {
        let rain = precipitation.get_index(i);
        // Deep soil stores more water, so infiltration rises with soil depth.
        let capacity = infiltration_mm * (1.0 + soil_depth.get_index(i) / 2.0);
        out.set_index(i, effective_runoff_mm(rain, capacity));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::{Boundary, BoundaryKind};

    fn test_grid(w: usize, h: usize) -> Boundary {
        Boundary::new(BoundaryKind::Rectangular, w, h)
    }

    fn flat_elevation(b: Boundary, z: f64) -> ScalarField {
        ScalarField::filled(b, z)
    }

    #[test]
    fn rainfall_is_non_negative_and_bounded() {
        let b = test_grid(24, 24);
        let z = flat_elevation(b, 1000.0);
        let rain = precipitation(&z, &RainfallParams::default(), 5, 100.0);
        assert!(rain.min() >= 0.0, "rain went negative");
        assert!(rain.max() <= 20_000.0, "rain exceeded the cap");
        assert_eq!(rain.non_finite_count(), 0);
    }

    #[test]
    fn zero_base_rain_gives_a_zero_field() {
        let b = test_grid(12, 12);
        let z = flat_elevation(b, 500.0);
        let rain = precipitation(
            &z,
            &RainfallParams { annual_mm: 0.0, ..Default::default() },
            1,
            100.0,
        );
        assert_eq!(rain.max(), 0.0);
    }

    #[test]
    fn elevation_gain_increases_rainfall_when_positive() {
        let b = test_grid(16, 16);
        let mut z = ScalarField::zeros(b);
        for y in 0..16 {
            for x in 0..16 {
                z.set(x, y, if x < 8 { 0.0 } else { 2000.0 });
            }
        }
        let params = RainfallParams {
            annual_mm: 1000.0,
            heterogeneity: 0.0,
            elevation_dependence: 1.0,
            rain_shadow_strength: 0.0,
            ..Default::default()
        };
        let rain = precipitation(&z, &params, 3, 100.0);
        assert!(
            rain.get(12, 8) > rain.get(4, 8),
            "high ground should be wetter: {} vs {}",
            rain.get(12, 8),
            rain.get(4, 8)
        );
    }

    #[test]
    fn windward_is_wetter_than_leeward() {
        // A ridge with wind blowing in +x: the west flank is windward.
        let b = test_grid(32, 8);
        let mut z = ScalarField::zeros(b);
        for y in 0..8 {
            for x in 0..32 {
                // Gaussian ridge centred at x = 16.
                let d = (x as f64 - 16.0) / 4.0;
                z.set(x, y, 2000.0 * (-(d * d)).exp());
            }
        }
        let params = RainfallParams {
            annual_mm: 1000.0,
            heterogeneity: 0.0,
            elevation_dependence: 0.0,
            rain_shadow_strength: 1.0,
            wind_dir_deg: 0.0, // blowing toward +x
            ..Default::default()
        };
        let rain = precipitation(&z, &params, 9, 100.0);
        let windward = rain.get(10, 4);
        let leeward = rain.get(22, 4);
        assert!(
            windward > leeward,
            "windward {windward} should exceed leeward {leeward}"
        );
    }

    #[test]
    fn flat_and_seed_deterministic() {
        let b = test_grid(16, 16);
        let z = flat_elevation(b, 800.0);
        let a = precipitation(&z, &RainfallParams::default(), 42, 100.0);
        let c = precipitation(&z, &RainfallParams::default(), 42, 100.0);
        assert_eq!(a.as_slice(), c.as_slice());
        let d = precipitation(&z, &RainfallParams::default(), 43, 100.0);
        assert_ne!(a.as_slice(), d.as_slice(), "seed had no effect");
    }

    #[test]
    fn patchiness_increases_spatial_variance() {
        let b = test_grid(24, 24);
        let z = flat_elevation(b, 500.0);
        let smooth = precipitation(
            &z,
            &RainfallParams { heterogeneity: 0.0, ..Default::default() },
            11,
            100.0,
        );
        let patchy = precipitation(
            &z,
            &RainfallParams { heterogeneity: 1.0, ..Default::default() },
            11,
            100.0,
        );
        assert!(patchy.std_dev() > smooth.std_dev());
        // Means must still agree: patchiness redistributes, it does not create.
        assert!(
            (patchy.mean() - smooth.mean()).abs() / smooth.mean() < 0.1,
            "patchiness changed the mean: {} vs {}",
            patchy.mean(),
            smooth.mean()
        );
    }

    #[test]
    fn wet_season_fraction_endpoints() {
        assert!((wet_season_fraction(0.0) - 0.25).abs() < 1e-12);
        assert!(wet_season_fraction(1.0) > 0.8);
    }

    #[test]
    fn runoff_is_rain_minus_infiltration() {
        assert_eq!(effective_runoff_mm(1000.0, 300.0), 700.0);
        assert_eq!(effective_runoff_mm(200.0, 300.0), 0.0);
    }

    #[test]
    fn runoff_field_accounts_for_soil_storage() {
        let b = test_grid(8, 8);
        let rain = ScalarField::filled(b, 800.0);
        let thin = ScalarField::filled(b, 0.2);
        let deep = ScalarField::filled(b, 5.0);
        let r_thin = runoff_field(&rain, 400.0, &thin);
        let r_deep = runoff_field(&rain, 400.0, &deep);
        assert!(
            r_deep.mean() < r_thin.mean(),
            "deep soil should store more water"
        );
    }
}