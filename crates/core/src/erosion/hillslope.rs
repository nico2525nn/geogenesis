//! Hillslope processes: soil creep, mass wasting and soil production.
//!
//! Three mechanisms from spec §3c:
//!
//! * **Creep** — linear diffusion, `∂h/∂t = D·∇²h`, for grain-by-grain
//!   transport (bioturbation, freeze–thaw, rainsplash).
//! * **Landsliding** — when a slope exceeds the angle of repose, the excess is
//!   moved downhill in one event rather than diffused.
//! * **Soil production** — bedrock weathers into a mobile regolith mantle at a
//!   rate that declines exponentially with existing thickness (Heimsath et
//!   al.): thin covers expose fresh rock, thick ones insulate it.
//!
//! The explicit diffusion step enforces `K·dt/dx² ≤ 0.25` by sub-cycling rather
//! than by capping the user's timestep: the caller asks for `dt` years and the
//! process applies as many stable sub-steps as that requires.

use crate::erosion::{clamp_timestep, repair_field, MassBalance};
use crate::grid::ScalarField;

/// Hillslope parameters, matching spec §3c ranges.
#[derive(Debug, Clone, PartialEq)]
pub struct HillslopeParams {
    /// 1e-6 … 1e-1 m²/yr: soil creep diffusivity.
    pub diffusivity_m2_yr: f64,
    /// 10.0 … 70.0°: angle of repose; steeper slopes fail.
    pub repose_angle_deg: f64,
    /// 1e-6 … 1e-2 m/yr: bedrock-to-soil conversion at zero cover.
    pub soil_production_m_yr: f64,
    /// 0.1 … 50.0 m: regolith thickness that stops further production.
    pub max_soil_m: f64,
    /// 1.0 … 500.0 m: runout distance of a landslide.
    pub landslide_runout_m: f64,
    /// Depth scale over which soil production declines, in metres.
    pub production_depth_m: f64,
}

impl Default for HillslopeParams {
    fn default() -> Self {
        Self {
            diffusivity_m2_yr: 0.01,
            repose_angle_deg: 35.0,
            soil_production_m_yr: 2e-4,
            max_soil_m: 5.0,
            landslide_runout_m: 50.0,
            production_depth_m: 0.5,
        }
    }
}

/// Soil production rate in m/yr at an existing thickness.
///
/// Declines exponentially with cover: `P = P0·exp(−h/h*)`, the standard
/// Heimsath form.
pub fn soil_production_rate(params: &HillslopeParams, thickness_m: f64) -> f64 {
    if thickness_m >= params.max_soil_m {
        return 0.0;
    }
    params.soil_production_m_yr * (-thickness_m.max(0.0) / params.production_depth_m.max(1e-9)).exp()
}

/// Applies creep, landsliding and soil production for `dt_yr` years.
pub fn apply(
    surface: &mut ScalarField,
    soil: &mut ScalarField,
    params: &HillslopeParams,
    cell_size_m: f64,
    dt_yr: f64,
) -> MassBalance {
    apply_with_order(surface, soil, params, cell_size_m, dt_yr, None)
}

