//! User-defined process plugins (SPEC Phase 4 #29).
//!
//! GeoGenesis is deterministic and dependency-free, so plugins are **Rust
//! crates** that implement [`ProcessPlugin`] and are registered on a
//! [`Simulation`](crate::sim::Simulation) with
//! [`Simulation::with_plugins`](crate::sim::Simulation::with_plugins). A plugin
//! runs once per simulation step, after the built-in process chain, receives a
//! [`ProcessContext`] and returns a [`MassBalance`] that is folded into the
//! global ledger. Because plugins are plain Rust code they can live outside
//! this workspace (the `plugin-example` crate demonstrates that) without
//! touching the engine.
//!
//! Determinism rules: a plugin must only use the inputs in the context plus its
//! own immutable configuration (no clocks, no random state, no OS entropy).
//! The same seed and step sequence must produce identical terrain.
//!
//! # Example
//!
//! ```ignore
//! use geogenesis_core::erosion::MassBalance;
//! use geogenesis_core::plugin::{ProcessContext, ProcessPlugin};
//!
//! #[derive(Debug)]
//! struct MyProcess;
//!
//! impl ProcessPlugin for MyProcess {
//!     fn name(&self) -> &'static str { "my-process" }
//!
//!     fn apply(&self, ctx: &mut ProcessContext<'_>) -> MassBalance {
//!         // ... mutate ctx.world, return the volumes moved ...
//!         MassBalance::default()
//!     }
//! }
//! ```
//!
//! # Built-in demo: meteor impacts
//!
//! [`ImpactPlugin`] is a small, deterministic example plugin shipped with the
//! engine: rare impact events excavate a crater and redeposit part of the
//! ejecta on the rim. It demonstrates state-free determinism (impact times come
//! from hashing the seed and the event index), spatial mutation, and exact ledger
//! accounting. Enable it from the CLI with `--plugin impacts`.

use crate::climate::Climate;
use crate::erosion::MassBalance;
use crate::geology::Geology;
use crate::grid::Grid;
use crate::hydrology::Hydro;
use crate::math::rng::hash2;
use crate::params::Params;
use crate::world::World;

/// Everything a process plugin may read or mutate for one step.
pub struct ProcessContext<'a> {
    /// World state (elevation, crust, sediment, ...).
    pub world: &'a mut World,
    /// Parameters in effect.
    pub params: &'a Params,
    /// Climate snapshot for this step, if computed.
    pub climate: Option<&'a Climate>,
    /// Current hydrology (filled surface, receivers, discharge).
    pub hydro: &'a Hydro,
    /// Geological state (layers, erodibility).
    pub geology: &'a Geology,
    /// Length of this step in years.
    pub dt_yr: f64,
    /// Simulated time at the start of this step, years.
    pub elapsed_yr: f64,
    /// Grid geometry of the world.
    pub grid: Grid,
}

impl ProcessContext<'_> {
    /// Convenience: cell size in metres.
    #[must_use]
    pub fn cell_m(&self) -> f64 {
        self.grid.cell_size_m()
    }
}

/// A user-defined process that runs once per simulation step.
///
/// Implementations must be deterministic given the context and configuration.
pub trait ProcessPlugin: Send + Sync + std::fmt::Debug {
    /// Stable identifier used in reports and the CLI (`--plugin NAME`).
    fn name(&self) -> &'static str;

    /// One-line human description.
    fn description(&self) -> &'static str {
        ""
    }

    /// Applies the process and returns the mass it moved or moved out.
    fn apply(&self, ctx: &mut ProcessContext<'_>) -> MassBalance;
}

/// Looks up a built-in demo plugin by name (used by the CLI).
///
/// Users with their own crates do not need this function; they pass their
/// plugin directly to [`Simulation::with_plugins`](crate::sim::Simulation::with_plugins).
#[must_use]
pub fn builtin(name: &str, seed: u64) -> Option<Box<dyn ProcessPlugin>> {
    match name {
        "impacts" => Some(Box::new(ImpactPlugin::new(seed))),
        _ => None,
    }
}

/// Built-in demo plugin: deterministic meteor impacts.
///
/// Events are scheduled at `(k + fraction_k) * mean_interval_yr`, where
/// `fraction_k` is a hash of the world seed and `k`; only events inside the
/// current step window fire. Each event excavates a parabolic crater and
/// redeposits `retention` of the ejecta on its rim; the remaining volume is
/// exported (dust lost to the atmosphere/ocean). The ledger therefore closes
/// exactly.
#[derive(Clone, Debug)]
pub struct ImpactPlugin {
    seed: u64,
    /// Mean time between impacts, years.
    pub mean_interval_yr: f64,
    /// Crater diameter (rim to rim), metres.
    pub diameter_m: f64,
    /// Fraction of excavated volume redeposited on the rim.
    pub retention: f64,
}

