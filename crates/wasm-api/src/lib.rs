use geogenesis_core::sim::{Simulation, SimulationConfig};
use geogenesis_core::world::WorldConfig;
use wasm_bindgen::prelude::*;

/// WASM 経由で地形生成を実行
#[wasm_bindgen]
pub struct GeoGenesisEngine {
    sim: Option<Simulation>,
}

#[wasm_bindgen]
impl GeoGenesisEngine {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self { sim: None }
    }

    /// 設定からシミュレーションを初期化
    #[wasm_bindgen]
    pub fn init(&mut self, world_json: &str, config_json: &str) -> Result<(), JsValue> {
        let world: WorldConfig = serde_json::from_str(world_json)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        let config: SimulationConfig = serde_json::from_str(config_json)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        self.sim = Some(Simulation::new(world, config));
        Ok(())
    }

    /// 初期標高マップを生成
    #[wasm_bindgen]
    pub fn generate_initial(&mut self) -> Result<(), JsValue> {
        let sim = self.sim.as_mut().ok_or(JsValue::from_str("Not initialized"))?;
        sim.generate_initial_heightmap();
        Ok(())
    }

    /// 1ステップ実行
    #[wasm_bindgen]
    pub fn step(&mut self, dt: f64) -> Result<(), JsValue> {
        let sim = self.sim.as_mut().ok_or(JsValue::from_str("Not initialized"))?;
        sim.step(dt);
        Ok(())
    }

    /// 全シミュレーション実行
    #[wasm_bindgen]
    pub fn run(&mut self) -> Result<(), JsValue> {
        let sim = self.sim.as_mut().ok_or(JsValue::from_str("Not initialized"))?;
        sim.run();
        Ok(())
    }

    /// 標高データを取得（f64 配列）
    #[wasm_bindgen]
    pub fn get_heightmap(&self) -> Result<Vec<f64>, JsValue> {
        let sim = self.sim.as_ref().ok_or(JsValue::from_str("Not initialized"))?;
        Ok(sim.heightmap.data.clone())
    }

    /// グリッド幅を取得
    #[wasm_bindgen]
    pub fn get_width(&self) -> Result<u32, JsValue> {
        let sim = self.sim.as_ref().ok_or(JsValue::from_str("Not initialized"))?;
        Ok(sim.world.grid_size.0)
    }

    /// グリッド高さを取得
    #[wasm_bindgen]
    pub fn get_height(&self) -> Result<u32, JsValue> {
        let sim = self.sim.as_ref().ok_or(JsValue::from_str("Not initialized"))?;
        Ok(sim.world.grid_size.1)
    }

    /// 現在の経過時間を取得
    #[wasm_bindgen]
    pub fn get_current_time(&self) -> Result<f64, JsValue> {
        let sim = self.sim.as_ref().ok_or(JsValue::from_str("Not initialized"))?;
        Ok(sim.current_time)
    }

    /// 統計情報を取得
    #[wasm_bindgen]
    pub fn get_statistics(&self) -> Result<String, JsValue> {
        let sim = self.sim.as_ref().ok_or(JsValue::from_str("Not initialized"))?;
        let stats = sim.statistics();
        serde_json::to_string(&stats).map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// 標高マップを指定形式でエクスポート
    #[wasm_bindgen]
    pub fn export_heightmap(&self, format: &str) -> Result<Vec<u8>, JsValue> {
        let sim = self.sim.as_ref().ok_or(JsValue::from_str("Not initialized"))?;
        let bytes: Vec<u8> = sim.heightmap.data
            .iter()
            .flat_map(|&v| v.to_le_bytes())
            .collect();

        match format {
            "raw" => Ok(bytes),
            "json" => Ok(serde_json::to_string(&sim.heightmap.data)
                .map_err(|e| JsValue::from_str(&e.to_string()))?
                .into_bytes()),
            _ => Err(JsValue::from_str(&format!("Unknown format: {}", format))),
        }
    }
}
