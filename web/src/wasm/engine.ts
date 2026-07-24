/**
 * WASM バインディングラッパー
 * 開発時はソフトウェアフォールバック、本番時は wasm モジュールを使用
 */

import type { TerrainStatistics } from '../types';

export interface EngineHandle {
  init(worldJson: string, configJson: string): Promise<void>;
  generateInitial(): Promise<void>;
  step(dt: number): Promise<void>;
  run(): Promise<void>;
  getHeightmap(): Promise<Float64Array>;
  getWidth(): Promise<number>;
  getHeight(): Promise<number>;
  getStatistics(): Promise<string>;
}

/**
 * WASM エンジンを非同期に初期化
 */
export async function createEngine(): Promise<EngineHandle> {
  try {
    // Vite に静的に解決されないよう変数経由の動的インポート
    const wasmId = 'geogenesis-wasm';
    const module = await import(wasmId);
    const engine = new module.GeoGenesisEngine();

    return {
      async init(worldJson: string, configJson: string) {
        engine.init(worldJson, configJson);
      },
      async generateInitial() {
        engine.generateInitial();
      },
      async step(dt: number) {
        engine.step(dt);
      },
      async run() {
        engine.run();
      },
      async getHeightmap(): Promise<Float64Array> {
        const data: number[] = engine.getHeightmap();
        return new Float64Array(data);
      },
      async getWidth() {
        return engine.getWidth();
      },
      async getHeight() {
        return engine.getHeight();
      },
      async getStatistics() {
        return engine.getStatistics();
      },
    };
  } catch {
    // WASM 未ビルド → ソフトウェアフォールバック (WebWorker で Core 計算)
    console.warn('WASM module not available, using JS fallback generator');
    return createFallbackEngine();
  }
}

/**
 * ソフトウェアフォールバックエンジン
 * WASM なしでも動作する簡易地形生成
 */
function createFallbackEngine(): EngineHandle {
  let width = 256;
  let height = 256;
  let heightmap = new Float64Array(width * height);
  let initialized = false;

  function generateNoise(x: number, y: number, seed: number): number {
    // 簡易ハッシュベースノイズ（Sin ベース）
    let n = Math.sin(x * 12.9898 + y * 78.233 + seed * 42.13) * 43758.5453;
    return n - Math.floor(n);
  }

  function fractalNoise(x: number, y: number, octaves: number, seed: number): number {
    let value = 0;
    let amplitude = 1;
    let frequency = 1;
    let maxAmp = 0;

    for (let i = 0; i < octaves; i++) {
      value += generateNoise(x * frequency, y * frequency, seed + i) * amplitude;
      maxAmp += amplitude;
      amplitude *= 0.5;
      frequency *= 2;
    }
    return value / maxAmp;
  }

  function generateTerrain() {
    const vScale = 5000;
    for (let y = 0; y < height; y++) {
      for (let x = 0; x < width; x++) {
        const nx = (x / width) * 8;
        const ny = (y / height) * 8;

        const large = fractalNoise(nx * 0.2, ny * 0.2, 3, 42) * 0.7;
        const detail = fractalNoise(nx, ny, 5, 99) * 0.3;
        const elevation = (large + detail) * vScale;
        heightmap[y * width + x] = elevation;
      }
    }
  }

  return {
    async init(_worldJson: string, _configJson: string) {
      // パースは省略、デフォルト値使用
      initialized = true;
    },
    async generateInitial() {
      generateTerrain();
    },
    async step(_dt: number) {
      // フォールバックでは侵食シミュレーションはスキップ
    },
    async run() {
      // 同上
    },
    async getHeightmap() {
      return heightmap.slice();
    },
    async getWidth() {
      return width;
    },
    async getHeight() {
      return height;
    },
    async getStatistics() {
      const data = heightmap;
      let min = Infinity, max = -Infinity, sum = 0;
      for (const v of data) {
        if (v < min) min = v;
        if (v > max) max = v;
        sum += v;
      }
      const mean = sum / data.length;
      const stats: TerrainStatistics = {
        minElevation: min,
        maxElevation: max,
        meanElevation: mean,
        stdDev: 0,
        median: 0,
        totalCells: data.length,
      };
      return JSON.stringify(stats);
    },
  };
}
