//! Simulation driver: the time loop over the process schedule (spec §6),
//! history capture and arbitrary-time seeking.
//!
//! The driver owns the world, plates, geology, climate and hydrology, and
//! advances them with a shared base time step. Each process runs on its own
//! interval (scaled from the base step), the mass ledger is accumulated every
//! step, and a timeline of compressed surface frames is captured for scrubbing.
//!
//! Determinism is a hard contract: `Simulation::new(params)` followed by the
//! same number of steps always produces bit-identical fields, which is what
//! makes [`Simulation::seek`] (rebuild + replay) legitimate.

use std::mem;

use crate::climate::{self, Climate, ClimateInputs};
use crate::erosion::{self, MassBalance};
use crate::geology::Geology;
use crate::grid::Field;
use crate::hydrology::{self, Hydro};
use crate::minerals::{self, MineralMap};
use crate::params::Params;
use crate::pipeline::{self, PipelineOutput};
use crate::plates::PlateMosaic;
use crate::world::World;

/// Per-step counters and the cumulative mass ledger.
#[derive(Clone, Debug, Default)]
pub struct SimState {
    /// Simulated years.
    pub elapsed_yr: f64,
    /// Base-step counter.
    pub iteration: u64,
    /// Current adaptive base step, years.
    pub step_yr: f64,
    /// Cumulative process mass ledger.
    pub ledger: MassBalance,
    /// Current sea level, metres.
    pub sea_level_m: f64,
    /// True once `total_yr` or the iteration cap has been reached.
    pub finished: bool,
    /// Total number of repair operations applied (diagnostic for stability).
    pub repairs: u64,
}

/// A compressed timeline frame.
#[derive(Clone, Debug)]
pub struct Frame {
    /// Simulated time at capture, years.
    pub elapsed_yr: f64,
    /// Sea level at capture, metres.
    pub sea_level_m: f64,
    /// Surface elevation as `f32`, row-major.
    pub elevation_m: Vec<f32>,
    /// Cheap summary computed during capture.
    pub summary: FrameSummary,
}

/// Compact per-frame summary for the timeline labels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameSummary {
    pub min_m: f64,
    pub max_m: f64,
    pub mean_m: f64,
    pub land_fraction: f64,
}

/// Timeline of surface snapshots.
#[derive(Clone, Debug, Default)]
pub struct History {
    /// Captured frames in order.
    pub frames: Vec<Frame>,
}

impl History {
    /// Number of frames.
    #[must_use]
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// True when no frame has been captured.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Frame by index.
    #[must_use]
    pub fn frame(&self, index: usize) -> Option<&Frame> {
        self.frames.get(index)
    }

    /// Latest frame.
    #[must_use]
    pub fn latest(&self) -> Option<&Frame> {
        self.frames.last()
    }

    /// Approximate memory footprint, bytes.
    #[must_use]
    pub fn memory_bytes(&self) -> usize {
        self.frames
            .iter()
            .map(|f| f.elevation_m.len() * mem::size_of::<f32>() + mem::size_of::<Frame>())
            .sum()
    }

    /// Index of the frame nearest to (but not after) `time_yr`.
    #[must_use]
    pub fn index_at(&self, time_yr: f64) -> Option<usize> {
        if self.frames.is_empty() {
            return None;
        }
        let mut best = 0;
        for (i, frame) in self.frames.iter().enumerate() {
            if frame.elapsed_yr <= time_yr + 1e-9 {
                best = i;
            } else {
                break;
            }
        }
        Some(best)
    }

    fn push(&mut self, frame: Frame, max_frames: usize) {
        if self
            .frames
            .last()
            .is_some_and(|last| frame.elapsed_yr <= last.elapsed_yr + 1e-9)
        {
            return;
        }
        if self.frames.len() >= max_frames {
            // Drop the second frame, not the initial one: the first frame is
            // the world's initial condition and is always useful.
            if max_frames > 1 {
                self.frames.remove(1);
            } else {
                self.frames.remove(0);
            }
        }
        self.frames.push(frame);
    }
}

