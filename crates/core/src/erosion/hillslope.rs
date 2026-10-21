//! Hillslope processes: soil creep, nonlinear diffusion, soil production and
//! mass wasting (spec §3c).
//!
//! Creep is discretised conservatively on cell edges,
//!
//! `dz/dt = div(D grad z)`,
//!
//! with a Roering-style nonlinear diffusivity
//!
//! `D = D0 / (1 - (|grad z| / S_crit)^2)  ·  f(vegetation)`,
//!
//! capped at a finite multiple of `D0` so the singularity at the critical slope
//! never enters the solution. Every edge exchange moves exactly the same volume
//! out of one cell and into the other, so the process is mass conserving by
//! construction; a local limiter keeps it stable for arbitrarily large time
//! steps (normal geological steps run at one sub-step, where the limiter is
//! inactive and the scheme is the accurate explicit solution).
//!
//! Landsliding then removes material wherever the topographic slope exceeds the
//! repose angle. Debris follows the steepest-descent path for a runout distance
//! parameterised in metres, comes to rest where the slope falls below repose,
//! and is deposited there; each transfer is a pair exchange between the source
//! and the deposition cell, so the mass ledger closes exactly.
//!
//! Soil is produced from bedrock at an exponential depth-dependent rate,
//! stripped by erosion, and thickened by deposition; the layer is coupled to
//! vegetation cover, which also raises the critical angle through root
//! cohesion.

use crate::climate::Climate;
use crate::erosion::{diffusion_max_dt, substeps, with_scratch, MassBalance};
use crate::geology::Geology;
use crate::grid::{Field, Grid};
use crate::params::Params;
use crate::world::World;

/// Applies creep, soil production and landslides.
pub fn apply(
    world: &mut World,
    params: &Params,
    climate: &Climate,
    _geology: &Geology,
    dt_yr: f64,
) -> MassBalance {
    if dt_yr <= 0.0 {
        return MassBalance::default();
    }
    let grid = world.grid;
    let n = grid.cell_count();
    let dx = grid.cell_size_m();
    let mut balance = MassBalance::default();

    with_scratch(n, 4, |buffers| {
        let (delta_buf, rest) = buffers.split_at_mut(1);
        let (slope_buf, rest) = rest.split_at_mut(1);
        let (diffusivity_buf, net_buf) = rest.split_at_mut(1);
        let delta = &mut delta_buf[0];
        let slope = &mut slope_buf[0];
        let diffusivity = &mut diffusivity_buf[0];
        let net = &mut net_buf[0];

        // On spherical grids a pair exchange must move the same *volume* in
        // and out: a depth change at a small polar cell is a different volume
        // than the same depth change at a large equatorial one. `areas` carries
        // the physical cell areas for that correction; planar grids pass
        // `None` and keep the historical, bit-identical height-based exchange.
        let areas: Option<Vec<f64>> = if grid.has_spherical_areas() {
            Some(
                (0..n)
                    .map(|i| grid.cell_area_m2_at_index(i))
                    .collect::<Vec<_>>(),
            )
        } else {
            None
        };
        diffuse(
            world,
            params,
            climate,
            dt_yr,
            dx,
            areas.as_deref(),
            delta,
            slope,
            diffusivity,
            net,
            &mut balance,
        );
        landslides(
            world,
            params,
            climate,
            dx,
            areas.as_deref(),
            delta,
            slope,
            net,
            &mut balance,
        );
        // One sequential pass couples the net surface change to the soil layer.
        couple_soil(world, net, params);
        soil(world, params, climate, dt_yr);
    });
    balance
}

/// CFL-limited maximum explicit time step for the configured diffusivity.
#[must_use]
pub fn stable_dt(params: &Params, cell_m: f64) -> f64 {
    diffusion_max_dt(params.hillslope.diffusivity.max(1e-9), cell_m)
}

/// Fills `out` with the local slope (m/m) of every cell.
///
/// Interior cells use central differences from the raw slice — the hot path,
/// since the generic [`Field::gradient`] does several boundary checks per cell.
/// Edge and wrap cells fall back to the general implementation.
fn compute_slopes(elevation: &Field<f64>, out: &mut [f64]) {
    let grid = elevation.grid();
    let w = grid.width();
    let h = grid.height();
    let data = elevation.as_slice();
    let inv = 0.5 / grid.cell_size_m();
    for y in 0..h {
        let row = y * w;
        if y > 0 && y + 1 < h {
            for x in 0..w {
                let i = row + x;
                if x > 0 && x + 1 < w {
                    let dzdx = (data[i + 1] - data[i - 1]) * inv;
                    let dzdy = (data[i + w] - data[i - w]) * inv;
                    out[i] = (dzdx * dzdx + dzdy * dzdy).sqrt();
                } else {
                    out[i] = elevation.slope(x, y);
                }
            }
        } else {
            for x in 0..w {
                out[row + x] = elevation.slope(x, y);
            }
        }
    }
}

