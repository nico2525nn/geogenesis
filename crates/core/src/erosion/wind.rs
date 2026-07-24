use crate::world::HeightMap;
use serde::{Deserialize, Serialize};
use super::ErosionProcess;

/// 風食・風成過程設定
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindErosionConfig {
    pub avg_wind_speed: f64,
    pub prevailing_wind_dir_deg: f64,
    pub erosion_intensity: f64,
    pub sand_flux_coef: f64,
    pub vegetation_shelter: f64,
    pub dune_formation: bool,
}

impl Default for WindErosionConfig {
    fn default() -> Self {
        Self {
            avg_wind_speed: 5.0,
            prevailing_wind_dir_deg: 270.0,
            erosion_intensity: 0.3,
            sand_flux_coef: 0.1,
            vegetation_shelter: 0.5,
            dune_formation: false,
        }
    }
}

/// 風食プロセス
pub struct WindErosion {
    pub config: WindErosionConfig,
}

impl WindErosion {
    pub fn new(config: WindErosionConfig) -> Self {
        Self { config }
    }
}

impl ErosionProcess for WindErosion {
    fn apply(&self, heightmap: &mut HeightMap, dt: f64) {
        let w = heightmap.width;
        let h = heightmap.height;

        // 風向ベクトル
        let wind_dir = self.config.prevailing_wind_dir_deg.to_radians();
        let wx = wind_dir.cos();
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                let idx = y * w + x;

                // 風上からの遮蔽効果を考慮した風食量
                let upwind_h = if wx > 0.0 {
                    heightmap.data[y * w + (x as i64 - 1) as usize]
                } else if wx < 0.0 {
                    heightmap.data[y * w + (x as i64 + 1) as usize]
                } else {
                    heightmap.data[idx]
                };

                let shelter = if upwind_h > heightmap.data[idx] {
                    self.config.vegetation_shelter
                } else {
                    1.0
                };

                let erosion = self.config.erosion_intensity *
                    self.config.avg_wind_speed / 10.0 *
                    shelter * dt;

                heightmap.data[idx] -= erosion * 0.01; // 風食は遅い
            }
        }
    }

    fn name(&self) -> &'static str {
        "wind"
    }
}
