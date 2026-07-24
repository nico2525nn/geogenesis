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
        let max_change = heightmap.max().max(1000.0).abs() * 0.2;

        for y in 1..h - 1 {
            for x in 1..w - 1 {
                let idx = y * w + x;
                let h_center = heightmap.data[idx];
                if !h_center.is_finite() { continue; }

                let h_up = heightmap.data[(y - 1) * w + x];
                let h_down = heightmap.data[(y + 1) * w + x];
                let h_left = heightmap.data[y * w + (x - 1)];
                let h_right = heightmap.data[y * w + (x + 1)];

                if !(h_up.is_finite() && h_down.is_finite() && h_left.is_finite() && h_right.is_finite()) {
                    continue;
                }

                let laplacian = h_up + h_down + h_left + h_right - 4.0 * h_center;
                let creep = self.config.diffusion_coef * laplacian * dt;
                let creep = creep.clamp(-max_change, max_change);
                new_data[idx] += creep;

                let slope = heightmap.slope(x, y);
                if slope.is_finite() && slope > critical_slope {
                    let excess = ((slope - critical_slope) * 0.1 * dt).min(max_change);
                    new_data[idx] -= excess;
                    if y + 1 < h && new_data[(y + 1) * w + x].is_finite() {
                        new_data[(y + 1) * w + x] += excess * 0.5;
                    }
                }
            }
        }

        for v in &mut new_data {
            if !v.is_finite() { *v = 0.0; }
            if *v < -10000.0 { *v = -10000.0; }
        }

        heightmap.data = new_data;
    }

    fn name(&self) -> &'static str {
        "hillslope"
    }
}