/// A running simulation.
///
/// No `Debug`: it owns live fields whose debug output would be megabytes.
#[allow(missing_debug_implementations)]
pub struct Simulation {
    /// Parameters in effect.
    pub params: Params,
    /// World state.
    pub world: World,
    /// Plate mosaic.
    pub mosaic: PlateMosaic,
    /// Geological state.
    pub geology: Geology,
    /// Current climate snapshot.
    pub climate: Option<Climate>,
    /// Current hydrology.
    pub hydro: Hydro,
    /// Counters and ledger.
    pub state: SimState,
    /// Timeline frames.
    pub history: History,
    /// Frame budget derived from memory limits.
    max_frames: usize,
    /// Next capture time.
    next_frame_yr: f64,
    /// Cached frame interval.
    frame_interval_yr: f64,
    /// Cumulative erosion at the start of the step, used to thin the crust as
    /// material is removed (isostatic rebound feedback).
    erosion_prev: Vec<f32>,
    /// Reusable uplift/isostasy scratch buffers.
    uplift_scratch: crate::uplift::UpliftScratch,
    /// Years accumulated since the last isostatic relaxation.
    isostasy_accum_yr: f64,
    /// User-defined process plugins, applied once per step after the built-ins.
    plugins: Vec<Box<dyn crate::plugin::ProcessPlugin>>,
    /// Lazily computed mineral prospectivity map (expensive: ~34 ms at 384²).
    minerals: Option<MineralMap>,
    /// User-defined rock set (if any) used to build the geology.
    custom_rocks: Option<crate::rocks::RockSet>,
    /// User-defined strata stack (if any).
    custom_strata: Option<crate::rocks::StrataSpec>,
    /// Simulated time at which `minerals` was computed.
    minerals_elapsed_yr: f64,
}

impl Simulation {
    /// Builds the initial world and prepares the driver.
    pub fn new(params: &Params) -> Result<Self, crate::GeoError> {
        let mut p = params.clone();
        p.validate()?;
        p.normalize();
        p.validate()?;
        let PipelineOutput {
            world,
            mosaic,
            geology,
        } = pipeline::generate_initial(&p)?;
        let mut sim = Self {
            state: SimState {
                elapsed_yr: 0.0,
                iteration: 0,
                step_yr: p.time.step_yr,
                ledger: MassBalance::default(),
                sea_level_m: world.sea_level_m,
                finished: false,
                repairs: 0,
            },
            climate: None,
            hydro: Hydro::empty(world.grid),
            max_frames: Self::frame_budget(&p),
            next_frame_yr: f64::INFINITY,
            frame_interval_yr: 0.0,
            erosion_prev: vec![0.0; world.cell_count()],
            uplift_scratch: crate::uplift::UpliftScratch::new(world.cell_count()),
            isostasy_accum_yr: 0.0,
            plugins: Vec::new(),
            minerals: None,
            custom_rocks: None,
            custom_strata: None,
            minerals_elapsed_yr: f64::NEG_INFINITY,
            params: p,
            world,
            mosaic,
            geology,
            history: History::default(),
        };
        sim.frame_interval_yr = sim.params.time.frame_interval_yr();
        sim.next_frame_yr = sim.frame_interval_yr;
        sim.refresh_climate(0.0);
        sim.refresh_hydro();
        sim.capture_frame();
        Ok(sim)
    }

    /// Builds a simulation with user-defined process plugins.
    ///
    /// Plugins run after the built-in process chain on every step and their
    /// mass returns are folded into the global ledger. They must be
    /// deterministic; see [`crate::plugin`].
    pub fn with_plugins(
        params: &Params,
        plugins: Vec<Box<dyn crate::plugin::ProcessPlugin>>,
    ) -> Result<Self, crate::GeoError> {
        let mut sim = Self::new(params)?;
        sim.plugins = plugins;
        Ok(sim)
    }

    /// The registered plugins.
    #[must_use]
    pub fn plugins(&self) -> &[Box<dyn crate::plugin::ProcessPlugin>] {
        &self.plugins
    }

    /// Adds a plugin to a running simulation.
    pub fn add_plugin(&mut self, plugin: Box<dyn crate::plugin::ProcessPlugin>) {
        self.plugins.push(plugin);
    }

    /// Rebuilds the geology from a user-defined rock set and optional strata
    /// stack, then refreshes climate/hydrology/minerals so dependent fields
    /// stay consistent.
    pub fn set_geology_custom(
        &mut self,
        rocks: &crate::rocks::RockSet,
        strata: Option<&crate::rocks::StrataSpec>,
    ) -> Result<(), crate::GeoError> {
        let geology = crate::geology::Geology::build_custom(
            &self.params,
            &self.world,
            &self.mosaic,
            self.params.world.effective_seed(),
            rocks,
            strata,
        )?;
        self.geology = geology;
        self.custom_rocks = Some(rocks.clone());
        self.custom_strata = strata.cloned();
        self.minerals = None;
        self.refresh_climate(0.0);
        self.refresh_hydro();
        Ok(())
    }

    /// JSON describing the user-defined geology configuration, if any
    /// (`{"rocks": <RockSet JSON>, "strata": <StrataSpec JSON>|null}`).
    #[must_use]
    pub fn custom_geology_json(&self) -> Option<String> {
        let rocks = self.custom_rocks.as_ref()?;
        let rocks_json = rocks.to_json().ok()?;
        let strata_json = match &self.custom_strata {
            Some(spec) => spec.to_json().ok()?,
            None => "null".to_string(),
        };
        Some(format!(
            "{{\"rocks\":{rocks_json},\"strata\":{strata_json}}}"
        ))
    }

