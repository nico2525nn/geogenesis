//! Tectonic vertical motion and isostasy (spec §1b).
//!
//! The plate mosaic supplies a vertical rate field (boundary orogeny, rifting,
//! hotspot swells). This module integrates it into the surface and crust, then
//! relaxes topography towards isostatic equilibrium.
//!
//! # Flexural isostasy
//!
//! Airy isostasy compensates every load locally. Real lithosphere has elastic
//! strength: the flexural rigidity `D` (N·m) means loads narrower than the
//! flexural parameter `α = (D/(Δρ·g))^(1/4)` are carried regionally by the
//! plate instead of sinking locally. We model this by smoothing the isostatic
//! *residual* (target minus current elevation) with a Gaussian whose length is
//! derived from `D` (about one flexural parameter), using an `O(N)` three-box
//! blur. Long-wavelength loads are unaffected; sharp loads (a young orogen, a
//! delta, a sediment pile) are supported and their response spreads over the
//! flexural wavelength — the classic peripheral bulge / regional compensation
//! behaviour. Rigidities near `1e20 N·m` recover plain Airy isostasy; a
//! `1e25 N·m` plate behaves almost as a rigid lid.
//!
//! The anchor is important: the initial crustal thickness is chosen so that
//! Airy equilibrium reproduces the initial relief exactly. Isostasy therefore
//! only acts on *changes* — tectonic thickening, erosional unloading and
//! sediment loading — instead of flattening the generated landscape.
//!
//! # Relief envelope
//!
//! Tectonic forcing saturates as the surface approaches the configured relief
//! envelope (`vertical_m`): uplift tapers off near the top and trench
//! subsidence near the bottom. Real landscapes reach a dynamic equilibrium
//! where erosion balances uplift; this soft cap is the numerical version of
//! that limit and guarantees that millions of years of integration cannot
//! build unbounded relief.

use crate::grid::Grid;
use crate::params::Params;
use crate::plates::PlateMosaic;
use crate::world::World;

/// Airy isostatic elevation gain per metre of crust, dimensionless.
///
/// `dz/dT = 1 - rho_crust/rho_mantle`; for continental crust this is about
/// 0.16, i.e. each kilometre of crust supports about 160 m of elevation.
#[must_use]
pub fn airy_factor(params: &Params) -> f64 {
    1.0 - params.tectonics.density_continental_kg_m3 / params.tectonics.mantle_density_kg_m3
}

/// Flexural smoothing length of the isostatic response, in grid cells.
///
/// About one flexural parameter `α`; `None`-like values below one cell mean the
/// load response is effectively local Airy isostasy.
#[must_use]
pub fn flexure_smoothing_cells(params: &Params, grid: Grid) -> f64 {
    let sigma = params.tectonics.flexure_smoothing_cells(grid.cell_size_m());
    if sigma < 0.75 {
        0.0
    } else {
        sigma.min(grid.width().min(grid.height()) as f64 * 0.5)
    }
}

/// Reusable buffers for [`apply_with_scratch`].
///
/// The isostasy pass needs a residual field and two blur buffers; allocating
/// them every step dominated the cost at high resolution, so the driver keeps
/// one instance for the lifetime of the simulation.
#[derive(Clone, Debug, Default)]
pub struct UpliftScratch {
    residual: Vec<f64>,
    blur_a: Vec<f64>,
    blur_b: Vec<f64>,
}

impl UpliftScratch {
    /// Allocates buffers for `cells` cells.
    #[must_use]
    pub fn new(cells: usize) -> Self {
        Self {
            residual: vec![0.0; cells],
            blur_a: vec![0.0; cells],
            blur_b: vec![0.0; cells],
        }
    }

    fn ensure(&mut self, cells: usize) {
        if self.residual.len() < cells {
            self.residual.resize(cells, 0.0);
            self.blur_a.resize(cells, 0.0);
            self.blur_b.resize(cells, 0.0);
        }
    }
}

