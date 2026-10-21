//! Surface processes: the erosion–deposition engine and its mass ledger
//! (spec §3).
//!
//! Each process is an independent module with one entry point that mutates the
//! world and returns a [`MassBalance`]. The simulation driver decides when to
//! run each one; the ledger proves that mass removed from one place is either
//! deposited elsewhere or exported off the map.
//!
//! Process order matters and follows the spec schedule: fluvial incision first
//! (it sets base level), then hillslope diffusion and mass wasting (which
//! delivers sediment to the rivers), then glacial, aeolian, coastal and
//! chemical processes.

pub mod aeolian;
pub mod chemical;
pub mod coastal;
pub mod fluvial;
pub mod glacial;
pub mod hillslope;

use crate::params::Params;
use crate::world::World;

use std::cell::RefCell;

thread_local! {
    /// Per-thread pool of `f64` scratch buffers. Processes run every step, so
    /// reusing these removes their transient allocations without any global
    /// state: each worker thread owns its pool.
    static SCRATCH: RefCell<Vec<Vec<f64>>> = const { RefCell::new(Vec::new()) };
}

/// Runs `f` with `count` zeroed `f64` scratch buffers of `cells` values each.
///
/// The buffers are reused across calls on the same thread. Calls must not
/// nest: the pool is borrowed for the duration of `f`.
pub fn with_scratch<R>(cells: usize, count: usize, f: impl FnOnce(&mut [Vec<f64>]) -> R) -> R {
    SCRATCH.with(|pool| {
        let mut pool = pool.borrow_mut();
        while pool.len() < count {
            pool.push(Vec::new());
        }
        for buffer in pool.iter_mut().take(count) {
            buffer.clear();
            buffer.resize(cells, 0.0);
        }
        f(&mut pool[..count])
    })
}

/// Volume balance of one process application, in m³.
///
/// The invariant every process must satisfy is
/// `erosion ≈ deposition + export` within numerical tolerance. `storage`
/// covers material the process legitimately parks in a sediment buffer that is
/// not part of the surface field (for example material held in the channel
/// network between sub-steps).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MassBalance {
    /// Volume removed from the surface, m³.
    pub erosion_m3: f64,
    /// Volume added back to the surface, m³.
    pub deposition_m3: f64,
    /// Volume that left the domain or entered the ocean sink, m³.
    pub export_m3: f64,
    /// Volume kept in an internal sediment buffer, m³.
    pub storage_m3: f64,
}

impl MassBalance {
    /// Net closure error, m³ (`erosion - deposition - export - storage`).
    #[must_use]
    pub fn closure_error_m3(&self) -> f64 {
        self.erosion_m3 - self.deposition_m3 - self.export_m3 - self.storage_m3
    }

    /// True when the ledger closes within `tolerance_m3`.
    #[must_use]
    pub fn is_conservative(&self, tolerance_m3: f64) -> bool {
        self.closure_error_m3().abs() <= tolerance_m3.abs().max(f64::EPSILON)
    }

    /// Adds another balance into this one.
    pub fn add_assign(&mut self, other: &Self) {
        self.erosion_m3 += other.erosion_m3;
        self.deposition_m3 += other.deposition_m3;
        self.export_m3 += other.export_m3;
        self.storage_m3 += other.storage_m3;
    }

    /// Sum of all absolute volumes, for diagnostics.
    #[must_use]
    pub fn total_volume_m3(&self) -> f64 {
        self.erosion_m3 + self.deposition_m3 + self.export_m3 + self.storage_m3.abs()
    }
}

/// Large-scale CFL-style guard: the largest stable explicit time step for a
/// diffusion coefficient `d` (m²/yr) on a grid with cell size `dx`.
#[must_use]
pub fn diffusion_max_dt(d: f64, dx_m: f64) -> f64 {
    if d <= 0.0 || dx_m <= 0.0 {
        return f64::INFINITY;
    }
    0.24 * dx_m * dx_m / d
}

/// Splits `dt_yr` into `n` equal sub-steps no larger than `max_dt`.
#[must_use]
pub fn substeps(dt_yr: f64, max_dt: f64, cap: u32) -> (u32, f64) {
    if !max_dt.is_finite() || max_dt <= 0.0 || dt_yr <= max_dt {
        return (1, dt_yr);
    }
    let n = (dt_yr / max_dt).ceil().clamp(1.0, f64::from(cap.max(1))) as u32;
    (n, dt_yr / f64::from(n))
}

/// Clamps the elevation field to physical bounds and repairs non-finite cells.
///
/// Used by the driver after every process so one unstable module cannot poison
/// the rest of the step.
pub fn repair(world: &mut World) -> usize {
    let sea = world.sea_level_m;
    let vertical = 60_000.0;
    let mut repaired = 0usize;
    for z in world.elevation_m.as_mut_slice() {
        if !z.is_finite() {
            *z = sea;
            repaired += 1;
        }
        let clamped = z.clamp(-vertical, vertical);
        if clamped != *z {
            repaired += 1;
            *z = clamped;
        }
    }
    repaired
}

