pub mod fluvial;
pub mod hillslope;
pub mod wind;
pub mod glacial;
pub mod coastal;
pub mod chemical;

use crate::world::HeightMap;
use serde::{Deserialize, Serialize};

/// 侵食プロセス共通トレイト
pub trait ErosionProcess {
    fn apply(&self, heightmap: &mut HeightMap, dt: f64);
    fn name(&self) -> &'static str;
}

/// 侵食システム全体の設定
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErosionConfig {
    pub fluvial: fluvial::FluvialConfig,
    pub hillslope: hillslope::HillslopeConfig,
    pub wind: wind::WindErosionConfig,
    pub glacial: glacial::GlacialConfig,
    pub coastal: coastal::CoastalConfig,
    pub chemical: chemical::ChemicalConfig,
}

impl Default for ErosionConfig {
    fn default() -> Self {
        Self {
            fluvial: fluvial::FluvialConfig::default(),
            hillslope: hillslope::HillslopeConfig::default(),
            wind: wind::WindErosionConfig::default(),
            glacial: glacial::GlacialConfig::default(),
            coastal: coastal::CoastalConfig::default(),
            chemical: chemical::ChemicalConfig::default(),
        }
    }
}