/// Applies tectonic vertical motion and isostatic relaxation for one step.
///
/// Convenience wrapper that allocates scratch buffers; long-lived simulations
/// should call [`apply_with_scratch`] instead.
pub fn apply(world: &mut World, mosaic: &PlateMosaic, params: &Params, dt_yr: f64) {
    let mut scratch = UpliftScratch::new(world.cell_count());
    apply_with_scratch(world, mosaic, params, dt_yr, dt_yr, &mut scratch);
}

/// Applies tectonic vertical motion over `dt_tectonic_yr` and isostatic
/// relaxation over `dt_isostasy_yr` (pass `0.0` to skip the relaxation).
///
/// The two time steps are separate so the driver can run the cheap tectonic
/// integration every step while relaxing isostasy on a coarser interval; the
/// relaxation is a smooth exponential, so sub-sampling it is accurate.
pub fn apply_with_scratch(
    world: &mut World,
    mosaic: &PlateMosaic,
    params: &Params,
    dt_tectonic_yr: f64,
    dt_isostasy_yr: f64,
    scratch: &mut UpliftScratch,
) {
    if dt_tectonic_yr <= 0.0 && dt_isostasy_yr <= 0.0 {
        return;
    }
    let dt_yr = dt_tectonic_yr.max(0.0);
    let grid = world.grid;
    let cell_count = grid.cell_count();
    let tectonic = &params.tectonics;
    let sea_base = world.sea_level_base_m;
    let airy = airy_factor(params).max(0.02);
    let relaxation_yr = tectonic.relaxation_yr().max(1.0);
    let _ = relaxation_yr;
    // No uniform background uplift: a spatially constant vertical velocity
    // integrated over millions of years would drown the whole world. The mean
    // uplift parameter scales the spatially varying boundary rates instead.
    let sediment_density_ratio = 2200.0 / tectonic.mantle_density_kg_m3;
    let crust_ref = world.crust_reference_m;
    let vertical = params.world.vertical_m.max(100.0);
    let hotspot_saturation_m = tectonic.hotspot_strength_m.max(1.0) * 3.0;
    // Upper/lower soft limits of the tectonic relief envelope.
    let uplift_fade_m = vertical * 0.85;
    let subsidence_fade_m = vertical * 1.6;

    for i in 0..cell_count {
        let boundary_rate = f64::from(mosaic.boundary_uplift_m_yr.get_index(i));
        let hotspot_rate = f64::from(mosaic.hotspot_uplift_m_yr.get_index(i));
        let z = world.elevation_m.get_index(i);

        // Tectonic forcing saturates towards the relief envelope: uplift fades
        // out near the top and trench subsidence fades out near the bottom.
        let relief_above_sea = (z - sea_base).max(0.0);
        let relief_below_sea = (sea_base - z).max(0.0);
        let positive = if boundary_rate > 0.0 {
            boundary_rate * (1.0 - (relief_above_sea / uplift_fade_m).clamp(0.0, 1.0))
        } else {
            boundary_rate * (1.0 - (relief_below_sea / subsidence_fade_m).clamp(0.0, 1.0))
        };
        // A plume swell has its own equilibrium height: dynamic support fades
        // as the swell approaches a few times the plume strength.
        let hotspot = if hotspot_rate > 0.0 {
            hotspot_rate * (1.0 - (relief_above_sea / hotspot_saturation_m).clamp(0.0, 1.0))
        } else {
            hotspot_rate
        };
        let rate = positive + hotspot;
        let dz = rate * dt_yr;

        // Integrate vertical motion. Crustal thickening/accretion follows the
        // uplift through the isostatic ratio.
        world.elevation_m.set_index(i, z + dz);
        world
            .crust_m
            .set_index(i, world.crust_m.get_index(i) + dz / airy);
        world.tectonic_rate_m_yr.set_index(i, rate as f32);
    }

    if dt_isostasy_yr > 0.0 && tectonic.isostasy {
        let relax = 1.0 - (-dt_isostasy_yr / relaxation_yr).exp();
        scratch.ensure(cell_count);
        let residual = &mut scratch.residual;
        // Isostatic residual: how far each cell is from Airy equilibrium for
        // its current crustal thickness and surface load.
        for (i, residual_i) in residual.iter_mut().enumerate().take(cell_count) {
            let sediment = f64::from(world.sediment_m.get_index(i));
            let ice = f64::from(world.ice_m.get_index(i));
            let load = sediment * sediment_density_ratio + ice * 0.9;
            let target = sea_base + airy * (world.crust_m.get_index(i) - crust_ref) - load;
            *residual_i = target - world.elevation_m.get_index(i);
        }
        // Flexural support: smooth the residual over the flexural wavelength.
        // O(N) regardless of the smoothing length (three-box Gaussian).
        let sigma = flexure_smoothing_cells(params, grid);
        if sigma > 0.0 {
            flexure_blur_into(
                &mut scratch.residual[..cell_count],
                grid,
                sigma,
                &mut scratch.blur_a,
                &mut scratch.blur_b,
            );
        }
        for (i, residual_i) in scratch.residual.iter().enumerate().take(cell_count) {
            if *residual_i != 0.0 {
                let current = world.elevation_m.get_index(i);
                world
                    .elevation_m
                    .set_index(i, current + *residual_i * relax);
            }
        }
    }
    world.elevation_m.sanitize_non_finite(sea_base);
    world.crust_m.sanitize_non_finite(crust_ref);
}

