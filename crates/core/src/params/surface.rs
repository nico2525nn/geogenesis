//! Hydrology, river and hillslope parameter blocks (spec §3b–3c).

use crate::math::rng::Rng;
use crate::params::spec::{ParamGroup, ParamSpec};

use super::ParamBlock;

/// Flow routing and runoff generation (spec §3a–3b).
#[derive(Clone, Debug, PartialEq)]
pub struct HydrologyParams {
    /// Fraction of precipitation that infiltrates rather than runs off.
    pub infiltration: f64,
    /// Evapotranspiration as a fraction of precipitation.
    pub evapotranspiration_factor: f64,
    /// Groundwater baseflow as a fraction of infiltration.
    pub groundwater_baseflow: f64,
    /// Multiple-flow-direction concentration exponent (1 = uniform).
    pub mfd_exponent: f64,
    /// Whether depressions are filled/lake-routed before flow routing.
    pub depression_fill: bool,
    /// Evaporation fraction of standing lake water per climate interval.
    pub lake_evaporation: f64,
    /// Minimum catchment area, in cells, for channel initiation.
    pub channel_init_cells: u32,
    /// Flow router: D8 or MFD.
    pub routing: f64,
}

impl Default for HydrologyParams {
    fn default() -> Self {
        Self {
            infiltration: 0.35,
            evapotranspiration_factor: 0.4,
            groundwater_baseflow: 0.2,
            mfd_exponent: 1.1,
            depression_fill: true,
            lake_evaporation: 0.3,
            channel_init_cells: 20,
            routing: 0.0,
        }
    }
}

impl HydrologyParams {
    /// Fraction of rainfall that becomes surface runoff.
    #[must_use]
    pub fn runoff_fraction(&self) -> f64 {
        (1.0 - self.infiltration).clamp(0.0, 1.0)
    }

    /// True when multiple-flow-direction routing is selected.
    #[must_use]
    pub fn uses_mfd(&self) -> bool {
        self.routing.round() >= 0.5
    }
}

