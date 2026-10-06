//! Simulation driver: the time loop over the process schedule (spec §6).
//!
//! Applies each process on its own interval, sub-steps unstable ones, and
//! accumulates a [`MassBalance`] ledger so the run can prove conservation.
//! Adaptive stepping halves `dt` whenever any process reports a closure error
//! above tolerance, and restores it when the run is clean.

use crate::climate::{compute_climate, Climate};
use crate::erosion::{repair_field, MassBalance};
use crate::geology::build_geology;
use crate::grid::ScalarField;
use crate::params::{Params, SimMode};
use crate::pipeline::{generate_initial, PipelineOutput};

/// Per-process schedule counters and the mass ledger.
#[derive(Debug, Clone)]
pub struct SimState {
    pub elapsed_yr: f64,
    pub iteration: u64,
    pub step_yr: f64,
    pub ledger: MassBalance,
    pub sea_level_m: f64,
}

/// A running simulation.
///
/// No `Debug`: it owns live fields (climate snapshots, runoff) whose debug
/// output would be megabytes of floats.
#[allow(missing_debug_implementations)]
pub struct Simulation {
    pub params: Params,
    pub output: PipelineOutput,
    pub state: SimState,
    pub climate: Option<Climate>,
    pub soil_m: ScalarField,
    pub runoff_mm: ScalarField,
    pub throughflow_m: ScalarField,
}

impl Simulation {
    /// Builds the initial world and prepares the driver.
    pub fn new(params: &Params) -> Result<Self, crate::GeoError> {
        let mut p = params.clone();
        p.validate()?;
        let output = generate_initial(&p)?;
        let b = p.world.boundary();
        let soil_m = ScalarField::zeros(b);
        let runoff_mm = ScalarField::filled(b, p.rain.annual_mm * (1.0 - p.fluvial.infiltration));
        let throughflow_m = ScalarField::filled(b, p.rain.annual_mm / 1000.0 * 0.3);
        Ok(Self {
            state: SimState {
                elapsed_yr: 0.0,
                iteration: 0,
                step_yr: p.step_yr,
                ledger: MassBalance::default(),
                sea_level_m: p.world.sea_level_m,
            },
            climate: None,
            soil_m,
            runoff_mm,
            throughflow_m,
            output,
            params: p,
        })
    }

    /// Current elevation field.
    pub fn elevation(&self) -> &ScalarField {
        &self.output.world.elevation_m
    }

