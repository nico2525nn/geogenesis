//! World geometry, datum, seed and time-stepping parameters.

use crate::error::{GeoError, GeoResult};
use crate::grid::{BoundaryKind, Grid, MAX_GRID_DIM, MIN_GRID_DIM};
use crate::math::rng::Rng;
use crate::params::spec::{ParamGroup, ParamSpec};

use super::ParamBlock;

/// Default seed used when the configuration asks for "random" (`seed = 0`).
///
/// The engine itself is deterministic and never reads the clock; an explicit
/// random seed is chosen by the caller (CLI, browser) and passed in. This
/// constant keeps `seed = 0` reproducible for tests and batch runs.
pub const DEFAULT_SEED: u64 = 0x6765_6F67_656E_6573;

/// Grid, extent, datum and seed (spec §2 Phase 0).
#[derive(Clone, Debug, PartialEq)]
pub struct WorldParams {
    /// Grid width in cells.
    pub width: u32,
    /// Grid height in cells.
    pub height: u32,
    /// Ground size of one cell in kilometres.
    pub cell_km: f64,
    /// Vertical amplitude, i.e. half the elevation span, in metres.
    pub vertical_m: f64,
    /// Sea-level datum in metres.
    pub sea_level_m: f64,
    /// Edge topology.
    pub boundary: BoundaryKind,
    /// Planet radius in kilometres, used for spherical corrections.
    pub planet_radius_km: f64,
    /// World seed. `0` selects [`DEFAULT_SEED`] deterministically.
    pub seed: u64,
}

impl Default for WorldParams {
    fn default() -> Self {
        Self {
            width: 384,
            height: 384,
            cell_km: 10.0,
            vertical_m: 8000.0,
            sea_level_m: 0.0,
            boundary: BoundaryKind::Toroidal,
            planet_radius_km: 6371.0,
            seed: 1337,
        }
    }
}

impl WorldParams {
    /// Cell edge length in metres.
    #[inline]
    #[must_use]
    pub fn cell_size_m(&self) -> f64 {
        self.cell_km * 1000.0
    }

    /// Domain width in kilometres.
    #[inline]
    #[must_use]
    pub fn extent_x_km(&self) -> f64 {
        f64::from(self.width) * self.cell_km
    }

    /// Domain height in kilometres.
    #[inline]
    #[must_use]
    pub fn extent_y_km(&self) -> f64 {
        f64::from(self.height) * self.cell_km
    }

    /// Effective seed: `0` maps to the deterministic default.
    #[inline]
    #[must_use]
    pub fn effective_seed(&self) -> u64 {
        if self.seed == 0 {
            DEFAULT_SEED
        } else {
            self.seed
        }
    }

    /// Builds the validated [`Grid`] described by these parameters.
    ///
    /// Spherical worlds carry the planet radius, which enables the
    /// `cos(latitude)` per-cell area correction in
    /// [`Grid::cell_area_m2_at`](crate::grid::Grid::cell_area_m2_at). Planar
    /// worlds keep the planar cell area unchanged.
    pub fn grid(&self) -> GeoResult<Grid> {
        let grid = Grid::new(self.width, self.height, self.cell_size_m(), self.boundary)?;
        if self.boundary == BoundaryKind::Spherical {
            grid.with_planet_radius(self.planet_radius_m())
        } else {
            Ok(grid)
        }
    }

    /// Planet radius in metres (the parameter is expressed in kilometres).
    #[inline]
    #[must_use]
    pub fn planet_radius_m(&self) -> f64 {
        self.planet_radius_km * 1000.0
    }

    /// A short human summary such as `384x384 @ 10 km`.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "{}×{} @ {} km ({}×{} km)",
            self.width,
            self.height,
            self.cell_km,
            self.extent_x_km().round(),
            self.extent_y_km().round()
        )
    }
}