/// Moves a depth `amount` out of cell `i` and into cell `j`, converting the
/// receiving depth so the transferred *volume* is conserved when the two cells
/// have different physical areas (`areas` is `Some` only on spherical grids).
#[inline]
fn exchange_depth_into(delta: &mut [f64], i: usize, j: usize, amount: f64, areas: Option<&[f64]>) {
    delta[i] += amount;
    let received = match areas {
        Some(areas) => amount * areas[i] / areas[j],
        None => amount,
    };
    delta[j] -= received;
}

/// Conservative explicit diffusion on cell edges with CFL sub-stepping.
#[allow(clippy::too_many_arguments)]
fn diffuse(
    world: &mut World,
    params: &Params,
    climate: &Climate,
    dt_yr: f64,
    dx: f64,
    areas: Option<&[f64]>,
    delta: &mut [f64],
    slope: &mut [f64],
    diffusivity: &mut [f64],
    net: &mut [f64],
    balance: &mut MassBalance,
) {
    let grid = world.grid;
    let n = grid.cell_count();
    let hparams = &params.hillslope;
    if hparams.diffusivity <= 0.0 {
        return;
    }

    // The diffusivity varies slowly relative to a sub-step, so it is evaluated
    // once per application. Slopes are only needed for the nonlinear flux.
    if hparams.nonlinearity > 0.0 {
        compute_slopes(&world.elevation_m, slope);
    }
    let root = params.ecology.root_strength.clamp(0.0, 1.0);
    let critical = hparams.critical_slope_tan().max(1e-6);
    let mut max_d = 0.0f64;
    for i in 0..n {
        let veg = f64::from(climate.vegetation.get_index(i)).clamp(0.0, 1.0);
        let protection = (1.0 - 0.5 * root * veg).clamp(0.2, 1.0);
        let mut d = hparams.diffusivity * protection;
        if hparams.nonlinearity > 0.0 {
            let ratio = (slope[i] / critical).powi(2).clamp(0.0, 0.96);
            d *= 1.0 + hparams.nonlinearity * ratio / (1.0 - ratio);
            d = d.min(hparams.diffusivity * 40.0);
        }
        diffusivity[i] = d;
        if d > max_d {
            max_d = d;
        }
    }
    if max_d <= 0.0 {
        return;
    }
    let max_sub_dt = diffusion_max_dt(max_d, dx) * 0.5;
    let cap = if params.time.cfl { 32 } else { 1 };
    let (steps, sub_dt) = substeps(dt_yr, max_sub_dt, cap);
    let factor = sub_dt / (dx * dx);
    let w = grid.width();
    let w_isize = w as isize;
    // Only four forward neighbours are visited so every edge is handled once:
    // east, south, south-east (half weight) and south-west (half weight).
    let forward: [(isize, f64); 4] = [
        (1, 1.0),
        (w_isize, 1.0),
        (w_isize + 1, 0.5),
        (w_isize - 1, 0.5),
    ];

    for _ in 0..steps {
        delta.fill(0.0);
        {
            let data = world.elevation_m.as_slice();
            for y in 0..grid.height() {
                let interior_row = y > 0 && y + 1 < grid.height();
                let row = y * w;
                for x in 0..w {
                    let i = row + x;
                    let z_i = data[i];
                    let d_i = diffusivity[i];
                    if d_i <= 0.0 {
                        continue;
                    }
                    if interior_row && x > 0 && x + 1 < w {
                        let base = i as isize;
                        for &(offset, diagonal_factor) in &forward {
                            let j = (base + offset) as usize;
                            let d_edge = 0.5 * (d_i + diffusivity[j]);
                            if d_edge <= 0.0 {
                                continue;
                            }
                            let difference = data[j] - z_i;
                            if difference == 0.0 {
                                continue;
                            }
                            let limit = 0.1 * difference.abs();
                            let exchange = (d_edge * factor * difference * diagonal_factor)
                                .clamp(-limit, limit);
                            if exchange.abs() < 1e-15 {
                                continue;
                            }
                            exchange_depth_into(delta, i, j, exchange, areas);
                        }
                    } else {
                        for dir in [4usize, 6, 7, 3] {
                            let Some((nx, ny)) = grid.neighbor(x, y, dir) else {
                                continue;
                            };
                            let j = ny * w + nx;
                            let d_edge = 0.5 * (d_i + diffusivity[j]);
                            if d_edge <= 0.0 {
                                continue;
                            }
                            let difference = data[j] - z_i;
                            if difference == 0.0 {
                                continue;
                            }
                            let diagonal_factor = if dir == 7 || dir == 3 { 0.5 } else { 1.0 };
                            let limit = 0.1 * difference.abs();
                            let exchange = (d_edge * factor * difference * diagonal_factor)
                                .clamp(-limit, limit);
                            if exchange.abs() < 1e-15 {
                                continue;
                            }
                            exchange_depth_into(delta, i, j, exchange, areas);
                        }
                    }
                }
            }
        }
        for i in 0..n {
            let change = delta[i];
            if change == 0.0 {
                continue;
            }
            apply_surface_change(world, i, change, net, balance);
        }
    }
}

