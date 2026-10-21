// Colour tables shared by the GPU terrain shader and the 2D minimap.
//
// Every palette is derived from `web/src/colors.ts` so the 3D viewport, the
// minimap and the legend never drift apart. Each palette is baked once into
// two lookup tables:
//
//   * `srgb`     – 8-bit sRGB triplets, used by the canvas minimap,
//   * `linearRgba` – float RGBA triplets in linear light, used by three.js
//                    data textures (the shaders work in linear space and let
//                    three convert the final colour to sRGB on output).
//
// The tables remove per-frame allocations and keep the hot loops (minimap
// rasterisation, vertex displacement) branch-light.

import {
  categoricalColour,
  LAND_STOPS,
  MODE_RAMPS,
  mineralColour,
  OCEAN_STOPS,
  rampColour,
  type Ramp,
} from "../colors";
import type { BiomeInfo, ColorMode, MineralInfo, RockInfo } from "../types";

/** Palette-indexed modes (one colour per integer cell value). */
export type CategoricalMode =
  | "biome"
  | "rock"
  | "river"
  | "minerals"
  | "mineral_deposits";
/** Ramp modes; must stay in sync with `MODE_RAMPS` in colors.ts. */
export type ContinuousMode = Exclude<ColorMode, "terrain" | CategoricalMode>;

/** One baked palette. */
export interface PaletteLut {
  /** Linear-light RGBA, `size * 4` floats, for GPU data textures. */
  readonly linearRgba: Float32Array;
  /** 8-bit sRGB RGB, `size * 3` bytes, for the minimap. */
  readonly srgb: Uint8Array;
  readonly size: number;
}

/** Land ramp subdivision used by `colors.ts`'s `terrainColour`. */
export const LAND_SPAN_M = 2600;
/** Ocean ramp subdivision used by `colors.ts`'s `terrainColour`. */
export const OCEAN_SPAN_M = 6000;
export const LAND_LUT_SIZE = 512;
export const OCEAN_LUT_SIZE = 256;

function srgbToLinear(value: number): number {
  const c = value / 255;
  return c <= 0.04045 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
}

function bake(stops: Ramp, size: number): PaletteLut {
  const srgb = new Uint8Array(size * 3);
  const linearRgba = new Float32Array(size * 4);
  for (let i = 0; i < size; i += 1) {
    const t = size === 1 ? 0 : i / (size - 1);
    const colour = rampColour(stops, t);
    srgb[i * 3] = colour[0];
    srgb[i * 3 + 1] = colour[1];
    srgb[i * 3 + 2] = colour[2];
    linearRgba[i * 4] = srgbToLinear(colour[0]);
    linearRgba[i * 4 + 1] = srgbToLinear(colour[1]);
    linearRgba[i * 4 + 2] = srgbToLinear(colour[2]);
    linearRgba[i * 4 + 3] = 1;
  }
  return { linearRgba, srgb, size };
}

/** Hypsometric land ramp (beach → lowland → forest → rock → snow). */
export const LAND_LUT: PaletteLut = bake(LAND_STOPS, LAND_LUT_SIZE);
/** Bathymetric ocean ramp (shelf teal → abyssal navy). */
export const OCEAN_LUT: PaletteLut = bake(OCEAN_STOPS, OCEAN_LUT_SIZE);

/** Description of a continuous mode's value range (mirrors the legend). */
export interface ModeRange {
  min: number;
  max: number;
  span: number;
  unit: string;
  title: string;
}

const rangeCache = new Map<ContinuousMode, ModeRange>();
const paletteCache = new Map<ContinuousMode, PaletteLut>();

function rampOf(mode: ContinuousMode) {
  return MODE_RAMPS[mode];
}

/** Value range of a continuous overlay mode. */
export function modeRange(mode: ContinuousMode): ModeRange {
  const cached = rangeCache.get(mode);
  if (cached !== undefined) {
    return cached;
  }
  const ramp = rampOf(mode);
  const span = Math.max(ramp.max - ramp.min, 1e-9);
  const range: ModeRange = {
    min: ramp.min,
    max: ramp.max,
    span,
    unit: ramp.unit,
    title: ramp.title,
  };
  rangeCache.set(mode, range);
  return range;
}

/** Baked ramp for a continuous overlay mode. */
export function continuousPalette(mode: ContinuousMode): PaletteLut {
  let lut = paletteCache.get(mode);
  if (lut === undefined) {
    lut = bake(rampOf(mode).stops, 512);
    paletteCache.set(mode, lut);
  }
  return lut;
}

