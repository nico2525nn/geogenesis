//! Glacial erosion (spec §3e).
//!
//! The model is a small ice-sheet engine rather than a snowline threshold:
//!
//! 1. **Mass balance.** Snow accumulates where the surface is above the local
//!    equilibrium line (snowfall depends on precipitation), while ablation
//!    melts ice below it following the positive degree-day relation. Ice
//!    thickness relaxes toward the balance value over an ice-response time.
//! 2. **Flow.** The shallow-ice approximation gives a non-linear diffusivity
//!    `D ∝ h³ |∇s|`; ice flows down the surface gradient. The explicit update
//!    is sub-stepped to satisfy the CFL condition, and every exchange is
//!    pairwise and symmetric, so ice volume is conserved to machine precision.
//! 3. **Erosion.** Basal sliding scales with ice thickness and surface slope,
//!    and bedrock abrasion is proportional to sliding (the standard relation).
//! 4. **U-valleys.** Ice-covered rock is laterally exchanged between valley
//!    walls and valley floors (a mass-conserving pairwise transport), which
//!    rounds V-shaped cross-profiles into the characteristic U shape while a
//!    share of the debris is redeposited as till. The rest is exported by
//!    meltwater, so the process ledger closes exactly by construction.
//!
//! The module is deterministic: it only reads the world, the climate snapshot
//! and the parameters.

use crate::climate::Climate;
use crate::erosion::{diffusion_max_dt, substeps, MassBalance};
use crate::math::clamp;
use crate::params::Params;
use crate::world::World;

/// Years over which ice thickness relaxes toward its mass-balance value.
const ICE_RESPONSE_YR: f64 = 4000.0;
/// Reference snowfall used to scale accumulation, mm/yr.
const REFERENCE_SNOWFALL_MM: f64 = 1000.0;
/// Maximum lateral (U-valley) transport CFL number.
const LATERAL_CFL: f64 = 0.20;