impl ParamBlock for HydrologyParams {
    fn specs() -> &'static [ParamSpec] {
        const SPECS: [ParamSpec; 8] = [
            ParamSpec::float(
                "infiltration",
                "Infiltration",
                "浸透率",
                ParamGroup::Hydrology,
                "",
                0.0,
                1.0,
                0.35,
                "Share of rainfall absorbed by soil instead of running off.",
            ),
            ParamSpec::float(
                "evapotranspiration_factor",
                "Evapotranspiration",
                "蒸発散",
                ParamGroup::Hydrology,
                "",
                0.0,
                1.0,
                0.4,
                "Share of precipitation returned to the atmosphere.",
            ),
            ParamSpec::float(
                "groundwater_baseflow",
                "Baseflow",
                "基底流出",
                ParamGroup::Hydrology,
                "",
                0.0,
                1.0,
                0.2,
                "Infiltration released as steady groundwater flow.",
            ),
            ParamSpec::float(
                "mfd_exponent",
                "MFD exponent",
                "MFD指数",
                ParamGroup::Hydrology,
                "",
                0.1,
                8.0,
                1.1,
                "Flow concentration for multiple-flow-direction routing.",
            ),
            ParamSpec::boolean(
                "depression_fill",
                "Fill depressions",
                "窪地充填",
                ParamGroup::Hydrology,
                true,
                "Fill closed basins so rivers can cross them as lakes.",
            ),
            ParamSpec::float(
                "lake_evaporation",
                "Lake evaporation",
                "湖面蒸発",
                ParamGroup::Hydrology,
                "",
                0.0,
                1.0,
                0.3,
                "Evaporation from standing water bodies.",
            ),
            ParamSpec::int(
                "channel_init_cells",
                "Channel initiation",
                "河道発生",
                ParamGroup::Hydrology,
                "cells",
                1.0,
                1000.0,
                20.0,
                "Minimum contributing area that forms a channel.",
            ),
            ParamSpec::enumerated(
                "routing",
                "Flow routing",
                "流路計算",
                ParamGroup::Hydrology,
                &["D8 (single)", "MFD (multiple)"],
                0.0,
                "D8 gives sharp channels; MFD spreads flow on hillslopes.",
            ),
        ];
        &SPECS
    }

    fn values_into(&self, out: &mut Vec<f64>) {
        out.extend_from_slice(&[
            self.infiltration,
            self.evapotranspiration_factor,
            self.groundwater_baseflow,
            self.mfd_exponent,
            f64::from(u8::from(self.depression_fill)),
            self.lake_evaporation,
            f64::from(self.channel_init_cells),
            self.routing,
        ]);
    }

    fn set_values(&mut self, v: &[f64]) {
        let g = |i: usize, d: f64| v.get(i).copied().unwrap_or(d);
        self.infiltration = g(0, 0.35);
        self.evapotranspiration_factor = g(1, 0.4);
        self.groundwater_baseflow = g(2, 0.2);
        self.mfd_exponent = g(3, 1.1);
        self.depression_fill = g(4, 1.0) >= 0.5;
        self.lake_evaporation = g(5, 0.3);
        self.channel_init_cells = g(6, 20.0).round().max(1.0) as u32;
        self.routing = g(7, 0.0).clamp(0.0, 1.0).round();
    }

    fn set_key(&mut self, key: &str, value: f64) -> bool {
        match key {
            "infiltration" => self.infiltration = value,
            "evapotranspiration_factor" => self.evapotranspiration_factor = value,
            "groundwater_baseflow" => self.groundwater_baseflow = value,
            "mfd_exponent" => self.mfd_exponent = value,
            "depression_fill" => self.depression_fill = value >= 0.5,
            "lake_evaporation" => self.lake_evaporation = value,
            "channel_init_cells" => {
                self.channel_init_cells = value.round().max(1.0) as u32;
            }
            "routing" => self.routing = value.round().clamp(0.0, 1.0),
            _ => return false,
        }
        true
    }

    fn get_key(&self, key: &str) -> Option<f64> {
        Some(match key {
            "infiltration" => self.infiltration,
            "evapotranspiration_factor" => self.evapotranspiration_factor,
            "groundwater_baseflow" => self.groundwater_baseflow,
            "mfd_exponent" => self.mfd_exponent,
            "depression_fill" => f64::from(u8::from(self.depression_fill)),
            "lake_evaporation" => self.lake_evaporation,
            "channel_init_cells" => f64::from(self.channel_init_cells),
            "routing" => self.routing,
            _ => return None,
        })
    }

    fn validate(&self) -> crate::error::GeoResult<()> {
        if !(self.routing == 0.0 || self.routing == 1.0) {
            return Err(crate::error::GeoError::InvalidConfig(
                "routing must be D8 (0) or MFD (1)".into(),
            ));
        }
        if self.infiltration + self.evapotranspiration_factor > 1.5 {
            return Err(crate::error::GeoError::InvalidConfig(
                "infiltration + evapotranspiration leaves no water for runoff".into(),
            ));
        }
        Ok(())
    }

    fn randomize(&mut self, rng: &mut Rng) {
        self.infiltration = rng.range_f64(0.15, 0.6);
        self.evapotranspiration_factor = rng.range_f64(0.2, 0.55);
        self.groundwater_baseflow = rng.range_f64(0.05, 0.35);
        self.channel_init_cells = rng.range_u32(8, 80);
        self.routing = 0.0;
        self.depression_fill = true;
    }
}

/// River incision and sediment transport (spec §3b).
#[derive(Clone, Debug, PartialEq)]
pub struct FluvialParams {
    /// Transport law index: 0 detachment-limited, 1 transport-limited, 2 SPACE.
    pub law: f64,
    /// Stream-power erodibility K (detachment) and capacity scale.
    pub kf: f64,
    /// Deposition coefficient: settling velocity for SPACE and settling rate
    /// for the transport-limited law.
    ///
    /// The depositional laws integrate settling exactly over the step with
    /// `φ = 1 - exp(-kd dt)`, so the value is stable for any time step; it is
    /// not a per-step clamp.
    pub kd: f64,
    /// Drainage-area exponent m.
    pub area_exponent: f64,
    /// Slope exponent n.
    pub slope_exponent: f64,
    /// Channel width scaling.
    pub channel_width_coeff: f64,
    /// Manning roughness for discharge-to-width conversion.
    pub manning_roughness: f64,
    /// Threshold stream power below which no incision occurs.
    pub incision_threshold: f64,
    /// Detachment rate relative to the transport capacity (K / Kt). Smaller
    /// values make the SPACE law more supply-limited.
    ///
    /// The implicit depositional solver uses this as the detachment ratio `λ`
    /// in `E = λ (C - Qs)` and to scale the transport capacity `Kt = K/λ`.
    pub transport_efficiency: f64,
    /// Bank/lateral erosion strength; channels undercut and redeposit banks.
    pub bank_erosion: f64,
    /// Cap on incision per step, metres.
    ///
    /// Used by the detachment-limited law as a user-facing safety bound. The
    /// depositional laws no longer need it: their implicit solve is bounded by
    /// the receiver elevation and the available sediment, so it is not applied
    /// there.
    pub max_incision_m: f64,
}