// ---------------------------------------------------------------------------
// Categorical palettes
// ---------------------------------------------------------------------------

/** Catalogues mirroring the Rust engine, used until main.ts supplies them. */
export const FALLBACK_BIOMES: readonly BiomeInfo[] = [
  { id: 0, key: "ocean", label: "Ocean", label_ja: "海洋", colour: [30, 70, 120] },
  { id: 1, key: "tropical_rainforest", label: "Tropical rainforest", label_ja: "熱帯雨林", colour: [16, 92, 42] },
  { id: 2, key: "savanna", label: "Savanna", label_ja: "サバンナ", colour: [154, 150, 62] },
  { id: 3, key: "desert", label: "Desert", label_ja: "砂漠", colour: [214, 183, 120] },
  { id: 4, key: "grassland", label: "Grassland", label_ja: "草原", colour: [126, 155, 80] },
  { id: 5, key: "temperate_forest", label: "Temperate forest", label_ja: "温帯林", colour: [54, 112, 62] },
  { id: 6, key: "mediterranean", label: "Mediterranean", label_ja: "地中海性", colour: [128, 138, 78] },
  { id: 7, key: "boreal_forest", label: "Boreal forest", label_ja: "針葉樹林", colour: [40, 84, 66] },
  { id: 8, key: "tundra", label: "Tundra", label_ja: "ツンドラ", colour: [134, 143, 128] },
  { id: 9, key: "ice", label: "Ice", label_ja: "氷", colour: [235, 241, 245] },
  { id: 10, key: "alpine", label: "Alpine", label_ja: "高山", colour: [148, 146, 140] },
  { id: 11, key: "wetland", label: "Wetland", label_ja: "湿地", colour: [78, 124, 104] },
];

export const FALLBACK_ROCKS: readonly RockInfo[] = [
  { id: 0, key: "granite", label: "Granite", label_ja: "花崗岩", colour: [196, 176, 166] },
  { id: 1, key: "basalt", label: "Basalt", label_ja: "玄武岩", colour: [90, 92, 100] },
  { id: 2, key: "limestone", label: "Limestone", label_ja: "石灰岩", colour: [219, 213, 188] },
  { id: 3, key: "sandstone", label: "Sandstone", label_ja: "砂岩", colour: [204, 165, 114] },
  { id: 4, key: "shale", label: "Shale", label_ja: "頁岩", colour: [120, 120, 128] },
  { id: 5, key: "schist", label: "Schist", label_ja: "片岩", colour: [170, 160, 150] },
  { id: 6, key: "gneiss", label: "Gneiss", label_ja: "片麻岩", colour: [130, 140, 145] },
  { id: 7, key: "marble", label: "Marble", label_ja: "大理石", colour: [230, 226, 220] },
  { id: 8, key: "slate", label: "Slate", label_ja: "粘板岩", colour: [80, 88, 100] },
  { id: 9, key: "dolomite", label: "Dolomite", label_ja: "苦灰岩", colour: [176, 150, 128] },
  { id: 10, key: "chalk", label: "Chalk", label_ja: "白亜", colour: [205, 190, 168] },
  { id: 11, key: "greywacke", label: "Greywacke", label_ja: "硬砂岩", colour: [200, 195, 180] },
  { id: 12, key: "andesite", label: "Andesite", label_ja: "安山岩", colour: [110, 105, 98] },
  { id: 13, key: "rhyolite", label: "Rhyolite", label_ja: "流紋岩", colour: [160, 150, 140] },
];

const BENCH_SLATE: [number, number, number] = [16, 24, 36];

/** Barren / "no deposit" slate used by the mineral maps and the legend. */
export const MINERAL_BARREN: [number, number, number] = [46, 52, 62];
/** Economic deposit highlight (matches colors.ts `legendFor("mineral_deposits")`). */
export const MINERAL_DEPOSIT: [number, number, number] = [226, 186, 80];

/**
 * Number of slots in a categorical palette (index == cell value).
 *
 * `minerals` is indexed by the raw dominant-mineral index (255 = barren), so
 * the palette always spans the full u8 range even before the catalogue is
 * known; entries without a catalogue match bake grey.
 */
export function categoricalCount(
  mode: CategoricalMode,
  biomes: readonly BiomeInfo[],
  rocks: readonly RockInfo[],
  minerals: readonly MineralInfo[] = [],
): number {
  if (mode === "biome") {
    return Math.max(12, biomes.length);
  }
  if (mode === "rock") {
    return Math.max(14, rocks.length);
  }
  if (mode === "minerals") {
    void minerals;
    return 256;
  }
  if (mode === "mineral_deposits") {
    return 2;
  }
  return 3;
}