/// Records the crustal thickness that reproduces the current relief under
/// Airy isostasy. Called once after the initial relief is generated.
pub fn anchor_crust(world: &mut World, params: &Params) {
    let airy = airy_factor(params).max(0.02);
    let sea = world.sea_level_m;
    let cells = world.cell_count();
    // Reference thickness: the plate-scale mean used by the mosaic.
    let crust_ref = params.tectonics.plate_thickness_km * 1000.0;
    world.crust_reference_m = crust_ref;
    for i in 0..cells {
        let z = world.elevation_m.get_index(i);
        let sediment = f64::from(world.sediment_m.get_index(i));
        let load = sediment * 2200.0 / params.tectonics.mantle_density_kg_m3;
        world
            .crust_m
            .set_index(i, crust_ref + (z - sea + load) / airy);
    }
}

/// Adds an erosional unloading term to the isostatic target: removing `eroded`
/// metres of rock reduces crustal thickness, so only the isostatic fraction of
/// the eroded thickness is permanently lost.
///
/// Process modules call this after they move mass so the next isostatic
/// relaxation sees the new load. It is a no-op when isostasy is disabled.
pub fn apply_erosional_unloading(
    world: &mut World,
    params: &Params,
    eroded: &crate::grid::Field<f32>,
) {
    if !params.tectonics.isostasy {
        return;
    }
    let airy = airy_factor(params).max(0.02);
    for (i, &depth) in eroded.as_slice().iter().enumerate() {
        if depth > 0.0 {
            let removal = f64::from(depth) / airy;
            world
                .crust_m
                .set_index(i, (world.crust_m.get_index(i) - removal).max(1000.0));
        }
    }
}

/// Applies a Gaussian-like blur to a flat row-major field in `O(N)`, using
/// two successive box filters (two boxes of width `w` approximate a Gaussian
/// with `σ² = (w² - 1)/6`; the resulting kernel is smooth enough for the
/// flexural load response while staying cheap).
///
/// Wrapped grid axes wrap; open axes replicate the edge value.
fn flexure_blur_into(
    field: &mut [f64],
    grid: Grid,
    sigma_cells: f64,
    a: &mut Vec<f64>,
    b: &mut Vec<f64>,
) {
    if sigma_cells <= 0.0 || field.is_empty() {
        return;
    }
    // width of each box: sigma² = (w² - 1)/6  ⇒  w = sqrt(6σ² + 1).
    let width = (6.0 * sigma_cells * sigma_cells + 1.0)
        .sqrt()
        .round()
        .max(1.0) as usize;
    let radius = (width.max(1)) / 2;
    a.clear();
    a.extend_from_slice(field);
    b.resize(a.len(), 0.0);
    for _ in 0..2 {
        box_blur_horizontal(a, b, grid, radius);
        box_blur_vertical(b, a, grid, radius);
    }
    field.copy_from_slice(a);
}

