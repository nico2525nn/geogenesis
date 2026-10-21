// Shared colour science for the renderer, minimap and legends.
//
// Keeping ramps in one module guarantees the legend matches the 3D view and
// the minimap exactly. All functions are pure and allocation-free unless noted.

import type { BiomeInfo, ColorMode, MineralInfo, RockInfo } from "./types";

export type RGB = [number, number, number];

/** Stops of a linear ramp, sorted by `t` in `0..1`. */
export type Ramp = Array<[number, RGB]>;

/** Samples a ramp with linear interpolation. */
export function rampColour(stops: Ramp, t: number): RGB {
  const clamped = t < 0 ? 0 : t > 1 ? 1 : t;
  let lo = stops[0] ?? [0, [0, 0, 0]];
  let hi = stops[stops.length - 1] ?? [1, [255, 255, 255]];
  for (let i = 0; i < stops.length - 1; i += 1) {
    const a = stops[i];
    const b = stops[i + 1];
    if (a !== undefined && b !== undefined && clamped >= a[0] && clamped <= b[0]) {
      lo = a;
      hi = b;
      break;
    }
  }
  const span = Math.max(hi[0] - lo[0], 1e-9);
  const k = (clamped - lo[0]) / span;
  return [
    Math.round(lo[1][0] + (hi[1][0] - lo[1][0]) * k),
    Math.round(lo[1][1] + (hi[1][1] - lo[1][1]) * k),
    Math.round(lo[1][2] + (hi[1][2] - lo[1][2]) * k),
  ];
}

/** Hypsometric land ramp: beach → lowland → forest → rock → snow. */
export const LAND_STOPS: Ramp = [
  [0.0, [204, 192, 150]],
  [0.06, [150, 164, 104]],
  [0.3, [98, 148, 82]],
  [0.55, [96, 126, 88]],
  [0.75, [138, 120, 100]],
  [0.88, [172, 158, 148]],
  [1.0, [242, 246, 250]],
];

/** Ocean ramp: shelf teal → abyssal navy. */
export const OCEAN_STOPS: Ramp = [
  [0.0, [64, 132, 150]],
  [0.25, [38, 96, 138]],
  [0.6, [22, 60, 108]],
  [1.0, [10, 26, 62]],
];

/** Continuous-mode ramp definitions used by the renderer and the legend. */
export const MODE_RAMPS: Record<
  Exclude<
    ColorMode,
    "terrain" | "biome" | "rock" | "river" | "minerals" | "mineral_deposits"
  >,
  { min: number; max: number; unit: string; stops: Ramp; title: string }
