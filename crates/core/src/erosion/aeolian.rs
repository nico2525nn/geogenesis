//! Wind erosion, dune building and dust export (spec §3d).
//!
//! The wind erodes loose sediment where the surface shear velocity exceeds the
//! threshold shear velocity: `u* = u₁₀·√C_d`. The threshold scales the
//! user-facing 10 m wind speed the same way, so `threshold_wind_ms` stays an
//! intuitive control while the physics runs on shear velocity.
//!
//! Transport is split into three streams:
//!
//! * **saltation** (bedload) — bounces one or two cells downwind and is
//!   deposited where the surface decelerates, building dunes and lee sheets;
//! * **loess** — fine bedload carried further downwind and deposited on quiet
//!   ground;
//! * **suspension** (dust) — leaves the map, the aeolian contribution to the
//!   global sediment budget.
//!
//! The split is set by the saltation and dust-emission parameters. Every cell's
//! removed volume is tracked, so the ledger closes exactly:
//! `erosion = deposition + export`.

use crate::climate::Climate;
use crate::erosion::MassBalance;
use crate::grid::D8;
use crate::math::clamp;
use crate::params::Params;
use crate::world::World;

/// Reference aeolian deflation rate in m/yr at full transport capacity.
const REFERENCE_DEFLATION_M_YR: f64 = 6.0e-5;
/// Maximum loose material a single application may remove, m.
const MAX_DEFLATION_M: f64 = 2.0;