impl Default for FluvialParams {
    fn default() -> Self {
        Self {
            law: 0.0,
            kf: 1e-4,
            kd: 0.02,
            area_exponent: 0.5,
            slope_exponent: 1.0,
            channel_width_coeff: 1.0,
            manning_roughness: 0.05,
            incision_threshold: 0.0,
            transport_efficiency: 0.5,
            bank_erosion: 0.15,
            max_incision_m: 400.0,
        }
    }
}

impl FluvialParams {
    /// Names of the supported transport laws.
    pub const LAW_NAMES: [&'static str; 3] = [
        "Detachment-limited stream power",
        "Transport-limited",
        "SPACE (erosion–deposition)",
    ];

    /// Whether the law allows sediment deposition.
    #[must_use]
    pub fn deposits(&self) -> bool {
        self.law.round() >= 1.0
    }
}

impl ParamBlock for FluvialParams {
    fn specs() -> &'static [ParamSpec] {
        const SPECS: [ParamSpec; 11] = [
            ParamSpec::enumerated(
                "fluvial_law",
                "Transport law",
                "流砂モデル",
                ParamGroup::Fluvial,
                &FluvialParams::LAW_NAMES,
                0.0,
                "Detachment stream power, transport-limited, or SPACE erosion-deposition.",
            ),
            ParamSpec::log(
                "kf",
                "Erodibility Kf",
                "侵食係数 Kf",
                ParamGroup::Fluvial,
                "m^(1-2m)/yr",
                1e-8,
                1e-2,
                1e-4,
                "Stream-power erodibility: larger values cut deeper channels.",
            ),
            ParamSpec::log(
                "kd",
                "Deposition Kd",
                "堆積係数 Kd",
                ParamGroup::Fluvial,
                "",
                1e-5,
                1.0,
                0.02,
                "Settling rate for sediment that exceeds the transport capacity.",
            ),
            ParamSpec::float(
                "area_exponent",
                "Area exponent m",
                "面積指数 m",
                ParamGroup::Fluvial,
                "",
                0.2,
                0.7,
                0.5,
                "Drainage-area exponent in the stream-power law.",
            ),
            ParamSpec::float(
                "slope_exponent",
                "Slope exponent n",
                "勾配指数 n",
                ParamGroup::Fluvial,
                "",
                0.5,
                2.0,
                1.0,
                "Slope exponent; the implicit solver handles any value.",
            ),
            ParamSpec::float(
                "channel_width_coeff",
                "Channel width",
                "河道幅",
                ParamGroup::Fluvial,
                "×",
                0.1,
                10.0,
                1.0,
                "Scales channel width for a given discharge.",
            ),
            ParamSpec::float(
                "manning_roughness",
                "Manning roughness",
                "粗度係数",
                ParamGroup::Fluvial,
                "s/m^(1/3)",
                0.01,
                0.3,
                0.05,
                "Bed roughness controlling flow velocity and shear stress.",
            ),
            ParamSpec::float(
                "incision_threshold",
                "Incision threshold",
                "侵食閾値",
                ParamGroup::Fluvial,
                "",
                0.0,
                100.0,
                0.0,
                "Stream power that must be exceeded before incision.",
            ),
            ParamSpec::float(
                "transport_efficiency",
                "Transport efficiency",
                "輸送効率",
                ParamGroup::Fluvial,
                "",
                0.0,
                1.0,
                0.5,
                "Detachment rate relative to transport capacity (K / Kt).",
            ),
            ParamSpec::float(
                "bank_erosion",
                "Bank erosion",
                "側岸侵食",
                ParamGroup::Fluvial,
                "",
                0.0,
                1.0,
                0.15,
                "Lateral channel migration: banks are undercut and redeposited.",
            ),
            ParamSpec::float(
                "max_incision_m",
                "Max incision",
                "最大下刻",
                ParamGroup::Fluvial,
                "m",
                10.0,
                2000.0,
                400.0,
                "Safety cap on incision per step for the detachment-limited law; the implicit depositional laws are stable without it.",
            ),
        ];
        &SPECS
    }

    fn values_into(&self, out: &mut Vec<f64>) {
        out.extend_from_slice(&[
            self.law,
            self.kf,
            self.kd,
            self.area_exponent,
            self.slope_exponent,
            self.channel_width_coeff,
            self.manning_roughness,
            self.incision_threshold,
            self.transport_efficiency,
            self.bank_erosion,
            self.max_incision_m,
        ]);
    }

    fn set_values(&mut self, v: &[f64]) {
        let g = |i: usize, d: f64| v.get(i).copied().unwrap_or(d);
        self.law = g(0, 0.0).round().clamp(0.0, 2.0);
        self.kf = g(1, 1e-4);
        self.kd = g(2, 0.02);
        self.area_exponent = g(3, 0.5);
        self.slope_exponent = g(4, 1.0);
        self.channel_width_coeff = g(5, 1.0);
        self.manning_roughness = g(6, 0.05);
        self.incision_threshold = g(7, 0.0);
        self.transport_efficiency = g(8, 0.5);
        self.bank_erosion = g(9, 0.15);
        self.max_incision_m = g(10, 400.0);
    }

    fn set_key(&mut self, key: &str, value: f64) -> bool {
        match key {
            "fluvial_law" => self.law = value.round().clamp(0.0, 2.0),
            "kf" => self.kf = value,
            "kd" => self.kd = value,
            "area_exponent" => self.area_exponent = value,
            "slope_exponent" => self.slope_exponent = value,
            "channel_width_coeff" => self.channel_width_coeff = value,
            "manning_roughness" => self.manning_roughness = value,
            "incision_threshold" => self.incision_threshold = value,
            "transport_efficiency" => self.transport_efficiency = value,
            "bank_erosion" => self.bank_erosion = value,
            "max_incision_m" => self.max_incision_m = value,
            _ => return false,
        }
        true
    }

    fn get_key(&self, key: &str) -> Option<f64> {
        Some(match key {
            "fluvial_law" => self.law,
            "kf" => self.kf,
            "kd" => self.kd,
            "area_exponent" => self.area_exponent,
            "slope_exponent" => self.slope_exponent,
            "channel_width_coeff" => self.channel_width_coeff,
            "manning_roughness" => self.manning_roughness,
            "incision_threshold" => self.incision_threshold,
            "transport_efficiency" => self.transport_efficiency,
            "bank_erosion" => self.bank_erosion,
            "max_incision_m" => self.max_incision_m,
            _ => return None,
        })
    }

    fn randomize(&mut self, rng: &mut Rng) {
        // Erodibility and precipitation trade off in the stream-power law;
        // sample Kf around values that produce visible relief in ~1 Myr.
        self.kf = (rng.range_f64(-6.0, -3.0)).exp();
        self.kd = (rng.range_f64(-3.0, -0.7)).exp();
        self.area_exponent = rng.range_f64(0.35, 0.6);
        self.slope_exponent = 1.0;
        self.law = 0.0;
        self.transport_efficiency = rng.range_f64(0.3, 0.8);
    }
}

