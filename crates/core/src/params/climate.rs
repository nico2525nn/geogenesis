//! Climate and ecology parameter blocks.

use crate::math::rng::Rng;
use crate::params::spec::{ParamGroup, ParamSpec};

use super::ParamBlock;

/// Temperature, moisture, wind and long-term climate (spec §3a, §5a).
#[derive(Clone, Debug, PartialEq)]
pub struct ClimateParams {
    /// Global mean surface temperature, °C.
    pub mean_temp_c: f64,
    /// Equator-to-pole temperature difference, °C.
    pub latitudinal_gradient_c: f64,
    /// Latitude at the centre of a regional (non-spherical) map, degrees.
    ///
    /// A plane/torus map is a window onto the planet, not the whole planet:
    /// this parameter places that window so an Earth-like default covers the
    /// tropics-to-temperate belt rather than a band centred on the equator.
    pub latitude_center_deg: f64,
    /// Latitude range covered by a regional map, degrees.
    pub latitude_span_deg: f64,
    /// Strength of the subtropical dry belt under the Hadley cells, 0..1.
    pub subtropical_dryness: f64,
    /// Extra cooling where snow persists (ice-albedo feedback), °C.
    pub snow_albedo_cooling_c: f64,
    /// Coastal seasonal temperature range, °C (amplified inland).
    pub seasonal_range_c: f64,
    /// Temperature lapse rate, °C/km.
    pub lapse_rate_c_km: f64,
    /// Continentality: how much interiors amplify seasonal extremes.
    pub continentality: f64,
    /// Ocean-current moderation of coastal temperature.
    pub ocean_current_moderation: f64,
    /// Large-scale mean precipitation, mm/yr.
    pub precipitation_mm: f64,
    /// Prevailing wind direction, degrees (from which the wind blows).
    pub wind_dir_deg: f64,
    /// Mean surface wind speed, m/s.
    pub wind_speed_ms: f64,
    /// Orographic precipitation enhancement.
    pub orographic_strength: f64,
    /// Rain-shadow drying downwind of ridges.
    pub rain_shadow_strength: f64,
    /// Fraction of evapotranspired moisture recycled into downwind rain.
    pub moisture_recycling: f64,
    /// Correlation of precipitation with elevation (-1..1).
    pub elev_precip_correlation: f64,
    /// Seasonal precipitation concentration (0 = uniform, 1 = monsoon).
    pub seasonality: f64,
    /// Atmospheric CO₂ concentration, ppm.
    pub co2_ppm: f64,
    /// Temperature change per CO₂ doubling, °C.
    pub co2_sensitivity_c: f64,
    /// Amplitude of long-period climate oscillation, °C.
    pub climate_cycle_amplitude_c: f64,
    /// Period of the long-period climate oscillation, kyr.
    pub climate_cycle_period_kyr: f64,
    /// Eustatic sea-level amplitude driven by ice volume, m.
    pub sea_level_amplitude_m: f64,
}

impl Default for ClimateParams {
    fn default() -> Self {
        Self {
            mean_temp_c: 16.0,
            latitudinal_gradient_c: 18.0,
            latitude_center_deg: 22.0,
            latitude_span_deg: 62.0,
            subtropical_dryness: 0.55,
            snow_albedo_cooling_c: 2.5,
            seasonal_range_c: 12.0,
            lapse_rate_c_km: 6.5,
            continentality: 0.6,
            ocean_current_moderation: 0.4,
            precipitation_mm: 1200.0,
            wind_dir_deg: 270.0,
            wind_speed_ms: 8.0,
            orographic_strength: 0.6,
            rain_shadow_strength: 0.6,
            moisture_recycling: 0.3,
            elev_precip_correlation: 0.3,
            seasonality: 0.3,
            co2_ppm: 420.0,
            co2_sensitivity_c: 3.0,
            climate_cycle_amplitude_c: 2.0,
            climate_cycle_period_kyr: 100.0,
            sea_level_amplitude_m: 40.0,
        }
    }
}

impl ClimateParams {
    /// Wind direction as a unit vector pointing *downwind*.
    ///
    /// The parameter follows the meteorological convention: `wind_dir_deg` is
    /// the direction the wind blows *from*. The returned vector points where
    /// the air is going (270° -> +x, i.e. westerly wind blowing east).
    #[must_use]
    pub fn wind_vector(&self) -> crate::math::Vec2 {
        let from = self.wind_dir_deg.to_radians();
        crate::math::Vec2::new(-from.sin(), -from.cos())
    }