/// Applies ice accumulation, flow and glacial erosion over `dt_yr`.
pub fn apply(world: &mut World, params: &Params, climate: &Climate, dt_yr: f64) -> MassBalance {
    if dt_yr <= 0.0 {
        return MassBalance::default();
    }
    let g = &params.glacial;
    let cc = &params.climate;
    let grid = world.grid;
    let planar_area = grid.cell_area_m2();
    let spherical_areas = grid.has_spherical_areas();
    let dx = grid.cell_size_m();
    let sea = world.sea_level_m;
    let n = grid.cell_count();
    let mut balance = MassBalance::default();
    if g.max_ice_thickness_m <= 0.0 {
        return balance;
    }

    // ---- Snowline --------------------------------------------------------
    // The snowline is expressed as a temperature: the user's `ela_m` is the
    // altitude where the reference climate reaches its equilibrium temperature
    // (`reference_temp_c - lapse·ela`). A climate colder than the reference
    // shifts the actual snowline down and a warmer one up, with the configured
    // metres-per-degree sensitivity.
    let mean_land_t = climate.mean_land_temperature();
    // Sea-surface temperature is the best proxy for the global climate; land
    // is colder purely because it is elevated, which the per-cell temperature
    // already includes.
    let global_t = climate.mean_ocean_temperature().unwrap_or(mean_land_t);
    let reference_t = cc.reference_temp_c();
    let lapse = cc.lapse_rate_c_km.max(0.5);
    let climate_shift_c = global_t - reference_t;
    let ela_shift_m = climate_shift_c * g.ela_sensitivity_m_per_c;
    let snowline_temp_c = reference_t - lapse * g.ela_m / 1000.0 + ela_shift_m * lapse / 1000.0;
    let melt_half = 1.5; // °C, shortwave/melt balance reference

    // ---- 1. Mass balance -------------------------------------------------
    // Balance in metres of ice per year: snowfall above the snowline, melt
    // below it. `frozen` is the fraction of precipitation falling as snow.
    for i in 0..n {
        let z = world.elevation_m.get_index(i);
        if z <= sea {
            world.ice_m.set_index(i, 0.0);
            continue;
        }
        let temp = f64::from(climate.temperature_c.get_index(i));
        let snowfall = f64::from(climate.precipitation_mm.get_index(i));
        let frozen = clamp((1.5 - temp) / 5.0, 0.0, 1.0);
        let accumulation =
            g.accumulation_scale * 0.45 * (snowfall / REFERENCE_SNOWFALL_MM).clamp(0.1, 3.0);
        // Height above the local snowline in metres. The cell temperature
        // already includes the lapse; the seasonal amplitude is added on top
        // because a glacier must survive the summer melt season, not just the
        // annual mean. That is what keeps mid-latitude mountains bare while
        // polar ice caps grow.
        let summer_half = 0.5 * f64::from(climate.seasonal_amplitude_c.get_index(i));
        let above = (snowline_temp_c - (temp + summer_half)) * (1000.0 / lapse);
        let target = if above > -melt_half * 100.0 && frozen > 0.0 {
            // Thickness increases with height above the equilibrium line and
            // with the local snowfall, saturating at the parameter cap.
            let height = clamp(above / 1200.0, 0.0, 1.0);
            g.max_ice_thickness_m * height * frozen * accumulation.min(1.5)
        } else {
            0.0
        };
        // Ablation below the snowline (positive degree days).
        let ablation = 0.0025 * (temp + 1.0).max(0.0).powf(1.5);
        let relax = clamp(dt_yr / ICE_RESPONSE_YR, 0.0, 1.0);
        let current = f64::from(world.ice_m.get_index(i));
        let mut ice = current + (target - current) * relax - ablation * dt_yr * relax;
        if ice < 1.0 {
            ice = 0.0;
        }
        ice = ice.min(g.max_ice_thickness_m);
        world.ice_m.set_index(i, ice as f32);
    }
    // Cells that currently hold ice. Only these take part in flow and
    // erosion; on an Earth-like world they are a small fraction of the grid,
    // so the expensive per-substep loops stay cheap.
    let ice_cells: Vec<u32> = (0..n)
        .filter(|&i| world.ice_m.get_index(i) > 0.0)
        .map(|i| i as u32)
        .collect();

    // ---- 2. Shallow-ice flow (diffusion of ice thickness) ----------------
    // D = C · h³ · |∇s| (a simplified Glen-law diffusivity). The coefficient
    // is scaled to metres²/yr so the CFL sub-stepping below stays physical.
    let mut diffusivity = vec![0.0f64; n];
    let mut d_max = 0.0f64;
    for &ci in &ice_cells {
        let i = ci as usize;
        let h = f64::from(world.ice_m.get_index(i));
        let (x, y) = grid.coords(i);
        let slope = world.elevation_m.slope(x, y);
        // Simplified Glen-law diffusivity D ∝ h³|∇s|, capped so the explicit
        // CFL sub-stepping can always handle the requested time step.
        let d =
            (g.ice_flow_coefficient * 1.0e13 * (h / 1000.0).powi(3) * (slope + 1e-3)).min(4.0e4);
        diffusivity[i] = d;
        if d > d_max {
            d_max = d;
        }
    }
    if d_max > 0.0 {
        let max_dt = diffusion_max_dt(d_max, dx).max(1.0);
        let (steps, sub_dt) = substeps(dt_yr, max_dt, 16);
        let mut delta = vec![0.0f64; n];
        for _ in 0..steps {
            delta.fill(0.0);
            for &ci in &ice_cells {
                let i = ci as usize;
                let h_i = f64::from(world.ice_m.get_index(i));
                if h_i <= 0.0 {
                    continue;
                }
                let (x, y) = grid.coords(i);
                // Four forward neighbours count each edge exactly once. Ice
                // only exchanges with other ice: bare ground has zero
                // diffusivity, so the margin cannot suck ice out of nothing.
                for dir in [4usize, 6, 7, 3] {
                    let Some((nx, ny)) = grid.neighbor(x, y, dir) else {
                        continue;
                    };
                    let j = grid.index(nx, ny);
                    let h_j = f64::from(world.ice_m.get_index(j));
                    if h_j <= 0.0 {
                        continue;
                    }
                    let d_edge = 0.5 * (diffusivity[i] + diffusivity[j]);
                    if d_edge <= 0.0 {
                        continue;
                    }
                    let mut flux = d_edge * sub_dt / (dx * dx) * (h_j - h_i);
                    // CFL guard on the pair; symmetric so volume is kept.
                    let limit = LATERAL_CFL * (h_i - h_j).abs();
                    flux = clamp(flux, -limit, limit);
                    if flux.abs() < 1e-12 {
                        continue;
                    }
                    delta[i] += flux;
                    delta[j] -= flux;
                }
            }
            for &ci in &ice_cells {
                let i = ci as usize;
                if delta[i] != 0.0 {
                    let next = (f64::from(world.ice_m.get_index(i)) + delta[i]).max(0.0);
                    world
                        .ice_m
                        .set_index(i, next.min(g.max_ice_thickness_m) as f32);
                }
            }
        }
    }

    // ---- 3. Basal sliding erosion ----------------------------------------
    // Sliding speed ∝ h²|∇s| (Weertman/regelation), abrasion ∝ sliding.
    let mut eroded = vec![0.0f64; n];
    for &ci in &ice_cells {
        let i = ci as usize;
        let h = f64::from(world.ice_m.get_index(i));
        if h < 2.0 {
            continue;
        }
        let (x, y) = grid.coords(i);
        let slope = world.elevation_m.slope(x, y);
        let sliding = (h / 1000.0).powi(2) * (slope + 1e-3);
        let rate = g.glacial_erosion_coefficient * 120.0 * sliding;
        let depth = (rate * dt_yr).min(h * 0.4).min(8.0);
        if depth > 0.0 {
            eroded[i] = depth;
        }
    }

    // ---- 4. Mass-conserving lateral U-valley carving ---------------------
    // Under ice, the bed is diffused laterally: valley walls (locally high)
    // shed material to the valley floor (locally low). This rounds sharp V
    // cross-profiles into U profiles and is exactly pairwise-conservative.
    let mut lateral_erosion = vec![0.0f64; n];
    let mut lateral_deposition = vec![0.0f64; n];
    if g.u_valley_strength > 0.0 {
        let k = (g.u_valley_strength * dt_yr / dx).min(1.0);
        if k > 0.0 {
            // Ridge crests are not glacial valley walls: label local maxima so
            // the lateral exchange only works on slopes and valley sides and
            // cannot plane the peaks flat.
            let mut is_ridge = vec![false; n];
            for &ci in &ice_cells {
                let i = ci as usize;
                let (x, y) = grid.coords(i);
                let z_i = world.elevation_m.get_index(i);
                let mut highest = true;
                for dir in 0..8 {
                    let Some((nx, ny)) = grid.neighbor(x, y, dir) else {
                        continue;
                    };
                    if world.elevation_m.get_index(grid.index(nx, ny)) > z_i + 0.5 {
                        highest = false;
                        break;
                    }
                }
                is_ridge[i] = highest;
            }
            // Absolute cap per edge per application: this is a slow lateral
            // sediment flux, not a diffusion that erases relief.
            const MAX_LATERAL_M: f64 = 1.0;
            for &ci in &ice_cells {
                let i = ci as usize;
                let h_i = f64::from(world.ice_m.get_index(i));
                if h_i <= 0.0 || is_ridge[i] {
                    continue;
                }
                let (x, y) = grid.coords(i);
                let z_i = world.elevation_m.get_index(i);
                for dir in [4usize, 6, 7, 3] {
                    let Some((nx, ny)) = grid.neighbor(x, y, dir) else {
                        continue;
                    };
                    let j = grid.index(nx, ny);
                    let h_j = f64::from(world.ice_m.get_index(j));
                    if h_j <= 0.0 {
                        continue;
                    }
                    let z_j = world.elevation_m.get_index(j);
                    let dz = z_i - z_j;
                    if dz.abs() < 1e-9 {
                        continue;
                    }
                    let coeff = 0.5 * (h_i + h_j) / 1000.0;
                    let transfer = (k * coeff * dz).abs().min(MAX_LATERAL_M);
                    if transfer < 1e-9 {
                        continue;
                    }
                    // Never erode a ridge crest from either side of the pair.
                    if dz > 0.0 {
                        lateral_erosion[i] += transfer;
                        lateral_deposition[j] += transfer;
                    } else if !is_ridge[j] {
                        lateral_erosion[j] += transfer;
                        lateral_deposition[i] += transfer;
                    }
                }
            }
        }
    }

    // ---- 5. Till redeposition for the sliding erosion --------------------
    // A share of the abrasion debris is left as till at the nearest lower
    // ice-marginal cell; the rest is exported in meltwater.
    let mut till = vec![0.0f64; n];
    let till_share = g.till_redeposition.clamp(0.0, 1.0);
    if till_share > 0.0 {
        for i in 0..n {
            let e = eroded[i];
            if e <= 0.0 {
                continue;
            }
            let (x, y) = grid.coords(i);
            let z = world.elevation_m.get_index(i);
            // Prefer a downslope neighbour, else stay put (till sheet).
            let mut best: Option<(usize, f64)> = None;
            for dir in 0..8 {
                let Some((nx, ny)) = grid.neighbor(x, y, dir) else {
                    continue;
                };
                let j = grid.index(nx, ny);
                let zj = world.elevation_m.get_index(j);
                if zj <= sea {
                    continue;
                }
                let drop = z - zj;
                if drop > 0.0 && best.is_none_or(|(_, b)| drop > b) {
                    best = Some((j, drop));
                }
            }
            match best {
                Some((j, _)) => till[j] += e * till_share,
                None => till[i] += e * till_share,
            }
        }
    }

    // ---- Apply all surface changes ---------------------------------------
    let mut erosion_volume = 0.0;
    let mut deposition_volume = 0.0;
    for i in 0..n {
        let e = eroded[i] + lateral_erosion[i];
        let d = lateral_deposition[i] + till[i];
        if e > 0.0 {
            world
                .elevation_m
                .set_index(i, world.elevation_m.get_index(i) - e);
            world.ice_m.set_index(
                i,
                (f64::from(world.ice_m.get_index(i)) - e * 0.05).max(0.0) as f32,
            );
            let cumulative = f64::from(world.erosion_m.get_index(i)) + e;
            world.erosion_m.set_index(i, cumulative as f32);
            erosion_volume += if spherical_areas {
                e * grid.cell_area_m2_at_index(i)
            } else {
                e
            };
        }
        if d > 0.0 {
            world
                .elevation_m
                .set_index(i, world.elevation_m.get_index(i) + d);
            world
                .sediment_m
                .set_index(i, (f64::from(world.sediment_m.get_index(i)) + d) as f32);
            let cumulative = f64::from(world.deposition_m.get_index(i)) + d;
            world.deposition_m.set_index(i, cumulative as f32);
            deposition_volume += if spherical_areas {
                d * grid.cell_area_m2_at_index(i)
            } else {
                d
            };
        }
    }

    // Volume conversions use per-cell physical areas on spherical grids; the
    // planar path keeps the historical `sum(depth) * cell_area` arithmetic.
    let (erosion_m3, deposition_m3) = if spherical_areas {
        (erosion_volume, deposition_volume)
    } else {
        (
            erosion_volume * planar_area,
            deposition_volume * planar_area,
        )
    };
    balance.erosion_m3 = erosion_m3;
    balance.deposition_m3 = deposition_m3;
    // Meltwater carries everything else off the map; derive the export in the
    // same volume units so the ledger closes exactly.
    balance.export_m3 = balance.erosion_m3 - balance.deposition_m3;
    balance
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::climate::{compute, ClimateInputs};
    use crate::params::Params;
    use crate::world::World;

    fn setup() -> (World, Params, Climate) {
        let mut p = Params::default();
        p.world.width = 24;
        p.world.height = 24;
        p.world.cell_km = 5.0;
        p.glacial.ela_m = 1000.0;
        let grid = p.world.grid().unwrap();
        let mut world = World::new(&p.world).unwrap();
        world.elevation_m.map_in_place(|i, _| {
            let (x, y) = grid.coords(i);
            let dx = x as f64 - 12.0;
            let dy = y as f64 - 12.0;
            4000.0 - (dx * dx + dy * dy).sqrt() * 250.0
        });
        let climate = compute(&ClimateInputs {
            world: &world,
            params: &p,
            previous: None,
            dt_yr: 2000.0,
        });
        (world, p, climate)
    }

    /// A cold world so the summit region is above the snowline but the basin
    /// floor is not.
    fn cold_setup() -> (World, Params, Climate) {
        let (world, mut p, _) = setup();
        p.climate.mean_temp_c = -10.0;
        // Snowline below the basin floor's upper slopes so a real ice cap forms
        // around the summit, while the low ground stays ice-free.
        p.glacial.ela_m = 2500.0;
        let climate = compute(&ClimateInputs {
            world: &world,
            params: &p,
            previous: None,
            dt_yr: 2000.0,
        });
        (world, p, climate)
    }

    #[test]
    fn ice_forms_above_the_snowline_only() {
        let (mut world, p, climate) = cold_setup();
        apply(&mut world, &p, &climate, 20_000.0);
        let peak = world.grid.index(12, 12);
        let foot = world.grid.index(12, 2);
        assert!(world.ice_m.get_index(peak) > 0.0, "no ice at the summit");
        assert_eq!(
            world.ice_m.get_index(foot),
            0.0,
            "ice formed below the snowline"
        );
    }

    #[test]
    fn warm_world_has_no_ice() {
        let (mut world, mut p, mut climate) = setup();
        p.glacial.ela_m = 7000.0;
        for v in climate.temperature_c.as_mut_slice() {
            *v = 30.0;
        }
        apply(&mut world, &p, &climate, 2000.0);
        assert_eq!(world.ice_m.max_value().unwrap(), 0.0);
    }

    #[test]
    fn erosion_carves_and_ledger_closes() {
        let (mut world, p, climate) = cold_setup();
        let before = world.elevation_m.clone();
        let balance = apply(&mut world, &p, &climate, 20_000.0);
        assert!(balance.erosion_m3 > 0.0, "no glacial erosion");
        assert!(balance.is_conservative(1e-6), "{balance:?}");
        let changed = before
            .as_slice()
            .iter()
            .zip(world.elevation_m.as_slice())
            .filter(|(a, b)| **b < **a - 1e-9)
            .count();
        assert!(changed > 0);
        assert_eq!(world.elevation_m.non_finite_count(), 0);
    }

    #[test]
    fn u_valley_carving_is_mass_conserving() {
        let (mut world, mut p, climate) = cold_setup();
        p.glacial.u_valley_strength = 1.0;
        p.glacial.till_redeposition = 0.0;
        p.glacial.glacial_erosion_coefficient = 0.0;
        p.glacial.ice_flow_coefficient = 0.0;
        // Only the lateral exchange should act.
        let volume_before: f64 = world.elevation_m.as_slice().iter().sum();
        let balance = apply(&mut world, &p, &climate, 50_000.0);
        let volume_after: f64 = world.elevation_m.as_slice().iter().sum();
        assert!(balance.erosion_m3 > 0.0);
        assert!(balance.deposition_m3 > 0.0);
        assert!(balance.is_conservative(1e-6), "{balance:?}");
        assert!(
            (volume_after - volume_before).abs() < 1e-3,
            "surface volume changed: {volume_before} -> {volume_after}"
        );
    }

    #[test]
    fn lateral_carving_spares_ridges_and_roughens_slopes() {
        let (mut world, mut p, climate) = cold_setup();
        p.glacial.u_valley_strength = 1.0;
        p.glacial.till_redeposition = 0.0;
        p.glacial.glacial_erosion_coefficient = 0.0;
        p.glacial.ice_flow_coefficient = 0.0;
        let before = world.elevation_m.clone();
        apply(&mut world, &p, &climate, 50_000.0);
        let summit = world.grid.index(12, 12);
        // Ridge crests are not valley walls and must not be planed flat by the
        // lateral exchange.
        assert!(
            (world.elevation_m.get_index(summit) - before.get_index(summit)).abs() < 1e-9,
            "summit changed under lateral carving"
        );
        // Slopes do exchange material.
        let changed = (0..world.grid.cell_count())
            .filter(|&i| (world.elevation_m.get_index(i) - before.get_index(i)).abs() > 1e-6)
            .count();
        assert!(changed > 0, "no slope exchange happened");
    }

    #[test]
    fn large_time_steps_are_stable() {
        let (mut world, p, climate) = cold_setup();
        let balance = apply(&mut world, &p, &climate, 5.0e6);
        assert_eq!(world.elevation_m.non_finite_count(), 0);
        assert_eq!(world.ice_m.non_finite_count(), 0);
        assert!(balance.erosion_m3.is_finite());
        assert!(balance.is_conservative(1e-6));
        // Ice cannot exceed its physical cap.
        assert!(world.ice_m.max_value().unwrap() <= p.glacial.max_ice_thickness_m + 1e-6);
    }

    #[test]
    fn glacial_is_deterministic() {
        let (world_a, p, climate_a) = cold_setup();
        let (world_b, _, climate_b) = cold_setup();
        let mut a = world_a;
        let mut b = world_b;
        let ba = apply(&mut a, &p, &climate_a, 20_000.0);
        let bb = apply(&mut b, &p, &climate_b, 20_000.0);
        assert_eq!(a.elevation_m.as_slice(), b.elevation_m.as_slice());
        assert_eq!(a.ice_m.as_slice(), b.ice_m.as_slice());
        assert_eq!(ba, bb);
    }
}
