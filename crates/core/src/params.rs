//! Simulation parameters with spec-exact ranges (Phases 0–6).
//!
//! Every field documents its spec range. [`Params::validate`] enforces all of
//! them; [`Params::clamp_to_spec`] repairs a UI-driven struct instead of
//! rejecting it.

use crate::climate::{ClimateParams, VegetationParams};
use crate::erosion::chemical::ChemicalParams;
use crate::erosion::coastal::CoastalParams;
use crate::erosion::fluvial::{FluvialParams, TransportLaw};
use crate::erosion::glacial::GlacialParams;
use crate::erosion::hillslope::HillslopeParams;
use crate::erosion::rainfall::RainfallParams;
use crate::erosion::wind::WindParams;
use crate::geology::RockKind;
use crate::noise::{Anisotropy, FractalKind, NoiseKind, NoiseParams};
use crate::plates::PlateParams;
use crate::uplift::UpliftParams;
use crate::world::WorldConfig;
use crate::GeoError;

/// Simulation mode (spec §6c).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SimMode {
    #[default]
    OneShot,
    Stepwise,
    Manual,
    Realtime,
    Batch,
}

/// Full parameter set: world + every process.
#[derive(Debug, Clone, PartialEq)]
pub struct Params {
    pub world: WorldConfig,
    pub noise: NoiseParams,
    /// 0.0 … 1000 m fault offset at plate boundaries.
    pub fault_offset_m: f64,
    /// 1.0 … 500 km fold wavelength.
    pub fold_wavelength_km: f64,
    /// 0.0 … 5000 m fold amplitude.
    pub fold_amplitude_m: f64,
    pub plates: PlateParams,
    pub uplift: UpliftParams,
    pub rain: RainfallParams,
    pub fluvial: FluvialParams,
    pub hillslope: HillslopeParams,
    pub wind: WindParams,
    pub glacial: GlacialParams,
    pub coastal: CoastalParams,
    pub chemical: ChemicalParams,
    pub climate: ClimateParams,
    pub vegetation: VegetationParams,
    pub rock: RockKind,
    /// 1000 … 5e8 yr total simulated time.
    pub total_yr: f64,
    /// 1 … 100_000 yr base timestep.
    pub step_yr: f64,
    pub adaptive_dt: bool,
    pub cfl_limit: bool,
    /// 1 … 1,000,000 iterations.
    pub max_iters: u32,
    pub mode: SimMode,
    /// How often each process runs, in base steps (spec §6b).
    pub schedule: Schedule,
}

/// Per-process application intervals in base steps (spec §6b).
#[derive(Debug, Clone, PartialEq)]
pub struct Schedule {
    pub tectonic: u32,
    pub fluvial: u32,
    pub hillslope: u32,
    pub glacial: u32,
    pub wind: u32,
    pub climate: u32,
    pub vegetation: u32,
    pub sea_level: u32,
}

impl Default for Schedule {
    fn default() -> Self {
        Self {
            tectonic: 1,
            fluvial: 1,
            hillslope: 3,
            glacial: 30,
            wind: 10,
            climate: 300,
            vegetation: 100,
            sea_level: 3000,
        }
    }
}

impl Default for Params {
    fn default() -> Self {
        Self {
            world: WorldConfig::default(),
            noise: NoiseParams::default(),
            fault_offset_m: 200.0,
            fold_wavelength_km: 60.0,
            fold_amplitude_m: 800.0,
            plates: PlateParams::default(),
            uplift: UpliftParams::default(),
            rain: RainfallParams::default(),
            fluvial: FluvialParams::default(),
            hillslope: HillslopeParams::default(),
            wind: WindParams::default(),
            glacial: GlacialParams::default(),
            coastal: CoastalParams::default(),
            chemical: ChemicalParams::default(),
            climate: ClimateParams::default(),
            vegetation: VegetationParams::default(),
            rock: RockKind::Granite,
            total_yr: 1e6,
            step_yr: 1000.0,
            adaptive_dt: true,
            cfl_limit: true,
            max_iters: 100_000,
            mode: SimMode::OneShot,
            schedule: Schedule::default(),
        }
    }
}

