//! Glacial, aeolian, coastal and chemical process parameters
//! (spec §3d–3g).

use crate::math::rng::Rng;
use crate::params::spec::{ParamGroup, ParamSpec};

use super::ParamBlock;

/// Ice sheets and glacial erosion (spec §3e).
#[derive(Clone, Debug, PartialEq)]
pub struct GlacialParams {
    /// Equilibrium-line altitude (snowline), m.
    pub ela_m: f64,
    /// Ice flow coefficient.
    pub ice_flow_coefficient: f64,
    /// Glacial erosion coefficient.
    pub glacial_erosion_coefficient: f64,
    /// U-valley widening strength, 0..1.
    pub u_valley_strength: f64,
    /// Maximum ice thickness, m.
    pub max_ice_thickness_m: f64,
    /// Sea-level temperature offset that shifts the ELA, m/°C.
    pub ela_sensitivity_m_per_c: f64,
    /// Scaling of snow accumulation by precipitation (1 = reference climate).
    pub accumulation_scale: f64,
    /// Share of glacial debris re-deposited as till rather than exported, 0..1.
    pub till_redeposition: f64,
    /// Update interval, years.
    pub glacial_update_yr: f64,
}

impl Default for GlacialParams {
    fn default() -> Self {
        Self {
            ela_m: 2800.0,
            ice_flow_coefficient: 1e-7,
            glacial_erosion_coefficient: 1e-5,
            u_valley_strength: 0.6,
            max_ice_thickness_m: 1500.0,
            ela_sensitivity_m_per_c: 150.0,
            accumulation_scale: 1.0,
            till_redeposition: 0.45,
            glacial_update_yr: 2000.0,
        }
    }
}

