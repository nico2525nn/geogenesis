use crate::world::HeightMap;
use serde::{Deserialize, Serialize};
use super::ErosionProcess;

/// 氷河侵食設定
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlacialConfig {
    pub annual_mean_temp_c: f64,
    pub lapse_rate_c_per_km: f64,
    pub ice_flow_coef: f64,
    pub glacial_erosion_coef: f64,
    pub snowline_altitude_m: f64,
    pub u_valley_strength: f64,
}

impl Default for GlacialConfig {
    fn default() -> Self {
        Self {
            annual_mean_temp_c: 5.0,
            lapse_rate_c_per_km: 6.5,
            ice_flow_coef: 1e-8,
            glacial_erosion_coef: 1e-6,
            snowline_altitude_m: 2500.0,
            u_valley_strength: 0.3,
        }
    }
}

/// 氷河侵食プロセス
pub struct GlacialErosion {
    pub config: GlacialConfig,
}

impl GlacialErosion {
    pub fn new(config: GlacialConfig) -> Self {
        Self { config }
    }

    fn is_glacial_zone(&self, elevation: f64) -> bool {
        let temp_at_alt = self.config.annual_mean_temp_c -
            (elevation / 1000.0) * self.config.lapse_rate_c_per_km;
        elevation > self.config.snowline_altitude_m && temp_at_alt < 0.0
    }
}

impl ErosionProcess for GlacialErosion {
    fn apply(&self, heightmap: &mut HeightMap, dt: f64) {
        let w = heightmap.width;

        for y in 0..heightmap.height {
            for x in 0..heightmap.width {
                let idx = y * w + x;
                let elev = heightmap.data[idx];

                if !self.is_glacial_zone(elev) {
                    continue;
                }

                let slope = heightmap.slope(x, y);
                let ice_flux = self.config.ice_flow_coef * slope * elev.max(0.0);
                let erosion = self.config.glacial_erosion_coef * ice_flux * dt;

                heightmap.data[idx] -= erosion;

                // U字谷効果: 谷壁方向も侵食
                if self.config.u_valley_strength > 0.0 {
                    if x > 0 {
                        heightmap.data[idx - 1] -= erosion * self.config.u_valley_strength * 0.1;
                    }
                    if x + 1 < heightmap.width {
                        heightmap.data[idx + 1] -= erosion * self.config.u_valley_strength * 0.1;
                    }
                }
            }
        }
    }

    fn name(&self) -> &'static str {
        "glacial"
    }
}