/// Applies every scheduled process for one time step and returns the combined
/// balance. Kept here (rather than in the driver) so integration tests can
/// exercise the full process chain without owning a [`crate::sim::Simulation`].
pub fn apply_all(
    world: &mut World,
    params: &Params,
    dt_yr: f64,
    hydrology: &crate::hydrology::Hydro,
    climate: &crate::climate::Climate,
    geology: &crate::geology::Geology,
) -> MassBalance {
    let mut total = MassBalance::default();
    total.add_assign(&fluvial::apply(
        world, params, hydrology, climate, geology, dt_yr,
    ));
    total.add_assign(&hillslope::apply(world, params, climate, geology, dt_yr));
    total.add_assign(&glacial::apply(world, params, climate, dt_yr));
    total.add_assign(&aeolian::apply(world, params, climate, dt_yr));
    total.add_assign(&coastal::apply(world, params, climate, dt_yr));
    total.add_assign(&chemical::apply(
        world, params, climate, hydrology, geology, dt_yr,
    ));
    repair(world);
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn balance_closure() {
        let b = MassBalance {
            erosion_m3: 100.0,
            deposition_m3: 60.0,
            export_m3: 40.0,
            storage_m3: 0.0,
        };
        assert!(b.is_conservative(1e-9));
        let bad = MassBalance {
            erosion_m3: 100.0,
            deposition_m3: 10.0,
            export_m3: 10.0,
            storage_m3: 0.0,
        };
        assert!(!bad.is_conservative(1e-3));
    }

    #[test]
    fn substeps_respect_the_cap() {
        let (n, dt) = substeps(1000.0, 100.0, 50);
        assert_eq!(n, 10);
        assert!((dt - 100.0).abs() < 1e-12);
        let (n, dt) = substeps(1000.0, 10.0, 8);
        assert_eq!(n, 8);
        assert!((dt - 125.0).abs() < 1e-12);
    }

    #[test]
    fn diffusion_dt_is_positive() {
        assert!(diffusion_max_dt(0.02, 10_000.0) > 0.0);
        assert!(diffusion_max_dt(0.0, 10_000.0).is_infinite());
    }

    /// Every process must convert its surface change to a volume with the
    /// *physical* area of the changed cell, not the nominal planar area: on a
    /// spherical grid an equatorial cell is an order of magnitude larger than a
    /// polar one. The test measures the area-weighted surface change directly
    /// and compares it with the reported ledger, which also checks that each
    /// ledger closes internally (export residual / pairwise-conservative
    /// transfers).
    #[test]
    fn process_ledgers_use_physical_areas_on_spherical_worlds() {
        use crate::climate::{compute, ClimateInputs};
        use crate::geology::Geology;
        use crate::grid::{BoundaryKind, Field};
        use crate::hydrology::route;
        use crate::plates::PlateMosaic;
        use crate::world::World;

        let mut p = Params::default();
        p.world.width = 32;
        p.world.height = 24;
        p.world.cell_km = 10.0;
        p.world.boundary = BoundaryKind::Spherical;
        p.world.planet_radius_km = 6371.0;
        p.world.sea_level_m = -500.0;
        p.hydrology.channel_init_cells = 1;
        p.fluvial.law = 1.0;
        p.fluvial.kf = 1e-3;
        p.ecology.root_strength = 0.0;
        let grid = p.world.grid().unwrap();
        assert!(grid.has_spherical_areas());
        let mut world = World::new(&p.world).unwrap();
        // Latitude-cosine relief plus an x tilt: every process has something to
        // erode or deposit on both large and small cells.
        world.elevation_m = Field::from_fn(grid, |x, y| {
            let lat = (y as f64 + 0.5) * std::f64::consts::PI / grid.height() as f64
                - std::f64::consts::FRAC_PI_2;
            300.0 + 800.0 * lat.cos() - x as f64 * 6.0
        });
        let mosaic = PlateMosaic::build(&p, grid, 11);
        let mut geology = Geology::build(&p, &world, &mosaic, 11);
        geology
            .erodibility
            .as_mut_slice()
            .iter_mut()
            .for_each(|v| *v = 1.0);
        geology
            .solubility
            .as_mut_slice()
            .iter_mut()
            .for_each(|v| *v = 1.0);
        let mut climate = compute(&ClimateInputs {
            world: &world,
            params: &p,
            previous: None,
            dt_yr: 0.0,
        });
        climate
            .vegetation
            .as_mut_slice()
            .iter_mut()
            .for_each(|v| *v = 0.0);
        let hydro = route(&world, &p, Some(&climate));
        let before = world.elevation_m.as_slice().to_vec();

        let check = |name: &str, balance: MassBalance, after: &World| {
            let mut erosion_measured = 0.0;
            let mut deposition_measured = 0.0;
            for i in 0..grid.cell_count() {
                let delta = after.elevation_m.get_index(i) - before[i];
                let area = grid.cell_area_m2_at_index(i);
                if delta < 0.0 {
                    erosion_measured += -delta * area;
                } else if delta > 0.0 {
                    deposition_measured += delta * area;
                }
            }
            let scale = balance
                .erosion_m3
                .abs()
                .max(balance.deposition_m3.abs())
                .max(1.0);
            assert!(
                (balance.erosion_m3 - erosion_measured).abs() / scale < 1e-9,
                "{name}: ledger erosion {} vs measured {erosion_measured}",
                balance.erosion_m3
            );
            assert!(
                (balance.deposition_m3 - deposition_measured).abs() / scale < 1e-9,
                "{name}: ledger deposition {} vs measured {deposition_measured}",
                balance.deposition_m3
            );
            assert!(
                balance.closure_error_m3().abs() / scale < 1e-9,
                "{name}: ledger does not close: {balance:?}"
            );
        };

        let dt = 10_000.0;
        let run_once = |name: &str, f: &dyn Fn(&mut World) -> MassBalance| {
            let mut clone = world.clone();
            let balance = f(&mut clone);
            check(name, balance, &clone);
        };
        run_once("fluvial", &|w| {
            fluvial::apply(w, &p, &hydro, &climate, &geology, dt)
        });
        run_once("hillslope", &|w| {
            hillslope::apply(w, &p, &climate, &geology, dt)
        });
        run_once("glacial", &|w| glacial::apply(w, &p, &climate, dt));
        run_once("aeolian", &|w| aeolian::apply(w, &p, &climate, dt));
        run_once("coastal", &|w| coastal::apply(w, &p, &climate, dt));
        run_once("chemical", &|w| {
            chemical::apply(w, &p, &climate, &hydro, &geology, dt)
        });
        // The unmodified reference world must not have moved.
        assert_eq!(world.elevation_m.as_slice(), before.as_slice());
    }

    /// Long-run conservation guard: the default Earth-like world on a 96²
    /// grid, run for 100 steps of 20 kyr (2 Myr), must keep the mass ledger
    /// closed to 1e-6 relative, keep the elevation field finite and the relief
    /// inside a physically plausible envelope.
    ///
    /// This exercises the whole process chain (tectonics, climate, hydrology,
    /// fluvial, hillslope, glacial, aeolian, coastal, chemical) through the
    /// real simulation driver, including its adaptive sub-stepping.
    #[test]
    fn long_run_default_world_conserves_mass_and_relief() {
        let mut p = Params::default();
        p.world.width = 96;
        p.world.height = 96;
        p.world.cell_km = 10.0;
        p.time.total_yr = 2.0e6;
        p.time.step_yr = 2.0e4;
        p.time.max_iters = 100;
        p.time.history_frames = 4;
        p.normalize();
        let mut sim = crate::sim::Simulation::new(&p).unwrap();
        let ran = sim.step_many(100);
        assert_eq!(ran, 100, "simulation did not run the full 100 steps");
        assert!(
            sim.state.elapsed_yr >= 2.0e6 * 0.99,
            "only {:.1} kyr simulated",
            sim.state.elapsed_yr / 1000.0
        );

        let ledger = sim.state.ledger;
        let scale = ledger.total_volume_m3().max(1.0);
        // erod ed ≈ deposited + exported (+ buffer storage) to 1e-6 relative.
        assert!(
            ledger.closure_error_m3().abs() / scale < 1e-6,
            "ledger closure {:.3e} relative: {ledger:?}",
            ledger.closure_error_m3().abs() / scale
        );
        let accounted = ledger.deposition_m3 + ledger.export_m3 + ledger.storage_m3;
        assert!(
            (ledger.erosion_m3 - accounted).abs() / scale < 1e-6,
            "eroded {} vs deposited+exported {accounted}: {ledger:?}",
            ledger.erosion_m3
        );
        assert!(ledger.erosion_m3 > 0.0, "nothing eroded in 2 Myr");
        assert!(ledger.export_m3 > 0.0, "nothing exported in 2 Myr");

        // Finite elevation and bounded relief after the long run.
        assert_eq!(
            sim.world.elevation_m.non_finite_count(),
            0,
            "non-finite elevation after 2 Myr"
        );
        let (low, high) = sim.world.relief_range_m();
        assert!(low.is_finite() && high.is_finite(), "relief {low}..{high}");
        assert!(high < 9_000.0, "highest point {high} m");
        assert!(low > -9_000.0, "lowest point {low} m");
        let relief = high - low;
        assert!(
            (1_000.0..12_000.0).contains(&relief),
            "implausible relief {relief} m"
        );
        println!(
            "long run: {:.2} Myr, {} steps, ledger closure {:.3e} relative, \
             erosion {:.3e} m³, deposition {:.3e} m³, export {:.3e} m³, relief {relief:.0} m",
            sim.state.elapsed_yr / 1e6,
            ran,
            ledger.closure_error_m3().abs() / scale,
            ledger.erosion_m3,
            ledger.deposition_m3,
            ledger.export_m3
        );
    }
}