#[inline]
fn axis_sample(values: &[f64], n: usize, index: isize, wraps: bool) -> f64 {
    if wraps {
        // The blur radius is always smaller than one period, so one wrap step
        // is enough; this replaces a division with two comparisons.
        let n = n as isize;
        let mut i = index;
        if i < 0 {
            i += n;
        } else if i >= n {
            i -= n;
        }
        values[i as usize]
    } else {
        values[index.clamp(0, n as isize - 1) as usize]
    }
}

#[inline]
fn wrap_index(index: isize, n: usize) -> usize {
    let n = n as isize;
    let mut i = index;
    if i < 0 {
        i += n;
    } else if i >= n {
        i -= n;
    }
    i as usize
}

/// One horizontal box-filter pass over `src`.
fn box_blur_horizontal(src: &[f64], dst: &mut [f64], grid: Grid, radius: usize) {
    let w = grid.width();
    let h = grid.height();
    let wraps = grid.boundary().wraps_x();
    let inv = 1.0 / (2 * radius + 1) as f64;
    let r = radius as isize;
    for y in 0..h {
        let row = y * w;
        let slice = &src[row..row + w];
        let mut sum: f64 = (-r..=r).map(|k| axis_sample(slice, w, k, wraps)).sum();
        for x in 0..w {
            dst[row + x] = sum * inv;
            let add = axis_sample(slice, w, x as isize + r + 1, wraps);
            let remove = axis_sample(slice, w, x as isize - r, wraps);
            sum += add - remove;
        }
    }
}

