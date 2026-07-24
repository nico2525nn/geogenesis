use crate::world::HeightMap;
use serde::{Deserialize, Serialize};
use super::ErosionProcess;

/// 河流侵食方式
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum StreamPowerModel {
    /// Stream Power (古典的)
    SP,
    /// Modified SP (堆積項・輸送限界項追加)
    ModifiedSP,
    /// ダンパー・ブラウン（短距離斜面輸送）
    DampBrown,
    /// Tucker-Slingerland（土壌厚考慮）
    TuckerSlingerland,
    /// 物理ベース完全式
    PhysicsBased,
}

/// 河流侵食設定
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FluvialConfig {
    pub kf: f64,
    pub kd: f64,
    pub basin_area_threshold: u32,
    pub channel_slope_index: f64,
    pub channel_width_coef: f64,
    pub bed_roughness: f64,
    pub infiltration_rate: f64,
    pub model: StreamPowerModel,
}

impl Default for FluvialConfig {
    fn default() -> Self {
        Self {
            kf: 1e-5,
            kd: 0.01,
            basin_area_threshold: 10,
            channel_slope_index: 0.5,
            channel_width_coef: 1.0,
            bed_roughness: 0.1,
            infiltration_rate: 0.3,
            model: StreamPowerModel::SP,
        }
    }
}

/// 河流侵食プロセス
pub struct FluvialErosion {
    pub config: FluvialConfig,
}

impl FluvialErosion {
    pub fn new(config: FluvialConfig) -> Self {
        Self { config }
    }

    /// D8 単一流向アルゴリズム: 各セルの流量を計算
    fn compute_flow_accumulation(&self, heightmap: &HeightMap) -> Vec<f64> {
        let w = heightmap.width;
        let h = heightmap.height;
        let mut flow = vec![0.0; w * h];
        let mut order: Vec<usize> = (0..w * h).collect();
        order.sort_by(|&a, &b| {
            heightmap.data[b].partial_cmp(&heightmap.data[a]).unwrap_or(std::cmp::Ordering::Equal)
        });

        let dx: [i64; 8] = [-1, 0, 1, -1, 1, -1, 0, 1];
        let dy: [i64; 8] = [-1, -1, -1, 0, 0, 1, 1, 1];

        for &idx in &order {
            let x = (idx % w) as i64;
            let y = (idx / w) as i64;
            let cell_h = heightmap.data[idx];

            // 最急降下方向を探す
            let mut min_slope = 0.0;
            let mut flow_to = idx;
            for d in 0..8 {
                let nx = x + dx[d];
                let ny = y + dy[d];
                if nx < 0 || nx >= w as i64 || ny < 0 || ny >= h as i64 {
                    continue;
                }
                let nidx = (ny as usize) * w + (nx as usize);
                let dh = cell_h - heightmap.data[nidx];
                let dist = if d % 2 == 0 { 1.414 } else { 1.0 };
                let slope = dh / dist;
                if slope > min_slope {
                    min_slope = slope;
                    flow_to = nidx;
                }
            }

            if flow_to != idx {
                flow[flow_to] += flow[idx] + 1.0;
            }
        }
        flow
    }

    /// 多流向 (MFD) アルゴリズム
    fn compute_mfd_flow(&self, heightmap: &HeightMap) -> Vec<f64> {
        let w = heightmap.width;
        let h = heightmap.height;
        let mut flow = vec![0.0; w * h];
        let p = 1.1; // MFD 重みパラメータ

        let dx: [i64; 8] = [-1, 0, 1, -1, 1, -1, 0, 1];
        let dy: [i64; 8] = [-1, -1, -1, 0, 0, 1, 1, 1];
        let dist: [f64; 8] = [1.414, 1.0, 1.414, 1.0, 1.0, 1.414, 1.0, 1.414];

        for y in 0..h {
            for x in 0..w {
                let idx = y * w + x;
                let cell_h = heightmap.data[idx];

                let mut total_weight = 0.0;
                for d in 0..8 {
                    let nx = x as i64 + dx[d];
                    let ny = y as i64 + dy[d];
                    if nx < 0 || nx >= w as i64 || ny < 0 || ny >= h as i64 {
                        continue;
                    }
                    let nidx = (ny as usize) * w + (nx as usize);
                    let dh = cell_h - heightmap.data[nidx];
                    if dh > 0.0 {
                        total_weight += (dh / dist[d]).powf(p);
                    }
                }

                if total_weight > 0.0 {
                    for d in 0..8 {
                        let nx = x as i64 + dx[d];
                        let ny = y as i64 + dy[d];
                        if nx < 0 || nx >= w as i64 || ny < 0 || ny >= h as i64 {
                            continue;
                        }
                        let nidx = (ny as usize) * w + (nx as usize);
                        let dh = cell_h - heightmap.data[nidx];
                        if dh > 0.0 {
                            let weight = (dh / dist[d]).powf(p) / total_weight;
                            flow[nidx] += weight * (flow[idx] + 1.0);
                        }
                    }
                }
            }
        }
        flow
    }
}

impl ErosionProcess for FluvialErosion {
    fn apply(&self, heightmap: &mut HeightMap, dt: f64) {
        let flow = self.compute_flow_accumulation(heightmap);
        let w = heightmap.width;

        for y in 0..heightmap.height {
            for x in 0..heightmap.width {
                let idx = y * w + x;
                let area = flow[idx].max(1.0);
                let slope = heightmap.slope(x, y);

                // Stream Power モデル: E = Kf * A^m * S^n
                let m = 0.5;
                let n = self.config.channel_slope_index;
                let erosion = self.config.kf * area.powf(m) * slope.powf(n) * dt;

                let new_h = heightmap.data[idx] - erosion;
                if new_h > -10000.0 {
                    heightmap.data[idx] = new_h;
                }
            }
        }
    }

    fn name(&self) -> &'static str {
        "fluvial"
    }
}