impl ImpactPlugin {
    /// Default configuration: one ~12 km impact per ~4 Myr.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            mean_interval_yr: 4.0e6,
            diameter_m: 12_000.0,
            retention: 0.65,
        }
    }

    /// Applies one impact at `index`; returns the mass balance.
    fn impact(&self, ctx: &mut ProcessContext<'_>, index: u64) -> MassBalance {
        let grid = ctx.grid;
        let cell_m = grid.cell_size_m().max(1.0);
        let w = grid.width();
        let h = grid.height();
        let sea = ctx.world.sea_level_m;
        let radius_cells = (self.diameter_m * 0.5 / cell_m).max(1.0);
        let depth_m = self.diameter_m * 0.06;

        // Deterministic target cell; prefer land for visible landforms.
        let mut centre = None;
        for attempt in 0..12u64 {
            let hx = hash2(self.seed ^ 0x1_0000_0000, index, attempt);
            let hy = hash2(self.seed, index ^ 0x9E37_79B9, attempt);
            let x = (hx % w as u64) as usize;
            let y = (hy % h as u64) as usize;
            if ctx.world.elevation_m.get(x, y) > sea {
                centre = Some((x, y));
                break;
            }
            if centre.is_none() {
                centre = Some((x, y));
            }
        }
        let Some((cx, cy)) = centre else {
            return MassBalance::default();
        };
        let radius_cells_i = radius_cells.ceil() as isize + 2;
        let mut bowl: Vec<(usize, usize, f64)> = Vec::new();
        let mut removed_m3 = 0.0;
        for dy in -radius_cells_i..=radius_cells_i {
            for dx in -radius_cells_i..=radius_cells_i {
                let r = ((dx * dx + dy * dy) as f64).sqrt() / radius_cells;
                if r > 1.0 {
                    continue;
                }
                let x = ((cx as isize + dx).rem_euclid(w as isize)) as usize;
                let y = if grid.boundary().wraps_y() {
                    ((cy as isize + dy).rem_euclid(h as isize)) as usize
                } else {
                    (cy as isize + dy).clamp(0, h as isize - 1) as usize
                };
                let depth = depth_m * (1.0 - r * r);
                if depth <= 0.0 {
                    continue;
                }
                removed_m3 += depth * grid.cell_area_m2_at(x, y);
                bowl.push((x, y, depth));
            }
        }
        // Rim ring: redeposit the retained ejecta in a Gaussian ring.
        let ring_radius = radius_cells * 1.15;
        let ring_width = (radius_cells * 0.45).max(1.0);
        let mut ring_cells: Vec<(usize, usize, f64)> = Vec::new();
        let ring_extent = (ring_radius + 3.0 * ring_width).ceil() as isize + 1;
        let mut ring_weight_sum = 0.0;
        for dy in -ring_extent..=ring_extent {
            for dx in -ring_extent..=ring_extent {
                let r = ((dx * dx + dy * dy) as f64).sqrt();
                let t = (r - ring_radius) / ring_width;
                let weight = (-t * t).exp();
                if weight < 1e-4 {
                    continue;
                }
                let x = ((cx as isize + dx).rem_euclid(w as isize)) as usize;
                let y = if grid.boundary().wraps_y() {
                    ((cy as isize + dy).rem_euclid(h as isize)) as usize
                } else {
                    (cy as isize + dy).clamp(0, h as isize - 1) as usize
                };
                ring_weight_sum += weight * grid.cell_area_m2_at(x, y);
                ring_cells.push((x, y, weight));
            }
        }
        let deposited_m3 = removed_m3 * self.retention;
        if ring_weight_sum > 0.0 {
            let scale = deposited_m3 / ring_weight_sum;
            for (x, y, weight) in ring_cells {
                let lift = weight * scale;
                let z = ctx.world.elevation_m.get(x, y);
                ctx.world.elevation_m.set(x, y, z + lift);
                let deposition = ctx.world.deposition_m.get(x, y) + lift as f32;
                ctx.world.deposition_m.set(x, y, deposition);
            }
        }
        for (x, y, depth) in bowl {
            let z = ctx.world.elevation_m.get(x, y);
            ctx.world.elevation_m.set(x, y, z - depth);
            let erosion = ctx.world.erosion_m.get(x, y) + depth as f32;
            ctx.world.erosion_m.set(x, y, erosion);
        }
        MassBalance {
            erosion_m3: removed_m3,
            deposition_m3: deposited_m3,
            export_m3: removed_m3 - deposited_m3,
            storage_m3: 0.0,
        }
    }
}

