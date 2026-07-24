use crate::noise::{NoiseConfig, NoiseGenerator};
use rand_xoshiro::Xoshiro256StarStar;
use crate::world::HeightMap;
use serde::{Deserialize, Serialize};
use rand::SeedableRng;

/// プレート情報
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plate {
    pub id: u32,
    pub centroid_x: f64,
    pub centroid_y: f64,
    pub velocity_x: f64,
    pub velocity_y: f64,
    pub thickness_km: f64,
    pub density: f64,
    pub is_oceanic: bool,
}

/// テクトニック設定
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TectonicConfig {
    pub plate_count: u32,
    pub plate_thickness_km: f64,
    pub plate_densities: [f64; 3],
    pub collision_threshold: f64,
    pub avg_uplift_rate_mm_yr: f64,
    pub uplift_noise_frequency: f64,
    pub isostasy_correction: bool,
    pub flexural_rigidity_nm: f64,
    pub hotspot_count: u32,
    pub hotspot_strength_m: f64,
}

impl Default for TectonicConfig {
    fn default() -> Self {
        Self {
            plate_count: 8,
            plate_thickness_km: 35.0,
            plate_densities: [2.7, 3.0, 3.3],
            collision_threshold: 0.5,
            avg_uplift_rate_mm_yr: 1.0,
            uplift_noise_frequency: 0.01,
            isostasy_correction: true,
            flexural_rigidity_nm: 1e23,
            hotspot_count: 5,
            hotspot_strength_m: 2000.0,
        }
    }
}

/// テクトニックシステム
pub struct TectonicSystem {
    pub plates: Vec<Plate>,
    pub config: TectonicConfig,
    noise: NoiseGenerator,
}

impl TectonicSystem {
    pub fn new(config: TectonicConfig, seed: u64) -> Self {
        let mut rng = Xoshiro256StarStar::seed_from_u64(seed);
        let plates = if config.plate_count > 0 {
            Self::generate_plates(config.plate_count, seed, &mut rng)
        } else {
            vec![Plate {
                id: 0,
                centroid_x: 0.0,
                centroid_y: 0.0,
                velocity_x: 0.0,
                velocity_y: 0.0,
                thickness_km: config.plate_thickness_km,
                density: config.plate_densities[0],
                is_oceanic: false,
            }]
        };

        Self {
            plates,
            config,
            noise: NoiseGenerator::new(seed.wrapping_add(2)),
        }
    }
    fn generate_plates(count: u32, _seed: u64, rng: &mut Xoshiro256StarStar) -> Vec<Plate> {
        use rand::Rng;
        let mut plates = Vec::with_capacity(count as usize);
        for id in 0..count {
            let angle = rng.gen_range(0.0..std::f64::consts::TAU);
            let speed = rng.gen_range(-2.0..2.0);
            let is_oceanic = id % 3 == 1;
            plates.push(Plate {
                id,
                centroid_x: rng.gen_range(-1.0..1.0),
                centroid_y: rng.gen_range(-1.0..1.0),
                velocity_x: angle.cos() * speed,
                velocity_y: angle.sin() * speed,
                thickness_km: 25.0 + rng.gen_range(0.0..20.0),
                density: if is_oceanic { 3.0 } else { 2.7 },
                is_oceanic,
            });
        }
        plates
    }

    /// プレート境界からの応力マップを生成
    pub fn stress_map(&self, width: usize, height: usize) -> HeightMap {
        let mut map = HeightMap::new(width, height);
        for y in 0..height {
            for x in 0..width {
                let nx = x as f64 / width as f64 * 2.0 - 1.0;
                let ny = y as f64 / height as f64 * 2.0 - 1.0;

                // 最も近いプレート境界までの距離を計算（簡易: ボロノイ距離）
                let mut min_dist = f64::MAX;
                let mut min_dist2 = f64::MAX;
                for plate in &self.plates {
                    let dx = nx - plate.centroid_x;
                    let dy = ny - plate.centroid_y;
                    let d = dx * dx + dy * dy;
                    if d < min_dist {
                        min_dist2 = min_dist;
                        min_dist = d;
                    } else if d < min_dist2 {
                        min_dist2 = d;
                    }
                }
                let boundary_dist = min_dist.sqrt() - min_dist2.sqrt();
                map.set(x, y, boundary_dist);
            }
        }
        map
    }

    /// 隆起マップを生成
    pub fn uplift_map(&self, width: usize, height: usize, config: &NoiseConfig) -> HeightMap {
        let mut map = HeightMap::new(width, height);
        let freq = self.config.uplift_noise_frequency;
        for y in 0..height {
            for x in 0..width {
                let nx = x as f64 * freq;
                let ny = y as f64 * freq;
                let noise_val = self.noise.fractal_2d(nx, ny, config);
                let uplift = self.config.avg_uplift_rate_mm_yr * noise_val;
                map.set(x, y, uplift);
            }
        }
        map
    }

    pub fn hotspot_map(&self, width: usize, height: usize) -> HeightMap {
        let mut map = HeightMap::new(width, height);
        let hs_noise = NoiseGenerator::new(42);

        let count = self.config.hotspot_count;
        for _ in 0..count {
            let cx = rand::random::<f64>() * width as f64;
            let cy = rand::random::<f64>() * height as f64;
            for y in 0..height {
                for x in 0..width {
                    let dx = (x as f64 - cx) / (width as f64 * 0.1);
                    let dy = (y as f64 - cy) / (height as f64 * 0.1);
                    let val = hs_noise.simplex_2d(dx, dy).max(0.0);
                    map.add(x, y, val * self.config.hotspot_strength_m);
                }
            }
        }
        map
    }
}