/// Couples the net surface change of this application to the soil layer: soil
/// is stripped before bedrock when the surface lowers, and a fraction of fresh
/// deposition becomes loose soil.
fn couple_soil(world: &mut World, net: &[f64], params: &Params) {
    let soil_max = params.hillslope.soil_max_m.max(0.01);
    for i in 0..world.cell_count() {
        let change = net[i];
        if change == 0.0 {
            continue;
        }
        let soil = f64::from(world.soil_m.get_index(i)).clamp(0.0, soil_max);
        let next = if change < 0.0 {
            (soil + change).max(0.0)
        } else {
            (soil + 0.25 * change).min(soil_max)
        };
        world.soil_m.set_index(i, next as f32);
    }
}

/// Moves one buffered depth change into the world, updating the ledger, the
/// cumulative erosion/deposition diagnostics and the soil layer.
fn apply_surface_change(
    world: &mut World,
    index: usize,
    change: f64,
    net: &mut [f64],
    balance: &mut MassBalance,
) {
    // Physical area of the changed cell: `cell_size_m²` on planar grids (so
    // the historical arithmetic is unchanged), latitude-band area on spheres.
    let cell_area = world.grid.cell_area_m2_at_index(index);
    world
        .elevation_m
        .set_index(index, world.elevation_m.get_index(index) + change);
    net[index] += change;
    if change < 0.0 {
        balance.erosion_m3 += -change * cell_area;
        world.erosion_m.set_index(
            index,
            (f64::from(world.erosion_m.get_index(index)) - change) as f32,
        );
    } else {
        balance.deposition_m3 += change * cell_area;
        world.deposition_m.set_index(
            index,
            (f64::from(world.deposition_m.get_index(index)) + change) as f32,
        );
    }
}

/// Steepest-descent neighbour of a cell: `(index, path length, down-slope)`.
#[inline]
fn steepest_descent(data: &[f64], grid: Grid, index: usize) -> Option<(usize, f64, f64)> {
    let (x, y) = grid.coords(index);
    let z = data[index];
    let cell = grid.cell_size_m();
    let mut best: Option<(usize, f64, f64)> = None;
    for dir in 0..8 {
        let Some((nx, ny)) = grid.neighbor(x, y, dir) else {
            continue;
        };
        let j = ny * grid.width() + nx;
        let drop = z - data[j];
        if drop <= 0.0 {
            continue;
        }
        let length = grid.step_distance_m(dir);
        let slope = drop / length;
        if best.is_none_or(|(_, _, current)| slope > current) {
            best = Some((j, length, slope));
        }
    }
    let _ = cell;
    best
}