impl ParamBlock for GlacialParams {
    fn specs() -> &'static [ParamSpec] {
        const SPECS: [ParamSpec; 9] = [
            ParamSpec::float(
                "ela_m",
                "Snowline (ELA)",
                "雪線高度",
                ParamGroup::Cryosphere,
                "m",
                0.0,
                8000.0,
                2800.0,
                "Equilibrium-line altitude: ice accumulates above, melts below.",
            ),
            ParamSpec::log(
                "ice_flow_coefficient",
                "Ice flow",
                "氷河流動",
                ParamGroup::Cryosphere,
                "1/(m·yr)",
                1e-10,
                1e-5,
                1e-7,
                "Glen-law flow coefficient for ice deformation.",
            ),
            ParamSpec::log(
                "glacial_erosion_coefficient",
                "Glacial erosion",
                "氷河侵食",
                ParamGroup::Cryosphere,
                "1/m",
                1e-8,
                1e-4,
                1e-5,
                "Bed abrasion rate per unit sliding velocity.",
            ),
            ParamSpec::float(
                "u_valley_strength",
                "U-valley carving",
                "U字谷形成",
                ParamGroup::Cryosphere,
                "",
                0.0,
                1.0,
                0.6,
                "Lateral widening that turns V-valleys into U-valleys.",
            ),
            ParamSpec::float(
                "max_ice_thickness_m",
                "Max ice thickness",
                "最大氷厚",
                ParamGroup::Cryosphere,
                "m",
                100.0,
                5000.0,
                1500.0,
                "Cap on ice thickness to keep margins crustal.",
            ),
            ParamSpec::float(
                "ela_sensitivity_m_per_c",
                "ELA sensitivity",
                "雪線感度",
                ParamGroup::Cryosphere,
                "m/°C",
                0.0,
                500.0,
                150.0,
                "How far the snowline rises per degree of warming.",
            ),
            ParamSpec::float(
                "accumulation_scale",
                "Accumulation",
                "涵養スケール",
                ParamGroup::Cryosphere,
                "",
                0.1,
                5.0,
                1.0,
                "Scales ice growth with the cell's snowfall.",
            ),
            ParamSpec::float(
                "till_redeposition",
                "Till redeposition",
                "堆積物再堆積",
                ParamGroup::Cryosphere,
                "",
                0.0,
                1.0,
                0.45,
                "Share of glacial debris left as till instead of exported by meltwater.",
            ),
            ParamSpec::log(
                "glacial_update_yr",
                "Update interval",
                "更新間隔",
                ParamGroup::Cryosphere,
                "yr",
                100.0,
                100_000.0,
                2000.0,
                "How often ice and glacial erosion are refreshed.",
            ),
        ];
        &SPECS
    }

    fn values_into(&self, out: &mut Vec<f64>) {
        out.extend_from_slice(&[
            self.ela_m,
            self.ice_flow_coefficient,
            self.glacial_erosion_coefficient,
            self.u_valley_strength,
            self.max_ice_thickness_m,
            self.ela_sensitivity_m_per_c,
            self.accumulation_scale,
            self.till_redeposition,
            self.glacial_update_yr,
        ]);
    }

    fn set_values(&mut self, v: &[f64]) {
        let g = |i: usize, d: f64| v.get(i).copied().unwrap_or(d);
        self.ela_m = g(0, 2800.0);
        self.ice_flow_coefficient = g(1, 1e-7);
        self.glacial_erosion_coefficient = g(2, 1e-5);
        self.u_valley_strength = g(3, 0.6);
        self.max_ice_thickness_m = g(4, 1500.0);
        self.ela_sensitivity_m_per_c = g(5, 150.0);
        self.accumulation_scale = g(6, 1.0);
        self.till_redeposition = g(7, 0.45);
        self.glacial_update_yr = g(8, 2000.0);
    }

    fn set_key(&mut self, key: &str, value: f64) -> bool {
        match key {
            "ela_m" => self.ela_m = value,
            "ice_flow_coefficient" => self.ice_flow_coefficient = value,
            "glacial_erosion_coefficient" => self.glacial_erosion_coefficient = value,
            "u_valley_strength" => self.u_valley_strength = value,
            "max_ice_thickness_m" => self.max_ice_thickness_m = value,
            "ela_sensitivity_m_per_c" => self.ela_sensitivity_m_per_c = value,
            "accumulation_scale" => self.accumulation_scale = value,
            "till_redeposition" => self.till_redeposition = value,
            "glacial_update_yr" => self.glacial_update_yr = value,
            _ => return false,
        }
        true
    }

    fn get_key(&self, key: &str) -> Option<f64> {
        Some(match key {
            "ela_m" => self.ela_m,
            "ice_flow_coefficient" => self.ice_flow_coefficient,
            "glacial_erosion_coefficient" => self.glacial_erosion_coefficient,
            "u_valley_strength" => self.u_valley_strength,
            "max_ice_thickness_m" => self.max_ice_thickness_m,
            "ela_sensitivity_m_per_c" => self.ela_sensitivity_m_per_c,
            "accumulation_scale" => self.accumulation_scale,
            "till_redeposition" => self.till_redeposition,
            "glacial_update_yr" => self.glacial_update_yr,
            _ => return None,
        })
    }

    fn randomize(&mut self, rng: &mut Rng) {
        self.ela_m = rng.range_f64(1500.0, 4500.0);
        self.u_valley_strength = rng.range_f64(0.2, 0.9);
        self.till_redeposition = rng.range_f64(0.2, 0.7);
    }
}

/// Wind erosion and dune building (spec §3d).
#[derive(Clone, Debug, PartialEq)]
pub struct AeolianParams {
    /// Erosion efficiency of wind, 0..1.
    pub erosion_strength: f64,
    /// Saltation (bedload) transport share.
    pub saltation_coefficient: f64,
    /// Threshold wind speed for movement, m/s.
    pub threshold_wind_ms: f64,
    /// Vegetation shielding of the surface, 0..1.
    pub vegetation_shielding: f64,
    /// Whether dunes are allowed to form.
    pub dune_formation: bool,
    /// Dust emission strength.
    pub dust_emission: f64,
    /// Surface drag coefficient relating wind speed to shear velocity.
    pub drag_coefficient: f64,
    /// Update interval, years.
    pub aeolian_update_yr: f64,
}

