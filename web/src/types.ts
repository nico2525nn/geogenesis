/** ワールド設定 */
export interface WorldConfig {
  worldName: string;
  seed: number;
  gridSize: [number, number];
  tileDivisions: [number, number];
  spatialScaleKm: number;
  verticalScaleM: number;
  seaLevelM: number;
  planetRadiusM: number;
  coordinateSystem: 'planar' | 'equirectangular' | 'spherical-hex';
  boundary: 'rectangular' | 'toroidal' | 'spherical';
}

/** シミュレーション設定 */
export interface SimulationConfig {
  totalTimeYears: number;
  baseTimeStepYears: number;
  adaptiveTimestep: boolean;
  cflCondition: boolean;
  maxIterations: number;
  tectonic: TectonicConfig;
  fluvial: FluvialConfig;
  hillslope: HillslopeConfig;
  noise: NoiseConfig;
}

export interface TectonicConfig {
  plateCount: number;
  plateThicknessKm: number;
  plateDensities: [number, number, number];
  collisionThreshold: number;
  avgUpliftRateMmYr: number;
  hotspotCount: number;
  hotspotStrengthM: number;
}

export interface FluvialConfig {
  kf: number;
  kd: number;
  basinAreaThreshold: number;
  channelSlopeIndex: number;
}

export interface HillslopeConfig {
  diffusionCoef: number;
  criticalSlopeDeg: number;
  soilProductionRate: number;
}

export interface NoiseConfig {
  noiseType: 'perlin' | 'simplex' | 'value' | 'wavelet';
  octaves: number;
  lacunarity: number;
  persistence: number;
  anisotropy: number;
}

/** 地形統計 */
export interface TerrainStatistics {
  minElevation: number;
  maxElevation: number;
  meanElevation: number;
  stdDev: number;
  median: number;
  totalCells: number;
}

/** パラメータグループ定義 */
export interface ParameterGroup {
  id: string;
  label: string;
  params: ParameterDef[];
}

export interface ParameterDef {
  id: string;
  label: string;
  type: 'number' | 'select' | 'boolean';
  min?: number;
  max?: number;
  step?: number;
  default: number | string | boolean;
  options?: string[];
}
