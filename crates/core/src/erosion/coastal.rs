//! Coastal erosion and deposition (spec §3f).
//!
//! Wave energy at a coast is estimated from the local wind and the open-water
//! **fetch**: for every coastal cell the module marches upwind over water and
//! measures how far the air travelled across the sea. A long fetch builds
//! larger waves, so exposed ocean-facing coasts retreat faster than sheltered
//! bays. The sea-level datum itself is advanced by the driver (climate cycles
//! and the eustatic trend), so this module only handles wave-driven sediment
//! movement; a rising sea is expressed as faster cliff retreat.
//!
//! Eroded cliff material is partitioned into:
//!
//! * **delta / beach** deposition in the shallow water just offshore, weighted
//!   by the local runoff (wet coastlines build deltas and beaches),
//! * **alongshore drift**, moving sediment parallel to the shore in the
//!   direction of the longshore current,
//! * **export** to the deep ocean for whatever finds no shallow target.
//!
//! All shares are tracked, so the mass ledger closes exactly.

use crate::climate::Climate;
use crate::erosion::MassBalance;
use crate::grid::D8;
use crate::math::clamp;
use crate::params::Params;
use crate::world::World;

/// Reference cliff retreat rate at full wave exposure, m/yr.
const REFERENCE_RETREAT_M_YR: f64 = 3.0e-4;
/// Maximum fetch march distance in cells (32 × 10 km ≈ 320 km).
const MAX_FETCH_CELLS: isize = 32;

