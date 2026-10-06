import type { EngineStats, ParamValues } from "./types";

/** One slider/select row bound to a flat param key. */
export interface ParamDef {
  key: string;
  label: string;
  min: number;
  max: number;
  step: number;
  def: number;
  unit?: string;
  log?: boolean;
}

export interface ParamGroup {
  title: string;
  params: ParamDef[];
}

/** Spec-exact ranges: the UI cannot send what the engine would reject. */
export const PARAM_GROUPS: ParamGroup[] = [
  {
    title: "World",
    params: [
      { key: "seed", label: "Seed", min: 1, max: 999999, step: 1, def: 42 },
      { key: "width", label: "Width", min: 16, max: 1024, step: 16, def: 256 },
      { key: "height", label: "Height", min: 16, max: 1024, step: 16, def: 256 },
      { key: "spatial_km", label: "Extent", min: 1, max: 10000, step: 10, def: 200, unit: "km" },
      { key: "vertical_m", label: "Relief", min: 100, max: 50000, step: 100, def: 5000, unit: "m" },
      { key: "sea_level_m", label: "Sea level", min: -5000, max: 5000, step: 50, def: 0, unit: "m" },
    ],
  },
  {
    title: "Structure",
    params: [
      { key: "octaves", label: "Octaves", min: 1, max: 16, step: 1, def: 6 },
      { key: "lacunarity", label: "Lacunarity", min: 1, max: 4, step: 0.05, def: 2 },
      { key: "persistence", label: "Persistence", min: 0.1, max: 1, step: 0.01, def: 0.5 },
      { key: "anisotropy", label: "Anisotropy", min: 0, max: 1, step: 0.01, def: 0 },
      { key: "anisotropy_deg", label: "Strike", min: 0, max: 360, step: 1, def: 0, unit: "°" },
      { key: "fault_offset_m", label: "Fault offset", min: 0, max: 1000, step: 10, def: 200, unit: "m" },
      { key: "fold_wavelength_km", label: "Fold wavelength", min: 1, max: 500, step: 1, def: 60, unit: "km" },
      { key: "fold_amplitude_m", label: "Fold amplitude", min: 0, max: 5000, step: 50, def: 800, unit: "m" },
      { key: "plates", label: "Plates", min: 1, max: 32, step: 1, def: 8 },
      { key: "uplift_mm_yr", label: "Uplift", min: -10, max: 50, step: 0.5, def: 1, unit: "mm/yr" },
      { key: "hotspots", label: "Hotspots", min: 0, max: 50, step: 1, def: 5 },
      { key: "hotspot_strength_m", label: "Plume rise", min: 0, max: 5000, step: 50, def: 2000, unit: "m" },
    ],
  },
  {
    title: "Erosion",
    params: [
      { key: "rain_mm", label: "Rainfall", min: 0, max: 10000, step: 50, def: 1200, unit: "mm/yr" },
      { key: "rain_shadow", label: "Rain shadow", min: 0, max: 1, step: 0.01, def: 0.7 },
      { key: "kf", label: "Fluvial K", min: 1e-8, max: 1e-3, step: 1e-6, def: 3e-6, log: true },
      { key: "slope_exp", label: "Slope exponent", min: 0.2, max: 0.7, step: 0.01, def: 0.5 },
      { key: "diffusivity", label: "Diffusivity", min: 1e-6, max: 1e-1, step: 1e-6, def: 0.01, log: true, unit: "m²/yr" },
      { key: "repose_deg", label: "Repose angle", min: 10, max: 70, step: 1, def: 35, unit: "°" },
      { key: "wind_speed", label: "Wind", min: 0, max: 50, step: 0.5, def: 8, unit: "m/s" },
      { key: "mean_temp_c", label: "Mean temp", min: -50, max: 50, step: 1, def: 8, unit: "°C" },
      { key: "ela_m", label: "Snow line", min: 0, max: 8000, step: 50, def: 2500, unit: "m" },
      { key: "wave_k", label: "Wave erosion", min: 0, max: 1, step: 0.01, def: 0.3 },
    ],
  },
  {
    title: "Time",
    params: [
      { key: "total_yr", label: "Total time", min: 1000, max: 5e8, step: 1000, def: 1e6, unit: "yr", log: true },
      { key: "step_yr", label: "Step", min: 1, max: 100000, step: 100, def: 1000, unit: "yr", log: true },
    ],
  },
];

export function defaultParams(): ParamValues {
  const out: ParamValues = { noise_kind: "perlin", fractal: "fbm", transport_law: "sp", boundary: "rectangular" };
  for (const group of PARAM_GROUPS) {
    for (const p of group.params) out[p.key] = p.def;
  }
  return out;
}

export function formatValue(def: ParamDef, value: number): string {
  if (def.log && value !== 0) {
    const exp = Math.floor(Math.log10(Math.abs(value)));
    const mant = value / 10 ** exp;
    return `${mant.toFixed(1)}e${exp}`;
  }
  const text = value >= 100 ? value.toFixed(0) : value >= 1 ? value.toFixed(2) : value.toPrecision(2);
  return def.unit !== undefined ? `${text} ${def.unit}` : text;
}

/** Renders the stats table; flags physically suspicious values. */
export function renderStats(container: HTMLElement, stats: EngineStats): void {
  const suspectLand = stats.land_fraction < 0.01 || stats.land_fraction > 0.99;
  const rows: Array<[string, string, string]> = [
    ["Elevation", `${stats.min.toFixed(0)} … ${stats.max.toFixed(0)} m`, ""],
    ["Mean ± std", `${stats.mean.toFixed(0)} ± ${stats.std.toFixed(0)} m`, ""],
    ["Median (p10–p90)", `${stats.median.toFixed(0)} (${stats.p10.toFixed(0)}–${stats.p90.toFixed(0)})`, ""],
    ["Slope mean / max", `${stats.slope_mean.toFixed(1)}° / ${stats.slope_max.toFixed(1)}°`, stats.slope_max > 80 ? "stat-bad" : ""],
    ["Ruggedness", `${stats.ruggedness.toFixed(1)} m`, ""],
    ["Land fraction", `${(stats.land_fraction * 100).toFixed(1)}%`, suspectLand ? "stat-warn" : ""],
    ["Hypsometric integral", stats.hypsometric_integral.toFixed(3), ""],
    ["Elapsed", `${stats.elapsed_yr.toFixed(0)} yr`, ""],
  ];
  const table = document.createElement("table");
  for (const [label, value, cls] of rows) {
    const tr = document.createElement("tr");
    const a = document.createElement("td");
    a.textContent = label;
    const b = document.createElement("td");
    b.textContent = value;
    if (cls !== "") b.className = cls;
    tr.append(a, b);
    table.append(tr);
  }
  container.replaceChildren(table);
}