fn check(name: &'static str, v: f64, lo: f64, hi: f64) -> Result<(), GeoError> {
    if !v.is_finite() || v < lo || v > hi {
        return Err(GeoError::InvalidParameter {
            name,
            value: format!("{v}"),
            reason: format!("outside {lo}…{hi}"),
        });
    }
    Ok(())
}

fn check_u32(name: &'static str, v: u32, lo: u32, hi: u32) -> Result<(), GeoError> {
    if v < lo || v > hi {
        return Err(GeoError::InvalidParameter {
            name,
            value: format!("{v}"),
            reason: format!("outside {lo}…{hi}"),
        });
    }
    Ok(())
}

impl Params {
    /// Enforces every spec range. Syncs derived fields first so a hand-built
    /// struct validates the way the pipeline actually runs it.
    pub fn validate(&mut self) -> Result<(), GeoError> {
        self.world.validate()?;
        self.sync_derived();

        check("octaves", self.noise.octaves as f64, 1.0, 16.0)?;
        check("lacunarity", self.noise.lacunarity, 1.0, 4.0)?;
        check("persistence", self.noise.persistence, 0.1, 1.0)?;
        check("anisotropy", self.noise.anisotropy.amount, 0.0, 1.0)?;
        check("fault_offset_m", self.fault_offset_m, 0.0, 1000.0)?;
        check("fold_wavelength_km", self.fold_wavelength_km, 1.0, 500.0)?;
        check("fold_amplitude_m", self.fold_amplitude_m, 0.0, 5000.0)?;

        check_u32("plates", self.plates.count, 1, 32)?;
        check("thickness_km", self.plates.thickness_km, 5.0, 200.0)?;
        for (i, d) in self.plates.densities.iter().enumerate() {
            check("plate_density", *d, 2.7, 3.3).map_err(|_| GeoError::InvalidParameter {
                name: "plate_density",
                value: format!("{d}"),
                reason: format!("density[{i}] outside 2.7…3.3"),
            })?;
        }
        check("collision_threshold", self.plates.collision_threshold, 0.3, 0.8)?;

        check("uplift_mm_yr", self.uplift.mean_rate_mm_yr, -10.0, 50.0)?;
        check("uplift_noise_freq", self.uplift.noise_frequency, 0.001, 1.0)?;
        check("flexural_rigidity", self.uplift.flexural_rigidity_nm, 1e20, 1e25)?;
        check_u32("hotspots", self.uplift.hotspot_count, 0, 50)?;
        check("hotspot_strength", self.uplift.hotspot_strength_m, 0.0, 5000.0)?;

        check("rain_mm", self.rain.annual_mm, 0.0, 10_000.0)?;
        check("heterogeneity", self.rain.heterogeneity, 0.0, 1.0)?;
        check("wind_dir", self.rain.wind_dir_deg, 0.0, 360.0)?;
        check("rain_shadow", self.rain.rain_shadow_strength, 0.0, 1.0)?;
        check("elev_dependence", self.rain.elevation_dependence, -1.0, 1.0)?;
        check("seasonality", self.rain.seasonality, 0.0, 1.0)?;

        check("kf", self.fluvial.kf, 1e-8, 1e-3)?;
        check("kd", self.fluvial.kd, 1e-5, 1.0)?;
        check_u32("channel_cells", self.fluvial.min_channel_area_cells, 1, 1000)?;
        check("slope_exp", self.fluvial.slope_exponent, 0.2, 0.7)?;
        check("width_coef", self.fluvial.width_coef, 0.1, 10.0)?;
        check("roughness", self.fluvial.roughness, 0.01, 1.0)?;
        check("infiltration", self.fluvial.infiltration, 0.0, 1.0)?;

        check("diffusivity", self.hillslope.diffusivity_m2_yr, 1e-6, 1e-1)?;
        check("repose", self.hillslope.repose_angle_deg, 10.0, 70.0)?;
        check("soil_prod", self.hillslope.soil_production_m_yr, 1e-6, 1e-2)?;
        check("max_soil", self.hillslope.max_soil_m, 0.1, 50.0)?;
        check("runout", self.hillslope.landslide_runout_m, 1.0, 500.0)?;

        check("wind_speed", self.wind.wind_speed_ms, 0.0, 50.0)?;
        check("aeolian_dir", self.wind.wind_dir_deg, 0.0, 360.0)?;
        check("erosivity", self.wind.erosivity, 0.0, 1.0)?;
        check("sand_flux", self.wind.sand_flux, 0.0, 1.0)?;
        check("shelter", self.wind.vegetation_shelter, 0.0, 1.0)?;

        check("glacial_temp", self.glacial.mean_temp_c, -50.0, 50.0)?;
        check("lapse", self.glacial.lapse_rate_c_km, 4.0, 10.0)?;
        check("ice_flow", self.glacial.flow_coef, 1e-9, 1e-6)?;
        check("glacial_k", self.glacial.erosion_coef, 1e-7, 1e-4)?;
        check("ela", self.glacial.ela_m, 0.0, 8000.0)?;
        check("u_valley", self.glacial.u_valley_strength, 0.0, 1.0)?;

        check("wave_k", self.coastal.wave_erosivity, 0.0, 1.0)?;
        check("tide", self.coastal.tidal_range_m, 0.0, 20.0)?;
        check("longshore", self.coastal.longshore_m3_yr, 0.0, 1e6)?;
        check("sl_change", self.coastal.sea_level_change_mm_yr, -10.0, 10.0)?;

        check("dissolution", self.chemical.dissolution, 0.0, 1.0)?;
        check("reactivity", self.chemical.reactivity, 0.0, 1.0)?;
        check("co2", self.chemical.co2_pct, 0.0, 0.1)?;

        check("latitude", self.climate.latitude_deg, -90.0, 90.0)?;
        check("mean_temp", self.climate.mean_temp_c, -50.0, 50.0)?;
        check("diurnal", self.climate.diurnal_range_c, 0.0, 30.0)?;
        check("annual_range", self.climate.annual_range_c, 0.0, 50.0)?;
        check("ocean_mod", self.climate.ocean_moderation, 0.0, 1.0)?;
        check("continentality", self.climate.continentality, 0.0, 1.0)?;

        check("veg_density", self.vegetation.density, 0.0, 1.0)?;
        check("root_depth", self.vegetation.root_depth_m, 0.1, 50.0)?;
        check("root_strength", self.vegetation.root_strength, 0.0, 1.0)?;
        check("litter", self.vegetation.litter_m, 0.0, 1.0)?;
        check("productivity", self.vegetation.productivity, 0.0, 10.0)?;
        check("succession", self.vegetation.succession, 0.0, 1.0)?;

        check("total_yr", self.total_yr, 1000.0, 5e8)?;
        check("step_yr", self.step_yr, 1.0, 100_000.0)?;
        check_u32("max_iters", self.max_iters, 1, 1_000_000)?;
        Ok(())
    }