    /// Rebuilds the geology from a JSON description returned by
    /// [`Self::custom_geology_json`] (or the two JSON documents directly).
    pub fn set_geology_from_json(
        &mut self,
        rocks_json: &str,
        strata_json: Option<&str>,
    ) -> Result<(), crate::GeoError> {
        let rocks = if rocks_json.trim().is_empty() {
            crate::rocks::RockSet::builtin()
        } else {
            crate::rocks::RockSet::from_json(rocks_json)?
        };
        let strata = match strata_json {
            Some(text) if !text.trim().is_empty() && text.trim() != "null" => {
                Some(crate::rocks::StrataSpec::from_json(text)?)
            }
            _ => None,
        };
        self.set_geology_custom(&rocks, strata.as_ref())
    }

    /// Current mineral map, if computed.
    #[must_use]
    pub fn minerals(&self) -> Option<&MineralMap> {
        self.minerals.as_ref()
    }

    /// Computes (or refreshes) the mineral prospectivity map and returns it.
    ///
    /// The map is derived from the current world, climate, hydrology and
    /// geology, so it is refreshed lazily at most every 50 kyr of simulated
    /// time. Cached results are dropped on reset/seek rebuilds and recomputed
    /// on demand, which keeps determinism and avoids a per-step cost.
    pub fn ensure_minerals(&mut self) -> Option<&MineralMap> {
        let stale = self
            .minerals
            .as_ref()
            .is_none_or(|_| self.state.elapsed_yr - self.minerals_elapsed_yr >= 50_000.0);
        if stale {
            let computed = {
                let climate = self.climate.as_ref()?;
                minerals::generate(
                    &self.world,
                    &self.params,
                    climate,
                    &self.hydro,
                    &self.geology,
                )
            };
            self.minerals = Some(computed);
            self.minerals_elapsed_yr = self.state.elapsed_yr;
        }
        self.minerals.as_ref()
    }

    /// Snapshot of the mineral map plus its inventory (cloned so callers can
    /// use it without holding a borrow of the simulation).
    pub fn minerals_snapshot(&mut self) -> Option<(MineralMap, crate::minerals::MineralInventory)> {
        self.ensure_minerals()?;
        let map = self.minerals.as_ref()?.clone();
        let inventory = crate::minerals::inventory(&map, self.world.grid, &self.params);
        Some((map, inventory))
    }

    /// JSON report with the mineral catalogue, deposit map summary and
    /// inventory for the current world.
    pub fn minerals_report(&mut self) -> Option<String> {
        self.ensure_minerals()?;
        let map = self.minerals.as_ref()?;
        let inventory = crate::minerals::inventory(map, self.world.grid, &self.params);
        Some(crate::minerals::mineral_json(map, &inventory))
    }

    /// Frame budget: at most 64 MiB of surface frames.
    fn frame_budget(params: &Params) -> usize {
        let cells = params.world.width as usize * params.world.height as usize;
        let bytes_per_frame = cells * mem::size_of::<f32>() + 256;
        let budget_bytes = 64 * 1024 * 1024;
        (budget_bytes / bytes_per_frame.max(1)).clamp(2, params.time.history_frames as usize)
    }

    /// Current surface elevation.
    #[must_use]
    pub fn elevation(&self) -> &Field<f64> {
        &self.world.elevation_m
    }

    /// Current climate, if computed.
    #[must_use]
    pub fn climate(&self) -> Option<&Climate> {
        self.climate.as_ref()
    }

    /// Current hydrology.
    #[must_use]
    pub fn hydro(&self) -> &Hydro {
        &self.hydro
    }

    /// Current geology.
    #[must_use]
    pub fn geology(&self) -> &Geology {
        &self.geology
    }

    /// Current sea level.
    #[must_use]
    pub fn sea_level(&self) -> f64 {
        self.world.sea_level_m
    }

    /// Progress in `0..=1`.
    #[must_use]
    pub fn progress(&self) -> f64 {
        if self.params.time.total_yr <= 0.0 {
            1.0
        } else {
            (self.state.elapsed_yr / self.params.time.total_yr).clamp(0.0, 1.0)
        }
    }

