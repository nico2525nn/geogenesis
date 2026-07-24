use crate::erosion::ErosionProcess;
use crate::erosion::fluvial::{FluvialConfig, FluvialErosion};
use crate::erosion::hillslope::{HillslopeConfig, HillslopeProcess};
use crate::erosion::wind::{WindErosionConfig, WindErosion};
use crate::erosion::glacial::{GlacialConfig, GlacialErosion};
use crate::erosion::coastal::{CoastalConfig, CoastalErosion};
use crate::erosion::chemical::{ChemicalConfig, ChemicalErosion};
use crate::noise::{NoiseConfig, NoiseGenerator, NoiseType};
use crate::tectonic::{TectonicConfig, TectonicSystem};
use crate::world::{HeightMap, WorldConfig};
use serde::{Deserialize, Serialize};

/// シミュレーション設定
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulationConfig {
    pub total_time_years: f64,
    pub base_time_step_years: f64,
    pub adaptive_timestep: bool,
    pub cfl_condition: bool,
    pub max_iterations: u32,
    pub tectonic: TectonicConfig,
    pub fluvial: FluvialConfig,
    pub hillslope: HillslopeConfig,
    pub wind: WindErosionConfig,
    pub glacial: GlacialConfig,
    pub coastal: CoastalConfig,
    pub chemical: ChemicalConfig,
    pub noise: NoiseConfig,
}

impl Default for SimulationConfig {
    fn default() -> Self {
        Self {
            total_time_years: 1_000_000.0,
            base_time_step_years: 1000.0,
            adaptive_timestep: false,
            cfl_condition: false,
            max_iterations: 100_000,
            tectonic: TectonicConfig::default(),
            fluvial: FluvialConfig::default(),
            hillslope: HillslopeConfig::default(),
            wind: WindErosionConfig::default(),
            glacial: GlacialConfig::default(),
            coastal: CoastalConfig::default(),
            chemical: ChemicalConfig::default(),
            noise: NoiseConfig::default(),
        }
    }
}

/// シミュレーションモード
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum SimulationMode {
    OneShot,
    Stepwise,
    Manual,
    Realtime,
    Batch,
}

/// シミュレーション実行エンジン
pub struct Simulation {
    pub config: SimulationConfig,
    pub world: WorldConfig,
    pub heightmap: HeightMap,
    pub mode: SimulationMode,
    pub current_time: f64,
    pub iteration: u32,

    tectonic: TectonicSystem,
    noise_gen: NoiseGenerator,
    fluvial: FluvialErosion,
    hillslope: HillslopeProcess,
    wind: WindErosion,
    glacial: GlacialErosion,
    coastal: CoastalErosion,
    chemical: ChemicalErosion,
}

impl Simulation {
    pub fn new(world: WorldConfig, config: SimulationConfig) -> Self {
        let seed = if world.seed == 0 {
            use std::time::{SystemTime, UNIX_EPOCH};
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64
        } else {
            world.seed
        };

        let noise_gen = NoiseGenerator::new(seed);
        let tectonic = TectonicSystem::new(config.tectonic.clone(), seed.wrapping_add(1));

        let heightmap = HeightMap::new(world.grid_size.0 as usize, world.grid_size.1 as usize);

        Self {
            fluvial: FluvialErosion::new(config.fluvial.clone()),
            hillslope: HillslopeProcess::new(config.hillslope.clone()),
            wind: WindErosion::new(config.wind.clone()),
            glacial: GlacialErosion::new(config.glacial.clone()),
            coastal: CoastalErosion::new(config.coastal.clone(), world.sea_level_m),
            chemical: ChemicalErosion::new(config.chemical.clone()),
            noise_gen,
            tectonic,
            heightmap,
            world,
            config,
            mode: SimulationMode::OneShot,
            current_time: 0.0,
            iteration: 0,
        }
    }

    /// 初期標高マップ生成
    pub fn generate_initial_heightmap(&mut self) {
        let w = self.world.grid_size.0 as usize;
        let h = self.world.grid_size.1 as usize;
        let v_scale = self.world.vertical_scale_m;
        let mut map = HeightMap::new(w, h);

        for y in 0..h {
            for x in 0..w {
                let nx = x as f64 / w as f64 * 10.0;
                let ny = y as f64 / h as f64 * 10.0;

                let n1 = self.noise_gen.fractal_2d(nx, ny, &self.config.noise);

                let large_config = NoiseConfig {
                    octaves: 2,
                    lacunarity: 2.0,
                    persistence: 0.5,
                    ..self.config.noise
                };
                let n2 = self.noise_gen.fractal_2d(nx * 0.2, ny * 0.2, &large_config);

                let elevation = (n2 * 0.7 + n1 * 0.3) * v_scale;
                map.set(x, y, elevation);
            }
        }

        let stress = self.tectonic.stress_map(w, h);
        let uplift_noise_cfg = NoiseConfig {
            noise_type: NoiseType::Perlin,
            octaves: 3,
            lacunarity: 2.0,
            persistence: 0.5,
            anisotropy: 0.0,
            seed: self.world.seed.wrapping_add(10),
        };
        let uplift = self.tectonic.uplift_map(w, h, &uplift_noise_cfg);
        let hotspots = self.tectonic.hotspot_map(w, h);

        for y in 0..h {
            for x in 0..w {
                let idx = y * w + x;
                let u = uplift.data[idx];
                let s = stress.data[idx].abs().min(1.0);
                let hs = hotspots.data[idx];
                map.data[idx] += u * s * 100.0 + hs;
            }
        }

        self.heightmap = map;
    }

    /// 1タイムステップ実行
    pub fn step(&mut self, dt: f64) {
        // 侵食プロセスを順次適用
        self.fluvial.apply(&mut self.heightmap, dt);
        self.hillslope.apply(&mut self.heightmap, dt);
        self.wind.apply(&mut self.heightmap, dt);
        self.glacial.apply(&mut self.heightmap, dt);
        self.coastal.apply(&mut self.heightmap, dt);
        self.chemical.apply(&mut self.heightmap, dt);

        self.current_time += dt;
        self.iteration += 1;
    }

    /// 全シミュレーション実行
    pub fn run(&mut self) {
        let dt = self.config.base_time_step_years;
        let max_steps = (self.config.total_time_years / dt) as u32;
        let steps = max_steps.min(self.config.max_iterations);

        for _ in 0..steps {
            self.step(dt);
        }
    }

    /// 地形統計
    pub fn statistics(&self) -> TerrainStatistics {
        let hm = &self.heightmap;
        let mean = hm.data.iter().sum::<f64>() / hm.data.len() as f64;
        let mut sorted = hm.data.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());

        let variance = hm.data.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / hm.data.len() as f64;

        TerrainStatistics {
            min_elevation: hm.min(),
            max_elevation: hm.max(),
            mean_elevation: mean,
            std_dev: variance.sqrt(),
            median: sorted[sorted.len() / 2],
            p10: sorted[sorted.len() / 10],
            p90: sorted[sorted.len() * 9 / 10],
            total_cells: hm.data.len() as u64,
        }
    }
}

/// 地形統計情報
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerrainStatistics {
    pub min_elevation: f64,
    pub max_elevation: f64,
    pub mean_elevation: f64,
    pub std_dev: f64,
    pub median: f64,
    pub p10: f64,
    pub p90: f64,
    pub total_cells: u64,
}