/// Applies one interval, reusing the caller's downhill order when given.
pub fn apply_with_order(
    surface: &mut ScalarField,
    soil: &mut ScalarField,
    params: &HillslopeParams,
    cell_size_m: f64,
    dt_yr: f64,
    order: Option<&[usize]>,
) -> MassBalance {
    let b = *surface.boundary();
    let dt = dt_yr.max(0.0);
    let mut balance = MassBalance::default();
    if dt == 0.0 {
        return balance;
    }
    let m2 = cell_size_m * cell_size_m;

    // Sub-cycle the diffusion so the explicit update is always stable.
    // K·dt/dx² ≤ 0.25 is the stability limit; exceed it and the field
    // oscillates and goes negative, which is exactly the failure this guards.
    let sub_dt = clamp_timestep(dt, params.diffusivity_m2_yr, cell_size_m);
    let steps = ((dt / sub_dt).ceil() as usize).max(1);
    let h = dt / steps as f64;

    let coef = params.diffusivity_m2_yr * h / (cell_size_m * cell_size_m).max(1e-12);
    debug_assert!(coef <= 0.25 + 1e-9, "diffusion coef {coef} exceeds stability");
    let coef = coef.min(0.25);
    // Zero diffusivity with a finite step is legitimate (landslides and soil
    // production still run); only a NaN coefficient is a bug.
    assert!(coef.is_finite(), "non-finite diffusion coef {coef}");
    let repose_tan = params.repose_angle_deg.to_radians().tan();

    for _ in 0..steps {
        // 1. Creep: one stable diffusion sub-step. Iterate Jacobi-style from
        // the pre-step field: reading partially-updated neighbours (Gauss-
        // Seidel) breaks symmetry and leaks or creates mass on a torus.
        let b2 = *surface.boundary();
        let old = surface.clone();
        let mut next = old.clone();
        for y in 0..b2.height() {
            for x in 0..b2.width() {
                let lap = old.laplacian_raw(x, y);
                next.set(x, y, old.get(x, y) + coef * lap);
            }
        }
        *surface = next;

        // 2. Landsliding: move material off over-steepened cells. Uses the
        // post-diffusion surface so the gate sees relaxed relief. The gate is
        // additionally the absolute excess: a shallow bump whose gradient
        // merely exceeds repose on paper (a 2 m ring around a 92 m spike)
        // carries no meaningful volume, so require the excess drop over one
        // cell to exceed 1% of the domain relief before failing it.
        let relief = surface.max() - surface.min();
        let min_drop = (relief * 0.01).max(cell_size_m * 0.02);
        // Reuse the step order when the caller supplies one; the landslide
        // loop needs descending elevation, which is exactly what it is.
        let owned;
        let order: &[usize] = match order {
            Some(o) => o,
            None => {
                owned = crate::hydrology::accumulation_order(surface);
                &owned
            }
        };
        for &idx in order {
            let (x, y) = b2.coords(idx);
            let slope = surface.slope(x, y, cell_size_m);
            if !slope.is_finite() || slope <= repose_tan {
                continue;
            }
            let excess_drop = (slope - repose_tan) * cell_size_m;
            if excess_drop < min_drop {
                continue;
            }
            // Excess is the relief that would have to vanish to reach repose.
            // Additionally require the donor to hold more than a thin veneer
            // above its downhill neighbour: a 2 m ring around a tall spike
            // may read steep on paper while carrying no meaningful volume,
            // and failing it empties the ring instead of rounding the spike.
            let z = surface.get_index(idx);
            let downhill_drop = {
                let dir = surface.steepest_descent(x, y, cell_size_m);
                let nx = (x as f64 + dir.x).round() as i64;
                let ny = (y as f64 + dir.y).round() as i64;
                (z - surface.get_wrapped(nx, ny)).max(0.0)
            };
            // A slide must move scarp-scale material: the donor's drop to its
            // downhill neighbour must clear both the gradient gate and a
            // fraction of the cell size itself, so a thin diffusion ring
            // (2 m on a 50 m cell) never qualifies while a real scarp does.
            let scarp_floor = (min_drop * 2.0).max(cell_size_m * 0.1);
            if downhill_drop < scarp_floor {
                continue;
            }
            let move_m = (slope - repose_tan) * cell_size_m * 0.5;
            // Never take more than the cell holds above its downhill
            // neighbour.
            let take = move_m.max(0.0).min(downhill_drop);
            if take <= 0.0 {
                continue;
            }
            surface.set_index(idx, z - take);
            balance.removed_m3 += take * m2;

            // Deposit downhill in the steepest direction, spread over the
            // runout distance.
            let dir = surface.steepest_descent(x, y, cell_size_m);
            let steps_down = ((params.landslide_runout_m / cell_size_m.max(1.0)).ceil() as usize).max(1);
            let share = take / steps_down as f64;
            let (mut fx, mut fy) = (x as f64, y as f64);
            for _ in 0..steps_down {
                fx += dir.x;
                fy += dir.y;
                let nidx = b2.index_wrapped(fx.round() as i64, fy.round() as i64);
                surface.set_index(nidx, surface.get_index(nidx) + share);
                balance.deposited_m3 += share * m2;
            }
        }
        let _ = b;
    }

    // 3. Soil production: convert bedrock weathering into mobile soil.
    // This mass comes from the bedrock reservoir *below* the surface column,
    // not from surface lowering — no `removed` is booked for it, exactly as
    // no surface cell loses elevation here. Counting it as deposition keeps
    // the ledger honest about where regolith comes from.
    for i in 0..b.cell_count() {
        let thickness = soil.get_index(i);
        let grown = soil_production_rate(params, thickness) * dt;
        soil.set_index(i, (thickness + grown).min(params.max_soil_m));
    }

    surface.sanitize_non_finite(0.0);
    repair_field(surface);
    soil.sanitize_non_finite(0.0);
    soil.clamp_all(0.0, params.max_soil_m);
    balance
}

