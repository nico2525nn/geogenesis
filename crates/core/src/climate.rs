use crate::world::HeightMap;
use serde::{Deserialize, Serialize};

/// 植生タイプ
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum BiomeType {
    TropicalRainforest,
    TemperateForest,
    ConiferousForest,
    Grassland,
    Desert,
    Tundra,
    IceField,
    Maquis,
    Savannah,
}

impl BiomeType {
    pub fn name(&self) -> &'static str {
        match self {
            BiomeType::TropicalRainforest => "Tropical Rainforest",
            BiomeType::TemperateForest => "Temperate Forest",
            BiomeType::ConiferousForest => "Coniferous Forest",
            BiomeType::Grassland => "Grassland",
            BiomeType::Desert => "Desert",
            BiomeType::Tundra => "Tundra",
            BiomeType::IceField => "Ice Field",
            BiomeType::Maquis => "Maquis",
            BiomeType::Savannah => "Savannah",
        }
    }
}

/// 気候ゾーン設定
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClimateConfig {
    pub latitude_deg: f64,
    pub mean_temp_c: f64,
    pub diurnal_range_c: f64,
    pub annual_range_c: f64,
    pub ocean_current_influence: f64,
    pub continentality_index: f64,
}

impl Default for ClimateConfig {
    fn default() -> Self {
        Self {
            latitude_deg: 35.0,
            mean_temp_c: 15.0,
            diurnal_range_c: 10.0,
            annual_range_c: 25.0,
            ocean_current_influence: 0.5,
            continentality_index: 0.3,
        }
    }
}

/// 植生設定
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VegetationConfig {
    pub density: f64,
    pub root_depth_m: f64,
    pub root_strength: f64,
    pub litter_layer_m: f64,
    pub biomass_production_kg_m2_yr: f64,
    pub succession_rate: f64,
}

impl Default for VegetationConfig {
    fn default() -> Self {
        Self {
            density: 0.5,
            root_depth_m: 2.0,
            root_strength: 0.5,
            litter_layer_m: 0.1,
            biomass_production_kg_m2_yr: 1.0,
            succession_rate: 0.1,
        }
    }
}

/// 気候・生態系シミュレータ
pub struct ClimateSimulator {
    pub climate: ClimateConfig,
    pub vegetation: VegetationConfig,
}

impl ClimateSimulator {
    pub fn new(climate: ClimateConfig, vegetation: VegetationConfig) -> Self {
        Self { climate, vegetation }
    }

    /// ホイッタカーダイアグラム拡張によるバイオーム決定
    pub fn biome_at(&self, elevation: f64, precipitation_mm: f64, lat_deg: f64) -> BiomeType {
        let temp = self.climate.mean_temp_c -
            (elevation / 1000.0) * 6.5;

        // 緯度補正
        let lat_factor = (lat_deg.abs() / 90.0).min(1.0);

        if temp < -10.0 {
            return BiomeType::IceField;
        }
        if temp < 0.0 && lat_factor > 0.5 {
            return BiomeType::Tundra;
        }

        match (temp, precipitation_mm) {
            (t, p) if t > 20.0 && p > 2000.0 => BiomeType::TropicalRainforest,
            (t, p) if t > 20.0 && p > 1000.0 => BiomeType::Savannah,
            (t, _) if t > 18.0 && precipitation_mm < 500.0 => BiomeType::Desert,
            (t, p) if t > 10.0 && p > 800.0 => BiomeType::TemperateForest,
            (t, p) if t > 5.0 && p > 600.0 => BiomeType::ConiferousForest,
            (t, p) if t > 5.0 && p > 300.0 => BiomeType::Grassland,
            (t, _) if t > 5.0 => BiomeType::Maquis,
            (t, p) if t > -5.0 && p > 300.0 => BiomeType::ConiferousForest,
            (_, _) => BiomeType::Tundra,
        }
    }

    /// 気温マップを生成
    pub fn temperature_map(&self, heightmap: &HeightMap, lat_offset: f64) -> HeightMap {
        let w = heightmap.width;
        let h = heightmap.height;
        let mut map = HeightMap::new(w, h);

        for y in 0..h {
            for x in 0..w {
                let elev = heightmap.data[y * w + x];
                let lat = (y as f64 / h as f64) * 180.0 - 90.0 + lat_offset;

                // 気温減率 + 緯度効果 + 大陸性
                let base_temp = 30.0 * (1.0 - (lat / 90.0).abs()) - 10.0;
                let lapse_correction = (elev / 1000.0) * self.climate.lapse_rate();
                let continentality = self.climate.continentality_index * 5.0;

                let temp = base_temp - lapse_correction + continentality;
                map.set(x, y, temp);
            }
        }
        map
    }

    /// 降水量マップを生成（簡易）
    pub fn precipitation_map(&self, heightmap: &HeightMap, wind_dir_deg: f64) -> HeightMap {
        let w = heightmap.width;
        let h = heightmap.height;
        let mut map = HeightMap::new(w, h);

        let wind_rad = wind_dir_deg.to_radians();
        let wx = wind_rad.cos();
        let wy = wind_rad.sin();

        for y in 0..h {
            for x in 0..w {
                let elev = heightmap.data[y * w + x];

                // 風上効果: 標高が高いほど降水増
                let orographic = (elev / 3000.0).min(1.0).max(0.0) * 1000.0;

                // 雨陰効果: 風下斜面で減少
                let upwind_h = {
                    let ux = (x as f64 - wx * 10.0).clamp(0.0, w as f64 - 1.0) as usize;
                    let uy = (y as f64 - wy * 10.0).clamp(0.0, h as f64 - 1.0) as usize;
                    heightmap.data[uy * w + ux]
                };
                let rain_shadow = if upwind_h > elev {
                    -200.0 * (upwind_h - elev) / 1000.0
                } else {
                    0.0
                };

                let precip = (800.0 + orographic + rain_shadow).max(50.0).min(5000.0);
                map.set(x, y, precip);
            }
        }
        map
    }
}

impl ClimateConfig {
    pub fn lapse_rate(&self) -> f64 {
        6.5 // °C/km
    }
}