impl Default for AeolianParams {
    fn default() -> Self {
        Self {
            erosion_strength: 0.25,
            saltation_coefficient: 0.3,
            threshold_wind_ms: 6.0,
            vegetation_shielding: 0.7,
            dune_formation: true,
            dust_emission: 0.3,
            drag_coefficient: 0.0025,
            aeolian_update_yr: 1000.0,
        }
    }
}

impl ParamBlock for AeolianParams {
    fn specs() -> &'static [ParamSpec] {
        const SPECS: [ParamSpec; 8] = [
            ParamSpec::float(
                "aeolian_erosion_strength",
                "Wind erosion",
                "風食強度",
                ParamGroup::Aeolian,
                "",
                0.0,
                1.0,
                0.25,
                "Deflation efficiency where the surface is bare.",
            ),
            ParamSpec::float(
                "saltation_coefficient",
                "Saltation",
                "飛砂",
                ParamGroup::Aeolian,
                "",
                0.0,
                1.0,
                0.3,
                "Share of wind transport as bouncing sand.",
            ),
            ParamSpec::float(
                "threshold_wind_ms",
                "Threshold wind",
                "限界風速",
                ParamGroup::Aeolian,
                "m/s",
                0.0,
                25.0,
                6.0,
                "Wind speed needed to move sand.",
            ),
            ParamSpec::float(
                "vegetation_shielding",
                "Vegetation shield",
                "植生遮蔽",
                ParamGroup::Aeolian,
                "",
                0.0,
                1.0,
                0.7,
                "How strongly vegetation protects the surface.",
            ),
            ParamSpec::boolean(
                "dune_formation",
                "Build dunes",
                "砂丘形成",
                ParamGroup::Aeolian,
                true,
                "Accumulate transported sand into dunes.",
            ),
            ParamSpec::float(
                "dust_emission",
                "Dust emission",
                "ダスト放出",
                ParamGroup::Aeolian,
                "",
                0.0,
                1.0,
                0.3,
                "Fine material removed to the atmosphere.",
            ),
            ParamSpec::log(
                "drag_coefficient",
                "Drag coefficient",
                "抗力係数",
                ParamGroup::Aeolian,
                "",
                5e-4,
                0.02,
                0.0025,
                "Converts mean wind to shear velocity: u* = u·sqrt(Cd).",
            ),
            ParamSpec::log(
                "aeolian_update_yr",
                "Update interval",
                "更新間隔",
                ParamGroup::Aeolian,
                "yr",
                100.0,
                100_000.0,
                1000.0,
                "How often wind erosion is applied.",
            ),
        ];
        &SPECS
    }

    fn values_into(&self, out: &mut Vec<f64>) {
        out.extend_from_slice(&[
            self.erosion_strength,
            self.saltation_coefficient,
            self.threshold_wind_ms,
            self.vegetation_shielding,
            f64::from(u8::from(self.dune_formation)),
            self.dust_emission,
            self.drag_coefficient,
            self.aeolian_update_yr,
        ]);
    }

    fn set_values(&mut self, v: &[f64]) {
        let g = |i: usize, d: f64| v.get(i).copied().unwrap_or(d);
        self.erosion_strength = g(0, 0.25);
        self.saltation_coefficient = g(1, 0.3);
        self.threshold_wind_ms = g(2, 6.0);
        self.vegetation_shielding = g(3, 0.7);
        self.dune_formation = g(4, 1.0) >= 0.5;
        self.dust_emission = g(5, 0.3);
        self.drag_coefficient = g(6, 0.0025);
        self.aeolian_update_yr = g(7, 1000.0);
    }

    fn set_key(&mut self, key: &str, value: f64) -> bool {
        match key {
            "aeolian_erosion_strength" => self.erosion_strength = value,
            "saltation_coefficient" => self.saltation_coefficient = value,
            "threshold_wind_ms" => self.threshold_wind_ms = value,
            "vegetation_shielding" => self.vegetation_shielding = value,
            "dune_formation" => self.dune_formation = value >= 0.5,
            "dust_emission" => self.dust_emission = value,
            "drag_coefficient" => self.drag_coefficient = value,
            "aeolian_update_yr" => self.aeolian_update_yr = value,
            _ => return false,
        }
        true
    }

    fn get_key(&self, key: &str) -> Option<f64> {
        Some(match key {
            "aeolian_erosion_strength" => self.erosion_strength,
            "saltation_coefficient" => self.saltation_coefficient,
            "threshold_wind_ms" => self.threshold_wind_ms,
            "vegetation_shielding" => self.vegetation_shielding,
            "dune_formation" => f64::from(u8::from(self.dune_formation)),
            "dust_emission" => self.dust_emission,
            "drag_coefficient" => self.drag_coefficient,
            "aeolian_update_yr" => self.aeolian_update_yr,
            _ => return None,
        })
    }

    fn randomize(&mut self, rng: &mut Rng) {
        self.erosion_strength = rng.range_f64(0.0, 0.5);
        self.dust_emission = rng.range_f64(0.0, 0.5);
        self.dune_formation = true;
    }
}

