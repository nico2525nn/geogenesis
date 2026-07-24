use serde::{Deserialize, Serialize};

/// 岩石タイプ
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RockType {
    Granite,
    Basalt,
    Limestone,
    Sandstone,
    Shale,
    Gneiss,
    Schist,
    Marble,
    Slate,
    Conglomerate,
    Tuff,
    Dolomite,
    Mudstone,
    Chert,
}

impl RockType {
    pub fn name(&self) -> &'static str {
        match self {
            RockType::Granite => "Granite",
            RockType::Basalt => "Basalt",
            RockType::Limestone => "Limestone",
            RockType::Sandstone => "Sandstone",
            RockType::Shale => "Shale",
            RockType::Gneiss => "Gneiss",
            RockType::Schist => "Schist",
            RockType::Marble => "Marble",
            RockType::Slate => "Slate",
            RockType::Conglomerate => "Conglomerate",
            RockType::Tuff => "Tuff",
            RockType::Dolomite => "Dolomite",
            RockType::Mudstone => "Mudstone",
            RockType::Chert => "Chert",
        }
    }

    pub fn default_hardness(&self) -> u8 {
        match self {
            RockType::Granite => 7,
            RockType::Basalt => 6,
            RockType::Limestone => 3,
            RockType::Sandstone => 4,
            RockType::Shale => 3,
            RockType::Gneiss => 7,
            RockType::Schist => 5,
            RockType::Marble => 4,
            RockType::Slate => 5,
            RockType::Conglomerate => 5,
            RockType::Tuff => 3,
            RockType::Dolomite => 4,
            RockType::Mudstone => 3,
            RockType::Chert => 7,
        }
    }

    pub fn default_density(&self) -> f64 {
        match self {
            RockType::Granite => 2.7,
            RockType::Basalt => 3.0,
            RockType::Limestone => 2.5,
            RockType::Sandstone => 2.3,
            RockType::Shale => 2.4,
            RockType::Gneiss => 2.8,
            RockType::Schist => 2.7,
            RockType::Marble => 2.7,
            RockType::Slate => 2.6,
            RockType::Conglomerate => 2.4,
            RockType::Tuff => 1.8,
            RockType::Dolomite => 2.8,
            RockType::Mudstone => 2.3,
            RockType::Chert => 2.6,
        }
    }
}

/// 岩石物理特性
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RockProperties {
    pub rock_type: RockType,
    pub hardness_mohs: u8,
    pub permeability_m2: f64,
    pub friction_angle_deg: f64,
    pub cohesion_mpa: f64,
    pub density_g_cm3: f64,
    pub chemical_solubility: f64,
    pub weathering_rate_m_yr: f64,
    pub custom_name: Option<String>,
    pub color_rgba: Option<[u8; 4]>,
    pub origin: RockOrigin,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RockOrigin {
    Igneous,
    Sedimentary,
    Metamorphic,
}

impl RockProperties {
    pub fn new(rock_type: RockType) -> Self {
        Self {
            rock_type,
            hardness_mohs: rock_type.default_hardness(),
            permeability_m2: 1e-10,
            friction_angle_deg: 35.0,
            cohesion_mpa: 5.0,
            density_g_cm3: rock_type.default_density(),
            chemical_solubility: 0.1,
            weathering_rate_m_yr: 1e-4,
            custom_name: None,
            color_rgba: None,
            origin: match rock_type {
                RockType::Granite | RockType::Basalt | RockType::Tuff => RockOrigin::Igneous,
                RockType::Limestone | RockType::Sandstone | RockType::Shale
                | RockType::Conglomerate | RockType::Dolomite
                | RockType::Mudstone | RockType::Chert => RockOrigin::Sedimentary,
                RockType::Gneiss | RockType::Schist | RockType::Marble
                | RockType::Slate => RockOrigin::Metamorphic,
            },
        }
    }
}

/// 層序の1層
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StratigraphicLayer {
    pub thickness_m: f64,
    pub rock: RockProperties,
    pub dip_deg: f64,
    pub strike_deg: f64,
    pub fault_offset_m: f64,
    pub unconformity: bool,
}

/// 層序スタック
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StratigraphicStack {
    pub layers: Vec<StratigraphicLayer>,
}

impl StratigraphicStack {
    pub fn new() -> Self {
        Self { layers: Vec::new() }
    }

    pub fn total_thickness(&self) -> f64 {
        self.layers.iter().map(|l| l.thickness_m).sum()
    }

    /// 標高に対する露出岩石を取得
    pub fn exposed_rock(&self, elevation: f64, surface_elevation: f64) -> Option<&RockProperties> {
        let depth = surface_elevation - elevation;
        let mut accumulated = 0.0;
        for layer in &self.layers {
            accumulated += layer.thickness_m;
            if depth <= accumulated {
                return Some(&layer.rock);
            }
        }
        self.layers.last().map(|l| &l.rock)
    }
}