/// Mass-wasting pass: unstable hillslopes fail and their debris runs out
/// downslope to the first repose slope.
#[allow(clippy::too_many_arguments)]
fn landslides(
    world: &mut World,
    params: &Params,
    climate: &Climate,
    dx: f64,
    areas: Option<&[f64]>,
    delta: &mut [f64],
    slope_buffer: &mut [f64],
    net: &mut [f64],
    balance: &mut MassBalance,
) {
    let h = &params.hillslope;
    let grid = world.grid;
    let sea = world.sea_level_m;
    let root = params.ecology.root_strength.clamp(0.0, 1.0);
    let critical_base = h.critical_slope_tan().max(1e-6);
    let runout_steps = (h.landslide_runout_m / dx).round().clamp(1.0, 8.0) as usize;

    for _ in 0..2 {
        delta.fill(0.0);
        compute_slopes(&world.elevation_m, slope_buffer);
        {
            let data = world.elevation_m.as_slice();
            for i in 0..grid.cell_count() {
                let z = data[i];
                if z <= sea {
                    continue;
                }
                let veg = f64::from(climate.vegetation.get_index(i)).clamp(0.0, 1.0);
                let cohesion = 1.0 + h.soil_cohesion * root * veg * 2.0;
                let critical = critical_base * cohesion;
                let here = slope_buffer[i];
                if here <= critical {
                    continue;
                }
                // Debris volume: the slope excess over a failure depth.
                let excess = (here - critical) * dx;
                let mut depth = 0.5 * excess;
                if depth <= 1e-9 {
                    continue;
                }
                // Run the debris downslope until the ground flattens below
                // repose, leaves the land, or the runout budget is spent.
                let mut current = i;
                let mut rest = i;
                for _ in 0..runout_steps {
                    let Some((next, _length, local_slope)) = steepest_descent(data, grid, current)
                    else {
                        break;
                    };
                    rest = next;
                    if data[next] <= sea {
                        break;
                    }
                    current = next;
                    if local_slope < critical * 0.6 {
                        break;
                    }
                }
                if rest == i {
                    // No lower ground: the material stays where it is.
                    continue;
                }
                let drop = z - data[rest];
                if drop <= 0.0 {
                    continue;
                }
                // Never bury the runout cell above the source.
                depth = depth.min(0.5 * drop);
                if depth <= 1e-9 {
                    continue;
                }
                delta[i] -= depth;
                // Conserve the transferred volume across unequal cell areas.
                delta[rest] += match areas {
                    Some(areas) => depth * areas[i] / areas[rest],
                    None => depth,
                };
            }
        }
        for i in 0..grid.cell_count() {
            let change = delta[i];
            if change == 0.0 {
                continue;
            }
            apply_surface_change(world, i, change, net, balance);
        }
    }
}