/// Waves, tides and coastal processes (spec §3f).
#[derive(Clone, Debug, PartialEq)]
pub struct CoastalParams {
    /// Wave erosion efficiency, 0..1.
    pub wave_erosivity: f64,
    /// Tidal range, m.
    pub tidal_range_m: f64,
    /// Alongshore sediment transport strength, 0..1.
    pub alongshore_transport: f64,
    /// Residual eustatic trend, mm/yr.
    pub sea_level_rate_mm_yr: f64,
    /// Delta-building efficiency, 0..1.
    pub delta_build_strength: f64,
    /// Wave-base depth, m.
    pub wave_base_depth_m: f64,
    /// Fetch that produces fully developed seas, km.
    pub fetch_reference_km: f64,
}

impl Default for CoastalParams {
    fn default() -> Self {
        Self {
            wave_erosivity: 0.3,
            tidal_range_m: 2.0,
            alongshore_transport: 0.4,
            sea_level_rate_mm_yr: 0.0,
            delta_build_strength: 0.5,
            wave_base_depth_m: 60.0,
            fetch_reference_km: 200.0,
        }
    }
}

impl ParamBlock for CoastalParams {
    fn specs() -> &'static [ParamSpec] {
        const SPECS: [ParamSpec; 7] = [
            ParamSpec::float(
                "wave_erosivity",
                "Wave erosion",
                "波食強度",
                ParamGroup::Coastal,
                "",
                0.0,
                1.0,
                0.3,
                "Cliff retreat and shore abrasion efficiency.",
            ),
            ParamSpec::float(
                "tidal_range_m",
                "Tidal range",
                "潮位差",
                ParamGroup::Coastal,
                "m",
                0.0,
                20.0,
                2.0,
                "Vertical tidal range widening the active shore zone.",
            ),
            ParamSpec::float(
                "alongshore_transport",
                "Longshore drift",
                "沿岸漂砂",
                ParamGroup::Coastal,
                "",
                0.0,
                1.0,
                0.4,
                "Sediment redistribution parallel to the coast.",
            ),
            ParamSpec::float(
                "sea_level_rate_mm_yr",
                "Eustatic trend",
                "海水準トレンド",
                ParamGroup::Coastal,
                "mm/yr",
                -10.0,
                10.0,
                0.0,
                "Slow residual sea-level change on top of climate cycles.",
            ),
            ParamSpec::float(
                "delta_build_strength",
                "Delta building",
                "デルタ形成",
                ParamGroup::Coastal,
                "",
                0.0,
                1.0,
                0.5,
                "How efficiently river sediment builds deltas.",
            ),
            ParamSpec::float(
                "wave_base_depth_m",
                "Wave base depth",
                "波浪基準深度",
                ParamGroup::Coastal,
                "m",
                5.0,
                200.0,
                60.0,
                "Depth below which waves no longer rework sediment.",
            ),
            ParamSpec::log(
                "fetch_reference_km",
                "Fetch reference",
                "吹送距離基準",
                ParamGroup::Coastal,
                "km",
                5.0,
                5000.0,
                200.0,
                "Open-water fetch at which wave energy reaches its full value.",
            ),
        ];
        &SPECS
    }

    fn values_into(&self, out: &mut Vec<f64>) {
        out.extend_from_slice(&[
            self.wave_erosivity,
            self.tidal_range_m,
            self.alongshore_transport,
            self.sea_level_rate_mm_yr,
            self.delta_build_strength,
            self.wave_base_depth_m,
            self.fetch_reference_km,
        ]);
    }

    fn set_values(&mut self, v: &[f64]) {
        let g = |i: usize, d: f64| v.get(i).copied().unwrap_or(d);
        self.wave_erosivity = g(0, 0.3);
        self.tidal_range_m = g(1, 2.0);
        self.alongshore_transport = g(2, 0.4);
        self.sea_level_rate_mm_yr = g(3, 0.0);
        self.delta_build_strength = g(4, 0.5);
        self.wave_base_depth_m = g(5, 60.0);
        self.fetch_reference_km = g(6, 200.0);
    }

    fn set_key(&mut self, key: &str, value: f64) -> bool {
        match key {
            "wave_erosivity" => self.wave_erosivity = value,
            "tidal_range_m" => self.tidal_range_m = value,
            "alongshore_transport" => self.alongshore_transport = value,
            "sea_level_rate_mm_yr" => self.sea_level_rate_mm_yr = value,
            "delta_build_strength" => self.delta_build_strength = value,
            "wave_base_depth_m" => self.wave_base_depth_m = value,
            "fetch_reference_km" => self.fetch_reference_km = value,
            _ => return false,
        }
        true
    }

    fn get_key(&self, key: &str) -> Option<f64> {
        Some(match key {
            "wave_erosivity" => self.wave_erosivity,
            "tidal_range_m" => self.tidal_range_m,
            "alongshore_transport" => self.alongshore_transport,
            "sea_level_rate_mm_yr" => self.sea_level_rate_mm_yr,
            "delta_build_strength" => self.delta_build_strength,
            "wave_base_depth_m" => self.wave_base_depth_m,
            "fetch_reference_km" => self.fetch_reference_km,
            _ => return None,
        })
    }

    fn randomize(&mut self, rng: &mut Rng) {
        self.wave_erosivity = rng.range_f64(0.1, 0.6);
        self.alongshore_transport = rng.range_f64(0.1, 0.7);
        self.delta_build_strength = rng.range_f64(0.2, 0.8);
    }
}