/// Hillslope diffusion, soils and mass wasting (spec §3c).
#[derive(Clone, Debug, PartialEq)]
pub struct HillslopeParams {
    /// Linear diffusion coefficient, m²/yr.
    pub diffusivity: f64,
    /// Nonlinear Roering-style flux strength, 0..1; the diffusivity grows
    /// toward the critical slope instead of staying constant.
    pub nonlinearity: f64,
    /// Repose angle: hillslopes steeper than this fail as landslides, degrees.
    pub critical_slope_deg: f64,
    /// Landslide runout length, m.
    pub landslide_runout_m: f64,
    /// Soil production rate at bare bedrock, m/yr (exponential depth decay).
    pub soil_production: f64,
    /// Maximum soil thickness, m.
    pub soil_max_m: f64,
    /// Soil cohesion from vegetation, 0..1.
    pub soil_cohesion: f64,
    /// Active creep thickness, m.
    pub creep_depth_m: f64,
}

impl Default for HillslopeParams {
    fn default() -> Self {
        Self {
            diffusivity: 0.02,
            nonlinearity: 0.3,
            critical_slope_deg: 38.0,
            landslide_runout_m: 80.0,
            soil_production: 1e-4,
            soil_max_m: 2.0,
            soil_cohesion: 0.5,
            creep_depth_m: 1.0,
        }
    }
}

impl HillslopeParams {
    /// Critical slope as a tangent (metres rise per metre run).
    #[must_use]
    pub fn critical_slope_tan(&self) -> f64 {
        self.critical_slope_deg.to_radians().tan()
    }
}

