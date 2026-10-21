//! Example user-defined process plugin for GeoGenesis.
//!
//! This crate lives outside `geogenesis-core` and shows that users can add
//! their own process without modifying the engine: implement
//! [`ProcessPlugin`] and register it on a simulation with
//! `Simulation::with_plugins`.
//!
//! The example implements **custom thermal diffusion** (a conservative
//! pairwise smoothing of the surface). It is deliberately simple but complete:
//! it mutates the terrain, updates the cumulative erosion/deposition fields so
//! isostasy sees the load changes, and returns an exact [`MassBalance`] so the
//! global ledger keeps closing.
//!
//! ```no_run
//! use geogenesis_core::params::Params;
//! use geogenesis_core::sim::Simulation;
//! use geogenesis_plugin_example::CustomDiffusion;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let params = Params::default();
//! let mut sim = Simulation::with_plugins(
//!     &params,
//!     vec![Box::new(CustomDiffusion { diffusivity_m2_yr: 120.0 })],
//! )?;
//! sim.step();
//! # Ok(())
//! # }
//! ```

use geogenesis_core::erosion::MassBalance;
use geogenesis_core::plugin::{ProcessContext, ProcessPlugin};

/// A conservative pairwise diffusion of the surface.
#[derive(Clone, Copy, Debug)]
pub struct CustomDiffusion {
    /// Diffusivity in m²/yr; a few tens to a few hundred is a plausible creep rate.
    pub diffusivity_m2_yr: f64,
}

impl ProcessPlugin for CustomDiffusion {
    fn name(&self) -> &'static str {
        "custom-diffusion"
    }

    fn description(&self) -> &'static str {
        "Example plugin: conservative pairwise surface diffusion"
    }

    fn apply(&self, ctx: &mut ProcessContext<'_>) -> MassBalance {
        let grid = ctx.grid;
        let cell_m = ctx.cell_m().max(1.0);
        let cells = grid.cell_count();
        if ctx.dt_yr <= 0.0 || cells == 0 {
            return MassBalance::default();
        }
        // Explicit stability limit: coefficient per pair <= 0.25.
        let coefficient = (self.diffusivity_m2_yr * ctx.dt_yr / (cell_m * cell_m) * 0.5).min(0.25);
        if coefficient <= 0.0 {
            return MassBalance::default();
        }
        let area = grid.cell_area_m2();
        // D8 directions 2 (east) and 4 (south); every unordered pair is visited
        // once because each cell only looks right/down.
        let mut erosion_m3 = 0.0;
        let mut deposition_m3 = 0.0;
        for index in 0..cells {
            let (x, y) = grid.coords(index);
            for dir in [2usize, 4] {
                let Some(other) = grid.neighbor_index(index, dir) else {
                    continue;
                };
                if other <= index {
                    continue;
                }
                let za = ctx.world.elevation_m.get(x, y);
                let (ox, oy) = grid.coords(other);
                let zb = ctx.world.elevation_m.get(ox, oy);
                let delta = za - zb;
                if delta.abs() < 1e-12 {
                    continue;
                }
                let transfer = coefficient * delta;
                if transfer.abs() < 1e-12 {
                    continue;
                }
                ctx.world.elevation_m.set(x, y, za - transfer);
                ctx.world.elevation_m.set(ox, oy, zb + transfer);
                // Track cumulative move for the isostatic feedback.
                if transfer > 0.0 {
                    let e = ctx.world.erosion_m.get(x, y) + transfer as f32;
                    ctx.world.erosion_m.set(x, y, e);
                    let d = ctx.world.deposition_m.get(ox, oy) + transfer as f32;
                    ctx.world.deposition_m.set(ox, oy, d);
                } else {
                    let e = ctx.world.erosion_m.get(ox, oy) + (-transfer) as f32;
                    ctx.world.erosion_m.set(ox, oy, e);
                    let d = ctx.world.deposition_m.get(x, y) + (-transfer) as f32;
                    ctx.world.deposition_m.set(x, y, d);
                }
                let volume = transfer.abs() * area;
                erosion_m3 += volume;
                deposition_m3 += volume;
            }
        }
        MassBalance {
            erosion_m3,
            deposition_m3,
            export_m3: 0.0,
            storage_m3: 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use geogenesis_core::params::Params;
    use geogenesis_core::sim::Simulation;

    fn params() -> Params {
        let mut p = Params::default();
        p.world.width = 64;
        p.world.height = 64;
        p.world.seed = 3;
        p.time.total_yr = 200_000.0;
        p.time.step_yr = 10_000.0;
        p.time.max_iters = 50;
        p
    }

    fn surface_mass(sim: &Simulation) -> f64 {
        let area = sim.world.grid.cell_area_m2();
        sim.world.elevation_m.as_slice().iter().sum::<f64>() * area
    }

    #[test]
    fn plugin_runs_and_conserves_mass() {
        let p = params();
        let mut sim = Simulation::with_plugins(
            &p,
            vec![Box::new(CustomDiffusion {
                diffusivity_m2_yr: 40.0,
            })],
        )
        .unwrap();
        // The plugin runs after the built-in chain and only redistributes mass,
        // so after one step the surface volume must equal the no-plugin baseline.
        let mut baseline = Simulation::new(&p).unwrap();
        sim.step();
        baseline.step();
        let plugin_mass = surface_mass(&sim);
        let baseline_mass = surface_mass(&baseline);
        assert!(
            (plugin_mass - baseline_mass).abs() < 1e-6 * baseline_mass.abs().max(1.0),
            "plugin changed total mass: {plugin_mass} vs {baseline_mass}"
        );
        assert!(sim.state.ledger.erosion_m3 > 0.0);
        assert!(
            sim.state.ledger.closure_error_m3().abs() < 1e-6 * sim.state.ledger.erosion_m3.max(1.0)
        );
        assert_eq!(sim.plugins()[0].name(), "custom-diffusion");
    }

    #[test]
    fn plugin_is_deterministic() {
        let p = params();
        let run = || {
            let mut sim = Simulation::with_plugins(
                &p,
                vec![Box::new(CustomDiffusion {
                    diffusivity_m2_yr: 25.0,
                })],
            )
            .unwrap();
            for _ in 0..5 {
                sim.step();
            }
            sim.world.elevation_m.as_slice().to_vec()
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn zero_diffusivity_is_a_noop() {
        let p = params();
        let mut sim = Simulation::with_plugins(
            &p,
            vec![Box::new(CustomDiffusion {
                diffusivity_m2_yr: 0.0,
            })],
        )
        .unwrap();
        let mut baseline = Simulation::new(&p).unwrap();
        sim.step();
        baseline.step();
        assert_eq!(
            sim.world.elevation_m.as_slice(),
            baseline.world.elevation_m.as_slice()
        );
    }
}