> = {
  temperature: {
    min: -25,
    max: 40,
    unit: "°C",
    title: "Temperature",
    stops: [
      [0, [58, 74, 140]],
      [0.25, [80, 150, 190]],
      [0.45, [130, 195, 150]],
      [0.62, [214, 210, 120]],
      [0.8, [222, 148, 84]],
      [1, [186, 62, 54]],
    ],
  },
  precipitation: {
    min: 0,
    max: 3000,
    unit: "mm/yr",
    title: "Precipitation",
    stops: [
      [0, [122, 96, 66]],
      [0.2, [168, 150, 92]],
      [0.4, [116, 158, 94]],
      [0.65, [62, 138, 114]],
      [1, [38, 92, 154]],
    ],
  },
  vegetation: {
    min: 0,
    max: 1,
    unit: "",
    title: "Vegetation",
    stops: [
      [0, [142, 130, 112]],
      [0.35, [150, 156, 96]],
      [0.7, [92, 146, 78]],
      [1, [32, 104, 58]],
    ],
  },
  snow: {
    min: 0,
    max: 1,
    unit: "",
    title: "Snow cover",
    stops: [
      [0, [96, 100, 104]],
      [0.5, [172, 184, 196]],
      [1, [244, 250, 255]],
    ],
  },
  slope: {
    min: 0,
    max: 45,
    unit: "°",
    title: "Slope",
    stops: [
      [0, [72, 120, 96]],
      [0.35, [160, 176, 96]],
      [0.65, [204, 146, 74]],
      [1, [196, 78, 62]],
    ],
  },
  erodibility: {
    min: 0.3,
    max: 2.0,
    unit: "×",
    title: "Erodibility",
    stops: [
      [0, [86, 140, 110]],
      [0.5, [186, 168, 96]],
      [1, [196, 92, 72]],
    ],
  },
  sediment: {
    min: 0,
    max: 120,
    unit: "m",
    title: "Sediment thickness",
    stops: [
      [0, [96, 100, 104]],
      [0.5, [176, 156, 112]],
      [1, [222, 204, 158]],
    ],
  },
  soil: {
    min: 0,
    max: 3,
    unit: "m",
    title: "Soil thickness",
    stops: [
      [0, [110, 104, 96]],
      [0.5, [150, 124, 90]],
      [1, [108, 84, 58]],
    ],
  },
  ice: {
    min: 0,
    max: 1500,
    unit: "m",
    title: "Ice thickness",
    stops: [
      [0, [96, 120, 150]],
      [0.4, [150, 188, 216]],
      [1, [242, 250, 255]],
    ],
  },
  erosion: {
    min: 0,
    max: 1500,
    unit: "m",
    title: "Cumulative erosion",
    stops: [
      [0, [90, 92, 100]],
      [0.35, [148, 108, 150]],
      [0.7, [206, 132, 96]],
      [1, [240, 208, 132]],
    ],
  },
  lake: {
    min: 0,
    max: 80,
    unit: "m",
    title: "Lake depth",
    stops: [
      [0, [90, 116, 130]],
      [0.5, [58, 120, 178]],
      [1, [24, 68, 140]],
    ],
  },
  aridity: {
    min: 0.3,
    max: 6,
    unit: "PET/P",
    title: "Aridity index",
    stops: [
      [0, [52, 112, 148]],
      [0.35, [96, 158, 118]],
      [0.65, [198, 184, 108]],
      [1, [186, 108, 66]],
    ],
  },
  mineral_richness: {
    min: 0,
    max: 1,
    unit: "",
    title: "Mineral prospectivity",
    stops: [
      [0, [40, 44, 54]],
      [0.35, [92, 96, 78]],
      [0.7, [198, 166, 74]],
      [1, [255, 236, 170]],
    ],
  },
  uplift: {
    min: -10,
    max: 10,
    unit: "mm/yr",
    title: "Tectonic uplift",
    stops: [
      [0, [54, 86, 164]],
      [0.5, [196, 200, 204]],
      [1, [190, 76, 60]],
    ],
  },
};

/** Converts a raw field value into a colour for a continuous mode. */
export function modeColour(
  mode: Exclude<ColorMode, "terrain" | "biome" | "rock" | "river" | "minerals" | "mineral_deposits">,
  value: number,
): RGB {
  const ramp = MODE_RAMPS[mode];
  const t = (value - ramp.min) / Math.max(ramp.max - ramp.min, 1e-9);
  return rampColour(ramp.stops, t);
}

/** Colour for the terrain mode, given elevation, sea level and slope. */
export function terrainColour(
  elevation: number,
  seaLevel: number,
  slopeDegrees: number,
): RGB {
  if (elevation <= seaLevel) {
    const depth = (seaLevel - elevation) / 6000;
    return rampColour(OCEAN_STOPS, depth);
  }
  const above = (elevation - seaLevel) / 2600;
  const base = rampColour(LAND_STOPS, above);
  // Slope darkening gives ridges and valleys readable form even without light.
  const shade = 1 - Math.min(slopeDegrees / 55, 1) * 0.28;
  return [
    Math.round(base[0] * shade),
    Math.round(base[1] * shade),
    Math.round(base[2] * shade),
  ];
}