    /// Copies world-derived quantities into the process params so a hand-built
    /// struct behaves like a UI-built one (time span, extents, lapse rate).
    pub fn sync_derived(&mut self) {
        let extent_m = self.world.spatial_km * 1000.0;
        self.uplift.time_span_yr = self.total_yr;
        self.uplift.spatial_extent_m = extent_m;
        self.uplift.vertical_scale_m = self.world.vertical_m;
        self.glacial.lapse_rate_c_km = self.glacial.lapse_rate_c_km.clamp(4.0, 10.0);
    }

    /// Repairs out-of-range values in place (UI path).
    pub fn clamp_to_spec(&mut self) {
        let c = |v: &mut f64, lo: f64, hi: f64| {
            if !v.is_finite() {
                *v = lo;
            }
            *v = v.clamp(lo, hi);
        };
        c(&mut self.world.spatial_km, 1.0, 10_000.0);
        c(&mut self.world.vertical_m, 100.0, 50_000.0);
        c(&mut self.world.sea_level_m, -5000.0, 5000.0);
        c(&mut self.noise.lacunarity, 1.0, 4.0);
        c(&mut self.noise.persistence, 0.1, 1.0);
        c(&mut self.noise.anisotropy.amount, 0.0, 1.0);
        self.noise.octaves = self.noise.octaves.clamp(1, 16);
        self.noise.anisotropy.direction = self.noise.anisotropy.direction.rem_euclid(360.0);
        c(&mut self.fault_offset_m, 0.0, 1000.0);
        c(&mut self.fold_wavelength_km, 1.0, 500.0);
        c(&mut self.fold_amplitude_m, 0.0, 5000.0);
        self.plates.count = self.plates.count.clamp(1, 32);
        c(&mut self.plates.thickness_km, 5.0, 200.0);
        c(&mut self.plates.collision_threshold, 0.3, 0.8);
        c(&mut self.uplift.mean_rate_mm_yr, -10.0, 50.0);
        c(&mut self.uplift.noise_frequency, 0.001, 1.0);
        c(&mut self.uplift.flexural_rigidity_nm, 1e20, 1e25);
        self.uplift.hotspot_count = self.uplift.hotspot_count.min(50);
        c(&mut self.uplift.hotspot_strength_m, 0.0, 5000.0);
        c(&mut self.rain.annual_mm, 0.0, 10_000.0);
        c(&mut self.rain.heterogeneity, 0.0, 1.0);
        c(&mut self.rain.wind_dir_deg, 0.0, 360.0);
        c(&mut self.rain.rain_shadow_strength, 0.0, 1.0);
        c(&mut self.rain.elevation_dependence, -1.0, 1.0);
        c(&mut self.rain.seasonality, 0.0, 1.0);
        c(&mut self.fluvial.kf, 1e-8, 1e-3);
        c(&mut self.fluvial.kd, 1e-5, 1.0);
        self.fluvial.min_channel_area_cells = self.fluvial.min_channel_area_cells.clamp(1, 1000);
        c(&mut self.fluvial.slope_exponent, 0.2, 0.7);
        c(&mut self.fluvial.width_coef, 0.1, 10.0);
        c(&mut self.fluvial.roughness, 0.01, 1.0);
        c(&mut self.fluvial.infiltration, 0.0, 1.0);
        c(&mut self.hillslope.diffusivity_m2_yr, 1e-6, 1e-1);
        c(&mut self.hillslope.repose_angle_deg, 10.0, 70.0);
        c(&mut self.hillslope.soil_production_m_yr, 1e-6, 1e-2);
        c(&mut self.hillslope.max_soil_m, 0.1, 50.0);
        c(&mut self.hillslope.landslide_runout_m, 1.0, 500.0);
        c(&mut self.wind.wind_speed_ms, 0.0, 50.0);
        c(&mut self.wind.wind_dir_deg, 0.0, 360.0);
        c(&mut self.wind.erosivity, 0.0, 1.0);
        c(&mut self.wind.sand_flux, 0.0, 1.0);
        c(&mut self.wind.vegetation_shelter, 0.0, 1.0);
        c(&mut self.glacial.mean_temp_c, -50.0, 50.0);
        c(&mut self.glacial.lapse_rate_c_km, 4.0, 10.0);
        c(&mut self.glacial.flow_coef, 1e-9, 1e-6);
        c(&mut self.glacial.erosion_coef, 1e-7, 1e-4);
        c(&mut self.glacial.ela_m, 0.0, 8000.0);
        c(&mut self.glacial.u_valley_strength, 0.0, 1.0);
        c(&mut self.coastal.wave_erosivity, 0.0, 1.0);
        c(&mut self.coastal.tidal_range_m, 0.0, 20.0);
        c(&mut self.coastal.longshore_m3_yr, 0.0, 1e6);
        c(&mut self.coastal.sea_level_change_mm_yr, -10.0, 10.0);
        c(&mut self.chemical.dissolution, 0.0, 1.0);
        c(&mut self.chemical.reactivity, 0.0, 1.0);
        c(&mut self.chemical.co2_pct, 0.0, 0.1);
        c(&mut self.climate.latitude_deg, -90.0, 90.0);
        c(&mut self.climate.mean_temp_c, -50.0, 50.0);
        c(&mut self.climate.diurnal_range_c, 0.0, 30.0);
        c(&mut self.climate.annual_range_c, 0.0, 50.0);
        c(&mut self.climate.ocean_moderation, 0.0, 1.0);
        c(&mut self.climate.continentality, 0.0, 1.0);
        c(&mut self.vegetation.density, 0.0, 1.0);
        c(&mut self.vegetation.root_depth_m, 0.1, 50.0);
        c(&mut self.vegetation.root_strength, 0.0, 1.0);
        c(&mut self.vegetation.litter_m, 0.0, 1.0);
        c(&mut self.vegetation.productivity, 0.0, 10.0);
        c(&mut self.vegetation.succession, 0.0, 1.0);
        c(&mut self.total_yr, 1000.0, 5e8);
        c(&mut self.step_yr, 1.0, 100_000.0);
        self.max_iters = self.max_iters.clamp(1, 1_000_000);
        self.sync_derived();
    }
}