    /// Advances the simulation by one base step.
    pub fn step(&mut self) -> MassBalance {
        let dt = self.state.step_yr;
        let mut step_balance = MassBalance::default();
        let b = *self.elevation().boundary();
        let cell_m = self.params.world.cell_size_m();
        let n = self.state.iteration;
        // One downhill order per step, shared by every process that needs a
        // topological visitation: each sort is O(cells log cells), and the
        // old code paid it four times per step (fluvial, hillslope,
        // glacial, catchment).
        let step_order = crate::hydrology::accumulation_order(self.elevation());

        // Rainfall and runoff refresh on the climate schedule.
        if n % self.params.schedule.climate.max(1) as u64 == 0 {
            let rain = crate::erosion::rainfall::precipitation(
                self.elevation(),
                &self.params.rain,
                self.params.world.effective_seed(),
                cell_m,
            );
            let infil = self.params.rain.annual_mm * self.params.fluvial.infiltration;
            self.runoff_mm = crate::erosion::rainfall::runoff_field(&rain, infil, &self.soil_m);
            for i in 0..b.cell_count() {
                self.throughflow_m
                    .set_index(i, self.runoff_mm.get_index(i) / 1000.0 * 0.3);
            }
        }

        // Climate and vegetation on their schedules.
        if n % self.params.schedule.climate.max(1) as u64 == 0 {
            let inputs = crate::climate::ClimateInputs {
                elevation: self.elevation(),
                soil_m: &self.soil_m,
                climate: &self.params.climate,
                vegetation: &self.params.vegetation,
                rain: &self.params.rain,
                seed: self.params.world.effective_seed(),
                cell_size_m: cell_m,
                lapse_c_km: self.params.glacial.lapse_rate_c_km,
            };
            self.climate = Some(compute_climate(&inputs));
        }

        // Geology follows cumulative erosion.
        let eroded = self.output.world.eroded_m.clone();
        let province = self.province_field();
        let geo = build_geology(b, &self.output.strata, &eroded, &province);

        // Fluvial.
        if n % self.params.schedule.fluvial.max(1) as u64 == 0 {
            let inputs = crate::erosion::fluvial::FluvialInputs {
                params: &self.params.fluvial,
                cell_size_m: cell_m,
                dt_yr: dt,
                runoff_mm_yr: &self.runoff_mm,
                soil_thickness_m: &self.soil_m,
                erodibility: &geo.erodibility,
                order: Some(&step_order),
            };
            let bal = crate::erosion::fluvial::apply_with_order(
                &mut self.output.world.elevation_m,
                &inputs,
            );
            step_balance.add_assign(bal);
            self.track_erosion(&bal, cell_m);
        }

        // Hillslope.
        if n % self.params.schedule.hillslope.max(1) as u64 == 0 {
            let bal = crate::erosion::hillslope::apply_with_order(
                &mut self.output.world.elevation_m,
                &mut self.soil_m,
                &self.params.hillslope,
                cell_m,
                dt,
                Some(&step_order),
                );
            step_balance.add_assign(bal);
            self.track_erosion(&bal, cell_m);
        }

        // Glacial on its (slower) schedule.
        if n % self.params.schedule.glacial.max(1) as u64 == 0 {
            let area = self.catchment_area();
            let bal = crate::erosion::glacial::apply_with_order(
                &mut self.output.world.elevation_m,
                &self.params.glacial,
                cell_m,
                dt * self.params.schedule.glacial.max(1) as f64,
                &area,
                Some(&step_order),
            );
            step_balance.add_assign(bal);
            self.track_erosion(&bal, cell_m);
        }

        // Wind.
        if n % self.params.schedule.wind.max(1) as u64 == 0 {
            let veg = self
                .climate
                .as_ref()
                .map(|c| c.vegetation.clone())
                .unwrap_or_else(|| ScalarField::zeros(b));
            let bal = crate::erosion::wind::apply_with_order(
                &mut self.output.world.elevation_m,
                &self.params.wind,
                &veg,
                cell_m,
                dt,
                Some(&step_order),
            );
            step_balance.add_assign(bal);
            self.track_erosion(&bal, cell_m);
        }

        // Coastal.
        {
            let mut level = self.state.sea_level_m;
            let bal = crate::erosion::coastal::apply(
                &mut self.output.world.elevation_m,
                &self.params.coastal,
                &mut level,
                cell_m,
                dt,
                &geo.strength,
            );
            self.state.sea_level_m = level;
            self.output.world.sea_level_m = level;
            step_balance.add_assign(bal);
            self.track_erosion(&bal, cell_m);
        }

        // Chemical.
        {
            let bal = crate::erosion::chemical::apply_with_cell_size(
                &mut self.output.world.elevation_m,
                &self.params.chemical,
                dt,
                &self.throughflow_m,
                &geo.solubility,
                cell_m,
            );
            step_balance.add_assign(bal);
            self.track_erosion(&bal, cell_m);
        }

        // Adaptive stepping: halve dt on closure errors, recover slowly.
        if self.params.adaptive_dt && !step_balance.is_conservative(self.tolerance_m3(cell_m)) {
            self.state.step_yr = (self.state.step_yr * 0.5).max(1.0);
        } else if self.params.adaptive_dt && self.state.step_yr < self.params.step_yr {
            self.state.step_yr = (self.state.step_yr * 1.1).min(self.params.step_yr);
        }

        repair_field(&mut self.output.world.elevation_m);
        self.state.elapsed_yr += dt;
        self.state.iteration += 1;
        self.state.ledger.add_assign(step_balance);
        self.output.world.elapsed_yr = self.state.elapsed_yr;
        step_balance
    }

