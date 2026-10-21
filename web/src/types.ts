// Shared types for the GeoGenesis Web UI and the engine Worker.
//
// These types mirror the JSON produced by the WASM ABI (`gg_schema_ptr`,
// `gg_state_ptr`, `gg_stats_ptr`, ...) and are the contract between the main
// thread and the worker. Keep them in sync with `crates/wasm/src/lib.rs`.

/** Display modes the renderer supports. Each maps to a WASM field id. */
export type ColorMode =
  | "terrain"
  | "biome"
  | "temperature"
  | "precipitation"
  | "vegetation"
  | "snow"
  | "slope"
  | "rock"
  | "erodibility"
  | "sediment"
  | "soil"
  | "ice"
  | "erosion"
  | "river"
  | "lake"
  | "uplift"
  | "aridity"
  | "minerals"
  | "mineral_richness"
  | "mineral_deposits";

/** Modes whose data is categorical (palette lookup, no continuous ramp). */
export const CATEGORICAL_MODES: ReadonlySet<ColorMode> = new Set<ColorMode>([
  "biome",
  "rock",
  "river",
]);

/** Human labels for the colour-mode selector. */
export const COLOR_MODE_LABELS: ReadonlyArray<{ key: ColorMode; label: string; labelJa: string }> = [
  { key: "terrain", label: "Terrain", labelJa: "地形" },
  { key: "biome", label: "Biomes", labelJa: "バイオーム" },
  { key: "temperature", label: "Temperature", labelJa: "気温" },
  { key: "precipitation", label: "Precipitation", labelJa: "降水" },
  { key: "vegetation", label: "Vegetation", labelJa: "植生" },
  { key: "snow", label: "Snow & ice", labelJa: "積雪・氷" },
  { key: "slope", label: "Slope", labelJa: "傾斜" },
  { key: "rock", label: "Bedrock", labelJa: "岩石" },
  { key: "erodibility", label: "Erodibility", labelJa: "侵食性" },
  { key: "sediment", label: "Sediment", labelJa: "堆積物" },
  { key: "soil", label: "Soil", labelJa: "土壌" },
  { key: "ice", label: "Ice thickness", labelJa: "氷厚" },
  { key: "erosion", label: "Cumulative erosion", labelJa: "侵食量" },
  { key: "river", label: "Rivers", labelJa: "河川" },
  { key: "lake", label: "Lakes", labelJa: "湖" },
  { key: "uplift", label: "Tectonic uplift", labelJa: "隆起速度" },
  { key: "aridity", label: "Aridity", labelJa: "乾燥度" },
  { key: "minerals", label: "Minerals", labelJa: "鉱物" },
  { key: "mineral_richness", label: "Mineral richness", labelJa: "鉱床ポテンシャル" },
  { key: "mineral_deposits", label: "Mineral deposits", labelJa: "鉱床分布" },
];

/** One parameter group (UI section). */
export interface ParamGroupSpec {
  key: string;
  label: string;
  label_ja: string;
}

/** One schema row, mirroring `params::ParamSpec`. */
export interface ParamSpec {
  key: string;
  label: string;
  label_ja: string;
  group: string;
  unit: string;
  min: number;
  max: number;
  default: number;
  scale: "linear" | "log";
  kind: "float" | "int" | "enum";
  options?: string[];
  help: string;
}

/** Full parameter schema returned by `gg_schema_ptr`. */
export interface Schema {
  version: string;
  groups: ParamGroupSpec[];
  params: ParamSpec[];
}

/** A preset, mirroring `params::presets::Preset`. */
export interface PresetSpec {
  key: string;
  label: string;
  label_ja: string;
  description: string;
  overrides: Record<string, number>;
}

/** Biome catalogue entry. */
export interface BiomeInfo {
  id: number;
  key: string;
  label: string;
  label_ja: string;
  colour: [number, number, number];
}

/** Rock catalogue entry. */
export interface RockInfo {
  id: number;
  key: string;
  label: string;
  label_ja: string;
  colour: [number, number, number];
}

/** Mass ledger from the simulation. Volumes in m³. */
export interface Ledger {
  erosion_m3: number;
  deposition_m3: number;
  export_m3: number;
  storage_m3: number;
  closure_error_m3: number;
}

/** Timeline frame metadata (no pixel data). */
export interface FrameMeta {
  time: number;
  sea: number;
  min: number;
  max: number;
  mean: number;
  land: number;
}

/** Grid description. */
export interface GridInfo {
  width: number;
  height: number;
  cell_km: number;
  extent_km: number;
}

/** Simulation state summary (`gg_state_ptr`). */
export interface EngineState {
  elapsed_yr: number;
  total_yr: number;
  iteration: number;
  step_yr: number;
  progress: number;
  sea_level_m: number;
  finished: boolean;
  grid: GridInfo;
  ledger: Ledger;
  history: {
    count: number;
    memory_bytes: number;
    frames: FrameMeta[];
  };
}

/** A renderable surface frame. */
export interface TerrainData {
  width: number;
  height: number;
  elevation: Float32Array;
  seaLevel: number;
  time: number;
  /** Optional extra overlay fields, keyed by mode. */
  fields?: Partial<Record<ColorMode, Float32Array>>;
}

/** Elevation statistics block. */
export interface ElevationStats {
  min: number;
  max: number;
  mean: number;
  std: number;
  median: number;
  p10: number;
  p90: number;
  cells: number;
}

/** Slope statistics block. */
export interface SlopeStats {
  mean_deg: number;
  max_deg: number;
  p90_deg: number;
  mean_gradient: number;
}