/// Dissolution and chemical weathering (spec §3g).
#[derive(Clone, Debug, PartialEq)]
pub struct ChemicalParams {
    /// Dissolution efficiency, 0..1.
    pub dissolution_coefficient: f64,
    /// Chemical reactivity of the rock, 0..1.
    pub reactivity: f64,
    /// Temperature sensitivity of dissolution, 0..1.
    pub temperature_sensitivity: f64,
    /// Karst surface roughening strength, 0..1.
    pub karst_roughness: f64,
    /// Apparent activation energy of dissolution, kJ/mol.
    pub activation_energy_kj_mol: f64,
    /// Exponent of the runoff dependence of dissolution.
    pub runoff_exponent: f64,
}

impl Default for ChemicalParams {
    fn default() -> Self {
        Self {
            dissolution_coefficient: 0.2,
            reactivity: 0.5,
            temperature_sensitivity: 0.5,
            karst_roughness: 0.3,
            activation_energy_kj_mol: 50.0,
            runoff_exponent: 0.7,
        }
    }
}

impl ParamBlock for ChemicalParams {
    fn specs() -> &'static [ParamSpec] {
        const SPECS: [ParamSpec; 6] = [
            ParamSpec::float(
                "dissolution_coefficient",
                "Dissolution",
                "溶食係数",
                ParamGroup::Chemistry,
                "",
                0.0,
                1.0,
                0.2,
                "Rate of chemical removal of soluble rock.",
            ),
            ParamSpec::float(
                "reactivity",
                "Reactivity",
                "反応性",
                ParamGroup::Chemistry,
                "",
                0.0,
                1.0,
                0.5,
                "Chemical susceptibility of the bedrock.",
            ),
            ParamSpec::float(
                "temperature_sensitivity",
                "Temperature effect",
                "温度依存性",
                ParamGroup::Chemistry,
                "",
                0.0,
                1.0,
                0.5,
                "How much warmer, wetter climates dissolve faster.",
            ),
            ParamSpec::float(
                "karst_roughness",
                "Karst relief",
                "カルスト起伏",
                ParamGroup::Chemistry,
                "",
                0.0,
                1.0,
                0.3,
                "Roughening of limestone terrain by dissolution.",
            ),
            ParamSpec::float(
                "activation_energy_kj_mol",
                "Activation energy",
                "活性化エネルギー",
                ParamGroup::Chemistry,
                "kJ/mol",
                5.0,
                120.0,
                50.0,
                "Arrhenius temperature dependence of dissolution kinetics.",
            ),
            ParamSpec::float(
                "runoff_exponent",
                "Runoff exponent",
                "流出指数",
                ParamGroup::Chemistry,
                "",
                0.2,
                1.5,
                0.7,
                "Power-law dependence of dissolution on water flux.",
            ),
        ];
        &SPECS
    }

    fn values_into(&self, out: &mut Vec<f64>) {
        out.extend_from_slice(&[
            self.dissolution_coefficient,
            self.reactivity,
            self.temperature_sensitivity,
            self.karst_roughness,
            self.activation_energy_kj_mol,
            self.runoff_exponent,
        ]);
    }

    fn set_values(&mut self, v: &[f64]) {
        let g = |i: usize, d: f64| v.get(i).copied().unwrap_or(d);
        self.dissolution_coefficient = g(0, 0.2);
        self.reactivity = g(1, 0.5);
        self.temperature_sensitivity = g(2, 0.5);
        self.karst_roughness = g(3, 0.3);
        self.activation_energy_kj_mol = g(4, 50.0);
        self.runoff_exponent = g(5, 0.7);
    }

    fn set_key(&mut self, key: &str, value: f64) -> bool {
        match key {
            "dissolution_coefficient" => self.dissolution_coefficient = value,
            "reactivity" => self.reactivity = value,
            "temperature_sensitivity" => self.temperature_sensitivity = value,
            "karst_roughness" => self.karst_roughness = value,
            "activation_energy_kj_mol" => self.activation_energy_kj_mol = value,
            "runoff_exponent" => self.runoff_exponent = value,
            _ => return false,
        }
        true
    }

    fn get_key(&self, key: &str) -> Option<f64> {
        Some(match key {
            "dissolution_coefficient" => self.dissolution_coefficient,
            "reactivity" => self.reactivity,
            "temperature_sensitivity" => self.temperature_sensitivity,
            "karst_roughness" => self.karst_roughness,
            "activation_energy_kj_mol" => self.activation_energy_kj_mol,
            "runoff_exponent" => self.runoff_exponent,
            _ => return None,
        })
    }

    fn randomize(&mut self, rng: &mut Rng) {
        self.dissolution_coefficient = rng.range_f64(0.05, 0.4);
        self.reactivity = rng.range_f64(0.2, 0.8);
    }
}