impl ProcessPlugin for ImpactPlugin {
    fn name(&self) -> &'static str {
        "impacts"
    }

    fn description(&self) -> &'static str {
        "Deterministic meteor impacts with crater excavation and rim ejecta"
    }

    fn apply(&self, ctx: &mut ProcessContext<'_>) -> MassBalance {
        if ctx.dt_yr <= 0.0 || self.mean_interval_yr <= 0.0 {
            return MassBalance::default();
        }
        let start = ctx.elapsed_yr;
        let end = ctx.elapsed_yr + ctx.dt_yr;
        let first = (start / self.mean_interval_yr).floor() as u64;
        let last = (end / self.mean_interval_yr).ceil() as u64 + 1;
        let mut balance = MassBalance::default();
        // Cap the number of events per step (adversarial parameters).
        for index in first..=last.min(first + 64) {
            let jitter =
                (hash2(self.seed ^ 0x1234_5678, index, 7) % 1_000_003) as f64 / 1_000_003.0;
            let time = (index as f64 + jitter) * self.mean_interval_yr;
            if time > start - 1e-9 && time <= end + 1e-9 {
                balance.add_assign(&self.impact(ctx, index));
            }
        }
        balance
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::Simulation;

    fn params() -> Params {
        let mut p = Params::default();
        p.world.width = 64;
        p.world.height = 64;
        p.world.seed = 11;
        p.time.total_yr = 400_000.0;
        p.time.step_yr = 10_000.0;
        p.time.adaptive_dt = false;
        p.time.max_iters = 100;
        p.time.climate_interval_yr = 100_000.0;
        p.time.hydro_interval_yr = 50_000.0;
        p
    }

    #[test]
    fn impacts_change_terrain_and_close_the_ledger() {
        let p = params();
        let mut sim = Simulation::with_plugins(
            &p,
            vec![Box::new(ImpactPlugin {
                seed: 3,
                mean_interval_yr: 60_000.0,
                diameter_m: 30_000.0,
                retention: 0.7,
            })],
        )
        .unwrap();
        let before = sim.world.elevation_m.as_slice().to_vec();
        let mut total = MassBalance::default();
        for _ in 0..20 {
            total.add_assign(&sim.step());
        }
        let after = sim.world.elevation_m.as_slice();
        let changed = before
            .iter()
            .zip(after)
            .filter(|(a, b)| (*a - *b).abs() > 1e-6)
            .count();
        assert!(changed > 0, "impacts should modify the surface");
        assert!(total.erosion_m3 > 0.0, "impacts must excavate volume");
        // Volumes are ~1e9 m³; closure is exact up to float summation order.
        assert!(
            total.closure_error_m3().abs() < 1e-9 * total.erosion_m3.max(1.0),
            "closure error {}",
            total.closure_error_m3()
        );
        assert!(after.iter().all(|z| z.is_finite()));
        // Ledger in the simulation driver also closes for the plugin's volumes.
        assert!(sim.state.ledger.erosion_m3 > 0.0);
    }

    #[test]
    fn impact_plugin_is_deterministic() {
        let p = params();
        let run = || {
            let mut sim = Simulation::with_plugins(
                &p,
                vec![Box::new(ImpactPlugin {
                    seed: 5,
                    mean_interval_yr: 60_000.0,
                    diameter_m: 20_000.0,
                    retention: 0.6,
                })],
            )
            .unwrap();
            for _ in 0..20 {
                sim.step();
            }
            sim.world.elevation_m.as_slice().to_vec()
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn reset_preserves_plugins() {
        let p = params();
        let mut sim = Simulation::with_plugins(&p, vec![Box::new(ImpactPlugin::new(1))]).unwrap();
        assert_eq!(sim.plugins().len(), 1);
        sim.step();
        sim.reset_preserving_history().unwrap();
        assert_eq!(sim.plugins().len(), 1);
        sim.reset().unwrap();
        assert_eq!(sim.plugins().len(), 1);
    }

    #[test]
    fn builtin_lookup_works() {
        assert!(builtin("impacts", 42).is_some());
        assert!(builtin("nope", 42).is_none());
    }
}