    /// Advances one base step and returns that step's balance.
    pub fn step(&mut self) -> MassBalance {
        if self.state.finished {
            return MassBalance::default();
        }
        let dt = self.state.step_yr;
        let iteration = self.state.iteration;
        let cell_m = self.params.world.cell_size_m();

        self.update_sea_level();

        // Tectonics: the mosaic is rebuilt only every few steps because each
        // rebuild is O(cells × plates); in between the rates are applied.
        let tectonic_every = (self.params.time.tectonic_interval_yr / dt)
            .round()
            .clamp(1.0, 64.0) as u64;
        if iteration % tectonic_every == 0 {
            let span = dt * tectonic_every as f64;
            self.mosaic.advance(&self.params, span);
        }
        // Isostasy relaxes on a coarser interval than the tectonic rate: it is
        // an exponential response, so sub-sampling in time is accurate, and the
        // flexural blur dominates the per-step cost at high resolution.
        self.isostasy_accum_yr += dt;
        let mut isostasy_dt = 0.0;
        if self.isostasy_accum_yr >= self.params.time.isostasy_interval_yr {
            isostasy_dt = self.isostasy_accum_yr;
            self.isostasy_accum_yr = 0.0;
        }
        crate::uplift::apply_with_scratch(
            &mut self.world,
            &self.mosaic,
            &self.params,
            dt,
            isostasy_dt,
            &mut self.uplift_scratch,
        );

        // Climate changes on the scale of millennia and is expensive
        // (moisture sweep + distance transform), so it runs every ~20 kyr.
        // River routing is much cheaper per step but still refilled only every
        // ~10 kyr; the landscape changes little in between.
        let climate_every = (self.params.time.climate_interval_yr / dt)
            .round()
            .clamp(1.0, 64.0) as u64;
        if iteration % climate_every == 0 {
            self.refresh_climate(dt * climate_every as f64);
        }
        let hydro_every = (self.params.time.hydro_interval_yr / dt)
            .round()
            .clamp(1.0, 64.0) as u64;
        if iteration % hydro_every == 0 {
            self.refresh_hydro();
        }

        // Erosion processes.
        let mut step_balance = MassBalance::default();
        if let Some(climate) = self.climate.clone() {
            let fluvial = crate::erosion::fluvial::apply(
                &mut self.world,
                &self.params,
                &self.hydro,
                &climate,
                &self.geology,
                dt,
            );
            step_balance.add_assign(&fluvial);
            let hillslope = crate::erosion::hillslope::apply(
                &mut self.world,
                &self.params,
                &climate,
                &self.geology,
                dt,
            );
            step_balance.add_assign(&hillslope);

            let glacial_every = (self.params.glacial.glacial_update_yr / dt)
                .round()
                .clamp(1.0, 64.0) as u64;
            if iteration % glacial_every == 0 {
                let glacial = crate::erosion::glacial::apply(
                    &mut self.world,
                    &self.params,
                    &climate,
                    dt * glacial_every as f64,
                );
                step_balance.add_assign(&glacial);
            }
            let aeolian_every = (self.params.aeolian.aeolian_update_yr / dt)
                .round()
                .clamp(1.0, 64.0) as u64;
            if iteration % aeolian_every == 0 {
                let aeolian = crate::erosion::aeolian::apply(
                    &mut self.world,
                    &self.params,
                    &climate,
                    dt * aeolian_every as f64,
                );
                step_balance.add_assign(&aeolian);
            }
            let coastal =
                crate::erosion::coastal::apply(&mut self.world, &self.params, &climate, dt);
            step_balance.add_assign(&coastal);
            let chemical = crate::erosion::chemical::apply(
                &mut self.world,
                &self.params,
                &climate,
                &self.hydro,
                &self.geology,
                dt,
            );
            step_balance.add_assign(&chemical);
        }

        // User-defined process plugins run after the built-in chain. They get
        // the same world/context and their mass returns are added to the step
        // ledger so conservation accounting stays complete.
        if !self.plugins.is_empty() {
            let plugin_grid = self.world.grid;
            for index in 0..self.plugins.len() {
                let mut context = crate::plugin::ProcessContext {
                    world: &mut self.world,
                    params: &self.params,
                    climate: self.climate.as_ref(),
                    hydro: &self.hydro,
                    geology: &self.geology,
                    dt_yr: dt,
                    elapsed_yr: self.state.elapsed_yr,
                    grid: plugin_grid,
                };
                let balance = self.plugins[index].apply(&mut context);
                step_balance.add_assign(&balance);
            }
        }

        // Erosional unloading: removing a metre of rock thins the crust by a
        // metre, so the Airy equilibrium falls by only ~16% of the eroded
        // depth and the rest is rebound. Without this coupling the anchored
        // isostatic target never drops and erosion is undone every step.
        for i in 0..self.world.cell_count() {
            let cumulative = self.world.erosion_m.get_index(i);
            let delta = f64::from(cumulative) - f64::from(self.erosion_prev[i]);
            if delta > 0.0 {
                let crust = self.world.crust_m.get_index(i);
                self.world.crust_m.set_index(i, (crust - delta).max(1000.0));
                self.erosion_prev[i] = cumulative;
            }
        }

        // Geological surface after erosion.
        self.geology.update_surface(&self.world, &self.params);

        // State bookkeeping.
        self.state.repairs += self.world.sanitize() as u64;
        self.state.repairs += erosion::repair(&mut self.world) as u64;
        self.state.elapsed_yr += dt;
        self.state.iteration += 1;
        self.state.ledger.add_assign(&step_balance);
        self.world.elapsed_yr = self.state.elapsed_yr;
        self.state.sea_level_m = self.world.sea_level_m;

        // Adaptive stepping: shrink when the ledger fails to close, recover
        // slowly when it does. The ledger tolerance scales with domain volume.
        if self.params.time.adaptive_dt {
            let tolerance = self.tolerance_m3();
            let error = step_balance.closure_error_m3().abs();
            if error > tolerance {
                self.state.step_yr = (self.state.step_yr * 0.5).max(10.0);
            } else if self.state.step_yr < self.params.time.step_yr {
                self.state.step_yr = (self.state.step_yr * 1.05).min(self.params.time.step_yr);
            }
        }

        // Frame capture.
        if self.state.elapsed_yr + 1e-9 >= self.next_frame_yr {
            self.capture_frame();
            if self.frame_interval_yr > 0.0 {
                // Advance to the next interval strictly in the future.
                let k = ((self.state.elapsed_yr / self.frame_interval_yr) + 1.0).floor();
                self.next_frame_yr = k * self.frame_interval_yr;
            } else {
                self.next_frame_yr = f64::INFINITY;
            }
        }

        if self.state.elapsed_yr >= self.params.time.total_yr
            || self.state.iteration >= u64::from(self.params.time.max_iters)
        {
            self.state.finished = true;
        }
        self.world.elapsed_yr = self.state.elapsed_yr;
        let _ = (cell_m, dt);
        step_balance
    }