    /// Baseline temperature offset from CO₂ relative to 280 ppm, °C.
    #[must_use]
    pub fn co2_offset_c(&self) -> f64 {
        let ppm = self.co2_ppm.max(1.0);
        self.co2_sensitivity_c * (ppm / 280.0).log2()
    }

    /// Temperature at the datum (sea level, mid-latitude band reference).
    #[must_use]
    pub fn reference_temp_c(&self) -> f64 {
        self.mean_temp_c + self.co2_offset_c()
    }

    /// Latitude, in degrees, of the centre of a regional (non-spherical) map.
    ///
    /// Spherical maps always span pole to pole, so they ignore this.
    #[must_use]
    pub fn regional_center_deg(&self) -> f64 {
        self.latitude_center_deg
    }

    /// Latitude of fractional row `y_fraction` (0 = north edge, 1 = south).
    ///
    /// Spherical grids span pole to pole; regional grids show a window of
    /// [`Self::latitude_span_deg`] degrees centred on
    /// [`Self::latitude_center_deg`], defaulting to the tropics-to-temperate
    /// belt where terrestrial biomes are most varied.
    #[must_use]
    pub fn latitude_at(&self, y_fraction: f64, spherical: bool) -> f64 {
        let north = 0.5 - y_fraction.clamp(0.0, 1.0);
        if spherical {
            north * 180.0
        } else {
            self.latitude_center_deg + north * self.latitude_span_deg
        }
    }

    /// Zonal-mean temperature anomaly relative to the global mean at `lat`.
    ///
    /// The profile is linear in latitude and anchored so the area-weighted
    /// mean over the sphere is exactly zero, which keeps `mean_temp_c` meaning
    /// the global mean. The equator runs above the mean and the pole below it,
    /// giving an equator-to-pole contrast of twice the gradient parameter.
    #[must_use]
    pub fn zonal_temperature_anomaly_c(&self, lat_deg: f64) -> f64 {
        // Area-weighted mean of |latitude|/90 over a sphere: 1 - 2/π.
        const MEAN_ABS_U: f64 = 1.0 - 2.0 / std::f64::consts::PI;
        let u = (lat_deg / 90.0).clamp(-1.0, 1.0);
        self.latitudinal_gradient_c * 2.0 * (MEAN_ABS_U - u.abs())
    }
}