impl ParamBlock for HillslopeParams {
    fn specs() -> &'static [ParamSpec] {
        const SPECS: [ParamSpec; 8] = [
            ParamSpec::log(
                "diffusivity",
                "Diffusivity",
                "拡散係数",
                ParamGroup::Hillslope,
                "m²/yr",
                1e-6,
                1e-1,
                0.02,
                "Soil creep diffusivity; smooths hillslopes.",
            ),
            ParamSpec::float(
                "nonlinearity",
                "Nonlinear creep",
                "非線形クリープ",
                ParamGroup::Hillslope,
                "",
                0.0,
                1.0,
                0.3,
                "Roering nonlinear creep: faster transport on steep slopes.",
            ),
            ParamSpec::float(
                "critical_slope_deg",
                "Landslide angle",
                "崩壊角",
                ParamGroup::Hillslope,
                "°",
                15.0,
                70.0,
                38.0,
                "Repose angle above which slopes fail and debris runs out.",
            ),
            ParamSpec::log(
                "landslide_runout_m",
                "Runout distance",
                "崩壊距離",
                ParamGroup::Hillslope,
                "m",
                1.0,
                500.0,
                80.0,
                "Distance landslide debris travels downslope.",
            ),
            ParamSpec::log(
                "soil_production",
                "Soil production",
                "土壌生成",
                ParamGroup::Hillslope,
                "m/yr",
                1e-6,
                1e-2,
                1e-4,
                "Soil production: weathering of bedrock into mobile soil.",
            ),
            ParamSpec::float(
                "soil_max_m",
                "Max soil depth",
                "最大土壌厚",
                ParamGroup::Hillslope,
                "m",
                0.1,
                50.0,
                2.0,
                "Thickness limit for hillslope soil.",
            ),
            ParamSpec::float(
                "soil_cohesion",
                "Soil cohesion",
                "土壌凝集",
                ParamGroup::Hillslope,
                "",
                0.0,
                1.0,
                0.5,
                "Vegetation-reinforced resistance to mass wasting.",
            ),
            ParamSpec::float(
                "creep_depth_m",
                "Creep depth",
                "クリープ深度",
                ParamGroup::Hillslope,
                "m",
                0.1,
                10.0,
                1.0,
                "Depth of the actively creeping layer.",
            ),
        ];
        &SPECS
    }

    fn values_into(&self, out: &mut Vec<f64>) {
        out.extend_from_slice(&[
            self.diffusivity,
            self.nonlinearity,
            self.critical_slope_deg,
            self.landslide_runout_m,
            self.soil_production,
            self.soil_max_m,
            self.soil_cohesion,
            self.creep_depth_m,
        ]);
    }

    fn set_values(&mut self, v: &[f64]) {
        let g = |i: usize, d: f64| v.get(i).copied().unwrap_or(d);
        self.diffusivity = g(0, 0.02);
        self.nonlinearity = g(1, 0.3);
        self.critical_slope_deg = g(2, 38.0);
        self.landslide_runout_m = g(3, 80.0);
        self.soil_production = g(4, 1e-4);
        self.soil_max_m = g(5, 2.0);
        self.soil_cohesion = g(6, 0.5);
        self.creep_depth_m = g(7, 1.0);
    }

    fn set_key(&mut self, key: &str, value: f64) -> bool {
        match key {
            "diffusivity" => self.diffusivity = value,
            "nonlinearity" => self.nonlinearity = value,
            "critical_slope_deg" => self.critical_slope_deg = value,
            "landslide_runout_m" => self.landslide_runout_m = value,
            "soil_production" => self.soil_production = value,
            "soil_max_m" => self.soil_max_m = value,
            "soil_cohesion" => self.soil_cohesion = value,
            "creep_depth_m" => self.creep_depth_m = value,
            _ => return false,
        }
        true
    }

    fn get_key(&self, key: &str) -> Option<f64> {
        Some(match key {
            "diffusivity" => self.diffusivity,
            "nonlinearity" => self.nonlinearity,
            "critical_slope_deg" => self.critical_slope_deg,
            "landslide_runout_m" => self.landslide_runout_m,
            "soil_production" => self.soil_production,
            "soil_max_m" => self.soil_max_m,
            "soil_cohesion" => self.soil_cohesion,
            "creep_depth_m" => self.creep_depth_m,
            _ => return None,
        })
    }

    fn randomize(&mut self, rng: &mut Rng) {
        self.diffusivity = (rng.range_f64(-4.5, -1.2)).exp();
        self.nonlinearity = rng.range_f64(0.0, 0.6);
        self.critical_slope_deg = rng.range_f64(30.0, 45.0);
        self.soil_production = (rng.range_f64(-6.0, -3.5)).exp();
    }
}