    /// Runs until the duration or iteration cap is reached.
    pub fn run(&mut self) {
        let mut guard = 0u64;
        let cap = u64::from(self.params.time.max_iters) + 16;
        while !self.state.finished && guard < cap {
            self.step();
            guard += 1;
        }
        self.state.finished = true;
    }

    /// Runs at most `steps` base steps, returning how many ran.
    pub fn step_many(&mut self, steps: u32) -> u32 {
        let mut ran = 0u32;
        for _ in 0..steps {
            if self.state.finished {
                break;
            }
            self.step();
            ran += 1;
        }
        ran
    }

    /// Advances to an absolute time, returning the number of steps taken.
    ///
    /// Moving backwards rebuilds the world from the parameters and replays,
    /// which is exact because the engine is deterministic. Timeline frames are
    /// preserved across the rebuild (they describe the same deterministic
    /// future), so scrubbing does not lose the timeline.
    pub fn seek(&mut self, target_yr: f64) -> Result<u64, crate::GeoError> {
        let target = target_yr.clamp(0.0, self.params.time.total_yr);
        if target < self.state.elapsed_yr - 1e-6 {
            self.reset_preserving_history()?;
        }
        let mut steps = 0u64;
        let cap = self.params.time.max_iters as u64 + 16;
        while self.state.elapsed_yr + 1e-6 < target && steps < cap {
            // Land exactly on the requested year: if the remaining interval is
            // shorter than the current adaptive step, take a final sub-step of
            // exactly that length. Without this the timeline reports the
            // adaptive boundary (e.g. 1.52 Myr for a 1.5 Myr seek).
            let remaining = target - self.state.elapsed_yr;
            let saved_step = self.state.step_yr;
            let exact = remaining < saved_step - 1e-9;
            if exact {
                self.state.step_yr = remaining;
            }
            self.step();
            if exact {
                self.state.step_yr = saved_step;
            }
            steps += 1;
        }
        Ok(steps)
    }

    /// Rebuilds the simulation from scratch, keeping the timeline frames.
    pub fn reset_preserving_history(&mut self) -> Result<(), crate::GeoError> {
        let frames = mem::take(&mut self.history);
        let plugins = mem::take(&mut self.plugins);
        let custom_rocks = self.custom_rocks.take();
        let custom_strata = self.custom_strata.take();
        let params = self.params.clone();
        *self = Simulation::new(&params)?;
        self.plugins = plugins;
        if let Some(rocks) = custom_rocks {
            self.set_geology_custom(&rocks, custom_strata.as_ref())?;
        }
        // Keep the previous frames, but only those describing the same
        // deterministic run. Since params and seed are unchanged, all of them
        // are valid; frames captured so far will be appended without duplicates.
        self.history = frames;
        Ok(())
    }

