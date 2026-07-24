use serde::{Deserialize, Serialize};

/// 座標系
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum CoordinateSystem {
    /// 平面（無限大）
    Planar,
    /// 球面正距円筒
    Equirectangular,
    /// 球面六角
    SphericalHex,
}

/// ワールド境界
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum WorldBoundary {
    /// 矩形
    Rectangular,
    /// トーラス
    Toroidal,
    /// 球面
    Spherical,
}

/// ワールド設定
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldConfig {
    pub world_name: String,
    pub seed: u64,
    pub grid_size: (u32, u32),
    pub tile_divisions: (u32, u32),
    pub spatial_scale_km: f64,
    pub vertical_scale_m: f64,
    pub sea_level_m: f64,
    pub planet_radius_m: f64,
    pub coordinate_system: CoordinateSystem,
    pub boundary: WorldBoundary,
}

impl Default for WorldConfig {
    fn default() -> Self {
        Self {
            world_name: "Untitled".into(),
            seed: 0,
            grid_size: (512, 512),
            tile_divisions: (1, 1),
            spatial_scale_km: 100.0,
            vertical_scale_m: 5000.0,
            sea_level_m: 0.0,
            planet_radius_m: 6_371_000.0,
            coordinate_system: CoordinateSystem::Planar,
            boundary: WorldBoundary::Rectangular,
        }
    }
}

impl WorldConfig {
    pub fn total_cells(&self) -> usize {
        self.grid_size.0 as usize * self.grid_size.1 as usize
    }

    pub fn tile_width(&self) -> u32 {
        self.grid_size.0 / self.tile_divisions.0
    }

    pub fn tile_height(&self) -> u32 {
        self.grid_size.1 / self.tile_divisions.1
    }

    pub fn cell_area_m2(&self) -> f64 {
        let w = self.spatial_scale_km * 1000.0 / self.grid_size.0 as f64;
        let h = self.spatial_scale_km * 1000.0 / self.grid_size.1 as f64;
        w * h
    }
}

/// 標高マップ（2次元グリッド）
#[derive(Debug, Clone)]
pub struct HeightMap {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f64>,
}

impl HeightMap {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            data: vec![0.0; width * height],
        }
    }

    pub fn from_vec(width: usize, height: usize, data: Vec<f64>) -> Self {
        assert_eq!(data.len(), width * height);
        Self { width, height, data }
    }

    pub fn get(&self, x: usize, y: usize) -> f64 {
        self.data[y * self.width + x]
    }

    pub fn get_wrap(&self, x: i64, y: i64) -> f64 {
        let x = x.rem_euclid(self.width as i64) as usize;
        let y = y.rem_euclid(self.height as i64) as usize;
        self.get(x, y)
    }

    pub fn set(&mut self, x: usize, y: usize, value: f64) {
        self.data[y * self.width + x] = value;
    }

    pub fn add(&mut self, x: usize, y: usize, delta: f64) {
        self.data[y * self.width + x] += delta;
    }

    pub fn min(&self) -> f64 {
        self.data.iter().cloned().fold(f64::INFINITY, f64::min)
    }

    pub fn max(&self) -> f64 {
        self.data.iter().cloned().fold(f64::NEG_INFINITY, f64::max)
    }

    pub fn normalize(&mut self, target_min: f64, target_max: f64) {
        let mn = self.min();
        let mx = self.max();
        let range = mx - mn;
        if range > 0.0 {
            for v in &mut self.data {
                *v = target_min + (*v - mn) / range * (target_max - target_min);
            }
        }
    }

    pub fn slope(&self, x: usize, y: usize) -> f64 {
        let cx = x as i64;
        let cy = y as i64;
        let dzdx = (self.get_wrap(cx + 1, cy) - self.get_wrap(cx - 1, cy)) / 2.0;
        let dzdy = (self.get_wrap(cx, cy + 1) - self.get_wrap(cx, cy - 1)) / 2.0;
        (dzdx.powi(2) + dzdy.powi(2)).sqrt()
    }
}