/** Hypsometry block. */
export interface HypsometryStats {
  land_fraction: number;
  ocean_fraction: number;
  mean_land_m: number;
  integral: number;
  curve: Array<[number, number]>;
}

/** Biome share row. */
export interface BiomeShare {
  key: string;
  label: string;
  label_ja: string;
  fraction: number;
}

/** Full statistics report (`gg_stats_ptr`). */
export interface EngineStats {
  grid: { width: number; height: number; cell_km: number };
  elapsed_yr: number;
  sea_level_m: number;
  elevation: ElevationStats;
  slope: SlopeStats;
  hypsometry: HypsometryStats;
  biomes: BiomeShare[];
  climate: { mean_temp_c: number; mean_precip_mm: number };
  hydrology: {
    drainage_density: number;
    channel_cells: number;
    lake_volume_km3: number;
  };
  land_area_km2: number;
  max_relief_m: number;
  mean_erosion_m: number;
  mean_sediment_m: number;
  mean_ice_m: number;
}

/** One cross-section sample. */
export interface ProfileSample {
  distance_m: number;
  elevation_m: number;
  water_depth_m: number;
  rock: number;
  biome: number;
}

/** Cross-section line in cell coordinates. */
export interface ProfileLine {
  a: [number, number];
  b: [number, number];
}

// ---------------------------------------------------------------------------
// Worker protocol
// ---------------------------------------------------------------------------

/** Requests from the main thread to the engine worker. */
/** One entry of the engine's mineral catalogue. */
export interface MineralInfo {
  key: string;
  label: string;
  label_ja: string;
  symbol: string;
  category: string;
  colour: [number, number, number];
  unit_value_usd_per_t: number;
  endowment_t_per_km2: number;
  deposit_threshold: number;
  affinity: string;
}

/** One line of the mineral inventory report (join `index` with `minerals`). */
export interface MineralEntry {
  index: number;
  key: string;
  cells: number;
  coverage: number;
  area_km2: number;
  mean_richness: number;
  tonnage_t: number;
  unit_value_usd_per_t: number;
  value_usd: number;
}

/** Mineral map summary + inventory for the current world. */
export interface MineralsReport {
  minerals: MineralInfo[];
  map: {
    width: number;
    height: number;
    cells: number;
    deposit_cells: number;
    deposit_area_km2: number;
    mean_richness: number;
    dominant: Array<{ index: number; key: string; fraction: number }>;
  };
  inventory: {
    grid_width: number;
    grid_height: number;
    cell_area_km2: number;
    deposit_cells: number;
    deposit_area_km2: number;
    mean_richness: number;
    total_value_usd: number;
    entries: MineralEntry[];
  };
}

export type WorkerRequest =
  | { id: number; kind: "init" }
  | {
      id: number;
      kind: "generate";
      params: Float64Array;
      rocksJson?: string;
      strataJson?: string;
    }
  | { id: number; kind: "step"; steps: number }
  | { id: number; kind: "run" }
  | { id: number; kind: "seek"; year: number }
  | { id: number; kind: "field"; mode: ColorMode }
  | { id: number; kind: "stats" }
  | { id: number; kind: "state" }
  | { id: number; kind: "historyFrame"; index: number }
  | { id: number; kind: "profile"; line: ProfileLine; samples: number }
  | { id: number; kind: "preview"; params: Float64Array; grid: number }
  | { id: number; kind: "minerals" }
  | { id: number; kind: "geology" }
  | {
      id: number;
      kind: "export";
      format:
        | "obj"
        | "xyz"
        | "raw"
        | "params"
        | "stats"
        | "height16"
        | "normals"
        | "gltf"
        | "profile"
        | "exr"
        | "netcdf";
    };

/** Responses from the engine worker. `id` echoes the request (0 for pushes). */
export type WorkerResponse =
  | {
      id: number;
      kind: "ready";
      schema: Schema;
      presets: PresetSpec[];
      biomes: BiomeInfo[];
      rocks: RockInfo[];
    }
  | { id: number; kind: "generated"; state: EngineState; terrain: TerrainData }
  | { id: number; kind: "progress"; phase: "run" | "seek"; state: EngineState; terrain?: TerrainData }
  | { id: number; kind: "stepped"; state: EngineState; terrain: TerrainData }
  | { id: number; kind: "state"; state: EngineState }
  | { id: number; kind: "field"; mode: ColorMode; values: Float32Array }
  | { id: number; kind: "stats"; stats: EngineStats }
  | { id: number; kind: "historyFrame"; index: number; time: number; elevation: Float32Array }
  | { id: number; kind: "profile"; samples: ProfileSample[] }
  | { id: number; kind: "preview"; terrain: TerrainData }
  | { id: number; kind: "minerals"; report: MineralsReport }
  | { id: number; kind: "geology"; json: string }
  | {
      id: number;
      kind: "export";
      format: string;
      text?: string;
      bytes?: Uint8Array;
    }
  | { id: number; kind: "error"; message: string };

/** WASM field ids, mirroring `sim::FieldKind` and slope = 8. */
export const FIELD_IDS: Readonly<Record<ColorMode, number>> = {
  terrain: 0,
  biome: 1,
  temperature: 2,
  precipitation: 3,
  vegetation: 4,
  snow: 5,
  rock: 6,
  erodibility: 7,
  slope: 8,
  sediment: 9,
  soil: 10,
  ice: 11,
  erosion: 12,
  river: 13,
  lake: 14,
  uplift: 15,
  aridity: 16,
  minerals: 17,
  mineral_richness: 18,
  mineral_deposits: 19,
};
