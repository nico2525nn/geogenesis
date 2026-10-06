/** Shared types between the UI thread and the engine worker. */

export type ColorMode = "relief" | "hypsometric" | "slope" | "biome";

/** Flat parameter map sent to the engine (keys match the WASM/CLI schema). */
export type ParamValues = Record<string, number | string>;

export interface EngineStats {
  min: number;
  max: number;
  mean: number;
  std: number;
  median: number;
  p10: number;
  p90: number;
  cells: number;
  slope_mean: number;
  slope_max: number;
  ruggedness: number;
  land_fraction: number;
  hypsometric_integral: number;
  elapsed_yr: number;
}

export interface GenerateResult {
  ok: boolean;
  error?: string;
  width?: number;
  height?: number;
  seaLevel?: number;
}

export type WorkerRequest =
  | { id: number; kind: "generate"; params: ParamValues }
  | { id: number; kind: "step"; steps: number }
  | { id: number; kind: "run" }
  | { id: number; kind: "elevation" }
  | { id: number; kind: "stats" };

export type WorkerResponse =
  | { id: number; kind: "ready" }
  | { id: number; kind: "generated"; result: GenerateResult }
  | { id: number; kind: "stepped"; steps: number; elapsedYr: number }
  | { id: number; kind: "elevation"; width: number; height: number; seaLevel: number; data: Float64Array }
  | { id: number; kind: "biomes"; width: number; height: number; data: Uint8Array }
  | { id: number; kind: "stats"; stats: EngineStats }
  | { id: number; kind: "error"; message: string };