impl ParamBlock for ClimateParams {
    fn specs() -> &'static [ParamSpec] {
        const SPECS: [ParamSpec; 23] = [
            ParamSpec::float(
                "mean_temp_c",
                "Mean temperature",
                "平均気温",
                ParamGroup::Climate,
                "°C",
                -50.0,
                50.0,
                16.0,
                "Global mean surface temperature at sea level.",
            ),
            ParamSpec::float(
                "latitudinal_gradient_c",
                "Polar gradient",
                "緯度温度差",
                ParamGroup::Climate,
                "°C",
                0.0,
                40.0,
                18.0,
                "Latitudinal temperature contrast: the equator runs above the global mean and the poles below.",
            ),
            ParamSpec::float(
                "latitude_center_deg",
                "Map latitude",
                "地図緯度",
                ParamGroup::Climate,
                "°",
                -60.0,
                60.0,
                22.0,
                "Latitude at the centre of a regional map; spherical maps ignore it.",
            ),
            ParamSpec::float(
                "latitude_span_deg",
                "Latitude span",
                "緯度範囲",
                ParamGroup::Climate,
                "°",
                10.0,
                180.0,
                62.0,
                "Latitude range covered by a regional map.",
            ),
            ParamSpec::float(
                "subtropical_dryness",
                "Subtropical dryness",
                "亜熱帯乾燥",
                ParamGroup::Climate,
                "",
                0.0,
                1.0,
                0.55,
                "Dry belt under the subtropical high-pressure cells.",
            ),
            ParamSpec::float(
                "snow_albedo_cooling_c",
                "Snow albedo cooling",
                "雪面冷却",
                ParamGroup::Climate,
                "°C",
                0.0,
                8.0,
                2.5,
                "Extra cooling where snow persists (ice-albedo feedback).",
            ),
            ParamSpec::float(
                "seasonal_range_c",
                "Seasonal range",
                "気温年較差",
                ParamGroup::Climate,
                "°C",
                0.0,
                40.0,
                12.0,
                "Coastal seasonal temperature range, amplified inland and at high latitude.",
            ),
            ParamSpec::float(
                "lapse_rate_c_km",
                "Lapse rate",
                "気温減率",
                ParamGroup::Climate,
                "°C/km",
                4.0,
                10.0,
                6.5,
                "Temperature drop with altitude.",
            ),
            ParamSpec::float(
                "continentality",
                "Continentality",
                "大陸性",
                ParamGroup::Climate,
                "",
                0.0,
                1.0,
                0.6,
                "How much interiors swing away from the mean.",
            ),
            ParamSpec::float(
                "ocean_current_moderation",
                "Ocean moderation",
                "海流緩和",
                ParamGroup::Climate,
                "",
                0.0,
                1.0,
                0.4,
                "Coastal temperature buffering by ocean currents.",
            ),
            ParamSpec::log(
                "precipitation_mm",
                "Precipitation",
                "降水量",
                ParamGroup::Climate,
                "mm/yr",
                10.0,
                8000.0,
                1200.0,
                "Large-scale mean annual precipitation.",
            ),
            ParamSpec::float(
                "wind_dir_deg",
                "Prevailing wind",
                "卓越風向",
                ParamGroup::Climate,
                "°",
                0.0,
                360.0,
                270.0,
                "Direction the wind blows from.",
            ),
            ParamSpec::float(
                "wind_speed_ms",
                "Wind speed",
                "風速",
                ParamGroup::Climate,
                "m/s",
                0.0,
                40.0,
                8.0,
                "Mean surface wind speed.",
            ),
            ParamSpec::float(
                "orographic_strength",
                "Orographic rain",
                "地形性降水",
                ParamGroup::Climate,
                "",
                0.0,
                1.0,
                0.6,
                "Rain enhancement on windward slopes.",
            ),
            ParamSpec::float(
                "rain_shadow_strength",
                "Rain shadow",
                "雨陰",
                ParamGroup::Climate,
                "",
                0.0,
                1.0,
                0.6,
                "Drying downwind of topographic barriers.",
            ),
            ParamSpec::float(
                "moisture_recycling",
                "Moisture recycling",
                "水蒸気再循環",
                ParamGroup::Climate,
                "",
                0.0,
                1.0,
                0.3,
                "Share of evaporated water rained out downwind.",
            ),
            ParamSpec::float(
                "elev_precip_correlation",
                "Elevation–rain coupling",
                "標高降水相関",
                ParamGroup::Climate,
                "",
                -1.0,
                1.0,
                0.3,
                "Positive values make high ground wetter.",
            ),
            ParamSpec::float(
                "seasonality",
                "Seasonality",
                "季節性",
                ParamGroup::Climate,
                "",
                0.0,
                1.0,
                0.3,
                "Concentration of rain into a wet season.",
            ),
            ParamSpec::float(
                "co2_ppm",
                "CO₂",
                "CO₂濃度",
                ParamGroup::Climate,
                "ppm",
                100.0,
                2000.0,
                420.0,
                "Atmospheric CO₂; shifts baseline temperature.",
            ),
            ParamSpec::float(
                "co2_sensitivity_c",
                "CO₂ sensitivity",
                "CO₂感度",
                ParamGroup::Climate,
                "°C/2×",
                0.0,
                8.0,
                3.0,
                "Temperature change per CO₂ doubling.",
            ),
            ParamSpec::float(
                "climate_cycle_amplitude_c",
                "Climate cycles",
                "気候周期振幅",
                ParamGroup::Climate,
                "°C",
                0.0,
                15.0,
                2.0,
                "Amplitude of long-period (orbital) temperature swings.",
            ),
            ParamSpec::log(
                "climate_cycle_period_kyr",
                "Cycle period",
                "気候周期",
                ParamGroup::Climate,
                "kyr",
                20.0,
                2000.0,
                100.0,
                "Period of the long-term climate oscillation.",
            ),
            ParamSpec::float(
                "sea_level_amplitude_m",
                "Eustatic amplitude",
                "海水準振幅",
                ParamGroup::Climate,
                "m",
                0.0,
                200.0,
                40.0,
                "Sea-level swing from ice-volume changes.",
            ),
        ];
        &SPECS
    }

    fn values_into(&self, out: &mut Vec<f64>) {
        out.extend_from_slice(&[
            self.mean_temp_c,
            self.latitudinal_gradient_c,
            self.latitude_center_deg,
            self.latitude_span_deg,
            self.subtropical_dryness,
            self.snow_albedo_cooling_c,
            self.seasonal_range_c,
            self.lapse_rate_c_km,
            self.continentality,
            self.ocean_current_moderation,
            self.precipitation_mm,
            self.wind_dir_deg,
            self.wind_speed_ms,
            self.orographic_strength,
            self.rain_shadow_strength,
            self.moisture_recycling,
            self.elev_precip_correlation,
            self.seasonality,
            self.co2_ppm,
            self.co2_sensitivity_c,
            self.climate_cycle_amplitude_c,
            self.climate_cycle_period_kyr,
            self.sea_level_amplitude_m,
        ]);
    }

    fn set_values(&mut self, v: &[f64]) {
        let g = |i: usize, d: f64| v.get(i).copied().unwrap_or(d);
        self.mean_temp_c = g(0, 16.0);
        self.latitudinal_gradient_c = g(1, 18.0);
        self.latitude_center_deg = g(2, 22.0);
        self.latitude_span_deg = g(3, 62.0);
        self.subtropical_dryness = g(4, 0.55);
        self.snow_albedo_cooling_c = g(5, 2.5);
        self.seasonal_range_c = g(6, 12.0);
        self.lapse_rate_c_km = g(7, 6.5);
        self.continentality = g(8, 0.6);
        self.ocean_current_moderation = g(9, 0.4);
        self.precipitation_mm = g(10, 1200.0);
        self.wind_dir_deg = g(11, 270.0);
        self.wind_speed_ms = g(12, 8.0);
        self.orographic_strength = g(13, 0.6);
        self.rain_shadow_strength = g(14, 0.6);
        self.moisture_recycling = g(15, 0.3);
        self.elev_precip_correlation = g(16, 0.3);
        self.seasonality = g(17, 0.3);
        self.co2_ppm = g(18, 420.0);
        self.co2_sensitivity_c = g(19, 3.0);
        self.climate_cycle_amplitude_c = g(20, 2.0);
        self.climate_cycle_period_kyr = g(21, 100.0);
        self.sea_level_amplitude_m = g(22, 40.0);
    }

    fn set_key(&mut self, key: &str, value: f64) -> bool {
        match key {
            "mean_temp_c" => self.mean_temp_c = value,
            "latitudinal_gradient_c" => self.latitudinal_gradient_c = value,
            "latitude_center_deg" => self.latitude_center_deg = value,
            "latitude_span_deg" => self.latitude_span_deg = value,
            "subtropical_dryness" => self.subtropical_dryness = value,
            "snow_albedo_cooling_c" => self.snow_albedo_cooling_c = value,
            "seasonal_range_c" => self.seasonal_range_c = value,
            "lapse_rate_c_km" => self.lapse_rate_c_km = value,
            "continentality" => self.continentality = value,
            "ocean_current_moderation" => self.ocean_current_moderation = value,
            "precipitation_mm" => self.precipitation_mm = value,
            "wind_dir_deg" => self.wind_dir_deg = value,
            "wind_speed_ms" => self.wind_speed_ms = value,
            "orographic_strength" => self.orographic_strength = value,
            "rain_shadow_strength" => self.rain_shadow_strength = value,
            "moisture_recycling" => self.moisture_recycling = value,
            "elev_precip_correlation" => self.elev_precip_correlation = value,
            "seasonality" => self.seasonality = value,
            "co2_ppm" => self.co2_ppm = value,
            "co2_sensitivity_c" => self.co2_sensitivity_c = value,
            "climate_cycle_amplitude_c" => self.climate_cycle_amplitude_c = value,
            "climate_cycle_period_kyr" => self.climate_cycle_period_kyr = value,
            "sea_level_amplitude_m" => self.sea_level_amplitude_m = value,
            _ => return false,
        }
        true
    }

    fn get_key(&self, key: &str) -> Option<f64> {
        Some(match key {
            "mean_temp_c" => self.mean_temp_c,
            "latitudinal_gradient_c" => self.latitudinal_gradient_c,
            "latitude_center_deg" => self.latitude_center_deg,
            "latitude_span_deg" => self.latitude_span_deg,
            "subtropical_dryness" => self.subtropical_dryness,
            "snow_albedo_cooling_c" => self.snow_albedo_cooling_c,
            "seasonal_range_c" => self.seasonal_range_c,
            "lapse_rate_c_km" => self.lapse_rate_c_km,
            "continentality" => self.continentality,
            "ocean_current_moderation" => self.ocean_current_moderation,
            "precipitation_mm" => self.precipitation_mm,
            "wind_dir_deg" => self.wind_dir_deg,
            "wind_speed_ms" => self.wind_speed_ms,
            "orographic_strength" => self.orographic_strength,
            "rain_shadow_strength" => self.rain_shadow_strength,
            "moisture_recycling" => self.moisture_recycling,
            "elev_precip_correlation" => self.elev_precip_correlation,
            "seasonality" => self.seasonality,
            "co2_ppm" => self.co2_ppm,
            "co2_sensitivity_c" => self.co2_sensitivity_c,
            "climate_cycle_amplitude_c" => self.climate_cycle_amplitude_c,
            "climate_cycle_period_kyr" => self.climate_cycle_period_kyr,
            "sea_level_amplitude_m" => self.sea_level_amplitude_m,
            _ => return None,
        })
    }

    fn validate(&self) -> crate::error::GeoResult<()> {
        if self.lapse_rate_c_km <= 0.0 {
            return Err(crate::error::GeoError::param(
                "lapse_rate_c_km",
                self.lapse_rate_c_km,
                "must be positive",
            ));
        }
        Ok(())
    }

    fn randomize(&mut self, rng: &mut Rng) {
        self.mean_temp_c = rng.range_f64(-2.0, 26.0);
        self.latitudinal_gradient_c = rng.range_f64(6.0, 28.0);
        self.subtropical_dryness = rng.range_f64(0.15, 0.85);
        self.seasonal_range_c = rng.range_f64(4.0, 24.0);
        self.continentality = rng.range_f64(0.3, 0.9);
        self.precipitation_mm = (rng.range_f64(0.8, 1.5)).exp() * 700.0;
        self.wind_dir_deg = rng.range_f64(0.0, 360.0);
        self.wind_speed_ms = rng.range_f64(3.0, 16.0);
        self.co2_ppm = rng.range_f64(280.0, 800.0);
        self.climate_cycle_amplitude_c = rng.range_f64(0.0, 6.0);
    }
}

