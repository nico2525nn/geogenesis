use crate::world::HeightMap;
use serde::{Deserialize, Serialize};
use super::ErosionProcess;

/// 海岸・波浪侵食設定
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoastalConfig {
    pub wave_erosion_coef: f64,
    pub tidal_range_m: f64,
    pub longshore_sediment_m3_yr: f64,
    pub sea_level_rise_mm_yr: f64,
}

impl Default for CoastalConfig {
    fn default() -> Self {
        Self {
            wave_erosion_coef: 0.2,
            tidal_range_m: 2.0,
            longshore_sediment_m3_yr: 10000.0,
            sea_level_rise_mm_yr: 1.0,
        }
    }
}

/// 海岸侵食プロセス
pub struct CoastalErosion {
    pub config: CoastalConfig,
    pub sea_level: f64,
}

impl CoastalErosion {
    pub fn new(config: CoastalConfig, sea_level: f64) -> Self {
        Self { config, sea_level }
    }
}

impl ErosionProcess for CoastalErosion {
    fn apply(&self, heightmap: &mut HeightMap, dt: f64) {
        let w = heightmap.width;
        let h = heightmap.height;
        let current_sea_level = self.sea_level + self.config.sea_level_rise_mm_yr * dt * 0.001;

        for y in 0..h {
            for x in 0..w {
                let idx = y * w + x;
                let elev = heightmap.data[idx];

                // 海岸線付近のみ侵食
                let coastal_zone = (elev - current_sea_level).abs();
                if coastal_zone < 50.0 {
                    let intensity = (1.0 - coastal_zone / 50.0) *
                        self.config.wave_erosion_coef * dt * 0.001;
                    heightmap.data[idx] -= intensity;

                    // 堆積: 海側に運ばれた土砂を浅海域に堆積
                    if elev < current_sea_level && elev > current_sea_level - 20.0 {
                        heightmap.data[idx] += intensity * 0.3;
                    }
                }
            }
        }
    }

    fn name(&self) -> &'static str {
        "coastal"
    }
}