/** Colour for a field value in a categorical mode. */
export function categoricalColour(
  mode: "biome" | "rock" | "river",
  value: number,
  biomes: readonly BiomeInfo[],
  rocks: readonly RockInfo[],
): RGB {
  if (mode === "biome") {
    const entry = biomes.find((biome) => biome.id === Math.round(value));
    return entry?.colour ?? [110, 110, 110];
  }
  if (mode === "rock") {
    const entry = rocks.find((rock) => rock.id === Math.round(value));
    return entry?.colour ?? [130, 130, 130];
  }
  // river masks: 0 hillslope, 1 channel, 2 major river
  if (value >= 1.5) return [96, 176, 224];
  if (value >= 0.5) return [70, 138, 196];
  return [0, 0, 0];
}

/** Colour for a mineral-map cell (`255` = barren). */
export function mineralColour(value: number, minerals: readonly MineralInfo[]): RGB {
  const id = Math.round(value);
  if (id === 255 || id < 0) return [46, 52, 62];
  const entry = minerals[id];
  return entry?.colour ?? [128, 128, 128];
}

/** Legend for the categorical mineral map (all catalogue entries). */
export function mineralLegend(
  minerals: readonly MineralInfo[],
  text: (key: string) => string,
): LegendSpec {
  return {
    kind: "list",
    title: text("minerals"),
    unit: "",
    items: [
      ...minerals.map((mineral) => ({
        label: mineral.label,
        colour: mineral.colour,
      })),
      { label: text("none"), colour: [46, 52, 62] },
    ],
  };
}

/** Legend description for the current overlay mode. */
export interface LegendSpec {
  kind: "ramp" | "list";
  title: string;
  unit: string;
  stops?: Ramp;
  min?: number;
  max?: number;
  items?: Array<{ label: string; colour: RGB }>;
}

/** Builds the legend description for a mode. */
export function legendFor(
  mode: ColorMode,
  biomes: readonly BiomeInfo[],
  rocks: readonly RockInfo[],
  text: (key: string) => string,
  minerals: readonly MineralInfo[] = [],
): LegendSpec {
  if (mode === "minerals") {
    return mineralLegend(minerals, text);
  }
  if (mode === "mineral_deposits") {
    return {
      kind: "list",
      title: text("mineral_deposits"),
      unit: "",
      items: [
        { label: text("deposit"), colour: [226, 186, 80] },
        { label: text("none"), colour: [46, 52, 62] },
      ],
    };
  }
  if (mode === "terrain") {
    return {
      kind: "ramp",
      title: text("elevation"),
      unit: "m",
      stops: [
        [0, OCEAN_STOPS[3]?.[1] ?? [10, 26, 62]],
        [0.35, OCEAN_STOPS[1]?.[1] ?? [38, 96, 138]],
        [0.5, LAND_STOPS[0]?.[1] ?? [204, 192, 150]],
        [0.7, LAND_STOPS[2]?.[1] ?? [98, 148, 82]],
        [0.9, LAND_STOPS[5]?.[1] ?? [172, 158, 148]],
        [1, [242, 246, 250]],
      ],
      min: -6000,
      max: 4000,
    };
  }
  if (mode === "biome" || mode === "rock") {
    const items =
      mode === "biome"
        ? biomes.map((biome) => ({ label: biome.label, colour: biome.colour }))
        : rocks.map((rock) => ({ label: rock.label, colour: rock.colour }));
    return {
      kind: "list",
      title: mode === "biome" ? text("biomes") : text("bedrock"),
      unit: "",
      items,
    };
  }
  if (mode === "river") {
    return {
      kind: "list",
      title: text("rivers"),
      unit: "",
      items: [
        { label: text("major_river"), colour: [96, 176, 224] },
        { label: text("channel"), colour: [70, 138, 196] },
      ],
    };
  }
  const ramp = MODE_RAMPS[mode];
  return {
    kind: "ramp",
    title: ramp.title,
    unit: ramp.unit,
    stops: ramp.stops,
    min: ramp.min,
    max: ramp.max,
  };
}