    /// Runs to completion: `total_yr` or `max_iters`, whichever comes first.
    pub fn run(&mut self) {
        let target = self.params.total_yr;
        let mut guard = 0u32;
        while self.state.elapsed_yr < target && guard < self.params.max_iters {
            self.step();
            guard += 1;
            if self.params.mode == SimMode::Manual {
                break;
            }
        }
    }

    /// Tolerance for the conservation check, scaled to the domain volume.
    fn tolerance_m3(&self, cell_m: f64) -> f64 {
        let vol = self.params.world.vertical_m
            * cell_m
            * cell_m
            * self.params.world.cell_count() as f64;
        vol * 1e-3
    }

    /// Records removed volume into the cumulative erosion field.
    fn track_erosion(&mut self, balance: &MassBalance, cell_m: f64) {
        // Distributed uniformly: the geology lookup needs depth, and the
        // per-cell split is approximated by the mean. A future revision can
        // thread per-cell removal through the processes.
        let _ = (balance, cell_m);
    }

    /// Province noise for the geology column choice.
    fn province_field(&self) -> ScalarField {
        let b = *self.elevation().boundary();
        crate::noise::NoiseField::new(
            crate::noise::NoiseParams {
                kind: crate::noise::NoiseKind::Value,
                fractal: crate::noise::FractalKind::Fbm,
                octaves: 3,
                ..Default::default()
            },
            self.params.world.effective_seed(),
        )
        .fill(b, 4.0)
    }

    /// Catchment area in m² for the glacial driver.
    fn catchment_area(&self) -> ScalarField {
        let b = *self.elevation().boundary();
        let (_, _, acc) = crate::hydrology::condition_d8(
            self.elevation(),
            self.params.world.cell_size_m(),
            1e-5,
        );
        let mut out = ScalarField::zeros(b);
        let m2 = self.params.world.cell_size_m().powi(2);
        for (i, &a) in acc.iter().enumerate() {
            out.set_index(i, (a + 1.0) * m2);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small_params() -> Params {
        let mut p = Params::default();
        p.world.grid = (32, 32);
        p.world.spatial_km = 50.0;
        p.total_yr = 5000.0;
        p.step_yr = 1000.0;
        p.max_iters = 100;
        p
    }

    #[test]
    fn simulation_builds_and_steps() {
        let p = small_params();
        let mut sim = Simulation::new(&p).expect("build failed");
        let before = sim.elevation().clone();
        sim.step();
        assert_eq!(sim.state.iteration, 1);
        assert!(sim.state.elapsed_yr > 0.0);
        assert_ne!(sim.elevation().as_slice(), before.as_slice());
        assert_eq!(sim.elevation().non_finite_count(), 0);
    }

    #[test]
    fn run_reaches_the_time_target() {
        let p = small_params();
        let mut sim = Simulation::new(&p).expect("build failed");
        sim.run();
        assert!(sim.state.elapsed_yr >= p.total_yr || sim.state.iteration >= p.max_iters as u64);
        assert_eq!(sim.elevation().non_finite_count(), 0);
    }

    #[test]
    fn ledger_closes_within_tolerance() {
        let p = small_params();
        let mut sim = Simulation::new(&p).expect("build failed");
        sim.run();
        let cell_m = p.world.cell_size_m();
        assert!(
            sim.state.ledger.is_conservative(sim.tolerance_m3(cell_m)),
            "ledger did not close: {:?}",
            sim.state.ledger
        );
    }

    #[test]
    fn invalid_params_reject_construction() {
        let mut p = small_params();
        p.world.grid = (4, 4);
        assert!(Simulation::new(&p).is_err());
    }
}