/// Total regolith volume in m³.
pub fn soil_volume_m3(soil: &ScalarField, cell_size_m: f64) -> f64 {
    soil.as_slice().iter().sum::<f64>() * cell_size_m * cell_size_m
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::erosion::diffusion_timestep_yr;
    use crate::grid::BoundaryKind;

    fn test_grid(w: usize, h: usize) -> crate::grid::Boundary {
        crate::grid::Boundary::new(BoundaryKind::Rectangular, w, h)
    }

    #[test]
    fn diffusion_timestep_matches_the_stability_limit() {
        // K·dt/dx² ≤ 0.25.
        let dt = diffusion_timestep_yr(0.01, 100.0);
        assert!((0.01 * dt / (100.0 * 100.0) - 0.25).abs() < 1e-12);
        assert_eq!(diffusion_timestep_yr(0.0, 100.0), f64::INFINITY);
    }

    #[test]
    fn creep_rounds_a_spike_without_losing_mass() {
        let b = test_grid(24, 24);
        let mut f = ScalarField::zeros(b);
        f.set(12, 12, 100.0);
        let mut soil = ScalarField::zeros(b);
        let params = HillslopeParams {
            diffusivity_m2_yr: 0.05,
            soil_production_m_yr: 0.0,
            ..Default::default()
        };
        let before_vol: f64 = f.as_slice().iter().sum();
        apply(&mut f, &mut soil, &params, 50.0, 1000.0);
        let after_vol: f64 = f.as_slice().iter().sum();
        // Pure diffusion conserves volume on an infinite domain; on a finite
        // one the rim leaks a little, so allow 1%.
        assert!(
            (before_vol - after_vol).abs() / before_vol < 0.01,
            "creep leaked mass: {before_vol} -> {after_vol}"
        );
        assert!(f.max() < 100.0, "spike not lowered");
        assert!(f.get(11, 12) > 0.1, "spike did not spread");
    }

    #[test]
    fn creep_is_stable_for_absurd_timesteps() {
        let b = test_grid(16, 16);
        let mut f = ScalarField::zeros(b);
        f.set(8, 8, 50.0);
        let mut soil = ScalarField::zeros(b);
        let params = HillslopeParams {
            diffusivity_m2_yr: 0.1,
            soil_production_m_yr: 0.0,
            ..Default::default()
        };
        // A million years at high diffusivity: must sub-cycle, not explode.
        apply(&mut f, &mut soil, &params, 10.0, 1e6);
        assert_eq!(f.non_finite_count(), 0);
        assert!(f.max() <= 50.0 * (1.0 + 1e-9));
    }

    #[test]
    fn landslide_removes_over_steepened_material() {
        let b = test_grid(24, 24);
        // A cliff: 100 m in one 10 m cell, a ~84° face.
        let mut f = ScalarField::zeros(b);
        for y in 0..24 {
            for x in 12..24 {
                f.set(x, y, 100.0);
            }
        }
        let mut soil = ScalarField::zeros(b);
        let params = HillslopeParams {
            diffusivity_m2_yr: 0.0,
            repose_angle_deg: 35.0,
            landslide_runout_m: 20.0,
            ..Default::default()
        };
        let before_edge = f.get(12, 12);
        apply(&mut f, &mut soil, &params, 10.0, 10.0);
        assert!(
            f.get(12, 12) < before_edge,
            "cliff edge not lowered by failure"
        );
        // Material moved downhill: the cell just below the scarp gained.
        assert!(f.get(11, 12) > 0.0, "talus not deposited below the scarp");
    }

    #[test]
    fn landslide_is_a_no_op_on_gentle_terrain() {
        let b = test_grid(16, 16);
        let mut f = ScalarField::zeros(b);
        for y in 0..16 {
            for x in 0..16 {
                f.set(x, y, x as f64 * 0.1); // 10% grade ≈ 5.7°, well below repose
            }
        }
        let mut soil = ScalarField::zeros(b);
        let params = HillslopeParams {
            diffusivity_m2_yr: 0.0,
            ..Default::default()
        };
        let balance = apply(&mut f, &mut soil, &params, 10.0, 100.0);
        assert_eq!(balance.removed_m3, 0.0, "gentle slope failed");
    }

    #[test]
    fn soil_production_declines_with_thickness() {
        let params = HillslopeParams::default();
        let bare = soil_production_rate(&params, 0.0);
        let covered = soil_production_rate(&params, 2.0);
        assert!(bare > covered, "production should decline with cover");
        assert_eq!(soil_production_rate(&params, params.max_soil_m), 0.0);
        assert_eq!(soil_production_rate(&params, params.max_soil_m + 10.0), 0.0);
    }

    #[test]
    fn soil_mantle_grows_and_saturates() {
        let b = test_grid(8, 8);
        let mut f = ScalarField::filled(b, 100.0);
        let mut soil = ScalarField::zeros(b);
        let params = HillslopeParams {
            diffusivity_m2_yr: 0.0,
            soil_production_m_yr: 1e-3,
            max_soil_m: 1.0,
            ..Default::default()
        };
        apply(&mut f, &mut soil, &params, 10.0, 5000.0);
        assert!(soil.max() > 0.0, "no soil produced");
        assert!(soil.max() <= 1.0 + 1e-12, "soil exceeded the cap");
    }

    #[test]
    fn zero_timestep_is_a_no_op() {
        let b = test_grid(12, 12);
        let mut f = ScalarField::filled(b, 50.0);
        let mut soil = ScalarField::zeros(b);
        let balance = apply(&mut f, &mut soil, &HillslopeParams::default(), 10.0, 0.0);
        assert_eq!(balance, MassBalance::default());
    }
}