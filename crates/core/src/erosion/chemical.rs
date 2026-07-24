use crate::world::HeightMap;
use serde::{Deserialize, Serialize};
use super::ErosionProcess;

/// ケミカル侵食（溶食）設定
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChemicalConfig {
    pub dissolution_coef: f64,
    pub geochemical_reactivity: f64,
    pub co2_concentration: f64,
}

impl Default for ChemicalConfig {
    fn default() -> Self {
        Self {
            dissolution_coef: 0.01,
            geochemical_reactivity: 0.5,
            co2_concentration: 0.04,
        }
    }
}

/// ケミカル侵食プロセス
pub struct ChemicalErosion {
    pub config: ChemicalConfig,
}

impl ChemicalErosion {
    pub fn new(config: ChemicalConfig) -> Self {
        Self { config }
    }
}

impl ErosionProcess for ChemicalErosion {
    fn apply(&self, heightmap: &mut HeightMap, dt: f64) {
        let w = heightmap.width;

        for y in 0..heightmap.height {
            for x in 0..heightmap.width {
                let idx = y * w + x;
                let precip = 1000.0; // 仮の年間降水量

                // 溶食量 = 溶解係数 × 化学反応性 × CO₂影響 × 降水量
                let dissolution = self.config.dissolution_coef *
                    self.config.geochemical_reactivity *
                    (1.0 + self.config.co2_concentration * 10.0) *
                    (precip / 1000.0) * dt * 1e-6;

                heightmap.data[idx] -= dissolution;
            }
        }
    }

    fn name(&self) -> &'static str {
        "chemical"
    }
}