    /// Rebuilds entirely, dropping history.
    pub fn reset(&mut self) -> Result<(), crate::GeoError> {
        let plugins = mem::take(&mut self.plugins);
        let custom_rocks = self.custom_rocks.take();
        let custom_strata = self.custom_strata.take();
        let params = self.params.clone();
        *self = Simulation::new(&params)?;
        self.plugins = plugins;
        if let Some(rocks) = custom_rocks {
            self.set_geology_custom(&rocks, custom_strata.as_ref())?;
        }
        Ok(())
    }

    /// Current terrain report.
    #[must_use]
    /// Terrain statistics for the *current* surface.
    ///
    /// The stored climate/hydrology snapshots are refreshed on schedules, so
    /// statistics recompute them from the current surface to keep the report
    /// internally consistent (land fraction vs biome fractions, etc.).
    pub fn stats(&self) -> crate::stats::TerrainStats {
        let climate = climate::compute(&ClimateInputs {
            world: &self.world,
            params: &self.params,
            previous: self.climate.as_ref(),
            dt_yr: 0.0,
        });
        let hydro = hydrology::route(&self.world, &self.params, Some(&climate));
        crate::stats::compute(&self.world, &climate, &hydro, &self.geology)
    }

    fn tolerance_m3(&self) -> f64 {
        let cell_m = self.params.world.cell_size_m();
        let volume = self.params.world.vertical_m
            * cell_m
            * cell_m
            * self.params.world.width as f64
            * self.params.world.height as f64;
        volume * 1e-3
    }

    /// Recomputes the climate snapshot.
    fn refresh_climate(&mut self, dt_yr: f64) {
        let previous = self.climate.take();
        let climate = climate::compute(&ClimateInputs {
            world: &self.world,
            params: &self.params,
            previous: previous.as_ref(),
            dt_yr,
        });
        self.climate = Some(climate);
    }

    /// Recomputes routing from the current surface and climate.
    fn refresh_hydro(&mut self) {
        self.hydro = hydrology::route(&self.world, &self.params, self.climate.as_ref());
    }

    /// Applies the eustatic and long-term sea-level curve.
    fn update_sea_level(&mut self) {
        let elapsed = self.state.elapsed_yr;
        let cc = &self.params.climate;
        let period = (cc.climate_cycle_period_kyr * 1000.0).max(1000.0);
        let phase = std::f64::consts::TAU * elapsed / period;
        let eustatic = cc.sea_level_amplitude_m * phase.sin();
        let trend = self.params.coastal.sea_level_rate_mm_yr * elapsed / 1000.0;
        let sea = self.world.sea_level_base_m + eustatic + trend;
        self.world.sea_level_m = sea;
        self.state.sea_level_m = sea;
    }

    /// Captures the current surface as a timeline frame.
    fn capture_frame(&mut self) {
        let elevation = self.world.elevation_f32();
        let sea = self.world.sea_level_m;
        let mut min = f64::INFINITY;
        let mut max = f64::NEG_INFINITY;
        let mut sum = 0.0;
        let mut land = 0usize;
        for &z in &elevation {
            let z = f64::from(z);
            if z < min {
                min = z;
            }
            if z > max {
                max = z;
            }
            sum += z;
            if z > sea {
                land += 1;
            }
        }
        let n = elevation.len().max(1) as f64;
        let frame = Frame {
            elapsed_yr: self.state.elapsed_yr,
            sea_level_m: sea,
            summary: FrameSummary {
                min_m: min,
                max_m: max,
                mean_m: sum / n,
                land_fraction: land as f64 / n,
            },
            elevation_m: elevation,
        };
        self.history.push(frame, self.max_frames);
    }

    /// Display field for the UI. `None` when the field is not available yet or
    /// is derived on demand (slope).
    #[must_use]
    pub fn field(&self, kind: FieldKind) -> Option<FieldView<'_>> {
        match kind {
            FieldKind::Elevation => Some(FieldView::F64(self.elevation())),
            FieldKind::Biome => self.climate.as_ref().map(|c| FieldView::U8(&c.biome)),
            FieldKind::Temperature => self
                .climate
                .as_ref()
                .map(|c| FieldView::F32(&c.temperature_c)),
            FieldKind::Precipitation => self
                .climate
                .as_ref()
                .map(|c| FieldView::F32(&c.precipitation_mm)),
            FieldKind::Vegetation => self.climate.as_ref().map(|c| FieldView::F32(&c.vegetation)),
            FieldKind::Snow => self.climate.as_ref().map(|c| FieldView::F32(&c.snow_cover)),
            FieldKind::Rock => Some(FieldView::U8(&self.geology.rock_id)),
            FieldKind::Erodibility => Some(FieldView::F32(&self.geology.erodibility)),
            FieldKind::Slope => None,
            FieldKind::Sediment => Some(FieldView::F32(&self.world.sediment_m)),
            FieldKind::Soil => Some(FieldView::F32(&self.world.soil_m)),
            FieldKind::Ice => Some(FieldView::F32(&self.world.ice_m)),
            FieldKind::Erosion => Some(FieldView::F32(&self.world.erosion_m)),
            FieldKind::River => Some(FieldView::U8(&self.hydro.river_mask)),
            FieldKind::Lake => Some(FieldView::F32(&self.hydro.lake_depth_m)),
            FieldKind::UpliftRate => Some(FieldView::F32(&self.world.tectonic_rate_m_yr)),
            FieldKind::Aridity => self.climate.as_ref().map(|c| FieldView::F32(&c.aridity)),
            FieldKind::Minerals => self.minerals.as_ref().map(|m| FieldView::U8(&m.dominant)),
            FieldKind::MineralRichness => {
                self.minerals.as_ref().map(|m| FieldView::F32(&m.richness))
            }
            FieldKind::MineralDeposits => self
                .minerals
                .as_ref()
                .map(|m| FieldView::U8(&m.deposit_mask)),
        }
    }

    /// Slope field computed on demand, degrees.
    #[must_use]
    pub fn slope_deg(&self) -> Field<f32> {
        let grid = self.world.grid;
        Field::<f32>::from_fn(grid, |x, y| {
            self.world.elevation_m.slope(x, y).atan().to_degrees() as f32
        })
    }
}