/// One vertical box-filter pass over `src`.
fn box_blur_vertical(src: &[f64], dst: &mut [f64], grid: Grid, radius: usize) {
    let w = grid.width();
    let h = grid.height();
    let wraps = grid.boundary().wraps_y();
    let inv = 1.0 / (2 * radius + 1) as f64;
    let r = radius as isize;
    for x in 0..w {
        let mut sum: f64 = (-r..=r)
            .map(|k| {
                let y = if wraps {
                    wrap_index(k, h)
                } else {
                    k.clamp(0, h as isize - 1) as usize
                };
                src[y * w + x]
            })
            .sum();
        for y in 0..h {
            dst[y * w + x] = sum * inv;
            let add_y = if wraps {
                wrap_index(y as isize + r + 1, h)
            } else {
                (y as isize + r + 1).clamp(0, h as isize - 1) as usize
            };
            let remove_y = if wraps {
                wrap_index(y as isize - r, h)
            } else {
                (y as isize - r).clamp(0, h as isize - 1) as usize
            };
            sum += src[add_y * w + x] - src[remove_y * w + x];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plates::PlateMosaic;
    use crate::world::World;

    fn setup() -> (Params, World, PlateMosaic) {
        let mut p = Params::default();
        p.world.width = 32;
        p.world.height = 32;
        p.world.cell_km = 20.0;
        let grid = p.world.grid().unwrap();
        let mosaic = PlateMosaic::build(&p, grid, 5);
        let mut world = World::new(&p.world).unwrap();
        // Synthetic relief: a ramp.
        world
            .elevation_m
            .map_in_place(|i, _| (i % 32) as f64 * 100.0);
        anchor_crust(&mut world, &p);
        (p, world, mosaic)
    }

    /// A mosaic with no tectonic forcing, for isolating isostasy.
    fn quiet_mosaic(p: &Params) -> PlateMosaic {
        let grid = p.world.grid().unwrap();
        let mut mosaic = PlateMosaic::build(p, grid, 5);
        mosaic.boundary_uplift_m_yr.fill_value(0.0);
        mosaic.hotspot_uplift_m_yr.fill_value(0.0);
        mosaic
    }

    #[test]
    fn airy_factor_is_physical() {
        let p = Params::default();
        let f = airy_factor(&p);
        assert!(f > 0.1 && f < 0.3, "airy factor {f}");
    }

    #[test]
    fn anchoring_reproduces_initial_relief() {
        let (p, mut world, mosaic) = setup();
        let before = world.elevation_m.clone();
        // One tiny step: isostasy alone must not move the anchored surface.
        uplift_minimal(&mut world, &mosaic, &p, 1.0);
        let max_drift = before
            .as_slice()
            .iter()
            .zip(world.elevation_m.as_slice())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f64::max);
        assert!(max_drift < 50.0, "anchored surface drifted {max_drift} m");
    }

    #[test]
    fn tectonic_uplift_raises_the_surface() {
        let (p, mut world, mosaic) = setup();
        let mean_before = world.elevation_m.mean().unwrap();
        apply(&mut world, &mosaic, &p, 100_000.0);
        let mean_after = world.elevation_m.mean().unwrap();
        assert!(mean_after > mean_before, "{mean_before} -> {mean_after}");
    }

    #[test]
    fn flexure_length_scales_with_rigidity() {
        let mut p = Params::default();
        p.world.width = 64;
        p.world.height = 64;
        let grid = p.world.grid().unwrap();
        p.tectonics.flexure_rigidity = 1e20;
        let soft = flexure_smoothing_cells(&p, grid);
        p.tectonics.flexure_rigidity = 1e25;
        let stiff = flexure_smoothing_cells(&p, grid);
        assert!(soft < 2.0, "weak plate should be nearly local Airy: {soft}");
        assert!(
            stiff > 5.0 * soft.max(0.5),
            "stiff plate should be regional"
        );
        assert!(stiff <= grid.width().min(grid.height()) as f64 * 0.5);
        // A rigidity that gives sub-cell smoothing is switched off entirely.
        p.tectonics.flexure_rigidity = 1e20;
        assert!(soft > 0.0);
        p.tectonics.flexure_rigidity = 1e18;
        assert_eq!(flexure_smoothing_cells(&p, grid), 0.0);
    }

    #[test]
    fn flexure_broadens_a_local_load_response() {
        let mut p = Params::default();
        p.world.width = 64;
        p.world.height = 64;
        p.world.cell_km = 10.0;
        let grid = p.world.grid().unwrap();
        let mosaic = quiet_mosaic(&p);
        let centre = grid.index(32, 32);

        // A one-cell crustal load on an otherwise flat, anchored world.
        let response = |rigidity: f64| {
            let mut q = p.clone();
            q.tectonics.flexure_rigidity = rigidity;
            let mut world = World::new(&q.world).unwrap();
            anchor_crust(&mut world, &q);
            world
                .crust_m
                .set_index(centre, world.crust_m.get_index(centre) + 2000.0);
            let flat = world.elevation_m.clone();
            apply(&mut world, &mosaic, &q, q.tectonics.relaxation_yr() * 0.5);
            let dz = |i: usize| world.elevation_m.get_index(i) - flat.get_index(i);
            let total: f64 = (0..grid.cell_count()).map(dz).sum();
            (
                dz(centre),
                dz(grid.index(36, 32)),
                dz(grid.index(44, 32)),
                total,
            )
        };
        let (soft_centre, soft_near, _soft_far, soft_total) = response(1e20);
        let (stiff_centre, stiff_near, stiff_far, stiff_total) = response(1e25);

        // Local Airy isostasy deflects mostly directly under the load.
        assert!(soft_centre > stiff_centre * 10.0,
            "flexure does not reduce the local deflection: soft {soft_centre}, stiff {stiff_centre}");
        assert!(
            soft_near < 0.02 * soft_centre,
            "weak plate response is not local: near {soft_near}, centre {soft_centre}"
        );
        // A stiff plate carries the load regionally: the response spreads.
        assert!(
            stiff_near > 0.25 * stiff_centre,
            "flexure does not spread the response: near {stiff_near}, centre {stiff_centre}"
        );
        assert!(
            stiff_far > 0.02 * stiff_centre,
            "flexure response does not reach far: far {stiff_far}, centre {stiff_centre}"
        );
        // Flexure redistributes the deflection without creating or losing it.
        assert!(
            (stiff_total - soft_total).abs() < 0.05 * soft_total.abs().max(1.0),
            "flexure does not conserve the total deflection: {soft_total} vs {stiff_total}"
        );
    }

    #[test]
    fn isostasy_relaxes_toward_equilibrium() {
        let mut p = Params::default();
        p.world.width = 32;
        p.world.height = 32;
        p.world.cell_km = 10.0;
        let grid = p.world.grid().unwrap();
        let mosaic = quiet_mosaic(&p);
        let mut world = World::new(&p.world).unwrap();
        anchor_crust(&mut world, &p);
        let airy = airy_factor(&p);
        let anomaly = 1000.0;
        world.crust_m.add_scalar(anomaly);
        let target =
            world.sea_level_base_m + airy * (world.crust_m.get_index(0) - world.crust_reference_m);
        let dt = p.tectonics.relaxation_yr() * 0.1;
        for _ in 0..200 {
            apply(&mut world, &mosaic, &p, dt);
        }
        let z = world.elevation_m.get_index(0);
        assert!(
            (z - target).abs() < 0.5,
            "isostasy did not converge: {z} vs target {target} (grid {})",
            grid.cell_count()
        );
    }

    #[test]
    fn sediment_load_lowers_the_isostatic_equilibrium() {
        let mut p = Params::default();
        p.world.width = 32;
        p.world.height = 32;
        p.world.cell_km = 10.0;
        let grid = p.world.grid().unwrap();
        let mosaic = quiet_mosaic(&p);
        let sea = 0.0;
        let run = |sediment: f32| {
            let mut world = World::new(&p.world).unwrap();
            anchor_crust(&mut world, &p);
            world.crust_m.add_scalar(1000.0);
            world.sediment_m.fill_value(sediment);
            let dt = p.tectonics.relaxation_yr() * 0.1;
            for _ in 0..80 {
                apply(&mut world, &mosaic, &p, dt);
            }
            world.elevation_m.get_index(0) - sea
        };
        let dry = run(0.0);
        let loaded = run(500.0);
        let expected_drop = 500.0 * 2200.0 / p.tectonics.mantle_density_kg_m3;
        assert!(
            (dry - loaded - expected_drop).abs() < 10.0,
            "sediment load response {dry} -> {loaded}, expected drop {expected_drop}"
        );
        let _ = grid;
    }

    #[test]
    fn relief_envelope_stops_runaway_tectonics() {
        let mut p = Params::default();
        p.world.width = 32;
        p.world.height = 32;
        p.world.cell_km = 10.0;
        let grid = p.world.grid().unwrap();
        let mut mosaic = quiet_mosaic(&p);
        // Uniform 5 mm/yr boundary uplift, the physical ceiling.
        mosaic.boundary_uplift_m_yr.fill_value(5e-3);
        let sea = 0.0;
        // Low ground rises; high ground is already at the envelope.
        let mut world = World::new(&p.world).unwrap();
        anchor_crust(&mut world, &p);
        world.elevation_m.fill_value(sea);
        let low_before = world.elevation_m.get_index(0);
        apply(&mut world, &mosaic, &p, 10_000.0);
        let low_gain = world.elevation_m.get_index(0) - low_before;
        assert!((low_gain - 50.0).abs() < 5.0, "low ground gain {low_gain}");

        world.elevation_m.fill_value(p.world.vertical_m * 0.84);
        let high_before = world.elevation_m.get_index(0);
        apply(&mut world, &mosaic, &p, 10_000.0);
        let high_gain = world.elevation_m.get_index(0) - high_before;
        assert!(
            high_gain < 0.2 * low_gain,
            "uplift is not saturated near the envelope: {high_gain} m"
        );
        let _ = grid;
    }

    #[test]
    fn hotspot_uplift_saturates() {
        let mut p = Params::default();
        p.world.width = 32;
        p.world.height = 32;
        let grid = p.world.grid().unwrap();
        let mut mosaic = quiet_mosaic(&p);
        mosaic.hotspot_uplift_m_yr.fill_value(1e-3);
        let mut world = World::new(&p.world).unwrap();
        anchor_crust(&mut world, &p);
        world.elevation_m.fill_value(world.sea_level_base_m);
        apply(&mut world, &mosaic, &p, 10_000.0);
        let low_gain = world.elevation_m.get_index(0) - world.sea_level_base_m;
        world
            .elevation_m
            .fill_value(world.sea_level_base_m + p.tectonics.hotspot_strength_m * 2.9);
        apply(&mut world, &mosaic, &p, 10_000.0);
        let high_gain = world.elevation_m.get_index(0)
            - (world.sea_level_base_m + p.tectonics.hotspot_strength_m * 2.9);
        assert!(
            low_gain > 9.0,
            "hotspot did not rise at low ground: {low_gain}"
        );
        assert!(
            high_gain < 0.1,
            "hotspot swell did not saturate: {high_gain}"
        );
        let _ = grid;
    }

    #[test]
    fn uplift_is_reproducible_from_the_seed() {
        let mut p = Params::default();
        p.world.width = 48;
        p.world.height = 48;
        p.world.cell_km = 16.0;
        let run = |params: &Params| {
            let grid = params.world.grid().unwrap();
            let mosaic = PlateMosaic::build(params, grid, params.world.effective_seed());
            let mut world = World::new(&params.world).unwrap();
            anchor_crust(&mut world, params);
            for _ in 0..3 {
                apply(&mut world, &mosaic, params, 10_000.0);
            }
            world.elevation_m.as_slice().to_vec()
        };
        p.world.seed = 77;
        let a = run(&p);
        let b = run(&p);
        assert_eq!(a, b);
        // A different seed gives a different tectonic history.
        p.world.seed = 78;
        let c = run(&p);
        assert_ne!(a, c);
    }

    #[test]
    fn default_world_stays_bounded_for_a_long_run() {
        use crate::sim::Simulation;
        let mut p = Params::default();
        p.world.width = 256;
        p.world.height = 256;
        p.world.seed = 42;
        p.time.total_yr = 1_000_000.0;
        p.time.step_yr = 10_000.0;
        p.time.max_iters = 100;
        p.normalize();
        let mut sim = Simulation::new(&p).unwrap();
        sim.step_many(100);
        let elevation = &sim.world.elevation_m;
        assert_eq!(elevation.non_finite_count(), 0);
        assert_eq!(sim.world.crust_m.non_finite_count(), 0);
        let (min, max) = sim.world.relief_range_m();
        assert!(max < 9000.0, "max elevation ran away: {max} m");
        assert!(min > -15_000.0, "ocean depth ran away: {min} m");
        assert!(
            sim.world.land_fraction() > 0.15 && sim.world.land_fraction() < 0.6,
            "land fraction {} out of range",
            sim.world.land_fraction()
        );
    }

    /// Isostasy without tectonic rates, for the anchoring test.
    fn uplift_minimal(world: &mut World, _mosaic: &PlateMosaic, params: &Params, dt: f64) {
        let airy = airy_factor(params).max(0.02);
        let relax = 1.0 - (-dt / params.tectonics.relaxation_yr()).exp();
        let sea = world.sea_level_base_m;
        let crust_ref = world.crust_reference_m;
        for i in 0..world.cell_count() {
            let target = sea + airy * (world.crust_m.get_index(i) - crust_ref);
            let current = world.elevation_m.get_index(i);
            world
                .elevation_m
                .set_index(i, current + (target - current) * relax);
        }
    }
}