/// Vegetation and biome parameters (spec §5b).
#[derive(Clone, Debug, PartialEq)]
pub struct EcologyParams {
    /// Maximum vegetation cover fraction.
    pub vegetation_max: f64,
    /// Logistic growth rate for vegetation, 1/yr.
    pub vegetation_growth_rate: f64,
    /// Rooting depth, m.
    pub root_depth_m: f64,
    /// Root-cohesion strength (erosion protection), 0..1.
    pub root_strength: f64,
    /// Litter-layer depth, m.
    pub litter_depth_m: f64,
    /// Biome transition rate, 1/yr.
    pub succession_rate: f64,
    /// Temperature offset applied to biome classification, °C.
    pub biome_shift_c: f64,
    /// Interval between vegetation updates, years.
    pub vegetation_update_yr: f64,
    /// Strength of disturbance resets from steep or eroding ground, 0..1.
    pub vegetation_disturbance: f64,
}

impl Default for EcologyParams {
    fn default() -> Self {
        Self {
            vegetation_max: 0.85,
            vegetation_growth_rate: 1e-4,
            root_depth_m: 2.0,
            root_strength: 0.6,
            litter_depth_m: 0.1,
            succession_rate: 1e-3,
            biome_shift_c: 0.0,
            vegetation_update_yr: 2000.0,
            vegetation_disturbance: 0.6,
        }
    }
}