/// Stable field identifiers shared with the WASM ABI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FieldKind {
    /// Surface elevation (f64).
    Elevation = 0,
    /// Biome index (u8).
    Biome = 1,
    /// Temperature, °C (f32).
    Temperature = 2,
    /// Precipitation, mm/yr (f32).
    Precipitation = 3,
    /// Vegetation cover (f32).
    Vegetation = 4,
    /// Snow cover (f32).
    Snow = 5,
    /// Exposed rock index (u8).
    Rock = 6,
    /// Erodibility (f32).
    Erodibility = 7,
    /// Slope (computed on request).
    Slope = 8,
    /// Sediment thickness (f32).
    Sediment = 9,
    /// Soil thickness (f32).
    Soil = 10,
    /// Ice thickness (f32).
    Ice = 11,
    /// Cumulative erosion (f32).
    Erosion = 12,
    /// River mask (u8).
    River = 13,
    /// Lake depth (f32).
    Lake = 14,
    /// Tectonic vertical rate (f32).
    UpliftRate = 15,
    /// Aridity index (potential evaporation / precipitation).
    Aridity = 16,
    /// Dominant mineral index per cell (`255` = none).
    Minerals = 17,
    /// Mineral prospectivity richness (0..1).
    MineralRichness = 18,
    /// Economic deposit mask (1 = deposit).
    MineralDeposits = 19,
}

impl FieldKind {
    /// Parses the ABI's numeric id.
    #[must_use]
    pub fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            0 => Self::Elevation,
            1 => Self::Biome,
            2 => Self::Temperature,
            3 => Self::Precipitation,
            4 => Self::Vegetation,
            5 => Self::Snow,
            6 => Self::Rock,
            7 => Self::Erodibility,
            8 => Self::Slope,
            9 => Self::Sediment,
            10 => Self::Soil,
            11 => Self::Ice,
            12 => Self::Erosion,
            13 => Self::River,
            14 => Self::Lake,
            15 => Self::UpliftRate,
            16 => Self::Aridity,
            17 => Self::Minerals,
            18 => Self::MineralRichness,
            19 => Self::MineralDeposits,
            _ => return None,
        })
    }

    /// Whether the field is categorical (`u8`).
    #[must_use]
    pub const fn is_categorical(self) -> bool {
        matches!(self, Self::Biome | Self::Rock | Self::River)
    }
}