impl ParamBlock for WorldParams {
    fn specs() -> &'static [ParamSpec] {
        const SPECS: [ParamSpec; 8] = [
            ParamSpec::int(
                "width",
                "Grid width",
                "グリッド幅",
                ParamGroup::World,
                "cells",
                32.0,
                4096.0,
                384.0,
                "Simulation grid width; height follows the same cell size. 4096² is memory-hungry in single-shot mode; tiled generation is the large-grid route.",
            ),
            ParamSpec::int(
                "height",
                "Grid height",
                "グリッド高さ",
                ParamGroup::World,
                "cells",
                32.0,
                4096.0,
                384.0,
                "Simulation grid height. 4096² is memory-hungry in single-shot mode; tiled generation is the large-grid route.",
            ),
            ParamSpec::log(
                "cell_km",
                "Cell size",
                "セルサイズ",
                ParamGroup::World,
                "km",
                1.0,
                200.0,
                10.0,
                "Ground size of one grid cell. Wider cells cover more area.",
            ),
            ParamSpec::log(
                "vertical_m",
                "Vertical scale",
                "鉛直スケール",
                ParamGroup::World,
                "m",
                300.0,
                20_000.0,
                8000.0,
                "Half the elevation range the generated world can span.",
            ),
            ParamSpec::float(
                "sea_level_m",
                "Sea level",
                "海面高度",
                ParamGroup::World,
                "m",
                -5000.0,
                5000.0,
                0.0,
                "Datum splitting ocean from land.",
            ),
            ParamSpec::enumerated(
                "boundary",
                "Boundary",
                "境界",
                ParamGroup::World,
                &["Rectangular", "Toroidal", "Spherical"],
                1.0,
                "How the map edges connect: open, wrapped, or longitude-wrapped sphere.",
            ),
            ParamSpec::log(
                "planet_radius_km",
                "Planet radius",
                "惑星半径",
                ParamGroup::World,
                "km",
                1000.0,
                100_000.0,
                6371.0,
                "Used for spherical coordinates and area corrections.",
            ),
            ParamSpec::int(
                "seed",
                "Seed",
                "シード",
                ParamGroup::World,
                "",
                0.0,
                9_007_199_254_740_992.0,
                1337.0,
                "Deterministic world seed; 0 selects the built-in default.",
            ),
        ];
        &SPECS
    }

    fn values_into(&self, out: &mut Vec<f64>) {
        out.push(f64::from(self.width));
        out.push(f64::from(self.height));
        out.push(self.cell_km);
        out.push(self.vertical_m);
        out.push(self.sea_level_m);
        out.push(match self.boundary {
            BoundaryKind::Rectangular => 0.0,
            BoundaryKind::Toroidal => 1.0,
            BoundaryKind::Spherical => 2.0,
        });
        out.push(self.planet_radius_km);
        out.push(self.seed as f64);
    }

    fn set_values(&mut self, v: &[f64]) {
        if let Some(&x) = v.first() {
            self.width = x.round().max(1.0) as u32;
        }
        if let Some(&x) = v.get(1) {
            self.height = x.round().max(1.0) as u32;
        }
        if let Some(&x) = v.get(2) {
            self.cell_km = x;
        }
        if let Some(&x) = v.get(3) {
            self.vertical_m = x;
        }
        if let Some(&x) = v.get(4) {
            self.sea_level_m = x;
        }
        if let Some(&x) = v.get(5) {
            self.boundary = match x.round() as i64 {
                0 => BoundaryKind::Rectangular,
                2 => BoundaryKind::Spherical,
                _ => BoundaryKind::Toroidal,
            };
        }
        if let Some(&x) = v.get(6) {
            self.planet_radius_km = x;
        }
        if let Some(&x) = v.get(7) {
            self.seed = x.max(0.0) as u64;
        }
    }

    fn set_key(&mut self, key: &str, value: f64) -> bool {
        match key {
            "width" => self.width = value.round().max(1.0) as u32,
            "height" => self.height = value.round().max(1.0) as u32,
            "cell_km" => self.cell_km = value,
            "vertical_m" => self.vertical_m = value,
            "sea_level_m" => self.sea_level_m = value,
            "boundary" => {
                self.boundary = match value.round() as i64 {
                    0 => BoundaryKind::Rectangular,
                    2 => BoundaryKind::Spherical,
                    _ => BoundaryKind::Toroidal,
                }
            }
            "planet_radius_km" => self.planet_radius_km = value,
            "seed" => self.seed = value.max(0.0) as u64,
            _ => return false,
        }
        true
    }

    fn get_key(&self, key: &str) -> Option<f64> {
        Some(match key {
            "width" => f64::from(self.width),
            "height" => f64::from(self.height),
            "cell_km" => self.cell_km,
            "vertical_m" => self.vertical_m,
            "sea_level_m" => self.sea_level_m,
            "boundary" => match self.boundary {
                BoundaryKind::Rectangular => 0.0,
                BoundaryKind::Toroidal => 1.0,
                BoundaryKind::Spherical => 2.0,
            },
            "planet_radius_km" => self.planet_radius_km,
            "seed" => self.seed as f64,
            _ => return None,
        })
    }

    fn normalize(&mut self) {
        // Allow up to the engine limit so the CLI can attempt 4096² (the
        // single-shot path is memory-hungry; tiled generation is the planned
        // Phase 3 route). The UI slider keeps its own sane default range.
        self.width = self.width.clamp(MIN_GRID_DIM, MAX_GRID_DIM);
        self.height = self.height.clamp(MIN_GRID_DIM, MAX_GRID_DIM);
        self.cell_km = self.cell_km.clamp(1.0, 200.0);
        self.vertical_m = self.vertical_m.clamp(300.0, 20_000.0);
        self.sea_level_m = self.sea_level_m.clamp(-5000.0, 5000.0);
        self.planet_radius_km = self.planet_radius_km.clamp(1000.0, 100_000.0);
    }

    fn validate(&self) -> GeoResult<()> {
        if !(MIN_GRID_DIM..=MAX_GRID_DIM).contains(&self.width) {
            return Err(GeoError::InvalidGrid(format!(
                "width {} outside {}..={}",
                self.width, MIN_GRID_DIM, MAX_GRID_DIM
            )));
        }
        if !(MIN_GRID_DIM..=MAX_GRID_DIM).contains(&self.height) {
            return Err(GeoError::InvalidGrid(format!(
                "height {} outside {}..={}",
                self.height, MIN_GRID_DIM, MAX_GRID_DIM
            )));
        }
        if !self.cell_km.is_finite() || self.cell_km <= 0.0 {
            return Err(GeoError::param("cell_km", self.cell_km, "must be positive"));
        }
        if !self.vertical_m.is_finite() || self.vertical_m <= 0.0 {
            return Err(GeoError::param(
                "vertical_m",
                self.vertical_m,
                "must be positive",
            ));
        }
        if self.sea_level_m.abs() > self.vertical_m * 2.0 {
            return Err(GeoError::param(
                "sea_level_m",
                self.sea_level_m,
                "must stay within twice the vertical scale",
            ));
        }
        Ok(())
    }

    fn randomize(&mut self, rng: &mut Rng) {
        // Keep grids browser-sized; randomisation explores shape, not size.
        self.width = 256;
        self.height = 256;
        self.cell_km = rng.range_f64(4.0, 20.0);
        self.vertical_m = rng.range_f64(5000.0, 14_000.0);
        self.sea_level_m = 0.0;
        self.boundary = if rng.chance(0.85) {
            BoundaryKind::Toroidal
        } else {
            BoundaryKind::Rectangular
        };
        self.seed = rng.next_u64() & 0x1F_FFFF_FFFF_FFFF;
        if self.seed == 0 {
            self.seed = 1;
        }
    }
}