/// Applies wind erosion, downwind dune deposition and dust export.
pub fn apply(world: &mut World, params: &Params, climate: &Climate, dt_yr: f64) -> MassBalance {
    if dt_yr <= 0.0 {
        return MassBalance::default();
    }
    let a = &params.aeolian;
    let cc = &params.climate;
    if a.erosion_strength <= 0.0 || cc.wind_speed_ms <= 0.0 {
        return MassBalance::default();
    }
    // Shear-velocity threshold: u* = u·√Cd.
    let drag = a.drag_coefficient.clamp(1e-6, 0.2).sqrt();
    let u_star = cc.wind_speed_ms * drag;
    let u_star_threshold = a.threshold_wind_ms.max(0.05) * drag;
    let excess = ((u_star - u_star_threshold) / u_star_threshold).max(0.0);
    if excess <= 0.0 {
        return MassBalance::default();
    }

    let grid = world.grid;
    let planar_area = grid.cell_area_m2();
    let spherical_areas = grid.has_spherical_areas();
    let sea = world.sea_level_m;
    let wind = cc.wind_vector();
    // Nearest D8 direction to the downwind vector (direction indices are not
    // ordered by angle, so compare dot products).
    let mut dir_index = 4usize;
    let mut best_dot = f64::NEG_INFINITY;
    for (k, &(ddx, ddy)) in D8.iter().enumerate() {
        let len = f64::hypot(ddx as f64, ddy as f64);
        let dot = (wind.x * ddx as f64 + wind.y * ddy as f64) / len;
        if dot > best_dot {
            best_dot = dot;
            dir_index = k;
        }
    }
    let (dx, dy) = D8[dir_index];

    // Transport split. Bedload bounces, suspension leaves; the remainder is
    // deposited as loess further downwind.
    let saltation = clamp(a.saltation_coefficient, 0.0, 1.0);
    let dust = clamp(a.dust_emission, 0.0, 1.0);
    let suspended = 1.0 - saltation;
    let loess_share = suspended * (1.0 - dust);
    // The dust share (`suspended * dust`) also leaves the domain; it is
    // accounted for by the residual export.
    // Shear excess with a soft onset; transport grows faster than erosion.
    let transport = a.erosion_strength * excess.powf(1.5);

    let mut erosion = vec![0.0f64; grid.cell_count()];
    let mut deposition = vec![0.0f64; grid.cell_count()];

    // ---- Deflation -------------------------------------------------------
    for y in 0..grid.height() {
        for x in 0..grid.width() {
            let i = grid.index(x, y);
            let z = world.elevation_m.get_index(i);
            if z <= sea {
                continue;
            }
            let veg = f64::from(climate.vegetation.get_index(i)).clamp(0.0, 1.0);
            let shield = (1.0 - a.vegetation_shielding * veg).max(0.0);
            // Dry, bare, sand-rich ground deflates fastest.
            let aridity = f64::from(climate.aridity.get_index(i));
            let dry = clamp((aridity - 0.6) / 2.5, 0.05, 1.0);
            let sediment = f64::from(world.sediment_m.get_index(i));
            let soil = f64::from(world.soil_m.get_index(i));
            let loose = sediment + 0.5 * soil;
            if loose < 0.02 {
                continue; // bedrock with no loose cover does not deflate
            }
            let supply = clamp(loose / 0.4, 0.0, 1.0);
            let rate = REFERENCE_DEFLATION_M_YR * transport * shield * dry * supply;
            let depth = (rate * dt_yr).min(loose * 0.6).min(MAX_DEFLATION_M);
            if depth > 1e-12 {
                erosion[i] = depth;
            }
        }
    }

    // ---- Downwind transport and deposition -------------------------------
    // Walk the transport direction one or two cells and deposit where the
    // surface decelerates (rise, shelter or a dune crest). Water and the open
    // edge export the material.
    let mut loess = vec![0.0f64; grid.cell_count()];
    for y in 0..grid.height() {
        for x in 0..grid.width() {
            let i = grid.index(x, y);
            let e = erosion[i];
            if e <= 0.0 {
                continue;
            }
            let z_src = world.elevation_m.get_index(i);
            // Saltation: prefer the first downwind cell where sand is trapped.
            let mut bedload_target: Option<usize> = None;
            let mut last_land: Option<usize> = None;
            for step_cells in 1..=2isize {
                let tx = x as isize + dx * step_cells;
                let ty = y as isize + dy * step_cells;
                let ok = (grid.boundary().wraps_x() || (0..grid.width() as isize).contains(&tx))
                    && (grid.boundary().wraps_y() || (0..grid.height() as isize).contains(&ty));
                if !ok {
                    break;
                }
                let j = grid.index(grid.clamp_x(tx), grid.clamp_y(ty));
                if world.elevation_m.get_index(j) <= sea {
                    break; // blown into the sea: exported
                }
                last_land = Some(j);
                let z = world.elevation_m.get_index(j);
                if a.dune_formation && z >= z_src - 1.0 {
                    bedload_target = Some(j);
                    break;
                }
                if !a.dune_formation {
                    bedload_target = Some(j);
                    break;
                }
            }
            let bedload_target = bedload_target.or(last_land);
            match bedload_target {
                Some(j) => {
                    deposition[j] += e * saltation;
                }
                None => {}
            }
            // Loess is carried further and settles on the second/third cell;
            // with no land there it leaves the domain like the dust.
            for step_cells in 2..=3isize {
                let tx = x as isize + dx * step_cells;
                let ty = y as isize + dy * step_cells;
                let ok = (grid.boundary().wraps_x() || (0..grid.width() as isize).contains(&tx))
                    && (grid.boundary().wraps_y() || (0..grid.height() as isize).contains(&ty));
                if !ok {
                    break;
                }
                let j = grid.index(grid.clamp_x(tx), grid.clamp_y(ty));
                if world.elevation_m.get_index(j) <= sea {
                    break;
                }
                loess[j] += e * loess_share;
                break;
            }
        }
    }
    for i in 0..grid.cell_count() {
        deposition[i] += loess[i];
    }

    // ---- Apply ------------------------------------------------------------
    let mut erosion_volume = 0.0f64;
    let mut deposition_volume = 0.0f64;
    for i in 0..grid.cell_count() {
        let e = erosion[i];
        if e > 0.0 {
            world
                .elevation_m
                .set_index(i, world.elevation_m.get_index(i) - e);
            // Loose cover is stripped before bedrock.
            let sediment = f64::from(world.sediment_m.get_index(i));
            let taken = e.min(sediment);
            world
                .sediment_m
                .set_index(i, (sediment - taken).max(0.0) as f32);
            let remaining = e - taken;
            if remaining > 0.0 {
                let soil = f64::from(world.soil_m.get_index(i));
                world
                    .soil_m
                    .set_index(i, (soil - remaining * 2.0).max(0.0) as f32);
            }
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
    // Export is the residual in volume units, so the ledger closes exactly.
    balance.export_m3 = balance.erosion_m3 - balance.deposition_m3;
    balance
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::climate::{compute, ClimateInputs};
    use crate::params::Params;
    use crate::world::World;

    fn setup(wind_speed: f64) -> (World, Params, Climate) {
        let mut p = Params::default();
        p.world.width = 32;
        p.world.height = 32;
        p.world.cell_km = 5.0;
        p.climate.wind_speed_ms = wind_speed;
        p.aeolian.erosion_strength = 0.6;
        p.aeolian.vegetation_shielding = 0.3;
        let mut world = World::new(&p.world).unwrap();
        world.elevation_m.fill_value(200.0);
        world.soil_m.fill_value(0.5);
        world.sediment_m.fill_value(0.5);
        let climate = compute(&ClimateInputs {
            world: &world,
            params: &p,
            previous: None,
            dt_yr: 1000.0,
        });
        (world, p, climate)
    }

    #[test]
    fn wind_moves_material_downwind_and_conserves_mass() {
        let (mut world, p, climate) = setup(14.0);
        // Loose sand only in a western stripe so transport has a clear source.
        world.sediment_m.fill_value(0.0);
        world.soil_m.fill_value(0.0);
        for y in 0..world.grid.height() {
            for x in 0..4 {
                world.sediment_m.set(x, y, 1.0);
            }
        }
        let before = world.elevation_m.clone();
        let balance = apply(&mut world, &p, &climate, 10_000.0);
        assert!(balance.erosion_m3 > 0.0);
        assert!(balance.deposition_m3 > 0.0);
        assert!(balance.is_conservative(1e-6), "{balance:?}");
        assert_eq!(world.elevation_m.non_finite_count(), 0);
        // Net transport is downwind (wind from the west): the centroid of the
        // deposited material lies east of the centroid of the eroded material.
        let (mut ero_x, mut ero_w, mut dep_x, mut dep_w) = (0.0, 0.0, 0.0, 0.0);
        for y in 0..world.grid.height() {
            for x in 0..world.grid.width() {
                let dz = world.elevation_m.get(x, y) - before.get(x, y);
                if dz < 0.0 {
                    ero_x += -dz * x as f64;
                    ero_w += -dz;
                } else if dz > 0.0 {
                    dep_x += dz * x as f64;
                    dep_w += dz;
                }
            }
        }
        assert!(ero_w > 0.0 && dep_w > 0.0);
        assert!(
            dep_x / dep_w > ero_x / ero_w,
            "deposition centroid {:.2} is not downwind of erosion centroid {:.2}",
            dep_x / dep_w,
            ero_x / ero_w
        );
    }

    #[test]
    fn calm_wind_does_nothing() {
        let (mut world, p, mut climate) = setup(2.0);
        climate.temperature_c.fill_value(10.0);
        let before = world.elevation_m.clone();
        let balance = apply(&mut world, &p, &climate, 1000.0);
        assert_eq!(balance.erosion_m3, 0.0);
        assert_eq!(before.as_slice(), world.elevation_m.as_slice());
    }

    #[test]
    fn below_threshold_wind_does_nothing() {
        let (mut world, p, climate) = setup(5.0);
        let balance = apply(&mut world, &p, &climate, 10_000.0);
        assert_eq!(balance.erosion_m3, 0.0);
        assert_eq!(balance.deposition_m3, 0.0);
    }

    #[test]
    fn dust_export_grows_with_dust_emission() {
        let (mut world, mut p, climate) = setup(14.0);
        p.aeolian.dust_emission = 0.0;
        let sand_only = apply(&mut world, &p, &climate, 10_000.0);
        let (mut world2, mut p2, climate2) = setup(14.0);
        p2.aeolian.dust_emission = 1.0;
        let dusty = apply(&mut world2, &p2, &climate2, 10_000.0);
        assert!(dusty.export_m3 > sand_only.export_m3);
        assert!(dusty.is_conservative(1e-6));
    }

    #[test]
    fn aeolian_is_deterministic() {
        let (mut a, p, ca) = setup(14.0);
        let (mut b, _, cb) = setup(14.0);
        let ba = apply(&mut a, &p, &ca, 10_000.0);
        let bb = apply(&mut b, &p, &cb, 10_000.0);
        assert_eq!(a.elevation_m.as_slice(), b.elevation_m.as_slice());
        assert_eq!(ba, bb);
    }
}