/// Applies wave erosion, alongshore drift and shallow-water deposition.
pub fn apply(world: &mut World, params: &Params, climate: &Climate, dt_yr: f64) -> MassBalance {
    if dt_yr <= 0.0 || params.coastal.wave_erosivity <= 0.0 {
        return MassBalance::default();
    }
    let c = &params.coastal;
    let grid = world.grid;
    let planar_area = grid.cell_area_m2();
    let spherical_areas = grid.has_spherical_areas();
    let cell_km = grid.cell_size_m() / 1000.0;
    let sea = world.sea_level_m;
    let n = grid.cell_count();

    // Ocean mask.
    let ocean: Vec<bool> = world
        .elevation_m
        .as_slice()
        .iter()
        .map(|&z| z <= sea)
        .collect();
    // Active shore band: from the wave base up to just above the tide mark.
    let upper = sea + (c.tidal_range_m * 1.5).max(5.0) + 60.0;
    let lower = sea - c.wave_base_depth_m.max(1.0);

    let wind = climate.wind_vector();
    let up_dir = nearest_d8(-wind.x, -wind.y);
    // Alongshore directions: wind rotated ±90°.
    let drift_dir = nearest_d8(-wind.y, wind.x);
    let drift_dir_alt = nearest_d8(wind.y, -wind.x);
    let wind_energy = clamp((params.climate.wind_speed_ms / 8.0).powi(2), 0.15, 4.0);
    let fetch_ref = c.fetch_reference_km.max(1.0);
    // A rising sea accelerates retreat of low cliffs; a falling sea does not.
    let eustatic = 1.0 + clamp(c.sea_level_rate_mm_yr * 0.07, -0.5, 1.0);
    let precip_mean = climate.mean_land_precipitation().max(50.0);

    let mut erosion = vec![0.0f64; n];
    let mut deposition = vec![0.0f64; n];

    // ---- Wave erosion of the shore band ----------------------------------
    for y in 0..grid.height() {
        for x in 0..grid.width() {
            let i = grid.index(x, y);
            let z = world.elevation_m.get_index(i);
            if z <= sea || z > upper {
                continue;
            }
            // Coastal only when adjacent to the sea.
            let mut ocean_neighbour = false;
            for dir in 0..8 {
                if let Some((nx, ny)) = grid.neighbor(x, y, dir) {
                    if ocean[grid.index(nx, ny)] {
                        ocean_neighbour = true;
                        break;
                    }
                }
            }
            if !ocean_neighbour {
                continue;
            }
            // Open-water fetch upwind of the coast.
            let fetch_km = fetch_km(&ocean, grid, x, y, up_dir) * cell_km;
            let exposure = (0.08 + 0.92 * (fetch_km / fetch_ref).min(1.0).powf(0.6))
                * (1.0 + 0.05 * c.tidal_range_m);
            // Cliff retreat: softer rock retreats faster (a higher
            // `erodibility_scale` means a more resistant world).
            let resistance = params.geology.erodibility_scale.clamp(0.2, 5.0);
            let rate = REFERENCE_RETREAT_M_YR
                * c.wave_erosivity
                * exposure
                * wind_energy
                * eustatic
                * clamp((1.0 / resistance).sqrt(), 0.5, 2.0);
            let depth = (rate * dt_yr).min(6.0).min(z - sea);
            if depth > 0.0 {
                erosion[i] = depth;
            }
        }
    }

    // ---- Partition the eroded volume -------------------------------------
    let precip = climate.precipitation_mm.as_slice();
    for y in 0..grid.height() {
        for x in 0..grid.width() {
            let i = grid.index(x, y);
            let e = erosion[i];
            if e <= 0.0 {
                continue;
            }
            // Wet coastlines deliver more river sediment and build deltas.
            let runoff = clamp(f64::from(precip[i]) / precip_mean, 0.15, 3.0);
            let delta_share = clamp(c.delta_build_strength * runoff * 0.5, 0.0, 0.75);
            let drift_share = clamp(c.alongshore_transport * 0.5, 0.0, 0.45);
            let delta_share = delta_share.min(1.0 - drift_share);

            // Delta / beach: the shallowest offshore cell within two steps,
            // preferring downwind drift so deltas grow into the current.
            let mut delta_target = None;
            for step in 1..=2isize {
                let tx = x as isize + (wind.x * step as f64).round() as isize;
                let ty = y as isize + (wind.y * step as f64).round() as isize;
                if let Some(j) = cell_at(grid, tx, ty) {
                    let zj = world.elevation_m.get_index(j);
                    if zj <= sea && zj >= lower {
                        delta_target = Some(j);
                        break;
                    }
                }
            }
            if delta_target.is_none() {
                for dir in 0..8 {
                    if let Some((nx, ny)) = grid.neighbor(x, y, dir) {
                        let j = grid.index(nx, ny);
                        let zj = world.elevation_m.get_index(j);
                        if zj <= sea && zj >= lower {
                            delta_target = Some(j);
                            break;
                        }
                    }
                }
            }
            if let Some(j) = delta_target {
                deposition[j] += e * delta_share;
            }

            // Alongshore drift: move sediment parallel to the coast. Whatever
            // finds no shallow target is exported (residual below).
            for &dir in &[drift_dir, drift_dir_alt] {
                let (dx, dy) = D8[dir];
                if let Some(j) = cell_at(grid, x as isize + dx, y as isize + dy) {
                    let zj = world.elevation_m.get_index(j);
                    if zj <= sea && zj >= lower {
                        deposition[j] += e * drift_share;
                        break;
                    }
                }
            }
        }
    }

    // ---- Apply ------------------------------------------------------------
    let mut erosion_volume = 0.0f64;
    let mut deposition_volume = 0.0f64;
    for i in 0..n {
        let e = erosion[i];
        if e > 0.0 {
            world
                .elevation_m
                .set_index(i, world.elevation_m.get_index(i) - e);
            let cumulative = f64::from(world.erosion_m.get_index(i)) + e;
            world.erosion_m.set_index(i, cumulative as f32);
            erosion_volume += if spherical_areas {
                e * grid.cell_area_m2_at_index(i)
            } else {
                e
            };
        }
        let d = deposition[i];
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

    let mut balance = MassBalance::default();
    // Per-cell physical areas on spherical grids; historical arithmetic on
    // planar grids.
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
    // Export is the residual in volume units: exact closure.
    balance.export_m3 = balance.erosion_m3 - balance.deposition_m3;
    balance
}

/// Nearest D8 direction index to the vector `(vx, vy)`.
fn nearest_d8(vx: f64, vy: f64) -> usize {
    let mut best = 0usize;
    let mut best_dot = f64::NEG_INFINITY;
    for (k, &(dx, dy)) in D8.iter().enumerate() {
        let (fx, fy) = (dx as f64, dy as f64);
        let len = f64::hypot(fx, fy);
        let dot = (vx * fx + vy * fy) / len;
        if dot > best_dot {
            best_dot = dot;
            best = k;
        }
    }
    best
}

/// Cell index at possibly-wrapped/clamped coordinates, respecting the boundary.
fn cell_at(grid: crate::grid::Grid, x: isize, y: isize) -> Option<usize> {
    let ok_x = grid.boundary().wraps_x() || (0..grid.width() as isize).contains(&x);
    let ok_y = grid.boundary().wraps_y() || (0..grid.height() as isize).contains(&y);
    if !ok_x || !ok_y {
        return None;
    }
    Some(grid.index(grid.clamp_x(x), grid.clamp_y(y)))
}

/// Number of consecutive ocean cells one step upwind of `(x, y)`.
fn fetch_km(ocean: &[bool], grid: crate::grid::Grid, x: usize, y: usize, dir: usize) -> f64 {
    let (dx, dy) = D8[dir];
    let mut cells = 0isize;
    let (mut cx, mut cy) = (x as isize + dx, y as isize + dy);
    for _ in 0..MAX_FETCH_CELLS {
        let ok_x = grid.boundary().wraps_x() || (0..grid.width() as isize).contains(&cx);
        let ok_y = grid.boundary().wraps_y() || (0..grid.height() as isize).contains(&cy);
        if !ok_x || !ok_y {
            break;
        }
        let i = grid.index(grid.clamp_x(cx), grid.clamp_y(cy));
        if !ocean[i] {
            break;
        }
        cells += 1;
        cx += dx;
        cy += dy;
    }
    cells as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::climate::{compute, ClimateInputs};
    use crate::params::Params;
    use crate::world::World;

    fn setup() -> (World, Params, Climate) {
        let mut p = Params::default();
        p.world.width = 48;
        p.world.height = 32;
        p.world.cell_km = 5.0;
        p.coastal.wave_erosivity = 0.6;
        let mut world = World::new(&p.world).unwrap();
        // Ocean to the west, a coastline at x = 12, land to the east.
        world.elevation_m.map_in_place(|i, _| {
            let x = i % 48;
            if x < 12 {
                -20.0
            } else if x == 12 {
                50.0
            } else {
                400.0
            }
        });
        let climate = compute(&ClimateInputs {
            world: &world,
            params: &p,
            previous: None,
            dt_yr: 1000.0,
        });
        (world, p, climate)
    }

    #[test]
    fn coast_retreats_and_builds_beach() {
        let (mut world, p, climate) = setup();
        let before = world.elevation_m.clone();
        let balance = apply(&mut world, &p, &climate, 1e5);
        assert!(balance.erosion_m3 > 0.0);
        assert!(balance.deposition_m3 > 0.0);
        assert!(balance.is_conservative(1e-6), "{balance:?}");
        assert_eq!(world.elevation_m.non_finite_count(), 0);
        assert!(world.elevation_m.get(12, 5) < before.get(12, 5));
        // Some material ended up in the water just offshore.
        let mut raised = 0;
        for x in 0..12 {
            for y in 0..32 {
                if world.elevation_m.get(x, y) > before.get(x, y) + 1e-9 {
                    raised += 1;
                }
            }
        }
        assert!(raised > 0, "no beach/delta deposition");
    }

    #[test]
    fn inland_cells_are_untouched() {
        let (mut world, p, climate) = setup();
        let before = world.elevation_m.get(25, 20);
        apply(&mut world, &p, &climate, 1e5);
        assert!((world.elevation_m.get(25, 20) - before).abs() < 1e-9);
    }

    #[test]
    fn long_fetch_erodes_more_than_sheltered_coast() {
        // Two islands: one behind a wide ocean, one behind a narrow bay.
        let mut p = Params::default();
        p.world.width = 64;
        p.world.height = 32;
        p.world.cell_km = 5.0;
        p.coastal.wave_erosivity = 0.8;
        p.coastal.fetch_reference_km = 100.0;
        let mut world = World::new(&p.world).unwrap();
        world.elevation_m.map_in_place(|i, _| {
            let x = i % 64;
            // Wide ocean from x=0..32, then a long coast; a narrow bay at x=44
            // with land at 42..44 acting as a breakwater.
            if x < 32 || (44..46).contains(&x) {
                -30.0
            } else {
                40.0
            }
        });
        let climate = compute(&ClimateInputs {
            world: &world,
            params: &p,
            previous: None,
            dt_yr: 1000.0,
        });
        let exposed_before = world.elevation_m.get(32, 16);
        let sheltered_before = world.elevation_m.get(46, 16);
        // Keep both below the per-application retreat cap so the fetch
        // contrast is visible.
        apply(&mut world, &p, &climate, 2e4);
        let exposed = exposed_before - world.elevation_m.get(32, 16);
        let sheltered = sheltered_before - world.elevation_m.get(46, 16);
        assert!(
            exposed > sheltered,
            "exposed coast retreat {exposed} should exceed sheltered {sheltered}"
        );
    }

    #[test]
    fn rising_sea_increases_retreat() {
        let (mut world, mut p, climate) = setup();
        p.coastal.sea_level_rate_mm_yr = 8.0;
        let rising = apply(&mut world, &p, &climate, 1e5);
        let (mut world2, mut p2, climate2) = setup();
        p2.coastal.sea_level_rate_mm_yr = -8.0;
        let falling = apply(&mut world2, &p2, &climate2, 1e5);
        assert!(rising.erosion_m3 > falling.erosion_m3);
    }
}