/** Bakes the palette for a categorical (palette-indexed) mode. */
export function categoricalPalette(
  mode: CategoricalMode,
  biomes: readonly BiomeInfo[],
  rocks: readonly RockInfo[],
  minerals: readonly MineralInfo[] = [],
): PaletteLut {
  const count = categoricalCount(mode, biomes, rocks, minerals);
  const srgb = new Uint8Array(Math.max(count, 1) * 3);
  const linearRgba = new Float32Array(Math.max(count, 1) * 4);
  for (let id = 0; id < count; id += 1) {
    let colour: [number, number, number];
    if (mode === "minerals") {
      // `mineralColour` handles 255 → barren slate and an empty/short
      // catalogue → grey without throwing.
      colour = mineralColour(id, minerals);
    } else if (mode === "mineral_deposits") {
      colour = id > 0 ? MINERAL_DEPOSIT : MINERAL_BARREN;
    } else {
      colour = categoricalColour(mode, id, biomes, rocks);
      if (mode === "river" && id === 0) {
        colour = BENCH_SLATE;
      }
    }
    srgb[id * 3] = colour[0];
    srgb[id * 3 + 1] = colour[1];
    srgb[id * 3 + 2] = colour[2];
    linearRgba[id * 4] = srgbToLinear(colour[0]);
    linearRgba[id * 4 + 1] = srgbToLinear(colour[1]);
    linearRgba[id * 4 + 2] = srgbToLinear(colour[2]);
    linearRgba[id * 4 + 3] = 1;
  }
  return { linearRgba, srgb, size: count };
}

// ---------------------------------------------------------------------------
// Minimap sampling helpers (hot path: inline-friendly)
// ---------------------------------------------------------------------------

/** Terrain-mode palette index for an elevation, split into land / ocean. */
export function terrainLutIndex(elevation: number, seaLevel: number): number {
  if (elevation <= seaLevel) {
    const depth = (seaLevel - elevation) / OCEAN_SPAN_M;
    const t = depth < 0 ? 0 : depth > 1 ? 1 : depth;
    return Math.round(t * (OCEAN_LUT_SIZE - 1));
  }
  const above = (elevation - seaLevel) / LAND_SPAN_M;
  const t = above < 0 ? 0 : above > 1 ? 1 : above;
  return Math.round(t * (LAND_LUT_SIZE - 1));
}

/** Writes the sRGB colour of an elevation into `out` at `offset`. */
export function terrainSrgb(
  elevation: number,
  seaLevel: number,
  out: Uint8Array,
  offset: number,
): void {
  const lut = elevation <= seaLevel ? OCEAN_LUT : LAND_LUT;
  const index = terrainLutIndex(elevation, seaLevel) * 3;
  out[offset] = lut.srgb[index] ?? 0;
  out[offset + 1] = lut.srgb[index + 1] ?? 0;
  out[offset + 2] = lut.srgb[index + 2] ?? 0;
}

/** Writes the sRGB colour of a continuous field value into `out`. */
export function continuousSrgb(
  mode: ContinuousMode,
  value: number,
  out: Uint8Array,
  offset: number,
): void {
  const lut = continuousPalette(mode);
  const range = modeRange(mode);
  const t = (value - range.min) / range.span;
  const clamped = t < 0 ? 0 : t > 1 ? 1 : t;
  const index = Math.round(clamped * (lut.size - 1)) * 3;
  out[offset] = lut.srgb[index] ?? 0;
  out[offset + 1] = lut.srgb[index + 1] ?? 0;
  out[offset + 2] = lut.srgb[index + 2] ?? 0;
}

/** Writes the sRGB colour of a categorical cell value into `out`. */
export function categoricalSrgb(
  mode: CategoricalMode,
  value: number,
  biomes: readonly BiomeInfo[],
  rocks: readonly RockInfo[],
  out: Uint8Array,
  offset: number,
  minerals: readonly MineralInfo[] = [],
): void {
  const lut = categoricalPalette(mode, biomes, rocks, minerals);
  const id = Math.round(value);
  const clamped = id < 0 ? 0 : id >= lut.size ? lut.size - 1 : id;
  out[offset] = lut.srgb[clamped * 3] ?? 0;
  out[offset + 1] = lut.srgb[clamped * 3 + 1] ?? 0;
  out[offset + 2] = lut.srgb[clamped * 3 + 2] ?? 0;
}