/// Duration, step size and history budget (spec §6).
#[derive(Clone, Debug, PartialEq)]
pub struct TimeParams {
    /// Total geological duration to simulate, years.
    pub total_yr: f64,
    /// Base time step, years. Processes scale or sub-step from this.
    pub step_yr: f64,
    /// Hard iteration cap.
    pub max_iters: u32,
    /// Halve `dt` automatically when a step fails the mass-balance check.
    pub adaptive_dt: bool,
    /// Apply diffusion-type CFL limits to process sub-stepping.
    pub cfl: bool,
    /// Number of timeline frames kept for scrubbing (including t=0).
    pub history_frames: u32,
    /// Interval between plate-mosaic rebuilds, years.
    pub tectonic_interval_yr: f64,
    /// Interval between climate recomputations, years.
    pub climate_interval_yr: f64,
    /// Interval between flow-routing refreshes, years.
    pub hydro_interval_yr: f64,
    /// Interval between isostatic relaxation updates, years.
    pub isostasy_interval_yr: f64,
}

impl Default for TimeParams {
    fn default() -> Self {
        Self {
            total_yr: 2_000_000.0,
            step_yr: 10_000.0,
            max_iters: 2000,
            adaptive_dt: true,
            cfl: true,
            history_frames: 48,
            tectonic_interval_yr: 200_000.0,
            climate_interval_yr: 100_000.0,
            hydro_interval_yr: 50_000.0,
            isostasy_interval_yr: 20_000.0,
        }
    }
}

