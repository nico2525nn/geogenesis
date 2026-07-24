use crate::world::HeightMap;
use serde::{Deserialize, Serialize};
use super::ErosionProcess;

/// 斜面プロセス設定
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HillslopeConfig {
    pub diffusion_coef: f64,
    pub critical_slope_deg: f64,
    pub soil_production_rate: f64,
    pub max_soil_depth: f64,
    pub landslide_distance: f64,
}

impl Default for HillslopeConfig {
    fn default() -> Self {
        Self {
            diffusion_coef: 1e-4,
            critical_slope_deg: 35.0,
            soil_production_rate: 1e-4,
            max_soil_depth: 10.0,
            landslide_distance: 100.0,
        }
    }
}

/// 斜面プロセス（クリープ・崩壊）
pub struct HillslopeProcess {
    pub config: HillslopeConfig,
}

impl HillslopeProcess {
    pub fn new(config: HillslopeConfig) -> Self {
        Self { config }
    }
}

impl ErosionProcess for HillslopeProcess {
    fn apply(&self, heightmap: &mut HeightMap, dt: f64) {
        let w = heightmap.width;
        let h = heightmap.height;
        let critical_slope = self.config.critical_slope_deg.to_radians().tan();
        let mut new_data = heightmap.data.clone();

        for y in 1..h - 1 {
            for x in 1..w - 1 {
                let idx = y * w + x;
                let slope = heightmap.slope(x, y);

                // 土壌クリープ（拡散）
                let laplacian =
                    heightmap.data[(y - 1) * w + x] +
                    heightmap.data[(y + 1) * w + x] +
                    heightmap.data[y * w + (x - 1)] +
                    heightmap.data[y * w + (x + 1)] -
                    4.0 * heightmap.data[idx];

                let creep = self.config.diffusion_coef * laplacian * dt;
                new_data[idx] += creep;

                // 崩壊（臨界傾斜角超過）
                if slope > critical_slope {
                    let excess = (slope - critical_slope) * 0.1;
                    new_data[idx] -= excess * dt;
                    // 崩壊土砂を下方へ移動（簡易）
                    if y + 1 < h {
                        new_data[(y + 1) * w + x] += excess * dt * 0.5;
                    }
                }
            }
        }

        heightmap.data = new_data;
    }

    fn name(&self) -> &'static str {
        "hillslope"
    }
}