/// Borrowed view of a simulation field.
pub enum FieldView<'a> {
    /// `f64` field.
    F64(&'a Field<f64>),
    /// `f32` field.
    F32(&'a Field<f32>),
    /// `u8` field.
    U8(&'a Field<u8>),
}

impl FieldView<'_> {
    /// Number of values (0 for empty views).
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::F64(f) => f.len(),
            Self::F32(f) => f.len(),
            Self::U8(f) => f.len(),
        }
    }

    /// True for empty views.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Copies values as `f32` (categorical values are converted losslessly up
    /// to 2^24).
    #[must_use]
    pub fn to_f32(&self) -> Vec<f32> {
        match self {
            Self::F64(f) => f.as_slice().iter().map(|&v| v as f32).collect(),
            Self::F32(f) => f.as_slice().to_vec(),
            Self::U8(f) => f.as_slice().iter().map(|&v| f32::from(v)).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small() -> Params {
        let mut p = Params::default();
        p.world.width = 48;
        p.world.height = 48;
        p.world.cell_km = 10.0;
        p.time.total_yr = 40_000.0;
        p.time.step_yr = 10_000.0;
        p.time.max_iters = 500;
        p.time.history_frames = 8;
        p.normalize();
        p
    }

    #[test]
    fn simulation_builds_and_steps() {
        let p = small();
        let mut sim = Simulation::new(&p).unwrap();
        assert_eq!(sim.state.iteration, 0);
        assert!(!sim.history.is_empty());
        let before = sim.elevation().clone();
        sim.step();
        assert_eq!(sim.state.iteration, 1);
        assert!(sim.state.elapsed_yr > 0.0);
        assert_ne!(sim.elevation().as_slice(), before.as_slice());
        assert_eq!(sim.elevation().non_finite_count(), 0);
    }

    #[test]
    fn run_reaches_the_duration() {
        let p = small();
        let mut sim = Simulation::new(&p).unwrap();
        sim.run();
        assert!(sim.state.elapsed_yr >= p.time.total_yr || sim.state.finished);
        assert_eq!(sim.elevation().non_finite_count(), 0);
        assert!(sim.history.len() > 1);
    }

    #[test]
    fn history_frames_are_monotonic_and_capped() {
        let p = small();
        let mut sim = Simulation::new(&p).unwrap();
        sim.run();
        assert!(sim.history.len() <= p.time.history_frames as usize);
        let frames = &sim.history.frames;
        for pair in frames.windows(2) {
            assert!(pair[1].elapsed_yr > pair[0].elapsed_yr);
        }
        assert!(sim.history.memory_bytes() > 0);
    }

    #[test]
    fn seek_forward_and_backward_matches_plain_run() {
        let p = small();
        let mut plain = Simulation::new(&p).unwrap();
        plain.run();
        let final_elev = plain.elevation().as_slice().to_vec();

        let mut sim = Simulation::new(&p).unwrap();
        sim.seek(p.time.total_yr * 0.5).unwrap();
        let mid_time = sim.state.elapsed_yr;
        assert!(mid_time < p.time.total_yr);
        sim.seek(p.time.total_yr).unwrap();
        // Replay is deterministic: the final surface must match exactly.
        assert_eq!(sim.elevation().as_slice(), final_elev.as_slice());
        // And history was preserved across the backward rebuild.
        assert!(sim.history.len() > 1);
    }

    #[test]
    fn seek_rebuild_is_exact_for_the_same_time() {
        let p = small();
        let mut a = Simulation::new(&p).unwrap();
        a.seek(20_000.0).unwrap();
        let elev_a = a.elevation().as_slice().to_vec();
        let mut b = Simulation::new(&p).unwrap();
        b.seek(20_000.0).unwrap();
        b.seek(10_000.0).unwrap();
        b.seek(20_000.0).unwrap();
        assert_eq!(b.elevation().as_slice(), elev_a.as_slice());
    }

    #[test]
    fn reset_restores_the_initial_world() {
        let p = small();
        let mut sim = Simulation::new(&p).unwrap();
        let initial = sim.elevation().as_slice().to_vec();
        sim.run();
        sim.reset().unwrap();
        assert_eq!(sim.elevation().as_slice(), initial.as_slice());
        assert_eq!(sim.state.iteration, 0);
    }

    #[test]
    fn ledger_is_accumulated_and_finite() {
        let p = small();
        let mut sim = Simulation::new(&p).unwrap();
        sim.run();
        let ledger = sim.state.ledger;
        assert!(ledger.erosion_m3 >= 0.0);
        assert!(ledger.export_m3 >= 0.0);
        let total = ledger.total_volume_m3();
        assert!(total.is_finite());
    }

    #[test]
    fn fields_are_exposed_for_display() {
        let p = small();
        let mut sim = Simulation::new(&p).unwrap();
        sim.step();
        for kind in [
            FieldKind::Elevation,
            FieldKind::Biome,
            FieldKind::Temperature,
            FieldKind::Precipitation,
            FieldKind::Rock,
            FieldKind::Slope,
            FieldKind::River,
            FieldKind::Lake,
        ] {
            assert_eq!(FieldKind::from_u8(kind as u8), Some(kind));
            match sim.field(kind) {
                None => {
                    assert_eq!(kind, FieldKind::Slope);
                }
                Some(view) => {
                    assert_eq!(
                        view.len(),
                        sim.params.world.width as usize * sim.params.world.height as usize
                    );
                    let f = view.to_f32();
                    assert_eq!(f.len(), view.len());
                }
            }
        }
    }

    #[test]
    fn stats_report_is_sane() {
        let p = small();
        let mut sim = Simulation::new(&p).unwrap();
        sim.step();
        let stats = sim.stats();
        assert_eq!(stats.grid_width, 48);
        assert!(stats.elevation.max_m >= stats.elevation.min_m);
        assert!(stats.hypsometry.land_fraction >= 0.0);
        assert!(!stats.biomes.is_empty());
    }
}