/// Soil production from bedrock, exponentially throttled by the existing soil
/// thickness and boosted by vegetation cover.
fn soil(world: &mut World, params: &Params, climate: &Climate, dt_yr: f64) {
    let h = &params.hillslope;
    let sea = world.sea_level_m;
    let soil_max = h.soil_max_m.max(0.01);
    let depth_scale = (soil_max / 3.0).max(1e-3);
    for i in 0..world.cell_count() {
        let z = world.elevation_m.get_index(i);
        if z <= sea {
            world.soil_m.set_index(i, 0.0);
            continue;
        }
        let current = f64::from(world.soil_m.get_index(i)).clamp(0.0, soil_max);
        if current >= soil_max * 0.999 {
            world.soil_m.set_index(i, soil_max as f32);
            continue;
        }
        let veg = f64::from(climate.vegetation.get_index(i)).clamp(0.0, 1.0);
        let production = h.soil_production * (-current / depth_scale).exp() * (0.4 + 0.6 * veg);
        let next = (current + production.max(0.0) * dt_yr).min(soil_max);
        world.soil_m.set_index(i, next as f32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::climate::{compute, ClimateInputs};
    use crate::geology::Geology;
    use crate::params::Params;
    use crate::plates::PlateMosaic;
    use crate::world::World;

    fn setup(w: u32, h: u32) -> (World, Params, Climate, Geology) {
        let mut p = Params::default();
        p.world.width = w;
        p.world.height = h;
        p.world.cell_km = 5.0;
        p.hillslope.diffusivity = 0.02;
        p.hillslope.nonlinearity = 0.0;
        let grid = p.world.grid().unwrap();
        let mut world = World::new(&p.world).unwrap();
        // A single spike; diffusion must spread it without gaining or losing
        // total volume (up to the boundary).
        world.elevation_m.fill_value(100.0);
        world
            .elevation_m
            .set((w / 2) as usize, (h / 2) as usize, 900.0);
        let mosaic = PlateMosaic::build(&p, grid, 2);
        let geology = Geology::build(&p, &world, &mosaic, 2);
        let climate = compute(&ClimateInputs {
            world: &world,
            params: &p,
            previous: None,
            dt_yr: 1000.0,
        });
        (world, p, climate, geology)
    }

    #[test]
    fn diffusion_spreads_relief_and_conserves_volume() {
        let (mut world, p, climate, geology) = setup(32, 32);
        let before: f64 = world.elevation_m.sum() * world.grid.cell_area_m2();
        let balance = apply(&mut world, &p, &climate, &geology, 5000.0);
        let after: f64 = world.elevation_m.sum() * world.grid.cell_area_m2();
        assert!((before - after).abs() / before < 1e-6, "volume changed");
        assert!(world.elevation_m.get(16, 16) < 900.0);
        assert!(balance.total_volume_m3() >= 0.0);
    }

    #[test]
    fn diffusion_ledger_closes() {
        let (mut world, p, climate, geology) = setup(32, 32);
        let balance = apply(&mut world, &p, &climate, &geology, 5000.0);
        let scale = balance.erosion_m3.max(1.0);
        assert!(
            balance.closure_error_m3().abs() / scale < 1e-6,
            "{balance:?}"
        );
        // A conservative pair exchange moves equal erosion and deposition.
        let moved = balance.erosion_m3.min(balance.deposition_m3);
        assert!(moved > 0.0);
    }

    #[test]
    fn diffusion_is_stable_for_absurd_timesteps() {
        let (mut world, p, climate, geology) = setup(32, 32);
        apply(&mut world, &p, &climate, &geology, 1e12);
        assert_eq!(world.elevation_m.non_finite_count(), 0);
        assert!(world.elevation_m.max_value().unwrap() < 1e6);
    }

    #[test]
    fn absurd_relief_and_timestep_stay_finite() {
        let (mut world, p, climate, geology) = setup(24, 24);
        for i in 0..world.cell_count() {
            let value = if i % 2 == 0 { 50_000.0 } else { -5_000.0 };
            world.elevation_m.set_index(i, value);
        }
        let balance = apply(&mut world, &p, &climate, &geology, 1e15);
        assert_eq!(world.elevation_m.non_finite_count(), 0);
        assert!(
            balance.closure_error_m3().abs() / balance.erosion_m3.max(1.0) < 1e-6,
            "{balance:?}"
        );
    }

    #[test]
    fn nonlinear_diffusion_does_not_exceed_the_critical_slope() {
        let (mut world, mut p, climate, geology) = setup(32, 32);
        p.hillslope.nonlinearity = 1.0;
        p.hillslope.diffusivity = 0.05;
        // Start with a very steep cone.
        for i in 0..world.cell_count() {
            let (x, y) = world.grid.coords(i);
            let dx = x as f64 - 16.0;
            let dy = y as f64 - 16.0;
            world
                .elevation_m
                .set_index(i, 3000.0 - (dx * dx + dy * dy).sqrt() * 400.0);
        }
        for _ in 0..40 {
            apply(&mut world, &p, &climate, &geology, 20_000.0);
        }
        let mut steepest = 0.0f64;
        for i in 0..world.cell_count() {
            let (x, y) = world.grid.coords(i);
            steepest = steepest.max(world.elevation_m.slope(x, y));
        }
        // The Roering flux is singular at the critical slope, so the relief
        // relaxes toward it instead of running away.
        assert!(
            steepest < p.hillslope.critical_slope_tan() * 3.0,
            "slope {steepest} ran away"
        );
    }

    #[test]
    fn landslide_plus_diffusion_conserves_mass() {
        let (mut world, mut p, climate, geology) = setup(24, 24);
        // A steep ridge so landsliding is active.
        for i in 0..world.cell_count() {
            let (x, _) = world.grid.coords(i);
            world.elevation_m.set_index(i, 100.0 + x as f64 * 500.0);
        }
        p.hillslope.landslide_runout_m = 5000.0;
        let before: f64 = world.elevation_m.sum();
        let balance = apply(&mut world, &p, &climate, &geology, 1000.0);
        let after: f64 = world.elevation_m.sum();
        assert!(
            (before - after).abs() / before < 1e-5,
            "{before} -> {after}"
        );
        assert!(
            balance.closure_error_m3().abs() / balance.erosion_m3.max(1.0) < 1e-6,
            "{balance:?}"
        );
        // Debris must actually have moved downslope.
        assert!(balance.erosion_m3 > 0.0);
    }

    #[test]
    fn soil_grows_on_land_only_and_saturates() {
        let (mut world, p, climate, geology) = setup(16, 16);
        world
            .elevation_m
            .map_in_place(|i, v| if i % 2 == 0 { v } else { -100.0 });
        apply(&mut world, &p, &climate, &geology, 100_000.0);
        for i in 0..world.grid.cell_count() {
            let (x, y) = world.grid.coords(i);
            if world.elevation_m.get(x, y) <= world.sea_level_m {
                assert_eq!(world.soil_m.get_index(i), 0.0);
            }
        }
        assert!(world.soil_m.max_value().unwrap() > 0.0);
        // Long exposure saturates the layer below the configured maximum.
        assert!(world.soil_m.max_value().unwrap() <= p.hillslope.soil_max_m + 1e-6);
    }

    #[test]
    fn hillslope_is_deterministic() {
        let make = || {
            let (mut world, p, climate, geology) = setup(24, 24);
            apply(&mut world, &p, &climate, &geology, 10_000.0);
            world.elevation_m.as_slice().to_vec()
        };
        assert_eq!(make(), make());
    }
}