impl ParamBlock for EcologyParams {
    fn specs() -> &'static [ParamSpec] {
        const SPECS: [ParamSpec; 9] = [
            ParamSpec::float(
                "vegetation_max",
                "Max vegetation",
                "最大植生",
                ParamGroup::Ecology,
                "",
                0.0,
                1.0,
                0.85,
                "Upper limit of vegetation cover.",
            ),
            ParamSpec::log(
                "vegetation_growth_rate",
                "Growth rate",
                "植生成長率",
                ParamGroup::Ecology,
                "1/yr",
                1e-6,
                1e-2,
                1e-4,
                "Speed at which vegetation colonises bare ground.",
            ),
            ParamSpec::log(
                "root_depth_m",
                "Root depth",
                "根系深さ",
                ParamGroup::Ecology,
                "m",
                0.1,
                20.0,
                2.0,
                "Depth of roots binding the soil.",
            ),
            ParamSpec::float(
                "root_strength",
                "Root strength",
                "根系強度",
                ParamGroup::Ecology,
                "",
                0.0,
                1.0,
                0.6,
                "Erosion protection from root cohesion.",
            ),
            ParamSpec::float(
                "litter_depth_m",
                "Litter layer",
                "落葉層",
                ParamGroup::Ecology,
                "m",
                0.0,
                1.0,
                0.1,
                "Surface litter that shields against rain and wind.",
            ),
            ParamSpec::log(
                "succession_rate",
                "Succession rate",
                "遷移速度",
                ParamGroup::Ecology,
                "1/yr",
                1e-5,
                1e-1,
                1e-3,
                "Speed of biome transitions after climate change.",
            ),
            ParamSpec::float(
                "biome_shift_c",
                "Biome offset",
                "バイオーム補正",
                ParamGroup::Ecology,
                "°C",
                -10.0,
                10.0,
                0.0,
                "Shifts biome classification warmer or colder.",
            ),
            ParamSpec::log(
                "vegetation_update_yr",
                "Update interval",
                "更新間隔",
                ParamGroup::Ecology,
                "yr",
                100.0,
                50_000.0,
                2000.0,
                "How often vegetation cover and biomes update.",
            ),
            ParamSpec::float(
                "vegetation_disturbance",
                "Disturbance",
                "植生撹乱",
                ParamGroup::Ecology,
                "",
                0.0,
                1.0,
                0.6,
                "How strongly steep, eroding ground resets vegetation cover.",
            ),
        ];
        &SPECS
    }

    fn values_into(&self, out: &mut Vec<f64>) {
        out.extend_from_slice(&[
            self.vegetation_max,
            self.vegetation_growth_rate,
            self.root_depth_m,
            self.root_strength,
            self.litter_depth_m,
            self.succession_rate,
            self.biome_shift_c,
            self.vegetation_update_yr,
            self.vegetation_disturbance,
        ]);
    }

    fn set_values(&mut self, v: &[f64]) {
        let g = |i: usize, d: f64| v.get(i).copied().unwrap_or(d);
        self.vegetation_max = g(0, 0.85);
        self.vegetation_growth_rate = g(1, 1e-4);
        self.root_depth_m = g(2, 2.0);
        self.root_strength = g(3, 0.6);
        self.litter_depth_m = g(4, 0.1);
        self.succession_rate = g(5, 1e-3);
        self.biome_shift_c = g(6, 0.0);
        self.vegetation_update_yr = g(7, 2000.0);
        self.vegetation_disturbance = g(8, 0.6);
    }

    fn set_key(&mut self, key: &str, value: f64) -> bool {
        match key {
            "vegetation_max" => self.vegetation_max = value,
            "vegetation_growth_rate" => self.vegetation_growth_rate = value,
            "root_depth_m" => self.root_depth_m = value,
            "root_strength" => self.root_strength = value,
            "litter_depth_m" => self.litter_depth_m = value,
            "succession_rate" => self.succession_rate = value,
            "biome_shift_c" => self.biome_shift_c = value,
            "vegetation_update_yr" => self.vegetation_update_yr = value,
            "vegetation_disturbance" => self.vegetation_disturbance = value,
            _ => return false,
        }
        true
    }

    fn get_key(&self, key: &str) -> Option<f64> {
        Some(match key {
            "vegetation_max" => self.vegetation_max,
            "vegetation_growth_rate" => self.vegetation_growth_rate,
            "root_depth_m" => self.root_depth_m,
            "root_strength" => self.root_strength,
            "litter_depth_m" => self.litter_depth_m,
            "succession_rate" => self.succession_rate,
            "biome_shift_c" => self.biome_shift_c,
            "vegetation_update_yr" => self.vegetation_update_yr,
            "vegetation_disturbance" => self.vegetation_disturbance,
            _ => return None,
        })
    }

    fn randomize(&mut self, rng: &mut Rng) {
        self.vegetation_max = rng.range_f64(0.5, 0.98);
        self.root_strength = rng.range_f64(0.3, 0.9);
        self.litter_depth_m = rng.range_f64(0.0, 0.3);
        self.biome_shift_c = rng.range_f64(-4.0, 4.0);
        self.vegetation_disturbance = rng.range_f64(0.2, 0.9);
    }
}