impl TimeParams {
    /// Number of base steps needed to reach `total_yr`.
    #[must_use]
    pub fn steps_needed(&self) -> u64 {
        if self.step_yr <= 0.0 {
            return 1;
        }
        (self.total_yr / self.step_yr).ceil().max(1.0) as u64
    }

    /// Years between timeline frames.
    #[must_use]
    pub fn frame_interval_yr(&self) -> f64 {
        if self.history_frames <= 1 {
            self.total_yr
        } else {
            self.total_yr / f64::from(self.history_frames - 1)
        }
    }

    /// A display duration such as `2.0 Myr`.
    #[must_use]
    pub fn summary(&self) -> String {
        format_duration(self.total_yr)
    }
}

/// Formats a duration in years using geological units.
#[must_use]
pub fn format_duration(years: f64) -> String {
    let y = years.abs();
    if y >= 1e9 {
        format!("{:.2} Gyr", years / 1e9)
    } else if y >= 1e6 {
        format!("{:.2} Myr", years / 1e6)
    } else if y >= 1e3 {
        format!("{:.1} kyr", years / 1e3)
    } else {
        format!("{years:.0} yr")
    }
}

impl ParamBlock for TimeParams {
    fn specs() -> &'static [ParamSpec] {
        const SPECS: [ParamSpec; 10] = [
            ParamSpec::log(
                "total_yr",
                "Duration",
                "シミュレーション期間",
                ParamGroup::Time,
                "yr",
                1000.0,
                5e8,
                2e6,
                "Total geological time to simulate.",
            ),
            ParamSpec::log(
                "step_yr",
                "Time step",
                "時間刻み",
                ParamGroup::Time,
                "yr",
                10.0,
                100_000.0,
                10_000.0,
                "Base step; processes sub-step or run on their own schedule.",
            ),
            ParamSpec::int(
                "max_iters",
                "Max iterations",
                "最大反復",
                ParamGroup::Time,
                "",
                1.0,
                500_000.0,
                2000.0,
                "Safety cap on simulation steps.",
            ),
            ParamSpec::boolean(
                "adaptive_dt",
                "Adaptive step",
                "適応刻み",
                ParamGroup::Time,
                true,
                "Halve dt when a step fails the conservation check.",
            ),
            ParamSpec::boolean(
                "cfl",
                "CFL limiting",
                "CFL制限",
                ParamGroup::Time,
                true,
                "Restrict diffusion-type sub-steps for numerical stability.",
            ),
            ParamSpec::int(
                "history_frames",
                "History frames",
                "履歴フレーム",
                ParamGroup::Time,
                "",
                2.0,
                128.0,
                48.0,
                "Timeline snapshots kept in memory for scrubbing.",
            ),
            ParamSpec::log(
                "tectonic_interval_yr",
                "Plate rebuild interval",
                "プレート更新間隔",
                ParamGroup::Time,
                "yr",
                1000.0,
                2e6,
                1e5,
                "How often the plate mosaic is rebuilt. Rebuilds are O(cells × plates).",
            ),
            ParamSpec::log(
                "climate_interval_yr",
                "Climate interval",
                "気候更新間隔",
                ParamGroup::Time,
                "yr",
                1000.0,
                2e5,
                4e4,
                "How often temperature, moisture, biomes and vegetation refresh.",
            ),
            ParamSpec::log(
                "hydro_interval_yr",
                "Routing interval",
                "流路更新間隔",
                ParamGroup::Time,
                "yr",
                500.0,
                1e5,
                5e4,
                "How often depression filling and flow routing refresh.",
            ),
            ParamSpec::log(
                "isostasy_interval_yr",
                "Isostasy interval",
                "アイソスタシー更新間隔",
                ParamGroup::Time,
                "yr",
                1000.0,
                2e5,
                2e4,
                "How often flexural isostasy relaxes; the tectonic rate is applied every step.",
            ),
        ];
        &SPECS
    }

    fn values_into(&self, out: &mut Vec<f64>) {
        out.push(self.total_yr);
        out.push(self.step_yr);
        out.push(f64::from(self.max_iters));
        out.push(if self.adaptive_dt { 1.0 } else { 0.0 });
        out.push(if self.cfl { 1.0 } else { 0.0 });
        out.push(f64::from(self.history_frames));
        out.push(self.tectonic_interval_yr);
        out.push(self.climate_interval_yr);
        out.push(self.hydro_interval_yr);
        out.push(self.isostasy_interval_yr);
    }

    fn set_values(&mut self, v: &[f64]) {
        if let Some(&x) = v.first() {
            self.total_yr = x;
        }
        if let Some(&x) = v.get(1) {
            self.step_yr = x;
        }
        if let Some(&x) = v.get(2) {
            self.max_iters = x.round().max(1.0) as u32;
        }
        if let Some(&x) = v.get(3) {
            self.adaptive_dt = x >= 0.5;
        }
        if let Some(&x) = v.get(4) {
            self.cfl = x >= 0.5;
        }
        if let Some(&x) = v.get(5) {
            self.history_frames = x.round().max(2.0) as u32;
        }
        if let Some(&x) = v.get(6) {
            self.tectonic_interval_yr = x;
        }
        if let Some(&x) = v.get(7) {
            self.climate_interval_yr = x;
        }
        if let Some(&x) = v.get(8) {
            self.hydro_interval_yr = x;
        }
        if let Some(&x) = v.get(9) {
            self.isostasy_interval_yr = x;
        }
    }

    fn set_key(&mut self, key: &str, value: f64) -> bool {
        match key {
            "total_yr" => self.total_yr = value,
            "step_yr" => self.step_yr = value,
            "max_iters" => self.max_iters = value.round().max(1.0) as u32,
            "adaptive_dt" => self.adaptive_dt = value >= 0.5,
            "cfl" => self.cfl = value >= 0.5,
            "history_frames" => self.history_frames = value.round().max(2.0) as u32,
            "tectonic_interval_yr" => self.tectonic_interval_yr = value,
            "climate_interval_yr" => self.climate_interval_yr = value,
            "hydro_interval_yr" => self.hydro_interval_yr = value,
            "isostasy_interval_yr" => self.isostasy_interval_yr = value,
            _ => return false,
        }
        true
    }

    fn get_key(&self, key: &str) -> Option<f64> {
        Some(match key {
            "total_yr" => self.total_yr,
            "step_yr" => self.step_yr,
            "max_iters" => f64::from(self.max_iters),
            "adaptive_dt" => f64::from(u8::from(self.adaptive_dt)),
            "cfl" => f64::from(u8::from(self.cfl)),
            "history_frames" => f64::from(self.history_frames),
            "tectonic_interval_yr" => self.tectonic_interval_yr,
            "climate_interval_yr" => self.climate_interval_yr,
            "hydro_interval_yr" => self.hydro_interval_yr,
            "isostasy_interval_yr" => self.isostasy_interval_yr,
            _ => return None,
        })
    }

    fn normalize(&mut self) {
        self.total_yr = self.total_yr.clamp(1000.0, 5e8);
        self.step_yr = self.step_yr.clamp(10.0, 100_000.0);
        self.max_iters = self.max_iters.clamp(1, 500_000);
        if self.step_yr > self.total_yr {
            self.step_yr = self.total_yr;
        }
        let needed = self.steps_needed();
        if needed > u64::from(self.max_iters) {
            // Raise the cap when that is enough, otherwise stretch the step.
            if needed <= 500_000 {
                self.max_iters = needed as u32;
            } else {
                self.step_yr = (self.total_yr / f64::from(self.max_iters)).max(10.0);
            }
        }
        self.history_frames = self.history_frames.clamp(2, 128);
        self.tectonic_interval_yr = self.tectonic_interval_yr.clamp(1000.0, 2e6);
        self.climate_interval_yr = self.climate_interval_yr.clamp(1000.0, 2e5);
        self.hydro_interval_yr = self.hydro_interval_yr.clamp(500.0, 1e5);
        self.isostasy_interval_yr = self.isostasy_interval_yr.clamp(1000.0, 2e5);
        if u64::from(self.history_frames) > self.steps_needed() + 1 {
            self.history_frames = (self.steps_needed() + 1).min(128) as u32;
        }
    }

    fn validate(&self) -> GeoResult<()> {
        if !self.total_yr.is_finite() || self.total_yr <= 0.0 {
            return Err(GeoError::param(
                "total_yr",
                self.total_yr,
                "must be positive",
            ));
        }
        if !self.step_yr.is_finite() || self.step_yr <= 0.0 {
            return Err(GeoError::param("step_yr", self.step_yr, "must be positive"));
        }
        if self.step_yr > self.total_yr {
            return Err(GeoError::param(
                "step_yr",
                self.step_yr,
                "must not exceed total_yr",
            ));
        }
        if self.max_iters == 0 {
            return Err(GeoError::param("max_iters", self.max_iters, "must be ≥ 1"));
        }
        if self.history_frames < 2 {
            return Err(GeoError::param(
                "history_frames",
                self.history_frames,
                "must be at least 2",
            ));
        }
        Ok(())
    }

    fn randomize(&mut self, rng: &mut Rng) {
        // Pick a duration and aim for roughly 120–300 steps, which keeps a
        // browser run interactive while still resolving the process timescales.
        let total = (rng.range_f64(4.0, 7.0)).exp(); // e^4..e^7 ≈ 55 kyr..1.1 Myr
        self.total_yr = (total * 1e5).clamp(1e5, 2e7);
        let target_steps = rng.range_f64(120.0, 300.0);
        self.step_yr = (self.total_yr / target_steps).clamp(100.0, 100_000.0);
        self.max_iters = (self.steps_needed() + 16) as u32;
        self.adaptive_dt = true;
        self.cfl = true;
        self.history_frames = 40;
        self.tectonic_interval_yr = 200_000.0;
        self.climate_interval_yr = 100_000.0;
        self.hydro_interval_yr = 50_000.0;
        self.isostasy_interval_yr = 20_000.0;
        self.normalize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::ParamBlock;

    #[test]
    fn default_world_is_valid_and_builds_a_grid() {
        let p = WorldParams::default();
        p.validate().unwrap();
        let g = p.grid().unwrap();
        assert_eq!(g.width(), 384);
        assert_eq!(g.cell_size_m(), 10_000.0);
        assert_eq!(g.boundary(), BoundaryKind::Toroidal);
        assert_eq!(p.extent_x_km(), 3840.0);
    }

    #[test]
    fn zero_seed_maps_to_default_seed() {
        let mut p = WorldParams::default();
        p.seed = 0;
        assert_eq!(p.effective_seed(), DEFAULT_SEED);
    }

    #[test]
    fn spherical_worlds_carry_the_planet_radius_into_the_grid() {
        let mut planar = WorldParams::default();
        planar.boundary = BoundaryKind::Toroidal;
        let g = planar.grid().unwrap();
        assert_eq!(g.planet_radius_m(), None);
        assert!(!g.has_spherical_areas());
        assert_eq!(planar.planet_radius_m(), 6_371_000.0);

        let mut spherical = WorldParams::default();
        spherical.boundary = BoundaryKind::Spherical;
        spherical.planet_radius_km = 3_389.5; // Mars
        let g = spherical.grid().unwrap();
        assert_eq!(g.planet_radius_m(), Some(3_389_500.0));
        assert!(g.has_spherical_areas());
        let sphere = 4.0 * std::f64::consts::PI * 3_389_500.0_f64.powi(2);
        assert!((g.total_area_m2() - sphere).abs() / sphere < 1e-15);
        // And the physical cell areas are no longer the planar ones.
        let planar_area = spherical.cell_size_m() * spherical.cell_size_m();
        assert!(g.cell_area_m2_at(0, 0) > 0.0);
        assert_ne!(
            g.cell_area_m2_at(g.width() / 2, g.height() / 2),
            planar_area
        );

        // A non-positive radius on a spherical world is rejected, not silently
        // treated as planar.
        spherical.planet_radius_km = 0.0;
        assert!(spherical.grid().is_err());
    }

    #[test]
    fn world_values_roundtrip_through_block_interface() {
        let p = WorldParams::default();
        let mut v = Vec::new();
        p.values_into(&mut v);
        assert_eq!(v.len(), WorldParams::specs().len());
        let mut q = WorldParams::default();
        q.set_values(&v);
        assert_eq!(p, q);
        assert_eq!(p.get_key("width"), Some(384.0));
    }

    #[test]
    fn time_normalize_reconciles_step_and_cap() {
        let mut t = TimeParams {
            total_yr: 5e6,
            step_yr: 1000.0,
            max_iters: 10,
            adaptive_dt: true,
            cfl: true,
            history_frames: 200,
            tectonic_interval_yr: 100_000.0,
            climate_interval_yr: 40_000.0,
            hydro_interval_yr: 20_000.0,
            isostasy_interval_yr: 20_000.0,
        };
        t.normalize();
        assert_eq!(t.steps_needed(), 5000);
        assert!(t.max_iters >= 5000);
        assert!(t.history_frames <= 128);
        t.validate().unwrap();
    }

    #[test]
    fn time_rejects_step_longer_than_duration() {
        let t = TimeParams {
            total_yr: 1000.0,
            step_yr: 2000.0,
            ..Default::default()
        };
        assert!(t.validate().is_err());
    }

    #[test]
    fn duration_formatting() {
        assert_eq!(format_duration(500.0), "500 yr");
        assert_eq!(format_duration(2.5e6), "2.50 Myr");
        assert_eq!(format_duration(1.2e9), "1.20 Gyr");
    }

    #[test]
    fn random_world_and_time_are_coherent() {
        let mut rng = Rng::new(8);
        let mut w = WorldParams::default();
        let mut t = TimeParams::default();
        w.randomize(&mut rng);
        t.randomize(&mut rng);
        w.validate().unwrap();
        t.validate().unwrap();
        assert!(t.steps_needed() <= u64::from(t.max_iters));
    }
}