/// Builds a [`NoiseParams`] with spec defaults for a given kind.
pub fn noise_preset(kind: NoiseKind, fractal: FractalKind) -> NoiseParams {
    NoiseParams {
        kind,
        fractal,
        ..Default::default()
    }
}

/// Builds [`Anisotropy`] from amount and direction degrees.
pub fn anisotropy(amount: f64, direction_deg: f64) -> Anisotropy {
    Anisotropy {
        amount: amount.clamp(0.0, 1.0),
        direction: direction_deg.to_radians(),
    }
}

/// Selects a [`TransportLaw`] by name for the CLI and UI.
pub fn transport_law_named(name: &str) -> Option<TransportLaw> {
    match name {
        "sp" | "stream-power" => Some(TransportLaw::StreamPower),
        "modified" | "modified-sp" => Some(TransportLaw::ModifiedStreamPower),
        "damp-brown" | "damped" => Some(TransportLaw::DampedBrownian),
        "tucker" | "tucker-slingerland" => Some(TransportLaw::TuckerSlingerland),
        "full" | "physics" => Some(TransportLaw::FullPhysics),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_params_validate() {
        let mut p = Params::default();
        p.validate().expect("default params invalid");
    }

    #[test]
    fn each_spec_range_is_enforced() {
        let mut p = Params::default();
        p.fluvial.kf = 1.0;
        assert!(p.validate().is_err());
        p.fluvial.kf = 3e-6;
        p.hillslope.repose_angle_deg = 5.0;
        assert!(p.validate().is_err());
        p.hillslope.repose_angle_deg = 35.0;
        p.total_yr = 10.0;
        assert!(p.validate().is_err());
    }

    #[test]
    fn clamp_repairs_without_rejecting() {
        let mut p = Params::default();
        p.fluvial.kf = 5.0;
        p.rain.annual_mm = -100.0;
        p.noise.octaves = 99;
        p.clamp_to_spec();
        p.validate().expect("clamp did not repair");
    }

    #[test]
    fn clamp_handles_nan() {
        let mut p = Params::default();
        p.fluvial.kf = f64::NAN;
        p.clamp_to_spec();
        assert!(p.fluvial.kf.is_finite());
    }

    #[test]
    fn sync_derived_copies_extents() {
        let mut p = Params::default();
        p.world.spatial_km = 500.0;
        p.world.vertical_m = 8000.0;
        p.total_yr = 2e6;
        p.sync_derived();
        assert_eq!(p.uplift.spatial_extent_m, 500_000.0);
        assert_eq!(p.uplift.vertical_scale_m, 8000.0);
        assert_eq!(p.uplift.time_span_yr, 2e6);
    }

    #[test]
    fn transport_law_names_resolve() {
        assert_eq!(transport_law_named("sp"), Some(TransportLaw::StreamPower));
        assert_eq!(transport_law_named("full"), Some(TransportLaw::FullPhysics));
        assert_eq!(transport_law_named("nope"), None);
    }

    #[test]
    fn schedule_defaults_match_spec() {
        let s = Schedule::default();
        assert_eq!(s.tectonic, 1);
        assert_eq!(s.fluvial, 1);
        assert!(s.sea_level >= 1000);
    }
}